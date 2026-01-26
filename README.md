# Paper-TTY

A Rust implementation of an e-ink terminal emulator for IT8951-based displays, inspired by [PaperTTY](https://github.com/joukos/PaperTTY).

## Features

- **Terminal Rendering**: Display Linux virtual console output on e-ink
- **Font Support**: TrueType fonts via fontdue
- **Partial Refresh**: Fast updates for changed screen regions
- **Grayscale Mapping**: Configurable terminal color to grayscale conversion
- **Multiple Cursor Styles**: Block, underline, bar, or no cursor
- **VNC Support** (planned): Connect to VNC servers for graphical content

## Requirements

- Raspberry Pi (3/4/5 recommended)
- IT8951-based e-paper display (6", 7.8", 9.7", 10.3")
- SPI enabled on Raspberry Pi
- Linux with virtual console access

## Installation

```bash
# Clone the repository
git clone --recursive https://github.com/yourusername/paper-tty-rs.git
cd paper-tty-rs

# Build
cargo build --release

# Install (optional)
cargo install --path .
```

## Usage

### Terminal Mode

```bash
# Render TTY1 to the display (requires root for VCSA access)
sudo paper-tty terminal --tty 1 --vcsa --font /path/to/font.ttf

# With partial refresh for faster updates
sudo paper-tty terminal --tty 1 --vcsa --partial

# Custom font size
sudo paper-tty terminal --tty 1 --vcsa --size 14
```

### Other Commands

```bash
# Clear the display
paper-tty clear

# Display information
paper-tty info

# Run display test
paper-tty test
```

### Options

```
paper-tty [OPTIONS] <COMMAND>

Commands:
  terminal    Render a Linux terminal to the display
  clear       Clear the display
  test        Run display tests
  info        Show display information

Options:
  -r, --rotate <DEGREES>  Rotation (0, 90, 180, 270) [default: 0]
  -v, --verbose           Enable verbose logging
  -c, --config <FILE>     Config file path
  -h, --help              Print help
  -V, --version           Print version
```

## Configuration

Create `~/.config/paper-tty/config.toml`:

```toml
[display]
driver = "IT8951"
rotation = 0
vcom = 1500

[terminal]
tty = 1
use_vcsa = true
refresh_rate_ms = 250
partial_refresh = true

[font]
path = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
size = 16

[cursor]
style = "block"
blink = false

[colors]
black = 0
white = 255
```

## Architecture

```
paper-tty-rs/
├── drivers/it8951/      # IT8951 Rust driver (git submodule)
├── src/
│   ├── main.rs          # CLI entry point
│   ├── terminal/        # Terminal reading (VCSA)
│   ├── font/            # Font rendering
│   ├── renderer/        # Text to framebuffer
│   ├── display/         # Display abstraction
│   └── vnc/             # VNC client (optional)
└── examples/
```

## Building from Source

```bash
# Standard build
cargo build

# With all features
cargo build --features full

# Release build
cargo build --release
```

## License

MIT License - see LICENSE file for details.

## Acknowledgments

- [PaperTTY](https://github.com/joukos/PaperTTY) - Original Python implementation
- [IT8951 Rust Driver](https://github.com/kylephillipsau/IT8951) - Our Rust driver
- [fontdue](https://github.com/mooman219/fontdue) - Font rendering
