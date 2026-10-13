//! Creative-coding math + extended randomness: clamp, lerp, map_range,
//! distance, mag, pow, sign, fract, smoothstep, radians, degrees, exp, log,
//! random_int, choose.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;
use super::vec2::{push_vec3, vec3_parts};

pub(super) fn native_clamp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "clamp")?;
    // Int-preserving, the same contract `min`/`max` already keep: a clamped
    // list index or `range` bound has to stay an int, or the caller gets
    // "Cannot index list with float" for what reads like integer arithmetic.
    // Only an all-int call stays int; one float argument makes the whole
    // expression float, as it does for `+`.
    if let (Value::Int(v), Value::Int(lo), Value::Int(hi)) = (
        state.get_value(1)?,
        state.get_value(2)?,
        state.get_value(3)?,
    ) {
        state.push_int(v.max(lo).min(hi));
        return Ok(1);
    }
    let v = state.get_float(1)?;
    let lo = state.get_float(2)?;
    let hi = state.get_float(3)?;
    state.push_float(v.max(lo).min(hi));
    Ok(1)
}

/// `lerp(a, b, t)`: `a + (b - a) * t` — for two numbers, or component-wise
/// for two vec2s, two vec3s, two colors (per channel, alpha too; see
/// `color_lerp`) or two equal-length lists of numbers.
pub(super) fn native_lerp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "lerp")?;
    match (state.get_value(1)?, state.get_value(2)?) {
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            let t = state.get_float(3)?;
            state.push_value(Value::Vec2(ax + (bx - ax) * t, ay + (by - ay) * t));
            return Ok(1);
        }
        (a @ Value::Vec3(_), b @ Value::Vec3(_)) => {
            let t = state.get_float(3)?;
            let (a, b) = (vec3_parts(state, a).unwrap(), vec3_parts(state, b).unwrap());
            push_vec3(
                state,
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ],
            );
            return Ok(1);
        }
        (Value::Map(a), Value::Map(b))
            if super::color::is_color_map(state, a) && super::color::is_color_map(state, b) =>
        {
            let t = state.get_float(3)?;
            super::color::lerp_color_maps(state, a, b, t);
            return Ok(1);
        }
        (Value::List(a), Value::List(b)) => {
            let t = state.get_float(3)?;
            let (a, b) = (state.heap().get_list(a), state.heap().get_list(b));
            if a.len() != b.len() {
                return Err(format!(
                    "lerp(a, b, t) on lists needs equal lengths, got {} and {}",
                    a.len(),
                    b.len()
                ));
            }
            let mixed = a
                .iter()
                .zip(b)
                .map(|(x, y)| match (x.as_f64(), y.as_f64()) {
                    (Some(x), Some(y)) => Ok(Value::Float(x + (y - x) * t)),
                    _ => Err(format!(
                        "lerp(a, b, t) on lists needs numbers, got {} and {}",
                        x.type_name(),
                        y.type_name()
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let id = state.heap_mut().alloc_list(mixed);
            state.push_value(Value::List(id));
            return Ok(1);
        }
        // Two records that are not colors: field by field.
        (Value::Map(a), Value::Map(b)) => {
            let t = state.get_float(3)?;
            let v = lerp_records(state, a, b, t, 0);
            state.push_value(v);
            return Ok(1);
        }
        (
            Value::Vec2(..) | Value::Vec3(_) | Value::Map(_) | Value::List(_),
            _,
        )
        | (_, Value::Vec2(..) | Value::Vec3(_) | Value::Map(_) | Value::List(_)) => {
            return Err(
                "lerp(a, b, t) expects two numbers, two vectors of the same kind, \
                 two colors {r, g, b}, two lists of numbers or two records"
                    .into(),
            );
        }
        _ => {}
    }
    let a = state.get_float(1)?;
    let b = state.get_float(2)?;
    let t = state.get_float(3)?;
    state.push_float(a + (b - a) * t);
    Ok(1)
}

/// How deep [`lerp_records`] follows records inside records before it stops
/// blending and picks a side. Data is a tree, so this is never met in
/// practice; it bounds the native stack.
const LERP_DEPTH: usize = 32;

/// `lerp` of two records, field by field: the result has `a`'s fields (and
/// its class), each blended with the field of the same name in `b` where the
/// two can be — see [`lerp_field`] — and otherwise taken whole from `a` while
/// `t < 0.5` and from `b` after. A field `b` does not have keeps `a`'s value.
fn lerp_records(
    state: &mut PetalCxt,
    a: crate::heap::MapId,
    b: crate::heap::MapId,
    t: f64,
    depth: usize,
) -> Value {
    let fields: Vec<(crate::record::Key, Value, Option<Value>)> = {
        let (ra, rb) = (state.heap().get_map(a), state.heap().get_map(b));
        ra.iter()
            .map(|(k, v)| (k.clone(), *v, rb.get(k.as_str()).copied()))
            .collect()
    };
    let mut out = crate::heap::RecordMap::with_capacity(fields.len());
    for (key, x, y) in fields {
        let v = match y {
            None => x,
            Some(y) => match lerp_field(state, x, y, t, depth) {
                Some(v) => v,
                None if t < 0.5 => x,
                None => y,
            },
        };
        out.insert(key, v);
    }
    let id = match state.heap().map_class(a) {
        Some(class) => state.heap_mut().alloc_class_instance(out, class),
        None => state.heap_mut().alloc_map(out),
    };
    Value::Map(id)
}

/// The blend of two field values, or `None` when they have none (a string, a
/// bool, values of different kinds): two numbers, two vectors of one kind,
/// two colors, two records, or two equal-length lists whose elements all
/// blend.
fn lerp_field(state: &mut PetalCxt, x: Value, y: Value, t: f64, depth: usize) -> Option<Value> {
    match (x, y) {
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            Some(Value::Vec2(ax + (bx - ax) * t, ay + (by - ay) * t))
        }
        (Value::Vec3(_), Value::Vec3(_)) => {
            let (a, b) = (vec3_parts(state, x)?, vec3_parts(state, y)?);
            let id = state.heap_mut().alloc_vec3([
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ]);
            Some(Value::Vec3(id))
        }
        (Value::Map(a), Value::Map(b)) => {
            if super::color::is_color_map(state, a) && super::color::is_color_map(state, b) {
                Some(super::color::lerp_color_value(state, a, b, t))
            } else if depth < LERP_DEPTH {
                Some(lerp_records(state, a, b, t, depth + 1))
            } else {
                None
            }
        }
        (Value::List(a), Value::List(b)) => {
            let pairs: Vec<(Value, Value)> = {
                let (a, b) = (state.heap().get_list(a), state.heap().get_list(b));
                if a.len() != b.len() {
                    return None;
                }
                a.iter().copied().zip(b.iter().copied()).collect()
            };
            if depth >= LERP_DEPTH {
                return None;
            }
            let mut out = Vec::with_capacity(pairs.len());
            for (p, q) in pairs {
                out.push(lerp_field(state, p, q, t, depth + 1)?);
            }
            Some(Value::List(state.heap_mut().alloc_list(out)))
        }
        _ => match (x, y) {
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                let (a, b) = (x.as_f64()?, y.as_f64()?);
                Some(Value::Float(a + (b - a) * t))
            }
            _ => None,
        },
    }
}

