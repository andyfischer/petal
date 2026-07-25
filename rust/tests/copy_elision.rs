//! Copy-elision harness: end-to-end assertions that a mutation loop does not
//! duplicate its container's backing store.
//!
//! The unit tests in `backend::bytecode::tests` assert what the *analysis*
//! proves (`escape::analyze(...).len()`). These assert what the *run* actually
//! costs, via the `DupStats` counters the heap records on every copy-on-write —
//! so a mutation that the analysis blesses but lowering fails to route in place
//! still shows up here.
//!
//! Each shape below is a pattern real Petal programs use (a sim's particle
//! array, a builder loop, a two-pass build-then-update). A copy count of 0 is
//! the bar: a per-iteration clone makes an indexed write O(len), which is the
//! difference between a flat frame budget and one that grows with the data.
//!
//! `DupStats` collection is compiled out unless `DUP_STATS_ENABLED` (debug
//! builds, or the `dup-stats` feature) — every test here skips when it is off,
//! rather than passing vacuously against zeroed counters.

use petal::env::Env;
use petal::stats::DUP_STATS_ENABLED;

/// Elements written by each shape. Large enough that a per-iteration clone is
/// unmistakable in the counters (and quadratic in wall time), small enough to
/// stay a fast test.
const N: usize = 2000;

/// Run `code` to completion and return `(copies, bytes_copied)` recorded by the
/// heap. Panics if the program fails — a broken snippet must not read as 0
/// copies.
fn copy_cost(code: &str) -> (u64, u64) {
    let mut env = Env::new();
    let pid = env
        .load_program(code)
        .unwrap_or_else(|e| panic!("load failed: {e}\n--- source ---\n{code}"));
    let sid = env
        .create_stack(pid)
        .unwrap_or_else(|e| panic!("create_stack failed: {e}"));
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed: {e}\n--- source ---\n{code}"));
    let stats = env.dup_stats();
    (stats.total_count(), stats.total_bytes())
}

/// Assert `code` runs without duplicating any backing store.
#[track_caller]
fn assert_copy_free(shape: &str, code: &str) {
    if !DUP_STATS_ENABLED {
        return;
    }
    let (count, bytes) = copy_cost(code);
    assert_eq!(
        count, 0,
        "{shape}: expected no copies, got {count} copies ({bytes} bytes). \
         The mutation is falling back to clone-and-alloc, making each write O(len).\n\
         --- source ---\n{code}"
    );
}

/// Assert `code` DOES copy — a value-semantics guard. Some shapes must copy
/// (a live alias observes the pre-mutation value); pinning them keeps a future
/// analysis relaxation from silently breaking observable semantics.
#[track_caller]
fn assert_copies(shape: &str, code: &str) {
    if !DUP_STATS_ENABLED {
        return;
    }
    let (count, _) = copy_cost(code);
    assert!(
        count > 0,
        "{shape}: expected copy-on-write (a live alias observes the old value), got none.\n\
         --- source ---\n{code}"
    );
}

// ── Shapes that already elide ───────────────────────────────────────────────

#[test]
fn let_bound_append_accumulator_is_copy_free() {
    // The canonical builder loop, and the shape the escape analysis was built
    // for. A guard against regressing the case that works.
    assert_copy_free(
        "let-bound append accumulator",
        &format!("let xs = []\nfor i in range(0, {N}) do\n  xs = append(xs, i)\nend\nprint(len(xs))"),
    );
}

#[test]
fn let_bound_list_literal_indexed_write_is_copy_free() {
    assert_copy_free(
        "let-bound list literal, indexed write",
        &format!(
            "let xs = [0, 0, 0]\nfor i in range(0, {N}) do\n  xs[i % 3] = i\nend\nprint(xs[0])"
        ),
    );
}

// ── Gap 1: a bare (non-`let`) binding ───────────────────────────────────────

#[test]
fn bare_bound_append_accumulator_is_copy_free() {
    // `xs = []` and `let xs = []` differ only in a keyword — bare assignment
    // lowers an extra `Copy` between the `AllocList` and the loop phi's init.
    // If the analysis does not chase that Copy back to the fresh root, omitting
    // one keyword silently costs a clone per iteration.
    assert_copy_free(
        "bare-bound append accumulator",
        &format!("xs = []\nfor i in range(0, {N}) do\n  xs = append(xs, i)\nend\nprint(len(xs))"),
    );
}

