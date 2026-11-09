# d2-scheduler

The scheduler of the Dragology paper's §4, with declarative dragging
(`import ui/drag`). Meetings are bars on room tracks; the puzzle is to fit
them into as few rooms as possible without a clash (clashes are red).

- A bar's body drags it between rooms:
  `between(choices(s, ["meetings", i, "room"], range(ROOMS)))`. The bar
  slides up and down along the line its renderings in each room make, never
  sideways, and drops into the nearest room.
- A bar's end handles change its times:
  `vary(s, ["meetings", i, "start"], {constraint: ok}) |> on_drop(round5)`.
  The constraint keeps a meeting at least 30 minutes long and inside the
  day; on release the time is rounded to five minutes.

```
cd core-libs/petal-ui-headless
cargo run --bin petal-ui-run -- ../../examples/ui/d2-scheduler/app.ptl \
  --scenario ../../examples/ui/d2-scheduler/scenarios/drag.json
```

The scenario moves `review` from room 2 to room 1, then drags `retro`'s end
past the end of the day, where the constraint stops it: the last record has
`review` in room 0 and `retro` ending at 600. `tests/drag_p2.rs` in petal-ui
checks it.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md#continuous-drags-between-and-vary).
