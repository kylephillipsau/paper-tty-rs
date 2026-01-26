//! Keyboard input handling for paper-tty
//!
//! This module provides direct keyboard input reading from Linux evdev devices,
//! which works even when running as a background service without stdin.

mod keyboard;

pub use keyboard::{KeyboardReader, find_keyboard_device};
