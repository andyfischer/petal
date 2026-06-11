# Game Framework (petal-sdl)

Petal-SDL is an SDL2-based game framework for writing 2D games in Petal. It provides
a game loop, rendering primitives, input handling, and hot reload.

This page is the high-level overview. For deeper material:

- [`petal-sdl/docs/game-dev-guide.md`](../petal-sdl/docs/game-dev-guide.md) — patterns for writing games (game-loop structure, AABB collision, spawning entities, animation)
- [`petal-sdl/docs/agent-protocol.md`](../petal-sdl/docs/agent-protocol.md) — full per-command reference for `--agent` / `--headless` modes
- [`docs/debug-protocol.md`](debug-protocol.md) — canonical JSON schema shared by petal-sdl (stdin/stdout) and petal-diagram-canvas (WebSocket)
- [`petal-fps/README.md`](../petal-fps/README.md) — a hybrid Rust + Petal 3D experiment that uses the same protocol for headless agent control

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

### Persistent canvas (accumulative drawing)

The framebuffer **persists between frames**: it is only wiped when your program
calls `clear()`. Game-style sketches call `clear(...)` at the top of every frame
and so always start from a blank screen — the usual case shown above.

For generative art where the *accumulated trace* is the art — attractors,
Lissajous figures, particle trails, brush strokes — simply **don't call
`clear()`**. Every primitive you draw stays on screen, and the image builds up
over time. Clear once on the first frame (guarded by a `state` flag) to set the
background, then let it accumulate:

```petal
state started = false
if !started then
  clear(0, 0, 0)   // paint the background once
  started = true
end

// Each frame draws a few more dots that persist forever.
draw_circle(int(x), int(y), 2, 255, 200, 80)
```

This replaces the old workaround of stashing every past point in `state` and
redrawing the whole history each frame (which grows O(n) per frame). See
`examples/cc_lissajous_trails.ptl`.

### Offscreen canvases (PGraphics-style layers)

For layered compositing, masks, and per-layer trails, allocate an **offscreen
canvas** — a separate render target you draw into and later blit onto the main
framebuffer. This is the standard creative-coding move (Processing's
`PGraphics`).

```petal
// Build a reusable stamp once, in a 24x24 offscreen canvas.
let stamp = create_canvas(24, 24)   // returns a canvas handle (an int)
draw_to(stamp)                       // redirect drawing into the canvas
draw_rect(9, 2, 6, 20, 240, 220, 120)
draw_rect(2, 9, 20, 6, 240, 220, 120)
draw_to_screen()                     // redirect back to the main framebuffer

// Composite the stamp wherever you like — transparent pixels show the
// background through, only the drawn pixels land.
draw_canvas(stamp, 100, 50)
draw_canvas(stamp, 200, 80)
```

An offscreen canvas starts **fully transparent**, so blitting it composites
only the pixels you painted. Canvases are recreated fresh from the draw stream
every frame (handles are stable across the per-frame re-run), so call
`create_canvas` each frame just like any other draw call. See
`examples/cc_offscreen_layers.ptl`.

## Native Functions

### Drawing

| Function | Description |
|----------|-------------|
| `clear(r, g, b)` | Clear the screen with an RGB color. If a frame never calls `clear()`, the previous frame's pixels persist (see [Persistent canvas](#persistent-canvas-accumulative-drawing)) |
| `draw_rect(x, y, w, h, r, g, b)` | Draw a filled rectangle |
| `draw_rect_outline(x, y, w, h, r, g, b)` | Draw a rectangle outline |
| `draw_line(x1, y1, x2, y2, r, g, b)` | Draw a line |
| `draw_circle(cx, cy, radius, r, g, b)` | Draw a filled circle |
| `fill_triangle(x1, y1, x2, y2, x3, y3, r, g, b)` | Draw a filled triangle |
| `fill_poly(points, r, g, b)` | Draw a filled polygon; `points` is a list of `vec2` or `[x, y]` pairs (≥ 3) |
| `draw_text(text, x, y, size, r, g, b)` | Draw text at a position |
| `create_canvas(w, h)` | Allocate an offscreen canvas (PGraphics-style render target); returns a canvas handle (see [Offscreen canvases](#offscreen-canvases-pgraphics-style-layers)) |
| `draw_to(canvas)` | Redirect subsequent draw commands into the given offscreen canvas |
| `draw_to_screen()` | Redirect subsequent draw commands back to the main framebuffer |
| `draw_canvas(canvas, x, y)` | Blit an offscreen canvas onto the current render target at `(x, y)`; transparent pixels show the destination through |

All color values are integers 0-255.

### Input

| Function | Description |
|----------|-------------|
| `key_down(name)` | `true` if a key is currently held down |
| `key_pressed(name)` | `true` if a key was pressed this frame (edge-triggered) |
| `mouse_x()` | Current mouse X position |
| `mouse_y()` | Current mouse Y position |
| `mouse_down(button)` | `true` if a mouse button is currently held |
| `mouse_pressed(button)` | `true` if a mouse button was pressed this frame |

Key names are lowercase strings. The supported set is: `a`–`z`, `0`–`9`,
`up`, `down`, `left`, `right`, `space`, `return`, `escape`, `tab`,
`backspace`, `shift`, `ctrl`, `alt`. Any other key name returns `false`
(there's no "unknown key" error).

Mouse `button` is `1` (left), `2` (middle), or `3` (right).

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

The `petal-sdl/examples/` directory contains a mix of playable games and
sketches from *The Nature of Code* (the `noc_*.ptl` set):

| Game / Sketch | Description |
|---------------|-------------|
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
| `browser.ptl` | UI/browser mockup (uses the example-launcher natives) |
| `noc_*.ptl` | *Nature of Code* reproductions — random walkers, flocking, flow fields, springs, cloth, fractal trees, elementary CA, etc. |

### Example launcher natives

The `browser.ptl` example uses four host functions for building a menu that
launches other examples:

| Function | Description |
|----------|-------------|
| `example_count()` | Number of bundled examples |
| `example_name(i)` | Display name of example `i` |
| `example_path(i)` | Absolute `.ptl` path of example `i` |
| `launch_script(path)` | Replace the running program with the one at `path` |

These are petal-sdl-specific; they are not part of the core language and
are only available when running under `petal-sdl`.

## Agent Protocol

The `--agent` flag enables a JSON-based debugging protocol over stdin/stdout, designed
for AI assistants to interact with running games programmatically. The `--headless` flag
combines this with no-window mode for automated testing and screenshot capture.

See [`docs/debug-protocol.md`](debug-protocol.md) for the command/response
schema — the same protocol is used by `petal-diagram-canvas` over
WebSocket, so tooling written against one transport works against the
other.
