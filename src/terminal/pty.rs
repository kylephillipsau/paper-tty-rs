//! PTY-based terminal emulator
//!
//! This module provides a full terminal emulator using a pseudo-terminal (PTY)
//! with custom dimensions. Unlike VCSA which reads from the system console,
//! this creates its own terminal session that can be any size.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use vte::{Params, Parser, Perform};

use crate::error::{Error, Result};
use crate::terminal::{Cell, ScreenBuffer, TerminalReader};

/// Terminal state managed by the VTE parser
struct TerminalState {
    /// Screen buffer
    buffer: ScreenBuffer,
    /// Current cursor position
    cursor_col: u16,
    cursor_row: u16,
    /// Current cell attributes
    current_fg: u8,
    current_bg: u8,
    current_bold: bool,
    current_underline: bool,
    current_inverse: bool,
    /// Saved cursor position (for save/restore)
    saved_cursor: Option<(u16, u16)>,
}

impl TerminalState {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            buffer: ScreenBuffer::new(cols, rows),
            cursor_col: 0,
            cursor_row: 0,
            current_fg: 7,  // Default white
            current_bg: 0,  // Default black
            current_bold: false,
            current_underline: false,
            current_inverse: false,
            saved_cursor: None,
        }
    }

    /// Get current cell with attributes
    fn current_cell(&self, c: char) -> Cell {
        Cell {
            character: c,
            fg_color: self.current_fg,
            bg_color: self.current_bg,
            bold: self.current_bold,
            underline: self.current_underline,
            inverse: self.current_inverse,
            blink: false,
        }
    }

    /// Move cursor, handling bounds
    fn move_cursor(&mut self, col: u16, row: u16) {
        self.cursor_col = col.min(self.buffer.cols.saturating_sub(1));
        self.cursor_row = row.min(self.buffer.rows.saturating_sub(1));
    }

    /// Advance cursor after printing a character
    fn advance_cursor(&mut self) {
        self.cursor_col += 1;
        if self.cursor_col >= self.buffer.cols {
            self.cursor_col = 0;
            self.cursor_row += 1;
            if self.cursor_row >= self.buffer.rows {
                self.scroll_up();
                self.cursor_row = self.buffer.rows - 1;
            }
        }
    }

    /// Scroll the screen up by one line
    fn scroll_up(&mut self) {
        let cols = self.buffer.cols as usize;
        let total = self.buffer.cells.len();

        // Shift all rows up using copy_within
        self.buffer.cells.copy_within(cols..total, 0);

        // Clear last row
        let last_row_start = total - cols;
        for cell in &mut self.buffer.cells[last_row_start..] {
            *cell = Cell::default();
        }

        self.buffer.scroll_count += 1;
    }

    /// Clear from cursor to end of line
    fn clear_to_eol(&mut self) {
        for col in self.cursor_col..self.buffer.cols {
            self.buffer.set(col, self.cursor_row, Cell::default());
        }
    }

    /// Clear from cursor to beginning of line
    fn clear_to_bol(&mut self) {
        for col in 0..=self.cursor_col {
            self.buffer.set(col, self.cursor_row, Cell::default());
        }
    }

    /// Clear entire line
    fn clear_line(&mut self) {
        for col in 0..self.buffer.cols {
            self.buffer.set(col, self.cursor_row, Cell::default());
        }
    }

    /// Clear from cursor to end of screen
    fn clear_to_eos(&mut self) {
        self.clear_to_eol();
        for row in (self.cursor_row + 1)..self.buffer.rows {
            for col in 0..self.buffer.cols {
                self.buffer.set(col, row, Cell::default());
            }
        }
    }

    /// Clear from cursor to beginning of screen
    fn clear_to_bos(&mut self) {
        self.clear_to_bol();
        for row in 0..self.cursor_row {
            for col in 0..self.buffer.cols {
                self.buffer.set(col, row, Cell::default());
            }
        }
    }

    /// Clear entire screen
    fn clear_screen(&mut self) {
        self.buffer.clear();
    }

    /// Reset all attributes to default
    fn reset_attributes(&mut self) {
        self.current_fg = 7;
        self.current_bg = 0;
        self.current_bold = false;
        self.current_underline = false;
        self.current_inverse = false;
    }
}

