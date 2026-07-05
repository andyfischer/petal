# Bytecode backend — future ideas

The bytecode VM is Petal's only execution engine (an earlier term-graph step
evaluator served as the reference oracle during bring-up and was removed at
parity — see [bytecode-migration.md](bytecode-migration.md)). It runs the entire
language — value, print output, final state, and error text verified across the
`examples/` golden corpus and the vitest suite — is 4–10x faster on compute-bound
code, and — by default — mutates provably-unique
containers in place so the loop-accumulator COW cost is gone (`DupStats` is 0
bytes on `append`/`particles`/`life`; recover the clone-and-alloc baseline with
`--no-opt` / `PETAL_OPT=off`). The design, milestone history, and the hard-won
gotchas that got it there lived in the old `bytecode-status.md`; this file keeps
only the **open follow-ups** that outlived it.

Nothing here is required. Each item is gated on a *specific* workload the current
profile does not cover — the standing rule is **re-run `ts/bin/bench-opts.ts`
and `--dup-stats` (default *and* `--no-opt`) first**, and only pick an item up if
the numbers demand it. If none do, the honest move is to leave the VM as-is and
spend effort elsewhere in the language.

## Correctness surface & invariants (read before touching the VM)

Everything below is a *performance* idea. The parity invariants are non-negotiable
and the differential machinery is how you keep them:

- **The oracles** — BC-noopt / BC-route-A-only / BC-all — must agree exactly
  (value, output, state, error text). BC-noopt (clone-and-alloc) is canonical;
  the example golden corpus (originally frozen from the graph engine) anchors
  absolute correctness.
- **The differential fuzzer** (`backend/bytecode/fuzz.rs`) is the guard: seeded
  generator + four-oracle exact-agreement runner, `PETAL_FUZZ_ITERS` to soak
  (routes A and B were each soaked to 300k seeds before their default flip).
  Any new opcode or encoding must earn the same soak before going default-on.
- **Every optimization stays individually flag-gated** (`backend::OptFlags`), so
  "opts off" remains a clean third oracle for isolating a bug.

## Deferred M5 items (profiling says not warranted yet)

### Structural sharing (RRB vectors / HAMTs)
The *complement* to the M4 in-place analysis, not its alternative: it caps the
worst case at O(log n) copy instead of O(n) when uniqueness **can't** be proven,
preserving fork/speculation semantics with zero hazards, at the price of
read-path constants.

**Why deferred:** the trigger was "post-M4 `DupStats` shows a stubborn remainder
from params/state containers." It does not — routes A+B prove uniqueness across
the whole corpus and `DupStats::total_bytes()` is 0 under default flags on every
accumulator workload. There is no un-provable COW left to cap.

**Revisit when:** a real program surfaces a params/state-container COW remainder
that the analyses genuinely can't prove, **or** `fork` moves to `Rc`-shared
payloads (speculative-execution plan, Increment 4) — at which point structural
sharing is the natural fit and the `fork_watermark` hazard (see below) reappears.

### Superinstructions / packed encoding
M3.5 attributed ~70% of a call-heavy run to plain interpreter dispatch, so fusing
hot instruction pairs or packing the enum-of-structs `Inst` into a byte stream
*could* help.

**Why deferred:** large, parity-risky change (each fused/packed op is a new
correctness surface against the opt-level oracles and the golden corpus) for an
unquantified win. The packed encoding was always
intended to live behind the same `Inst` type / flag APIs so the disassembler and
lowering stay unchanged.

**Revisit when:** a real workload is measured dispatch-bound *after* the M5a
allocation work (frame pooling + operand `SmallVec`) already landed —
not on the existing microbenchmarks.

### Pattern-tree micro-ops
Replace the `MatchArm` fat-op (which re-runs the shared `match_pattern` per arm)
with a Maranget-style decision tree of small opcodes, sharing the discrimination
work across arms.

**Why deferred:** there is no match-heavy workload in `test/benchmarks/`, and none in
the sketch corpus that dominates runtime.

**Revisit when:** add a match-bound benchmark first; only spend here if it shows
`MatchArm` is hot.

### Register-file reuse/compaction beyond pooling
M3.5 measured the per-call `VmFrame` at ~13%; the malloc/free of it is gone
(frame pooling, shipped — a returned `VmFrame`, register file included, recycles
into `Stack::vm_frame_pool` rather than dropping). The remainder is the `Nil`-fill
and the frame push/pop bookkeeping itself. A Lua-style single contiguous register
stack (frames as base offsets into one `Vec`, no per-frame allocation at all)
would remove the rest.

**Why deferred:** pooling already captured the allocator-traffic win; the
residual is small and the contiguous-stack refactor is invasive (every register
access becomes base-relative, and GC-root scanning / the `vm_frame_pool` free-list
would need rework).

**Revisit when:** a call-heavy profile after pooling still shows frame management
as a top cost.

## Resolved decisions

### Error re-annotation was a quirk — now a single annotation
Historically the VM **re-annotated** an already-annotated error as it propagated
back up through a synchronous intrinsic call (`call_closure_sync`): the closure's
`step()` annotated at the true failing term, then the error returned via `?` into
the intrinsic's call site, and the *outer* `step()` annotated it again. The result
spliced the call site's position mid-message and stacked a second snippet and
stack trace onto the message — e.g. a `map(xs, boom)` where `boom` divides by zero
showed `in boom() [line 6, column 10]` (the `map` call site grafted onto the inner
trace) followed by a duplicate snippet. This matched the (now-removed) graph
engine, and was deliberately kept during the parity push.

**Verdict: quirk, fixed.** The re-annotation was misleading — the outer call site
is `map(...)`, not where the division happened, so the second pass pointed at the
wrong source. The innermost annotation (at the actual failing term, with the
frames present at failure) is authoritative. `call_closure_sync` now sets
`Vm::error_already_annotated` when it unwinds with an already-annotated error, and
`step`'s error path passes such errors through untouched instead of annotating a
second time. Raw intrinsic errors (e.g. `map() expects a list`) are *not* flagged,
so they still get annotated once at the `map` call site. The differential fuzzer
stays green (both opt levels annotate identically), as do `error_parity` and the
example golden corpus.

## Hazards that reappear if the substrate changes

- **`fork_watermark`.** Today `Heap::fork` deep-copies the slot vectors, so a
  speculative child mutates its own copy and in-place mutation can't cross a fork
  boundary — the watermark is unnecessary. It becomes required the moment `fork`
  moves to `Rc`-shared payloads (structural sharing / speculative Increment 4):
  `Heap::*_in_place` must then refuse ids below a per-heap `fork_watermark`.
- **Heap free-list id reuse.** In-place mutation only fires while the container
  is a live root; `debug_assert!(alive)` in `Heap::*_in_place` is the net. Any
  change to root scanning or the `vm_frame_pool` free-list must keep that true
  (pooled frames hold only cleared register files — `VmFrame::recycle()` empties
  them — and are not GC roots, so they never resurrect an id; don't add them as
  roots).
