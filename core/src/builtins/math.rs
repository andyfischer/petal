//! Basic math, trig, and numeric conversion: abs, sqrt, floor, ceil, round,
//! float, int, parse_float, parse_int, random, min, max, sin/cos/tan,
//! asin/acos/atan, atan2, hypot, pi.

use crate::native_fn::PetalCxt;
use crate::numeric::{self, IntArithError};
use crate::value::{Value, compare_values};

use super::autodiff::{D, arg_d, args_d, push_d};
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
    int_fn: fn(i64) -> Result<i64, IntArithError>,
    float_fn: fn(f64) -> f64,
    deriv: fn(f64) -> f64,
) -> Result<u32, String> {
    require_args(state, 1, name)?;
    match state.get_value(1)? {
        Value::Int(n) => {
            state.push_int(int_fn(n).map_err(|_| format!("Integer overflow in {}()", name))?)
        }
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
    unary_num_preserving(state, "abs", numeric::int_abs, f64::abs, |x| {
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
    unary_num_preserving(state, "floor", Ok, f64::floor, |_| 0.0)
}

pub(super) fn native_ceil(state: &mut PetalCxt) -> Result<u32, String> {
    unary_num_preserving(state, "ceil", Ok, f64::ceil, |_| 0.0)
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

/// `random()` is a float in [0, 1); `random(hi)` in [0, hi); `random(lo, hi)`
/// in [lo, hi).
pub(super) fn native_random(state: &mut PetalCxt) -> Result<u32, String> {
    let (min, max) = match state.arg_count() {
        0 => (0.0, 1.0),
        1 => (0.0, state.get_float(1)?),
        2 => (state.get_float(1)?, state.get_float(2)?),
        n => {
            return Err(format!(
                "random() expects 0, 1 or 2 arguments (random(), random(hi), random(lo, hi)), got {n}"
            ))
        }
    };
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
    unary_num_preserving(state, "round", Ok, f64::round, |_| 0.0)
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
    let ([y, x], dual) = args_d::<2>(state)?;
    // d atan2(y, x) = (x dy - y dx) / (x^2 + y^2), and 0 at the origin.
    let r2 = x.v * x.v + y.v * y.v;
    let d = if r2 == 0.0 { 0.0 } else { (x.v * y.d - y.v * x.d) / r2 };
    push_d(state, D { v: y.v.atan2(x.v), d }, dual);
    Ok(1)
}

pub(super) fn native_asin(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx asin(x) = 1 / sqrt(1 - x^2)
    unary_float_dual(state, "asin", f64::asin, |x| 1.0 / (1.0 - x * x).sqrt())
}

pub(super) fn native_acos(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx acos(x) = -1 / sqrt(1 - x^2)
    unary_float_dual(state, "acos", f64::acos, |x| -1.0 / (1.0 - x * x).sqrt())
}

pub(super) fn native_atan(state: &mut PetalCxt) -> Result<u32, String> {
    // d/dx atan(x) = 1 / (1 + x^2)
    unary_float_dual(state, "atan", f64::atan, |x| 1.0 / (1.0 + x * x))
}

/// `hypot(x, y)`: `sqrt(x*x + y*y)`, without the intermediate overflow.
pub(super) fn native_hypot(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "hypot")?;
    let ([x, y], dual) = args_d::<2>(state)?;
    // d hypot(x, y) = (x dx + y dy) / hypot(x, y), and 0 at the origin.
    let h = x.v.hypot(y.v);
    let d = if h == 0.0 { 0.0 } else { (x.v * x.d + y.v * y.d) / h };
    push_d(state, D { v: h, d }, dual);
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

// ── Piecewise-linear tables and stateless hashes ────────────────────────────

/// The `(x, y)` of one knot of an `interp` table: a two-number list or a vec2.
fn knot(state: &PetalCxt, v: Value, i: usize) -> Result<(f64, f64), String> {
    let pair = match v {
        Value::Vec2(x, y) => Some((x, y)),
        Value::List(id) => match state.heap().get_list(id) {
            [x, y] => x.as_f64().zip(y.as_f64()),
            _ => None,
        },
        _ => None,
    };
    pair.ok_or_else(|| {
        format!("interp(table, x) expects each entry of the table to be [x, y], got entry {i} that is not")
    })
}

/// `interp(table, x)` — the piecewise-linear function through the knots
/// `[[x0, y0], [x1, y1], …]` (ascending `x`), read at `x`. Before the first
/// knot it is the first `y`, after the last it is the last `y`: it clamps,
/// never extrapolates. Two knots at one `x` are a step, which takes the
/// earlier `y` at exactly that `x`. Always a float.
pub(super) fn native_interp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "interp")?;
    let Value::List(table) = state.get_value(1)? else {
        return Err("interp(table, x) expects a list of [x, y] entries".into());
    };
    let (D { v: x, d: dx }, dual) = arg_d(state, 2)?;
    let n = state.heap().get_list(table).len();
    if n == 0 {
        return Err("interp(table, x) needs a table with at least one entry".into());
    }
    let at = |state: &PetalCxt, i: usize| knot(state, state.heap().get_list(table)[i], i);
    // The first knot at or past `x`, by bisection: `lo` ends as its index.
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if at(state, mid)?.0 < x {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // The value, and the slope of the segment `x` is on (0 past either end
    // and at a step) for a dual `x`.
    let (y, slope) = if lo == 0 {
        (at(state, 0)?.1, 0.0)
    } else if lo == n {
        (at(state, n - 1)?.1, 0.0)
    } else {
        let (x0, y0) = at(state, lo - 1)?;
        let (x1, y1) = at(state, lo)?;
        if x1 == x0 {
            (y1, 0.0)
        } else {
            let slope = (y1 - y0) / (x1 - x0);
            (y0 + (y1 - y0) * ((x - x0) / (x1 - x0)), slope)
        }
    };
    push_d(state, D { v: y, d: slope * dx }, dual);
    Ok(1)
}

/// One round of SplitMix64 (Steele, Lea and Flood, "Fast splittable
/// pseudorandom number generators", 2014): the finalizer the hashes below are
/// built from. Fixed constants, wrapping arithmetic, no state.
pub(crate) fn splitmix64(x: u64) -> u64 {
    let x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A number as hash input: its value as an IEEE-754 double, so `2` and `2.0`
/// hash alike, with `-0.0` taken as `0.0`.
fn number_bits(v: Value, who: &str) -> Result<u64, String> {
    match v.as_f64() {
        Some(d) => Ok(if d == 0.0 { 0.0f64 } else { d }.to_bits()),
        None => Err(format!("{who}() expects numbers, got {}", v.type_name())),
    }
}

/// The 64-bit hash of a call's one to four number arguments: start from
/// `0x243f6a8885a308d3` and fold each argument in with
/// `h = splitmix64(h ^ bits(arg))`.
fn hash_number_args(state: &PetalCxt, who: &str) -> Result<u64, String> {
    let argc = state.arg_count();
    if !(1..=4).contains(&argc) {
        return Err(format!("{who}() expects 1 to 4 arguments (numbers), got {argc}"));
    }
    let mut h = 0x243f_6a88_85a3_08d3u64;
    for i in 1..=argc {
        h = splitmix64(h ^ number_bits(state.get_value(i)?, who)?);
    }
    Ok(h)
}

/// `hash_int(a, b?, c?, d?)` — an int in `[0, 2^53)` that depends only on the
/// arguments: the top 53 bits of [`hash_numbers`]. The same on every machine
/// and in every run; the result is exact as a float, and is a seed itself.
pub(super) fn native_hash_int(state: &mut PetalCxt) -> Result<u32, String> {
    let h = hash_number_args(state, "hash_int")?;
    state.push_int((h >> 11) as i64);
    Ok(1)
}

/// `hash01(a, b?, c?, d?)` — the same hash as a float in `[0, 1)`:
/// `hash_int(...) / 2^53`.
pub(super) fn native_hash01(state: &mut PetalCxt) -> Result<u32, String> {
    let h = hash_number_args(state, "hash01")?;
    state.push_float((h >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0));
    Ok(1)
}

/// The 64-bit hash of a string's UTF-8 bytes and an optional salt: start from
/// `0x452821e638d01377`, fold in the salt (if any) and then the byte length
/// with `h = splitmix64(h ^ v)`, then each 8-byte little-endian word of the
/// text (the last one zero-padded) the same way.
pub(crate) fn hash_bytes(text: &[u8], salt: Option<f64>) -> u64 {
    let mut h = 0x4528_21e6_38d0_1377u64;
    if let Some(s) = salt {
        h = splitmix64(h ^ if s == 0.0 { 0.0f64 } else { s }.to_bits());
    }
    h = splitmix64(h ^ text.len() as u64);
    for chunk in text.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        h = splitmix64(h ^ u64::from_le_bytes(word));
    }
    h
}

/// `hash_str(text, salt?)` — an int in `[0, 2^53)` from a string (an id, a
/// name) and an optional number: the top 53 bits of [`hash_bytes`].
pub(super) fn native_hash_str(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    if !(1..=2).contains(&argc) {
        return Err(format!(
            "hash_str() expects 1 or 2 arguments (text, salt), got {argc}"
        ));
    }
    let salt = match if argc == 2 { state.get_value(2)? } else { Value::Nil } {
        Value::Nil => None,
        v => Some(f64::from_bits(number_bits(v, "hash_str")?)),
    };
    let h = match state.get_value(1)? {
        Value::String(id) => hash_bytes(state.heap().get_string(id).as_bytes(), salt),
        other => {
            return Err(format!(
                "hash_str() expects a string, got {}",
                other.type_name()
            ));
        }
    };
    state.push_int((h >> 11) as i64);
    Ok(1)
}
