//! Keyboard input via Linux evdev
//!
//! Reads keyboard events directly from /dev/input/eventX devices,
//! converting them to terminal escape sequences.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use evdev::{Device, InputEventKind, Key};

use crate::error::{Error, Result};

/// Find a keyboard device in /dev/input/
pub fn find_keyboard_device() -> Option<PathBuf> {
    let input_dir = PathBuf::from("/dev/input");

    // Try to find a device that has keyboard capabilities
    if let Ok(entries) = fs::read_dir(&input_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.to_string_lossy().contains("event") {
                if let Ok(device) = Device::open(&path) {
                    // Check if this device has key events (keyboard)
                    if device.supported_keys().map_or(false, |keys| {
                        keys.contains(Key::KEY_A) && keys.contains(Key::KEY_ENTER)
                    }) {
                        log::info!("Found keyboard device: {:?} ({})",
                            path,
                            device.name().unwrap_or("unknown"));
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

/// Keyboard reader that reads from evdev and converts to terminal sequences
pub struct KeyboardReader {
    receiver: mpsc::Receiver<Vec<u8>>,
    _thread: thread::JoinHandle<()>,
}

impl KeyboardReader {
    /// Create a new keyboard reader from the specified device path
    pub fn new(device_path: &PathBuf) -> Result<Self> {
        let device = Device::open(device_path)
            .map_err(|e| Error::Terminal(format!("Failed to open keyboard device: {}", e)))?;

        let name = device.name().unwrap_or("unknown").to_string();
        log::info!("Opened keyboard: {}", name);

        // Grab the device exclusively so keys don't go to other programs
        // This is optional - comment out if you want keys to go to both
        // device.grab().map_err(|e| Error::Terminal(format!("Failed to grab keyboard: {}", e)))?;

        let (tx, rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            Self::read_loop(device, tx);
        });

        Ok(Self {
            receiver: rx,
            _thread: thread,
        })
    }

    /// Try to receive pending keyboard input (non-blocking)
    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.receiver.try_recv().ok()
    }

    /// Wait for keyboard input up to the given timeout.
    /// Returns the input data if a key arrived, or None on timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        self.receiver.recv_timeout(timeout).ok()
    }

    /// Main event reading loop
    fn read_loop(mut device: Device, tx: mpsc::Sender<Vec<u8>>) {
        let mut shift_pressed = false;
        let mut ctrl_pressed = false;
        let mut alt_pressed = false;

        loop {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if let InputEventKind::Key(key) = event.kind() {
                            let pressed = event.value() == 1; // 1 = pressed, 0 = released, 2 = repeat
                            let repeat = event.value() == 2;

                            // Track modifier keys
                            match key {
                                Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT => {
                                    shift_pressed = pressed || repeat;
                                    continue;
                                }
                                Key::KEY_LEFTCTRL | Key::KEY_RIGHTCTRL => {
                                    ctrl_pressed = pressed || repeat;
                                    continue;
                                }
                                Key::KEY_LEFTALT | Key::KEY_RIGHTALT => {
                                    alt_pressed = pressed || repeat;
                                    continue;
                                }
                                _ => {}
                            }

                            // Only process key presses and repeats
                            if !pressed && !repeat {
                                continue;
                            }

                            // Convert key to terminal sequence
                            if let Some(seq) = key_to_sequence(key, shift_pressed, ctrl_pressed, alt_pressed) {
                                if tx.send(seq).is_err() {
                                    return; // Channel closed
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("Keyboard read error: {}", e);
                    return;
                }
            }
        }
    }
}

/// Convert an evdev key to a terminal escape sequence
fn key_to_sequence(key: Key, shift: bool, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    // Handle control key combinations
    if ctrl {
        let ctrl_char = match key {
            Key::KEY_A => Some(1),   // Ctrl+A
            Key::KEY_B => Some(2),   // Ctrl+B
            Key::KEY_C => Some(3),   // Ctrl+C (SIGINT)
            Key::KEY_D => Some(4),   // Ctrl+D (EOF)
            Key::KEY_E => Some(5),   // Ctrl+E
            Key::KEY_F => Some(6),   // Ctrl+F
            Key::KEY_G => Some(7),   // Ctrl+G (Bell)
            Key::KEY_H => Some(8),   // Ctrl+H (Backspace)
            Key::KEY_I => Some(9),   // Ctrl+I (Tab)
            Key::KEY_J => Some(10),  // Ctrl+J (Newline)
            Key::KEY_K => Some(11),  // Ctrl+K
            Key::KEY_L => Some(12),  // Ctrl+L (Clear)
            Key::KEY_M => Some(13),  // Ctrl+M (Enter)
            Key::KEY_N => Some(14),  // Ctrl+N
            Key::KEY_O => Some(15),  // Ctrl+O
            Key::KEY_P => Some(16),  // Ctrl+P
            Key::KEY_Q => Some(17),  // Ctrl+Q (XON)
            Key::KEY_R => Some(18),  // Ctrl+R
            Key::KEY_S => Some(19),  // Ctrl+S (XOFF)
            Key::KEY_T => Some(20),  // Ctrl+T
            Key::KEY_U => Some(21),  // Ctrl+U
            Key::KEY_V => Some(22),  // Ctrl+V
            Key::KEY_W => Some(23),  // Ctrl+W
            Key::KEY_X => Some(24),  // Ctrl+X
            Key::KEY_Y => Some(25),  // Ctrl+Y
            Key::KEY_Z => Some(26),  // Ctrl+Z (SIGTSTP)
            Key::KEY_LEFTBRACE => Some(27),  // Ctrl+[ (Escape)
            Key::KEY_BACKSLASH => Some(28),  // Ctrl+\
            Key::KEY_RIGHTBRACE => Some(29), // Ctrl+]
            _ => None,
        };

        if let Some(c) = ctrl_char {
            return Some(vec![c]);
        }
    }

    // Handle special keys with escape sequences
    let escape_seq = match key {
        // Arrow keys
        Key::KEY_UP => Some(b"\x1b[A".to_vec()),
        Key::KEY_DOWN => Some(b"\x1b[B".to_vec()),
        Key::KEY_RIGHT => Some(b"\x1b[C".to_vec()),
        Key::KEY_LEFT => Some(b"\x1b[D".to_vec()),

        // Navigation keys
        Key::KEY_HOME => Some(b"\x1b[H".to_vec()),
        Key::KEY_END => Some(b"\x1b[F".to_vec()),
        Key::KEY_PAGEUP => Some(b"\x1b[5~".to_vec()),
        Key::KEY_PAGEDOWN => Some(b"\x1b[6~".to_vec()),
        Key::KEY_INSERT => Some(b"\x1b[2~".to_vec()),
        Key::KEY_DELETE => Some(b"\x1b[3~".to_vec()),

        // Function keys
        Key::KEY_F1 => Some(b"\x1bOP".to_vec()),
        Key::KEY_F2 => Some(b"\x1bOQ".to_vec()),
        Key::KEY_F3 => Some(b"\x1bOR".to_vec()),
        Key::KEY_F4 => Some(b"\x1bOS".to_vec()),
        Key::KEY_F5 => Some(b"\x1b[15~".to_vec()),
        Key::KEY_F6 => Some(b"\x1b[17~".to_vec()),
        Key::KEY_F7 => Some(b"\x1b[18~".to_vec()),
        Key::KEY_F8 => Some(b"\x1b[19~".to_vec()),
        Key::KEY_F9 => Some(b"\x1b[20~".to_vec()),
        Key::KEY_F10 => Some(b"\x1b[21~".to_vec()),
        Key::KEY_F11 => Some(b"\x1b[23~".to_vec()),
        Key::KEY_F12 => Some(b"\x1b[24~".to_vec()),

        // Control keys
        Key::KEY_ENTER => Some(b"\r".to_vec()),
        Key::KEY_TAB => Some(b"\t".to_vec()),
        Key::KEY_BACKSPACE => Some(b"\x7f".to_vec()),
        Key::KEY_ESC => Some(b"\x1b".to_vec()),
        Key::KEY_SPACE => Some(b" ".to_vec()),

        _ => None,
    };

    if let Some(seq) = escape_seq {
        // Prepend escape for Alt combinations
        if alt {
            let mut with_alt = vec![0x1b];
            with_alt.extend(seq);
            return Some(with_alt);
        }
        return Some(seq);
    }

    // Handle regular characters
    let ch = key_to_char(key, shift)?;

    if alt {
        Some(vec![0x1b, ch as u8])
    } else {
        Some(vec![ch as u8])
    }
}

/// Convert a key to its ASCII character representation
fn key_to_char(key: Key, shift: bool) -> Option<char> {
    let (normal, shifted) = match key {
        // Letters
        Key::KEY_A => ('a', 'A'),
        Key::KEY_B => ('b', 'B'),
        Key::KEY_C => ('c', 'C'),
        Key::KEY_D => ('d', 'D'),
        Key::KEY_E => ('e', 'E'),
        Key::KEY_F => ('f', 'F'),
        Key::KEY_G => ('g', 'G'),
        Key::KEY_H => ('h', 'H'),
        Key::KEY_I => ('i', 'I'),
        Key::KEY_J => ('j', 'J'),
        Key::KEY_K => ('k', 'K'),
        Key::KEY_L => ('l', 'L'),
        Key::KEY_M => ('m', 'M'),
        Key::KEY_N => ('n', 'N'),
        Key::KEY_O => ('o', 'O'),
        Key::KEY_P => ('p', 'P'),
        Key::KEY_Q => ('q', 'Q'),
        Key::KEY_R => ('r', 'R'),
        Key::KEY_S => ('s', 'S'),
        Key::KEY_T => ('t', 'T'),
        Key::KEY_U => ('u', 'U'),
        Key::KEY_V => ('v', 'V'),
        Key::KEY_W => ('w', 'W'),
        Key::KEY_X => ('x', 'X'),
        Key::KEY_Y => ('y', 'Y'),
        Key::KEY_Z => ('z', 'Z'),

        // Numbers
        Key::KEY_1 => ('1', '!'),
        Key::KEY_2 => ('2', '@'),
        Key::KEY_3 => ('3', '#'),
        Key::KEY_4 => ('4', '$'),
        Key::KEY_5 => ('5', '%'),
        Key::KEY_6 => ('6', '^'),
        Key::KEY_7 => ('7', '&'),
        Key::KEY_8 => ('8', '*'),
        Key::KEY_9 => ('9', '('),
        Key::KEY_0 => ('0', ')'),

        // Symbols
        Key::KEY_MINUS => ('-', '_'),
        Key::KEY_EQUAL => ('=', '+'),
        Key::KEY_LEFTBRACE => ('[', '{'),
        Key::KEY_RIGHTBRACE => (']', '}'),
        Key::KEY_BACKSLASH => ('\\', '|'),
        Key::KEY_SEMICOLON => (';', ':'),
        Key::KEY_APOSTROPHE => ('\'', '"'),
        Key::KEY_GRAVE => ('`', '~'),
        Key::KEY_COMMA => (',', '<'),
        Key::KEY_DOT => ('.', '>'),
        Key::KEY_SLASH => ('/', '?'),

        _ => return None,
    };

    Some(if shift { shifted } else { normal })
}
