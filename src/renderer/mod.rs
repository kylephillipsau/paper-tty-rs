//! Display rendering for terminal content
//!
//! This module handles converting terminal screen buffers into framebuffer
//! pixel data, including font rendering, color mapping, and cursor display.

use crate::config::ColorConfig;
use crate::display::EinkDisplay;
use crate::error::{Error, Result};
use crate::font::{FontMetrics, FontRenderer, TtfFont};
use crate::terminal::ScreenBuffer;

/// Cursor rendering style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    /// Solid block cursor
    Block,
    /// Underline cursor
    Underline,
    /// Vertical bar cursor
    Bar,
    /// No cursor displayed
    None,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self::Block
    }
}

impl std::str::FromStr for CursorStyle {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "block" => Ok(Self::Block),
            "underline" => Ok(Self::Underline),
            "bar" => Ok(Self::Bar),
            "none" => Ok(Self::None),
            _ => Err(Error::InvalidParameter(format!("Unknown cursor style: {}", s))),
        }
    }
}

/// A dirty rectangle indicating a changed region
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl DirtyRect {
    /// Create a new dirty rectangle
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
    }

    /// Merge two rectangles into their bounding box
    pub fn merge(&self, other: &DirtyRect) -> DirtyRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        DirtyRect::new(x, y, right - x, bottom - y)
    }

    /// Convert to IT8951 Area
    pub fn to_area(&self) -> it8951::Area {
        it8951::Area::new(self.x, self.y, self.width, self.height)
    }
}

/// Terminal text renderer
///
/// Renders terminal content to a framebuffer using logical (0,0) based coordinates.
/// The display layer is responsible for translating these coordinates to actual
/// display positions (e.g., applying viewport margins).
pub struct TextRenderer {
    font: Box<dyn FontRenderer>,
    metrics: FontMetrics,
    colors: ColorConfig,
    cursor_style: CursorStyle,
    /// Previous screen buffer for differential updates
    prev_buffer: Option<ScreenBuffer>,
}

