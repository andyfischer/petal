//! Creative-coding math + extended randomness: clamp, lerp, map_range,
//! distance, mag, pow, sign, fract, smoothstep, radians, degrees, exp, log,
//! random_int, choose.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

pub(super) fn native_clamp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "clamp")?;
    let v = state.get_float(1)?;
    let lo = state.get_float(2)?;
    let hi = state.get_float(3)?;
    state.push_float(v.max(lo).min(hi));
    Ok(1)
}

pub(super) fn native_lerp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "lerp")?;
    let a = state.get_float(1)?;
    let b = state.get_float(2)?;
    let t = state.get_float(3)?;
    state.push_float(a + (b - a) * t);
    Ok(1)
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
            // distance(vec2, vec2)
            let a = state.get_value(1)?;
            let b = state.get_value(2)?;
            match (a, b) {
                (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
                    let dx = bx - ax;
                    let dy = by - ay;
                    state.push_float((dx * dx + dy * dy).sqrt());
                    Ok(1)
                }
                _ => Err("distance(a, b) expects two vec2 values".into()),
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
        _ => Err("distance() expects 2 (vec2, vec2) or 4 (x1, y1, x2, y2) arguments".into()),
    }
}

pub(super) fn native_mag(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    match argc {
        1 => {
            // mag(vec2)
            match state.get_value(1)? {
                Value::Vec2(x, y) => {
                    state.push_float((x * x + y * y).sqrt());
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
    let range = (max - min) as f64;
    let val = min + (state.rng_next_f64() * range) as i64;
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
