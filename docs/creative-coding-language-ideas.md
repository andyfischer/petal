# Petal for Creative Coding: Language Improvement Ideas

Research and ideation for making Petal the best language for creative coding,
inspired by Processing/p5.js, GLSL, Sonic Pi, Nannou, and pain points found
while writing Nature of Code samples.

---

## Pain Points Found Writing Nature of Code Samples

These patterns came up repeatedly across 15 NOC samples (2,400+ lines):

### 1. Vector math is extremely verbose

The pattern `sqrt(vx * vx + vy * vy)` appears **20+ times** across the samples.
Normalizing a vector takes 3 lines every time:

```petal
let mag = sqrt(dx * dx + dy * dy) + 0.1
let nx = dx / mag
let ny = dy / mag
```

Limiting a vector's magnitude is even worse:

```petal
let speed = sqrt(vx * vx + vy * vy)
if speed > max_speed {
    vx = vx / speed * max_speed
    vy = vy / speed * max_speed
}
```

### 2. Record updates are painfully repetitive

In the cloth simulation, updating one field of a record requires rewriting the
entire record. This pattern appears **50+ times** in noc_cloth.ptl alone:

```petal
points[idx] = {
    x: a.x + ox, y: a.y + oy, old_x: a.old_x, old_y: a.old_y,
    pinned: a.pinned, conn_right: a.conn_right, conn_down: a.conn_down
}
```

When you only want to change `x` and `y`, copying 5 other fields is tedious
and error-prone.

### 3. int/float casting everywhere

Drawing functions need `int`, physics uses `float`. Every frame:

```petal
let sw = float(screen_width())   // cast on entry
draw_circle(int(obj.x), int(obj.y), int(radius), r, g, b)  // cast on draw
let r = int(100.0 + t * 30.0)   // cast in color math
```

### 4. Missing essential math functions

Several functions are needed constantly but missing:

- `clamp(v, lo, hi)` — written as `max(lo, min(hi, v))` dozens of times
- `lerp(a, b, t)` — written as `a + (b - a) * t` repeatedly
- `map(v, in_lo, in_hi, out_lo, out_hi)` — the #1 function in Processing
- `pow(x, n)` — needed for easing, falloff curves
- `sign(x)` — manual conditionals used instead
- `distance(x1, y1, x2, y2)` — sqrt(dx*dx + dy*dy) every time
- Perlin noise — the core of organic-feeling motion

### 5. Color handling is manual

HSV-to-RGB conversion is hand-coded (6 branches) in multiple samples:

```petal
if h < 60 { cr = 255; cg = h * 4; cb = 0 }
else if h < 120 { cr = (120 - h) * 4; cg = 255; cb = 0 }
else if h < 180 { ... }
// ... 4 more branches
```

No way to work in HSV natively or lerp between colors.

### 6. No way to get random integers or pick from a list

Only `random(min, max)` exists (returns float). Need:

- `random_int(min, max)` — very common for grid positions, indices
- `random(list)` — pick a random element (p5.js supports this)
- `random_gaussian(mean, stddev)` — for natural-looking distributions

---

## Proposed Language Changes

### Tier 1: High Impact, Easy to Add (builtins)

These are just new built-in functions — no parser or compiler changes needed.

#### Math functions

```petal
clamp(value, lo, hi)              // constrain to range
lerp(a, b, t)                     // linear interpolation
map(v, in_lo, in_hi, out_lo, out_hi)  // remap value between ranges
norm(v, lo, hi)                   // normalize to 0..1
distance(x1, y1, x2, y2)         // euclidean distance
mag(x, y)                         // vector magnitude (also mag(x,y,z))
pow(base, exp)                    // exponentiation
exp(x)                            // e^x
log(x)                            // natural log
sign(x)                           // -1, 0, or 1
fract(x)                          // fractional part (x - floor(x))
radians(degrees)                  // degrees to radians
degrees(radians)                  // radians to degrees
smoothstep(edge0, edge1, x)       // hermite interpolation (essential for GLSL-style work)
```

#### Noise functions

