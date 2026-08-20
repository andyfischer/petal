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
[`docs/building-apps.md`](../../docs/building-apps.md) for
the pattern.

## Audio, gamepads, and `end_frame`

Three extension points exist for library users (they change nothing for a plain
`.ptl` sketch under the shipped binary):

- **`Host::on_sdl_init(&sdl)`** — runs right after `sdl2::init()` in the
  windowed modes, so a host can open an audio device or other SDL subsystem
  without forking the loop.
- **`Host::end_frame(&mut env)`** — runs after every committed frame in *every*
  mode, windowed or not, so a host can flush its own per-frame output when
  there is nothing to present. Not called for speculative capture frames.
- **`audio::AudioOutput`** — a thin safe wrapper over SDL's `AudioQueue<i16>`
  (open / queue interleaved samples / `queued_frames()` / pause / resume).
  Transport only; the host owns synthesis. Queued rather than callback-driven
  because Petal's `Env` is single-threaded, which is what lets a script
  synthesize its own audio.

**Gamepads** are folded into the same normalized key stream as the keyboard —
no separate pad API — so `key_down("left")` works from a d-pad or the left
stick. Pad 0 maps to arrows + `z`/`x` + `return`/`shift`, pad 1 to `i k j l` +
`n`/`m`. Hot-plug is handled; with no controller attached nothing changes.
See [`docs/design.md`](docs/design.md) for the full table.
