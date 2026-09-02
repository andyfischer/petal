# Petal Architecture

This document describes the implementation of the Petal compiler and runtime.
It's the internal counterpart to the [Language Guide](../language-guide.md) — read
this if you're working on the compiler or debugging IR behavior.

```
Source Code → Lexer → Parser → AST → Compiler → IR (Term Graph) → Bytecode VM
```

All of the above lives in `rust/src/` as a single crate. The binary entry
point is `rust/src/main.rs`, which delegates to the CLI dispatcher in
`cli/`.

The term graph is the canonical, introspectable IR — provenance, slicing,
autodiff, `explain`, hot-reload, and the IR-as-target contract all reason about
it. It is executed by the **bytecode VM** (`rust/src/backend/bytecode/`), a
register machine that runs a linear *lowering* of the graph, with
escape-analysis-driven in-place mutation gated by `backend::OptFlags`. The VM
populates the trace buffer that the graph-level introspection reads. See
[performance.md](performance.md) for what the optimizer does.

---

## Source File Map

The main files and directories under `rust/src/`, grouped by pipeline stage.

**Front end**

| File | Purpose |
|------|---------|
| `main.rs` / `lib.rs` | Binary entry point (delegates to `cli::run`) / crate root |
| `cli/` | `args.rs` (arg parsing), `handlers.rs` (command handlers), `help.rs` (`petal help`), `mod.rs` (dispatch) |
| `lexer.rs` | Source text → token stream |
| `parse.rs`, `ast.rs`, `ast_display.rs` | Tokens → AST; AST node types; AST pretty-printer |
| `cst/`, `cst_project.rs`, `trivia.rs` | Lossless concrete syntax tree for source-preserving edits, and its projection to the typed AST. `parse.rs` and the CST parser are kept in lockstep; unifying them is future work |
| `desugar.rs` | `@`-argument (in-out call) desugaring |
| `compiler/` | AST → IR term graph: `expr.rs`, `stmt.rs`, `function.rs`, `phi.rs` (phi insertion), `state_ids.rs` (state keys and callsite ids), `capture_lag.rs` |
| `typecheck/` | The warning-only static type checker |
| `diagnostic.rs`, `error.rs` | Diagnostics and error types |

**IR**

| File | Purpose |
|------|---------|
| `program.rs` | `Program`, `Block`, `Term`, `TermOp` definitions |
| `constant_table.rs` | Deduplicated literal storage |
| `source_map.rs` | `TermId` → source span mapping |
| `module.rs` | Module resolution and import walk (merges imports into one `Program`) |
| `ir_display.rs`, `ir_serialize.rs`, `ir_validate.rs` | Text pretty-printer; serde helpers for IR JSON; structural validation when loading JSON IR |
| `ir_equiv.rs` | "Are these two programs the same?" ignoring positions (`petal ir-equal`) |
| `program_analysis.rs`, `provenance.rs`, `dot_graph.rs` | Dataflow slicing, provenance walks, DOT rendering |
| `static_value.rs`, `types.rs`, `classes.rs` | Compile-time values, type representation, and the `ClassTable` of `class` declarations (including the built-in `Rect`) |

**Source editing and tooling**

| File | Purpose |
|------|---------|
| `rewrite.rs`, `goal_based_editing.rs`, `direct_manipulation.rs` | Formatting-preserving source rewriting, goal-based edits, and emit-trace provenance |
| `lint/` | `petal lint`: `reindent.rs` (formatting), `casts.rs` (identity casts), `to_match.rs` (if-chain to match) |
| `lsp/` | `petal lsp`, the language server |
| `inspect.rs`, `observe.rs`, `profile.rs`, `stats.rs` | Introspection helpers, the observation buffer, the `--profile` counters, `--dup-stats` |
| `test_corpus.rs` | Enumerates the repo's `.ptl` files for corpus-wide unit tests |

**Runtime**

