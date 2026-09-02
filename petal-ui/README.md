# petal-ui

The standard UI layer for Petal programs: a small Rust crate every graphical
host embeds, plus the `ui` prelude module — the widget/component library Petal
scripts draw interfaces with.

Hosts that embed it today: **Garden** panels (`garden/garden-script`),
**petal-desktop-sdl**, **petal-web-canvas**, the standalone app embedders
(`~/biz/experiment-*`), and the headless harness/CLI in this crate.

## The model

Immediate mode. The script's entire top level re-runs every frame:

```
input events → begin_frame(dt) → bind_frame_info / bind_input
→ clear_draw_commands → env.run(script) → take_draw_commands → rasterize
```

- **Layer 0 — input** (`src/input.rs`): `InputEvent`/`InputState` and the
  polling natives (`mouse_x`, `key_pressed`, `text_input`, `dt`, `time`, …).
  Scripts poll each frame; there are no callbacks.
- **Layer 2 — draw** (`src/draw.rs`): draw natives (`draw_rect`, `draw_text`,
  `fill_arc`, `draw_rect_gradient`, `draw_shadow`, `clip`/`clip_push`, …)
  append `DrawCommand`s to a buffer; the host drains the buffer and turns it
  into pixels. Hosts implement rasterization only.
  - A host that can't do a thing **degrades** it — a gradient filled with one
    of its stops, a rounded clip scissored square — rather than dropping the
    command. Silent omission is the one wrong answer.
  - `src/tess.rs` holds the CPU tessellation no host should re-derive.
    `shadow_mesh` turns a `Shadow` command into a single *non-overlapping*
    triangle list (a solid core plus a ring whose per-vertex alpha falls to 0
    at `blur`), because a translucent shadow assembled from overlapping pieces
    double-composites and shows every seam.
- **The `ui` prelude** (`prelude/ui.ptl`): the widget library, registered as an
  implicit import so scripts call `button(...)`, `list_update(...)`,
  `checkbox(...)` bare. Implicit bindings are weak — a script's own
  `fn button` shadows the prelude's.
  - Widget text is set in the theme's `font` face (`"ui"` — proportional), and
    every widget **measures and draws with the same style record**. The bare
    `text_width(s, size)` / `draw_text(s, pos, size, color)` pair disagrees
    about which face it means, which is why nothing in the library uses it.

**See [docs/components.md](docs/components.md) for the full component
reference**: theme system, RectCut layout, every widget, motion helpers, and
the conventions they all follow.

## Embedding

```rust
petal_ui::register_all(&mut env);          // input + draw + canvas + host_data
petal_ui::register_prelude(&mut env)?;     // the `ui` module, implicit import

// each frame:
petal_ui::input::bind_frame_info(&mut env, dt, frame);
petal_ui::input::bind_dimensions(&mut env, w, h);
petal_ui::input::bind_input(&mut env, &input_state);
petal_ui::draw::clear_draw_commands(&mut env);
env.run(...)?;
let commands = petal_ui::draw::take_draw_commands(&mut env);
```

To make the prelude paint in the host's own colors, publish a palette once at
startup (or each frame, if it can change live):

```rust
petal_ui::input::bind_host_palette(&mut env, &[("window_bg", [13, 17, 23, 255]),
                                               ("text", [230, 237, 243, 255]),
                                               ("accent", [88, 166, 255, 255]),
                                               /* … Garden palette() vocabulary … */]);
```

`ui_theme()` then resolves: explicit `theme_set` > host palette > built-in
dark default. Hosts that never bind a palette lose nothing. Garden binds its
full palette, so prelude widgets look native there with zero script-side setup.

## Running scripts headlessly

```
cargo run --bin petal-ui-run -- app.ptl --frames 60 --scenario monkey:1 --seed 1
```

`petal-ui-run` (see `docs/dev/headless-ui-run.md` at the repo root) runs a UI
script without a window, prints the draw commands/state as JSON, and replays
deterministic input scenarios — it is what the golden-trace corpus and the
per-widget tests drive. The showcase script
`garden/examples/panels/gallery.ptl` exercises every component and runs both
headlessly and as a Garden panel.

## Versioning

- `UI_VERSION` — the input/draw native contract (scripts read `ui_version()`).
- `PRELUDE_LEVEL` — the prelude export surface; level 3 is the component
  library described in docs/components.md, level 5 the gradient/shadow/clip
  primitives, level 6 the theme's type face (`font`, defaulting to the
  proportional `"ui"`), the elevation scale and the `over`/`tint` compositing
  helpers, level 7 the layer vocabulary (`layer`, `snapshot`,
  `draw_backdrop_blur`, `draw_material`) over the offscreen-canvas natives,
  which every host now registers. `garden --version --json` lists the exact
  exports compiled into a binary.

## Tests

```
cargo test                 # unit + prelude + widget + CLI tests
```

`tests/widgets.rs` covers the component library through `harness::Headless`
(synthetic clicks/keys, then assertions on state and draw commands);
`tests/prelude.rs` covers the pre-level-3 surface plus compat (shadowing,
style records); `harness::Headless` is public — use it to test your own
panels the same way.
