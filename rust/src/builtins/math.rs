//! Basic math, trig, and numeric conversion: abs, sqrt, floor, ceil, round,
//! float, int, random, min, max, sin/cos/tan, atan2, pi.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::compare_values;
use super::require_args;

pub(super) fn native_abs(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "abs")?;
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n.abs()); Ok(1) }
        Value::Float(f) => { state.push_float(f.abs()); Ok(1) }
        Value::Dual { value, derivative } => {
            // d/dx |x| = sign(x) * dx
            let sign = if value > 0.0 { 1.0 } else if value < 0.0 { -1.0 } else { 0.0 };
            state.push_value(Value::Dual { value: value.abs(), derivative: sign * derivative });
            Ok(1)
        }
        _ => Err("abs() expects a number".into()),
    }
}

pub(super) fn native_sqrt(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "sqrt")?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx sqrt(x) = 1 / (2 * sqrt(x))
            let sqrt_val = value.sqrt();
            let d = if sqrt_val == 0.0 { 0.0 } else { derivative / (2.0 * sqrt_val) };
            state.push_value(Value::Dual { value: sqrt_val, derivative: d });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.sqrt());
            Ok(1)
        }
    }
}

pub(super) fn native_floor(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "floor")?;
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.floor()); Ok(1) }
        Value::Dual { value, .. } => {
            // floor is a step function: derivative is 0 almost everywhere
            state.push_value(Value::Dual { value: value.floor(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("floor() expects a number".into()),
    }
}

pub(super) fn native_ceil(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "ceil")?;
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.ceil()); Ok(1) }
        Value::Dual { value, .. } => {
            state.push_value(Value::Dual { value: value.ceil(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("ceil() expects a number".into()),
    }
}

pub(super) fn native_float(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "float")?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            state.push_value(Value::Dual { value, derivative });
            Ok(1)
        }
        _ => {
            let f = state.get_float(1)?;
            state.push_float(f);
            Ok(1)
        }
    }
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
    let pseudo = ((std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as f64)
        / 4_294_967_295.0)
        * (max - min)
        + min;
    state.push_float(pseudo);
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
    require_args(state, 1, "round")?;
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.round()); Ok(1) }
        Value::Dual { value, .. } => {
            state.push_value(Value::Dual { value: value.round(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("round() expects a number".into()),
    }
}

pub(super) fn native_sin(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "sin")?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx sin(x) = cos(x) * dx
            state.push_value(Value::Dual { value: value.sin(), derivative: value.cos() * derivative });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.sin());
            Ok(1)
        }
    }
}

pub(super) fn native_cos(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "cos")?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx cos(x) = -sin(x) * dx
            state.push_value(Value::Dual { value: value.cos(), derivative: -value.sin() * derivative });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.cos());
            Ok(1)
        }
    }
}

pub(super) fn native_tan(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "tan")?;
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx tan(x) = sec^2(x) * dx = dx / cos^2(x)
            let cos_val = value.cos();
            state.push_value(Value::Dual { value: value.tan(), derivative: derivative / (cos_val * cos_val) });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.tan());
            Ok(1)
        }
    }
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
