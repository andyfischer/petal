# Declare what a native does, instead of inferring it at runtime

Status: **proposed**, 2026-09-16. Not started.

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

The ~365 registration sites span five repositories, two of which
(`~/worlds-fair`, `~/garden`) are not in this tree. So the row cannot be made
mandatory in one commit, and it does not need to be.

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
   114 + 33 natives are where the four-call-site gap actually lives.
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

Plus, per the reactive plan's outstanding hazard: the same differential run
over the scripts in `~/garden`, `~/.garden` and `~/worlds-fair/ui/ptl`, which
**no layer has yet been checked against** (see
[reactive-rendering-plan.md](../dev/reactive-rendering-plan.md) hazard table,
"Correctness: a missed dependency is a stale pixel").

Performance is a secondary check — this is a correctness and structure change
first — but step 5 should show up as a small win on native-heavy frames, where
the `activity()` snapshot per call disappears:

```bash
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/dashboards/analytics-dashboard/app.ptl 600 --wiggle
```

The real payoff is not measured here; it is that P2 becomes buildable.

## Sequencing

This lands *between* P1 and P2, not after them, because P2 consumes its
output. The ordering below also folds in two related seams — a shared validity
layer (**B**) and a named run policy (**C**) — identified in the same review.

| # | Work | Why here |
|---|---|---|
| 1 | **Run the gate/memo oracle over Garden and worlds-fair** | Outstanding correctness debt on *shipped* layers. Do it before adding a third. It also produces the list of undeclared host natives that step 4 above needs. |
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
  should first include the two largest real Petal codebases, which it does not.

`D` — formalizing the passive observers (`explain` trace, observations,
profiler, absorption log, emit origins) behind the existing single
`Vm::hooks` test — is genuinely optional and fits anywhere. It is the only
part of the runtime where a plugin-style registry honestly applies, and the
fact that it is this small is the measure of how little a general plugin model
would have bought.
