//! Paper-TTY: E-ink terminal emulator
//!
//! Renders Linux terminal output on IT8951-based e-paper displays.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use log::{error, info, warn};

use paper_tty::{
    config::Config,
    display::EinkDisplay,
    error::Result,
    font::{system::find_monospace_font, TtfFont},
    input::{find_keyboard_device, KeyboardReader},
    renderer::TextRenderer,
    terminal::{PtyReader, TerminalReader, VcsaReader},
};

#[derive(Parser)]
#[command(name = "paper-tty")]
#[command(about = "E-ink terminal emulator for IT8951-based displays")]
#[command(version)]
struct Cli {
    /// Rotation in degrees (0, 90, 180, 270)
    #[arg(short, long, default_value = "0")]
    rotate: u16,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Config file path
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Render a Linux terminal to the display
    Terminal {
        /// TTY number to render (1 = /dev/tty1) - only used with --vcsa
        #[arg(short, long, default_value = "1")]
        tty: u8,

        /// Use VCSA interface (reads system console, requires root)
        #[arg(long)]
        vcsa: bool,

        /// Use PTY mode (spawns shell with custom dimensions)
        #[arg(long)]
        pty: bool,

        /// Shell to use for PTY mode (default: $SHELL or /bin/sh)
        #[arg(long)]
        shell: Option<String>,

        /// Path to font file
        #[arg(short, long)]
        font: Option<PathBuf>,

        /// Font size in pixels
        #[arg(short, long, default_value = "16")]
        size: f32,

        /// Cursor style (block, underline, bar, none)
        #[arg(long, default_value = "block")]
        cursor: String,

        /// Refresh interval in milliseconds
        #[arg(long, default_value = "250")]
        refresh_rate: u64,

        /// Enable partial refresh (faster but may ghost)
        #[arg(long)]
        partial: bool,

        /// Display mode for updates: du (fast mono), gc16 (quality), gl16 (balanced), a2 (fastest)
        #[arg(long, default_value = "gl16")]
        mode: String,

        /// Left margin in pixels
        #[arg(long, default_value = "0")]
        margin_left: u16,

        /// Right margin in pixels
        #[arg(long, default_value = "0")]
        margin_right: u16,

        /// Top margin in pixels
        #[arg(long, default_value = "0")]
        margin_top: u16,

        /// Bottom margin in pixels
        #[arg(long, default_value = "0")]
        margin_bottom: u16,

        /// Use light theme (white background, black text)
        #[arg(long)]
        light: bool,

        /// Keyboard device path (e.g., /dev/input/event0) - auto-detected if not specified
        #[arg(long)]
        keyboard: Option<PathBuf>,
    },

    /// Clear the display
    Clear {
        /// Grayscale value (0=black, 255=white)
        #[arg(short, long, default_value = "255")]
        gray: u8,
    },

    /// Display a test pattern
    Test,

    /// Capture Sway compositor output to e-ink display
    ///
    /// Input handling is delegated to Sway via seatd/libseat - this is display-only.
    /// Ensure seatd is running and your user is in the 'seat' group.
    #[cfg(feature = "sway")]
    Sway {
        /// Display mode: du (fast mono), gc16 (quality), gl16 (balanced), a2 (fastest)
        #[arg(long, default_value = "du")]
        mode: String,

        /// Minimum frame interval in milliseconds
        #[arg(long, default_value = "100")]
        frame_interval: u64,

        /// Full refresh every N frames (0 = never)
        #[arg(long, default_value = "0")]
        full_refresh_interval: u32,

        /// Minimum row gap to split into separate update regions (0 = single bounding box)
        ///
        /// When disjoint areas of the screen change (e.g., waybar at top + terminal at bottom),
        /// setting this to 50+ will send separate partial updates instead of one large bounding box.
        /// This reduces unnecessary refresh of unchanged middle areas.
        #[arg(long, default_value = "50")]
        region_gap: usize,

        /// Left margin in pixels
        #[arg(long, default_value = "0")]
        margin_left: u16,

        /// Right margin in pixels
        #[arg(long, default_value = "0")]
        margin_right: u16,

        /// Top margin in pixels
        #[arg(long, default_value = "0")]
        margin_top: u16,

        /// Bottom margin in pixels
        #[arg(long, default_value = "0")]
        margin_bottom: u16,
    },