| File | Purpose |
|------|---------|
| `backend/mod.rs` | `OptFlags` optimization toggles plus shared runtime helpers (`calls.rs`, `ops.rs`, `pattern.rs`, `errors.rs`) |
| `backend/bytecode/` | Register VM: `isa` (instructions), `lower` (graph → bytecode), `copyprop`, `escape`, `lastuse` (optimizer passes), `disasm` (`show-bytecode`), `fuzz`, `vm/` (`dispatch.rs`, `calls.rs`, `frame.rs`, `intrinsics.rs`, `native.rs`) |
| `stack.rs` | `Stack`: VM frames plus persistent state |
| `value.rs` | The `Value` enum |
| `heap.rs` | Mark-and-sweep GC for strings, lists, f64 arrays, maps, elements. A map slot also carries an optional class tag, which is what makes a record an instance |
| `closure_table.rs` | Runtime closures and overload sets; swept jointly with the heap |
| `execution_context.rs` | One isolated execution's mutable runtime bundle (heap plus closure/output/host registries) |
| `env/` | `Env`, which owns programs, stacks, and contexts: `run.rs`, `fork.rs`, `gc.rs`, `host_io.rs`, `state_json.rs`, `observations_json.rs` |
| `native_fn.rs`, `builtins/` | Native function FFI (`NativeFnTable`, `PetalCxt`) and the built-in functions, one module per topic |
| `handle.rs`, `symbol.rs`, `resource_table.rs` | Opaque host-object handles; interned symbols shared with the host; pending (unresolved) resources |
| `trace.rs` | Ring-buffered per-term execution trace |
| `transfer_state.rs` | Move a stack's state onto a different program, reconciling by `StateKey` (hot reload) |
| `extract.rs` | Typed accessors for pulling Rust data out of a `Value` |
| `wasm.rs` | `wasm-bindgen` bindings used by the browser integrations |

