# Bytecode backend — status & handoff

Tracking doc for the linear **bytecode VM** that runs alongside the term-graph
step evaluator. Update the milestone checkboxes and the handoff note as work
lands. Companion reading: [Architecture.md](Architecture.md) (backend split),
[speculative-execution-plan.md](dev/speculative-execution-plan.md) (why the heap
is immutable-by-construction — the substrate for the M4 optimization),
[goals.md](goals.md) (performance is the standing weak spot this targets).

Last updated: 2026-07-01. **Status: M1, M2 complete; M3 substantially complete
(state + full graph parity proven; default-flip + example-sweep pending); M4 not
started.** The bytecode VM runs the entire language — straight-line, calls,
closures, all control flow, match, and persistent state — and matches the graph
engine on value, print output, final state, and error text across the whole
`examples/` corpus and the vitest suite (both run under `PETAL_BACKEND=bytecode`).

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

### Escape / uniqueness analysis (M4)
Runs over the term graph using `Program::trace_dependents` (reverse dataflow) and
the phi-source set from `trace_provenance`. A mutation term `T` on container
input `C` lowers to an **in-place** opcode (`SetIndexInPlace`/`SetFieldInPlace`,
new `Heap::*_in_place` methods that mutate + reuse the id) iff **all** hold:
1. **Single static consumer:** `users(C) == {T}`.
2. **Last use:** implied by (1).
3. **Not a `phi_outs` src / loop-carry alias.**
4. **Does not escape:** `C` never feeds a `StateInit/Read/Write`, a
   `MakeClosure`/`MakeOverloadSet` capture, a `Return`, another escaping
   container, and never crosses a speculative fork boundary.
5. **Fresh/unique producer:** `C` is an `Alloc*` in this function or an
   in-place-eligible mutation chain; params/captures/state-reads are conservatively
   *not* unique.

Uncertain ⇒ fall back to clone-and-alloc. Gated behind `OptFlags.in_place_mutation`.
**Why sound:** the heap is immutable-by-construction, so a dataflow edge to `C`'s
producing term is the *only* way any code observes it — (1),(3),(4) completely
enumerate observers, a purely static graph property (same argument the codebase
uses for `fork` safety). **Verify** via triple differential (graph / BC-noopt /
BC-opt) + assert `DupStats::total_bytes()` strictly drops.
**Hazards:** heap free-list id reuse (in-place only fires while `C` is a live
root; add `debug_assert!(alive)`); speculative fork sharing (add a per-heap
`fork_watermark`; `*_in_place` refuses ids below it); state/closure-captured ids
(forbidden by condition 4).

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
- [~] **M3 — State + parity + default flip.** *Done:* state opcodes + per-frame
  `loop_idx`; `run_bounded` resumability (test); GC-between-steps (shared); sync
  intrinsics; shared error annotation (`backend/errors.rs`) → full value/output/
  state/**error** parity; entire vitest suite + all 22 examples green under
  `PETAL_BACKEND=bytecode`. *Remaining:* (1) add a `--backend=bytecode` sweep to
  `ts/bin/test-examples.ts`; (2) flip the default `Backend` to `Bytecode` (keep
  `Graph` as oracle) — proven safe by full-suite parity, deferred only so the
  broad-impact default change gets its own focused re-validation (apps, WASM).
- [ ] **M4 — In-place mutation.** `escape.rs` (conditions above), `Heap::*_in_place`,
  in-place opcodes, `fork_watermark` guard, behind `OptFlags.in_place_mutation`.
  Verify via triple differential + `DupStats` byte-drop assertions.
- [ ] **M5 (optional, profiling-gated).** Packed encoding, superinstructions,
  pattern-tree micro-ops — behind the same `Inst`/flag APIs.

---

## Handoff — next actions

M1, M2, and the substance of M3 are done and committed (one commit per
chunk: M1a/M1b/M1c, M2a/M2b/M2c, M3-state+annotation). The VM is at full
behavioral parity with the graph engine. Two small M3 tails, then M4.

### Finish M3
1. **`--backend` sweep in `ts/bin/test-examples.ts`.** Run each `examples/*.ptl`
   under both backends and diff stdout; fail on any divergence. (The Rust
   `backend::bytecode::tests` already do this at the `Value`/output/state level;
   this adds an end-to-end CLI sweep. Manual equivalent that currently passes:
   loop over the examples running `petal run --backend=graph` vs
   `--backend=bytecode` and compare — see the sweep used while building M2c/M3.)
2. **Flip the default backend.** Change `Backend::default()` (`backend/mod.rs`)
   from `Graph` to `Bytecode`. This is proven safe (the full vitest suite and all
   22 examples already pass under `PETAL_BACKEND=bytecode` with only the 3
   pre-existing, backend-independent failures — `stdlib-extract`, `canvas-offscreen`).
   Deferred only because it changes the default for every embedder (CLI, apps,
   WASM), so re-run those app/integration paths once after flipping. Keep `Graph`
   reachable via `--backend=graph` / `PETAL_BACKEND=graph` as the oracle.

### Then M4 — in-place mutation (the payoff)
`escape.rs` is still a stub returning an empty set. Implement the uniqueness/
escape analysis (conditions enumerated in the **Escape / uniqueness analysis**
section above), add `Heap::*_in_place` methods, emit `SetIndexInPlace`/
`SetFieldInPlace` (already in the ISA) when the analysis proves safety, gate on
`OptFlags.in_place_mutation`, and add the `fork_watermark` guard. **Verify** via
triple differential (graph / BC-noopt / BC-opt agree) plus assert
`DupStats::total_bytes()` strictly drops on a mutation-heavy sketch. The
`OptFlags` plumbing, `--no-opt` flag, and both correctness oracles are already in
place.

**Gotchas already discovered (still true).**
- `let x = <expr>` names the result term directly — no trailing `Copy`/`Move`.
- Root-block registers start ~80 because builtin phantom terms reserve registers
  first. Flat registers are correct regardless; don't assume registers start at 0.
- Petal `if` uses `then … end`, not braces (`if x > 0 then x = 2 end`).
- break/continue must run enclosing `phi_outs` (see the M2 notes above) — the
  single subtlest correctness point in the whole lowering.
- The graph **re-annotates** already-annotated errors that propagate back up
  through a synchronous intrinsic call (`call_closure_sync`); the VM matches this
  by annotating at every `step()` error. Don't "fix" the apparent double-annotation.
- Loop-index keys use the 0-based *iteration count*, not the loop value (matters
  for `range(start, end)` with `start != 0` and for state-map key parity).

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
  backend (complete through M3); `escape.rs` is the M4 stub.
- `rust/src/heap.rs` + `rust/src/stats.rs` — COW mutators + free-list + `fork`
  (M4 in-place target + hazard surface); `DupStats` verification oracle.
- `rust/src/env/mod.rs` — `run`/`run_bounded`/`RunOutcome` (backend dispatch goes here).
- `rust/src/cli.rs`, `ts/tools/petal-mcp.ts` — `show-bytecode` / `ShowBytecode`.
- `ts/bin/test-examples.ts` — differential harness (extend for `--backend`).
