# Block

A control flow block within a [[Program]]. Blocks group terms that execute together under the same control flow scope.

## Related Data Structures

- [[Program]] - Contains a list of Blocks
- [[Term]] - Each term belongs to exactly one Block
- [[Stack]] - Frames track the current block

## Definition

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

pub struct Block {
    pub id: BlockId,

    /// The term that creates this block's scope (e.g., an if-expression).
    /// None for the root block.
    pub parent_term_id: Option<TermId>,

    /// Entry point for this block's term list
    pub entry: TermId,
}
```

## Purpose

Blocks represent control flow scopes in the program:

- **Root block**: The top-level scope of the program. Has no `parent_term_id`.
- **Nested blocks**: Created by control flow constructs like `if`, `while`, `for`, or function bodies. The `parent_term_id` points to the term that introduces the scope.

For example, in this code:

```
let x = 1
if condition {
    let y = 2    // These terms are in a nested block
    let z = y + 1
}
let w = x + 1
```

There are two blocks:
1. **Root block** containing: `x = 1`, `if condition {...}`, `w = x + 1`
2. **If-block** containing: `y = 2`, `z = y + 1` (with `parent_term_id` pointing to the `if` term)

## Block Nesting

The `parent_term_id` creates a hierarchical nesting structure:

```
Root Block (parent_term_id: None)
├── term: let x = 1
├── term: if condition { ... }  <-- This term is the parent_term_id for the if-block
│   └── If Block (parent_term_id: if-term)
│       ├── term: let y = 2
│       └── term: let z = y + 1
└── term: let w = x + 1
```

This structure is essential for:
- **[[NameLookup|Name lookup]]**: Finding variables in scope by walking up the block hierarchy
- **Scope analysis**: Determining which terms are visible at any point
- **Control flow analysis**: Understanding branch structure

## Terms Within Blocks

Each [[Term]] has a `block_id` field indicating which block it belongs to. Terms within a block form a linked list via `block_next` and `block_prev` fields, establishing their execution order within that block.

---

See also: [[Outline|Implementation Plan]]
