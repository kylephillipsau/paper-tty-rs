//! Font loading and glyph rendering
//!
//! This module provides font rendering capabilities using the fontdue crate
//! for TrueType fonts, with optional support for bitmap fonts.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result};

/// Font metrics for layout calculations
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    /// Character cell width in pixels
    pub width: u16,
    /// Character cell height in pixels
    pub height: u16,
    /// Baseline offset from top of cell
    pub baseline: u16,
    /// Total line height (height + line spacing)
    pub line_height: u16,
}

/// Rendered glyph bitmap
#[derive(Debug, Clone)]
pub struct GlyphBitmap {
    /// Glyph width in pixels
    pub width: u16,
    /// Glyph height in pixels
    pub height: u16,
    /// X offset within cell
    pub x_offset: i16,
    /// Y offset from baseline
    pub y_offset: i16,
    /// Grayscale pixel data (0 = transparent, 255 = opaque)
    pub data: Vec<u8>,
}

/// Trait for font rendering implementations
pub trait FontRenderer: Send {
    /// Render a glyph for the given character
    fn render_glyph(&mut self, c: char) -> Option<GlyphBitmap>;

    /// Get font metrics
    fn metrics(&self) -> FontMetrics;
}

/// TrueType font renderer using fontdue
pub struct TtfFont {
    font: fontdue::Font,
    size: f32,
    metrics: FontMetrics,
    cache: HashMap<char, Option<GlyphBitmap>>,
}

impl TtfFont {
    /// Load a TrueType font from a file
    pub fn from_file<P: AsRef<Path>>(path: P, size: f32) -> Result<Self> {
        let data = std::fs::read(path.as_ref()).map_err(|e| {
            Error::Font(format!("Failed to read font file: {}", e))
        })?;

        Self::from_bytes(&data, size)
    }

    /// Load a TrueType font from bytes
    pub fn from_bytes(data: &[u8], size: f32) -> Result<Self> {
        let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
            .map_err(|e| Error::Font(format!("Failed to parse font: {}", e)))?;

        // Calculate metrics based on a reference character
        let metrics = Self::calculate_metrics(&font, size);

        Ok(Self {
            font,
            size,
            metrics,
            cache: HashMap::new(),
        })
    }

    /// Calculate font metrics
    fn calculate_metrics(font: &fontdue::Font, size: f32) -> FontMetrics {
        // Use 'M' as reference for width (typical monospace approach)
        let (m_metrics, _) = font.rasterize('M', size);

        // Get line metrics
        let line_metrics = font.horizontal_line_metrics(size);

        let ascent = line_metrics.map(|m| m.ascent).unwrap_or(size * 0.8);
        let descent = line_metrics.map(|m| m.descent.abs()).unwrap_or(size * 0.2);

        let height = (ascent + descent).ceil() as u16;
        let width = m_metrics.advance_width.ceil() as u16;
        let baseline = ascent.ceil() as u16;

        FontMetrics {
            width: width.max(1),
            height: height.max(1),
            baseline,
            line_height: height,
        }
    }

    /// Clear the glyph cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl FontRenderer for TtfFont {
    fn render_glyph(&mut self, c: char) -> Option<GlyphBitmap> {
        // Check cache first
        if let Some(cached) = self.cache.get(&c) {
            return cached.clone();
        }

        // Rasterize the glyph
        let (metrics, bitmap) = self.font.rasterize(c, self.size);

        let glyph = if bitmap.is_empty() {
            // No visible glyph (space, control chars, etc.)
            None
        } else {
            Some(GlyphBitmap {
                width: metrics.width as u16,
                height: metrics.height as u16,
                x_offset: metrics.xmin as i16,
                y_offset: metrics.ymin as i16,
                data: bitmap,
            })
        };

        // Cache and return
        self.cache.insert(c, glyph.clone());
        glyph
    }

    fn metrics(&self) -> FontMetrics {
        self.metrics
    }
}

/// System font finder utilities
pub mod system {
    use std::path::PathBuf;

    /// Common monospace font paths to try
    const FONT_PATHS: &[&str] = &[
        // Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        // macOS
        "/System/Library/Fonts/Menlo.ttc",
        "/Library/Fonts/Courier New.ttf",
        // Generic
        "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
    ];

    /// Find a suitable monospace font on the system
    pub fn find_monospace_font() -> Option<PathBuf> {
        for path in FONT_PATHS {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_metrics() {
        // This test requires a font file, so we just test the structure
        let metrics = FontMetrics {
            width: 8,
            height: 16,
            baseline: 12,
            line_height: 16,
        };

        assert_eq!(metrics.width, 8);
        assert_eq!(metrics.height, 16);
    }

    #[test]
    fn test_glyph_bitmap() {
        let glyph = GlyphBitmap {
            width: 8,
            height: 12,
            x_offset: 0,
            y_offset: -2,
            data: vec![0; 96],
        };

        assert_eq!(glyph.data.len(), 96);
    }
}
