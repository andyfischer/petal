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

## Status (2026-11-17)

| Gap | Verdict |
|---|---|
| G1 spans | Done |
| G2 natives | Done: a `natives` table in `dataflow_ir` and `show-ir --json --user-only` |
| G3 module terms | Done, through G1 |
| G4 statement structure | Rejected |
| G5 `Phi` arm values | Rejected: already in the JSON |
| G6 resolved callee | Deferred |
| G7 overload merge | Lint done; semantics unchanged |
| G8 higher-order flow | Rejected |
| G9 stable ids | Deferred |
| G10 unknown names | Done: `diagnostics` in `dataflow_ir` |
| G11 live values | Deferred |
| G12 floats | Done, in the user-only view only |
| `dataflow_view.rs` in core | Rejected for now |

Each section below ends with what was done or why not.

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

**Done.** `ir_display::natives_json(&Env, &Program)` lists every native the
entry file reaches, by name: through a `BuiltinCall`, a builtin passed as a
value (`map(xs, sin)`), or the body of a module or prelude function the file
calls. Each row has `kind`, `reads` (lowercase class names: `pointer`,
`clock`, `viewport` …), the flags `emits`, `effect`, `declares`, `command`,
`probe`, `constant`, `ticks` and `contained`, plus `direct` (the file's own
code names it) and `registered` (false for a name this `Env` does not
register). It is built from the `Env`, not stored in `Program`, so it reflects
whatever host compiled the script. `dataflow_ir` returns it as `natives`,
and `show-ir --json --user-only` as a top-level `natives` key. The reach is
over-inclusive (a reached module closure counts as called), and natives
reached only through `MethodCall` dispatch are missed. `extract.ts` can drop
`HOST_INPUTS` for `natives[name].reads`, `emits` and `constant`.

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

**Done**, statement spans only (no separate name span). `StateInit` (and
`memo`, `state var`), `CellNew`, `StateWrite`, `CellWrite` and the rebinding
`Copy` cover the whole statement; a loop and its loop-variable `Copy` cover
the header through `do` (`Compiler::loop_header_span`); a function's
`MakeClosure` covers its declaration even when the name joins an overload
set, and so do the hoisted-cell terms a declaration fills. Two compiler-made
terms stay spanless: the arm-entry `Copy` of an `if` arm, and the set
`merge_disjoint_sets` builds when two imports offer the same name. A
runtime error's "Caused by:" list can now name more terms (declarations,
loop variables, closures). `extract.ts` can drop the line-of-first-input
fallbacks and the `for … do` regex.

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

**Done, through G1.** The module terms were spanless, so they took the
entry file's id and survived the file filter. With spans they carry their
own module's file, and `IrFilter` drops them: no name heuristics. With
`import ui` the view of a small script went from about 95 terms to 43, and
every remaining root-block term without a span is a builtin binding phantom.
`extract.ts` can drop its `::` and spanless filters.

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

**Rejected.** A block's result is its last term by convention (the bytecode
lowering's `is_block_result`), so a `result` field would restate that. "Is
this function's result used" is a whole-program question that depends on
every call site, which a per-block flag cannot answer either. G2's `emits`
covers the case that matters most, a native drawn as an output.

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

**Rejected: the values are already in the JSON.** Each arm block lists its
exits as `phi_outs: [{src_term, dest_term}]`; a consumer finds the `Phi`'s
arm values there. Making them `Phi` inputs would change the bytecode
lowering and every analysis that reads `inputs[0]` as the value from before
the branch.

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

**Deferred.** With G3 the module functions are gone from the view, and
following `Copy` to `MakeClosure` finds a user function, which covers most
of it. Revisit when a second consumer needs it.

## G7. A user function named like a module function joins an overload set

`fn toggle(label, x, y)` in a script that imports `ui` (which has a
`toggle`) compiles to `MakeOverloadSet [ui::toggle, <user toggle>]`, and the
user's `MakeClosure` loses its span. The visualizer picks the user's
overload by looking for a closure whose body has spans. Dispatch is by
arity at run time, so a call might in fact reach `ui::toggle`. Probably
correct language behaviour, but a surprise to a reader: worth a lint, and
G6's resolved callee would settle it per call site where arity decides it
statically.

