# Tetris

A headless Tetris core: a 10×20 well, the seven tetrominoes, SRS rotation with
the standard wall-kick tables, gravity on a tick clock, hard drop, line clears
with guideline scoring, levels, and game over. Pieces come from a 7-bag fed by a
fixed-seed LCG, and a deterministic placement heuristic plays the game.

This is a headless console program, not a Garden panel app: it prints to
stdout and is fully deterministic, so two runs print byte-identical output.
It was written as a language stress test.

## Run it

```bash
./ts/bin/run-petal.ts run examples/games/tetris/tetris.ptl
./ts/bin/run-petal.ts check --strict examples/games/tetris/tetris.ptl
```