#[test]
fn bare_bound_indexed_write_is_copy_free() {
    assert_copy_free(
        "bare-bound list literal, indexed write",
        &format!("xs = [0, 0, 0]\nfor i in range(0, {N}) do\n  xs[i % 3] = i\nend\nprint(xs[0])"),
    );
}

// ── Gap 2: a container whose root is a call result ──────────────────────────

#[test]
fn f64_array_indexed_write_is_copy_free() {
    // `f64_array(n)` is the ONLY way to make an f64 array, and it is a builtin
    // call. If a call result can never root a unique web, no f64 array write is
    // ever in place — and there is no script-level workaround.
    assert_copy_free(
        "f64_array indexed write",
        &format!(
            "let a = f64_array({N})\nfor i in range(0, {N}) do\n  a[i] = i * 1.0\nend\nprint(a[5])"
        ),
    );
}

#[test]
fn f64_array_set_builtin_is_copy_free() {
    // `set(a, i, v)` is the call form of `a[i] = v`; both must elide.
    assert_copy_free(
        "f64_array set() builtin",
        &format!(
            "let a = f64_array({N})\nfor i in range(0, {N}) do\n  a = set(a, i, i * 1.0)\nend\nprint(get(a, 5))"
        ),
    );
}

#[test]
fn user_function_result_root_is_copy_free() {
    // A helper that returns a freshly built container is the natural way to
    // factor setup out of a sim's main loop. The returned value is unaliased,
    // so writes to it should be in place.
    assert_copy_free(
        "user function result root",
        &format!(
            "fn build()\n  let a = f64_array({N})\n  a\nend\nlet a = build()\nfor i in range(0, {N}) do\n  a[i] = i * 1.0\nend\nprint(a[5])"
        ),
    );
}

// ── Gap 3: build in one loop, write in another ──────────────────────────────

#[test]
fn build_then_update_in_second_loop_is_copy_free() {
    // Two sequential loops over one accumulator: fill it, then update it. The
    // web spans two loop spines, which a one-spine rule rejects wholesale —
    // yet nothing outside the two loops ever observes the container.
    assert_copy_free(
        "build loop then update loop",
        &format!(
            "let xs = []\nfor i in range(0, {N}) do\n  xs = append(xs, 0)\nend\nfor i in range(0, {N}) do\n  xs[i] = i * 2\nend\nprint(xs[7])"
        ),
    );
}

// ── Value-semantics guards ──────────────────────────────────────────────────

#[test]
fn aliased_container_still_copies() {
    // `ys = xs` makes a live observer of the pre-append value: eliding here
    // would let `ys` see the mutation. This MUST keep copying.
    assert_copies(
        "aliased accumulator",
        "let xs = []\nlet ys = xs\nfor i in range(0, 8) do\n  xs = append(xs, i)\nend\nprint(len(xs), len(ys))",
    );
}

#[test]
fn elided_shapes_still_compute_the_right_answer() {
    // Copy counts are only meaningful if the results are correct. Every shape
    // above that writes `i` at index `i` must read back exactly that — an
    // in-place route that corrupted the buffer would still score 0 copies.
    let cases = [
        (
            "f64 array",
            format!(
                "let a = f64_array({N})\nfor i in range(0, {N}) do\n  a[i] = i * 1.0\nend\nprint(a[0], a[{}], a[{}])",
                N / 2,
                N - 1
            ),
            // f64-array reads print as floats.
            format!("0.0 {}.0 {}.0", N / 2, N - 1),
        ),
        (
            "bare-bound accumulator",
            format!(
                "xs = []\nfor i in range(0, {N}) do\n  xs = append(xs, i)\nend\nprint(xs[0], xs[{}], xs[{}])",
                N / 2,
                N - 1
            ),
            format!("0 {} {}", N / 2, N - 1),
        ),
        (
            "build then update",
            format!(
                "let xs = []\nfor i in range(0, {N}) do\n  xs = append(xs, 0)\nend\nfor i in range(0, {N}) do\n  xs[i] = i\nend\nprint(xs[0], xs[{}], xs[{}])",
                N / 2,
                N - 1
            ),
            format!("0 {} {}", N / 2, N - 1),
        ),
    ];

    for (shape, code, expected) in cases {
        let mut env = Env::new();
        let pid = env.load_program(&code).expect("load");
        let sid = env.create_stack(pid).expect("stack");
        env.run(sid).unwrap_or_else(|e| panic!("{shape}: {e}"));
        let out = env.take_output().join("\n");
        assert_eq!(out, expected, "{shape}: wrong values after in-place writes");
    }
}
