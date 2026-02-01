//! Wayland screencopy client using wlr-screencopy-unstable-v1.
//!
//! Connects to a Wayland compositor (e.g. Sway), captures frames via
//! the wlr-screencopy protocol, converts to grayscale, and pushes to
//! the e-ink display.
//!
//! Input handling is delegated to the compositor via seatd/libseat.
//! This module is display-only.

use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use crate::display::EinkDisplay;
use crate::error::{Error, Result};

use super::convert::xrgb8888_to_gray;
use super::shm::ShmBuffer;

/// Configuration for the screencopy capture loop.
pub struct CaptureConfig {
    /// Minimum interval between frame captures (ms).
    pub frame_interval: Duration,
    /// Display mode string (e.g. "du", "gc16").
    pub display_mode: it8951::DisplayMode,
    /// Full refresh interval (0 = never).
    pub full_refresh_interval: u32,
    /// Minimum row gap to split into separate regions (0 = single bounding box).
    pub region_gap: usize,
}

/// A dirty region representing a contiguous band of changed rows.
#[derive(Debug, Clone)]
struct DirtyRegion {
    y_start: usize,
    y_end: usize,
    min_x: usize,
    max_x: usize,
}

impl DirtyRegion {
    /// Create an aligned Area suitable for IT8951 (4-pixel width alignment).
    fn to_aligned_area(&self, content_w: usize) -> it8951::Area {
        let aligned_x = self.min_x & !3;
        let aligned_w = ((self.max_x + 4) & !3) - aligned_x;
        let aligned_w = aligned_w.min(content_w - aligned_x);
        it8951::Area::new(
            aligned_x as u16,
            self.y_start as u16,
            aligned_w as u16,
            (self.y_end - self.y_start + 1) as u16,
        )
    }

    /// Calculate pixel count for this region.
    fn pixel_count(&self) -> usize {
        (self.max_x - self.min_x + 1) * (self.y_end - self.y_start + 1)
    }
}

/// Find dirty regions by clustering consecutive changed rows.
///
/// When there's a gap of `gap_threshold` or more unchanged rows between
/// changed rows, the regions are split into separate entries.
fn find_dirty_regions(
    row_changed: &[bool],
    row_min_x: &[usize],
    row_max_x: &[usize],
    gap_threshold: usize,
) -> Vec<DirtyRegion> {
    let mut regions = Vec::new();
    let mut current: Option<DirtyRegion> = None;
    let mut gap_count = 0;

    for y in 0..row_changed.len() {
        if row_changed[y] {
            gap_count = 0;
            if let Some(ref mut region) = current {
                region.y_end = y;
                region.min_x = region.min_x.min(row_min_x[y]);
                region.max_x = region.max_x.max(row_max_x[y]);
            } else {
                current = Some(DirtyRegion {
                    y_start: y,
                    y_end: y,
                    min_x: row_min_x[y],
                    max_x: row_max_x[y],
                });
            }
        } else if current.is_some() {
            gap_count += 1;
            if gap_threshold > 0 && gap_count >= gap_threshold {
                // Gap too large, finalize current region
                if let Some(region) = current.take() {
                    regions.push(region);
                }
                gap_count = 0;
            }
        }
    }

    // Don't forget trailing region
    if let Some(region) = current {
        regions.push(region);
    }

    regions
}

/// Internal state for the Wayland event loop.
pub struct State {
    // Globals
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    screencopy_manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,

    // Frame state
    buffer: Option<ShmBuffer>,
    frame_width: u32,
    frame_height: u32,
    frame_stride: u32,
    frame_format: Option<wl_shm::Format>,
    frame_ready: bool,
    frame_failed: bool,

    // Damage tracking
    damage_x: u32,
    damage_y: u32,
    damage_w: u32,
    damage_h: u32,
    has_damage: bool,

    // Format info
    format_is_bgr: bool,

    // Control
    running: bool,
}

