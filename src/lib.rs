//! Paper-TTY: E-ink terminal emulator and VNC client
//!
//! This crate provides functionality to render Linux terminal output
//! and VNC sessions on IT8951-based e-paper displays.
//!
//! # Features
//!
//! - **Terminal rendering**: Read from `/dev/vcsa*` or `/dev/tty*` and render text
//! - **Font support**: TrueType and bitmap fonts via fontdue
//! - **Partial refresh**: Only update changed screen regions
//! - **VNC client**: Connect to VNC servers (optional feature)
//!
//! # Example
//!
//! ```rust,no_run
//! use paper_tty::{EinkDisplay, TerminalRenderer, VcsaReader};
//!
//! // Initialize display
//! let mut display = EinkDisplay::new(Default::default())?;
//!
//! // Create terminal reader
//! let reader = VcsaReader::new(1)?; // /dev/vcsa1
//!
//! // Create renderer with font
//! let renderer = TerminalRenderer::new("DejaVuSansMono.ttf", 16)?;
//!
//! // Main loop
//! loop {
//!     let screen = reader.read_screen()?;
//!     renderer.render(&screen, display.framebuffer())?;
//!     display.update_partial()?;
//! }
//! # Ok::<(), paper_tty::Error>(())
//! ```

pub mod config;
pub mod display;
pub mod error;
pub mod font;
pub mod renderer;
pub mod terminal;

#[cfg(feature = "vnc-support")]
pub mod vnc;

// Re-exports
pub use config::Config;
pub use display::EinkDisplay;
pub use error::{Error, Result};
pub use font::{FontMetrics, FontRenderer};
pub use renderer::{CursorStyle, TextRenderer};
pub use terminal::{Cell, ScreenBuffer, TerminalReader, VcsaReader};

// Re-export IT8951 types that users might need
pub use it8951::{Area, DisplayMode, Framebuffer, PixelFormat};
