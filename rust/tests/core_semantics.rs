//! End-to-end regression tests for the core-semantics bugs found by the
//! formal-verification pilot (docs/dev/formal-verification.md). The kernels are
//! proven in `src/proofs/`; these pin the behavior a Petal program sees.

use petal::env::Env;

fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.run_source(src)
        .unwrap_or_else(|e| panic!("run failed: {e}\n{src}"));
    env.take_output()
}

fn run_err(src: &str) -> String {
    Env::new()
        .run_source(src)
        .expect_err("expected a runtime error")
}

#[test]
fn records_compare_by_content() {
    let out = run("let m = {a: 1}
print(m == m, {a: 1, b: 2} == {b: 2, a: 1}, {a: 1} == {a: 2}, {a: 1} != {a: 1})");
    assert_eq!(out, ["true true false false"]);
}

#[test]
fn a_class_instance_only_equals_its_own_class() {
    let out = run("class P
  x: int
end
class Q
  x: int
end
print(P(1) == P(1), P(1) == Q(1), P(1) == {x: 1})");
    assert_eq!(out, ["true false false"]);
}

#[test]
fn int_float_equality_is_exact() {
    let out = run("print(9007199254740993 == 9007199254740992.0, \
                   9007199254740992 == 9007199254740992.0, 2 == 2.0, 0 == -0.0)");
    assert_eq!(out, ["false true true true"]);
    let out = run("print(9007199254740993 > 9007199254740992.0, 3 < 3.5, -3 > -3.5)");
    assert_eq!(out, ["true true true"]);
}

#[test]
fn every_ordering_with_nan_is_false() {
    let out = run("let n = sqrt(-1.0)
print(n < 1, n <= 1, n > 1, n >= 1, n == n, n != n)");
    assert_eq!(out, ["false false false false false true"]);
}

#[test]
fn sort_puts_nan_last_instead_of_aborting() {
    // Enough elements that the standard library's sort detects an
    // inconsistent comparator (it panicked on the old NaN-is-Equal order).
    let out = run("let n = sqrt(-1.0)
print(sort([n, 3, 1, n, 2, 5, n, 4, 0, 9, 8, 7, 6, 11, 10, 12, 13, 14, 15, 16, 17, 18, 19, 20, n, 1.5]))");
    assert_eq!(
        out,
        [
            "[0, 1, 1.5, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, NaN, NaN, NaN, NaN]"
        ]
    );
}

#[test]
fn keyed_state_treats_equal_keys_as_one_slot() {
    let out = run("fn f(k)
  state(k) n = 0
  n += 1
  n
end
for i in range(3) do
  print(f({id: 1}), f(2), f(2.0))
end");
    assert_eq!(out, ["1 1 2", "2 3 4", "3 5 6"]);
}

#[test]
fn smallest_int_negation_and_abs_are_errors_not_panics() {
    let min = "let x = -9223372036854775807 - 1\n";
    assert!(run_err(&format!("{min}print(-x)")).contains("Integer overflow when trying to negate"));
    assert!(run_err(&format!("{min}print(abs(x))")).contains("Integer overflow in abs()"));
}

#[test]
fn smallest_int_mod_minus_one_is_zero() {
    let out = run("print((-9223372036854775807 - 1) % -1, -7 % 3, 7 % -3)");
    assert_eq!(out, ["0 -1 1"]);
}

#[test]
fn random_int_accepts_the_full_int_range() {
    let out = run(
        "let r = random_int(-9223372036854775807 - 1, 9223372036854775807)
let s = random_int(-1, 9223372036854775807)
print(type(r), s >= -1)",
    );
    assert_eq!(out, ["int true"]);
}

#[test]
fn huge_allocations_are_errors_not_aborts() {
    let big = "9223372036854775807";
    for src in [
        format!("print(range({big}))"),
        format!("print(range(0, {big}, 2))"),
        format!("print(f64_array({big}))"),
        format!("print(pad_start(\"a\", {big}))"),
    ] {
        assert!(run_err(&src).contains("too large"), "{src}");
    }
    let out =
        run("print(range(-9223372036854775807 - 1, 9223372036854775807, 9223372036854775807))");
    assert_eq!(out, ["[-9223372036854775808, -1, 9223372036854775806]"]);
}

#[test]
fn numeric_literal_patterns_match_what_they_equal() {
    let out = run("fn m(x)
  match x
    when 2 -> \"two\"
    when -0.0 -> \"zero\"
    when _ -> \"other\"
  end
end
print(m(2.0), m(2), m(0), m(\"2\"))");
    assert_eq!(out, ["two two zero other"]);
}
