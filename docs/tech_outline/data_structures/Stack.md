# Stack

The runtime evaluation context.

## Related Data Structures

- [[Env]] - Owns the stacks
- [[Program]] - The program being executed
- [[Term]] - Terms being evaluated
- [[Value]] - Values stored in registers and state

## Definition

```rust
pub struct Stack {
    pub id: StackKey,

    /// The program being executed
    pub program_id: ProgramKey,

    /// Stack of activation frames
    frames: Vec<Frame>,

    /// Persistent state storage (for `state` declarations)
    state_storage: HashMap<StateKey, Value>,
}

pub struct Frame {
    /// Current term being executed
    pub current_term: TermId,

    /// Register file for this frame
    pub registers: Vec<Value>,

    /// Return address (term to jump to when this frame completes)
    pub return_term: Option<TermId>,

    /// For loops: iteration context
    pub loop_context: Option<LoopContext>,
}

pub struct LoopContext {
    pub iteration_index: usize,
    pub state_prefix: StateKey,
}
```

## Key Behaviors

- Each frame has its own register file
- State storage persists across invocations (for inline state)
- Loop context enables per-iteration state keying (see [[StateSchema]])

---

See also: [[Outline|Implementation Plan]]
