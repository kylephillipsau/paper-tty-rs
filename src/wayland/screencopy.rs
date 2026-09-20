//! Wayland screencopy client using wlr-screencopy-unstable-v1.
//!
//! Connects to a Wayland compositor (e.g. Sway), captures frames via
//! the wlr-screencopy protocol, converts to grayscale, and pushes to
//! the e-ink display.
//!
//! Input handling is delegated to the compositor via seatd/libseat.
//! This module is display-only.
//!
//! # Update strategy
//!
//! Measured on the IT8951 (9.7" M841 firmware): a waveform costs ~200 ms (DU/A2)
//! or ~500 ms (GL16/GC16) regardless of the area, the controller blocks the host
//! on HRDY for the whole waveform, and it serialises updates. So the loop is built
//! around one merged update per frame in a fast mode, sent as soon as the panel is
//! idle, with the compositor telling us when there is damage instead of polling:
//!
//! 1. wait for the panel to finish the previous waveform
//! 2. ask the compositor for the next frame *with damage* (blocks until something changed)
//! 3. convert + diff only the damaged rectangle, in the quantised domain the fast mode uses
//! 4. send the aligned bounding box of real changes with the interactive mode
//! 5. after the screen has been still for a while, re-render the regions that were
//!    updated in a binary mode with a greyscale cleanup mode to remove ghosting

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use it8951::{Area, DisplayMode, PixelFormat};

use crate::display::EinkDisplay;
use crate::error::{Error, Result};

use super::shm::ShmBuffer;

/// How pixels are reduced before a binary / 4-level update and diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantize {
    /// Plain threshold (binary) or nearest level (4-level).
    Threshold(u8),
    /// 4x4 ordered (Bayer) dither.
    Bayer,
}

/// Configuration for the screencopy capture loop.
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Minimum time between two updates (0 = as fast as the panel allows).
    pub min_update_interval: Duration,
    /// Mode for interactive updates (typing, scrolling).
    pub display_mode: DisplayMode,
    /// Mode used to clean ghosting from regions updated in a binary mode.
    pub cleanup_mode: DisplayMode,
    /// How long the screen must be still before a cleanup pass runs.
    pub cleanup_delay: Duration,
    /// Every N cleanup passes, refresh the whole viewport with GC16 (0 = never).
    pub full_refresh_interval: u32,
    /// Quantisation for binary / 4-level modes.
    pub quantize: Quantize,
    /// Pixel format used for interactive updates.
    pub interactive_format: PixelFormat,
    /// Pixel format used for cleanup / greyscale updates.
    pub cleanup_format: PixelFormat,
    /// Composite the pointer into captured frames.
    pub show_cursor: bool,
    /// Mode for small changes (pointer moves, single keystrokes), if different.
    pub small_mode: Option<DisplayMode>,
    /// Changes up to this many pixels (aligned bounding box) use `small_mode`.
    pub small_max_pixels: usize,
}

/// Number of grey levels a display mode can actually show.
fn levels_for_mode(mode: DisplayMode) -> u32 {
    match mode {
        DisplayMode::Du | DisplayMode::A2 => 2,
        DisplayMode::Du4 => 4,
        _ => 256,
    }
}

/// Pixels per 16-bit word for a pixel format; regions are aligned to this.
fn alignment_for(format: PixelFormat) -> usize {
    match format {
        PixelFormat::Bpp8 => 2,
        PixelFormat::Bpp4 => 4,
        PixelFormat::Bpp2 => 8,
        PixelFormat::Bpp3 => 2,
    }
}

const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Quantise one pixel to `levels` grey levels (returned as an 8-bit value).
#[inline]
fn quantize_pixel(gray: u8, x: usize, y: usize, levels: u32, q: Quantize) -> u8 {
    if levels >= 256 {
        return gray;
    }
    let step = 255 / (levels - 1);
    let idx = match q {
        Quantize::Threshold(t) if levels == 2 => (gray >= t) as u32,
        Quantize::Threshold(_) => (gray as u32 + step / 2) / step,
        Quantize::Bayer => {
            // Dither offset in [0, step): (2m+1)/32 of a step
            let m = BAYER4[y & 3][x & 3] as u32;
            let t = ((2 * m + 1) * step) / 32;
            (gray as u32 + t) / step
        }
    };
    (idx.min(levels - 1) * step) as u8
}

