//! Vector builtins: vec2, vec3, normalize, dot, cross, limit, rotate.

// `vec2` is an inline `Value::Vec2`; `vec3` is a heap-allocated `Value::Vec3`
// (three f64s would not fit in a `Value` — see docs/dev/vec3.md). The shared
// natives (`normalize`, `dot`, `limit`, and `distance`/`mag`/`lerp` in
// `creative_coding`) take either kind and answer in the kind they were given.

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

pub(super) fn native_vec3(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "vec3")?;
    let x = state.get_float(1)?;
    let y = state.get_float(2)?;
    let z = state.get_float(3)?;
    let v = state.heap_mut().vec3_value(x, y, z);
    state.push_value(v);
    Ok(1)
}

/// The components of a `vec3` argument, if `v` is one.
pub(super) fn vec3_parts(state: &PetalCxt, v: Value) -> Option<[f64; 3]> {
    match v {
        Value::Vec3(id) => Some(state.heap().get_vec3(id)),
        _ => None,
    }
}

/// Push a fresh `vec3`.
pub(super) fn push_vec3(state: &mut PetalCxt, [x, y, z]: [f64; 3]) {
    let v = state.heap_mut().vec3_value(x, y, z);
    state.push_value(v);
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(super) fn native_normalize(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "normalize")?;
    let v = state.get_value(1)?;
    match v {
        Value::Vec2(x, y) => {
            let m = (x * x + y * y).sqrt();
            if m < f64::EPSILON {
                state.push_value(Value::Vec2(0.0, 0.0));
            } else {
                state.push_value(Value::Vec2(x / m, y / m));
            }
            Ok(1)
        }
        Value::Vec3(_) => {
            let a = vec3_parts(state, v).unwrap();
            let m = dot3(a, a).sqrt();
            // The zero vector stays zero, as for vec2.
            let out = if m < f64::EPSILON {
                [0.0; 3]
            } else {
                [a[0] / m, a[1] / m, a[2] / m]
            };
            push_vec3(state, out);
            Ok(1)
        }
        _ => Err("normalize() expects a vec2 or vec3".into()),
    }
}

pub(super) fn native_dot(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "dot")?;
    let (a, b) = (state.get_value(1)?, state.get_value(2)?);
    match (a, b) {
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            state.push_float(ax * bx + ay * by);
            Ok(1)
        }
        (Value::Vec3(_), Value::Vec3(_)) => {
            let d = dot3(vec3_parts(state, a).unwrap(), vec3_parts(state, b).unwrap());
            state.push_float(d);
            Ok(1)
        }
        _ => Err("dot() expects two vec2 or two vec3 values".into()),
    }
}

/// `cross(a, b)`: the cross product of two vec3s — perpendicular to both, in
/// the right-handed sense (`cross(vec3(1, 0, 0), vec3(0, 1, 0))` is
/// `vec3(0, 0, 1)`).
pub(super) fn native_cross(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "cross")?;
    let (a, b) = (state.get_value(1)?, state.get_value(2)?);
    match (vec3_parts(state, a), vec3_parts(state, b)) {
        (Some([ax, ay, az]), Some([bx, by, bz])) => {
            push_vec3(
                state,
                [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx],
            );
            Ok(1)
        }
        _ => Err("cross() expects two vec3 values".into()),
    }
}

pub(super) fn native_limit(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "limit")?;
    let v = state.get_value(1)?;
    match v {
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
        Value::Vec3(_) => {
            let max_mag = state.get_float(2)?;
            let a = vec3_parts(state, v).unwrap();
            let m = dot3(a, a).sqrt();
            if m > max_mag && m > f64::EPSILON {
                push_vec3(state, scale3(a, max_mag / m));
            } else {
                // Already short enough: hand back the same (immutable) vec3.
                state.push_value(v);
            }
            Ok(1)
        }
        _ => Err("limit() expects a vec2 or vec3 as first argument".into()),
    }
}

/// `rotate(v, angle)`: `v` turned by `angle` radians, the same sense as
/// `vec2(cos(a), sin(a))` — so `rotate(vec2(1, 0), a)` is that vector.
pub(super) fn native_rotate(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "rotate")?;
    match state.get_value(1)? {
        Value::Vec2(x, y) => {
            let a = state.get_float(2)?;
            let (s, c) = a.sin_cos();
            state.push_value(Value::Vec2(x * c - y * s, x * s + y * c));
            Ok(1)
        }
        _ => Err("rotate() expects a vec2 as first argument".into()),
    }
}
