# Programmatic Program Modification

The ways Petal code can be changed by a program: tools, agents, and embedders.

## Two modes of editing

| Mode | When | Modify by | Preserves |
|---|---|---|---|
| **Static editing** | no running app; you have the `.ptl` source | text splice / tree splice / goal-based edit / lint | comments and formatting |
| **Live editing** | a running app whose `state` must survive the change | hot reload / state-set / input / speculative fork | live state across a swap |

Both are simpler than in most languages because Petal is a dataflow language:
rebindings lower to pure `Phi` joins rather than register writes (see
[debugging-visibility.md](dev/debugging-visibility.md)). Runtime state is keyed
by name so it can migrate across an edit, and source can be rewritten through a
lossless tree without reformatting.

> A third surface, constructing or transforming a program *as IR data*, exists
> but is **experimental and unfinished**. See
> [dev/experimental-ir-based-editing.md](dev/experimental-ir-based-editing.md).

---

## Static editing (no running app)

Rewrites the `.ptl` source text. The output is new source you can write to
disk, compile, or diff.

### Formatting-preserving tree splices (`rust/src/rewrite.rs`)

The AST is lossy: it carries spans but drops comments and whitespace, so
rewriting through it would reformat the whole file. Source edits instead go
through the **lossless green tree** (the CST, [`rust/src/cst/`](../rust/src/cst/)):
find the node covering the construct, splice in a subtree parsed from the
replacement snippet, keep the old node's surrounding trivia, and re-emit.
Everything outside the replaced node is untouched. The invariant
`build_lossless(src).text() == src` holds for every source.

Primitives in [`rust/src/rewrite.rs`](../rust/src/rewrite.rs):

| Function | Purpose |
|---|---|
| `parse_ast(source)` | Parse to `(green tree, top-level Stmts)` for inspection and rewrite |
| `find_call(stmts, name)` | Span of the first top-level `name(...)` statement call |
| `find_binding(stmts, name)` | Span of the *value* of the last top-level `let name = …` / `name = …` binding |
| `splice_node(tree, span, replacement)` | Replace a node, preserving surrounding trivia |
| `splice(source, span, replacement)` | String-level fallback (char offsets, multi-byte safe) when the replacement is not a single parseable expression |

These treat the program as a live document the user is also editing: a tool
can rewrite one `layout(...)` call and write the result back without touching
the rest of the file.

### Goal-based editing (`rust/src/goal_based_editing.rs`)

A declarative layer over those primitives. Instead of "replace this span" the
caller states **goals**, properties the edited source should satisfy, and the
module decides whether to insert or update in place.

| Item | Purpose |
|---|---|
| `Goal::should_call(function, params)` | The source should contain a top-level `function(params...)` call. Updates the first existing call or appends one. |
| `Goal::should_set_value(name, value)` | Reading `name` out of the source should yield `value`. Replaces the last top-level binding's right-hand side, or inserts `let name = value`. A goal that already holds writes nothing. |
| `Goal::after(anchor)` / `Goal::before(anchor)` | Where a newly inserted statement goes. |
| `StaticValue` | A structured value (`Str`/`Int`/`Float`/`Bool`/`Nil`/`List`/`Record`/`Call`) that always renders to well-formed Petal. |
| `modify_source_with_goals(source, goals)` | Apply goals in order. `Ok(String)` is the rewritten source; `Err(GoalError)` a typed failure. |

Goals compose (later goals see earlier insertions), and `Goal` is the place to
add richer intents (ensure an import, remove a call, set a field).
**Usage guide:** [goal-based-editing.md](goal-based-editing.md).

### Reading values back (`rust/src/static_value.rs`)

The read counterpart, with no `Env`, heap, or side effects:

- `get_static_value(source, name)` returns the `StaticValue` bound to a
  top-level `name`.
- `static_values(source)` returns every statically readable binding.
- `static_bindings(source)` returns every binding, readable or not, with its
  reason, its right-hand side as written, and the comment above it.

Literals, lists, records, negation, references to names bound above, and
(unevaluated) calls are static. Arithmetic, interpolation, `if`/`match`, `fn`
and `state` are not, and report `StaticValueError::NotStatic`.

Reading and writing share `StaticValue`, so a value round-trips.
**Usage guide:** [config-files.md](config-files.md).

### Goal-based edits to a *running* program's output

"This value the program emitted should have been X; what do I change?" is
answered by `petal::direct_manipulation` and the `petal propose-edit` command.
Run with emit tracing, address an emitted value by channel and index, state the
new value for one of the producing call's arguments, and get back concrete
source edits (one per variable that can be moved to satisfy the goal). The
edit is text you apply with `apply_edits`, then re-run. See
[direct-manipulation.md](direct-manipulation.md).

