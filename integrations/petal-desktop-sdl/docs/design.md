# How petal-sdl is built

This page describes the structure of the `petal-sdl` crate: how the game loop,
the `Host` trait, and the run modes fit together, and what the shipped binary
adds on top of `petal-ui`.

For other material:

- [`../README.md`](../README.md) — building, running, CLI flags, examples
- [`game-dev-guide.md`](game-dev-guide.md) — the script-side API and common game patterns
- [`agent-protocol.md`](agent-protocol.md) — the `--agent` / `--headless` command reference
- [`docs/building-apps.md`](../../../docs/building-apps.md) — how to build your own app on this crate
- [`examples/custom-integrations/petal-fps/README.md`](../../../examples/custom-integrations/petal-fps/README.md) — a Rust + Petal 3D app that implements its own `Host`

## Layers

The crate is a library (`petal_sdl`) plus a thin binary (`petal-sdl`,
`src/main.rs`) that parses flags and picks a run mode.

- **The game loop** (`src/game_loop.rs`) owns platform policy: SDL init, the
  window and canvas, the event pump, frame timing, hot reload, pointer grab,
  and the run modes. It knows nothing about how a frame is painted.
- **The `Host` trait** (also in `game_loop.rs`) is what the loop drives for the
  per-app parts: registering natives, presenting a frame, rendering a frame to
  an image, and a few optional hooks.
- **`DefaultHost`** (`src/default_host.rs`) is the host the shipped binary
  runs. It paints the `petal-ui` draw vocabulary onto an SDL canvas and adds
  the example browser, sandboxed file I/O, and `sfx`/`synth` sound natives.
- **Building blocks** are public so other hosts can compose them: `input`
  (SDL event translation and gamepad folding), `audio` (queued sample
  output), `sfx` (sfxr-style synthesis), `sound` (the sound natives, a voice
  mixer, and the `SoundBoard` that drives them), `protocol` (agent JSON), `watcher` (hot reload), `screenshot` (PNG
  encoding), `font` (size ladder), and `renderer` (SDL canvas primitives).

The draw functions (`draw_rect`, `draw_text`, offscreen canvases, ...) and
input functions (`key_down`, `mouse_x`, ...) are not defined here. They come
from `petal-ui`, which every graphical Petal host shares, so scripts written
for `petal-sdl` run unchanged under `petal-web-canvas`. See
[`petal-ui/README.md`](../../../petal-ui/README.md).

## Run modes

| Function | CLI | What it does |
|----------|-----|--------------|
| `run_game` | default | Window, interactive, hot reload |
| `run_agent` | `--agent` | Window plus JSON commands on stdin |
| `run_headless` | `--headless` | No window; starts paused; `step` advances frames |
| `run_screenshot` | `--screenshot` | Headless; runs N frames, writes a PNG, exits |
| `run_record` | (library only) | Headless; renders a sequence of frames to images |

Headless, screenshot, and record never initialize SDL.

## The `Host` trait

Required:

- `register(&mut env)` — register the host's natives and modules into a fresh `Env`.
- `present(&mut canvas, &mut env)` — paint the live frame's draw output to the window. Windowed modes only.
- `render_image(&mut env, stack, w, h)` — rasterize a frame to an RGB image with no window. Used by `--screenshot`, record mode, and the agent `screenshot` command.

Optional, with defaults:

- `default_source()` — the program to run when the CLI got no path. `DefaultHost` returns the example browser.
- `on_program_loaded(&mut env, path)` — bind host state after each load or reload.
- `prepare_frame(&mut env)` — reset per-frame bindings before the script runs.
- `draw_commands_json(&mut env, stack)` — serialize draw output for `capture_draw_commands`.
- `draw_stats(&mut env, stack)` — per-frame statistics for the agent `draw_stats` command. `DefaultHost` does not implement it.
- `on_escape(&mut env)` — what Escape does in the window. Default: quit.
- `after_frame(&mut env)` — request a script switch after an interactive frame. The example browser uses this.
- `on_sdl_init(&sdl)` and `end_frame(&mut env)` — see below.

### Frame order

```text
prepare_frame → env.run → drain print output → end_frame → [after_frame → present]
```

