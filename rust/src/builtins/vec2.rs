//! 2D vector builtins: vec2, normalize, dot, limit.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

pub(super) fn native_vec2(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "vec2")?;
    let x = state.get_float(1)?;
    let y = state.get_float(2)?;
    state.push_value(Value::Vec2(x, y));
    Ok(1)
}

pub(super) fn native_normalize(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "normalize")?;
    match state.get_value(1)? {
        Value::Vec2(x, y) => {
            let m = (x * x + y * y).sqrt();
            if m < f64::EPSILON {
                state.push_value(Value::Vec2(0.0, 0.0));
            } else {
                state.push_value(Value::Vec2(x / m, y / m));
            }
            Ok(1)
        }
        _ => Err("normalize() expects a vec2".into()),
    }
}

pub(super) fn native_dot(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "dot")?;
    match (state.get_value(1)?, state.get_value(2)?) {
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            state.push_float(ax * bx + ay * by);
            Ok(1)
        }
        _ => Err("dot() expects two vec2 values".into()),
    }
}

pub(super) fn native_limit(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "limit")?;
    match state.get_value(1)? {
        Value::Vec2(x, y) => {
            let max_mag = state.get_float(2)?;
            let m = (x * x + y * y).sqrt();
            if m > max_mag && m > f64::EPSILON {
                let scale = max_mag / m;
                state.push_value(Value::Vec2(x * scale, y * scale));
            } else {
                state.push_value(Value::Vec2(x, y));
            }
            Ok(1)
        }
        _ => Err("limit() expects a vec2 as first argument".into()),
    }
}