    /// Show display information
    Info,
}

fn main() {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    env_logger::Builder::new().filter_level(log_level).init();

    // Load configuration
    let config = if let Some(ref path) = cli.config {
        Config::load(path).unwrap_or_else(|e| {
            warn!("Failed to load config file: {}, using defaults", e);
            Config::default()
        })
    } else {
        Config::load_default().unwrap_or_else(|e| {
            warn!("Failed to load default config: {}, using defaults", e);
            Config::default()
        })
    };

    // Run the appropriate command
    let result = match cli.command {
        Commands::Terminal {
            tty,
            vcsa,
            pty,
            shell,
            font,
            size,
            cursor,
            refresh_rate,
            partial,
            mode,
            margin_left,
            margin_right,
            margin_top,
            margin_bottom,
            light,
            keyboard,
        } => run_terminal(
            tty, vcsa, pty, shell.as_deref(), font, size, &cursor, refresh_rate, partial, &mode,
            (margin_left, margin_right, margin_top, margin_bottom), light, keyboard, &config
        ),
        #[cfg(feature = "sway")]
        Commands::Sway { mode, frame_interval, full_refresh_interval, region_gap,
                         margin_left, margin_right, margin_top, margin_bottom } => {
            run_sway(&mode, frame_interval, full_refresh_interval, region_gap,
                     (margin_left, margin_right, margin_top, margin_bottom), &config)
        }
        Commands::Clear { gray } => run_clear(gray, &config),
        Commands::Test => run_test(&config),
        Commands::Info => run_info(&config),
    };

    if let Err(e) = result {
        error!("Error: {}", e);
        std::process::exit(1);
    }
}

fn parse_display_mode(mode: &str) -> it8951::DisplayMode {
    match mode.to_lowercase().as_str() {
        "du" => it8951::DisplayMode::Du,
        "gc16" => it8951::DisplayMode::Gc16,
        "gl16" => it8951::DisplayMode::Gl16,
        "a2" => it8951::DisplayMode::A2,
        "init" => it8951::DisplayMode::Init,
        _ => it8951::DisplayMode::Gl16, // Default to balanced mode
    }
}

/// Set up keyboard input via evdev
fn setup_keyboard_reader(device_path: Option<PathBuf>) -> Result<KeyboardReader> {
    // Use specified path or auto-detect
    let device_path = device_path
        .or_else(find_keyboard_device)
        .ok_or_else(|| paper_tty::Error::Terminal(
            "No keyboard device found in /dev/input/".to_string()
        ))?;

    KeyboardReader::new(&device_path)
}

