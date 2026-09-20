# Paper-TTY

A Rust application for driving IT8951-based e-ink displays on Linux. Supports both terminal emulation and full Wayland compositor display via Sway.

## Features

- **Terminal Mode**: PTY-based terminal emulator with shell spawning and evdev keyboard input
- **Sway Mode**: Capture and display a headless Sway Wayland compositor with native input via seatd
- **Optimised for E-ink**: Partial updates, multiple display modes, configurable refresh rates
- **Pipelined Rendering**: Frame capture overlaps with display updates for improved responsiveness

## Hardware Requirements

- Raspberry Pi (3/4/5)
- IT8951-based e-paper display (Waveshare 6", 7.8", 9.7", 10.3")
- SPI enabled on Raspberry Pi
- Keyboard (USB or ADB via adapter)

### Tested Hardware

| Display | Resolution | Controller |
|---------|------------|------------|
| Waveshare 9.7" e-Paper HAT | 1200x825 | IT8951 |

### Raspberry Pi Configuration

Enable SPI and configure for optimal performance in `/boot/firmware/config.txt`:

```ini
# Enable SPI
dtparam=spi=on

# Pin the GPU core clock. The SPI clock divider is computed for a 500 MHz core,
# but on a Pi 4 the core floats between ~200 and 500 MHz when the GPU is idle,
# which silently drops the "24 MHz" bus to 9-12 MHz (measured: 1.15 MB/s
# instead of 2.76 MB/s). Both lines are required.
core_freq=500
core_freq_min=500
```

Reboot after making changes and confirm with `vcgencmd measure_clock core`
(should read 500000000 constantly).

### SPI Speed

The driver uses two SPI speeds:
- **Commands**: 1 MHz (protocol overhead)
- **Data transfers**: 24 MHz (bulk pixel data)

This is a **24x improvement** over the default 1 MHz speed and significantly reduces display update times. The `core_freq=500` setting ensures the SPI clock divider produces a stable 24 MHz signal.

## Building

```bash
# Clone with submodules
git clone --recursive https://github.com/kylephillipsau/paper-tty-rs.git
cd paper-tty-rs

# Terminal mode only
cargo build --release

# With Sway/Wayland support
cargo build --release --features sway

# All features
cargo build --release --features full
```

### Cross-compiling for Raspberry Pi

```bash
# Install cross-compilation toolchain
rustup target add armv7-unknown-linux-gnueabihf

# Build
cargo build --release --target armv7-unknown-linux-gnueabihf --features sway
```

Or build directly on the Pi:

```bash
ssh pi "cd ~/paper-tty-rs && cargo build --release --features sway"
```

## Configuration

Create `/etc/paper-tty.conf`:

```bash
DISPLAY_MODE=gl16
MARGIN_LEFT=80
MARGIN_RIGHT=80
MARGIN_TOP=20
MARGIN_BOTTOM=20
THEME=--light
REFRESH=--partial
FONT_SIZE=16
REFRESH_RATE=250
CURSOR=block
```

## Terminal Mode

Spawns a PTY shell and renders it to the e-ink display.

```bash
# Basic usage
sudo paper-tty terminal

# With options
sudo paper-tty terminal \
    --shell /bin/bash \
    --size 16 \
    --cursor block \
    --light \
    --partial \
    --mode gl16 \
    --margin-left 80 \
    --margin-right 80 \
    --margin-top 20 \
    --margin-bottom 20
```

### Terminal Options

| Option | Description |
|--------|-------------|
| `--shell` | Shell to spawn (default: $SHELL or /bin/sh) |
| `--size` | Font size in points |
| `--cursor` | Cursor style: block, underline, bar, none |
| `--light` | Light theme (white background) |
| `--partial` | Enable partial display updates |
| `--mode` | Display mode: du, gc16, gl16, a2 |
| `--refresh-rate` | Refresh interval in milliseconds |
| `--margin-*` | Display margins in pixels |

## Sway Mode

