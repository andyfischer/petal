# petal-fantasy-nes — design

How the host is built. To write a cart, read
[cart-authoring.md](cart-authoring.md) instead; the [README](../README.md)
covers building, running and the run modes.

The console is a Shape B app on `integrations/petal-desktop-sdl` (see
[docs/building-apps.md](../../../../docs/building-apps.md)), the same way
`examples/custom-integrations/petal-fps` is. The window, event pump, frame
timing, input, hot reload and the agent/headless/screenshot/record modes are
all inherited from `petal-sdl`. The Rust here adds only the console itself: a
PPU-shaped rasterizer and an APU-shaped sound chip, both fed from Petal every
frame.

```
petal (core) ── petal-ui (input) ── petal-sdl (loop) ── petal-fantasy-nes ── carts/*.ptl
```

## Hardware model

Deliberately NES-shaped, but not a cycle-accurate emulator. The constraints are
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

Everything above is per-frame state pushed from the script. The cart is re-run
top to bottom every frame, sprites are cleared each frame, and tile, palette
and map writes are idempotent. A hot reload mid-game therefore repaints
correctly with gameplay state preserved.

## Crate layout

```
examples/custom-integrations/petal-fantasy-nes/
  Cargo.toml               package `petal-fantasy-nes`, bin `fantasy-nes`
  src/main.rs              CLI: arg parsing, run-mode dispatch to petal_sdl::run_*
  src/host.rs              `NesHost`: impl petal_sdl::Host (natives, present, render_image, cart switching)
  src/ppu/mod.rs           palettes, pattern table, tile map, OAM, rasterizer -> RGB
  src/ppu/palette.rs       the 64-entry master palette
  src/apu/mod.rs           the sound chip: channels, envelopes, mixing, resample
  src/apu/channels.rs      pulse / triangle / noise / PCM generators
  src/audio.rs             SDL AudioQueue plumbing, PCM sound bank, Petal DSP bus and its budget
  src/natives/mod.rs       registration entry point
  src/natives/video.rs     palette / tile / map / sprite / scroll natives
  src/natives/audio.rs     apu_*, register_sound, play_sound, enable_dsp, dsp_cost_ms
  src/natives/system.rs    pad_*, cart browser, log, presentation natives
  prelude/nes.ptl          the `nes` module: art, maps, sprites, text, pads, scenes, collision
  prelude/nes_sound.ptl    the `nes_sound` module: tracker, sound effects, PCM helpers
  carts/                   demo carts, the launcher, and the showcase game
  tests/carts.rs           runs every cart headless for 120 frames
```

Both prelude modules are registered as implicit imports, so carts call their
exports bare.

## Frame flow

Per frame, `petal-sdl` drives the host through its `Host` trait:

```
prepare_frame (PPU begin_frame) → cart runs → end_frame (drain video + audio
commands, pump the audio device) → after_frame (cart switch) → present
(rasterize + blit)
```

Natives do not touch the PPU or APU directly. They append commands to
per-frame buffers stored in the `Env`, and `end_frame` drains those buffers
into the console. That keeps the natives cheap and makes the same code path
work with or without a window: `end_frame` runs in every mode, so a headless
run and a windowed run agree frame for frame.

`Host::on_program_loaded` runs when a cart is loaded or switched. It rescans
`carts/` for the launcher, mutes the chip (channels are sticky by design, so a
held note would otherwise drone under the new cart), and resets the sound bank.

## Script-facing native API

The natives are the contract between the host and the preludes. All
coordinates are pixels unless named `_cell`. Colors are indices into the
64-entry master palette. Most carts use the prelude helpers rather than these
directly; the authoring guide's [API index](cart-authoring.md#api-index) lists
both layers.

### System
- `nes_version()` → int
- `log(msg)`
- `set_scale(n)`, `set_crt(on)` — window presentation only
- `cart_count()`, `cart_name(i)`, `cart_path(i)`, `launch_cart(path)` — for
  the launcher cart. `launch_cart` only works in the interactive loop, because
  `Host::after_frame` is only called there.

### Palettes
- `set_palette(index, c0, c1, c2, c3)` — index 0–7
- `set_backdrop(c)` — universal color 0
- `master_rgb(c)` → packed `0xRRGGBB` int

### Pattern table (artwork)
- `define_tile(index, rows)` — `rows` is 8 strings of 8 chars using
  `.`/`0`, `1`, `2`, `3` for the four palette entries, or 8 ints of packed
  2bpp data. Idempotent and safe to call every frame: the host hashes the
  rows and skips unchanged writes.
- `define_tiles(base_index, list_of_row_lists)`
- `load_tiles_png(path, base_index)` — cached on the file's mtime

### Background map
- `set_map_size(w_cells, h_cells)` — up to 64×60, default 32×30
- `set_tile(x_cell, y_cell, tile, palette)`, `get_tile(x_cell, y_cell)`
- `fill_map(tile, palette)`
- `set_scroll(x, y)`
- `set_scroll_at(scanline, x)` — per-scanline horizontal scroll override

### Sprites (cleared every frame)
- `sprite(x, y, tile, palette[, flags])` — flags: 1 flip-x, 2 flip-y, 4 behind-bg
- `sprite_meta(x, y, tile_base, palette, w_tiles, h_tiles, flags)`
- `set_sprite_limit(on)` — emulate the 8-per-scanline drop-out (default off)

