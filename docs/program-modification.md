# Programmatic Program Modification

How Petal programs can be **changed by code** — by tools, agents, embedders, and
the language's own live-editing machinery — rather than only by a human typing
into an editor.

This document is a **catalogue of what exists today**. It is the starting point
for a body of work exploring better ways to modify programs: richer
**hot reloading**, **programming-by-direct-manipulation** (drag an output, edit
the source that produced it), and **goal-driven edits** (specify a target
result, let the system suggest source changes). Those directions are sketched at
the end and cross-referenced to [dev/goals.md](dev/goals.md); everything before
that is shipped and testable.

> Design context: [dev/Architecture.md](dev/Architecture.md) (the term graph),
> [dev/ir-as-target.md](dev/ir-as-target.md) (the IR emit contract),
> [dev/speculative-execution-plan.md](dev/speculative-execution-plan.md) (forking),
> and [dev/debugging-visibility.md](dev/debugging-visibility.md) (agent surface).

---

## The three layers a program can be modified at

A Petal program exists in three representations, and each is a distinct
modification surface with its own tooling:

| Layer | Representation | Modify by | Preserves |
|---|---|---|---|
| **Source** | `.ptl` text (+ lossless CST) | text splice / tree splice / rebind desugar / lint | comments & formatting (CST path) |
| **IR** | term-graph `Program` (JSON) | emit / transform / validate | dataflow structure |
| **Running program** | live `Stack` + `ExecutionContext` | hot reload / state-set / input / fork | live state across a swap |

Because Petal is a dataflow language with no register-mutation primitive
(rebindings lower to pure `Phi` joins — see
[dev/debugging-visibility.md](dev/debugging-visibility.md)), all three layers are
unusually legible: the IR is a clean graph you can walk and rewrite, and runtime
state is keyed structurally so it can migrate across an edit.

---

## Layer 1 — Source modification

### 1a. The rebind operator `@` (source desugar)

The narrowest form of programmatic edit is a **source-level rewrite baked into
the language**. `something(@var)` desugars to `var = something(var)` — the
immutable-value "call and assign back" pattern — entirely at parse time in
[`rust/src/desugar.rs`](../rust/src/desugar.rs). It adds no runtime machinery;
`append(@nums, 4)` compiles to exactly what `nums = append(nums, 4)` does.

See [rebind-operator.md](rebind-operator.md). Relevant because it is a worked
example of a purely syntactic transform with well-defined scoping rules (nearest
enclosing call, lifted to nearest statement, refused in deferred/conditional
positions) — the same discipline a larger refactoring engine needs.

### 1b. Formatting-preserving tree splices (`rust/src/rewrite.rs`)

The AST is **lossy** — it carries spans but drops comments and whitespace — so
rewriting through it would reformat the whole file. Instead, source edits go
through the **lossless green tree** (the CST, [`rust/src/cst/`](../rust/src/cst/)):
locate the node covering the construct, splice in a subtree parsed from the
replacement snippet, keep the old node's leading/trailing trivia, and re-emit.
Everything outside the replaced node is untouched because it is still the same
shared green subtrees. The invariant `build_lossless(src).text() == src` holds
for every source ([`cst/mod.rs`](../rust/src/cst/mod.rs)).

Low-level primitives in [`rust/src/rewrite.rs`](../rust/src/rewrite.rs):

| Function | Purpose |
|---|---|
| `parse_ast(source)` | Parse to `(green tree, top-level Stmts)` for inspection/rewrite |
| `find_call(stmts, name)` | Span of the first top-level `name(...)` statement call |
| `splice_node(tree, span, replacement)` | Tree-splice: replace a node, preserve surrounding trivia |
| `splice(source, span, replacement)` | String-level fallback (char offsets; multi-byte safe) when the replacement isn't a single parseable expression |

These are the intended surface for "the program is *also* a live document the
user is editing": a tool can rewrite a single `layout(...)` call and write the
result back without clobbering the rest of the file.

### 1b′. Goal-based editing (`rust/src/goal_based_editing.rs`)

The primitives above are wrapped by a **declarative, goal-based** API. Rather
than "replace this span," a caller states *goals* — properties the edited source
should satisfy — and the module decides whether to insert or update in place.

