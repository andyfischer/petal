# petal-fantasy-nes

An **NES-style fantasy console** whose cartridges are Petal scripts. A cart is
one `.ptl` file: it defines the artwork, the maps, the sprites, the music and
sound effects, the menus and the gameplay. The Rust host supplies only the two
things a script cannot do fast enough or at all — a PPU-shaped rasterizer and an
APU-shaped sound chip — both fed from Petal every frame.

It is a Shape B app on the [`petal-sdl`](../petal-desktop-sdl/) integration (see
[docs/building-apps.md](../../docs/building-apps.md)): the window, event loop,
frame timing, input, hot reload, and the agent/headless/screenshot/record modes
are all inherited, and only the console itself is new.

```
petal (core) ── petal-ui (input) ── petal-sdl (loop) ── petal-fantasy-nes ── carts/*.ptl
```

To write a cart, read [docs/cart-authoring.md](docs/cart-authoring.md) — it is
the whole authoring surface and assumes no Rust. [docs/design.md](docs/design.md)
is the host's own design. [LANGUAGE_NOTES.md](LANGUAGE_NOTES.md) is the honest
report on what Petal was good and bad at while this was built.

## Hardware model

Deliberately NES-shaped, but not a cycle-accurate emulator: the constraints are
what make the aesthetic; the accuracy is not the point.

| | |
|---|---|
| Screen | 256×240, integer-scaled to the window (default 3×), optional scanline filter |
| Color | 64-entry fixed master palette; 8 four-color palettes (0–3 background, 4–7 sprite); color 0 of every palette is the shared backdrop |
| Tiles | 8×8, 2 bits per pixel, up to 512 in the pattern table |
| Background | a tile map up to 64×60 cells with wrapping scroll, plus per-scanline horizontal scroll for status-bar splits and parallax |
| Sprites | up to 64 per frame, x/y flip, behind-background priority, optional 8-per-scanline drop-out |
| Sound | 2 pulse (4 duties), 1 triangle, 1 noise (LFSR), 1 PCM channel synthesized in Petal |
| Input | two 8-button pads (up/down/left/right/a/b/select/start), keyboard-mapped, gamepad when present |

All of it is per-frame state *pushed from the script*: the cart re-runs top to
bottom every frame, sprites are cleared each frame, and tile/palette/map writes
are idempotent — which is why a hot reload mid-game repaints correctly with
gameplay state preserved.

## Prerequisites

- Rust (any recent stable toolchain)
- SDL2:
  - macOS: `brew install sdl2`
  - Debian/Ubuntu: `sudo apt-get install libsdl2-dev`

## Build

From this directory:

```bash
LIBRARY_PATH=/opt/homebrew/lib cargo build --release
```

**The `LIBRARY_PATH=/opt/homebrew/lib` prefix is required on Homebrew macOS**,
on every cargo invocation for this crate (`build`, `run`, `test`) — it is where
the linker finds SDL2. On Linux, drop it. The first build is slow (it compiles
the Petal compiler in `../../rust`); later builds are quick.

## Run

```bash
LIBRARY_PATH=/opt/homebrew/lib cargo run --release                        # boot into the launcher
LIBRARY_PATH=/opt/homebrew/lib cargo run --release -- carts/hello.ptl     # boot a cart directly
LIBRARY_PATH=/opt/homebrew/lib cargo run --release -- carts/hello.ptl --scale 4 --crt
```

With no cart named, the console boots `carts/launcher.ptl`, a menu of every
`.ptl` at the top level of `carts/` — so a new cart appears in the menu the
moment you save it. Escape returns from a cart to the launcher, and quits from
the launcher.

Edit the running cart in another window and it hot-reloads on the next frame,
with `state` — score, position, the music playhead — preserved.

| Flag | Default | Meaning |
|------|---------|---------|
| `--scale <n>` | 3 | Integer pixel scale (1–8) |
| `--crt` | off | Scanline filter |
| `--no-hot-reload` | off | Disable the file watcher |
| `--agent` | off | Windowed + JSON protocol on stdin/stdout |
| `--headless` | off | Same protocol, no window (implies `--agent`) |
| `--screenshot <path>` | — | Run N frames, write a PNG, exit |
| `--record <dir>` | — | Write one PNG per frame into a directory |
| `--frames <n>` | 60 | Frames for `--screenshot` / `--record` |
| `--warmup <n>` | 30 | Warmup frames before `--record` starts saving |

## Controls

| Pad 0 | Pad 1 | |
|---|---|---|
| Arrows / WASD | I J K L | d-pad |
| Z or C | N | A |
| X or V | M | B |
| Shift / Tab | — | Select |
| Enter | — | Start |

A connected gamepad drives pad 0 through the same normalized key stream, so
nothing in a cart has to know the difference. Escape returns to the launcher.

## Carts

