//! Display abstraction layer
//!
//! This module provides a high-level interface to the e-ink display,
//! wrapping the IT8951 driver with convenience methods for terminal rendering.

use it8951::{Area, DisplayMode, Framebuffer, IT8951Builder};

use crate::config::DisplayConfig;
use crate::error::{Error, Result};

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
    partial_refresh_count: u32,
    full_refresh_interval: u32,
}

impl EinkDisplay {
    /// Create a new e-ink display from configuration
    pub fn new(config: DisplayConfig) -> Result<Self> {
        // Build the IT8951 driver
        let mut device = IT8951Builder::new()
            .vcom(config.vcom)
            .build()
            .map_err(|e| Error::Display(format!("Failed to create IT8951 device: {}", e)))?;

        // Initialize the device
        device.init().map_err(|e| {
            Error::Display(format!("Failed to initialize IT8951: {}", e))
        })?;

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
            partial_refresh_count: 0,
            full_refresh_interval: 0, // Disabled by default - full refresh is very slow
        })
    }

    /// Get display width
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Get display height
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Get mutable access to the framebuffer
    pub fn framebuffer(&mut self) -> &mut Framebuffer {
        &mut self.framebuffer
    }

    /// Get immutable access to the framebuffer
    pub fn framebuffer_ref(&self) -> &Framebuffer {
        &self.framebuffer
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

    /// Perform a partial display update for the specified area
    pub fn update_partial(&mut self, area: &Area, mode: DisplayMode) -> Result<()> {
        // Check if we should do a full refresh instead
        self.partial_refresh_count += 1;
        if self.full_refresh_interval > 0
            && self.partial_refresh_count >= self.full_refresh_interval
        {
            log::debug!("Full refresh triggered after {} partial updates", self.partial_refresh_count);
            return self.update_full(DisplayMode::Gc16);
        }

        // Extract the sub-region from the framebuffer
        let sub_fb = self.extract_region(area)?;
        self.device.draw_framebuffer(&sub_fb, area, true, mode)?;
        Ok(())
    }

    /// Update multiple areas efficiently
    pub fn update_areas(&mut self, areas: &[Area], mode: DisplayMode) -> Result<()> {
        if areas.is_empty() {
            return Ok(());
        }

        // If there are many areas, merge into a bounding box
        if areas.len() > 5 {
            let merged = Self::merge_areas(areas);
            log::debug!("Merging {} areas into bounding box: {}x{} at ({},{})",
                areas.len(), merged.width, merged.height, merged.x, merged.y);
            let sub_fb = self.extract_region(&merged)?;
            self.device.draw_framebuffer(&sub_fb, &merged, true, mode)?;
            self.partial_refresh_count += 1;
            return Ok(());
        }

        // Update each area individually
        for area in areas {
            log::debug!("Partial update: {}x{} at ({},{})", area.width, area.height, area.x, area.y);
            let sub_fb = self.extract_region(area)?;
            self.device.draw_framebuffer(&sub_fb, area, true, mode)?;
        }

        self.partial_refresh_count += 1;
        Ok(())
    }

    /// Extract a rectangular region from the framebuffer
    fn extract_region(&self, area: &Area) -> Result<Framebuffer> {
        let mut sub_fb = Framebuffer::new(area.width, area.height);

        for y in 0..area.height {
            for x in 0..area.width {
                let src_x = area.x + x;
                let src_y = area.y + y;

                if src_x < self.width && src_y < self.height {
                    if let Ok(pixel) = self.framebuffer.get_pixel(src_x, src_y) {
                        let _ = sub_fb.set_pixel(x, y, pixel);
                    }
                }
            }
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