/// A rectangle in content coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    fn clamp(self, w: usize, h: usize) -> Option<Rect> {
        let x0 = self.x.min(w);
        let y0 = self.y.min(h);
        let x1 = (self.x + self.w).min(w);
        let y1 = (self.y + self.h).min(h);
        if x1 > x0 && y1 > y0 {
            Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
        } else {
            None
        }
    }

    /// Widen so that `viewport_x + x` and the width are multiples of `align`,
    /// staying inside the content area.
    fn aligned(self, align: usize, viewport_x: usize, content_w: usize) -> Rect {
        let abs_x = viewport_x + self.x;
        let start = abs_x - (abs_x % align);
        let x = start.saturating_sub(viewport_x);
        let end = self.x + self.w;
        let end = ((end + align - 1) / align) * align;
        let w = end.min(content_w) - x;
        Rect { x, y: self.y, w, h: self.h }
    }

    fn to_area(self) -> Area {
        Area::new(self.x as u16, self.y as u16, self.w as u16, self.h as u16)
    }
}

/// Internal state for the Wayland event loop.
pub struct State {
    // Globals
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    screencopy_manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    screencopy_version: u32,

    // Frame state
    buffer: Option<ShmBuffer>,
    frame_width: u32,
    frame_height: u32,
    frame_stride: u32,
    frame_format: Option<wl_shm::Format>,
    frame_ready: bool,
    frame_failed: bool,

