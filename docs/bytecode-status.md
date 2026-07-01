# Bytecode backend — status & handoff

Tracking doc for the linear **bytecode VM** that runs alongside the term-graph
step evaluator. Update the milestone checkboxes and the handoff note as work
lands. Companion reading: [Architecture.md](Architecture.md) (backend split),
[speculative-execution-plan.md](dev/speculative-execution-plan.md) (why the heap
is immutable-by-construction — the substrate for the M4 optimization),
[goals.md](goals.md) (performance is the standing weak spot this targets).

Last updated: 2026-07-01. **Status: M1–M3.5 complete — the bytecode VM is the
default backend, batched dispatch makes it 3–6x faster than the graph engine
on compute-bound code, and a differential fuzzer guards parity. M4 is next.**
The VM
runs the entire language — straight-line, calls, closures, all control flow,
match, and persistent state — and matches the graph engine on value, print
output, final state, and error text across the whole `examples/` corpus and the
vitest suite. `ts/bin/test-examples.ts` now runs every example under *both*
backends and fails on any divergence. `Backend::default()` is `Bytecode`; the
graph engine remains reachable via `--backend=graph` / `PETAL_BACKEND=graph`
as the correctness oracle.

**2026-07-01 plan revision (expert review).** Three changes to the plan below:
(1) the M4 uniqueness analysis was re-specced — the original condition 3
("not a loop-carry alias") *excluded* the loop-carried accumulator
(`row = append(row, x)` inside a loop), which is the dominant mutation pattern
in the corpus (`game_of_life.ptl`, `particles.ptl`) and in sketch code
generally; the revised spec makes the phi cycle the centerpiece via a
**phi-cycle uniqueness rule**. (2) A new **M3.5 benchmark/profiling
checkpoint** gates M4 — the 11–17 fps number predates the VM, and per-call
frame allocation may rival COW cost. (3) A **differential fuzzer** is added:
two oracles exist and only 22 hand-written examples exercise them.

---

## Why

Execution today walks the term graph node-by-node (`backend/graph/`). It is
*introspection-first, not a fast VM* — heavy creative-coding sketches run
~11–17 fps. Two structural costs dominate:

1. **Per-block frame churn.** Every `Branch`/loop/`Match` arm pushes a `Frame`;
   cross-scope reads walk `parent_frame` links (`backend/graph/mod.rs`,
   `read_register`).
2. **Whole-container copies.** The heap is immutable-by-construction: every
   `SetIndex`/`SetField`/`append` clones the entire backing `Vec`/`IndexMap` and
   allocates a new id (`heap.rs`). `DupStats`/`AllocStats` (`stats.rs`) exist
   *specifically* to measure this — the module comment says the counters are
   there "so we can watch the numbers fall as escape analysis and structural
   sharing teach the runtime to reuse live payloads."

The bytecode VM is a linear **lowering** of the term graph that removes cost (1),
and (behind escape analysis) removes cost (2) by mutating provably-unique,
non-escaping containers in place.

## Design decisions (locked)

- **Bytecode is a lowering of the term-graph IR, not a replacement.** The graph
  stays canonical — provenance, slicing, autodiff, `explain`, hot-reload, and the
  IR-as-target JSON contract keep working on it unchanged. Lowering source =
  `Program` (`program.rs`).
- **The original engine is the "graph" backend** (`Backend::Graph`), contrasting
  `Backend::Bytecode`. (It walks the term graph, so "graph" is accurate — not
  "AST".)
- **Every optimization is individually flag-gated** (`OptFlags`). "Bytecode with
  all opts off" and "graph backend" are two independent correctness oracles for
  "bytecode with opts on." Build toward the optimization payoff early, but keep
  each opt disable-able to isolate bugs.

---

## Architecture

