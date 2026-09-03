# petal-ui

The standard UI layer for Petal programs. It is two things:

- a small Rust crate that every graphical host embeds, providing the input and
  draw natives, and
- the `ui` prelude module (`prelude/ui.ptl`), the widget library that Petal
  scripts build interfaces with.

Hosts that embed it: Garden panels (`garden/garden-script`),
`integrations/petal-desktop-sdl` (crate and binary still named `petal-sdl`),
`integrations/petal-web-canvas` (which also serves `diagram-canvas`), the
`petal-fps` and `petal-fantasy-nes` examples under
`examples/custom-integrations/`, standalone app embedders outside this repo
(the todo app, worlds-fair), and the headless `petal-ui-run` CLI in this crate.

## The model

Immediate mode: the script's whole top level re-runs every frame.

```
input events → bind_frame_info / bind_input → clear_draw_commands
→ env.run(script) → take_draw_commands → rasterize
```

- **Input** (`src/input.rs`). `InputEvent` and `InputState`, plus the polling
  natives scripts call each frame: `mouse_x`, `key_pressed`, `text_input`,
  `dt`, `time`, and so on. There are no callbacks. Relative pointer motion is
  available as `mouse_dx()` / `mouse_dy()`, and a script can lock the pointer
  with `grab_mouse()` / `release_mouse()`; a host that supports pointer lock
  drains the request with `input::take_mouse_grab` after each frame.
- **Draw** (`src/draw.rs`). Natives such as `draw_rect`, `draw_text`,
  `fill_arc`, `draw_rect_gradient`, `draw_shadow` and `clip_push` append
  `DrawCommand`s to a buffer. The host drains the buffer and turns it into
  pixels; it implements rasterization only. A host that cannot do something
  should degrade it (fill a gradient with one stop, scissor a rounded clip
  square) rather than drop the command.
- **Tessellation** (`src/tess.rs`). CPU geometry hosts should not re-derive.
  `shadow_mesh` turns a shadow command into one non-overlapping triangle
  list, because a translucent shadow built from overlapping pieces shows
  every seam.
- **The `ui` prelude** (`prelude/ui.ptl`). Registered as an implicit import,
  so scripts call `button(...)`, `list_update(...)`, `checkbox(...)` bare.
  Implicit bindings are weak: a script's own `fn button` shadows the
  prelude's.

See [docs/components.md](docs/components.md) for the full reference: theme,
layout, every widget, motion helpers, draw primitives and layers.

For a component layer *above* this one, written in Petal rather than Rust, see
[`petal-libs/bloom`](../petal-libs/bloom/README.md): buttons, menus, controls
and overlays that animate by default, built on these same primitives. The two
compose in one script.

## Embedding

```rust
petal_ui::register_all(&mut env);        // input + draw + canvas + host_data natives
petal_ui::register_prelude(&mut env);    // the `ui` module, as an implicit import

// each frame:
petal_ui::input::bind_frame_info(&mut env, dt, frame);
petal_ui::input::bind_dimensions(&mut env, w, h);
petal_ui::input::bind_input(&mut env, &input_state);
petal_ui::draw::clear_draw_commands(&mut env);
env.run(...)?;
let commands = petal_ui::draw::take_draw_commands(&mut env);
```

To make the widgets paint in the host's own colors, publish a palette once at
startup (or every frame, if it can change live):

```rust
petal_ui::input::bind_host_palette(&mut env, &[
    ("window_bg", [13, 17, 23, 255]),
    ("text", [230, 237, 243, 255]),
    ("accent", [88, 166, 255, 255]),
    // ... the rest of the host's palette vocabulary
]);
```

`ui_theme()` then resolves in this order: an explicit `theme_set` in the
script, then the host palette, then the built-in dark default. Garden binds
its full palette, so prelude widgets look native there with no script-side
setup.

## Running scripts headlessly

```
cargo run --bin petal-ui-run -- app.ptl --frames 60 --scenario monkey:1 --seed 1
```

`petal-ui-run` runs a UI script without a window, prints the draw commands
and state as JSON, and replays deterministic input scenarios. See
`docs/dev/headless-ui-run.md` at the repo root. The showcase script
`garden/examples/panels/gallery.ptl` exercises every component and runs both
headlessly and as a Garden panel.

## Versioning

- `UI_VERSION` counts incompatible changes to the input/draw native contract.
  Scripts can read it with `ui_version()`.
- `PRELUDE_LEVEL` counts additions to the prelude's export surface. Levels
  are additive, so a script written against an older level keeps working.
  `garden --version --json` lists the exact exports compiled into a binary
  under `prelude.exports`.

## Tests

```
cargo test
```

`tests/widgets.rs` covers the component library through `harness::Headless`:
synthetic clicks and keys, then assertions on state and draw commands.
`tests/prelude.rs` covers the older prelude surface and compatibility
(shadowing, style records). `harness::Headless` is public, so you can test
your own panels the same way.
