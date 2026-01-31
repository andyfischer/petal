# Backflow

How to use differentiation to apply signals and goals to the output of a program.

## Related Topics

- [[Execution]] - Forward execution produces values
- [[Projection]] - Slicing to find relevant terms

## Overview

Backflow (automatic differentiation) lets you compute how changes to inputs would affect outputs. This enables:

- **Optimization**: Adjust parameters to minimize/maximize an objective
- **Sensitivity analysis**: Which inputs matter most for the output?
- **Goal propagation**: Given a desired output, what inputs would achieve it?

## Basic Backpropagation

```rust
// Forward pass: run the program
env.run(stack)?;

// Backward pass: compute gradients
let gradients = env.backpropagate(
    stack,
    output_term,           // The term whose gradient we're computing from
    Value::Float(1.0),     // The "seed" gradient (usually 1.0)
)?;

// Inspect gradients
for (term_id, gradient) in &gradients.term_gradients {
    println!("d(output)/d(term {:?}) = {:?}", term_id, gradient);
}
```

## The DiffGraph

The [[DiffGraph|DiffGraph]] maps forward operations to their backward counterparts:

```rust
pub struct DiffGraph {
    backward_ops: HashMap<TermId, Vec<BackwardOp>>,
}

pub struct BackwardOp {
    input_index: usize,      // Which input's gradient
    gradient_fn: GradientFn, // How to compute it
}
```

## Differentiable Operations

Not all operations support differentiation:

| Operation | Differentiable | Notes |
|-----------|---------------|-------|
| Add, Sub  | Yes | Linear: gradient passes through |
| Mul       | Yes | Product rule |
| Div       | Yes | Quotient rule |
| Pow       | Yes | Power rule |
| Branch    | Partial | Gradient flows through taken branch only |
| Loop      | Partial | Unrolled differentiation |
| StateRead | No | State is treated as constant |

## Applying Gradients

Use gradients to update values:

```rust
let learning_rate = 0.01;

for (term_id, gradient) in &gradients.term_gradients {
    if let Some(Value::Float(g)) = gradient.as_float() {
        // Get current parameter value
        let current = env.get_term_value(stack, *term_id)?;

        if let Value::Float(v) = current {
            // Update: new = old - learning_rate * gradient
            let new_value = v - learning_rate * g;
            env.set_term_value(stack, *term_id, Value::Float(new_value))?;
        }
    }
}
```

## Goal-Directed Execution

Given a target output, find inputs that produce it:

```rust
// Define the goal
let target_output = Value::Float(100.0);

// Iteratively adjust inputs
for _ in 0..1000 {
    // Forward pass
    env.reset_stack(stack)?;
    env.run(stack)?;

    let actual = env.get_term_value(stack, output_term)?;

    // Compute error
    let error = compute_error(&actual, &target_output);
    if error < 0.001 {
        break; // Close enough
    }

    // Backward pass with error as seed
    let gradients = env.backpropagate(stack, output_term, error_gradient)?;

    // Update input terms
    apply_gradients(&mut env, stack, &gradients, learning_rate)?;
}
```

## Forward vs Reverse Mode

Petal primarily uses **reverse mode** (backpropagation):

- **Reverse mode**: Efficient when outputs << inputs (common case)
- **Forward mode**: Efficient when inputs << outputs

For cases with few inputs and many outputs, request forward mode:

```rust
let gradients = env.forward_differentiate(
    stack,
    input_term,
    Value::Float(1.0), // Perturbation
)?;
```

## Symbolic Differentiation

For simple expressions, Petal can compute symbolic derivatives:

```rust
// Get the derivative as a new program
let derivative_program = env.symbolic_derivative(
    program,
    with_respect_to: term_id,
)?;

// The derivative program computes d(output)/d(term)
let derivative_stack = env.create_stack(derivative_program)?;
env.run(derivative_stack)?;
```

---

See also: [[Outline|Implementation Plan]]