Captures frames from a headless Sway compositor and displays them on the e-ink screen. Input devices (keyboard, mouse) are handled natively by Sway via seatd/libseat.

### Architecture

```
Keyboard/Mouse → libinput → seatd → Sway (headless)
Sway → wlr-screencopy → paper-tty → grayscale conversion → SPI → IT8951
```

Paper-tty is display-only in Sway mode. Sway handles all input devices directly through seatd, providing proper mouse support and avoiding input duplication issues.

### seatd Setup

Sway requires seatd to access input devices. Install and configure seatd:

```bash
# Install seatd
sudo apt install seatd

# Enable and start seatd
sudo systemctl enable seatd
sudo systemctl start seatd
```

**Group membership**: seatd grants access to users in a specific group. Check which group your seatd uses:

```bash
# Check seatd configuration
grep ExecStart /lib/systemd/system/seatd.service
# Output: ExecStart=seatd -g video  (or -g seat)
```

Add your user to the appropriate group:

```bash
# For Debian/Raspberry Pi OS (uses video group)
sudo usermod -aG video $USER

# For other distros that use seat group
sudo usermod -aG seat $USER

# Log out and back in for group membership to take effect
```

### Running Manually

```bash
# Start Sway headless
WLR_BACKENDS=headless,libinput sway -c /etc/paper-tty/sway-eink.conf &

# Run paper-tty sway capture
WAYLAND_DISPLAY=wayland-1 paper-tty sway \
    --mode du \
    --cleanup-mode gl16 \
    --cleanup-delay 1000 \
    --margin-left 80 \
    --margin-right 80 \
    --margin-top 20 \
    --margin-bottom 20
```

### Sway Configuration

Create `/etc/paper-tty/sway-eink.conf`:

```
output HEADLESS-1 resolution 800x600
default_border pixel 2
gaps inner 0
gaps outer 0
output * bg #ffffff solid_color

set $mod Mod4

bindsym $mod+Return exec foot
bindsym $mod+Shift+q kill
bindsym $mod+1 workspace number 1
bindsym $mod+2 workspace number 2
# ... additional bindings

exec waybar
exec foot
```

## Systemd Services

### User Services (Recommended)

Sway must run as a user service so the Wayland socket is created in the correct `XDG_RUNTIME_DIR`.

**Enable user lingering** (required for boot-time user services):

```bash
sudo loginctl enable-linger $USER
```

**`~/.config/systemd/user/sway-eink.service`**:

```ini
[Unit]
Description=Sway Headless for E-ink Display
Requires=seatd.service
After=seatd.service

[Service]
Type=simple
Environment=WLR_BACKENDS=headless,libinput
ExecStart=/usr/bin/sway -c /etc/paper-tty/sway-eink.conf
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Notes:
- `WLR_BACKENDS` must list `libinput` explicitly. When the variable is set, wlroots
  creates only the named backends, and `headless` alone means Sway never opens any
  input device (`swaymsg -t get_inputs` prints `[]`).
- Do not set `WLR_LIBINPUT_NO_DEVICES=1`. With wlroots 0.15 it skips the initial
  libinput dispatch, so devices present at startup are not registered until the first
  key press. On a Raspberry Pi the HDMI CEC inputs always exist, so Sway starts fine
  without it even with no keyboard attached.

**`~/.config/systemd/user/paper-tty-sway.service`**:

```ini
[Unit]
Description=Paper-TTY Sway E-ink Capture
After=sway-eink.service
BindsTo=sway-eink.service

[Service]
Type=simple
ExecStartPre=/bin/sleep 3
ExecStart=/home/USER/paper-tty-sway.sh
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

**Wrapper script** (`~/paper-tty-sway.sh`):