### Module layout (`rust/src/backend/`)
```
backend/
  mod.rs        # Backend enum, OptFlags, re-exports StepResult/RuntimeClosure/Evaluator
  ops.rs        # SHARED pure value ops (arithmetic/compare/alloc/field/index)  [DONE]
  calls.rs      # SHARED call resolution (resolve_callable, make_overload_set)  [DONE]
  pattern.rs    # SHARED match_pattern  [DONE]
  errors.rs     # SHARED error annotation (position/snippet/provenance/trace)  [DONE]
  graph/        # the step evaluator (delegates value/call/pattern/error to the shared modules)
  bytecode/
    isa.rs      # Inst, Reg, Label, BytecodeFn (+origins/result_reg), BytecodeProgram (+match_binds)  [DONE]
    lower.rs    # Program -> BytecodeProgram (recursive block emitter, jump backpatching)  [DONE thru M3]
    disasm.rs   # text + JSON rendering for show-bytecode / ShowBytecode  [DONE]
    vm.rs       # Vm, VmFrame, step(), calls, all control flow, state, match, intrinsics  [DONE thru M3]
    tests.rs    # differential + multi-run-state + resumability tests vs the graph oracle  [DONE]
    escape.rs   # uniqueness/escape analysis for in-place mutation  [STUB — returns empty set]
```

**The parity lever landed.** Every value-producing op, call resolution,
pattern match, and error-annotation path is a *shared* free function that both
`graph::Evaluator` and `bytecode::Vm` call, so the two engines cannot diverge on
semantics — only on the mechanical shape of their frames.

### Lowering model
- **Unit = one function.** One `BytecodeFn` per `FunctionDef`, plus the root
  block as an implicit root function. Within a function, *all* its blocks (body +
  every nested control-flow `child_block`, but never a called function body —
  those hang off `MakeClosure`) flatten into one instruction stream over one flat
  register file. This is what eliminates per-block frame churn: cross-block reads
  become direct register reads.
