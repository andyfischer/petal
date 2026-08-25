//! `state-shared-callsites` — an in-function `state` whose enclosing function
//! is called from more than one place, or from inside a loop.
//!
//! Today the slot is keyed by the declaration alone, so every one of those
//! calls reaches the same value. Under per-call-path keying
//! (docs/dev/state-callsite-keying-plan.md) each callsite, and each loop
//! iteration around it, gets its own — so code that relies on the sharing has
//! to say so out loud, with a top-level `state var` read and written through
//! `get`/`set` (plan §2.4). This lint finds those declarations before the
//! semantics change under them.
//!
//! The pass is `typecheck::state_callsites`; its findings are ordinary
//! compile-time warnings, so these tests read them off the compiled program.

use petal::env::Env;

/// Compile `src` and return its `state-shared-callsites` warnings. Panics if
/// the program does not compile, so a test cannot pass on a broken source.
fn lints(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    env.get_program(pid)
        .expect("program")
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .filter(|m| m.starts_with("state-shared-callsites:"))
        .collect()
}

/// The one lint `src` produces, asserting there is exactly one.
#[track_caller]
fn one_lint(src: &str) -> String {
    let mut all = lints(src);
    assert_eq!(all.len(), 1, "expected exactly one lint, got: {all:?}");
    all.pop().unwrap()
}

#[track_caller]
fn no_lint(src: &str) {
    let all = lints(src);
    assert!(all.is_empty(), "expected no lint, got: {all:?}");
}

// ---------------------------------------------------------------------------
// It fires
// ---------------------------------------------------------------------------

#[test]
fn two_callsites_warn() {
    let w =
        one_lint("fn tick()\n  state n = 0\n  n = n + 1\n  n\nend\nprint(tick())\nprint(tick())");
    assert!(w.contains("`n`"), "should name the state, got: {w}");
    assert!(w.contains("`tick`"), "should name the function, got: {w}");
    assert!(w.contains("2 places"), "should count callsites, got: {w}");
}

#[test]
fn the_message_points_at_the_migration_idiom() {
    let w = one_lint("fn tick()\n  state n = 0\n  n\nend\nprint(tick())\nprint(tick())");
    assert!(
        w.contains("state var n"),
        "should suggest a top-level cell, got: {w}"
    );
    assert!(
        w.contains("get n") && w.contains("set n"),
        "should name the `get`/`set` accessors, got: {w}"
    );
}

#[test]
fn a_single_callsite_inside_a_for_warns() {
    let w = one_lint(
        "fn widget(i)\n  state clicks = 0\n  clicks + i\nend\nfor i in range(0, 3) do\n  print(widget(i))\nend",
    );
    assert!(
        w.contains("in a loop") || w.contains("inside a loop"),
        "got: {w}"
    );
}

#[test]
fn a_callsite_inside_a_while_warns() {
    let w = one_lint(
        "fn step()\n  state n = 0\n  n\nend\nvar i = 0\nwhile i < 3 do\n  print(step())\n  set i = i + 1\nend",
    );
    assert!(w.contains("`step`"), "got: {w}");
}

#[test]
fn recursion_counts_as_a_second_callsite() {
    let w = one_lint(
        "fn down(k)\n  state seen = 0\n  if k > 0 then\n    down(k - 1)\n  else\n    seen\n  end\nend\nprint(down(3))",
    );
    assert!(w.contains("2 places"), "got: {w}");
}

#[test]
fn every_state_in_the_function_is_reported() {
    let src = "fn tick()\n  state a = 0\n  state b = 1\n  a + b\nend\nprint(tick())\nprint(tick())";
    let all = lints(src);
    assert_eq!(all.len(), 2, "one lint per declaration, got: {all:?}");
    assert!(all.iter().any(|m| m.contains("`a`")));
    assert!(all.iter().any(|m| m.contains("`b`")));
}

