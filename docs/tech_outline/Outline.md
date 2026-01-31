# Petal Implementation Outline

This document describes the Rust implementation strategy for the Petal language runtime.

## Source Files

| File | Purpose |
|------|---------|
| `lib.rs` | Public API exports and crate root |
| `env.rs` | [[Env\|Env]] implementation - owns all programs and stacks |
| `program.rs` | [[Program\|Program]] and [[Term\|Term]] types |
| `stack.rs` | [[Stack\|Stack]] and Frame types for execution |
| `value.rs` | [[Value\|Value]] enum and operations |
| `eval.rs` | Interpreter/evaluator - executes terms |
| `parse.rs` | Source code parser - text to Program |
| `source_map.rs` | [[SourceMap\|SourceMap]] - term to source location mapping |
| `typing.rs` | [[TypeSystem\|Type system]] and type checking |
| `live_edit.rs` | Live editing support and [[StateSchema\|StateSchema]] |
| `projection.rs` | Program slicing/projection |
| `differentiate.rs` | Automatic differentiation and [[DiffGraph\|DiffGraph]] |
| `provenance.rs` | [[ExecutionTrace\|ExecutionTrace]] and data lineage tracking |
| `heap.rs` | Garbage-collected heap for strings, lists, maps |
| `wasm.rs` | WebAssembly FFI layer |

---

## Topics

Detailed guides on specific functionality:

- [[Setup|Setup]] - How to set up an Env and get started
- [[CodeManipulation|Code Manipulation]] - How to modify a compiled program
- [[Execution|Execution]] - How to run a program with the interpreter
- [[LiveEditing|Live Editing]] - How to transfer state from a running program to a modified program
- [[Backflow|Backflow]] - How to use differentiation to apply signals & goals
- [[Projection|Projection]] - How to create and use a projection
- [[Heap|Heap]] - Garbage-collected heap for runtime values
- [[WebAssembly|WebAssembly]] - More about the WASM API

---

## Data Structures

Key data structures are documented in the [[data_structures]] folder:

- [[Env|Env]] - The foundational environment, owns programs and stacks
- [[Program|Program]] - A block of code as a collection of terms
- [[Term|Term]] - A single expression/node in the program graph
- [[Stack|Stack]] - Runtime evaluation context
- [[Value|Value]] - Runtime representation of data
- [[SourceMap|SourceMap]] - Maps terms to source locations
- [[TypeSystem|TypeSystem]] - TypeScript-inspired type system
- [[ExecutionTrace|ExecutionTrace]] - For provenance and debugging
- [[DiffGraph|DiffGraph]] - For automatic differentiation
- [[StateSchema|StateSchema]] - For state reconciliation during live editing

---

## Public API

### Core Operations

```rust
impl Env {
    /// Create a new environment
    pub fn new() -> Self;

    /// Load a program from source
    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey, ParseError>;

    /// Create a new execution stack for a program
    pub fn create_stack(&mut self, program_id: ProgramKey) -> Result<StackKey, Error>;

    /// Run one step of execution
    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult, Error>;

    /// Run until completion or breakpoint
    pub fn run(&mut self, stack_id: StackKey) -> Result<Value, Error>;

    /// Get the current value of a term (for inspection)
    pub fn get_term_value(&self, stack_id: StackKey, term_id: TermId) -> Option<&Value>;

    /// Get provenance: what terms influenced this term?
    pub fn get_provenance(&self, program_id: ProgramKey, term_id: TermId) -> Vec<TermId>;
}

pub enum StepResult {
    Continue,
    Complete(Value),
    Breakpoint(TermId),
    Error(Error),
}
```

### Live Editing

```rust
impl Env {
    /// Apply a source edit while a stack is running
    pub fn live_edit(
        &mut self,
        program_id: ProgramKey,
        edit: SourceEdit
    ) -> Result<LiveEditResult, Error>;

    /// Reconcile state after a live edit
    pub fn reconcile_state(
        &mut self,
        stack_id: StackKey
    ) -> Result<StateReconciliation, Error>;
}

pub struct SourceEdit {
    pub range: SourceRange,
    pub new_text: String,
}

pub struct LiveEditResult {
    pub added_terms: Vec<TermId>,
    pub removed_terms: Vec<TermId>,
    pub modified_terms: Vec<TermId>,
}

pub struct StateReconciliation {
    pub preserved: Vec<StateKey>,
    pub initialized: Vec<StateKey>,
    pub removed: Vec<StateKey>,
}
```

### Projection

