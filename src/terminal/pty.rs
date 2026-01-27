//! PTY-based terminal emulator
//!
//! This module provides a full terminal emulator using a pseudo-terminal (PTY)
//! with custom dimensions. Unlike VCSA which reads from the system console,
//! this creates its own terminal session that can be any size.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex, mpsc};
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
    /// Whether cursor is visible (DECTCEM)
    cursor_visible: bool,
    /// Alternate screen buffer (for fullscreen apps like vim, htop)
    alt_buffer: Option<ScreenBuffer>,
    /// Saved main screen state when switching to alt buffer
    saved_main_cursor: Option<(u16, u16)>,
    /// Whether we're currently on the alternate screen
    on_alt_screen: bool,
    /// Scroll region top (inclusive, 0-based)
    scroll_top: u16,
    /// Scroll region bottom (inclusive, 0-based)
    scroll_bottom: u16,
    /// Channel to send responses back to the PTY (e.g. DSR replies)
    response_tx: mpsc::Sender<Vec<u8>>,
}

impl TerminalState {
    fn new(cols: u16, rows: u16, response_tx: mpsc::Sender<Vec<u8>>) -> Self {
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
            cursor_visible: true,
            alt_buffer: None,
            saved_main_cursor: None,
            on_alt_screen: false,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            response_tx,
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
            if self.cursor_row > self.scroll_bottom {
                self.scroll_up();
                self.cursor_row = self.scroll_bottom;
            }
        }
    }

    /// Scroll the scroll region up by one line
    fn scroll_up(&mut self) {
        let cols = self.buffer.cols as usize;
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;

        let region_start = top * cols;
        let region_end = (bottom + 1) * cols;

        // Shift rows up within the scroll region
        self.buffer.cells.copy_within(region_start + cols..region_end, region_start);

        // Clear bottom row of the region
        let last_row_start = bottom * cols;
        for cell in &mut self.buffer.cells[last_row_start..last_row_start + cols] {
            *cell = Cell::default();
        }

        self.buffer.scroll_count += 1;
    }

    /// Scroll the scroll region down by one line
    fn scroll_down(&mut self) {
        let cols = self.buffer.cols as usize;
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;

        let region_start = top * cols;
        let region_end = (bottom + 1) * cols;

        // Shift rows down within the scroll region
        self.buffer.cells.copy_within(region_start..region_end - cols, region_start + cols);

        // Clear top row of the region
        for cell in &mut self.buffer.cells[region_start..region_start + cols] {
            *cell = Cell::default();
        }
    }

    /// Insert n lines at the cursor row, scrolling down within the scroll region
    fn insert_lines(&mut self, n: u16) {
        let cols = self.buffer.cols as usize;
        let row = self.cursor_row as usize;
        let bottom = self.scroll_bottom as usize;

        if row > bottom {
            return;
        }

        for _ in 0..n {
            // Shift rows from cursor to bottom-1 down by one
            let src_start = row * cols;
            let src_end = bottom * cols;
            if src_end > src_start {
                self.buffer.cells.copy_within(src_start..src_end, src_start + cols);
            }
            // Clear the inserted row
            for cell in &mut self.buffer.cells[src_start..src_start + cols] {
                *cell = Cell::default();
            }
        }
    }

    /// Delete n lines at the cursor row, scrolling up within the scroll region
    fn delete_lines(&mut self, n: u16) {
        let cols = self.buffer.cols as usize;
        let row = self.cursor_row as usize;
        let bottom = self.scroll_bottom as usize;

        if row > bottom {
            return;
        }

        for _ in 0..n {
            let src_start = (row + 1) * cols;
            let region_end = (bottom + 1) * cols;
            if src_start < region_end {
                self.buffer.cells.copy_within(src_start..region_end, row * cols);
            }
            // Clear the bottom row
            let last_start = bottom * cols;
            for cell in &mut self.buffer.cells[last_start..last_start + cols] {
                *cell = Cell::default();
            }
        }
    }

    /// Switch to alternate screen buffer
    fn enter_alt_screen(&mut self) {
        if self.on_alt_screen {
            return;
        }
        self.saved_main_cursor = Some((self.cursor_col, self.cursor_row));
        self.alt_buffer = Some(self.buffer.clone());
        self.buffer.clear();
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.on_alt_screen = true;
    }

    /// Switch back to main screen buffer
    fn leave_alt_screen(&mut self) {
        if !self.on_alt_screen {
            return;
        }
        if let Some(main_buf) = self.alt_buffer.take() {
            self.buffer = main_buf;
        }
        if let Some((col, row)) = self.saved_main_cursor.take() {
            self.cursor_col = col;
            self.cursor_row = row;
        }
        self.on_alt_screen = false;
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

    /// Map a 256-color index to the nearest ANSI 0-15 color
    fn map_256_to_ansi(color: u16) -> u8 {
        if color < 16 {
            // Standard and bright colors map directly
            color as u8
        } else if color < 232 {
            // 6x6x6 color cube (indices 16-231)
            let idx = color - 16;
            let r = idx / 36;
            let g = (idx % 36) / 6;
            let b = idx % 6;
            // Map to nearest ANSI using luminance
            let lum = r * 2 + g * 4 + b;
            if lum < 4 { 0 }       // black
            else if lum < 12 { 8 }  // bright black (dark gray)
            else if lum < 20 { 7 }  // white (light gray)
            else { 15 }             // bright white
        } else {
            // Grayscale ramp (indices 232-255)
            let level = color - 232; // 0-23
            if level < 6 { 0 }
            else if level < 12 { 8 }
            else if level < 18 { 7 }
            else { 15 }
        }
    }

    /// Map RGB values (0-255 each) to the nearest ANSI 0-15 color
    fn map_rgb_to_ansi(r: u16, g: u16, b: u16) -> u8 {
        let lum = (r * 30 + g * 59 + b * 11) / 100;
        if lum < 32 { 0 }
        else if lum < 96 { 8 }
        else if lum < 192 { 7 }
        else { 15 }
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
                if self.cursor_row == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_row < self.buffer.rows - 1 {
                    self.cursor_row += 1;
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

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // Handle DEC private modes (CSI ? Pn h/l)
        if intermediates == [b'?'] {
            let mode = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
            match (action, mode) {
                ('h', 25) => self.cursor_visible = true,   // DECTCEM: show cursor
                ('l', 25) => self.cursor_visible = false,   // DECTCEM: hide cursor
                ('h', 1049) => self.enter_alt_screen(),     // Alt screen buffer on
                ('l', 1049) => self.leave_alt_screen(),     // Alt screen buffer off
                ('h', 47) | ('h', 1047) => self.enter_alt_screen(),
                ('l', 47) | ('l', 1047) => self.leave_alt_screen(),
                _ => {}
            }
            return;
        }

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

                // Collect all sub-params for extended color handling
                let param_list: Vec<Vec<u16>> = params.iter()
                    .map(|p| p.iter().map(|&v| v as u16).collect())
                    .collect();

                let mut i = 0;
                while i < param_list.len() {
                    let code = param_list[i].first().copied().unwrap_or(0);
                    match code {
                        0 => self.reset_attributes(),
                        1 => self.current_bold = true,
                        2 => {} // dim - not supported in our color model
                        3 => {} // italic - not supported
                        4 => self.current_underline = true,
                        7 => self.current_inverse = true,
                        22 => self.current_bold = false,
                        23 => {} // italic off
                        24 => self.current_underline = false,
                        27 => self.current_inverse = false,
                        30..=37 => self.current_fg = (code - 30) as u8,
                        38 => {
                            // Extended foreground: 38;5;n (256-color) or 38;2;r;g;b (RGB)
                            if i + 1 < param_list.len() {
                                let mode = param_list[i + 1].first().copied().unwrap_or(0);
                                if mode == 5 && i + 2 < param_list.len() {
                                    // 256-color: map to nearest ANSI
                                    let color = param_list[i + 2].first().copied().unwrap_or(0);
                                    self.current_fg = Self::map_256_to_ansi(color);
                                    i += 2;
                                } else if mode == 2 && i + 4 < param_list.len() {
                                    // RGB: map to nearest grayscale
                                    let r = param_list[i + 2].first().copied().unwrap_or(0);
                                    let g = param_list[i + 3].first().copied().unwrap_or(0);
                                    let b = param_list[i + 4].first().copied().unwrap_or(0);
                                    self.current_fg = Self::map_rgb_to_ansi(r, g, b);
                                    i += 4;
                                }
                            }
                        }
                        39 => self.current_fg = 7, // Default foreground
                        40..=47 => self.current_bg = (code - 40) as u8,
                        48 => {
                            // Extended background: 48;5;n or 48;2;r;g;b
                            if i + 1 < param_list.len() {
                                let mode = param_list[i + 1].first().copied().unwrap_or(0);
                                if mode == 5 && i + 2 < param_list.len() {
                                    let color = param_list[i + 2].first().copied().unwrap_or(0);
                                    self.current_bg = Self::map_256_to_ansi(color);
                                    i += 2;
                                } else if mode == 2 && i + 4 < param_list.len() {
                                    let r = param_list[i + 2].first().copied().unwrap_or(0);
                                    let g = param_list[i + 3].first().copied().unwrap_or(0);
                                    let b = param_list[i + 4].first().copied().unwrap_or(0);
                                    self.current_bg = Self::map_rgb_to_ansi(r, g, b);
                                    i += 4;
                                }
                            }
                        }
                        49 => self.current_bg = 0, // Default background
                        90..=97 => self.current_fg = (code - 90 + 8) as u8, // Bright foreground
                        100..=107 => self.current_bg = (code - 100 + 8) as u8, // Bright background
                        _ => {}
                    }
                    i += 1;
                }
            }
            // Insert Characters
            '@' => {
                let n = if first == 0 { 1 } else { first } as usize;
                let cols = self.buffer.cols as usize;
                let row = self.cursor_row as usize;
                let col = self.cursor_col as usize;
                let start = row * cols + col;
                let end = (row + 1) * cols;
                // Shift right
                if start + n < end {
                    self.buffer.cells.copy_within(start..end - n, start + n);
                }
                for i in start..(start + n).min(end) {
                    self.buffer.cells[i] = Cell::default();
                }
            }
            // Delete Characters
            'P' => {
                let n = if first == 0 { 1 } else { first } as usize;
                let cols = self.buffer.cols as usize;
                let row = self.cursor_row as usize;
                let col = self.cursor_col as usize;
                let start = row * cols + col;
                let end = (row + 1) * cols;
                if start + n < end {
                    self.buffer.cells.copy_within(start + n..end, start);
                }
                for i in (end - n).min(end)..end {
                    self.buffer.cells[i] = Cell::default();
                }
            }
            // Erase Characters
            'X' => {
                let n = if first == 0 { 1 } else { first };
                for i in 0..n {
                    let col = self.cursor_col + i;
                    if col < self.buffer.cols {
                        self.buffer.set(col, self.cursor_row, Cell::default());
                    }
                }
            }
            // Insert Lines
            'L' => {
                let n = if first == 0 { 1 } else { first };
                self.insert_lines(n);
            }
            // Delete Lines
            'M' => {
                let n = if first == 0 { 1 } else { first };
                self.delete_lines(n);
            }
            // Device Status Report
            'n' => {
                if first == 6 {
                    // Cursor Position Report: ESC [ row ; col R (1-based)
                    let response = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    let _ = self.response_tx.send(response.into_bytes());
                }
            }
            // Set Scrolling Region (DECSTBM)
            'r' => {
                let top = if first == 0 { 1 } else { first };
                let bottom = if second == 0 { self.buffer.rows } else { second };
                self.scroll_top = (top - 1).min(self.buffer.rows - 1);
                self.scroll_bottom = (bottom - 1).min(self.buffer.rows - 1);
                // Move cursor to home after setting scroll region
                self.cursor_col = 0;
                self.cursor_row = 0;
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
            // Scroll Up (SU)
            'S' => {
                let n = if first == 0 { 1 } else { first };
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            // Scroll Down (SD)
            'T' => {
                let n = if first == 0 { 1 } else { first };
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // ESC 7 - Save Cursor (DECSC)
            b'7' => {
                self.saved_cursor = Some((self.cursor_col, self.cursor_row));
            }
            // ESC 8 - Restore Cursor (DECRC)
            b'8' => {
                if let Some((col, row)) = self.saved_cursor {
                    self.cursor_col = col;
                    self.cursor_row = row;
                }
            }
            // ESC M - Reverse Index (scroll down if at top of scroll region)
            b'M' => {
                if self.cursor_row == self.scroll_top {
                    self.scroll_down();
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                }
            }
            // ESC D - Index (scroll up if at bottom of scroll region)
            b'D' => {
                if self.cursor_row == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_row < self.buffer.rows - 1 {
                    self.cursor_row += 1;
                }
            }
            // ESC E - Next Line
            b'E' => {
                self.cursor_col = 0;
                if self.cursor_row == self.scroll_bottom {
                    self.scroll_up();
                } else if self.cursor_row < self.buffer.rows - 1 {
                    self.cursor_row += 1;
                }
            }
            // ESC c - Full Reset (RIS)
            b'c' => {
                let rows = self.buffer.rows;
                let cols = self.buffer.cols;
                self.buffer.clear();
                self.cursor_col = 0;
                self.cursor_row = 0;
                self.reset_attributes();
                self.scroll_top = 0;
                self.scroll_bottom = rows - 1;
                self.cursor_visible = true;
                self.saved_cursor = None;
                let _ = (cols, rows); // suppress warnings
            }
            _ => {}
        }
    }
}

/// Build the command to spawn on the PTY.
///
/// If `shell` is provided, spawns that command directly.
/// Otherwise spawns `login` so the user sees a standard Linux login prompt.
fn build_pty_command(shell: Option<&str>) -> CommandBuilder {
    let mut cmd = if let Some(shell_cmd) = shell {
        let mut c = CommandBuilder::new(shell_cmd);
        c.args(["-l", "-i"]);
        c
    } else {
        // Use `login` for a proper console login experience
        CommandBuilder::new("login")
    };
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd
}

/// PTY-based terminal reader
///
/// When no explicit shell is specified, spawns `login` to present a standard
/// Linux login prompt. When the user logs out, `login` is respawned so
/// another user can log in — just like a real console getty.
pub struct PtyReader {
    state: Arc<Mutex<TerminalState>>,
    _reader_thread: thread::JoinHandle<()>,
    _response_thread: thread::JoinHandle<()>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    cols: u16,
    rows: u16,
}

impl PtyReader {
    /// Create a new PTY terminal with the specified dimensions.
    ///
    /// If `shell` is `None`, spawns `login` for a console login experience.
    /// When the session ends (logout / shell exit), the process is respawned
    /// automatically.
    pub fn new(cols: u16, rows: u16, shell: Option<&str>) -> Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Terminal(format!("Failed to create PTY: {}", e)))?;

        // Spawn initial process
        let cmd = build_pty_command(shell);
        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Terminal(format!("Failed to spawn login: {}", e)))?;

        // Get reader and writer
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Terminal(format!("Failed to clone PTY reader: {}", e)))?;
        let writer: Box<dyn Write + Send> = pair
            .master
            .take_writer()
            .map_err(|e| Error::Terminal(format!("Failed to take PTY writer: {}", e)))?;
        let writer = Arc::new(Mutex::new(writer));

        // Create response channel for DSR replies
        let (response_tx, response_rx) = mpsc::channel::<Vec<u8>>();

        // Spawn response thread to send DSR replies back to PTY
        let writer_for_responses = Arc::clone(&writer);
        let response_thread = thread::spawn(move || {
            while let Ok(data) = response_rx.recv() {
                if let Ok(mut w) = writer_for_responses.lock() {
                    let _ = w.write_all(&data);
                    let _ = w.flush();
                }
            }
        });

        // Create shared terminal state
        let state = Arc::new(Mutex::new(TerminalState::new(cols, rows, response_tx)));
        let state_clone = Arc::clone(&state);

        // Keep slave handle and shell config for respawning after logout
        let slave = pair.slave;
        let shell_owned = shell.map(String::from);

        // Spawn reader thread — reads PTY output, feeds VTE parser,
        // and respawns login on EOF (user logged out)
        let reader_thread = thread::spawn(move || {
            let mut parser = Parser::new();
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF — session ended (user logged out)
                        log::info!("Session ended, respawning login");

                        // Clear screen for fresh login prompt
                        {
                            let mut state = state_clone.lock().unwrap();
                            state.clear_screen();
                            state.move_cursor(0, 0);
                            state.reset_attributes();
                        }

                        // Respawn
                        let cmd = build_pty_command(shell_owned.as_deref());
                        match slave.spawn_command(cmd) {
                            Ok(_child) => {
                                log::info!("Login respawned");
                                // Continue reading — same PTY master, new child
                            }
                            Err(e) => {
                                log::error!("Failed to respawn login: {}", e);
                                break;
                            }
                        }
                    }
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
            _response_thread: response_thread,
            writer,
            cols,
            rows,
        })
    }

    /// Send input to the terminal
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock()
            .map_err(|e| Error::Terminal(format!("Failed to lock PTY writer: {}", e)))?;
        writer
            .write_all(data)
            .map_err(|e| Error::Terminal(format!("Failed to write to PTY: {}", e)))?;
        writer
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
        buffer.cursor_pos = if state.cursor_visible {
            Some((state.cursor_col, state.cursor_row))
        } else {
            None
        };
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