pub(super) fn native_map_range(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 5, "map_range")?;
    let v = state.get_float(1)?;
    let in_lo = state.get_float(2)?;
    let in_hi = state.get_float(3)?;
    let out_lo = state.get_float(4)?;
    let out_hi = state.get_float(5)?;
    let t = if (in_hi - in_lo).abs() < f64::EPSILON {
        0.0
    } else {
        (v - in_lo) / (in_hi - in_lo)
    };
    state.push_float(out_lo + (out_hi - out_lo) * t);
    Ok(1)
}

pub(super) fn native_distance(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    match argc {
        2 => {
            // distance(vec2, vec2) / distance(vec3, vec3)
            let a = state.get_value(1)?;
            let b = state.get_value(2)?;
            match (a, b) {
                (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
                    let dx = bx - ax;
                    let dy = by - ay;
                    state.push_float((dx * dx + dy * dy).sqrt());
                    Ok(1)
                }
                (Value::Vec3(_), Value::Vec3(_)) => {
                    let (a, b) = (vec3_parts(state, a).unwrap(), vec3_parts(state, b).unwrap());
                    let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
                    state.push_float((dx * dx + dy * dy + dz * dz).sqrt());
                    Ok(1)
                }
                _ => Err("distance(a, b) expects two vec2 or two vec3 values".into()),
            }
        }
        4 => {
            let x1 = state.get_float(1)?;
            let y1 = state.get_float(2)?;
            let x2 = state.get_float(3)?;
            let y2 = state.get_float(4)?;
            let dx = x2 - x1;
            let dy = y2 - y1;
            state.push_float((dx * dx + dy * dy).sqrt());
            Ok(1)
        }
        _ => Err(
            "distance() expects 2 (two vec2s or two vec3s) or 4 (x1, y1, x2, y2) arguments".into(),
        ),
    }
}