    // Damage reported by the compositor for the current frame
    damage: Option<Rect>,

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
            screencopy_version: 0,
            buffer: None,
            frame_width: 0,
            frame_height: 0,
            frame_stride: 0,
            frame_format: None,
            frame_ready: false,
            frame_failed: false,
            damage: None,
            format_is_bgr: false,
            running: true,
        }
    }

    fn reset_frame(&mut self) {
        self.frame_ready = false;
        self.frame_failed = false;
        self.frame_format = None;
        self.damage = None;
    }

    fn add_damage(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let r = Rect { x: x as usize, y: y as usize, w: w as usize, h: h as usize };
        self.damage = Some(match self.damage {
            Some(d) => d.union(r),
            None => r,
        });
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
                    let v = version.min(3);
                    state.screencopy_version = v;
                    state.screencopy_manager = Some(registry.bind(name, v, qh, ()));
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

                if let Some(ref buf) = state.buffer {
                    if state.screencopy_version >= 2 {
                        // Compositor completes the copy only once the output has damage.
                        frame.copy_with_damage(buf.wl_buffer());
                    } else {
                        frame.copy(buf.wl_buffer());
                    }
                }
            }
            zwlr_screencopy_frame_v1::Event::Damage { x, y, width, height } => {
                state.add_damage(x as u32, y as u32, width as u32, height as u32);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                if state.damage.is_none() {
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

/// Pump the Wayland connection until `timeout` elapses or events arrive.
///
/// Returns `Ok(true)` if events were dispatched, `Ok(false)` on timeout.
fn dispatch_with_timeout(
    queue: &mut EventQueue<State>,
    state: &mut State,
    timeout: Duration,
) -> Result<bool> {
    let wl_err = |e: &dyn std::fmt::Display| Error::Wayland(format!("Dispatch failed: {}", e));

    // Deliver anything already queued first.
    if queue.dispatch_pending(state).map_err(|e| wl_err(&e))? > 0 {
        return Ok(true);
    }
    queue.flush().map_err(|e| wl_err(&e))?;

    let guard = match queue.prepare_read() {
        Some(g) => g,
        None => {
            queue.dispatch_pending(state).map_err(|e| wl_err(&e))?;
            return Ok(true);
        }
    };

    let mut pfd = libc::pollfd {
        fd: guard.connection_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: pfd is a valid pollfd for the duration of the call.
    let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if n < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            drop(guard);
            return Ok(false);
        }
        return Err(Error::Wayland(format!("poll failed: {}", err)));
    }
    if n == 0 {
        drop(guard);
        return Ok(false);
    }
    guard.read().map_err(|e| wl_err(&e))?;
    queue.dispatch_pending(state).map_err(|e| wl_err(&e))?;
    Ok(true)
}

/// Convert the damaged rectangle of the captured frame to grayscale, scaling to the
/// content size with nearest-neighbour if the output size differs.
fn convert_rect(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    src_stride: usize,
    bgr: bool,
    dst: &mut [u8],
    content_w: usize,
    content_h: usize,
    rect: Rect,
) {
    let scaled = src_w != content_w || src_h != content_h;
    for dy in rect.y..rect.y + rect.h {
        let sy = if scaled { dy * src_h / content_h } else { dy };
        let src_row = &src[sy * src_stride..];
        let dst_row = &mut dst[dy * content_w..(dy + 1) * content_w];
        for dx in rect.x..rect.x + rect.w {
            let sx = if scaled { dx * src_w / content_w } else { dx };
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

/// Run the screencopy capture loop.
///
/// This connects to the Wayland compositor, captures frames, converts them
/// to grayscale, and pushes them to the e-ink display. See the module docs
/// for the update strategy.
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

    info!(
        "Wayland connection established (screencopy v{}, {})",
        state.screencopy_version,
        if state.screencopy_version >= 2 { "damage-driven capture" } else { "polling capture" }
    );
    info!("Input handled by compositor via seatd/libseat");
    info!(
        "Interactive: {:?} {:?} {:?}; cleanup: {:?} {:?} after {:?}; full GC16 every {} cleanups",
        config.display_mode,
        config.interactive_format,
        config.quantize,
        config.cleanup_mode,
        config.cleanup_format,
        config.cleanup_delay,
        config.full_refresh_interval
    );
    if let Some(m) = config.small_mode {
        info!("Small changes (<= {} px): {:?}", config.small_max_pixels, m);
    }

    let viewport = *display.viewport();
    let content_w = viewport.width as usize;
    let content_h = viewport.height as usize;
    let vp_x = viewport.x as usize;
    let levels = levels_for_mode(config.display_mode);
    let align = alignment_for(config.interactive_format)
        .max(alignment_for(config.cleanup_format));
    if vp_x % align != 0 || content_w % align != 0 {
        warn!(
            "Viewport x={} width={} is not a multiple of {} px; regions touching the edges may \
             violate the IT8951 word alignment for the chosen pixel format. Use margins that are \
             multiples of {}.",
            vp_x, content_w, align, align
        );
    }

    // Latest greyscale frame, and what we believe the panel currently shows
    // (in the quantised domain of the interactive mode).
    let mut gray = vec![0xFFu8; content_w * content_h];
    let mut shown = vec![0xFFu8; content_w * content_h];
    let mut quant = vec![0xFFu8; content_w * content_h];

    // Region updated in the interactive mode since the last cleanup pass
    let mut dirty_since_cleanup: Option<Rect> = None;
    let mut last_change = Instant::now();
    let mut last_send: Option<Instant> = None;
    let mut cleanups_since_full: u32 = 0;
    let mut capture_pending = false;
    let mut first_frame = true;

    let mut frame_count = 0u64;
    let mut updates_sent = 0u64;

    // The busy register can read idle for a moment right after a refresh is accepted.
    const BUSY_GUARD: Duration = Duration::from_millis(20);
    const IDLE_POLL: Duration = Duration::from_millis(1000);

    while state.running {
        // Request the next frame if none is in flight. With copy_with_damage the
        // compositor answers only once something has changed on the output.
        if !capture_pending {
            state.reset_frame();
            let overlay_cursor = config.show_cursor as i32;
            let _frame = screencopy_mgr.capture_output(overlay_cursor, &output, &qh, ());
            capture_pending = true;
        }

        // Work out how long we can block: until the cleanup pass is due, if one is pending.
        let timeout = match dirty_since_cleanup {
            Some(_) => config
                .cleanup_delay
                .checked_sub(last_change.elapsed())
                .unwrap_or(Duration::ZERO),
            None => IDLE_POLL,
        };
        dispatch_with_timeout(&mut event_queue, &mut state, timeout)?;

        if state.frame_failed {
            warn!("Frame {} failed, retrying", frame_count);
            capture_pending = false;
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        if state.frame_ready {
            capture_pending = false;
            frame_count += 1;

            let damage = state
                .damage
                .and_then(|d| {
                    // Damage is in output pixels; map to content pixels if scaled.
                    let sw = state.frame_width as usize;
                    let sh = state.frame_height as usize;
                    if sw == content_w && sh == content_h {
                        Some(d)
                    } else if sw > 0 && sh > 0 {
                        Some(Rect {
                            x: d.x * content_w / sw,
                            y: d.y * content_h / sh,
                            w: (d.w * content_w + sw - 1) / sw + 1,
                            h: (d.h * content_h + sh - 1) / sh + 1,
                        })
                    } else {
                        None
                    }
                })
                .and_then(|d| d.clamp(content_w, content_h));

            let damage = if first_frame {
                Some(Rect { x: 0, y: 0, w: content_w, h: content_h })
            } else {
                damage
            };

            let Some(damage) = damage else {
                continue;
            };

            if let Some(ref buf) = state.buffer {
                convert_rect(
                    buf.data(),
                    state.frame_width as usize,
                    state.frame_height as usize,
                    state.frame_stride as usize,
                    state.format_is_bgr,
                    &mut gray,
                    content_w,
                    content_h,
                    damage,
                );
            }

            // Quantise the damaged rect and find the bounding box of real changes.
            let mut changed: Option<Rect> = None;
            for y in damage.y..damage.y + damage.h {
                let row = y * content_w;
                let mut min_x = usize::MAX;
                let mut max_x = 0;
                for x in damage.x..damage.x + damage.w {
                    let q = quantize_pixel(gray[row + x], x, y, levels, config.quantize);
                    quant[row + x] = q;
                    if q != shown[row + x] {
                        min_x = min_x.min(x);
                        max_x = x;
                    }
                }
                if min_x != usize::MAX {
                    let r = Rect { x: min_x, y, w: max_x - min_x + 1, h: 1 };
                    changed = Some(changed.map_or(r, |c| c.union(r)));
                }
            }

            if first_frame {
                // Bring the panel to a known state: full viewport, greyscale.
                first_frame = false;
                let full = Rect { x: 0, y: 0, w: content_w, h: content_h };
                display.wait_idle(last_send, BUSY_GUARD)?;
                display.update_region_from(
                    &full.to_area(),
                    &gray,
                    content_w,
                    config.cleanup_format,
                    DisplayMode::Gc16,
                )?;
                last_send = Some(Instant::now());
                updates_sent += 1;
                shown.copy_from_slice(&quant);
                debug!("Frame 1: initial full GC16 update");
                continue;
            }

            let Some(changed) = changed else {
                if frame_count % 100 == 0 {
                    debug!("Frame {}: damage {:?} but nothing changed after quantisation", frame_count, damage);
                }
                continue;
            };

            let region = changed.aligned(align, vp_x, content_w);

            // Pace: the panel blocks us anyway, but make sure we capture *after* it
            // is idle so we always send the freshest frame.
            if let Some(t) = last_send {
                let since = t.elapsed();
                if since < config.min_update_interval {
                    std::thread::sleep(config.min_update_interval - since);
                }
            }
            let wait_start = Instant::now();
            display.wait_idle(last_send, BUSY_GUARD)?;
            let waited = wait_start.elapsed();

            let mode = match config.small_mode {
                Some(m) if region.w * region.h <= config.small_max_pixels => m,
                _ => config.display_mode,
            };
            let send_start = Instant::now();
            display.update_region_from(
                &region.to_area(),
                &quant,
                content_w,
                config.interactive_format,
                mode,
            )?;
            last_send = Some(Instant::now());
            updates_sent += 1;

            for y in region.y..region.y + region.h {
                let off = y * content_w + region.x;
                shown[off..off + region.w].copy_from_slice(&quant[off..off + region.w]);
            }
            last_change = Instant::now();
            if levels < 256 {
                dirty_since_cleanup = Some(dirty_since_cleanup.map_or(region, |d| d.union(region)));
            }

            debug!(
                "Frame {}: {:?} {}x{} at ({},{}) [damage {}x{}] waited {:?}, sent in {:?}",
                frame_count,
                mode,
                region.w,
                region.h,
                region.x,
                region.y,
                damage.w,
                damage.h,
                waited,
                send_start.elapsed()
            );
            continue;
        }

        // No new frame: run the cleanup pass if the screen has been still long enough.
        if let Some(dirty) = dirty_since_cleanup {
            if last_change.elapsed() >= config.cleanup_delay {
                display.wait_idle(last_send, BUSY_GUARD)?;

                cleanups_since_full += 1;
                let (region, mode) = if config.full_refresh_interval > 0
                    && cleanups_since_full >= config.full_refresh_interval
                {
                    cleanups_since_full = 0;
                    (Rect { x: 0, y: 0, w: content_w, h: content_h }, DisplayMode::Gc16)
                } else {
                    (dirty.aligned(align, vp_x, content_w), config.cleanup_mode)
                };

                let send_start = Instant::now();
                display.update_region_from(
                    &region.to_area(),
                    &gray,
                    content_w,
                    config.cleanup_format,
                    mode,
                )?;
                last_send = Some(Instant::now());
                updates_sent += 1;
                dirty_since_cleanup = None;
                debug!(
                    "Cleanup: {:?} {}x{} at ({},{}) sent in {:?} [{}/{}]",
                    mode,
                    region.w,
                    region.h,
                    region.x,
                    region.y,
                    send_start.elapsed(),
                    cleanups_since_full,
                    config.full_refresh_interval
                );
            }
        }
    }

    info!(
        "Capture loop ended after {} frames ({} updates sent)",
        frame_count, updates_sent
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_binary_threshold() {
        assert_eq!(quantize_pixel(127, 0, 0, 2, Quantize::Threshold(128)), 0);
        assert_eq!(quantize_pixel(128, 0, 0, 2, Quantize::Threshold(128)), 255);
    }

    #[test]
    fn quantize_four_levels_nearest() {
        assert_eq!(quantize_pixel(0, 0, 0, 4, Quantize::Threshold(128)), 0);
        assert_eq!(quantize_pixel(90, 0, 0, 4, Quantize::Threshold(128)), 85);
        assert_eq!(quantize_pixel(255, 0, 0, 4, Quantize::Threshold(128)), 255);
    }

    #[test]
    fn quantize_bayer_mid_gray_is_checkerboard_ish() {
        let mut whites = 0;
        for y in 0..4 {
            for x in 0..4 {
                if quantize_pixel(128, x, y, 2, Quantize::Bayer) == 255 {
                    whites += 1;
                }
            }
        }
        assert!((7..=9).contains(&whites), "got {} whites", whites);
    }

    #[test]
    fn quantize_passthrough_for_gray_modes() {
        assert_eq!(quantize_pixel(37, 1, 2, 256, Quantize::Bayer), 37);
    }

    #[test]
    fn rect_align_widens_to_multiple_in_display_coords() {
        // viewport x = 80 (multiple of 8); region x=3,w=5 -> x=0,w=8
        let r = Rect { x: 3, y: 0, w: 5, h: 1 }.aligned(8, 80, 100);
        assert_eq!(r, Rect { x: 0, y: 0, w: 8, h: 1 });
        // clamps to content width
        let r = Rect { x: 97, y: 0, w: 2, h: 1 }.aligned(8, 80, 100);
        assert_eq!(r, Rect { x: 96, y: 0, w: 4, h: 1 });
        // unaligned viewport: display x = 82+3 = 85 -> start 80 -> content x = -2 -> 0
        let r = Rect { x: 3, y: 0, w: 5, h: 1 }.aligned(8, 82, 100);
        assert_eq!(r.x, 0);
    }

    #[test]
    fn rect_union_and_clamp() {
        let a = Rect { x: 0, y: 0, w: 2, h: 2 };
        let b = Rect { x: 5, y: 5, w: 1, h: 1 };
        assert_eq!(a.union(b), Rect { x: 0, y: 0, w: 6, h: 6 });
        assert_eq!(Rect { x: 8, y: 0, w: 4, h: 1 }.clamp(10, 10), Some(Rect { x: 8, y: 0, w: 2, h: 1 }));
        assert_eq!(Rect { x: 10, y: 0, w: 4, h: 1 }.clamp(10, 10), None);
    }
}