| Item | Purpose |
|---|---|
| `Goal::should_call(function, params)` | Goal: source should contain a top-level `function(params...)` call. `params` are structured `Arg` values (`&str`/`i64`/`f64`/`bool` coerce in via `From`). |
| `Arg` | A structured argument: `Str`/`Int`/`Float`/`Bool`/`Nil`, plus `Expr` (verbatim source escape hatch). Strings are rendered as quoted, escaped Petal literals. |
| `modify_source_with_goals(source, goals)` | Apply a list of goals in order; returns the rewritten source. |

`ShouldCall` updates the first existing top-level call to `function` (replacing
its argument list, layout-flexibly) or appends the call if absent — the shape a
config file like `~/.garden/init.ptl` wants for a "set the color scheme" menu
action. Goals compose (apply several in one pass, later goals see earlier
insertions), and `Goal` is the extension point for richer intents (ensure an
import, remove a call, set a field). This is the seam a broader structured-edit
and goal-driven API grows from. **Usage guide:**
[goal-based-editing.md](goal-based-editing.md).

### 1c. `petal lint --fix` (normalize source in place)

[`rust/src/lint/`](../rust/src/lint/) is the only **agent-usable command that
rewrites program source on disk today**. `lint_source`
([`lint/mod.rs`](../rust/src/lint/mod.rs)) applies two normalizations:

- **Rebind rewrite** — `x = f(x)` → `f(@x)`, **gated by an IR-equivalence check**
  (the rewrite only applies if the before/after compile to the same IR).
- **Reindent** — token-driven 2-space re-indentation
  ([`lint/reindent.rs`](../rust/src/lint/reindent.rs)), preserving structure.

CLI: `petal lint [--fix | --check]`. `--check` reports without writing; `--fix`
writes. This is a real, if narrow, example of a **verified automatic edit** —
the IR-equivalence gate is the pattern goal-driven refactors should follow.

---

## Layer 2 — The IR as a construction & transformation target

Petal's IR is a **documented, versioned, load-and-run emit target**. You can
construct or transform a program *as data* and run it directly, bypassing the
Petal front-end entirely. Full contract:
[dev/ir-as-target.md](dev/ir-as-target.md).

### 2a. The term graph as a data structure

A `Program` ([`rust/src/program.rs:352`](../rust/src/program.rs)) owns the whole
graph:

- `terms: Vec<Term>` — nodes, indexed so `terms[i].id == i`.
- `blocks: Vec<Block>` — scopes; `root_block`, `constants`, `functions`, `match_arms`.

Each `Term` ([`program.rs:237`](../rust/src/program.rs)) is in **two graphs at
once**: the **dataflow DAG** via `inputs: SmallVec<[TermId;4]>` (ordered value
edges) and the **intra-block execution order** via `block_next`/`block_prev`
(a linked list; `Block.entry` is the head). Other fields: `op: TermOp`,
`block_id`, `name` (binding label), `register`, `state_key`, `child_blocks`,
`in_loop`.

`TermOp` ([`program.rs:73`](../rust/src/program.rs)) is the operation vocabulary:
arithmetic/comparison, `Copy`, `Phi`, `Branch`, `Return`, `Constant(id)`,
`MethodCall(id)`, `MakeClosure(fn)`, `AllocMap`, `AllocElement`,
`MakeEnumVariant`, etc. There is **no register-mutation op** — cross-block
rebinding goes through a `Phi` and `Block.phi_outs: Vec<PhiOut>`
([`program.rs:287`](../rust/src/program.rs)): on child-frame pop, `src_term`'s
value is written into the parent frame at `dest_term`'s register (`dest_term`
must be a `Phi`).

### 2b. Emit / load / run round-trip

Serialization is **JSON via serde derives** — the wire shape is the derived
serialization of `Program`, matching `show-ir --json` byte-for-byte.

- **Emit:** `petal show-ir --json` →
  [`cli/handlers.rs:314`](../rust/src/cli/handlers.rs) (`serde_json::to_string_pretty`).
- **Load:** `Program::from_json` ([`ir_validate.rs:19`](../rust/src/ir_validate.rs))
  → `rebuild_indexes()` (rebuilds the `block_terms` index + constant dedup) →
  `validate()` (eight structural invariants).