The crate is about 120 source files and 60k lines.

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
    pub collect: bool,                 // value-position loop collects its results
}
```

Terms participate in **two graphs** at once:

1. **Dataflow** — via `inputs`. A term's inputs are the terms whose values
   it consumes. This graph is a DAG.
2. **Block ordering** — via `block_next`/`block_prev`. Each block holds a
   linked list that defines execution order within that scope. (In-memory
   only: the serialized IR carries an ordered `terms` array per block, and
   the loader rebuilds these links from it.)

The evaluator walks the block's linked list; it evaluates dataflow inputs
by reading the corresponding register (they've already run).

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
- **Arithmetic / comparison / logical** — `Add`, `Sub`, `Eq`, `And`, …
- **Control flow** — `Branch`, `ForLoop`, `NumericForLoop`, `WhileLoop`, `Break`, `Continue`, `Return`
- **Data joins** — `Phi` (see below)
- **State** — `StateInit`, `StateRead`, `StateWrite`
- **Cells** — `CellNew`, `CellRead`, `CellWrite` (the `var` escape hatch)
- **Functions** — `MakeClosure`, `MakeOverloadSet`, `Call`, `MethodCall`
- **Data** — `AllocList`, `AllocMap`, `AllocMapSpread`, `AllocElement`, `MakeEnumVariant`
- **Access** — `GetField`, `SetField`, `GetIndex`, `SetIndex`
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
so provenance stops there. See [var.md](../var.md) for the
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
This powers error messages, `explain`, `show-provenance`, and
the trace buffer's `line`/`column` fields.

### Functions and Closures

`FunctionDef` holds compile-time metadata; `MakeClosure` creates a
runtime closure value with the captured values baked in.

```rust
pub struct FunctionDef {
    pub id: FunctionId,
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body_block: BlockId,
    pub capture_names: Vec<String>,
    pub capture_registers: Vec<RegisterIndex>,
    pub self_ref_register: Option<RegisterIndex>, // for recursion
    pub register_count: u16,
}
```

Overloading (see [function-overloading.md](../function-overloading.md)) is
compiled as one `MakeClosure` per variant plus one `MakeOverloadSet` that
bundles them. Dispatch at runtime selects the variant by argument count.

---

## Runtime

### Value

`Value` is a `Copy` 24-byte enum. Heap-allocated values (strings, lists,
f64 arrays, maps, elements) are stored by ID into the `Heap` — `Value`
just carries the ID. `Vec2` and `Dual` are stored inline (unboxed), no
heap allocation.

```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    F64Array(F64ArrayId),
    Map(MapId),
    Closure(ClosureId),
    OverloadSet(OverloadSetId),
    NativeFunction(NativeFnId),
    EnumVariant { tag: StringId, data: ListId },
    Element(ElementId),
    Cell(CellId),
    Dual { value: f64, derivative: f64 },
    Vec2(f64, f64),
    Symbol(SymbolId),
    Handle(HandleVal),
    Pending(PendingId),
}
```

Because `Value` is `Copy`, there are no `Rc<RefCell<...>>` dances — the
heap handles aliasing, and the GC handles reclamation.

### Heap & GC

`Heap` is a mark-and-sweep garbage collector. A collection is triggered by
*work owed* rather than object count: each allocation charges its payload size
plus a per-slot term, and a sweep runs once that reaches a budget the previous
sweep sized from the live set (`should_collect`). Live values are found by
walking all live stacks' registers plus the other roots — closure captures,
host bindings, output buffers, the resource table, observations. Reclaimed
slots go onto a free list.

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

Runtime execution is register-based. A `Stack` owns a vector of `Frame`s;
each frame has a flat register array sized by the block's `register_count`.
When a term completes, its result is stored in the register `term.register`.
Dataflow lookup is just an array index.

Control flow pushes and pops frames:
- `Branch` / `Match` → push one frame for the chosen arm block
- `ForLoop` / `NumericForLoop` / `WhileLoop` → push/pop a body frame per iteration
- `Call` → push a frame for the function body block
- `And` / `Or` → push the RHS block only if short-circuit demands it

On pop, `Block.phi_outs` entries copy updated register values back into
the parent frame (this is how rebindings propagate outward).

### Env

`Env` is the top-level runtime object. It owns:
- All loaded programs (`Vec<Program>`)
- All live stacks (`SlotMap<StackKey, Stack>`)
- The shared `Heap`
- The `NativeFnTable` (builtins plus host-registered natives)
- The trace buffer

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
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> NativeFnId;
    pub fn run_speculative(&mut self, stack: StackKey) -> Result<Value, String>;
    pub fn snapshot_state(...);  // hot-reload support
    pub fn restore_state(...);
    pub fn trace(&self) -> &TraceBuffer;
    pub fn take_output(&mut self) -> Vec<String>;
}
```

`step` runs one term; `run` loops `step` until the program completes.
`run_bounded` is `run` with a step budget: it returns `RunOutcome::Done(val)`
on completion or `RunOutcome::Yielded { steps }` once `max_steps` is reached,
leaving the stack runnable so the host can resume next frame or abort. This
lets an in-process host (e.g. an editor driving Petal panels at ~60fps) bound a
single run and recover from a runaway script instead of hanging its main
thread; resuming across many `run_bounded` calls yields the same result and
state-sweep behavior as one `run`.
`reset_stack` rewinds execution to the entry point **without dropping
persistent `state`**, which is the core of the live-editing story.

`call_function` lets the host invoke a single top-level Petal function by
name and get its return `Value` back, the event-callback counterpart to
`run`. Each `run` captures the stack's top-level named functions (and
lambdas bound to a name); `call_function` then invokes one synchronously
without re-running the program. This replaces the older "re-run the whole
program and stash a side effect in a thread-local" pattern. The captured
table is cleared on `transfer_state` and refreshed on the next `run`.

### Native Functions

Built-ins live in `src/builtins/`, one module per topic (`io`, `math`,
`collections`, `format`, `creative_coding`, `noise`, `color`, `vec2`,
`autodiff`, `classes`, `effects`, `handle`, `pending`, `output`). They are
registered into the `NativeFnTable` at startup by `register_builtins`.

