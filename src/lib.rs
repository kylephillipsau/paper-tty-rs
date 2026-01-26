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
//! ```rust,ignore
//! use paper_tty::{
//!     display::EinkDisplay, config::DisplayConfig,
//!     terminal::{VcsaReader, TerminalReader},
//!     renderer::TextRenderer,
//!     font::TtfFont,
//! };
//!
//! // Initialize display
//! let mut display = EinkDisplay::new(DisplayConfig::default())?;
//!
//! // Create terminal reader
//! let mut reader = VcsaReader::new(1)?; // /dev/vcsa1
//!
//! // Create renderer with font
//! let font = TtfFont::from_file("/path/to/font.ttf", 16.0)?;
//! let mut renderer = TextRenderer::new(font, Default::default());
//!
//! // Read and render
//! let screen = reader.read_screen()?;
//! let dirty = renderer.render(&screen, display.framebuffer());
//! display.update_full(it8951::DisplayMode::Gc16)?;
//! # Ok::<(), paper_tty::Error>(())
//! ```

pub mod config;
pub mod display;
pub mod error;
pub mod font;
pub mod input;
pub mod renderer;
pub mod terminal;

#[cfg(feature = "vnc-support")]
pub mod vnc;

// Re-exports
pub use config::Config;
pub use display::EinkDisplay;
pub use error::{Error, Result};
pub use font::{BitmapFont, BuiltinFont, FontMetrics, FontRenderer, GlyphBitmap, TtfFont};
pub use renderer::{CursorStyle, TextRenderer};
pub use terminal::{Cell, ScreenBuffer, TerminalReader, VcsaReader};

// Re-export IT8951 types that users might need
pub use it8951::{Area, DisplayMode, Framebuffer, PixelFormat};
