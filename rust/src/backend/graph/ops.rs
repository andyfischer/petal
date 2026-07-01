//! Arithmetic and comparison operators, including forward-mode AD (dual
//! numbers) and vec2 component-wise math.

use super::*;
use crate::builtins;

impl<'a> Evaluator<'a> {
    /// Arithmetic on Int/Float pairs. Dual-number and Vec2 operands are
    /// delegated to their own handlers.
    pub(super) fn numeric_binop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        if inputs.len() < 2 {
            return ControlFlow::Error(format!(
                "{} expects 2 operands, got {}",
                binop_verb(&term.op),
                inputs.len()
            ));
        }
        if matches!(term.op, TermOp::Div) {
            match &inputs[1] {
                Value::Int(0) => return ControlFlow::Error("Division by zero".into()),
                Value::Float(f) if *f == 0.0 => {
                    return ControlFlow::Error("Division by zero".into())
                }
                _ => {}
            }
        }
        let val = match (&inputs[0], &inputs[1]) {
            (Value::Dual { .. }, _) | (_, Value::Dual { .. }) => {
                return self.dual_binop(term, inputs)
            }
            (Value::Vec2(..), _) | (_, Value::Vec2(..)) => {
                return self.vec2_binop(term, inputs)
            }
            (Value::Int(a), Value::Int(b)) => match int_arith(&term.op, *a, *b) {
                Ok(v) => Value::Int(v),
                Err(e) => return ControlFlow::Error(e),
            },
            (Value::Float(a), Value::Float(b)) => Value::Float(float_arith(&term.op, *a, *b)),
            (Value::Int(a), Value::Float(b)) => Value::Float(float_arith(&term.op, *a as f64, *b)),
            (Value::Float(a), Value::Int(b)) => Value::Float(float_arith(&term.op, *a, *b as f64)),
            // `+` on two strings is the mistake every JS/Python user makes
            // first; point them at `++` / interpolation instead of the vague
            // "Cannot add string and string".
            (Value::String(_), Value::String(_)) if matches!(term.op, TermOp::Add) => {
                return ControlFlow::Error(
                    "Cannot add string and string — use ++ to concatenate strings, \
                     or string interpolation: \"{a}{b}\""
                        .into(),
                )
            }
            _ => {
                return ControlFlow::Error(format!(
                    "Cannot {} {} and {}",
                    binop_verb(&term.op),
                    inputs[0].type_name(),
                    inputs[1].type_name()
                ))
            }
        };
        self.produce(term, val)
    }

    /// Forward-mode AD arithmetic for Dual numbers.
    fn dual_binop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let (Some(a_val), Some(b_val)) = (inputs[0].as_f64(), inputs[1].as_f64()) else {
            return ControlFlow::Error(format!(
                "Cannot perform arithmetic on {} and {}",
                inputs[0].type_name(),
                inputs[1].type_name()
            ));
        };
        let a_deriv = inputs[0].derivative();
        let b_deriv = inputs[1].derivative();

        let (value, derivative) = match &term.op {
            TermOp::Add => (a_val + b_val, a_deriv + b_deriv),
            TermOp::Sub => (a_val - b_val, a_deriv - b_deriv),
            TermOp::Mul => (a_val * b_val, a_deriv * b_val + a_val * b_deriv),
            TermOp::Div => {
                if b_val == 0.0 {
                    return ControlFlow::Error("Division by zero".into());
                }
                (
                    a_val / b_val,
                    (a_deriv * b_val - a_val * b_deriv) / (b_val * b_val),
                )
            }
            TermOp::Mod => {
                // Mod derivative: d(a%b)/da = 1, d(a%b)/db is complex;
                // approximate: treat as a - floor(a/b)*b
                (a_val % b_val, a_deriv)
            }
            _ => return ControlFlow::Error("Unsupported dual operation".into()),
        };

        self.produce(term, Value::Dual { value, derivative })
    }

    /// Vec2 arithmetic: component-wise add/sub/mul between vectors, and
    /// scalar broadcast for vec2-with-scalar operands.
    fn vec2_binop(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        let val = match (&inputs[0], &inputs[1]) {
            // vec2 op vec2
            (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => match &term.op {
                TermOp::Add => Value::Vec2(ax + bx, ay + by),
                TermOp::Sub => Value::Vec2(ax - bx, ay - by),
                TermOp::Mul => Value::Vec2(ax * bx, ay * by),
                TermOp::Div => {
                    if *bx == 0.0 || *by == 0.0 {
                        return ControlFlow::Error("Division by zero in vec2".into());
                    }
                    Value::Vec2(ax / bx, ay / by)
                }
                _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
            },
            // vec2 op scalar
            (Value::Vec2(x, y), other) => {
                let s = match other.as_f64() {
                    Some(v) => v,
                    None => {
                        return ControlFlow::Error(format!(
                            "Cannot perform arithmetic on vec2 and {}",
                            other.type_name()
                        ))
                    }
                };
                match &term.op {
                    TermOp::Mul => Value::Vec2(x * s, y * s),
                    TermOp::Div => {
                        if s == 0.0 {
                            return ControlFlow::Error("Division by zero".into());
                        }
                        Value::Vec2(x / s, y / s)
                    }
                    TermOp::Add => Value::Vec2(x + s, y + s),
                    TermOp::Sub => Value::Vec2(x - s, y - s),
                    _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
                }
            }
            // scalar op vec2
            (other, Value::Vec2(x, y)) => {
                let s = match other.as_f64() {
                    Some(v) => v,
                    None => {
                        return ControlFlow::Error(format!(
                            "Cannot perform arithmetic on {} and vec2",
                            other.type_name()
                        ))
                    }
                };
                match &term.op {
                    TermOp::Mul => Value::Vec2(s * x, s * y),
                    TermOp::Add => Value::Vec2(s + x, s + y),
                    TermOp::Sub => Value::Vec2(s - x, s - y),
                    _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
                }
            }
            _ => return ControlFlow::Error("Unsupported vec2 operation".into()),
        };
        self.produce(term, val)
    }

    /// Lt / Le / Gt / Ge via the shared value-ordering in builtins.
    pub(super) fn comparison_op(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        use std::cmp::Ordering;
        match builtins::compare_values(&inputs[0], &inputs[1], self.heap) {
            Ok(ord) => {
                let result = match term.op {
                    TermOp::Lt => ord == Ordering::Less,
                    TermOp::Le => ord != Ordering::Greater,
                    TermOp::Gt => ord == Ordering::Greater,
                    TermOp::Ge => ord != Ordering::Less,
                    _ => unreachable!("comparison_op called for non-comparison op"),
                };
                self.produce(term, Value::Bool(result))
            }
            Err(e) => ControlFlow::Error(e),
        }
    }
}

