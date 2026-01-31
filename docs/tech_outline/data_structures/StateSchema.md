# StateSchema

For state reconciliation during live editing. State is nested and recursive (like React's useState nested in subcomponents).

## Related Data Structures

- [[Term]] - State nodes reference terms
- [[Stack]] - State storage lives in stacks
- [[SourceMap]] - Source locations for debugging
- [[TypeSystem]] - State nodes have type information

## Definition

```rust
/// Schema describing the shape of program state
pub struct StateSchema {
    /// Root-level state declarations
    root: Vec<StateNode>,
}

/// A node in the state tree (recursive structure)
pub struct StateNode {
    /// The term that declares this state
    pub term_id: TermId,

    /// Name of the state variable (for matching when program changes)
    pub name: String,

    /// Type information for this state
    pub type_info: Type,

    /// Source location for debugging
    pub source_location: SourceSpan,

    /// Child state nodes (for nested components/functions)
    pub children: Vec<StateNode>,

    /// For loop state: the iteration key pattern
    pub iteration_key: Option<IterationKey>,
}

/// How loop iterations are keyed for state matching
pub enum IterationKey {
    /// Simple index-based (state resets if loop bounds change)
    Index,
    /// Key expression (state follows the key across reorderings)
    Expression(TermId),
}
```

## State Reconciliation

```rust
impl StateSchema {
    /// Build schema from a program
    pub fn from_program(program: &Program) -> Self;

    /// Match old state to new schema after a live edit
    pub fn reconcile(
        &self,
        old_schema: &StateSchema,
        old_state: &HashMap<StateKey, Value>,
    ) -> StateReconciliation;
}

/// Result of reconciling state between schema versions
pub struct StateReconciliation {
    /// State that transferred directly (same term_id or matched by name)
    pub preserved: HashMap<StateKey, Value>,

    /// New state nodes that need initialization
    pub needs_init: Vec<StateKey>,

    /// Old state that has no match in new schema
    pub orphaned: Vec<StateKey>,
}
```

## Key Features

- Recursive tree structure mirrors component/function nesting
- State matching by name enables preservation across refactoring
- Iteration keys allow state to follow data through reorderings

---

See also: [[Outline|Implementation Plan]]