| Cart | |
|---|---|
| `hello.ptl` | The smallest cart that exercises the whole console: a palette, two tiles, a floor, one sprite walked with the d-pad. |
| `palette.ptl` | The 64-color master palette with its indices, plus color cycling and a fade routine. |
| `sprites.ptl` | Everything the sprite layer does, on one screen: metasprites, both flip bits, behind-background priority, and what happens past the 64-sprite cap. |
| `scroll.ptl` | Wrapping scroll, per-scanline parallax, and a status bar locked above a scrolling world. |
| `music_demo.ptl` | The tracker driver with its own playhead on screen — the pattern grid, the row it is on, and what each of the four channels resolved to. |
| `sound_lab.ptl` | The four chip channels with the registers exposed as knobs: pitch, duty, volume, noise period, all written directly with `apu_*`. |
| `dsp_lab.ptl` | Sound synthesized in Petal, both ways round — `register_sound` rendered ahead of time versus `enable_dsp` in realtime, with the cost of each measured. |
| `tile_editor.ptl` | An 8×8 tile editor that runs on the console and prints the `define_art(...)` source of what you drew — the art tool is a cart. |
| `launcher.ptl` | The boot menu, and itself an ordinary cart — it is written in exactly the API a game uses. |
| `petal_quest/game.ptl` | **The showcase game**: a side-scrolling platformer — three worlds, four enemy kinds, a boss, a status-bar split — with its artwork in `petal_quest/art.ptl` and its levels in `petal_quest/levels.ptl`. It lives in a subdirectory, so run it by path: `cargo run --release -- carts/petal_quest/game.ptl`. |

### Known gap

`sfx_play` / `drum` currently raise `Unknown builtin: has_field` from inside
`prelude/nes_sound.ptl`. The core `std` prelude (where `has_field` lives) is
merged into the *cart*, but not into a host prelude module, so the sound-effect
layer cannot call it — carts can. Music, the chip natives, and every PCM path
are unaffected. Details and the workaround are in
[cart-authoring.md](docs/cart-authoring.md#sound-effects-and-drums).

## Agent, headless and screenshot modes

Every mode below speaks the same JSON-over-stdio protocol as `petal-sdl`, so an
agent can play a cart with no display, dump the cart's `state`, and take
pictures. Full command table:
[agent-protocol.md](../petal-desktop-sdl/docs/agent-protocol.md).

One PNG after ten frames:

```bash
LIBRARY_PATH=/opt/homebrew/lib cargo run --release -- \
  --screenshot out.png --frames 10 carts/hello.ptl
```

A flipbook, one PNG per frame:

```bash
LIBRARY_PATH=/opt/homebrew/lib cargo run --release -- \
  --record frames/ --frames 30 --warmup 0 carts/scroll.ptl
```

Driving a cart headlessly — step frames, inject pad input, read state back, grab
a base64 PNG:

```bash
./target/release/fantasy-nes --headless carts/hello.ptl <<'EOF'
{"cmd":"step","n":3}
{"cmd":"input","keys_down":["right"]}
{"cmd":"step","n":30}
{"cmd":"state"}
{"cmd":"screenshot"}
EOF
```

`--agent` is the same protocol with a window open, so you can watch the agent
play. Pad input is injected as the keys from the [Controls](#controls) table
(`"right"`, `"z"`, `"return"`, …). The `state` dump carries the cart's own `state`
variables plus the prelude's, which are namespaced (`nes::_nes_scroll`, …):

```json
{"ok":true,"state":{"hero_x":180,"hero_y":200,
                    "nes::_nes_backdrop":33,"nes::_nes_map":{"h":30,"w":32}}}
```

A screenshot never perturbs a running game: the capture frame runs on a forked
stack and is applied to a clone of the console, and its audio is dropped.

Two things behave differently outside a window, both inherited from
`petal-sdl`: there is no audio device (the chip and any Petal DSP still run,
into a discarded buffer), and `launch_cart` — the launcher handing the machine
to a cart — does nothing, because the loop's cart-switch hook is windowed-only.
Point the command line straight at the cart you want to drive.

## A cart, in full

```petal
set_backdrop(light_blue)                       // the sky — every palette's color 0
palette(0, dark_green, green, white)           // background palette 0
palette(4, maroon, red, peach)                 // sprite palette 4

define_art(1, ["oooooooo", "-o--o-o-", "--------", "---o----",     // grass
               "--------", "-------o", "--------", "-o------"])
define_art(2, ["..----..", ".-####-.", "-#-##-#-", "-######-",     // the hero
               "-#-##-#-", ".-####-.", "..oooo..", ".o.oo.o."])

set_map_size(32, 30)
map_rect(0, 26, 32, 4, 1, 0)                   // a floor along the bottom four rows

state var x = 120.0
set x = clamp(x + btn_dx() * 1.5, 0, 248)      // arrows / WASD / a d-pad

text_sprites_center(24, "HELLO", 4)
sprite(px(x), 200, 2, 4)
```

That is a complete, running cart: a green field, a hero you can walk left and
right, and the word HELLO. Save it as `carts/mine.ptl` and it is in the launcher
menu.

## Layout

The Rust here is only the console's delta; the loop, input, protocol, PNG
encoding and hot reload come from `petal-sdl`.

```
petal-fantasy-nes/
├── src/
│   ├── main.rs           CLI: parse args → petal_sdl::run_*
│   ├── host.rs           NesHost: impl petal_sdl::Host (natives, console state, present)
│   ├── ppu/              palettes, pattern table, tile map, OAM, the rasterizer
│   ├── apu/              pulse / triangle / noise / PCM generators and the mixer
│   ├── audio.rs          SDL audio queue, the PCM sound bank, the Petal DSP path
│   └── natives/          the cart-facing native set (video, audio, system)
├── prelude/
│   ├── nes.ptl           the `nes` module: art, maps, sprites, text, pads, scenes, collision
│   └── nes_sound.ptl     the `nes_sound` module: a tracker, sound effects, PCM helpers
├── carts/                the carts, and the launcher
└── docs/
    ├── cart-authoring.md how to write a cart
    └── design.md         how the host is built
```

Both prelude modules are implicit imports, so a cart calls everything in them
bare. They are ordinary Petal — worth reading when a helper does not do quite
what you want, because the answer is usually one native call underneath.
