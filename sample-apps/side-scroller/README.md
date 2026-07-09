# Side-Scroller (Petal experiment)

A 2D side-scrolling platformer written almost entirely in Petal, hosted by
the [`petal-desktop-sdl`](../../integrations/petal-desktop-sdl/README.md) integration
(the `petal-sdl` runtime). Built as an experiment to stress-test
Petal in a real, non-trivial use case and produce a debrief on what worked
and what did not — see [`DEBRIEF.md`](./DEBRIEF.md).

## Layout

```
side-scroller/
  game.ptl          gameplay (title, level select, play, pause, win, lose)
  editor.ptl        level editor with mouse placement, save/load
  levels/
    01_first_steps.lvl
    02_cavern.lvl
    03_sky_ruins.lvl
  run-game.sh       launcher
  run-editor.sh     launcher
  README.md
  DEBRIEF.md
```

The "hybrid" Rust/Petal split:

- **Rust side** (`integrations/petal-desktop-sdl/`) provides the engine: SDL window, input,
  rendering, frame loop, hot reload, and the host-function bridge. We
  added three new natives here: `load_text_file`, `save_text_file`,
  `file_exists` (used by the editor and level loader).
- **Petal side** (`game.ptl`, `editor.ptl`) is everything else: physics,
  camera, enemies, HUD, particles, parallax, level format, editor logic.

## Running

```bash
# from the repo root, build the host once:
( cd integrations/petal-desktop-sdl && cargo build )

# play
./sample-apps/side-scroller/run-game.sh

# edit
./sample-apps/side-scroller/run-editor.sh
```

You can also pass the script directly:

```bash
./integrations/petal-desktop-sdl/target/debug/petal-sdl sample-apps/side-scroller/game.ptl --width 960 --height 600
```

## Game controls

| Key | Action |
|---|---|
| `Arrows` / `WASD` | Move |
| `Space` / `W` / `Up` | Jump (variable height — tap for a hop, hold for a leap) |
| `R` | Restart current level |
| `Esc` / `P` | Pause |
| `Q` (in pause/win/lose) | Back to title |
| `Enter` | Confirm menu / next level |

## Editor controls

| Key | Action |
|---|---|
| `1`..`9` | Choose tool: plat, oneway, coin, spike, goomba, jumper, check, start, goal |
| `Left-click` | Place. For `plat` / `oneway`, drag to size. |
| `Right-click` | Delete the object under cursor |
| `Arrows` (+ shift) | Pan camera |
| `G` | Toggle grid snap |
| `[` / `]` | Switch between level slots 1–3 |
| `S` | Save the current level back to its slot file |
| `L` | Reload from disk |
| `N` | New empty level |

## Level file format

Plain text, one entity per line. Comments not supported; unknown tags ignored.

```
name First Steps
width 2400
bg 110 180 240
start 100 450
goal 2300 370
plat 0 550 2400 50          # x y w h (solid)
oneway 820 400 160          # x y w   (drop-through from above)
coin 240 430                # x y
spike 900 540               # x y     (each is 24px wide)
goomba 1280 400 80          # x surface_y patrol_radius
jumper 1560 340 0           # x surface_y unused
check 1400 500              # x y     (mid-level checkpoint)
```

The exact same parser is used by `game.ptl` and `editor.ptl` — see the
debrief for why this is duplicated rather than imported.
