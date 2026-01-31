//! Wayland client for capturing Sway compositor output via wlr-screencopy.
//!
//! This module is display-only. Input handling is delegated to Sway via seatd/libseat.

pub mod convert;
pub mod screencopy;
pub mod shm;