impl State {
    fn new() -> Self {
        Self {
            shm: None,
            output: None,
            screencopy_manager: None,
            buffer: None,
            frame_width: 0,
            frame_height: 0,
            frame_stride: 0,
            frame_format: None,
            frame_ready: false,
            frame_failed: false,
            damage_x: 0,
            damage_y: 0,
            damage_w: 0,
            damage_h: 0,
            has_damage: false,
            format_is_bgr: false,
            running: true,
        }
    }

    fn reset_frame(&mut self) {
        self.frame_ready = false;
        self.frame_failed = false;
        self.frame_format = None;
        self.has_damage = false;
        self.damage_x = 0;
        self.damage_y = 0;
        self.damage_w = 0;
        self.damage_h = 0;
    }

    /// Accumulate damage into bounding box.
    fn add_damage(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if !self.has_damage {
            self.damage_x = x;
            self.damage_y = y;
            self.damage_w = w;
            self.damage_h = h;
            self.has_damage = true;
        } else {
            let x2 = self.damage_x + self.damage_w;
            let y2 = self.damage_y + self.damage_h;
            let nx = self.damage_x.min(x);
            let ny = self.damage_y.min(y);
            self.damage_x = nx;
            self.damage_y = ny;
            self.damage_w = x2.max(x + w) - nx;
            self.damage_h = y2.max(y + h) - ny;
        }
    }
}

// --- Wayland dispatch implementations ---

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_output" => {
                    if state.output.is_none() {
                        state.output = Some(registry.bind(name, version.min(4), qh, ()));
                    }
                }
                "zwlr_screencopy_manager_v1" => {
                    state.screencopy_manager =
                        Some(registry.bind(name, version.min(3), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _state: &mut Self,
        _shm: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // We only need XRGB8888 which is mandatory, no need to track formats.
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        _state: &mut Self,
        _output: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // We don't need output info beyond having a reference.
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _mgr: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _event: zwlr_screencopy_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    wayland_client::WEnum::Value(f) => f,
                    _ => return,
                };
                // Accept XRGB8888 or XBGR8888 (same layout, R/B swapped)
                let is_bgr = match format {
                    wl_shm::Format::Xrgb8888 => false,
                    wl_shm::Format::Xbgr8888 => true,
                    _ => {
                        debug!("Skipping unsupported pixel format: {:?}", format);
                        return;
                    }
                };
                // Only accept the first supported format per frame
                // (copy() must only be called once)
                if state.frame_format.is_some() {
                    return;
                }
                state.frame_width = width;
                state.frame_height = height;
                state.frame_stride = stride;
                state.frame_format = Some(format);
                state.format_is_bgr = is_bgr;

                // Allocate SHM buffer if needed (or if size changed)
                let needs_alloc = state.buffer.is_none()
                    || state.buffer.as_ref().map(|b| b.data().len())
                        != Some((stride * height) as usize);

                if needs_alloc {
                    if let Some(ref shm) = state.shm {
                        match ShmBuffer::new(shm, qh, width, height, format) {
                            Ok(buf) => {
                                state.buffer = Some(buf);
                            }
                            Err(e) => {
                                error!("Failed to create SHM buffer: {}", e);
                                state.frame_failed = true;
                                return;
                            }
                        }
                    }
                }

                // Copy into the buffer
                if let Some(ref buf) = state.buffer {
                    frame.copy(buf.wl_buffer());
                }
            }
            zwlr_screencopy_frame_v1::Event::Damage { x, y, width, height } => {
                state.add_damage(x as u32, y as u32, width as u32, height as u32);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                if !state.has_damage {
                    // No damage reported means full frame changed
                    state.add_damage(0, 0, state.frame_width, state.frame_height);
                }
                state.frame_ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                error!("Screencopy frame failed");
                state.frame_failed = true;
            }
            _ => {}
        }
    }
}

// No-op dispatchers for pool and buffer objects
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);

