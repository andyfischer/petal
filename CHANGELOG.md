# Changelog

All notable changes to Petal are recorded here.

## Unreleased

### Breaking

- **`hsv()` and `hsl()` now take hue in `[0, 1)` instead of degrees `[0, 360)`.**
  Every other channel of the color API (`s`, `v`, `l`, alpha) is already
  normalized to `0..1`, and p5.js / three.js / Processing default hue to `0..1`
  as well — so the degree-based hue was the odd one out, and existing sketches
  avoided `hsv()` by computing RGB by hand. Migrate by dividing the old hue by
  `360`, or call the new `hsv_deg()` / `hsl_deg()` which still take degrees:

  ```petal
  hsv(120.0, 1.0, 1.0)        // OLD: green;  NOW: effectively red (120 wraps)
  hsv(120.0 / 360.0, ...)     // normalized equivalent
  hsv_deg(120.0, 1.0, 1.0)    // green — degrees variant
  ```

### Added

- `hsv_deg(h, s, v)` and `hsl_deg(h, s, l)` — hue in degrees `[0, 360)`, for
  code that prefers to think in degrees.
- `f64_array(n)`, `get`, `set`, `swap`, and `a[i]` indexing — a flat, unboxed
  `f64` array type for numeric inner loops.
- `fill_triangle(...)` and `fill_poly(points, r, g, b)` filled-polygon drawing
  primitives across the SDL and canvas integrations.
- Reference external IR emitter (`ts/tools/calc-to-ir.ts`): a toy language that
  compiles to Petal IR JSON and runs via `petal run --ir`.
- MIT `LICENSE`.
- Automated secret scanning: a gitleaks CI workflow (full history) with a
  `scripts/scan-secrets.sh` local runner, plus a forbidden-terms guard driven by
  a repository secret.

### Changed

- `for x in range(a, b)` is lowered to a non-allocating counted loop
  (`NumericForLoop`) instead of materializing a list each iteration.
- petal-sdl: the framebuffer now persists between frames — a sketch that never
  calls `clear()` accumulates its drawing (trails, attractors).

### Removed

- The web playground (`playground/`), which depended on a private, unpublished
  framework that external users could not build against.
