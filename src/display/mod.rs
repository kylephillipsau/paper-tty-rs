//! Display abstraction layer
//!
//! This module provides a high-level interface to the e-ink display,
//! wrapping the IT8951 driver with convenience methods for terminal rendering.

use std::time::{Duration, Instant};

use it8951::{Area, DisplayMode, Framebuffer, IT8951Builder, PixelFormat};

use crate::config::DisplayConfig;
use crate::error::{Error, Result};

/// Viewport defines the content area within the display (after margins)
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    /// Offset from left edge of display
    pub x: u16,
    /// Offset from top edge of display
    pub y: u16,
    /// Width of content area
    pub width: u16,
    /// Height of content area
    pub height: u16,
}

impl Viewport {
    /// Create a new viewport with the given margins
    pub fn with_margins(display_width: u16, display_height: u16, margins: (u16, u16, u16, u16)) -> Self {
        let (left, right, top, bottom) = margins;
        Self {
            x: left,
            y: top,
            width: display_width.saturating_sub(left + right),
            height: display_height.saturating_sub(top + bottom),
        }
    }

    /// Create a full-screen viewport (no margins)
    pub fn full(display_width: u16, display_height: u16) -> Self {
        Self {
            x: 0,
            y: 0,
            width: display_width,
            height: display_height,
        }
    }

    /// Translate a content-relative area to display coordinates
    pub fn translate_area(&self, area: &Area) -> Area {
        Area::new(
            self.x + area.x,
            self.y + area.y,
            area.width,
            area.height,
        )
    }
}

/// Maximum SPI clock in the IT8951 datasheet.
const SPI_SPEC_HZ: u32 = 24_000_000;
/// Rows of the image buffer used by the start-up SPI read-back test (~60 KB).
const SPI_VERIFY_ROWS: u16 = 50;

/// E-ink display wrapper for terminal rendering
pub struct EinkDisplay {
    device: it8951::IT8951<
        it8951::hal::linux::LinuxSpi,
        it8951::hal::linux::LinuxInputPin,
        it8951::hal::linux::NoOpOutputPin,
        it8951::hal::linux::LinuxOutputPin,
    >,
    framebuffer: Framebuffer,
    width: u16,
    height: u16,
    viewport: Viewport,
    partial_refresh_count: u32,
    full_refresh_interval: u32,
}

impl EinkDisplay {
    /// Create a new e-ink display from configuration
    pub fn new(config: DisplayConfig) -> Result<Self> {
        // Build the IT8951 driver
        let mut device = IT8951Builder::new()
            .vcom(config.vcom)
            .spi_data_hz(config.spi_hz)
            .build_with_spi(&config.spi_device)
            .map_err(|e| Error::Display(format!("Failed to create IT8951 device: {}", e)))?;

        // Initialize the device
        device.init().map_err(|e| {
            Error::Display(format!("Failed to initialize IT8951: {}", e))
        })?;

        // The IT8951 is specified to 24 MHz. Anything above that is verified by loading a
        // pattern and reading it back; on any mismatch fall back to the specified clock.
        if config.spi_hz > SPI_SPEC_HZ {
            let ok = device
                .verify_spi_integrity(SPI_VERIFY_ROWS)
                .map_err(|e| Error::Display(format!("SPI self-test failed: {}", e)))?;
            if ok {
                log::info!("SPI data clock {} MHz verified (read-back OK)", config.spi_hz / 1_000_000);
            } else {
                log::warn!(
                    "SPI data clock {} MHz FAILED read-back verification; falling back to 24 MHz",
                    config.spi_hz / 1_000_000
                );
                device.set_spi_speeds(1_000_000, SPI_SPEC_HZ);
            }
        }

        let width = device.width();
        let height = device.height();

        // Create framebuffer matching display size
        let framebuffer = device.create_framebuffer().map_err(|e| {
            Error::Display(format!("Failed to create framebuffer: {}", e))
        })?;

        log::info!(
            "Display initialized: {}x{} (firmware: {})",
            width,
            height,
            device.device_info().map(|i| i.fw_version.as_str()).unwrap_or("unknown")
        );

        Ok(Self {
            device,
            framebuffer,
            width,
            height,
            viewport: Viewport::full(width, height),
            partial_refresh_count: 0,
            full_refresh_interval: 0, // Disabled by default - full refresh is very slow
        })
    }

