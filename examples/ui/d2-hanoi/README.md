# d2-hanoi

Tower of Hanoi with declarative dragging (`import ui/drag`): only a peg's top
disk is draggable, and its spec is `closest(hanoi_moves(pegs, p)) |>
floating()`, where `hanoi_moves` lists the legal boards one move away. An
illegal peg is not in the list, so the drag cannot reach it; the disk lands on
the nearest legal peg instead.

```
cd core-libs/petal-ui
cargo run --bin petal-ui-run -- ../../examples/ui/d2-hanoi/app.ptl \
  --scenario ../../examples/ui/d2-hanoi/scenarios/drag.json
```

The scenario moves disk 1 to peg 3, then drags disk 2 toward peg 3 (where
disk 1 is) and lets go: it lands on peg 2. Final `pegs`: `[[4, 3], [2], [1]]`.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md).
