//! Basic math, trig, and numeric conversion: abs, sqrt, floor, ceil, round,
//! float, int, parse_float, parse_int, random, min, max, sin/cos/tan, atan2,
//! pi.

use crate::native_fn::PetalCxt;
use crate::value::{Value, compare_values};

use super::require_args;

/// Apply a differentiable unary float function: on a dual number, propagate the
/// primal through `primal` and the derivative through `deriv` (the chain-rule
/// multiplier `df/dx`); otherwise coerce the argument to f64 and push `primal`.
fn unary_float_dual(
    state: &mut PetalCxt,
    name: &str,
    primal: fn(f64) -> f64,
    deriv: fn(f64) -> f64,
) -> Result<u32, String> {
    require_args(state, 1, name)?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => state.push_value(Value::Dual {
            value: primal(value),
            derivative: deriv(value) * derivative,
        }),
        _ => {
            let n = state.get_float(1)?;
            state.push_float(primal(n));
        }
    }
    Ok(1)
}

/// Apply a unary numeric function that preserves integer arguments: `Int` maps
/// through `int_fn`, `Float` through `float_fn`, a dual number propagates
/// `float_fn` with chain-rule multiplier `deriv`; anything else is an error.
fn unary_num_preserving(
    state: &mut PetalCxt,
    name: &str,
    int_fn: fn(i64) -> i64,
    float_fn: fn(f64) -> f64,
    deriv: fn(f64) -> f64,
) -> Result<u32, String> {
    require_args(state, 1, name)?;
    match state.get_value(1)? {
        Value::Int(n) => state.push_int(int_fn(n)),
        Value::Float(f) => state.push_float(float_fn(f)),
        Value::Dual { value, derivative } => state.push_value(Value::Dual {
            value: float_fn(value),
            derivative: deriv(value) * derivative,
        }),
        _ => return Err(format!("{}() expects a number", name)),
    }
    Ok(1)
}

pub(super) fn native_abs(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx |x| = sign(x), with the derivative pinned to 0 at exactly 0
    unary_num_preserving(state, "abs", i64::abs, f64::abs, |x| {
        if x > 0.0 {
            1.0
        } else if x < 0.0 {
            -1.0
        } else {
            0.0
        }
    })
}

pub(super) fn native_sqrt(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx sqrt(x) = 1 / (2 * sqrt(x)), guarding sqrt(x) == 0
    unary_float_dual(state, "sqrt", f64::sqrt, |x| {
        let s = x.sqrt();
        if s == 0.0 { 0.0 } else { 1.0 / (2.0 * s) }
    })
}

pub(super) fn native_floor(state: &mut PetalCxt) -> Result<u32, String> {
    // floor is a step function: derivative is 0 almost everywhere
    unary_num_preserving(state, "floor", |n| n, f64::floor, |_| 0.0)
}

pub(super) fn native_ceil(state: &mut PetalCxt) -> Result<u32, String> {
    unary_num_preserving(state, "ceil", |n| n, f64::ceil, |_| 0.0)
}

/// Parse `s` as a float the way `float`/`parse_float` accept it: surrounding
/// whitespace is ignored, and only a finite number counts (so `"inf"`/`"nan"`,
/// which `f64::from_str` accepts, are rejected — a spreadsheet cell reading
/// "nan" is bad input, not a value).
fn parse_f64(s: &str) -> Option<f64> {
    match s.trim().parse::<f64>() {
        Ok(f) if f.is_finite() => Some(f),
        _ => None,
    }
}

pub(super) fn native_float(state: &mut PetalCxt) -> Result<u32, String> {
    // A numeric string converts, mirroring `int("42")`; anything else falls
    // through to the dual-aware numeric path.
    if state.arg_count() == 1 {
        if let Value::String(id) = state.get_value(1)? {
            let s = state.heap().get_string(id).to_string();
            return match parse_f64(&s) {
                Some(f) => {
                    state.push_float(f);
                    Ok(1)
                }
                None => Err(format!("Cannot convert '{}' to float", s)),
            };
        }
    }
    unary_float_dual(state, "float", |x| x, |_| 1.0)
}

/// `parse_float(s)` — the failable counterpart of `float`: `nil` instead of an
/// abort when the text isn't a number, so user input can be validated rather
/// than crashing the program that read it.
pub(super) fn native_parse_float(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "parse_float")?;
    match state.get_value(1)? {
        Value::Int(n) => state.push_float(n as f64),
        Value::Float(f) => state.push_float(f),
        Value::String(id) => {
            let s = state.heap().get_string(id).to_string();
            match parse_f64(&s) {
                Some(f) => state.push_float(f),
                None => state.push_nil(),
            }
        }
        _ => state.push_nil(),
    }
    Ok(1)
}

/// `parse_int(s)` — the failable counterpart of `int`. Only a whole number
/// parses; `"3.5"` is `nil`, because silently truncating a user's "3.5" into 3
/// is the kind of quiet wrong answer this builtin exists to avoid. Use
/// `int(parse_float(s))` when truncation is what you want.
pub(super) fn native_parse_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "parse_int")?;
    match state.get_value(1)? {
        Value::Int(n) => state.push_int(n),
        Value::Float(f) => state.push_int(f as i64),
        Value::String(id) => {
            let s = state.heap().get_string(id).to_string();
            match s.trim().parse::<i64>() {
                Ok(n) => state.push_int(n),
                Err(_) => state.push_nil(),
            }
        }
        _ => state.push_nil(),
    }
    Ok(1)
}

