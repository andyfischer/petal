# d2-dial

A rotary dial with declarative dragging (`import ui/drag`): the model is one
angle, the view puts the knob on a circle at that angle, and the knob's spec
is `vary(dial, ["angle"])`. Nothing in the app turns the pointer back into an
angle: each frame the engine takes one solver step on the angle so the knob's
centre goes under the pointer, measuring how the knob moves with the angle by
rendering the view once more with the angle nudged (a forward difference).

```
cd core-libs/petal-ui-headless
cargo run --bin petal-ui-run -- ../../examples/ui/d2-dial/app.ptl \
  --scenario ../../examples/ui/d2-dial/scenarios/drag.json
```

The scenario turns the knob a quarter turn clockwise, from 0° to 90°: the
last record's `dial.angle` is about π/2. `tests/drag_p2.rs` in petal-ui
checks it.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md#continuous-drags-between-and-vary).
