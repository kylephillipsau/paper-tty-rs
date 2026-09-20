//! Configuration management for paper-tty

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::{Error, Result};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: DisplayConfig,
    pub terminal: TerminalConfig,
    pub font: FontConfig,
    pub cursor: CursorConfig,
    pub vnc: VncConfig,
    pub colors: ColorConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            display: DisplayConfig::default(),
            terminal: TerminalConfig::default(),
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            vnc: VncConfig::default(),
            colors: ColorConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        toml::from_str(&content).map_err(|e| Error::Config(e.to_string()))
    }

    /// Save configuration to a TOML file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load from default config path (~/.config/paper-tty/config.toml)
    pub fn load_default() -> Result<Self> {
        if let Some(config_dir) = dirs::config_dir() {
            let config_path = config_dir.join("paper-tty").join("config.toml");
            if config_path.exists() {
                return Self::load(config_path);
            }
        }
        Ok(Self::default())
    }
}

/// Display configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Display driver (currently only "IT8951" supported)
    pub driver: String,
    /// Display rotation in degrees (0, 90, 180, 270)
    pub rotation: u16,
    /// VCOM voltage (display-specific, check your panel)
    pub vcom: u16,
    /// SPI device path
    pub spi_device: String,
    /// SPI clock for pixel data in Hz. The IT8951 datasheet says 24 MHz; higher values
    /// are verified with a read-back test at start-up and fall back to 24 MHz on failure.
    pub spi_hz: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            driver: "IT8951".to_string(),
            rotation: 0,
            vcom: 1500,
            spi_device: "/dev/spidev0.0".to_string(),
            spi_hz: 32_000_000,
        }
    }
}

/// Terminal rendering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// TTY number to render (1 = /dev/tty1, /dev/vcsa1)
    pub tty: u8,
    /// Use VCSA interface (faster) vs TTY
    pub use_vcsa: bool,
    /// Refresh rate in milliseconds
    pub refresh_rate_ms: u32,
    /// Enable partial refresh for changed areas
    pub partial_refresh: bool,
    /// Full refresh every N partial refreshes (to reduce ghosting)
    pub full_refresh_interval: u32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            tty: 1,
            use_vcsa: true,
            refresh_rate_ms: 250,
            partial_refresh: true,
            full_refresh_interval: 30,
        }
    }
}

/// Font configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Path to TTF font file
    pub path: Option<String>,
    /// Font size in pixels
    pub size: u16,
    /// Additional line spacing
    pub line_spacing: u16,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            path: None, // Will use system default or bundled font
            size: 16,
            line_spacing: 0,
        }
    }
}

/// Cursor rendering configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    /// Cursor style: "block", "underline", "bar", "none"
    pub style: String,
    /// Enable cursor blinking (note: may cause frequent refreshes)
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: "block".to_string(),
            blink: false,
        }
    }
}

/// VNC client configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VncConfig {
    /// VNC server hostname
    pub host: String,
    /// VNC server port
    pub port: u16,
    /// VNC password (optional)
    pub password: Option<String>,
    /// Dithering mode: "none", "floyd-steinberg", "atkinson"
    pub dither: String,
}

impl Default for VncConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5900,
            password: None,
            dither: "floyd-steinberg".to_string(),
        }
    }
}

/// Terminal color to grayscale mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    pub black: u8,
    pub red: u8,
    pub green: u8,
    pub yellow: u8,
    pub blue: u8,
    pub magenta: u8,
    pub cyan: u8,
    pub white: u8,
    // Bright variants
    pub bright_black: u8,
    pub bright_red: u8,
    pub bright_green: u8,
    pub bright_yellow: u8,
    pub bright_blue: u8,
    pub bright_magenta: u8,
    pub bright_cyan: u8,
    pub bright_white: u8,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self::dark()
    }
}

impl ColorConfig {
    /// Dark theme: white/light text on black/dark background
    pub fn dark() -> Self {
        Self {
            black: 0,
            red: 64,
            green: 128,
            yellow: 160,
            blue: 96,
            magenta: 80,
            cyan: 144,
            white: 220,
            bright_black: 48,
            bright_red: 96,
            bright_green: 160,
            bright_yellow: 192,
            bright_blue: 128,
            bright_magenta: 112,
            bright_cyan: 176,
            bright_white: 255,
        }
    }

    /// Light theme: black/dark text on white/light background
    /// Swaps the black/white colors for better e-ink readability
    pub fn light() -> Self {
        Self {
            // Swap black and white for light theme
            black: 255,         // "Black" text is actually white background
            white: 0,           // "White" text is actually black
            bright_black: 220,  // Dark gray
            bright_white: 32,   // Near black
            // Adjust other colors for light background readability
            red: 48,
            green: 64,
            yellow: 80,
            blue: 48,
            magenta: 56,
            cyan: 72,
            bright_red: 32,
            bright_green: 48,
            bright_yellow: 64,
            bright_blue: 32,
            bright_magenta: 40,
            bright_cyan: 56,
        }
    }

    /// Map ANSI color code (0-15) to grayscale value
    pub fn ansi_to_gray(&self, color: u8) -> u8 {
        match color {
            0 => self.black,
            1 => self.red,
            2 => self.green,
            3 => self.yellow,
            4 => self.blue,
            5 => self.magenta,
            6 => self.cyan,
            7 => self.white,
            8 => self.bright_black,
            9 => self.bright_red,
            10 => self.bright_green,
            11 => self.bright_yellow,
            12 => self.bright_blue,
            13 => self.bright_magenta,
            14 => self.bright_cyan,
            15 => self.bright_white,
            // Extended colors (16-255) - simplified grayscale mapping
            16..=231 => {
                // Color cube: 6x6x6 = 216 colors
                let c = color - 16;
                let r = (c / 36) * 51;
                let g = ((c / 6) % 6) * 51;
                let b = (c % 6) * 51;
                // Convert to grayscale using luminance formula
                ((r as u16 * 30 + g as u16 * 59 + b as u16 * 11) / 100) as u8
            }
            232..=255 => {
                // Grayscale ramp: 24 shades
                let gray = (color - 232) * 10 + 8;
                gray
            }
        }
    }
}
