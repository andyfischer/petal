# petal-sdl

Desktop host for Petal programs. It opens an SDL2 window, runs your `.ptl`
script once per frame, and draws the result. Graphics, input, and audio come
from SDL2; the draw and input vocabulary comes from [`petal-ui`](../../core-libs/petal-ui/).

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
| `--subpixel` | Keep fractional coordinates: circles, lines, polylines, polygons, ellipses, arcs and circle gradients draw at the script's exact numbers; the rect family, images and text stay whole pixels unless the script calls `pixel_snap(false)` (`pixel_snap(true)` snaps everything). Off by default |
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
| `sfx_board.ptl` | Procedural sound effects: every `sfx` preset plus a `synth` zap |
| `browser.ptl` | The example browser (uses the host's launcher natives) |
| `cc_*.ptl` | Creative-coding sketches: attractors, metaballs, reaction-diffusion, offscreen layers |
| `noc_*.ptl` | *Nature of Code* sketches: flocking, flow fields, springs, cloth, fractal trees |

## How it works

Your `.ptl` file runs every frame (about 60 fps). Use `state` variables to keep
data between frames. Edit the file while it runs and it reloads in place,
keeping `state` values.

```petal
import ui

state x = 100.0
x += 100.0 * dt()
draw_rect(int(x), 100, 20, 20, 255, 0, 0)
```

`import ui` brings in the drawing and input functions; it is petal-ui's
interface, which this runner provides. The runner's own extras (`sfx`,
`synth`, sandboxed file I/O, the example browser) are declared by the `sdl`
interface package in [`packages/sdl/`](packages/sdl/), which the runner
registers and provides: a script that uses them adds `import sdl`. Plain
`petal run` provides neither interface and refuses such a script at load;
`petal lint` checks it without running.

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
| `F9` | Edit mode (below): freezes the game; drag a shape to edit the code that drew it |

`F8` (any time, not only with the timeline) switches the `ui/drag` drag
debugger on and off: an app that drags with `ui/drag` shows its drop zones,
its target table with each target's distance, and its drag spec with the
chosen branch lit (`drag_debug`, read by the script as `ui_debug("drag")`).

The part worth trying: freeze, scrub back to just before a jump, press `F7`
on the player, then **edit a constant in the script and save**. The cursor
frame and the frames after it are replayed through the new code (starting
from the frame before the cursor) with the input that was recorded for
them, and the orange future trail shows the new trajectory next to the blue
past. Change the number again; the future moves again. The rest
of the game's state — score, level, everything — is exactly what it was.

The same operations are agent-protocol commands (`freeze`, `scrub`,
`rewind`, `replay`, `track`, `trail`, `timeline`; see
[docs/agent-protocol.md](docs/agent-protocol.md#timeline)), and a
`screenshot` taken while frozen renders the frame on screen with its
overlay. Recording costs one heap copy per frame (about 2 ms on a small
game in a release build); `--no-timeline` turns it off, `--history` sets the
length. `examples/games/hopper/` in the repo root is a game written for it.

## Edit mode

`F9` turns on edit mode and freezes the game at the frame on screen. Click a
shape and the overlay names the call that drew it (`` `draw_rect` at line 5:
… ``). Drag it, and Petal finds an edit of the script's numbers that puts it
where you dropped it, writes the file, and the hot reload replays the frozen
frame through the new code. There is no source view to choose which numbers
may change, so it works **best effort**: a `tunable(…)` number moves first,
a `const(…)` or `rigid(…)` one never does, and when several numbers tie the
move is split equally between them. When no edit reaches the spot, the
overlay says why and nothing is written. The applied edit stays in the
overlay; `z` undoes it. `F9` again (or `F5`) resumes the game. See
[direct-manipulation.md](../../docs/features/direct-manipulation.md#hosts);
the agent protocol has the same operations (`edit_mode`, `edit_pick`,
`edit_drag`, `edit_undo`).

## Use as a library

The crate is also a library (`petal_sdl`). Apps that need a different renderer
or native set, such as the `petal-fps` software 3D rasterizer, implement the
`Host` trait and reuse the window, event loop, agent protocol, screenshot and
record modes, hot reload, audio output, and gamepad handling. See
[`docs/usage/building-apps.md`](../../docs/usage/building-apps.md) for the pattern and
[`docs/design.md`](docs/design.md) for the extension points.
