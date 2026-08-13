//! A function captures module bindings at its own textual position, and a
//! `state` rebound below it therefore reads one run behind.
//!
//! A function captures module-level bindings **by value, at its own textual
//! position** — `MakeClosure` takes the term that carries the name's value on
//! the line the `fn` is written, and a later rebinding produces a *different*
//! term the closure never sees:
//!
//! ```text
//! let x = 1
//! fn peek() x end
//! x = 2
//! print(peek())   // 1, not 2
//! ```
//!
//! For a `let` that is the **defined** behaviour and not a diagnostic at all:
//! the rebinding is a new binding, and a function written above it sees the
//! earlier one, exactly as if the second binding had been spelled `let` again.
//!
//! For a `state` it is a hazard worth mentioning. `x = e` on a `state` writes
//! the persisted slot, and the *next* run of the file initialises the name from
//! that slot — so in a panel, whose script re-runs top to bottom every frame,
//! the function sees last frame's value. Exactly one frame of lag, every frame,
//! presenting as faint input latency rather than as a bug. That is a **warning**
//! (the program still compiles and runs), with the fix being to take the value
//! as a parameter.
//!
//! Cells (`var`, `state var`) are governed by `get` instead: a bare read of an
//! outer cell is already an error, and the `get` it demands is live.

use petal::env::Env;

fn check(src: &str) -> Result<(), String> {
    let mut env = Env::new();
    env.load_program(src).map(|_| ())
}

/// Compile `src` and return its compile-time warnings, one string per
/// diagnostic. Panics if the program does not compile.
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// The one capture-lag warning `src` produces. Panics if there is not exactly
/// one, so a test cannot pass on some unrelated diagnostic.
fn lag_warning(src: &str) -> String {
    let all = warnings(src);
    let mut lag: Vec<String> = all
        .iter()
        .filter(|m| m.contains("captured at the definition"))
        .cloned()
        .collect();
    assert_eq!(
        lag.len(),
        1,
        "expected one capture-lag warning, got: {all:?}"
    );
    lag.pop().unwrap()
}

fn no_lag_warning(src: &str) {
    let all = warnings(src);
    assert!(
        !all.iter().any(|m| m.contains("captured at the definition")),
        "expected no capture-lag warning, got: {all:?}"
    );
}

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

// ---------------------------------------------------------------------------
// The rule fires — for `state`, as a warning
// ---------------------------------------------------------------------------

#[test]
fn capturing_a_state_rebound_later_warns() {
    let w = lag_warning("state s = 1\nfn peek()\n  s\nend\ns = 2\nprint(peek())");
    assert!(w.contains("`s`"), "should name the binding, got: {w}");
}

#[test]
fn the_warning_suggests_a_parameter() {
    let w = lag_warning("state s = 1\nfn peek()\n  s\nend\ns = 2\nprint(peek())");
    assert!(
        w.contains("parameter"),
        "the fix is to pass it in; got: {w}"
    );
}

#[test]
fn the_warning_names_the_rebinding_line() {
    let w = lag_warning("state s = 1\nfn peek()\n  s\nend\n\n\ns = 2\nprint(peek())");
    assert!(
        w.contains("line 7"),
        "should point at the write on line 7, got: {w}"
    );
}

#[test]
fn a_compound_rebinding_counts() {
    let w = lag_warning("state n = 1\nfn peek()\n  n\nend\nn += 1\nprint(peek())");
    assert!(w.contains("`n`"), "got: {w}");
}

#[test]
fn a_field_write_counts_as_a_rebinding() {
    // `r.a = 1` rebinds `r` — values are immutable, so the name now carries a
    // different record and the captured one is stale.
    let w = lag_warning("state r = {a: 1}\nfn peek()\n  r.a\nend\nr.a = 2\nprint(peek())");
    assert!(w.contains("`r`"), "got: {w}");
}

#[test]
fn an_index_write_counts_as_a_rebinding() {
    let w = lag_warning("state xs = [1]\nfn peek()\n  xs[0]\nend\nxs[0] = 2\nprint(peek())");
    assert!(w.contains("`xs`"), "got: {w}");
}

#[test]
fn a_rebinding_inside_a_top_level_block_counts() {
    // Still module scope, and still runs after the definition.
    let w =
        lag_warning("state x = 1\nfn peek()\n  x\nend\nif true then\n  x = 2\nend\nprint(peek())");
    assert!(w.contains("`x`"), "got: {w}");
}

