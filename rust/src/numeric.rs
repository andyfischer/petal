//! Pure numeric semantics of the core language: checked integer arithmetic,
//! negation, the exact Int/Float ordering behind `==` and `<`, the total order
//! `sort` uses, the hash key that keyed state uses, and list index resolution.
//!
//! Everything here is a pure function over machine scalars with no heap, no
//! allocation and no formatting, so it can be proven, not just tested: the
//! Kani harnesses in `crate::proofs` check each contract below for *every*
//! input (see docs/dev/formal-verification.md). The value-level operations in
//! `backend::ops`, `value` and `builtins` call into this module rather than
//! re-deriving the rules, so the proofs cover what programs actually run.

use std::cmp::Ordering;

/// A numeric scalar: the two representations `==` and `<` compare across.
#[derive(Clone, Copy, Debug)]
pub enum Num {
    Int(i64),
    Float(f64),
}

/// Integer arithmetic operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Why an integer operation has no result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntArithError {
    DivisionByZero,
    Overflow,
}

/// Checked integer arithmetic. Contract (proven): never panics; returns
/// `Ok(r)` exactly when the mathematical result is an integer in `i64` range
/// (Div/Mod truncate toward zero, as Rust does), and then `r` is that result;
/// a zero divisor is `DivisionByZero`, anything else out of range `Overflow`.
pub fn int_arith(op: IntOp, a: i64, b: i64) -> Result<i64, IntArithError> {
    let result = match op {
        IntOp::Add => a.checked_add(b),
        IntOp::Sub => a.checked_sub(b),
        IntOp::Mul => a.checked_mul(b),
        // checked_div returns None for a zero divisor and for i64::MIN / -1,
        // whose quotient 2^63 does not fit.
        IntOp::Div => a.checked_div(b),
        // Not checked_rem: it also returns None for i64::MIN % -1, but that
        // remainder is 0, which fits. wrapping_rem is exact for every nonzero
        // divisor (only the quotient of MIN / -1 wraps, and it is discarded).
        IntOp::Mod => (b != 0).then(|| a.wrapping_rem(b)),
    };
    result.ok_or(if b == 0 && matches!(op, IntOp::Div | IntOp::Mod) {
        IntArithError::DivisionByZero
    } else {
        IntArithError::Overflow
    })
}

/// Checked integer negation: `-i64::MIN` is not representable.
pub fn int_neg(a: i64) -> Result<i64, IntArithError> {
    a.checked_neg().ok_or(IntArithError::Overflow)
}

/// Checked absolute value: `abs(i64::MIN)` is not representable.
pub fn int_abs(a: i64) -> Result<i64, IntArithError> {
    a.checked_abs().ok_or(IntArithError::Overflow)
}

/// Exact comparison of an integer with a float, with no rounding: `None` only
/// when `f` is NaN. A plain `(i as f64).partial_cmp(&f)` rounds `i` to 53 bits,
/// which makes `2^53 + 1 == 2^53 as float` true and `==` non-transitive.
pub fn cmp_int_float(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    // 2^63 is exactly representable; every i64 is below it and at or above -2^63.
    const TWO_63: f64 = 9223372036854775808.0;
    if f >= TWO_63 {
        return Some(Ordering::Less);
    }
    if f < -TWO_63 {
        return Some(Ordering::Greater);
    }
    // Now -2^63 <= f < 2^63, so its integer part is an exact i64 and the
    // fractional part `f - t` is computed exactly.
    let t = f.trunc();
    match i.cmp(&(t as i64)) {
        Ordering::Equal => {
            let frac = f - t;
            Some(if frac > 0.0 {
                Ordering::Less
            } else if frac < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            })
        }
        ord => Some(ord),
    }
}

/// Exact numeric comparison; `None` iff either side is NaN. This is the
/// ordering behind `==`, `<`, `<=`, `>`, `>=`, `min` and `max` on numbers.
pub fn num_cmp(a: Num, b: Num) -> Option<Ordering> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
        (Num::Int(x), Num::Float(y)) => cmp_int_float(x, y),
        (Num::Float(x), Num::Int(y)) => cmp_int_float(y, x).map(Ordering::reverse),
    }
}

/// Numeric `==`: an equivalence on non-NaN numbers, and NaN equals nothing.
pub fn num_eq(a: Num, b: Num) -> bool {
    num_cmp(a, b) == Some(Ordering::Equal)
}

/// The total order `sort` uses on numbers: [`num_cmp`], with every NaN equal
/// to every other NaN and after all other numbers. `sort` must not use
/// `num_cmp` with NaN mapped to `Equal` — that is not transitive
/// (`1 = NaN = 2` but `1 < 2`), and the standard library's sort panics on it.
pub fn num_total_cmp(a: Num, b: Num) -> Ordering {
    let a_nan = matches!(a, Num::Float(f) if f.is_nan());
    let b_nan = matches!(b, Num::Float(f) if f.is_nan());
    match (a_nan, b_nan) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => num_cmp(a, b).unwrap_or(Ordering::Equal),
    }
}

/// A canonical key for hashing a number consistently with [`num_eq`]: if two
/// numbers are `==`, their keys are equal. Integral floats in `i64` range key
/// as that integer (so `2`, `2.0` and `-0.0 == 0` agree); other floats key by
/// their bits, with every NaN sharing one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumKey {
    Int(i64),
    FloatBits(u64),
}

