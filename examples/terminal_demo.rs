//! Terminal rendering demo
//!
//! This example demonstrates reading from a virtual console and
//! rendering it to an e-ink display.
//!
//! Requires root privileges to access /dev/vcsa*.
//!
//! Usage:
//!   sudo cargo run --example terminal_demo

use paper_tty::terminal::{TerminalReader, VcsaReader};

#[cfg(target_os = "linux")]
use paper_tty::{
    config::DisplayConfig,
    display::EinkDisplay,
    font::{system::find_monospace_font, TtfFont},
    renderer::TextRenderer,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Terminal Demo");
    println!("=============");
    println!();

    // Try to read from VCSA (requires root)
    let tty_num = 1;
    println!("Attempting to read from /dev/vcsa{}", tty_num);

    let mut reader = match VcsaReader::new(tty_num) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Failed to open /dev/vcsa{}: {}", tty_num, e);
            eprintln!();
            eprintln!("This example requires root privileges to access the virtual console.");
            eprintln!("Try running with: sudo cargo run --example terminal_demo");
            return Err(e.into());
        }
    };

    let (cols, rows) = reader.dimensions();
    println!("Terminal dimensions: {}x{}", cols, rows);

    // Read and display the current screen
    let screen = reader.read_screen()?;
    println!("Cursor position: {:?}", screen.cursor_pos);
    println!();

    // Print the screen content
    println!("Screen content:");
    println!("{}", "-".repeat(cols as usize + 2));
    for row in 0..screen.rows.min(24) {
        print!("|");
        for col in 0..screen.cols {
            if let Some(cell) = screen.get(col, row) {
                let c = if cell.character.is_ascii_graphic() || cell.character == ' ' {
                    cell.character
                } else {
                    '.'
                };
                print!("{}", c);
            }
        }
        println!("|");
    }
    println!("{}", "-".repeat(cols as usize + 2));

    // If we have display hardware, render to it
    #[cfg(target_os = "linux")]
    {
        println!();
        println!("Attempting to initialize display...");

        match EinkDisplay::new(DisplayConfig::default()) {
            Ok(mut display) => {
                println!("Display initialized: {}x{}", display.width(), display.height());

                // Find a font
                let font_path = find_monospace_font()
                    .ok_or("No system font found")?;
                println!("Using font: {:?}", font_path);

                let font = TtfFont::from_file(&font_path, 16.0)?;
                let mut renderer = TextRenderer::new(font, Default::default());

                // Clear and render
                display.clear()?;
                let dirty = renderer.render(&screen, display.framebuffer());
                println!("Rendered {} dirty regions", dirty.len());

                display.update_full(it8951::DisplayMode::Gc16)?;
                display.wait_ready()?;
                println!("Display updated!");
            }
            Err(e) => {
                println!("Display not available: {}", e);
                println!("(This is normal if not running on a Raspberry Pi with e-ink display)");
            }
        }
    }

    Ok(())
}
