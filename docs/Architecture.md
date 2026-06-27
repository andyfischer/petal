# Petal Architecture

This document describes the implementation of the Petal compiler and runtime.
It's the internal counterpart to the [Language Guide](Language_Guide.md) — read
this if you're working on the compiler or debugging IR behavior.

```
Source Code → Lexer → Parser → AST → Compiler → IR (Term Graph) → Step Evaluator
```

All of the above lives in `rust/src/` as a single crate. The binary entry
point is `rust/src/main.rs`, which delegates to the CLI dispatcher in
`cli.rs`.

---

## Source File Map

| File | Purpose |
|------|---------|
| `main.rs` | Binary entry point (delegates to `cli::run`) |
| `lib.rs` | Crate root / public exports |
| `cli.rs` | CLI command dispatch (`run`, `show-ir`, `explain`, ...) |
| `lexer.rs` | Source text → token stream |
| `parse.rs` | Tokens → AST |
| `ast.rs` | AST node types |
| `compiler.rs` | AST → IR term graph |
| `program.rs` | `Program`, `Block`, `Term`, `TermOp` definitions |
| `constant_table.rs` | Deduplicated literal storage |
| `source_map.rs` | `TermId` → source span mapping |
| `rewrite.rs` | Formatting-preserving source rewriting (find a top-level call by name, splice its span) |
| `eval.rs` | Step evaluator (the runtime) |
| `stack.rs` | `Stack` and `Frame` — execution context |
| `value.rs` | `Value` enum (runtime values) |
| `heap.rs` | Mark-and-sweep GC for strings, lists, maps, vecs, elements |
| `env.rs` | `Env` — owns programs, stacks, heap, native table |
| `native_fn.rs` | Native function FFI (`NativeFnTable`, `PetalCxt`) |
| `builtins/` | Built-in function implementations (io, math, collections, …) |
| `trace.rs` | Ring-buffered per-term execution trace |
| `transfer_state.rs` | Transfer a stack's state onto a different program — reconciles state by StateKey (used for hot-reload in petal-sdl) |
| `ir_display.rs` | Text pretty-printer for IR (for `show-ir` without `--json`) |
| `ir_serialize.rs` | Custom serde helpers for IR JSON output |
| `wasm.rs` | `wasm-bindgen` FFI used by petal-web and petal-diagram-canvas |

Current size: ~30 source files, ~12k lines.

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
    pub id: ProgramId,
    pub source: String,
    pub terms: Vec<Term>,            // indexed by TermId
    pub blocks: Vec<Block>,          // indexed by BlockId
    pub root_block: BlockId,         // entry point
    pub constants: ConstantTable,    // deduplicated literals
    pub source_map: SourceMap,       // term → source span
    pub has_errors: bool,            // true if any Error terms
    pub functions: Vec<FunctionDef>, // function definitions
    pub match_arms: HashMap<TermId, Vec<MatchArmMeta>>,
}
```

`Program` is the unit the CLI prints when you run `show-ir --json`. See
[CLI Reference](CLI.md) for the full JSON schema.

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
    pub in_loop: bool,
}
```

Terms participate in **two graphs** at once:

1. **Dataflow** — via `inputs`. A term's inputs are the terms whose values
   it consumes. This graph is a DAG.
