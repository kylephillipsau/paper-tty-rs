//! Virtual keyboard input forwarding via zwp-virtual-keyboard-v1.
//!
//! Reads evdev events from the physical keyboard and forwards them
//! to the Sway compositor as virtual keyboard events.

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd};
use std::path::PathBuf;

use log::{info, warn};
use wayland_client::protocol::wl_seat;
use wayland_client::QueueHandle;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

use crate::error::{Error, Result};
use crate::input::find_keyboard_device;

/// Size of a Linux input_event struct
const INPUT_EVENT_SIZE: usize = std::mem::size_of::<InputEvent>();

/// Linux input_event structure
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    type_: u16,
    code: u16,
    value: i32,
}

const EV_KEY: u16 = 1;

// Modifier key evdev codes
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_LEFTALT: u16 = 56;
const KEY_RIGHTALT: u16 = 100;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;
const KEY_CAPSLOCK: u16 = 58;

// XKB modifier mask bits
const MOD_SHIFT: u32 = 1;
const MOD_LOCK: u32 = 2;
const MOD_CONTROL: u32 = 4;
const MOD_ALT: u32 = 8;    // Mod1
const MOD_SUPER: u32 = 64;  // Mod4

/// Returns the modifier bit for a given evdev keycode, or 0 if not a modifier.
fn modifier_bit(code: u16) -> u32 {
    match code {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => MOD_SHIFT,
        KEY_LEFTCTRL | KEY_RIGHTCTRL => MOD_CONTROL,
        KEY_LEFTALT | KEY_RIGHTALT => MOD_ALT,
        KEY_LEFTMETA | KEY_RIGHTMETA => MOD_SUPER,
        KEY_CAPSLOCK => MOD_LOCK,
        _ => 0,
    }
}

