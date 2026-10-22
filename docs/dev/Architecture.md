# Petal Architecture

This document describes the implementation of the Petal compiler and runtime.
It's the internal counterpart to the [Language Guide](../language-guide.md) — read
this if you're working on the compiler or debugging IR behavior.

```
Source Code → Lexer → Parser → AST → Compiler → IR (Term Graph) → Bytecode VM
```

All of the above lives in `core/src/` as a single crate. The binary entry
point is `core/src/main.rs`, which delegates to the CLI dispatcher in
`cli/`. The libraries built on the crate are outside it: `core-libs/petal-ui`
(the UI host library and the headless harness), `core-libs/petal-query`,
`core-runtime/` (Petal-source packages such as `bloom`), and the hosts under
`integrations/` (SDL, web, the C bridge).

The term graph is the canonical, introspectable IR — provenance, slicing,
autodiff, `explain`, hot-reload, and the IR-as-target contract all reason about
it. It is executed by the **bytecode VM** (`core/src/backend/bytecode/`), a
register machine that runs a linear *lowering* of the graph, with
escape-analysis-driven in-place mutation gated by `backend::OptFlags`. The VM
populates the trace buffer that the graph-level introspection reads. See
[performance.md](performance.md) for what the optimizer does.

---

## Source File Map

The main files and directories under `core/src/`, grouped by pipeline stage.

**Front end**

| File | Purpose |
|------|---------|
| `main.rs` / `lib.rs` | Binary entry point (delegates to `cli::run`) / crate root |
| `cli/` | `args.rs` (arg parsing), `handlers.rs` (command handlers), `help.rs` (`petal help`), `bench.rs` (`petal bench`), `pm.rs` (the package commands), `source_tools.rs` (`fmt`, `lint`, `apply-change`), `mod.rs` (dispatch) |
| `lexer.rs` | Source text → token stream |
| `parse.rs`, `ast.rs`, `ast_display.rs` | Tokens → AST; AST node types; AST pretty-printer |
| `cst/`, `cst_project.rs`, `trivia.rs` | Lossless concrete syntax tree for source-preserving edits, and its projection to the typed AST. `parse.rs` and the CST parser are kept in lockstep; unifying them is future work |
| `desugar.rs`, `export_keyword.rs` | `@`-argument (in-out call) desugaring; `export` as the deprecated spelling of `pub` |
| `module.rs`, `package.rs`, `pkgman.rs`, `bundled.rs` | Module resolution and the import walk (merges imports into one `Program`); `petal.toml` packages; the package manager (git dependencies, `petal.lock`, `vendor/`); the packages compiled into the binary (`core/packages/`) |
| `native_decl.rs` | `native fn` interface declarations: what a package says about a host native |
| `compiler/` | AST → IR term graph: `expr.rs`, `stmt.rs`, `function.rs`, `phi.rs` (phi insertion), `state_ids.rs` (state keys and callsite ids), `enums.rs`, `match_check.rs`, `native.rs`, `capture_lag.rs` |
| `typecheck/` | The warning-only static type checker |
| `check.rs` | `Env::check_program`: "will this program run?", answered without running it (`petal check`) |
| `diagnostic.rs`, `error.rs` | Diagnostics and error types |

**IR**

