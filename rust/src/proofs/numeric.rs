//! Proofs of the contracts in `crate::numeric`, for every input.

use std::cmp::Ordering;

use crate::numeric::*;

fn any_num() -> Num {
    if kani::any() {
        Num::Int(kani::any())
    } else {
        Num::Float(kani::any())
    }
}

fn is_nan(n: Num) -> bool {
    matches!(n, Num::Float(f) if f.is_nan())
}

// ── Integer arithmetic ──────────────────────────────────────────

/// `int_arith` agrees with exact (i128) arithmetic: it succeeds exactly when
/// the true result fits in i64, returns that result, and reports a zero
/// divisor as `DivisionByZero`.
fn check_int_arith(op: IntOp) {
    let a: i64 = kani::any();
    let b: i64 = kani::any();
    let (wa, wb) = (a as i128, b as i128);
    let exact = match op {
        IntOp::Add => wa + wb,
        IntOp::Sub => wa - wb,
        IntOp::Mul => wa * wb,
        IntOp::Div | IntOp::Mod => unreachable!("see int_div_is_exact / int_mod_is_exact"),
    };
    match int_arith(op, a, b) {
        Ok(r) => assert!(r as i128 == exact),
        Err(IntArithError::Overflow) => {
            assert!(exact < i64::MIN as i128 || exact > i64::MAX as i128)
        }
        Err(IntArithError::DivisionByZero) => panic!("no divisor"),
    }
}

#[kani::proof]
fn int_add_is_exact() {
    check_int_arith(IntOp::Add);
}

#[kani::proof]
fn int_sub_is_exact() {
    check_int_arith(IntOp::Sub);
}

#[kani::proof]
fn int_mul_is_exact() {
    check_int_arith(IntOp::Mul);
}

/// Truncated division, specified without dividing (128-bit division is too
/// costly to bit-blast): `q` is the quotient iff `a = q*b + r` with `|r| < |b|`
/// and `r` zero or of `a`'s sign — a characterization with exactly one
/// solution. The only unrepresentable quotient is `i64::MIN / -1 = 2^63`.
#[kani::proof]
#[kani::solver(cadical)]
fn int_div_is_exact() {
    let a: i64 = kani::any();
    let b: i64 = kani::any();
    match int_arith(IntOp::Div, a, b) {
        Err(IntArithError::DivisionByZero) => assert!(b == 0),
        Err(IntArithError::Overflow) => assert!(a == i64::MIN && b == -1),
        Ok(q) => {
            assert!(b != 0);
            let r = a as i128 - q as i128 * b as i128;
            assert!(r.abs() < (b as i128).abs());
            assert!(r == 0 || (r < 0) == (a < 0));
        }
    }
}

/// `%` is the machine's truncated remainder (Rust's `%`, `-7 % 3 == -1`) for
/// every nonzero divisor, and never overflows — including `i64::MIN % -1`,
/// which is 0 (`checked_rem` alone reports it as overflow). Unlike the other
/// arithmetic proofs this one does not re-derive the remainder from a
/// reference: a second 64-bit division in the spec takes the solver over an
/// hour. The remainder's value is the `%` primitive's; what is proven is that
/// `int_arith` returns it, or the right error, for every input.
#[kani::proof]
fn int_mod_is_the_remainder() {
    let a: i64 = kani::any();
    let b: i64 = kani::any();
    match int_arith(IntOp::Mod, a, b) {
        Err(IntArithError::DivisionByZero) => assert!(b == 0),
        Err(IntArithError::Overflow) => panic!("a remainder always fits"),
        Ok(m) => assert!(b != 0 && m == a.wrapping_rem(b)),
    }
}

#[kani::proof]
fn int_neg_is_exact() {
    let a: i64 = kani::any();
    match int_neg(a) {
        Ok(r) => assert!(r as i128 == -(a as i128)),
        Err(_) => assert!(a == i64::MIN),
    }
}

#[kani::proof]
fn int_abs_is_exact() {
    let a: i64 = kani::any();
    match int_abs(a) {
        Ok(r) => assert!(r as i128 == (a as i128).abs()),
        Err(_) => assert!(a == i64::MIN),
    }
}

// ── Int/Float comparison ────────────────────────────────────────