**Lint done; semantics unchanged.** The span half is G1. The new opt-in
rule `import-overload-merge` reports an entry-file `fn` that the compiler
merged by arity with a function from a plainly imported module
(`--rules-include=import-overload-merge`). It is opt-in because the repo
and downstream projects have about 140 such merges, most of them on
purpose. Changing the merge itself would go through a language proposal.

## G8. Higher-order builtins do not say where their arguments go

In `map(xs, fn(v) v * 2)` the elements of `xs` become `v`. The visualizer
draws `xs → ys` and `result of fn(v) → ys`, but cannot draw `xs → v`,
because nothing says which argument of `map` feeds which parameter of the
callback (`reduce` feeds two, `sort` with a key one, `for_each` one).

**Add:** a small table, next to the native rows, of callback parameter
flow for the core higher-order builtins (`map: arg 0 elements → callback
param 0`).

**Rejected.** It would be a table of special cases for each builtin, kept in
step with the builtins by hand. The visualizer can keep its own.

## G9. Ids change on every edit

Term and block ids are allocation order, so a one-character edit renumbers
everything after it. The visualizer keys folded functions by their header
text to keep them folded across edits, and the layout jumps when the graph
changes. A stable id per binding (scope path + name + ordinal, like the call
site ids `state` already uses for call paths) would let a tool keep folds
and selection, and animate the diagram between edits instead of redrawing
it.

**Deferred.** This is a design project. `Term.call_site` is a starting
point.

## G10. Unknown names are runtime errors, not compile errors

`let delta = dt()` without `import ui` compiles, and the visualizer draws
it, but the first run fails with `Unknown function: dt`. A diagram of a
program that cannot run is misleading. A compile-time unresolved-name
diagnostic (a warning at least) in `compile_program` would let
`dataflow_ir` report it.

**Done.** The check already existed (`Env::scan_unresolved`); it was just
not passed on. `dataflow_ir` now returns `{ok, ir, natives, diagnostics}`,
each diagnostic with `message`, `severity`, `line`, `column`, `end_line`,
`end_column` and `file` (null for the entry file), from the new
`petal::diagnostic::program_diagnostics_json`. `let delta = dt()` without
`import ui` gives an error "unknown function `dt`" at line 1, column 13.
`petal lint --json` and `check --json` use the same helper and gain
`end_line` and `end_column`.

## G11. Opportunity: live values

The diagram is static. The runtime can trace (garden-web's `trace_all`,
`petal explain`), so a future `dataflow_ir` could take a frame and return
each node's value, and each `state` declaration's instance count (three
`slider` call sites, three `value` slots). The visualizer would show values
on edges and `×3` on the `slider` frame. That would make "the program is
the diagram" literal.

**Deferred.** A feature idea, not a gap.

## G12. Minor: float constants are bit patterns in JSON

`Constant({Float: 4610334938539176755})` is `1.7`. The visualizer never
reads constants (it shows source text), but every other JSON consumer has
to decode `f64::from_bits`. A readable `{Float: 1.7}` (or both) in the
user-only view, which is a debugging view anyway, would be friendlier.

**Done, in the user-only view only.** `user_only_json` renders
`{"Float": 1.7}`, with `"NaN"`, `"inf"` and `"-inf"` for values JSON cannot
hold. The interchange `Program` serialization still carries the bits.

## Where this should end up

The fold in `extract.ts` is about 900 lines of inference over IR. With G1 to
G6 fixed most of the inference disappears, and the rest is small enough to
live in core as a supported view: `core/src/dataflow_view.rs`, exposed as
`petal show-graph --json` (today `show-graph` is DOT only, over raw terms)
and as `dataflow_graph(source)` in petal-web-canvas and garden-web. Garden
could then draw the same diagram beside the editor, and the LSP could
answer "what feeds this?" from it.

**Rejected for now.** With G1, G2, G3 and G10 done, `extract.ts` loses most
of its guessing, and one consumer does not justify a supported view in core.
Revisit when Garden wants the diagram.
