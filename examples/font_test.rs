//! Font rendering test example
//!
//! This example demonstrates font rendering capabilities by rendering
//! text to a framebuffer and optionally saving it as an image or
//! displaying it on the e-ink display.
//!
//! Usage:
//!   cargo run --example font_test -- --font /path/to/font.ttf
//!   cargo run --example font_test -- --font /path/to/font.ttf --output test.png
//!   cargo run --example font_test -- --font /path/to/font.ttf --display

use std::path::PathBuf;

use clap::Parser;
use image::GrayImage;

use paper_tty::font::{system::find_monospace_font, FontMetrics, FontRenderer, TtfFont};

#[derive(Parser)]
#[command(name = "font_test")]
#[command(about = "Test font rendering")]
struct Args {
    /// Path to TTF font file
    #[arg(short, long)]
    font: Option<PathBuf>,

    /// Font size in pixels
    #[arg(short, long, default_value = "16")]
    size: f32,

    /// Output image file (PNG)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Display on e-ink (requires hardware)
    #[arg(short, long)]
    display: bool,

    /// Test text to render
    #[arg(short, long, default_value = "The quick brown fox jumps over the lazy dog.")]
    text: String,

    /// Show all ASCII printable characters
    #[arg(long)]
    ascii: bool,

    /// Show font metrics
    #[arg(long)]
    metrics: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Find font
    let font_path = args.font
        .or_else(find_monospace_font)
        .ok_or("No font specified and no system font found. Use --font <path>")?;

    println!("Loading font: {:?}", font_path);
    let mut font = TtfFont::from_file(&font_path, args.size)?;

    let metrics = font.metrics();
    println!("Font metrics:");
    println!("  Cell size: {}x{} pixels", metrics.width, metrics.height);
    println!("  Baseline: {} pixels from top", metrics.baseline);
    println!("  Line height: {} pixels", metrics.line_height);

    if args.metrics {
        // Just show metrics and exit
        return Ok(());
    }

    // Determine text to render
    let text = if args.ascii {
        // All printable ASCII characters
        let mut s = String::new();
        for row in 0..6 {
            for col in 0..16 {
                let c = (32 + row * 16 + col) as u8 as char;
                if c.is_ascii() && !c.is_ascii_control() {
                    s.push(c);
                }
            }
            s.push('\n');
        }
        s
    } else {
        args.text.clone()
    };

    // Calculate image dimensions
    let lines: Vec<&str> = text.lines().collect();
    let max_cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let num_rows = lines.len();

    let img_width = (max_cols as u16 * metrics.width).max(100);
    let img_height = (num_rows as u16 * metrics.line_height).max(50);

    println!("Rendering {} lines, max {} cols", num_rows, max_cols);
    println!("Image size: {}x{} pixels", img_width, img_height);

    // Create framebuffer (white background)
    let mut pixels = vec![255u8; (img_width as usize) * (img_height as usize)];

    // Render text
    for (row, line) in lines.iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            render_char(
                &mut font,
                &metrics,
                &mut pixels,
                img_width,
                col as u16,
                row as u16,
                c,
                0, // Black text
            );
        }
    }

    // Output
    if let Some(output_path) = args.output {
        // Save as PNG
        let img = GrayImage::from_raw(img_width as u32, img_height as u32, pixels)
            .ok_or("Failed to create image")?;
        img.save(&output_path)?;
        println!("Saved to {:?}", output_path);
    } else if args.display {
        // Display on e-ink
        #[cfg(target_os = "linux")]
        {
            use paper_tty::display::EinkDisplay;
            use paper_tty::config::DisplayConfig;

            println!("Initializing display...");
            let mut display = EinkDisplay::new(DisplayConfig::default())?;
            display.clear()?;

            let fb = display.framebuffer();
            let disp_width = fb.width();
            let disp_height = fb.height();

            // Copy rendered text to display framebuffer
            for y in 0..img_height.min(disp_height) {
                for x in 0..img_width.min(disp_width) {
                    let idx = (y as usize) * (img_width as usize) + (x as usize);
                    let _ = fb.set_pixel(x, y, pixels[idx]);
                }
            }

            println!("Updating display...");
            display.update_full(it8951::DisplayMode::Gc16)?;
            display.wait_ready()?;
            println!("Done!");
        }

        #[cfg(not(target_os = "linux"))]
        {
            println!("Display output requires Linux with SPI hardware.");
            println!("Use --output to save as PNG instead.");
        }
    } else {
        // Print ASCII art preview
        println!("\nASCII preview (scaled down):");
        print_ascii_preview(&pixels, img_width as usize, img_height as usize);
    }

    Ok(())
}

/// Render a single character to the pixel buffer
fn render_char(
    font: &mut TtfFont,
    metrics: &FontMetrics,
    pixels: &mut [u8],
    img_width: u16,
    col: u16,
    row: u16,
    c: char,
    fg_color: u8,
) {
    let cell_x = col * metrics.width;
    let cell_y = row * metrics.line_height;

    if let Some(glyph) = font.render_glyph(c) {
        // Calculate glyph position within cell
        let glyph_x = cell_x as i32 + glyph.x_offset as i32;
        let glyph_y = cell_y as i32 + metrics.baseline as i32 - glyph.height as i32 - glyph.y_offset as i32;

        // Render glyph pixels
        for gy in 0..glyph.height {
            for gx in 0..glyph.width {
                let px = glyph_x + gx as i32;
                let py = glyph_y + gy as i32;

                if px >= 0 && py >= 0 && (px as u16) < img_width && (py as usize) < pixels.len() / img_width as usize {
                    let px = px as usize;
                    let py = py as usize;
                    let idx = py * (img_width as usize) + px;

                    let glyph_idx = (gy as usize) * (glyph.width as usize) + (gx as usize);
                    let alpha = glyph.data[glyph_idx];

                    if alpha > 0 && idx < pixels.len() {
                        // Blend with background
                        let bg = pixels[idx] as u32;
                        let fg = fg_color as u32;
                        let a = alpha as u32;
                        let blended = (fg * a + bg * (255 - a)) / 255;
                        pixels[idx] = blended as u8;
                    }
                }
            }
        }
    }
}

/// Print a simple ASCII art preview of the rendered text
fn print_ascii_preview(pixels: &[u8], width: usize, height: usize) {
    let scale_x = (width / 80).max(1);
    let scale_y = (height / 24).max(1);

    for y in (0..height).step_by(scale_y) {
        for x in (0..width).step_by(scale_x) {
            let idx = y * width + x;
            if idx < pixels.len() {
                let c = match pixels[idx] {
                    0..=31 => '@',
                    32..=95 => '#',
                    96..=159 => '+',
                    160..=223 => '.',
                    224..=255 => ' ',
                };
                print!("{}", c);
            }
        }
        println!();
    }
}
