# layers

Layers, hit testing, named targets, retained drawing and flow layout from the
`ui` package, in one small app: a grid "world" under a HUD panel and a modal
dialog.

- `cached_layer` + `draw_rects`: the 640-cell grid is recorded once per
  palette and is one command.
- `with_layer`, `ui_block`, `pointer_captured`: a click on the panel never
  reaches the world.
- `modal`: what is under the dialog is inert, with no guard in the app.
- `flow_begin` … `flow_end`, `ui_measure`: the panel is laid out by a cursor
  and sized by running its own body without drawing it.
- `tooltip` drawn mid-frame on its own layer.

```
cd core-libs/petal-ui
cargo run --bin petal-ui-run -- ../../examples/ui/layers/app.ptl --frames 3 --targets
```

Reference: [docs/ui-reference.md](../../../docs/ui-reference.md).
