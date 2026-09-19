# 2048

A 4×4 board with the real slide-and-merge rules: a tile merges at most once
per move, merges resolve from the leading edge, and a new tile spawns only on
moves that actually changed the board. Spawns come from a small built-in LCG
with a fixed seed.

This is a headless console program, not a Garden panel app: it prints to
stdout and is fully deterministic, so two runs print byte-identical output.
It was written as a language stress test.

## Run it

```bash
./ts/bin/run-petal.ts run examples/games/2048/2048.ptl
./ts/bin/run-petal.ts check --strict examples/games/2048/2048.ptl
```
