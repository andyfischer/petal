# petal-ui

The standard UI layer for Petal programs. It is two things:

- a small Rust crate that every graphical host embeds, providing the input and
  draw natives, and
- the `ui` package (`prelude/`), which declares those natives and holds the
  widget library that Petal scripts build interfaces with. A script writes
  `import ui`.

Hosts that embed it: Garden panels (`garden/garden-script`),
`integrations/petal-desktop-sdl` (crate and binary still named `petal-sdl`),
`integrations/petal-web-canvas` (which also serves `diagram-canvas`), the
`petal-fps` and `petal-fantasy-nes` examples under
`examples/custom-apps/`, standalone app embedders outside this repo
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
- **The native interface** (the top of `prelude/ui.ptl`, and
  `prelude/input.ptl`). Every native this
  crate registers is declared there — `pub native fn mouse_x() -> int`,
  `native fn _native_rect(x: num, …, a?: num) = "draw_rect"` — which is where
  its parameter names, optional parameters, argument types and result type
  come from: `petal check` reads the declarations without registering
  anything, and the natives bind named arguments by them
  (`petal_ui::interface()`). A script that imports `ui` loads only into an
  `Env` that registers the natives it refers to; a test holds the declarations
  to `register_all`, both ways.
- **The `ui` package** (`prelude/ui.ptl`, `prelude/input.ptl`). A script
  writes `import ui` and calls `button(...)`, `list_update(...)`,
  `checkbox(...)` bare (or `ui.button`). Without the import none of these
  names exist. A plain import is weak: a script's own `fn button` shadows
  the package's.
  `import ui/input` is the input, timing and frame half alone, for a host
  that registers only `petal_ui::input::register_input_module`. The `petal` binary
  bundles the package, so `petal check` resolves `import ui` with no host;
  `petal run` refuses such a script at load, because it registers none of
  these natives. The package's own source imports the builtin packages it
  uses (`color`, `creative`, `io`). It also wraps every draw native in record overloads
  (`draw_rect(rect, c)`, `draw_line(p1, p2, c, a, width)`), and each shape's
  parameters carry plain names — `x, y, w, h` / `rect`, `cx, cy` / `center`,
  `radius`, `r, g, b` / `c`, `a`, `width` — so a call can be written by name:
  `draw_rect_outline(x: 0, y: 0, w: 10, h: 4, c: red, width: 2)`. The
  signatures are tabulated in
  [docs/components.md](docs/components.md#draw-primitives--fills-and-strokes).

See [docs/components.md](docs/components.md) for the full reference: theme,
layout, every widget, motion helpers, draw primitives and layers.

For a component layer *above* this one, written in Petal rather than Rust, see
[`core-runtime/bloom`](../../core-runtime/bloom/README.md): buttons, menus, controls
and overlays that animate by default, built on these same primitives. The two
compose in one script.

## Embedding

```rust
petal_ui::register_all(&mut env);        // input + draw + canvas + host_data natives
// (register_all includes register_prelude: the `ui` package, `import ui`)
assert!(env.verify_interface(petal_ui::INTERFACE_PACKAGES).is_ok()); // drift test
// register_all also announces the interface: env.provides("ui", <the package's version>),
// which is what lets packages that state `[requires] ui = ">=0.1"` (bloom, text_layout) load.
// A host that registers the pieces itself calls petal_ui::provide_interface(&mut env)
// once all of them are in; a host with only the input natives (`import ui/input`) does not.

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
its full palette, so the widgets look native there with no script-side
setup.

## Layers, hit testing and retained drawing

Three things the prelude can say that a painter's-order command list cannot,
each resolved inside this crate so a host that knows nothing about them still
draws the right picture:

- **Layers** (`ui_layer`, `with_layer`, `ui_overlay`). `take_draw_commands`
  sorts a frame by layer before handing it over (`src/compose.rs`).
- **Hit testing** (`interact`, `ui_modal`, `pointer_captured`,
  `ui_target_rect`). Widgets register their rects; `bind_input` turns one
  frame's registrations into the registry the next is tested against
  (`src/hit.rs`). A host reads the same registry to click a widget by id:
  `hit::targets(env)`, `Headless::click_target("Save")`.
- **Retained drawing** (`cached_layer`, `draw_rects`). Lowered to the classic
  vocabulary by default; a host with render targets calls
  `compose::set_host_caps` and receives `CacheBegin` / `CacheEnd` /
  `CacheDraw` and `Rects` instead.

The script side is in [docs/ui-reference.md](../../docs/ui-reference.md), the
host side in [docs/ui-host-contract.md](../../docs/ui-host-contract.md).

## Running scripts headlessly

```
cargo run --bin petal-ui-run -- app.ptl --frames 60 --scenario monkey:1 --seed 1
```

`petal-ui-run` runs a UI script without a window, prints the draw commands
and state as JSON, and replays deterministic input scenarios. See
`docs/dev/headless-ui-run.md` at the repo root. The showcase script
`garden/examples/panels/gallery.ptl` exercises every component and runs both
headlessly and as a Garden panel.

`petal-ui-propose-edit` is `petal propose-edit` (direct manipulation v2)
for UI scripts, which the core CLI cannot load: it runs the app headlessly
for `--frames N` frames and proposes source edits that move a draw call's
argument (`--channel draw_commands --emit K --native-arg I --to V`;
`--trace-emits` lists the draw calls). See
[direct-manipulation.md](../../docs/features/direct-manipulation.md#scripts-that-draw-with-ui).

## Versioning

- The **interface version** is the `ui` package's version
  (`prelude/petal.toml`, currently 0.1.0). It is what a host provides and
  what a package's `[requires] ui = "…"` is matched against.
- `UI_VERSION` counts incompatible changes to the input/draw native contract.
  Scripts can read it with `ui_version()`. It is a separate integer and not
  what `[requires]` compares.
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
`tests/hit.rs` covers layers, hit testing, the target registry, `ui_measure`
and flow layout; `tests/retained.rs` covers `cached_layer`, `draw_rects`, the
image and text-style additions, in both the lowered form every host gets and
the form a host that asked for it gets. `tests/reference.rs` fails if an
export is missing from `docs/ui-reference.md`.
`tests/prelude.rs` covers the older prelude surface and compatibility
(shadowing, style records). `harness::Headless` is public, so you can test
your own panels the same way.
