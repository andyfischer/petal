# petal-sdl

Desktop game engine for Petal programs. Uses SDL2 for graphics, input, and audio.

## Prerequisites

- **Rust** (latest stable)
- **SDL2 development libraries**:
  ```bash
  # macOS
  brew install sdl2 sdl2_image sdl2_ttf

  # Ubuntu/Debian
  sudo apt install libsdl2-dev libsdl2-image-dev libsdl2-ttf-dev
  ```

## Build

```bash
cd petal-sdl
cargo build
```

## Run

```bash
cargo run -- examples/pong.ptl
```

### Options

| Flag | Description |
|------|-------------|
| `--width <n>` | Window width (default: 800) |
| `--height <n>` | Window height (default: 600) |
| `--title <str>` | Window title |
| `--no-hot-reload` | Disable live code reloading |
| `--agent` | Agent protocol mode (JSON over stdin/stdout) |
| `--headless` | No window, frame-driven (implies `--agent`) |
| `--screenshot <path> --frames <n>` | Capture a screenshot after N frames |

## Examples

| File | Description |
|------|-------------|
| `pong.ptl` | Classic Pong with neon effects |
| `breakout.ptl` | Brick breaker with particles |
| `tetris.ptl` | Tetris with 3D beveled pieces |
| `snake.ptl` | Snake with gradient body |
| `asteroids.ptl` | Asteroids with ship thrust |
| `invaders.ptl` | Space Invaders with shields |
| `flappy.ptl` | Flappy Bird clone |
| `platformer.ptl` | Side-scrolling platformer |
| `dodge.ptl` | Dodge obstacles game |
| `particles.ptl` | Particle effects demo |
| `paint.ptl` | Drawing app with color palette |
| `browser.ptl` | UI/browser mockup |
| `cc_strange_attractor.ptl` | Clifford & De Jong attractors with live param tuning |
| `cc_metaballs.ptl` | Implicit-surface blobs sampled on a coarse grid |
| `cc_10_print.ptl` | The Commodore 64 `10 PRINT` weave with palettes & mutation |
| `cc_differential_growth.ptl` | Self-avoiding curve that buds into flower-like lobes |
| `cc_reaction_diffusion.ptl` | Gray-Scott model — spots, stripes, mazes, coral |

Run any example:

```bash
cargo run -- examples/tetris.ptl
```

## How it works

Your `.ptl` file runs **every frame** (~60fps). Use `state` variables to persist
data between frames. Edit the file while running for **hot reload** — state is
preserved.

```petal
state x = 100.0
x += 100.0 * dt()
draw_rect(int(x), 100, 20, 20, 255, 0, 0)
```

See [`docs/game-dev-guide.md`](docs/game-dev-guide.md) for the full API reference.

## Use as a library

This crate is also a library (`petal_sdl`). Apps that need a different renderer
or native set — like the `petal-fps` software 3D rasterizer — depend on it and
implement the `Host` trait instead of copying the host code, reusing the window,
event loop, agent protocol, screenshot/record modes, and hot reload. See
[`docs/building-on-integrations.md`](../../docs/building-on-integrations.md) for
the pattern.
