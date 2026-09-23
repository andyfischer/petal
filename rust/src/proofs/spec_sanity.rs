//! Spec sanity checks: each harness runs a property from `numeric.rs` against
//! the implementation that shipped before it was proven, and is marked
//! `should_panic` — Kani must find a counterexample. They guard against a
//! vacuous spec (one that would pass any implementation) and record the bug
//! each proof was written to rule out.

use std::cmp::Ordering;

/// Pre-fix `==` on an int and a float: rounds the int to f64 first.
fn old_int_float_eq(i: i64, f: f64) -> bool {
    (i as f64) == f
}

/// `==` was not transitive: `2^53 + 1 == 2^53 as float == 2^53`, but
/// `2^53 + 1 != 2^53`.
#[kani::proof]
#[kani::should_panic]
fn old_int_float_eq_was_not_transitive() {
    let a: i64 = kani::any();
    let f: f64 = kani::any();
    let c: i64 = kani::any();
    if old_int_float_eq(a, f) && old_int_float_eq(c, f) {
        assert!(a == c);
    }
}

/// Pre-fix sort comparator: NaN compares `Equal` to everything.
fn old_sort_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// It was not transitive (`1 = NaN = 2`, `1 < 2`), and `sort_by` panics on
/// such a comparator.
#[kani::proof]
#[kani::should_panic]
fn old_sort_cmp_was_not_transitive() {
    let a: f64 = kani::any();
    let b: f64 = kani::any();
    let c: f64 = kani::any();
    if old_sort_cmp(a, b) != Ordering::Greater && old_sort_cmp(b, c) != Ordering::Greater {
        assert!(old_sort_cmp(a, c) != Ordering::Greater);
    }
}

/// Pre-fix `%` reported `i64::MIN % -1` (which is 0) as an overflow.
#[kani::proof]
#[kani::should_panic]
fn old_int_mod_rejected_a_representable_remainder() {
    let a: i64 = kani::any();
    let b: i64 = kani::any();
    kani::assume(b != 0);
    assert!(a.checked_rem(b).is_some());
}

/// Pre-fix unary minus: `-n` overflows (panics in debug, wraps in release).
#[kani::proof]
#[kani::should_panic]
fn old_negate_overflowed() {
    let a: i64 = kani::any();
    let _ = -a;
}

/// Pre-fix `random_int`: `max - min` overflows for a wide range.
#[kani::proof]
#[kani::should_panic]
fn old_random_int_span_overflowed() {
    let lo: i64 = kani::any();
    let hi: i64 = kani::any();
    kani::assume(lo < hi);
    let _ = hi - lo;
}
