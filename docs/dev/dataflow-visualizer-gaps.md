# Dataflow visualizer: gaps in what the compiler exposes

petal-lang.org has an experimental visualizer that draws any Petal script as
a dataflow diagram (`/home-page-proposals/#dataflow`; code in
`~/biz/petal-lang.org/frontend/src/proposals/dataflow/`). It compiles in the
page with the real compiler through `PetalRuntime::dataflow_ir(source)`
(petal-web-canvas, which returns the `petal show-ir --json --user-only` view),
then folds the IR into one node per binding in TypeScript (`extract.ts`) and
lays it out with ELK.

The fold works on every `.ptl` on the site, but it has to guess in places,
because the IR is built to execute, not to be read back as source structure.
This page lists those places, most important first, with what upstream could
add. The tag (G1…) is the one the comments in `extract.ts` cite.

What the visualizer draws, for reference: one node per `let`, `state`,
`var`, parameter and loop variable; host inputs (`mouse_x()`, `dt()`) as
sources; statement calls whose result is unused (`draw_circle(…)`) as
effects; a function's result; one frame per function body and loop body.
Everything between two nodes (arithmetic, pure builtins, constants, copies)
is folded into the edge.

## G2. The IR does not say what a native does (most important)

To put `mouse_x()` on the left as an input and fold `sin()` away, the
visualizer needs to know which builtins read the host. The IR's
`BuiltinCall("mouse_x")` looks exactly like `BuiltinCall("sin")`, so
`extract.ts` keeps a hand-written `HOST_INPUTS` list copied from the canvas
API. It is wrong for any other host (Garden panels, Cheesecake, a native an
embedder registers) and goes stale when a native is added.

The runtime already knows: every native is registered with a
`NativeEffects` row (docs/dev/native-effects.md): `reads` (POINTER,
KEYBOARD, CLOCK, VIEWPORT, RNG, HOST_DATA …), `emits`, `effect`, `PURE`,
`CONST`. It just is not in the dump.

**Add:** a `natives` table in the user-only view (or alongside it in
`dataflow_ir`): for each native name a call in the program reaches, its row
as flags. The visualizer then classifies every call from the row: `reads` →
an input node (labelled by class: pointer, clock, viewport), `emits` → an
output node, `PURE`/`CONST` → folded. This also answers G4 for natives: an
`emits` call is an output whether or not its value is used.

## G1. Declarations and control terms have no source span

Several terms that a reader points at carry no span in `source_map`:

| Term | Example | What the visualizer does instead |
|---|---|---|
| `StateInit` | `state angle = 0.9` | takes the line of the init block's first spanned term |
| `CellNew` | `var total = 0` | takes the line of its input |
| `StateWrite`, `CellWrite` | `angle += speed * delta` | takes the span of the written value |
| the `Copy` that rebinds a name after an assignment | `x = sw + 10.0` | same |
| `ForLoop`, `NumericForLoop`, `WhileLoop` | `for k in range(0, len(trail))` | regex-matches `for … do` on the line of the header's inputs |
| `MakeClosure` of a function that joins an overload set (G7) | `fn toggle(label, x, y)` | finds the function by its body's spans |

**Add:** a span on each of these covering the statement as written
(`state angle = 0.9`, the whole `for` header line, the whole assignment).
Two spans would be better still for declarations: the statement and the
name, so a tool can highlight `angle` in `state angle = 0.9`.

## G3. `--user-only` still carries module top-level terms

`ir_display::user_only_json` drops prelude terms but keeps some top-level
terms of imported modules in the root block. With `import ui`: a dozen or more
`Constant(nil)` terms, `CellWrite`s and `StateInit`s named
`ui/_style::_live_theme` and the like, and a `CellNew` named `_focus_now`
with no `::` at all. The visualizer drops any term whose name contains `::`
and any term with no span in, under or upstream of it, which is a heuristic.

**Add:** filter by the source map's file of each term (the entry file is
file 0), not by the prelude flag alone, so the view holds exactly the entry
file's terms plus the binding phantoms they reference. If a term must stay
(a capture target), mark it `"external": "ui"` instead.

## G4. No statement structure: is a value used, returned, or dropped?