pub(super) fn native_mag(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    match argc {
        1 => {
            // mag(vec2) / mag(vec3)
            match state.get_value(1)? {
                Value::Vec2(x, y) => {
                    state.push_float((x * x + y * y).sqrt());
                    Ok(1)
                }
                v @ Value::Vec3(_) => {
                    let [x, y, z] = vec3_parts(state, v).unwrap();
                    state.push_float((x * x + y * y + z * z).sqrt());
                    Ok(1)
                }
                _ => {
                    let x = state.get_float(1)?;
                    state.push_float(x.abs());
                    Ok(1)
                }
            }
        }
        2 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            state.push_float((x * x + y * y).sqrt());
            Ok(1)
        }
        3 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            let z = state.get_float(3)?;
            state.push_float((x * x + y * y + z * z).sqrt());
            Ok(1)
        }
        _ => Err("mag() expects 1-3 arguments".into()),
    }
}

pub(super) fn native_pow(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "pow")?;
    let base = state.get_float(1)?;
    let exp = state.get_float(2)?;
    state.push_float(base.powf(exp));
    Ok(1)
}

pub(super) fn native_sign(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "sign")?;
    match state.get_value(1)? {
        Value::Int(n) => {
            state.push_int(if n > 0 {
                1
            } else if n < 0 {
                -1
            } else {
                0
            });
            Ok(1)
        }
        Value::Float(f) => {
            state.push_float(if f > 0.0 {
                1.0
            } else if f < 0.0 {
                -1.0
            } else {
                0.0
            });
            Ok(1)
        }
        _ => Err("sign() expects a number".into()),
    }
}

pub(super) fn native_fract(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "fract")?;
    let f = state.get_float(1)?;
    state.push_float(f - f.floor());
    Ok(1)
}

pub(super) fn native_smoothstep(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "smoothstep")?;
    let edge0 = state.get_float(1)?;
    let edge1 = state.get_float(2)?;
    let x = state.get_float(3)?;
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    state.push_float(t * t * (3.0 - 2.0 * t));
    Ok(1)
}

pub(super) fn native_radians(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "radians")?;
    let deg = state.get_float(1)?;
    state.push_float(deg.to_radians());
    Ok(1)
}

pub(super) fn native_degrees(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "degrees")?;
    let rad = state.get_float(1)?;
    state.push_float(rad.to_degrees());
    Ok(1)
}

pub(super) fn native_exp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "exp")?;
    let x = state.get_float(1)?;
    state.push_float(x.exp());
    Ok(1)
}

pub(super) fn native_log(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "log")?;
    let x = state.get_float(1)?;
    state.push_float(x.ln());
    Ok(1)
}

pub(super) fn native_random_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "random_int")?;
    let min = state.get_int(1)?;
    let max = state.get_int(2)?;
    if min >= max {
        state.push_int(min);
        return Ok(1);
    }
    // `max - min` overflows i64 for wide bounds; the kernel works in i128 and
    // is proven to land in [min, max).
    let val = crate::numeric::scale_unit_to_range(state.rng_next_f64(), min, max);
    state.push_int(val);
    Ok(1)
}

pub(super) fn native_choose(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "choose")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let len = state.heap().get_list(id).len();
            if len == 0 {
                state.push_nil();
            } else {
                let idx = ((state.rng_next_f64() * len as f64) as usize).min(len - 1);
                let val = state.heap().get_list(id)[idx];
                state.push_value(val);
            }
            Ok(1)
        }
        _ => Err("choose() expects a list".into()),
    }
}
