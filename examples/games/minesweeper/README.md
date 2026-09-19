# Minesweeper

A 9×9 board with 10 mines placed by a self-contained fixed-seed LCG,
neighbour counts, flagging, and the recursive flood-fill reveal of contiguous
zero-count cells plus their numbered border. A scripted sequence of moves drives
it to a mine hit.

This is a headless console program, not a Garden panel app: it prints to
stdout and is fully deterministic, so two runs print byte-identical output.
It was written as a language stress test.

## Run it

```bash
./ts/bin/run-petal.ts run examples/games/minesweeper/minesweeper.ptl
./ts/bin/run-petal.ts check --strict examples/games/minesweeper/minesweeper.ptl
```
