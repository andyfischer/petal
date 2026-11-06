# d2-switch

A three-way switch with declarative dragging (`import ui/drag`): the knob
has three positions in a triangle, and its spec is `between(["off", "low",
"high"])`. While it is held, the knob moves anywhere inside the triangle and
stops at its edges; the knob's colour and the background are placed values
too, so they blend between the three positions' colours by the knob's
barycentric weights. Letting go settles on the nearest position.

```
cd core-libs/petal-ui
cargo run --bin petal-ui-run -- ../../examples/ui/d2-switch/app.ptl \
  --scenario ../../examples/ui/d2-switch/scenarios/drag.json
```

The scenario drags the knob from `off` through the middle of the triangle,
out past its right edge (where it stops on the edge) and up to `high`: the
last record's `pos` is `"high"`. `tests/drag_p2.rs` in petal-ui checks it.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md#continuous-drags-between-and-vary).
