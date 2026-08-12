//! A function may not capture a module binding that is rebound after it.
//!
//! A function captures module-level bindings **by value, at its own textual
//! position** — `MakeClosure` takes the term that carries the name's value on
//! the line the `fn` is written, and a later rebinding produces a *different*
//! term the closure never sees. That is the correct consequence of `let` being
//! an immutable dataflow edge, but it means this reads `1`, twice:
//!
//! ```text
//! let x = 1
//! fn peek() x end
//! x = 2
//! print(peek())   // 1, not 2
//! ```
//!
//! In a panel, whose script re-runs top to bottom every frame, the capture
//! re-runs too — so the function sees the value as of *this* frame's
//! definition point, which for state mutated further down is last frame's
//! value. Exactly one frame of lag, every frame, presenting as faint input
//! latency rather than as a bug.
//!
//! So it is an error, at the read, with the fix being to take the value as a
//! parameter. This is the same move §2a made for cross-function `=`: the
//! honest half of the behaviour was the half that failed.
//!
//! The rule's payoff is that it removes the *wrongness* while `get` removes the
//! *ambiguity*: once a function can only capture names that are never rebound
//! after it, a bare read inside a function is provably equal to a live read,
//! whichever kind of binding it names.

use petal::env::Env;

fn check(src: &str) -> Result<(), String> {
    let mut env = Env::new();
    env.load_program(src).map(|_| ())
}

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn err(src: &str) -> String {
    match check(src) {
        Ok(()) => panic!("expected a compile error, but it compiled"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// The rule fires
// ---------------------------------------------------------------------------

#[test]
fn capturing_a_let_rebound_later_is_an_error() {
    let e = err("let x = 1\nfn peek()\n  x\nend\nx = 2\nprint(peek())");
    assert!(
        e.contains("`x`") && e.contains("rebound"),
        "should name the binding and say it is rebound, got: {e}"
    );
}

#[test]
fn capturing_a_state_rebound_later_is_an_error() {
    let e = err("state s = 1\nfn peek()\n  s\nend\ns = 2\nprint(peek())");
    assert!(e.contains("`s`"), "got: {e}");
}

#[test]
fn the_error_suggests_a_parameter() {
    let e = err("let x = 1\nfn peek()\n  x\nend\nx = 2\nprint(peek())");
    assert!(
        e.contains("parameter"),
        "the fix is to pass it in; got: {e}"
    );
}

#[test]
fn the_error_names_the_rebinding_line() {
    let e = err("let x = 1\nfn peek()\n  x\nend\n\n\nx = 2\nprint(peek())");
    assert!(
        e.contains("line 7"),
        "should point at the rebinding on line 7, got: {e}"
    );
}

#[test]
fn a_compound_rebinding_counts() {
    let e = err("let n = 1\nfn peek()\n  n\nend\nn += 1\nprint(peek())");
    assert!(e.contains("`n`"), "got: {e}");
}

#[test]
fn a_field_write_counts_as_a_rebinding() {
    // `r.a = 1` rebinds `r` — values are immutable, so the name now carries a
    // different record and the captured one is stale.
    let e = err("let r = {a: 1}\nfn peek()\n  r.a\nend\nr.a = 2\nprint(peek())");
    assert!(e.contains("`r`"), "got: {e}");
}

#[test]
fn an_index_write_counts_as_a_rebinding() {
    let e = err("let xs = [1]\nfn peek()\n  xs[0]\nend\nxs[0] = 2\nprint(peek())");
    assert!(e.contains("`xs`"), "got: {e}");
}

#[test]
fn a_rebinding_inside_a_top_level_block_counts() {
    // Still module scope, and still runs after the definition.
    let e = err("let x = 1\nfn peek()\n  x\nend\nif true then\n  x = 2\nend\nprint(peek())");
    assert!(e.contains("`x`"), "got: {e}");
}

#[test]
fn an_inline_lambda_argument_is_exempt() {
    // `map(xs, fn(a) … end)` runs its callback inside the statement that made
    // it, so no later rebinding can reach it — and if it were flagged there
    // would be no fix, since the author does not control a callback's
    // parameter list. Under-approximating here is deliberate: see the module
    // docs on where the rule stops.
    check("let k = 2\nlet ys = map([1, 2], fn(a)\n  a * k\nend)\nk = 3\nprint(ys)").unwrap();
}

#[test]
fn a_lambda_stored_in_a_binding_is_exempt_too() {
    // The same exemption, and here it really does let a hazard through: `f`
    // outlives the rebinding and reads the captured `1`. Accepted knowingly —
    // the rule cannot tell this apart from the callback above without escape
    // analysis, and rejecting callbacks is the worse error.
    let out = run("let x = 1\nlet f = fn()\n  x\nend\nx = 2\nprint(f())").unwrap();
    assert_eq!(out, "1", "the known gap: a stored lambda still snapshots");
}

// ---------------------------------------------------------------------------
// The rule stays quiet
// ---------------------------------------------------------------------------

#[test]
fn capturing_a_binding_that_is_never_rebound_is_fine() {
    check("let x = 1\nfn peek()\n  x\nend\nprint(peek())").unwrap();
}

#[test]
fn a_function_defined_after_the_last_rebinding_is_fine() {
    // The capture is the final value, so snapshot and live agree.
    check("let x = 1\nx = 2\nfn peek()\n  x\nend\nprint(peek())").unwrap();
}

#[test]
fn a_parameter_of_the_same_name_is_not_a_capture() {
    check("let x = 1\nfn peek(x)\n  x\nend\nx = 2\nprint(peek(9))").unwrap();
}

#[test]
fn a_local_of_the_same_name_is_not_a_capture() {
    check("let x = 1\nfn peek()\n  let x = 5\n  x\nend\nx = 2\nprint(peek())").unwrap();
}

#[test]
fn a_var_is_exempt_because_get_already_governs_it() {
    // A cell read is live by construction, so there is no lag to report — and
    // `get` is what makes that visible at the read.
    check("var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())").unwrap();
}

#[test]
fn a_state_var_is_exempt_too() {
    check("state var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())").unwrap();
}

#[test]
fn a_function_that_does_not_read_the_rebound_name_is_fine() {
    check("let x = 1\nlet y = 5\nfn peek()\n  y\nend\nx = 2\nprint(peek())").unwrap();
}

#[test]
fn calling_another_function_is_not_a_capture_of_its_reads() {
    check("let y = 5\nfn inner()\n  y\nend\nfn outer()\n  inner()\nend\nprint(outer())").unwrap();
}

// ---------------------------------------------------------------------------
// The pair with `get`: after both rules, a bare read in a function is safe
// ---------------------------------------------------------------------------

#[test]
fn the_two_rules_together_leave_no_silently_stale_read() {
    // Every way of reading a mutable module binding from inside a function is
    // now either explicit (`get`) or rejected.
    err("let x = 1\nfn peek()\n  x\nend\nx = 2\nprint(peek())"); // lexical: rejected
    err("var c = 1\nfn peek()\n  c\nend\nset c = 2\nprint(peek())"); // cell, bare: rejected
    check("var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())").unwrap(); // explicit: fine
}
