# Program

A block of code represented as a collection of [[Term]] objects.

## Related Data Structures

- [[Env]] - The environment that owns programs
- [[Term]] - Individual expressions/nodes in the program
- [[SourceMap]] - Maps terms to source locations

## Definition

```rust
pub struct Program {
    pub id: ProgramKey,

    /// All terms in this program, indexed by TermId
    terms: Vec<Term>,

    /// Entry point for the control flow list
    entry: TermId,

    /// Constant value table for literals and fixed data
    constants: ConstantTable,

    /// Metadata for live editing support
    source_map: SourceMap,

    /// Whether this program contains any parse errors
    has_errors: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TermId(pub u32);
```

## Constant Table

Programs store literal values in a constant table rather than embedding them directly in terms. This enables:
- Deduplication of identical constants
- Efficient storage for large strings
- Simpler term representation

```rust
pub struct ConstantTable {
    values: Vec<ConstantValue>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstantId(pub u32);

pub enum ConstantValue {
    Int(i64),
    Float(u64),  // Stored as bits for Eq/Hash
    String(String),
    // Future: large literals, regex patterns, etc.
}
```

Terms reference constants via `ConstantId`:

```rust
pub enum TermOp {
    /// Load a constant from the constant table
    Constant(ConstantId),
    // ...other ops
}
```

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
- Constants are deduplicated in the constant table
- Parse errors become terms, allowing partial program analysis
- [[SourceMap]] tracks correspondence between terms and source locations

---

See also: [[Outline|Implementation Plan]]
