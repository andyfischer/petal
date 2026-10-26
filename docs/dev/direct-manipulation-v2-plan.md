# Direct Manipulation v2: plan

Status: design, 23 October 2026, revised after an adversarial review the
same day ([§10](#10-review-notes) lists what the review changed). M1 (the
speculative runner) landed the same day, M2 (discovery, Jacobian, hints) and
M3 (solver, ranking, public API) on 24 October, and M4 (Garden as M4b;
CLI, MCP and docs as M4a) the same day, and M5 (examples ladder and
complexity study, with six ranking and budget revisions) the same day,
followed by a code review of the whole change set that fixed five bugs,
and a final test pass over every suite, the study and the ecosystem the
same day. **Final: all milestones done.** Since 24 October 2026 v2 is the
default engine on the CLI and MCP (`--v1` / `engine: "v1"` reach the legacy
engine), and the hint names `tunable` and `rigid` are accepted. See
[Status](#9-status) at the end.

The goals are in [docs/tasks/direct-manipulation-v2.md](../tasks/direct-manipulation-v2.md).
Where this plan and an older doc disagree (the user page
[features/direct-manipulation.md](../features/direct-manipulation.md), the
module docs of `direct_manipulation.rs` and `provenance.rs`, the Garden drag
notes), this plan wins. The decisions it overrides are listed in
[§7](#7-decisions-this-plan-overrides). The hint builtins have their own
proposal, [proposals/dm-v2-hints.md](../proposals/dm-v2-hints.md).

In short: v2 treats the program as a black-box function from its source
literals to its observable outputs. It runs the program again, speculatively
and with no side effects, with one literal nudged at a time. It measures how
the output moves (a finite-difference Jacobian) and solves for the literal
changes that put the dragged output where the user wants it, using damped
least squares and re-running to verify. This works through function calls,
loops, records, trig and recursion, because none of them has to be understood,
only re-run. Static analysis, the hint builtins, `config let` and the client's
choices decide which literals may move and in which order of preference.

Contents

1. [What exists today](#1-what-exists-today)
2. [Architecture](#2-architecture)
3. [Runtime refactoring needed](#3-runtime-refactoring-needed)
4. [Surfaces](#4-surfaces)
5. [Milestones](#5-milestones)
6. [Interfaces between milestones](#6-interfaces-between-milestones)
7. [Decisions this plan overrides](#7-decisions-this-plan-overrides)
8. [Risks and open questions](#8-risks-and-open-questions)
9. [Status](#9-status)
10. [Review notes](#10-review-notes)

---

## 1. What exists today

Line numbers are as of commit `4d59549b`.

### 1.1 The v1 solver: `core/src/direct_manipulation.rs`

- The entry points are `propose_edits(program, goal, trace, policy)`
  (`direct_manipulation.rs:150`) and `propose_edits_batch` (`:259`), which
  resolves several goals together and keeps only proposals that can be chosen
  without colliding (`:279-318`). `apply_edits` (`:334`) splices chosen edits
  back to front.
- A goal is `ManipulationGoal { term, arg_index, new_value }` (`:103`), meaning
  "argument `arg_index` of call term `term` should evaluate to `new_value`".
  The call term is the user-file frame that `provenance::pick_frame` picked
  from an emit's chain.
- It can edit three things:
  - a literal written in the call (`ArgKind::Literal`);
  - a binding whose definition is a literal (`ArgKind::Binding`), editing the
    definition and setting `shared` when `reader_count > 1` (`:393`);
  - a computed argument, by `solve` (`:451`). `solve` walks `+ - * / neg`
    and `Copy`/`StateInit` and inverts each node (`invert_left`/`invert_right`,
    `:539`, `:556`) against the sibling's value. It reads that value from the
    trace with `current_value` (`:585`), which takes the **last** event for the
    term, so inside a loop it inverts against the wrong iteration.
- It refuses everything else: `_ => {}` at `:534`. A call, a field read,
  `%`, a comparison or a `Phi` ends the walk. The module doc says why
  (`:32-36`): a guess could rewrite the code to mean something else.
- Policy (`apply_policy`, `:213`): `VarPolicy::Static` drops a proposal and
  `Configurable` displaces the others. When the source declares any
  `config let`, every non-config **named** binding is treated as Static
  (`:218-235`). Call-site literals are never pinned.
- Spelling: `render_matching` (`:430`) keeps an int literal an int when the
  goal is whole. The fuller scalar style rules (padded decimals such as `3.50`)
  are in `literal_edit::float_like` (`literal_edit.rs:1888`) and are not used
  by DM.

### 1.2 Provenance: `core/src/provenance.rs`

- `pick_frame(program, chain, file)` (`provenance.rs:150`) chooses the
  innermost frame of an emit chain whose span is in `file`.
- `CallSite::resolve` (`:180`) maps a call term to its callee and argument
  sites. It skips the receiver of a `MethodCall` and the callable of a `Call`.
- `resolve_arg` classifies each argument as Literal, Binding (following
  `alias_target`, which crosses `Copy`/`StateInit` only and never `Phi`,
  `:265`) or Computed.
- `literal_at` (`:299`) unwraps `Neg(Constant)` and records `is_int` and
  `negated`.
- The module doc (`:21-24`) rejects "re-run the program" as unsound unless the
  program is deterministic. v2 re-runs, and makes the run deterministic
  (§2.1.4).

### 1.3 Tracing

- `TraceBuffer` (`trace.rs:38`) is a ring of `TraceEvent { sequence, term_id,
  inputs, result }`, holding values weakly (`:24-30`). `push` costs one bool
  check when disabled (`:93`). The queries are `last_for_term` (`:115`),
  `all_for_term` (`:120`) and `last_for_term_before(term, seq)` (`:130`). There
  is no term filter.
- The `TraceBuffer` belongs to the **`Env`** (`env/mod.rs:60`), not to a
  stack or a context. Every stack and every fork writes into the same ring,
  and Garden's panel trace (`panel.rs:184`, enabled at `:2226`) is this ring.
  A spec run that leaves tracing on would therefore fill the live trace with
  speculative events, and because events hold values weakly, a value read
  back after the run may already be `<collected>`. v2 does not capture
  through this buffer (R5, revised).
- Emit attribution: `ExecutionContext::trace_emit` and `emit_origins`
  (`execution_context.rs`, the `emit_origins` field and `EmitSite` struct)
  record one `EmitSite { chain }` per buffered emit, index-aligned with
  `output_buffers`. `Vm::emit_call_chain` (`backend/bytecode/vm/native.rs:31`)
  builds the chain.
- `petal run --trace-emits` builds the report in `emit_trace_report`
  (`cli/handlers.rs:424`), and `petal propose-edit` is `handle_propose_edit`
  (`cli/handlers.rs:573`). That handler runs once with emit tracing and term
  tracing on, picks the frame and calls `propose_edits_batch`.
- MCP: `TraceEmits` and `ProposeEdit` in `tools/petal-mcp.ts:194-240`
  (documented in `docs/dev/mcp-server.md:17`).

### 1.4 Speculative execution, forks and literal patching (the foundation for M1)

- `Env::fork_execution(src)` (`env/fork.rs:35`) gives a new stack and a new
  `ExecutionContext` holding a deep copy of the heap, closures, bindings,
  counters, `rng_state`, `noise_seed` and resources, with fresh output
  buffers. `restore_execution` (`:73`) is the inverse and `drop_fork` (`:19`)
  releases one.
- `Env::run_speculative(stack)` (`env/run.rs:567`) forks, turns echo off,
  resets, runs and drops the fork. It returns only the run's value, so its
  emits are lost. `capture_draw_commands` drives a fork by hand.
- Late-bound literals (`env/reload.rs:20-36`): each `Constant` term lowers to
  a `LoadConst` that reads the constant table on every execution. A value-only
  reload calls `ConstantTable::alloc_slot` (`constant_table.rs:89`), which
  gives the term an un-deduplicated slot of its own, retargets the one
  instruction (`retarget_constants`, `reload.rs:916`), and from then on writes
  the slot (`set_slot`, `:104`; patch loop at `reload.rs:490-510`). Literals
  are interned and deduplicated otherwise (`ConstantTable::intern`), so two
  `10`s share a `ConstantId`. Overriding by `ConstantId` without slotting
  first would move both.
- What a run derived from a literal (closures, captured function table, memo
  records, statics, the frame gate's verdict) is dropped by
  `transfer_state::transfer_stack_state` (`transfer_state.rs:31`) and by the
  loop in `restore_execution` (`fork.rs:109-119`). Static values are stamped
  with `constants.revision()` (`run.rs`, in `make_vm`:
  `stack.statics.sync(&bc.statics, program.constants.revision())`).
- Two dispatch loops read `LoadConst`: `vm/dispatch.rs:22` and
  `vm/fast.rs:138`. No pass folds constants or fuses a constant into another
  instruction (the only `LoadConst` producer is `lower.rs:349`), so a slot
  override reaches every use. The Recompile oracle (§2.1.8) keeps this true.
- `fork_execution` clones the `Stack` whole (`fork.rs:52`), **including** its
  statics, memo, captured functions and methods. A fork that runs with an
  override would load a static built from the old value. Hence R4.
- `ExecutionContext::output_buffers` and `emit_origins` are
  `std::collections::HashMap` with the default random hasher
  (`execution_context.rs:9,31,93`). Iterating channels gives a different order
  in every fork, so everything v2 builds from them iterates channels sorted by
  `SymbolId`. Petal's own `map` and record values are insertion-ordered with an
  Fx index (`dict.rs:27-32`, `record.rs`), so script-visible iteration order is
  deterministic.
- A value-only reload and `set_config_value` both refuse a change of type:
  `10` to `10.5` is `NeedsReload` (`env/reload.rs`, `set_config_value` doc).
  An edit that respells an int literal as a float is therefore a structural
  reload (state is still transferred by key), not a value-only one.
- v1's `apply_edits(source, edits)` (`direct_manipulation.rs:334`) takes one
  source string and ignores `SourceSpan::file`. Edits to an imported file
  would be spliced into the entry file at the wrong offsets.
- `docs/features/time-travel.md`, "What is captured and what is not"
  (`:297-333`): a fork does **not** isolate host-side state. A forked run
  calls the same natives again, so a native with a host side effect repeats
  it. This is the main safety gap v2 has to close.

### 1.5 Effects: `core/src/native_fn.rs`

- `NativeEffects { reads: InputClasses, probe, emits, effect, pending,
  constant }` (`native_fn.rs:129`) is required on every registration
  (`Env::register_native`, `env/mod.rs:1870`; `register_native_boxed`, `:1903`).
- `InputClasses` (`:65-92`): `POINTER`, `KEYBOARD`, `CLOCK`, `VIEWPORT`,
  `HOST_DATA`, `RESOURCES`, `RNG`, `BINDINGS`.
- `effect` covers both effects that stay inside the execution context
  (`print` into `ctx.output`, `noise_seed` into `ctx.noise_seed`, counters,
  resources, `expect`) and effects that reach the host (an embedder's
  `spawn_entity`, file writes, Garden mutations). The row does not
  distinguish the two. v2 needs it to (§3, R2).
- The core rows with `effect` are `print`, `noise_seed`, `__pending`,
  `__resolve`, `__reject`, `__declare_method` (writes `stack.methods`), the
  enum-data intrinsic, `freeze`, `frozen`, `expect`, `expect_eq` and
  `expect_near` (`builtins/mod.rs:174-555`). All of them touch only the
  context or the stack. The in-repo hosts also declare effects that a script's
  output depends on: petal-ui `ui_mark` (`hit.rs:378`), `create_canvas`,
  `draw_to`, `draw_to_screen` (`draw.rs:1390-1392`), and the `invalidate` and
  `mutate` stubs; garden-script `row`, `column` (`native_fns.rs:37-38`),
  `invalidate`, `mutate` and `panel_store`. Suppressing any of these as
  "escaping" would change what the program draws, so they have to be
  classified in M1, not in M4's ecosystem sweep.
- `call_native_fn` (`vm/native.rs:406-487`) is the single leaf every native
  call goes through, so it is the hook point for suppression, replay and arg
  capture.
- Handle classes (`handle.rs:35`) dispatch methods through host closures and
  have no effect row.
- The audits are `--effect-audit` (`effect_audit.rs`) and
  `tools/native-effect-audit.ts` (see `docs/dev/native-effects.md`).

### 1.6 Sources of nondeterminism

| Source | Where | How the fork treats it today | v2 plan |
|---|---|---|---|
| `random`, `random_int`, `choose` | `builtins/mod.rs:187,282,287`, `reads(RNG)`, xorshift in `ctx.rng_state` (`builtins/mod.rs:96-110`) | fork copies the state, so the stream is the same **until a perturbation changes how many draws come before**, after which every later draw shifts | replay by call site (§2.1.4) |
| `noise` | `builtins/mod.rs:273`, `PURE`, reads `ctx.noise_seed` | copied, deterministic | call through |
| `noise_seed` | `EFFECT`, writes the ctx | contained in the fork | call through (contained) |
| `time`, `dt`, `frame_count`, `mouse_*`, keys, `screen_*` | petal-ui `input.rs:606-642`, `probe(CLOCK/POINTER/...)`, read **bindings** | bindings are cloned into the fork, so they are frozen | call through, plus a clock-coupling check (§2.5) |
| initial seed | `initial_seed()` (`builtins/mod.rs:70`) reads the wall clock unless `PETAL_SEED` is set | not relevant once forked, **but** two CLI invocations (`run --trace-emits`, then `propose-edit`) see different random draws, so "emit 7" may name a different shape | the v2 CLI pins the seed (default 0, `--seed`, `PETAL_SEED`) and prints it |
| `HOST_DATA` / `RESOURCES` reads (`is_valid`, petal-query, fonts) | various | the same host state is visible to every run in one session | `HOST_DATA`: replay by (native, args) from the baseline (§2.1.3); `RESOURCES`: call through, caught by the double baseline (§2.1.5) |
| petal-ui host providers: `host_data`'s `DataProvider` (`host_data.rs:132`), the font provider behind `text_width` (`text.rs:565`), the clipboard (`clipboard.rs:63`) | thread-locals that the **host installs only around its own `env.run`** | a fork run by anyone else sees no provider: `host_data` answers nil, text measures differently | spec runs execute inside the host's run wrapper (`SpecHost`, R11) |
| pointer during the drag | bindings (`mouse_*`, `drag_active`, `hovered`) | a checkpoint taken mid-gesture holds the gesture's own pointer state | the checkpoint is the frame **before** mouse-down (§2.1.1) |
| channel order | `output_buffers` is a `std` `HashMap` | differs per fork | iterate sorted by `SymbolId` |
| handle methods | `call_handle_method` (`vm/native.rs:489`) | host state, shared | refuse unless the class is marked safe (§2.1.3) |
| state | `stack.state`, keyed per call path | cloned by the fork | frozen at the pre-frame checkpoint (§2.1.2) |
| `var` cells | heap | cloned by the fork | deterministic within one run |

### 1.7 Quantization at the native boundary

`draw_circle` and the other draw natives read their arguments with
`PetalCxt::get_int` (`native_fn.rs:984`), which truncates a float
(`f as i64`). See `int_args` at `petal-ui/src/draw.rs:1495` and
`native_draw_circle` at `:1830`. The **emitted** command is therefore
piecewise constant in every literal, and a finite difference over emitted
values with a small epsilon is zero. v2 observes the argument values **as
passed to the native, before conversion** (§2.1.3). This is the most
important practical finding of the survey.

The petal-ui prelude unpacks the overloads (`prelude/ui.ptl:326-340`:
`draw_circle(center, radius, c)` calls the positional native
`_native_circle`). So at the native boundary the arguments are uniform
(`cx, cy, radius, ...`) whichever overload the user wrote.

The draw natives live in the `petal-ui` crate, which the core `petal`
binary does not link: `make_env` (`cli/handlers.rs:1720`) is `Env::new()`
plus module paths. `draw_circle` does not exist under `petal propose-edit`
(v1's documented examples use `push_output` channels). petal-ui scripts run
headless under `petal-ui-run` (`core-libs/petal-ui/src/bin/petal-ui-run.rs`),
which already pins the clock, dt, seed, input scenario and host-data fixtures.
That binary is the right CLI host for DM on UI scripts (§4.2), and core tests
cannot call `draw_circle` (§5, M3).

### 1.8 Dataflow analysis: `core/src/program_analysis.rs`

`trace_provenance` (`:484`) is a backward slice over value edges. It stops at
cells and reports a `CellFrontier`. `trace_dependents` (`:571`) is the forward
may-slice and includes `CellMay` edges. `slice` is at `:723`. Both walks are
**intraprocedural**: a `Call` term's inputs are the callee value and the
arguments, so the callee's body (where the literals of a helper live) is not
reached. v2 needs an interprocedural over-approximation (§2.2).

### 1.9 Dual numbers

`Value::Dual { value, derivative }` (`value.rs:56`) is forward-mode, single
tangent. Arithmetic and the math builtins carry the rule. A native without a
rule raises "Expected float ... got dual" (`get_float`,
`native_fn.rs:996-1000`), and `get_int` rejects a dual too.
`docs/features/differentiation.md` has the table. Because there is only one
tangent, getting `k` derivatives takes `k` runs, the same order as forward
differences. The benefit is exactness, not fewer runs (§2.3.6).

### 1.10 `config let`

The `is_config` flag on `Term` (`program.rs:412`) is set by the compiler
(`compiler/stmt.rs:78`). It is allowed inside function bodies
(`docs/syntax/config-let.md`, "Where it is allowed"). It is used in 177 `.ptl`
files across the ecosystem (examples, garden, worlds-fair, petal-creative,
cheesecake). `Env::set_config_value` (`env/reload.rs:588`) sets a config
value live.

### 1.11 Garden

`PanelHost::propose_drag_edits(cmd_index, goals)`
(`garden/garden-script/src/panel.rs:2279`) is v1 with no policy. It refuses a
computed argument for any shape but the last one a looping call drew
(`:2304-2318`), takes the first proposal per goal, and returns
`DragOutcome::Edits`. `panel_trace::drag_handle` (`panel_trace.rs:230`) maps a
`PanelCmd` plus callee plus arity to the `(arg index, current value)` pairs
for x and y. The goal is "current value + drag delta", which is the right
shape for v2 as well. The panel keeps a per-term trace ring of 20k events
(`panel.rs:184`).

### 1.12 What v1 can and cannot do

It can: a literal in the call, a literal behind a binding chain, and
`+ - * /` chains with known siblings. Multi-goal batches are kept consistent.
`config let` is a default policy.

It cannot:
- see through any call, including the user's own helpers and `cos`/`sin`;
- see through fields, lists or records;
- solve a goal inside a loop for any iteration but the last;
- handle `%`, `min`/`max`, `clamp` or `lerp`;
- distribute one change across several parameters;
- measure collateral (it uses `reader_count`, a static count of readers, not
  what actually moves);
- verify that an edit works.

---

## 2. Architecture

```
            ┌────────────── propose_edits_v2 (M3) ───────────────┐
 goals ───▶ │ discover (M2) → screen (M2) → jacobian (M2) →      │ ──▶ ranked
 filter     │ solve+verify (M3) → round+verify (M3) → rank (M3)  │     alternatives
            └───────────────────────┬────────────────────────────┘
                                    │ N runs with literal overrides
                        ┌───────────▼───────────┐
                        │ SpecSession (M1)      │  fork of a pre-frame checkpoint,
                        │  - const overrides    │  effects gated, RNG replayed,
                        │  - native gate        │  emits + native args captured,
                        │  - capture/fingerprint│  control-flow fingerprinted
                        └───────────────────────┘
```

New code lives in `core/src/dm_v2/`:

| File | Milestone | Contents |
|---|---|---|
| `dm_v2/mod.rs` | M1 | re-exports, `DmError` |
| `dm_v2/spec.rs` | M1 | `SpecSession`, `SpecConfig`, `SpecRun`, `Overrides`, capture types |
| `dm_v2/observe.rs` | M1 | `Observable`, `EmitKey`, `ObsValue`, value extraction |
| `dm_v2/params.rs` | M2 | `Param`, static discovery, empirical screening |
| `dm_v2/hints.rs` | M2 | reading `tunable`/`rigid` from the IR |
| `dm_v2/jacobian.rs` | M2 | finite differences, column quality, optional dual path |
| `dm_v2/solve.rs` | M3 | damped least squares, verification loop, rounding |
| `dm_v2/rank.rs` | M3 | weights, alternatives, collateral |
| `dm_v2/api.rs` | M3 | `propose_edits_v2`, `DmRequest`, `DmResponse` |

The VM-side hooks (§3) go in `backend/bytecode/vm/spec.rs`, so the VM files
stay focused. v1 (`direct_manipulation.rs`) stays as it is. v2 reuses its
`SourceEdit` and `apply_edits`, and falls back to v1 for non-numeric goals
(strings, bools), which have no derivative.

### 2.1 (a) Speculative runner (M1)

#### 2.1.1 The session

A `SpecSession` is opened once per manipulation request (or per drag gesture)
and reused for every run that request needs.

1. **Checkpoint.** The caller hands in a stack that is *ready to run the frame
   being edited*: its inputs are bound and its state is as it was before the
   frame. Where that stack comes from depends on the host:
   - CLI and MCP: a fresh stack (`create_stack`) that has never run, with
     the RNG seed pinned (§1.6). For petal-ui scripts the host is
     `petal-ui-run`, and the checkpoint is its headless harness after
     `--frames N - 1` frames, with the frame's inputs bound.
   - Garden and other live hosts: a pre-run fork that the host takes after
     binding the frame's inputs and before `run` while DM is armed (R6:
     `Env::set_dm_checkpoint(stack, true)` keeps the last pre-run fork). This
     reuses the `petal-sdl` fork-per-frame pattern. The gesture uses the
     checkpoint of the frame **before** mouse-down. That frame's pointer
     bindings show no drag, and the gesture's own pointer state (`mouse_down`,
     `drag_active`, `hovered` over the dragged shape) must not leak into the
     runs that measure it. While a gesture is open, the host does not advance
     the panel's clock.
   - As an optimization, and the default when it applies: if the last run's
     `run_deps` says it left every `state` slot as it found it, the
     post-frame stack is equal to the pre-frame one for state and can be used
     directly. R6 then forks per frame only for programs that write state.

   `begin` forks the checkpoint into a stack **the session owns**, so R6
   refreshing (or dropping) the host's checkpoint during the gesture does not
   make the session `Stale`.
2. **Slotting.** The session gives each candidate literal term its own
   constant slot, the value-neutral half of the reload patch (R3). This is
   done once per term per program. It bumps `constants.revision`, which costs
   the live stack one statics rebuild. That cost is paid once, not per run.
3. **Baseline, twice.** The session runs the checkpoint twice with no
   overrides (§2.1.5) and keeps the first as `baseline`.
4. **Runs.** `run(overrides)` forks the checkpoint, drops the derived caches
   in the fork (statics, memo, closures, functions, R4), installs the native
   gate and the overrides, runs under the step budget, collects a `SpecRun`
   and drops the fork. The checkpoint and the live stack are never touched.
   The run happens inside the host's run wrapper (`SpecHost::with_run_env`,
   R11), which installs the same thread-local providers (host data, fonts,
   clipboard) that the host installs around a live frame. The `Env`'s trace,
   `observations` and profile are switched off for the run and restored after
   it, so the live trace ring never sees a speculative event.

Runs are sequential (the `Env` is `&mut`). Parallel runs would need `Env`
clones and are out of scope.

#### 2.1.2 State

`StateMode::Frozen` is the default: each run starts from the checkpoint's
state, so state behaves like an input. `StateMode::Fresh` runs from empty
state, so `state` initializers execute and their literals have an effect.
This is opt-in, because an edit to a state initializer does not show in the
running program after a hot reload: reload keeps state
(`env/reload.rs:30-36`). Parameters that only feed a `StateInit` are marked
`state_init: true`. They are excluded by default **only when the checkpoint
already holds that state slot**. On the CLI's fresh stack, state is empty, so
every run executes the initializer, and editing it is exactly what changes
the next `petal run`. There those literals are ordinary parameters.

**State-key misses.** State is keyed per call path, including loop index
parts and `for … key` keys. A perturbation that changes a key (a literal that
feeds a keyed loop's key, or the trip count of an enclosing loop) makes the
frozen state miss, and the slot re-initializes. No branch changes, so the
branch fingerprint alone would not notice. In spec mode the VM counts
`StateInit` executions per state term and folds the counts into the
fingerprint (R7), so such a run counts as a regime change.

A run's own `set` writes land in the fork and are discarded with it. Writes
to state are not observables in this plan (an `Observable::State` was a
stretch goal in the first draft and is cut, §10). They are what a
physics-ish program's next frame reads, which is the multi-frame problem of
§8.4.

#### 2.1.3 The native gate (effects suppressed)

In a speculative run, `call_native_fn` consults `SpecHooks` (R1) before
calling. The policy, by the native's row:

| Row | Policy | Why |
|---|---|---|
| `PURE` / `CONST` / `probe` reads of bindings | call | a function of args and frozen bindings |
| `emits` | call, and **capture the raw args** first | output stays in the fork's buffers; raw args avoid the int truncation of §1.7 |
| `reads(RNG)` | replay by call site (§2.1.4) | removes draw-order shift |
| `reads(HOST_DATA)` | baseline: call and record the answer by (native, args hash); perturbed runs: replay the recorded answer, and for unseen args answer `Nil` and count `new_host_keys` | a provider may start a fetch for a key it has not seen (petal-query, Garden's panel store). A perturbed page index must not reach the network |
| `reads(RESOURCES)` | call | resource state is forked with the context; the double baseline catches violations |
| `effect` and **contained** (new row facet, R2) | call | lands in the fork's ctx or stack and is discarded. Every core row with `effect` is contained by construction (§1.5); a test asserts it |
| `effect` and not contained (escaping) | **suppress**: return `Nil`, record a `SuppressedCall` | must not touch the host |
| `tunable` / `rigid` (§2.6) | `rigid`: replay its baseline value; `tunable`: identity | `rigid` is a feedback barrier |
| handle methods | call if the class is marked spec-safe (R2), else abort the run with `SpecError::Unsafe` | host objects, unknown effects |

Suppressing an escaping effect can change what the program computes (a
`spawn()` that now returns `Nil`). The baseline comparison in §2.1.5 catches
the case that matters: when the speculative baseline's observables differ
from the live frame's, the session fails with `BaselineMismatch` and names
the suppressed natives.

#### 2.1.4 Deterministic randomness

`RngMode::ReplayByCallSite` is the default. In the baseline run every
`random`/`random_int`/`choose` call draws from the forked stream as usual
(so the baseline reproduces the live frame exactly, since the checkpoint
fork copied `rng_state`). The **uniform draw** (the raw `u64` the native took
from the stream), not the native's result, is recorded under the key
`(hash(call chain incl. origin term), hash(loop path), occurrence index under
that chain)`. In perturbed runs the same key hands the native the recorded
uniform, and the native maps it with its *current* arguments. This is what a
real re-run does whenever the draw order is unchanged: `random_int(0, w)`
with a new `w` gives a different value from the same uniform. So collateral
through RNG arguments is measured honestly, and is not hidden behind a frozen
result. A key the
baseline never saw (the perturbation added loop iterations) gets a value
derived from `hash(key, session_seed)` through the native's own mapping, and
the run is flagged `new_random_keys > 0`. The effect is that random values
belong to *where* they are drawn, not *when*, so a perturbation that adds one
draw does not reshuffle every later shape.

`RngMode::ForkStream` (plain fork behaviour) is kept for comparison tests,
and for one more run per proposed alternative. Replay-by-call-site is a model
that is stable to perturb. The edited program will really draw sequentially.
So the final verification of each alternative (§2.4.5) also runs once under
`ForkStream`. When the two runs disagree, the alternative is flagged
`rng_order_changed` with the first emit that differs ("applying this
reshuffles the random shapes after line 12"). It stays verified, because the
goal was met in the stable model, but ranking penalizes it.

The user requirement "rand cannot receive feedback" is kept separate from
how the RNG is replayed. The draw itself is never a parameter. A literal
whose only path to the goal goes through an RNG native's arguments is marked
`through_rng` and is ineligible by default (§2.5.1). Its column is still
measured, for the candidate report and for collateral.

The same mechanism, keyed the same way, backs `rigid(e)` replay and any
future "replay this native" policy.

#### 2.1.5 Determinism check

- The session runs the baseline twice and compares every captured observable
  bit for bit, plus the fingerprint (§2.1.6). If they differ, the session
  fails with `SpecError::Nondeterministic { first_difference }`.
- When the caller passes the live frame's observed values (Garden passes the
  command list, the CLI's fresh stack has nothing to compare), the
  speculative baseline must match them within `1e-9` on the addressed goals.
  Otherwise the session fails with `BaselineMismatch { suppressed, ... }`.

#### 2.1.6 What a run captures

- **Emits**: for every buffered emit, the channel, the `EmitSite` chain, its
  `EmitKey`, the emitted value, and (for emitting natives) the **raw native
  arguments** converted to `ObsValue`. `EmitKey = { chain_hash, path_hash,
  occurrence }`. `path_hash` hashes the loop-index parts of the frame's state
  path (the `Index` parts `ForEachInit`/`ForEachNext` push), and `occurrence`
  counts earlier emits with the same chain and path in this run. The path
  part makes the key of shape 3 in loop B independent of loop A's trip count
  (the call chain alone has no iteration in it, `vm/native.rs:31-48`). A key
  is stable across runs whose control flow did not change, which is what
  lets one shape out of a loop be matched between runs. Callbacks driven by
  `map`/`each` have no `Index` part, so they fall back to `occurrence`.
- **Watched terms**: values of chosen `TermId`s, captured by `SpecHooks`
  itself (R5, revised) and converted to `ObsValue` **at record time**, each
  with a spec-local sequence number. They are not read from the Env-wide,
  weakly-holding `TraceBuffer` (§1.3). Emits record the same sequence at emit
  time. That maps a v1-style goal ("arg 2 of user frame T for emit N") to
  the last watched value of `arg_term` before `emit_seq`.
- **Fingerprint**: a hash over the sequence of (instruction index, taken)
  pairs for every conditional branch, plus loop trip counts, plus per-term
  `StateInit` counts (§2.1.2) (R7). It is computed only in spec mode. Equal
  fingerprints mean the same path was taken with the same state hits.
- Channels are iterated in `SymbolId` order wherever a `SpecRun` is built or
  compared (§1.4).
- **Loaded parameters**: the set of slotted constants a `LoadConst` read
  during the run (one bitset write, spec mode only). A parameter that was
  never loaded cannot affect anything.
- `steps`, `elapsed`, `error: Option<String>` (a runtime error or a
  budget overrun counts as a failed run, not a failed session),
  `suppressed: Vec<SuppressedCall>`, and `new_random_keys`.

#### 2.1.7 Budget

Each run uses `run_bounded` with `SpecConfig::max_steps` (default: ten times
the baseline's step count) and a wall-clock cap. A perturbation that makes a
`while` loop run away fails that run only. The step budget is
deterministic. The wall-clock cap is not, so a run it stops is reported as
`error: TimedOut`, distinct from `StepBudget`. Such a run is retried once,
and it never takes part in the double-baseline comparison.

#### 2.1.8 Correctness oracle

`OverrideStrategy::Recompile` (test-only): instead of slot overrides, splice
the literal into the source text, compile, and run a fresh fork. M1 tests
check that both strategies give bit-identical observables over the
examples. This guards the slotting/override path against constant folding or
caches that bake a value in. The two programs are compared by
`(channel, emit index)` and raw args, not by `EmitKey`, because a recompile
may renumber terms (a sign change turns `5` into `Neg(5)`), and the chain
hash is built from `TermId`s.

### 2.2 (b) Candidate parameter discovery (M2)

A **parameter** is a numeric literal term (`Constant` or `Neg(Constant)`)
with a source span. The candidates for a set of goals come in three stages,
from cheapest to most precise.

1. **Static superset** (`discover_params`). Backward-slice from each goal's
   anchor terms (the emitting native call's argument terms for `EmitArg`, the
   watched term for `Watch`), using `trace_provenance` plus four additions
   that make it interprocedural and conservative:
   - At a `Call` whose callee resolves statically to `MakeClosure(fid)`
     (through `alias_target`, as `callable_name` does), add the literals
     that `fid`'s body slices to from its `Return`s. Do this recursively,
     with each function visited once.
   - For a callback passed to `map`/`filter`/`reduce`/`sort`/`each`/
     `any`/`all`/`find`, include the callback's body.
   - A dynamic `Call` that does not resolve (a closure from a record field or
     a list), or a `MethodCall`: use the **observed call edges** of the
     baseline. In spec mode the VM records each `(call term → FunctionId)`
     pair it executes (R13, one hash-set insert per call), and discovery
     follows only those. Only if a call term has no observed edge (it never
     ran) does discovery fall back to `fn Class.name` name matching, as
     `EdgeKind::DispatchMay` does. The old fallback, "every function in the
     program", is gone. On a 19-big-sized program it would make the
     candidate set the whole program.
   - At a cell frontier (`var`) or a `StateRead` of a state written in the
     program, follow the writes' slices (the forward walk's `CellMay` edges).
     Mark the parameters reached this way `through_var` / `through_state`.
   - Stop at `rigid(...)` (§2.6), at the arguments of `reads(RNG)` natives
     (their output is replayed, so their arguments have no effect), and at
     natives declared no-feedback.
2. **Loaded filter.** Drop every static candidate the baseline run never
   loaded (§2.1.6).
3. **Eligibility before screening.** Apply the hard filters of §2.5.1 now,
   not after the Jacobian. Library literals (the bundled `ui` prelude alone
   has hundreds), hint-bound literals and structural literals are never
   perturbed unless the filter asks for them. They are still listed in the
   candidate report, with their reason and no column. Under an explicit
   `allow`, only the allowed set is screened.
4. **Proximity order and cap.** Order the survivors by static distance from
   the anchors: the number of call, definition and cell edges on the
   shortest slice path, where literals in the goal's own frame come first.
   A second, **focused** walk (§9, Canopy study fixes) follows only the part
   of each value a goal reads: the field into a record literal, the element
   into a list literal or a `for` variable's list, through calls, `append`,
   `concat`, `find`, `filter`, `sort` and the like; anything it does not
   model wants the whole value. A literal it reaches has
   `Param::focus = Some(distance)`. The order is the focused literals first
   (by focused distance), then the rest (`params::screen_order`).
   Screen at most `max_screen` (default 256). Truncation is reported
   (`screened: 256 of 1,930`), so a client can narrow with `Function`/`Line`
   selectors instead of paying for the whole program.
5. **Empirical screening** (`screen_params`) by adaptive group testing.
   Start with all `n` remaining candidates in one group and split in halves
   (the classic adaptive binary splitting). Perturb every member of a group
   in one run, each by a distinct, incommensurate relative step
   (`h_i * (1 + 0.137 * j)`, sign alternating). Int literals take a
   **type-preserving** step (`±1`, §2.3.4), so a group never errors merely
   because an index became a float. If no goal row moves (beyond `1e-12`
   relative), the whole group is irrelevant. Otherwise split it in half and
   recurse. With `k` relevant out of `n` this costs about
   `k * log2(n / k) + 1` runs (for `n = 256`, `k = 8`, about 41). A group
   whose run errors or changes the fingerprint is split as usual. Only a
   singleton that still errors is recorded `Errored`. Fingerprint-changing
   singletons are kept, because they are relevant. The focused literals are
   their own first group. A group that moves only goals that hold their
   value (`DmRequest::holds`, a `--by 0`; or a target equal to the
   baseline) or only the fingerprint is set aside and split after every
   group that moves a goal asking for a change (`ScreenResult::secondary`);
   what the budget leaves untested is `ScreenResult::unscreened`.

Literals are candidates wherever they are written: call-site arguments,
`let` / `config let` definitions, function bodies, default parameter
expressions, record and list literals, loop bounds. Default **eligibility**
is decided in §2.5. Discovery reports ineligible literals too, so a client
can show them and opt in.

Parameter identity is the `TermId`, which is valid for one program. Clients
across a reload use `ParamSelector::Span` or `Variable` (§2.7).

### 2.3 (c) Sensitivity: the Jacobian (M2)

Rows are the **goal rows** plus the **collateral rows**: every number in
every raw native argument of every emit in the baseline, read through
containers (a record's fields, a list's items, a list of records, a record in
a record, a variant's payload; at most 256 numbers per argument), capped at
8,192 rows, in which case only emits on the goals' channels are kept. Columns
are the screened parameters.

#### 2.3.1 Step size

`h_i = 1e-3 * max(|p_i|, 1)`. Raw native arguments are plain `f64` paths
with no quantization in the usual case, and a step of 1e-3 relative stays
far above float noise while keeping a pixel-space map close to linear.

#### 2.3.2 Central differences

Each column takes two runs, `p_i + h_i` and `p_i - h_i`:

`J[:, i] = (f(p + h e_i) - f(p - h e_i)) / (2 h_i)`.

`FdMode::Forward` (one run per column, using the baseline) is used when the
run budget is short.

#### 2.3.3 Column quality

| Quality | Detected by | Treatment |
|---|---|---|
| `Smooth` | both fingerprints equal the baseline's, and the forward and backward slopes agree within 10% (or both are tiny) | use |
| `Kink` | fingerprints equal, slopes disagree (`abs`, `min`/`max`, `clamp`) | use the slope on the side of the goal's direction |
| `BranchFlip` | one side's fingerprint differs | use the one-sided slope from the matching side; retry with `h/10` up to twice |
| `Step` | both sides differ, or the slope is 0 at `h` but not at `100h` | escalate `h` by ×10 up to `0.1 * max(\|p\|, 1)` and keep the secant; mark it discrete |
| `Zero` | no movement at any scale | drop |
| `Errored(msg)` | a perturbed run failed | drop and report (`10 // 4` with a float literal, list index by float) |
| `Discrete` | int literal (probed type-preserving, §2.3.4) | keep the integer secant (`±1`, escalating to `±2`, `±4`) |

#### 2.3.4 Int literals

Probes are **type-preserving** by default: an int literal `n` is probed
with `Int(n ± 1)`, escalating to `±2` and `±4` for a `Step`, and its column
is the integer secant (`Discrete`). A float probe `n ± h` is not safe. Int
`/` truncates (`numeric.rs:49`), so `10 / 4` is `2` but `10.001 / 4` is
`2.50025`. The value jumps by `0.5` at any `h`, with no error and no branch,
and central differences read that as a huge slope or a `Kink`. A float
probe also changes `type_name`, string formatting, and `==` against ints.
The float probe is tried only when the client allows respelling
(`allow_respell`, off by default) **and** the integer secant is non-zero. Its
column is used only if it agrees with the integer secant within 25%.
Otherwise the parameter stays integral. Final values are rounded (§2.4.4).

Big integer literals used as pixel coordinates (`draw_circle(100, ...)`) are
fine with `±1`: the secant is the slope.

#### 2.3.5 Cost

Two baseline runs, plus screening (`k * log2(n/k) + 1`), plus `2k` for
central differences, plus, **per verified alternative**, 1 to 6 LM
iterations, one rounding run and one `ForkStream` run (§2.1.4). Subset
selection (§2.5.3) predicts with the Jacobian at zero runs, and only the top
`max_alternatives + 2` subsets are solved and verified. A typical sketch
(`n ≤ 64`, `k ≤ 8`, 3 alternatives) needs about 25 + 16 + 3 × 4 ≈ 55 runs.
`RunBudget` enforces `max_runs` across all of it, so a request that runs out
returns the best verified alternative so far with `budget_exhausted`.

#### 2.3.5a Per-run cost on big programs

Each run deep-copies the heap (`fork`) and rebuilds every static value
(R4). The static values can be most of a dashboard's frame. The planned
optimization, after M3 and measured in M5: during the baseline, record per
static slot the set of params `LoadConst` read while building it (the
loaded bitset, scoped to the `StaticSkip`…`StaticStore` segment), and in a
run with overrides `O`, clear only the slots whose set meets `O`.

#### 2.3.6 Optional dual fast path (future work, not a milestone)

Review note: this was M2b. It is cut from the milestone plan. It saves at
most half the column runs, it needs a gate rule for every emitting native,
and it is not on the critical path to any user goal. The design stays here
for later.

For one column, override the slot with `Value::Dual(p_i, 1.0)` (R1 allows a
`Value` override, not only `f64`). Capture the raw native arguments' `deriv`
at the native boundary, and have the gate pass `value_of` stripped arguments
into the emitting native so it does not raise. If any native raises "no
derivative rule", fall back to finite differences for that column. One run
per column instead of two, and an exact answer. Multi-lane duals (several
tangents per value) would be the real win, but they need a change to
`Value`'s 16-byte layout (`value.rs:79`) and are not planned.

### 2.4 (d) Solver (M3)

#### 2.4.1 Problem

The unknowns are `Δp` over an allowed parameter subset `S`. Goals are
`g_j(p) = t_j` with tolerance `τ_j` and weight `w_j`. Collateral rows `c`
should stay put.

Minimize
`Σ_j w_j² (J_g Δp - r)_j² + λ_c Σ_c (J_c Δp)_c² + μ Σ_i (d_i Δp_i / s_i)²`

where:
- `r = t - g(p)`;
- `s_i = max(|p_i|, 1)` normalizes units;
- `d_i` is the parameter cost weight from the heuristics (§2.5);
- `λ_c` is the collateral weight (default `0.05` per row, divided by the
  number of collateral rows so a single dragged shape's other emits count
  together as about one goal);
- `μ` is the LM damping (initially `1e-3 * trace(JᵀJ)/k`).

This is a dense `k×k` normal-equation solve (`k ≤ 32`), done with a small
hand-written Cholesky with pivoting fallback (SVD-free, no new dependency).
With `μ → 0` and no collateral it is the minimum-norm solution.

#### 2.4.2 Iteration (Levenberg-Marquardt with re-run verification)

```
p ← baseline; J ← jacobian(p)
repeat up to max_iters (default 6):
    Δp ← argmin(above); clamp p+Δp to hint ranges (projected)
    run(p+Δp) → g'
    if fingerprint changed and goal error grew: μ ← μ*10, retry
    if |g'-t|_j ≤ τ_j for all j: accept
    if error decreased: p ← p+Δp, μ ← μ/3, Broyden rank-1 update of J
    else μ ← μ*10
    every 3rd iteration, or after a fingerprint change: recompute J at p
```

Multiple goals (the x and y of a drag, or several shapes) are just more
rows. Goals that conflict leave a least-squares compromise with
`verified: false` and a per-goal residual.

#### 2.4.3 Tolerance

The default `τ` is `0.5` for draw coordinates (the native truncates to whole
pixels), and `1e-6 * max(|t|, 1)` otherwise. The request can override both.
Because `get_int` truncates toward zero, a client that wants the drawn pixel
`P` (not a raw value) sets the raw target to `P + 0.5` (for `P ≥ 0`), so
`±τ` stays inside the pixel. Garden does this. The CLI's `--to` is raw.

#### 2.4.4 Rounding to nice literals

After a verified solve, for each changed parameter, try in order:
- an int, if the original was an int;
- the original literal's number of decimals (`float_like` style);
- the fewest decimals (0 to 6) that keep every goal within `τ`.

Try the jointly rounded vector first, in one run. If it fails, round
parameters one at a time, largest change first. A parameter that has to
change from int to float spelling is reported (`respelled: true`) and
penalized in ranking. Respelling is only possible under `allow_respell`
(§2.3.4), and a respelled edit is a structural reload, not a value-only one
(§1.4). Rendering uses `literal_edit::float_like` (to be made `pub(crate)`)
and v1's `render_matching`, keeping a `-` inside the span for `negated`
literals. When a new value's sign differs from the old one's, the result is
re-parsed with the edit in place. If the text would read `x - -3` or would
not parse, the replacement is wrapped in parentheses (`x - (-3)`). Operators
outside the span are never rewritten.

Edits are grouped by `SourceSpan::file`. v1's `apply_edits` handles one
source string and ignores the file (§1.4), so v2 adds
`apply_edits_multi(files: &mut [(FileId, String)], edits)`. It splices
against the text the running program holds for each file
(`Env::program_source`, which already includes values set live by
`set_config_value`), not against the disk.

#### 2.4.5 Verified

An alternative is `verified` only when a final spec run with the rendered
literal values (parsed back from the new text) hits every goal within `τ`.

### 2.5 (e) Heuristics and ranking (M3)

#### 2.5.1 Eligibility (hard filters, before solving)

**Never editable, whatever the filter says:**
- literals in library code: the auto-loaded `std` prelude, bundled packages
  (`ui` and the rest, `bundled.rs`: embedded in the binary, with no file to
  write), git- or registry-installed packages (the package cache), and
  vendored copies (`vendor/`, `third_party/`). "User file" means a
  `SourceFile` whose `origin` is a path under the entry file's project root
  (the nearest `petal.toml`, else the entry file's directory) and outside
  the package cache. A bundled file has no `origin` and is never a user
  file. `ReportLibrary` (§2.7) can screen these literals for the
  explanation, and they are never offered as edits.
- the literal arguments of the hint builtins themselves (`min`, `max`,
  `step` in `tunable(x, 10, 80)`). Editing a range to make a goal reachable
  would defeat the hint.

Excluded by default, and included when the client allows them:
- literals in a **path dependency** (a local package the user owns), with
  `AllowPathDeps`;
- `rigid(...)` literals;
- `state_init` parameters whose state slot the checkpoint already holds
  (§2.1.2);
- `through_rng` literals: the only path to the goal goes through a
  `reads(RNG)` native's arguments (§2.1.4), or through a `rigid` barrier;
- **structural literals**: every consumer is a comparison, a `match`
  pattern, a list index, a `range`/loop bound, or a state or `memo` key.
  Editing one changes which code runs, not where a shape is.
  A literal that also reaches the goal through arithmetic is not structural;
- int literals in a `Discrete` column with `|Δ| < 0.5` (nothing to round to).

#### 2.5.2 Parameter cost weights `d_i` (lower means preferred)

| Kind | `d_i` |
|---|---|
| a knob: `config let` (any literal in its data), `tunable(...)`, or both | 0.5 |
| literal written in the goal's own user-frame call | 0.75 |
| `let` binding at top level of the entry file | 1.0 |
| literal in a user function body or a default parameter, entry file | 1.5 |
| literal in another user file | 2.0 |
| `through_var` | ×1.5 |
| `through_state` (feedback through state from earlier frames: the frozen-state slope covers one frame only) | ×4, and `stateful: true` on the alternative |
| clock- or input-coupled (§2.5.4) | ×4 |

The two marks were separate tiers until 24 October 2026 (`tunable` 0.25,
`config let` 0.5, both together 0.25). They are one tier now
(`rank::KNOB_WEIGHT`, `Param::is_knob`), so marking a literal twice changes
nothing; `tunable` keeps its bounds, and only the preference merged. The
study measured 0.25 and 0.5 for the merged tier and they tied on outcomes;
0.5 changed no run count (§9, "One knob tier").

#### 2.5.3 Alternatives

The candidate subsets are:
1. every single parameter (for one goal) or pair of parameters (for two
   goals) among the top 12 columns by `|J_g| / d_i`, where the subset can
   reach the goals (rank of `J_g[:, S]` = number of goals);
2. a "config-only" subset, if any knob (`config let` or `tunable`) has a
   nonzero column;
3. the client's allowed set, when it gave one;
4. the full screened set (minimum-norm spread across everything).

For a two-goal drag, item 1 alone is up to 66 pairs, and verifying each
would cost more runs than the whole budget. So subsets are first solved
**linearly against the cached Jacobian** (zero runs), and scored with the
§2.5.4 formula using predicted residuals and predicted collateral
(`J_c Δp`). Only the best `max_alternatives + 2` predictions, plus items 2
and 3 when present, go on to the LM-with-verification loop of §2.4.2.
Duplicates (the same edits) collapse.

#### 2.5.4 Score (lower is better)

```
score = Σ_i d_i |Δp_i|/s_i                      (how much code meaning moves)
      + κ_c · collateral_norm                   (how much else moved, measured)
      + κ_n · (|S| - #goals)                     (prefer fewer edits)
      + κ_r · respelled_count
      + κ_f · [regime_changed]                   (final fingerprint ≠ baseline's)
      + κ_g · [rng_order_changed]                (§2.1.4)
      + κ_e · max_j (residual_j / τ_j)           (0 when verified)
      + ∞ if not verified and a verified alternative exists
```

The defaults are `κ_c = 1`, `κ_n = 0.5`, `κ_r = 0.3`, `κ_f = 0.5`,
`κ_g = 0.5` and `κ_e = 2`. `regime_changed` is reported on the alternative:
the edit verified, and it also made the program take a different branch or
re-initialize a state slot somewhere. Collateral shows where.
`collateral_norm` is measured from the verification run: the RMS movement of
all non-goal rows, normalized by the goal movement. It is reported with the
list of moved emits (`CollateralItem { key, channel, callee, line,
max_move }`). This replaces v1's static `shared` flag with what actually
moved, so a literal that feeds one shape ranks above one that feeds fifty.

**Config preference.** v1 hard-pinned all non-config bindings once any
`config let` existed. v2 makes this a soft preference: knobs (`config let`
and `tunable`, one tier) weigh less, and alternatives built only from them
are listed first when they verify. The others are still returned, marked `config: false`.
`propose_edits_v2` with `Prefer::ConfigStrict` reproduces the v1 behaviour.

**Clock and input coupling** (late M3, droppable if the budget is tight:
it costs extra runs, and the ladder's example 10 is its only consumer).
When the baseline read `CLOCK` or `POINTER`
(from the `run_deps` record), recompute the top columns once more with the
clock binding shifted (`time += 0.37`, `frame_count += 1`) on a fork via
`set_binding_for`. A column whose value changes by more than 25% relative or
flips sign is coupled. Dragging the orbiting dot in
`examples/dashboards/server-monitoring/app.ptl:493` should move `mark_x` or
`11.0`, not the angular rate `1.1` in `time() * 1.1`.

### 2.6 (f) Hint declarations

Full proposal with corpus survey:
[proposals/dm-v2-hints.md](../proposals/dm-v2-hints.md). The summary:

```petal
tunable(x)                       // x's literal is a preferred DM target
tunable(x, min, max)             // ... and stays within [min, max]
tunable(x, min: 0, max: 255, step: 1)
rigid(x)                         // x's literal never moves; x is a feedback barrier
```

- **Runtime**: both are the identity on their first argument (duals pass
  through), with a `CONST` row, so static values, memo and the optimizer are
  unaffected. In spec runs, `rigid` replays its baseline value (§2.1.3).
- **Static meaning**: the IR keeps them as `BuiltinCall(name)`
  (`program.rs:178`, the name is a string constant resolved to a native by
  name). `hints.rs` therefore must not match by name alone. An embedder that
  registers its own native `rigid` (a physics engine's rigid-body
  constructor is the obvious one) would otherwise have its calls read as
  hints. `hints.rs` checks that the name resolves to the `NativeFnId` core
  registered for the hint. If an embedder replaced it, hints are disabled
  for that name and `DmDiagnostics.warnings` says so. `hints.rs` resolves
  argument 0 the way `provenance::resolve_arg` does (a
  literal in place, or a binding chain to one). It marks that literal term
  with the hint. `min`/`max`/`step` must be literals, or `lint` warns.
- **Forms**: at the definition (`let r = tunable(40, 10, 80)`), at the use
  (`draw_circle(tunable(cx), …)`), or as a bare statement on a name
  (`tunable(radius, 10, 80)`), which marks the binding. A computed argument
  `tunable(a + b)` marks nothing and gets the lint
  `dm-hint-not-literal`.
- **Names**: `tunable` and `rigid` occur nowhere in the ecosystem as `.ptl`
  functions or bindings, nor as native names in core, core-libs, garden or
  integrations (checked 2026-10-23 over sample-apps, garden, examples,
  core-libs, the prelude, `~/worlds-fair/ui/ptl`, `~/petal-creative` and
  `~/cheesecake/games`). Rejected: `fixed` (an existing number-format
  builtin, `builtins/mod.rs:450`), `freeze`/`frozen` (existing builtins),
  `pin`/`pinned`/`hold`/`held`/`anchor`/`locked`/`knob`/`nudge` (in use in
  `.ptl` code). A user function named `tunable` shadows the builtin and is
  then a plain `Call`, which `hints.rs` ignores. That is non-breaking, but
  such a function now gets the `shadows-builtin` lint
  (`lint/builtin_names.rs`) when its arity matches, which is a new warning.
  The review re-ran the grep (2026-10-23): `rigid` appears in the ecosystem
  only in comments (physics-playground, cheesecake `tides`/`lantern`/
  `skyrace`) and in Rust (`source_diff.rs`), and `tunable` only in
  worlds-fair's Rust `wf-tuning` docs. Neither is a `.ptl` identifier.
  `rigid` remains the most collision-prone of the two in physics-heavy
  embedders, which is why the `NativeFnId` check above exists.
- **Bare-statement form** (`tunable(radius, 10, 80)` on its own line) is a
  discarded statement-level expression. It must not be flagged by any
  unused-value lint, and the static-value classifier must not drop it. Since
  the row is `CONST` and nothing is removed from lowered code
  (`static_cache.rs` "No instruction is moved or removed"), the call stays
  in the IR, where `hints.rs` reads it.
- **`config let` stays.** It remains the top-level spelling, readable by
  `static_bindings`. `tunable` adds ranges, function-body and call-site
  scope, and the negative form.

Also new, as natives: a no-feedback row facet, `NativeEffects::no_feedback()`
(R2). It covers the RNG natives (already replayed) and lets embedders mark
natives whose output must not drive solving (a physics query, an id
allocator). It is the native-side twin of `rigid`.

### 2.7 (g) Client disambiguation API

The client says which parameters may move (an IDE drag mode with a variable
picked, or an agent that knows the intent). The engine works out how.

```rust
pub enum ParamSelector {
    Term(TermId),               // exact, this program only
    Span(SourceSpan),           // a literal or a binding name at this span (survives reload if the text didn't move)
    Variable(String),           // a binding name, base name or module-qualified
    Line(u32),                  // any literal on this line of the entry file
    ConfigOnly,                 // config let + tunable only
    Function(String),           // literals inside this function's body
    AllUserCode,                // lift the entry-file-first weighting
    AllowPathDeps,              // local path-dependency packages the user owns
    ReportLibrary,              // screen library literals for the report only; never editable (§2.5.1)
    IncludeStateInit,
    IncludeStructural,          // comparison / index / key literals (§2.5.1)
    IncludeThroughRng,
}
pub struct ParamFilter { pub allow: Option<Vec<ParamSelector>>, pub deny: Vec<ParamSelector> }
```

`allow: None` means the defaults of §2.5.1. `allow: Some(..)` is exact: the
engine uses only those, except that the never-editable classes of §2.5.1
stay out even when a selector names them, and the response says why. Asking for alternatives is
`DmRequest::max_alternatives` (default 3). The response always includes the
**candidate report** (every screened parameter with its column, quality,
weight and eligibility reason), so a client can show "this drag could move:
`offset` (config), `x`, `12` in `layout()`" and re-ask with a selection.

### 2.8 (h) Surfaces

See [§4](#4-surfaces).

---

## 3. Runtime refactoring needed

| # | Change | Where | Notes |
|---|---|---|---|
| R1 | `SpecHooks` on the VM: `Option<&mut SpecHooks>` built in `make_vm`; holds the const override table (`Vec<Option<Value>>` indexed by `ConstantId`, consulted by `LoadConst` in **both** `vm/dispatch.rs:22` and `vm/fast.rs:138` only when `Some`), the native gate (§2.1.3), the RNG/`rigid` replay table, the emit arg capture, the loaded-param bitset and the branch fingerprint | `backend/bytecode/vm/spec.rs` (new), `vm/mod.rs`, `vm/native.rs`, `env/run.rs` | Off path: one `Option` check on `LoadConst`, which is hot. Measure with `petal bench` and `bench_panel`. If it shows up, select a spec-mode dispatch loop at `make_vm` (the fast loop is skipped under hooks already: `hooks = trace.enabled \|\| ...`) |
| R2 | Row facets: `NativeEffects::contained: bool` (effect stays in the `ExecutionContext` or the stack), `no_feedback: bool`. Constructors `.contained()` and `.no_feedback()`. Mark **every** core row with `effect` contained (the full list is in §1.5, including `__declare_method` and the enum-data intrinsic, which the first draft missed: suppressing them breaks every program with methods or enums), with a test that fails when a new core `effect` row is not. Classify the in-repo hosts in M1 as well: petal-ui (`ui_mark`, `create_canvas`/`draw_to`/`draw_to_screen`, the panel stubs) and garden-script (`row`, `column`, `invalidate`, `mutate`, `panel_store`). Out-of-repo embedder natives with `effect` default to **escaping**. `Env::mark_handle_class_spec_safe(id)` as a side registry, not a new `HandleClass` field, so vendored embedders don't break. C bridge: `PB_FX_CONTAINED`, `PB_FX_NO_FEEDBACK` | `native_fn.rs`, `builtins/mod.rs`, `env/mod.rs`, `integrations/petal-c-bridge` | Adding fields keeps `..NativeEffects::PURE` struct literals compiling. Extend `effect_audit` to report an escaping effect seen in spec mode |
| R3 | `Env::slot_literals(pid, &[TermId]) -> Result<(), String>`: factor the `alloc_slot` + `retarget_constants` part out of the reload patch (`reload.rs:490-510`) as a value-neutral operation | `env/reload.rs` | Must keep `core/tests/hot_reload.rs` green. A later reload of an already-slotted term takes the existing `set_slot` path |
| R4 | `Stack::drop_derived()`: clear statics, memo, enums, functions and methods, and `run_deps.force()`. Factored from `restore_execution` (`fork.rs:109-119`) and `transfer_stack_state` | `stack.rs`, `env/fork.rs` | The fork in each spec run calls it. Memo is also disabled in spec runs (`memo = false` in `make_vm`) |
| R5 | (revised) Watched-term capture lives in `SpecHooks`, not in the Env-wide `TraceBuffer` (§1.3): a `HashSet<TermId>` of watched terms; at each traced result of a watched term, convert to `ObsValue` (strong, GC-proof) with a spec-local sequence. Spec runs switch the Env's trace, `observations` and profile off and restore them | `vm/spec.rs`, `env/run.rs` | Reuses the trace hook points (`hooks` flag) |
| R6 | `Env::set_dm_checkpoint(stack, on)` and `Env::dm_checkpoint(stack) -> Option<StackKey>`: keep one pre-run fork, refreshed at each `run` while on | `env/run.rs`, `env/fork.rs` | Garden turns it on in IDE mode. The cost is one heap fork per frame, only while armed |
| R7 | Branch fingerprint: in spec mode, fold `(ip, taken)` of every conditional jump, each loop's trip count and each `StateInit` execution (§2.1.2) into a `u64` (FxHash) | `vm/dispatch.rs` (spec loop only) | Never on the normal path |
| R8 | Emit records: when spec capture is on, push `EmitRecord` (channel, chain, key with loop-path hash, spec seq, raw args) beside `emit_origins`. `PetalCxt::push_output`/`emit` already know the chain. Raw args are captured in `call_native_fn` before `natives.call` and attached to the emits that call produced | `vm/native.rs`, `native_fn.rs` (`push_output`, `emit` at `:1209`) | Bounded: arg conversion depth 3, lists ≤ 64 |
| R9 | `literal_edit::float_like` becomes `pub(crate)` | `literal_edit.rs:1888` | For §2.4.4 |
| R10 | `tunable`/`rigid` builtins (identity, `CONST`, declared in `core/packages/core` like the other builtins), identity look-through in `provenance::resolve_arg` so v1 keeps working, docs in `Builtins.md`, `stdlib.json` regen, lints `dm-hint-not-literal` / `dm-hint-state-init` / non-literal bounds | `builtins/`, `packages/core`, `provenance.rs`, `lint/` | M2. See the proposal |
| R11 | `trait SpecHost { fn with_run_env(&mut self, env: &mut Env, f: &mut dyn FnMut(&mut Env)); }`: the host installs its thread-local providers (host data, fonts, clipboard) around a spec run exactly as around a live frame. `NoHost` for plain core. petal-ui's `Headless` and Garden's panel implement it | `dm_v2/spec.rs`, `core-libs/petal-ui/src/harness.rs`, `garden-script/src/panel.rs` | Without it, `host_data` answers nil and `text_width` measures differently in spec runs, and every baseline mismatches |
| R12 | `HOST_DATA` replay in the gate: record by (native, args hash) in the baseline, replay after; unseen keys give `Nil` and `new_host_keys += 1` | `vm/spec.rs` | Keeps a perturbed key from triggering a host fetch |
| R13 | Observed call edges: in spec mode, record `(call term, FunctionId)` for every user-function call | `vm/spec.rs`, call paths in `vm/` | Feeds §2.2 for dynamic calls and methods |
| R14 | `apply_edits_multi` (per-file splice, §2.4.4) and `DmResponse` edits carrying `FileId` + path | `dm_v2/api.rs` | v1 `apply_edits` unchanged |

No change to the language grammar, IR schema or bytecode format.

---

## 4. Surfaces

### 4.1 Rust API (M3)

`dm_v2::propose_edits_v2(env, checkpoint, &DmRequest) -> Result<DmResponse,
DmError>` (§6.3). It is re-exported from `lib.rs` as `petal::dm_v2`. The
embedding guide gets a section in M4.

### 4.2 CLI (M4)

`petal propose-edit` gains `--v2`, which becomes the default when M5's study
passes the bar in §5 (M5); `--v1` keeps the old engine. (Done 24 October
2026: v2 is the default, see §9.)

Two binaries carry it, because the core `petal` binary has no draw natives
(§1.7). The argument parser and JSON rendering live once, in core
(`dm_v2::cli`), and both binaries call them:
- `petal propose-edit --v2` for scripts that emit through `push_output`/`emit`
  channels (no petal-ui).
- `petal-ui-run propose-edit [--frames N] [--size WxH] [--scenario …]
  [--host-data …]` for petal-ui scripts. It runs the harness to frame
  `N - 1`, uses that as the checkpoint, and implements `SpecHost` (R11).

```
petal-ui-run propose-edit --channel draw_commands --emit 7 --native-arg 0 --by 25 sketch.ptl
petal-ui-run propose-edit --channel draw_commands --emit 7 --native-arg 0 --to 148 \
                   --native-arg 1 --to 90 --allow radius --allow line:12 --alternatives 5 --json sketch.ptl
petal propose-edit --v2 --channel shapes --emit 0 --arg 1 --by 10 script.ptl
... --explain           # candidate table, Jacobian columns, iterations
... --apply [--pick N]  # writes the top (or Nth) verified alternative (every file it touches)
```

- `--arg i` (v1 addressing: argument `i` of the user-file frame) stays and
  maps to `Observable::Watch` (§2.1.6). `--native-arg i` addresses the
  emitting native's raw argument. `--by d` is relative to the baseline value.
- `--allow`/`--deny` take `name`, `line:N`, `config`, `fn:NAME`, `user`,
  `lib` and `state-init`, and one literal by `at:FILE:LINE:COL` /
  `at:LINE:COL` (`ParamSelector::At`) or `term:N` (`ParamSelector::Term`). The old `--configurable`/`--static` map to
  `allow`-with-preference and `deny`.
- `--tol`, `--max-runs`, `--max-wall` (seconds; the 5 s default of
  `DmRequest::max_wall`), `--max-screen`, `--allow-respell` and `--seed`. The
  report names the budget that ended the session (`diagnostics.stopped_by`:
  `runs`, `wall` or `converged`; `RunBudget::stopped` records the first limit
  a run was refused by).
- `--frames N` runs `N` real frames (state carrying, `run --runs`'s loop)
  before the checkpoint, so the core CLI can solve frame `N + 1` against
  frozen state, as `petal-ui-propose-edit --frames` does for petal-ui
  scripts; the report echoes `frames`.
- `--at-most V`, `--at-least V`, `--within LO..HI` are range goals
  (`GoalTarget::Range`; §9 "Range goals (C6)").
- Each change and candidate names its place in the expression (`site`:
  `thorns(x:)`, `spawn(cfg:).speed`, `rows[3].x`), computed by the CLI from
  the term's consumer chain; the text output prints it after the line.
  The seed defaults to `0` (or `PETAL_SEED`) and is echoed in the output,
  so `run --trace-emits` and `propose-edit` agree on which shape "emit 7" is.
- `--apply` with `--v2` writes the alternative `--pick` names (it must be
  verified); without `--pick`, the only alternative or the only verified
  one, and it refuses when several are verified.
- `petal run --trace-emits` gains `native_args` per emit (raw values) and the
  `EmitKey`.

### 4.3 MCP (M4)

`tools/petal-mcp.ts`: `ProposeEdit` gains `engine: "v1" | "v2"` (default `v2`
after M5), `nativeArg`, `by`, `allow`, `deny`, `alternatives` and `explain`.
It returns `alternatives[]`, each with `edits`, `changes`, `achieved`,
`maxError`, `collateral` and `score`, plus `candidates[]`. Document this in
`docs/dev/mcp-server.md`. An agent workflow (TraceEmits, then ProposeEdit
with `explain`, then re-ask with `allow`) goes in
`docs/features/direct-manipulation.md`.

### 4.4 Garden (M4)

`PanelHost::propose_drag_edits_v2(cmd_index, goals: &[(native_arg, f64)],
filter) -> DragOutcomeV2`. It goes beside v1 and becomes the default when
stable.

- Turn on `set_dm_checkpoint` in IDE mode (R6), and use the pre-mouse-down
  checkpoint (§2.1.1). The panel implements `SpecHost` (R11).
- `drag_handle` maps to native arg indices (for `draw_circle`, `cx`=0 and
  `cy`=1 at `_native_circle`). The goal is the raw baseline value plus the
  drag delta.
- `EmitKey` comes from the session baseline's draw-channel emits, matched to
  `cmd_index` by position, with an assertion that the chains equal the
  frame's `frame_origins`.
- During a drag: open the session on mouse-down. Preview on each mouse move
  with one linear solve against the cached Jacobian (no runs) and show the
  predicted literals in a ghost overlay. On mouse-up, run the full verify and
  round, then apply the top alternative and hot-reload. That is a value-only
  reload unless an int was respelled as a float, which is a structural reload
  (§1.4). State is kept by key either way. The allow-list mode the task doc
  asks for is required. The minimum is a picker listing the candidate report
  and re-asking with `allow`. Holding Alt to cycle alternatives is a stretch.
- Edits to an imported user file are applied to that file's buffer
  (`FileId` → path, R14).
- The v1 rule that refuses all but the last shape of a looping call goes away
  (occurrence-keyed emits).

### 4.5 Docs (M4)

Rewrite `docs/features/direct-manipulation.md` for v2 (what can be edited:
"anything numeric the shape depends on, through any function"; the
heuristics; hints; the sliding scale of modifiability, where `var` and
`state` feedback and clock coupling lower it). Update
`docs/features/program-modification.md` (capability matrix row and known
limitation at `:241-243`), `docs/features/time-travel.md` (spec mode
suppresses escaping effects), `docs/dev/native-effects.md` (the new
facets), `docs/usage/ffi.md` (declare `contained`/`no_feedback`) and
`docs/CLI.md`.

---

## 5. Milestones

Each milestone is a set of commits on `main`. Each lands with its tests
green (`cargo test` in `core/`, plus the named suites) and updates
[§9](#9-status).

### M1: speculative runner

Scope: R1 (overrides, gate, capture, loaded set), R2 (facets, core rows,
in-repo host rows, handle registry), R3, R4, R5 (revised), R7, R8, R11, R12,
R13, and `dm_v2/{mod,spec,observe}.rs`.

Acceptance (in `core/tests/dm_v2_spec.rs` unless noted):
1. `override_moves_only_that_literal`: `let a = 10` / `let b = 10` /
   `push_output(sym, a)` / `push_output(sym, b)`. Overriding `a`'s term to
   12.5 moves emit 0 only. The two `10`s were interned to one `ConstantId`
   before slotting.
2. `live_stack_untouched`: after 50 spec runs with overrides, the live
   stack's next run emits exactly what it emitted before, its state is equal
   (`diff_state` empty), and `take_output` has no speculative prints.
3. `print_is_contained_and_silent`: a spec run of `print("x")` echoes nothing
   and leaves the live output buffer empty.
4. `escaping_effect_suppressed`: an embedder native registered `EFFECT`
   (escaping) increments a Rust `Cell` counter. Spec runs never increment it,
   `SpecRun::suppressed` names it, and the same native marked `.contained()`
   is called.
5. `rng_replay_by_callsite`: `let n = 3` and a loop of `n` iterations, each
   calling `random()` and emitting it, followed by a separate `random()` emit.
   Overriding `n` to 4 keeps the first three emitted values and the last one
   bit-identical to the baseline. Under `ForkStream` the last one differs
   (proving the test bites).
6. `double_baseline_detects_nondeterminism`: a native reading a Rust counter
   (declared `PURE` on purpose, which is wrong) makes `begin` fail with
   `Nondeterministic`.
7. `frozen_state`: `state var n = 0` / `set n = n + 1` / emit `n`. Every spec
   run from the checkpoint emits the same value, and the live `n` is
   unchanged. `StateMode::Fresh` emits the initializer path.
8. `raw_native_args_not_truncated`: under petal-ui (`core-libs/petal-ui/tests/`),
   `draw_circle(100.3, ...)` captures raw arg 0 = 100.3 while the emitted
   command says 100.
9. `emit_key_stable_in_loops`: five circles from one call in a loop get keys
   occurrence 0 to 4. Perturbing a literal that does not change the trip
   count keeps all keys.
10. `fingerprint_flips_on_branch`: `if x > 5 then emit(1) else emit(2) end`
    with `x = 5.0`. The override 5.1 changes the fingerprint and 4.9 does not.
11. `recompile_oracle_agrees`: for every `.ptl` in
    `examples/direct-manipulation-v2/` (or the M1 seed set
    `core/tests/dm_v2/*.ptl` until M5), override each literal in turn by
    `+1e-3` with both strategies. The observables are bit-identical.
12. `budget_overrun_is_a_failed_run`: a `while i < n` loop with `n`
    overridden to `1e12` returns `SpecRun { error: Some(..) }` and the
    session stays usable.
13. Bench: `petal bench` and `bench_panel` show no regression beyond noise
    (≤ 1%) with spec mode off. Record the numbers in §9.
14. `env_trace_untouched`: with the Env trace enabled (as Garden does), 20
    spec runs add no event to `env.trace()` and leave its `next_sequence`
    unchanged. Watched values survive a forced GC inside the run.
15. `core_effect_rows_contained`: every core native whose row has `effect`
    is `contained`. A program with `fn Point.len` methods and an enum runs
    identically in spec mode.
16. `host_data_replayed`: under petal-ui's `Headless` with a counting
    provider, a baseline asks for `("item", "3")`. A perturbed run asking
    for `("item", "4")` gets `Nil`, the provider is not called, and
    `new_host_keys == 1`. Without `SpecHost` the baseline mismatches (the
    test bites).
17. `state_key_miss_changes_fingerprint`: `for i in range(n) key i` with
    `state var s = 0` inside. Overriding `n` from 3 to 4 changes the
    fingerprint through the new slot's `StateInit`.
18. `rng_uniform_replay_maps_args`: `random_int(0, w)` with `w` overridden
    gives the value a real re-run with the same seed gives.
19. `channel_order_independent`: two channels, compared across 10 forks:
    `SpecRun` equality never depends on `HashMap` order.

### M2: discovery, Jacobian, hints

Scope: `dm_v2/{params,hints,jacobian}.rs`, R9, R10, the no-feedback facet
wired into discovery and the gate. (The dual fast path is future work,
§2.3.6.)

Core tests cannot call `draw_circle` (§1.7). `core/tests/dm_v2_*` register a
test native `draw(x, y, r)` with `EMITS`, which reads its args with
`get_int` exactly as petal-ui does. Wherever the tests below say
`draw_circle`, they mean this native. The petal-ui variants live in
`core-libs/petal-ui/tests/`.

Acceptance (`core/tests/dm_v2_jacobian.rs`):
1. `discovers_literal_inside_helper`: `fn place(i) 20 + i * 30 end` (the body on its own line) /
   `draw(place(2), 50)`. Discovery for arg 0 includes `20` and `30` (in the
   body) and `2` (the call arg), and not `50`. Their Jacobian columns are 1,
   2 and 30.
2. `discovers_through_map_and_records`: a layout built with `map` over a list
   of records `{w: 40}` with a gap literal. The columns match the analytic
   ones within 1e-6 relative.
3. `trig_column`: `cx + cos(a) * r`, with each column within 1e-6 of the
   analytic derivative.
4. `rng_cuts_path`: `random_int(lo, hi)` feeding x gives `lo` and `hi`
   columns of exactly 0, and they are pruned statically.
5. `rigid_is_a_barrier`: `let w = rigid(40)` excludes `40`. `rigid(a + b)`
   gives zero columns for `a` and `b` through that path but not through
   another path.
6. `tunable_range_recorded`: `tunable(r, 10, 80)` sets the `Param` hint with
   `min = 10`, `max = 80`. `tunable(a + b)` gets lint
   `dm-hint-not-literal`.
7. `screening_cost`: a program with 200 irrelevant literals and 3 relevant
   ones is screened in ≤ 30 runs, and exactly the 3 survive. A program with
   2,000 literals reached only through an unresolved dynamic call screens
   only the functions the baseline actually called (R13).
8. `branch_flip_and_step_quality`: `abs(x)` at 0 gives `Kink`, `round(x) *
   10` gives `Step` with a secant of about 10, `if x > 5` at the threshold
   gives `BranchFlip`, and `xs[i]` with an int `i` gives `Discrete`.
9. `int_literal_type_preserving`: `range(n)` with an int `n` gives a
   `Discrete` column from `±1` probes. `draw(a / 4)` with `let a = 10` gives
   the integer secant (`11 / 4 - 10 / 4 = 0`, then `±4` gives 1/4), never the
   float jump of `0.5 / h`.
10. Names: `tunable`/`rigid` added to `stdlib.json` and `Builtins.md`. The
    collision grep from the proposal is re-run in the commit message.
11. `library_literals_never_proposed`: a goal reached through the `std`
    prelude's `lerp` and petal-ui's `draw_circle` overloads (petal-ui test)
    never produces an edit inside a bundled file, even with
    `allow: [ReportLibrary, AllUserCode]`.
12. `structural_and_hint_bounds_excluded`: `if x > 5`'s `5`, `xs[2]`'s `2`
    and `tunable(r, 10, 80)`'s `10`/`80` are ineligible by default, and are
    listed in the candidate report with their reason.
13. `embedder_rigid_is_not_a_hint`: an embedder native registered as
    `rigid` turns hints off for that name, with a warning.

### M3: solver, ranking, disambiguation, public API

Scope: `dm_v2/{solve,rank,api}.rs`, `propose_edits_v2`.

Acceptance (`core/tests/dm_v2_solve.rs`):
1. `through_helper_single_goal`: `fn place(i) 20 + i * 30 end` (the body on
   its own line), then `draw_circle(place(1.0), 100, 10, …)` and
   `draw_circle(place(2.0), 100, 10, …)`. Dragging the second circle to
   x = 95 makes the top alternative, verified, edit the call argument `2.0`
   → `2.5`, which moves nothing else. The alternatives editing `20` → `35`
   or `30` → `37.5` (in the helper body) are also verified and listed below
   it, each with collateral naming the first circle. With the first circle
   deleted from the program, all three verify and the ranking follows
   §2.5.4. The test pins that ranking, so a weight change shows up as a
   test diff.
2. `config_preferred_softly`: with `config let offset = 10` and an
   alternative through a call-site literal, `offset` comes first and the
   call-site literal is still listed.
3. `two_goals_one_gesture`: a drag in x and y of one circle placed through
   `polar(cx, cy, angle, r)` gives verified edits to `cx` and `cy` with
   collateral 0. The alternative that moves `r` and `angle` is listed with a
   higher score.
4. `shared_literal_penalized_by_measured_collateral`: `let gap = 10`, used by
   five shapes, and a per-shape literal. Dragging shape 3 prefers its own
   literal. Collateral reports 4 moved emits for the `gap` alternative.
5. `allowed_set_exact`: `allow: [Variable("gap")]` returns only `gap`
   alternatives, even though they have collateral.
6. `range_clamp`: `tunable(r, 10, 80)` with a goal that needs `r = 120`
   gives `verified: false`, `r = 80`, and the residual reported.
7. `rounding`: an int literal stays an int when within τ. A float keeps its
   decimals style (`3.50`).
8. `loop_shape_any_iteration`: dragging the third of five bars (a computed
   x in a loop) succeeds. v1 refuses this.
9. `nonlinear_converges`: a fractal-tree branch tip (recursion with
   `cos`/`sin` and a length scale of 0.7) reaches its goal within τ in ≤ 6
   LM iterations.
10. `clock_coupling_penalized`: the server-monitoring orbit pattern. The
    alternative editing the angular rate ranks below `mark_x`/radius.
11. `string_goal_falls_back_to_v1`.
12. `budget`: a request with `max_runs: 30` never exceeds 30 spec runs and
    reports `budget_exhausted` when it stops early. A two-goal drag with 12
    relevant columns verifies at most `max_alternatives + 2` subsets.
13. `multi_file_edit`: a helper in a local `import ./layout` file. The top
    alternative edits `layout.ptl` at the right offsets, and
    `apply_edits_multi` leaves the entry file alone.
14. `rng_reshuffle_flagged`: an edit that adds a `random()` draw before
    others verifies under replay and is flagged `rng_order_changed`.
15. `state_init_on_fresh_stack`: on a CLI-style fresh stack, `state var x =
    100` feeding the goal is an eligible parameter. From a live checkpoint
    that holds `x`, it is not.

### M4: surfaces

Scope: CLI (§4.2), MCP (§4.3), Garden (§4.4), docs (§4.5).

Acceptance:
1. CLI golden tests in `core/tests/cli_propose_edit_v2.rs`: `--json` shape
   (alternatives, candidates, collateral), `--explain` text, and
   `--apply --pick` writing the file. v1 output is unchanged under `--v1`.
2. `tools/petal-mcp.ts`: a smoke test in its test file calls `ProposeEdit`
   with `engine: "v2"` on a helper-function example.
3. Garden: `garden-script` unit test `drag_through_helper_v2` (panel
   script with `fn place(i)`, drag cmd 2 by +25 in x) gives `Edits` touching
   the helper literal or the call arg. The integration test under
   `garden/tools` drives a drag via the debug server and checks the file
   changed and the panel re-rendered with the shape moved by 25 ±0.5.
4. Docs updated (§4.5), links checked.

### M5: examples ladder and complexity study

Scope: `examples/direct-manipulation-v2/*.ptl` and the harness
`tools/dm-v2-study.ts`.

Each example carries its goals and expectations in header comments the
harness reads:

```petal
// dm-goal: channel=draw_commands emit=2 native-arg=0 by=+25 native-arg=1 by=-10
// dm-expect: changes ⊆ {gap, x0}      (the intended parameters; the top alternative must use only these)
// dm-allow: ...                       (optional client filter)
```

The ladder, from trivial to complex:

| # | File | What it exercises |
|---|---|---|
| 01 | `01-literal.ptl` | literal in the call (v1 parity) |
| 02 | `02-binding.ptl` | `let` and `config let` (v1 parity) |
| 03 | `03-arith.ptl` | `x + offset * 2` (v1 parity) |
| 04 | `04-helper.ptl` | through one helper function |
| 05 | `05-nested.ptl` | three nested helpers with default params |
| 06 | `06-loop-row.ptl` | a row of shapes from a `for` loop; drag the third |
| 07 | `07-map-layout.ptl` | `map` over a list building positions with a running sum |
| 08 | `08-records.ptl` | records and field reads (`{x, y}` points, `draw_circle(center, r, c)`) |
| 09 | `09-clock-hand.ptl` | trig: a clock hand at a fixed time, drag the tip |
| 10 | `10-orbit.ptl` | trig plus `time()`: clock coupling must not pick the rate |
| 11 | `11-fractal-tree.ptl` | recursion: drag a leaf, solve the branch length or angle |
| 12 | `12-flow-layout.ptl` | positions computed from text widths (`text_width`), wrapping |
| 13 | `13-physics.ptl` | in-frame iterated simulation (100 Euler steps of a projectile); drag the landing point, solve `v0`/angle |
| 14 | `14-rand.ptl` | scattered shapes with `random()`; the drag moves one and rand stays put |
| 15 | `15-state.ptl` | `state var` animation; frozen state, `through_state` flagged |
| 16 | `16-var.ptl` | `var` accumulators in a loop |
| 17 | `17-hints.ptl` | `tunable` ranges and `rigid` |
| 18 | `18-ui-panel.ptl` | a real petal-ui widget layout (copy of an `examples/ui` panel) |
| 19 | `19-big.ptl` | a large example from `examples/dashboards` (server-monitoring), many emits |

The **complexity study** (`tools/dm-v2-study.ts`, run as
`./tools/dm-v2-study.ts [--json] [--out <file>] [--extra <dir>]`) runs
`petal-ui-run propose-edit --json` per goal for petal-ui examples, and
`petal propose-edit --v2 --json` for channel-only ones (§4.2). It reports,
per example and in total:
- the success rate (top alternative verified, and within `dm-expect`);
- verified-any rate;
- runs used;
- wall time (median of 3);
- final max error / τ;
- the candidate count: static, loaded, eligible, screened (and whether
  `max_screen` truncated), relevant;
- the number of alternatives, and how many are flagged `regime_changed` or
  `rng_order_changed`;
- per-run cost split into fork, statics rebuild and execution, which decides
  whether §2.3.5a is worth building.

It also reports, per example, static program metrics: terms, functions,
max call depth reached, loops, `var` count, `state` count, emits, and
whether the baseline read the clock or RNG. These let the report plot
success against complexity. `--extra` points at copies of large external
projects. Copy them to `/Volumes/bureau/petal-dmv2-study/`, never into the
repo: worlds-fair `ui/ptl` screens, petal-creative projects, cheesecake
games. For each one the harness auto-generates goals ("move emit k's arg 0
by +10" for 20 sampled emits) with no `dm-expect`, scoring only
verified-ness and collateral. This part is best-effort. The cheesecake and
worlds-fair scripts need their embedders' natives, which these headless
binaries do not have, so only scripts that run under `petal-ui-run` count.
The rest are listed as "not runnable headless", which is itself a finding.

Acceptance:
1. All 19 examples run. Examples 01 to 14 and 16 to 17 meet `dm-expect` with
   a verified top alternative. 15 is verified with `stateful` flagged.
2. The report is written to `docs/dev/direct-manipulation-v2-study.md` (a
   generated table plus a short analysis of where and why it fails), and
   §9 links to it.
3. The bar for making v2 the default (§4.2): a ≥ 90% verified top
   alternative on the ladder, and ≥ 70% verified on the auto-generated
   external goals, at a median of ≤ 60 runs.

---

## 6. Interfaces between milestones

These are the contracts later milestones code against. Names may gain
fields. Changing the meaning of a field requires updating this section.

### 6.1 M1 provides (`dm_v2::spec`, `dm_v2::observe`)

```rust
pub struct SpecConfig {
    pub state: StateMode,                 // Frozen (default) | Fresh
    pub rng: RngMode,                     // ReplayByCallSite (default) | ForkStream
    pub max_steps: Option<u64>,           // None = 10x baseline
    pub max_wall: std::time::Duration,    // default 2s per run
    pub capture: CaptureSpec,
    pub strategy: OverrideStrategy,       // Slots (default) | Recompile (tests)
    pub session_seed: u64,
}
pub struct CaptureSpec {
    pub channels: Option<Vec<SymbolId>>,  // None = all
    pub native_args: bool,                // default true
    pub watch: Vec<TermId>,
}

pub struct Overrides(pub SmallVec<[(TermId, OverrideValue); 8]>);
pub enum OverrideValue { Num(f64), Int(i64), Dual(f64, f64) }

pub struct SpecSession { /* checkpoint StackKey, ProgramId, slotted terms, baseline, replay tables */ }

impl SpecSession {
    /// `checkpoint` must be ready to run the frame (inputs bound, pre-frame state).
    /// Slots `params`, runs the baseline twice, checks determinism and,
    /// when `live` is given, the match against the live frame.
    /// Forks `checkpoint` into a stack the session owns (§2.1.1).
    pub fn begin(env: &mut Env, host: &mut dyn SpecHost, checkpoint: StackKey, params: &[TermId],
                 cfg: SpecConfig, live: Option<&LiveObservation>) -> Result<SpecSession, SpecError>;
    pub fn add_params(&mut self, env: &mut Env, params: &[TermId]) -> Result<(), SpecError>;
    pub fn run(&mut self, env: &mut Env, host: &mut dyn SpecHost, overrides: &Overrides) -> SpecRun;  // a failed run is data, not Err
    pub fn run_fork_stream(&mut self, env: &mut Env, host: &mut dyn SpecHost, overrides: &Overrides) -> SpecRun; // §2.1.4
    pub fn baseline(&self) -> &SpecRun;
    pub fn runs_used(&self) -> usize;
    pub fn program(&self) -> ProgramId;
    pub fn end(self, env: &mut Env);       // drops forks; slots stay (value-neutral)
}

pub trait SpecHost { fn with_run_env(&mut self, env: &mut Env, f: &mut dyn FnMut(&mut Env)); }  // R11
pub struct NoHost;                         // plain core: calls `f(env)`

pub enum RunError { Runtime(String), StepBudget, TimedOut }
pub struct SpecRun {
    pub error: Option<RunError>,
    pub emits: Vec<EmitRecord>,
    pub watched: Vec<WatchRecord>,         // (term, trace seq, ObsValue)
    pub fingerprint: u64,
    pub loaded: FixedBitSet,               // indexed by the session's param index
    pub suppressed: Vec<SuppressedCall>,   // native name, origin term
    pub new_random_keys: u32,
    pub new_host_keys: u32,                // §2.1.3, R12
    pub call_edges: Vec<(TermId, FunctionId)>,  // R13, baseline only
    pub steps: u64,
    pub elapsed: std::time::Duration,
}
pub struct EmitRecord {
    pub channel: SymbolId,
    pub key: EmitKey,
    pub chain: EmitSite,
    pub seq: u64,                          // spec-local sequence (R5 revised), not the Env trace's
    pub native: Option<NativeFnId>,
    pub native_args: SmallVec<[ObsValue; 8]>,
    pub value: ObsValue,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EmitKey { pub chain_hash: u64, pub path_hash: u64, pub occurrence: u32 }
pub enum ObsValue { Num(f64), Int(i64), Bool(bool), Str(u64 /* hash */), Nil,
                    List(Vec<ObsValue>), Record(Vec<(String, ObsValue)>), Opaque }

pub enum Observable {
    /// raw arg `arg` of the native that produced emit `key`, then `path` into it
    EmitArg { key: EmitKey, arg: usize, path: Vec<PathStep> },
    /// a field of the emitted value itself
    EmitValue { key: EmitKey, path: Vec<PathStep> },
    /// last value of `term` before the emit `at` (v1-style user-frame arg)
    Watch { term: TermId, at: EmitKey, path: Vec<PathStep> },
}
pub enum PathStep { Index(usize), Field(String) }
impl SpecRun { pub fn read(&self, o: &Observable) -> Option<f64>; }

pub enum SpecError {
    Nondeterministic { first_difference: String },
    BaselineMismatch { goal: usize, live: f64, spec: f64, suppressed: Vec<String> },
    Unsafe { native: String },             // a handle class not marked spec-safe
    BaselineFailed(String),
    Stale,                                 // checkpoint stack / program gone
}
```

`Env` additions (R3, R6): `slot_literals`, `set_dm_checkpoint` and
`dm_checkpoint`. `NativeEffects` additions (R2): `contained`,
`no_feedback`, plus `Env::mark_handle_class_spec_safe`.

### 6.2 M2 provides (`dm_v2::params`, `dm_v2::hints`, `dm_v2::jacobian`)

```rust
pub struct Param {
    pub term: TermId, pub span: SourceSpan, pub file: FileId,
    pub value: f64, pub is_int: bool, pub negated: bool,
    pub binding: Option<String>, pub config: bool,
    pub hint: Option<Hint>, pub function: Option<FunctionId>,
    pub through_var: bool, pub through_state: bool, pub state_init: bool,
    pub through_rng: bool, pub structural: bool, pub hint_bound: bool,
    pub in_goal_frame: bool,
    pub origin: CodeOrigin,                          // §2.5.1
    pub distance: u32,                               // §2.2 proximity order
}
pub enum CodeOrigin { Entry, UserFile, PathDep, Library /* never editable */ }
pub enum Hint { Tunable { min: Option<f64>, max: Option<f64>, step: Option<f64> }, Rigid }

pub fn read_hints(program: &Program) -> HashMap<TermId, Hint>;
pub fn discover_params(program: &Program, anchors: &[TermId], hints: &HashMap<TermId, Hint>)
    -> Vec<Param>;                                   // static superset, all eligibilities
pub fn anchors_for(program: &Program, run: &SpecRun, goals: &[Observable]) -> Vec<TermId>;
pub fn screen_params(session: &mut SpecSession, env: &mut Env, host: &mut dyn SpecHost,
                     params: &[Param], eligible: &[usize], max_screen: usize,
                     goals: &[Observable], budget: &mut RunBudget) -> ScreenResult;  // kept, truncated

pub struct FdOptions { pub mode: FdMode /* Central | Forward */, pub rel_step: f64 /* 1e-3 */,
                       pub max_escalations: u8, pub allow_respell: bool /* float probes of ints, §2.3.4 */ }
pub struct Jacobian {
    pub params: Vec<usize>,          // indices into the Param list
    pub rows: Vec<Observable>,       // goal rows first, then collateral
    pub goal_rows: usize,
    pub d: Vec<f64>,                 // row-major rows × params
    pub step: Vec<f64>,
    pub quality: Vec<ColumnQuality>,
}
pub enum ColumnQuality { Smooth, Kink { left: f64, right: f64 }, BranchFlip { side: Side },
                         Step { secant_h: f64 }, Discrete, Zero, Errored(String) }
pub fn jacobian(session: &mut SpecSession, env: &mut Env, params: &[Param], keep: &[usize],
                rows: Vec<Observable>, goal_rows: usize, at: &Overrides,
                opts: &FdOptions, budget: &mut RunBudget) -> Jacobian;
pub fn collateral_rows(baseline: &SpecRun, goals: &[Observable], cap: usize) -> Vec<Observable>;

pub struct RunBudget { pub max_runs: usize, pub used: usize, pub deadline: std::time::Instant }
```

### 6.3 M3 provides (`dm_v2::api`, `dm_v2::solve`, `dm_v2::rank`)

```rust
pub struct DmGoal { pub observable: Observable, pub target: f64,
                    pub tolerance: Option<f64>, pub weight: f64 }
pub struct DmRequest {
    pub goals: Vec<DmGoal>,
    pub filter: ParamFilter,                     // §2.7
    pub prefer: Prefer,                          // Soft (default) | ConfigStrict (v1 behaviour)
    pub max_alternatives: usize,                 // default 3
    pub max_runs: usize,                         // default 200
    pub max_screen: usize,                       // default 256 (§2.2)
    pub allow_respell: bool,                     // default false (§2.3.4)
    pub max_wall: std::time::Duration,           // default 5s
    pub spec: SpecConfig,
    pub live: Option<LiveObservation>,
    pub explain: bool,
}
pub struct DmResponse {
    pub alternatives: Vec<Alternative>,          // ranked, best first
    pub candidates: Vec<CandidateReport>,
    pub diagnostics: DmDiagnostics,              // runs, wall time, suppressed natives, budget_exhausted, warnings
    pub explain: Option<ExplainLog>,
}
pub struct Alternative {
    pub edits: Vec<FileEdit>,                    // SourceEdit + FileId + origin path (R14)
    pub changes: Vec<ParamChange>,               // param, old, new, new_text, respelled
    pub achieved: Vec<f64>,                      // per goal
    pub max_error: f64,                          // max_j |achieved - target| / τ_j
    pub verified: bool,
    pub collateral: Collateral,                  // norm + Vec<CollateralItem>
    pub stateful: bool, pub config_only: bool,
    pub regime_changed: bool, pub rng_order_changed: bool,
    pub score: f64,
    pub description: String,                     // "set `gap` to 14 (line 3); moves 4 other shapes"
}
pub struct CandidateReport { pub param: Param, pub eligible: Result<(), &'static str>,
                             pub column: Option<Vec<f64>> /* goal rows */, pub quality: Option<ColumnQuality>,
                             pub weight: f64 }

pub fn propose_edits_v2(env: &mut Env, host: &mut dyn SpecHost, checkpoint: StackKey, req: &DmRequest)
    -> Result<DmResponse, DmError>;

/// The reusable form for a drag: open once, preview with no runs, verify at the end.
pub struct DmSession { /* SpecSession + params + Jacobian */ }
impl DmSession {
    pub fn open(env: &mut Env, host: &mut dyn SpecHost, checkpoint: StackKey, req: &DmRequest) -> Result<DmSession, DmError>;
    pub fn preview(&self, targets: &[f64]) -> Vec<ParamChange>;   // linear, zero runs
    pub fn finish(self, env: &mut Env, host: &mut dyn SpecHost, targets: &[f64]) -> Result<DmResponse, DmError>;
}

pub fn apply_edits_multi(files: &mut [(FileId, String)], edits: &[FileEdit]) -> Result<(), DmError>;  // R14

pub enum DmError { Spec(SpecError), NoGoals, BadObservable(String), NothingToMove { why: String } }
```

`apply_edits_multi` applies `Alternative::edits` (v1's single-source
`apply_edits` ignores the file, §1.4).

### 6.4 M4 and M5 consume

M4 consumes only §6.3 (plus `EmitKey`/`Observable` from §6.1 for
addressing). M5 consumes the CLI's `--json` shape. Freeze that shape in M4
with a golden test.

---

## 7. Decisions this plan overrides

| Old decision | Where | v2 |
|---|---|---|
| A value that flows through a call gets no proposal; guessing is worse than refusing | `direct_manipulation.rs:32-36`, `:534`; `features/direct-manipulation.md` "Some values can't be edited by dragging"; `program-modification.md:241-243` | v2 solves through calls. A guess cannot slip through, because every proposal is verified by re-running, and unverified ones are labelled |
| Re-running the program is unsound unless it is deterministic, so provenance never re-runs | `provenance.rs:21-24` | v2 re-runs and *makes* it deterministic (checkpoint fork, RNG replay by call site, effect gate), and it **checks** determinism (double baseline). Provenance itself stays lazy and run-free |
| Once a program declares a `config let`, every other named binding is pinned | `direct_manipulation.rs:213-235`; `features/direct-manipulation.md` "Marking what's tunable" | Soft preference by weight. `Prefer::ConfigStrict` keeps the old rule on request |
| `shared` = more than one static reader | `direct_manipulation.rs:393` | Collateral is measured from what actually moved in the verification run |
| Only the last shape of a looping call can be solved | `garden/garden-script/src/panel.rs:2304-2318` | Emits are matched by `EmitKey` occurrence, so any iteration can be solved |
| A fork calls the same natives, so host side effects repeat | `features/time-travel.md:310-317` | In spec mode, escaping effects are suppressed (R2). Plain forks keep the old behaviour |
| `effect` is one bit | `native_fn.rs:139-143` | It gains the `contained` and `no_feedback` facets |
| Edits are spliced into one source string | `direct_manipulation.rs:334` | v2 edits carry their file and are applied per file (R14). v1 keeps its function |
| "rand cannot receive feedback" was first planned as "replay the random result" (draft §2.1.4) | this plan's first draft | The uniform draw is replayed and the native maps it with its current arguments. Feedback is refused by eligibility (`through_rng`), not by freezing the result, so collateral stays honest |

---

## 8. Risks and open questions

1. **Embedder rows.** Escaping-by-default can make spec runs of cheesecake or
   worlds-fair scripts suppress natives the script depends on, which then
   shows as `BaselineMismatch`. Mitigation: the error names the natives, and
   the ecosystem sweep in M4 marks the contained ones (the vendored
   `petal-c-bridge` needs `PB_FX_CONTAINED`, then re-vendoring). petal-ui and
   garden-script are swept in M1, because their own tests depend on it.
   `register_native_boxed` closures with captured `Cell`s are the usual
   escaping case. The doc at `env/mod.rs:1890-1900` already warns that
   forks call the same closure.
2. **Handles.** Refusing unmarked handle classes may block games
   entirely. M1 decides whether a `reads`-only method list is worth adding.
3. **Cost on big panels.** 30 runs of an 8 ms frame is 240 ms, fine for
   mouse-up and not for mouse-move. Hence `DmSession::preview` (linear) during
   the drag. The study measures this.
4. **Multi-frame dynamics** (accumulated `state`, physics across frames).
   This plan measures one frame. A horizon mode (replay the last N frames
   from a checkpoint with recorded inputs, via the `petal-sdl` timeline
   pattern) is future work. M5's example 15 documents the limit.
5. **The `LoadConst` hot path.** R1's check must be measured (M1 acceptance
   13).
6. **Hint names.** Settled 24 October 2026: the owner approved `tunable`
   and `rigid` ([proposal](../proposals/dm-v2-hints.md), accepted).
7. **Heap fork and statics per run.** Big programs pay a deep heap copy and
   a full statics rebuild on every run. M5 measures the split, and §2.3.5a
   is the planned fix. A copy-on-write heap is out of scope.
8. **Replay is a model.** Under `ReplayByCallSite` and `HOST_DATA` replay,
   a verified alternative is verified in a world where random draws and host
   answers stay put. The `ForkStream` check (§2.1.4) and `new_host_keys`
   say when the real program will differ. Clients should surface both.
9. **Wall-clock time in spec runs.** `petal-ui-run` pins the clock, while a
   live host binds `time` once per frame, and the checkpoint freezes the
   binding. A native that reads `Instant::now()` directly (none in core or
   petal-ui today; the `frame_core.rs:161` wall clock feeds the binding) would
   be caught by the double baseline as `Nondeterministic`, which fails the
   session instead of degrading. Such natives must declare `reads(CLOCK)` and
   be replayed like `HOST_DATA`. That is a one-line gate rule if one appears.

---

## 9. Status

| Milestone | State | Commits | Notes |
|---|---|---|---|
| Design | done, 2026-10-23 | `c9eb67b7` | plan + [hints proposal](../proposals/dm-v2-hints.md) |
| Design review | done, 2026-10-23 | (this commit) | adversarial review against the code, [§10](#10-review-notes) |
| M1 speculative runner | done, 2026-10-23 | `07e81adf` | `dm_v2::{spec,observe}`, `vm/spec.rs`, `env/spec_run.rs`; tests `core/tests/dm_v2_spec.rs` (24), `core-libs/petal-ui/tests/dm_v2_spec.rs` (2); deviations below |
| M2 discovery + Jacobian + hints | done, 2026-10-24 | `43ee2820` | `dm_v2::{params,hints,jacobian}`, `builtins/hints.rs`, `lint/dm_hints.rs`; tests `core/tests/dm_v2_jacobian.rs` (17), `core-libs/petal-ui/tests/dm_v2_params.rs` (1); deviations below |
| M3 solver + ranking + API | done, 2026-10-24 | `1c28a217` | `dm_v2::{solve,rank,api}`, `propose_edits_v2`, `DmSession`, `apply_edits_multi`; tests `core/tests/dm_v2_solve.rs` (27); deviations below |
| M4 surfaces | M4a (CLI, MCP, docs) done, 2026-10-24; M4b (Garden) done, 2026-10-24 | M4a: `83cd1469`; M4b: `6da29349`, `1f565eca` | M4b: Garden drag-to-edit on v2 (`garden-script/src/panel/dm_v2.rs`, `garden-app/src/app/manip_v2.rs`); tests `garden-script/tests/dm_v2_drag.rs` (13), `tools/dm-drag-integration-test.ts`. M4a: `cli/dm_v2.rs` (`propose-edit --v2`), `run --trace-emits` keys + native args, MCP `ProposeEdit`/`TraceEmits`, docs; tests `core/tests/cli_propose_edit_v2.rs` (12), `test/vitest/petal-mcp.test.ts` (5); deviations below |
| M5 examples + study | done, 2026-10-24 | `ccc25ce7` | `examples/direct-manipulation-v2/` (27 examples, 105 goals), harness `core-libs/petal-ui/tests/dm_v2_study.rs`, report [direct-manipulation-v2-study.md](direct-manipulation-v2-study.md) + `.json`; engine revisions in `dm_v2::{rank,api,jacobian,spec}`; deviations below |
| Code review (post-M5) | done, 2026-10-24 | `0d6dae6e`, `08bc37cd`, `1e6f7913`, `4dea9231` | adversarial review of the whole change set; five bugs fixed, each pinned by `core/tests/dm_v2_review.rs` (7); see "Code review" below |
| Final test pass | done, 2026-10-24 | `4a8d7145`, `62be2098` (+ this commit) | every suite green, study re-run (unchanged outcomes), effect audits clean, ecosystem checked; see "Final test pass" below |
| v2 the default | done, 2026-10-24 | (this commit) | `petal propose-edit` uses v2 unless `--v1` (`cli/args.rs`); MCP `ProposeEdit` defaults to v2 (the argv carries no engine flag); hint names `tunable`/`rigid` accepted; tests `core/tests/cli_propose_edit_v2.rs`, `test/vitest/petal-mcp.test.ts` updated; see "v2 the default" below |
| Garden drag mode (M4c) | done, 2026-10-24 | `58144b0d`, `43eea4ff`, `35c7ef95`, `8c9c32cb` (+ this commit) | Petal IDE drag mode (`:DragMode`, toolbar Drag, Cmd+Shift+D): modifier-free drags, the drag result panel (alternatives, candidates with state, session chips), `tunable`/`rigid` toggled by mouse as source rewrites (`garden-app/src/dm_hints.rs`), Garden `:DragEngine auto` = v2 for every shape; examples `garden/examples/panels/dm-v2/` (7); tests `dm_hints` (12), 5 `garden-app` tests, `the_drag_mode_examples_solve`, `garden/tools/dm-mode-integration-test.ts` (36 checks); see "Garden drag mode" below |
| One knob tier | done, 2026-10-24 | (this commit) | `config let` and `tunable` share one weight, 0.5 (`rank::KNOB_WEIGHT`, `Param::is_knob`); the `config` selector gains the alias `knob`; study re-run with 0.25 and 0.5 (same outcomes; 0.5 changes no run count); test `dm_v2_solve::config_and_tunable_are_one_tier`; see "One knob tier" below |

Deviations from this plan are recorded here as they happen, with the reason.

**M1 (speculative runner), 2026-10-23.** What landed: R1 (overrides, gate,
capture, loaded set; `SpecHooks` in `backend/bytecode/vm/spec.rs`, consulted
by the hooked path of `Vm::step_in` (`LoadConst` overrides, branch
fingerprint, watched terms), `call_native_fn`, frame pushes and
`deliver_value`), R2 (`NativeEffects::contained` / `no_feedback`, every core
`effect` row contained, petal-ui and garden-script rows classified,
`Env::mark_handle_class_spec_safe`, `PB_FX_CONTAINED` / `PB_FX_NO_FEEDBACK`
and `fx::Contained` / `fx::NoFeedback` in the C bridge), R3
(`Env::slot_literals`), R4 (`Stack::drop_derived`, now also used by
`restore_execution` and `transfer_stack_state`), R5 (watched terms recorded
by the hooks as `ObsValue`; the Env's trace, observations, profile and
static audit are swapped for idle ones around a spec run,
`Env::run_spec_fork`), R7, R8, R11 (`SpecHost`, `NoHost`, petal-ui
`FrameCore::spec_parts` → `FrameSpecHost`), R12, R13. The bench numbers for
acceptance item 13 are at the end of this entry.

Deviations:

1. *`EmitKey` in a loop.* Item 9 says five emits from one call in a `for`
   loop get occurrences 0 to 4. With the loop path in `path_hash` (§2.1.6),
   each iteration has its own path and occurrence 0; the five keys differ by
   `path_hash`. Occurrences 0 to 4 are what a `forEach`/`map` callback gets
   (no `Index` part). The test checks both. `loop_path_hash` also folds the
   `Call` parts of the path, so the same loop under two callers is two
   places.
2. *Fingerprint.* Instead of separate per-term `StateInit` counts, the
   fingerprint folds `(function, ip, next ip)` of every conditional
   instruction: jumps, `ForEachNext`/`RangeNext` (trip counts), match arms
   and `StateInit`, whose hit jumps past the init block and whose miss falls
   through. Same information, one mechanism (items 10 and 17).
3. *`ObsValue`* gained `Variant { tag, data }` (an enum variant, which is
   what a draw command is); a path step `Field("x"|"y"|"z")` reads a vec
   list; a dual records its primal. *`RunError`* gained `Unsafe(class)`.
   *`loaded`* is a `ParamSet` (no new dependency for `FixedBitSet`).
4. *Baselines and the replay tables.* Both baselines draw from the forked
   stream and ask the host-data provider; the first's records are what later
   runs replay, the second records into tables of its own and must agree
   (§2.1.5). A host-data answer that cannot be carried to another heap (a
   handle, a closure) is recorded as "ask again" and called through.
5. *`add_params` takes the host* (`add_params(env, host, params)`): it
   re-runs the baseline once so the loaded set covers the new parameters,
   and that run needs the host's providers like any other.
6. *Garden's `SpecHost`* is deferred to M4 (Garden surfaces): nothing opens
   a session in Garden before then. The garden-script rows (`row`,
   `column`, `mutate` contained; `invalidate`, `panel_store_set` escaping)
   are classified in M1 as planned.
7. *Effect audit.* The "report an escaping effect seen in spec mode"
   extension of `effect_audit` is not done: each run already lists what it
   suppressed (`SpecRun::suppressed`), and `BaselineMismatch` names them.
8. *Recompile oracle* (item 11) edits entry-file literals only, probes ints
   by `+1` and floats by `+1e-3` (§2.3.4), and when both strategies fail it
   compares only that both failed (the messages quote different source
   text). It leaves one compiled program per run loaded; it is test-only.
   It runs over the seed set `core/tests/dm_v2/*.ptl` until M5.
9. *Baseline step budget* is `max_steps` or 10⁹; perturbed runs default to
   ten times the baseline's steps, at least 10,000.
10. *Hot path.* A spec run sets `Vm::hooks`, so every instruction takes
    the hooked path of `Vm::step_in`, which is where the override is
    applied: neither the straight-line loop (`fast.rs`) nor `exec_inst`
    (`dispatch.rs`) changed. A first version checked `Vm::spec` in
    `exec_inst`'s `LoadConst` and at the top of `call_native_fn` and cost
    the native-heavy `spreadsheet` benchmark 2.5%; the gate now sits before
    the call chain is built, `call_native_spec` is out of line, and the
    ordinary path hands natives no tap. The off-path cost left is one
    `Option` test in `call_native_fn`, `push_frame`/`frame_from_pool` and
    `deliver_value`, and one in `PetalCxt::rng_next_f64` and `push_output`.

Bench (acceptance item 13), release builds of `d4de0db6` and of `07e81adf`
on the same machine, interleaved. `petal run test/benchmarks/*.ptl`,
median of 16 runs (wall time, process start included): spreadsheet 192.5 →
193.6 ms (+0.6%; a second series +0.5%), calls 29.7 → 29.6, life 16.2 →
15.7, match_dispatch 330.1 → 329.0, audio_synth 95.8 → 95.8. `bench_panel
<app> 120 1200x800 --wiggle`, p50 frame ms over five runs, memoized /
`--no-memo`: analytics-dashboard 0.52 / 1.84 → 0.52 / 1.86,
server-monitoring 1.15 / 4.06 → 1.13 / 4.05, photo-adjust 0.60 / 2.23 →
0.60 / 2.24, notes 0.22 / 0.48 → 0.21 / 0.48, spreadsheet 0.71 / 0.79 →
0.73 / 0.80. Everything is within about 1%, at the noise floor of these
measurements; the native-call-heavy spreadsheet is the one that leans
positive (≈ +0.5 to +1%).

**M2 (discovery, Jacobian, hints), 2026-10-24.** What landed:
`dm_v2::hints` (`read_hints`, checked against the core's `NativeFnId`s,
`NativeFnTable::hint_tunable` / `hint_rigid`), `dm_v2::params` (`Param`,
`CodeOrigin`, `Anchor`, `anchors_for`, `discover_params`, `ParamSelector`,
`ParamFilter`, `eligibility`, `RunBudget`, `screen_params`), `dm_v2::jacobian`
(`FdOptions`, `Jacobian`, `ColumnQuality`, `jacobian`, `collateral_rows`), R10
(the `tunable` / `rigid` builtins in `builtins/hints.rs`, declared in
`core/packages/core/src/dm.ptl`, CONST; the lints; docs/Builtins.md
"Direct-Manipulation Hints"; `stdlib.json` regenerated with a `hints`
category), `rigid` replay in the gate, and the no-feedback facet set on the
core rows and read by discovery. `Env::native_table()` exposes the natives to
discovery. R9 (`float_like` → `pub(crate)`) is left to M3, its only user.

Deviations:

1. *Signatures.* `read_hints(program, natives) -> Hints` (marks, bound
   literals, hint calls, disabled names, warnings) instead of a bare map: the
   `NativeFnId` check of §2.6 needs the table. `discover_params(program,
   natives, hints, anchors, call_edges) -> Discovered { params, truncated }`,
   with `anchors_for` returning `Anchor { term, chain }`: the emit's chain
   lets the walk leave the anchor's function through the caller that really
   called it (below). `screen_params` and `jacobian` take the `SpecHost`, as
   every run needs it. `RunBudget::deadline` is an `Option<Instant>` (wasm has
   no clock).
2. *`Param` identity.* `term` is the `Constant` term (what a run overrides);
   `value` is the value as written, sign included, and `negated` says the
   constant holds `-value`. `Param::override_for` / `override_int` handle the
   sign and keep an int an int.
3. *Item 4 (`rng_cuts_path`).* "Columns of exactly 0" predates the review's
   revision of §2.1.4: the uniform is replayed and mapped through the
   native's current arguments, so `random_int(lo, hi)` does move with `lo`.
   What makes the literals receive no feedback is eligibility: every path is
   through a no-feedback argument (`through_rng`), so they are excluded before
   screening and never measured unless the client opts in
   (`IncludeThroughRng`). The test checks exactly that.
4. *No-feedback rows.* Set on `random`, `random_int`, `choose`, `noise`, and
   also `hash_int`, `hash01`, `hash_str` (deterministic pseudo-randomness;
   moving a seed literal "moves" a shape at random). Any `reads(RNG)` row is
   treated as no-feedback whatever it says. The scope line says "wired into
   discovery and the gate": the gate has nothing to do with it (a
   no-feedback native is called as usual; only `rigid` is replayed), so only
   discovery reads it.
5. *v1 looks through `tunable` only.* R10 said identity look-through for both
   hints in `provenance::alias_target`. For `rigid` that would let v1 edit a
   literal the author declared rigid, so v1 keeps refusing it as a computed
   argument (overrides R10 for `rigid`).
6. *Eligibility moved up from M3.* `ParamSelector`, `ParamFilter` and
   `eligibility` (§2.5.1, §2.7) are in `params.rs`, because M2 items 11 and 12
   test them. `Eligibility { screen, edit }` separates "measure it"
   (`ReportLibrary` screens library literals) from "offer it". Explicit
   selectors (`Term`, `Span`, `Variable`, `Line`, `Function`) lift the default
   exclusions (rigid, structural, state-init, through-RNG, path deps) for what
   they name; the broad ones (`ConfigOnly`, `AllUserCode`) do not, and the
   `Include*` / `AllowPathDeps` modifiers lift one exclusion each. The state
   rule uses `EligibilityContext::checkpoint_has_state`, one flag per
   checkpoint; the per-slot check (M3 item 15) is left to M3.
7. *The static walk.* A 0-1 BFS over (term, context, how-reached bits).
   Context-sensitivity: entering a callee through its results at a call
   records that call, and the callee's parameter maps back to that call's
   arguments only; in the anchor's own frames it follows the emit's chain;
   anywhere else, every caller (static, observed, and calls a closure is
   handed to). A dynamic call with observed edges follows only those, and its
   callee expression is not walked (that is what keeps 2,000 literals behind
   `fs[k](…)` down to the one function called). Records are field-insensitive.
   *Structural* means every path goes through a comparison, an index, a
   `range` argument, a numeric loop's end, a branch condition, a `match`
   subject or a loop key. Control dependence (which branch an emit is in) is
   not followed. Visits are capped at 2·10⁶ (`Discovered::truncated`).
8. *Distance.* Entering a callee costs 1, leaving it back through the call it
   was entered by costs −1, leaving through the chain or an unknown caller
   +1, and cell and state edges +1. So `2` in `draw(place(2))` is at 0 and the
   literals in `place`'s body at 1. `in_goal_frame` is "written inside the
   span of the goal's user-frame call" (`pick_frame` on the entry file).
9. *Span offsets count chars, not bytes* (the `std` prelude is not ASCII).
   Discovery reads a literal's text by chars. M1's Recompile oracle slices
   bytes; its seed files are ASCII, so it is unaffected, but M3's
   `apply_edits_multi` must splice by chars.
10. *Screening* uses the standard shortcut of adaptive splitting: when the
    first half of a group that moved does not move, the second half is known
    to and is split without a run of its own. Item 7's program (203 eligible,
    3 relevant) screens in 12 runs. A budget that runs out keeps everything
    not yet screened.
11. *Column details.* A `Step` found by escalation is taken over one more ×10
    when that still fits under `0.1 · max(|p|, 1)`, so the secant averages over
    at least one jump (`round(y) * 10` at `y = 20.3` gives 9.85). A `Kink`
    column holds the central (average) slope; the one-sided slopes of the first
    goal row are in the quality. A failed probing run makes the column
    `Errored` (no one-sided fallback). A `BranchFlip` retries at `h/10` twice
    and keeps the one-sided slope of the side that stayed.
12. *Lints* are AST rules in `lint/dm_hints.rs`, report-only:
    `dm-hint-not-literal`, `dm-hint-bounds` (a bound that is not a literal, and
    `min > max`, which the proposal called an error; lint has no severities),
    `dm-hint-state-init`. A name bound to a computed value is reported where
    the hints are read (`Hints::warnings`), which sees bindings.
13. *`rigid` replay* keys the baseline's answer by (chain hash, loop-path
    hash, occurrence), like the random draws, and an answer that cannot be
    carried to another heap is asked again.
14. *Not yet used.* `Hint::Tunable`'s range and step are recorded for M3's
    projection and rounding; proposal open question 2 (`step: 1` means stay an
    int) is M3's to settle.

**M3 (solver, ranking, disambiguation, public API), 2026-10-24.** What
landed: `dm_v2::solve` (`solve_dense`, `rank`, `linear_step`, `Bound`,
`GoalRow`, `SolveCtx::{solve_subset, round_solution}`, `ladder`),
`dm_v2::rank` (`param_weight`, `ScoreWeights`, `score`, `collateral_norm`,
`candidate_subsets`, `strength`, `SubsetKind`), `dm_v2::api`
(`propose_edits_v2`, `DmSession::{open, preview, finish}`, `DmRequest`,
`DmGoal`, `GoalTarget`, `Prefer`, `DmResponse`, `Alternative`,
`ParamChange`, `FileEdit`, `Collateral`, `CandidateReport`,
`DmDiagnostics`, `ExplainLog`, `apply_edits_multi`, `program_files`), R9
(`literal_edit::float_like` is `pub(crate)`), R14, the v1 fallback for
non-numeric goals, and `SpecSession::drew_random`. Every M3 acceptance item
has a test in `core/tests/dm_v2_solve.rs`, which also checks each accepted
alternative end to end (its edits applied to the text, the edited program
compiled and run from scratch, the goal met there) and compares v2 with v1
on v1's cases.

Deviations:

1. *Goals are constraints.* §2.4.1 put the goals in the objective with the
   collateral and the cost terms beside them. A penalty form lets the
   collateral term pull the solution off the goal: for one literal that
   also moves `k` of `n_c` collateral rows as much as the goal, the optimum
   falls short by a factor `1 / (1 + λ_c k / n_c)`, which for a 100-pixel
   drag with `k = 4`, `n_c = 20` is 1 pixel, twice `τ`. `linear_step` instead solves `min Σ (d_i Δ_i / s_i)² + ‖J_c Δ‖² /
   ‖r‖²` subject to `J_g Δ = r` (the KKT system) whenever `J_g` has full
   row rank, and falls back to the weighted least squares of the goals,
   regularized by a vanishing multiple of the same objective, when the goals
   conflict or there are fewer parameters than goals. Collateral is measured
   relative to the drag (`1/‖r‖²`), which makes the solve's objective the
   square of the score's `edit` and `collateral` terms.
2. *Damping is a step length.* With the goals as constraints, `μ` no longer
   shortens a step, so the loop of §2.4.2 keeps its shape with a step factor
   instead: a step that does not lower `Σ (w_j r_j / τ_j)²` is retried at a
   quarter of its length (down to 1/1000), an accepted one doubles it back
   toward 1. Broyden updates scale `Δp` per parameter by `max(|p|, 1)` so
   that literals of different size mix. The columns are measured again (one
   run plus two per column) every third accepted step or after the control
   flow changed, only for subsets of at most three columns and when the
   budget has room; larger subsets rely on Broyden.
3. *`collateral_norm` is a root-sum-square,* `‖Δ other rows‖ / ‖Δ goals‖`,
   not an RMS: an RMS over every row is diluted by the shapes that did not
   move, so moving one of a hundred shapes would cost almost nothing. A row
   that disappeared counts 1.
4. *Sort tiers.* "Config-only alternatives are listed first when they
   verify" and "∞ when not verified and a verified alternative exists" are
   implemented as sort tiers: verified config-only (under `Prefer::Soft`),
   then verified, then unverified, each by score. `Prefer::ConfigStrict`
   pins every non-config named binding once the program declares a
   `config let`, and uses only config and tunable literals when one of them
   moves the goals.
5. *Sparse spreads.* The config-only, client and full subsets are pruned
   before they are predicted: a literal that moves the goals by less than a
   tenth of the drag is dropped and the rest solved again (keeping the goal
   rank). A spread that comes down to a tuple is that tuple. Without this
   the full set proposed edits like `20 → 19.9` beside the edit that did the
   work. Item 1's second half (the first circle deleted) then lists the
   two-literal spread `2.0 → 2.4, 30 → 31.4` (score 1.02) between `30 →
   37.5` (0.675) and `20 → 35` (1.125), as §2.5.4 scores it; the test pins
   that order with `max_alternatives: 4`.
6. *Clock coupling.* The criterion of §2.5.4 (a column that changes by more
   than 25% or flips sign when the clock is shifted) also flags a radius at
   most phases, since `cos(time() * rate) * radius` changes with the clock
   too. A column is coupled when it changes that way *and* points along the
   goal emits' own clock velocity over their rows (`|cos| > 0.95`): a rate
   moves the shape along its path, a radius across it, a position not with
   the clock at all. The velocity comes for free from the shifted session's
   baseline. Detection is static (the program calls a `reads(CLOCK)`
   native, and `time` or `frame_count` is bound on the checkpoint), not
   from `run_deps`; pointer coupling is not detected. The strongest four
   usable columns are checked, costing two runs each plus the shifted
   session's baselines. Item 10's program binds the radius once (`let rad =
   11.0`): with the radius written twice, the x-only literal is partly along
   the motion (`|cos| = |sin(orb)|`), which is a real ambiguity.
7. *The state rule is per slot through the loaded filter.* A `state`
   initializer whose slot the checkpoint holds is never evaluated by the
   baseline, so it fails the loaded filter with the reason "a `state`
   initializer whose state the checkpoint already holds"; one whose slot is
   missing (a new key) runs and is eligible. The API passes
   `checkpoint_has_state: false` to `eligibility` (item 15; M2 open issue).
8. *Request and response shapes* (§6.3). `DmGoal::target` is a
   `GoalTarget` (`Num(f64)` or `Value(StaticValue)`), so a string or bool
   goal can be asked for and answered by v1; `DmGoal::to/value/within` build
   goals. `DmRequest` gains `max_iters` (default 6). `Alternative` gains
   `predicted` (the linear prediction), `rng_difference`, `score_parts`,
   `kind` (the subset it came from), `iterations` and `engine`;
   `ParamChange` carries `old_text`, `function` and `config`. `DmError`
   gains `Edit`. `DmSession::runs_used` is new; `finish` takes one target
   per goal; `preview` rounds to 6 decimals and makes no runs.
9. *v1 fallback.* A request with a non-numeric goal is answered by
   `propose_edits_batch` on the call that made each goal's emit (`EmitArg`
   maps to the native call's argument, `Watch` to the chain call that takes
   the watched term); `allow`/`deny` variables become v1's
   `Configurable`/`Static` policy. Its alternatives have `engine: V1` and
   `verified: false`: a string literal cannot be overridden, so they are not
   re-run.
10. *Default tolerance.* "Draw coordinate" is decided from the emit: `0.5`
    for a raw argument of a native whose emitted value holds only ints (it
    truncated), else `1e-6 · max(|t|, 1)`.
11. *Budget.* `max_runs` counts every speculative run of the request
    (screening, Jacobian, verification steps, rounding, the final run of the
    rendered text, the sequential-draw check, the clock shift), not the
    session's baselines. When it runs out before any column is measured, the
    response has no alternatives and a warning, not `NothingToMove`.
12. *Screening fix (M2 code).* Int members of a screening group step by
    exactly `±1` with alternating signs, so `offset + 50` cancelled and the
    group was dropped as inert. A group with two or more ints that moved
    nothing is probed once more with every int stepped `+1`.
13. *Ints and steps.* An int literal without `allow_respell` is projected
    onto the integers at every step, so the loop is integer-constrained. A
    `tunable` step projects onto `min + n · step` (or `n · step`), cleaned up
    to the step's decimals; the spelling follows the literal, which settles
    proposal open question 2: `step: 1` on an int keeps an int, on a float
    keeps a float.
14. *Rounding.* The ladder is: the int (for an int literal), the literal's
    own decimals, 0 to 6 decimals, 9 decimals, each projected onto the
    bounds. The nicest entry the linear model keeps within `τ` is picked per
    parameter and the picks are run jointly; when that misses, parameters are
    rounded one at a time, largest relative change first, at most four runs
    each. A best effort (unverified) solve is rounded only within `1e-9`.
    The text is then parsed back and run once more if its value differs
    from the last run's.
15. *Signs.* A negative value for a literal written without a minus is
    parenthesized when the character before it is `-` or `+` (`100 - 5` →
    `100 - (-3)`), or when the file does not parse with the bare text.
16. *Kinks.* The column of a `Kink` holds the central slope; picking the
    side of the goal's direction (§6.2) is left to the verification loop and
    Broyden.
17. *v1 parity.* For v1's cases (a literal in the call, a binding, a
    `config let`, `+ - *` chains), every v1 proposal is among v2's
    single-literal alternatives, to the same value or, where v1's is
    unwieldy (`6.666666666666667`), to the fewest decimals within `τ`
    (`6.7`); where v1 has a single answer, it is v2's top one; and
    `ConfigStrict` returns exactly v1's set on a program with a `config
    let`. v2 also proposes literals v1 misses (the `10` of `10 + w * 2`).

**M4a (CLI, MCP, docs), 2026-10-24.** What landed: `petal propose-edit
--v2` (§4.2) with `--arg`/`--native-arg`/`--value`, `--field`, `--to`/`--by`,
`--allow`/`--deny`, `--alternatives`, `--explain`, `--tol`, `--max-runs`,
`--max-screen`, `--allow-respell`, `--seed`, `--config-strict` and `--apply
[--pick N]`; `run --trace-emits` reports each emit's `key` and
`native_args`; the MCP `ProposeEdit` takes the same options and `TraceEmits`
a seed (§4.3); the CLI reference, the MCP page, the user page
`features/direct-manipulation.md` (rewritten for v2: what can be edited, what
cannot receive feedback, the sliding scale, hints, client disambiguation,
how speculative runs stay safe, limits, the agent workflow) and the
changelog. The `--json` shape is frozen by
`core/tests/cli_propose_edit_v2.rs::json_shape_is_frozen` and written out in
`docs/CLI.md`. Deviations:

1. *Where the shared CLI code lives.* §4.2 put the parser and JSON rendering
   in `dm_v2::cli`; they are in `petal::cli::dm_v2` (`core/src/cli/dm_v2.rs`,
   public): `V2Parser::flag` (leaves the index on the last token it
   consumed, the core CLI's convention, so a host binary can mix in its own
   flags), `V2Opts`, `propose(env, host, pid, checkpoint, opts, entry_path)
   -> serde_json::Value`, `print_text`, `baseline_emits`, `parse_selector`.
   `petal-ui-run propose-edit` (frames, size, scenario, host data, its
   `SpecHost`) is not in M4a: it is a ~40-line caller of these, left to
   whoever owns `petal-ui-run` (M5's study needs it).
2. *Engine selection.* v1 stays the default for v1-shaped requests (`--arg`
   + `--to` on one emit), as §4.2 says until M5. Any v2-only flag, or goals on
   more than one emit, selects v2 without `--v2`; `--v1` with a v2-only flag
   is an error naming the flag. The MCP tool follows the same rule.
3. *Extra goal forms.* `--value` (an `EmitValue` observable), `--field <path>`
   (a `PathStep` path) and several emits per request (each goal binds to the
   `--channel`/`--emit` given last before it) were not in §4.2; they map
   one-to-one onto §6.1's `Observable`.
4. *`--configurable`/`--static` under v2* are plain `--allow`/`--deny` of a
   `Variable` selector (an exact allow), not an "allow with preference":
   `ParamFilter` has no preference tier, and `config let` / `tunable` already
   carry the soft preference. `--config-strict` gives v1's rule.
5. *`--tol`* is one tolerance for every goal, not per goal.
6. *JSON names.* The MCP result is the CLI's JSON unchanged (snake_case:
   `max_error`, not §4.3's `maxError`), so there is one shape. `max_error` is
   in units of each goal's tolerance (as `solve::max_error` computes it);
   `misses` was added with the same distance in the goal's units, and the
   text output prints that. EmitKey hashes are 64-bit hex strings (JSON
   numbers are not exact past 2^53).
7. *`--apply` under v2* refuses an unverified alternative, including v1
   fallback ones (a non-numeric goal); the message points at `--v1 --apply`.
   It checks every target path before writing any file.
8. *`run --trace-emits`* gets `key`/`native_args` from a separate
   speculative baseline of the fresh stack, opened before the live run (it
   changes nothing). They are merged per channel only when the baseline
   emitted as many values on that channel as the live run, and only for a
   single run without `--state-storage`.
9. *Found, not fixed (M3 engine; fixed by the code review, `0d6dae6e`):* for a `Watch` goal (`--arg`) or an
   `EmitValue` goal (`--value`), `collateral_rows` keeps the goal emit's
   own native-argument row, which carries the same number, so every
   alternative "moves 1 other emit" (the goal itself). Ranking is mostly
   unaffected (all alternatives pay it), but the description misleads.
   `--native-arg` goals are not affected. The fix belongs in
   `jacobian::collateral_rows` (drop a row of the goal emit whose baseline
   value equals the goal's baseline and whose column equals the goal's).
10. *Docs not in M4a's files:* §4.5's updates to
   `features/program-modification.md`, `features/time-travel.md` and
   `usage/ffi.md` were not made (M4a owned the CLI, MCP and user DM pages);
   `dev/native-effects.md` was updated in M1.

**M4b (Garden), 2026-10-24.** What landed (commits `6da29349`, `1f565eca`):
Petal IDE's Cmd-drag solves through function calls with the v2 engine.
`garden-script`: `PanelHost::{set_dm_checkpoint, pin_dm_checkpoint,
begin_drag_v2, preview_drag_v2, finish_drag_v2, cancel_drag_v2,
propose_drag_edits_v2}` in `garden-script/src/panel/dm_v2.rs`, Garden's
`SpecHost` (R11), `DragFilter`/`DragSelector`, `DragCandidate`,
`DragAlternative`, `DragOutcomeV2`, `apply_rewrites_to`, and
`SourceRewrite::file` for edits in imported modules. `garden-app`
(`app/manip_v2.rs`): engine choice per drag, ranked alternatives
(`:DragNext`, `:DragPrev`, `:DragPick N`), client disambiguation
(`:DragAllow` / `:DragDeny` with `name`, `line:N`, `fn:NAME`, `config`),
`:DragEngine auto|v1|v2`, and `/state`'s `dm` (feature `state.dm-drag`).
Core: `DmSession::cancel` and `DmSession::renew_budget`. Tests:
`garden/garden-script/tests/dm_v2_drag.rs` (13, including
`drag_through_helper_v2` and the shipped `drag-to-edit.ptl` demo), five new
`garden-app` tests, and `garden/tools/dm-drag-integration-test.ts` (15
checks over the debug server). Docs: `garden/docs/petal-ide-mode.md`
(the drag section rewritten), `keybindings.md`, `debug-server.md`,
`architecture.md`.

Measured on a 300-circle sketch placed through a helper (debug Garden,
`petal` at opt-level 3): a frame 2.6 ms, the same with the checkpoint armed
2.3 ms (the fork is within noise); opening the drag 87 ms (31 runs); a
preview 1.2 ms; the release 6.8 ms.

Deviations:

1. *v1 stays for what it sees exactly.* `:DragEngine auto` (the default)
   uses v1 when every position argument of the grabbed call is a literal or a
   binding written in the call (exact, free, and the edit follows the pointer
   frame by frame), and v2 for everything else, and for every drag while
   `:DragAllow` / `:DragDeny` is set. §4.4 said v2 "becomes the default when
   stable"; this is that default for the cases v1 cannot answer.
2. *R6 is host-side.* No `Env::set_dm_checkpoint`: `PanelHost` forks the stack
   in `FrameHooks::prepare` (after the frame's inputs are bound, before it
   runs) for each frame that runs, keeps one, and drops it on a reload. A press
   pins it (§2.1.1's pre-mouse-down checkpoint), so frames that run before the
   first move cannot replace it. Without a checkpoint the live stack is used
   (right for a still sketch).
3. *A probe run finds the `EmitKey`.* `DmSession::open` needs keyed goals up
   front, so `begin_drag_v2` first opens a bare `SpecSession` (two baselines)
   and takes the k-th draw-channel emit whose chain equals the command's
   recorded origin (k counted among the frame's commands), falling back to
   the raw command index. A command with no matching emit is refused rather
   than asserted.
4. *Both axes are goals.* The axis the drag did not move is held at its
   baseline, so a solution cannot slide the shape off-axis. Goals are on the
   drawing native's raw arguments; a record argument is read through
   `Field("x")` / `Field("y")`, so `draw_rect(rect, c)` and the `center`
   overloads drag too (v1 refuses them).
5. *The preview is the editor and a ghost.* While the pointer is down the
   panel's live binding is held (the session's program keeps running); the
   editor shows the predicted literals, rewritten each move from the
   press-time text; the canvas paints only the grabbed shape translated by the
   pointer offset. The release writes the verified answer and the canvas
   recompiles. No separate overlay of literals.
6. *The picker is the status bar and commands.* After a release the status bar
   names the edit, its rank (`[1 of 3: :DragNext]`) and the other eligible
   literals that move the shape on the dragged axis (`also movable: x0
   (:DragAllow)`); the full candidate report is in `/state`. `:DragAllow`
   is exact and persists across drags until cleared. Holding Alt to cycle
   (the stretch) was not done.
7. *Budgets.* Open: 96 runs / 400 ms; release: 48 runs / 400 ms
   (`renew_budget`, so time the pointer is held is not the solver's);
   `max_iters` 3; 250 ms per run; 3 alternatives.
8. *Other files.* An edit in an imported module goes into that file's editor
   buffer when one is open, else to the file on disk. Cycling alternatives
   rewrites every touched file from its press-time text.
9. *`file_origins` canonicalizes paths* (core, `6da29349`): the loader records
   an imported module by its resolved path and the entry by the path it was
   given, which on macOS (`/var` → `/private/var`) made a sibling module a
   path dependency, so it was never edited.
10. *Acceptance item 3's integration test* is a new file,
    `tools/dm-drag-integration-test.ts`. It checks the moved shape by hovering
    its new position (the trace), since draw commands are integer pixels; the
    unit tests assert the move to ±1 px.

**M5 (examples ladder and complexity study), 2026-10-24.** What landed:
27 examples in `examples/direct-manipulation-v2/` (L0 to L10, the
adversarial A1 to A7, and a verbatim copy of `server-monitoring`), with 105
drag goals in their headers; the harness
`core-libs/petal-ui/tests/dm_v2_study.rs` (the asserted `ladder` test and the
ignored `full_study`, which adds a scaling sweep and an external pass over
every headless-runnable app of the ecosystem and rewrites the report); the
report [direct-manipulation-v2-study.md](direct-manipulation-v2-study.md)
with its JSON. Results: on the ladder every goal not marked as an expected
failure succeeds (91 of 105; the 14 others are written down with their
cause), the top alternative is verified on 94%, median 20 runs; on 258
automatic drags over 45 external apps the top alternative is verified on 93%,
median 56 runs. The bar of acceptance item 3 is met.

Deviations:

1. *The harness is Rust over the API*, not `tools/dm-v2-study.ts` over
   `propose-edit --json`: it runs while the CLI was being written (M4a), and
   in-process it can check each top alternative against the source edit
   itself. Its oracle applies the edit to a copy on disk, hot-reloads it into
   a copy run to the same checkpoint, and requires the reloaded frame to draw
   exactly what an override run of the original drew (every command, every
   number); a restart from scratch is recorded too (it differs exactly for
   edits that change how `state` evolves). Re-reading the goal by `EmitKey`
   or by draw-list position in the edited program is wrong (terms are
   renumbered; shapes before the goal can appear or vanish): seven false
   misses traced to that, none to the engine.
2. *Files and format.* The ladder is `l00`–`l10b`, `a01`–`a07` and
   `big-server-monitoring/` rather than `01`–`19` (all 19 planned programs
   are there; the README maps them), with hinted variants (`l01b`, `l07b`,
   `l09c`). Goals are `// dm-goal name: tag#k.arg += n[, …] | expect=… allow=…
   deny=… tol=… xfail=…`; `expect=` names bindings, `lit:<text>`, `fn:<f>`,
   `L<n>`.
3. *Acceptance item 1 is met with expected failures.* The examples carry
   harder goals than the plan's one per program, to find the limits, and 14
   of them are marked `xfail=` with the reason (cosmetic offsets, physics
   constants, counts, an auto-scaled axis, a translate/reshape tie, branch
   flips, step functions, int-only paths, hint ranges). Every other goal,
   including all of the plan's intended drags, succeeds; `a02-state` is
   verified with `stateful` set.
4. *Report contents.* Per-run cost is reported as milliseconds per run by
   emit count, not split into fork, statics rebuild and execution (the API
   does not expose the split). Static metrics are read from the source
   (lines, functions, loops, `var`, `state`, `config let`, hints) plus emits;
   max call depth is not reported. The external pass samples 6 drags per app
   (not 20).
5. *Six engine revisions* (the study's §4 has the before/after numbers: the
   ladder went from 82 to 91 successes, external verified from 77% to 93%):
   1. Edit cost `d_i · (share_i + 0.1 · min(rel_i, 10))` with
      `share_i = ‖J_i Δ_i‖₁ / ‖r‖₁` (`rank::edit_cost`, `ScoredChange::share`)
      replaces §2.5.4's `d_i |Δp_i| / s_i`, which preferred large absolute
      coordinates to the offsets, gaps and angles a drag means.
   2. The count penalty is charged per edit beyond the leanest *verified*
      alternative, not beyond the goal count.
   3. Lean subsets: config-only tuples (`CONFIG_TUPLE_COLUMNS`) before the
      all-config spread; with several goals, single literals too
      (`SINGLES_FOR_SEVERAL_GOALS`), and the closest and the closest smooth
      single always verified; predictions deduplicated by the literals they
      move. `subsets_verified` is now at most `max_alternatives + 4`.
   4. Budget shares: screening ≤ 30% of `max_runs`, screening + Jacobian
      ≤ 60%; at most `MAX_MEASURE` (32) columns, the cheapest by weight then
      nearest; past `MANY_COLUMNS` (8), one step escalation instead of three.
      Garden's 96-run open budget is split the same way (its tests pass).
   5. Collateral rows: `COLLATERAL_ROWS` 8,192 (was 512) and the numeric
      fields of record arguments; `SpecRun::read_all` indexes emits once.
   6. The score charges per-shape collateral (`rank::collateral_shapes`,
      reported as `Collateral::shapes`; `norm` is unchanged), compressed past
      1 (`rank::compress_collateral`).
   A seventh, unverifying an alternative whose flow change renumbered the
   goal call's emits, was tried and reverted (it rejected correct answers).
   `core/tests/dm_v2_solve.rs`: three tests updated for the new scores and
   orders (`through_helper_single_goal`, `config_preferred_softly`,
   `budget`). M3's open issue about the two-literal spread is moot: with the
   first circle gone it now ranks third, after `20 → 35`, because `30 →
   37.5` pays for its respelling.
6. *Limits found and left* (the study's §3): a literal whose column is zero
   until a branch flips is never re-measured past the flip; screening drops
   a step function that is flat at its 0.1% probe; on apps with 160+
   eligible literals per drag, screening's inferred positives can spend the
   Jacobian's share on zero columns (6 of 258 external drags); geometry held
   in `state` has nothing to edit at a live checkpoint; 152 of 197 external
   entry files need another host's natives.

**Code review (post-M5), 2026-10-24.** An adversarial review of everything
since `c9eb67b7`: effect leaks, live-state and cache mutation, determinism,
span splicing, formatting, solver edge cases, v1 parity. Fixed, each with a
failing test first in `core/tests/dm_v2_review.rs`:

1. *A verified edit that computes something else once written* (`0d6dae6e`).
   A literal receiving a method call (`5.map_range(...)`) that went negative
   was written `-2.map_range(...)`, which parses as `-(2.map_range(...))`.
   `needs_parens` now also wraps a literal followed by `.` or `[`.
2. *A value or watched goal counted as its own collateral* (`0d6dae6e`; M4a
   deviation 9). `Jacobian::drop_goal_aliases` drops a collateral row on the
   goal's emit with the goal's baseline value and the goal's derivative in
   every column (a real collateral row on the same emit stays).
3. *Previews without parentheses* (`08bc37cd`). `DmSession::preview`
   rendered `x - -10` and `-2.f()`; it applies the same neighbour rule now
   (without the parse check, which needs the Env).
4. *Default tolerances computed from a NaN target* (`1e6f7913`). A session
   opened before its targets are known (Garden's drag, the CLI's `--by`) got
   `1e-6` whatever the target; `finish` recomputes the default tolerances
   from the real targets (`--by` on 183 now gives `1.794118`, not
   `1.794117647`).
5. *`--arg k` on a name or a parameter never readable* (`4dea9231`). The
   argument's term is a `Copy` copy propagation removed, so no instruction
   wrote it and every such goal failed with "the baseline run has no
   number" (`push_output(s, x)`, `emit(v)`). The speculative VM now also
   notes a call term's watched inputs from the call instruction's operands
   just before the call runs (`Vm::spec_watch_call_args`, hooked path only).

Checked and found sound: forks are isolated contexts (heap, RNG, outputs);
the Env trace, observations, profile and static audit are swapped out and
the effect audit is never fed by the spec gate; memo is off and statics are
dropped on every fork; `slot_literals` only ever meets `LoadConst` (the only
lowering of a `Constant` term), and a hot reload of the written edit into
the same Env after a proposal runs exactly as the edited program from
scratch (`reload_after_propose_matches_fresh_run`); edits splice by
character offsets in every surface (core, CLI, Garden's
`apply_rewrites_to`), including non-ASCII imported modules; prelude and
package literals are never edited; the CLI rejects `--pick 0`, NaN/inf
targets and flag-looking selector values.

Left as open issues (not fixed): a best-effort (unverified) alternative
whose rounding made it worse keeps the unrounded value
(`1251.5532326870916`); a scientific literal (`1e3`) is respelled
`2500.0`; every speculative run deep-clones the checkpoint's heap
(`fork_execution`), which is the dominant cost for a panel with a large
heap; an emitted value is converted to `ObsValue` up to depth 5 × 256 per
level, which a deeply nested emitted tree could make expensive; screening
can lose a float/int pair whose probes cancel exactly (`1000.0 + n`: the
float steps by `+1.0`, the int by `-1`); `rigid` at a place the baseline
never reached (or with a non-portable answer) passes its argument through.

**Final test pass, 2026-10-24.** The whole test plan, run on `main` after
the code review:

| Suite | Result |
|---|---|
| core `cargo build && cargo test --no-fail-fast` | 3058 passed, 0 failed, 16 ignored (88 test binaries) |
| `make test` (full vitest, includes every `examples/console/*.ptl`) | 76 files, 1742 passed, 0 failed |
| petal-ui `cargo test` (includes the study `ladder` and the `effect_audit` corpus test) | 374 passed, 0 failed, 7 ignored |
| petal-c-bridge `cargo test` / `make test-c-bridge` | 44 passed, 0 failed / 4 of 4 ctest |
| petal-query `cargo test` | 33 passed, 0 failed |
| garden `cargo test -p garden-script -p garden-app` | garden-app 998; garden-script lib 138 (1 ignored), bloom 10, dm_v2_drag 13, script_host 19, text_layout 15; 0 failed |
| garden `tools/dm-drag-integration-test.ts` (headless, debug server) | 15 passed, 0 failed |
| petal-desktop-sdl `cargo check --all-targets` | clean |
| study `ladder` (release) | 105 goals, 91 succeed, 14 xfail, 0 unexpected |
| study `full_study` (release, external copies) | re-run; outcomes, run counts and edits identical to M5, only wall times moved |
| `tools/native-effect-audit.ts` (static) | 1 finding, petal-desktop-sdl `queue_on_host`: a test-only native in a `#[cfg(test)]` module, pre-existing (`473c89a2`) |
| `petal-ui-run --effect-audit --frames 3` on all 28 DM v2 example entry files | 0 under-declared, 0 over-declared on each |
| clippy, CI's enforced wasm32 `disallowed_methods` step | clean (the spec runner's clock read is cfg'd out of wasm) |
| clippy, host, on the dm_v2 modules | 7 warnings, 4 fixed (`4a8d7145`); the 3 `needless_range_loop` in the linear algebra are left as written. No new warnings in the other touched core files or in Garden's DM files (the ones there predate v2) |

Ecosystem: no `.ptl` in `examples/`, `garden/`, `~/worlds-fair/ui/ptl`,
`~/petal-creative`, `~/cheesecake/games` or `~/biz/petal-lang.org` calls or
defines `tunable` or `rigid` (the words occur only in comments), and no
host registers a native by either name. `petal check` over all 535 `.ptl`
files of those trees (examples 188, cheesecake 304, worlds-fair 20,
petal-creative 23) passes on 532; the 3 failures are cheesecake `rivet`
shaders calling the host's undeclared `hash11`, unrelated to v2.

Deviations in this pass: the study's external copies were made in the
session scratchpad (`/Volumes/bureau` was not readable), and the generated
report's paths are rewritten to the documented `/Volumes/bureau` location.
The study's run instructions (§7) were corrected: the examples path must
be canonical, and the copies must include `petal.toml`, without which
package host requirements are not enforced and 8 library modules load as
entry files (45 runnable becomes 53).

Still open at the end (all recorded above, none a test failure): the
milestone open issues (`petal-ui-run propose-edit`; §4.5 docs for
program-modification, time-travel and ffi; re-vendoring the C bridge into
cheesecake and worlds-fair with `PB_FX_CONTAINED`; making v2 the CLI/MCP
default, which the study's numbers now support), the study's §3 limits,
and the code review's left-open list.

**v2 the default, 2026-10-24.** The owner decided to make v2 the default
engine and approved the hint names. What changed:

- `petal propose-edit` answers with v2 unless `--v1` is given (`--v2` is
  still accepted). `--configurable` / `--static` without `--v1` are read as
  `--allow` / `--deny`, as the v2 parser already did; under `--v1` they keep
  v1's meaning, and v2-only flags or goals on several emits are still
  refused there. No other Rust host parses `propose-edit` (there is no
  `petal-ui-run propose-edit` yet), and Garden's engine choice is its own.
- MCP `ProposeEdit` defaults to v2: with no `engine` the argv carries no
  engine flag and the CLI picks v2; `engine: "v1"` passes `--v1`.
- `tunable` and `rigid` are accepted ([proposal](../proposals/dm-v2-hints.md)
  status and open question 1; open issue 6 in §8).
- Output change for existing users: a v1-shaped request now prints v2's text
  (`engine v2 · seed 0 · …`, ranked alternatives) and v2's `--json` object
  instead of `N proposals:` / `{goals, proposals, …}`. Scripts that parse
  v1's output pass `--v1`, which keeps it byte for byte. Two answers differ
  for the same request: an `--allow` is exact where `--configurable` was a
  preference, and int literals stay ints without `--allow-respell` (the old
  docs example, `--to 42.5` on `let x = 20`, now lands on 43 unverified; v1
  proposed `32.5`). The CHANGELOG lists this under "Changed".
- Tests: `cli_propose_edit_v2.rs` `v1_output_is_unchanged_under_v1` now
  pins the default as v2 (and `--static` as `--deny`); `petal-mcp.test.ts`
  splits the no-engine case (v2) from `engine: "v1"` (legacy shape).
- Docs: features/direct-manipulation.md presents v2 as the engine and v1 as
  legacy; CLI.md, mcp-server.md and `petal help propose-edit` likewise.

| Suite | Result |
|---|---|
| core `cargo build && cargo test` | 3058 passed, 0 failed, 16 ignored (88 test binaries) |
| `npx vitest run` | 76 files, 1743 passed, 0 failed (needs `petal-ui-run` built; without it `verify.test.ts` fails 5) |

Ecosystem: nothing outside this repo runs `petal propose-edit` or reads its
output. `~/cheesecake/third_party/petal` and `~/worlds-fair/vendor/petal`
vendor a pre-v2 core (v1 only) and change only when re-vendored.
`~/biz/petal-lang.org`'s docs are generated by its `scripts/import-docs.ts`
from petal `44ee71c1` (release 0.1.0, before v2) and describe v1 as the
only engine, which is right for that release; the next docs import picks
up this change. `~/petal-creative` and `sample-apps` have no reference.

**Garden drag mode (M4c), 2026-10-24.** The first-draft interactive UI for
§2.7's client disambiguation, beyond M4b's status bar and ex commands
(`garden/docs/petal-ide-mode.md`, "Drag mode"):

1. *Modifier-free drags.* In drag mode a plain drag on a traced canvas is the
   drag-to-edit gesture and the sketch's mouse is suspended (its frames see
   the pointer off the canvas); Cmd-drag is unchanged outside the mode.
2. *The picker is a panel.* After a drag, a card over the canvas lists the
   ranked alternatives (click to write one, `:DragPick`'s path) and every
   candidate with a display state derived from the editor's current text
   (`free` / `tunable` / `rigid` / `config` / `fixed` with `why_not`), plus
   the session `:DragAllow` / `:DragDeny` lists as per-candidate `only` /
   `never` chips. M4b deviation 6's "status bar and commands" picker stays as
   the keyboard path.
3. *Hints by mouse are source edits.* Clicking an underlined number, a gutter
   square or a state chip cycles free → tunable → rigid by wrapping the
   literal in the real builtins, line-locally and as one undo step
   (`dm_hints.rs`); a `tunable` with bounds becomes `rigid(x)` (bounds
   dropped), bounds are never toggled, a multi-line hint call is not
   recognized. The alternatives on screen were solved without the new hint,
   so they are greyed until the next drag; the candidates' spans are carried
   across the rewrite.
4. *v2 for every Garden drag.* `:DragEngine auto` no longer picks v1 for a
   position written in the call (M4b deviation 1): v1 has no alternatives or
   candidate report and ignores the hints (it reads `rigid` as computed), so
   the panel and the toggles would work on some shapes only. For a literal
   the linear preview is exact; what is lost is v1's live recompile of other
   shapes sharing the binding during the drag. Opening a drag measured 30 to
   55 ms on the examples (debug Garden).
5. *Bug found on the way:* a drag pressed while the canvas ran older text than
   the editor (an edit made while paused) spliced the old spans into the new
   text. The press now recompiles the canvas from the buffer first
   (`PanelView::source_behind`); this predates drag mode (any Cmd-drag after a
   paused edit).

**One knob tier, 2026-10-24.** The owner reversed open question 2 of
[config-let-vs-tunable.md](../proposals/config-let-vs-tunable.md): a literal
marked by `config let` (including the literals of a config record or list),
by `tunable(...)`, or by both now weighs the same in §2.5.2, so the ecosystem
has one notion of a knob. What changed:

1. `Param::is_knob()` is the one test, used by `rank::param_weight`
   (`KNOB_WEIGHT`), `ParamSelector::ConfigOnly`, the config-only subsets,
   `Prefer::ConfigStrict`'s pinning, and `ParamChange::config` (hence
   `Alternative::config_only` and the CLI's `[config]` tag). Most of these
   already treated the two marks alike; the weight was the only split.
   `tunable` keeps its bounds as box constraints.
2. The selector `config` (CLI `--allow`/`--deny`, MCP, Garden
   `:DragAllow`/`:DragDeny`) gains the alias `knob`. Nothing is renamed:
   `config`, `--config-strict`, `Prefer::ConfigStrict`, `config_only` and
   the JSON shape stay, and now mean "a knob". A candidate's `config` field
   still says whether the literal is a `config let` (with `hint` saying
   whether it is `tunable`), so Garden's chips still show which marker a
   number has.
3. The value was chosen by the study (§7 of the study doc, external copies
   from a fresh `rsync`):

   | Weights | Ladder ok | Top verified | Median runs | Total runs | External verified (re-run ok) | External runs |
   |---|---:|---:|---:|---:|---:|---:|
   | before: `tunable` 0.25, `config let` 0.5 | 91/105 | 99 | 20 | 2,274 | 240/258 (240) | 16,535 |
   | one tier at 0.25 | 91/105 | 99 | 21 | 2,279 | 240/258 (240) | 16,535 |
   | one tier at 0.5 | 91/105 | 99 | 20 | 2,274 | 240/258 (240) | 16,535 |

   Both values left every outcome and every top edit as it was. 0.25 cost
   five more runs on `a01-rand`, whose knobs are both `config let` and so
   got cheaper (`star-right` 23 → 26, its third alternative now `200→202,
   160→178` for `200→203, 160→177`; `star-pair` 20 → 22). 0.5 changed no run count anywhere; only the
   trailing digits of two unverified lower-ranked alternatives of
   `l07b`'s `shrink-clamped` (an xfail) moved. So the tier is 0.5: the
   result that does best, and the smaller change (only `tunable` literals
   move, and the gap to a literal in the goal's own call, 0.75, stays).
4. No `dm-goal` expectation changed. `l07b` no longer relies on `tunable`
   being cheaper: `shrink` still wins `taller` and `rightmost-out` on the
   size of the change (its comment says so now).

**Canopy study fixes, 2026-10-25.** The Canopy side-scroller study
(cheesecake `games/canopy/DM-DEBRIEF.md`, 46 attempts) found two solver
gaps it marked "first":

1. *S1, collateral reads nested values.* Collateral rows stopped at one level
   (a number, a record of at most 8 numbers, a list of at most 4), so a list
   of records (the lives icons, every platform of a level) or a record in a
   record was invisible: a shared `MARGIN` that moved the three lives icons
   was reported as "moves nothing else" and ranked first (#26, #42).
   `jacobian::collateral_rows` now walks every container the recorded value
   kept (`ObsBounds::ARGS`, depth 3), up to `COLLATERAL_LEAVES_PER_ARG`
   (256) numbers per argument. A `CollateralItem` names the number that moved
   most (`emit`, `arg`, `path`, `moved`, `label()`: `lives[0].2.x +2`), and
   the description and the CLI list them. The ladder's outcomes, top edits
   and run counts are unchanged (its draw arguments were already flat).
2. *S2, screening that does not starve the goal's own literal.* With ~130
   literals in reach of a whole-level harness, screening flagged 50 to 140 as
   moving the goal, measured the knobs first (the weight sort; all zero in
   #36) and then went in source order, and the budget ended before the
   literal that mattered (#7, #12, #16, #19, #33, #36). Three changes:
   - *Focused reach* (§2.2 stage 4): a field- and element-sensitive second
     walk. A knob the goal's fields do not read (a decoration's `config let`
     in the same records) has no focus and waits behind every focused
     literal, in screening and in measuring.
   - *Primary goals first* (§2.2 stage 5): groups that move only held goals
     or only the flow are split last. The CLI marks `--by 0` goals
     (`DmRequest::holds`); the API also treats a target equal to the
     baseline as held, unless every goal is (Garden opens a drag that way).
   - *Measuring order and zero columns*: proven movers of a primary goal,
     then unscreened, then secondary; in each, focused first, then weight
     (the one knob tier, 0.5, is unchanged), then `screen_order`. A column
     that measures `Zero` gives its `MAX_MEASURE` place to the next
     candidate while the Jacobian's share of the budget lasts
     (`Jacobian::append_columns`).

   Before and after, on Canopy's harnesses (attempts re-created on the
   current game, targets moved to stay off the baseline; #16, #19 and #33
   with the workaround knobs removed, #26 with `coins` on `MARGIN` again):

   | Attempt | Before | After |
   |---|---|---|
   | #7 beetle `from_cp` (level.ptl) | 122 runs, wrong literal (the checkpoint offset) | 126 runs, the beetle's own `x0` |
   | #12 `to_edge` + hold `from_cp` | 120 runs, no alternative | 126 runs, verified (the beetle's `x1`) |
   | #36 `to_edge` + hold `x0` | 120 runs, no alternative | 126 runs, verified, the intended `8.0 -> 7.0` |
   | #16 thorns landing, unscoped (segment) | 185 runs, unverified (`RUN`) | 182 runs, verified (the trunk under the thorns) |
   | #19 same, `--allow fn:the_crown` | 137 runs, verified | 149 runs, same edit, collateral now listed |
   | #33 two coins on the bot's path | 188 runs, unverified (wall clock) | 187 runs, unverified (wall clock) |
   | #42 timer `y` | 12 runs, `MARGIN` first ("moves nothing else") | 12 runs, the timer's own `20`; `MARGIN` lists `lives[0].0.x +5` |
   | #26 coin panel | 10 runs, `MARGIN`, "1 other emit" | 6 runs, `MARGIN`, names the lives and the timer |

   (A run count near 120 to 126 is the request using its screening and
   Jacobian shares; the failures stopped there.) #33 remains: a run of that
   harness takes about 25 ms, and the hard-coded 5 s wall budget ends the
   request. On the ladder no outcome or top edit changed; total runs went
   from 1,921 to 1,866 (median 25 to 24), and no example got more than one
   run slower.

**Constraints between literals, 2026-10-25 (L3, the cheap part).** Canopy
#13, #23 and #29 asked for relations between literals (a patrol's end after
its start, a positive width). A run that raises an error was already not
taken (`new_err = ∞`), so an `assert` in the data constructor is such a
constraint today; what was missing was saying so:

1. `SolveCtx::run_at` records every run that raised an error
   (`solve::FailedRun`). Each alternative carries the ones its solve,
   rounding and verification made, grouped by error (`api::Rejection`: the
   message, the position the VM's annotation gives, the `assert`'s condition
   read from the source, the count, the literals the first one had moved);
   `DmDiagnostics` has `rejected_runs` and the request-wide groups. The text
   output prints `rejected: assertion failed at level.ptl:4 (x1 >= x0) · 3
   runs, first at ...`, and an unverified alternative's description ends in
   `rejected beyond it: ...`. Probing (FD) runs that fail stay on the
   candidate (`ColumnQuality::Errored`).
2. `round_solution` rounds a solve that stopped short too. Each goal gets a
   slack: τ when the solve verified (as before), else its miss plus τ, so
   the ladder (int, written decimals, 0..6 decimals, 9) picks the nicest
   value no worse than one tolerance, verified by a run. The boundary value
   `3.5546874999999756` prints as `3.55`.
3. Documented in features/direct-manipulation.md ("Constraints between
   literals") and the writing guide. Parameter-level `tunable` (L3b) and
   linear constraints in the solver (L3c) are not done.

**Range goals (C6), 2026-10-25.** The Canopy study could not state "the
bot passes within 0.6 of each coin" (#33, #34: an exact 0.2 the kinked
min-over-frames measure could not reach; #35 faked a band with `--to 0.3
--tol 0.25`), nor "clearance at least 1" or "inside the screen". Range
goals are now real goals, not CLI sugar for a midpoint and a tolerance:

- *Request.* `GoalTarget::Range { lo, hi }` (either end open;
  `DmGoal::at_most`/`at_least`/`between`); CLI `--at-most V`,
  `--at-least V`, `--within LO..HI`, one of these or `--to`/`--by` per goal;
  JSON `at_most`, `at_least`, `within: [lo, hi]` (additive keys). A drag's
  `targets` entry for a range goal is ignored. An empty or non-finite range
  is refused at `open`.
- *Solver.* `GoalRow` gains `range: Option<(lo, hi)>` with `holds`, `miss`,
  `error`, `aim`, `residual`, `active`. A range's error is 0 inside and
  `1 + miss / τ` outside, so `max_error ≤ 1` still means "holds" and the
  bound is checked exactly (τ is the margin, not slack). `goal_step` wraps
  `linear_step` in a small active set: rows that hold are left out (they are
  not pinned, which a zero residual would do); when the linear step would
  push one out, it joins, aimed one tolerance inside the bound it would
  cross, and the step is solved again (at most once per goal). Both the
  linear predictions and `SolveCtx::step` use it, and the rounding check
  uses `holds`, so a nice literal deeper inside a range is accepted.
- *Ranking.* `candidate_subsets` takes the active goal rows: tuples have one
  literal per goal that asks for something, and the count penalty and the
  spread pruning count active goals only. Screening treats a range the
  baseline is inside as held (§2.2 stage 5).
- *Report.* `DmDiagnostics::holds_at_baseline` (JSON `holds_at_baseline` per
  goal); a request whose goals all hold proposes nothing and warns `the goal
  already holds at the baseline (coins[0].y = 3.4 ≤ 5)`. Descriptions add
  `near[0].0 ≤ 0.6 (was 0.8448, now 0.5978)` per range goal (`keeps …` for
  one that held already); `misses` is the distance outside the range.
- *Tests.* `core/tests/dm_v2_ranges.rs` (each kind, already holds,
  unreachable, an equality with a `--by 0` pin and a range that joins the
  step, and a distilled #33/#34 harness: three `at_most 0.6` goals on a
  min-over-frames distance; the coin row's two knobs verify first, the
  beetle's `x` second), `cli_propose_edit_v2::range_goals_on_the_command_line`,
  `dm_v2_embed` (JSON keys, `propose_with_natives`).

  Equality-only requests take the same path as before (every row active, one
  `linear_step`), so the ladder is unchanged.

---

## 10. Review notes

An adversarial review on 2026-10-23 checked the first draft against the code
at `4d59549b`/`c9eb67b7`. The claims about late-bound `LoadConst` (no constant
folding anywhere, `lower.rs:349` the only producer), the fork, `get_int`
truncation, the input natives reading bindings and the RNG living in the
context all held. What changed:

1. **CLI host for UI scripts.** The core `petal` binary has no draw natives
   (`make_env` = `Env::new()`), so `petal propose-edit --channel
   draw_commands` on a `draw_circle` sketch cannot work. DM for petal-ui
   scripts runs under `petal-ui-run propose-edit`, with a shared
   `dm_v2::cli`. Core tests use a test-registered `draw` native (§1.7,
   §4.2, M2/M3 preambles, M5).
2. **Trace isolation.** The `TraceBuffer` is Env-wide and holds values
   weakly. Watch capture moved into `SpecHooks` as strong `ObsValue`s, and
   spec runs switch the Env trace off (R5 revised, M1 test 14).
3. **Host providers.** `host_data`, fonts and clipboard are thread-locals
   that the host installs only around its own run. Added `SpecHost` (R11).
   Without it, every petal-ui baseline would mismatch.
4. **`HOST_DATA` replay** by (native, args), so that perturbed keys cannot
   start host fetches (R12, M1 test 16).
5. **Core effect rows.** The draft's "contained" list missed
   `__declare_method` and the enum-data intrinsic, so suppression would have
   broken methods and enums. Now all core `effect` rows are contained, with a
   test. The petal-ui and garden-script rows move from M4 to M1 (R2, M1
   test 15).
6. **RNG.** Replay the uniform, not the result, so `random_int(0, w)` still
   responds to `w`. Feedback refusal moved to eligibility (`through_rng`). A
   `ForkStream` check flags `rng_order_changed` (§2.1.4, §7).
7. **Hidden regime changes.** Keyed state can silently re-initialize when a
   key or trip count moves. `StateInit` counts go into the fingerprint
   (§2.1.2, R7). `EmitKey` gains a loop-path hash (§2.1.6). Channels are
   iterated by `SymbolId`, because `output_buffers` is a randomly-seeded
   `HashMap` (§1.4).
8. **Checkpoint.** It is the frame before mouse-down, so that the gesture's
   pointer state is not measured. The session owns its fork, so an R6
   refresh cannot make it stale. The per-frame fork happens only when the
   program writes state (§2.1.1).
9. **Int literals.** Float probes are wrong for ints: int `/` truncates, so
   `10/4 → 10.001/4` jumps by 0.5 with no error or branch. Probes are
   type-preserving (`±1`) by default, and float probing needs
   `allow_respell`. Respelled edits are structural reloads (§2.3.4, §2.4.4,
   §1.4).
10. **Library literals are never editable.** These are the prelude, bundled
    packages (no file to write), the package cache and vendored copies.
    `IncludeLibrary` became `ReportLibrary` (report only). Path dependencies
    are opt-in. "User file" is defined by `SourceFile::origin` (§2.5.1, §2.7).
11. **New default exclusions**: structural literals (comparisons, patterns,
    indices, loop bounds, keys) and the hint builtins' own bound literals.
    `state_init` is excluded only when the checkpoint already holds the slot,
    so it is eligible on the CLI's fresh stack (§2.1.2, §2.5.1).
12. **Multi-file edits.** v1 `apply_edits` ignores the file, so edits now
    carry `FileId` and are applied per file against the running program's
    text (R14, M3 test 13).
13. **Scaling.** Eligibility runs before screening. Dynamic calls resolve
    through the baseline's observed call edges (R13) instead of "every
    function". Candidates are screened in proximity order, capped at
    `max_screen`. Adaptive splitting starts from a single group. Alternative
    subsets are pre-ranked with the linear model, and only the top
    `max_alternatives + 2` are verified. Without that, a two-goal drag would
    verify 66 pairs. The cost model now counts alternatives (§2.2, §2.3.5,
    §2.5.3). Partial statics invalidation is planned for big programs
    (§2.3.5a).
14. **Hint name safety.** `BuiltinCall` resolves by name, so `hints.rs`
    checks core's `NativeFnId`, and an embedder native named `rigid` is not
    taken as a hint. The `shadows-builtin` lint consequence is noted, the
    grep was re-run, and the bare-statement form is kept (§2.6).
15. **Smaller fixes.** These cover the sign of rendered negative values, the
    pixel-centre target for truncating natives (§2.4.3), `TimedOut` versus
    the step budget (§2.1.7), the Recompile oracle comparing by emit index,
    not term-hashed keys (§2.1.8), and a pinned, echoed seed on the CLI.
16. **De-scoped.** The dual fast path (was M2b) is now future work, and
    `Observable::State` is cut. Clock-coupling detection is late M3 and can
    be dropped. Alt-cycling in Garden is a stretch, while the allow-list
    picker stays required. External-project study goals are best-effort.
