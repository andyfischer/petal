# petal-fantasy-nes — design

An **NES-style fantasy console** driven entirely by Petal. A "cart" is a `.ptl`
script; it defines the artwork (tiles, palettes), the maps, the sprites, the
music and sound effects, the menus and the gameplay. The Rust host supplies
only the two things a script cannot do fast enough or at all: a PPU-shaped
rasterizer and an APU-shaped sound chip, both fed from Petal every frame.

Tier: **integration** (a reusable host for many carts), built *on top of*
`integrations/petal-desktop-sdl` as a library — Shape B in
[docs/building-apps.md](../../../../docs/building-apps.md), the same way
`examples/custom-integrations/petal-fps` reuses that crate's window, event pump, timing,
hot reload, and agent/headless/screenshot modes.

```
petal (core) ── petal-ui (input) ── petal-sdl (loop) ── petal-fantasy-nes ── carts/*.ptl
```

## Hardware model

Deliberately NES-shaped, but not a cycle-accurate emulator: the constraints are
what make the aesthetic, the accuracy is not the point.

| | |
|---|---|
| Screen | 256×240, integer-scaled to the window (default 3×), optional scanline filter |
| Color | 64-entry fixed master palette; 8 4-color palettes (0–3 background, 4–7 sprite); color 0 of every palette is the shared backdrop |
| Tiles | 8×8, 2 bits per pixel, up to 512 in the pattern table |
| Background | a tile map up to 64×60 cells with wrapping scroll (`set_scroll`), plus per-scanline horizontal scroll for status-bar splits and parallax |
| Sprites | up to 64 per frame, x/y flip, behind-background priority, optional 8-per-scanline drop-out |
| Sound | 2 pulse (4 duties), 1 triangle, 1 noise (LFSR), 1 PCM/sample channel |
| Input | two 8-button pads (up/down/left/right/a/b/select/start), keyboard-mapped, gamepad when present |

