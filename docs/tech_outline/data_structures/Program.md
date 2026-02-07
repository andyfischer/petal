# Program

A block of code represented as a collection of [[Term]] objects organized into [[Block|Blocks]].

## Related Data Structures

- [[Env]] - The environment that owns programs
- [[Term]] - Individual expressions/nodes in the program
- [[Block]] - Control flow blocks that group terms
- [[ConstantTable]] - Stores literal values
- [[SourceMap]] - Maps terms to source locations

## Definition

```rust
pub struct Program {
    pub id: ProgramId,

    /// All terms in this program, indexed by TermId
    terms: Vec<Term>,

    /// All blocks in this program, indexed by BlockId
    blocks: Vec<Block>,

    /// The root block (entry point for the program)
    root_block: BlockId,

    /// Constant value table for literals and fixed data
    constants: ConstantTable,

    /// Metadata for live editing support
    source_map: SourceMap,

    /// Whether this program contains any parse errors
    has_errors: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramId(pub u32);
```

## Blocks

Programs are organized into [[Block|Blocks]] that represent control flow scopes. Each term belongs to exactly one block, and blocks form a tree structure via their `parent_term_id` field.

See [[Block]] for details on block structure and [[NameLookup]] for how blocks enable lexical scoping.

## Parse Errors

When parsing fails, the parser still produces a Program, but includes **error terms** at the locations where parsing failed:

```rust
pub enum TermOp {
    /// A parse error - contains the error message as a constant
    Error(ConstantId),
    // ...other ops
}
```

This allows:
- Partial programs to be inspected and even partially executed
- IDE features (highlighting, completion) to work on invalid programs
- Error messages to have source locations via the [[SourceMap]]

The `has_errors` flag indicates whether the program contains any error terms.

## Key Properties

- Terms are stored in a flat array for cache-friendly access
- Blocks provide hierarchical scoping for control flow constructs
- Constants are deduplicated in the [[ConstantTable|constant table]]
- Parse errors become terms, allowing partial program analysis
- [[SourceMap]] tracks correspondence between terms and source locations

---

See also: [[Outline|Implementation Plan]]
