# petal-fps

A hybrid Rust + Petal first-person-shooter experiment. The Rust host provides
windowing, input, and a software z-buffered triangle rasterizer; everything
else — camera, projection math, level geometry, enemies, shooting, HUD —
lives in a `.ptl` script that can be hot-reloaded while the game is running.

See `examples/fps_game.ptl` for the full cyberpunk-city demo
(12 neon skyscrapers, 8 patrol bots, raycast shooting, health/ammo/minimap
HUD) and `LANGUAGE_IDEAS.md` for Petal friction points discovered while
building it.

## Prerequisites

- Rust (any recent stable toolchain)
- SDL2 development libraries (host OS):
  - Debian/Ubuntu: `sudo apt-get install libsdl2-dev libsdl2-ttf-dev libsdl2-image-dev`
  - macOS: `brew install sdl2 sdl2_ttf sdl2_image`

## Build

From this directory:

```bash
cargo build --release
```

The first build is slow (it compiles the Petal compiler in `../rust`); later
builds are quick.

## Run — windowed gameplay

```bash
cargo run --release -- examples/fps_game.ptl
```

Controls:

| Input           | Action              |
|-----------------|---------------------|
| Mouse           | Look around         |
| W / A / S / D   | Move               |
| Left-click / Space | Shoot (raycast)  |
| R               | Reload              |
| Esc             | Release mouse grab  |

The script is hot-reloaded on save — edit `examples/fps_game.ptl` in another
window, and changes appear the next frame while player position, score, and
enemy HP are preserved (state is keyed by name).

## Agent / headless modes

Every run mode below forwards the same JSON-over-stdio protocol so an agent
can drive gameplay without a display.

### `--screenshot` — one-shot PNG

Runs N frames with fixed `dt = 1/60` and writes a PNG of the final frame.
Useful to quickly iterate on visuals:

```bash
cargo run --release -- --screenshot out.png --frames 60 examples/fps_game.ptl
```

### `--record` — flipbook

Writes one PNG per frame into a directory (after an optional warmup):

```bash
cargo run --release -- --record frames/ --frames 30 --warmup 0 examples/fps_game.ptl
```

### `--headless` — stdin-driven JSON protocol

No window. Commands arrive as JSON lines on stdin; responses go to stdout.
Each command starts paused at frame 0.

```bash
cargo run --release -- --headless examples/fps_game.ptl
```

Supported commands:

| Command | Fields | Effect |
|---------|--------|--------|
| `step` | `n` (default 1) | Run N frames. |
| `state` | – | Dump all `state` variables as JSON. |
| `set_state` | `name`, `value` | Override a state variable. |
| `input` | `keys_down[]`, `mouse{x,y,buttons[]}`, `mouse_delta{dx,dy}` | Inject input for the next frame. |
| `screenshot` | – | Return the current frame as a base64 PNG (+ draw stats). |
| `capture_draw_commands` | – | Return all draw commands for the next speculative frame. |
| `draw_stats` | – | Count triangles / lines / rects + depth range. |
| `pause` / `resume` | – | Toggle the run loop (agent mode only). |

Example: move the player, shoot, and grab a screenshot:

```bash
./target/release/petal-fps --headless examples/fps_game.ptl <<'EOF'
{"cmd":"step","n":1}
{"cmd":"set_state","name":"yaw","value":0.4}
{"cmd":"set_state","name":"player_x","value":-2.0}
{"cmd":"set_state","name":"player_z","value":0.0}
{"cmd":"input","mouse":{"x":400,"y":300,"buttons":[1]}}
{"cmd":"step","n":1}
{"cmd":"screenshot"}
EOF
```

### `--agent` — windowed + protocol

Same protocol as `--headless`, but also opens a window so you can watch the
agent play.

## Other flags

| Flag | Default | Meaning |
|------|---------|---------|
| `--width <n>` / `--height <n>` | 800 × 600 | Framebuffer size |
| `--title <str>` | "petal-fps" | Window title |
| `--no-hot-reload` | off | Disable the file watcher |
| `--frames <n>` | 60 | Frames for `--screenshot` / `--record` |
| `--warmup <n>` | 30 | Warmup frames before `--record` starts saving |

## Layout

```
petal-fps/
├── src/                        Rust host
│   ├── main.rs                 CLI entry point and arg parsing
│   ├── game_loop.rs            Run modes: game / agent / headless / screenshot / record
│   ├── framebuffer.rs          Software z-buffered triangle rasterizer
│   ├── renderer.rs             SDL2 streaming-texture blit
│   ├── commands.rs             DrawCommand enum (Petal → Rust)
│   ├── native_fns.rs           Petal-callable natives: dt, triangle3d, key_down, ...
│   ├── protocol.rs             JSON-over-stdio agent protocol
│   ├── input.rs                Keyboard / mouse state
│   ├── font.rs                 5×7 embedded bitmap font for HUD text
│   └── screenshot.rs           Draw-command → PNG encoding
└── examples/
    ├── fps_game.ptl            The full cyberpunk-city game
    ├── cyberpunk_city.ptl      Step-1 scaffold (camera + ground + one cube)
    ├── test_triangle.ptl       Minimal rasterizer smoke test
    └── debug_state.ptl         State-persistence repro / sanity check
```