```rust
impl Env {
    /// Create a projection (slice) of a program
    pub fn project(
        &self,
        program_id: ProgramKey,
        focus: ProjectionFocus,
    ) -> Result<Projection, Error>;
}

pub enum ProjectionFocus {
    /// Forward slice: what does this term influence?
    Forward(TermId),

    /// Backward slice: what influences this term?
    Backward(TermId),

    /// Dynamic slice: what was active for this execution?
    Dynamic { stack_id: StackKey, target_term: TermId },
}

pub struct Projection {
    /// The subset of terms included in this projection
    pub included_terms: HashSet<TermId>,

    /// Simplified dataflow graph
    pub dataflow_edges: Vec<(TermId, TermId)>,
}
```

### Differentiation

```rust
impl Env {
    /// Compute gradients via backpropagation
    pub fn backpropagate(
        &mut self,
        stack_id: StackKey,
        output_term: TermId,
        target_gradient: Value,
    ) -> Result<Gradients, Error>;
}

pub struct Gradients {
    /// Maps term ID to its gradient value
    pub term_gradients: HashMap<TermId, Value>,
}
```

---

## Foreign API Layer (WebAssembly)

For WebAssembly interop, we use numeric object IDs rather than direct references. A dedicated `wasm.rs` module manages the object registry.

### wasm.rs - Object Registry

```rust
// wasm.rs

use std::cell::RefCell;
use std::collections::HashMap;

/// Opaque handle types for FFI
pub type EnvHandle = u32;
pub type ProgramHandle = u32;
pub type StackHandle = u32;
pub type TermHandle = u32;

/// Thread-local registry (single-threaded WASM environment)
thread_local! {
    static REGISTRY: RefCell<ObjectRegistry> = RefCell::new(ObjectRegistry::new());
}

struct ObjectRegistry {
    envs: HashMap<EnvHandle, Env>,
    next_handle: u32,
}

impl ObjectRegistry {
    fn new() -> Self {
        Self {
            envs: HashMap::new(),
            next_handle: 1, // 0 reserved for null/error
        }
    }

    fn insert_env(&mut self, env: Env) -> EnvHandle {
        let handle = self.next_handle;
        self.next_handle += 1;
        self.envs.insert(handle, env);
        handle
    }

    fn get_env(&self, handle: EnvHandle) -> Option<&Env> {
        self.envs.get(&handle)
    }

    fn get_env_mut(&mut self, handle: EnvHandle) -> Option<&mut Env> {
        self.envs.get_mut(&handle)
    }

    fn remove_env(&mut self, handle: EnvHandle) -> Option<Env> {
        self.envs.remove(&handle)
    }
}
```

### Exported FFI Functions