This inverts arithmetic against the values the traced run actually saw. It is
not a general solver: an argument that flows through a call or a comparison
gets no proposal. There is still no reverse-mode AD; Petal ships forward-mode
sensitivity only (`dual`/`value_of`/`deriv_of` in
[`builtins/autodiff.rs`](../rust/src/builtins/autodiff.rs)). See
[dev/goals.md](dev/goals.md) for the direction.

### `petal lint` (normalize source in place)

[`rust/src/lint/`](../rust/src/lint/) is the one agent-usable command that
rewrites program source on disk. `lint_source` applies three passes:

- **Reindent** — token-driven 2-space re-indentation; only leading whitespace
  changes, so comments and content are safe by construction.
- **Identity casts** — drop `int(n)` where `n` is already an `int` (likewise
  `float`/`str`), based on the type checker's conservative inference.
- **`if`-chain to `match`** — rewrite an `if`/`elsif` chain over one subject
  and literal patterns into a `match`.

CLI: `petal lint [--fix | --check] [--verify[=ir|strict]] <file>`. With no
option it reports and exits 1 if a change is needed; `--fix` (or
`petal lint-fix`) writes; `--check` is the silent CI mode. `--verify` compiles
both sides and compares IR before writing, refusing a rewrite it cannot prove.
See [CLI.md](CLI.md#lint--normalize-source).

---

## Live editing (running program, state preserved)

Modifies a program, or its inputs, while it runs, keeping the parts of live
`state` that still make sense. This is what hot reload and the debug protocol
do.

### State-preserving hot reload (`transfer_state`)

The primitive is `Env::transfer_state`
([`rust/src/transfer_state.rs`](../rust/src/transfer_state.rs)): reshape a
running stack onto a freshly compiled `Program`, keeping matching state values.

```rust
let new_program = env.compile_program_at(pid, &source, &path)?;
let result = env.transfer_state(stack, new_program)?;  // TransferStateResult { state_preserved, state_dropped }
```

What it does: swap the program under the same `ProgramId` (invalidating cached
bytecode), clear closures and the stack's cached function table (they point
into old code and are recaptured on the next run), drop state keys the new
program no longer declares, and reset execution so the next `run` starts from
the new root.

