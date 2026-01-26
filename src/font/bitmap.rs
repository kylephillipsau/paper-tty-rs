//! Bitmap font support
//!
//! This module provides pixel-perfect bitmap fonts that are ideal for
//! small displays where TrueType fonts don't render cleanly.

use std::collections::HashMap;

use super::{FontMetrics, FontRenderer, GlyphBitmap};

/// Built-in bitmap font variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFont {
    /// Tiny 4x6 font - minimal, fits many characters
    Tiny4x6,
    /// Small 5x8 font - readable, good balance
    Small5x8,
    /// Medium 6x10 font - clearer, still compact
    Medium6x10,
}

impl BuiltinFont {
    /// Get the font data for this built-in font
    pub fn load(self) -> BitmapFont {
        match self {
            BuiltinFont::Tiny4x6 => BitmapFont::tiny_4x6(),
            BuiltinFont::Small5x8 => BitmapFont::small_5x8(),
            BuiltinFont::Medium6x10 => BitmapFont::medium_6x10(),
        }
    }
}

/// A bitmap font with fixed-width character glyphs
#[derive(Debug, Clone)]
pub struct BitmapFont {
    /// Font metrics
    metrics: FontMetrics,
    /// Glyph data indexed by character
    glyphs: HashMap<char, Vec<u8>>,
}

impl BitmapFont {
    /// Create a new bitmap font with the given dimensions
    pub fn new(width: u16, height: u16, baseline: u16) -> Self {
        Self {
            metrics: FontMetrics::new(width, height, baseline),
            glyphs: HashMap::new(),
        }
    }

    /// Add a glyph to the font
    ///
    /// The data should be a packed bitmap where each bit represents a pixel.
    /// Bits are packed MSB first, row by row.
    pub fn add_glyph(&mut self, c: char, data: Vec<u8>) {
        self.glyphs.insert(c, data);
    }

    /// Add a glyph from a pattern string (for easy definition)
    ///
    /// Pattern uses '#' or 'X' for on pixels, anything else for off.
    /// Rows are separated by newlines.
    pub fn add_glyph_pattern(&mut self, c: char, pattern: &str) {
        let mut data = Vec::new();
        let width = self.metrics.width as usize;

        for line in pattern.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Pack bits into bytes
            let mut byte = 0u8;
            let mut bit_pos = 7;

            for (i, ch) in line.chars().take(width).enumerate() {
                if ch == '#' || ch == 'X' || ch == '1' {
                    byte |= 1 << bit_pos;
                }

                if bit_pos == 0 || i == width - 1 {
                    data.push(byte);
                    byte = 0;
                    bit_pos = 7;
                } else {
                    bit_pos -= 1;
                }
            }
        }

