# ExecutionTrace

For provenance and debugging.

## Related Data Structures

- [[Term]] - Traces record term execution
- [[Value]] - Traces record input/output values
- [[Stack]] - Execution context that produces traces

## Definition

```rust
pub struct ExecutionTrace {
    steps: Vec<TraceStep>,
}

pub struct TraceStep {
    term: TermId,
    inputs: Vec<Value>,
    output: Value,
    timestamp: u64,
}
```

## Key Features

- Records each step of program execution
- Captures inputs and outputs for each term
- Timestamps enable temporal analysis
- Essential for provenance tracking and debugging

---

See also: [[Outline|Implementation Plan]]
