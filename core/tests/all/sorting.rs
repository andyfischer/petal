//! `sort(list, compare)` and `sort_by(list, key, descending)` — the two
//! closure-driven sorts (`Vm::builtin_sort_cmp`, `Vm::builtin_sort_by`).

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

/// Rows whose `v` ties twice, so a sort that is not stable shows.
const ROWS: &str = "let rows = [{n: \"a\", v: 3, w: 1}, {n: \"b\", v: 1, w: 2}, \
                    {n: \"c\", v: 3, w: 0}, {n: \"d\", v: 1, w: 9}]
fn names(rs)
  join(map(rs, fn(r) -> r.n), \"\")
end
";

fn rows(expr: &str) -> Vec<String> {
    run(&format!("{ROWS}print(names({expr}))"))
}

#[test]
fn a_comparator_may_answer_a_number() {
    assert_eq!(run("print(sort([2, 3, 1], fn(a, b) -> a - b))"), ["[1, 2, 3]"]);
    assert_eq!(run("print(sort([2, 3, 1], fn(a, b) -> b - a))"), ["[3, 2, 1]"]);
    assert_eq!(run("print(sort([2.5, 0.5], fn(a, b) -> a - b))"), ["[0.5, 2.5]"]);
}

#[test]
fn a_comparator_may_answer_a_bool() {
    assert_eq!(run("print(sort([2, 3, 1], fn(a, b) -> a < b))"), ["[1, 2, 3]"]);
    assert_eq!(run("print(sort([\"b\", \"a\"], fn(a, b) -> a < b))"), ["[\"a\", \"b\"]"]);
}

#[test]
fn a_comparator_sort_is_stable() {
    assert_eq!(rows("sort(rows, fn(a, b) -> a.v - b.v)"), ["bdac"]);
    assert_eq!(rows("sort(rows, fn(a, b) -> b.v - a.v)"), ["acbd"]);
    assert_eq!(rows("sort(rows, fn(a, b) -> a.v < b.v)"), ["bdac"]);
}

#[test]
fn a_comparator_must_answer_a_number_or_a_bool() {
    let e = run_err("print(sort([2, 1], fn(a, b) -> \"x\"))");
    assert!(e.contains("comparator must return a number"), "{e}");
}

#[test]
fn sort_by_orders_by_a_number_or_a_string_key() {
    assert_eq!(rows("sort_by(rows, fn(r) -> r.w)"), ["cabd"]);
    assert_eq!(rows("sort_by(rows, fn(r) -> r.n, true)"), ["dcba"]);
}

#[test]
fn sort_by_is_stable_in_both_directions() {
    assert_eq!(rows("sort_by(rows, fn(r) -> r.v)"), ["bdac"]);
    assert_eq!(rows("sort_by(rows, fn(r) -> r.v, true)"), ["acbd"]);
    assert_eq!(rows("sort_by(rows, fn(r) -> r.v, \"desc\")"), ["acbd"]);
    assert_eq!(rows("sort_by(rows, fn(r) -> r.v, \"asc\")"), ["bdac"]);
}

/// Stability is what makes chaining the way to sort by several keys: the last
/// `sort_by` is the primary key.
#[test]
fn chained_sort_by_composes_keys() {
    assert_eq!(
        rows("sort_by(sort_by(rows, fn(r) -> r.w), fn(r) -> r.v, \"desc\")"),
        ["cabd"]
    );
}

/// A key that is neither a number nor a string has no order. It used to rank
/// equal to every other such key, which returned the list unsorted.
#[test]
fn sort_by_rejects_a_key_it_cannot_order() {
    for key in ["[r.v, r.w]", "r", "nil", "r.v > 1"] {
        let e = run_err(&format!("{ROWS}print(sort_by(rows, fn(r) -> {key}))"));
        assert!(e.contains("key function must return a number or a string"), "{key}: {e}");
    }
    let e = run_err(&format!("{ROWS}print(sort_by(rows, fn(r) -> [r.v, r.w]))"));
    assert!(e.contains("got list"), "{e}");
}

#[test]
fn sort_by_of_an_empty_list_calls_nothing() {
    assert_eq!(run("print(sort_by([], fn(r) -> [r]))"), ["[]"]);
}

#[test]
fn sort_by_rejects_an_unknown_direction() {
    let e = run_err("print(sort_by([1], fn(x) -> x, \"sideways\"))");
    assert!(e.contains("direction"), "{e}");
}
