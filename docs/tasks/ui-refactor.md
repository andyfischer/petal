# Reorganizing petal-ui: what is left

Status: 2026-11-08. The reorganization this tracked is done: `ui.ptl` is
split into modules behind an unchanged facade (R1), the headless tooling is
`core-libs/petal-ui-headless` and Garden's panel stubs are
`garden_script::headless` (R2), and the core-feature tests are
`test/ui-corpus/` (R3). See `docs/skills/project-organization.md` for where
things live now and `docs/dev/performance.md` ("Text wrap: native vs Petal")
for the text-wrap measurement. What is left needs a decision or a person at
a display.

## Owner decisions

- [ ] **R4: whether `ui` or `bloom` owns the widgets.** `ui/widgets` (about
  3,100 lines: button through load state) and `core-runtime/bloom` ("the
  default widget set", built partly on `ui`'s `button`, `checkbox` and
  `slider`) are two bundled widget sets. If `ui` ended at primitives plus
  interaction and the widgets came from one library, the bundled API would
  shrink a lot, but cheesecake (110 `.ptl` files import `ui`),
  petal-creative (15) and `examples/` (94) use `ui`'s widgets, so it would be
  a migration with a deprecation period. R1 made `ui/widgets` a separate
  module, which makes either choice cheaper. Decide before many more widgets
  go into `ui/widgets`.
- [ ] **`ui::` qualifiers.** Since R1, module-qualified names follow the
  file: `ui/widgets::button`, `ui/layout::ease_out`, `ui/_frame::_floor`.
  This shows in state-map keys, `Env::call_function` names and the "needs
  native fns" load error, and a state map saved by an older binary loses the
  prelude's internal state (the theme `theme_set` set, for example). No
  downstream use of the old names was found. Keep it, or give core a
  package-level display name for internal modules so they read `ui::` again.
- [ ] **Porting `hit::roll`, `focus` and `popover` to Petal** (about 1,500
  lines). Possible, but held off: these decisions run before the script each
  frame, in Rust so that no script can skip them. A port needs a host-run
  Petal prologue (`ui._roll()`) and a check that the frame gate and memo
  still behave the same. The code is small and stable, so the gain is small.

## Needs a display

- [ ] **SDL sub-pixel in a live window.** `petal-sdl --subpixel` (with the
  antialiased fills for fractional polygons, ellipses and arcs) is covered by
  the renderer's unit tests on a software surface, but has not been looked
  at in a live or HiDPI window.