/// `cmp_int_float` is the exact mathematical comparison of `i` with `f`:
/// `i < f` iff `i < ceil(f)`, `i > f` iff `i > floor(f)`, for every finite `f`.
#[kani::proof]
fn cmp_int_float_is_exact() {
    let i: i64 = kani::any();
    let f: f64 = kani::any();
    let r = cmp_int_float(i, f);
    if f.is_nan() {
        assert!(r.is_none());
        return;
    }
    let r = r.unwrap();
    // Beyond ±2^70 every i64 is strictly on one side (also covers ±inf).
    const BIG: f64 = 1180591620717411303424.0; // 2^70
    if f >= BIG {
        assert!(r == Ordering::Less);
    } else if f <= -BIG {
        assert!(r == Ordering::Greater);
    } else {
        // |f| < 2^70: floor and ceil are integers that fit in i128 exactly.
        let lo = f.floor() as i128;
        let hi = f.ceil() as i128;
        let wi = i as i128;
        assert!((r == Ordering::Less) == (wi < hi));
        assert!((r == Ordering::Greater) == (wi > lo));
        assert!((r == Ordering::Equal) == (lo == hi && wi == lo));
    }
}

/// `num_cmp(a, b)` is `num_cmp(b, a)` reversed, and undefined only for NaN.
#[kani::proof]
fn num_cmp_is_antisymmetric() {
    let a = any_num();
    let b = any_num();
    assert!(num_cmp(a, b) == num_cmp(b, a).map(Ordering::reverse));
    assert!(num_cmp(a, b).is_none() == (is_nan(a) || is_nan(b)));
}

/// `==` on numbers is transitive — the property the old `i as f64` rounding
/// broke: `2^53 + 1 == 2^53.0` and `2^53.0 == 2^53`, but `2^53 + 1 != 2^53`.
#[kani::proof]
#[kani::solver(cadical)]
fn num_eq_is_transitive() {
    let a = any_num();
    let b = any_num();
    let c = any_num();
    if num_eq(a, b) && num_eq(b, c) {
        assert!(num_eq(a, c));
    }
}

/// `<` on numbers is transitive.
#[kani::proof]
#[kani::solver(cadical)]
fn num_lt_is_transitive() {
    let a = any_num();
    let b = any_num();
    let c = any_num();
    if num_cmp(a, b) == Some(Ordering::Less) && num_cmp(b, c) == Some(Ordering::Less) {
        assert!(num_cmp(a, c) == Some(Ordering::Less));
    }
}

/// `<` and `==` compose: `a == b < c` implies `a < c`.
#[kani::proof]
#[kani::solver(cadical)]
fn num_eq_lt_compose() {
    let a = any_num();
    let b = any_num();
    let c = any_num();
    if num_eq(a, b) && num_cmp(b, c) == Some(Ordering::Less) {
        assert!(num_cmp(a, c) == Some(Ordering::Less));
    }
}

// ── The sort order ──────────────────────────────────────────────

/// `num_total_cmp` is a total order, which `slice::sort_by` requires (it
/// panics on a comparator that is not). Reflexivity and antisymmetry here;
/// with transitivity of `<=` below they give every total-order law (a strict
/// `<` step in a chain follows: `a < b <= c` with `a == c` would force
/// `b <= a`, contradicting antisymmetry).
#[kani::proof]
fn num_total_cmp_is_antisymmetric() {
    let a = any_num();
    let b = any_num();
    assert!(num_total_cmp(a, a) == Ordering::Equal);
    assert!(num_total_cmp(a, b) == num_total_cmp(b, a).reverse());
}

/// `a <= b <= c` implies `a <= c` in the sort order.
#[kani::proof]
#[kani::solver(cadical)]
fn num_total_cmp_is_transitive() {
    let a = any_num();
    let b = any_num();
    let c = any_num();
    if num_total_cmp(a, b) != Ordering::Greater && num_total_cmp(b, c) != Ordering::Greater {
        assert!(num_total_cmp(a, c) != Ordering::Greater);
    }
}

/// The sort order refines `<`: whenever `a < b`, `a` sorts first.
#[kani::proof]
fn num_total_cmp_agrees_with_lt() {
    let a = any_num();
    let b = any_num();
    if let Some(o) = num_cmp(a, b) {
        assert!(num_total_cmp(a, b) == o);
    }
}

