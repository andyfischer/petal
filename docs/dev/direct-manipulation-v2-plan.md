# Direct Manipulation v2: plan

Status: design, 23 October 2026. Milestones M1 to M5 are not started. See
[Status](#9-status) at the end, which each milestone updates when it lands.

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
  `vm/fast.rs:138`.
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
| initial seed | `initial_seed()` (`builtins/mod.rs:70`) reads the wall clock unless `PETAL_SEED` is set | not relevant once forked | none |
| `HOST_DATA` / `RESOURCES` reads (`is_valid`, petal-query, fonts) | various | the same host state is visible to every run in one session | call through, caught by the double baseline (§2.1.5) |
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
   - CLI and MCP: a fresh stack (`create_stack`) that has never run.
   - Garden and other live hosts: a pre-run fork that the host takes after
     binding the frame's inputs and before `run` while DM is armed (R6:
     `Env::set_dm_checkpoint(stack, true)` keeps the last pre-run fork). This
     reuses the `petal-sdl` fork-per-frame pattern.
   - As an optimization when no checkpoint was kept: if the last run's
     `run_deps` says it left every `state` slot as it found it, the
     post-frame stack is equal to the pre-frame one for state and can be used
     directly.
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

Runs are sequential (the `Env` is `&mut`). Parallel runs would need `Env`
clones and are out of scope.

#### 2.1.2 State

`StateMode::Frozen` is the default: each run starts from the checkpoint's
state, so state behaves like an input. `StateMode::Fresh` runs from empty
state, so `state` initializers execute and their literals have an effect.
This is opt-in, because an edit to a state initializer does not show in the
running program after a hot reload: reload keeps state
(`env/reload.rs:30-36`). Parameters that only feed a `StateInit` are marked
`state_init: true` and excluded by default (§2.5).

A run's own `set` writes land in the fork and are discarded with it. Writes
to state are captured as observables only when a client asks (`Observable::
State`, a stretch goal). They are what a physics-ish program's next frame
reads.

#### 2.1.3 The native gate (effects suppressed)

In a speculative run, `call_native_fn` consults `SpecHooks` (R1) before
calling. The policy, by the native's row:

| Row | Policy | Why |
|---|---|---|
| `PURE` / `CONST` / `probe` reads of bindings | call | a function of args and frozen bindings |
| `emits` | call, and **capture the raw args** first | output stays in the fork's buffers; raw args avoid the int truncation of §1.7 |
| `reads(RNG)` | replay by call site (§2.1.4) | removes draw-order shift |
| other `reads` (`HOST_DATA`, `RESOURCES`) | call | host data is fixed for the session; the double baseline catches violations |
| `effect` and **contained** (new row facet, R2) | call | lands in the fork's ctx and is discarded |
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
fork copied `rng_state`). The result is recorded under the key
`(hash(call chain incl. origin term), occurrence index under that chain)`.
In perturbed runs the same key returns the recorded value. A key the
baseline never saw (the perturbation added loop iterations) gets a value
derived from `hash(key, session_seed)` through the native's own mapping, and
the run is flagged `new_random_keys > 0`. The effect is that random values
belong to *where* they are drawn, not *when*, so a perturbation that adds one
draw does not reshuffle every later shape.

`RngMode::ForkStream` (plain fork behaviour) is kept for comparison tests.

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
  arguments** converted to `ObsValue`. `EmitKey = { chain_hash, occurrence }`,
  where `occurrence` counts earlier emits with the same chain in this run. A
  key is stable across runs whose control flow did not change, which is what
  lets one shape out of a loop be matched between runs.