```rust
// wasm.rs (continued)

/// Create a new Petal environment
/// Returns: EnvHandle (0 on error)
#[no_mangle]
pub extern "C" fn petal_create_env() -> EnvHandle {
    REGISTRY.with(|registry| {
        let env = Env::new();
        registry.borrow_mut().insert_env(env)
    })
}

/// Destroy an environment and free its resources
/// Returns: 1 on success, 0 on error
#[no_mangle]
pub extern "C" fn petal_destroy_env(env: EnvHandle) -> u32 {
    REGISTRY.with(|registry| {
        match registry.borrow_mut().remove_env(env) {
            Some(_) => 1,
            None => 0,
        }
    })
}

/// Load a program from source code
/// Returns: ProgramHandle (0 on error)
#[no_mangle]
pub extern "C" fn petal_load_program(
    env: EnvHandle,
    source_ptr: *const u8,
    source_len: u32,
) -> ProgramHandle {
    REGISTRY.with(|registry| {
        let mut reg = registry.borrow_mut();
        let env = match reg.get_env_mut(env) {
            Some(e) => e,
            None => return 0,
        };

        let source = unsafe {
            std::str::from_utf8_unchecked(
                std::slice::from_raw_parts(source_ptr, source_len as usize)
            )
        };

        match env.load_program(source) {
            Ok(program_key) => program_key.0.as_ffi(),
            Err(_) => 0,
        }
    })
}

/// Create a new execution stack
/// Returns: StackHandle (0 on error)
#[no_mangle]
pub extern "C" fn petal_create_stack(
    env: EnvHandle,
    program: ProgramHandle,
) -> StackHandle {
    REGISTRY.with(|registry| {
        let mut reg = registry.borrow_mut();
        let env = match reg.get_env_mut(env) {
            Some(e) => e,
            None => return 0,
        };

        match env.create_stack(ProgramKey::from_ffi(program)) {
            Ok(stack_key) => stack_key.0.as_ffi(),
            Err(_) => 0,
        }
    })
}

/// Destroy a stack
/// Returns: 1 on success, 0 on error
#[no_mangle]
pub extern "C" fn petal_destroy_stack(
    env: EnvHandle,
    stack: StackHandle,
) -> u32 {
    REGISTRY.with(|registry| {
        let mut reg = registry.borrow_mut();
        let env = match reg.get_env_mut(env) {
            Some(e) => e,
            None => return 0,
        };

        env.destroy_stack(StackKey::from_ffi(stack));
        1
    })
}

/// Execute one step
/// Returns: 0 = continue, 1 = complete, 2 = error, 3 = breakpoint
#[no_mangle]
pub extern "C" fn petal_step(
    env: EnvHandle,
    stack: StackHandle,
) -> u32 {
    REGISTRY.with(|registry| {
        let mut reg = registry.borrow_mut();
        let env = match reg.get_env_mut(env) {
            Some(e) => e,
            None => return 2,
        };

        match env.step(StackKey::from_ffi(stack)) {
            Ok(StepResult::Continue) => 0,
            Ok(StepResult::Complete(_)) => 1,
            Ok(StepResult::Breakpoint(_)) => 3,
            Err(_) => 2,
        }
    })
}

/// Run until completion
/// Returns: 1 on success, 0 on error
#[no_mangle]
pub extern "C" fn petal_run(
    env: EnvHandle,
    stack: StackHandle,
) -> u32 {
    REGISTRY.with(|registry| {
        let mut reg = registry.borrow_mut();
        let env = match reg.get_env_mut(env) {
            Some(e) => e,
            None => return 0,
        };

        match env.run(StackKey::from_ffi(stack)) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    })
}

/// Get the last error message
/// Writes to the provided buffer, returns length written
#[no_mangle]
pub extern "C" fn petal_get_error(
    env: EnvHandle,
    buffer_ptr: *mut u8,
    buffer_len: u32,
) -> u32 {
    // Implementation would copy error message to buffer
    0
}

/// Apply a live edit to a running program
#[no_mangle]
pub extern "C" fn petal_live_edit(
    env: EnvHandle,
    program: ProgramHandle,
    edit_json_ptr: *const u8,
    edit_json_len: u32,
) -> u32 {
    // Parse JSON edit description and apply
    // Returns 1 on success, 0 on error
    1
}
```

### Memory Management for Strings/Data

```rust
/// Allocate memory for passing data into Wasm
#[no_mangle]
pub extern "C" fn petal_alloc(size: u32) -> *mut u8 {
    let layout = std::alloc::Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { std::alloc::alloc(layout) }
}

/// Free memory allocated by petal_alloc
#[no_mangle]
pub extern "C" fn petal_free(ptr: *mut u8, size: u32) {
    let layout = std::alloc::Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { std::alloc::dealloc(ptr, layout) }
}

/// Get a result value as JSON
/// Returns pointer to JSON string (caller must free)
#[no_mangle]
pub extern "C" fn petal_get_result_json(
    env: EnvHandle,
    stack: StackHandle,
    out_len: *mut u32,
) -> *mut u8 {
    // Serialize result to JSON and return pointer
    std::ptr::null_mut()
}
```

---

## Further Thoughts

### Rust-Friendly Design Patterns

**Arena Allocation**

Arena allocators (`typed-arena`, `bumpalo`) provide better cache locality and simpler lifetime management. However, arenas alone don't provide ID-based lookup—you'd still need a separate index structure (e.g., `HashMap<TermId, &Term>` or storing arena indices). For our use case, `SlotMap` provides both allocation and indexing, making it the better choice.

**ECS-Style Architecture**

For future consideration: storing term properties in separate vectors (struct-of-arrays) can improve cache utilization when iterating over specific properties. This could be beneficial if we find hot loops that only access one property (e.g., iterating all `control_flow_next` links).

---

### Considerations for Goals

**For Dataflow/Provenance:**
- Store input edges explicitly on each term
- Consider a separate "provenance mode" that records full execution traces
- May want sparse vs. dense provenance tracking options

**For Live Editing:**
- Term IDs must be stable across edits (use UUIDs or content-addressed hashes)
- Need efficient diffing between old and new program versions
- State migration may need user-defined transformers for complex cases

**For Projection:**
- Pre-compute dependency graphs for fast slicing
- Consider incremental updates to projections as the program changes
- May want "projection templates" for common views

**For Differentiation:**
- Not all operations are differentiable - need graceful fallbacks
- Consider forward-mode vs reverse-mode AD based on input/output dimensions
- May want symbolic differentiation for simple cases

