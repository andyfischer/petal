# Term

A single expression/node in the program graph.

## Related Data Structures

- [[Program]] - Contains the collection of terms
- [[Value]] - Runtime values produced by term evaluation
- [[Stack]] - Execution context that evaluates terms

## Term IDs

Terms have two forms of identification:

**Local ID (`TermId`)** - A numeric index unique within a single [[Program]]. This is compact and used for all intra-program references (inputs, control flow links, etc.).

**Global ID (`GlobalTermId`)** - Combines the `ProgramKey` with the local `TermId`. This is unique within an [[Env]] and used when referencing terms across program boundaries.

```rust
/// Local term identifier - unique within a Program
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);

/// Global term identifier - unique within an Env
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalTermId {
    pub program: ProgramKey,
    pub term: TermId,
}
```

There are no UUIDs. Term IDs are simple numeric indices assigned during parsing/compilation. When a program is modified via live editing, the system rebuilds the ID mapping as needed (see [[StateSchema]] for how state is reconciled).

## Definition

```rust
pub struct Term {
    pub id: TermId,

    /// The operation this term performs
    pub op: TermOp,

    /// Input terms (dataflow edges)
    pub inputs: SmallVec<[TermId; 4]>,

    /// Control flow ordering (for effectful terms only)
    pub control_flow_next: Option<TermId>,
    pub control_flow_prev: Option<TermId>,

    /// Register assignment for evaluation
    pub register: RegisterIndex,

    /// For state terms: unique identifier for state reconciliation
    pub state_key: Option<StateKey>,
}

pub struct RegisterIndex(pub u16);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateKey(pub u64);

pub enum TermOp {
    // Constants (reference into Program's constant table)
    Constant(ConstantId),

    // Parse error (message is a constant)
    Error(ConstantId),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,

    // Comparison
    Eq,
    Lt,
    Gt,

    // Control flow
    Branch { then_term: TermId, else_term: TermId },
    Jump { target: TermId },
    Return,

    // State
    StateRead,
    StateWrite,

    // Functions
    Call { function: FunctionId },

    // Data access
    GetField { field: FieldId },
    SetField { field: FieldId },

    // Heap allocation (see topics/Heap)
    AllocList,
    AllocMap,
}
```

## Dataflow vs Control Flow

Terms participate in two graphs:

**Dataflow Graph** - The `inputs` field connects terms by data dependency. A term's inputs are the terms whose values it consumes. This graph is acyclic for pure computation.

**Control Flow List** - The `control_flow_next`/`control_flow_prev` fields form a linked list of *effectful* terms that must execute in order. Pure/dataflow-only terms (like `Add`, `Mul`, literals) do not participate in this list and have `None` for these fields.

Examples:
- `Add` term: Has `inputs` (the two operands), but no control flow links. Evaluated when needed.
- `StateWrite` term: Has `inputs` (the value to write), AND control flow links (must execute in order relative to other state operations).
- `Call` term: Has `inputs` (arguments), AND control flow links (function may have effects).

The interpreter walks the control flow list for execution order, evaluating dataflow dependencies on-demand as needed.

## Design Notes

- `SmallVec` avoids heap allocation for common cases (most terms have 0-3 inputs)
- Control flow links only exist on effectful terms - pure terms are demand-driven
- `StateKey` enables state reconciliation during live editing (see [[StateSchema]])

---

See also: [[Outline|Implementation Plan]]