#[test]
fn the_warned_program_still_compiles_and_runs() {
    // A warning, not an error: the behaviour is defined, so the program runs.
    let out = run("state s = 1\nfn peek()\n  s\nend\ns = 2\nprint(peek())").unwrap();
    assert_eq!(out, "1", "the capture is the value as of the `fn`");
}

// ---------------------------------------------------------------------------
// The rule stays quiet
// ---------------------------------------------------------------------------

#[test]
fn capturing_a_let_rebound_later_is_not_reported() {
    // A `let` rebinding is a *new* binding; a function above it is supposed to
    // read the earlier one. Nothing is stale, so nothing is said.
    let src = "let x = 1\nfn peek()\n  x\nend\nx = 2\nprint(peek())";
    no_lag_warning(src);
    assert_eq!(run(src).unwrap(), "1");
}

#[test]
fn a_let_field_or_index_write_is_not_reported_either() {
    no_lag_warning("let r = {a: 1}\nfn peek()\n  r.a\nend\nr.a = 2\nprint(peek())");
    no_lag_warning("let xs = [1]\nfn peek()\n  xs[0]\nend\nxs[0] = 2\nprint(peek())");
}

#[test]
fn an_inline_lambda_argument_is_exempt() {
    // `map(xs, fn(a) … end)` runs its callback inside the statement that made
    // it, so no later rebinding can reach it — and if it were flagged there
    // would be no fix, since the author does not control a callback's
    // parameter list. Under-approximating here is deliberate: see the module
    // docs on where the rule stops.
    no_lag_warning("state k = 2\nlet ys = map([1, 2], fn(a)\n  a * k\nend)\nk = 3\nprint(ys)");
}

#[test]
fn a_lambda_stored_in_a_binding_is_exempt_too() {
    // The same exemption, and here it really does let a hazard through: `f`
    // outlives the rebinding and reads the captured `1`. Accepted knowingly —
    // the rule cannot tell this apart from the callback above without escape
    // analysis, and shouting at callbacks is the worse outcome.
    let out = run("state x = 1\nlet f = fn()\n  x\nend\nx = 2\nprint(f())").unwrap();
    assert_eq!(out, "1", "the known gap: a stored lambda still snapshots");
}

#[test]
fn capturing_a_binding_that_is_never_rebound_is_fine() {
    no_lag_warning("state x = 1\nfn peek()\n  x\nend\nprint(peek())");
}

#[test]
fn a_function_defined_after_the_last_rebinding_is_fine() {
    // The capture is the final value, so snapshot and live agree.
    no_lag_warning("state x = 1\nx = 2\nfn peek()\n  x\nend\nprint(peek())");
}

#[test]
fn a_parameter_of_the_same_name_is_not_a_capture() {
    no_lag_warning("state x = 1\nfn peek(x)\n  x\nend\nx = 2\nprint(peek(9))");
}

#[test]
fn a_local_of_the_same_name_is_not_a_capture() {
    no_lag_warning("state x = 1\nfn peek()\n  let x = 5\n  x\nend\nx = 2\nprint(peek())");
}

#[test]
fn a_var_is_exempt_because_get_already_governs_it() {
    // A cell read is live by construction, so there is no lag to report — and
    // `get` is what makes that visible at the read.
    no_lag_warning("var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())");
}

#[test]
fn a_state_var_is_exempt_too() {
    no_lag_warning("state var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())");
}

#[test]
fn a_function_that_does_not_read_the_rebound_name_is_fine() {
    no_lag_warning("state x = 1\nlet y = 5\nfn peek()\n  y\nend\nx = 2\nprint(peek())");
}

#[test]
fn calling_another_function_is_not_a_capture_of_its_reads() {
    no_lag_warning("let y = 5\nfn inner()\n  y\nend\nfn outer()\n  inner()\nend\nprint(outer())");
}

// ---------------------------------------------------------------------------
// The pair with `get`: a live read of a cell is always spelled `get`
// ---------------------------------------------------------------------------

#[test]
fn a_bare_cell_read_across_a_function_is_still_an_error() {
    // The `get` half of the pair is untouched by the downgrade here: a bare
    // read of an outer cell is still rejected outright.
    let e = check("var c = 1\nfn peek()\n  c\nend\nset c = 2\nprint(peek())")
        .expect_err("bare cell read should be rejected");
    assert!(e.contains("get c"), "got: {e}");
    check("var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())").unwrap();
}
