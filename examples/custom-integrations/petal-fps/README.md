# petal-fps

A first-person-shooter experiment written in Rust and Petal.

The Rust side is small. It builds on the
[`petal-sdl`](../../../integrations/petal-desktop-sdl/) integration and adds
only a software z-buffered triangle rasterizer and the `triangle3d` family of
native draw functions. The window, event loop, input, agent/headless/screenshot
/record modes, and hot reload all come from `petal-sdl`; input, timing, and
mouselook natives (`key_down`, `mouse_dx`, `grab_mouse`, `dt`, `time`,
`screen_width`, ...) come from `petal-ui`, the same as every other host.
(This is the "thin host delta" shape described in
[docs/building-apps.md](../../../docs/building-apps.md).)

Everything else lives in a `.ptl` script: camera, projection math, level
geometry, enemies, shooting, and HUD. The script can be edited while the game
is running.

`examples/fps_game.ptl` is the full demo: a neon city with 12 skyscrapers,
8 patrol bots, raycast shooting, and a health/ammo/minimap HUD.
`LANGUAGE_IDEAS.md` records language friction found while building it.

## Prerequisites

- Rust (any recent stable toolchain)
- SDL2 development libraries:
  - Debian/Ubuntu: `sudo apt-get install libsdl2-dev libsdl2-ttf-dev libsdl2-image-dev`
  - macOS: `brew install sdl2 sdl2_ttf sdl2_image`

On macOS the linker also needs Homebrew's lib directory. Either use `./run.sh`
(which sets it for you) or export it yourself:

```bash
export LIBRARY_PATH=/opt/homebrew/lib
```

## Build

From this directory:

```bash
cargo build --release
```

The first build compiles the Petal compiler in `../../../rust` and is slow.
Later builds are quick.

## Run

```bash
cargo run --release -- examples/fps_game.ptl
# or, on macOS:
./run.sh                      # defaults to examples/fps_game.ptl
```

Controls:

| Input              | Action             |
|--------------------|--------------------|
| Mouse              | Look around        |
| W / A / S / D      | Move               |
| Left-click / Space | Shoot (raycast)    |
| R                  | Reload             |
| Esc                | Release mouse grab |

The script hot-reloads on save. Edit `examples/fps_game.ptl` in another window
and the change appears on the next frame. `state` values such as player
position, score, and enemy HP survive the reload, because each `state` slot is
keyed by its declaration and call path rather than by source position.

## Agent and headless modes

These modes let a program (or an agent) drive the game through a JSON-over-stdio
protocol, with or without a display.

### `--screenshot` — one PNG

Runs N frames at a fixed `dt = 1/60` and writes the final frame:

```bash
cargo run --release -- --screenshot out.png --frames 60 examples/fps_game.ptl
```

### `--record` — one PNG per frame

Writes a flipbook into a directory, after an optional warmup:

```bash
cargo run --release -- --record frames/ --frames 30 --warmup 0 examples/fps_game.ptl
```

### `--headless` — JSON commands on stdin

No window. Commands arrive as JSON lines on stdin; responses go to stdout.
The game starts paused at frame 0.

```bash
cargo run --release -- --headless examples/fps_game.ptl
```

Commands:

| Command | Fields | Effect |
|---------|--------|--------|
| `step` | `n` (default 1) | Run N frames. |
| `state` | – | Dump all `state` variables as JSON. |
| `set_state` | `name`, `value` | Override a state variable. |
| `input` | `keys_down[]`, `mouse{x,y,buttons[]}`, `mouse_delta{dx,dy}`, `text` | Inject input for the next frame. |
| `screenshot` | – | Return the current frame as a base64 PNG, plus draw stats. |
| `capture_draw_commands` | – | Return all draw commands for the next frame. |
| `draw_stats` | – | Count triangles, lines, and rects, plus the depth range. |
| `pending_report` | – | Report every live pending resource. |
| `pause` / `resume` | – | Stop or start the run loop. |

Example: move the player, shoot, and take a screenshot:

```bash
./target/release/petal-fps --headless examples/fps_game.ptl <<'EOF2'
{"cmd":"step","n":1}
{"cmd":"set_state","name":"yaw","value":0.4}
{"cmd":"set_state","name":"player_x","value":-2.0}
{"cmd":"set_state","name":"player_z","value":0.0}
{"cmd":"input","mouse":{"x":400,"y":300,"buttons":[1]}}
{"cmd":"step","n":1}
{"cmd":"screenshot"}
EOF2
```

### `--agent` — same protocol, with a window

Same as `--headless`, but also opens a window so you can watch.

## Other flags

| Flag | Default | Meaning |
|------|---------|---------|
| `--width <n>` / `--height <n>` | 800 x 600 | Framebuffer size |
| `--title <str>` | "petal-fps" | Window title |
| `--no-hot-reload` | off | Disable the file watcher |
| `--frames <n>` | 60 | Frames for `--screenshot` / `--record` |
| `--warmup <n>` | 30 | Warmup frames before `--record` starts saving |

## Layout

```
petal-fps/
├── run.sh                      Build and run with LIBRARY_PATH set (macOS)
├── src/
│   ├── main.rs                 CLI: parse args, then call petal_sdl::run_*
│   ├── host.rs                 FpsHost: implements petal_sdl::Host (renderer + natives + stats)
│   ├── framebuffer.rs          Software z-buffered triangle rasterizer
│   ├── renderer.rs             Uploads the framebuffer to an SDL2 streaming texture
│   ├── commands.rs             DrawCommand enum and decoding (Petal to Rust)
│   ├── native_fns.rs           3D/2D draw natives (triangle3d, ...) and log
│   └── font.rs                 5x7 bitmap font for HUD text
└── examples/
    ├── fps_game.ptl            The full city demo
    ├── cyberpunk_city.ptl      Early scaffold: camera, ground, one cube
    ├── test_triangle.ptl       Minimal rasterizer smoke test
    └── debug_state.ptl,        State-persistence checks
        debug_state2.ptl
```
