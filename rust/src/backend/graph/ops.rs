//! Arithmetic and comparison dispatch for the graph engine.
//!
//! The actual value-level semantics (Int/Float/dual/vec2 math, checked integer
//! ops, value ordering) live in [`super::super::ops`] so they are shared
//! verbatim with the bytecode VM. These methods only adapt that shared logic to
//! the evaluator's `ControlFlow` protocol.

use super::*;
use crate::backend::ops;

impl<'a> Evaluator<'a> {
    /// Arithmetic on Int/Float/dual/vec2 operands (delegates to `ops::arithmetic`).
    pub(super) fn numeric_binop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if inputs.len() < 2 {
            return ControlFlow::Error(format!(
                "{:?} expects 2 operands, got {}",
                term.op,
                inputs.len()
            ));
        }
        match ops::arithmetic(&term.op, inputs[0], inputs[1], self.heap) {
            Ok(val) => self.produce(term, val),
            Err(e) => ControlFlow::Error(e),
        }
    }

    /// Lt / Le / Gt / Ge (delegates to `ops::comparison`).
    pub(super) fn comparison_op(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        match ops::comparison(&term.op, inputs[0], inputs[1], self.heap) {
            Ok(val) => self.produce(term, val),
            Err(e) => ControlFlow::Error(e),
        }
    }
}
