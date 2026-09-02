# petal-sdl

Desktop host for Petal programs. It opens an SDL2 window, runs your `.ptl`
script once per frame, and draws the result. Graphics, input, and audio come
from SDL2; the draw and input vocabulary comes from [`petal-ui`](../../petal-ui/).

The directory is `integrations/petal-desktop-sdl`, but the crate and binary are
still named `petal-sdl`.

## Prerequisites

- Rust (latest stable)
- SDL2, SDL2_image, and SDL2_ttf development libraries:

  ```bash
  # macOS
  brew install sdl2 sdl2_image sdl2_ttf

  # Ubuntu/Debian
  sudo apt install libsdl2-dev libsdl2-image-dev libsdl2-ttf-dev
  ```

## Build

```bash
cd integrations/petal-desktop-sdl
cargo build
```

On macOS with Homebrew, the linker needs to be told where SDL2 lives:

```bash
LIBRARY_PATH=/opt/homebrew/lib cargo build
```

## Run

```bash
cargo run -- examples/pong.ptl
```

With no file argument, `petal-sdl` opens a browser over the bundled examples.

### Options

| Flag | Description |
|------|-------------|
| `--width <n>` | Window width (default: 800) |
| `--height <n>` | Window height (default: 600) |
| `--title <str>` | Window title (default: "Petal Game") |
| `--no-hot-reload` | Disable file watching |
| `--agent` | Accept JSON commands on stdin (see [agent protocol](docs/agent-protocol.md)) |
| `--headless` | No window; frames advance only on `step` commands (implies `--agent`) |
| `--screenshot <file>` | Run headlessly, save a PNG, then exit |
| `--frames <n>` | Frames to run before the screenshot (default: 120) |

## Examples

`examples/` holds playable games, creative-coding sketches, and
*Nature of Code* reproductions. A few to start with:

| File | Description |
|------|-------------|
| `pong.ptl` | Pong with neon effects |
| `breakout.ptl` | Brick breaker with particles |
| `tetris.ptl` | Tetris with beveled pieces |
| `snake.ptl` | Snake with a gradient body |
| `asteroids.ptl` | Asteroids with ship thrust |
| `invaders.ptl` | Space Invaders with shields |
| `platformer.ptl` | Side-scrolling platformer |
| `paint.ptl` | Drawing app with a color palette |
| `browser.ptl` | The example browser (uses the host's launcher natives) |
| `cc_*.ptl` | Creative-coding sketches: attractors, metaballs, reaction-diffusion, offscreen layers |
| `noc_*.ptl` | *Nature of Code* sketches: flocking, flow fields, springs, cloth, fractal trees |

## How it works

Your `.ptl` file runs every frame (about 60 fps). Use `state` variables to keep
data between frames. Edit the file while it runs and it reloads in place,
keeping `state` values.

```petal
state x = 100.0
x += 100.0 * dt()
draw_rect(int(x), 100, 20, 20, 255, 0, 0)
```

See [`docs/game-dev-guide.md`](docs/game-dev-guide.md) for the API and common
patterns, and [`docs/design.md`](docs/design.md) for how the host is built.

## Use as a library

The crate is also a library (`petal_sdl`). Apps that need a different renderer
or native set, such as the `petal-fps` software 3D rasterizer, implement the
`Host` trait and reuse the window, event loop, agent protocol, screenshot and
record modes, hot reload, audio output, and gamepad handling. See
[`docs/building-apps.md`](../../docs/building-apps.md) for the pattern and
[`docs/design.md`](docs/design.md) for the extension points.