2. **Block ordering** — via `block_next`/`block_prev`. Each block holds a
   linked list that defines execution order within that scope.

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
documented in [CLI.md](CLI.md#termop--serdes-externally-tagged-encoding);
the important groups are:

- **Loads** — `Constant`, `Error`, `Copy`
- **Arithmetic / comparison / logical** — `Add`, `Sub`, `Eq`, `And`, …
- **Control flow** — `Branch`, `ForLoop`, `NumericForLoop`, `WhileLoop`, `Break`, `Continue`, `Return`
- **Data joins** — `Phi` (see below)
- **State** — `StateInit`, `StateRead`, `StateWrite`
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

Ongoing design notes live in [MutabilityPlan.md](MutabilityPlan.md).

### ConstantTable

Literals (ints, floats, strings, bools, nil) are stored once per program
in `ConstantTable` and referenced by `ConstantId`. The table deduplicates:
two `"hello"` literals share the same entry.

### SourceMap

Maps each `TermId` to a `SourceSpan` (`{line, column, offset}` for start
and end). This powers error messages, `explain`, `show-provenance`, and
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

Overloading (see [Function_Overloading.md](Function_Overloading.md)) is
compiled as one `MakeClosure` per variant plus one `MakeOverloadSet` that
bundles them. Dispatch at runtime selects the variant by argument count.

---

## Runtime

### Value

`Value` is a `Copy` 16-byte enum. Heap-allocated values (strings, lists,
maps, vec2, elements) are stored by ID into the `Heap` — `Value` just
carries the ID.

```rust
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    Map(MapId),
    Vec2(Vec2Id),
    Element(ElementId),
    Dual { value: f64, deriv: f64 },
    Closure(ClosureId),
    OverloadSet(OverloadSetId),
    EnumVariant(EnumVariantId),
    NativeFn(NativeFnId),
}
```

Because `Value` is `Copy`, there are no `Rc<RefCell<...>>` dances — the
heap handles aliasing, and the GC handles reclamation.

### Heap & GC

`Heap` is a mark-and-sweep garbage collector. Every 1024 allocations it
sweeps; live values are found by walking all live stacks' registers
plus the string-intern table. Reclaimed slots go onto a free list.

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

Public API (abridged — see `env.rs`):

```rust
impl Env {
    pub fn new() -> Self;
    pub fn load_program(&mut self, source: &str) -> Result<ProgramId, String>;
    pub fn create_stack(&mut self, program_id: ProgramId) -> Result<StackKey, String>;
    pub fn run(&mut self, stack: StackKey) -> Result<Value, String>;
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

Built-ins live in `src/builtins/` (one module per topic: `io`, `math`,
`collections`, `creative_coding`, `noise`, `color`, `vec2`, `autodiff`).
Each registers into the `NativeFnTable` at startup.

Registration order **is load-bearing** — the compiler allocates a
"phantom" `Copy` term in every program's root block for each native, and
that term's ID must match the native's table ID. Don't reorder
registrations; append only. See `builtins/mod.rs`.

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
  Forwards the same explicit-key input as the matching `StateInit` so the
  resolved `RuntimeStateKey` agrees.

### Keying

Each declaration gets a static `StateKey` hashed from its source-level
name. The runtime composes a `RuntimeStateKey` per access:

- Top-level `state x`: `RuntimeStateKey { base, loop_indices: [] }`
- Inside a loop, default form `state x`: `loop_indices` filled from each
  active loop's iteration index. Stable as long as the iterated list
  doesn't reorder or shrink.
- Explicit-key form `state(expr) x`: the computed `expr` is hashed into
  `loop_indices: [Explicit(hash)]`. This is the recommended form when an
  iterated collection has a domain identifier (entity id, slot name) —
  state survives reordering and item removal, since the key follows the
  data.

That means:

- Reordering code doesn't reshuffle state across hot reloads.
- Renaming or deleting a state variable drops the old slot cleanly.
- Adding a new state variable falls through to `StateInit` on the next tick.

### Lifecycle

`Env::run` brackets each top-level run with `start_run_tracking` /
`sweep_untouched_state`. Every `StateInit`/`StateRead`/`StateWrite`
records the `RuntimeStateKey` it touched; on completion, entries that
weren't touched this run are dropped. This is what reclaims state for
removed list items and for `state` declarations deleted on hot reload —
without it, the persistent store would grow unboundedly.

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

These live in `cli.rs`; the underlying graph walks are a few dozen lines
each thanks to the flat term array.

---

## Differentiation (forward-mode)

Petal has built-in forward-mode automatic differentiation via `Value::Dual
{ value, deriv }`. Arithmetic ops and math builtins (`sqrt`, `abs`,
`sin`, `cos`, `pow`, `exp`, `log`, `floor`, `ceil`, `round`) propagate
derivatives through the chain rule. See `examples/differentiation.ptl`.

Reverse-mode (back-propagation through the dataflow graph) is a design
goal but not yet implemented — see [goals.md](goals.md) for the vision,
remaining work, and roadmap.

---

## Compilation from AST to IR

The compiler (`compiler.rs`) is a single pass that walks the AST and
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

`wasm.rs` exposes a small `wasm-bindgen` API used by `petal-web` and
`petal-diagram-canvas`. The browser embeddings:

1. Fetch `.ptl` source over HTTP.
2. Call `compile(source)` → wasm returns a program handle.
3. Call `run_frame(program, state_json, input_json)` → wasm returns
   draw commands / element tree / state snapshot as JSON.

State is round-tripped as JSON so the JS host owns it across reloads.
See `apps/petal-web/src/runtime.ts` and `apps/petal-diagram-canvas/src/runtime.ts`
for the host side.

---

## Further Reading

- [Language Guide](Language_Guide.md) — user-facing language reference
- [CLI Reference](CLI.md) — full CLI command list + IR JSON schema
- [Builtins Reference](Builtins.md) — all built-in functions
- [Function Overloading](Function_Overloading.md) — multi-arity dispatch
- [Debug Protocol](debug-protocol.md) — SDL / canvas agent protocol
- [Debugging & Visibility](debugging-visibility.md) — observability stack
- [Mutability Plan](MutabilityPlan.md) — design notes on phi joins
- [Goals](goals.md) — vision, remaining work, and sequencing
