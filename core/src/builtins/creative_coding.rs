//! Creative-coding math + extended randomness: clamp, lerp, map_range,
//! distance, mag, pow, sign, fract, smoothstep, radians, degrees, exp, log,
//! random_int, choose.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::autodiff::{D, args_d, push_d};
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
    let ([v, lo, hi], dual) = args_d::<3>(state)?;
    // `max(v, lo)` then `min(.., hi)`, choosing whole arguments so a dual
    // keeps the derivative of the side that won: `v`'s inside the range, a
    // bound's (usually 0) outside it. A NaN loses to the other side, as in
    // `f64::max`/`f64::min`.
    let m = if v.v.is_nan() || v.v < lo.v { lo } else { v };
    let r = if m.v.is_nan() || hi.v < m.v { hi } else { m };
    push_d(state, r, dual);
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
                    (Some(_), Some(_))
                        if matches!(x, Value::Dual { .. }) || matches!(y, Value::Dual { .. }) =>
                    {
                        let (a, b) = (D { v: x.as_f64().unwrap(), d: x.derivative() }, D {
                            v: y.as_f64().unwrap(),
                            d: y.derivative(),
                        });
                        let r = lerp_d(a, b, D::konst(t));
                        Ok(Value::Dual { value: r.v, derivative: r.d })
                    }
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
    let ([a, b, t], dual) = args_d::<3>(state)?;
    push_d(state, lerp_d(a, b, t), dual);
    Ok(1)
}

/// `a + (b - a) * t` on dual parts.
fn lerp_d(a: D, b: D, t: D) -> D {
    a.add(b.sub(a).mul(t))
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
            (
                Value::Int(_) | Value::Float(_) | Value::Dual { .. },
                Value::Int(_) | Value::Float(_) | Value::Dual { .. },
            ) => {
                let a = D { v: x.as_f64()?, d: x.derivative() };
                let b = D { v: y.as_f64()?, d: y.derivative() };
                let r = lerp_d(a, b, D::konst(t));
                Some(Value::Dual { value: r.v, derivative: r.d })
            }
            _ => None,
        },
    }
}

pub(super) fn native_map_range(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 5, "map_range")?;
    let ([v, in_lo, in_hi, out_lo, out_hi], dual) = args_d::<5>(state)?;
    let t = if (in_hi.v - in_lo.v).abs() < f64::EPSILON {
        D::konst(0.0)
    } else {
        v.sub(in_lo).div(in_hi.sub(in_lo))
    };
    push_d(state, lerp_d(out_lo, out_hi, t), dual);
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
            let ([x1, y1, x2, y2], dual) = args_d::<4>(state)?;
            let (dx, dy) = (x2.sub(x1), y2.sub(y1));
            push_d(state, dx.mul(dx).add(dy.mul(dy)).sqrt(), dual);
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
                    let ([x], dual) = args_d::<1>(state)?;
                    let sign = if x.v > 0.0 { 1.0 } else if x.v < 0.0 { -1.0 } else { 0.0 };
                    push_d(state, x.chain(x.v.abs(), sign), dual);
                    Ok(1)
                }
            }
        }
        2 => {
            let ([x, y], dual) = args_d::<2>(state)?;
            push_d(state, x.mul(x).add(y.mul(y)).sqrt(), dual);
            Ok(1)
        }
        3 => {
            let ([x, y, z], dual) = args_d::<3>(state)?;
            push_d(state, x.mul(x).add(y.mul(y)).add(z.mul(z)).sqrt(), dual);
            Ok(1)
        }
        _ => Err("mag() expects 1-3 arguments".into()),
    }
}

pub(super) fn native_pow(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "pow")?;
    let ([base, exp], dual) = args_d::<2>(state)?;
    let v = base.v.powf(exp.v);
    // d(a^b) = b a^(b-1) da + a^b ln(a) db. Each term only where its input
    // moves, so a constant exponent never meets `ln` of a negative base and
    // a constant base never meets `0 * inf`.
    let mut d = 0.0;
    if base.d != 0.0 {
        d += exp.v * base.v.powf(exp.v - 1.0) * base.d;
    }
    if exp.d != 0.0 {
        d += v * base.v.ln() * exp.d;
    }
    push_d(state, D { v, d }, dual);
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
        // A step function: derivative 0, and the result is still a dual.
        Value::Dual { value, .. } => {
            let s = if value > 0.0 {
                1.0
            } else if value < 0.0 {
                -1.0
            } else {
                0.0
            };
            state.push_value(Value::Dual { value: s, derivative: 0.0 });
            Ok(1)
        }
        _ => Err("sign() expects a number".into()),
    }
}

pub(super) fn native_fract(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "fract")?;
    // x - floor(x): slope 1 between the jumps.
    let ([x], dual) = args_d::<1>(state)?;
    push_d(state, x.chain(x.v - x.v.floor(), 1.0), dual);
    Ok(1)
}

pub(super) fn native_smoothstep(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "smoothstep")?;
    let ([edge0, edge1, x], dual) = args_d::<3>(state)?;
    let raw = x.sub(edge0).div(edge1.sub(edge0));
    // Clamped to [0, 1]: flat (derivative 0) outside it.
    let t = if raw.v.is_nan() {
        raw
    } else if raw.v <= 0.0 {
        D::konst(0.0)
    } else if raw.v >= 1.0 {
        D::konst(1.0)
    } else {
        raw
    };
    // t^2 (3 - 2t), whose slope is 6 t (1 - t).
    let r = t.chain(t.v * t.v * (3.0 - 2.0 * t.v), 6.0 * t.v * (1.0 - t.v));
    push_d(state, r, dual);
    Ok(1)
}

pub(super) fn native_radians(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "radians")?;
    let ([deg], dual) = args_d::<1>(state)?;
    push_d(state, deg.chain(deg.v.to_radians(), std::f64::consts::PI / 180.0), dual);
    Ok(1)
}

pub(super) fn native_degrees(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "degrees")?;
    let ([rad], dual) = args_d::<1>(state)?;
    push_d(state, rad.chain(rad.v.to_degrees(), 180.0 / std::f64::consts::PI), dual);
    Ok(1)
}

pub(super) fn native_exp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "exp")?;
    let ([x], dual) = args_d::<1>(state)?;
    let e = x.v.exp();
    push_d(state, x.chain(e, e), dual);
    Ok(1)
}

pub(super) fn native_log(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "log")?;
    let ([x], dual) = args_d::<1>(state)?;
    push_d(state, x.chain(x.v.ln(), 1.0 / x.v), dual);
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
