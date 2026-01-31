# SourceMap

Maps terms to source locations for error reporting and live editing.

## Related Data Structures

- [[Program]] - Contains the source map
- [[Term]] - Terms mapped to source locations
- [[StateSchema]] - Uses source locations for debugging

## Definition

```rust
pub struct SourceMap {
    term_spans: HashMap<TermId, SourceSpan>,
    // Reverse mapping for "what term is at this position?"
    position_index: IntervalTree<TermId>,
}

pub struct SourceSpan {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

pub struct SourcePosition {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}
```

## Key Features

- Forward mapping: term ID → source span
- Reverse mapping: source position → term ID (via interval tree)
- Essential for live editing and error reporting

---

See also: [[Outline|Implementation Plan]]