pub fn num_key(n: Num) -> NumKey {
    match n {
        Num::Int(i) => NumKey::Int(i),
        Num::Float(f) => {
            if f.is_nan() {
                return NumKey::FloatBits(f64::NAN.to_bits());
            }
            let t = f.trunc();
            if t == f && (-9223372036854775808.0..9223372036854775808.0).contains(&f) {
                NumKey::Int(f as i64)
            } else {
                NumKey::FloatBits(f.to_bits())
            }
        }
    }
}

/// Resolve a possibly-negative list index against a list of length `len`:
/// `i >= 0` addresses `i`, `i < 0` addresses `len + i`. `None` when the
/// resolved position is outside `0..len`. Shared by index reads and writes so
/// `xs[i]` and `xs[i] = v` address the same slot.
pub fn resolve_index(len: usize, i: i64) -> Option<usize> {
    let k = if i < 0 {
        // |i| <= 2^63 fits in u64; the slot exists only when |i| <= len.
        let back = i.unsigned_abs();
        if back > len as u64 {
            return None;
        }
        len - back as usize
    } else {
        // Not `i as usize`: on 32-bit targets (wasm32) that truncates, and
        // `xs[2^32 + 1]` would silently read `xs[1]`.
        usize::try_from(i).ok()?
    };
    (k < len).then_some(k)
}

/// Clamp a possibly-negative slice bound into `0..=len`: `i >= 0` is `min(i,
/// len)`, `i < 0` counts from the end and stops at 0. Used by `slice` and the
/// char-indexed string builtins; never truncates on 32-bit targets.
pub fn clamp_slice_bound(len: usize, i: i64) -> usize {
    if i < 0 {
        len.saturating_sub(usize::try_from(i.unsigned_abs()).unwrap_or(usize::MAX))
    } else {
        usize::try_from(i).map_or(len, |k| k.min(len))
    }
}

/// A non-negative index strictly below `len`, or `None`. For containers that
/// do not accept negative indices (f64 arrays).
pub fn checked_index(len: usize, i: i64) -> Option<usize> {
    usize::try_from(i).ok().filter(|&k| k < len)
}

/// The number of elements of `range(start, end, step)`: how many of `start`,
/// `start + step`, `start + 2*step`, … lie before `end` (below it for a
/// positive step, above it for a negative one). `step` must be nonzero. Exact
/// for every input — the count can exceed `i64::MAX` (it is at most `2^64 - 1`)
/// — so a caller can refuse an impossible allocation up front.
pub fn range_len(start: i64, end: i64, step: i64) -> u64 {
    debug_assert!(step != 0);
    // The distance to cover, which fits in u64 (at most 2^64 - 1).
    let span = if step > 0 {
        if end <= start {
            return 0;
        }
        end.abs_diff(start)
    } else {
        if start <= end {
            return 0;
        }
        start.abs_diff(end)
    };
    // Ceiling division with a single divide (span >= 1 here); one division
    // instead of a divide and a remainder also halves the proof's cost.
    (span - 1) / step.unsigned_abs() + 1
}

/// Element `k` of `range(start, end, step)`, for `k < range_len(...)`; always
/// in `i64` range under that precondition.
pub fn range_nth(start: i64, step: i64, k: u64) -> i64 {
    (start as i128 + k as i128 * step as i128) as i64
}

/// Map a uniform `u` in `[0, 1)` onto the integers of `[lo, hi)` (`lo < hi`),
/// the body of `random_int`. The span `hi - lo` can exceed `i64::MAX`, and a
/// float product can round up to the span itself, so both are handled: the
/// result is always in `[lo, hi)`.
pub fn scale_unit_to_range(u: f64, lo: i64, hi: i64) -> i64 {
    debug_assert!(lo < hi);
    let span = (hi as i128 - lo as i128) as u128;
    // `as` saturates (and maps NaN to 0), so this is in `0..=span`; clamp the
    // round-up case back inside.
    let offset = ((u * span as f64) as u128).min(span - 1);
    (lo as i128 + offset as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `range_len` is division-bound, too slow for the model checker, so it
    /// is checked exhaustively instead: every small range against a naive
    /// count, and every combination of i64 extremes against the exact (i128)
    /// characterization — the last element is before `end`, the next is not.
    #[test]
    fn range_len_is_exact() {
        let naive = |start: i64, end: i64, step: i64| {
            let (mut n, mut x) = (0u64, start as i128);
            while (step > 0 && x < end as i128) || (step < 0 && x > end as i128) {
                n += 1;
                x += step as i128;
            }
            n
        };
        for start in -12..=12 {
            for end in -12..=12 {
                for step in (-6..=6).filter(|&s| s != 0) {
                    assert_eq!(range_len(start, end, step), naive(start, end, step));
                }
            }
        }
        let edges = [
            i64::MIN,
            i64::MIN + 1,
            -3,
            -1,
            0,
            1,
            2,
            3,
            i64::MAX - 1,
            i64::MAX,
        ];
        for &start in &edges {
            for &end in &edges {
                for &step in edges.iter().filter(|&&s| s != 0) {
                    let n = range_len(start, end, step) as i128;
                    let (s, e, st) = (start as i128, end as i128, step as i128);
                    let before_end = |x: i128| if st > 0 { x < e } else { x > e };
                    if n > 0 {
                        assert!(before_end(s + (n - 1) * st));
                        assert_eq!(
                            range_nth(start, step, (n - 1) as u64) as i128,
                            s + (n - 1) * st
                        );
                    }
                    assert!(!before_end(s + n * st), "{start} {end} {step}");
                }
            }
        }
    }
}
