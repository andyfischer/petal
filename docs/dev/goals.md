# Petal — Goals

The single source for Petal's **vision** (the *why* and the eventual *where*)
and its **remaining unfinished work** (the honest *what's left*). Shipped goals
are deleted from this file as they land — if a capability isn't listed under
"Remaining work" below, it's either done or was never planned.

> Design context for already-shipped pillars lives elsewhere and is not
> repeated here: [ir-as-target.md](ir-as-target.md) (the IR-as-emit-target
> contract) and [Architecture.md](Architecture.md) (how the term graph
> realizes these goals).

Status legend:

- 🟡 **Partial** — a usable subset exists; named gaps remain.
- 🔭 **Aspirational** — described in the vision, not yet started.
- ⚠️ **Needs hardening** — works, but has a known correctness/robustness risk
  under a core promise.

Last reviewed: 2026-06-22.

---

## North Star

Petal is built on a central insight: **programs are graphs of data
transformations, and making this structure explicit enables capabilities**
traditional imperative languages struggle to provide. Four pillars:

1. **Dataflow-first semantics.** Every construct maps to a dataflow graph, so
   the flow of data is explicit and traceable. This is the substrate for
   provenance ("what influenced this value?"), program slicing, and
   differentiable programming (apply the chain rule through the active
   computation graph to suggest source-value changes that move a result toward
   a target — back-propagation for general programs, with the human in the loop
   to resolve ambiguity).

2. **First-class state.** Inline `state` (React-`useState`-like, but a language
   primitive) declared where it's used, including inside loops and conditionals
   for per-iteration / per-branch state. State creates *temporal edges* in the
   dataflow graph, so stateful computation stays traceable and differentiable
   across time steps.

3. **Projectional views.** Derive simplified representations of a program by
   focusing on one aspect — program slices, scenario-based views (only the
   branches/iterations that ran), and ultimately *bidirectional* projections
   where edits on the view map back to the source. The far end of this pillar
   is cross-language abstract programming: manipulating foreign programs through
   a Petal lens.

4. **Live editing.** Modify source while a program runs and see changes take
   effect without losing state. Inline state makes state reconciliation
   principled: additions default-initialize, removals are GC'd, modifications
   migrate by structural correspondence between source locations and runtime
   state.

These compose: project a program to the slice that influences a chosen output,
back-propagate sensitivities along that slice, and live-edit the source
constants — all while state is preserved.

### Design implications

- **Syntax:** pipe (`|>`) and method-call sugar make dataflow visible at the
  call site; expression-oriented; immutable IR (rebindings lower to pure
  `Phi` joins) preserves traceability.
- **Semantics:** no hidden side effects; state changes are explicit; calls are
  referentially transparent modulo state.
- **Tooling:** the compiler keeps dataflow metadata so tools can query
  provenance, compute projections, and differentiate.

### Related work (the compass)

Dataflow languages (Lucid, Lustre, LabVIEW); automatic & differentiable
programming (JAX, PyTorch, Swift for TensorFlow); FRP (Elm, signal graphs);
program slicing (Weiser); data provenance / lineage; bidirectional transforms
and lenses; projectional editing (JetBrains MPS); live coding (Sonic Pi, Tidal,
Extempore); hot reloading (Smalltalk images, Erlang hot swap, React Fast
Refresh).

---

## Where we are

The **foundation is real and shipped**: a flat, SSA-style dataflow term graph
with explicit input edges, `Phi` joins for rebinding (no mutation primitive),
reified control flow, first-class
`state` with temporal edges, a mark-sweep GC, and a bytecode VM. The
introspection pillars built on it — provenance, forward dependents, slicing,
`ExplainTerm`, structured traces — and **state-preserving hot reload** are
shipped and tested. Forward-mode autodiff (dual numbers) propagates through
arithmetic and `sin`/`cos`/`tan`/`sqrt`/`abs`. The IR is a documented
load-and-run **emit target** (`run --ir`; see [ir-as-target.md](ir-as-target.md)).

The **near-term wedge** is live, dataflow-legible **creative coding**, where
Petal's state + hot-reload + legibility combination is genuinely
differentiated. The Phase 0 perf/ergonomics enablers for it have shipped:
typed numeric arrays (`f64_array`), non-allocating range loops
(`NumericForLoop`), offscreen canvases, filled polygon/triangle primitives, and
normalized `hsv`/`hsl` hue.

The gap to the **headline research payoffs** remains: reverse-mode
back-propagation, bidirectional projectional editing, and general cross-language
mounting are still aspirational.

---

## Remaining work

### Goal 1 — Dataflow-first semantics

| Capability | Status | Notes |
|---|---|---|
| Reverse-mode AD / back-propagation ("specify a target, suggest source-value changes") | 🔭 | No gradient/adjoint code exists. The headline differentiable-programming goal; centerpiece of Phase 1. |
| Forward-mode derivatives for `exp` / `log` | 🟡 | `exp`/`log` operate on the primal only and drop the derivative. `sin`/`cos`/`tan`/`sqrt`/`abs` already propagate. Small, mechanical to close. |

### Goal 2 — First-class state

| Capability | Status | Notes |
|---|---|---|
| State correctness under repeated reassignment / SSA `Copy` masking | ⚠️ | A second `state` reassignment per frame previously dropped silently (SSA `Copy` masked the `StateInit`); fixed, but no invariant check exists yet. **Add the compiler-side check** (`StateWrite` count == top-level reassignment count) and property-test the Phi/state machinery. State trust is existential for the pitch. |

### Goal 3 — Projectional views

| Capability | Status | Notes |
|---|---|---|
| Dynamic / scenario slices ("what was active for this run") as a product *view* | 🟡 | The trace buffer captures execution; a polished "projected linear trace for one scenario" surface is not yet built. |
| Bidirectional / projectional editing (edit the projection, map edits back) | 🔭 | No edit-mapping exists. Related to lenses / MPS. North Star. |
| Cross-language abstract programming (mount foreign programs through a Petal lens) | 🔭 | Research-scale moonshot as originally framed. The tractable inverse — *be a legible IR others emit into* — has shipped (see [ir-as-target.md](ir-as-target.md)). |

### Goal 4 — Live editing

| Capability | Status | Notes |
|---|---|---|
| Incremental dataflow update (recompute only affected nodes) | 🟡 | Integrations re-run the whole program each frame; true incremental graph diffing isn't implemented. Fine at current frame sizes; relevant if sketches scale. |
| Live editing flowing through back-prop paths | 🔭 | Depends on reverse-mode AD (Goal 1). Realized as the Phase 1 drag-to-edit demo. |

### Cross-cutting

| Area | Status | Notes |
|---|---|---|
| AI-legibility as a *named* goal | 🟡 | The pieces ship and de-facto differentiate (MCP tools, headless protocol, structured JSON traces, `ExplainTerm`). Remaining: consolidate them into a coherent, documented agent-facing surface (Phase 2). |
| Types as a projection | 🔭 | Dynamically typed today (runtime tags only). Future: infer shapes from the dataflow graph and surface them to tooling/agents — never enforced (consistent with "low floor, forgiving types"). |
| Modules / imports | ✅ | v1 landed: `import` with qualified/selective/alias forms, pluggable resolution (in-memory, importer-relative, search paths), merge-at-compile-time, module-qualified state keys, hot reload across files. See [module-system.md](../module-system.md). |
| Performance | 🟡 | Introspection-first interpreter, not a fast VM. The Phase 0 enablers shipped; heavy sketches still run ~11–17fps with an inner-loop boxing tax. Profile and chip away as the wedge demands. |

---

## Sequencing

Commit to **live, dataflow-legible creative coding** as the near-term wedge, and
make the research pillars pay rent *inside* that wedge rather than as separate
moonshots. The foundation is shared, so this is about headline and ordering.

### Phase 0 — Foundation trust (nearly done)

Perf/ergonomics enablers have all shipped (typed arrays, non-allocating range,
offscreen canvas, polygon fill, hue normalization). **Remaining:** the
state-correctness audit — add the invariant check and property-test the
Phi/state machinery (see ⚠️ in Goal 2). That's the exit criterion.

### Phase 1 — Differentiable direct-manipulation live coding

One demo exercising Goals 1, 3, and 4 in a single gesture, which finally makes
reverse-mode AD worth building by scoping it to a use case:

- Implement **reverse-mode AD** over the dataflow graph, scoped to the
  drag-to-edit path (not general program optimization). Closes the Goal 1 gap.
- **Drag an output on the canvas** → back-prop to the source constants that
  influence it → **project the candidate slice** of editable constants → when
  ambiguous, show the slice and let the user pick → **live-edit the source
  numbers** with state preserved.
- **Scrubbable provenance** (companion): record input/frame history; scrub past
  frames and ask "why is this pixel this color"; on hot-reload, re-run recorded
  history through the edited graph to show a change retroactively.

### Phase 2 — AI-legibility as a named goal

- Consolidate `ExplainTerm` / trace / headless protocol into a coherent,
  documented agent-facing surface.
- **Types as a projection:** infer shapes and surface them to tooling/agents
  (hover, structured output) without enforcement.

### North Star (not scheduled)

Full bidirectional projectional editing, general cross-language mounting, and
back-propagation as general-program optimization. The compass, not the roadmap;
the phases above are how we earn the right to attempt them.

---

## Creative-coding ergonomics — open items

Petal is a strong fit for creative coding (live-reload loop, right-shape
builtins, records-and-lists data model, headless agent protocol). The shipped
vocabulary — `clamp`/`lerp`/`map_range`/`distance`/`smoothstep`/noise/`vec2`
with operator overloading, `random_int`/`choose`, `hsv`/`hsl`/`color_lerp`,
record spread, in-place field mutation, typed arrays, offscreen canvases,
filled polygons — is documented in [Builtins.md](../Builtins.md). What's left,
ranked roughly by impact-per-effort:

- **Drawing functions accept float.** `petal-sdl` draw fns still require `int`,
  forcing `int()` casts everywhere physics meets rendering. Accept float and
  truncate internally. Lowest effort, highest ergonomic payoff for anything
  visual.
- **Destructuring let** — `let { x, y, vx, vy } = particle`. Complements record
  spread; check `ast.rs` first, the pattern-matching infra may already cover
  most of it.
- **Easing functions** — `ease_in`/`ease_out`/`ease_in_out` or `ease(t, kind)`.
  Builtins only, trivial.
- **`random_gaussian(mean, stddev)`** — natural-looking scatter/particle
  distributions. Small.
- **More draw primitives & styling (SDL)** — `draw_ellipse`, `draw_arc`,
  outlined `draw_polygon`; accept color records directly
  (`draw_circle(x, y, r, color)`); alpha/opacity (`set_alpha`); separate fill
  and stroke (`set_fill`, `set_stroke`, `set_stroke_width`).
- **Transformation stack (SDL)** — `push_matrix`/`pop_matrix`/`translate`/
  `rotate`/`scale`. Essential for hierarchical animation; touches the renderer.
- **List comprehensions** — `[ expr for i in range(...) ]`. Sugar over the `for`
  + `push` loop; big readability win for initializers. Medium parser work.
- **`vec3` / `vec4`** — generalize the `vec2` machinery if 3D/4D color math or a
  3D renderer ever becomes interesting. Not urgent.

Wishlist (larger / speculative):

- **Audio reactivity** — `audio_amplitude()` / `audio_fft(n)` builtins fed from
  system audio.
- **Spatial-hash helper** — a `grid_lookup(field, x, y)` builtin to make
  `O(n²)`-style neighbor queries (differential growth, flocking) tractable at
  large `n`.

### Doc nits worth fixing

- `noise()` accepts 1, 2, or 3 args (octave-less Perlin) — document the arities.
- The headless `--screenshot` renderer has no font, so `draw_text` is a no-op
  there — note it so people don't think text drawing is broken.

### Design philosophy (creative coding)

Low floor, high ceiling · math as prose · visible by default · forgiving types
(truncate a float where an int is wanted; unpack a color record) · built-in
vocabulary for the domain. Lessons drawn from Processing/p5.js (`map`,
`push`/`pop`, `colorMode`, Perlin `noise`), GLSL/Shadertoy (`smoothstep`,
`fract`, `mix`, component-wise vector math), Sonic Pi (domain vocabulary, rings,
live reload), Nannou (good defaults over verbose types), and Scratch (remove
complexity, immediate feedback).
</content>
</invoke>
