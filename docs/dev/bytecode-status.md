# Bytecode backend — status & handoff

Tracking doc for the linear **bytecode VM** that runs alongside the term-graph
step evaluator. Update the milestone checkboxes and the handoff note as work
lands. Companion reading: [Architecture.md](Architecture.md) (backend split),
[docs/dev/speculative-execution-plan.md](dev/speculative-execution-plan.md) (why the heap
is immutable-by-construction — the substrate for the M4 optimization),
[goals.md](goals.md) (performance is the standing weak spot this targets).

Last updated: 2026-07-03. **Status: M1–M4 complete (routes A *and* B), both
**default-on**, and the profiling-justified slice of **M5 shipped** — the
bytecode VM is the default backend, batched dispatch makes it 3.4–14x faster
than the graph engine on compute-bound code, and in-place mutation eliminates
the COW cost on both fronts: route B (graph-side escape analysis) zeroes the
loop accumulators (`append.ptl`'s 1.08 GB of copies → 0, `particles.ptl`
108.7 MB → 0, `game_of_life.ptl` 2.0 MB → 0) and route A (a last-use liveness
pass on the lowered bytecode, `lastuse.rs`) covers straight-line builders
(`let xs = […]; xs[0] = v`), read-then-mutate (`len(xs)` then `append(xs, …)`),
and per-iteration fresh containers — all with byte-identical output. A
**four-oracle** differential fuzzer (graph / BC-noopt / BC-route-A-only /
BC-all) guards them; both routes soaked to 300k seeds. `OptFlags::default()`
enables both `in_place_mutation` (B) and `in_place_straight_line` (A); recover
the clone-and-alloc oracle per-run with `--no-opt` / `PETAL_OPT=off`. **M5a
(call-path allocation elimination) is in:** the M3.5 re-profile showed ~20% of
a call-heavy workload was frame malloc/free, so `VmFrame`s are now pooled on
the `Stack` and the remaining per-call allocations (arg gathering via an
operand `SmallVec`, capture clone, builtin-name `String`) are gone — the
steady-state call path allocates nothing, and the profile shows zero allocator
frames. The remaining M5 micro-architecture items (superinstructions / packed
encoding / pattern-tree / structural sharing) were re-profiled and found **not
warranted** — see the M5 milestone entry; the current profile is pure
dispatch.**
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
    vm.rs       # Vm, VmFrame, step(), calls, all control flow, state, match, intrinsics; register-file pooling (M5)  [DONE]
    tests.rs    # differential + multi-run-state + resumability tests vs the graph oracle  [DONE]
    escape.rs   # route-B uniqueness/escape analysis (graph-side, feeds lowering)  [DONE]
    lastuse.rs  # route-A last-use in-place rewrite pass (bytecode-side, post-lowering)  [DONE]
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