### Input
- `pad_down(pad, button)`, `pad_pressed(pad, button)`, `pad_released(pad, button)`
  with `button` one of `"up" "down" "left" "right" "a" "b" "select" "start"`.
  The keyboard mapping is in the [README](../README.md#controls). A gamepad is
  translated by `petal-sdl` into the same normalized key stream, so the host
  never sees the difference. `petal-ui`'s raw keyboard and mouse natives remain
  available.

### Sound — chip channels (written every frame)
- `apu_pulse(ch, note, duty, volume)` — `ch` 0|1, `note` in MIDI semitones
  (float, so pitch bends work), `duty` 0–3, `volume` 0–15
- `apu_triangle(note, on)`
- `apu_noise(period, volume, mode)` — `period` 0–15, `mode` 0 long / 1 short
- `apu_mute()`

### Sound — Petal-synthesized PCM
- `register_sound(name, seconds, fn_name)` — the host calls the named cart
  function as `fn_name(start_sample, count, sample_rate)` in blocks and caches
  the result. The function returns an `f64_array` (the fast path) or a list of
  floats in −1..1. Re-rendered when the function changes under hot reload.
- `play_sound(name[, volume])`, `stop_sound(name)`
- `enable_dsp(fn_name)` — realtime Petal synthesis: the same block signature,
  called once per frame for the next frame's samples and mixed with the chip.
  `enable_dsp("")` turns it off.
- `dsp_cost_ms()` — what the last realtime call cost, so a cart can show it.

## Audio transport

Audio goes through SDL's `AudioQueue<i16>` (stereo, 44 100 Hz), filled from
the main thread once per frame with a lead of a few frames. There is no
callback thread. That is what makes realtime Petal synthesis possible: Petal's
single-threaded `Env` can synthesize straight into the queue, and a block that
runs long steals from the video frame rather than underrunning the device.

The chip mixer always runs; it is cheap Rust. With no audio device (headless,
screenshot, record) the chip and any DSP still run into a discarded buffer, so
every mode sees the same channel state.

### The DSP budget

Realtime synthesis in an interpreted language works because the per-frame
cost is small. Measured with `test/benchmarks/audio_synth.ptl` on the plain
`petal` CLI (release build, Apple M4), one 60 fps frame of audio (735 samples)
costs:

| Voice | µs / block | % of a 16.6 ms frame |
|---|---:|---:|
| Lean pulse (1 oscillator, multiplicative envelope) | 188 | 1.1% |
| Chip voice (square + triangle + noise, `pow` envelope) | 545 | 3.3% |
| The same chip voice returning a boxed list | 849 | 5.1% |
| Rich voice (3 detuned saws, state-variable filter, LFO) | 733 | 4.4% |

From that, the host enforces a **2 ms per frame budget** (`DSP_BUDGET_MS` in
`src/audio.rs`), enough for roughly eight lean voices or three full chip
voices. The cost is measured around the whole call, including converting the
returned buffer to `i16`, and smoothed over several frames. Only when the
smoothed average stays over budget for consecutive frames does the host warn
and fade the DSP bus out; it fades back in when the cost drops under budget
again. Re-enabling DSP resets the guard, so an edited cart gets a fresh chance.

Three findings behind that design:

- **The block buffer should be an `f64_array`, not a list.** The same
  synthesis math costs about 56% more when built with `append`, and the
  allocation drags the collector into the audio slice. Both forms are
  accepted; the docs show the array.
- **Native calls dominate cheap per-sample work.** A native invocation costs
  about as much as eight interpreted instructions, so `pow` or `float` per
  sample should be hoisted to per-block or written as an accumulator.
- **The interpreter must be optimized even in debug builds.** An unoptimized
  `petal` runs the same block about 20× slower, which is why `Cargo.toml`
  sets `opt-level = 3` for the `petal` dependency under `profile.dev`.

Rendering ahead of time (`register_sound`) runs at the same rate: a 0.3 s
effect takes about 10 ms, so a cart with a dozen effects pays a short, visible
hitch at load and on each hot reload.

## What petal-sdl provides for this

Three hooks in `petal-sdl` exist for hosts like this one; all default to no-ops
so `DefaultHost` and `petal-fps` are unaffected:

1. `Host::on_sdl_init(&mut self, sdl)` — after SDL init in every windowed mode,
   so the host can open the audio device.
2. `Host::end_frame(&mut self, env)` — after the script runs in every mode,
   so the host can drain its command buffers without a window.
3. `Host::on_program_loaded(&mut self, env, path)` — when a script is loaded
   or switched.

Gamepads are opened by `petal-sdl` and translated into the normalized key
stream in its `input.rs`.

## Testing

- **Rust unit tests** in `#[cfg(test)]` blocks inside the modules: tile
  decoding, palette resolution, scroll wrapping, sprite priority/flip/limit,
  and rasterizer output against small hand-built frames; APU period tables,
  envelope shapes, LFSR sequence, mixer level and resampling; the DSP budget
  guard tripping and recovering.
- **Cart smoke tests** (`tests/carts.rs`): every cart in `carts/`, plus each
  `carts/<name>/game.ptl`, runs 120 headless frames with no Petal error and a
  non-blank framebuffer.
- Carts live outside the repo's script corpus, so the `test-samples` run
  (which uses the bare `petal` CLI, without these natives) is unaffected.