/// Integer arithmetic with checked operators. A raw `+`/`*`/`%` would panic on
/// overflow or a zero divisor, which in WASM is an `unreachable` trap that
/// poisons the whole module (only a page reload recovers). Returning a clean
/// `Err` lets the evaluator surface a normal runtime error the user can fix.
fn int_arith(op: &TermOp, a: i64, b: i64) -> Result<i64, String> {
    let result = match op {
        TermOp::Add => a.checked_add(b),
        TermOp::Sub => a.checked_sub(b),
        TermOp::Mul => a.checked_mul(b),
        // checked_div / checked_rem return None for both a zero divisor and the
        // i64::MIN / -1 overflow case.
        TermOp::Div => a.checked_div(b),
        TermOp::Mod => a.checked_rem(b),
        _ => unreachable!("non-arithmetic op in numeric_binop"),
    };
    result.ok_or_else(|| {
        if b == 0 && matches!(op, TermOp::Div | TermOp::Mod) {
            "Division by zero".to_string()
        } else {
            format!("Integer overflow when trying to {}", binop_verb(op))
        }
    })
}

fn float_arith(op: &TermOp, a: f64, b: f64) -> f64 {
    match op {
        TermOp::Add => a + b,
        TermOp::Sub => a - b,
        TermOp::Mul => a * b,
        TermOp::Div => a / b,
        TermOp::Mod => a % b,
        _ => unreachable!("non-arithmetic op in numeric_binop"),
    }
}

/// Human-readable verb for a binary op — used in error messages so the
/// message says "Cannot add Int and String" instead of the vague
/// "Cannot perform arithmetic on Int and String".
fn binop_verb(op: &TermOp) -> &'static str {
    match op {
        TermOp::Add => "add",
        TermOp::Sub => "subtract",
        TermOp::Mul => "multiply",
        TermOp::Div => "divide",
        TermOp::Mod => "take the modulus of",
        _ => "perform arithmetic on",
    }
}
