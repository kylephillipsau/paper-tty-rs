//! Terminal reading and parsing
//!
//! This module provides interfaces for reading Linux terminal content,
//! either via the VCSA (Virtual Console Screen Area) device or directly
//! from TTY character streams.

mod vcsa;

pub use vcsa::VcsaReader;

use crate::Result;

/// A single cell in the terminal screen buffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character displayed in this cell
    pub character: char,
    /// Foreground color (ANSI color code 0-15, or extended 0-255)
    pub fg_color: u8,
    /// Background color (ANSI color code 0-15, or extended 0-255)
    pub bg_color: u8,
    /// Bold attribute
    pub bold: bool,
    /// Underline attribute
    pub underline: bool,
    /// Inverse/reverse video attribute
    pub inverse: bool,
    /// Blink attribute
    pub blink: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            fg_color: 7,  // White
            bg_color: 0,  // Black
            bold: false,
            underline: false,
            inverse: false,
            blink: false,
        }
    }
}

/// Screen buffer containing the current terminal state
#[derive(Debug, Clone)]
pub struct ScreenBuffer {
    /// Number of columns
    pub cols: u16,
    /// Number of rows
    pub rows: u16,
    /// Cell data (row-major order: cells[row * cols + col])
    pub cells: Vec<Cell>,
    /// Cursor position (col, row), if known
    pub cursor_pos: Option<(u16, u16)>,
}

impl ScreenBuffer {
    /// Create a new empty screen buffer
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = (cols as usize) * (rows as usize);
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); size],
            cursor_pos: None,
        }
    }

    /// Get a cell at the given position
    pub fn get(&self, col: u16, row: u16) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            let idx = (row as usize) * (self.cols as usize) + (col as usize);
            self.cells.get(idx)
        } else {
            None
        }
    }

    /// Get a mutable reference to a cell at the given position
    pub fn get_mut(&mut self, col: u16, row: u16) -> Option<&mut Cell> {
        if col < self.cols && row < self.rows {
            let idx = (row as usize) * (self.cols as usize) + (col as usize);
            self.cells.get_mut(idx)
        } else {
            None
        }
    }

    /// Set a cell at the given position
    pub fn set(&mut self, col: u16, row: u16, cell: Cell) {
        if col < self.cols && row < self.rows {
            let idx = (row as usize) * (self.cols as usize) + (col as usize);
            if let Some(c) = self.cells.get_mut(idx) {
                *c = cell;
            }
        }
    }

    /// Clear the screen buffer with default cells
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
    }

    /// Compare with another buffer and return changed cell positions
    pub fn diff(&self, other: &ScreenBuffer) -> Vec<(u16, u16)> {
        let mut changed = Vec::new();

        if self.cols != other.cols || self.rows != other.rows {
            // Different dimensions - everything changed
            for row in 0..self.rows {
                for col in 0..self.cols {
                    changed.push((col, row));
                }
            }
            return changed;
        }

        for (idx, (a, b)) in self.cells.iter().zip(other.cells.iter()).enumerate() {
            if a != b {
                let col = (idx % self.cols as usize) as u16;
                let row = (idx / self.cols as usize) as u16;
                changed.push((col, row));
            }
        }

        changed
    }
}

/// Trait for reading terminal screen content
pub trait TerminalReader {
    /// Read the current screen buffer
    fn read_screen(&mut self) -> Result<ScreenBuffer>;

    /// Get terminal dimensions (cols, rows)
    fn dimensions(&self) -> (u16, u16);
}
