# Petal as a Creative-Coding Language: Evaluation & Feedback

Written while implementing five generative-art examples for `petal-sdl`:

| File | Idea |
|---|---|
| `cc_strange_attractor.ptl` | Clifford & De Jong attractors |
| `cc_metaballs.ptl` | Implicit-surface blobs on a coarse grid |
| `cc_10_print.ptl` | The Commodore 64 `10 PRINT` weave |
| `cc_differential_growth.ptl` | Self-avoiding curve that buds into lobes |
| `cc_reaction_diffusion.ptl` | Gray-Scott model |

## Bugs found and fixed in this session

### 1. `random()` returned values clustered near 0 (and within a frame, all the same)

`builtins/math.rs::native_random` was implemented as
`subsec_nanos() / 4_294_967_295.0`.

- `subsec_nanos()` returns 0–999_999_999, so dividing by 2³² produced values
  in `[0, 0.233)` rather than `[0, 1)`. Anything that called
  `random(0.0, 1.0)` and then thresholded on 0.5 got `false` 100% of the
  time.
- All `random()` calls inside one frame ran within the same nanosecond, so
  successive calls returned essentially the same value. In the metaballs
  sketch, all five blobs spawned on top of each other.

`random_int()` and `choose()` had the same property: same nanosecond → same
index, so `choose(palette)` is effectively constant per frame.

**Fix.** `builtins/mod.rs` now hosts a process-wide xorshift64* state seeded
from system time on first use. `random`, `random_int`, and `choose` route
through it. No external dep added.

### 2. Re-assigning a `state` variable a second time silently dropped

```petal
state x = 0
x = 5
x = 10
```

After running this, `x` was 5, not 10. Multiple state assignments per frame
are common in real game code (`if hit { score += 1 }; if combo { score *= 2 }`),
so this was a serious correctness hole.

The compiler fix lives in `compile_assign`. Each assignment to a state
variable does two things: emit a `StateWrite` (so the new value persists into
the next frame) and emit a `Copy` term that rebinds the name in scope. The
`Copy` was previously plain — its only input was the assigned value — so when
a second assignment looked the name up in scope and walked back through the
binding chain, it landed on `Copy(value)` and `find_state_init` returned
`None`. No `StateWrite` was emitted for the second assignment.

**Fix.** A `state_inits: HashMap<StateKey, TermId>` is now populated when a
`StateInit` is emitted. The `Copy` term produced by a state-tracking
assignment has its `state_key` set to the original `StateInit`'s key, and
`find_state_init` walks `Copy` nodes by looking up that key in the map. New
regression tests in `ts/test/bug-state-in-if.test.ts`.

This is in the same family as a previously-fixed bug (`state` assignment
inside an `if` body) — the existing test file's name and lengthy comment
suggest this category warrants a deeper audit. The IR has SSA-style `Copy`
nodes that mask the underlying state-init through re-binding; any future
`compile_assign`-adjacent change should run the IR-level check
`StateWrite` count == top-level reassignment count.

## Language evaluation

### What worked very well

- **`state` declarations + hot reload.** This is the right model for
  creative coding. Code lives at the top level, the engine re-runs it every
  frame, and `state` does the right thing so you can iterate live without
  losing your simulation. It's the p5.js `setup` / `draw` split done well.
- **Records and lists are pleasant.** `{ x: ..., y: ..., vx: ..., vy: ... }`
  for particles reads like Python dicts and is fast enough for ~1k entities.
- **Builtins are well-chosen.** `lerp`, `clamp`, `map_range`, `smoothstep`,
  `vec2`, `noise`, `hsv` — exactly the toolbox a creative coder reaches for.
  Names match Processing/p5.js, which lowers the cognitive cost of porting
  examples.
- **The headless agent protocol is gold for this work.** Being able to step
  N frames, snapshot state, and grab a screenshot from a CI-like pipeline
  meant I could iterate on these sketches without ever opening a window.
  More languages should ship with this.

### Friction points

#### `hsv()` takes hue in 0–360, not 0–1

Every other piece of the colour API is normalized — `s`, `v`, `l`, alpha.
But hue is in degrees. p5.js, three.js, and Processing all default to 0–1 (or
make it explicit with `colorMode`). Every existing `petal-sdl` example
sidesteps this by computing RGB by hand instead of calling `hsv()`. That's
the tell — no one was reaching for the function.

Suggested fix: change `hsv` and `hsl` to take hue in `[0, 1)`, document the
break in `CHANGELOG.md`, and provide `hsv_deg`/`hsl_deg` for the rare cases
someone wants to type `120` for green. Now is the right time — the function
is essentially unused by current sketches.

