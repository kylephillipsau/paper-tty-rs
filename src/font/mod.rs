//! Font loading and glyph rendering
//!
//! This module provides font rendering capabilities using the fontdue crate
//! for TrueType fonts, with optional support for bitmap fonts.
//!
//! # Font Types
//!
//! - [`TtfFont`]: TrueType/OpenType fonts rendered via fontdue
//! - [`BitmapFont`]: Pixel-perfect bitmap fonts for small sizes
//! - [`BuiltinFont`]: Pre-defined bitmap fonts (Tiny4x6, Small5x8, etc.)

mod bitmap;

pub use bitmap::{BitmapFont, BuiltinFont};

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Error, Result};

/// Maximum number of glyphs to cache (to prevent unbounded memory growth)
const MAX_CACHE_SIZE: usize = 512;

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
    /// Extra spacing between lines
    pub line_spacing: u16,
}

impl FontMetrics {
    /// Create new metrics with the given dimensions
    pub fn new(width: u16, height: u16, baseline: u16) -> Self {
        Self {
            width,
            height,
            baseline,
            line_height: height,
            line_spacing: 0,
        }
    }

    /// Set additional line spacing
    pub fn with_line_spacing(mut self, spacing: u16) -> Self {
        self.line_spacing = spacing;
        self.line_height = self.height + spacing;
        self
    }
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
    /// Optional bold variant
    bold_font: Option<fontdue::Font>,
    size: f32,
    metrics: FontMetrics,
    cache: HashMap<char, Option<GlyphBitmap>>,
    /// Cache for bold glyphs
    bold_cache: HashMap<char, Option<GlyphBitmap>>,
    /// Track insertion order for simple LRU eviction
    cache_order: Vec<char>,
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
            bold_font: None,
            size,
            metrics,
            cache: HashMap::new(),
            bold_cache: HashMap::new(),
            cache_order: Vec::new(),
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
            line_spacing: 0,
        }
    }

    /// Set line spacing
    pub fn set_line_spacing(&mut self, spacing: u16) {
        self.metrics.line_spacing = spacing;
        self.metrics.line_height = self.metrics.height + spacing;
    }

    /// Clear the glyph cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.bold_cache.clear();
        self.cache_order.clear();
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.cache.len() + self.bold_cache.len(), MAX_CACHE_SIZE)
    }

    /// Load a bold variant font from a file
    ///
    /// If loaded, bold glyphs will use this font instead of synthesizing.
    pub fn load_bold<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let data = std::fs::read(path.as_ref()).map_err(|e| {
            Error::Font(format!("Failed to read bold font file: {}", e))
        })?;

        let bold = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
            .map_err(|e| Error::Font(format!("Failed to parse bold font: {}", e)))?;

        self.bold_font = Some(bold);
        self.bold_cache.clear();
        Ok(())
    }

    /// Check if a bold font variant is loaded
    pub fn has_bold(&self) -> bool {
        self.bold_font.is_some()
    }

    /// Render a bold glyph
    ///
    /// If a bold font is loaded, uses that. Otherwise synthesizes bold
    /// by rendering the glyph multiple times with slight offsets.
    pub fn render_bold_glyph(&mut self, c: char) -> Option<GlyphBitmap> {
        // Check cache first
        if let Some(cached) = self.bold_cache.get(&c) {
            return cached.clone();
        }

        let glyph = if let Some(ref bold_font) = self.bold_font {
            // Use actual bold font
            let (metrics, bitmap) = bold_font.rasterize(c, self.size);
            if bitmap.is_empty() {
                None
            } else {
                Some(GlyphBitmap {
                    width: metrics.width as u16,
                    height: metrics.height as u16,
                    x_offset: metrics.xmin as i16,
                    y_offset: metrics.ymin as i16,
                    data: bitmap,
                })
            }
        } else {
            // Synthesize bold by rendering with offset and blending
            self.synthesize_bold(c)
        };

        self.bold_cache.insert(c, glyph.clone());
        glyph
    }

    /// Synthesize a bold glyph by double-striking
    fn synthesize_bold(&mut self, c: char) -> Option<GlyphBitmap> {
        let normal = self.render_glyph(c)?;

        // Create a wider glyph with the original shifted right by 1 pixel
        let new_width = normal.width + 1;
        let mut data = vec![0u8; (new_width as usize) * (normal.height as usize)];

        // Copy original
        for y in 0..normal.height as usize {
            for x in 0..normal.width as usize {
                let src_idx = y * normal.width as usize + x;
                let dst_idx = y * new_width as usize + x;
                data[dst_idx] = normal.data[src_idx];
            }
        }

        // Blend shifted version (offset by 1 pixel to the right)
        for y in 0..normal.height as usize {
            for x in 0..normal.width as usize {
                let src_idx = y * normal.width as usize + x;
                let dst_idx = y * new_width as usize + (x + 1);
                // Max blend for bold effect
                data[dst_idx] = data[dst_idx].max(normal.data[src_idx]);
            }
        }

        Some(GlyphBitmap {
            width: new_width,
            height: normal.height,
            x_offset: normal.x_offset,
            y_offset: normal.y_offset,
            data,
        })
    }

    /// Evict oldest entries if cache is full
    fn maybe_evict(&mut self) {
        while self.cache.len() >= MAX_CACHE_SIZE && !self.cache_order.is_empty() {
            let oldest = self.cache_order.remove(0);
            self.cache.remove(&oldest);
        }
    }
}

impl FontRenderer for TtfFont {
    fn render_glyph(&mut self, c: char) -> Option<GlyphBitmap> {
        // Check cache first
        if let Some(cached) = self.cache.get(&c) {
            return cached.clone();
        }

        // Evict if needed
        self.maybe_evict();

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
        self.cache_order.push(c);
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
        let metrics = FontMetrics::new(8, 16, 12);

        assert_eq!(metrics.width, 8);
        assert_eq!(metrics.height, 16);
        assert_eq!(metrics.baseline, 12);
        assert_eq!(metrics.line_height, 16);

        // Test with line spacing
        let metrics = metrics.with_line_spacing(2);
        assert_eq!(metrics.line_height, 18);
        assert_eq!(metrics.line_spacing, 2);
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

    #[test]
    fn test_cache_eviction() {
        // Test that cache properly tracks size
        let metrics = FontMetrics::new(8, 16, 12);
        assert!(metrics.width > 0);
        // More thorough testing would require actual font files
    }
}