*(Route A shipped as specced — a last-use liveness pass over the lowered
bytecode in `lastuse.rs`, generalized to Move-closure alias groups because the
lowering emits a `Move` per variable use. See the route-A milestone entry for
what was learned in implementation.)*

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
- [x] **M4 — In-place mutation (route B — the loop-accumulator payoff).**
  Shipped behind `OptFlags.in_place_mutation` (default off; the empty in-place
  set reproduces the clone-and-alloc oracle byte-for-byte, so "opts off" stays a
  clean third oracle). Delivered:
  - **`escape.rs` route B** — a container **value-web** over `Copy`/`Phi`/
    mutation carrier edges, proven unique by: (1) a single fresh-`Alloc*` root
    whose only references are the accumulator spine; (2) one loop-carried phi
    spine (found via a *backward* cone so post-loop escapes never merge in — the
    fix that let `particles.ptl`'s `next` fire without dragging in the
    `particles` it feeds); (3) the mutation is *on the spine* (its result flows
    back into the loop phi's back edge — a bystander mutation on an alias of the
    carried value is rejected); (4) all mutations in-region and every in-region
    observer is a web carrier, *linear* (≤1, or in mutually-exclusive branch/
    match arms — which is how `game_of_life.ptl`'s two-arm `if … append … else
    … append` accumulator lowers). Reads of the *final* value after the loop are
    unrestricted. Route B was chosen as the graph phi-cycle analysis the plan
    called for; the "last-use liveness on lowered bytecode" precision for
    read-then-mutate is folded into the conservative in-region-observer rule
    (rejects an in-loop `len(xs)` — sound, slightly conservative).
  - **`Heap::*_in_place`** (list append/set/drop_last, map set/remove, f64 set/
    swap): mutate the backing store, reuse the id, record no `DupKind` copy,
    `debug_assert!(alive)` (the container is always a live root, so id-reuse is a
    non-hazard). **`ops::set_{field,index}_in_place`** for the `SetField`/
    `SetIndex` opcodes; a `PetalCxt::in_place` flag routes the mutating builtins
    (`append`/…) to the in-place heap methods — set only by the VM, never the
    graph engine. **In-place opcodes** `SetFieldInPlace`/`SetIndexInPlace` and a
    `BuiltinCall{in_place}` flag are chosen *at lowering time* from the analysis,
    so there is zero per-op runtime lookup; the `Env` bytecode cache is keyed on
    the active `OptFlags` and re-lowered when they change.
  - **Verification.** Triple differential (graph / BC-noopt / BC-opt) across the
    22 examples, the `tests.rs` M4 cases, and the fuzzer — now a *triple*-oracle
    fuzzer that also generates list/record aliases (`let ys = xs`) and prints
    every live list's `len`, so an in-place mutation of a shared alias surfaces
    as a value divergence. `DupStats::total_bytes()` strictly drops (to 0) on
    `append`/`particles`/`life`. **The fuzzer found two real unsoundnesses in
    the first draft** (both now pinned by named regression tests + the checks
    above): (a) a pre-loop alias `let ys = xs` of the fresh root — fixed by
    requiring the root's only users be on the spine; (b) *seed 84619* — a
    bystander mutation `let al = xs; al = append(al, v)` on an alias of an
    **outer**-loop carried value, where the append result is discarded — fixed by
    the spine-membership check. Soaked to 300k seeds green.
  - **`fork_watermark` — not needed under the current fork.** `Heap::fork`
    deep-copies the slot vectors, so a speculative child mutates its own copy;
    in-place mutation cannot cross a fork boundary. The watermark becomes
    necessary only if `fork` moves to `Rc`-shared payloads (speculative plan
    Increment 4); a `debug_assert!(alive)` covers the free-list id-reuse hazard.
- [x] **M4 default-on flip.** `OptFlags::default()` now enables
  `in_place_mutation` (spelled out field-by-field, not delegated to
  `OptFlags::all()`, so a future not-yet-proven opt added to `all()` won't
  auto-default-on). Escape hatches unchanged: `--no-opt` and `PETAL_OPT=off`
  both map to `OptFlags::none()` and recover the clone-and-alloc oracle. The
  M3-style re-validation was run in full and is green: entire Rust suite
  (incl. the 300k-capable triple fuzzer at default iters), the 24-example
  differential sweep (graph-clone vs bytecode-in-place, byte-identical),
  vitest (483/483 — the 3 formerly-pre-existing failures are gone),
  petal-sdl build + 7 tests, both WASM packages (`petal-web` +
  `petal-diagram-canvas`, both embed the main crate), and a Node smoke test
  driving `PetalRuntime` on the fresh WASM (persistent `count` increments
  1→2→3 across `reset_and_run` while a fresh in-place loop accumulator stays
  correct each run — proving default-on in-place doesn't corrupt or leak into
  state-captured containers). `DupStats` under **default** flags is now 0
  bytes on `append`/`particles`/`life`; `--no-opt` restores the exact recorded
  baselines (1.08 GB / 108.7 MB / 2.0 MB), confirming the flip is what zeroes
  them.
- [x] **M4 route A — straight-line last-use uniqueness. Shipped and
  default-on** behind its own flag (`OptFlags.in_place_straight_line`,
  independent of route B's `in_place_mutation` so either can be disabled to
  isolate a bug). Implemented as the plan's original spec demanded — **last-use
  liveness on the lowered bytecode** (`lastuse.rs`), *not* a graph pass — the
  linear instruction stream's total order makes "is the container dead after
  the mutation" a reachability question, and it admits the read-then-mutate
  sequences (`len(xs)` then `append(xs, …)`) that a single-static-consumer
  graph test forbids. Runs in `Env::ensure_bytecode` after lowering (and after
  route B's opcode selection), rewriting `SetIndex`/`SetField`/mutating
  `BuiltinCall` to their in-place forms. A candidate fires iff:
  (1) its container register chases back through single-def `Move`s to a
  **fresh root** — an `Alloc*` or an already-in-place mutation (the chain rule:
  `xs[0] = v; xs[1] = w` converts both); (2) the root's **Move-closure alias
  group** is fully tracked (a `Move` into a multi-def register — a phi/carry
  slot — rejects) and no member has a *retaining* reader (call args, closure
  captures, `Return`, state writes, storage into another container, match
  subjects); (3) every member is **dead after the mutation**: no read
  reachable from the candidate before the member's single def re-executes
  (the kill), and no member is the function's fall-off result register.
  Key findings:
  - **The lowering emits a `Move` per variable use** (each `Copy` term has its
    own register), so tracking a single register is vacuous — the unit of
    analysis has to be the Move-closure alias group. The first draft missed
    this and never fired.
  - **The kill in condition (3) is what lets the per-iteration builder fire**:
    `for … do let t = [0, 0]; t[0] = i; grid = append(grid, t) end` re-executes
    the alloc on the back edge before any re-read, so `t`'s mutation is
    in-place even inside a loop — composing with route B on the outer `grid`.
  - **Returning the *rebound* result is safe and fires** (`fn f() let xs =
    [1,2]; xs[0] = 5; xs end`): only the final, value-identical container
    escapes. Returning a *pre-mutation alias* is caught by the group walk.
  - Phi registers (multiple defs) reject at condition (1), which is precisely
    what keeps this pass out of route B's loop-phi territory — mutations in
    branch arms decline (arm carry slots are multi-def), staying conservative.
  - On the three loop benchmarks route A adds nothing (route B already zeroed
    them) — its payoff is builder code: fresh-container field/index
    initialization, pinned by `route_a_dup_bytes_drop_on_builder`.
  - **Verification:** the differential fuzzer grew a fourth oracle
    (BC-route-A-only) so a route-B interaction can't mask a route-A bug, and
    now prints every live container's full contents (not just lengths) so
    element-level corruption diverges; soaked to **300k seeds green** before
    the default flip. The full M3-style re-validation ran green after it:
    entire Rust suite (235), vitest 483/483 (default and `PETAL_OPT=all`),
    24-example differential sweep (default and `PETAL_OPT=all`), petal-sdl
    build + 7 tests, both WASM packages, and a Node `PetalRuntime` smoke test
    (persistent `count` 1→2→3 across `reset_and_run` while route-A builder
    chains and a declined alias stay value-correct each run).
- [x] **M5a — call-path allocation elimination (the register-file reuse the
  M3.5 profile predicted).** The gating re-profile (2026-07-03, post-M4
  defaults) found: `DupStats` already 0 bytes on every benchmark (structural
  sharing has no remainder to cap), but ~20% of `calls.ptl`'s samples were
  allocator traffic — `drop_in_place<VmFrame>` on every return, the
  `vec![Nil; reg_count]` + `captures.clone()` on every call, a `Vec` collect
  for every `Call`/`BuiltinCall` arg list, and a name-constant `to_string()`
  on every `BuiltinCall`/`MethodCall`. (`arith.ptl`, which makes no calls,
  showed zero allocator frames — the costs were all call-path.) Delivered,
  all mechanical, no flag needed (no observable-behavior surface):
  - **Frame pool** (`Stack::vm_frame_pool`): `deliver_value` recycles the
    popped frame (registers/cursors/loop-context cleared) instead of dropping
    it; `push_closure_frame`/`push_root_frame` reset a pooled frame instead of
    allocating. Pooled frames hold no values, so the pool is deliberately
    **not** a GC root — `recycle()` clearing the register file is what makes
    that sound; don't skip it. Capped at 1024 frames so a deep-recursion
    high-water mark isn't retained forever.
  - **Arg gathering** (`Vm::regs`) returns a `SmallVec<[Value; 8]>` — call
    arities above 8 are the only remaining per-call allocation. The capture
    clone in `push_closure_frame` is now a reborrow, and builtin/method name
    lookups borrow the `&str` straight from the program's constant table.
  - **Result (release medians, 5 runs, incl. ~5ms startup):** calls 69.6→63.5
    ms, append 7.7→6.2 ms, particles 30.6→28.0 ms; arith/life unchanged (no
    calls, as expected). The re-sampled profile has **zero allocator frames**
    left — remaining time is pure dispatch (`exec_inst`/`step`/`binop`) plus
    the irreducible register-file Nil-fill on frame reset.
  - **Verification:** full Rust suite (235), fuzzer soaked to **300k seeds**
    (the four-oracle differential exercises deep call/loop nesting, which is
    exactly what frame recycling could corrupt), 24-example differential
    sweep, vitest 483/483, petal-sdl `cargo check` + both WASM crates
    `cargo check --target wasm32-unknown-unknown` clean. (petal-sdl's *link*
    step currently fails on this machine — homebrew `libSDL2.dylib` is
    missing, unrelated to the crate.)
- [ ] **M5 remainder — deferred, profiling says not warranted (2026-07-03).**
  The remaining sketch items are micro-architecture whose payoff the current
  profile does not support; recorded here so the next person re-checks rather
  than re-discovers:
  - **Structural sharing (RRB vectors / HAMTs).** The stated trigger was "post-M4
    `DupStats` shows a stubborn remainder from params/state containers." It does
    not: `DupStats::total_bytes()` is **0** under default flags on every
    accumulator workload (`append`/`particles`/`life`). Routes A+B already prove
    uniqueness for the whole corpus, so there is no un-provable COW left to cap.
    Revisit only if a real program surfaces a params/state-container remainder,
    or if `fork` moves to `Rc`-shared payloads (speculative plan Increment 4),
    where structural sharing becomes the natural fit.
  - **Superinstructions / packed encoding.** M3.5 attributed ~70% of a
    call-heavy run to plain interpreter dispatch, so these *could* help — but
    they are a large, parity-risky change (each fused/packed op is a new
    correctness surface against the four oracles) for an unquantified win, and
    the VM is already 4–10x faster than the reference. Gate on a real workload
    that is dispatch-bound *after* the allocation work above, not on the
    microbenchmarks.
  - **Pattern-tree micro-ops** (Maranget decision trees replacing the
    `MatchArm` fat-op). No match-heavy workload in `benchmarks/` and none in the
    sketch corpus that dominates runtime; add a match-bound benchmark before
    spending here.

---

## Handoff — next actions

M1–M4 are done and **default-on**, both routes, and M5a (call-path allocation
elimination) is in (earlier chunks: M1a/M1b/M1c, M2a/M2b/M2c,
M3-state+annotation, M3-flip, M3.5-bench+batch+fuzz, M4 route B + flip,
M4 route A + flip, M5a frame pool + operand `SmallVec`). The VM is at full
behavioral parity with the graph engine, is the default backend, runs 3.4–14x
faster on compute-bound code (release medians: append 14.1x, arith 5.9x, life
5.3x, calls 5.1x, particles 3.7x), mutates provably-unique containers in place
by default (zero COW bytes on all five benchmarks), and its steady-state call
path allocates nothing (frames pool on the `Stack`). `--no-opt` /
`PETAL_OPT=off` recover the clone-and-alloc oracle.

### Next: nothing is *required*
The bytecode backend is feature-complete and its optimizations are done to the
point the profiling justifies. The post-M5a profile is pure interpreter
dispatch (`exec_inst`/`step`/`binop`/shared ops) and `DupStats` is 0 bytes on
every benchmark, so the M5 remainder (superinstructions / packed encoding /
pattern-tree / structural sharing) is **deferred with a recorded rationale** —
see the two M5 milestone entries. Before picking any of it up, re-run
`ts/bin/bench-backends.ts` and `--dup-stats` (default *and* `--no-opt`) on
*real workloads* (sketches under petal-sdl / Garden), not just `benchmarks/`:
the bar is a *specific* workload the current profile doesn't cover
(dispatch-bound after the allocation work, a match-bound loop, or a genuine
params/state COW remainder). If none appears, the honest move is to leave the
VM as-is and spend effort elsewhere in the language.

**M4 route A gotchas (learned in implementation).**
- **Track Move-closure alias groups, not registers.** The lowering emits a
  `Move` for every variable *use*, so the container id always lives in several
  registers; a per-register analysis never fires. `Move` is the only
  instruction that propagates an id unchanged (clone-mutations write new ids;
  `GetIndex`/`GetField` extract element values), so the group is exactly the
  Move-closure of the fresh root.
- **Retention and ordering are separate axes.** A retaining read (alias into
  another container/closure/state/call) rejects *wherever* it sits — the
  reference outlives the mutation. A pure read is fine *before* the mutation
  and fatal *after* it — that's the reachability walk. Conflating the two
  either over-rejects (no read-then-mutate) or under-rejects (unsound).
- **The kill node is load-bearing**: stopping the reachability walk at a group
  member's single def is what lets per-iteration builders inside loops fire.
  Removing it silently degrades route A to top-level-only with no test
  failures — the DupStats builder test pins it.
- The pass runs per `OptFlags` change (the `Env.bytecode` cache is keyed on
  the flags), and `show-bytecode` mirrors runtime defaults via
  `Env::opt_flags_from_env` — keep them in sync or introspection lies.

**M4 route B gotchas (learned the hard way — the fuzzer found both).**
- **The mutation must be *on the spine*.** It is not enough that the container
  strips back to a loop-carried phi: an alias of an *outer* accumulator
  (`let al = xs; al = append(al, v)` in a nested loop, result discarded) strips
  to the outer loop phi but is a bystander — mutating it in place corrupts `xs`.
  `route_b_ok` requires the mutation's result to flow into the loop phi's back
  edge (seed 84619). Don't relax this.
- **The fresh root's only users must be the spine.** A pre-loop `let ys = xs`
  aliases the initial container and observes every in-place mutation; the
  "out-region reads are safe" rule is for reads of the *final* value only.
- **Build the spine web with a *backward* cone**, not a bidirectional flood:
  following the value forward past the loop merges the accumulator with whatever
  consumes its final value (`next` → the `particles` it feeds), yielding a
  two-root web that never fires. Confine the linearity web to the loop region.
- In-place stats **must not** record a `DupKind` (the verification asserts bytes
  *fall*); the `debug_assert!(alive)` in `Heap::*_in_place` is the id-reuse net.

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
  `vm_started`, and the M5a `vm_frame_pool` frame free-list). Any operation that
  replaces a program in place or resets a stack must account for them —
  `Env::insert_program` drops the cached lowering, and `Stack::reset_execution()`
  is the single reset point for per-run state (don't reset stack fields by hand;
  the hand-maintained lists drifted once already and broke hot reload under the
  VM).
- `Stack::vm_frame_pool` (M5a) is **not** a GC root, which is only sound
  because `VmFrame::recycle()` empties the register file, loop cursors, and
  loop context before a frame enters the pool. If pooling is ever added on a
  path that skips `recycle()`, or a pooled frame gains a field that can hold
  a `Value`, the GC either needs to scan the pool or the frame must be
  cleared — a stale id in a pooled frame is an invisible leak, and a *reused*
  one is corruption.

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
  backend (complete through M4); `escape.rs` is the route-B analysis (graph-
  side, feeds lowering), `lastuse.rs` the route-A pass (bytecode-side, runs
  after lowering in `Env::ensure_bytecode`).
- `rust/src/backend/bytecode/fuzz.rs` — the differential fuzzer (seeded
  generator + four-oracle exact-agreement runner; `PETAL_FUZZ_ITERS` scales
  the soak).
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