The compiler allocates a "phantom" `Copy` term in every compiled program's
root block for each native (the VM seeds native function values into those
registers **by name** at root-frame push, so imported IR needs no phantoms
— see docs/dev/ir-as-target.md). Registration order still numbers the
phantom terms of compiled programs, so reordering would renumber every IR
snapshot: don't reorder registrations; append only. See `builtins/mod.rs`.

Host embeddings (petal-sdl, petal-web, petal-diagram-canvas) add their
own natives via `Env::register_native` before loading any program. Those
registrations also produce phantom terms, shifting the starting ID of
user terms accordingly.

The trace buffer (`trace.rs`) records every term execution (inputs,
result, source line/column) into a ring buffer. Default capacity is
200,000 events; oldest events are dropped once full. Enable via
`--trace`, `--record-trace <path>`, or `PETAL_DEBUG=1`.

---

## State

`state` declarations compile to three op kinds:

- `StateInit` — control-flow term whose `child_blocks[0]` holds the init
  expression. On each visit the evaluator resolves a `RuntimeStateKey`,
  checks the persistent store, and **only pushes the init block on a cache
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
per recursion depth. Both halves of the key are derived from names and
structure, never from `TermId`s or spans, so they survive a hot reload. The
full spec is [state-call-paths.md](state-call-paths.md).

**The declaration id** (`Term::state_key`, a `StateKey`, built by
`Compiler::state_key_for`) hashes the declaration's full name path: the
module qualifier (`ui::`), the enclosing function-name chain (`draw/row/`),
the variable name, and a shadow ordinal (`#1`, `#2`, …) separating repeated
declarations of one name in one function. A top-level declaration hashes
exactly its bare (module-qualified) name — the same key it had before
call-path keying, so persisted state carried over the change untouched. Two
functions declaring the same state name can no longer collide.

**The path** (`RuntimeStateKey::path`, `stack.rs`) is a `SmallVec` of
`PathPart`s, composed incrementally as the program runs rather than walked
at access time:

- `Call(site)` — pushed onto a frame's path when it is entered, taken from
  `Term::call_site` on the call term (`Compiler::call_site_for`): a hash of
  the callee's canonical text (`f`, `obj.method`, `m::f`), its ordinal among
  identically-spelled callees in the enclosing function, and that function's
  module/name chain.
- `Index(i)` — the current 0-based iteration of an enclosing `for`/`while`,
  pushed at *every* level of the live frame stack, not just the declaring
  function's. Positional: reordering the iterated list moves the slots.
- `Key(h)` — an explicit `state(expr)` key, hashed.

`Vm::state_key` (`vm/frame.rs`) then composes an access:

- Top-level `state x`: the root path is empty, so the key is
  `{ base, path: [] }` — byte-identical to the pre-path scheme.
- `state x` in a function: the frame's whole live path — its callsite chain
  plus whatever loop iterations it is inside right now.
- Explicit-key `state(expr) x` is **absolute**: the path is exactly
  `[Key(hash)]` and the call path is ignored. This is the escape hatch for
  "same entity ⇒ same slot, no matter who asks", and the recommended form
  when an iterated collection has a domain identifier (entity id, slot
  name) — state then survives reordering, removal and a change of recursion
  depth, because the key follows the data.
- A reassignment nested deeper in loops than its declaration drops that many
  innermost `Index` parts (`Term::path_pop`, a compile-time count), so the
  accumulator idiom — `state xs = []` at the top level, `xs = append(xs, i)`
  inside a `for` — still writes the one persisted slot.
- A host call (`Env::call_function`) has no caller frame, so it runs on a
  root path of a single `Call` part derived from the name the host asked
  for: repeated host calls of one function share slots with each other, but
  never with an in-program call of the same function.

That means:

- Editing anywhere that isn't the call structure around a `state` — adding
  statements, renaming an unrelated local, reformatting — leaves every slot
  where it was across a hot reload.
- Renaming or deleting a state variable, or moving its declaration between
  functions or modules, drops the old slot cleanly.