```bash
#!/bin/bash
source /etc/paper-tty.conf

for s in wayland-0 wayland-1; do
    if [ -S "/run/user/$(id -u)/$s" ]; then
        export WAYLAND_DISPLAY="$s"
        break
    fi
done

exec ~/paper-tty-rs/target/release/paper-tty -v sway \
    --mode "${DISPLAY_MODE}" \
    --cleanup-mode "${CLEANUP_MODE:-gl16}" \
    --cleanup-delay "${CLEANUP_DELAY:-1000}" \
    --margin-left "${MARGIN_LEFT}" \
    --margin-right "${MARGIN_RIGHT}" \
    --margin-top "${MARGIN_TOP}" \
    --margin-bottom "${MARGIN_BOTTOM}"
```

**Enable services**:

```bash
systemctl --user daemon-reload
systemctl --user enable sway-eink.service paper-tty-sway.service
systemctl --user start sway-eink.service paper-tty-sway.service
```

## Display Modes

Measured on a 9.7" panel (M841 firmware). Waveform time is a fixed cost per update
and barely depends on the area; the IT8951 blocks the host for the whole waveform
and runs updates one at a time.

| Mode | Levels | Waveform | Flash | Use Case |
|------|--------|----------|-------|----------|
| `a2` | 2 | ~160 ms | no | cursor, animation (most ghosting) |
| `du` | 2 | ~200 ms | no | typing, scrolling — default interactive mode |
| `du4` | 4 | ~330 ms | no | anti-aliased text without dithering |
| `gl16` | 16 | ~500 ms | little | cleanup of DU ghosting — default cleanup mode |
| `gc16` | 16 | ~500 ms | full white | images, periodic full refresh |

### Sway Options

| Option | Default | Description |
|--------|---------|-------------|
| `--mode` | `du` | Mode for interactive updates |
| `--cleanup-mode` | `gl16` | Mode for the cleanup pass that removes DU ghosting |
| `--cleanup-delay` | `1000` | ms the screen must be still before a cleanup pass |
| `--full-refresh-interval` | `10` | Full-viewport GC16 every N cleanup passes (0 = never) |
| `--threshold` | `200` | Luminance at/above which a pixel is white in mono modes. 200 keeps coloured text visible on a light theme; use ~100 for dark themes |
| `--dither` | off | 4x4 ordered dither instead of thresholding for mono / 4-level modes |
| `--bpp` | `4` | Bits per pixel sent for interactive updates (8, 4, 2). 4bpp is byte-exact (verified by memory read-back); 2bpp is 4x less data than 8bpp but this firmware stores 2bpp "white" as grey level 12/15 |
| `--cleanup-bpp` | `4` | Bits per pixel sent for cleanup updates (8, 4) |
| `--spi-hz` | config (`32000000`) | SPI clock for pixel data. Above 24 MHz a read-back self-test runs at start-up and falls back to 24 MHz on any mismatch |
| `--hide-cursor` | off | Do not composite the pointer (each pointer move otherwise costs a waveform) |
| `--small-mode` | unset | Mode for small changes (pointer moves, single keystrokes), e.g. `a2`: ~155 ms per update instead of ~190 ms with `du`, with lighter blacks and more ghosting until the cleanup pass |
| `--small-max-pixels` | `6000` | Aligned bounding-box size up to which a change counts as small |
| `--frame-interval` | `0` | Minimum ms between updates (0 = as fast as the panel allows) |

## Performance

### SPI Optimisation

The IT8951 communicates via SPI. By default, many implementations use 1 MHz which is very slow. This driver uses:
- **1 MHz** for commands (required for reliable protocol communication)
- **32 MHz** for data transfers by default (`display.spi_hz` in the config file or `--spi-hz`)

The IT8951 datasheet specifies 24 MHz (~2.76 MB/s measured). Above that, `paper-tty`
loads a 60 KB pseudo-random pattern at start-up and reads it back through the memory
burst-read command; any mismatch drops the clock back to 24 MHz. On a Pi 4 with the
Waveshare 9.7" HAT, 32, 40 and 48 MHz all read back byte-exact over 1.4 MB each
(3.7 / 4.2 / 4.85 MB/s).