impl TextRenderer {
    /// Create a new text renderer with the specified font
    pub fn new<F: FontRenderer + 'static>(font: F, colors: ColorConfig) -> Self {
        let metrics = font.metrics();
        Self {
            font: Box::new(font),
            metrics,
            colors,
            cursor_style: CursorStyle::default(),
            prev_buffer: None,
        }
    }

    /// Create a text renderer from a font file
    pub fn from_font_file(
        font_path: &str,
        font_size: f32,
        colors: ColorConfig,
    ) -> Result<Self> {
        let font = TtfFont::from_file(font_path, font_size)?;
        Ok(Self::new(font, colors))
    }

    /// Set the cursor style
    pub fn set_cursor_style(&mut self, style: CursorStyle) {
        self.cursor_style = style;
    }

    /// Get font metrics
    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// Calculate how many columns and rows fit in the given pixel dimensions
    pub fn calculate_dimensions(&self, width: u16, height: u16) -> (u16, u16) {
        let cols = width / self.metrics.width;
        let rows = height / self.metrics.line_height;
        (cols, rows)
    }

    /// Render the terminal buffer to the display
    ///
    /// Returns a list of dirty rectangles in content coordinates (0,0 based).
    /// The display handles translation to actual display coordinates.
    pub fn render(
        &mut self,
        buffer: &ScreenBuffer,
        display: &mut EinkDisplay,
    ) -> Vec<DirtyRect> {
        let mut dirty_rects = Vec::new();
        let mut scrolled = false;

        // Handle scroll optimization: shift framebuffer pixels instead of re-rendering
        if buffer.scroll_count > 0 {
            if let Some(ref mut prev) = self.prev_buffer {
                let scroll_lines = buffer.scroll_count as usize;
                let pixel_rows = scroll_lines * self.metrics.line_height as usize;
                let bg_gray = self.colors.ansi_to_gray(0); // default bg

                // Shift framebuffer pixels up
                let vp = display.viewport();
                let vp_x = vp.x;
                let vp_y = vp.y;
                let content_h = vp.height;
                let _ = (vp_x, vp_y, content_h); // used below
                display.framebuffer_raw().scroll_up(pixel_rows, bg_gray);

                // Shift prev_buffer cells to match
                let cols = prev.cols as usize;
                let total = prev.cells.len();
                let cell_offset = scroll_lines * cols;
                if cell_offset < total {
                    prev.cells.copy_within(cell_offset..total, 0);
                    for cell in &mut prev.cells[total - cell_offset..] {
                        *cell = crate::terminal::Cell::default();
                    }
                } else {
                    prev.cells.fill(crate::terminal::Cell::default());
                }

                scrolled = true;
            }
        }

        // Determine which cells changed
        let changed_cells = if let Some(ref prev) = self.prev_buffer {
            buffer.diff(prev)
        } else {
            // First render - all cells are "changed"
            let mut all = Vec::new();
            for row in 0..buffer.rows {
                for col in 0..buffer.cols {
                    all.push((col, row));
                }
            }
            all
        };

        // Render changed cells (using content coordinates - display handles translation)
        for (col, row) in changed_cells {
            if let Some(cell) = buffer.get(col, row) {
                let x = col * self.metrics.width;
                let y = row * self.metrics.line_height;

                // Render cell background
                let bg_gray = self.colors.ansi_to_gray(cell.bg_color);
                self.fill_cell(display, x, y, bg_gray);

                // Render character
                let fg_gray = if cell.inverse {
                    self.colors.ansi_to_gray(cell.bg_color)
                } else {
                    self.colors.ansi_to_gray(cell.fg_color)
                };

                let actual_bg = if cell.inverse {
                    self.colors.ansi_to_gray(cell.fg_color)
                } else {
                    bg_gray
                };

                if cell.inverse {
                    self.fill_cell(display, x, y, actual_bg);
                }

                self.render_char(display, x, y, cell.character, fg_gray);

                // Add to dirty rects (content coordinates)
                dirty_rects.push(DirtyRect::new(
                    x,
                    y,
                    self.metrics.width,
                    self.metrics.line_height,
                ));
            }
        }

        // Render cursor
        if let Some((cursor_col, cursor_row)) = buffer.cursor_pos {
            let cursor_x = cursor_col * self.metrics.width;
            let cursor_y = cursor_row * self.metrics.line_height;
            self.render_cursor(display, cursor_x, cursor_y);
            dirty_rects.push(DirtyRect::new(
                cursor_x,
                cursor_y,
                self.metrics.width,
                self.metrics.line_height,
            ));
        }

        // Store current buffer for next diff
        self.prev_buffer = Some(buffer.clone());

        // If we scrolled, the entire content area is dirty (pixels were shifted)
        if scrolled && !dirty_rects.is_empty() {
            return vec![DirtyRect::new(0, 0, display.content_width(), display.content_height())];
        }

        // Merge adjacent dirty rects for efficiency
        Self::merge_dirty_rects(&mut dirty_rects)
    }

    /// Fill a character cell with a solid color (content coordinates)
    fn fill_cell(&self, display: &mut EinkDisplay, x: u16, y: u16, gray: u8) {
        let vp = display.viewport();
        let display_x = vp.x + x;
        let display_y = vp.y + y;
        let w = self.metrics.width.min(display.content_width().saturating_sub(x));
        let h = self.metrics.line_height.min(display.content_height().saturating_sub(y));
        display.framebuffer_raw().fill_rect(display_x, display_y, w, h, gray);
    }

    /// Render a single character (content coordinates)
    fn render_char(&mut self, display: &mut EinkDisplay, x: u16, y: u16, c: char, fg: u8) {
        if let Some(glyph) = self.font.render_glyph(c) {
            let content_width = display.content_width();
            let content_height = display.content_height();

            // Calculate glyph position within cell
            let glyph_x = x as i32 + glyph.x_offset as i32;
            let glyph_y = y as i32 + self.metrics.baseline as i32 - glyph.height as i32 - glyph.y_offset as i32;

            // Render glyph pixels
            for gy in 0..glyph.height {
                for gx in 0..glyph.width {
                    let px = glyph_x + gx as i32;
                    let py = glyph_y + gy as i32;

                    if px >= 0 && py >= 0 {
                        let px = px as u16;
                        let py = py as u16;

                        if px < content_width && py < content_height {
                            let idx = (gy as usize) * (glyph.width as usize) + (gx as usize);
                            let alpha = glyph.data[idx];

                            if alpha > 0 {
                                // Blend with background
                                if let Ok(existing) = display.get_pixel(px, py) {
                                    let blended = Self::blend(existing, fg, alpha);
                                    let _ = display.set_pixel(px, py, blended);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Render cursor at the given position (content coordinates)
    fn render_cursor(&self, display: &mut EinkDisplay, x: u16, y: u16) {
        let content_width = display.content_width();
        let content_height = display.content_height();

        match self.cursor_style {
            CursorStyle::Block => {
                // Invert the cell
                for dy in 0..self.metrics.line_height {
                    for dx in 0..self.metrics.width {
                        let px = x + dx;
                        let py = y + dy;
                        if px < content_width && py < content_height {
                            if let Ok(existing) = display.get_pixel(px, py) {
                                let _ = display.set_pixel(px, py, 255 - existing);
                            }
                        }
                    }
                }
            }
            CursorStyle::Underline => {
                // Draw underline at bottom of cell
                let underline_y = y + self.metrics.line_height - 2;
                for dx in 0..self.metrics.width {
                    let px = x + dx;
                    if px < content_width && underline_y < content_height {
                        let _ = display.set_pixel(px, underline_y, 0);
                    }
                }
            }
            CursorStyle::Bar => {
                // Draw vertical bar at left of cell
                for dy in 0..self.metrics.line_height {
                    let py = y + dy;
                    if x < content_width && py < content_height {
                        let _ = display.set_pixel(x, py, 0);
                        if x + 1 < content_width {
                            let _ = display.set_pixel(x + 1, py, 0);
                        }
                    }
                }
            }
            CursorStyle::None => {}
        }
    }

    /// Blend foreground and background colors with alpha
    fn blend(bg: u8, fg: u8, alpha: u8) -> u8 {
        let bg = bg as u32;
        let fg = fg as u32;
        let alpha = alpha as u32;
        let result = (fg * alpha + bg * (255 - alpha)) / 255;
        result as u8
    }

    /// Merge dirty rectangles by row for efficient partial updates
    ///
    /// Terminal updates typically affect contiguous cells in a row.
    /// Merging by row reduces the number of partial update operations.
    fn merge_dirty_rects(rects: &mut Vec<DirtyRect>) -> Vec<DirtyRect> {
        if rects.is_empty() {
            return Vec::new();
        }
        if rects.len() == 1 {
            return rects.clone();
        }

        // Group rects by their Y position (row)
        // For each row, merge all rects into one spanning the full width of changes
        use std::collections::BTreeMap;
        let mut rows: BTreeMap<u16, (u16, u16, u16)> = BTreeMap::new(); // y -> (min_x, max_x, height)

        for rect in rects.iter() {
            let entry = rows.entry(rect.y).or_insert((rect.x, rect.x + rect.width, rect.height));
            entry.0 = entry.0.min(rect.x);
            entry.1 = entry.1.max(rect.x + rect.width);
        }

        // Now merge adjacent rows that have the same x span
        let mut merged = Vec::new();
        let mut current: Option<DirtyRect> = None;

        for (&y, &(min_x, max_x, height)) in &rows {
            let width = max_x - min_x;

            if let Some(ref mut curr) = current {
                // Check if this row is adjacent and has same x span
                if y == curr.y + curr.height && min_x == curr.x && width == curr.width {
                    // Extend current rect
                    curr.height += height;
                } else {
                    // Save current and start new
                    merged.push(*curr);
                    current = Some(DirtyRect::new(min_x, y, width, height));
                }
            } else {
                current = Some(DirtyRect::new(min_x, y, width, height));
            }
        }

        if let Some(curr) = current {
            merged.push(curr);
        }

        // If we still have many rects, merge into bounding box
        if merged.len() > 10 {
            let mut bbox = merged[0];
            for rect in merged.iter().skip(1) {
                bbox = bbox.merge(rect);
            }
            return vec![bbox];
        }

        merged
    }

    /// Clear the previous buffer state (forces full redraw on next render)
    pub fn invalidate(&mut self) {
        self.prev_buffer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_style_parse() {
        assert_eq!("block".parse::<CursorStyle>().unwrap(), CursorStyle::Block);
        assert_eq!("underline".parse::<CursorStyle>().unwrap(), CursorStyle::Underline);
        assert_eq!("bar".parse::<CursorStyle>().unwrap(), CursorStyle::Bar);
        assert_eq!("none".parse::<CursorStyle>().unwrap(), CursorStyle::None);
        assert!("invalid".parse::<CursorStyle>().is_err());
    }

    #[test]
    fn test_dirty_rect_merge() {
        let r1 = DirtyRect::new(0, 0, 10, 10);
        let r2 = DirtyRect::new(5, 5, 10, 10);
        let merged = r1.merge(&r2);

        assert_eq!(merged.x, 0);
        assert_eq!(merged.y, 0);
        assert_eq!(merged.width, 15);
        assert_eq!(merged.height, 15);
    }

    #[test]
    fn test_blend() {
        // Full opacity
        assert_eq!(TextRenderer::blend(255, 0, 255), 0);
        assert_eq!(TextRenderer::blend(0, 255, 255), 255);

        // No opacity
        assert_eq!(TextRenderer::blend(255, 0, 0), 255);
        assert_eq!(TextRenderer::blend(0, 255, 0), 0);

        // Half opacity
        assert_eq!(TextRenderer::blend(0, 255, 128), 128);
    }
}
