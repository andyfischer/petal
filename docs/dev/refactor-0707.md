# Refactor handoff — 2026-07-07

A multi-cluster survey of `rust/src/` for code-reorganization opportunities
(overloaded files to split, duplicate logic to dedup, layering to separate).
This doc records what shipped and — more importantly — the prioritized backlog
of what was found but deliberately deferred.

Baseline for all work here: `cargo test` (246) + `cd ts && npx vitest run` (482)
green, no compiler warnings.

## Shipped (commit `cbb69ec`)

Six low-risk, individually test-verified changes; net −627 lines in modified
files plus two new modules split out of `program.rs`.

- **`base_fn_name(&str)`** in `program.rs` — one helper for stripping the
  internal `#arity` overload suffix, replacing three copies in
  `backend/calls.rs` (×2) and `backend/bytecode/vm.rs` that had *disagreed*
  (`split('#')` vs `rfind('#')`).
- **`is_mutating_builtin(&str)`** promoted to `builtins/mod.rs` as the single
  source of truth; `backend/bytecode/escape.rs` and `::lastuse.rs` reference it
  instead of each carrying the 7-name list behind a "must stay in sync" comment.
- **Math unary collapse** (`builtins/math.rs`): 9 dual-number functions
  (`abs/sqrt/floor/ceil/round/sin/cos/tan/float`) reduced to two helpers,
  `unary_float_dual` and `unary_num_preserving`. Behavior preserved exactly,
  including the `abs` derivative pinned to 0 at x=0.
- **`checked_f64_index`** (`builtins/collections.rs`) replaces 4 identical
  bounds checks; **`hue_sector`** (`builtins/color.rs`) shares the 12-line
  hue-sector block between HSV and HSL.
- **`die`/`die_plain`** in `cli.rs` collapse ~9 copies of the
  `error → json / eprintln → process::exit(1)` idiom. (`ShowBytecode` keeps its
  distinct "Error lowering to bytecode" message and was left alone.)
- **`compare_values`** moved from `builtins/mod.rs` to `value.rs` beside
  `values_equal` — value-ordering now lives in the value layer.
- **`program.rs` split** (892 → 363 lines): type definitions stay put; the
  import validator (`from_json`/`rebuild_indexes`/`validate`) moved to new
  **`ir_validate.rs`**; the read-only graph analysis
  (`find_term`/`named_terms`/`trace_provenance`/`trace_dependents`/`slice` and
  their tests) moved to new **`program_analysis.rs`**. Both are additional
  `impl Program` blocks — no call sites changed.

## Shipped (branch `refactor/backlog-0707`, commits `028dbba`..`0cd521a`)

All five backlog items below are complete, merged to `main` via fast-forward.
Baseline held throughout: `cargo test` (392, up from 246) + `vitest` (482)
green, zero warnings, plus a 20k-iteration differential fuzzer pass on every
VM/GC-touching item and a clean wasm build.

- **#1 AST walkers**: `ExprVisitor`/`ExprVisitorMut` traits + `walk_*` in
  `ast.rs`; all ~7 hand-rolled walkers ported (desugar's `lift_expr`/
  `count_at`/`replace_one_at`, phi's assigned-names collection, lint's
  `rebind`/`for_each_expr`), each keeping its own boundary policy via
  overrides. Added phi characterization tests (previously untested).
- **#2 `Inst` metadata**: `isa.rs` now owns `for_each_write`,
  `falls_through`/`branch_target(_mut)`; `push_succs`/`patch` became thin
  wrappers. Scoped down from the original proposal — `for_each_read`/
  `input_regs` were left alone since folding them in would invert layering
  around the soundness-critical per-role-retaining/builtin-classification
  logic. Net win is `patch` and `push_succs` can no longer drift apart.
- **#3 God-file splits**: `lint.rs`, `cst.rs`, `cli.rs`, `env/mod.rs`,
  `bytecode/vm.rs` → directory modules; `make_vm` + `Stack::gc_roots`
  extracted; CLI provenance/dependents/slice handlers deduped. CLI output
  verified byte-identical; VM split additionally fuzzed 20k iterations.
- **#4 `heap.rs` slab**: 5 parallel stores collapsed into one generic
  `Slab<T>` (`alloc`/`mark`/`sweep_with`), 965 → 870 lines. Kept bare-index
  ids, interning, per-store COW, and the element-fans-into-3-slabs shape.
  Gated on the 20k fuzzer + `gc.test` + dup-stats, both feature configs.