/// Run the screencopy capture loop.
///
/// This connects to the Wayland compositor, captures frames, converts them
/// to grayscale, and pushes them to the e-ink display.
///
/// The loop is pipelined: frame capture happens in parallel with display updates.
/// While the IT8951 is driving pixels for frame N, we're already capturing frame N+1.
///
/// Input handling is delegated to Sway via seatd/libseat - this module is display-only.
pub fn run_capture_loop(display: &mut EinkDisplay, config: CaptureConfig) -> Result<()> {
    // Connect to Wayland
    let conn = Connection::connect_to_env()
        .map_err(|e| Error::Wayland(format!("Failed to connect: {}", e)))?;

    let display_wl = conn.display();

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = State::new();

    // Get registry and do initial roundtrip to bind globals
    let _registry = display_wl.get_registry(&qh, ());
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| Error::Wayland(format!("Roundtrip failed: {}", e)))?;

    // Verify we have all required globals
    let output = state
        .output
        .as_ref()
        .ok_or_else(|| Error::Wayland("No wl_output found".into()))?
        .clone();
    let screencopy_mgr = state
        .screencopy_manager
        .as_ref()
        .ok_or_else(|| Error::Wayland("No zwlr_screencopy_manager_v1 — is this Sway/wlroots?".into()))?
        .clone();

    if state.shm.is_none() {
        return Err(Error::Wayland("No wl_shm found".into()));
    }

    info!("Wayland connection established, starting pipelined capture loop");
    info!("Input handled by compositor via seatd/libseat");

    let viewport = display.viewport().clone();
    let content_w = viewport.width as usize;
    let content_h = viewport.height as usize;
    let disp_w = display.width() as usize;
    let mut gray_buf = vec![0u8; content_w * content_h];
    let mut prev_buf = vec![0u8; content_w * content_h];
    let mut frame_count = 0u64;
    let mut updates_sent = 0u64;
    let mut updates_skipped = 0u64;

    // Track whether display is currently updating (refresh command sent but LUT not finished)
    let mut display_busy = false;
    let mut last_update_start: Option<Instant> = None;

    if config.full_refresh_interval > 0 {
        display.set_full_refresh_interval(config.full_refresh_interval);
    }

    // Adaptive mode: DU for tiny changes (cursor), GC16 for larger changes
    const DU_MAX_PIXELS: usize = 3000; // ~55x55 pixels

    // Track DU region for delayed GC16 cleanup
    let mut du_region: Option<it8951::Area> = None;
    let mut last_du_time: Option<Instant> = None;
    const DU_CLEANUP_DELAY: Duration = Duration::from_secs(1);

    // After N GL16 cleanups, do a full GC16 to clear accumulated artifacts
    let mut gl16_cleanup_count: u32 = 0;
    const GL16_CLEANUPS_BEFORE_FULL: u32 = 10;

    loop {
        let frame_start = Instant::now();

        // Request a new frame capture - this happens in parallel with display update
        state.reset_frame();
        let _frame = screencopy_mgr.capture_output(1, &output, &qh, ());

        // Block until frame is ready or failed
        while !state.frame_ready && !state.frame_failed && state.running {
            event_queue
                .blocking_dispatch(&mut state)
                .map_err(|e| Error::Wayland(format!("Dispatch failed: {}", e)))?;
        }

        if state.frame_failed || !state.running {
            if !state.running {
                break;
            }
            warn!("Frame {} failed, retrying", frame_count);
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        frame_count += 1;

        // Convert XRGB8888 → grayscale
        let src_w = state.frame_width as usize;
        let src_h = state.frame_height as usize;
        let src_stride = state.frame_stride as usize;

        if let Some(ref buf) = state.buffer {
            let src_data = buf.data();
            let bgr = state.format_is_bgr;

            // Scale source to content area size (nearest-neighbor)
            if src_w == content_w && src_h == content_h {
                xrgb8888_to_gray(src_data, src_stride, &mut gray_buf, content_w, content_h, bgr);
            } else {
                for dy in 0..content_h {
                    let sy = dy * src_h / content_h;
                    let src_row = &src_data[sy * src_stride..];
                    let dst_row = &mut gray_buf[dy * content_w..dy * content_w + content_w];
                    for dx in 0..content_w {
                        let sx = dx * src_w / content_w;
                        let off = sx * 4;
                        let (r, g, b) = if bgr {
                            (src_row[off] as u32, src_row[off + 1] as u32, src_row[off + 2] as u32)
                        } else {
                            (src_row[off + 2] as u32, src_row[off + 1] as u32, src_row[off] as u32)
                        };
                        dst_row[dx] = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
                    }
                }
            }
        }

        // Debug logging
        if frame_count <= 5 || frame_count % 100 == 0 {
            let sum: u64 = gray_buf.iter().map(|&b| b as u64).sum();
            debug!("Frame {}: gray checksum = {}, updates sent = {}, skipped = {}",
                   frame_count, sum, updates_sent, updates_skipped);
        }

        // Compare with previous frame to find actual changed regions
        // Track changes per row to enable disjoint region detection
        let mut row_changed = vec![false; content_h];
        let mut row_min_x = vec![content_w; content_h];
        let mut row_max_x = vec![0usize; content_h];

        for y in 0..content_h {
            let row_off = y * content_w;
            for x in 0..content_w {
                if gray_buf[row_off + x] != prev_buf[row_off + x] {
                    row_changed[y] = true;
                    row_min_x[y] = row_min_x[y].min(x);
                    row_max_x[y] = row_max_x[y].max(x);
                }
            }
        }

        // Find disjoint dirty regions using row clustering
        let dirty_regions = find_dirty_regions(
            &row_changed,
            &row_min_x,
            &row_max_x,
            config.region_gap,
        );

        // Compute overall bounding box for change ratio calculation
        let (min_x, min_y, max_x, max_y) = if dirty_regions.is_empty() {
            (content_w, content_h, 0, 0)
        } else {
            let min_x = dirty_regions.iter().map(|r| r.min_x).min().unwrap_or(content_w);
            let min_y = dirty_regions.first().map(|r| r.y_start).unwrap_or(content_h);
            let max_x = dirty_regions.iter().map(|r| r.max_x).max().unwrap_or(0);
            let max_y = dirty_regions.last().map(|r| r.y_end).unwrap_or(0);
            (min_x, min_y, max_x, max_y)
        };

        // Copy new frame into previous buffer and display framebuffer
        prev_buf.copy_from_slice(&gray_buf);
        {
            let fb = display.framebuffer_raw();
            let fb_data = fb.data_mut();
            let vp_x = viewport.x as usize;
            let vp_y = viewport.y as usize;
            for y in 0..content_h {
                let src_off = y * content_w;
                let dst_off = (vp_y + y) * disp_w + vp_x;
                fb_data[dst_off..dst_off + content_w]
                    .copy_from_slice(&gray_buf[src_off..src_off + content_w]);
            }
        }

        if dirty_regions.is_empty() {
            // No changes - check if we need to clean up DU region
            if let (Some(area), Some(last_time)) = (du_region.as_ref(), last_du_time) {
                if last_time.elapsed() >= DU_CLEANUP_DELAY {
                    // Check display is ready
                    let ready = !display_busy || display.is_ready().unwrap_or(false);
                    if ready {
                        gl16_cleanup_count += 1;

                        if gl16_cleanup_count >= GL16_CLEANUPS_BEFORE_FULL {
                            // Periodic full GC16 to clear accumulated artifacts
                            debug!("Frame {}: full GC16 cleanup after {} GL16 refreshes",
                                   frame_count, gl16_cleanup_count);
                            display.update_full(it8951::DisplayMode::Gc16)?;
                            gl16_cleanup_count = 0;
                        } else {
                            // Normal GL16 partial cleanup
                            debug!("Frame {}: DU cleanup - GL16 for {}x{} at ({},{}) [{}/{}]",
                                   frame_count, area.width, area.height, area.x, area.y,
                                   gl16_cleanup_count, GL16_CLEANUPS_BEFORE_FULL);
                            display.update_partial(area, it8951::DisplayMode::Gl16)?;
                        }

                        display_busy = true;
                        last_update_start = Some(Instant::now());
                        updates_sent += 1;
                        du_region = None;
                        last_du_time = None;
                    }
                }
            }
        } else {
            let change_w = max_x - min_x + 1;
            let change_h = max_y - min_y + 1;
            let total_pixels = content_w * content_h;
            // Use actual changed pixels from regions (more accurate than bounding box)
            let actual_changed_pixels: usize = dirty_regions.iter().map(|r| r.pixel_count()).sum();
            let change_ratio = actual_changed_pixels as f32 / total_pixels as f32;

            // Check if display is ready for a new update
            if display_busy {
                match display.is_ready() {
                    Ok(true) => {
                        if let Some(start) = last_update_start {
                            debug!("Display ready after {:?}", start.elapsed());
                        }
                        // Display is ready, proceed to send update below
                    }
                    Ok(false) => {
                        // Display still busy - skip this frame to maintain responsiveness.
                        // The next frame will include accumulated changes.
                        updates_skipped += 1;
                        debug!(
                            "Frame {}: display busy, skipping update ({:.0}% changed)",
                            frame_count, change_ratio * 100.0
                        );

                        // Rate limit even when skipping
                        let elapsed = frame_start.elapsed();
                        if elapsed < config.frame_interval {
                            std::thread::sleep(config.frame_interval - elapsed);
                        }
                        continue;
                    }
                    Err(e) => {
                        warn!("Failed to check display ready state: {}, proceeding anyway", e);
                        // Assume display is ready and try to update
                    }
                }
            }

            // Send update to display
            updates_sent += 1;
            last_update_start = Some(Instant::now());

            // Adaptive mode selection:
            // - Very small changes (<3000 pixels, ~cursor/char): DU for speed
            // - Everything else: GC16 for quality grayscale
            let is_tiny_change = actual_changed_pixels < DU_MAX_PIXELS && dirty_regions.len() == 1;
            let update_mode = if is_tiny_change {
                it8951::DisplayMode::Du // Fast, no flash for cursor
            } else {
                it8951::DisplayMode::Gc16 // Quality for everything else
            };

            if frame_count == 1 {
                display.update_full(it8951::DisplayMode::Gc16)?;
                du_region = None;
                last_du_time = None;
                gl16_cleanup_count = 0;
                debug!("Frame {}: initial full GC16 update", frame_count);
            } else if change_ratio > 0.4 {
                display.update_full(it8951::DisplayMode::Gc16)?;
                du_region = None;
                last_du_time = None;
                gl16_cleanup_count = 0;
                debug!("Frame {}: full GC16 update ({:.0}% changed)", frame_count, change_ratio * 100.0);
            } else if dirty_regions.len() == 1 {
                // Single region - use DU for tiny, GC16 for larger
                let area = dirty_regions[0].to_aligned_area(content_w);
                display.update_partial(&area, update_mode)?;

                if is_tiny_change {
                    // Track DU region for later cleanup
                    du_region = Some(match du_region {
                        Some(existing) => existing.union(&area),
                        None => area,
                    });
                    last_du_time = Some(Instant::now());
                } else {
                    // GC16 clears DU debt
                    du_region = None;
                    last_du_time = None;
                }

                debug!(
                    "Frame {}: partial {:?} {}x{} at ({},{}) ({} px)",
                    frame_count, update_mode, change_w, change_h, min_x, min_y, actual_changed_pixels
                );
            } else {
                // Multiple disjoint regions - GC16 for each
                let region_summary: Vec<String> = dirty_regions.iter()
                    .map(|r| format!("rows {}-{}", r.y_start, r.y_end))
                    .collect();
                debug!(
                    "Frame {}: {} regions GC16 [{}] ({:.1}% changed)",
                    frame_count, dirty_regions.len(), region_summary.join(" + "), change_ratio * 100.0
                );

                for region in &dirty_regions {
                    let area = region.to_aligned_area(content_w);
                    display.update_partial(&area, it8951::DisplayMode::Gc16)?;
                }
                du_region = None;
                last_du_time = None;
            }

            // Mark display as busy - the IT8951 is now driving pixels asynchronously
            display_busy = true;
        }

        // Rate limit - can be shorter now since capture overlaps with display
        let elapsed = frame_start.elapsed();
        if elapsed < config.frame_interval {
            std::thread::sleep(config.frame_interval - elapsed);
        }
    }

    info!("Capture loop ended after {} frames ({} updates sent, {} skipped)",
          frame_count, updates_sent, updates_skipped);
    Ok(())
}
