# ConstantTable

A table for storing literal values used by a [[Program]].

## Related Data Structures

- [[Program]] - Contains a ConstantTable
- [[Term]] - Terms reference constants via `ConstantId`

## Definition

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

## Usage

Terms reference constants via `ConstantId`:

```rust
pub enum TermOp {
    /// Load a constant from the constant table
    Constant(ConstantId),
    // ...other ops
}
```

## Key Properties

- Constants are deduplicated automatically on insertion
- `ConstantId` is a compact numeric reference
- Float values are stored as bit patterns to allow `Eq` and `Hash` implementations

---

See also: [[Outline|Implementation Plan]]
