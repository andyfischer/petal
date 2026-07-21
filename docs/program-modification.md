# Programmatic Program Modification

Documents the ways Petal code can be changed programmatically — for tools,
agents, and embedders.

## Two modes of editing

A Petal program can be modified in two fundamentally different situations:

| Mode | When | Modify by | Preserves |
|---|---|---|---|
| **Static editing** | no running app — you have the `.ptl` source | text splice / tree splice / goal-based edit / lint | comments & formatting (CST path) |
| **Live editing** | a running app whose `state` must survive the change | hot reload / state-set / input / speculative fork | live state across a swap |

Because Petal is a dataflow language with no register-mutation primitive
(rebindings lower to pure `Phi` joins — see
[debugging-visibility.md](dev/debugging-visibility.md)), both modes are unusually
legible: runtime state is keyed structurally so it can migrate across an edit,
and the source can be rewritten through a lossless tree without reformatting.

> A third surface — constructing or transforming a program *as IR data* — exists
> but is **experimental and unfinished**; it is documented separately in
> [dev/experimental-ir-based-editing.md](dev/experimental-ir-based-editing.md).

---

## Static editing (no running app)

Rewrites the `.ptl` source text. Nothing needs to be running; the output is new
source you can write back to disk, hand to a compiler, or diff.

### Formatting-preserving tree splices (`rust/src/rewrite.rs`)

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

### Goal-based editing (`rust/src/goal_based_editing.rs`)

The primitives above are wrapped by a **declarative, goal-based** API. Rather
than "replace this span," a caller states *goals* — properties the edited source
should satisfy — and the module decides whether to insert or update in place.

| Item | Purpose |
|---|---|
| `Goal::should_call(function, params)` | Goal: source should contain a top-level `function(params...)` call. `params` are structured `Arg` values (`&str`/`i64`/`f64`/`bool` coerce in via `From`). |
| `Arg` | A structured argument: `Str`/`Int`/`Float`/`Bool`/`Nil`, plus composites `List`/`Record`/`Call`. Every variant renders to well-formed Petal (strings are quoted and escaped); there is no verbatim/raw-source variant. |
| `modify_source_with_goals(source, goals)` | Apply a list of goals in order; `Ok(String)` is the rewritten source, `Err(GoalError)` a typed failure. |

`ShouldCall` updates the first existing top-level call to `function` (replacing
its argument list, layout-flexibly) or appends the call if absent — the shape an
app's user-config script wants for a "set the color scheme" menu action. Goals
compose (apply several in one pass, later goals see earlier insertions), and
`Goal` is the extension point for richer intents (ensure an import, remove a
call, set a field). This is the seam a broader structured-edit and goal-driven
API grows from. **Usage guide:** [goal-based-editing.md](goal-based-editing.md).

### `petal lint --fix` (normalize source in place)

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

### Future: goal-driven source *suggestion*

*Specify a target output, get suggested source-value changes* is not built yet.
It depends on **reverse-mode AD**, which does not exist — Petal ships only
forward-mode sensitivity (dual numbers:
[`builtins/autodiff.rs`](../rust/src/builtins/autodiff.rs) registers
`dual`/`value_of`/`deriv_of`, threaded through
[`backend/ops.rs`](../rust/src/backend/ops.rs)), which answers "how sensitive is
this output to that input" but not "which inputs should change to hit a target."
See [dev/goals.md](dev/goals.md) for the direction.

---

## Live editing (running program, state preserved)

Modifies a program (or its inputs) *while it runs*, keeping the parts of live
`state` that still make sense. This is what hot reload and the debug protocol do.

### State-preserving hot reload (`transfer_state`)

The core primitive is **`Env::transfer_state`**
([`rust/src/transfer_state.rs`](../rust/src/transfer_state.rs)): it reshapes a
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
([`program.rs`](../rust/src/program.rs)) is a **hash of the state variable's
name**, computed at compile time by `Compiler::state_key_for`
([`compiler/mod.rs`](../rust/src/compiler/mod.rs)) — bare name for entry-file
`state`, qualified `"module::name"` for module state. The runtime key is
`RuntimeStateKey { base: StateKey, loop_indices }`
([`stack.rs`](../rust/src/stack.rs)); `transfer_state` matches on `base` only.
Migration semantics (pinned by tests in `transfer_state.rs`):

- **Addition** — new key absent in old state → `StateInit` runs, default-initializes.
- **Removal** — old key absent in new program → dropped in the `retain`.
- **Modification / reordering** — value preserved by matching `base`, regardless of declaration order.

A separate per-run GC (`Stack.touched_state_keys` +
`sweep_untouched_state`, [`stack.rs`](../rust/src/stack.rs)) reclaims state whose
declaration wasn't visited on a run (e.g. per-iteration state for a removed list
item).

**Known limitation:** renaming a `state` var — or moving/renaming its module —
changes its `StateKey` and **drops the value** (it reads as remove + add).

### Hosts that trigger reload

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

### Point-mutating live state, bindings, and input

Over the agent JSON protocol and MCP (see
[debug-protocol.md](dev/debug-protocol.md),
[mcp-server.md](dev/mcp-server.md),
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

### Speculative execution — safe experimental modification

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

The `ExecutionContext` machinery lives in
[`rust/src/execution_context.rs`](../rust/src/execution_context.rs) and
[`rust/src/env/fork.rs`](../rust/src/env/fork.rs). This is the substrate for "try
an edit, see the effect, keep or discard" — the execution half of
direct-manipulation and goal-driven editing.

---

## Capability matrix

| Capability | Read | Write | Where |
|---|---|---|---|
| Inspect source (tokens/AST/CST) | ✅ | — | `show-tokens/ast`, `rewrite::parse_ast` |
| Rewrite source, formatting-preserved | — | ✅ | `goal_based_editing.rs` (goals) over `rewrite.rs` primitives |
| Normalize source (verified) | — | ✅ | `petal lint --fix` |
| Hot reload (state-preserving) | — | ✅ | `transfer_state`, SDL watcher |
| Full reload (state reset) | — | ✅ | `petal.load` (web-canvas) |
| Mutate one live state var | — | ✅ | `set_state` / `DiagramSetState` |
| Inject input / bindings | — | ✅ | `input`, `set_binding_for` |
| Speculative variant run | ✅ | (forked) | `fork_execution`, `run_speculative`, `diff_state` |
| Forward-mode sensitivity | ✅ | — | `dual`/`deriv_of` |
| Goal-driven source suggestion | — | not yet available | needs reverse-mode AD; see [goals.md](dev/goals.md) |
| Construct/transform IR as data | — | ✅ (experimental) | [dev/experimental-ir-based-editing.md](dev/experimental-ir-based-editing.md) |

---

## Known limitations

- **Hot-reload reconciliation is by name.** Renaming a `state` variable (or its
  module) changes its key and drops the value (see Live editing).
- **Reverse-mode AD does not exist** — only forward-mode sensitivity, so "given a
  target output, which inputs should change" is not answerable today.
- **IR editing is experimental** — the graph-query passes are read-only and there
  is no in-place IR rewrite API; see
  [dev/experimental-ir-based-editing.md](dev/experimental-ir-based-editing.md).

For where these are headed, see [goals.md](dev/goals.md).
