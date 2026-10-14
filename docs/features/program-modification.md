# Programmatic Program Modification

The ways Petal code can be changed by a program: tools, agents, and embedders.
This page is the survey. Each mechanism has a short description here and a
link to the page that describes it in full.

## Two modes of editing

| Mode | When | Modify by | Preserves |
|---|---|---|---|
| **Static editing** | no running app; you have the `.ptl` source | text splice / tree splice / goal-based edit / CLI rewrite | comments and formatting |
| **Live editing** | a running app whose `state` must survive the change | hot reload / config-set / state-set / input / speculative fork | live state across a swap |

Both are simpler than in most languages because Petal is a dataflow language:
rebindings lower to pure `Phi` joins rather than register writes (see
[debugging-visibility.md](../dev/debugging-visibility.md)). Runtime state is
identified by names, never by position in the source, so it can migrate
across an edit ([inline-state.md](inline-state.md#state-and-hot-reload)).
Source can be rewritten through a lossless tree without reformatting.

> A third surface, constructing or transforming a program *as IR data*, exists
> but is **experimental and unfinished**. See
> [../dev/experimental-ir-based-editing.md](../dev/experimental-ir-based-editing.md).

---

## Static editing (no running app)

Rewrites the `.ptl` source text. The output is new source you can write to
disk, compile, or diff.

### Formatting-preserving tree splices (`core/src/rewrite.rs`)

The AST is lossy: it carries spans but drops comments and whitespace, so
rewriting through it would reformat the whole file. Source edits instead go
through the **lossless green tree** (the CST, [`core/src/cst/`](../../core/src/cst/)):
find the node covering the construct, splice in a subtree parsed from the
replacement snippet, keep the old node's surrounding trivia, and re-emit.
Everything outside the replaced node is untouched. The invariant
`build_lossless(src).text() == src` holds for every source.

Primitives in [`core/src/rewrite.rs`](../../core/src/rewrite.rs):

| Function | Purpose |
|---|---|
| `parse_ast(source)` | Parse to `(green tree, top-level Stmts)` for inspection and rewrite |
| `find_call(stmts, name)` | Span of the first top-level `name(...)` statement call |
| `find_binding(stmts, name)` | Span of the *value* of the last top-level `let name = …` / `name = …` binding |
| `find_binding_path(stmts, name, path)` | Span of one value *inside* a binding: `.field` steps into record literals, `[index]` steps into list literals and call arguments (`parse_binding_path("POST.effects[2].amount")`) |
| `splice_node(tree, span, replacement)` | Replace a node, preserving surrounding trivia |
| `splice(source, span, replacement)` | String-level fallback (char offsets, multi-byte safe) when the replacement is not a single parseable expression |
| `apply_splices(chars, splices)` | Apply several string-level replacements at once |

These treat the program as a live document the user is also editing: a tool
can rewrite one `layout(...)` call, or one number inside a commented
multi-line record, and write the result back without touching the rest of
the file. Most callers use one of the two layers above them, described next.

### Goal-based editing (`core/src/goal_based_editing.rs`)

A declarative layer over those primitives. Instead of "replace this span" the
caller states **goals**, properties the edited source should satisfy
(`Goal::should_call`, `should_set_value`, the path goals `should_set_path` /
`should_insert` / `should_append` / `should_remove`), and
`modify_source_with_goals(source, goals)` decides whether to insert or update
in place. A goal that already holds writes nothing. The API is in
[goal-based-editing.md](goal-based-editing.md).

The value and path goals edit literals through
[`core/src/literal_edit.rs`](../../core/src/literal_edit.rs): only the text
that differs is written, and new text is formatted like what surrounds it.
The rules and guarantees are in
[source-preservation.md](source-preservation.md). C and C++ hosts reach the
same edits as `pb_source_set_value`, `pb_source_insert`, `pb_source_append`
and `pb_source_remove`
([../usage/embedding-c.md](../usage/embedding-c.md#editing-source-a-script-as-a-config-file)).

### Reading values back (`core/src/static_value.rs`)

The read counterpart, with no `Env`, heap, or side effects:
`get_static_value(source, name)`, `static_values(source)` and
`static_bindings(source)` read top-level bindings out of source text as
`StaticValue`, the same type the goals write, so a value round-trips. What
counts as static, the errors, and the `StaticValue` variants are in
[../usage/config-files.md](../usage/config-files.md).

### Goal-based edits to a *running* program's output

"This value the program emitted should have been X; what do I change?" is
answered by `petal::direct_manipulation` and the `petal propose-edit` command.
Run with emit tracing, address an emitted value by channel and index, state the
new value for one of the producing call's arguments, and get back concrete
source edits (one per variable that can be moved to satisfy the goal). The
edit is text you apply with `apply_edits`, then re-run. See
[direct-manipulation.md](direct-manipulation.md#the-goal-based-protocol).

This inverts `+ - * /` and negation against the values the traced run
actually saw. It is not a general solver: an argument that flows through a
call, a field read or a comparison gets no proposal. There is no reverse-mode
AD; Petal ships forward-mode sensitivity only (`dual`/`value_of`/`deriv_of`
from `import autodiff`, see [differentiation.md](differentiation.md)), and
the goal solver does not use it. See the [README](../../README.md#goals) for the project goals.

### CLI commands that rewrite source

Each of these writes `.ptl` files in place, and each is described in
[../CLI.md](../CLI.md):

| Command | What it rewrites |
|---|---|
| [`petal fmt`](../CLI.md#fmt--canonical-layout) | Layout only: the canonical indentation and spacing. |
| [`petal lint --fix`](../CLI.md#lint--rules-with-fixes) | Code that has a better spelling, by named rule (`prefer-let`, `no-redundant-cast`, `prefer-match`, `prefer-repeat`, `prefer-compound-assign`, `prefer-pub`, and the opt-in `state-as-constant`). A fix is written only if a file that compiled still compiles; `--verify[=ir\|strict]` also compiles both sides and compares their IR, and refuses a rewrite it cannot prove. |
| [`petal suggest --apply`](../CLI.md#suggest--suggest-safe-refactors-for-a-file) | Refactors that are proved not to change behaviour (type annotations, return types, named arguments). |
| [`petal apply-change`](../CLI.md#apply-change--refactors-that-change-behaviour) | Refactors that do change behaviour, such as `convert-to-var`. |
| [`petal fix-imports`](../CLI.md#fix-imports--migrate-imports) | Imports, for the current module system. |

The library entry point for lint is `lint::lint_source`
([`core/src/lint/`](../../core/src/lint/)).

---

## Live editing (running program, state preserved)

Modifies a program, or its inputs, while it runs, keeping the parts of live
`state` that still make sense. This is what hot reload and the debug protocol
do.

### State-preserving hot reload (`transfer_state`)

A host calls `Env::reload_program(stack, source, path)` when a file changes.
It diffs the new text against what is running and does only the work the edit
calls for: a layout edit moves source positions, an edit of literal values is
written into the running program, and anything else is recompiled and swapped
in with `Env::transfer_state`, which keeps every `state` slot whose
declaration still exists. Static values the edit cannot have changed are
kept as well. The outcomes, what each leaves alone, the cost, the hosts that
reload and the limits are in [hot-reload.md](hot-reload.md); the
`transfer_state` primitive, the edits that lose state, and class instances
kept across a reload are under
[State across a reload](hot-reload.md#state-across-a-reload) there. Which
edits keep a slot, as a script's author sees it, is in
[inline-state.md](inline-state.md#state-and-hot-reload).

### Setting one config value in a running program

`Env::set_config_value(stack, file, "POST.effects[2].amount", &value)` sets
one literal of a top-level binding in the running program without writing
the file, for a drag that should not save sixty times a second. The result
is exactly what reloading the edited file would give, and
`Env::program_source` returns the edited text to write when the drag ends.
See [hot-reload.md](hot-reload.md#setting-a-value-without-writing-the-file).

### Point-mutating live state, bindings, and input

Over the agent JSON protocol and MCP (see
[debug-protocol.md](../dev/debug-protocol.md), [mcp-server.md](../dev/mcp-server.md),
and the [petal-sdl agent protocol](../../integrations/petal-desktop-sdl/docs/agent-protocol.md)):

| Surface | Effect |
|---|---|
| `set_state {name, value}` / `DiagramSetState` | Set one live **top-level** state var by name (`Env::set_state_from_json`) |
| `input {keys_down, mouse, text}` / `DiagramInput` | Inject keyboard/mouse/text into the next frame |
| `pause` / `resume` / `step {n}` | Control the frame loop (fixed `dt = 1/60`) |
| host bindings (`set_binding_for`) | Change host→script uniform inputs |

`set_state` addresses top-level `state` only, module-qualified for module
state. A slot reached through a call path has no name to set it by
([State from the host side](inline-state.md#state-from-the-host-side)).

These change **runtime state or inputs**, not the program. There is no
over-the-wire source-swap command; a reload is triggered by the host, from
its file watcher or its own call. The diagram-canvas command set is `pause,
resume, step, state, set_state, capture_draw_commands, input, screenshot,
pending_report`
([`debug.ts`](../../examples/custom-apps/diagram-canvas/src/debug.ts)), and
its `Diagram*` MCP tools are in `tools/petal-diagram-mcp.ts`.

### Speculative execution

Petal can **fork a running execution**, run the fork with different inputs, and
compare, without disturbing the original: `Env::fork_execution`,
`run_speculative`, `diff_state` and `drop_fork`. This backs
`capture_draw_commands` / `DiagramCaptureDrawCommands` / `DiagramScreenshot`,
which run a fork of one frame and discard it, so inspecting a canvas never
perturbs it. The operations and what a fork does and does not capture are in
[time-travel.md](time-travel.md#what-the-runtime-provides).

### Rewind, and replaying history through an edit

A fork is also a checkpoint: `Env::restore_execution(live, fork)` puts the
live execution back at it, reshaped onto whatever program is loaded now.
`petal-sdl` forks after every frame, so a running script can be frozen,
scrubbed, rewound, and have its recorded input replayed through edited code.
See [time-travel.md](time-travel.md).

---

## Capability matrix

| Capability | Read | Write | Where |
|---|---|---|---|
| Inspect source (tokens/AST/CST) | yes | — | `petal show-tokens` / `show-ast`, `rewrite::parse_ast` |
| Inspect a function value from inside the program | yes | — | `fn_info`, `fn_ast` ([function-introspection.md](function-introspection.md)) |
| Read a config value without running | yes | — | `static_value::get_static_value` / `static_values` |
| Read a config file's bindings, comments and unreadable names | yes | — | `static_value::static_bindings`; `pb_source_bindings_json` |
| Set a config value (formatting-preserved) | — | yes | `Goal::should_set_value` |
| Set one value inside a config record or list | — | yes | `Goal::should_set_path`; `pb_source_set_value` in the C bridge |
| Insert or remove a list element or record field in place | — | yes | `Goal::should_insert` / `should_append` / `should_remove`; `pb_source_insert` / `_append` / `_remove` |
| Ensure a top-level call, formatting-preserved | — | yes | `Goal::should_call` |
| Rewrite a span of source, formatting-preserved | — | yes | `rewrite::splice_node` / `splice` |
| Reformat source | — | yes | `petal fmt` |
| Apply lint fixes (optionally verified) | — | yes | `petal lint --fix [--verify]` |
| Apply refactors | proposals | yes | `petal suggest [--apply]`, `petal apply-change` |
| Propose edits that change an emitted value | yes | proposals | `direct_manipulation::propose_edits` / `propose_edits_batch`, `petal propose-edit`, MCP `ProposeEdit` |
| Classify an edit without applying it | yes | — | `source_diff::diff_source`, `Env::diff_program` |
| Hot reload (state-preserving) | — | yes | `Env::reload_program` (`transfer_state` for the recompile); the SDL watcher, `pb_vm_reload`, Garden |
| Set one config value in a running program | — | yes | `Env::set_config_value`; `pb_vm_set_config` |
| Full reload (state reset) | — | yes | `petal.load` (diagram-canvas) |
| Mutate one live top-level state var | — | yes | `set_state` / `DiagramSetState` |
| Inject input / bindings | — | yes | `input`, `set_binding_for` |
| Speculative variant run | yes | (forked) | `fork_execution`, `run_speculative`, `diff_state` ([time-travel.md](time-travel.md)) |
| Rewind to a checkpoint; replay recorded input through an edit | yes | yes | `restore_execution`; `petal-sdl` timeline (`rewind`, `replay`, `trail`) ([time-travel.md](time-travel.md)) |
| Forward-mode sensitivity | yes | — | `dual`/`deriv_of` |
| Construct/transform IR as data | — | experimental | [../dev/experimental-ir-based-editing.md](../dev/experimental-ir-based-editing.md) |

---

## Known limitations

- **Hot-reload reconciliation is by declaration name and call path.** Renaming
  a `state` variable (or its function or module) drops the value; editing the
  call structure around a callsite orphans that callsite's state
  ([hot-reload.md](hot-reload.md#state-across-a-reload)).
- **A structural edit recompiles the whole program.** There is no
  per-function recompile ([hot-reload.md](hot-reload.md#limits)).
- **A live config-set cannot change a value's type or shape.**
  `set_config_value` returns `NeedsReload` for those; write the file and
  reload.
- **Only top-level state can be set by name.** A slot under a call path can
  be read in a state dump and not addressed.
- **Edit proposals only invert `+ - * /` and negation.** An argument that
  flows through a call, a field read or a comparison gets no proposal, and
  there is no reverse-mode AD.
- **Static reading and path edits see literals only.** A computed binding
  reads as `NotStatic`, and setting it replaces the whole right-hand side
  ([../usage/config-files.md](../usage/config-files.md#limits)).
- **IR editing is experimental.** The graph-query passes are read-only and
  there is no in-place IR rewrite API.

For the project goals behind these, see the [README](../../README.md#goals).