- **Flat registers reuse the compiler's allocation.** Each block gets a base
  offset (running sum of preceding blocks' `register_count`); a term's flat
  register is `base[block] + term.register`. Phantom terms (params/captures/loop
  vars) map correctly because they carry a register even though they never appear
  in an execution list. See `FnLowerer::flat` / `assign_registers` in `lower.rs`.
- **Instruction encoding = enum-of-structs** (`Inst`). Operand counts are
  heterogeneous; the disassembler stays trivial; Rust compiles dispatch to a jump
  table. Packed encoding is a later, profiling-gated option behind the same type.

### How control flow lowers (M2 — IMPLEMENTED; notes on what differs from the sketch below)
- **New ops added beyond the original ISA:** `LoadNil`/`LoadBool` (branch default
  + short-circuit results), `MatchFail` (no-arm-matched runtime error). `StateInit`'s
  label field was renamed `init`→`after` (it's the cache-hit *skip* target, since
  the init block is inlined right after the op).
- **break/continue run enclosing phi carry-outs.** A break/continue emits the
  `phi_outs` of every region from its point up to the loop body (innermost first)
  *before* jumping — replicating the graph engine's per-frame phi propagation as
  it pops enclosing frames. Without this, a rebinding inside an `if` that then
  `break`s would not carry out. See `emit_break_or_continue` / `emit_exit_phi_chain`
  in `lower.rs` and the `break_carries_rebind_through_nested_if` test.
- **Match binding registers** are precomputed per arm for *both* the guard and
  body blocks (each has its own registers) into `BytecodeProgram.match_binds`;
  the `MatchArm` op runs the shared `match_pattern` then writes captures there.
- **Loop-index context** is maintained on the `VmFrame` by the loop opcodes and
  keys per-iteration `state`. Range keys by iteration count, not value.

Original design sketch (still accurate for the shape):
- `Phi` → `Move dst<-input0` at its parent-block position; `Block.phi_outs` →
  `Move dst<-src` at each exit edge of the child region (merge points / loop
  back-edges). Branches that don't rebind emit nothing, so the init value
  survives — identical to today's semantics.
- `Branch`/`And`/`Or` → `JumpIfFalse`/`JumpIfTrue` + `Jump` + a `Move` writing the
  arm result into the control term's register.
- `ForLoop`/`NumericForLoop`/`WhileLoop` → `*Init` / `*Next` (drives the exit
  jump) / body / phi-out moves / back-edge `Jump` / `LoopPop`. `Break`→`Jump end`,
  `Continue`→`Jump cont`; keep a `(cont,end)` label stack per active loop.
- `Match` → a sequence of `MatchArm` fat-ops that reuse the graph engine's
  `match_pattern` verbatim; guards lower to ordinary instructions ending in
  `JumpIfFalse -> next_arm`.

### State opcodes (M3)
The graph engine builds `RuntimeStateKey{base, loop_indices}` by scanning every
frame's `loop_states`. The VM instead keeps an explicit `loop_idx: SmallVec<LoopKeyPart>`
**on the VmFrame**, pushed/updated/popped by the loop opcodes (`*Init` push,
`*Next`/`LoopBumpIdx` set top, `LoopPop` pop) — O(1) per state op. `StateInit`
lowers its lazy-init child block inline, reached only on a cache miss.
`stack.state`, `touched_state_keys`, `sweep_untouched_state` are
backend-independent and reused as-is.

### Escape / uniqueness analysis (M4) — revised 2026-07-01
The original spec required "not a loop-carry alias", which excluded the pattern
the optimization exists for: every accumulator in real Petal code is a
loop-carried phi (`row = append(row, …)` in `game_of_life.ptl`; per-frame
particle lists in sketches). The revised analysis proves uniqueness through the
back edge instead of giving up at it.

A mutation term `T` on container input `C` lowers to an **in-place** opcode
(`SetIndexInPlace`/`SetFieldInPlace`, new `Heap::*_in_place` methods that
mutate + reuse the id) iff `C` is **statically unique at T**, established by
route A or B, **and** the shared escape condition E holds:

**E. Does not escape:** `C` never feeds a `StateInit/Read/Write`, a
`MakeClosure`/`MakeOverloadSet` capture, a `Return`, another escaping
container, and never crosses a speculative fork boundary.

**A. Straight-line uniqueness:**
1. **Last use:** `T` is the last read of `C`. Compute this by **last-use
   liveness on the lowered bytecode**, not `users(C) == {T}` on the graph —
   the linear IR has a total instruction order, making last-use a classic,
   easy liveness pass, and it is strictly more precise: it permits
   *read-then-mutate* sequences (`len(xs)` then `append(xs, …)`) that a
   single-static-consumer test forbids. (Graph-side `trace_dependents` remains
   the tool for the escape condition E, which is about *edge kinds*, not order.)
2. **Fresh/unique producer:** `C` is an `Alloc*` in this function, the result
   of an in-place-eligible mutation chain, or a phi proven unique by route B.
   Params, captures, and state-reads are conservatively *not* unique.

**B. Phi-cycle uniqueness (loop-carried accumulators — the payoff case):**
a loop-carried phi `P` is unique when its cycle is **linear**:
1. the only consumer of `P` inside the loop is `T` (or a chain of
   in-place-eligible mutations ending at `T`) — plus any number of pure
   *reads* that occur before `T` in bytecode order (route A's last-use test,
   applied within the iteration);
2. the only back-edge source of `P` (its `phi_outs` src) is `T`'s result; and
3. `P`'s loop-entry init value is itself fresh/unique per route A — or, if
   not, **clone once at loop entry** (O(1) amortized over the loop; still a
   categorical win vs. a clone per iteration).
Then each iteration holds the container exclusively and `T` mutates in place.
Nested accumulators (`next = append(next, row)` where `row` is itself a
route-B accumulator) compose: each phi is judged independently.

Uncertain ⇒ fall back to clone-and-alloc. Gated behind `OptFlags.in_place_mutation`.
**Why sound:** the heap is immutable-by-construction, so a dataflow edge to `C`'s
producing term is the *only* way any code observes it — A/B + E completely
enumerate observers, a purely static graph property (same argument the codebase
uses for `fork` safety). Route B extends the enumeration around the back edge:
if the cycle is linear, iteration *i+1*'s phi value has exactly one producer
(iteration *i*'s `T`) and no other live observer.
**Verify** via triple differential (graph / BC-noopt / BC-opt) + assert
`DupStats::total_bytes()` strictly drops on `game_of_life.ptl` and
`particles.ptl` specifically — these are the loop-accumulator workloads; if the
byte counts don't fall there, route B isn't firing and the analysis has a bug.
**Hazards:** heap free-list id reuse (in-place only fires while `C` is a live
root; add `debug_assert!(alive)`); speculative fork sharing (add a per-heap
`fork_watermark`; `*_in_place` refuses ids below it); state/closure-captured ids
(forbidden by condition E).

**Fallback route if static analysis misses too much:** dynamic uniqueness
(Swift-CoW `isKnownUniquelyReferenced` / Koka-Perceus style) — a refcount or
unique-bit per heap element, checked in the mutator at runtime. It handles
loop-carry, params, closures, and fork automatically with zero analysis, but is
invasive here: `Value` is `Copy` and register files are plain `Vec<Value>`
(no `Drop` hooks), so accurate counts would mean instrumenting every register
write in both backends. Keep in the back pocket; a hybrid "owner-bit" (set from
static last-use info, checked dynamically) is the cheaper bridge if needed.

---

## Inspection

```
petal show-bytecode <file>          # annotated text disassembly
petal show-bytecode --json <file>   # one object per function, disassembled + reg metadata
petal show-bytecode -e '<code>'     # inline source
```
Plus the `ShowBytecode` MCP tool (`ts/tools/petal-mcp.ts`). **Every program now
lowers and disassembles** — calls, control flow, loops, match, and state all
lower. Run any program on the VM with `petal run --backend=bytecode <file>` (or
`PETAL_BACKEND=bytecode`), and force the graph oracle with `--backend=graph`.

Example:
```
$ petal show-bytecode -e 'let x = 1 + 2 * 3'
fn <root>  (83 regs, 0 loop slots)
     0  r80 = const 1
     1  r81 = const 2
     2  r82 = const 3
     3  r83 = r81 * r82
     4  r84 = r80 + r83
```
(Registers start ~80 because the root block reserves registers for builtin
phantom terms — expected.)

---

## Milestones

- [x] **M0 — Rename + skeleton + inspection.** `eval/` → `backend/graph/`;
  `backend/mod.rs` (`Backend`, `OptFlags`); `isa.rs`, `disasm.rs`, straight-line
  `lower.rs`; `show-bytecode` CLI + `ShowBytecode` MCP. No VM. **Shipped**, 137
  tests green.
- [x] **M1 — Core VM.** `vm.rs` executing M0 ops + `Call`/`MethodCall`/
  `BuiltinCall`/`MakeClosure`/`MakeOverloadSet`/`Return`, CallFrame lifecycle,
  native dispatch, sync intrinsics. Shared handlers factored out
  (`backend/{ops,calls}.rs`). Differential tests green. **Shipped.**
- [x] **M2 — Control flow.** Phi→Move, phi_outs→Move at exit edges, Branch/And/Or,
  all loops, Break/Continue (with enclosing-phi-chain emission), Match (shared
  `pattern.rs`). Recursive block emitter + jump backpatching in `lower.rs`. All
  18 non-state examples differential-green. **Shipped.**
- [x] **M3 — State + parity + default flip.** State opcodes + per-frame
  `loop_idx`; `run_bounded` resumability (test); GC-between-steps (shared); sync
  intrinsics; shared error annotation (`backend/errors.rs`) → full value/output/
  state/**error** parity; entire vitest suite + all 22 examples green under
  `PETAL_BACKEND=bytecode`. `ts/bin/test-examples.ts` is now a differential
  sweep (both backends, byte-identical stdout/stderr required; `--backend=<b>`
  runs one). Default flipped to `Bytecode`. **The flip's focused re-validation
  earned its keep: it caught two real integration bugs** outside the parity
  suites, both in the hot-reload path (`transfer_state`): (a) `Env::insert_program`
  replaced a program under the same `ProgramId` without invalidating the cached
  bytecode lowering, so the VM ran stale code; (b) `transfer_state` reset the
  graph engine's frames but not `vm_frames`/`vm_started`, so the VM treated the
  post-transfer run as already complete and produced no output. Fixed by
  invalidating the cache in `insert_program` and by a shared
  `Stack::reset_execution()` used by both `reset_stack` and `transfer_state`
  (the two hand-maintained reset lists had already drifted). The transfer tests
  now run under both backends. Re-validated after the flip: full Rust suite,
  vitest (same 3 pre-existing backend-independent failures), differential
  example sweep, petal-sdl build + tests, both WASM packages built, and a Node
  smoke test driving `PetalRuntime` (`reset_and_run` state persistence) on the
  fresh WASM.
- [x] **M3.5 — Benchmark & profiling checkpoint (gates M4). Shipped — and it
  paid for itself several times over.** Substrate added: `benchmarks/*.ptl`
  (life, particles, calls, arith, append — engine-bound workloads mirroring
  sketch inner loops) + `ts/bin/bench-backends.ts` (release build, median of
  N runs per backend, output-diff enforced). Findings, in discovery order:
  1. **The VM was no faster than the graph engine** (arith: ~340ms both).
     Cause: `Env` re-resolved four maps and rebuilt the `Vm` struct **per
     instruction** (~190ns/inst of pure orchestration). Fixed with
     **batched dispatch** — `Vm::run_batch(budget)` runs an inner loop
     (yielding for GC/completion/error), `Env::step_n` hands it
     `BYTECODE_BATCH` (65_536); `run`/`run_bounded` consume (result, count).
     The public single-`step` API is unchanged. Result: **VM 340ms → 47ms.**
  2. **A CLI bug made earlier CLI-level sweeps vacuous:** the bare shorthand
     `petal <file> --backend=graph` silently dropped all flags after the
     file and ran the default backend — so the "differential" example sweep
     was comparing bytecode against bytecode (caught by sampling a
     `--backend=graph` process and finding `Vm::run_batch` frames). Fixed:
     the shorthand now goes through `parse_run_args`. (The Rust differential
     tests and `PETAL_BACKEND` paths were always genuinely differential.)
  3. **Speedups with batching (release, medians incl. ~10ms startup):**
     arith **6.1x**, life **5.6x**, calls **4.8x**, particles **2.9x**,
     append **1.25x**. Reading: dispatch-bound code enjoys the VM; COW-bound
     code (append) barely moves — M4 is squarely the next payoff.
  4. **`DupStats` baselines (debug build, `--dup-stats`):** `append.ptl`
     60,000 list dups / **1.08 GB** copied (quadratic COW); `particles.ptl`
     30,300 / **108.7 MB**; `life.ptl` 11,152 / **2.0 MB**. Every hot copy
     is a loop-carried accumulator — direct evidence for route B.
  5. **Per-call frame cost ≈ 13%** of a call-heavy microbenchmark
     (`VmFrame` register-Vec malloc/free, measured by sampling a scaled
     `calls.ptl`); plain interpreter dispatch is ~70%. The Lua-style
     register stack is therefore an M5 item, **not** an M4 gate.
  6. **Differential fuzzer** (`backend/bytecode/fuzz.rs`): seeded xorshift
     grammar over always-terminating programs (nested loops with frozen
     counters, break/continue behind ifs, match with guards, lists/records,
     functions), exact-agreement runner including error text; 500 seeds per
     `cargo test`, `PETAL_FUZZ_ITERS` to soak. **Found a real bug on its
     first soak run (seed 431)** — see the semantics fix below. 50,000 seeds
     green after the fix.
- [x] **M3.5a — Semantics fix: `break`/`continue` transfer control
  immediately (fuzzer seed 431).** The two engines disagreed on a real
  language semantic. The graph engine set a flag and **kept executing the
  rest of the current block** — dead statements after `break`/`continue`
  ran, could rebind loop variables, and could even raise errors; only
  loop-body frames were skipped, and a same-block `continue` followed by a
  not-yet-entered loop was mis-consumed by that loop. This run-to-completion
  quirk was also load-bearing: it kept per-block phi carry-outs coherent
  (src registers were always written). The VM exits immediately
  (conventional), so its exit-chain carry-outs could read a dead rebind's
  never-written register → nil. **Resolution — conventional semantics in
  both engines:**
  - *Graph:* the skip-to-pop on a set flag now applies to **all** frames
    (not just loop bodies); a loop term consumes the flag only when it is
    mid-iteration (`has_loop_state`), so a fresh loop after a `continue` is
    correctly skipped.
  - *Compiler (the shared fix):* branch/match arm blocks are now **seeded
    with carry-slot entry copies** (`seed_arm_entry_copies` in
    `compiler/phi.rs`), mirroring what `emit_body_phi_ins` already did for
    loop bodies: every phi'd name gets an entry `Copy` of the parent phi,
    logged as the arm's initial rebind, and later in-arm rebinds share that
    register. The phi-out src register therefore always holds the name's
    latest value — whether zero, some, or all of the arm's rebinds executed
    before a mid-block exit. Pattern-shadowed names are skipped (arm-local).
  - *Rejected approach:* statically filtering exit-chain carry-outs to
    "rebinds emitted before the exit" — it breaks the shared-slot design
    (an *earlier executed* rebind writes the same register a later dead
    rebind names; the vitest loop-carry tests pin exactly this).
  - Pinned by `break_continue_transfer_control_immediately` and
    `arm_carry_slots_survive_mid_block_exits` in `bytecode/tests.rs`; whole
    suite + 22-example sweep + vitest (same 3 pre-existing failures) +
    50k fuzz seeds green. **This is a user-visible semantics change** (dead
    code after `break`/`continue` no longer executes in the graph backend);
    no example or test relied on the old behavior.
- [ ] **M4 — In-place mutation.** `escape.rs` implementing routes A + B and
  escape condition E above (last-use liveness on the lowered bytecode +
  phi-cycle rule on the graph), `Heap::*_in_place`, in-place opcodes,
  `fork_watermark` guard, behind `OptFlags.in_place_mutation`. Verify via
  triple differential + `DupStats` byte-drop assertions on the loop-accumulator
  examples named above.
- [ ] **M5 (optional, profiling-gated).** Packed encoding, superinstructions,
  pattern-tree micro-ops (Maranget-style decision trees replacing the
  `MatchArm` fat-op), register-file reuse/compaction (if not already handled
  by the M3.5 register-stack work) — behind the same `Inst`/flag APIs.
  **Structural sharing (RRB vectors / HAMTs)** also lives here as the
  *complement* to M4, not its alternative: it caps the worst case (O(log n)
  copy instead of O(n)) when the analysis can't prove uniqueness, at the price
  of read-path constants, and preserves fork/speculation semantics with zero
  hazards. Reach for it only if post-M4 `DupStats` shows a stubborn remainder
  from params/state containers.

---

## Handoff — next actions

M1–M3.5 are done and committed (one commit per chunk: M1a/M1b/M1c,
M2a/M2b/M2c, M3-state+annotation, M3-flip, M3.5-bench+batch+fuzz). The VM is
at full behavioral parity with the graph engine, is the default backend, runs
3–6x faster on compute-bound code thanks to batched dispatch, and parity is
now guarded by a 500-seed-per-test differential fuzzer (soaked to 50k). The
checkpoint's outputs settled both open questions: the register stack defers
to M5 (~13% of a call-heavy microbench), and M4's before-numbers are pinned
(`append.ptl`: 1.08 GB copied; benchmark table in the M3.5 entry). Next: M4.

### Next: M4 — in-place mutation (the payoff)
`escape.rs` is still a stub returning an empty set. Implement the **revised**
uniqueness analysis (routes A + B and escape condition E in the **Escape /
uniqueness analysis** section above). The critical point of the revision: route
B (phi-cycle uniqueness) is not optional polish — the loop-carried accumulator
is the dominant mutation pattern in the corpus, and without route B the
analysis fires on almost nothing that matters. Last-use runs as a liveness pass
over the lowered bytecode; the escape and phi-cycle checks run on the term
graph (`trace_dependents` + the phi-source set from `trace_provenance`). Then
add `Heap::*_in_place` methods, emit `SetIndexInPlace`/`SetFieldInPlace`
(already in the ISA) when the analysis proves safety, gate on
`OptFlags.in_place_mutation`, and add the `fork_watermark` guard. **Verify** via
triple differential (graph / BC-noopt / BC-opt agree) plus assert
`DupStats::total_bytes()` strictly drops on `game_of_life.ptl` and
`particles.ptl` (if it doesn't drop there, route B isn't firing). The
`OptFlags` plumbing, `--no-opt` flag, and both correctness oracles are already in
place.

**Gotchas already discovered (still true).**
- `let x = <expr>` names the result term directly — no trailing `Copy`/`Move`.
- Root-block registers start ~80 because builtin phantom terms reserve registers
  first. Flat registers are correct regardless; don't assume registers start at 0.
- Petal `if` uses `then … end`, not braces (`if x > 0 then x = 2 end`).
- break/continue must run enclosing `phi_outs` (see the M2 notes above) — the
  single subtlest correctness point in the whole lowering. The carry-outs are
  only sound because **every carrying block initializes its carry slots at
  entry** (loop bodies via `emit_body_phi_ins`, branch/match arms via
  `seed_arm_entry_copies`) and all in-block rebinds share the slot register —
  see the M3.5a milestone entry. Don't "optimize away" the seed copies.
- `break`/`continue` transfer control immediately in **both** engines
  (M3.5a). The graph engine's old behavior — trailing dead code executing
  until the block ended — is gone; don't reintroduce flag-style semantics.
- The graph **re-annotates** already-annotated errors that propagate back up
  through a synchronous intrinsic call (`call_closure_sync`); the VM matches this
  by annotating at every `step()` error. Don't "fix" the apparent
  double-annotation while parity is the goal. Once bytecode is the default,
  decide whether this is *the semantics* or a quirk — and if a quirk, fix it in
  both engines at once via the shared `backend/errors.rs`, and write the
  decision down here.
- Loop-index keys use the 0-based *iteration count*, not the loop value (matters
  for `range(start, end)` with `start != 0` and for state-map key parity).
- The VM adds *derived caches* the graph engine never had: `Env.bytecode`
  (lowering per `ProgramId`) and per-stack VM run-state (`vm_frames`,
  `vm_started`). Any operation that replaces a program in place or resets a
  stack must account for them — `Env::insert_program` drops the cached lowering,
  and `Stack::reset_execution()` is the single reset point for per-run state
  (don't reset stack fields by hand; the hand-maintained lists drifted once
  already and broke hot reload under the VM).

---

## Key files
- `rust/src/program.rs` — lowering source (`TermOp`, `Term`, `Block.phi_outs`,
  `FunctionDef`) + analysis substrate (`trace_dependents`, `trace_provenance`).
- `rust/src/backend/{ops,calls,pattern,errors}.rs` — the **shared** handlers both
  engines call (value ops, call resolution, pattern matching, error annotation).
  These are the parity lever; change semantics here, not in one engine.
- `rust/src/backend/graph/{mod,exec,ops,call,state,loops,pattern,error}.rs` — the
  step evaluator, now delegating to the shared handlers.
- `rust/src/backend/bytecode/{isa,lower,vm,disasm,tests}.rs` — the bytecode
  backend (complete through M3.5); `escape.rs` is the M4 stub.
- `rust/src/backend/bytecode/fuzz.rs` — the differential fuzzer (seeded
  generator + exact-agreement runner; `PETAL_FUZZ_ITERS` scales the soak).
- `benchmarks/*.ptl` + `ts/bin/bench-backends.ts` — the backend benchmark
  suite (release build, medians, output-diff enforced).
- `rust/src/compiler/phi.rs` — phi emission, carry slots, and the entry-copy
  seeding (`emit_body_phi_ins`, `seed_arm_entry_copies`) that keeps mid-block
  exits sound.
- `rust/src/heap.rs` + `rust/src/stats.rs` — COW mutators + free-list + `fork`
  (M4 in-place target + hazard surface); `DupStats` verification oracle.
- `rust/src/env/mod.rs` — `run`/`run_bounded`/`RunOutcome` (backend dispatch goes here).
- `rust/src/cli.rs`, `ts/tools/petal-mcp.ts` — `show-bytecode` / `ShowBytecode`.
- `ts/bin/test-examples.ts` — differential harness (extend for `--backend`).
