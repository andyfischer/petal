# Stack

The runtime evaluation context.

## Related Data Structures

- [[Env]] - Owns the stacks
- [[Program]] - The program being executed
- [[Block]] - Frames correspond to blocks
- [[Term]] - Terms being evaluated
- [[Value]] - Values stored in registers and state

## Definition

```rust
pub struct Stack {
    pub id: StackKey,

    /// The program being executed
    pub program_id: ProgramId,

    /// Stack of activation frames
    frames: Vec<Frame>,
}

pub struct Frame {
    /// The block this frame is executing
    pub block_id: BlockId,

    /// Current term being executed within the block
    pub current_term: TermId,

    /// Register file for this frame
    pub registers: Vec<Value>,

    /// Return address (term to jump to when this frame completes)
    pub return_term: Option<TermId>,
}

```

## Frames and Blocks

Each [[Frame]] corresponds to a [[Block]] being executed. When control flow enters a nested block (e.g., the body of an if-expression), a new frame is pushed onto the stack. When the block completes, the frame is popped.

This means:
- The root block gets the initial frame
- Entering an if-body, loop-body, or function pushes a new frame
- Exiting returns to the previous frame
- The `block_id` on each frame identifies which block's terms it is executing

## Key Behaviors

- Each frame has its own register file
- Each frame tracks which block it is executing via `block_id`
- State storage persists across invocations (for inline state)
- Loop context enables per-iteration state keying (see [[StateSchema]])

---

See also: [[Outline|Implementation Plan]]
