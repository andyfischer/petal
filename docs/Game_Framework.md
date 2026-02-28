# Game Framework (petal-sdl)

Petal-SDL is an SDL2-based game framework for writing 2D games in Petal. It provides
a game loop, rendering primitives, input handling, and hot reload.

## Prerequisites

- SDL2, SDL2_image, and SDL2_ttf development libraries
- Rust toolchain

On macOS with Homebrew:

```bash
brew install sdl2 sdl2_image sdl2_ttf
```

## Building

```bash
cd petal-sdl
cargo build
```

## Running a Game

```bash
cargo run -- ../petal-sdl/examples/snake.ptl
```

Or after building:

```bash
./target/debug/petal-sdl examples/snake.ptl
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--width <n>` | Window width in pixels | 800 |
| `--height <n>` | Window height in pixels | 600 |
| `--title <str>` | Window title | "Petal Game" |
| `--no-hot-reload` | Disable file watching | enabled |
| `--agent` | Enable agent protocol (JSON over stdin/stdout) | off |
| `--headless` | Headless mode, no window (implies `--agent`) | off |
| `--screenshot <file>` | Run headlessly, save PNG screenshot, then exit | — |
| `--frames <n>` | Frames to run before screenshot | 120 |

## Game Loop Model

Petal-SDL runs your program once per frame. The program uses `state` variables to persist
data across frames. A typical game structure looks like:

```petal
// Persistent state
state x = 400
state y = 300
state speed = 200

// Input
if key_down("left")  { x -= speed * dt() }
if key_down("right") { x += speed * dt() }
if key_down("up")    { y -= speed * dt() }
if key_down("down")  { y += speed * dt() }

// Rendering
clear(20, 20, 40)
draw_rect(x - 10, y - 10, 20, 20, 255, 100, 100)
draw_text("Use arrow keys", 10, 10, 20, 255, 255, 255)
```

## Native Functions

### Drawing

| Function | Description |
|----------|-------------|
| `clear(r, g, b)` | Clear the screen with an RGB color |
| `draw_rect(x, y, w, h, r, g, b)` | Draw a filled rectangle |
| `draw_rect_outline(x, y, w, h, r, g, b)` | Draw a rectangle outline |
| `draw_line(x1, y1, x2, y2, r, g, b)` | Draw a line |
| `draw_circle(cx, cy, radius, r, g, b)` | Draw a filled circle |
| `draw_text(text, x, y, size, r, g, b)` | Draw text at a position |

All color values are integers 0-255.

### Input

| Function | Description |
|----------|-------------|
| `key_down(name)` | `true` if a key is currently held down |
| `key_pressed(name)` | `true` if a key was pressed this frame |
| `mouse_x()` | Current mouse X position |
| `mouse_y()` | Current mouse Y position |
| `mouse_down(button)` | `true` if a mouse button is down |

Key names are lowercase strings: `"left"`, `"right"`, `"up"`, `"down"`, `"space"`,
`"return"`, `"a"`-`"z"`, `"0"`-`"9"`, etc.

### Timing

| Function | Description |
|----------|-------------|
| `dt()` | Seconds elapsed since the last frame (float) |
| `frame_count()` | Total number of frames elapsed |

### Screen

| Function | Description |
|----------|-------------|
| `screen_width()` | Window width in pixels |
| `screen_height()` | Window height in pixels |

## Hot Reload

By default, petal-sdl watches the source file for changes. When you save the file,
it automatically recompiles and restarts execution while preserving `state` variables.
This means you can tweak colors, physics, or game logic and see results instantly
without restarting.

Disable with `--no-hot-reload`.

## Example Games

The `petal-sdl/examples/` directory contains playable games:

| Game | Description |
|------|-------------|
| `snake.ptl` | Classic snake game with gradient rendering |
| `pong.ptl` | Two-paddle pong |
| `breakout.ptl` | Brick breaker |
| `tetris.ptl` | Tetris with piece rotation |
| `invaders.ptl` | Space invaders |
| `flappy.ptl` | Flappy bird clone |
| `asteroids.ptl` | Asteroids with ship rotation |
| `platformer.ptl` | Side-scrolling platformer |
| `dodge.ptl` | Dodge falling objects |
| `particles.ptl` | Particle system demo |
| `paint.ptl` | Drawing application |

## Agent Protocol

The `--agent` flag enables a JSON-based debugging protocol over stdin/stdout, designed
for AI assistants to interact with running games programmatically. The `--headless` flag
combines this with no-window mode for automated testing and screenshot capture.
