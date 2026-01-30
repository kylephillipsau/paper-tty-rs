//! Wayland screencopy client using wlr-screencopy-unstable-v1.
//!
//! Connects to a Wayland compositor (e.g. Sway), captures frames via
//! the wlr-screencopy protocol, converts to grayscale, and pushes to
//! the e-ink display.

use std::time::{Duration, Instant};

use std::path::PathBuf;

use log::{debug, error, info, warn};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
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
    /// Keyboard device path (auto-detected if None).
    pub keyboard_device: Option<PathBuf>,
}

/// Internal state for the Wayland event loop.
pub struct State {
    // Globals
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    screencopy_manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    vk_manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,

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
            seat: None,
            vk_manager: None,
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
                "wl_seat" => {
                    if state.seat.is_none() {
                        state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                    }
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.vk_manager = Some(registry.bind(name, version.min(1), qh, ()));
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

// Dispatch for seat (we don't need capability events)
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
}

// No-op dispatchers for pool, buffer, and virtual keyboard objects
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);

/// Run the screencopy capture loop.
///
/// This connects to the Wayland compositor, captures frames, converts them
/// to grayscale, and pushes them to the e-ink display.
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

    // Set up virtual keyboard for input forwarding
    let mut virtual_keyboard = match (&state.vk_manager, &state.seat) {
        (Some(vk_mgr), Some(seat)) => {
            match super::input::VirtualKeyboard::new(vk_mgr, seat, &qh, config.keyboard_device) {
                Ok(vk) => {
                    // Need a roundtrip so the compositor processes the keymap
                    event_queue
                        .roundtrip(&mut state)
                        .map_err(|e| Error::Wayland(format!("Roundtrip failed: {}", e)))?;
                    info!("Virtual keyboard input forwarding enabled");
                    Some(vk)
                }
                Err(e) => {
                    warn!("Virtual keyboard not available: {}", e);
                    None
                }
            }
        }
        _ => {
            warn!("No seat or virtual keyboard manager — input forwarding disabled");
            None
        }
    };

    info!("Wayland connection established, starting capture loop");

    let viewport = display.viewport().clone();
    let content_w = viewport.width as usize;
    let content_h = viewport.height as usize;
    let disp_w = display.width() as usize;
    let disp_h = display.height() as usize;
    let mut gray_buf = vec![0u8; content_w * content_h];
    let mut prev_buf = vec![0u8; content_w * content_h];
    let mut frame_count = 0u64;

    if config.full_refresh_interval > 0 {
        display.set_full_refresh_interval(config.full_refresh_interval);
    }

    loop {
        let frame_start = Instant::now();

        // Forward keyboard input
        if let Some(ref mut vk) = virtual_keyboard {
            let forwarded = vk.poll_and_forward();
            if forwarded > 0 {
                // Flush the virtual keyboard events to the compositor
                conn.flush().ok();
            }
        }

        // Request a new frame capture
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

        // Debug: log a checksum of the grayscale buffer
        if frame_count <= 5 || frame_count % 100 == 0 {
            let sum: u64 = gray_buf.iter().map(|&b| b as u64).sum();
            debug!("Frame {}: gray checksum = {}", frame_count, sum);
        }

        // Compare with previous frame to find actual changed region
        let mut min_x = content_w;
        let mut min_y = content_h;
        let mut max_x = 0usize;
        let mut max_y = 0usize;

        for y in 0..content_h {
            let row_off = y * content_w;
            for x in 0..content_w {
                if gray_buf[row_off + x] != prev_buf[row_off + x] {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }

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

        if min_x > max_x || min_y > max_y {
            // No actual pixel changes
            debug!("Frame {}: no pixel changes, skipping", frame_count);
        } else {
            let change_w = max_x - min_x + 1;
            let change_h = max_y - min_y + 1;
            let change_pixels = change_w * change_h;
            let total_pixels = content_w * content_h;
            let change_ratio = change_pixels as f32 / total_pixels as f32;

            if frame_count == 1 {
                display.update_full(it8951::DisplayMode::Gc16)?;
                debug!("Frame {}: initial full GC16 update", frame_count);
            } else if change_ratio > 0.4 {
                display.update_full(config.display_mode)?;
                debug!("Frame {}: full update ({:.0}% changed)", frame_count, change_ratio * 100.0);
            } else {
                // Align area to 4-pixel boundaries (IT8951 requires word-aligned dimensions)
                let aligned_x = min_x & !3; // round down to multiple of 4
                let aligned_y = min_y;
                let aligned_w = ((max_x + 4) & !3) - aligned_x; // round up end to multiple of 4
                let aligned_w = aligned_w.min(content_w - aligned_x);
                let aligned_h = change_h;
                let area = it8951::Area::new(
                    aligned_x as u16,
                    aligned_y as u16,
                    aligned_w as u16,
                    aligned_h as u16,
                );
                display.update_partial(&area, config.display_mode)?;
                debug!(
                    "Frame {}: partial update {}x{} at ({},{}) ({:.0}% changed)",
                    frame_count, change_w, change_h, min_x, min_y, change_ratio * 100.0
                );
            }
        }

        // Rate limit
        let elapsed = frame_start.elapsed();
        if elapsed < config.frame_interval {
            std::thread::sleep(config.frame_interval - elapsed);
        }
    }

    info!("Capture loop ended after {} frames", frame_count);
    Ok(())
}
