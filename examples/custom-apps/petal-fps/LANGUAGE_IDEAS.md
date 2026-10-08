# Petal language ideas from petal-fps

Friction found while building petal-fps, a 3D shooter in Petal. Each entry
notes the problem and a possible fix. Entries marked **Shipped** have since
landed in the language; the rest are still open.

---

## 1. Built-in `vec3` (and `mat3`/`mat4`)

**Open.** `vec2` exists, but a 3D game wants 3D vectors everywhere. Every
position, velocity, ray, and vertex is a `{x, y, z}` record, and every
operation is written out by hand (`let dx = a.x - b.x; let dy = ...`).

Proposed: `vec3(x, y, z)` with operators like `vec2`, plus `normalize`, `dot`,
`cross`, `mag`, and a `mat4` with translate/rotate/perspective/multiply/apply.
The camera and projection code would shrink from about 40 lines to 8.

---

## 2. `state` inside functions — **Shipped**

Wanted: a per-frame cache inside `fn project(p)` for values derived from the
camera, without hoisting them to top level.

A `state` slot is now keyed by its declaration and the call path that reached
it, so a `state` in a helper gets one cell per callsite (and per loop
iteration). An explicit key ignores the call path, and the init expression
only runs on a key miss, so keying on the frame counter gives a once-per-frame
cache:

```petal
state frame = 0
frame = frame + 1

fn project(p)
  state(frame) cam = camera_basis()   // computed once per frame
  ...
end
```

See `docs/dev/state-call-paths.md`.

---

## 3. Overloading is invisible in the source

**Open.** Petal overloads by arity (see `docs/function-overloading.md`), but
reading two `fn draw_line(...)` definitions you cannot tell whether the second
replaces the first or adds an overload. Proposed: explicit syntax such as
`fn draw_line/3(...)`.

---

## 4. Destructuring on assignment

**Open.** `match` can destructure lists (`when [head, ...tail]`), but there is
no `let {a, b} = record` or `let [x, y, z] = list`. Swapping two values or
returning a pair needs a temporary or a record.

---

## 5. `range(0, n)` allocates a list

**Open.** `for i in range(0, n)` builds a list each time. In tight inner loops
(rasterizing hundreds of buildings) that is wasted work. A lazy range or a
C-style counted loop would avoid it.

---

## 6. Per-iteration state in loops — **Shipped**

An unkeyed `state` inside a loop body, or inside a function called from the
loop, gets one cell per iteration index. `state(item.id) hp = item.hp` keys by
a domain identifier instead, so the cell follows the item across reorders and
removals. See `docs/dev/state-call-paths.md`.

---

## 7. `match` on records

**Open.** `match` handles enums, literals, and list patterns, but not record
patterns. A record-based entity system falls back to `if e.kind == "bullet"
... else if e.kind == "enemy" ...` chains. Proposed:
`match e when {kind: "bullet"} -> ... when {kind: "enemy", hp} -> ...`.

---

## 8. Named arguments — **Shipped**

`triangle3d(x1, y1, z1, x2, y2, z2, x3, y3, z3, r, g, b)` was twelve positional
arguments. Calls can now name their arguments; see "Named Arguments" in
`docs/language-guide.md`.

---

## 9. Hot reload of list-valued `state`

**Open question.** Whether `state enemies = [...]` survives a mid-game source
edit has not been checked deliberately. It should, since slots are keyed by
declaration rather than source position.

---

## 10. Seconds since program start — **Shipped**

`time()` (from `petal-ui`) returns an absolute clock in seconds, read from the
host each frame rather than summed from `dt()`. The prelude's `elapsed()`
returns seconds since its callsite was first reached.

---

## 11. Better error locations for native argument mismatches

**Open.** Calling `triangle3d` with the wrong argument count or type gives
"Expected float at arg N, got int" with no reference to the `.ptl` line. A
stack trace from the Petal call site would help a lot during live editing.

---

## 12. String formatting — **Partly shipped**

String interpolation shipped: `print("hp={hp} pos=({px},{pz})")`. Format
specs such as `{px:.2}` are still open.

---

## 13. Built-in physics primitives

**Open.** AABB overlap, ray-vs-AABB, and ray-vs-sphere are the same in every
game and could live in the standard library.

---

## 14. Functions as record fields

**Open question.** Component-style entities (`{kind: "robot", update: fn(self,
dt) ... end}`) should work since functions are values, but whether such records
round-trip through hot reload has not been verified.

---

## 15. No `f32`

**Note.** All floats are `f64`. The native `triangle3d` signatures take `f32`
for rasterizer speed, so values are narrowed on the way in. This is the right
tradeoff; it is just worth knowing.

---

## 16. Scientific notation literals — **Shipped**

`1e9`, `2.5e-3`, and `2E+4` lex as float literals.

---

## 17. Reproducible time in `--screenshot` mode — **Shipped**

Headless, screenshot, and record runs bind `time()` from a fixed clock
(`frame_count / 60`) instead of the wall clock, so time-based animations
(muzzle flash, neon pulse) are reproducible frame for frame. See
`ClockSource` in `integrations/petal-desktop-sdl/src/protocol.rs`.

---

## 18. `state` init expression evaluated every frame — **Shipped**

The init expression now runs only when the slot is missing from the persistent
store, so a large `state enemies = [...]` literal is built once. Covered by
`rust/tests/state_lifecycle.rs` and `ts/test/state-lazy-init.test.ts`.