    /// Set the viewport margins (left, right, top, bottom)
    ///
    /// The viewport defines the content area where rendering occurs.
    /// Coordinates passed to rendering methods are relative to the viewport.
    pub fn set_margins(&mut self, margins: (u16, u16, u16, u16)) {
        self.viewport = Viewport::with_margins(self.width, self.height, margins);
        log::info!(
            "Viewport set: {}x{} at ({}, {})",
            self.viewport.width, self.viewport.height,
            self.viewport.x, self.viewport.y
        );
    }

    /// Get the viewport (content area dimensions)
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Get content area width (viewport width)
    pub fn content_width(&self) -> u16 {
        self.viewport.width
    }

    /// Get content area height (viewport height)
    pub fn content_height(&self) -> u16 {
        self.viewport.height
    }

    /// Get display width
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Get display height
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Get mutable access to the raw framebuffer (display coordinates)
    pub fn framebuffer_raw(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    /// Get immutable access to the raw framebuffer
    pub fn framebuffer_raw_ref(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// Set a pixel in content coordinates (relative to viewport)
    pub fn set_pixel(&mut self, x: u16, y: u16, value: u8) -> std::result::Result<(), ()> {
        let display_x = self.viewport.x + x;
        let display_y = self.viewport.y + y;
        if display_x < self.width && display_y < self.height {
            self.framebuffer.set_pixel(display_x, display_y, value).map_err(|_| ())
        } else {
            Err(())
        }
    }

    /// Get a pixel in content coordinates (relative to viewport)
    pub fn get_pixel(&self, x: u16, y: u16) -> std::result::Result<u8, ()> {
        let display_x = self.viewport.x + x;
        let display_y = self.viewport.y + y;
        if display_x < self.width && display_y < self.height {
            self.framebuffer.get_pixel(display_x, display_y).map_err(|_| ())
        } else {
            Err(())
        }
    }

    /// Clear the display to white
    pub fn clear(&mut self) -> Result<()> {
        self.device.clear(0xFF)?;
        self.framebuffer.clear(0xFF);
        self.partial_refresh_count = 0;
        Ok(())
    }

    /// Clear the display to a specific grayscale value
    pub fn clear_with(&mut self, gray: u8) -> Result<()> {
        self.device.clear(gray)?;
        self.framebuffer.clear(gray);
        self.partial_refresh_count = 0;
        Ok(())
    }

    /// Perform a full display update
    pub fn update_full(&mut self, mode: DisplayMode) -> Result<()> {
        self.device.draw_framebuffer_full(&self.framebuffer, mode)?;
        self.partial_refresh_count = 0;
        Ok(())
    }

    /// Perform a partial display update for the specified area (content coordinates)
    pub fn update_partial(&mut self, area: &Area, mode: DisplayMode) -> Result<()> {
        // Check if we should do a full refresh instead
        self.partial_refresh_count += 1;
        if self.full_refresh_interval > 0
            && self.partial_refresh_count >= self.full_refresh_interval
        {
            log::debug!("Full refresh triggered after {} partial updates", self.partial_refresh_count);
            return self.update_full(DisplayMode::Gc16);
        }

        // Translate from content coordinates to display coordinates
        let display_area = self.viewport.translate_area(area);

        // Extract the sub-region from the framebuffer
        let sub_fb = self.extract_region(&display_area)?;
        self.device.draw_framebuffer(&sub_fb, &display_area, true, mode)?;
        Ok(())
    }

    /// Update multiple areas efficiently (areas are in content coordinates)
    pub fn update_areas(&mut self, areas: &[Area], mode: DisplayMode) -> Result<()> {
        if areas.is_empty() {
            return Ok(());
        }

        // Translate areas from content coordinates to display coordinates
        let display_areas: Vec<Area> = areas.iter()
            .map(|a| self.viewport.translate_area(a))
            .collect();

        // Always merge into a single bounding box — one SPI transfer, one refresh
        let merged = Self::merge_areas(&display_areas);
        log::debug!("Partial update: {} areas merged to {}x{} at ({},{})",
            display_areas.len(), merged.width, merged.height, merged.x, merged.y);
        let sub_fb = self.extract_region(&merged)?;
        self.device.draw_framebuffer(&sub_fb, &merged, true, mode)?;

        self.partial_refresh_count += 1;
        Ok(())
    }

    /// Send a region straight from a caller-owned 8bpp content buffer.
    ///
    /// `area` is in content (viewport) coordinates; `src` is row-major with
    /// `src_stride` bytes per row and covers the whole content area. The region is
    /// packed to `format` in one pass (no framebuffer round trip) and refreshed with
    /// `mode`. The IT8951 blocks on HRDY if a previous update is still running, so
    /// call [`wait_idle`](Self::wait_idle) first when you want to capture fresh data
    /// right before sending.
    pub fn update_region_from(
        &mut self,
        area: &Area,
        src: &[u8],
        src_stride: usize,
        format: PixelFormat,
        mode: DisplayMode,
    ) -> Result<()> {
        let display_area = self.viewport.translate_area(area);
        let packed = pack_region(src, src_stride, area, format);
        self.device.load_image(&packed, &display_area, format)?;
        self.device.refresh_area(&display_area, mode)?;
        self.partial_refresh_count += 1;
        Ok(())
    }

    /// Block until the panel has finished its current waveform.
    ///
    /// The LUT-busy register can read idle for a moment right after a refresh
    /// command is accepted, so `guard` (measured from `sent_at`) is honoured before
    /// polling. Polls sleep between reads instead of spinning.
    pub fn wait_idle(&mut self, sent_at: Option<Instant>, guard: Duration) -> Result<()> {
        if let Some(t) = sent_at {
            let elapsed = t.elapsed();
            if elapsed < guard {
                std::thread::sleep(guard - elapsed);
            }
        }
        while !self.is_ready()? {
            std::thread::sleep(Duration::from_millis(3));
        }
        Ok(())
    }

    /// Extract a rectangular region from the framebuffer
    fn extract_region(&self, area: &Area) -> Result<Framebuffer> {
        let mut sub_fb = Framebuffer::new(area.width, area.height);
        let src = self.framebuffer.data();
        let dst = sub_fb.data_mut();
        let src_stride = self.width as usize;
        let w = area.width as usize;

        for y in 0..area.height as usize {
            let src_off = (area.y as usize + y) * src_stride + area.x as usize;
            let dst_off = y * w;
            dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
        }

        Ok(sub_fb)
    }

    /// Set the full refresh interval (0 = never auto-refresh)
    pub fn set_full_refresh_interval(&mut self, interval: u32) {
        self.full_refresh_interval = interval;
    }

    /// Force the next update to be a full refresh
    pub fn force_full_refresh(&mut self) {
        self.partial_refresh_count = self.full_refresh_interval;
    }

    /// Wait for the display to finish updating
    pub fn wait_ready(&mut self) -> Result<()> {
        self.device.wait_display_ready()?;
        Ok(())
    }

    /// Check if the display is ready for a new update (non-blocking)
    ///
    /// Returns `true` if the display has finished the previous update and is
    /// ready to accept new image data. This enables pipelining: capture the
    /// next frame while waiting for the current display update to complete.
    pub fn is_ready(&mut self) -> Result<bool> {
        Ok(self.device.is_display_ready()?)
    }

    /// Put the display into standby mode
    pub fn standby(&mut self) -> Result<()> {
        self.device.standby()?;
        Ok(())
    }

    /// Wake the display from standby
    pub fn wake(&mut self) -> Result<()> {
        self.device.run()?;
        Ok(())
    }

    /// Put the display into sleep mode (lowest power)
    pub fn sleep(&mut self) -> Result<()> {
        self.device.sleep()?;
        Ok(())
    }

    /// Merge multiple areas into their bounding box
    fn merge_areas(areas: &[Area]) -> Area {
        if areas.is_empty() {
            return Area::new(0, 0, 0, 0);
        }

        let mut min_x = areas[0].x;
        let mut min_y = areas[0].y;
        let mut max_x = areas[0].x + areas[0].width;
        let mut max_y = areas[0].y + areas[0].height;

        for area in areas.iter().skip(1) {
            min_x = min_x.min(area.x);
            min_y = min_y.min(area.y);
            max_x = max_x.max(area.x + area.width);
            max_y = max_y.max(area.y + area.height);
        }

        Area::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

/// Pack an 8bpp region of `src` into `format` for the IT8951.
///
/// Sub-byte formats put the first pixel in the low bits of each byte, matching the
/// little-endian word layout the 8bpp path already uses (first pixel = low byte).
/// The IT8951 needs `area.width` to be a multiple of the pixels per byte-pair
/// (2 for 8bpp, 4 for 4bpp, 8 for 2bpp); callers align regions accordingly.
pub fn pack_region(src: &[u8], src_stride: usize, area: &Area, format: PixelFormat) -> Vec<u8> {
    let (x, y, w, h) = (area.x as usize, area.y as usize, area.width as usize, area.height as usize);
    match format {
        PixelFormat::Bpp8 => {
            let mut out = Vec::with_capacity(w * h);
            for row in 0..h {
                let off = (y + row) * src_stride + x;
                out.extend_from_slice(&src[off..off + w]);
            }
            out
        }
        PixelFormat::Bpp4 => {
            let row_bytes = (w + 1) / 2;
            let mut out = Vec::with_capacity(row_bytes * h);
            for row in 0..h {
                let line = &src[(y + row) * src_stride + x..][..w];
                for pair in line.chunks(2) {
                    let lo = pair[0] >> 4;
                    let hi = pair.get(1).map(|p| p >> 4).unwrap_or(0xF);
                    out.push((hi << 4) | lo);
                }
            }
            out
        }
        PixelFormat::Bpp2 => {
            let row_bytes = (w + 3) / 4;
            let mut out = Vec::with_capacity(row_bytes * h);
            for row in 0..h {
                let line = &src[(y + row) * src_stride + x..][..w];
                for quad in line.chunks(4) {
                    let mut b = 0u8;
                    for (i, p) in quad.iter().enumerate() {
                        b |= (p >> 6) << (2 * i);
                    }
                    for i in quad.len()..4 {
                        b |= 0x3 << (2 * i);
                    }
                    out.push(b);
                }
            }
            out
        }
        PixelFormat::Bpp3 => {
            // Not a useful format for this pipeline; fall back to 8bpp packing.
            pack_region(src, src_stride, area, PixelFormat::Bpp8)
        }
    }
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    #[test]
    fn pack_8bpp_extracts_rows() {
        let src: Vec<u8> = (0..16).collect(); // 4x4
        let out = pack_region(&src, 4, &Area::new(1, 1, 2, 2), PixelFormat::Bpp8);
        assert_eq!(out, vec![5, 6, 9, 10]);
    }

    #[test]
    fn pack_4bpp_first_pixel_in_low_nibble() {
        let src = [0x10, 0x20, 0x30, 0x40];
        let out = pack_region(&src, 4, &Area::new(0, 0, 4, 1), PixelFormat::Bpp4);
        assert_eq!(out, vec![0x21, 0x43]);
    }

    #[test]
    fn pack_2bpp_first_pixel_in_low_bits() {
        let src = [0x00, 0x40, 0x80, 0xC0]; // levels 0,1,2,3
        let out = pack_region(&src, 4, &Area::new(0, 0, 4, 1), PixelFormat::Bpp2);
        assert_eq!(out, vec![0b11_10_01_00]);
    }
}

/// Mock display for testing without hardware
#[cfg(test)]
pub struct MockDisplay {
    framebuffer: Framebuffer,
    width: u16,
    height: u16,
}

#[cfg(test)]
impl MockDisplay {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
            width,
            height,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn framebuffer(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    pub fn clear(&mut self) {
        self.framebuffer.clear(0xFF);
    }
}