```petal
noise(x)                          // 1D Perlin/simplex noise, returns -1..1 or 0..1
noise(x, y)                       // 2D noise
noise(x, y, z)                    // 3D noise
noise_seed(seed)                  // set noise seed for reproducibility
```

Perlin noise is arguably the single most important function for creative coding.
It produces smooth, organic randomness — used for terrain, clouds, organic motion,
flow fields, textures. Every creative coding language provides it.

#### Randomness

```petal
random_int(lo, hi)                // random integer in range [lo, hi)
random_gaussian(mean, stddev)     // gaussian distribution
choose(list)                      // random element from list
shuffle(list)                     // return shuffled copy
```

#### Color functions

```petal
hsv(h, s, v)                     // create color from HSV (returns {r, g, b})
hsl(h, s, l)                     // create color from HSL
color_lerp(c1, c2, t)            // interpolate between two {r,g,b} colors
```

#### Easing functions

```petal
ease_in(t)                        // quadratic ease in (t^2)
ease_out(t)                       // quadratic ease out
ease_in_out(t)                    // smooth S-curve
// or a general: ease(t, type) where type is "quad", "cubic", "elastic", etc.
```

### Tier 2: High Impact, Moderate Effort (syntax changes)

#### Record spread operator

This alone would eliminate hundreds of lines across the NOC samples:

```petal
// Instead of rewriting every field:
points[i] = { ...points[i], x: nx, y: ny }

// Also useful for creating variations:
let bullet = { ...template, x: player.x, y: player.y }
```

#### Record field mutation

Allow mutating individual fields without reconstructing:

```petal
// Currently: must rebuild entire record
points[i] = { x: nx, y: ny, old_x: p.old_x, old_y: p.old_y, pinned: p.pinned }

// Proposed: direct field assignment
points[i].x = nx
points[i].y = ny
```

This is the #1 pain point in the cloth and spring simulations. It would
dramatically simplify any code that updates objects in-place.

#### Destructuring let

```petal
let { x, y, vx, vy } = particle
// instead of:
let x = particle.x
let y = particle.y
let vx = particle.vx
let vy = particle.vy
```

#### Implicit numeric coercion

Drawing functions require int but physics uses float. Consider:

- Option A: Drawing functions accept float (truncate internally)
- Option B: Implicit float-to-int coercion in function calls
- Option C: A `draw_circle` overload that takes float

Option A is simplest and would eliminate most `int()` casts. The SDL
renderer truncates to pixels anyway.

### Tier 3: Bigger Ideas (new language features)

#### First-class Vec2 type

A `vec2` type with operator overloading would transform creative coding in Petal:

```petal
let pos = vec2(100, 200)
let vel = vec2(3.5, -2.0)

pos += vel * dt()                // component-wise math with operators
let dist = mag(pos - target)     // magnitude of difference
let dir = normalize(vel)         // unit vector
vel = limit(vel, max_speed)      // clamp magnitude

// Works with existing drawing:
draw_circle(pos, 10, color)      // overloads accept vec2
```

This is what makes Processing's `PVector` and GLSL's `vec2`/`vec3` so powerful.
Instead of tracking `x` and `y` as separate variables (doubling every line of
physics code), you work with single values that represent 2D points.

Compare the flocking boid update:

```petal
// Current (6 lines):
let sep_x = 0.0
let sep_y = 0.0
sep_x -= dx / dist
sep_y -= dy / dist
let mag = sqrt(sep_x * sep_x + sep_y * sep_y)
steer_x += sep_x / mag * max_force

// With vec2 (2 lines):
let sep = vec2(0, 0)
sep -= vec2(dx, dy) / dist
steer += normalize(sep) * max_force
```

#### Transformation stack for SDL

```petal
push_matrix()
    translate(400, 300)
    rotate(angle)
    scale(2.0)
    draw_rect(-25, -25, 50, 50, 255, 255, 255)  // draws centered, rotated, scaled
pop_matrix()
```

Essential for hierarchical animation (arms on bodies, branches on trees).
Currently rotation must be computed manually with sin/cos.

#### Color type with modes

