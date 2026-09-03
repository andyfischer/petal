# bloom gallery

Every component in [bloom](../../../petal-libs/bloom/) — the component library
written entirely in Petal — on one screen: buttons, menus, controls, overlays,
the animation core they are built from, and the vector icon set.

It doubles as the usage reference. Each section of
[`app.ptl`](app.ptl) is the shortest honest example of the components it names,
so a panel author can copy a block and have a working control.

## Run it

In Garden (which registers the bloom modules, so `import bloom` just works):

```bash
./launch.sh
```

Headless, for a trace or a screenshot:

```bash
./launch.sh --headless --debug-port 0
```

Or without Garden at all, through the headless UI driver — outside Garden the
library is found on the module path, which is what `-I` is for:

```bash
cd petal-ui
cargo run --bin petal-ui-run -- ../examples/ui/bloom-gallery/app.ptl \
    --frames 120 --size 1100x760 -I ../petal-libs/bloom/src
```

## What to look for

- **Nothing in `app.ptl` holds animation state.** Every hover fade, press
  squash, click ripple, sliding segmented thumb, springing switch knob,
  staggered menu row and entering dialog is `state` *inside* the component,
  keyed by the call path. A component costs the caller exactly as much as a
  `draw_rect`.
- **Overlays capture input.** Open a menu or the dialog and the controls
  underneath go inert — there is no `if !blocked` guard anywhere in the file.
- **Buttons fire on release, not press.** Press one, slide off it, let go:
  nothing happens, as in every other toolkit.
- **Tab reaches every control with an `id`**, and Return fires the focused one.
  The focus ring blooms outside the control instead of changing its size.
- **The last line is `bloom.overlays()`.** Menus and tooltips defer their
  painting to that call, so a dropdown can be written inline with its button
  and still land on top of everything drawn after it.
- **Motion** section draws the animators raw: a spring and an eased value
  chasing the pointer, a staggered entrance, a pulse, and a shake.

## Controls

| Input | What it does |
|---|---|
| Click a rail item | switch section (the marker springs to it) |
| Tab / Shift+Tab | walk the focus ring; Return fires the focused control |
| Right-click (Menus section) | the context menu |
| Escape | close whatever is open |