A block is a flat list of terms. To tell `draw_circle(x, y, 4.0)` (a
statement) from `a + (b - a) * t` (a function's result), the visualizer
checks whether any term reads the value, and treats the last term of a
function body as its result, unless no call site of that function uses its
result. That gets `fn firefly(i) … draw_circle(…)` right (called as a
statement, so its last draw is an effect), but it is a whole-program guess.

**Add:** per block, the term that is its result (`result: TermId` on
`Block`; the evaluator knows it already), and per statement-level term a
flag that its value is discarded. G2 covers natives; this covers user
function calls and expressions.

## G5. A `Phi` names only the value from before the branch

`if x < -10.0 then x = sw + 10.0 end` compiles to `Phi [x_before] name=x`
plus a `Branch` whose arm writes the new value; the arm's value is not an
input of the `Phi`. Any consumer walking inputs from a later read of `x`
misses `sw` unless it also knows to look inside the branch. The visualizer
avoids the problem only because it merges every version of a `state`, `var`
or reassigned `let` into one node and collects the assigned values from the
writes themselves.

**Add:** the arm-exit values as `Phi` inputs (one per arm, the fall-through
being the pre-branch value), or a `merges: [TermId]` side table. This would
help `petal graph` and `explain` too, which face the same question.

## G6. Calls do not say what they call, by name

For `BuiltinCall` the name is a constant. For `Call`, input 0 is a value:
a `Copy` of a `MakeClosure` (user function), a capture phantom of a module
function the user-only view filtered out (`draw_circle` from `ui`), or a
`MakeOverloadSet`. The visualizer recovers a label by slicing the source
text at input 0's span, and finds a user function by following copies and
captures. Separately, arguments are mapped to parameters by position; the
`arg_names` on the term are not used yet (a visualizer gap, not upstream).

**Add:** on each call term the resolved callee when it is static: function
id (user) or qualified name (`ui::draw_circle`).

## G7. A user function named like a module function joins an overload set

`fn toggle(label, x, y)` in a script that imports `ui` (which has a
`toggle`) compiles to `MakeOverloadSet [ui::toggle, <user toggle>]`, and the
user's `MakeClosure` loses its span. The visualizer picks the user's
overload by looking for a closure whose body has spans. Dispatch is by
arity at run time, so a call might in fact reach `ui::toggle`. Probably
correct language behaviour, but a surprise to a reader: worth a lint, and
G6's resolved callee would settle it per call site where arity decides it
statically.

## G8. Higher-order builtins do not say where their arguments go

In `map(xs, fn(v) v * 2)` the elements of `xs` become `v`. The visualizer
draws `xs → ys` and `result of fn(v) → ys`, but cannot draw `xs → v`,
because nothing says which argument of `map` feeds which parameter of the
callback (`reduce` feeds two, `sort` with a key one, `for_each` one).

**Add:** a small table, next to the native rows, of callback parameter
flow for the core higher-order builtins (`map: arg 0 elements → callback
param 0`).

## G9. Ids change on every edit

Term and block ids are allocation order, so a one-character edit renumbers
everything after it. The visualizer keys folded functions by their header
text to keep them folded across edits, and the layout jumps when the graph
changes. A stable id per binding (scope path + name + ordinal, like the call
site ids `state` already uses for call paths) would let a tool keep folds
and selection, and animate the diagram between edits instead of redrawing
it.

## G10. Unknown names are runtime errors, not compile errors

`let delta = dt()` without `import ui` compiles, and the visualizer draws
it, but the first run fails with `Unknown function: dt`. A diagram of a
program that cannot run is misleading. A compile-time unresolved-name
diagnostic (a warning at least) in `compile_program` would let
`dataflow_ir` report it.

## G11. Opportunity: live values

The diagram is static. The runtime can trace (garden-web's `trace_all`,
`petal explain`), so a future `dataflow_ir` could take a frame and return
each node's value, and each `state` declaration's instance count (three
`slider` call sites, three `value` slots). The visualizer would show values
on edges and `×3` on the `slider` frame. That would make "the program is
the diagram" literal.

## G12. Minor: float constants are bit patterns in JSON

`Constant({Float: 4610334938539176755})` is `1.7`. The visualizer never
reads constants (it shows source text), but every other JSON consumer has
to decode `f64::from_bits`. A readable `{Float: 1.7}` (or both) in the
user-only view, which is a debugging view anyway, would be friendlier.

## Where this should end up

The fold in `extract.ts` is about 900 lines of inference over IR. With G1 to
G6 fixed most of the inference disappears, and the rest is small enough to
live in core as a supported view: `core/src/dataflow_view.rs`, exposed as
`petal show-graph --json` (today `show-graph` is DOT only, over raw terms)
and as `dataflow_graph(source)` in petal-web-canvas and garden-web. Garden
could then draw the same diagram beside the editor, and the LSP could
answer "what feeds this?" from it.