**State matches by name, not position.** `StateKey` is a hash of the
declaration's full name path: module qualifier, enclosing function chain,
variable name (`"score"`, `"ui::scroll"`, `"ui::draw/row"`). The runtime key
adds the call path that reached the declaration
([one slot per call path](language-guide.md#one-slot-per-call-path));
`transfer_state` matches on the declaration id only. So:

- **Added** declaration: initialized fresh on the next run.
- **Removed** declaration: dropped.
- **Reordered or edited elsewhere**: value preserved.

A separate per-run sweep (`Stack::sweep_untouched_state`) drops any slot the
run did not touch, which is also what cleans up after a call-structure edit.

**Known limitations.** Both are the same event, an identity the edit changed,
and both read as remove + add:

- Renaming a `state` variable, or the function or module it sits in, changes
  its key and **drops the value**.
- Editing the call structure around a callsite (renaming the callee, inserting
  an earlier call to the same callee, extracting or inlining a helper) moves
  every slot below it to a fresh path. The reader sees a freshly initialized
  slot and the orphan is swept after the next run.

### Class instances across a reload

A class instance is an ordinary record carrying an interned class *name*, not
a pointer into the program that built it. A preserved instance keeps the fields
it was constructed with and `type()` keeps reporting its label even if the
class is gone. Petal does **not** migrate it: a field the edit adds is not
invented on an older instance.

Method calls on such a value resolve against the new code:

- **Pinning.** Where the compiler can tell the receiver's class (a constructor
  call, a `let` bound to one, or a class annotation), the call is bound to
  `fn Class.method` at compile time.
- **Stale-label fallback.** Otherwise the call dispatches on the label, but
  also carries the class its declaration named, which is consulted only when
  the label names no class in the running program.

See [Classes & Methods](language-guide.md#when-the-call-is-resolved-at-compile-time)
and `rust/tests/class_live_edit.rs`.

### Hosts that trigger reload

- **Native SDL file watcher** (all `petal-sdl` hosts) —
  [`watcher.rs`](../integrations/petal-desktop-sdl/src/watcher.rs) watches the
  entry script's directory plus every imported module's directory (from
  `env.module_manifest`), so editing an imported `palette.ptl` reloads its
  importer. On change: `compile_program_at`, then `transfer_state`. Compile
  errors are non-fatal; the old program keeps running. On by default,
  `--no-hot-reload` disables.
- **Browser live editor (diagram-canvas)** — a CodeMirror widget
  ([`editor.ts`](../examples/custom-integrations/diagram-canvas/src/editor.ts))
  with a debounced recompile. This is a **full reload, not state-preserving**:
  it calls `petal.load(source)`, which recreates the stack (state resets,
  frame count returns to 0). Browser UI only; not exposed over the debug
  protocol or MCP.

### Point-mutating live state, bindings, and input

Over the agent JSON protocol and MCP (see
[debug-protocol.md](dev/debug-protocol.md), [mcp-server.md](dev/mcp-server.md),
and the [petal-sdl agent protocol](../integrations/petal-desktop-sdl/docs/agent-protocol.md)):

| Surface | Effect |
|---|---|
| `set_state {name, value}` / `DiagramSetState` | Set one live **top-level** state var by name (`Env::set_state_from_json`) |
| `input {keys_down, mouse, text}` / `DiagramInput` | Inject keyboard/mouse/text into the next frame |
| `pause` / `resume` / `step {n}` | Control the frame loop (fixed `dt = 1/60`) |
| host bindings (`set_binding_for`) | Change host→script uniform inputs |

`set_state` addresses declarations on the empty call path: top-level `state`,
module-qualified for module state. A slot reached through a call path shows up
in a `state` dump under its path (`[3]/row/hovered`) and has no name to set it
by; drive it from a top-level cell instead.

These change **runtime state or inputs**, not the program. There is no
over-the-wire source-swap command; reload is file-watcher-driven only. The
diagram-canvas command set is `pause, resume, step, state, set_state,
capture_draw_commands, input, screenshot, pending_report`
([`debug.ts`](../examples/custom-integrations/diagram-canvas/src/debug.ts)).

### Speculative execution

Petal can **fork a running execution**, run the fork with different inputs, and
compare, without disturbing the original. The heap is immutable by
construction, so a fork shares no mutable state with its source.

- `Env::fork_execution(src)` deep-clones the `ExecutionContext` (heap,
  closures, bindings, counters) and gives the fork fresh output buffers.
- `run_speculative(fork)` runs it; `diff_state(program_id, source, fork)`
  compares committed state **by value**; `drop_fork` discards it.
- This backs `capture_draw_commands` / `DiagramCaptureDrawCommands` /
  `DiagramScreenshot`: they run a fork of one frame and discard it, so
  inspecting a canvas never perturbs it.

Code: [`execution_context.rs`](../rust/src/execution_context.rs) and
[`env/fork.rs`](../rust/src/env/fork.rs).

---

## Capability matrix

| Capability | Read | Write | Where |
|---|---|---|---|
| Inspect source (tokens/AST/CST) | yes | — | `show-tokens/ast`, `rewrite::parse_ast` |
| Read a config value without running | yes | — | `static_value::get_static_value` / `static_values` |
| Read a config file's bindings, comments and unreadable names | yes | — | `static_value::static_bindings` |
| Set a config value (formatting-preserved) | — | yes | `Goal::should_set_value` |
| Rewrite source, formatting-preserved | — | yes | `goal_based_editing` over `rewrite.rs` |
| Normalize source (optionally verified) | — | yes | `petal lint --fix [--verify]` |
| Propose edits that change an emitted value | yes | proposals | `direct_manipulation::propose_edits`, `petal propose-edit` |
| Hot reload (state-preserving) | — | yes | `transfer_state`, SDL watcher |
| Full reload (state reset) | — | yes | `petal.load` (web-canvas) |
| Mutate one live state var | — | yes | `set_state` / `DiagramSetState` |
| Inject input / bindings | — | yes | `input`, `set_binding_for` |
| Speculative variant run | yes | (forked) | `fork_execution`, `run_speculative`, `diff_state` |
| Forward-mode sensitivity | yes | — | `dual`/`deriv_of` |
| Construct/transform IR as data | — | experimental | [dev/experimental-ir-based-editing.md](dev/experimental-ir-based-editing.md) |

---

## Known limitations

- **Hot-reload reconciliation is by name.** Renaming a `state` variable (or
  its function or module) drops the value; editing the call structure around a
  callsite orphans that callsite's state.
- **Edit proposals only invert simple arithmetic.** An argument that flows
  through a call or comparison gets no proposal, and there is no reverse-mode
  AD.
- **IR editing is experimental.** The graph-query passes are read-only and
  there is no in-place IR rewrite API.

For where these are headed, see [goals.md](dev/goals.md).
