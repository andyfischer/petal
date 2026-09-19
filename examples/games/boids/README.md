# Boids

Craig Reynolds' Boids flocking simulation. 72 boids in a toroidal 2D field
steer by the three classic rules (separation, alignment, cohesion), with a
speed clamp and wrap-around edges. Every few frames the field is rendered as an
ASCII occupancy grid and the mean distance to the flock centroid is printed, so
the flock's convergence is both visible and measurable.

This is a headless console program, not a Garden panel app: it prints to
stdout and is fully deterministic, so two runs print byte-identical output.
It was written as a language stress test.

## Run it

```bash
./ts/bin/run-petal.ts run examples/games/boids/boids.ptl
./ts/bin/run-petal.ts check --strict examples/games/boids/boids.ptl
```