/// VTE Perform implementation that updates terminal state
impl Perform for TerminalState {
    fn print(&mut self, c: char) {
        let cell = self.current_cell(c);
        self.buffer.set(self.cursor_col, self.cursor_row, cell);
        self.advance_cursor();
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // Bell
            0x07 => {}
            // Backspace
            0x08 => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                }
            }
            // Tab
            0x09 => {
                let next_tab = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next_tab.min(self.buffer.cols - 1);
            }
            // Line feed / Vertical tab / Form feed
            0x0A | 0x0B | 0x0C => {
                self.cursor_row += 1;
                if self.cursor_row >= self.buffer.rows {
                    self.scroll_up();
                    self.cursor_row = self.buffer.rows - 1;
                }
            }
            // Carriage return
            0x0D => {
                self.cursor_col = 0;
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let mut params_iter = params.iter();
        let first = params_iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u16;
        let second = params_iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u16;

        match action {
            // Cursor Up
            'A' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            // Cursor Down
            'B' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_row = (self.cursor_row + n).min(self.buffer.rows - 1);
            }
            // Cursor Forward
            'C' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_col = (self.cursor_col + n).min(self.buffer.cols - 1);
            }
            // Cursor Back
            'D' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_col = self.cursor_col.saturating_sub(n);
            }
            // Cursor Next Line
            'E' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_col = 0;
                self.cursor_row = (self.cursor_row + n).min(self.buffer.rows - 1);
            }
            // Cursor Previous Line
            'F' => {
                let n = if first == 0 { 1 } else { first };
                self.cursor_col = 0;
                self.cursor_row = self.cursor_row.saturating_sub(n);
            }
            // Cursor Horizontal Absolute
            'G' => {
                let col = if first == 0 { 1 } else { first };
                self.cursor_col = (col - 1).min(self.buffer.cols - 1);
            }
            // Cursor Position (row;col)
            'H' | 'f' => {
                let row = if first == 0 { 1 } else { first };
                let col = if second == 0 { 1 } else { second };
                self.move_cursor(col - 1, row - 1);
            }
            // Erase in Display
            'J' => {
                match first {
                    0 => self.clear_to_eos(),
                    1 => self.clear_to_bos(),
                    2 | 3 => self.clear_screen(),
                    _ => {}
                }
            }
            // Erase in Line
            'K' => {
                match first {
                    0 => self.clear_to_eol(),
                    1 => self.clear_to_bol(),
                    2 => self.clear_line(),
                    _ => {}
                }
            }
            // SGR (Select Graphic Rendition)
            'm' => {
                // Handle all parameters
                if params.is_empty() {
                    self.reset_attributes();
                    return;
                }

                for param in params.iter() {
                    let code = param.first().copied().unwrap_or(0);
                    match code {
                        0 => self.reset_attributes(),
                        1 => self.current_bold = true,
                        4 => self.current_underline = true,
                        7 => self.current_inverse = true,
                        22 => self.current_bold = false,
                        24 => self.current_underline = false,
                        27 => self.current_inverse = false,
                        30..=37 => self.current_fg = (code - 30) as u8,
                        38 => {
                            // Extended foreground color (256 or RGB)
                            // TODO: Handle 256-color and RGB
                        }
                        39 => self.current_fg = 7, // Default foreground
                        40..=47 => self.current_bg = (code - 40) as u8,
                        48 => {
                            // Extended background color
                            // TODO: Handle 256-color and RGB
                        }
                        49 => self.current_bg = 0, // Default background
                        90..=97 => self.current_fg = (code - 90 + 8) as u8, // Bright foreground
                        100..=107 => self.current_bg = (code - 100 + 8) as u8, // Bright background
                        _ => {}
                    }
                }
            }
            // Save cursor position
            's' => {
                self.saved_cursor = Some((self.cursor_col, self.cursor_row));
            }
            // Restore cursor position
            'u' => {
                if let Some((col, row)) = self.saved_cursor {
                    self.cursor_col = col;
                    self.cursor_row = row;
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

/// PTY-based terminal reader
pub struct PtyReader {
    state: Arc<Mutex<TerminalState>>,
    _reader_thread: thread::JoinHandle<()>,
    writer: Box<dyn Write + Send>,
    cols: u16,
    rows: u16,
}

impl PtyReader {
    /// Create a new PTY terminal with the specified dimensions
    ///
    /// This spawns a shell in a new pseudo-terminal.
    pub fn new(cols: u16, rows: u16, shell: Option<&str>) -> Result<Self> {
        let pty_system = native_pty_system();

        // Create PTY with our custom dimensions
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Terminal(format!("Failed to create PTY: {}", e)))?;

        // Determine shell to use
        let shell_cmd = shell
            .map(String::from)
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".to_string());

        // Spawn shell
        let mut cmd = CommandBuilder::new(&shell_cmd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Terminal(format!("Failed to spawn shell: {}", e)))?;

        // Get reader and writer
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Terminal(format!("Failed to clone PTY reader: {}", e)))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::Terminal(format!("Failed to take PTY writer: {}", e)))?;

        // Create shared terminal state
        let state = Arc::new(Mutex::new(TerminalState::new(cols, rows)));
        let state_clone = Arc::clone(&state);

        // Spawn reader thread
        let reader_thread = thread::spawn(move || {
            let mut parser = Parser::new();
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let mut state = state_clone.lock().unwrap();
                        for byte in &buf[..n] {
                            parser.advance(&mut *state, *byte);
                        }
                    }
                    Err(e) => {
                        log::error!("PTY read error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            state,
            _reader_thread: reader_thread,
            writer,
            cols,
            rows,
        })
    }

    /// Send input to the terminal
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer
            .write_all(data)
            .map_err(|e| Error::Terminal(format!("Failed to write to PTY: {}", e)))?;
        self.writer
            .flush()
            .map_err(|e| Error::Terminal(format!("Failed to flush PTY: {}", e)))?;
        Ok(())
    }

    /// Send a string to the terminal
    pub fn write_str(&mut self, s: &str) -> Result<()> {
        self.write(s.as_bytes())
    }
}

impl TerminalReader for PtyReader {
    fn read_screen(&mut self) -> Result<ScreenBuffer> {
        let mut state = self.state.lock().unwrap();
        let mut buffer = state.buffer.clone();
        buffer.cursor_pos = Some((state.cursor_col, state.cursor_row));
        // Transfer scroll count and reset
        buffer.scroll_count = state.buffer.scroll_count;
        state.buffer.scroll_count = 0;
        Ok(buffer)
    }

    fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    fn write_input(&mut self, data: &[u8]) -> Result<bool> {
        self.write(data)?;
        Ok(true)
    }

    fn supports_input(&self) -> bool {
        true
    }
}
