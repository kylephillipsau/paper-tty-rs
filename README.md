# Paper-TTY

A Rust application for driving IT8951-based e-ink displays on Linux. Supports both terminal emulation and full Wayland compositor display via Sway.

## Features

- **Terminal Mode**: PTY-based terminal emulator with shell spawning
- **Sway Mode**: Capture and display a headless Sway Wayland compositor
- **Optimised for E-ink**: Partial updates, multiple display modes, configurable refresh rates
- **Keyboard Input**: evdev-based input with modifier key support
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

# Set core frequency for stable SPI clock at high speeds
# Required for 24 MHz SPI data transfers
core_freq=500
core_freq_min=500
```

Reboot after making changes.

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

Captures frames from a headless Sway compositor and displays them on the e-ink screen. Keyboard input is forwarded via the Wayland virtual keyboard protocol.

### Architecture

```
Keyboard → evdev → paper-tty (EVIOCGRAB) → virtual keyboard → Sway
Sway → wlr-screencopy → paper-tty → grayscale conversion → SPI → IT8951
```

### Running Manually

```bash
# Start Sway headless
WLR_BACKENDS=headless sway -c /etc/paper-tty/sway-eink.conf &

# Run paper-tty sway capture
WAYLAND_DISPLAY=wayland-1 paper-tty sway \
    --mode gl16 \
    --frame-interval 200 \
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

[Service]
Type=simple
Environment=WLR_BACKENDS=headless
ExecStart=/usr/bin/sway -c /etc/paper-tty/sway-eink.conf
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

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
    --frame-interval 500 \
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

| Mode | Description | Speed | Quality | Use Case |
|------|-------------|-------|---------|----------|
| `du` | Direct Update | Fast | 1-bit | Typing, cursor |
| `gc16` | Grayscale Clearing | Slow | 16-level | Initial render, images |
| `gl16` | Grayscale Level | Medium | 16-level | General use |
| `a2` | Animation | Fastest | 1-bit | Rapid updates |

## Performance

### SPI Optimisation

The IT8951 communicates via SPI. By default, many implementations use 1 MHz which is very slow. This driver uses:
- **1 MHz** for commands (required for reliable protocol communication)
- **24 MHz** for data transfers (pixel data)

Ensure `core_freq=500` is set in `/boot/firmware/config.txt` for stable high-speed SPI.

### Pipelined Rendering

The capture loop is pipelined: frame capture happens in parallel with display updates. While the IT8951 is driving pixels for frame N, frame N+1 is already being captured.

- **Frame capture**: ~10-50ms depending on compositor
- **Partial update (GL16)**: ~500ms for small regions
- **Full update (GC16)**: ~2-4s for high-quality refresh

If the display is still busy when new changes are ready, intermediate frames are skipped to maintain input responsiveness.

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
│   ├── input/           # Keyboard detection
│   └── wayland/         # Sway capture (screencopy, input)
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

### Doubled keyboard input
- EVIOCGRAB should prevent this; check no other process reads the evdev device
- Verify exclusive grab succeeded in logs

### Diagonal artifacts on partial updates
- IT8951 requires 4-pixel aligned widths (handled automatically)
- If persists, try full refresh: `paper-tty clear`

## License

MIT License

## Acknowledgments

- [PaperTTY](https://github.com/joukos/PaperTTY) - Original Python implementation
- [Modos Glider](https://github.com/Modos-Labs/Glider) - E-ink performance research
- [IT8951](https://github.com/kylephillipsau/IT8951) - Rust driver
