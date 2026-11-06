# d2-beads

The ring of beads from the Dragology paper (*Declarative Dragging*, UIST '26),
with `import ui/drag`: `closest(moves(beads, i)) |> floating() |>
on_drop(a_first)`. A ring has no first bead, so `on_drop` turns each drop
state until A is at the top again; the whole ring rotates into place after
the drop, animated by `place`.

```
cd core-libs/petal-ui
cargo run --bin petal-ui-run -- ../../examples/ui/d2-beads/app.ptl \
  --scenario ../../examples/ui/d2-beads/scenarios/drag.json
```

The scenario drags A two places round, then D to the top. Final `beads`:
`["A", "E", "F", "B", "C", "D"]`.

Guide: [core-libs/petal-ui/docs/drag.md](../../../core-libs/petal-ui/docs/drag.md).
