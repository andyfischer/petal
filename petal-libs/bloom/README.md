# bloom

A UI component library written entirely in Petal: buttons, menus, controls,
overlays, an animation core, and a vector icon set — about 2,000 lines of
`.ptl` and not one line of Rust.

```petal
import bloom

if bloom.button(rect(20, 20, 120, 32), "Save", {variant: "primary", icon: "check"}) then
  bloom.toast("Saved", "success")
end

let picked = bloom.dropdown(rect(160, 20, 140, 32), "Actions",
                            ["Rename", {sep: true}, {label: "Delete", danger: true}])
```

```petal
bloom.overlays()   // last line of the frame: paints menus and tooltips on top
```

That is the whole event model: a component draws itself and returns its value.
Everything animates — the hover fade, the press squash, the ripple from the
point you clicked, the menu's staggered entrance, the switch knob's spring —
and none of it is state the caller holds.

The showcase and usage reference is
[`examples/ui/bloom-gallery`](../../examples/ui/bloom-gallery/); the API
reference is [docs/components.md](docs/components.md).

## Why it exists next to `ui`

[`petal-ui`](../../petal-ui/) is the **host layer**: a Rust crate that gives an
embedder the input and draw natives, plus the `ui` prelude of primitives and
widgets over them. bloom is a **component layer** on top of that, in pure
Petal, with opinions `ui` deliberately does not take:

| | `ui` prelude | bloom |
|---|---|---|
| Lives in | a Rust crate, registered by the host | `.ptl` files you drop in |
| Cross-frame state | explicit records you keep (`list_state()`) | per-callsite `state` inside the component |
| Click | on press | on **release inside**, so sliding off cancels |
| Overlays | you guard your own input (`menu_blocking`) | the overlay **captures** input; callers need no guard |
| Focus | a registry you thread and list | announced in draw order; Tab just works |
| Animation | a few widgets ease internally | every component, from one motion core |

They compose: bloom draws through `ui`'s primitives and inherits its theme, so
a bloom panel looks native in its host on the first frame. Use both in one
script freely.

## Requirements

A host that embeds `petal-ui` (Garden panels, `petal-desktop-sdl`,
`petal-web-canvas`, the `petal-ui-run` driver, or your own embedder calling
`petal_ui::register_all`). That is all — bloom asks for no natives of its own,
no fonts, and no files at runtime.

## Installing it in a project

Copy `src/*.ptl` into your project and make them reachable one of three ways:

```bash
# 1. beside the script — the importing file's directory is searched first
cp petal-libs/bloom/src/*.ptl myapp/

# 2. on the module path
petal-ui-run myapp/app.ptl -I petal-libs/bloom/src
petal run -I petal-libs/bloom/src myapp/app.ptl
```

```rust
// 3. registered by the host, which also covers scripts pushed as source
for (name, src) in MODULES { env.register_module(name, src); }
```

Garden ships (3) in
[`garden/garden-script/src/bloom.rs`](../../garden/garden-script/src/bloom.rs) —
nine `include_str!`s and a loop — so every Garden panel can `import bloom`.

## The modules

| Module | Contents |
|--------|----------|
| `bloom.ptl` | The facade. Re-exports everything below; `import bloom` is all most apps need |
| `bloom_motion.ptl` | The animation core: `ease_to`, `ease_flag`, `spring`, `enter`, `impulse`, `stagger`, `shake`, easings, rect interpolation |
| `bloom_theme.ptl` | Tokens derived from the host palette, plus the shared painting: `surface`, `stroke`, `wash`, `focus_ring`, `text_in`, `ts` |
| `bloom_icon.ptl` | 22 vector glyphs, drawn as strokes in a unit box |
| `bloom_interact.ptl` | `probe` (hover/press/click + their animated twins), input capture, focus ring, drag, hotkeys |
| `bloom_button.ptl` | `button`, `icon_button`, `segmented`, `chip`, `link`, `spinner` |
| `bloom_controls.ptl` | `switch`, `checkbox`, `radio_group`, `slider`, `stepper`, `progress`, `text_field` |
| `bloom_menu.ptl` | `menu`, `dropdown`, `select`, `menu_bar`, `context_menu` |
| `bloom_overlay.ptl` | `tooltip`, `toast`, `dialog`, `popover`, `banner`, `skeleton` |

Import the facade, or a single module when you want a slice of it
(`import bloom_motion: spring` in a game's HUD).

## Tests

The library's tests drive it through a real host — a Garden panel:

```bash
cd garden && cargo test -p garden-script --test bloom
```

They cover click semantics, the focus ring, menu capture, dialog dismissal,
toast expiry, text editing at the caret, spring settling, and a smoke frame
that draws every component
([`fixtures/bloom_smoke.ptl`](../../garden/garden-script/tests/fixtures/bloom_smoke.ptl)).

Headlessly, without Garden:

```bash
cd petal-ui && cargo run --bin petal-ui-run -- \
    ../examples/ui/bloom-gallery/app.ptl --frames 120 --scenario monkey:7 \
    -I ../petal-libs/bloom/src
```

## Versioning

`bloom.VERSION` counts additions to the export surface. Levels are additive:
every existing export keeps its signature, so a script written against an
older one keeps working.
