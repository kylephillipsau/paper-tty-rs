//! VCSA (Virtual Console Screen Area) reader
//!
//! The VCSA devices (/dev/vcsa1, /dev/vcsa2, etc.) provide direct access
//! to the Linux virtual console screen buffer. This is the most efficient
//! way to read terminal content.
//!
//! VCSA Format:
//! - Byte 0: Number of rows
//! - Byte 1: Number of columns
//! - Byte 2: Cursor column
//! - Byte 3: Cursor row
//! - Bytes 4+: (character, attribute) pairs for each cell
//!
//! Attribute byte format (VGA text mode):
//! - Bits 0-3: Foreground color
//! - Bits 4-6: Background color
//! - Bit 7: Blink

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::error::{Error, Result};
use crate::terminal::{Cell, ScreenBuffer, TerminalReader};

/// VCSA-based terminal reader
pub struct VcsaReader {
    file: File,
    #[allow(dead_code)]
    tty_num: u8,
    cols: u16,
    rows: u16,
}

impl VcsaReader {
    /// Create a new VCSA reader for the specified TTY number
    ///
    /// # Arguments
    /// * `tty_num` - TTY number (1 = /dev/vcsa1, 2 = /dev/vcsa2, etc.)
    ///
    /// # Example
    /// ```no_run
    /// use paper_tty::VcsaReader;
    /// let reader = VcsaReader::new(1)?; // Read from /dev/vcsa1
    /// # Ok::<(), paper_tty::Error>(())
    /// ```
    pub fn new(tty_num: u8) -> Result<Self> {
        let path = format!("/dev/vcsa{}", tty_num);
        let file = File::open(&path).map_err(|e| {
            Error::Terminal(format!("Failed to open {}: {} (try running as root)", path, e))
        })?;

        let mut reader = Self {
            file,
            tty_num,
            cols: 0,
            rows: 0,
        };

        // Read initial dimensions
        reader.update_dimensions()?;

        Ok(reader)
    }

    /// Update stored dimensions by reading from device
    fn update_dimensions(&mut self) -> Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; 4];
        self.file.read_exact(&mut header)?;

        self.rows = header[0] as u16;
        self.cols = header[1] as u16;

        Ok(())
    }

    /// Parse VGA attribute byte into cell attributes
    fn parse_attribute(attr: u8) -> (u8, u8, bool, bool) {
        let fg = attr & 0x0F;        // Bits 0-3: foreground
        let bg = (attr >> 4) & 0x07; // Bits 4-6: background
        let blink = (attr & 0x80) != 0; // Bit 7: blink
        let bold = fg > 7;           // Bright colors indicate bold

        (fg, bg, bold, blink)
    }
}

impl TerminalReader for VcsaReader {
    fn read_screen(&mut self) -> Result<ScreenBuffer> {
        // Seek to beginning and read header
        self.file.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; 4];
        self.file.read_exact(&mut header)?;

        let rows = header[0] as u16;
        let cols = header[1] as u16;
        let cursor_col = header[2] as u16;
        let cursor_row = header[3] as u16;

        // Update stored dimensions
        self.rows = rows;
        self.cols = cols;

        // Read cell data
        let cell_count = (rows as usize) * (cols as usize);
        let mut data = vec![0u8; cell_count * 2]; // 2 bytes per cell
        self.file.read_exact(&mut data)?;

        // Parse into cells
        let mut cells = Vec::with_capacity(cell_count);
        for chunk in data.chunks(2) {
            let character = chunk[0] as char;
            let attr = chunk[1];
            let (fg_color, bg_color, bold, blink) = Self::parse_attribute(attr);

            cells.push(Cell {
                character,
                fg_color,
                bg_color,
                bold,
                underline: false, // VCSA doesn't provide underline info
                inverse: false,
                blink,
            });
        }

        Ok(ScreenBuffer {
            cols,
            rows,
            cells,
            cursor_pos: Some((cursor_col, cursor_row)),
        })
    }

    fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_attribute() {
        // White on black
        let (fg, bg, bold, blink) = VcsaReader::parse_attribute(0x07);
        assert_eq!(fg, 7);
        assert_eq!(bg, 0);
        assert!(!bold);
        assert!(!blink);

        // Bright white (bold) on blue
        let (fg, bg, bold, blink) = VcsaReader::parse_attribute(0x1F);
        assert_eq!(fg, 15);
        assert_eq!(bg, 1);
        assert!(bold);
        assert!(!blink);

        // Red on black with blink
        let (fg, bg, bold, blink) = VcsaReader::parse_attribute(0x84);
        assert_eq!(fg, 4);
        assert_eq!(bg, 0);
        assert!(!bold);
        assert!(blink);
    }

    #[test]
    fn test_screen_buffer_diff() {
        let mut buf1 = ScreenBuffer::new(80, 25);
        let mut buf2 = ScreenBuffer::new(80, 25);

        // Initially identical
        assert!(buf1.diff(&buf2).is_empty());

        // Change one cell
        buf1.set(10, 5, Cell {
            character: 'X',
            ..Cell::default()
        });

        let diff = buf1.diff(&buf2);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0], (10, 5));
    }
}
