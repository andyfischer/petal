# Procedural terrain

Builds a heightmap from multi-octave value noise (fractal Brownian motion),
normalizes it to 0..1, and renders it as ASCII with bands for deep water,
shallow water, beach, grass, forest, rock and snow. A histogram of the
terrain-type distribution is printed alongside the map so the output can be
checked numerically.

This is a headless console program, not a Garden panel app: it prints to
stdout and is fully deterministic, so two runs print byte-identical output.
It was written as a language stress test.

## Run it

```bash
./ts/bin/run-petal.ts run examples/games/terrain/terrain.ptl
./ts/bin/run-petal.ts check --strict examples/games/terrain/terrain.ptl
```
