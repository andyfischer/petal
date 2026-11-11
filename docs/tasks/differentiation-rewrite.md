# Differentiation rewrite: drop dual numbers, solve by approximation

Status: proposed, 2026-11-10

## Summary

Remove dual numbers from the language: the `Value::Dual` value kind, the
`dual` type, the `autodiff` functions `dual`, `value_of` and `deriv_of`, and
the dual-propagation rules in the arithmetic ops and the math natives. Every
"how does this output move when that input moves" question is then answered
the way direct manipulation v2 already answers it: by re-running the code with
an input nudged and measuring (finite differences), then taking damped
Gauss-Newton steps toward a goal.

Two things replace what duals did:

1. **For scripts:** the `autodiff` package keeps its name and gets new
   contents, written in Petal: `deriv`, `jacobian`, `solve_step` and
   `solve`. Every probe runs inside `speculate`, so the function being
   differentiated may touch `state` or emit without anything leaking. It
   reuses the Gauss-Newton kernel that `ui/drag`'s `vary` already calls
   (`drag_solve_step`).
2. **For editing code by its output:** finish the job DM v2 started, with an
   *edit mode* in every host that draws, not only in Garden. In an IDE the
   author selects the lines whose terms may change, then drags a drawn item
   and the solver rewrites literals on those lines. Outside an IDE the solver
   works **best effort**: `tunable` terms first, never `const(…)` or `rigid`
   terms, an equal split across whatever is still tied, and an error when no
   edit can reach the goal.
3. **For everyone using `speculate`:** a fast path for calls that touch
   nothing `speculate` would have to put back. Today a speculative call of a
   small pure function costs 2.5× a plain call (measured below), which
   `deriv` would pay on every probe.

