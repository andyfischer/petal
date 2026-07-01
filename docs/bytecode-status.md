# Bytecode backend — status & handoff

Tracking doc for the linear **bytecode VM** that runs alongside the term-graph
step evaluator. Update the milestone checkboxes and the handoff note as work
lands. Companion reading: [Architecture.md](Architecture.md) (backend split),
[speculative-execution-plan.md](dev/speculative-execution-plan.md) (why the heap
is immutable-by-construction — the substrate for the M4 optimization),
[goals.md](goals.md) (performance is the standing weak spot this targets).

Last updated: 2026-07-01.

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
  graph/        # the step evaluator (moved verbatim from rust/src/eval/)
  bytecode/
    isa.rs      # Inst, Reg, Label, BytecodeFn, BytecodeProgram, LoopCursor  [DONE]
    lower.rs    # Program -> BytecodeProgram (per-function flat-register linearization)  [straight-line DONE]
    disasm.rs   # text + JSON rendering for show-bytecode / ShowBytecode  [DONE]
    vm.rs       # Vm, VmFrame, step(), do_call/do_return, sync intrinsics  [STUB]
    escape.rs   # uniqueness/escape analysis for in-place mutation  [STUB — returns empty set]
```

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

### How control flow will lower (M2 — not yet implemented)
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
Plus the `ShowBytecode` MCP tool (`ts/tools/petal-mcp.ts`). Straight-line
programs disassemble fully today; programs with control flow / calls / state
currently error with `unlowered op: <Op>` (honest until M1–M3 land).

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
- [ ] **M1 — Core VM.** `vm.rs` executing M0 ops + `Call`/`MethodCall`/
  `BuiltinCall`/`MakeClosure`/`MakeOverloadSet`/`Return`, CallFrame lifecycle,
  native dispatch. Factor pure per-op handlers out of `backend/graph/` into shared
  free functions (the parity lever). Differential-test on functional examples.
- [ ] **M2 — Control flow.** Phi→Move, phi_outs→Move at exit edges, Branch/And/Or,
  all loops, Break/Continue, Match. Extend `lower.rs`'s `lower_term` + add label
  resolution. Differential-test on control-flow examples.
- [ ] **M3 — State + parity + default flip.** State opcodes + per-frame `loop_idx`;
  `run_bounded` instruction budgeting; sync intrinsics (map/filter/reduce);
  GC-between-steps. Full `examples/` differential green with `OptFlags::none()`.
  Flip default backend to `Bytecode`; keep `Graph` as oracle.
- [ ] **M4 — In-place mutation.** `escape.rs` (conditions above), `Heap::*_in_place`,
  in-place opcodes, `fork_watermark` guard, behind `OptFlags.in_place_mutation`.
  Verify via triple differential + `DupStats` byte-drop assertions.
- [ ] **M5 (optional, profiling-gated).** Packed encoding, superinstructions,
  pattern-tree micro-ops — behind the same `Inst`/flag APIs.

---

## Handoff — next actions (M1)

**Goal:** make the VM run straight-line + calls, wired into `Env` behind
`Backend::Bytecode`, and stand up differential testing.

1. **Backend dispatch in `Env`.** Add a `backend: Backend` + `opt_flags: OptFlags`
   field (default `Graph` / `none()`), plumb `--backend` / `PETAL_BACKEND` and
   `--no-opt` / `PETAL_OPT` through `cli.rs`. `Env::step`/`run`/`run_bounded`
   dispatch on it. Both paths must return the *same* `StepResult`/`RunOutcome`, so
   the outer loops in `env/mod.rs` stay shared. Lower a program to bytecode lazily
   (cache the `BytecodeProgram` next to the `Program`).
2. **Shared handlers (the parity lever).** Factor the pure per-op bodies out of
   `backend/graph/{exec,ops,call}.rs` into free functions taking `&mut Heap` etc.
   (e.g. `ops::add(a, b, heap)`, `heapops::alloc_map(...)`). Have *both* the graph
   `Evaluator` and the bytecode `Vm` call them, so arithmetic/alloc/field/call
   logic cannot diverge. This is the single most important step for correctness.
3. **`Vm` + `VmFrame` execution loop** (`vm.rs`). Mirror `Evaluator<'a>`'s borrow
   bundle (`heap`, `closures`, `overload_sets`, `native_fns`, `output`, `symbols`,
   state maps). `step()` returns `StepResult`. Implement:
   - straight-line ops (delegate to the shared handlers),
   - `Call`/`MethodCall`/`BuiltinCall`: resolve callee → `ClosureId` (overload by
     arg count), push a `VmFrame` mirroring `build_closure_frame`
     (`backend/graph/call.rs`: args→`param_regs`, captures→`capture_regs`,
     self→`self_ref_reg`), advance caller ip at call-issue time,
   - `Return`: pop, write into caller's `dst_in_caller`,
   - native (non-intrinsic) calls run inline via the existing `PetalCxt` path.
4. **Resumability & GC.** A VM step = one instruction (or a budgeted N).
   `run_bounded`/`RunOutcome::Yielded` should Just Work since all resumption state
   is on the frame stack. VM register files are GC roots exactly as
   `Frame.registers` are — fire `heap.should_collect()` between steps.
5. **Sync higher-order intrinsics.** `map`/`filter`/`reduce`/`forEach` mirror
   `call_closure_sync` (`backend/graph/call.rs`): push closure frame, run `step()`
   until `frames.len()` drops back. Reuse the existing `builtin_map/filter/reduce`
   bodies unchanged.
6. **Differential tests** (`backend/bytecode/tests.rs`): for each snippet, run
   `Backend::Graph` and `Backend::Bytecode` (opts off) and assert equal returned
   `Value`, equal `output` buffer, equal final `stack.state`. Add a
   `--backend=bytecode` sweep to `ts/bin/test-examples.ts` (skip examples whose
   ops aren't lowered yet).

**Gotchas already discovered.**
- `let x = <expr>` names the result term directly — no trailing `Copy`/`Move`.
- Root-block registers start ~80 because builtin phantom terms reserve registers
  first. Flat registers are correct regardless; don't assume registers start at 0.
- `Match` pattern-binding registers must resolve through the same flat map; reuse
  the graph engine's `apply_pattern_bindings` name→register logic, precomputed at
  lower time.
- Petal `if` uses `then … end`, not braces (`if x > 0 then x = 2 end`).

---

## Key files
- `rust/src/program.rs` — lowering source (`TermOp`, `Term`, `Block.phi_outs`,
  `FunctionDef`) + analysis substrate (`trace_dependents`, `trace_provenance`).
- `rust/src/backend/graph/{mod,exec,ops,call,state,loops,pattern}.rs` — semantics
  the VM must replicate; source of the shared handlers.
- `rust/src/backend/bytecode/{isa,lower,vm,escape,disasm}.rs` — the new backend.
- `rust/src/heap.rs` + `rust/src/stats.rs` — COW mutators + free-list + `fork`
  (M4 in-place target + hazard surface); `DupStats` verification oracle.
- `rust/src/env/mod.rs` — `run`/`run_bounded`/`RunOutcome` (backend dispatch goes here).
- `rust/src/cli.rs`, `ts/tools/petal-mcp.ts` — `show-bytecode` / `ShowBytecode`.
- `ts/bin/test-examples.ts` — differential harness (extend for `--backend`).