pub(super) fn native_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "int")?;
    match state.get_value(1)? {
        Value::Int(n) => {
            state.push_int(n);
            Ok(1)
        }
        Value::Float(f) => {
            state.push_int(f as i64);
            Ok(1)
        }
        Value::String(id) => {
            let s = state.heap().get_string(id).to_string();
            match s.parse::<i64>() {
                Ok(n) => {
                    state.push_int(n);
                    Ok(1)
                }
                Err(_) => Err(format!("Cannot convert '{}' to int", s)),
            }
        }
        v => Err(format!("Cannot convert {} to int", v.type_name())),
    }
}

pub(super) fn native_random(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "random")?;
    let min = state.get_float(1)?;
    let max = state.get_float(2)?;
    let r = state.rng_next_f64() * (max - min) + min;
    state.push_float(r);
    Ok(1)
}

pub(super) fn native_min(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "min")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => {
            state.push_value(a);
            Ok(1)
        }
        std::cmp::Ordering::Greater => {
            state.push_value(b);
            Ok(1)
        }
    }
}

pub(super) fn native_max(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "max")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => {
            state.push_value(a);
            Ok(1)
        }
        std::cmp::Ordering::Less => {
            state.push_value(b);
            Ok(1)
        }
    }
}

/// Round `x` to `places` decimal digits. Negative `places` rounds to a power of
/// ten to the left of the point (`round(1234.0, -2) == 1200.0`). Beyond ±17
/// digits the scaling would overflow the f64's precision without changing the
/// value, so the number is returned as-is.
fn round_to_places(x: f64, places: i64) -> f64 {
    if !x.is_finite() || places > 17 || places < -17 {
        return x;
    }
    let factor = 10f64.powi(places as i32);
    (x * factor).round() / factor
}

pub(super) fn native_round(state: &mut PetalCxt) -> Result<u32, String> {
    if state.arg_count() == 2 {
        let places = state.get_int(2)?;
        return match state.get_value(1)? {
            // An int is already whole: only rounding to the *left* of the
            // point can change it, and it stays an int either way.
            Value::Int(n) => {
                let r = if places >= 0 {
                    n
                } else {
                    round_to_places(n as f64, places) as i64
                };
                state.push_int(r);
                Ok(1)
            }
            Value::Float(f) => {
                state.push_float(round_to_places(f, places));
                Ok(1)
            }
            Value::Dual { value, derivative } => {
                // Rounding is a step function: derivative 0 almost everywhere.
                state.push_value(Value::Dual {
                    value: round_to_places(value, places),
                    derivative: 0.0 * derivative,
                });
                Ok(1)
            }
            _ => Err("round() expects a number".to_string()),
        };
    }
    unary_num_preserving(state, "round", |n| n, f64::round, |_| 0.0)
}

pub(super) fn native_sin(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx sin(x) = cos(x)
    unary_float_dual(state, "sin", f64::sin, f64::cos)
}

pub(super) fn native_cos(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx cos(x) = -sin(x)
    unary_float_dual(state, "cos", f64::cos, |x| -x.sin())
}

pub(super) fn native_tan(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx tan(x) = sec^2(x) = 1 / cos^2(x)
    unary_float_dual(state, "tan", f64::tan, |x| {
        let c = x.cos();
        1.0 / (c * c)
    })
}

pub(super) fn native_atan2(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "atan2")?;
    let y = state.get_float(1)?;
    let x = state.get_float(2)?;
    state.push_float(y.atan2(x));
    Ok(1)
}

pub(super) fn native_pi(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 0, "pi")?;
    state.push_float(std::f64::consts::PI);
    Ok(1)
}

/// `safe_div(a, b)` — division that answers `nil` instead of aborting when the
/// divisor is zero. Everything else about it is bare `/`: the same int/int
/// truncation, the same float, dual and list-broadcast behaviour, and the same
/// errors for a non-numeric operand or an i64 overflow.
///
/// This exists because bare `/` deliberately *aborts* on a zero divisor, which
/// is right for a program with a bug and wrong for an evaluator whose input is
/// user-supplied (a calculator, a spreadsheet cell, a ratio over a count that
/// can legitimately be 0). Those want the failure as data — `safe_div(a, b) ?? 0`
/// or a nil check — not a dead run. The semantics of `/` are unchanged.
pub(super) fn native_safe_div(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "safe_div")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    let divisor_is_zero = match b {
        Value::Int(0) => true,
        Value::Float(f) => f == 0.0,
        Value::Dual { value, .. } => value == 0.0,
        _ => false,
    };
    if divisor_is_zero {
        state.push_nil();
        return Ok(1);
    }
    // Delegate to the real `/` so every other case (dual numbers, vec2,
    // list broadcast, overflow reporting) stays byte-identical to the operator.
    let v = crate::backend::ops::arithmetic(&crate::program::TermOp::Div, a, b, state.heap_mut())?;
    state.push_value(v);
    Ok(1)
}
