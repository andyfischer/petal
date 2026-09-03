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
| `--no-timeline` | Disable frame history (see [Time travel](#time-travel)) |
| `--history <n>` | Frames of history to keep (default: 600, ten seconds) |
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

## Time travel

The host forks the running execution after every frame and keeps the last
ten seconds of forks, so any script can be frozen, scrubbed, rewound, and
re-simulated through an edit. Nothing in the script opts in.

| Key | Action |
|-----|--------|
| `F5` | Freeze / resume. Resuming from an earlier frame **rewinds** the game to it and forgets the frames after |
| `,` / `.` | While frozen: scrub one frame back / forward. Hold for real time, `shift` for 5× |
| `F6` | Toggle the trail overlay |
| `F7` | Track the shape under the pointer: its `draw_*` call is followed through every recorded frame and drawn as a path |

The part worth trying: freeze, scrub back to just before a jump, press `F7`
on the player, then **edit a constant in the script and save**. The frames
after the cursor are replayed through the new code with the input that was
recorded for them, and the orange future trail shows the new trajectory next
to the blue past. Change the number again; the future moves again. The rest
of the game's state — score, level, everything — is exactly what it was.

The same operations are agent-protocol commands (`freeze`, `scrub`,
`rewind`, `replay`, `track`, `trail`, `timeline`; see
[docs/agent-protocol.md](docs/agent-protocol.md#timeline)), and a
`screenshot` taken while frozen renders the frame on screen with its
overlay. Recording costs one heap copy per frame (about 2 ms on a small
game in a release build); `--no-timeline` turns it off, `--history` sets the
length. `examples/games/hopper/` in the repo root is a game written for it.

## Use as a library

The crate is also a library (`petal_sdl`). Apps that need a different renderer
or native set, such as the `petal-fps` software 3D rasterizer, implement the
`Host` trait and reuse the window, event loop, agent protocol, screenshot and
record modes, hot reload, audio output, and gamepad handling. See
[`docs/building-apps.md`](../../docs/building-apps.md) for the pattern and
[`docs/design.md`](docs/design.md) for the extension points.
