# Paper-TTY: Rust E-Ink Terminal Emulator

A Rust implementation inspired by [PaperTTY](https://github.com/joukos/PaperTTY), designed to render terminal output and VNC sessions on IT8951-based e-paper displays.

## Project Overview

Paper-TTY transforms e-ink displays into functional computer monitors, enabling:
- Interactive terminal applications (vim, tmux, htop)
- VNC desktop rendering
- System monitoring dashboards
- Low-power, eye-friendly computing

## Architecture

```
paper-tty-rs/
├── Cargo.toml
├── PLANNING.md
├── README.md
├── drivers/
│   └── it8951/              # Git submodule - IT8951 Rust driver
├── src/
│   ├── main.rs              # CLI entry point
│   ├── lib.rs               # Library exports
│   ├── error.rs             # Error types
│   ├── config.rs            # Configuration management
│   │
│   ├── terminal/            # Terminal emulation
│   │   ├── mod.rs
│   │   ├── vcsa.rs          # /dev/vcsa* reader (screen buffer)
│   │   ├── tty.rs           # /dev/tty* reader (character stream)
│   │   └── parser.rs        # ANSI/VT100 escape sequence parser
│   │
│   ├── font/                # Font rendering
│   │   ├── mod.rs
│   │   ├── ttf.rs           # TrueType font loading
│   │   ├── bitmap.rs        # Bitmap font support
│   │   ├── glyph_cache.rs   # Glyph caching for performance
│   │   └── builtin/         # Bundled fonts (optional)
│   │
│   ├── renderer/            # Display rendering
│   │   ├── mod.rs
│   │   ├── text.rs          # Text/terminal renderer
│   │   ├── cursor.rs        # Cursor rendering (block, underline, bar)
│   │   ├── diff.rs          # Differential update detection
│   │   └── dither.rs        # Grayscale dithering algorithms
│   │
│   ├── vnc/                 # VNC client (optional feature)
│   │   ├── mod.rs
│   │   ├── client.rs        # VNC protocol client
│   │   └── framebuffer.rs   # VNC framebuffer handling
│   │
│   └── display/             # Display abstraction
│       ├── mod.rs
│       ├── it8951.rs        # IT8951 driver wrapper
│       └── mock.rs          # Mock display for testing
│
├── examples/
│   ├── terminal_demo.rs     # Basic terminal rendering
│   ├── font_test.rs         # Font rendering test
│   └── vnc_demo.rs          # VNC client demo
│
└── assets/
    └── fonts/               # Bundled fonts (optional)
```

## Core Components

### 1. Terminal Reader (`terminal/`)

**VCSA Reader** - Read screen buffer directly from `/dev/vcsa*`:
- Fast access to current terminal state
- Includes character + attribute (color) data
- Format: 4 bytes header (rows, cols) + (char, attr) pairs

**TTY Reader** - Read character stream from `/dev/tty*`:
- Real-time character input
- Requires ANSI escape sequence parsing
- More complex but works with any terminal

```rust
pub trait TerminalReader {
    fn read_screen(&mut self) -> Result<ScreenBuffer>;
    fn dimensions(&self) -> (u16, u16);  // (cols, rows)
}

pub struct ScreenBuffer {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub cursor_pos: Option<(u16, u16)>,
}

pub struct Cell {
    pub character: char,
    pub fg_color: Color,
    pub bg_color: Color,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}
```

### 2. Font Renderer (`font/`)

**Features**:
- TrueType font loading via `fontdue` or `ab_glyph`
- Bitmap font support for pixel-perfect small fonts
- Glyph caching for repeated characters
- Unicode support (requires appropriate fonts)
- Configurable size and spacing

```rust
pub trait FontRenderer {
    fn render_glyph(&mut self, c: char) -> Option<GlyphBitmap>;
    fn metrics(&self) -> FontMetrics;
}

pub struct FontMetrics {
    pub width: u16,        // Character cell width
    pub height: u16,       // Character cell height
    pub baseline: u16,     // Baseline offset
    pub line_height: u16,  // Total line height
}

pub struct GlyphBitmap {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,     // Grayscale values 0-255
}
```

### 3. Display Renderer (`renderer/`)

**Text Renderer** - Convert terminal buffer to display framebuffer:
- Map terminal cells to pixel positions
- Apply font glyphs
- Handle colors (map to grayscale levels)
- Cursor rendering

**Differential Updates** - Only refresh changed regions:
- Track previous frame state
- Compute dirty rectangles
- Use partial refresh for changed areas
- Full refresh periodically (to reduce ghosting)

```rust
pub struct TextRenderer {
    font: Box<dyn FontRenderer>,
    cols: u16,
    rows: u16,
    prev_buffer: Option<ScreenBuffer>,
}

impl TextRenderer {
    pub fn render(&mut self, buffer: &ScreenBuffer, fb: &mut Framebuffer) -> Vec<DirtyRect>;
    pub fn render_cursor(&self, pos: (u16, u16), style: CursorStyle, fb: &mut Framebuffer);
}

pub enum CursorStyle {
    Block,
    Underline,
    Bar,
    None,
}
```

### 4. VNC Client (`vnc/`) [Optional Feature]

**Features**:
- Connect to VNC servers (RealVNC, TightVNC, x11vnc)
- Handle VNC protocol updates
- Dither color to grayscale
- Support screen rotation

```rust
pub struct VncClient {
    connection: VncConnection,
    width: u16,
    height: u16,
}

impl VncClient {
    pub fn connect(host: &str, port: u16, password: Option<&str>) -> Result<Self>;
    pub fn get_framebuffer(&mut self) -> Result<Framebuffer>;
    pub fn get_updates(&mut self) -> Result<Vec<UpdateRect>>;
}
```

### 5. Display Abstraction (`display/`)

**IT8951 Wrapper** - Integrate with our driver:
```rust
pub struct EinkDisplay {
    device: IT8951<LinuxSpi, LinuxInputPin, LinuxOutputPin, LinuxOutputPin>,
    framebuffer: Framebuffer,
    config: DisplayConfig,
}

impl EinkDisplay {
    pub fn new(config: DisplayConfig) -> Result<Self>;
    pub fn clear(&mut self) -> Result<()>;
    pub fn update_full(&mut self, mode: DisplayMode) -> Result<()>;
    pub fn update_partial(&mut self, area: &Area, mode: DisplayMode) -> Result<()>;
    pub fn framebuffer(&mut self) -> &mut Framebuffer;
}
```

## CLI Interface

```
paper-tty [OPTIONS] <COMMAND>

Commands:
  terminal    Render a Linux terminal to the display
  vnc         Connect to VNC server and render
  image       Display a static image
  clear       Clear the display
  test        Run display tests

Options:
  -d, --driver <DRIVER>     Display driver [default: IT8951]
  -r, --rotate <DEGREES>    Rotation (0, 90, 180, 270) [default: 0]
  -v, --verbose             Enable verbose logging
  -c, --config <FILE>       Config file path

terminal options:
  -t, --tty <N>             TTY number to render [default: 1]
  --vcsa                    Use VCSA interface (faster)
  -f, --font <PATH>         Font file path
  -s, --font-size <SIZE>    Font size [default: 16]
  --cursor <STYLE>          Cursor style (block, underline, bar, none)
  --refresh-rate <MS>       Refresh interval in ms [default: 250]
  --partial                 Enable partial refresh
  --spacing <N>             Line spacing [default: 0]

vnc options:
  -H, --host <HOST>         VNC server host [default: localhost]
  -P, --port <PORT>         VNC server port [default: 5900]
  -p, --password <PASS>     VNC password
  --dither <MODE>           Dithering mode (none, floyd-steinberg, atkinson)
```

## Configuration File

```toml
# ~/.config/paper-tty/config.toml

[display]
driver = "IT8951"
rotation = 0
vcom = 1500

[terminal]
tty = 1
use_vcsa = true
refresh_rate_ms = 250
partial_refresh = true
full_refresh_interval = 30  # Full refresh every N partial refreshes

[font]
path = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
size = 16
line_spacing = 0

[cursor]
style = "block"
blink = false

[vnc]
host = "localhost"
port = 5900
dither = "floyd-steinberg"

[colors]
# Map terminal colors to grayscale (0 = black, 255 = white)
black = 0
white = 255
red = 64
green = 128
blue = 96
# ... etc
```

## Implementation Phases

### Phase 1: Foundation
- [x] Create repository structure
- [x] Add IT8951 driver as submodule
- [x] Set up Cargo.toml with dependencies
- [ ] Implement error types
- [ ] Create display abstraction wrapper
- [ ] Basic CLI skeleton with clap

### Phase 2: Font Rendering
- [ ] Integrate fontdue for TTF rendering
- [ ] Implement glyph caching
- [ ] Create FontRenderer trait
- [ ] Add font metrics calculation
- [ ] Test with various fonts

### Phase 3: Terminal Reader
- [ ] Implement VCSA reader
- [ ] Parse terminal attributes (colors)
- [ ] Handle cursor position
- [ ] Test with real /dev/vcsa1

### Phase 4: Text Renderer
- [ ] Map terminal buffer to framebuffer
- [ ] Implement color-to-grayscale mapping
- [ ] Add cursor rendering
- [ ] Implement basic differential updates

### Phase 5: Integration
- [ ] Connect all components
- [ ] Add configuration file support
- [ ] Implement CLI commands
- [ ] Create terminal_demo example

### Phase 6: Optimization
- [ ] Improve differential update algorithm
- [ ] Add dirty rectangle merging
- [ ] Tune refresh modes for best quality/speed
- [ ] Performance profiling

### Phase 7: VNC Support [Optional]
- [ ] Integrate VNC client library
- [ ] Implement framebuffer conversion
- [ ] Add dithering algorithms
- [ ] Create vnc_demo example

### Phase 8: Polish
- [ ] Comprehensive error handling
- [ ] Documentation
- [ ] More examples
- [ ] README and usage guide

## Key Differences from Python PaperTTY

| Aspect | Python PaperTTY | Rust Paper-TTY |
|--------|-----------------|----------------|
| Performance | Interpreted, slower | Compiled, faster |
| Memory | Higher usage | Lower, controlled |
| Type Safety | Runtime errors | Compile-time checks |
| Dependencies | PIL, vncdotool | fontdue, image crate |
| Driver | Waveshare Python libs | Our Rust IT8951 driver |
| Async | Limited | Full tokio support (optional) |

## Hardware Requirements

- Raspberry Pi (3/4/5 recommended, Zero works but slower)
- IT8951-based e-paper display (6", 7.8", 9.7", 10.3")
- SPI enabled (`raspi-config` → Interface Options → SPI)
- For terminal mode: Linux virtual console access
- For VNC mode: VNC server running (x11vnc, TightVNC, etc.)

## Testing Strategy

1. **Unit Tests**: Mock display, test font rendering, test terminal parsing
2. **Integration Tests**: Test with real IT8951 driver (hardware-tests feature)
3. **Visual Tests**: Capture framebuffer output, compare to reference images

## Open Questions

1. **Font bundling**: Should we bundle default fonts or require user-provided?
2. **PTY support**: Should we support pseudo-terminals for shell embedding?
3. **Input forwarding**: Should we handle keyboard/mouse input back to the terminal?
4. **Multi-display**: Support for multiple e-ink displays?

## References

- [PaperTTY (Python)](https://github.com/joukos/PaperTTY)
- [IT8951 Rust Driver](https://github.com/kylephillipsau/IT8951)
- [Linux VCSA interface](https://www.kernel.org/doc/html/latest/admin-guide/vga-softcursor.html)
- [fontdue crate](https://docs.rs/fontdue/)
- [VNC Protocol](https://datatracker.ietf.org/doc/html/rfc6143)

---

**Created**: 2025-01-26
**Status**: Planning