```petal
// Create colors in any space
let c = hsv(200, 0.8, 1.0)       // hue, saturation, value
let warm = hsl(30, 0.9, 0.6)     // orange

// Use in drawing functions directly
draw_circle(x, y, r, c)          // accepts color record

// Interpolate in perceptually uniform space
let gradient = color_lerp(c, warm, 0.5)

// Color literals already exist in Petal (#rgb) — extend them:
let c = #ff6600                   // already works!
let c = hsv(30, 100, 100)        // proposed
```

#### Drawing function improvements

```petal
// Accept float coordinates (no more int() casting):
draw_circle(pos.x, pos.y, 10.5, 255, 0, 0)

// Accept color as record:
draw_circle(x, y, r, {r: 255, g: 0, b: 128})

// New primitives:
draw_triangle(x1, y1, x2, y2, x3, y3, r, g, b)
draw_ellipse(cx, cy, rx, ry, r, g, b)
draw_polygon(points, r, g, b)    // list of {x, y} records
draw_arc(cx, cy, radius, start_angle, end_angle, r, g, b)

// Alpha/opacity support:
draw_circle_alpha(x, y, r, red, green, blue, alpha)
// or globally: set_alpha(0.5)

// Fill + stroke separation:
set_fill(r, g, b)
set_stroke(r, g, b)
set_stroke_width(2)
```

#### Particle/object spawning sugar

Creative coding constantly creates lists of objects with slight variations.
A comprehension syntax could help:

```petal
// List comprehension
let particles = [{ x: random(0, sw), y: random(0, sh), vx: 0, vy: 0 } for i in range(0, 100)]

// Instead of:
let particles = []
for i in range(0, 100) {
    push(particles, { x: random(0, sw), y: random(0, sh), vx: 0, vy: 0 })
}
```

---

## Lessons from Other Creative Coding Languages

### From Processing/p5.js
- `map()` is the most-used function — it bridges between value domains
- `push()`/`pop()` transformation stack is essential for complex scenes
- `colorMode(HSB)` lets artists think in hue/saturation/brightness
- `noise()` (Perlin) is what makes things look "organic" instead of "random"
- Minimal boilerplate: `setup()` and `draw()` is all you need

### From GLSL/Shadertoy
- `smoothstep()` replaces hard edges with beautiful soft transitions
- `fract()` enables infinite tiling patterns
- `mix()` (lerp) on everything — numbers, vectors, colors
- Component-wise vector math eliminates enormous amounts of code
- `step()`, `mod()`, and `clamp()` are bread-and-butter

### From Sonic Pi
- Domain-specific vocabulary makes code read like the domain (music/art)
- **Rings** (cyclic arrays) with `.tick` are brilliant for repeating patterns
- Live reloading is transformative for creative exploration

### From Nannou (Rust)
- Strong type system doesn't have to mean verbose — good defaults help
- `map_range()` is their most-used utility
- Separation of `model` / `update` / `view` keeps code clean

### From Scratch
- Removing complexity increases creativity
- Immediate visual feedback is non-negotiable
- Color-coded categories help discoverability

---

## Priority Ranking

**Do first (biggest bang for buck):**
1. Add `clamp`, `lerp`, `map`, `distance`, `mag` builtins
2. Add `pow`, `sign`, `fract`, `smoothstep` builtins
3. Add record spread: `{ ...obj, field: val }`
4. Make drawing functions accept float (auto-truncate)
5. Add `noise(x)`, `noise(x,y)`, `noise(x,y,z)`

**Do second (significant quality of life):**
6. Record field mutation: `obj.x = val`
7. Add `hsv()`, `hsl()`, `color_lerp()`
8. Add `random_int()`, `choose()`, `random_gaussian()`
9. Destructuring let: `let { x, y } = point`
10. Add easing functions

**Do third (transformative but larger scope):**
11. First-class `vec2` type with operator overloading
12. Transformation stack (`push_matrix`/`pop_matrix`/`translate`/`rotate`)
13. List comprehensions
14. Alpha/opacity in drawing
15. New drawing primitives (triangle, ellipse, polygon, arc)

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
and hot reloading supports creative exploration. The improvements above
would close the gap between "writing physics simulations" and "expressing
creative ideas."