- **#5 Small dedups**: `TermOp::constant_ids()` (+ deleted dead `op_name`);
  `Program::state_terms()`; stdout echo/RNG/noise seed moved off process
  globals into `ExecutionContext` with fork isolation. Two intentional
  behavior changes ship here, both covered by new tests: speculative forks no
  longer leak `print` to stdout (gated behind an echo flag, on by default,
  since it's the sole stdout path for `petal run`), and forks get isolated
  RNG/noise streams instead of sharing process-global state.

## Backlog history (pre-refactor, now resolved above)

Ranked by leverage. Each was a good standalone session with its own verify pass.

### 1. Unify the hand-rolled AST walkers  (HIGH value, MEDIUM risk)
The same exhaustive `ExprKind`/`StmtKind` traversal is re-implemented ~7 times;
adding an AST variant means editing all of them, enforced only by the compiler
for the non-`_`-defaulted ones:
- `desugar.rs`: `lift_expr`, `count_at`, `replace_one_at`, and the test-only
  `count_atvars_in_stmt`.
- `compiler/phi.rs`: `collect_assigned_names_stmts` / `collect_assigned_names_expr`.
- `lint.rs`: `rebind_expr`/`rebind_stmts` vs `for_each_expr`/`for_each_expr_in_stmt`.

Proposed: a shared `walk_expr(&Expr, &mut impl FnMut(&Expr))` (+ `_mut`) or an
`ExprVisitor` trait in `ast.rs`. **Risk:** the walkers have deliberately
*different* stop-at-boundary policies (rebind stops at match arms / while
conditions; `for_each_expr` descends everywhere) — the visitor must let each
caller keep its policy. Do this first as it de-risks future AST changes.

### 2. Centralize `Inst` opcode metadata  (HIGH value, MEDIUM risk)
Every `Inst` variant is hand-enumerated in 8 places; 4 are near-supersets:
- `backend/bytecode/isa.rs`: `Inst::dst()`, `Inst::input_regs()`
- `backend/bytecode/vm.rs`: `exec_inst`
- `backend/bytecode/disasm.rs`: `render_inst` (legitimately per-variant — leave)
- `backend/bytecode/lastuse.rs`: `for_each_write`, `for_each_read`, `push_succs`
- `backend/bytecode/lower.rs`: `patch`

Proposed: make `isa.rs` the source of truth with `Inst::writes`/`reads`/
`successors`; re-express `dst`/`input_regs` and the three `lastuse` enumerations
as thin wrappers (deletes ~250 lines from `lastuse.rs`). **Risk:** touches the
correctness-critical last-use / escape in-place analyses — verify with the
bytecode `tests.rs` + `fuzz.rs` and the `gc`/`loop-state` vitest suites.

### 3. Split the remaining god-files  (MEDIUM value, LOW risk — mechanical)
All are `impl`-block moves in the `program.rs` mold:
- **`vm.rs` (1178)** → `vm/{frame,dispatch,calls,native,intrinsics}.rs`, keeping
  `step`/`run_batch`/register access in `vm/mod.rs`.
- **`env/mod.rs` (1128)** → `env/{run,gc,fork,state_json,host_io}.rs`. Note the
  identical 17-field `Vm { … }` literal in `step_bytecode` and `call_function` —
  extract `fn make_vm(...)` while splitting. `collect_garbage` reaches into
  `VmFrame`/`LoopCursor` internals; give `stack.rs` a `gc_roots(&mut dyn FnMut)`
  method to isolate that coupling.
- **`cli.rs` (1038)** → `cli/args.rs` (the `parse_*` fns) + per-command handlers;
  `execute()` is a 418-line god-match. Provenance/dependents/slice handlers are
  near-verbatim (dedup `edges_to_json`, `print_term_rows`, `resolve_terms`).
- **`lint.rs` (1027)** → `lint/{reindent,rebind}.rs`; `reindent()` is 193 lines.
- **`cst.rs` (1032)** → `cst/{green,red,events}.rs`. `parse_source` orchestrates
  lexer+parser+projector and belongs in a driver module, not `cst`.

### 4. `heap.rs` generic slab  (HIGH value, HIGHER risk)
`heap.rs` (965) hand-rolls 5 parallel stores (String/List/F64Array/Map/Element)
with copy-pasted `alloc_*`/`sweep`/`mark_*`/COW boilerplate (`sweep` is one loop
×5). Proposed: a generic `Slab<T>` + `Markable` trait, collapsing ~300 lines to
~120. **Risk:** GC correctness — gate behind `gc.test.ts` and the dup-stats
assertions. Best done after #3's `env/gc.rs` split isolates the collector.

### 5. Smaller shared-helper dedups (LOW, opportunistic)
- `TermOp` variant enumeration duplicated in `program.rs::validate` (now
  `ir_validate.rs`), `ir_display::format_op`, `lower.rs::op_name` — candidate for
  `TermOp::constant_ids()` / `impl Display`. `op_name`'s error fallback may now be
  dead (every op is lowered) — verify then delete.
- `env` / `transfer_state.rs` both scan terms for state keys — a
  `Program::state_keys()` accessor would centralize it.
- `native_fn.rs::PetalCxt::print` unconditionally `println!`s in addition to the
  output buffer — a layering wart for embedders/wasm/forks; drop the `println!`
  or gate it behind an explicit echo flag.
- RNG (`builtins/mod.rs`) and noise seed (`builtins/noise.rs`) are process-global
  statics, so two `Env`s (and speculative forks) share randomness state. Move
  into `ExecutionContext` if fork isolation of RNG matters.

## Do NOT touch
The `parse.rs` ↔ `cst_project.rs` dual parser is **intentional** migration
scaffolding — the planned "step 3d" makes `parse_cst` the sole parser and derives
the AST from the tree. The two AST-construction paths are kept bit-identical on
purpose (differential corpus test). Leave it until 3d lands.

## How to verify any of the above
```bash
cd rust && cargo build && cargo test
cd ts && npx vitest run          # 482 integration tests, shells out to the CLI
```
For backend/VM work also lean on `backend/bytecode/{tests,fuzz}.rs` and the
`gc` / `loop-state` / `autodiff` vitest files. Spot-check moved code paths
through the CLI (`show-slice`, `show-provenance`, `run --ir <json>`), not just
the test suite.