/// Generate a compiled XKB keymap using xkbcli on the system.
fn generate_keymap() -> Result<Vec<u8>> {
    // Try xkbcli first (available on most modern systems)
    let output = std::process::Command::new("xkbcli")
        .args(["compile-keymap", "--layout", "us"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();

    if let Ok(output) = output {
        if output.status.success() && !output.stdout.is_empty() {
            let mut keymap = output.stdout;
            // Ensure null-terminated
            if keymap.last() != Some(&0) {
                keymap.push(0);
            }
            info!("Generated {} byte keymap via xkbcli", keymap.len());
            return Ok(keymap);
        }
    }

    Err(Error::Wayland(
        "Failed to generate keymap: xkbcli not found or failed".into(),
    ))
}

/// Virtual keyboard that forwards evdev events to Wayland.
pub struct VirtualKeyboard {
    vk: ZwpVirtualKeyboardV1,
    evdev_file: File,
    /// Currently depressed modifier mask
    mods_depressed: u32,
    /// Locked modifier mask (e.g. caps lock)
    mods_locked: u32,
}

impl VirtualKeyboard {
    /// Create a virtual keyboard, binding to the compositor and opening the evdev device.
    pub fn new(
        manager: &ZwpVirtualKeyboardManagerV1,
        seat: &wl_seat::WlSeat,
        qh: &QueueHandle<super::screencopy::State>,
        keyboard_path: Option<PathBuf>,
    ) -> Result<Self> {
        let vk = manager.create_virtual_keyboard(seat, qh, ());

        // Generate a compiled keymap
        let keymap_bytes = generate_keymap()?;
        let keymap_size = keymap_bytes.len();

        // Create POSIX shared memory for the keymap
        let name = std::ffi::CStr::from_bytes_with_nul(b"/paper-tty-keymap\0").unwrap();
        let fd = rustix::shm::shm_open(
            name,
            rustix::shm::ShmOFlags::CREATE
                | rustix::shm::ShmOFlags::RDWR
                | rustix::shm::ShmOFlags::EXCL,
            rustix::shm::Mode::RUSR | rustix::shm::Mode::WUSR,
        )
        .or_else(|_| {
            let _ = rustix::shm::shm_unlink(name);
            rustix::shm::shm_open(
                name,
                rustix::shm::ShmOFlags::CREATE
                    | rustix::shm::ShmOFlags::RDWR
                    | rustix::shm::ShmOFlags::EXCL,
                rustix::shm::Mode::RUSR | rustix::shm::Mode::WUSR,
            )
        })
        .map_err(|e| Error::Wayland(format!("shm_open for keymap: {}", e)))?;
        let _ = rustix::shm::shm_unlink(name);

        rustix::fs::ftruncate(&fd, keymap_size as u64)
            .map_err(|e| Error::Wayland(format!("ftruncate keymap: {}", e)))?;

        // Write keymap data
        let written = unsafe {
            libc::write(
                fd.as_raw_fd(),
                keymap_bytes.as_ptr() as *const _,
                keymap_size,
            )
        };
        if written != keymap_size as isize {
            return Err(Error::Wayland("Failed to write keymap".into()));
        }
        unsafe {
            libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_SET);
        }

        // Send keymap to virtual keyboard (format 1 = XKB_V1)
        vk.keymap(1, fd.as_fd(), keymap_size as u32);

        // Open evdev device
        let device_path = keyboard_path
            .or_else(find_keyboard_device)
            .ok_or_else(|| Error::Wayland("No keyboard device found".into()))?;

        info!("Virtual keyboard using evdev device: {:?}", device_path);
        let evdev_file = File::open(&device_path)
            .map_err(|e| Error::Wayland(format!("Failed to open {}: {}", device_path.display(), e)))?;

        // Grab exclusive access so other consumers (TTY, etc.) don't also get events
        unsafe {
            // EVIOCGRAB = _IOW('E', 0x90, int) = 0x40044590
            let ret = libc::ioctl(evdev_file.as_raw_fd(), 0x40044590u64, 1);
            if ret < 0 {
                warn!("Failed to grab exclusive keyboard access (EVIOCGRAB)");
            }
        }

        // Set non-blocking
        unsafe {
            let flags = libc::fcntl(evdev_file.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(
                evdev_file.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            );
        }

        Ok(Self { vk, evdev_file, mods_depressed: 0, mods_locked: 0 })
    }

    /// Read and forward any pending evdev key events. Returns number of events forwarded.
    pub fn poll_and_forward(&mut self) -> usize {
        let mut buf = [0u8; INPUT_EVENT_SIZE * 64];
        let mut count = 0;

        loop {
            let n = unsafe {
                libc::read(
                    self.evdev_file.as_raw_fd(),
                    buf.as_mut_ptr() as *mut _,
                    buf.len(),
                )
            };

            if n <= 0 {
                break;
            }

            let n = n as usize;
            let num_events = n / INPUT_EVENT_SIZE;

            for i in 0..num_events {
                let event: InputEvent = unsafe {
                    std::ptr::read_unaligned(
                        buf[i * INPUT_EVENT_SIZE..].as_ptr() as *const InputEvent,
                    )
                };

                if event.type_ == EV_KEY {
                    let time_ms =
                        event.tv_sec as u32 * 1000 + (event.tv_usec / 1000) as u32;
                    // value: 0=release, 1=press, 2=repeat
                    self.vk
                        .key(time_ms, event.code as u32, event.value as u32);
                    count += 1;

                    // Update modifier state
                    let mod_bit = modifier_bit(event.code);
                    if mod_bit != 0 {
                        if event.code == KEY_CAPSLOCK && event.value == 1 {
                            // Toggle caps lock on press
                            self.mods_locked ^= MOD_LOCK;
                        } else if mod_bit != MOD_LOCK {
                            if event.value == 1 {
                                self.mods_depressed |= mod_bit;
                            } else if event.value == 0 {
                                self.mods_depressed &= !mod_bit;
                            }
                        }
                        self.vk.modifiers(
                            self.mods_depressed,
                            0, // latched
                            self.mods_locked,
                            0, // group
                        );
                    }
                }
            }
        }

        count
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        // Release exclusive grab
        unsafe {
            libc::ioctl(self.evdev_file.as_raw_fd(), 0x40044590u64, 0);
        }
        self.vk.destroy();
    }
}