#[test]
fn state_var_in_a_function_warns_too() {
    // A cell inside a function is per-path under the new rules just like a
    // plain `state`; the fix is to move the cell to the top level.
    let w = one_lint(
        "fn bump()\n  state var n = 0\n  set n = n + 1\n  get n\nend\nprint(bump())\nprint(bump())",
    );
    assert!(w.contains("`n`"), "got: {w}");
}

#[test]
fn state_nested_in_an_if_inside_the_function_warns() {
    let w = one_lint(
        "fn tick(on)\n  if on then\n    state n = 0\n    n\n  else\n    0\n  end\nend\nprint(tick(true))\nprint(tick(true))",
    );
    assert!(w.contains("`n`"), "got: {w}");
}

#[test]
fn a_method_called_from_two_places_warns() {
    let w = one_lint(
        "class Box\n  w\nend\nfn Box.grow(b)\n  state n = 0\n  n + b.w\nend\nlet b = Box(1)\nprint(b.grow())\nprint(b.grow())",
    );
    assert!(w.contains("`Box.grow`"), "should name the method, got: {w}");
}

// ---------------------------------------------------------------------------
// It stays quiet
// ---------------------------------------------------------------------------

#[test]
fn a_single_straight_line_callsite_is_silent() {
    // One callsite behaves identically before and after the flip.
    no_lint("fn tick()\n  state n = 0\n  n\nend\nprint(tick())");
}

#[test]
fn an_uncalled_function_is_silent() {
    // A host entry point has no callsite this pass can see.
    no_lint("fn tick()\n  state n = 0\n  n\nend");
}

#[test]
fn top_level_state_is_silent() {
    // Module scope is the root path: unchanged by the flip.
    no_lint("state n = 0\nfn bump()\n  n\nend\nprint(bump())\nprint(bump())");
}

#[test]
fn an_explicit_key_is_silent() {
    // `state(key)` is absolute under the new rules: it already says which slot
    // it wants, whoever calls the function.
    no_lint("fn slot(id)\n  state(id) n = 0\n  n\nend\nprint(slot(1))\nprint(slot(2))");
}

#[test]
fn a_function_without_state_is_silent() {
    no_lint("fn add(a, b)\n  a + b\nend\nprint(add(1, 2))\nprint(add(3, 4))");
}

#[test]
fn state_inside_a_lambda_is_silent() {
    // The lambda's own call path keys it; the enclosing function's callsite
    // count says nothing about how often the lambda runs.
    no_lint(
        "fn make()\n  let step = fn(x)\n    state n = 0\n    n + x\n  end\n  step\nend\nlet f = make()\nlet g = make()\nprint(f(1))\nprint(g(2))",
    );
}

#[test]
fn an_overload_keeps_its_own_callsite_count() {
    // `f(1)` and `f(1, 2)` are different functions: the two-argument one is
    // called once, so it must not borrow the other's callsites.
    let all = lints(
        "fn f(a)\n  state n = 0\n  n + a\nend\nfn f(a, b)\n  state m = 0\n  m + a + b\nend\nprint(f(1))\nprint(f(1))\nprint(f(1, 2))",
    );
    assert_eq!(all.len(), 1, "only the two-callsite variant warns: {all:?}");
    assert!(all[0].contains("`n`"), "got: {all:?}");
}

#[test]
fn a_nested_function_is_not_charged_for_its_parents_calls() {
    // `inner` is called once; `outer` twice. The state belongs to `inner`.
    no_lint(
        "fn outer()\n  fn inner()\n    state n = 0\n    n\n  end\n  inner()\nend\nprint(outer())\nprint(outer())",
    );
}

#[test]
fn a_call_inside_a_fn_declared_in_a_loop_is_not_an_in_loop_callsite() {
    // The loop body's `fn` is a declaration, not a call: statements inside it
    // run when *it* is called, so the enclosing loop does not multiply them.
    no_lint(
        "fn helper()\n  state n = 0\n  n\nend\nfor i in range(0, 2) do\n  fn wrapper()\n    helper()\n  end\n  print(wrapper())\nend",
    );
}
