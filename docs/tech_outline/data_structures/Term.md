# Term

A single expression/node in the program graph.

## Related Data Structures

- [[Program]] - Contains the collection of terms
- [[Block]] - Each term belongs to exactly one block
- [[Value]] - Runtime values produced by term evaluation
- [[Stack]] - Execution context that evaluates terms

## Term IDs

Terms have two forms of identification:

**Local ID (`TermId`)** - A numeric index unique within a single [[Program]]. This is compact and used for all intra-program references (inputs, control flow links, etc.).

**Global ID (`GlobalTermId`)** - Combines the `ProgramId` with the local `TermId`. This is unique within an [[Env]] and used when referencing terms across program boundaries.

```rust
/// Local term identifier - unique within a Program
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);

/// Global term identifier - unique within an Env
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalTermId {
    pub program: ProgramId,
    pub term: TermId,
}
```


## Definition

```rust
pub struct Term {
    pub id: TermId,

    /// The operation this term performs
    pub op: TermOp,

    /// Input terms (dataflow edges)
    pub inputs: SmallVec<[TermId; 4]>,

    /// The block this term belongs to
    pub block_id: BlockId,

    /// Linked list ordering within the block
    pub block_next: Option<TermId>,
    pub block_prev: Option<TermId>,

    /// Optional name for binding terms (e.g., variable declarations)
    pub name: Option<String>,

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

## Dataflow vs Block Ordering

Terms participate in two graphs:

**Dataflow Graph** - The `inputs` field connects terms by data dependency. A term's inputs are the terms whose values it consumes. This graph is acyclic for pure computation.

**Block Ordering** - The `block_next`/`block_prev` fields form a linked list of terms within a [[Block]]. This establishes the execution order for terms in the same scope. All terms in a block participate in this list.

Examples:
- `Add` term: Has `inputs` (the two operands), and block links positioning it in its block's execution order.
- `StateWrite` term: Has `inputs` (the value to write), and block links (must execute in order relative to other terms in the block).
- `Call` term: Has `inputs` (arguments), and block links (positioned in the block's execution sequence).

The interpreter walks the block's term list for execution order, evaluating dataflow dependencies on-demand as needed. When control flow branches into a nested block (e.g., entering an if-body), the interpreter creates a new [[Stack#Frame|Frame]] for that block.

## Design Notes

- `SmallVec` avoids heap allocation for common cases (most terms have 0-3 inputs)
- Block links (`block_next`/`block_prev`) establish order within a scope; nested scopes use separate [[Block|Blocks]]
- The `name` field is only set on binding terms (variable declarations, function parameters)
- `StateKey` enables state reconciliation during live editing (see [[StateSchema]])
- See [[NameLookup]] for how names are resolved using block structure

---

See also: [[Outline|Implementation Plan]]
