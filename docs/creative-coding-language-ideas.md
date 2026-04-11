# Petal for Creative Coding: Status & Roadmap

Research and ideation for making Petal the best language for creative coding,
inspired by Processing/p5.js, GLSL, Sonic Pi, and Nannou, and by pain points
found while writing Nature of Code samples.

This document tracks which language improvements have **shipped**, which are
**in progress**, and which remain **open** for future work. See
[Builtins.md](Builtins.md) for usage docs on anything marked shipped.

---

## Pain Points Found Writing Nature of Code Samples

These patterns came up repeatedly across 15 NOC samples (2,400+ lines):

1. **Vector math is extremely verbose.** `sqrt(vx * vx + vy * vy)` appeared 20+
   times. Normalizing and limiting required 3–5 boilerplate lines every time.
2. **Record updates are painfully repetitive.** In the cloth simulation, updating
   one field meant rewriting all of them. This pattern appeared 50+ times in
   `noc_cloth.ptl` alone.
3. **int/float casting everywhere.** Drawing functions want int, physics uses
   float. Every frame is littered with `int(...)` and `float(...)`.
4. **Missing essential math functions.** `clamp`, `lerp`, `map`, `distance`,
   `pow`, `sign`, noise — all routinely hand-rolled.
5. **Color handling is manual.** HSV-to-RGB conversion hand-coded (six branches)
   in multiple samples. No way to lerp colors.
6. **No random integers or list picking.** Only `random(min, max)` existed
   (returns float). No `random_int`, `choose`, gaussian.

---

## Shipped

Most of the Tier 1 vocabulary and part of Tier 3 have landed. See
[Builtins.md](Builtins.md) for docs and examples, and the `/examples/` directory
for usage in context.

### Math

- `clamp(v, lo, hi)`
- `lerp(a, b, t)`
- `map_range(v, in_lo, in_hi, out_lo, out_hi)`
- `distance(x1, y1, x2, y2)` and `distance(v1, v2)`
- `mag(x, y)`, `mag(x, y, z)`, `mag(v)`
- `pow(base, exp)`, `exp(x)`, `log(x)`
- `sign(x)`, `fract(x)`, `smoothstep(a, b, x)`
- `radians(deg)`, `degrees(rad)`
- `sin`, `cos`, `tan`, `atan2`, `pi()`

### Noise

- `noise(x)`, `noise(x, y)`, `noise(x, y, z)` (Perlin)
- `noise_seed(seed)`

### Randomness

- `random_int(lo, hi)`
- `choose(list)`

### Color

- `hsv(h, s, v)`, `hsl(h, s, l)` — return `{r, g, b}` records
- `color_lerp(c1, c2, t)`
- `#rrggbb` literals desugar to RGB records (shipped earlier)

### Vectors

- First-class `vec2` type with operator overloading (`+`, `-`, `*`, `/`)
- `vec2(x, y)`, `normalize(v)`, `dot(a, b)`, `limit(v, max)`
- `mag` and `distance` accept `vec2` values

### Syntax

- **Record spread**: `{ ...obj, field: val }` — eliminates the "rewrite every
  field" pattern that motivated this proposal.
- **Record field mutation**: `obj.x = val`, `list[i].x = val`, and nested
  `obj.inner.field = val`. Maps are heap-backed and mutated in place.

---

## Open — still worth doing

Ranked roughly by impact-per-effort.

### Drawing functions accept float

`petal-sdl` drawing functions currently require `int`, which forces `int()`
casts everywhere physics meets rendering. Option A: accept float and truncate
internally. This is the lowest-effort change with the highest ergonomic payoff
for anything visual.

### Destructuring let

```petal
let { x, y, vx, vy } = particle
```

Small, user-visible, complements record spread. Check `ast.rs` before scoping —
the pattern-matching infrastructure may already cover most of it.

### Easing functions

A small family — `ease_in`, `ease_out`, `ease_in_out` — or a single
`ease(t, kind)`. Builtins only, trivial to add.

### Random Gaussian

`random_gaussian(mean, stddev)` — for natural-looking distributions. Small,
useful for scatter/particle effects.

### Transformation stack (SDL)

```petal
push_matrix()
translate(400, 300)
rotate(angle)
draw_rect(-25, -25, 50, 50, color)
pop_matrix()
```

Essential for hierarchical animation (branches on trees, arms on bodies).
Larger scope — touches the renderer.

### Drawing primitives and styling (SDL)

- Accept color records directly: `draw_circle(x, y, r, color)`
- New primitives: `draw_triangle`, `draw_ellipse`, `draw_polygon`, `draw_arc`
- Alpha / opacity support (per-call or global `set_alpha`)
- Separate fill and stroke: `set_fill`, `set_stroke`, `set_stroke_width`

### List comprehensions

```petal
let particles = [
    { x: random(0.0, sw), y: random(0.0, sh), vx: 0, vy: 0 }
    for i in range(0, 100)
]
```

Sugar over the existing `for` + `push` loop. Medium parser work, big readability
win for initializers.

### `vec3` / `vec4`

If 3D or 4D ever becomes interesting (e.g., for color math in linear RGBA,
or for a future 3D renderer), generalize the `vec2` machinery. Not urgent.

---

## Lessons from Other Creative Coding Languages

### From Processing/p5.js
- `map()` (our `map_range`) is the most-used function — it bridges value domains.
- `push()`/`pop()` transformation stack is essential for complex scenes.
- `colorMode(HSB)` lets artists think in hue/saturation/brightness.
- `noise()` (Perlin) is what makes things look "organic" instead of "random".
- Minimal boilerplate: `setup()` and `draw()` is all you need.

### From GLSL/Shadertoy
- `smoothstep()` replaces hard edges with beautiful soft transitions.
- `fract()` enables infinite tiling patterns.
- `mix()` (lerp) on everything — numbers, vectors, colors.
- Component-wise vector math eliminates enormous amounts of code.
- `step()`, `mod()`, and `clamp()` are bread-and-butter.

### From Sonic Pi
- Domain-specific vocabulary makes code read like the domain (music/art).
- Rings (cyclic arrays) with `.tick` are brilliant for repeating patterns.
- Live reloading is transformative for creative exploration.

### From Nannou (Rust)
- A strong type system doesn't have to mean verbose — good defaults help.
- `map_range()` is their most-used utility.
- Separation of `model` / `update` / `view` keeps code clean.

### From Scratch
- Removing complexity increases creativity.
- Immediate visual feedback is non-negotiable.
- Color-coded categories help discoverability.

---

## Design Philosophy Notes

The best creative coding languages share these traits:

1. **Low floor, high ceiling.** Easy to start, powerful enough to go deep.
   "Hello circle" should be one line. An N-body sim should be possible.

2. **Math as prose.** `lerp(color1, color2, noise(x, y))` reads like intent.
   `int(float(c1.r) + (float(c2.r) - float(c1.r)) * n)` reads like implementation.

3. **Visible by default.** Everything draws to the screen. No setup, no
   boilerplate. Petal already does this well with its frame-based execution.

4. **Forgiving types.** Creative coders don't want to fight the type system.
   If they pass a float where an int is expected, truncate it. If they pass
   a color record where RGB values are expected, unpack it.

5. **Built-in vocabulary for the domain.** Processing doesn't make you write
   your own `map()` or `noise()`. The language speaks the artist's language.

Petal already has a strong foundation: the state/frame model is elegant,
pattern matching is powerful, the pipe operator enables clean data flow,
and hot reloading supports creative exploration. The remaining open items
above would close the gap between "writing physics simulations" and
"expressing creative ideas" fluently.