- **Watched terms**: values of chosen `TermId`s, through a term filter on the
  fork's `TraceBuffer` (R5), each with its trace sequence number. Emits also
  record the trace sequence at emit time. That maps a v1-style goal ("arg 2
  of user frame T for emit N") to `last_for_term_before(arg_term, emit_seq)`.
- **Fingerprint**: a hash over the sequence of (instruction index, taken)
  pairs for every conditional branch, plus loop trip counts (R7). It is
  computed only in spec mode. Equal fingerprints mean the same path was taken.
- **Loaded parameters**: the set of slotted constants a `LoadConst` read
  during the run (one bitset write, spec mode only). A parameter that was
  never loaded cannot affect anything.
- `steps`, `elapsed`, `error: Option<String>` (a runtime error or a
  budget overrun counts as a failed run, not a failed session),
  `suppressed: Vec<SuppressedCall>`, and `new_random_keys`.

#### 2.1.7 Budget

Each run uses `run_bounded` with `SpecConfig::max_steps` (default: ten times
the baseline's step count) and a wall-clock cap. A perturbation that makes a
`while` loop run away fails that run only.

#### 2.1.8 Correctness oracle

`OverrideStrategy::Recompile` (test-only): instead of slot overrides, splice
the literal into the source text, compile, and run a fresh fork. M1 tests
check that both strategies give bit-identical observables over the
examples. This guards the slotting/override path against constant folding or
caches that bake a value in.

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
     a list), or a `MethodCall`: include the bodies of every function whose
     name matches (`fn Class.name` dispatch, as `EdgeKind::DispatchMay` does),
     else every function in the program. Over-approximating is fine here,
     because stage 2 prunes.
   - At a cell frontier (`var`) or a `StateRead` of a state written in the
     program, follow the writes' slices (the forward walk's `CellMay` edges).
     Mark the parameters reached this way `through_var` / `through_state`.
   - Stop at `rigid(...)` (§2.6), at the arguments of `reads(RNG)` natives
     (their output is replayed, so their arguments have no effect), and at
     natives declared no-feedback.
2. **Loaded filter.** Drop every static candidate the baseline run never
   loaded (§2.1.6).
3. **Empirical screening** (`screen_params`) by adaptive group testing. Take
   the remaining `n` candidates in groups of up to 16. Perturb every member
   of a group in one run, each by a distinct, incommensurate relative step
   (`h_i * (1 + 0.137 * j)`, sign alternating). If no goal row moves (beyond
   `1e-12` relative), the whole group is irrelevant. Otherwise split it in
   half and recurse. With `k` relevant out of `n` this costs about
   `k * log2(16) + n/16` runs. A group whose run errors or flips the
   fingerprint is split down to singletons.

Literals are candidates wherever they are written: call-site arguments,
`let` / `config let` definitions, function bodies, default parameter
expressions, record and list literals, loop bounds. Default **eligibility**
is decided in §2.5. Discovery reports ineligible literals too, so a client
can show them and opt in.

Parameter identity is the `TermId`, which is valid for one program. Clients
across a reload use `ParamSelector::Span` or `Variable` (§2.7).

### 2.3 (c) Sensitivity: the Jacobian (M2)

