# Declare what a native does, instead of inferring it at runtime

Status: **in progress**, 2026-09-16. Sequencing steps 1 (the oracle over Garden
and worlds-fair) and 2 (a named `RunPolicy`) are done — see
[What the oracle found](#what-the-oracle-found) and
[RunPolicy](#runpolicy-sequencing-step-2). Steps 1–4 of
[Migration](#migration-in-five-steps-that-each-stand-alone) are done — see
[Steps 1–3 as landed](#steps-13-as-landed) and
[Step 4 as landed](#step-4-as-landed). Step 5 (declare the ecosystem, drop
the fallback) is next; the audit has already listed the 64 natives it covers.

Prerequisite for P2 of [the reactive rendering plan](../dev/reactive-rendering-plan.md).
See [Sequencing](#sequencing) for how it interleaves with the rest of that plan.

## The observation

Three layers now depend on knowing what a native function *does* — the frame
gate ([frame-gate.md](../dev/frame-gate.md)), memoized scopes
([memo-scopes.md](../dev/memo-scopes.md)), and P2's dependency classes, which
are not built. None of them can ask. There is no place where a native says so.

What exists instead is three partial mechanisms, none of them authoritative:

1. **`NativeClass`** (`rust/src/native_fn.rs`) sounds like an effect
   classification and is not one. It is the *Pending-argument policy* —
   `Strict` absorbs, `Effectful` no-ops, `AllowPending` inspects — consulted
   only by `intercept_pending` when a `Pending` argument is actually present.
   Twelve natives set it. `Effectful` there means "swallows a Pending", not
   "has an effect", and the two sets only happen to overlap.

2. **`PetalCxt` self-instrumentation.** `binding()` notes a binding read,
   `push_output()` notes an emit, `print()` / `rng_next_f64()` /
   `set_noise_seed()` and four other methods note an effect. This covers the
   core builtins by construction: a core native that does something
   interesting almost always does it through one of those methods.

3. **Manual `note_host_read()` / `note_effect()`.** The only mechanism
   available to a native that reaches host-owned state by some other route.
   Across the whole ecosystem there are **four** call sites — three in Garden
   (`query.rs`, `panel.rs`) and one in `petal-ui/src/host_data.rs` — against
   roughly **365 registered natives**: 111 core, 94 in petal-ui, 114 in
   worlds-fair, 33 in Garden, 13 in the integrations.

   (Counted since: the real total is **253**, and worlds-fair registers 8 rather
   than 114 — see [A count in this document was wrong](#a-count-in-this-document-was-wrong).
   There are now nine `note_*` call sites, the five added by sequencing step 1
   included.)

The plan's hazard table states the contract plainly — *"Any new native that
reaches host state must do one or the other"* — and nothing enforces it. A
native that forgets is not an error. It looks pure, and every scope that
called it memoizes stale.

### The consumer is inference, and inference is the wrong shape

`memo_note_native` (`rust/src/backend/bytecode/vm/memo.rs`) classifies a
native by diffing activity counters around the call:

```rust
let before = self.stack.run_deps.activity();   // native.rs — every native call
let count = func(&mut cxt)?;
if self.memo && self.stack.memo.recording() {
    self.memo_note_native(nid, args, result, before);   // after - before
}
```

It works, and it is how P1 shipped. But four things follow from classifying
*after the fact* rather than *up front*:

- **P2 cannot use it.** Dependency classes are a compile-time
  interprocedural pass over the term graph, seeded from the input natives.
  Seeding it requires knowing, before the program runs, that `mouse_x` reads
  the pointer class and `time` reads the clock. Runtime counters cannot say
  that, and cannot say *which* class moved — only that `binding_reads` went
  up by some amount.
- **Recording cost is paid before the decision.** A scope must be opened and
  recorded before the runtime can learn it was not worth recording. That is
  precisely the shape of the `edit` regression the cold-site guard fixed
  (`bd4976d`): the spreadsheet's formula recompute recorded ~30k scopes per
  commit that the next run evicted unreplayed, and memoized editing cost more
  than no memoization at all until the runtime *learned* those sites were
  cold. A declaration would have known.
- **`activity()` is snapshotted on every native call**, before the `self.memo`
  check, so the cost is paid whether or not anything consumes it.
- **A missing declaration is silent.** There is no state in which the runtime
  notices that a host native answered from somewhere it cannot see.

## The change

One declared effect row per native, required at registration, carrying
everything the three layers need:

```rust
pub struct NativeEffects {
    /// What this native reads, by input class. The seed set for P2's
    /// interprocedural class propagation. Empty = reads nothing external.
    pub reads: InputClasses,      // bitflags: POINTER KEYBOARD CLOCK
                                  // VIEWPORT HOST_DATA RESOURCES RNG
    /// The result is a pure function of the arguments and `reads`, and the
    /// call may be re-evaluated at validation time without observable
    /// consequence. `hovered`, `mouse_x`, `time`. A probe is what gives a
    /// memoized scope its early cutoff.
    pub probe: bool,
    /// Pushes into an output buffer (a draw command, an event).
    pub emits: bool,
    /// Does something no replay can reproduce: prints, advances the RNG,
    /// creates or resolves a resource, reaches through a handle into host
    /// state, publishes a method.
    pub effect: bool,
    /// What to do with a `Pending` argument. Subsumes today's `NativeClass`
    /// unchanged — same three cases, same call site.
    pub pending: PendingPolicy,
}
```

`InputClasses` is deliberately the *same* bitmask P2 will propagate, declared
once at the leaf where the knowledge actually lives, rather than reconstructed
by two layers that each guess at it.

Note what is not in the struct: nothing about *how much* a native costs.
Scope-worthiness stays a separate decision (today's `MIN_SCOPE_INSTS`
heuristic); this row is about correctness and class, not economics.

### Migration, in five steps that each stand alone

The 253 registration sites span five repositories, one of which
(`~/worlds-fair`) is not in this tree — Garden is, at `garden/`. So the row
cannot be made mandatory in one commit, and it does not need to be.

1. **Add the row without requiring it.** `NativeEffects::UNDECLARED` is the
   default; `register_with(name, func, effects)` sits beside the existing
   `register`. Nothing changes behavior: an `UNDECLARED` native is classified
   by the existing runtime inference, exactly as today.
2. **Declare the 111 core natives.** Mechanical — the instrumented `PetalCxt`
   methods each native already calls say what its row should be. A corpus
   test asserts every *core* native is declared, so the core can never
   regress to `UNDECLARED`.
3. **Consume declarations where they exist.** The memo takes a declared
   native's classification directly and skips the `activity()` snapshot for
   it; `UNDECLARED` natives keep the inference path. Both paths must be
   exercised in the differential oracle, because for the whole of this step
   they coexist.
4. **Add `petal --effect-audit`** (or a flag on `petal-ui-run`): run the
   corpus with inference on for *every* native, declared or not, and report
   each native whose observed behavior exceeds its declaration — including
   `UNDECLARED` natives that were seen doing something. This is the tool a
   host uses to migrate, and it is what turns "did we remember?" into a
   question with an answer. Point it at Garden and worlds-fair first: those
   8 + 33 natives are where the gap actually lives — sequencing step 1 found
   five undeclared among them statically, and a runtime audit should find at
   least those.
5. **Declare the ecosystem, then drop the fallback.** petal-ui, then Garden,
   then the integrations and worlds-fair. Once every registration site is
   declared, `UNDECLARED` becomes a hard error at registration and the
   per-call `activity()` snapshot goes away from the hot path.

### Keep the inference — as a test, not as the mechanism

The counter-diffing in `memo_note_native` should not be deleted at step 5. It
should be **demoted to the oracle**: a debug-build mode that runs inference
alongside the declarations and fails when a native does more than it claims.
That inverts the current relationship — today inference is load-bearing and
nothing checks the declarations, because there are none; afterward the
declarations are load-bearing and inference is what checks them.

This is the same discipline every reactive layer has shipped under
(`petal-ui/tests/gating.rs`, `petal-ui/tests/memo.rs`): a cheap exact
mechanism, verified continuously against an expensive exact one.

## How to check it

Steps 1–3 must produce byte-identical behavior. The existing oracles are the
test, run with declarations on and off — and under each run policy, which is
what [`RunPolicy`](#runpolicy-sequencing-step-2) is for:

```bash
cargo test -p petal-ui --test gating
cargo test -p petal-ui --test memo
cargo test -p petal --lib memo::
```

The corpus those two run over now includes Garden's example panels and GPP
apps (`petal-ui/tests/common/mod.rs`), so the reactive plan's outstanding
hazard — "no layer has yet been checked against the real codebases" — is
closed for the in-tree half. worlds-fair is out of tree and needs a generated
bundle and fixture models, so it runs on demand:

```bash
cd petal-ui && cargo build --release
cd ~/worlds-fair/ui && cargo build --release -p wf-ui-garden
./ts/bin/oracle-external.ts
./ts/bin/native-effect-audit.ts
```

Performance is a secondary check — this is a correctness and structure change
first — but step 5 should show up as a small win on native-heavy frames, where
the `activity()` snapshot per call disappears:

```bash
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/dashboards/analytics-dashboard/app.ptl 600 --wiggle
```

The real payoff is not measured here; it is that P2 becomes buildable.

## Steps 1–3 as landed

Done 2026-09-16, against the whole in-tree corpus.

- **The row** is `NativeEffects { reads: InputClasses, probe, emits, effect,
  pending }` in `rust/src/native_fn.rs`, as sketched above with two
  additions. `pending` is the existing `NativeClass`, unchanged — the
  Pending-argument policy is now one field of the row rather than a separate
  classification that looked like one. `InputClasses` gained `BINDINGS`
  beside the seven planned classes: the core `binding(sym)` reads whichever
  binding its argument names, so the class is not knowable at the leaf. P2
  will have to resolve it per call site from the symbol.
- **Registration.** `NativeFnTable::register_with` /
  `Env::register_native_with` take the row; `register` / `register_native`
  still exist and leave the native undeclared (`effects(id)` is `None`),
  which is what every host native is today. `set_class` on a declared native
  updates the row's `pending`, so the two cannot disagree.
- **The core is declared** — all 112 natives, in `register_builtins`, and
  `every_core_native_is_declared` fails the build of any future `register`
  there. The rows are the union over every path: the seven pending
  inspectors (`is_loading`, `or_else`, …) read the resource table only when
  handed a `Pending` and declare `RESOURCES` regardless. `noise` is pure (the
  seed is program state, set through `noise_seed`'s effect); the RNG builtins
  declare `RNG` and no effect, since the scope compares RNG state at entry
  and exit itself. The higher-order placeholders (`map`, `sort_by`, …) are
  pure rows the VM never consults — it intercepts them and records what the
  closure does.
- **The memo consumes the row.** `call_native_fn` snapshots the activity
  counters only for an undeclared native; a declared one goes through
  `memo_note_declared_native`, which maps the row onto exactly the deps the
  inferred path would have recorded (`effect` → unrecordable, `HOST_DATA` →
  `Dep::HostRead`, `RESOURCES` → `Dep::ResourcesRead`, any other read → a
  probe, or an effect if the row is not a probe or the call also emitted).
  A `Pending` *result* still makes the scope effectful on both paths — that
  is a fact about the answer, not the native.
- **Both paths are in the oracle.** `RunPolicy` has a fourth switch,
  `declared` (on in every named policy; `-declared` turns it off), which
  makes the memo classify every native by inference. The corpus test in
  `petal-ui/tests/memo.rs` now drives `replay-memo`, `replay` and
  `replay-declared` and requires the same frames *and the same replay
  counts* from the two memoized runs — so a row that says less than the
  native does shows as a stale frame, and one that says more as a lost
  replay. Across the 36 in-tree apps both hold. `oracle-external.ts` runs
  the same variant against worlds-fair when that half of step 1 lands.

What did not change: `NativeClass` is still the type hosts set through
`set_native_class`, and the per-call `activity()` snapshot is still paid for
every undeclared native — which is every host native until step 5.

## Step 4 as landed

Done 2026-09-16. The runtime audit is `rust/src/effect_audit.rs`, a switch on
the `Env` like the profiler (`Env::set_effect_audit`, report from
`Env::effect_audit_report`). With it on, `call_native_fn` snapshots the
activity counters around *every* native, declared or not, and also logs which
bindings the call read; the per-native union of those deltas is held against
the native's row. Off, it is one branch per native call.

Three ways to run it:

- `petal-ui-run <app> --effect-audit` and `petal run <file> --effect-audit`
  print the report to stderr. The runner exits **3** when a declared native
  did more than it declared, so a script can tell a staleness bug from a
  runtime error.
- `cargo test -p petal-ui --test effect_audit -- --nocapture` runs it over
  the whole in-tree corpus under `replay` and fails on any under-declaration;
  it prints the undeclared natives with what each was seen doing, merged
  across apps.
- `./ts/bin/oracle-external.ts` runs it once per worlds-fair fragment after
  the differentials, fails a fragment on an under-declaration, and summarizes
  the undeclared natives at the end. Still blocked on the same upstream
  `--print-fixtures` as the rest of that script.

The report has three kinds of line. **under-declared**: a declared native was
seen doing a facet its row lacks — an `effect`, a `host_read`, a
`resource_read`, an `emit`, or `binding a,b` when the row has no probe class.
The mapping is the memo's own: each facet is the counter whose movement
`memo_note_native` would have turned into a dep, and the field of the row
`memo_note_declared_native` would have consulted instead. **undeclared**: no
row, and the same facets, or `(silent)` if the native looked pure — which is
where a native that reaches host state by a route the counters cannot see
would hide, so silence is a prompt to read the native, not a pass.
**over-declared**: a declared facet the run never exercised; informational,
since a row is the union over every path.

What it found, across the 37 in-tree apps at 45 monkey frames:

- **No core native under-declares.** The strongest check on the 112 rows so
  far after the replay-count oracle, and the one that names the facet.
- **64 undeclared natives are reached by the corpus** — the whole of step 5's
  in-tree work, listed with the row each wants: the draw family and the
  region natives `emit`; the input readers each name their binding
  (`hovered` reads `mouse_x,mouse_y`, the `mod_*` family `modifiers`, the
  text measurers `text_advance,text_advances,text_vertical,text_fonts`);
  `create_canvas` and `draw_to` are `effect emit`; the stub `query` is
  `effect host_read`; and eight are silent (`claim_key`, `fonts`, `palette`,
  the `edit_view_*` readers, and the two panel-store stubs).
- **The panel-store stubs under-declared relative to Garden.** `panel_store_get`
  and `panel_store_set` in `petal-ui/src/panel_stubs.rs` were silent while the
  Garden natives they stand in for call `note_host_read` and `note_effect` —
  the same stub-under-declares-its-native gap step 1 found in `query`. Fixed;
  they now report what Garden's do, so a Garden panel is classified the same
  way in the corpus as in Garden.
- **A corpus app was dead.** The node-editor example (`5278d43`) calls
  `request_frame`, which had no headless stub, so it errored on frame 0 and
  the liveness check from step 1 refused it. `request_frame` / `animating`
  now have stubs that emit the same `animating` marker Garden's do.

The static script `./ts/bin/native-effect-audit.ts` stays: it covers the
natives no corpus app calls, which the runtime audit by construction cannot.

## What the oracle found

Sequencing step 1, done 2026-09-16. The gate and memo differentials now cover
Garden (in-tree, in CI) and worlds-fair (out of tree, on demand). **Every
differential passes**: across 36 in-tree apps and 12 worlds-fair fragments,
gate-on, memo-on and both reproduce the ungated unmemoized frames exactly. No
staleness bug was found in either shipped layer.

Four other things were, and three of them are the reason this task exists.

### Five natives reached host state without declaring it

The full list, from `./ts/bin/native-effect-audit.ts` — a static approximation
of step 4's runtime `--effect-audit`, checked in so the question has an answer
before the real tool exists:

| Native | Reaches | Owed | Consequence |
|---|---|---|---|
| `panel_store_get` | the host's panel store | `note_host_read` | a replayed scope serves a stale value |
| `panel_store_set` | the host's panel store | `note_effect` | **a replayed scope silently stops persisting** |
| `invalidate` | the query cache | `note_effect` | a replayed scope never invalidates, so the pane is served the stale entry forever |
| `load_text_file` | the filesystem | `note_host_read` | a replayed scope serves stale file contents |
| `save_text_file` | the filesystem | `note_effect` | a replayed scope silently stops writing |

All five are declared now. `panel_store_set` is the one worth reading twice: it
was already marked `NativeClass::Effectful`, with a comment explaining why —
and that did nothing for the memo, because `NativeClass` is the
Pending-argument policy and not an effect classification. It is the first
claim of this document, found in the wild by looking. A native that *looks*
declared and is not is worse than one that plainly is not.

The mechanism, for the record: `memo_note_native` classifies by diffing the
activity counters around the call. A native that moves none of them records
nothing, so its enclosing scope memoizes as a pure function of its arguments
and captures, and on the next frame the scope is replayed — the native is not
called at all.

### Reading `frame_count()` costs an app the frame gate, and the ecosystem does it

`frame_count` is a binding that changes every frame by definition, so a script
that reads it can never satisfy the gate. Four Garden GPP apps
(`garden_diff`, `git_panel`, `ok`, `db_view`) and **every** worlds-fair screen
do, usually as a once-per-frame cache key — worlds-fair's Garden host shim
stores `frame_count()` into a `state` cell to compute the fixture once per
frame, which also leaves the frame permanently `StateUnsettled`.

The measured effect: across 45-frame monkey runs, every worlds-fair fragment
skips **zero** frames. The gate is not wrong; it is bypassed. Nothing in the
in-tree `examples/` tree uses this idiom, which is why shipping the gate
against that corpus alone did not reveal it.

This is a live input to **P2**: a dependency-class pass that does not give the
frame counter a class of its own will conclude that these scripts depend on
everything.

### The corpus was passing vacuously in three ways

Each of these read as green while testing nothing, and each is now an
assertion rather than a hope:

- Three Garden GPP apps failed to resolve `bloom` and produced *empty* traces.
  Two empty traces compare equal. `common::assert_corpus_is_live` now requires
  every corpus app to draw on some frame.
- Every worlds-fair fragment drew two commands a frame — the "waiting for the
  game…" path — because `panel_stubs`' `query` answers a loading `Pending`
  forever and nothing supplied a model. `petal-ui-run --query-fixtures` (and
  `wf-ui --print-fixtures` upstream) now supply the real fixture models; the
  same fragments draw 53–363 commands a frame. `oracle-external.ts` fails a
  fragment that falls back under 10.
- The stub `query` did not call `note_host_read()`, though the Garden native it
  stands in for does. A stub that under-declares relative to its native hides
  exactly the staleness this corpus exists to find.

### A count in this document was wrong

worlds-fair registers **8** natives, not 114 — the four host-seam functions
(`wf_model`, `wf_fragments`, `wf_action`, `wf_goto`) plus the Garden stub's
four. The `wf_*` names that look like natives are Petal functions in
`lib/*.ptl`. The ecosystem total is **253** registered natives, not ~365:
112 core, 93 petal-ui, 33 Garden, 8 worlds-fair, 7 integrations. That makes
step 5 materially smaller than planned, and it moves where the work is: 95 of
the 112 undeclared natives are in the core, where step 2 already puts them.

## RunPolicy (sequencing step 2)

Done 2026-09-16. [`rust/src/policy.rs`](../../rust/src/policy.rs) holds one
`RunPolicy { opts, memo, gate }` per `Env`, and the combinations anyone asks
for have names:

| Policy | Optimizer | Memo | Gate |
|---|---|---|---|
| `fast` (default) | on | on | on |
| `baseline` | off | off | off |
| `explain` | on, preserving every traced instruction | off | off |
| `replay` | on | on | off |

Modifiers switch one layer: `fast-memo` is the gate alone, `replay-memo` is
neither over optimized code, `baseline+gate` the gate over unoptimized code.
`petal run --policy`, `petal-ui-run --policy`, `bench_panel --policy` and
`PETAL_POLICY` all take the same spelling; `--no-opt` / `PETAL_OPT=off` remain
as `baseline`, and `--no-gate` / `--no-memo` as modifiers on whatever policy is
in effect.

What moved:

- **`OptFlags` is lowering-only.** `memo_scopes` left it. It had been part of
  the bytecode cache key, so toggling memoization re-lowered every program
  for no reason.
- **The gate lives on the env.** `FrameCore` had its own `gate` and `memo`
  fields and wrote `memo` into the env before every run, so the env's setting
  and the host's could disagree — which is why the old escape-hatch test had
  to copy one into the other by hand. Both fields are gone; `FrameCore`,
  Garden's `set_frame_gating` and the SDL game loop read `env.policy().gate`,
  so `PETAL_POLICY=replay` turns the gate off in every host at once.
- **The oracles say what they compare.** `tests/gating.rs` is `fast` against
  `replay`, `tests/memo.rs` is `replay` against `replay-memo`, the bytecode
  differentials are `baseline` against `fast`, and `oracle-external.ts` runs
  `baseline` against `fast-memo`, `replay` and `fast` — a stronger baseline
  than before, which kept the optimizer on.

  Not yet run in that form: worlds-fair's `wf-ui` at HEAD (`9a8cc25`) has no
  `--print-fixtures` — the flag is silently ignored, the fixtures file comes
  out empty and `petal-ui-run` rejects it. The upstream half of step 1 needs
  landing in worlds-fair before `oracle-external.ts` runs again.

## Sequencing

This lands *between* P1 and P2, not after them, because P2 consumes its
output. The ordering below also folds in two related seams — a shared validity
layer (**B**) and a named run policy (**C**) — identified in the same review.

| # | Work | Why here |
|---|---|---|
| 1 | ~~**Run the gate/memo oracle over Garden and worlds-fair**~~ **done** | Outstanding correctness debt on *shipped* layers. Do it before adding a third. It also produces the list of undeclared host natives that step 4 above needs. Both differentials pass; five undeclared natives found and fixed; see [What the oracle found](#what-the-oracle-found). |
| 2 | ~~**C — a named `RunPolicy` in place of `OptFlags`**~~ **done** | Small, and it is a tool for everything after it: `fast` / `explain` / `baseline` / `replay` as named modes makes the differential oracle a one-word argument instead of an env var plus a comment. Worth having *before* the work that leans on it, not after. |
| 3 | ~~**A — this task, steps 1–3**~~ **done** ∥ **P1's top-level body and loop bodies** (next) | Independent of each other: A is the native boundary, P1's remainder is the lowering. The top-level body is the largest measured P1 gap (the spreadsheet moved only 1.32 → 1.15 ms because it is one long script with few calls worth replaying), so it should not wait. |
| 4 | ~~**A step 4**~~ **done**, **A step 5** ∥ **E — a shared host frame driver** | The ecosystem migration is mostly other repos and can proceed at its own pace. E consolidates the gate → run → retain → invalidate loop that five hosts hand-wire, so P0's remaining work lands once instead of five times. |
| 5 | **P2 — dependency classes, landing B with it** | P2 is the first consumer of the declared `reads` classes. Land the shared validity layer *as part of* P2 rather than as a standalone refactor first: with the gate and the memo it has two clients and the abstraction is speculative; with P2's block granularity it has three and pays for itself. |
| 6 | **P0's segmented output and damage rectangles** | Sits on E. Also benefits from P2's block guards, which is what gives a segment a stable identity. |
| 7 | **P3 — keyed collections and the hit-test index** | Last, as planned. It changes the representation of lists and maps, so it should not overlap a refactor of the native boundary; and its hit-test index is the thing that finally retires the per-row `hovered` validation that dominates P1's remaining list cost. |

Two ordering constraints are hard rather than preferential:

- **A before P2.** A compile-time class pass cannot be seeded from runtime
  counters. Building P2 on inference means building it twice.
- **The oracle (1) before anything new lands.** Three layers will shortly be
  validating each other's assumptions; the corpus they are validated against
  should first include the two largest real Petal codebases. It now does, and
  nothing new should land against the old corpus.

`D` — formalizing the passive observers (`explain` trace, observations,
profiler, absorption log, emit origins) behind the existing single
`Vm::hooks` test — is genuinely optional and fits anywhere. It is the only
part of the runtime where a plugin-style registry honestly applies, and the
fact that it is this small is the measure of how little a general plugin model
would have bought.
