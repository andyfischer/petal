# Petal — Goals

The single source for Petal's **vision** (the *why* and the eventual *where*)
and its **remaining unfinished work** (the honest *what's left*). Shipped goals
are deleted from this file as they land — if a capability isn't listed under
"Remaining work" below, it's either done or was never planned.

> Design context for already-shipped pillars lives elsewhere and is not
> repeated here: [ir-as-target.md](ir-as-target.md) (the IR-as-emit-target
> contract), [Architecture.md](Architecture.md) (how the term graph realizes
> these goals), [state-call-paths.md](state-call-paths.md) (call-path keyed
> `state`), [../var.md](../var.md) (mutable cells), and
> [../direct-manipulation.md](../direct-manipulation.md) (provenance and
> goal-based editing).

Status legend:

- 🟡 **Partial** — a usable subset exists; named gaps remain.
- 🔭 **Aspirational** — described in the vision, not yet started.
- ⚠️ **Needs hardening** — works, but has a known correctness/robustness risk
  under a core promise.

Last reviewed: 2026-08-30.

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
   for per-iteration / per-branch state. A slot is the declaration plus the
   **call path** that reached it, so a helper holding a `state` gives each of
   its callers an independent value, exactly as a React component instance gets
   its own hook slots. State creates *temporal edges* in the dataflow graph, so
   stateful computation stays traceable and differentiable across time steps.

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
  call site; expression-oriented.
- **Semantics:** no hidden side effects; state changes are explicit; calls are
  referentially transparent modulo state. The default binding is a dataflow
  binding — rebinding lowers to a pure `Phi` join, not a store. **Mutation is
  opt-in and visible:** where code genuinely wants a slot, `var` declares a
  cell, every write says `set`, and `get` is the explicit cross-function read.
  Dataflow walks stop at a cell and report the frontier, so the places the
  dataflow story goes dark are exactly the places the reader can see.
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
with explicit input edges, `Phi` joins for rebinding, `var`/`set`/`get` cells as
the visible mutation escape hatch, reified control flow, call-path-keyed
first-class `state` with temporal edges, a mark-sweep GC, and a bytecode VM. The
introspection pillars built on it — provenance, forward dependents, slicing,
`ExplainTerm`, structured traces — and **state-preserving hot reload** are
shipped and tested. Forward-mode autodiff (dual numbers) propagates through
arithmetic and `sin`/`cos`/`tan`/`sqrt`/`abs`. The IR is a documented
load-and-run **emit target** (`run --ir`; see [ir-as-target.md](ir-as-target.md)).

**Direct manipulation shipped — without back-propagation.** The Phase 1 headline
gesture ("point at what a program drew, say what it should have been, get the
edit") is built and documented, via a different mechanism than the one this file
originally assumed: emit-trace provenance records the call id the VM already
computed, resolves it to a source span and per-argument literals, and
**goal-based editing** rewrites the source through lossless CST primitives. See
[../direct-manipulation.md](../direct-manipulation.md) and
[../goal-based-editing.md](../goal-based-editing.md). This is a real result and
it changes the roadmap: reverse-mode AD is no longer the gate on the demo that
was meant to justify it (see Sequencing).

**The language filled out.** Classes over records (`class Rect … end`, methods,
no object model), optional type annotations with a warning-only checker,
module system v1, `??` / `?.`, `get`, a linter with `--fix`, an LSP
(`petal lsp`), and tree-sitter support.

**Petal grew a host ecosystem.** Since the last review the bulk of the work has
been Petal *as an application scripting language*: [Garden](../../garden/README.md)
(a GPU-accelerated IDE whose panes, layout and UI screens are Petal scripts),
the `petal-ui` component library, GPP protocol v2 (panel-only, id-correlated
JSON-RPC) with a Python client, a ~20-app panel testbed, and `petal-fantasy-nes`
(a fantasy console with a PPU, APU, tracker and demo carts). Alongside it, a
refactor-verification stack — deterministic runs (`--seed`, `--error-format
bare`), the `petal-ui-run` headless driver, IR-equivalence (`petal ir-equal`),
and plan-driven differential verification.

The gap to the **headline research payoffs** remains: reverse-mode
back-propagation, bidirectional projectional editing, incremental evaluation,
and general cross-language mounting are still aspirational.

---

## Remaining work

### Goal 1 — Dataflow-first semantics

| Capability | Status | Notes |
|---|---|---|
| Reverse-mode AD / back-propagation | 🔭 | Still no gradient/adjoint code (`rust/src/builtins/autodiff.rs` is forward-mode duals only). **Its motivating use case has been served another way** — drag-to-edit now works through provenance + goal-based editing, which is sound for literal arguments and needs no differentiability. Reverse-mode AD is now justified only by the cases provenance *can't* reach: an output that depends on a source constant through arithmetic rather than as a direct argument. Worth scoping to a concrete such case before building. |
| Forward-mode derivatives for `exp` / `log` | 🟡 | Confirmed still open: `native_exp`/`native_log` (`rust/src/builtins/creative_coding.rs:188`) call `get_float` and drop the derivative, unlike the `unary_float_dual` path used by `sin`/`cos`/`tan`/`sqrt`. Small, mechanical, ~10 lines. |

### Goal 2 — First-class state

| Capability | Status | Notes |
|---|---|---|
| State correctness under repeated reassignment / SSA `Copy` masking | ⚠️ | Partly closed. `rust/src/ir_validate.rs` now enforces state-op invariants — every state op carries a `state_key`, and every `StateRead`/`StateWrite` key has a matching `StateInit`. **Still missing:** the reassignment-count check (`StateWrite` count == top-level reassignment count), which is the invariant that would have caught the original `Copy`-masking bug, and property tests over the Phi/state machinery — the workspace has no `proptest`/`quickcheck` dependency. State trust is existential for the pitch. |

> `state` is per *use* (React-`useState`-style) as of 2026-08-25: a slot is the
> declaration id plus the call path that reached it, so callsites, loop
> iterations and recursion depths get independent values. `state(key)` is the
> absolute escape hatch; shared cross-function state is a top-level `state var`
> cell. See [state-call-paths.md](state-call-paths.md).

### Goal 3 — Projectional views

| Capability | Status | Notes |
|---|---|---|
| Dynamic / scenario slices ("what was active for this run") as a product *view* | 🟡 | The trace buffer captures execution and `petal-ui-run` / the GPP panel-trace tooling emit structured per-frame records, but a polished "projected linear trace for one scenario" *surface* still isn't built. Garden is now the obvious place to put one. |
| Bidirectional / projectional editing (edit the projection, map edits back) | 🟡 | Upgraded from 🔭. Edit-mapping now exists for one concrete projection: the rendered canvas. Provenance maps an emitted draw command back to its call and arguments, and goal-based editing maps a stated outcome back to a source mutation. That is a working bidirectional lens over *emitted output*, not over arbitrary projections — generalizing it (to slices, to traces, to structural views) is the remaining research. |
| Cross-language abstract programming (mount foreign programs through a Petal lens) | 🔭 | Research-scale moonshot as originally framed. The tractable inverse — *be a legible IR others emit into* — has shipped (see [ir-as-target.md](ir-as-target.md)). |

### Goal 4 — Live editing

| Capability | Status | Notes |
|---|---|---|
| Incremental dataflow update (recompute only affected nodes) | 🟡 | Unchanged and now the most load-bearing gap. Every integration re-runs the whole program per frame; a Garden panel re-runs its entire script every frame with no incremental evaluation ([performance.md](performance.md)). The workaround is discipline (hoist anything expensive behind state). This is the ceiling on how large a Petal-scripted app can get. |
| Live editing flowing through back-prop paths | 🟡 | Reframed. The *user-visible* goal — edit source live from a manipulated output, state preserved — is served by the provenance + goal-based path today. Only the back-prop-specific variant (edit a constant that reaches the output through arithmetic) still depends on Goal 1. |

### Cross-cutting

| Area | Status | Notes |
|---|---|---|
| AI-legibility as a *named* goal | 🟡 | More pieces have shipped and de-facto differentiate: MCP tools ([mcp-server.md](mcp-server.md)), the GPP v2 protocol with one wire definition, slimmed schema-0.2 IR/AST JSON dumps, `ExplainTerm`, `--observe`, goal-based editing as a programmatic edit API, and deterministic differential verification. Remaining: consolidate them into a coherent, *documented* agent-facing surface rather than a set of separately-documented tools. |
| Types as a projection | 🟡 | Still dynamically typed at runtime, by design. Since the last review: classes (`class Rect … end` naming a record shape with a constructor, type name and `fn Rect.method` declarations), receiver/field/arity/bound-signature checking, and `set` checked against a var's declared type. The checker still only ever *warns*. Future: richer inference from the dataflow graph, parameterized types. See [type-declarations-plan.md](type-declarations-plan.md). |
| Performance | 🟡 | Introspection-first interpreter, not a fast VM. Shipped since last review: copy propagation, O(1) builtin dispatch, execution profiling, closure GC, and prelude cache hoisting. The dominant practical factor is now build profile — a script-heavy panel costs ~19 ms/frame debug vs ~2.5 ms release — so the first advice to an embedder is "build release". The structural cost (whole-program re-run per frame, inner-loop boxing) is the incremental-evaluation gap above. See [performance.md](performance.md). |

---

## Sequencing

### The open question: what is the wedge?

This file has committed to **live, dataflow-legible creative coding** as the
near-term wedge. The last two months of actual work point somewhere adjacent:
Garden, `petal-ui`, GPP v2, the panel-app testbed and the fantasy console are
Petal as an **application and UI scripting language** — a language you embed to
make a host's UI live-editable and legible. The creative-coding pillars (live
reload, state, provenance) are exactly what makes that work, so this is not a
pivot away from the thesis; but it is a different headline, a different demo,
and a different set of next tasks (incremental evaluation and component-library
depth, rather than easing functions and `vec3`).

**Decide this explicitly** before planning the next phase. The rest of this
section assumes both readings and marks which items serve which.

### Phase 0 — Foundation trust (nearly done)

The perf/ergonomics enablers shipped, `state` was fixed at the semantic level,
and the IR validator now enforces state-key invariants. **Remaining exit
criterion:** the reassignment-count invariant check and property tests over the
Phi/state machinery (see ⚠️ in Goal 2). Everything else in Phase 0 is closed.

### Phase 1 — Direct manipulation (largely shipped; finish the loop)

The gesture is built. What's left is making it a *demo* rather than an API:

- Wire the provenance → goal-based-edit loop into a live host end-to-end
  (Garden is the natural home) so drag-on-canvas → source rewrite → hot reload
  with state preserved is one continuous experience.
- **Scrubbable provenance** (still unbuilt): record input/frame history; scrub
  past frames and ask "why is this pixel this color"; on hot-reload, re-run
  recorded history through the edited graph to show a change retroactively.
- **Reverse-mode AD** is now *optional and unscheduled*, not the gate. Build it
  when a concrete manipulation is blocked by needing it — i.e. when the target
  constant reaches the output through arithmetic rather than as a literal
  argument. Until then it has no user waiting on it.

### Phase 2 — Scale the host story

Serves the app/UI-scripting reading of the wedge:

- **Incremental evaluation** — the single highest-leverage remaining item. It
  is the ceiling on Petal-scripted app size and it is squarely a Goal 4 pillar,
  so it pays rent in both readings of the wedge.
- Depth in `petal-ui` and the GPP surface, driven by what the panel testbed
  apps had to hand-roll.

### Phase 3 — AI-legibility as a named goal

- Consolidate `ExplainTerm` / traces / `--observe` / MCP / goal-based editing
  into one coherent, documented agent-facing surface.
- **Types as a projection:** continue inferring shapes and surfacing them to
  tooling/agents (hover, structured output) without enforcement.

### North Star (not scheduled)

Full bidirectional projectional editing beyond the canvas lens, general
cross-language mounting, and back-propagation as general-program optimization.
The compass, not the roadmap; the phases above are how we earn the right to
attempt them.

---

## Creative-coding ergonomics — open items

Petal is a strong fit for creative coding (live-reload loop, right-shape
builtins, records-and-lists data model, headless agent protocol). The shipped
vocabulary — `clamp`/`lerp`/`map_range`/`distance`/`smoothstep`/noise/`vec2`
with operator overloading, `random_int`/`choose`, `hsv`/`hsl`/`color_lerp`,
record spread, in-place field mutation, typed arrays, offscreen canvases,
filled polygons, `sort_by`, string formatting, `safe_div` — is documented in
[Builtins.md](../Builtins.md).

**Closed since the last review:**

- ~~Drawing functions accept float.~~ `PetalCxt::get_int` accepts `Value::Float`
  and truncates (`rust/src/native_fn.rs:257`), so every native taking pixel
  coordinates takes floats without an `int()` cast. The lint even removes the
  now-redundant identity casts.
- ~~Easing functions.~~ `ease_out` / `ease_in_out` are exported from the
  `petal-ui` prelude (`petal-ui/prelude/ui.ptl:1607`). `ease_in` is still
  missing — trivial to add for symmetry.
- ~~`draw_ellipse`.~~ Shipped in the Garden panel protocol.
- Per-call alpha shipped on the draw commands (the optional trailing `a`
  argument), and colors can be passed as records.

**Still open**, ranked roughly by impact-per-effort:

- **Destructuring let** — `let { x, y, vx, vy } = particle`. Confirmed absent
  (no destructuring in the parser or language guide). Complements record
  spread; check `ast.rs` first, the pattern-matching infra may already cover
  most of it.
- **`random_gaussian(mean, stddev)`** — natural-looking scatter/particle
  distributions. Confirmed absent. Small.
- **More draw primitives & styling** — `draw_arc`, outlined `draw_polygon`;
  global `set_alpha`; separate fill and stroke (`set_fill`, `set_stroke`,
  `set_stroke_width`). Per-call alpha already covers the common case.
- **Transformation stack** — `push_matrix`/`pop_matrix`/`translate`/`rotate`/
  `scale`. Confirmed absent everywhere. Essential for hierarchical animation;
  touches the renderer.
- **List comprehensions** — `[ expr for i in range(...) ]`. Confirmed absent.
  Sugar over the `for` + `push` loop; big readability win for initializers.
  Medium parser work.
- **`vec3` / `vec4`** — generalize the `vec2` machinery if 3D/4D color math or
  a 3D renderer becomes interesting. Not urgent.

Wishlist (larger / speculative):

- **Audio reactivity** — `audio_amplitude()` / `audio_fft(n)` builtins fed from
  system audio. Note that audio *output* now exists (the SDL queued-audio
  transport and the fantasy console's APU), so the host plumbing is half-built.
- **Spatial-hash helper** — a `grid_lookup(field, x, y)` builtin to make
  `O(n²)`-style neighbor queries (differential growth, flocking) tractable at
  large `n`.

### Doc nits worth fixing

- The headless `--screenshot` renderer has no font (nothing font- or
  text-related in `integrations/petal-desktop-sdl/src/screenshot.rs`), so
  `draw_text` is a no-op there — note it so people don't think text drawing is
  broken.

> The `noise()` arity nit is closed — [Builtins.md](../Builtins.md) documents
> all three arities.

### Design philosophy (creative coding)

Low floor, high ceiling · math as prose · visible by default · forgiving types
(truncate a float where an int is wanted; unpack a color record) · built-in
vocabulary for the domain. Lessons drawn from Processing/p5.js (`map`,
`push`/`pop`, `colorMode`, Perlin `noise`), GLSL/Shadertoy (`smoothstep`,
`fract`, `mix`, component-wise vector math), Sonic Pi (domain vocabulary, rings,
live reload), Nannou (good defaults over verbose types), and Scratch (remove
complexity, immediate feedback).