// ── Hash keys ───────────────────────────────────────────────────

/// Keys agree with `==`: equal numbers key equally (so keyed state and
/// hashing are consistent with equality), and equal keys mean equal numbers
/// or two NaNs.
#[kani::proof]
fn num_key_matches_eq() {
    let a = any_num();
    let b = any_num();
    if num_eq(a, b) {
        assert!(num_key(a) == num_key(b));
    }
    if num_key(a) == num_key(b) {
        assert!(num_eq(a, b) || (is_nan(a) && is_nan(b)));
    }
}

// ── Indexing ────────────────────────────────────────────────────

/// `resolve_index` addresses exactly slot `i` (or `len + i` for negative `i`)
/// when that slot exists, and nothing otherwise.
#[kani::proof]
fn resolve_index_is_exact() {
    let len: usize = kani::any();
    let i: i64 = kani::any();
    let want = if i < 0 {
        len as i128 + i as i128
    } else {
        i as i128
    };
    match resolve_index(len, i) {
        Some(k) => {
            assert!(k < len);
            assert!(k as i128 == want);
        }
        None => assert!(want < 0 || want >= len as i128),
    }
}

/// `clamp_slice_bound` is the resolved bound clamped into `0..=len`.
#[kani::proof]
fn clamp_slice_bound_is_exact() {
    let len: usize = kani::any();
    let i: i64 = kani::any();
    let want = if i < 0 {
        len as i128 + i as i128
    } else {
        i as i128
    };
    let want = want.clamp(0, len as i128);
    assert!(clamp_slice_bound(len, i) as i128 == want);
}

#[kani::proof]
fn checked_index_is_exact() {
    let len: usize = kani::any();
    let i: i64 = kani::any();
    match checked_index(len, i) {
        Some(k) => assert!(k as i128 == i as i128 && k < len),
        None => assert!(i < 0 || i as i128 >= len as i128),
    }
}

// ── range() ─────────────────────────────────────────────────────

/// `range_len` counts exactly the progression elements before `end`: the last
/// counted element is strictly before `end`, the next one is not, and every
/// counted element (`range_nth`) is exact in i64.
#[kani::proof]
fn range_len_is_exact() {
    let start: i64 = kani::any();
    let end: i64 = kani::any();
    let step: i64 = kani::any();
    kani::assume(step != 0);
    let n = range_len(start, end, step) as i128;
    let (s, e, st) = (start as i128, end as i128, step as i128);
    let before_end = |x: i128| if st > 0 { x < e } else { x > e };
    if n > 0 {
        let last = s + (n - 1) * st;
        assert!(before_end(last));
        assert!(range_nth(start, step, (n - 1) as u64) as i128 == last);
    }
    assert!(!before_end(s + n * st));
}

/// Every element of the range is computed exactly (no i64 wraparound). The
/// precondition "`start + k*step` is before `end`" is the division-free form of
/// `k < range_len(...)` — `range_len_is_exact` proves they coincide — which
/// keeps a 64-bit division out of this proof.
#[kani::proof]
fn range_nth_is_exact() {
    let start: i64 = kani::any();
    let end: i64 = kani::any();
    let step: i64 = kani::any();
    let k: u64 = kani::any();
    kani::assume(step != 0);
    let want = start as i128 + k as i128 * step as i128;
    kani::assume(if step > 0 { want < end as i128 } else { want > end as i128 });
    assert!(range_nth(start, step, k) as i128 == want);
}

// ── random_int() ────────────────────────────────────────────────

/// `random_int(lo, hi)` stays inside its documented half-open range `[lo, hi)`
/// for every generator output and every bound pair, including spans wider than
/// `i64::MAX`.
#[kani::proof]
fn scale_unit_to_range_stays_in_range() {
    let lo: i64 = kani::any();
    let hi: i64 = kani::any();
    kani::assume(lo < hi);
    // Exactly the generator's outputs: k / 2^53 for a 53-bit k.
    let k: u64 = kani::any();
    kani::assume(k < (1u64 << 53));
    let u = k as f64 * (1.0 / (1u64 << 53) as f64);
    let r = scale_unit_to_range(u, lo, hi);
    assert!(lo <= r && r < hi);
}