### Pixel formats

Sub-byte formats put the first pixel in the **low** nibble / low bits of each byte,
consistent with the 8bpp little-endian word layout (first pixel in the low byte). This was
verified by reading the image buffer back after 8bpp, 4bpp and 2bpp loads of the same
pattern; the earlier "diagonal/fuzzy" 4bpp attempt had the nibbles swapped. The buffer
itself is 8bpp, byte-addressed, stride = panel width. 2bpp levels are stored as
0x00/0x40/0x80/0xC0, so 2bpp white is grey level 12, not 15.

`core_freq=500` **and** `core_freq_min=500` must be set in `/boot/firmware/config.txt`
(see Raspberry Pi Configuration); without them the bus actually runs at 9-12 MHz.

### Update Pipeline (Sway mode)

The capture loop is driven by compositor damage (`copy_with_damage`) rather than
polling, and sends one merged update per frame:

1. wait for the panel to finish the previous waveform (sleeping, not spinning)
2. ask Sway for the next frame; this blocks until something on the output changed
3. convert and diff only the damaged rectangle, in the quantised domain of the
   interactive mode (so anti-aliasing jitter does not cause updates)
4. send the aligned bounding box of real changes with the interactive mode (DU)
5. once the screen has been still for `--cleanup-delay`, re-render everything that
   was updated in a mono mode with `--cleanup-mode` (GL16) to remove ghosting

Measured per-update cost with DU: a keystroke is ~13 ms of host time plus one ~190 ms
waveform; a 280x740 scroll region is ~41 ms of SPI at 4bpp / 32 MHz (was 124 ms at
8bpp / 24 MHz) plus one waveform, with no flash.

## Other Commands

```bash
# Clear display to white
paper-tty clear

# Clear to gray
paper-tty clear --gray 128

# Show display info
paper-tty info

# Run test pattern
paper-tty test
```

## Project Structure

```
paper-tty-rs/
├── drivers/it8951/      # IT8951 Rust driver (submodule)
│   └── src/
│       ├── device/      # Device init, VCOM, power management
│       ├── display/     # Refresh, image loading
│       ├── graphics/    # Framebuffer, drawing primitives
│       ├── hal/         # SPI + GPIO abstraction
│       └── protocol/    # IT8951 command protocol
├── src/
│   ├── main.rs          # CLI entry point
│   ├── display/         # EinkDisplay wrapper, viewport
│   ├── terminal/        # PTY terminal emulator
│   ├── input/           # Keyboard detection (terminal mode)
│   └── wayland/         # Sway screencopy capture (display-only)
└── Cargo.toml
```

## Troubleshooting

### Display not updating
- Check SPI is enabled: `ls /dev/spidev*`
- Verify GPIO permissions or run as root
- Check IT8951 is powered and connected

### Wayland connection failed
- Ensure Sway is running: `pgrep sway`
- Check `WAYLAND_DISPLAY` is set correctly
- Verify socket exists: `ls /run/user/$(id -u)/wayland-*`

### Keyboard/mouse not working in Sway mode
- `swaymsg -t get_inputs` prints `[]`: make sure `WLR_BACKENDS=headless,libinput`
  (not just `headless`) and `WLR_LIBINPUT_NO_DEVICES` is unset — see the unit notes above
- Verify seatd is running: `systemctl status seatd`
- Check user is in seat group: `groups $USER`
- Ensure you logged out and back in after adding to seat group
- Check Sway logs for input device errors: `journalctl --user -u sway-eink`

### Diagonal artifacts on partial updates
- IT8951 requires 4-pixel aligned widths (handled automatically)
- If persists, try full refresh: `paper-tty clear`

## License

MIT License

## Acknowledgments

- [PaperTTY](https://github.com/joukos/PaperTTY) - Original Python implementation
- [Modos Glider](https://github.com/Modos-Labs/Glider) - E-ink performance research
- [IT8951](https://github.com/kylephillipsau/IT8951) - Rust driver