`present` only runs when there is a window. `end_frame` runs after every
committed frame in every mode, which makes it the right place to flush a
host's own per-frame output: a block of audio, a network packet, a trace
record. It is not called for speculative frames (the forked runs behind the
agent's `screenshot` and `capture_draw_commands` commands), because those
frames are read and discarded.

### `on_sdl_init`

Called right after `sdl2::init()` in the two windowed modes. Use it to open
subsystems the loop does not own, above all an audio device. The headless
modes never call it, so anything opened here must be optional.

### Audio

`audio::AudioOutput` wraps SDL's `AudioQueue<i16>`: `open(sdl, sample_rate,
channels, buffer_frames)`, `queue_samples(&[i16])` (interleaved),
`queued_frames()`, and `resume` / `pause` / `clear`. It is transport only; the
host owns synthesis.

It is a queue rather than a callback on purpose. Petal's `Env` is
single-threaded and lives on the main thread, so a callback on SDL's audio
thread could never call into a script. With a queue the host synthesizes
during its own frame (normally in `end_frame`) and tops the device up using
`queued_frames()` to decide how much. That is what lets a script synthesize
its own audio.

The device opens paused and reports the rate and channel count it actually
got, which may differ from the request.

### Sound effects

`sfx` is a pure synthesis module ported from cheesecake's sfxr-style synth:
`render_sfx(&SfxParams, sample_rate) -> Vec<f32>` in physical units (Hz,
seconds, octaves per second), eight presets (`jump`, `hit`, `pickup`/`coin`,
`explosion`, `blip`, `laser`, `powerup`, `bounce`) with deterministic xorshift
variants, and a render cache keyed by a hash of the parameters. Output is
deterministic, always finite, never above a peak of 1.0, and ends in a 3 ms
fade. No SDL, no `Env`.

`sound` puts it in front of scripts. `sfx(name, opts?)` and `synth(params)`
validate their arguments and push the request into the `sfx` output buffer;
they do nothing else during the run, so they are registered as
`NativeEffects::EMITS`. That is the honest row, and it matters: a memoized
helper that plays a sound is replayed by re-appending its buffered output, so
the request still arrives. (A native that queued the sound on a host-side list
instead would be skipped by the replay and would have to declare an `effect`;
see "Embedder pitfalls" in [`docs/ffi.md`](../../../docs/ffi.md).) Variant
rotation happens on the host for the same reason.

After the run, `SoundBoard::end_frame` drains the buffer, renders each request
through the cache, starts a `Mixer` voice (linear-interpolated `pitch`, linear
`pan`), and tops the `AudioOutput` queue up to about 50 ms of lead. The
`DefaultHost` wires it in: `on_sdl_init` opens the device, `end_frame` drives
the board, and `frame_gating` answers `false` while a voice is playing, since a
gated frame never reaches `end_frame` and the queue would run dry. In the
headless modes there is no device: requests are still drained and counted, and
nothing is rendered.

### Gamepads

Controllers have no separate script API. They are folded into the same key
stream as the keyboard, so `key_down("left")` works from a d-pad or the left
stick and a game's keyboard and pad bindings cannot drift apart.

`Gamepads::new(&sdl)` opens the controller subsystem (inert if unavailable)
and `poll_sdl_events_with_gamepads` handles hot-plug each frame. The windowed
modes do this already; `poll_sdl_events` stays keyboard and mouse only.

Pad slots are assigned in connection order and survive a disconnect, so
unplugging player 2 does not promote them to player 1.

| Control | Slot 0 | Slot 1 |
|---|---|---|
| d-pad / left stick | `up` `down` `left` `right` | `i` `k` `j` `l` |
| south (A) | `z` | `n` |
| east (B) | `x` | `m` |
| west (X) | `c` | `comma` |
| north (Y) | `v` | `period` |
| start | `return` | — |
| back / select | `shift` | — |
| shoulders | `leftbracket` `rightbracket` | — |

Slots 2 and up are ignored. The left stick is quantized to the four direction
keys with hysteresis, so a stick resting on a boundary does not chatter. The
right stick and triggers are not mapped.

## Natives specific to this host

Everything in the [game dev guide](game-dev-guide.md) comes from `petal-ui`.
`DefaultHost` adds these, which are only available under `petal-sdl`:

| Function | Description |
|----------|-------------|
| `example_count()` | Number of bundled examples |
| `example_name(i)` | Display name of example `i` |
| `example_path(i)` | Absolute `.ptl` path of example `i` |
| `launch_script(path)` | Replace the running program with the one at `path` |
| `load_text_file(path)` | Read a text file; returns `""` if missing |
| `save_text_file(path, text)` | Write a text file; returns `true` on success |
| `file_exists(path)` | Whether the file exists |

`examples/browser.ptl` is built on the first four. The file functions are
sandboxed: paths must be relative and may not contain `..`, and they resolve
against the working directory.

## Hot reload

The source file is watched by default. On save, the program is recompiled and
restarted with `state` values preserved. Disable with `--no-hot-reload`. How
`state` inside functions survives an edit is covered in the
[game dev guide](game-dev-guide.md#hot-reload).