        self.glyphs.insert(c, data);
    }

    /// Set line spacing
    pub fn set_line_spacing(&mut self, spacing: u16) {
        self.metrics.line_spacing = spacing;
        self.metrics.line_height = self.metrics.height + spacing;
    }

    /// Unpack a glyph bitmap to grayscale data
    fn unpack_glyph(&self, packed: &[u8]) -> Vec<u8> {
        let width = self.metrics.width as usize;
        let height = self.metrics.height as usize;
        let mut data = vec![0u8; width * height];

        let bytes_per_row = (width + 7) / 8;

        for y in 0..height {
            for x in 0..width {
                let byte_idx = y * bytes_per_row + x / 8;
                let bit_idx = 7 - (x % 8);

                if byte_idx < packed.len() {
                    if (packed[byte_idx] >> bit_idx) & 1 != 0 {
                        data[y * width + x] = 255; // Fully opaque
                    }
                }
            }
        }

        data
    }

    /// Create the tiny 4x6 font
    ///
    /// This is a minimal font inspired by "Tom Thumb" that fits in a 4x6 cell.
    pub fn tiny_4x6() -> Self {
        let mut font = Self::new(4, 6, 5);

        // Define ASCII printable characters (32-126)
        // Each glyph is 4 wide, 6 tall, packed into bits

        // Space
        font.add_glyph_pattern(' ', "
            ....
            ....
            ....
            ....
            ....
            ....
        ");

        // !
        font.add_glyph_pattern('!', "
            .#..
            .#..
            .#..
            ....
            .#..
            ....
        ");

        // Numbers 0-9
        font.add_glyph_pattern('0', "
            .##.
            #..#
            #..#
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('1', "
            .#..
            ##..
            .#..
            .#..
            ###.
            ....
        ");
        font.add_glyph_pattern('2', "
            .##.
            #..#
            ..#.
            .#..
            ####
            ....
        ");
        font.add_glyph_pattern('3', "
            ###.
            ...#
            .##.
            ...#
            ###.
            ....
        ");
        font.add_glyph_pattern('4', "
            #..#
            #..#
            ####
            ...#
            ...#
            ....
        ");
        font.add_glyph_pattern('5', "
            ####
            #...
            ###.
            ...#
            ###.
            ....
        ");
        font.add_glyph_pattern('6', "
            .##.
            #...
            ###.
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('7', "
            ####
            ...#
            ..#.
            .#..
            .#..
            ....
        ");
        font.add_glyph_pattern('8', "
            .##.
            #..#
            .##.
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('9', "
            .##.
            #..#
            .###
            ...#
            .##.
            ....
        ");

        // Uppercase letters
        font.add_glyph_pattern('A', "
            .##.
            #..#
            ####
            #..#
            #..#
            ....
        ");
        font.add_glyph_pattern('B', "
            ###.
            #..#
            ###.
            #..#
            ###.
            ....
        ");
        font.add_glyph_pattern('C', "
            .##.
            #...
            #...
            #...
            .##.
            ....
        ");
        font.add_glyph_pattern('D', "
            ###.
            #..#
            #..#
            #..#
            ###.
            ....
        ");
        font.add_glyph_pattern('E', "
            ####
            #...
            ###.
            #...
            ####
            ....
        ");
        font.add_glyph_pattern('F', "
            ####
            #...
            ###.
            #...
            #...
            ....
        ");
        font.add_glyph_pattern('G', "
            .##.
            #...
            #.##
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('H', "
            #..#
            #..#
            ####
            #..#
            #..#
            ....
        ");
        font.add_glyph_pattern('I', "
            ###.
            .#..
            .#..
            .#..
            ###.
            ....
        ");
        font.add_glyph_pattern('J', "
            .###
            ..#.
            ..#.
            #.#.
            .#..
            ....
        ");
        font.add_glyph_pattern('K', "
            #..#
            #.#.
            ##..
            #.#.
            #..#
            ....
        ");
        font.add_glyph_pattern('L', "
            #...
            #...
            #...
            #...
            ####
            ....
        ");
        font.add_glyph_pattern('M', "
            #..#
            ####
            #..#
            #..#
            #..#
            ....
        ");
        font.add_glyph_pattern('N', "
            #..#
            ##.#
            #.##
            #..#
            #..#
            ....
        ");
        font.add_glyph_pattern('O', "
            .##.
            #..#
            #..#
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('P', "
            ###.
            #..#
            ###.
            #...
            #...
            ....
        ");
        font.add_glyph_pattern('Q', "
            .##.
            #..#
            #..#
            #.#.
            .#.#
            ....
        ");
        font.add_glyph_pattern('R', "
            ###.
            #..#
            ###.
            #.#.
            #..#
            ....
        ");
        font.add_glyph_pattern('S', "
            .###
            #...
            .##.
            ...#
            ###.
            ....
        ");
        font.add_glyph_pattern('T', "
            ####
            .#..
            .#..
            .#..
            .#..
            ....
        ");
        font.add_glyph_pattern('U', "
            #..#
            #..#
            #..#
            #..#
            .##.
            ....
        ");
        font.add_glyph_pattern('V', "
            #..#
            #..#
            #..#
            .##.
            .##.
            ....
        ");
        font.add_glyph_pattern('W', "
            #..#
            #..#
            #..#
            ####
            #..#
            ....
        ");
        font.add_glyph_pattern('X', "
            #..#
            #..#
            .##.
            #..#
            #..#
            ....
        ");
        font.add_glyph_pattern('Y', "
            #..#
            #..#
            .##.
            .#..
            .#..
            ....
        ");
        font.add_glyph_pattern('Z', "
            ####
            ..#.
            .#..
            #...
            ####
            ....
        ");

        // Lowercase letters (same as uppercase for this tiny font)
        for c in 'a'..='z' {
            let upper = c.to_ascii_uppercase();
            if let Some(glyph) = font.glyphs.get(&upper).cloned() {
                font.glyphs.insert(c, glyph);
            }
        }

        // Common punctuation
        font.add_glyph_pattern('.', "
            ....
            ....
            ....
            ....
            .#..
            ....
        ");
        font.add_glyph_pattern(',', "
            ....
            ....
            ....
            .#..
            #...
            ....
        ");
        font.add_glyph_pattern(':', "
            ....
            .#..
            ....
            .#..
            ....
            ....
        ");
        font.add_glyph_pattern(';', "
            ....
            .#..
            ....
            .#..
            #...
            ....
        ");
        font.add_glyph_pattern('?', "
            .##.
            #..#
            ..#.
            ....
            .#..
            ....
        ");
        font.add_glyph_pattern('-', "
            ....
            ....
            ####
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('+', "
            ....
            .#..
            ###.
            .#..
            ....
            ....
        ");
        font.add_glyph_pattern('=', "
            ....
            ####
            ....
            ####
            ....
            ....
        ");
        font.add_glyph_pattern('(', "
            ..#.
            .#..
            .#..
            .#..
            ..#.
            ....
        ");
        font.add_glyph_pattern(')', "
            .#..
            ..#.
            ..#.
            ..#.
            .#..
            ....
        ");
        font.add_glyph_pattern('[', "
            .##.
            .#..
            .#..
            .#..
            .##.
            ....
        ");
        font.add_glyph_pattern(']', "
            .##.
            ..#.
            ..#.
            ..#.
            .##.
            ....
        ");
        font.add_glyph_pattern('/', "
            ...#
            ..#.
            .#..
            #...
            ....
            ....
        ");
        font.add_glyph_pattern('\\', "
            #...
            .#..
            ..#.
            ...#
            ....
            ....
        ");
        font.add_glyph_pattern('_', "
            ....
            ....
            ....
            ....
            ####
            ....
        ");
        font.add_glyph_pattern('|', "
            .#..
            .#..
            .#..
            .#..
            .#..
            ....
        ");
        font.add_glyph_pattern('@', "
            .##.
            #.##
            #.##
            #...
            .##.
            ....
        ");
        font.add_glyph_pattern('#', "
            .#.#
            ####
            .#.#
            ####
            .#.#
            ....
        ");
        font.add_glyph_pattern('$', "
            .###
            #.#.
            .##.
            .#.#
            ###.
            ....
        ");
        font.add_glyph_pattern('%', "
            #..#
            ..#.
            .#..
            #..#
            ....
            ....
        ");
        font.add_glyph_pattern('&', "
            .#..
            #.#.
            .#..
            #.#.
            .#.#
            ....
        ");
        font.add_glyph_pattern('*', "
            ....
            #.#.
            .#..
            #.#.
            ....
            ....
        ");
        font.add_glyph_pattern('"', "
            #.#.
            #.#.
            ....
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('\'', "
            .#..
            .#..
            ....
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('<', "
            ..#.
            .#..
            #...
            .#..
            ..#.
            ....
        ");
        font.add_glyph_pattern('>', "
            #...
            .#..
            ..#.
            .#..
            #...
            ....
        ");
        font.add_glyph_pattern('^', "
            .#..
            #.#.
            ....
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('`', "
            #...
            .#..
            ....
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('~', "
            ....
            .#.#
            #.#.
            ....
            ....
            ....
        ");
        font.add_glyph_pattern('{', "
            ..#.
            .#..
            #...
            .#..
            ..#.
            ....
        ");
        font.add_glyph_pattern('}', "
            #...
            .#..
            ..#.
            .#..
            #...
            ....
        ");

        font
    }

    /// Create the small 5x8 font
    pub fn small_5x8() -> Self {
        let mut font = Self::new(5, 8, 6);

        // Define basic characters - a subset for now
        font.add_glyph_pattern(' ', "
            .....
            .....
            .....
            .....
            .....
            .....
            .....
            .....
        ");

        font.add_glyph_pattern('A', "
            ..#..
            .#.#.
            #...#
            #####
            #...#
            #...#
            .....
            .....
        ");

        font.add_glyph_pattern('B', "
            ####.
            #...#
            ####.
            #...#
            #...#
            ####.
            .....
            .....
        ");

        // Add more as needed...
        // For brevity, copy uppercase to lowercase
        for c in 'A'..='Z' {
            if let Some(glyph) = font.glyphs.get(&c).cloned() {
                font.glyphs.insert(c.to_ascii_lowercase(), glyph);
            }
        }

        font
    }

    /// Create the medium 6x10 font
    pub fn medium_6x10() -> Self {
        let mut font = Self::new(6, 10, 8);

        // Define basic characters
        font.add_glyph_pattern(' ', "
            ......
            ......
            ......
            ......
            ......
            ......
            ......
            ......
            ......
            ......
        ");

        font.add_glyph_pattern('A', "
            ..##..
            .#..#.
            #....#
            #....#
            ######
            #....#
            #....#
            ......
            ......
            ......
        ");

        // Add more as needed...
        for c in 'A'..='Z' {
            if let Some(glyph) = font.glyphs.get(&c).cloned() {
                font.glyphs.insert(c.to_ascii_lowercase(), glyph);
            }
        }

        font
    }
}

impl FontRenderer for BitmapFont {
    fn render_glyph(&mut self, c: char) -> Option<GlyphBitmap> {
        let packed = self.glyphs.get(&c)?;
        let data = self.unpack_glyph(packed);

        Some(GlyphBitmap {
            width: self.metrics.width,
            height: self.metrics.height,
            x_offset: 0,
            y_offset: 0,
            data,
        })
    }

    fn metrics(&self) -> FontMetrics {
        self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_tiny_font() {
        let mut font = BuiltinFont::Tiny4x6.load();
        let metrics = font.metrics();

        assert_eq!(metrics.width, 4);
        assert_eq!(metrics.height, 6);

        // Test rendering 'A'
        let glyph = font.render_glyph('A');
        assert!(glyph.is_some());

        let glyph = glyph.unwrap();
        assert_eq!(glyph.width, 4);
        assert_eq!(glyph.height, 6);
        assert_eq!(glyph.data.len(), 24); // 4 * 6
    }

    #[test]
    fn test_glyph_pattern() {
        let mut font = BitmapFont::new(4, 4, 3);
        font.add_glyph_pattern('X', "
            #..#
            .##.
            .##.
            #..#
        ");

        let glyph = font.render_glyph('X').unwrap();

        // Check corners are set (1s)
        assert_eq!(glyph.data[0], 255);  // top-left
        assert_eq!(glyph.data[3], 255);  // top-right
        assert_eq!(glyph.data[12], 255); // bottom-left
        assert_eq!(glyph.data[15], 255); // bottom-right

        // Check center is set
        assert_eq!(glyph.data[5], 255);  // row 1, col 1
        assert_eq!(glyph.data[6], 255);  // row 1, col 2
    }
}
