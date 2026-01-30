//! Error types for paper-tty

use thiserror::Error;

/// Result type alias for paper-tty operations
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for paper-tty
#[derive(Error, Debug)]
pub enum Error {
    /// Display driver error
    #[error("Display error: {0}")]
    Display(String),

    /// IT8951 driver error
    #[error("IT8951 error: {0}")]
    It8951(#[from] it8951::Error),

    /// Font loading or rendering error
    #[error("Font error: {0}")]
    Font(String),

    /// Terminal reading error
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// VNC connection error
    #[error("VNC error: {0}")]
    Vnc(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Image processing error
    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    /// Invalid parameter
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Wayland error
    #[error("Wayland error: {0}")]
    Wayland(String),

    /// Feature not available
    #[error("Feature not available: {0}")]
    NotAvailable(String),

    /// Timeout error
    #[error("Operation timed out after {0}ms")]
    Timeout(u64),
}
