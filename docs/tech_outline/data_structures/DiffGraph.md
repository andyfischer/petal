# DiffGraph

For automatic differentiation.

## Related Data Structures

- [[Term]] - Forward terms have corresponding backward operations
- [[Value]] - Gradients are computed as values

## Definition

```rust
pub struct DiffGraph {
    /// Maps forward term -> list of backward ops
    backward_ops: HashMap<TermId, Vec<BackwardOp>>,
}

pub struct BackwardOp {
    /// Which input's gradient this computes
    input_index: usize,
    /// The operation to compute the gradient
    gradient_fn: GradientFn,
}
```

## Key Features

- Maps forward computation to backward gradient computation
- Each term can have multiple backward ops (one per differentiable input)
- Enables reverse-mode automatic differentiation (backpropagation)

---

See also: [[Outline|Implementation Plan]]