fn run_terminal(
    tty: u8,
    use_vcsa: bool,
    use_pty: bool,
    shell: Option<&str>,
    font_path: Option<PathBuf>,
    font_size: f32,
    cursor_style: &str,
    refresh_rate: u64,
    partial_refresh: bool,
    display_mode: &str,
    margins: (u16, u16, u16, u16), // left, right, top, bottom
    light_theme: bool,
    keyboard_device: Option<PathBuf>,
    config: &Config,
) -> Result<()> {
    info!("Starting terminal renderer ({})", if light_theme { "light theme" } else { "dark theme" });

    // Ignore SIGINT and SIGTSTP so Ctrl+C and Ctrl+Z are forwarded to the PTY shell
    // rather than killing the paper-tty process
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
    }

    // Initialize display
    let mut display = EinkDisplay::new(config.display.clone())?;
    info!("Display: {}x{}", display.width(), display.height());

    // Set viewport margins (display handles coordinate translation)
    display.set_margins(margins);

    // Clear display
    display.clear()?;

    // Load font
    let font_path = font_path
        .or_else(|| config.font.path.as_ref().map(PathBuf::from))
        .or_else(find_monospace_font)
        .ok_or_else(|| paper_tty::Error::Font("No font found".to_string()))?;

    info!("Loading font: {:?}", font_path);
    let font = TtfFont::from_file(&font_path, font_size)?;

    // Create renderer with appropriate color theme
    let colors = if light_theme {
        paper_tty::config::ColorConfig::light()
    } else {
        config.colors.clone()
    };
    let mut renderer = TextRenderer::new(font, colors);
    renderer.set_cursor_style(cursor_style.parse()?);

    // Calculate terminal dimensions from content area
    let (cols, rows) = renderer.calculate_dimensions(display.content_width(), display.content_height());
    info!("Terminal size: {}x{} characters", cols, rows);

    // Create terminal reader based on mode
    let mut reader: Box<dyn TerminalReader> = if use_pty {
        info!("Using PTY mode with {}x{} terminal", cols, rows);
        Box::new(PtyReader::new(cols, rows, shell)?)
    } else if use_vcsa {
        info!("Using VCSA interface for TTY{}", tty);
        Box::new(VcsaReader::new(tty)?)
    } else {
        return Err(paper_tty::Error::NotAvailable(
            "Specify --pty for PTY mode or --vcsa for VCSA mode".to_string(),
        ));
    };

    // Parse display mode
    let mode = parse_display_mode(display_mode);
    info!("Display mode: {:?}", mode);

    // Set up keyboard input forwarding if supported (via evdev)
    let keyboard = if reader.supports_input() {
        match setup_keyboard_reader(keyboard_device) {
            Ok(kb) => {
                info!("Keyboard input enabled via evdev");
                Some(kb)
            }
            Err(e) => {
                warn!("Keyboard input not available: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Main render loop
    info!("Starting render loop (Ctrl+C to exit)");
    let _ = refresh_rate; // poll interval now driven by settle_duration
    let settle_duration = Duration::from_millis(10);
    let scroll_settle_duration = Duration::from_millis(50);
    let mut frame_count = 0u64;
    let mut first_frame = true;
    let mut deferred_count: u32 = 0;

    // Helper: drain all queued keyboard input and forward to the PTY
    let flush_keyboard = |kb: &KeyboardReader, reader: &mut dyn TerminalReader| {
        while let Some(data) = kb.try_recv() {
            if let Err(e) = reader.write_input(&data) {
                log::warn!("Failed to write input: {}", e);
            }
        }
    };

    loop {
        frame_count += 1;

        // ── 1. Wait for keyboard input or poll timeout ──
        // Use settle_duration as the poll interval so we check for
        // program output frequently, not just on keypresses.
        if let Some(ref kb) = keyboard {
            if let Some(data) = kb.recv_timeout(settle_duration) {
                if let Err(e) = reader.write_input(&data) {
                    log::warn!("Failed to write input: {}", e);
                }
            }
        } else {
            thread::sleep(settle_duration);
        }

        // ── 2. Flush keyboard, settle, then read freshest state ──
        // Order matters: flush first so all queued keys are sent to PTY,
        // then settle so the PTY echoes them back and more keys accumulate,
        // then flush stragglers, then read the screen with everything visible.
        if let Some(ref kb) = keyboard {
            flush_keyboard(kb, reader.as_mut());
        }
        thread::sleep(settle_duration);
        if let Some(ref kb) = keyboard {
            flush_keyboard(kb, reader.as_mut());
        }
        thread::sleep(Duration::from_millis(2)); // let PTY echo the stragglers
        let screen = reader.read_screen()?;

        // ── 3. Scroll deferral check (before rendering) ──
        // If output is still streaming, defer to avoid rendering mid-burst.
        // Crucially, we do NOT call render() here — that would update
        // prev_buffer and cause the next diff to miss the changes.
        if screen.scroll_count > 0 {
            deferred_count += 1;
            if deferred_count < 3 {
                log::debug!("Frame {}: Output still arriving (scroll_count={}), defer #{}",
                    frame_count, screen.scroll_count, deferred_count);
                thread::sleep(scroll_settle_duration);
                continue;
            }
            log::debug!("Frame {}: Max deferrals reached, forcing update", frame_count);
        }

        // ── 4. Render to framebuffer ──
        let dirty_rects = renderer.render(&screen, &mut display);

        if dirty_rects.is_empty() && deferred_count == 0 {
            log::trace!("Frame {}: No changes", frame_count);
            continue;
        }

        // ── 5. Detect terminal clear ──
        if screen.is_blank() {
            log::debug!("Frame {}: Screen clear detected, full display clear", frame_count);
            display.clear()?;
            display.update_full(it8951::DisplayMode::Gc16)?;
            let _ = renderer.render(&screen, &mut display);
            deferred_count = 0;
            continue;
        }

        // ── 6. Send framebuffer to display ──
        log::debug!("Frame {}: Updating display", frame_count);
        let was_deferred = deferred_count > 0;
        deferred_count = 0;

        if first_frame {
            display.update_full(it8951::DisplayMode::Gc16)?;
            first_frame = false;
        } else if !partial_refresh || was_deferred || dirty_rects.is_empty() {
            // Full update when: partial refresh disabled, after scroll deferrals,
            // or when flushing deferred content with no new dirty rects
            display.update_full(mode)?;
        } else {
            let areas: Vec<_> = dirty_rects.iter().map(|r| r.to_area()).collect();
            let total_pixels: u32 = areas.iter()
                .map(|a| a.width as u32 * a.height as u32)
                .sum();
            let screen_pixels = display.content_width() as u32 * display.content_height() as u32;

            if total_pixels > screen_pixels * 2 / 5 {
                display.update_full(mode)?;
            } else {
                display.update_areas(&areas, mode)?;
            }
        }

        log::debug!("Frame {}: Update complete", frame_count);
    }
}

#[cfg(feature = "sway")]
fn run_sway(
    display_mode: &str,
    frame_interval: u64,
    full_refresh_interval: u32,
    region_gap: usize,
    margins: (u16, u16, u16, u16),
    config: &Config,
) -> Result<()> {
    use paper_tty::wayland::screencopy::{CaptureConfig, run_capture_loop};

    info!("Starting Sway screencopy capture (display-only, input via seatd)");

    let mut display = EinkDisplay::new(config.display.clone())?;
    info!("Display: {}x{}", display.width(), display.height());

    display.set_margins(margins);
    display.clear()?;

    let mode = parse_display_mode(display_mode);
    info!("Display mode: {:?}", mode);
    if region_gap > 0 {
        info!("Region gap: {} rows (disjoint region detection enabled)", region_gap);
    }

    let capture_config = CaptureConfig {
        frame_interval: std::time::Duration::from_millis(frame_interval),
        display_mode: mode,
        full_refresh_interval,
        region_gap,
    };

    run_capture_loop(&mut display, capture_config)
}

fn run_clear(gray: u8, config: &Config) -> Result<()> {
    info!("Clearing display to gray level {}", gray);
    let mut display = EinkDisplay::new(config.display.clone())?;
    display.clear_with(gray)?;
    display.wait_ready()?;
    info!("Display cleared");
    Ok(())
}

fn run_test(config: &Config) -> Result<()> {
    info!("Running display test");
    let mut display = EinkDisplay::new(config.display.clone())?;

    // Clear to white
    info!("Clearing to white...");
    display.clear()?;
    display.wait_ready()?;

    // Draw test pattern (using raw framebuffer for direct display coordinates)
    let fb = display.framebuffer_raw();
    let width = fb.width();
    let height = fb.height();

    // Gradient bars
    for y in 0..height {
        for x in 0..width {
            let gray = ((x as u32 * 255) / width as u32) as u8;
            let _ = fb.set_pixel(x, y, gray);
        }
    }

    // Border
    for x in 0..width {
        let _ = fb.set_pixel(x, 0, 0);
        let _ = fb.set_pixel(x, height - 1, 0);
    }
    for y in 0..height {
        let _ = fb.set_pixel(0, y, 0);
        let _ = fb.set_pixel(width - 1, y, 0);
    }

    info!("Updating display...");
    display.update_full(it8951::DisplayMode::Gc16)?;
    display.wait_ready()?;

    info!("Test complete");
    Ok(())
}

fn run_info(config: &Config) -> Result<()> {
    let display = EinkDisplay::new(config.display.clone())?;
    println!("Display Information:");
    println!("  Width:  {} pixels", display.width());
    println!("  Height: {} pixels", display.height());
    println!("  Driver: {}", config.display.driver);
    println!("  VCOM:   {} mV", config.display.vcom);
    Ok(())
}
