//! Basic math, trig, and numeric conversion: abs, sqrt, floor, ceil, round,
//! float, int, random, min, max, sin/cos/tan, atan2, pi.

use crate::native_fn::PetalCxt;
use crate::value::{compare_values, Value};

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
        if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }
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

pub(super) fn native_float(state: &mut PetalCxt) -> Result<u32, String> {
    unary_float_dual(state, "float", |x| x, |_| 1.0)
}

pub(super) fn native_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "int")?;
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_int(f as i64); Ok(1) }
        Value::String(id) => {
            let s = state.heap().get_string(id).to_string();
            match s.parse::<i64>() {
                Ok(n) => { state.push_int(n); Ok(1) }
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
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Greater => { state.push_value(b); Ok(1) }
    }
}

pub(super) fn native_max(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "max")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Less => { state.push_value(b); Ok(1) }
    }
}

pub(super) fn native_round(state: &mut PetalCxt) -> Result<u32, String> {
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
