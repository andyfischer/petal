# d2-list

A reorderable list with declarative dragging (`import ui/drag`). Each row
says where it is (`place`) and which states dragging it can reach
(`draggable` with `closest(moves(items, i)) |> floating()`); the library does
the slop, the hit testing, the rows sliding out of the way, the drop settling
into its slot and Escape.

```
cd core-libs/petal-ui
cargo run --bin petal-ui-run -- ../../examples/ui/d2-list/app.ptl \
  --scenario ../../examples/ui/d2-list/scenarios/drag.json
```

The scenario drags A down three rows: the last record's `items` is
`["B", "C", "D", "A", "E"]`. `tests/drag.rs` in petal-ui checks it.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md).