| File | Purpose |
|------|---------|
| `program.rs` | `Program`, `Block`, `Term`, `TermOp` definitions |
| `constant_table.rs` | Deduplicated literal storage |
| `source_map.rs` | `TermId` → source span mapping |
| `ir_display.rs`, `ir_serialize.rs`, `ir_validate.rs` | Text pretty-printer; serde helpers for IR JSON; structural validation when loading JSON IR |
| `ir_equiv.rs` | "Are these two programs the same?" ignoring positions (`petal ir-equal`) |
| `program_analysis.rs`, `provenance.rs`, `dot_graph.rs` | Dataflow slicing, provenance walks, DOT rendering |
| `named_calls.rs` | Static resolution of a call's callee and of the parameter each argument binds |
| `static_value.rs`, `static_json.rs`, `types.rs`, `classes.rs`, `enum_table.rs` | Compile-time values and their JSON form, type representation, the `ClassTable` of `class` declarations (including the built-in `Rect`), and the numbering of enum variants |
| `static_classify/` | Which top-level declarations are a function of the source text alone (`petal show-static`); see [Static values](#static-values) |

**Source editing and tooling**

| File | Purpose |
|------|---------|
| `rewrite.rs`, `literal_edit.rs`, `goal_based_editing.rs`, `direct_manipulation.rs` | Formatting-preserving source rewriting, style-preserving edits of literal values, goal-based edits, and emit-trace provenance |
| `source_diff.rs`, `source_watch.rs` | AST-based diff of two versions of a file (what kind of change an edit is; drives hot reload); polling change detection for a program's source files |
| `fmt/` | `petal fmt`, the canonical layout: `reindent.rs`, `spacing.rs`, `comments.rs` |
| `lint/` | `petal lint`, one rule per file: `var_to_let.rs`, `casts.rs`, `to_match.rs`, `compound.rs`, `repeat.rs`, `state_const.rs`, `export_to_pub.rs` |
| `suggest/`, `apply_change/`, `fix_imports.rs` | `petal suggest` (safe refactors, offered not applied), `petal apply-change` (refactors that change behaviour), `petal fix-imports` |
| `fn_introspect.rs` | What `fn_info(f)` and `fn_ast(f)` read |
| `lsp/` | `petal lsp`, the language server |
| `inspect.rs`, `observe.rs`, `profile.rs`, `stats.rs` | Introspection helpers, the observation buffer, the `--profile` counters and the per-call `CallBench` behind `petal bench`, `--dup-stats` |
| `effect_audit.rs`, `static_audit.rs`, `static_survey.rs` | Opt-in runtime audits of native effect declarations and of the static classifier; a measurement of constant work inside function bodies |
| `test_corpus.rs` | Enumerates the repo's `.ptl` files for corpus-wide unit tests |

**Runtime**

| File | Purpose |
|------|---------|
| `backend/mod.rs` | `OptFlags` optimization toggles plus shared runtime helpers (`calls.rs`, `ops.rs`, `pattern.rs`, `errors.rs`) |
| `backend/bytecode/` | Register VM: `isa` (instructions), `lower` (graph → bytecode), `copyprop`, `escape`, `lastuse`, `cells` (optimizer passes), `disasm` (`show-bytecode`), the differential fuzzers (`fuzz`, `cell_fuzz`, `static_fuzz`, `static_fn_fuzz`, `enum_all_fuzz`), `vm/` (`dispatch.rs`, `fast.rs`, `calls.rs`, `frame.rs`, `intrinsics.rs`, `introspect.rs`, `memo.rs`, `native.rs`) |
| `policy.rs` | `RunPolicy`: the optimizer, memo and frame-gate switches as one named value (`fast`, `baseline`, `explain`, `replay`) |
| `static_cache.rs`, `static_cache/in_fn.rs` | The static-value cache: constant top-level values, and constant values inside function bodies, built once and kept between runs |
| `memo.rs`, `run_deps.rs` | Memoized scopes ([memo-scopes.md](memo-scopes.md)); what a run depended on, for the frame gate ([frame-gate.md](frame-gate.md)) |
| `stack.rs` | `Stack`: VM frames plus persistent state |
| `value.rs` | The `Value` enum |
| `numeric.rs`, `proofs/` | Pure numeric semantics (checked arithmetic, the Int/Float ordering, the `sort` order, state-key hashing) and their Kani proofs ([formal-verification.md](formal-verification.md)) |
| `heap.rs`, `record.rs`, `dict.rs`, `fxhash.rs` | Mark-and-sweep GC for strings, lists, f64 arrays, records, `map`s (`Value::Dict`, keyed by content through `dict.rs`), elements, `Vec3`s and cells. A map slot also carries an optional class tag, which is what makes a record an instance. Records are stored as a shared key list (a `Shape`) plus one value per key |
| `closure_table.rs` | Runtime closures and overload sets; swept jointly with the heap |
| `execution_context.rs` | One isolated execution's mutable runtime bundle (heap plus closure/output/host registries) |
| `env/` | `Env`, which owns programs, stacks, and contexts: `run.rs`, `reload.rs` (hot reload), `fork.rs`, `gc.rs`, `host_io.rs`, `state_json.rs`, `state_storage.rs`, `statics.rs` and `static_follow.rs` (the static-value cache across runs and reloads), `observations_json.rs` |
| `native_fn.rs`, `builtins/` | Native function FFI (`NativeFnTable`, `PetalCxt`) and the built-in functions, one module per topic |
| `handle.rs`, `symbol.rs`, `resource_table.rs` | Opaque host-object handles; interned symbols shared with the host; pending (unresolved) resources |
| `trace.rs` | Ring-buffered per-term execution trace |
| `transfer_state.rs` | Move a stack's state onto a different program, reconciling by `StateKey` (hot reload) |
| `extract.rs` | Typed accessors for pulling Rust data out of a `Value` |
| `wasm.rs` | `wasm-bindgen` bindings used by the browser integrations |

The crate is about 190 source files and 130k lines, tests included.

---

## The Term Graph IR

Petal's IR is a **term graph** — a DAG of `Term` nodes connected by explicit
dataflow edges. Each term represents one operation (load a constant, add,
call a function, branch, read state, …) and references its inputs by
`TermId`.

### Program

`Program` owns everything for one compiled source file:

```rust
pub struct Program {
    pub schema: String,              // IR schema version
    pub id: ProgramId,
    pub source: String,              // optional for imported IR
    pub terms: Vec<Term>,            // indexed by TermId
    pub blocks: Vec<Block>,          // indexed by BlockId
    pub root_block: BlockId,         // entry point
    pub constants: ConstantTable,    // deduplicated literals
    pub source_map: SourceMap,       // term → source span
    pub has_errors: bool,            // true if any Error terms
    pub functions: Vec<FunctionDef>, // function definitions
    pub match_arms: HashMap<TermId, Vec<MatchArmMeta>>,
    pub class_names: BTreeSet<String>, // declared classes (runtime dispatch gate)
    pub enums: BTreeMap<String, EnumDef>,
    pub native_links: Vec<NativeLink>, // `native fn` declarations to link at load
    pub requires: Vec<RequiredInterface>, // host interfaces the program needs
    pub imports: ImportTable,
    // ... plus compile-time artifacts that are not serialized (warnings, indexes)
}
```

`Program` is what `show-ir --json` prints, and what `run --ir` loads. See
[CLI.md](../CLI.md#program-json-schema) for the full JSON schema.

### Term

```rust
pub struct Term {
    pub id: TermId,
    pub op: TermOp,
    pub inputs: SmallVec<[TermId; 4]>, // dataflow edges
    pub block_id: BlockId,
    pub block_next: Option<TermId>,    // linked list within block
    pub block_prev: Option<TermId>,
    pub name: Option<String>,          // binding name (let x = ...)
    pub register: RegisterIndex,       // stack slot for result
    pub state_key: Option<StateKey>,   // for StateInit/Read/Write
    pub child_blocks: SmallVec<[BlockId; 2]>,
    pub path_pop: u32,                 // StateRead/Write: loop steps to drop
    pub call_site: Option<u64>,        // Call/MethodCall/BuiltinCall: callsite id
    pub arg_names: SmallVec<[Option<ConstantId>; 4]>, // named arguments of a call
    pub collect: bool,                 // value-position loop collects its results
    pub is_config: bool,               // declared `config let`
    pub is_const: bool,                // declared `const`
}
```

Terms participate in **two graphs** at once:

1. **Dataflow** — via `inputs`. A term's inputs are the terms whose values
   it consumes. This graph is a DAG.
2. **Block ordering** — via `block_next`/`block_prev`. Each block holds a
   linked list that defines execution order within that scope. (In-memory
   only: the serialized IR carries an ordered `terms` array per block, and
   the loader rebuilds these links from it.)

Lowering walks each block's list in order and emits bytecode for it; an
instruction reads its dataflow inputs from the registers of the terms that
produced them (they have already run).

`SmallVec` avoids heap allocation for the common case (most terms have 0–3
inputs, most have 0–2 child blocks).

### Block

```rust
pub struct Block {
    pub id: BlockId,
    pub parent_term_id: Option<TermId>, // null for root & function bodies
    pub entry: Option<TermId>,          // first term in the linked list
    pub terms: Vec<TermId>,             // execution order (the wire form)
    pub param_names: Vec<String>,       // for fn bodies & for-loop vars
    pub register_count: u16,            // frame size
    pub phi_outs: Vec<PhiOut>,          // rebinding carry-outs
}
```

Blocks form a tree rooted at `Program.root_block`. Child blocks represent
scopes introduced by `if`/`else`, `for`, `while`, `match`, and short-circuit
`&&`/`||`. Function bodies are also blocks but have `parent_term_id: None` —
they're connected via `FunctionDef.body_block` and the `MakeClosure` term
that references the function.

### TermOp

The operation a term performs. All variants and their IR serialization are
documented in the TermOp table of
[CLI.md's Program JSON Schema](../CLI.md#program-json-schema);
the important groups are:

- **Loads** — `Constant`, `Error`, `Copy`
- **Arithmetic / comparison / logical** — `Add`, `Sub`, `Neg`, `Eq`, `Not`, `And`, `Or`, `Coalesce`, `Concat`, …
- **Control flow** — `Branch`, `ForLoop`, `NumericForLoop`, `WhileLoop`, `Break`, `Continue`, `Return`
- **Data joins** — `Phi` (see below)
- **State** — `StateInit`, `StateRead`, `StateWrite`
- **Cells** — `CellNew`, `CellRead`, `CellWrite` (the `var` escape hatch)
- **Functions** — `MakeClosure`, `MakeOverloadSet`, `Call`, `MethodCall`, `BuiltinCall`
- **Data** — `AllocList`, `AllocMap`, `AllocMapSpread`, `AllocElement`, `MakeEnumVariant`
- **Access** — `GetField`, `GetFieldOpt`, `SetField`, `GetIndex`, `GetIndexOpt`, `SetIndex`
- **Pattern matching** — `Match`

### Phi terms and the "no mutation" promise

Petal's design philosophy is that the IR has **no register-mutation
primitive** — every value is computed once and never changes. Rebinding
`x = 2` at the top level is fine: it creates a new term and moves the
`"x"` label.

Rebinding inside a child block (`if`, loop body, `match` arm) needs a
**phi join**: a `Phi` term in the parent block, placed *before* the
control-flow term. The phi initializes from its `inputs[0]` (the
pre-control-flow value) and gets updated by each child-frame pop via
`Block.phi_outs`. Branches that don't rebind leave the init value in
place; loop iterations read the latest value.

The one deliberate exception is a **`var`** binding, which binds a heap
*cell* rather than a value. Its reads lower to `CellRead` and its `set`
writes to `CellWrite`, so the name is never rebound and never needs a phi —
which is exactly what lets a `var` be written from inside a conditional or a
closure, where a phi would have to initialize from a term in another
function. The cost is that a cell read has no dataflow edge to walk back,
so provenance stops there. See [var.md](../syntax/var.md) for the
full argument, including the containment invariant that keeps cells out of the
value domain (no expression evaluates to a `Value::Cell`).

### ConstantTable

Literals (ints, floats, strings, bools, nil) are stored once per program
in `ConstantTable` and referenced by `ConstantId`. The table deduplicates:
two `"hello"` literals share the same entry.

### SourceMap

Maps each `TermId` to a `SourceSpan` (`{line, column, offset}` for start
and end, plus a file index for multi-file programs; serialized as a compact
array — see [Dump format conventions](../CLI.md#dump-format-conventions)).
This powers error messages, `explain`, `graph`, and
the trace buffer's `line`/`column` fields.

### Functions and Closures

`FunctionDef` holds compile-time metadata; `MakeClosure` creates a
runtime closure value with the captured values baked in.

```rust
pub struct FunctionDef {
    pub id: FunctionId,
    pub name: Option<String>,
    pub params: Vec<String>,
    pub optional_params: u16,        // trailing params that have a default
    pub body_block: BlockId,
    pub capture_names: Vec<String>,
    pub capture_registers: Vec<RegisterIndex>,
    pub self_ref_register: Option<RegisterIndex>, // for recursion
    pub register_count: u16,
    pub span: Option<SourceSpan>,    // where the function is written
}
```

Overloading (see [function-overloading.md](../syntax/function-overloading.md)) is
compiled as one `MakeClosure` per variant plus one `MakeOverloadSet` that
bundles them. Dispatch at runtime selects the variant by argument count and
argument names (`backend/calls.rs`); a parameter with a default owns a hidden
was-it-passed flag that the body tests before evaluating the default.

---

## Runtime

### Value

`Value` is a `Copy` 24-byte enum. Heap-allocated values (strings, lists,
f64 arrays, records, `map`s, elements, `Vec3`s, cells) are stored by ID into the `Heap` —
`Value` just carries the ID. `Vec2` and `Dual` are stored inline (unboxed), no
heap allocation; a `Vec3` is a heap id because three `f64`s would widen every
`Value` (see [vec3.md](vec3.md)). A static assertion in `value.rs` keeps it at
24 bytes.

Every heap id (and `ClosureId`/`OverloadSetId`) is a `(slot index,
generation)` pair naming one *allocation*, not one slot. Reusing a reclaimed
slot bumps its generation, so an id that outlives its object never compares
equal to the slot's new occupant, and `Heap::is_live(value)` (or
`ExecutionContext::is_live`, which also covers closures) can tell it is stale.
The rule for anything that keeps a `Value` past the instruction that produced
it: **an id held across a collection must either be a GC root (strong: listed
in `Env::collect_garbage`) or be checked with `is_live` before every
dereference (weak).** The execution trace buffer is weak (it renders a
collected value as `<collected>`); stacks, host bindings, output buffers, the
resource table and observations are strong. Dereferencing a stale id is caught
only by a `debug_assert!`. `restore_execution` carries the replaced context's
generation history into the restored heap and closure table, so ids a host read
before a restore are never reissued after it.

```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    F64Array(F64ArrayId),
    Map(MapId),          // a record: `{a: 1}`
    Dict(DictId),        // a `map`: `Map()`, keyed by content (dict.rs)
    Closure(ClosureId),
    OverloadSet(OverloadSetId),
    NativeFunction(NativeFnId),
    EnumVariant { tag: StringId, data: ListId },
    Element(ElementId),
    Cell(CellId),
    Dual { value: f64, derivative: f64 },
    Vec2(f64, f64),
    Vec3(Vec3Id),
    Symbol(SymbolId),
    Handle(HandleVal),
    Pending(PendingId),
}
```

Because `Value` is `Copy`, there are no `Rc<RefCell<...>>` dances — the
heap handles aliasing, and the GC handles reclamation.

A `Handle` is an opaque reference to an object the host owns: a (class, slot,
serial) triple, where the class indexes the `HandleClass` registry on `Env`
(`Env::register_handle_class`, `Env::make_handle`) and gives the handle its
methods. See [Handles](../usage/ffi.md#handles). A `Pending` is an unresolved
resource: ordinary operations absorb it and return it, and a small set of
builtins inspect it. See [pending-values-plan.md](pending-values-plan.md).

### Heap & GC

`Heap` is a mark-and-sweep garbage collector. A collection is triggered by
*work owed* rather than object count: each allocation charges its payload size
plus a per-slot term, and a sweep runs once that reaches a budget the previous
sweep sized from the live set (`should_collect`). Live values are found by
walking all live stacks' registers plus the other roots — closure captures,
host bindings, output buffers, the resource table, observations. Reclaimed
slots go onto a free list, and reuse advances the slot's generation (see
[Value](#value)).

Closures and overload sets live *beside* the heap, in the context's
`ClosureTable` (`closure_table.rs`), because the VM borrows the two disjointly
— but they are collected with it. A `Value::Closure` inside a heap object can
only be recorded, not followed, while the heap is being marked, so `mark_value`
pushes it onto a *gray set*; `Env::collect_garbage` then alternates — drain the
gray set, mark those table entries, feed their captures back through the heap —
until a round turns up nothing new, and sweeps both stores. Until that table
existed the two were append-only `Vec`s: a host that re-runs a program per
frame (a Garden panel, a game loop) allocated a closure per `fn` declaration
per frame forever, and since every capture was treated as a GC root, the dead
closures pinned everything they had captured.

Strings are interned on creation — `"hello"` returns the same `StringId`
regardless of how many times it's constructed.

For pulling Rust data back out of a `Value`, `extract.rs` adds typed
accessor methods on `Heap` (`field_str`, `field_int`, `field_list`,
`opt_field_str`, `as_record`, …). They collapse the match-the-`Value`,
`get_map`, match-the-field boilerplate an embedder would otherwise write
into one call per field, with errors that name the field and the type
actually found.

### Stack

Runtime execution is register-based. Lowering (`backend/bytecode/lower.rs`)
turns each function body, and the root block, into one linear run of
instructions with one flat register array sized for the whole function. A
`Stack` owns a vector of frames; when an instruction completes, its result is
stored in its destination register, so a dataflow lookup is an array index.

Only a call pushes a frame. Control flow inside a function is jumps within its
frame:

- `Branch` / `Match` → conditional jumps to the chosen arm
- `ForLoop` / `NumericForLoop` / `WhileLoop` → a loop of jumps; each iteration
  bumps the frame's state path (see [State](#state))
- `And` / `Or` → a jump over the right-hand side when it is not needed
- `Call` → push a frame for the callee (`vm/calls.rs`), unless the call is
  replayed from its memo record

`Block.phi_outs` entries are lowered to `Move` instructions at the end of each
child region, which is how a rebinding inside an arm or a loop body reaches the
phi register in the enclosing code.

The optimizer passes run during and after lowering: copy propagation, escape
analysis for in-place mutation of loop accumulators, last-use rewriting of
straight-line mutations, in-place writes through `var` cells, and match
dispatch. Each is a flag in `backend::OptFlags`, and the unoptimized lowering
is the oracle every one is held to. [performance.md](performance.md#what-the-optimizer-does)
describes them.

### Env

`Env` is the top-level runtime object. It owns:
- All loaded programs, and the bytecode each was lowered to
- All live stacks
- The execution contexts (`ExecutionContext`): each is one isolated
  execution's heap, closure table, output buffers, host bindings and resource
  table. A fork (`Env::fork_execution`) gets a context of its own
- The `NativeFnTable` (builtins plus host-registered natives), the handle
  classes and the symbol table
- The module registry (in-memory modules, packages, search paths)
- The trace buffer, the observation buffer and the profile counters
- The `RunPolicy` in effect

Public API (abridged — see `env/mod.rs`):

```rust
impl Env {
    pub fn new() -> Self;
    pub fn load_program(&mut self, source: &str) -> Result<ProgramId, String>;
    pub fn create_stack(&mut self, program_id: ProgramId) -> Result<StackKey, String>;
    pub fn run(&mut self, stack: StackKey) -> Result<Value, String>;
    pub fn run_bounded(&mut self, stack: StackKey, max_steps: u64) -> Result<RunOutcome, String>;
    pub fn run_source(&mut self, source: &str) -> Result<Value, String>;
    pub fn call_function(&mut self, stack: StackKey, name: &str, args: &[Value]) -> Result<Value, String>;
    pub fn step(&mut self, stack: StackKey) -> Result<StepResult, String>;
    pub fn reset_stack(&mut self, stack: StackKey) -> Result<(), String>;
    pub fn register_native(&mut self, name: &str, func: NativeFn, effects: NativeEffects) -> NativeFnId;
    pub fn run_speculative(&mut self, stack: StackKey) -> Result<Value, String>;
    pub fn run_needed(&self, stack: StackKey) -> bool;  // the frame gate
    pub fn reload_program(...);  // hot reload
    pub fn check_program(&mut self, pid: ProgramId) -> CheckReport;
    pub fn set_policy(&mut self, policy: RunPolicy);
    pub fn snapshot_state(...);
    pub fn restore_state(...);
    pub fn trace(&self) -> &TraceBuffer;
    pub fn take_output(&mut self) -> Vec<String>;
}
```

`step` runs one bytecode instruction; `run` hands the VM a batch of
instructions at a time until the program completes.
`run_bounded` is `run` with a step budget: it returns `RunOutcome::Done(val)`
on completion or `RunOutcome::Yielded { steps }` once `max_steps` is reached,
leaving the stack runnable so the host can resume next frame or abort. This
lets an in-process host (e.g. an editor driving Petal panels at ~60fps) bound a
single run and recover from a runaway script instead of hanging its main
thread; resuming across many `run_bounded` calls yields the same result and
state-sweep behavior as one `run`.
`reset_stack` rewinds execution to the entry point **without dropping
persistent `state`**, which is the core of the live-editing story.

The embedding API as a host uses it is documented in
[ffi.md](../usage/ffi.md); this section is the map of what is behind it.

`call_function` lets the host invoke a single top-level Petal function by
name and get its return `Value` back, the event-callback counterpart to
`run`. Each `run` captures the stack's top-level named functions (and
lambdas bound to a name); `call_function` then invokes one synchronously
without re-running the program. This replaces the older "re-run the whole
program and stash a side effect in a thread-local" pattern. The captured
table is cleared on `transfer_state` and refreshed on the next `run`.

### Native Functions

Built-ins live in `core/src/builtins/`, one module per topic (`io`, `math`,
`collections`, `format`, `json`, `creative_coding`, `noise`, `color`, `vec2`
(vec2 + vec3), `autodiff`, `classes`, `effects`, `handle`, `pending`,
`output`). They are registered into the `NativeFnTable` at startup by
`register_builtins`, each with its `NativeEffects` row.

Registering a native does not put its name in scope. A package declares it
with `native fn` (`native_decl.rs`), which gives the checker its parameter
names, arity and types, and a script sees the name by importing the package.
The builtins are declared by the packages under `core/packages/` (`core`,
`io`, `json`, `color`, `noise`, `vec2`, `autodiff`, `creative`, `web`), which
are compiled into the binary (`bundled.rs`); the std prelude is
`core/prelude/std.ptl`. See [ffi.md](../usage/ffi.md#native-interfaces-native-fn).

The compiler allocates a "phantom" `Copy` term in every compiled program's
root block for each native (the VM seeds native function values into those
registers **by name** at root-frame push, so imported IR needs no phantoms
— see [ir-as-target.md](ir-as-target.md)). Registration order still numbers the
phantom terms of compiled programs, so reordering would renumber every IR
snapshot: don't reorder registrations; append only. See `builtins/mod.rs`.

Host embeddings (`integrations/petal-desktop-sdl`, `petal-web-html`,
`petal-web-canvas`, `petal-c-bridge`, and Garden) add their own natives via
`Env::register_native` before loading any program, each with its
`NativeEffects` row (what it reads, emits or does; see
[ffi.md](../usage/ffi.md#declaring-native-effects-honestly)). Those
registrations also produce phantom terms, shifting the starting ID of user
terms accordingly.

The trace buffer (`trace.rs`) records every term execution (inputs,
result, source line/column) into a ring buffer. Default capacity is
200,000 events; oldest events are dropped once full. Enable via
`--trace`, `--record-trace <path>`, or `PETAL_DEBUG=1`.

---

## State

`state` declarations compile to three op kinds:

- `StateInit` — control-flow term whose `child_blocks[0]` holds the init
  expression. On each visit the VM resolves a `RuntimeStateKey`,
  checks the persistent store, and **only runs the init block on a cache
  miss**. On a cache hit the existing value is written straight into the
  term's register; the init RHS is not evaluated. This makes
  `state buildings = [{...12 records...}]` allocate once and never again,
  even though the term sits in the root block that re-runs every frame.
- `StateRead` — reads the current value for the resolved runtime key.
- `StateWrite` — writes a new value (used for `+=`, direct assignment).
  Forwards the same explicit-key input as the matching `StateInit`, and
  carries its own `path_pop` (how many loop levels lie between that
  declaration and this write), so the resolved `RuntimeStateKey` agrees with
  the declaration's.

### Keying

A slot is a **declaration** plus the **call path that reached it** — the
React-`useState` model. One `state` declaration inside a function therefore
holds one value per callsite, per loop iteration around that callsite, and
per recursion depth. An explicit `state(expr)` key is absolute and ignores the
call path.

The declaration id (`Term::state_key`, a `StateKey`) and the callsite id
(`Term::call_site`) are both built in `compiler/state_ids.rs` from names and
structure, never from `TermId`s or spans, so they survive a hot reload. The
path (`RuntimeStateKey::path` in `stack.rs`) is composed as the program runs:
a frame's path is extended when it is entered and at each loop iteration, and
`Vm::state_key` (`vm/frame.rs`) reads it without walking the frame stack.

[state-call-paths.md](state-call-paths.md) owns the rule, its consequences
for editing (what a rename or a moved call does to a slot), the host entry
points, and the design decisions. What a script author sees is in
[inline-state.md](../features/inline-state.md).

### Lifecycle

`Env::run` brackets each top-level run with `start_run_tracking` /
`sweep_untouched_state`. Every `StateInit`/`StateRead`/`StateWrite`
records the `RuntimeStateKey` it touched; on completion, entries that
weren't touched this run are dropped. This is what reclaims state for
removed list items, for `state` declarations deleted on hot reload, and for
call paths a run stopped taking (a branch not entered, a callsite an edit
removed) — without it, the persistent store would grow unboundedly. A section
that is deliberately skipped but still in use (a memoized scope) keeps its
state with `Stack::begin_touch_capture` / `retain_touches`; see
[state-call-paths.md §3.3a](state-call-paths.md).

Within a run, every user-function call is a memoized scope: the VM records
what the call read and did, and on a later run with equal arguments and
captures replays the record (output, writes, observations, touched keys)
instead of pushing a frame. See [memo-scopes.md](memo-scopes.md).

`Env::run` also records what the run depended on — the bindings it read,
whether it consulted host data or randomness, whether it left `state`
changed — so a host can ask `Env::run_needed` whether running again could
differ, and skip the frame if not. See [frame-gate.md](frame-gate.md).

`reset_stack` preserves the state store while rewinding execution, so a host
re-runs a script every frame and its `state` persists. `snapshot_state` /
`restore_state` give host code explicit access to the persistent store
(used by petal-sdl's agent protocol `state` and `set_state` commands, and
by `run_speculative` to checkpoint+restore around a non-committing run).

### Hot reload

`Env::reload_program` (`env/reload.rs`) is what a host calls when a source file
changed. It diffs each source file against the text the program was compiled
from (`source_diff.rs`) and takes the cheapest exact path: an edit to layout or
comments only moves source positions; an edit to literal values writes the
constants in place, with no compile and no lowering; anything else recompiles
and moves the stack's state onto the new program with `transfer_state`
(`transfer_state.rs`), which matches slots by declaration id. The outcomes,
their cost and why each is exact are in
[hot-reload.md](../features/hot-reload.md).

---

## Static values

A host re-runs the whole script every frame, so a top-level table would be
rebuilt every frame. Three pieces avoid that: the classifier
(`static_classify/`) says which values are a function of the source text
alone, the cache (`static_cache.rs`, `OptFlags::static_cache`) wraps the code
that builds them in `static_skip` / `static_store` instructions so a stack
keeps them between runs, and a reload puts back the ones an edit cannot have
changed (`env/static_follow.rs`). `const` is the declaration that must be
static. The implementation and its invariants are in
[internals/static-values.md](../internals/static-values.md); what a script
author and a host see is in
[features/static-values.md](../features/static-values.md); measurements are
in [performance.md](performance.md#static-values).

---

## Modules and packages

Imports are resolved at compile time and merged into one `Program`
(`module.rs`): the import walk turns an entry source into a flat,
dependency-ordered list of parsed modules, and the compiler qualifies each
module's names. An import path is relative (`./level`) or starts with a
package name; a package is a directory with a `petal.toml` (`package.rs`),
found among what the host registered (`Env::register_module`,
`Env::register_package`, `Env::add_package`), the importing file's own
package and its dependencies, the search path (`-I`, `PETAL_PATH`), and the
packages bundled in the binary. A module's identity is canonical, not the
spelling an importer used, and that identity is what qualifies its `state`
keys. `pkgman.rs` is the package manager behind `petal add` / `install` /
`update` / `vendor` / `bundle`.

A package can also declare a host interface: `native fn` declarations that
are linked against the host's native table at load (`Program::native_links`,
`Env::link_program`), and interfaces a program requires
(`Program::requires`, `Env::provides`). See
[module-system.md](../module-system.md) and [packages.md](../packages.md).

---

## Source tools

- **`petal fmt`** (`fmt/`) is the canonical layout. It changes whitespace
  only, has no options, and is held to "formats to identical IR, and
  formatting is a fixed point" over every `.ptl` in the repo.
- **`petal lint`** (`lint/`) is rules about which code to write, each with a
  fix. `--fix --verify` proves a rewrite with `ir_equiv` before writing it.
  The rule design is in [linter-plan.md](linter-plan.md).
- **`petal suggest`** (`suggest/`) offers safe refactors without applying
  them (return types, named arguments); see
  [suggestions-plan.md](suggestions-plan.md). **`petal apply-change`**
  (`apply_change/`) carries out refactors that change what the program does.
- The source-preserving edits these and the goal API share go through the
  lossless CST (`cst/`) and `rewrite.rs`.

Command-line usage for all of them is in [CLI.md](../CLI.md).

---

## Provenance & Dataflow Slicing

Because every term has explicit `inputs` edges, dataflow queries reduce
to graph traversals:

- **Backward slice** (`graph`) — walk `inputs` recursively from
  a target term. Answers "what feeds into this value?"
- **Forward slice** (`graph --direction forward`) — walk the reverse-inputs index
  from a source term. Answers "what does this value influence?"
- **Minimal slice** (`graph --slice`, or several `--term`s) — smallest subgraph connecting a set
  of target terms.

`petal explain --term <name>` combines a slice with recorded trace
values, producing a "why does `x` have value Y" walkback.

The graph walks live in `program_analysis.rs` (with the CLI handlers in
`cli/handlers.rs`); each is a few dozen lines thanks to the flat term array.

---

## Differentiation (forward-mode)

Petal has built-in forward-mode automatic differentiation via `Value::Dual
{ value, derivative }`, stored inline in the 16-byte `Value`. Usage, and the
list of builtins that carry or drop a derivative, is in
[features/differentiation.md](../features/differentiation.md).

- `backend/ops.rs`: `arithmetic` hands any pair with a dual operand to
  `dual_arith` (the sum, product and quotient rules), and `negate` has a dual
  arm.
- `builtins/math.rs`: `unary_float_dual` (`sqrt`, `sin`, `cos`, `tan`,
  `asin`, `acos`, `atan`, `float`) and `unary_num_preserving` (`abs`,
  `floor`, `ceil`, `round`) take a derivative function and apply the chain
  rule.
- `builtins/autodiff.rs`: the constructor and accessors (`dual`, `value_of`,
  `deriv_of`), declared to scripts by the bundled `autodiff` package
  (`core/packages/autodiff/`), and the helpers the multi-argument rules are
  written in: `D` (a value and derivative pair with `add`/`sub`/`mul`/`div`/
  `chain`), `args_d` (read the arguments as `D`s and say whether any was a
  dual) and `push_d` (push a dual only when one came in). `exp`, `log`,
  `pow`, `atan2`, `hypot`, `clamp`, `fract`, `sign`, `lerp`, `map_range`,
  `smoothstep`, `radians`, `degrees`, `interp` and the scalar forms of `mag`
  and `distance` use them.
- A native that reads an argument with `PetalCxt::get_float` has no rule, and
  `get_float` raises on a dual rather than hand back the value alone, so no
  builtin or host function drops a derivative silently. `vec2` and `vec3`
  refuse a dual component by name: a vector holds plain floats.
- `value.rs`: `==` and ordering compare a dual by its primal. `memo.rs` keys
  on both fields, so a memoized call never returns another seed's derivative.

Reverse-mode (back-propagation through the dataflow graph) is a design
goal but not yet implemented — see the [README](../../README.md#goals) for the
project goals.

---

## Compilation from AST to IR

The compiler (`compiler/`) is a single pass that walks the AST and
emits terms. Key responsibilities:

- **Register allocation** — each block gets a flat register array; each
  term gets an index. Registers can be reused across child frames since
  frames are independent.
- **Scope resolution** — names bind to the most recently named term in
  the enclosing scope chain (the `scopes` stack in `compiler/mod.rs`).
- **Phi insertion** — when a name is rebound inside a child block but
  was bound in an outer scope, emit a `Phi` in the outer block and
  record a `PhiOut` on the child.
- **Overload collection** — prescan `fn` declarations, bucket same-name
  variants by arity, emit one `MakeClosure` per variant and a single
  `MakeOverloadSet` bundling them.
- **Parse-error tolerance** — parse errors become `Error` terms; the
  program still compiles (with `has_errors: true`) so tooling can
  inspect partial results.

---

## WebAssembly

`wasm.rs` exposes a `PetalRuntime` struct through `wasm-bindgen`. It wraps an
`Env` and adds the element-tree natives petal-web needs. The JS host calls
`load_program(source)`, `create_stack(program_id)`, then `run(stack_id)` or
`reset_and_run(stack_id)` once per frame, and reads printed output with
`take_output()`. `register_module` and `register_package` set up module
resolution before loading; the runtime's own natives are declared by the
`web` package, which a script imports. State is round-tripped as JSON so the JS host can
keep it across reloads.

The host side lives in `integrations/petal-web-html/src/runtime.ts` and
`integrations/petal-web-canvas/src/runtime.ts`;
`examples/custom-apps/diagram-canvas` consumes the latter through the
`petal-web-canvas` package.

---

## Further Reading

- [Language Guide](../language-guide.md) — user-facing language reference
- [CLI Reference](../CLI.md) — full CLI command list + IR JSON schema
- [Builtins Reference](../Builtins.md) — all built-in functions
- [Function Overloading](../syntax/function-overloading.md) — multi-arity dispatch
- [Embedding (FFI)](../usage/ffi.md) — the `Env` API as a host uses it
- [Hot Reload](../features/hot-reload.md) — reload outcomes, cost and exactness
- [state-call-paths.md](state-call-paths.md), [memo-scopes.md](memo-scopes.md),
  [frame-gate.md](frame-gate.md), [ir-as-target.md](ir-as-target.md),
  [performance.md](performance.md) — the detail behind sections of this document
- [Testing](testing.md) and [Developer Scripts](scripts.md)
- [Debug Protocol](debug-protocol.md) — SDL / canvas agent protocol
- [Debugging & Visibility](debugging-visibility.md) — observability stack
- [Goals](../../README.md#goals) — the project goals and design decisions
