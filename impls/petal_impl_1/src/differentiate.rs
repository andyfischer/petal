//! Automatic differentiation (backpropagation)
//!
//! Implements reverse-mode automatic differentiation for computing gradients
//! through the computation graph.

use std::collections::HashMap;

use crate::program::{TermId, TermOp};
use crate::provenance::ExecutionTrace;
use crate::value::Value;

/// Gradient values for each term
#[derive(Debug, Clone, Default)]
pub struct Gradients {
    /// Maps term ID to its gradient value
    pub term_gradients: HashMap<TermId, f64>,
}

impl Gradients {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, term_id: TermId, gradient: f64) {
        self.term_gradients.insert(term_id, gradient);
    }

    pub fn get(&self, term_id: TermId) -> f64 {
        self.term_gradients.get(&term_id).copied().unwrap_or(0.0)
    }

    pub fn accumulate(&mut self, term_id: TermId, gradient: f64) {
        let current = self.get(term_id);
        self.set(term_id, current + gradient);
    }
}

/// Backward operation for computing gradients
#[derive(Debug, Clone)]
pub struct BackwardOp {
    /// The forward term this operates on
    pub forward_term: TermId,
    /// The operation type
    pub op: TermOp,
    /// Input term IDs
    pub inputs: Vec<TermId>,
    /// Forward input values (needed for some operations)
    pub input_values: Vec<f64>,
    /// Forward output value
    pub output_value: f64,
}

/// Differentiation graph for backpropagation
#[derive(Debug, Clone, Default)]
pub struct DiffGraph {
    /// Backward operations in reverse order
    backward_ops: Vec<BackwardOp>,
}

impl DiffGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a diff graph from an execution trace
    pub fn from_trace(trace: &ExecutionTrace) -> Self {
        let mut diff_graph = Self::new();

        for step in trace.all_steps() {
            let input_values: Vec<f64> = Vec::new(); // Would need to look up from trace

            // Get the output value as f64
            let output_value = match &step.output {
                Value::Int(n) => *n as f64,
                Value::Float(f) => *f,
                _ => 0.0,
            };

            diff_graph.backward_ops.push(BackwardOp {
                forward_term: step.term_id,
                op: TermOp::Constant(crate::program::ConstantId(0)), // Placeholder
                inputs: step.inputs.clone(),
                input_values,
                output_value,
            });
        }

        diff_graph
    }

    /// Add a backward operation
    pub fn add_op(&mut self, op: BackwardOp) {
        self.backward_ops.push(op);
    }
}

/// Compute gradients via backpropagation
pub fn backpropagate(
    trace: &ExecutionTrace,
    output_term: TermId,
    seed_gradient: f64,
) -> Gradients {
    let mut gradients = Gradients::new();

    // Set the seed gradient for the output
    gradients.set(output_term, seed_gradient);

    // Process trace steps in reverse order
    let steps: Vec<_> = trace.all_steps().iter().rev().collect();

    for step in steps {
        let output_grad = gradients.get(step.term_id);

        if output_grad == 0.0 {
            continue; // No gradient flowing through this term
        }

        // Compute gradients for inputs based on operation type
        let input_values: Vec<f64> = step
            .inputs
            .iter()
            .map(|&input_id| {
                // Find the step that produced this input
                trace
                    .get_last_step_for_term(input_id)
                    .map(|s| value_to_f64(&s.output))
                    .unwrap_or(0.0)
            })
            .collect();

        // Apply chain rule based on the operation
        // For now, we use heuristics based on the number of inputs
        match input_values.len() {
            0 => {
                // Constant - no inputs to propagate to
            }
            1 => {
                // Unary operation
                // Default: pass gradient through unchanged
                gradients.accumulate(step.inputs[0], output_grad);
            }
            2 => {
                // Binary operation
                // Default: treat as addition (gradient passes to both)
                let a = input_values[0];
                let b = input_values[1];
                let output = value_to_f64(&step.output);

                // Heuristic detection of operation type
                if (a + b - output).abs() < 1e-10 {
                    // Addition: d/da = 1, d/db = 1
                    gradients.accumulate(step.inputs[0], output_grad);
                    gradients.accumulate(step.inputs[1], output_grad);
                } else if (a - b - output).abs() < 1e-10 {
                    // Subtraction: d/da = 1, d/db = -1
                    gradients.accumulate(step.inputs[0], output_grad);
                    gradients.accumulate(step.inputs[1], -output_grad);
                } else if (a * b - output).abs() < 1e-10 {
                    // Multiplication: d/da = b, d/db = a
                    gradients.accumulate(step.inputs[0], output_grad * b);
                    gradients.accumulate(step.inputs[1], output_grad * a);
                } else if b != 0.0 && (a / b - output).abs() < 1e-10 {
                    // Division: d/da = 1/b, d/db = -a/b^2
                    gradients.accumulate(step.inputs[0], output_grad / b);
                    gradients.accumulate(step.inputs[1], output_grad * (-a / (b * b)));
                } else {
                    // Unknown operation, assume addition-like
                    gradients.accumulate(step.inputs[0], output_grad);
                    gradients.accumulate(step.inputs[1], output_grad);
                }
            }
            _ => {
                // Multi-input operation, propagate equally
                for &input_id in &step.inputs {
                    gradients.accumulate(input_id, output_grad);
                }
            }
        }
    }

    gradients
}

fn value_to_f64(value: &Value) -> f64 {
    match value {
        Value::Int(n) => *n as f64,
        Value::Float(f) => *f,
        _ => 0.0,
    }
}

/// Compute numerical gradient for verification
pub fn numerical_gradient<F>(f: F, x: f64, epsilon: f64) -> f64
where
    F: Fn(f64) -> f64,
{
    (f(x + epsilon) - f(x - epsilon)) / (2.0 * epsilon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gradients() {
        let mut gradients = Gradients::new();
        gradients.set(TermId(0), 1.0);
        gradients.accumulate(TermId(0), 0.5);
        assert!((gradients.get(TermId(0)) - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_numerical_gradient() {
        let f = |x: f64| x * x; // f(x) = x^2, f'(x) = 2x
        let grad = numerical_gradient(f, 3.0, 1e-6);
        assert!((grad - 6.0).abs() < 1e-4); // f'(3) = 6
    }
}