- Adding a new state variable falls through to `StateInit` on the next tick.
- Renaming a callee, or adding/removing an *earlier* call to the same callee
  in the same function, shifts that callsite's ordinal and therefore its id.
  The slots below the old id are orphaned (the sweep below reclaims them) and
  the call adopts whatever the id it moved onto holds — so deleting the first
  of two `f()` calls hands the survivor the first one's state. Same
  accepted-loss class as renaming a state variable; `state(key)` is the
  escape hatch when a slot must be immune to it.
- Genuinely shared, cross-function state is a top-level `state var` cell read
  and written with `get`/`set` — not a function wrapping a `state`, which now
  gives each of its callers a private slot.

### Lifecycle

`Env::run` brackets each top-level run with `start_run_tracking` /
`sweep_untouched_state`. Every `StateInit`/`StateRead`/`StateWrite`
records the `RuntimeStateKey` it touched; on completion, entries that
weren't touched this run are dropped. This is what reclaims state for
removed list items, for `state` declarations deleted on hot reload, and for
call paths a run stopped taking (a branch not entered, a callsite an edit
removed) — without it, the persistent store would grow unboundedly.

`reset_stack` preserves the state store while rewinding execution — that's
what makes `petal-sdl`'s hot reload work. `snapshot_state` /
`restore_state` give host code explicit access to the persistent store
(used by petal-sdl's agent protocol `state` and `set_state` commands, and
by `run_speculative` to checkpoint+restore around a non-committing run).

---

## Provenance & Dataflow Slicing

Because every term has explicit `inputs` edges, dataflow queries reduce
to graph traversals:

- **Backward slice** (`show-provenance`) — walk `inputs` recursively from
  a target term. Answers "what feeds into this value?"
- **Forward slice** (`show-dependents`) — walk the reverse-inputs index
  from a source term. Answers "what does this value influence?"
- **Minimal slice** (`show-slice`) — smallest subgraph connecting a set
  of target terms.

`petal explain --term <name>` combines a slice with recorded trace
values, producing a "why does `x` have value Y" walkback.

The graph walks live in `program_analysis.rs` (with the CLI handlers in
`cli/handlers.rs`); each is a few dozen lines thanks to the flat term array.

---

## Differentiation (forward-mode)

Petal has built-in forward-mode automatic differentiation via `Value::Dual
{ value, derivative }`. Arithmetic ops and the math builtins in
`builtins/math.rs` (`sqrt`, `abs`, `sin`, `cos`, `tan`, `floor`, `ceil`,
`round`, `float`) propagate derivatives through the chain rule. `exp`,
`log`, and `pow` (in `builtins/creative_coding.rs`) currently operate on
the primal only and drop the derivative. See
`examples/console/differentiation.ptl`.

Reverse-mode (back-propagation through the dataflow graph) is a design
goal but not yet implemented — see [goals.md](goals.md) for the vision,
remaining work, and roadmap.

---

## Compilation from AST to IR

The compiler (`compiler/`) is a single pass that walks the AST and
emits terms. Key responsibilities:

- **Register allocation** — each block gets a flat register array; each
  term gets an index. Registers can be reused across child frames since
  frames are independent.
- **Scope resolution** — names bind to the most recently named term in
  the enclosing scope chain (`NameScope` stack).
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
`take_output()`. `register_module` and `set_implicit_imports` set up module
resolution before loading. State is round-tripped as JSON so the JS host can
keep it across reloads.

The host side lives in `integrations/petal-web-html/src/runtime.ts` and
`integrations/petal-web-canvas/src/runtime.ts`;
`examples/custom-integrations/diagram-canvas` consumes the latter through the
`petal-web-canvas` package.

---

## Further Reading

- [Language Guide](../language-guide.md) — user-facing language reference
- [CLI Reference](../CLI.md) — full CLI command list + IR JSON schema
- [Builtins Reference](../Builtins.md) — all built-in functions
- [Function Overloading](../function-overloading.md) — multi-arity dispatch
- [Debug Protocol](debug-protocol.md) — SDL / canvas agent protocol
- [Debugging & Visibility](debugging-visibility.md) — observability stack
- [Goals](goals.md) — vision, remaining work, and sequencing
