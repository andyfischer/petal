# Env (Environment)

The foundational data structure used throughout the Petal runtime. Most operations require an `Env` as their first parameter.

## Related Data Structures

- [[Program]] - Programs owned by the Env
- [[Stack]] - Execution stacks owned by the Env
- [[Value]] - Runtime values produced during execution
- [[Heap|Heap]] - Garbage-collected heap owned by the Env

## Definition

```rust
use slotmap::{SlotMap, new_key_type};

new_key_type! {
    pub struct ProgramId;
    pub struct StackKey;
}

pub struct Env {
    /// Programs stored with generational indices for safe access
    programs: SlotMap<ProgramId, Program>,

    /// Stacks stored with generational indices for safe access
    stacks: SlotMap<StackKey, Stack>,

    /// Garbage-collected heap for strings, lists, maps
    heap: Heap,

    /// Function registry (builtins and user-defined)
    functions: FunctionTable,

    /// Global configuration and settings
    globals: Globals,
}

pub struct FunctionTable {
    /// Built-in functions
    builtins: HashMap<String, BuiltinFn>,
    /// User-defined functions (maps to program that defines the function)
    user_functions: HashMap<FunctionId, ProgramId>,
}

pub struct Globals {
    /// Default values, builtins, etc.
}
```

## Responsibilities

- Owns all [[Program]] and [[Stack]] instances via `SlotMap` (generational indices catch use-after-free bugs)
- Owns the [[Heap|Heap]] for garbage-collected values (strings, lists, maps)
- Provides safe ID-based lookup for cross-structure references
- Manages function registry for both builtins and user-defined functions
- Manages global state and configuration

---

See also: [[Outline|Implementation Plan]]
