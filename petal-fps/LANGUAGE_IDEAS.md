# Petal Language Improvement Ideas

Notes accumulated while building petal-fps, a 3D FPS in Petal. Each entry
includes the friction it would relieve and a sketch of the proposed fix.

---

## 1. Built-in `vec3` (and `mat3`/`mat4`)

**Friction**: `vec2` exists but a 3D game wants 3D vectors *everywhere*.
Currently every position, velocity, ray, and triangle vertex is a record
`{x, y, z}` and every operation is hand-inlined: `let dx = a.x - b.x; let dy =
a.y - b.y; let dz = a.z - b.z;`. That's ~3× the code and obscures intent.

**Proposed**:
```petal
let p = vec3(1.0, 2.0, 3.0)
let v = p + other_vec3       // operator overload like vec2 already has
let n = normalize(v)
let d = dot(v, n)
let c = cross(a, b)
let len = mag(v)
```
Plus `mat4` with `mat4_translate`, `mat4_rotate_y`, `mat4_perspective`,
`mat4_mul`, `mat4_apply(m, vec3)`. With these, the entire camera/projection
math in petal-fps would shrink from ~40 lines to ~8.

---

## 2. `state` keyword inside functions doesn't work as you'd hope

**Friction**: I want to define `fn project(p)` that uses `cy_, sy, focal`
computed once per frame from camera state. Today I have to hoist those
computations to the top-level scope so the closure captures them. If I put
them inside a function I'd recompute every call.

**Proposed**: A `cached(expr)` form or a memoization decorator
`@cached_per_frame fn ...`. Or expose `frame_count()` as a key so per-frame
caches are first-class. Or just a `let` evaluated lazily once.

---

## 3. Function overloading is invisible without checking the runtime

**Friction**: The platformer example does:
```petal
let _draw_line = draw_line
fn draw_line(x1,y1,x2,y2,r,g,b) { _draw_line(x1,y1,x2,y2,r,g,b) }
fn draw_line(a, b, color) { _draw_line(a.x,a.y,b.x,b.y,color.r,color.g,color.b) }
```
This pattern works but is opaque; you can't tell from the source whether the
"second `fn`" is a redefinition or an arity-overload.

**Proposed**: Explicit overload syntax: `fn draw_line/3(...) { }` and
`fn draw_line/7(...) { }`. Or: pattern-match on first arg type so a single
function dispatches.

---

## 4. No tuple destructuring on assignment

**Friction**: Want to swap two values, return a pair, etc. Currently I have to
return a record or use a temp.

**Proposed**: `let {a, b} = some_record` or `let (x, y, z) = vec3_tuple(v)`.

---

## 5. `for i in range(0, n)` re-evaluates `range(0, n)` to allocate a list

**Friction**: For tight inner loops (rasterizing 100s of buildings) we
allocate temporary lists. A `for i = 0; i < n; i += 1` C-style form would
avoid that, or `range` could return an iterator, not a list.

---

## 6. Per-iteration state in loops

**Friction**: Want `for enemy in enemies { state hp_anim = enemy.hp ... }`
keyed by enemy ID. Per-iteration `state` is partially in place (see
`examples/particles.ptl` and `ts/test/loop-state.test.ts`) but doesn't yet
key cleanly off a domain identifier — it keys off the loop iteration index,
which breaks if the list reorders or items are removed.

---

## 7. `match` on records would be lovely

**Friction**: Today match works on enums. For a record-based entity system,
a guard chain like `if e.kind == "bullet" { ... } else if e.kind == "enemy"`
gets repetitive.

**Proposed**: `match e { {kind: "bullet", ...} -> ...; {kind: "enemy", hp} -> ... }`

---

## 8. Named-argument calls

**Friction**: `triangle3d(x1, y1, z1, x2, y2, z2, x3, y3, z3, r, g, b)` —
twelve positional args. Easy to swap z2 and z3 by accident, and the code is
unreadable.

**Proposed**: Accept a single record literal: `triangle3d({v1: vec3(...),
v2: vec3(...), v3: vec3(...), color: rgb(255,0,0)})`. Or named keyword args:
`triangle3d(v1=..., v2=..., v3=..., color=...)`.

---

## 9. Hot reload state preservation for `state x = []` (lists)

**Friction**: Open question — when I edit the source file mid-game, does the
`enemies` list get preserved? Per the docs it should, since name-based keys
work. Worth verifying with a deliberate reload mid-frame.

---

## 10. A `time()` builtin that returns seconds since program start

**Friction**: `frame_count() * dt()` doesn't work because `dt()` varies; today
you have to accumulate `state t = 0; t += dt()` everywhere.

---

## 11. Better error locations for native function arg mismatches

**Friction**: When I called `triangle3d` with the wrong arg count or type, I
got a generic "Expected float at arg N, got int" which doesn't reference my
.ptl source line. Adding a stack trace from the Petal call site would be
huge for live game-dev.

---

## 12. `print` lacks formatting / flush control

**Friction**: For HUD-style live debugging, I'd want `printf("hp=%d pos=(%.2f,%.2f)", hp, px, pz)`. Today: `print("hp=" ++ str(hp) ++ " pos=(" ++ str(px) ++ "," ++ str(pz) ++ ")")`. The `++` chain is ugly.

**Proposed**: String interpolation with format specs: `print($"hp={hp} pos=({px:.2},{pz:.2})")`.

---

## 13. Built-in physics primitives

**Friction**: AABB intersection, ray-vs-AABB, ray-vs-sphere are the same in
every game. Could be standard library: `aabb_overlap(a, b)`, `ray_aabb_hit(origin, dir, box)`.

---

## 14. `fn` as a value isn't easy to put in a record

**Friction**: For component-style entity behavior (each enemy has a `.update`
function), I'd want `{kind: "robot", update: fn(self, dt) { ... }}`. Need to
verify whether this round-trips through hot reload.

---

## 15. Lack of `f32` — everything is `f64`

**Friction**: Native triangle_3d signatures take `f32`s for performance, so
`get_float` truncates. For a CPU rasterizer this is the right tradeoff but
worth surfacing.

---

## 16. Scientific notation numeric literals (`1e9`, `2.5e-3`)

**Friction**: Wrote `let best_t = 1e9` as a sentinel "very large" value for a
closest-hit raycast. Petal parses this as the token `1` followed by an
identifier `e9`, so it produced `Undefined variable: e9` at runtime. I had
to write `1000000.0` instead, which is visually harder to parse.

**Proposed**: Lex `1e9`, `2.5e-3`, `6.02e23` as float literals — same rule as
most languages. Should be a small lexer change.

---

## 17. `now()` / timer that agents can reset

**Friction**: In `--screenshot` mode the scene is frozen at the first frame.
No wall-clock is advancing so `dt()` yields 1/60 forever, but there's no
`time_since_start()` that would let animations (muzzle flash, neon pulse)
be reproducible. Today I keep `state muzzle = 4; if muzzle > 0 { muzzle -= 1 }`
which works but would be cleaner as `time() mod cycle`.

---

## 18. `state` initialization expression is evaluated every frame

**Friction**: I wrote `state enemies = [{...}, {...}, ...]` (a 8-element
literal). The RHS is evaluated every frame — the "first time" check only
decides whether to *use* the result, but allocates records+lists regardless.
For a large level-geometry literal (the 12 buildings in fps_game.ptl), this
is wasted work. 

**Proposed**: Skip RHS evaluation entirely when the state key is already
set. Would require the compiler to emit a conditional around the init
expression, guarded by the StateInit presence check — the information is
already there in the IR.