#### No persistent canvas / framebuffer

Every frame redraws from scratch. That's correct for game-style frames but
fights the grain of generative art, where the *accumulated trace* is the
art (Lissajous, attractors, painted brush strokes, particle trails with
fading). Today the workaround is to keep a list of past particles in `state`
and redraw them all every frame, which is O(n) per frame on top of n's
growth and so blows up fast.

Two practical paths:

1. **`begin_frame()` opt-in to skip clearing.** Cheapest possible change —
   if a frame doesn't call `clear()`, just don't clear the back buffer.
   Existing sketches already call `clear()` at the top, so it's
   backward-compatible.
2. **Offscreen-canvas primitive.** `let canvas = create_canvas(w, h)`,
   `draw_to(canvas) { ... }`, `draw_canvas(canvas, x, y)`. This is the
   mechanism Processing's `PGraphics` uses for trails, masks, and
   compositing.

(2) is the standard creative-coding move. (1) handles ~80% of the use case
for almost no engineering.

#### No filled triangle / polygon

`draw_circle` is filled, `draw_rect` is filled, but `draw_line` is the only
"path" primitive — there's no `fill_triangle` or `fill_poly`. Differential
growth wants filled lobes; metaballs would benefit from triangulating an
iso-contour; an L-system grass example wants to fill leaves. Right now you
fake it with stacks of lines, which renders flicker-y at large sizes.

#### Performance: the inner-loop tax

Order of magnitude (release build, headless, 800×600):

| Sketch | Per-frame inner work | fps |
|---|---|---|
| Strange attractor | 3000 sin/cos iterations + 3000 1×1 rects | ~17 |
| Metaballs (10-px cells, 5 balls) | ~4800 cells × 5 = 24k field evals | ~11 |
| Reaction-Diffusion (50×37, 3 sub-steps) | ~28k cell updates | ~12 |
| Differential growth, 250 nodes | O(n²) repulsion = 62k | ~12 |

For a creative coding language these numbers are workable but tight. p5.js
on V8 routinely does 100k+ operations per frame. Most of the loss is the
`for in range(...)` plus list-indexed access pattern; the Rust evaluator
walks the IR per term, and a 5000-iteration script stack blows past the
hot path on every frame.

The two highest-leverage improvements I'd want:

- **A `FlatList<f64>` value type** (or "typed array") that bypasses
  `Heap::get_list` boxing and lets numeric inner loops hit cache. Even
  exposing it only via builtins (`f64_array(n)`, `set/get/swap`) would let
  the simulation kernels above run 5–10× faster without language changes.
- **A `forEach`-style intrinsic that operates on numeric ranges without
  allocating a `range()` list each invocation.** I noticed `range` is a
  builtin returning a list; in a 5000-iteration inner loop the allocator
  and indexing pressure eclipses the actual math.

#### Documentation friction

The game dev guide is excellent — but a few things only became clear by
reading source:

- `hsv(h, s, v)` listed without saying h is degrees.
- `random()` documented as "random float in [min, max)" but until today did
  neither half of that interval correctly.
- `noise()` accepts 1, 2, or 3 args (octave-less Perlin) — not in the guide.
- The headless `--screenshot` flag doesn't render text (no font available
  in headless renderer). Worth a one-liner so people don't think their
  `draw_text` calls are broken.

### Things I wanted but didn't implement

- **Differential growth with proper spatial hashing** would let `n` reach
  the thousands, but doing spatial hashing in Petal-as-it-stands requires a
  list-of-lists of buckets and adds enough indexing that the constant-factor
  win is small. A `grid_lookup(field, x, y)` builtin would unlock this.
- **A second-order strange attractor (Lorenz / Aizawa) projected to 2D**
  was tempting, but the 50,000+ point trace per frame for a recognizable
  shape pushed past what the interpreter can do at 30 fps.
- **Audio reactivity** would be a wonderful addition for this engine —
  `audio_amplitude()` / `audio_fft(n)` builtins fed from system audio.

## Bottom line

Petal is a *very* nice fit for creative coding — the live-reload loop, the
right-shape builtins, the records-and-lists data model, and the headless
agent protocol are all strong. The two bugs I hit today were genuine
foot-guns (the `random()` one in particular makes any shuffle/distribution
work silently wrong); the friction points are more about closing the gap to
p5.js than about anything fundamentally broken. With a `FlatList<f64>`
type, an offscreen canvas, and a polygon fill, this could comfortably host
the full Nature of Code curriculum at interactive frame rates.