- **Run:** `petal run --ir <file|->` → `env.load_program_ir`
  ([`env/mod.rs:280`](../rust/src/env/mod.rs)) → same bytecode-VM path as a
  compiled program. Guarantee: `show-ir --json | run --ir -` equals `run`.

So a program can be **built or rewritten as JSON, validated, and executed**
without touching source text.

### 2c. Reference emitter & transform passes

- **Foreign-language emitter (the canonical builder pattern):**
  [`ts/tools/calc-to-ir.ts`](../ts/tools/calc-to-ir.ts) is a complete standalone
  front-end for a toy "calc" language that emits Petal IR JSON sharing **zero
  code** with Petal. Its `Emitter` class shows the mechanics: a deduped constant
  table (`constId`), phantom builtin `Copy` terms in leading slots
  (`addPhantom`), and `addListed` threading the `block_next`/`block_prev` linked
  list. This is the model for programmatic construction. Golden fixtures live in
  [`ts/test/fixtures/ir/`](../ts/test/fixtures/ir/).
- **Read/rewrite passes over the graph (Rust):**
  [`rust/src/program_analysis.rs`](../rust/src/program_analysis.rs) —
  `trace_provenance` (backward dataflow slice), `trace_dependents` (forward
  slice), `slice(targets)` (minimal connecting subgraph), `find_term`,
  `named_terms`. Exposed on the CLI as `show-provenance`, `show-dependents`,
  `show-slice`, `show-graph` (DOT). These are read-only today but define the
  graph queries a transformation would target (e.g. "slice the constants that
  influence this output, then rewrite them").

There is **no dedicated `IrBuilder` API in Rust** — the "builder" is either the
compiler (internal, [`rust/src/compiler/`](../rust/src/compiler/)) or a foreign
emitter following the JSON contract.

---

## Layer 3 — Modifying a running program (live)

### 3a. State-preserving hot reload (`transfer_state`)

The core primitive is **`Env::transfer_state`**
([`rust/src/transfer_state.rs:24`](../rust/src/transfer_state.rs)): it reshapes a
running stack onto a freshly compiled `Program` while keeping matching state
values. Hot reload is one use; the mechanism is general ("reshape a stack for any
new program that shares the same StateKeys").

Flow:
1. Capture the running stack's `program_id` (the swap must reuse the same `ProgramId`).
2. Collect the new program's state keys (`Program::state_terms()`).
3. `insert_program` swaps the program and **invalidates cached bytecode** (re-lowers the new IR).
4. `clear_closures()` — old closures point into the replaced function defs.
5. On the stack: `retain` drops removed state keys, `reset_execution()`, `functions.clear()`.
6. Next `run` re-pushes the VM root frame against the new lowering.

Returns `TransferStateResult { state_preserved, state_dropped }`.

**State reconciliation is by name-hash, order-independent.** `StateKey(u64)`
([`program.rs:44`](../rust/src/program.rs)) is a **hash of the state variable's
name**, computed at compile time by `Compiler::state_key_for`
([`compiler/mod.rs:461`](../rust/src/compiler/mod.rs)) — bare name for entry-file
`state`, qualified `"module::name"` for module state. The runtime key is
`RuntimeStateKey { base: StateKey, loop_indices }`
([`stack.rs:24`](../rust/src/stack.rs)); `transfer_state` matches on `base` only.
Migration semantics (pinned by tests in `transfer_state.rs`):

- **Addition** — new key absent in old state → `StateInit` runs, default-initializes.
- **Removal** — old key absent in new program → dropped in the `retain`.
- **Modification / reordering** — value preserved by matching `base`, regardless of declaration order.

A separate per-run GC (`Stack.touched_state_keys` +
`sweep_untouched_state`, [`stack.rs`](../rust/src/stack.rs)) reclaims state whose
declaration wasn't visited on a run (e.g. per-iteration state for a removed list
item).

**Known limitation:** renaming a `state` var — or moving/renaming its module —
changes its `StateKey` and **drops the value** (it reads as remove + add). This
is the obvious target for smarter reconciliation (see gaps below).

### 3b. Hosts that trigger reload

- **Native SDL file-watcher** (shared by all SDL hosts) —
  [`integrations/petal-desktop-sdl/src/watcher.rs`](../integrations/petal-desktop-sdl/src/watcher.rs):
  `check_hot_reload` reads the changed file, `compile_program_at` (imports
  resolved relative to the file), then `transfer_state`. `setup_watcher` (the
  `notify` crate) watches the entry script's directory **plus every imported
  module's directory** (`env.module_manifest`), so editing an imported
  `palette.ptl` reloads dependents. Compile errors are **non-fatal** — the old
  program keeps running. Driven by `Reloader` in
  [`game_loop.rs`](../integrations/petal-desktop-sdl/src/game_loop.rs); enabled
  by default, `--no-hot-reload` disables. Same in the `petal-fps` host.
- **Browser live source editor (diagram-canvas)** — a CodeMirror widget
  ([`sample-apps/diagram-canvas/src/editor.ts`](../sample-apps/diagram-canvas/src/editor.ts))
  with a 300 ms-debounced recompile. This is a **full reload, NOT
  state-preserving**: the callback calls `petal.load(source)`
  ([`integrations/petal-web-canvas/src/runtime.ts`](../integrations/petal-web-canvas/src/runtime.ts)),
  which reloads + recreates the stack (state resets, frame count → 0). A WASM
  panic poisons the module and requires a page reload. This is browser-UI only —
  **not exposed over the debug protocol/MCP**.

### 3c. Point-mutating live state, bindings, and input

Over the agent JSON protocol and MCP (see
[dev/debug-protocol.md](dev/debug-protocol.md),
[dev/mcp-server.md](dev/mcp-server.md),
[petal-desktop-sdl/docs/agent-protocol.md](../integrations/petal-desktop-sdl/docs/agent-protocol.md)):

| Surface | Effect |
|---|---|
| `set_state {name, value}` / `DiagramSetState` | Mutate one live state var by name (`set_state_json`) |
| `input {keys_down, mouse, text}` / `DiagramInput` | Inject keyboard/mouse/text into the next frame |
| `pause` / `resume` / `step {n}` | Control the frame loop (advances at fixed `dt=1/60`) |
| host bindings (`set_binding_for`) | Change host→script uniform inputs |

These modify **runtime state or inputs**, not the program. **There is no
over-the-wire source-swap / `reload` command** — reload is file-watcher-driven
only. The command set is exactly `pause, resume, step, state, set_state,
capture_draw_commands, input, screenshot, pending_report`
([`sample-apps/diagram-canvas/src/debug.ts`](../sample-apps/diagram-canvas/src/debug.ts)).

### 3d. Speculative execution — safe experimental modification

Petal can **fork a running execution**, apply a modification (different inputs or
a variant program), run it, and compare — **without disturbing the original**.
The heap is immutable by construction (collections are value-semantic), so a fork
shares no mutable state with its source.

- `Env::fork_execution(src)` ([`rust/src/env/`](../rust/src/env/)) deep-clones the
  `ExecutionContext` (heap + closures/overload sets/bindings/counters) and gives
  the fork fresh output buffers.
- `run_speculative` is re-expressed on the fork: fork → run → read/`diff_state` →
  drop. The source is left entirely untouched, including its print output.
- `diff_state(program_id, source, fork)` compares committed state **by value**
  (never by heap id — ids are non-deterministic across runs).
- This backs `capture_draw_commands` / `DiagramCaptureDrawCommands` /
  `DiagramScreenshot`: they run a **fork of one frame** and discard it, so
  inspecting a canvas never perturbs it.

Full design & the `ExecutionContext` machinery:
[dev/speculative-execution-plan.md](dev/speculative-execution-plan.md). This is
the substrate for "try an edit, see the effect, keep or discard" — the execution
half of direct-manipulation and goal-driven editing.

---

## Layer 4 — Goal-driven / differentiable edits (partial)

The headline direction — *specify a target output, get suggested source-value
changes* — is only **partially built**.

**Shipped:** forward-mode automatic differentiation via dual numbers.
[`rust/src/builtins/autodiff.rs`](../rust/src/builtins/autodiff.rs) registers
`dual(value, derivative)`, `value_of(x)`, `deriv_of(x)`; arithmetic threads
derivatives through [`backend/ops.rs`](../rust/src/backend/ops.rs) (`dual_arith`),
and `sin`/`cos`/`tan`/`sqrt`/`abs` propagate (`exp`/`log` drop the derivative —
a known gap). This lets a program compute *how sensitive an output is to an
input* in the forward direction.

**Not yet built (aspirational):**

- `grad()` / `optimize()` — the stdlib functions used in
  [docs/examples/aspirational/gradient_descent.ptl](examples/aspirational/gradient_descent.ptl)
  **do not exist**; that file is a target sketch, not runnable.
- **Reverse-mode AD / back-propagation** — no gradient/adjoint code exists. This
  is the centerpiece needed to go from "a target result" to "which source
  constants to change."
- **Drag-to-edit** — drag an output on the canvas → back-prop to the influencing
  source constants → project the candidate slice → live-edit the numbers with
  state preserved. This is the Phase 1 demo in [dev/goals.md](dev/goals.md) and
  the composition of all four layers above.

---

## Capability matrix

| Capability | Read | Write | Where |
|---|---|---|---|
| Inspect source (tokens/AST/CST) | ✅ | — | `show-tokens/ast`, `rewrite::parse_ast` |
| Rewrite source, formatting-preserved | — | ✅ | `goal_based_editing.rs` (goals) over `rewrite.rs` primitives |
| Normalize source (verified) | — | ✅ | `petal lint --fix` |
| Inspect IR graph | ✅ | — | `show-ir`, `program_analysis.rs`, MCP `ShowIR` |
| Construct/transform IR as data | — | ✅ | `run --ir`, `Program::from_json`, `calc-to-ir.ts` |
| Provenance / dependents / slice | ✅ | — | `show-provenance/dependents/slice` |
| Hot reload (state-preserving) | — | ✅ | `transfer_state`, SDL watcher |
| Full reload (state reset) | — | ✅ | `petal.load` (web-canvas) |
| Mutate one live state var | — | ✅ | `set_state` / `DiagramSetState` |
| Inject input / bindings | — | ✅ | `input`, `set_binding_for` |
| Speculative variant run | ✅ | (forked) | `fork_execution`, `run_speculative`, `diff_state` |
| Forward-mode sensitivity | ✅ | — | `dual`/`deriv_of` |
| Goal-driven source suggestion | — | 🔭 | not built (reverse-mode AD) |

---

## Gaps & directions this work will explore

Grounded in the catalogue above and [dev/goals.md](dev/goals.md):

1. **Richer hot reload.** Today reconciliation drops state on rename/move
   (name-hash keying, §3a) and integrations re-run the whole program each frame
   (no incremental graph diffing — `goals.md` Goal 4). Explore
   structural-correspondence migration that survives renames, and incremental
   recompute of only affected terms.

2. **Programming by direct manipulation.** The pieces exist in isolation — a
   formatting-preserving source rewriter (§1b), a walkable dataflow graph with
   provenance/slice (§2c), and safe speculative runs (§3d). What's missing is the
   **bidirectional link**: map a manipulated *output* back to the *source node*
   that produced it and edit it in place. This is the "projectional / bidirectional
   editing" pillar (`goals.md` Goal 3) plus the drag-to-edit demo (§4).

3. **Goal-driven edits.** Requires **reverse-mode AD** (§4) to turn a target
   result into gradients over source constants, the **slice** machinery (§2c) to
   scope which constants are editable, and the **verified-edit discipline** of
   `lint --fix` (§1c) and speculative `diff_state` (§3d) to apply changes safely.

4. **A first-class structured-edit API.** `goal_based_editing.rs` seeds this: a
   declarative `Goal` vocabulary over the `rewrite.rs` CST primitives, currently
   `ShouldCall`. `program_analysis.rs` is still read-only. A unified "state a
   goal → query the graph → propose an edit → verify (IR-equivalence or
   speculative diff) → write back through the CST" pipeline — growing the `Goal`
   enum toward richer and eventually graph-derived intents — would be the shared
   substrate for all three directions above.
