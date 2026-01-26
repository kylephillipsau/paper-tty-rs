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
    renderer::TextRenderer,
    terminal::{TerminalReader, VcsaReader},
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
        /// TTY number to render (1 = /dev/tty1)
        #[arg(short, long, default_value = "1")]
        tty: u8,

        /// Use VCSA interface (faster, requires root)
        #[arg(long)]
        vcsa: bool,

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
    },

    /// Clear the display
    Clear {
        /// Grayscale value (0=black, 255=white)
        #[arg(short, long, default_value = "255")]
        gray: u8,
    },

    /// Display a test pattern
    Test,

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
            font,
            size,
            cursor,
            refresh_rate,
            partial,
        } => run_terminal(tty, vcsa, font, size, &cursor, refresh_rate, partial, &config),
        Commands::Clear { gray } => run_clear(gray, &config),
        Commands::Test => run_test(&config),
        Commands::Info => run_info(&config),
    };

    if let Err(e) = result {
        error!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run_terminal(
    tty: u8,
    use_vcsa: bool,
    font_path: Option<PathBuf>,
    font_size: f32,
    cursor_style: &str,
    refresh_rate: u64,
    partial_refresh: bool,
    config: &Config,
) -> Result<()> {
    info!("Starting terminal renderer for TTY{}", tty);

    // Initialize display
    let mut display = EinkDisplay::new(config.display.clone())?;
    info!("Display: {}x{}", display.width(), display.height());

    // Clear display
    display.clear()?;

    // Load font
    let font_path = font_path
        .or_else(|| config.font.path.as_ref().map(PathBuf::from))
        .or_else(find_monospace_font)
        .ok_or_else(|| paper_tty::Error::Font("No font found".to_string()))?;

    info!("Loading font: {:?}", font_path);
    let font = TtfFont::from_file(&font_path, font_size)?;

    // Create renderer
    let mut renderer = TextRenderer::new(font, config.colors.clone());
    renderer.set_cursor_style(cursor_style.parse()?);

    let (cols, rows) = renderer.calculate_dimensions(display.width(), display.height());
    info!("Terminal size: {}x{} characters", cols, rows);

    // Create terminal reader
    let mut reader: Box<dyn TerminalReader> = if use_vcsa {
        info!("Using VCSA interface");
        Box::new(VcsaReader::new(tty)?)
    } else {
        // TODO: Implement TTY reader
        return Err(paper_tty::Error::NotAvailable(
            "TTY reader not yet implemented, use --vcsa".to_string(),
        ));
    };

    // Main render loop
    info!("Starting render loop (Ctrl+C to exit)");
    let refresh_duration = Duration::from_millis(refresh_rate);
    let mut frame_count = 0u64;

    loop {
        frame_count += 1;

        // Read terminal state
        log::debug!("Frame {}: Reading screen...", frame_count);
        let screen = reader.read_screen()?;

        // Render to framebuffer
        log::debug!("Frame {}: Rendering {} cells...", frame_count, screen.cells.len());
        let dirty_rects = renderer.render(&screen, display.framebuffer());

        // Update display
        if !dirty_rects.is_empty() {
            log::debug!("Frame {}: {} dirty rects", frame_count, dirty_rects.len());
            if partial_refresh && dirty_rects.len() < 20 {
                // Partial updates for small changes
                let areas: Vec<_> = dirty_rects.iter().map(|r| r.to_area()).collect();
                log::debug!("Frame {}: Partial update with {} areas", frame_count, areas.len());
                display.update_areas(&areas, it8951::DisplayMode::Du)?;
            } else {
                // Full update for large changes
                log::debug!("Frame {}: Full update ({} dirty rects)", frame_count, dirty_rects.len());
                display.update_full(it8951::DisplayMode::Gc16)?;
            }
            log::debug!("Frame {}: Update complete", frame_count);
        } else {
            log::trace!("Frame {}: No changes", frame_count);
        }

        thread::sleep(refresh_duration);
    }
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

    // Draw test pattern
    let fb = display.framebuffer();
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