Everything above is per-frame *state pushed from the script*: the script is
re-run top to bottom every frame (Petal's model), sprites are cleared each
frame, and tile/palette/map writes are idempotent, so a hot reload of the cart
mid-game repaints correctly with gameplay state preserved.

## Crate layout and file ownership

```
integrations/petal-fantasy-nes/
  Cargo.toml               package `petal-fantasy-nes`, bin `fantasy-nes`
  src/main.rs              CLI (arg parsing, run-mode dispatch)
  src/host.rs              `NesHost`: impl petal_sdl::Host — present, render_image, cart switching
  src/ppu/mod.rs           palettes, pattern table, tile map, OAM, rasterizer -> RGB
  src/ppu/palette.rs       the 64-entry master palette
  src/apu/mod.rs           the sound chip: channels, envelopes, mixing, resample
  src/apu/channels.rs      pulse / triangle / noise / PCM generators
  src/audio.rs             SDL AudioQueue plumbing, PCM sound bank, Petal DSP paths
  src/natives/mod.rs       registration entry point
  src/natives/video.rs     palette/tile/map/sprite/scroll natives
  src/natives/audio.rs     apu_*, register_sound, play_sound, enable_dsp natives
  src/natives/system.rs    pad_*, cart browser, log, config natives
  prelude/nes.ptl          the `nes` Petal module (implicit import for carts)
  carts/*.ptl              demo carts + the showcase game
  docs/                    design.md (this file), cart-authoring.md
  README.md
```

Each implementation task owns a disjoint set of these files. Nothing here
duplicates `petal-sdl`: the window, event loop, frame timing, hot reload,
agent/headless/screenshot/record protocol, and input translation are all
inherited.

## Script-facing native API

Names are the contract — implement exactly these. All coordinates are pixels
unless named `_cell`. Colors are indices into the 64-entry master palette.

### System
- `nes_version()` → int
- `log(msg)`
- `set_scale(n)`, `set_crt(on)` — window presentation only
- `cart_count()`, `cart_name(i)`, `cart_path(i)`, `launch_cart(path)` — the
  launcher cart (mirrors petal-sdl's `example_*` / `launch_script` natives)

### Palettes
- `set_palette(index, c0, c1, c2, c3)` — index 0–7
- `set_backdrop(c)` — universal color 0
- `master_rgb(c)` → packed `0xRRGGBB` int (for tooling/preview)

### Pattern table (artwork)
- `define_tile(index, rows)` — `rows` is 8 strings of 8 chars each, using
  `.`/`0`, `1`, `2`, `3` for the four palette entries, **or** 8 ints of packed
  2bpp data. Idempotent; safe to call every frame (the host hashes and skips
  unchanged writes).
- `define_tiles(base_index, list_of_row_lists)`
- `load_tiles_png(path, base_index)` — optional import path for external art

### Background map
- `set_map_size(w_cells, h_cells)` — up to 64×60, default 32×30
- `set_tile(x_cell, y_cell, tile, palette)`
- `get_tile(x_cell, y_cell)` → tile index
- `fill_map(tile, palette)`
- `set_scroll(x, y)`
- `set_scroll_at(scanline, x)` — per-scanline horizontal scroll override

### Sprites — cleared every frame, pushed every frame
- `sprite(x, y, tile, palette)`
- `sprite(x, y, tile, palette, flags)` — flags: 1 flip-x, 2 flip-y, 4 behind-bg
- `sprite_meta(x, y, tile_base, palette, w_tiles, h_tiles, flags)`
- `set_sprite_limit(on)` — emulate the 8-per-scanline drop-out (default off)

### Input
- `pad_down(pad, button)`, `pad_pressed(pad, button)`, `pad_released(pad, button)`
  where `button` is one of `"up" "down" "left" "right" "a" "b" "select" "start"`.
  Pad 0 = arrows/WASD + Z/X + Enter/RShift; pad 1 = IJKL + N/M.
  Raw keyboard/mouse natives from `petal-ui` remain available.

### Sound — chip channels (realtime, written every frame)
- `apu_pulse(ch, note, duty, volume)` — `ch` 0|1, `note` in MIDI semitones
  (float, so pitch bends work), `duty` 0–3, `volume` 0–15; `volume` 0 silences
- `apu_triangle(note, on)`
- `apu_noise(period, volume, mode)` — `period` 0–15, `mode` 0 long / 1 short
- `apu_mute()`

### Sound — Petal-synthesized PCM
- `register_sound(name, seconds, fn_name)` — the host calls the named Petal
  function as `fn_name(start_sample, count, sample_rate)` expecting a list of
  floats in −1..1, in blocks, and caches the result. Re-rendered automatically
  when the cart hot-reloads.
- `play_sound(name)` / `play_sound(name, volume)` / `stop_sound(name)`
- `enable_dsp(fn_name)` — opt-in **realtime** Petal synthesis: the same block
  signature, called once per frame for the next frame's worth of samples and
  mixed with the chip output. Budgeted: if the call overruns its slice the host
  prints a warning and fades the DSP bus out rather than glitching the frame.

## Audio transport

SDL's `AudioQueue<i16>` (stereo, 44 100 Hz), filled from the main thread once
per frame with a ~3-frame lead — no callback thread, so Petal's single-threaded
`Env` can synthesize into it directly. This is why realtime Petal DSP is
possible at all; a callback-based path would not be.

The chip mixer always runs (it is cheap Rust). `register_sound` renders ahead of
time at load. `enable_dsp` is the experimental realtime path, gated on the
benchmark in the first task.

## petal-sdl changes required

Kept small, generic, and back-compatible — `DefaultHost` and `petal-fps` must
keep working unchanged:

1. `Host::on_sdl_init(&mut self, sdl: &sdl2::Sdl)` — called after SDL init in
   every windowed run mode, so a host can open the audio device (default: no-op).
2. `Host::end_frame(&mut self, env: &mut Env)` — called after the script runs in
   *every* mode (interactive, agent, headless, screenshot), so a host can drain
   its own output buffers even when there is no window (default: no-op).
3. Gamepad: SDL `GameController` open/close + button/axis events translated in
   `input.rs` into the existing normalized key stream.

## Testing

- **Rust unit tests** — tile decoding, palette resolution, scroll wrapping,
  sprite priority/flip/limit, and rasterizer output against small hand-built
  golden frames; APU period tables, envelope shapes, LFSR sequence, and mixer
  level/resampling (measured by zero-crossing rate and RMS, no golden WAVs).
- **Cart smoke tests** — every cart in `carts/` runs 120 headless frames with no
  Petal error and a non-blank framebuffer.
- **Screenshots** — `--screenshot` for each cart, eyeballed and kept as a gallery.
- Carts live outside `examples/`, so the repo's `test-samples` corpus (which
  runs scripts on the bare `petal` CLI, without these natives) is unaffected.