Rows are the **goal rows** plus the **collateral rows**: every numeric raw
native argument of every emit in the baseline (capped at 20k rows by
default, in which case only emits on the goals' channels are kept). Columns
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
| `Discrete` | int literal whose float-valued probe errored | probe with `±1` instead and keep the integer secant |

#### 2.3.4 Int literals

The first probe replaces `n` with the float `n ± h`. Where Petal needs an
int (indexing, `range`, int `/`) that either errors or changes the
fingerprint, and the column falls back to integer steps (`n ± 1`, then `±2`,
`±4`). Final values are rounded (§2.4.4).

#### 2.3.5 Cost

Two baseline runs, plus screening, plus `2k` for central differences, plus
`1` to `5` verification iterations. A typical sketch (k ≤ 8) needs 20 to 30
runs.

#### 2.3.6 Optional dual fast path (M2b, low priority)

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

#### 2.4.4 Rounding to nice literals

After a verified solve, for each changed parameter, try in order:
- an int, if the original was an int;
- the original literal's number of decimals (`float_like` style);
- the fewest decimals (0 to 6) that keep every goal within `τ`.

Try the jointly rounded vector first, in one run. If it fails, round
parameters one at a time, largest change first. A parameter that has to
change from int to float spelling is reported (`respelled: true`) and
penalized in ranking. Rendering uses `literal_edit::float_like` (to be made
`pub(crate)`) and v1's `render_matching`, keeping a `-` inside the span for
`negated` literals.

#### 2.4.5 Verified

An alternative is `verified` only when a final spec run with the rendered
literal values (parsed back from the new text) hits every goal within `τ`.

### 2.5 (e) Heuristics and ranking (M3)

#### 2.5.1 Eligibility (hard filters, before solving)

Excluded by default, and included when the client allows them:
- literals outside the user's files: files that are not the entry file or
  one of its local imports (prelude, `core-libs`, packages, vendored code);
- `rigid(...)` literals;
- `state_init` parameters (§2.1.2);
- literals that only reach the goal through a `reads(RNG)` native argument
  (already cut statically) or a `rigid` barrier;
- int literals in a `Discrete` column with `|Δ| < 0.5` (nothing to round to).

#### 2.5.2 Parameter cost weights `d_i` (lower means preferred)

| Kind | `d_i` |
|---|---|
| `tunable(...)` | 0.25 |
| `config let` | 0.5 |
| literal written in the goal's own user-frame call | 0.75 |
| `let` binding at top level of the entry file | 1.0 |
| literal in a user function body or a default parameter, entry file | 1.5 |
| literal in another user file | 2.0 |
| `through_var` | ×1.5 |
| `through_state` (feedback through state from earlier frames: the frozen-state slope covers one frame only) | ×4, and `stateful: true` on the alternative |
| clock- or input-coupled (§2.5.4) | ×4 |

#### 2.5.3 Alternatives

The solver runs on several subsets and the results are ranked:
1. every single parameter (for one goal) or pair of parameters (for two
   goals) among the top 12 columns by `|J_g| / d_i`, where the subset can
   reach the goals (rank of `J_g[:, S]` = number of goals);
2. a "config-only" subset, if any config or tunable parameter has a nonzero
   column;
3. the client's allowed set, when it gave one;
4. the full screened set (minimum-norm spread across everything).

Each is solved and verified. Duplicates (the same edits) collapse.

#### 2.5.4 Score (lower is better)

```
score = Σ_i d_i |Δp_i|/s_i                      (how much code meaning moves)
      + κ_c · collateral_norm                   (how much else moved, measured)
      + κ_n · (|S| - #goals)                     (prefer fewer edits)
      + κ_r · respelled_count
      + κ_e · max_j (residual_j / τ_j)           (0 when verified)
      + ∞ if not verified and a verified alternative exists
```

The defaults are `κ_c = 1`, `κ_n = 0.5`, `κ_r = 0.3` and `κ_e = 2`.
`collateral_norm` is measured from the verification run: the RMS movement of
all non-goal rows, normalized by the goal movement. It is reported with the
list of moved emits (`CollateralItem { key, channel, callee, line,
max_move }`). This replaces v1's static `shared` flag with what actually
moved, so a literal that feeds one shape ranks above one that feeds fifty.

**Config preference.** v1 hard-pinned all non-config bindings once any
`config let` existed. v2 makes this a soft preference: config and tunable
weights are lower, and alternatives built only from them are listed first
when they verify. The others are still returned, marked `config: false`.
`propose_edits_v2` with `Prefer::ConfigStrict` reproduces the v1 behaviour.

**Clock and input coupling.** When the baseline read `CLOCK` or `POINTER`
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
- **Static meaning**: the IR keeps them as `BuiltinCall("tunable" | "rigid")`.
  `hints.rs` resolves argument 0 the way `provenance::resolve_arg` does (a
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
  then a plain `Call`, which `hints.rs` ignores. That is non-breaking.
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
    AllUserCode,                // lift the user-files-only default
    IncludeLibrary,             // allow prelude/package literals
    IncludeStateInit,
}
pub struct ParamFilter { pub allow: Option<Vec<ParamSelector>>, pub deny: Vec<ParamSelector> }
```

`allow: None` means the defaults of §2.5.1. `allow: Some(..)` is exact: the
engine uses only those. Asking for alternatives is
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
| R2 | Row facets: `NativeEffects::contained: bool` (effect stays in the `ExecutionContext`), `no_feedback: bool`. Constructors `.contained()` and `.no_feedback()`. Mark core's `print`, `noise_seed`, `expect*`, `freeze`/`frozen`, `__pending`/`__resolve`/`__reject` and the counters as contained. Embedder natives with `effect` default to **escaping**. `Env::mark_handle_class_spec_safe(id)` as a side registry, not a new `HandleClass` field, so vendored embedders don't break. C bridge: `PB_FX_CONTAINED`, `PB_FX_NO_FEEDBACK` | `native_fn.rs`, `builtins/mod.rs`, `env/mod.rs`, `integrations/petal-c-bridge` | Adding fields keeps `..NativeEffects::PURE` struct literals compiling. Extend `effect_audit` to report an escaping effect seen in spec mode |
| R3 | `Env::slot_literals(pid, &[TermId]) -> Result<(), String>`: factor the `alloc_slot` + `retarget_constants` part out of the reload patch (`reload.rs:490-510`) as a value-neutral operation | `env/reload.rs` | Must keep `core/tests/hot_reload.rs` green. A later reload of an already-slotted term takes the existing `set_slot` path |
| R4 | `Stack::drop_derived()`: clear statics, memo, enums, functions and methods, and `run_deps.force()`. Factored from `restore_execution` (`fork.rs:109-119`) and `transfer_stack_state` | `stack.rs`, `env/fork.rs` | The fork in each spec run calls it. Memo is also disabled in spec runs (`memo = false` in `make_vm`) |
| R5 | `TraceBuffer::set_filter(Option<HashSet<TermId>>)`: when set, `push` records only those terms | `trace.rs` | Used for `Observable::Watch`. Also expose `next_sequence()` |
| R6 | `Env::set_dm_checkpoint(stack, on)` and `Env::dm_checkpoint(stack) -> Option<StackKey>`: keep one pre-run fork, refreshed at each `run` while on | `env/run.rs`, `env/fork.rs` | Garden turns it on in IDE mode. The cost is one heap fork per frame, only while armed |
| R7 | Branch fingerprint: in spec mode, fold `(ip, taken)` of every conditional jump and each loop's trip count into a `u64` (FxHash) | `vm/dispatch.rs` (spec loop only) | Never on the normal path |
| R8 | Emit records: when spec capture is on, push `EmitRecord` (channel, chain, key, trace seq, raw args) beside `emit_origins`. `PetalCxt::push_output`/`emit` already know the chain. Raw args are captured in `call_native_fn` before `natives.call` and attached to the emits that call produced | `vm/native.rs`, `native_fn.rs` (`push_output`, `emit` at `:1209`) | Bounded: arg conversion depth 3, lists ≤ 64 |
| R9 | `literal_edit::float_like` becomes `pub(crate)` | `literal_edit.rs:1888` | For §2.4.4 |
| R10 | `tunable`/`rigid` builtins (identity, `CONST`), identity look-through in `provenance::resolve_arg` so v1 keeps working, docs in `Builtins.md`, `stdlib.json` regen, lints `dm-hint-not-literal` / `dm-hint-state-init` / non-literal bounds | `builtins/`, `provenance.rs`, `lint/` | M2. See the proposal |

No change to the language grammar, IR schema or bytecode format.

---

## 4. Surfaces

### 4.1 Rust API (M3)

`dm_v2::propose_edits_v2(env, checkpoint, &DmRequest) -> Result<DmResponse,
DmError>` (§6.3). It is re-exported from `lib.rs` as `petal::dm_v2`. The
embedding guide gets a section in M4.

### 4.2 CLI (M4)

`petal propose-edit` gains `--v2`, which becomes the default when M5's study
passes the bar in §5 (M5); `--v1` keeps the old engine.

```
petal propose-edit --v2 --channel draw_commands --emit 7 --native-arg 0 --by 25 sketch.ptl
petal propose-edit --v2 --channel draw_commands --emit 7 --native-arg 0 --to 148 \
                   --native-arg 1 --to 90 --allow radius --allow line:12 --alternatives 5 --json sketch.ptl
petal propose-edit --v2 ... --explain           # candidate table, Jacobian columns, iterations
petal propose-edit --v2 ... --apply [--pick N]  # writes the top (or Nth) verified alternative
```

- `--arg i` (v1 addressing: argument `i` of the user-file frame) stays and
  maps to `Observable::Watch` (§2.1.6). `--native-arg i` addresses the
  emitting native's raw argument. `--by d` is relative to the baseline value.
- `--allow`/`--deny` take `name`, `line:N`, `config`, `fn:NAME`, `user`,
  `lib` and `state-init`. The old `--configurable`/`--static` map to
  `allow`-with-preference and `deny`.
- `--tol`, `--max-runs` and `--seed`.
- `--apply` with `--v2` refuses unless the top alternative is verified and
  either it is the only one or `--pick` was given.
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

- Turn on `set_dm_checkpoint` in IDE mode (R6).
- `drag_handle` maps to native arg indices (for `draw_circle`, `cx`=0 and
  `cy`=1 at `_native_circle`). The goal is the raw baseline value plus the
  drag delta.
- `EmitKey` comes from the session baseline's draw-channel emits, matched to
  `cmd_index` by position, with an assertion that the chains equal the
  frame's `frame_origins`.
- During a drag: open the session on mouse-down. Preview on each mouse move
  with one linear solve against the cached Jacobian (no runs) and show the
  predicted literals in a ghost overlay. On mouse-up, run the full verify and
  round, then apply the top alternative and hot-reload (a value-only
  reload, so state is kept). Hold Alt to cycle alternatives, and a picker
  lists the candidates for the allow-list mode the task doc asks for.
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

Scope: R1 (overrides, gate, capture, loaded set), R2 (facets and core rows,
handle registry), R3, R4, R5, R7, R8, and `dm_v2/{mod,spec,observe}.rs`.

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

### M2: discovery, Jacobian, hints

Scope: `dm_v2/{params,hints,jacobian}.rs`, R9, R10, the no-feedback facet
wired into discovery and the gate, the dual fast path optional (M2b).

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
   ones is screened in ≤ 40 runs, and exactly the 3 survive.
8. `branch_flip_and_step_quality`: `abs(x)` at 0 gives `Kink`, `round(x) *
   10` gives `Step` with a secant of about 10, `if x > 5` at the threshold
   gives `BranchFlip`, and `xs[i]` with an int `i` gives `Discrete`.
9. `int_literal_float_probe`: `range(n)` with an int `n` gives a `Discrete`
   column. `draw(10 / 4)`, where float probing changes semantics, is marked
   discrete.
10. Names: `tunable`/`rigid` added to `stdlib.json` and `Builtins.md`. The
    collision grep from the proposal is re-run in the commit message.
11. (M2b) `dual_matches_fd`: on tests 1 to 3, dual columns equal the
    central-difference columns within 1e-6.

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
    reports `budget_exhausted` when it stops early.

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
`petal propose-edit --v2 --json` per goal and reports, per example and in
total:
- the success rate (top alternative verified, and within `dm-expect`);
- verified-any rate;
- runs used;
- wall time (median of 3);
- final max error / τ;
- the candidate count before and after screening;
- the number of alternatives.

It also reports, per example, static program metrics: terms, functions,
max call depth reached, loops, `var` count, `state` count, emits, and
whether the baseline read the clock or RNG. These let the report plot
success against complexity. `--extra` points at copies of large external
projects. Copy them to `/Volumes/bureau/petal-dmv2-study/`, never into the
repo: worlds-fair `ui/ptl` screens, petal-creative projects, cheesecake
games. For each one the harness auto-generates goals ("move emit k's arg 0
by +10" for 20 sampled emits) with no `dm-expect`, scoring only
verified-ness and collateral.

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
    pub fn begin(env: &mut Env, checkpoint: StackKey, params: &[TermId],
                 cfg: SpecConfig, live: Option<&LiveObservation>) -> Result<SpecSession, SpecError>;
    pub fn add_params(&mut self, env: &mut Env, params: &[TermId]) -> Result<(), SpecError>;
    pub fn run(&mut self, env: &mut Env, overrides: &Overrides) -> SpecRun;  // a failed run is data, not Err
    pub fn baseline(&self) -> &SpecRun;
    pub fn runs_used(&self) -> usize;
    pub fn program(&self) -> ProgramId;
    pub fn end(self, env: &mut Env);       // drops forks; slots stay (value-neutral)
}

pub struct SpecRun {
    pub error: Option<String>,
    pub emits: Vec<EmitRecord>,
    pub watched: Vec<WatchRecord>,         // (term, trace seq, ObsValue)
    pub fingerprint: u64,
    pub loaded: FixedBitSet,               // indexed by the session's param index
    pub suppressed: Vec<SuppressedCall>,   // native name, origin term
    pub new_random_keys: u32,
    pub steps: u64,
    pub elapsed: std::time::Duration,
}
pub struct EmitRecord {
    pub channel: SymbolId,
    pub key: EmitKey,
    pub chain: EmitSite,
    pub trace_seq: u64,
    pub native: Option<NativeFnId>,
    pub native_args: SmallVec<[ObsValue; 8]>,
    pub value: ObsValue,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EmitKey { pub chain_hash: u64, pub occurrence: u32 }
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
    pub in_goal_frame: bool, pub user_code: bool,
}
pub enum Hint { Tunable { min: Option<f64>, max: Option<f64>, step: Option<f64> }, Rigid }

pub fn read_hints(program: &Program) -> HashMap<TermId, Hint>;
pub fn discover_params(program: &Program, anchors: &[TermId], hints: &HashMap<TermId, Hint>)
    -> Vec<Param>;                                   // static superset, all eligibilities
pub fn anchors_for(program: &Program, run: &SpecRun, goals: &[Observable]) -> Vec<TermId>;
pub fn screen_params(session: &mut SpecSession, env: &mut Env, params: &[Param],
                     goals: &[Observable], budget: &mut RunBudget) -> Vec<usize>;  // indices kept

pub struct FdOptions { pub mode: FdMode /* Central | Forward */, pub rel_step: f64 /* 1e-3 */,
                       pub max_escalations: u8, pub use_dual: bool }
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
    pub edits: Vec<crate::direct_manipulation::SourceEdit>,
    pub changes: Vec<ParamChange>,               // param, old, new, new_text, respelled
    pub achieved: Vec<f64>,                      // per goal
    pub max_error: f64,                          // max_j |achieved - target| / τ_j
    pub verified: bool,
    pub collateral: Collateral,                  // norm + Vec<CollateralItem>
    pub stateful: bool, pub config_only: bool,
    pub score: f64,
    pub description: String,                     // "set `gap` to 14 (line 3); moves 4 other shapes"
}
pub struct CandidateReport { pub param: Param, pub eligible: Result<(), &'static str>,
                             pub column: Option<Vec<f64>> /* goal rows */, pub quality: Option<ColumnQuality>,
                             pub weight: f64 }

pub fn propose_edits_v2(env: &mut Env, checkpoint: StackKey, req: &DmRequest)
    -> Result<DmResponse, DmError>;

/// The reusable form for a drag: open once, preview with no runs, verify at the end.
pub struct DmSession { /* SpecSession + params + Jacobian */ }
impl DmSession {
    pub fn open(env: &mut Env, checkpoint: StackKey, req: &DmRequest) -> Result<DmSession, DmError>;
    pub fn preview(&self, targets: &[f64]) -> Vec<ParamChange>;   // linear, zero runs
    pub fn finish(self, env: &mut Env, targets: &[f64]) -> Result<DmResponse, DmError>;
}

pub enum DmError { Spec(SpecError), NoGoals, BadObservable(String), NothingToMove { why: String } }
```

`apply_edits` from v1 applies `Alternative::edits`.

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

---

## 8. Risks and open questions

1. **Embedder rows.** Escaping-by-default can make spec runs of cheesecake or
   worlds-fair scripts suppress natives the script depends on, which then
   shows as `BaselineMismatch`. Mitigation: the error names the natives, and
   the ecosystem sweep in M4 marks the contained ones (the vendored
   `petal-c-bridge` needs `PB_FX_CONTAINED`, then re-vendoring).
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
6. **Hint names** need the language owner's sign-off
   ([proposal](../proposals/dm-v2-hints.md), open questions).

---

## 9. Status

| Milestone | State | Commits | Notes |
|---|---|---|---|
| Design | done, 2026-10-23 | (this commit) | plan + [hints proposal](../proposals/dm-v2-hints.md) |
| M1 speculative runner | not started | | |
| M2 discovery + Jacobian + hints | not started | | |
| M3 solver + ranking + API | not started | | |
| M4 surfaces | not started | | |
| M5 examples + study | not started | | |

Deviations from this plan are recorded here as they happen, with the reason.
