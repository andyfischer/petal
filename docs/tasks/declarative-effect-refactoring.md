# Declare what a native does, instead of inferring it at runtime

Status: **in progress**, 2026-09-16. Sequencing step 1 (the oracle over Garden
and worlds-fair) is done — see [What the oracle found](#what-the-oracle-found).
The task itself (steps 1–5 of [Migration](#migration-in-five-steps-that-each-stand-alone))
is not started.

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
test, run with declarations on and off:

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

## Sequencing

This lands *between* P1 and P2, not after them, because P2 consumes its
output. The ordering below also folds in two related seams — a shared validity
layer (**B**) and a named run policy (**C**) — identified in the same review.

| # | Work | Why here |
|---|---|---|
| 1 | ~~**Run the gate/memo oracle over Garden and worlds-fair**~~ **done** | Outstanding correctness debt on *shipped* layers. Do it before adding a third. It also produces the list of undeclared host natives that step 4 above needs. Both differentials pass; five undeclared natives found and fixed; see [What the oracle found](#what-the-oracle-found). |
| 2 | **C — a named `RunPolicy` in place of `OptFlags`** | Small, and it is a tool for everything after it: `fast` / `explain` / `baseline` / `replay` as named modes makes the differential oracle a one-word argument instead of an env var plus a comment. Worth having *before* the work that leans on it, not after. |
| 3 | **A — this task, steps 1–3** ∥ **P1's top-level body and loop bodies** | Independent of each other: A is the native boundary, P1's remainder is the lowering. The top-level body is the largest measured P1 gap (the spreadsheet moved only 1.32 → 1.15 ms because it is one long script with few calls worth replaying), so it should not wait. |
| 4 | **A steps 4–5** ∥ **E — a shared host frame driver** | The ecosystem migration is mostly other repos and can proceed at its own pace. E consolidates the gate → run → retain → invalidate loop that five hosts hand-wire, so P0's remaining work lands once instead of five times. |
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