**Long-term direction (decided 2026-11-10):** exact differentiation will come
back later as a symbolic analysis of the AST or term graph, not as a runtime
value kind. Nothing in this plan should make that harder; see
[Compatibility](#compatibility).

**Recommendation: adopt.** The findings that drive it:

- **Nothing that edits code uses duals.** DM v2 measures sensitivities by
  finite differences (`core/src/dm/jacobian.rs:1-25`), and its one dual hook,
  `OverrideValue::Dual` (`core/src/dm/spec.rs:132`), is never constructed
  anywhere in the tree. Drag-to-edit in Garden, the CLI, MCP, the C bridge
  and `petal-ui-propose-edit` is already dual-free.
- **Duals are barely used, and cost a lot of surface.** In the Petal corpus,
  across this repo and every project in `CLAUDE.local.md`, they appear in
  four places. Two are tests and demos of duals themselves. In the
  implementation they reach 43 files of `core/src` (266 lines), plus the type
  checker, the FFI docs, the C bridge and petal-ui.
- **The finite-difference path is already the default and the better tested
  one.** `ui/drag`'s `vary` defaults to `sensitivity: "fd"`. When the opt-in
  `"dual"` mode is used, it is checked against differences on the first frame
  and replaced by them if they disagree (`core-libs/petal-ui/prelude/drag.ptl:790-806`).

**The strongest argument against:** duals give the exact derivative, and
finite differences give an approximation whose quality depends on the step
size. Until symbolic differentiation lands, a user who wants the true
derivative of `sin(x) * x` at a point cannot get it to full precision. This
plan judges that acceptable for a creative-coding language whose consumers of
derivatives are solvers that re-verify every step anyway (DM v2 re-runs to
verify, and `vary` re-renders). The cost is visible in the
[Assessment](#assessment).

---

## Motivation

### Where duals are used today

Petal source (`.ptl`), searched in this repo, `~/petal-creative`,
`~/cheesecake`, `~/worlds-fair/ui/ptl` and `~/biz/petal-lang.org`:

| Site | What it does |
|---|---|
| `core/packages/autodiff/autodiff.ptl` | The package itself: three `native fn` declarations |
| `examples/console/differentiation.ptl` | Teaching example: derivative check, then gradient descent on `(x - 3)^2` |
| `test/lerp-trig-random/main.ptl:12` | One line: `deriv_of(atan(dual(1.0, 1.0)))` |
| `core-libs/petal-ui/prelude/drag.ptl` | `vary`'s opt-in `sensitivity: "dual"` (31 lines: `_dual_run`, `_undual`, `_dual_center`, `_jac_agrees`) |
| `examples/ui/d2-dial/app.ptl:33` | The one user of `sensitivity: "dual"` |
| `~/biz/petal-lang.org/frontend/public/petal/examples/aim.ptl` | Website demo: Newton steps through a 70-step ballistic simulation, two dual passes per frame |

`~/cheesecake/third_party/petal` vendors `autodiff.ptl` and `drag.ptl`
verbatim. No game script, petal-creative document or worlds-fair UI uses
them. `docs/proposals/value-representation.md:131` says autodiff "appears 31
times in `examples/`". That is stale: today it is the two example files above.

Tests: `test/vitest/autodiff.test.ts`, the dual cases in
`core/src/backend/bytecode/vm/fast.rs:805-830`, the dual cases in
`core/src/env/tests.rs`, and the dual row in the C bridge's value view
(`integrations/petal-c-bridge/src/view.rs:427`).

### What duals cost in the implementation

`grep -i dual core/src` matches 266 lines in 43 files. The main pieces:

- **Value and heap:** `Value::Dual(DualId)` (`core/src/value.rs:58-61`, plus
  type name, display, equality, `num_f64` and truthiness). Duals share the
  pair slab with `vec2` (`core/src/heap.rs:899-1016, 1779-1806`).
- **Arithmetic:** `dual_arith` and dual negation (`core/src/backend/ops.rs:70, 107-137, 430`).
- **Math natives:** every differentiable unary and the numeric helpers carry
  a dual path. That is 31 lines in `builtins/math.rs`, 39 in
  `builtins/creative_coding.rs` (`clamp`, `lerp`, `remap` and the rest, via
  `args_d` and `push_d`), and 7 in `builtins/vec2.rs`.
- **Type checker:** `Type::Dual` and the "Float -> Float, Dual -> Dual"
  propagation rules (`core/src/types.rs:200, 315`,
  `core/src/typecheck/builtin_types.rs:13-90`).
- **Elsewhere:** state storage, memo, static cache, record templates,
  `drop_edit` (`"dual"` among the unspellable kinds) and stats.
- **Embedding surface:** `Value::Dual`, `heap.get_dual` / `dual_value` and
  `PetalCxt::push_dual` are in the FFI docs (`docs/usage/ffi.md:276-279, 484-494`).
  Every native that takes a number has to decide what to do with a dual
  (`core-libs/petal-ui/src/input.rs:776` reads the primal).

Each new numeric native has to choose a dual rule. Each new analysis has to
handle one more value kind.

### What already exists on the approximation side

- **DM v2** (`core/src/dm/`, about 7,000 lines, shipped 2026-10-24, see
  [dev/direct-manipulation-v2-plan.md](../dev/direct-manipulation-v2-plan.md)):
  - Speculative re-runs with literal overrides (`dm::spec`).
  - A finite-difference Jacobian with each column classified as smooth, kink,
    branch flip, step, discrete, zero or errored (`dm::jacobian`).
  - A constrained, damped Gauss-Newton solver with verification and rounding
    to nice literals (`dm::solve`).
  - Ranking of candidate edits (`dm::rank`), and the `Lever` trait that lets
    the same solver move source literals or `ui/drag` state paths (`dm::lever`).
- **Surfaces on DM v2:**
  - Garden drag mode (`garden/garden-script/src/panel/dm.rs`).
  - `petal propose-edit`, and MCP `ProposeEdit` / `TraceEmits`.
  - `pb_vm_propose_edit` in the C bridge (`integrations/petal-c-bridge/src/vm.rs:1094-1119`).
  - `petal-ui-propose-edit` in `core-libs/petal-ui-headless`.
- **`drag_solve_step`** (`core/src/builtins/drag_math.rs:249-262`): one
  Gauss-Newton step exposed to Petal. `ui/drag`'s `vary` uses it. A native
  cannot call a Petal closure (`drag_math.rs:2-4`), so the re-runs happen in
  Petal and only numbers cross into the native.
- **Algebraic inversion** (DM v1, `core/src/direct_manipulation.rs`) for
  `+ - * /` chains. It is exact, needs no iteration, and is v2's fallback for
  non-numeric goals.

### What is missing for "drag a drawn item, edit the code"

Drag-to-edit ships in Garden only. The hosts where people actually draw
things do not offer it:

| Host | Emit tracing | Drag-to-edit of source |
|---|---|---|
| Garden | yes | yes (drag mode, v2) |
| `petal-sdl` (`integrations/petal-desktop-sdl`) | yes, for timeline trails (`game_loop.rs:933`, `timeline.rs:957`) | no |
| `petal-web-canvas`, `diagram-canvas` | no | no |
| petal-creative editors | n/a | own literal edits. A computed layer is read-only ("the editor can only edit layers while `layers` … is plain literal elements", `petal-images/docs/document-format.md:55-58`) |
| C bridge | yes | API only (`pb_vm_propose_edit`), no reference UI |

---

## Design

### 1. Removed from the language

| Removed | Replacement |
|---|---|
| `dual(v, d)`, `value_of(x)`, `deriv_of(x)` (in `import autodiff`) | `deriv`, `jacobian`, `solve_step`, `solve`, in the same `import autodiff` (below) |
| The `dual` value kind, `type(x) == "dual"`, the `dual` type annotation | none |
| Dual propagation through `+ - * /`, negation, the float math natives, `clamp`, `lerp`, `remap`, the vec2 natives | none. These natives take plain numbers only. |
| `vary(..., {sensitivity: "dual"})` in `ui/drag` | The option is removed. `"fd"` is the only mode, and the key is accepted and ignored for one release, with a lint warning. |
| `Value::Dual`, `DualId`, `heap.get_dual`, `heap.dual_value`, `PetalCxt::push_dual`, `SpecOverride::Dual`, `OverrideValue::Dual` | none |

**Error messages after removal.** `import autodiff` keeps working. A call
to a removed name fails at compile time with a pointer to the replacement:

```
error: `dual` was removed from `autodiff` (dual numbers are gone).
  Use `deriv(f, x)` for a derivative, or `solve_step(f, xs, target)` to move
  inputs toward a goal. See docs/features/differentiation.md.
```

`value_of` and `deriv_of` get the same message.

### 2. The new `autodiff` package

The package keeps its name and its place (`core/packages/autodiff/`), but
its contents become Petal code instead of three `native fn` declarations.
Every function takes a Petal function and calls it. That is why it is Petal
code and not natives (`drag_math.rs:2-4`).

```petal ignore
import autodiff

// The derivative of a one-number function at x, by central differences.
// h defaults to 1e-4 * max(|x|, 1).
// Returns {value, quality, step}.
deriv(f, x, opts = {})                 -> record

// The Jacobian of f: list of numbers -> number or list of numbers, at xs.
// One row per output, one column per input. Central differences by default.
// Returns {rows, quality, step}: rows[i][j] = d out_i / d in_j, and one
// quality and one step per column (per input).
jacobian(f, xs, opts = {})             -> record

// One damped Gauss-Newton step from xs toward f(xs) == target.
// Returns {xs, residual, improved}. opts: bounds ([[lo, hi], …]), max_step,
// damping, mode ("central" | "forward").
solve_step(f, xs, target, opts = {})   -> record

// solve_step repeated until the residual is under tol or max_iter is spent.
solve(f, xs, target, opts = {})        -> record
```

**Quality** is the column classification DM v2 already computes
(`dm/jacobian.rs:9-21`), as a string:

| `quality` | Meaning |
|---|---|
| `"smooth"` | Same path both ways, and the one-sided slopes agree |
| `"kink"` | Same path, slopes that disagree (`abs`, `min`, `clamp`); `value` is the central slope |
| `"branch_flip"` | One side took another branch; `value` is the one-sided slope from the side that didn't, after trying a smaller step |
| `"step"` | Nothing moved at the base step but something did at a larger one (`round`, `floor`); `value` is a secant |
| `"zero"` | Nothing moved at any scale |
| `"errored"` | A probe failed; the record also has `error` |

`step` is the `h` the value was measured with, after any escalation.

**Semantics:**

- **Every probe runs inside `speculate`.** `f` may read and write `state`,
  emit draw commands or call effectful natives: each probe's changes are put
  back when it returns, and escaping effects are not called
  ([speculate.md](../features/speculate.md#what-is-put-back)). This is what
  makes it safe to differentiate a whole view or simulation step, not just
  an arithmetic helper. `speculate` refuses nesting today
  ([speculate.md, Rules](../features/speculate.md#rules): "speculate() cannot
  call itself"), so calling `deriv` from
  code that is already speculating (for example, inside a `ui/drag`
  candidate rendering) must either run the probes without a second
  `speculate`, since the outer one already isolates them, or be an error.
  The plan takes the first: `speculating()` true means call `f` directly.
- **Ints are probed as floats.** DM's int-literal discrete probing
  (`jacobian.rs`, `Discrete`) is for source literals, not script values.
- **Inputs that don't move the output.** A zero column (nothing moved) is
  reported in `solve_step`'s result as `stuck: [i, …]` and not divided by.
- **`solve_step` and `solve` return the quality too:** `{xs, residual,
  improved, stuck, quality}`, where `quality` is the Jacobian's per-input
  list, so a caller can tell a stuck input (`"zero"`) from one that jumped
  a branch.
- **The step math is shared.** `solve_step` calls `drag_solve_step`, renamed
  to the neutral `gn_step` (with `drag_solve_step` kept as an alias for one
  release), so `vary`, the script API and DM's `lever_step` share one kernel.

### 3. An edit mode in every drawing host

There is no new language feature here. This part extends DM v2's existing
`DmSession` API (`core/src/dm/api/mod.rs`) to the hosts in the table above.

**Edit mode is a separate mode, not a gesture.** Editing by output is more
than dragging: a drag is usually ambiguous, and the author has to say which
terms the solver may change. The mode has two halves:

- **Select what may change.** The author selects lines of source (in the
  host's source view, or by clicking a drawn item to jump to the call that
  drew it and widening from there). The selection becomes the request's
  **allow** set, as `line:N` selectors. DM v2 already accepts these
  ([direct-manipulation.md, "Telling Petal which numbers may move"](../features/direct-manipulation.md#telling-petal-which-numbers-may-move)),
  along with `fn:name`, variable names and `at:file:line:col`. The
  selection lives for the session only and is never written into the source
  (decision 8). With nothing selected, the solver uses the best-effort
  policy below.
- **Drag to set the goal.** Dragging a drawn item sets the goal on its
  emitted geometry. The solver proposes edits to literals on the selected
  lines only, the host shows the alternatives, and the author picks one.

**Outside an IDE: best effort.** A host with no source view (petal-sdl, the
petal-creative editors, a C/C++ embedder, the web canvas without its editor)
has no way to select lines. It still offers edit mode, and the solver picks
the terms itself with a fixed policy, `policy: "best_effort"` on the DM
request:

1. **Knobs first.** If any `tunable` (or setting) literal can move the goal,
   only those move. DM v2 already ranks knobs first (`dm/rank.rs`,
   `KNOB_WEIGHT`); best effort makes it a hard filter instead of a
   preference.
2. **Never `const` or `rigid`.** With no knob that reaches the goal, every
   eligible literal may move except one marked `rigid(…)` (already excluded,
   `dm/params.rs:339`) or one inside a `const(…)` initializer. The second is
   new: the compiler already sets `is_const` on such a binding's term (read
   by `static_classify/mod.rs:3006`), and screening has to exclude every
   literal that reaches the goal only through one.
3. **Equal split when still ambiguous.** If more than one literal remains in
   the best weight tier (DM's `param_weight`: same tier, same
   `through_var` / `through_state` / clock penalties), the goal's change is
   split equally: with `N` tied literals each is moved by the amount that
   produces `1/N` of the output's change on its own (from its Jacobian
   column; decision 9), then the result is verified by a re-run as usual. This is a new
   blend mode beside DM's default weighted least-squares spread, which
   moves the most sensitive literal most. An equal split is easier to
   predict for an author who didn't choose the terms.
4. **Error when unsalvageable.** If no literal is eligible, every candidate
   column is `"zero"` or `"errored"`, or the verified result misses the goal
   by more than the request's tolerance, the request fails with the reason
   DM already reports for rejections (`dm/api/rejection.rs`). The host shows
   it, and nothing is written.

The same policy applies to an IDE drag with nothing selected, so there is
one behaviour to explain: "with a selection, only those lines; without one,
best effort".

While edit mode is on, the program is frozen (inputs and `state` held), so
game input never reaches the script and the speculative runner has a stable
baseline. Garden's drag mode (`:DragMode`) is the reference behaviour; this
work moves the parts that aren't Garden-specific (selection to allow set,
the alternatives list, apply and reload) into a shared module that each host
draws its own way.

In order:

1. **`petal-sdl`:**
   - A key toggles edit mode (F-key, alongside the timeline's F5/F7/F8).
     Entering it freezes the timeline at the current frame.
   - No source pane: a drag proposes edits through `propose_edits_v2` with
     `policy: "best_effort"`. The top result is applied, the file is
     written, and the existing hot reload picks it up. A failure is shown
     as a message in the overlay, with nothing written.
   - Clicking a drawn item names the call that drew it in the overlay (emit
     tracing is already on for the timeline), so the author knows which
     line will change.
   - The timeline is frozen during the drag, so it reuses the frozen-input
     speculative runner.
   - Show the applied edit (and an undo key) in the existing overlay used
     by F7 trails.
2. **`petal-web-canvas`:**
   - The same mode. Line selection happens in the page's source editor
     (`diagram-canvas` has one), and the edit goes back to it.
   - Needs emit tracing turned on in the wasm host.
3. **petal-creative editors:**
   - When a layer is computed (not a plain literal element), the editor
     today shows it read-only. Instead, a drag on it becomes a best-effort
     goal on the layer's emitted `x`/`y`, solved against the literals in the
     helper or loop that computed it. The editors' undo/redo already works on
     the source text (`creative-core`'s `source` module), so a bad guess is
     one undo away.
   - This is the "programs as documents" case: a document with a `for` loop
     of cards becomes draggable without being flattened to literals.
4. **C bridge:** a reference integration in `integrations/petal-c-bridge/tests`
   showing a best-effort drag end to end. The API exists; it gains the
   `policy` field.

### 4. A fast path for `speculate`

**Measured today** (release build, `petal bench`, a two-line function
`(x - 3.0) * (x - 3.0) + sin(x)` called 500 times each way with a varying
argument):

| | ms/call (opt) | ms/call (no-opt) | allocations/call |
|---|---|---|---|
| `f(x)` | 0.000285 | 0.000292 | 0 |
| `speculate(f, x).value` | 0.000714 | 0.000790 | 3 |

So about 0.43 µs and 3 allocations of fixed cost per call, which is 2.5× the
call itself here. The existing benchmark
(`test/benchmarks/micro/speculate.ptl`, a ten-card view) measured 1.7 µs on
top of a 1.7 µs view. Most of the cost is fixed, paid whether or not the
call touches anything (`vm/speculate.rs:237-333`):

- **On entry:** each output buffer's length and origins, a clone of the
  counters, the random stream, the noise seed, the print buffer's length,
  the memo, echo, trace and observation flags, and the heap's speculation
  epoch.
- **On exit:** emits are cut off and collected per channel, everything is
  restored, and the `{value, emits, places}` record is built (two lists and
  a record: the 3 allocations).

There are two complementary fixes. Both benefit every `speculate` caller
(`ui/drag`, `ui_measure`, hover previews), not just `autodiff`.

**A. Pay only for what the call touches (no analysis needed).**

- Save lazily. The state slots and cells are already logged on first
  write. Do the same for the counters, random stream, noise seed and
  buffers: the writers check `speculating()` and log the old value once.
  An untouched call then saves nothing.
- Build the result lazily. With no emits and no places, `emits` and
  `places` share one frozen empty list, so no allocation. Add an internal
  entry, `speculate_value(f, …)`, that returns only the value. `autodiff`
  and `ui_measure` use it, and the record is never built.
- This is semantically identical by construction, since nothing a call
  doesn't touch needs restoring. It is tested by the existing
  `core/tests/all/speculate.rs` suite plus a counting test (an untouched
  call logs nothing).

**B. Skip `speculate` entirely for functions known to touch nothing.**

- Compute a per-function **effect-free** flag at compile time. The body
  must have:
  - no `StateWrite` / `StateInit`, no `set` on a cell it didn't create, and
    no emitting native;
  - only builtins marked `PURE` in `builtins/effects.rs` (the table
    `is_pure_builtin` reads), plus `random` and the noise functions, which
    `speculate` already restores and which can be handled by A;
  - calls only to functions that are themselves effect-free, solved as a
    fixpoint over the call graph. An unknown callee (a closure passed in)
    means not effect-free.
- `speculate` on an effect-free function calls it directly. `deriv`'s
  probes of an arithmetic helper then cost exactly a plain call.
- This differs from the static-value classifier's **static-pure**
  (`static_classify/mod.rs`), which also requires every term to be a
  function of the source text. A function that reads `random()` or its
  arguments freely is effect-free but not static-pure.
- Risk: a wrong flag would let a call leak a write. So the flag must be
  conservative, and in debug builds `speculate` should still run the log on
  flagged calls and assert that it stayed empty.

**Order:** only A is in this plan. It needs no analysis, can't be wrong in
a new way, and removes most of the fixed cost. B is deferred (decision 10):
it adds a compiler analysis for a gain that is small once A lands. The native effect annotations proposal
(`docs/proposals/native-effect-annotations.md`) would make B's builtin
table come from declarations rather than a hand-kept list.

---

## Before and after

All "before" snippets were run on 2026-11-10 with `cli/target/release/petal`.
`differentiation.ptl` and `test/lerp-trig-random/main.ptl` print what they
claim, and `aim.ptl` passes `petal lint` (three style warnings, no errors).
`d2-dial` needs a petal-ui host and was not run. "After" snippets are
**proposed** and do not run yet.

### 1. The teaching example: `examples/console/differentiation.ptl`

Before:

```petal ignore
import autodiff

fn f(x)
  (x - 3.0) * (x - 3.0)
end

let x0 = dual(5.0, 1.0)
let y0 = f(x0)
print("f(5)  = {value_of(y0)}")
print("f'(5) = {deriv_of(y0)}   (expected 4.0)")

var x = 10.0
for step in range(0, 12) do
  let yd = f(dual(x, 1.0))
  x = x - 0.1 * deriv_of(yd)
end
```

After (proposed):

```petal ignore
import autodiff

fn f(x)
  (x - 3.0) * (x - 3.0)
end

let d = deriv(f, 5.0)
print("f(5)  = {f(5.0)}")
print("f'(5) = {round(d.value, 6)}   (expected 4.0, {d.quality})")

var x = 10.0
for step in range(0, 12) do
  x = x - 0.1 * deriv(f, x).value
end
```

The writer no longer seeds a derivative, unwraps with `value_of`, or keeps
in mind that `f` returned a dual. One new idea replaces three. The printed
derivative is approximate: a central difference with `h = 0.0005` prints
`3.999999999998227` today, not `4.0`. A teaching example should say so out
loud, and print rounded.

### 2. The showcase demo: `aim.ptl` (website)

Before (`aim.ptl:61-77`, trimmed):

```petal
let wa = at(dual(angle, 1.0), dual(speed, 0.0), k)
let wv = at(dual(angle, 0.0), dual(speed, 1.0), k)
let ax = deriv_of(wa.x)
let ay = deriv_of(wa.y)
let sx = deriv_of(wv.x)
let sy = deriv_of(wv.y)
let rx = target.x - p.x
let ry = target.y - p.y
let det = ax * sy - sx * ay
if abs(det) > 0.000001 then
  let step_a = (rx * sy - ry * sx) / det
  let step_v = (ax * ry - ay * rx) / det
  angle = clamp(angle + clamp(step_a * 0.25, -0.06, 0.06), 0.05, 1.5)
  speed = clamp(speed + clamp(step_v * 0.25, -30.0, 30.0), 80.0, 900.0)
end
```

After (proposed):

```petal
let hit = fn(xs)
  let p = at(xs[0], xs[1], k)
  [p.x, p.y]
end
let r = solve_step(
  hit,
  [angle, speed],
  [target.x, target.y],
  {bounds: [[0.05, 1.5], [80.0, 900.0]], max_step: [0.06, 30.0], damping: 0.25},
)
angle = r.xs[0]
speed = r.xs[1]
```

The demo's point changes from "Petal differentiates through the loop" to
"Petal solves for the inputs that hit the goal", which is the
outcome-based framing the README now uses. The hand-written 2×2 solve,
`det` guard and clamps go away. Cost: central differences take four runs
of `flight` per frame (two per input) against the two dual passes today, or
two with `mode: "forward"`, the same as today. The comment at
`aim.ptl:5-7` must be rewritten.

### 3. `ui/drag` `vary`: `examples/ui/d2-dial/app.ptl:33`

Before:

```petal
draggable("knob", k, fn() vary(dial, ["angle"], {sensitivity: "dual"}) end)
```

After (proposed):

```petal
draggable("knob", k, fn() vary(dial, ["angle"]) end)
```

Under `"fd"` the dial renders once more per path per frame, and `"dual"` also
rendered once per path, so the count is unchanged (`drag.md:273`). The 31
lines of dual handling in `drag.ptl` go away, and with them a whole class of
bug: a view whose arithmetic drops the dual on the way to `place`
(`drag.md:392-396`), which today is caught only by the first-frame
agreement check.

### 4. Where the old form was better: an exact derivative

Before:

```petal ignore
import autodiff
print(deriv_of(atan(dual(1.0, 1.0))))   // 0.5, exactly
```

After (proposed):

```petal
import autodiff
print(deriv(atan, 1.0).value)           // 0.5000000008337224 (central difference, h = 1e-4, computed today by hand)
```

For checking a derivative formula, which is what this test line does, duals
are strictly better: exact, one pass, no step size to pick. The test becomes
an approximate comparison. Until symbolic differentiation lands, anyone who
needs the exact derivative of a closed form writes it by hand.

### 5. Interaction with `state` and hot reload

Today a dual can be stored in a `state` slot (`env/state_storage.rs` has
dual cases), where it persists across frames and reloads with whatever
derivative it was seeded with. No program in the corpus does this. `solve_step`
returns plain floats, so whatever is stored in `state` (as in `aim.ptl`'s
`state angle` / `state speed`) is always a plain number. A hot reload that
changes `f` simply changes what the next probe measures.

### Migration

| Code | Migration |
|---|---|
| `dual` / `deriv_of` for one input | Mechanical in the common shape (`deriv_of(f(dual(x, 1.0)))` → `deriv(f, x).value`); a `petal lint --fix` rule can do it when `f` is a named function. The `import autodiff` line stays |
| Several duals seeded by hand (`aim.ptl`) | Manual: rewrite as `jacobian` or `solve_step` |
| `sensitivity: "dual"` | Mechanical: delete the key (`lint --fix`) |
| Rust natives that handle `Value::Dual` | Manual: delete the arm. Compile errors find every one |
| Vendored copies (`~/cheesecake/third_party/petal`) | Re-vendor. Cheesecake game scripts need no change |
| Website (`~/biz/petal-lang.org`) | `aim.ptl` rewritten; `content/docs/features/differentiation.md`, `Builtins.md`, `packages.md`, `language-guide.md`, `ffi.md`, `CLI.md` re-imported from this repo |

---

## Assessment

Findings first, then opinion, in each subsection.

### Project goals

- **Findings:**
  - The change advances "outcome-based coding" and "goal-based program
    manipulation" directly only through part 3 (drag-to-edit in more hosts).
    Parts 1 and 2 are simplification.
  - It fits "easy to use": one concept (solve for inputs) replaces two (a
    special number kind, then a solver written by hand around it).
  - It works against nothing in the README. It does remove the literal basis
    for the README's "Differentiable programming" section, which already
    over-describes what Petal does (it speaks of backpropagation, and Petal
    has neither reverse mode nor `grad()`, per
    `docs/aspirational-ideas/README.md:22`).
- **Opinion:** Petal should be distinctive at "drag the output, get a code
  edit", not at "has a dual number type". Many languages have AD libraries,
  but few have DM v2.

### Ease of writing

- **Findings:**
  - A newcomer learns `deriv(f, x)` and `solve_step(f, xs, target)`, and
    that results are records (`.value`, `.quality`). Someone
    coming from Python would recognise the shape from `scipy.optimize`.
    Duals ask the reader to see that `f` returns a different kind of value
    depending on its argument.
  - After the change there is one way to get a derivative. Today there are
    two (duals, or differences written by hand in `drag.ptl`).
- **Risk, not measured:** users may pass an `f` with a discontinuity and get
  a huge derivative. The `quality` field is the answer: a `"branch_flip"` or
  `"step"` tells the caller not to trust the number as a slope. The cost is
  that the common case reads `deriv(f, x).value` instead of `deriv(f, x)`.

### Bug-freedom

- **Prevents:**
  - A dual silently dropped by a native that reads only the primal
    (`petal-ui/src/input.rs:776`), giving a zero derivative with no error.
  - Duals leaking into `state` or into drawn output (`drag.ptl`'s `_undual`
    exists for this).
  - Dual-unaware natives added in future.
- **Newly possible:**
  - A bad step size. A derivative that is wrong near a kink or a branch.
    `jacobian.rs`'s classification handles these for DM, and the `solve`
    package should reuse its step-escalation rules, not reinvent them.
- **Testing:** the differential oracle (optimizer on vs `--no-opt`) and the
  golden corpus have no dual-dependent goldens (`test/example-golden` has no
  match). Removing a value kind shrinks the small-scope check's input space.
  The Kani proofs need a read for heap pair-slab invariants, since only
  `vec2` remains in that slab.

### Performance

- **Findings:**
  - On the hot path, dual checks sit in the slow path already. The fast VM
    hands duals off (`vm/fast.rs:586, 805`), so removing them saves branch
    arms but no measurable fast-path time. Expect no change in
    `tools/bench-opts.ts`.
  - Script solving costs: central differences take 2 runs per input (forward
    takes 1) against 1 dual pass per input. `aim.ptl` goes from 2 to 4
    `flight` runs per frame with defaults, or stays at 2 with
    `mode: "forward"`.
  - Each probe also pays for `speculate`: about 0.43 µs and 3 allocations
    per call today, 2.5× the cost of a two-line pure function
    ([Design §4](#4-a-fast-path-for-speculate)). For `aim.ptl`'s `flight`
    (70 steps) that is noise. For `deriv` of a small helper it dominates,
    which is why the fast path comes first.
  - `vary` is unchanged in run count.
- **Benchmark to run before and after:** `petal bench` on `aim.ptl` (wrapped
  for a headless host), and `test/ui-corpus/tests/dm_study.rs`, whose
  outcomes and run counts must not change, since DM never used duals.

### Live coding and hot reload

No effect on reload equivalence (`docs/features/hot-reload.md`): no `state`
slot shapes or call paths change. A `state` slot that holds a dual today
(none in the corpus) would fail to restore after the change. It should
restore as its primal, with a warning, for one release.

### Introspection, debugging and program manipulation

- **Provenance, `explain`, slicing and time travel:** unaffected, or simpler
  with one value kind fewer.
- **DM v2, goal-based editing, source preservation and `propose-edit`:**
  unaffected, since none use duals.
- **`drop_edit`:** loses one unspellable kind.
- **"Autodiff" analyses:** the skill's checklist names autodiff as an
  analysis to preserve. After this change the analysis *is* DM's
  finite-difference Jacobian, so the checklist item should be reworded.

### Maintainability of the implementation

- **Stages touched:**
  - Value, heap, ops, about 10 natives, the type checker, state storage,
    memo, static cache, stats, and the C bridge view (deletions).
  - `ui/drag` prelude (deletion).
  - New Petal code in `core/packages/autodiff` (about 150 lines) and the
    `gn_step` rename.
  - Docs: `features/differentiation.md` is rewritten around the new functions;
    `Builtins.md`, `packages.md`, `language-guide.md`, `ffi.md`,
    `syntax/overview.md` and `Architecture.md` are updated.
  - Part 3 adds host UI code in petal-sdl, web-canvas and petal-creative.
- **Special cases removed:** every numeric native's dual rule, and `Type::Dual`.
  No new invariants are added.
- **`PRELUDE_LEVEL`:** bumps (the prelude's `drag.ptl` changes).
- **FFI:** `Value` loses a variant. That is a breaking change for Rust
  embedders that match exhaustively, so it goes in the changelog under
  **Embedders**.

### Domains

| Domain | Effect |
|---|---|
| Graphics, creative coding | Helps: drag-to-edit in petal-sdl and web-canvas. `to_glsl` never handled duals. |
| Games | Neutral for scripts (no game uses duals). Helps tuning: drag a sprite in petal-sdl to edit its spawn constants. |
| Productivity and UI | Neutral; `vary` keeps working. |
| Dashboards and data | Neutral. |
| Web | Helps (part 3, step 2). |
| Editor scripting and config | Helps: petal-creative computed layers become editable. |
| Embedding hosts | Small break (`Value::Dual`); reference drag integration added. |

### Compatibility

- **Breaks:** two example files, one test line, one `ui/drag` option and one
  website demo. No downstream project scripts break, but Cheesecake needs a
  re-vendor.
- **Ships in two steps:** a release where `autodiff` still works but warns
  (`[autodiff-deprecated]`, with `lint --fix` for the mechanical shape),
  then removal.
- **Long-term direction:** exact differentiation comes back later as a
  symbolic analysis of the AST or term graph, not as a runtime value kind.
  That is the route that also gives reverse mode and scales past DM v2's
  cap of 32 candidate parameters (`dm/solve.rs`, `k ≤ 32`). This plan keeps
  it open in two ways:
  - The `autodiff` API takes functions, not seeded values. A later symbolic
    `deriv` can answer from the AST when it can differentiate `f` and fall
    back to differences when it can't, with no change to callers.
  - `quality` gains a value for that case (`"exact"`), so callers already
    branch on it.

### Prior art

- **Bret Victor's demos, Sketch-n-Sketch, and the *Declarative Dragging*
  paper's Dragology:** all solve output-to-input by re-evaluation or
  local inversion, not by AD.
- **Spreadsheet goal seek and `scipy.optimize`:** default to finite
  differences, with the Jacobian as an optional input.
- **JAX and PyTorch:** AD-first, because they optimise thousands to billions
  of parameters. That is the scale argument above, and not Petal's use case.
- **Swift for TensorFlow:** built differentiation into the language and
  compiler, and the project was archived in 2021. Language-level AD is a
  large, long-lived commitment.

### Alternatives considered

1. **Do nothing.** It keeps exact derivatives, and the price is every
   numeric native carrying dual rules forever, for four call sites.
2. **Move duals out of the core into a library type** (a record
   `{v, d}` with operator methods). This keeps exactness for those who want
   it, but loses propagation through natives like `sin`, which is the point
   of duals. Rejected as half a feature.
3. **Keep duals and only build part 3.** Part 3 doesn't depend on parts 1
   and 2. This is the smallest version that delivers the user-visible
   benefit. If the removal is contentious, build part 3 first, which is the
   order proposed below anyway.
4. **Replace duals with symbolic differentiation now** (differentiate the
   AST or IR of `f`). That is exact, has no runtime value kind, and is the
   decided long-term direction. It is far more work than four call sites
   justify today, and finite differences are needed anyway as the fallback
   for code a symbolic pass can't handle (natives, loops with data-dependent
   trip counts), so they come first.

---

## Implementation plan

Each phase lands separately, with its tests green, in this order:

### Phase 0: `speculate` fast path (benefits every caller)

- A: lazy saves on entry (counters, random stream, noise seed, buffers
  logged on first write), a shared frozen empty list for empty
  `emits`/`places`, and an internal `speculate_value` entry.
- Tests: `core/tests/all/speculate.rs` unchanged and green; a new test that
  an untouched call logs nothing; the differential oracle.
- Measure: the benchmark above, and `test/benchmarks/micro/speculate.ptl`,
  before and after. Record both in `docs/features/speculate.md`,
  "Performance".
- B (the effect-free flag) is deferred and not part of this plan.

### Phase 1: the new `autodiff` functions (additive)

- `core/packages/autodiff/autodiff.ptl`: add `deriv`, `jacobian`,
  `solve_step` and `solve` beside the existing natives. Probes run under
  `speculate`, or directly when `speculating()` is already true.
- Rename `drag_solve_step` → `gn_step` in `builtins/drag_math.rs`, and keep
  the old name as an alias.
- Expose the column-quality rules from `dm/jacobian.rs` (`smooth_or_kink`,
  the branch-flip retry and step escalation) through a native that takes
  the sampled values and returns `{value, quality, step}`, so the package
  and DM classify identically.
- Tests: `test/ptl/autodiff.ptl` (derivatives of polynomials, trig and
  `atan` within 1e-6 and `"smooth"`; `abs` at 0 is `"kink"`; a branch is
  `"branch_flip"`; `floor` is `"step"`; a 2×2 solve; bounds; a stuck input;
  an `f` that writes `state` and emits, leaving nothing behind; `deriv`
  called inside `speculate`).
- Measure the `speculate` overhead per probe (see Performance).
- Docs: rewrite `docs/features/differentiation.md` around the new
  functions, plus the `Builtins.md` section.

### Phase 2: migrate the call sites, deprecate the dual functions

- Rewrite `examples/console/differentiation.ptl`,
  `test/lerp-trig-random/main.ptl:12` and `examples/ui/d2-dial`.
- Remove `sensitivity: "dual"` from `ui/drag` (the key is ignored, with a
  warning), and bump `PRELUDE_LEVEL`.
- Rewrite the website's `aim.ptl`.
- Add the `[dual-deprecated]` warning on `dual`, `value_of` and `deriv_of`,
  and its `lint --fix` rule.
- Update the goldens for the rewritten examples.

### Phase 3a: the best-effort policy in DM v2 (core)

- `policy: "best_effort"` on the DM request (`dm/api/request.rs`), also
  reachable as `petal propose-edit --best-effort`, MCP `ProposeEdit`, and
  `pb_vm_propose_edit`.
- Knobs as a hard filter when any reaches the goal.
- Exclude literals that reach the goal only through a `const(…)` binding
  (read `is_const` during screening in `dm/params.rs`, next to the `rigid`
  exclusion at `:339`).
- The equal-split blend in `dm/solve.rs`, chosen when more than one literal
  ties in the best tier. It is verified like any other step.
- A single answer or an error: no list of alternatives.
- Tests in `core/tests/all/dm_solve.rs`:
  - a knob wins over a closer plain literal;
  - a `const(…)` literal is never moved;
  - two tied literals each carry half;
  - no eligible literal is an error naming why;
  - a goal out of reach is an error.
- Re-run `dm_study` with the policy on, and record how many of its 105
  goals best effort solves against the default ranking's top alternative.

### Phase 3b: edit mode in petal-sdl and web-canvas (parallel with 2)

- Extract the host-independent half of Garden's drag mode (selection to
  allow set, `DmSession` lifecycle, apply) into a shared crate or module,
  with Garden as its first user. Garden's tests
  (`garden/tools/dm-mode-integration-test.ts`, `garden-script/tests/dm_drag.rs`)
  must stay green.
- petal-sdl: an edit-mode key freezes the timeline. A drag runs a
  best-effort request, applies the result, writes the file, and hot reload
  follows; an error is shown and nothing is written. Test it with a
  scripted-input integration test.
- petal-web-canvas: turn on emit tracing in the wasm host, and add the same
  mode, with selection in the page's source editor. Test it in
  `diagram-canvas` with Playwright.
- Document both in `docs/features/direct-manipulation.md` under "Hosts".

### Phase 4: petal-creative computed layers

- In `creative-core`'s `source` module, when an edit targets a computed
  layer, run a best-effort DM v2 request with the layer's emitted geometry
  as the goal, and apply it as one undoable source edit.
- Start with petal-images, then petal-design.
- Track it in `~/petal-creative/docs/petal-upstream-ideas.md`.

### Phase 5: remove duals

- Delete `Value::Dual`, `DualId`, the pair-slab dual accessors, `dual_arith`,
  every native's dual arm, `Type::Dual`, `SpecOverride::Dual`,
  `OverrideValue::Dual`, the three `native fn` lines in `autodiff.ptl`,
  `builtins/autodiff.rs`, the dual cases in `test/vitest/autodiff.test.ts`
  (rewritten for the new functions), and the dual cases in `vm/fast.rs`
  tests and `env/tests.rs`.
- `dual`, `value_of` and `deriv_of` become the compile error described in
  the Design.
- Update `docs/usage/ffi.md`, `Architecture.md`, `CHANGELOG.md` (with an
  **Embedders** note) and the stale count in `proposals/value-representation.md`.
- Re-vendor in Cheesecake, and re-import the docs into `petal-lang.org`.
- Verify: the full test suite, `make expensive_lint_tests`, the
  differential oracle, `dm_study` (unchanged outcomes and run counts) and
  the Kani proofs.

### Phase 6: README

Replace the README's "Differentiable programming" section with the
"how goals get solved" framing (algebraic inversion, finite-difference
sensitivities with Gauss-Newton, speculative search). Petal will no longer
contain the word "dual".

---

## Decisions

Recorded 2026-11-10:

1. **Package name:** keep `autodiff`, with new contents.
2. **`speculate`:** every probe runs inside `speculate` (or directly when
   already speculating).
3. **Quality:** `deriv`, `jacobian`, `solve_step` and `solve` return the
   column quality and step along with the value.
4. **Host UI:** a separate edit mode, not a modifier-drag. Selecting lines
   to say which terms may change is part of the mode, because a drag alone
   is usually ambiguous.
5. **Long term:** exact differentiation returns as a symbolic analysis of
   the AST, not as a runtime value kind.
6. **Outside an IDE:** best effort. `tunable` first, never `const(…)` or
   `rigid`, an equal split across what is still tied, and an error when the
   goal can't be reached.
7. **`speculate` overhead:** fix it in `speculate` itself, for every caller,
   with a fast path for calls that touch nothing.
8. **IDE selection:** session only. It is never written into the source as
   `tunable` / `rigid` hints.
9. **Equal split:** by output share. Each of `N` tied literals moves by the
   amount that produces `1/N` of the output's change on its own.
10. **`speculate` fast path B** (the effect-free flag): not now. Phase 0
    builds only A. B stays recorded in [Design §4](#4-a-fast-path-for-speculate)
    in case a later measurement calls for it.

## Open questions

None at the moment.

## Progress

### Phase 0 (A): done 2026-11-11

- `vm/speculate.rs` logs the random stream, noise seed, print buffer,
  counters and output buffers on first write (`SpecSaves`, noted by the
  `PetalCxt` writers), not on entry; the record is reused between calls
  (`Stack::speculation_spare`), and the result records share one shape per
  kind. Empty `emits`/`places` are `Heap::shared_empty_list()` (immutable,
  so in-place edits copy).
- `speculate_value(f, ...args)` is a public `core` native beside
  `speculate` (same rules, returns only the value); `ui_measure` uses it.
- Tests: `core/tests/all/speculate.rs` unchanged and green; unit tests in
  `vm/speculate.rs` (an untouched call logs nothing, each touched thing is
  logged once, counters, `speculate_value`, the shared empty list);
  `tools/test-examples.ts` (opt vs `--no-opt`) green.
- Measured (median, release): the Design §4 function's `speculate` fell
  from 0.000417 to 0.000292 ms/call (3 → 1 allocations), `speculate_value`
  0.000250 ms (0 allocations), against 0.000167 for the plain call; the
  ten-card view's overhead fell from 1.7 µs to 0.55 µs. Details in
  [speculate.md, Performance](../features/speculate.md#performance); the new
  benchmark is `test/benchmarks/micro/speculate_pure.ptl`.

### Phase 3a: the best-effort policy in DM v2 (core) — done 2026-11-11

- `Policy::BestEffort` on `DmRequest` (`dm/api/request.rs`); `petal
  propose-edit --best-effort`; `"policy": "best_effort"` (or
  `"best_effort": true`) in a JSON request, so MCP `ProposeEdit` (`policy`
  input), `propose_json` and `pb_vm_propose_edit` all take it. The `--json`
  report gained `policy` and each candidate's `flags.through_const`.
- Knobs are a hard filter when their goal block has full row rank on the
  goals that ask for a change. `Param::through_const` (a `via::CONST` bit in
  the discovery walk, set past any term with `is_const`) and
  `EligibilityContext::exclude_const` exclude a literal reached only through
  a `const` binding, next to the `rigid` rule.
- The equal split is `solve::equal_share_step` (`Blend::EqualShare`):
  `goal_step` with each column's cost its goal-column norm and no
  collateral, which for one goal gives `J_i Δ_i = r / N` exactly
  (decision 9) and with several goals gives each literal its own goal's
  share. It runs through the ordinary verification loop. Two refinements
  were needed on the ladder: when ints or bounds round every share to
  nothing, the remainder goes to one literal at a time (least sensitive
  first); and when the cheapest tier's verified result misses, up to two
  dearer tiers that reach the goals alone are tried before the error
  (`BEST_EFFORT_SETS`). Without them best effort solved 82 of 105, then 90.
- Errors: `DmError::NothingToMove` (no eligible literal, with the reasons)
  and a new `DmError::Unreachable` (the cheapest try's description: what it
  set, how far it missed, what rejected it).
- Tests: the five cases in `core/tests/all/dm_solve.rs`
  (`best_effort_*`), plus `dm_embed`, `cli_propose_edit_v2`, the C bridge's
  `propose_tests` and `test/vitest/petal-mcp.test.ts`.
- `dm_study` with the policy on (`best_effort_vs_ranked`, ignored test; or
  `DM_STUDY_BEST_EFFORT=1`): of the 105 ladder goals, best effort solves 94
  (verified, and the hot-reloaded edit meets the goal) against 99 for the
  ranked policy's top alternative; 64 against 91 with the exact edit the
  ladder's `expect=` names; 11 against 1 end with no answer. The ladder's
  ranked outcomes are unchanged. Details in
  [direct-manipulation.md, "Best effort"](../features/direct-manipulation.md#best-effort).

### Phase 3b part 1: shared edit-mode module + petal-sdl edit mode — done 2026-11-11

- The host-independent half of Garden's drag mode is
  `petal_ui::edit_mode` (`core-libs/petal-ui/src/edit_mode.rs`, wasm-safe):
  the plain drag vocabulary (`DragFilter`, `DragSelector`, `DragCandidate`,
  `DragAlternative`, `DragOutcomeV2`, `SourceRewrite`, `CodeSpan`, …),
  selection to allow set (`DragFilter::allow_lines` / `allow_selection`,
  `line:N` selectors), the goal (`GoalTarget::at` from an emit trace,
  `native_position_args` by native name), the session lifecycle
  (`EditSession::open` / `preview` / `finish` / `cancel`, with
  `SessionOptions::best_effort()`), and apply (`apply_rewrites_to`,
  `DragAlternative::apply_to`). `Env::stack_program` became public for it.
- Garden is its first user: `garden-script/src/panel/dm.rs` keeps only the
  checkpoint, its `SpecHost` wrapper and the grabbed command;
  `panel_trace` re-exports the shared types under their old paths, so
  garden-app and garden-web are unchanged. `garden-script` tests (including
  `tests/dm_drag.rs`, 18/18) pass. `tools/dm-mode-integration-test.ts` gives
  24 passed / 12 failed both before this change (at 7ef8d3cf, the Phase 3a
  tip) and after it: the 12 failures are pre-existing (01-helper-row's
  plain drag no longer rewrites `gap`, and the knob/gutter steps that
  follow from it; 03-fractal-tree's "the root stays").
- petal-sdl: `F9` toggles edit mode (`src/edit_mode.rs`, state on the
  `Timeline`), freezing the timeline; a click names the call that drew the
  item (banner + outline); a drag runs a best-effort request against a
  checkpoint rebuilt from the recorded frames
  (`Timeline::checkpoint_for_current`: the previous frame's fork with the
  frozen frame's input bound), writes the answer (sketch or imported file)
  and the watcher's hot reload replays the frozen frame; an error is shown
  and nothing is written; the applied edit and `z undo` show in the
  timeline overlay; `F9`/`F5` leave and resume. Agent-protocol commands
  `edit_mode`, `edit_pick`, `edit_drag`, `edit_undo`.
- Tests: `game_loop::tests::edit_mode_by_scripted_input` (scripted SDL-style
  input events through the windowed loop's key and edit handling, through
  the real hot reload), `an_unreachable_drag_writes_nothing`, and
  `tests/edit_mode_protocol.rs` (the headless binary over stdin).
- Docs: "Hosts" in `docs/features/direct-manipulation.md`, petal-sdl's
  README ("Edit mode") and `docs/agent-protocol.md`.
- Left for part 2 (web-canvas): emit tracing in the wasm host, the mode in
  diagram-canvas with selection in its source editor
  (`DragFilter::allow_selection`), and a Playwright test.

### Phase 3b part 2: edit mode in petal-web-canvas / diagram-canvas — done 2026-11-11

- Wasm host (`integrations/petal-web-canvas/rust/src/lib.rs`):
  `set_emit_trace` (traced drains keep the frame's whole-pixel commands and
  origins; the sub-pixel drain uses the new
  `petal_ui::draw::take_frame_commands_snapped_traced`),
  `set_edit_checkpoint` (a pre-run fork per `reset_and_run`, as Garden's),
  and JSON entry points `edit_pick`, `edit_begin(index, lines)`,
  `edit_preview`, `edit_finish`, `edit_cancel`, `edit_drag`. A non-empty
  line list is the allow set (`DragFilter::allow_lines`, ranked, up to three
  alternatives); an empty one runs `SessionOptions::best_effort()`. Each
  alternative carries the whole new sketch text. Loading a program drops the
  drag, checkpoint and traced frame. Six native unit tests (`cargo test` in
  `rust/`; `console_error_panic_hook` is now wasm-only so they can run).
- Shared: `hit_test` / `bounds` moved from petal-sdl's `timeline.rs` into
  `petal_ui::edit_mode` (re-exported there), plus `describe_call`
  (`CallLabel`), which petal-sdl's `name_call` now uses.
- Core fix found on the way: turning emit tracing on after frames had run
  left memo records made untraced, whose replay attributed emits to nothing
  (a pick named "a call with no source position"). `Env::enable_emit_trace`
  now clears the memo of the default context's stacks when it turns tracing
  on (`emit_trace_turned_on_late_still_attributes_memoized_calls`).
- diagram-canvas (`examples/custom-apps/diagram-canvas`): an **Edit Mode**
  sidebar button; `src/edit-mode.ts` freezes the frame loop after one traced
  frame, lays an overlay canvas over the page canvas (so the script gets no
  input), outlines and names the clicked call and selects its line in the
  editor; the editor's non-empty selection is the allow set and is marked
  (`src/editor.ts`, session only); a drag opens on passing the slop, previews
  per move, and on release puts the best answer in the editor as one minimal
  change (one undo) and runs it, listing the others; a refusal shows its
  reason and changes nothing.
- Test: `npm run test:e2e --workspace examples/custom-apps/diagram-canvas`
  (`tests/edit-mode.test.mjs`, Playwright, headless Chromium or system
  Chrome): click names `draw_circle` at line 5 and selects it; a selection
  holding nothing that moves the shape is refused with the text unchanged;
  selecting line 3 moves only `off` and the circle is drawn at the drop
  point; no selection is best effort with one answer; a `rigid` shape is
  refused; editor undo takes an edit back. `playwright` is a devDependency
  of the app.
- garden-web / the petal-lang.org `/manipulate` gallery: unaffected by the
  host change (garden-web goes through garden-script, whose types are
  unchanged; it still builds for wasm). `check-dm-samples.mjs` is green
  against the currently vendored build; the memo fix reaches it on the next
  re-vendor.

### Phases 1, 2, 5 and 6

- **Phase 1 (2026-11-11): the new `autodiff` functions — done.**
  `core/packages/autodiff/autodiff.ptl` adds `deriv`, `jacobian`,
  `solve_step` and `solve` (Petal code, beside the dual natives), each probe
  under `speculate` or direct when `speculating()`. Natives: `gn_step`
  (renamed from `drag_solve_step`, which stays as an alias; `vary` calls
  `gn_step`) and `fd_column`, which replays a script's samples through DM's
  float-column rules: `dm/jacobian.rs`'s `float_column` / `escalate` are now
  generic over a `ColumnProbe` (DM's run probe, or the replay), exposed as
  `dm::column_from_samples`. DM's own columns are unchanged (all core DM
  tests green). A script's `speculate` has no path fingerprint, so without
  one a kink is re-checked at `h / 10`: a side whose difference does not
  shrink jumped (`branch_flip`, or `step` when both did), and a gap that
  shrinks with the step is curvature (`smooth`, e.g. `x * x` at 0, which
  DM's path-aware rules call a kink). Only a non-number result is
  `"errored"`; a runtime error in `f` propagates (Petal has no catch).
  Tests: `test/ptl/autodiff.ptl` (18 cases), `core/tests/all/autodiff.rs`
  (no emits or state leak across frames), unit tests in `dm/jacobian.rs`.
  Docs: `features/differentiation.md` rewritten, `Builtins.md` sections,
  CHANGELOG. Measured (release, `petal bench`, `f` =
  `(x - 3)^2 + sin(x)`): `f(x)` 0.19 µs, `speculate(f, x).value` 0.47 µs
  and 3 allocations, `deriv(f, x)` 4.2 µs and 15 allocations (3 runs of
  `f`, 3 `fd_column` calls). When Phase 0's `speculate_value` lands,
  `autodiff.ptl`'s `_run` should switch to it.
  Pre-existing: the doc-snippet test fails on four blocks of this plan
  (the Design §2 signature block, the two `var x` / `x = …` blocks of
  Before/After 1, and `// 0.5, exactly`); they are proposals, not run.
- **Phase 2 (2026-11-11): call sites migrated, dual functions deprecated — done.**
  `examples/console/differentiation.ptl` (After §1, printing rounded; its
  golden updated), `test/lerp-trig-random` (`abs(deriv(atan, 1.0).value -
  0.5) < 1e-6`) and `examples/ui/d2-dial` (plain `vary`) are rewritten.
  `ui/drag`'s `vary` lost its dual mode (`_dual_run`, `_undual`,
  `_dual_center`, `_jac_agrees`); the `sensitivity` key is ignored, and
  `PRELUDE_LEVEL` is 14. A compile warning `[dual-deprecated]`
  (`compiler/dual_deprecated.rs`) fires once per name per file on `dual`,
  `value_of` and `deriv_of` reached through `import autodiff`. Lint rules
  (`lint/dual_migrate.rs`): `dual-deprecated` fixes
  `deriv_of(f(dual(x, 1.0)))` → `deriv(f, x).value` for a named `f`, and
  `vary-sensitivity` deletes the key. The four failing plan snippets above
  are tagged `petal ignore`. Website: `aim.ptl` rewritten per After §2
  (petal-lang.org commit). The site runs the vendored wasm
  (`frontend/public/petal`, from `pnpm build:sdk`), which predates
  Phase 1, so the demo needs a re-vendor before the site is deployed;
  that is left to Phase 5's re-import.
- **Phase 5 (2026-11-11): dual numbers removed — done.** Gone:
  `Value::Dual`, `DualId`, the pair slab's dual accessors
  (`dual_value`, `get_dual`, `derivative_of`; the slab now holds only
  `vec2`s), `dual_arith` and the dual negation arm, every native's dual arm
  (`math.rs`'s `unary_float_dual` became `unary_float`; `creative_coding.rs`
  reads plain floats through a `floats::<N>` helper that
  `tools/extract-stdlib.ts` understands), `builtins/autodiff.rs` and the
  three natives' registrations, `Type::Dual` (and the `dual` type name),
  `SpecOverride::Dual`, `OverrideValue::Dual`, `PetalCxt::push_dual`,
  `AllocKind::Dual`, and the dual cases in memo, the static cache, record
  templates, run deps, dict keys, DM observation, `drop_edit`, stats,
  `vm/fast.rs` tests, `petal-ui`'s `hovered`, the C bridge's value view and
  the typecheck tables. `dual`, `value_of` and `deriv_of` reached through
  `import autodiff` are the compile error of Design §1, word for word
  (`compiler/autodiff_removed.rs`, replacing Phase 2's warning); the
  `dual-deprecated` lint rule went with the warning, and `vary-sensitivity`
  moved to `lint/vary_sensitivity.rs`. State storage can hold a dual (the
  `--state-storage` file and the C bridge's `pb_vm_load_state` document,
  `{"$petal": "dual", …}`), so such a slot loads as its value and is named in
  `StateStorageLoad::converted` (warned by the CLI, listed under
  `converted` in the bridge's load report), for one release. Tests:
  `test/vitest/autodiff.test.ts` rewritten for `deriv` / `jacobian` /
  `solve_step` / `solve` (math builtins against closed-form derivatives
  within 1e-6, and the error), `core/tests/all/autodiff.rs` (the error,
  bare/qualified/aliased), a state-storage test. Docs: `ffi.md`,
  `Architecture.md`, `syntax/overview.md`, `language-guide.md`,
  `packages.md`, `Builtins.md`, `CLI.md`, `features/differentiation.md`
  (a migration section the error points at), CHANGELOG (Removed, with an
  **Embedders** note), the stale count in `proposals/value-representation.md`,
  and the propose-language-change checklist item. Plans, proposals and
  dated changelog entries that describe the old design were left as the
  record they are; the three old IR fixtures under
  `test/vitest/fixtures/ir/` still name the removed natives among their
  builtin phantom terms and load fine. Verified: core tests (both feature
  sets, doctests), petal-ui, petal-c-bridge, ui-corpus, vitest,
  `tools/test-examples.ts` (opts vs no-opt and static cache on/off, 35/35),
  `make expensive_lint_tests`, and `dm_study`'s ladder: all 105 goals have
  the same outcome, run count and top edit as before. The Kani proofs were
  not run (`cargo-kani` is not installed here); they have no pair-slab or
  dual harness to update.
- **Phase 6 (2026-11-11): README — done.** "Differentiable programming" is
  replaced by "How goals get solved" (algebraic inversion, measured
  sensitivities with Gauss-Newton steps, speculative search), after the
  direct-manipulation section, whose backpropagation aside is gone.
