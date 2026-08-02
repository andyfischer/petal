//! What a live edit to a `class` reports.
//!
//! `Env::transfer_state` replaces a running stack's program and keeps every
//! state value whose `StateKey` still exists. A class instance is an ordinary
//! record value, so it survives the transfer *with the shape it was built
//! with* — the same contract as changing a state variable's type on reload.
//! That is deliberate, but it used to surface as the *builtin's* complaint
//! rather than anything mentioning the class, which is what made a live edit
//! baffling. These tests pin the diagnostics.
//!
//! The stale instance itself is a state-migration question, not a message
//! one, and is deliberately left as-is: see the notes on each test.

use petal::env::Env;

/// Run `v1`, transfer onto `v2`, run again, and return the second run's error
/// (or its output when it succeeds).
fn after_reload(v1: &str, v2: &str) -> Result<Vec<String>, String> {
    let mut env = Env::new();
    let pid = env.load_program(v1).expect("v1 compiles");
    let sid = env.create_stack(pid).expect("v1 stack");
    env.run(sid).expect("v1 runs");
    env.take_output();
    let next = env.compile_program(pid, v2).expect("v2 compiles");
    env.transfer_state(sid, next).expect("transfer");
    env.run(sid).map(|_| env.take_output())
}

/// The case the message change was for: deleting `fn P.get` from a running
/// program used to fail with `get() expects 2 arguments` — the *global*
/// builtin's arity complaint, from a call that named no builtin.
#[test]
fn deleting_a_method_reports_the_class_not_the_builtin_it_collides_with() {
    let err = after_reload(
        "class P\n  a: int,\nend\nfn P.get(p: P)\n  p.a\nend\nstate p = P(1)\nprint(p.get())\n",
        "class P\n  a: int,\nend\nstate p = P(1)\nprint(p.get())\n",
    )
    .expect_err("the method is gone");
    assert!(err.contains("No method 'get' on class P"), "{err}");
    assert!(!err.contains("expects 2 arguments"), "{err}");
}

/// Renaming a class leaves the *old* instance in state, still tagged with the
/// old name, so dispatch looks for `C.get` and finds nothing. The stale
/// instance is the state-migration issue; what is pinned here is that the
/// message says so instead of blaming the builtin `get`.
#[test]
fn renaming_a_class_names_the_stale_instances_class() {
    let err = after_reload(
        "class C\n  x: int,\nend\nfn C.get(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.get())\n",
        "class D\n  x: int,\nend\nfn D.get(d: D)\n  d.x\nend\nstate c = D(1)\nprint(c.get())\n",
    )
    .expect_err("the instance in state is still a C");
    assert!(err.contains("No method 'get' on class C"), "{err}");
}

/// Adding a field likewise keeps the old instance, so the new field is absent.
/// The message names the class, which is the declaration to go and read.
#[test]
fn adding_a_field_names_the_class_the_field_is_missing_from() {
    let err = after_reload(
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(c.x)\n",
        "class C\n  x: int,\n  y: int,\nend\nstate c = C(1, 2)\nprint(c.x, c.y)\n",
    )
    .expect_err("the instance in state has no `y`");
    assert!(err.contains("No field 'y' on class C"), "{err}");
}

/// A state value outlives the declaration that produced it, so an instance of
/// a class the program no longer declares keeps reporting its class name.
/// Recorded rather than asserted-against: `type()` reports the tag the value
/// carries, and nothing rewrites a live value when a declaration disappears.
#[test]
fn an_instance_outlives_its_class_declaration() {
    let out = after_reload(
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(type(c))\n",
        "state c = {x: 1}\nprint(type(c))\n",
    )
    .expect("the record still works");
    assert_eq!(out, ["C"], "the vanished class's tag rides along");
}
