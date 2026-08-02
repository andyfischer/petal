//! What a live edit to a `class` does to the instances already in `state`.
//!
//! `Env::transfer_state` replaces a running stack's program and keeps every
//! state value whose `StateKey` still exists. The contract for a class instance
//! that survives that:
//!
//! - It is **a plain record carrying a label**. The heap tag is an interned
//!   *name*, never a pointer into the program that built it, so a transferred
//!   instance holds nothing of the old code — no field list, no method table,
//!   no class id. Its fields are whatever it was constructed with.
//! - **The new code decides what runs.** Where the checker can pin a receiver
//!   to one class, the compiler binds `r.m()` straight to `fn Class.m` in the
//!   program now running, instead of dispatching on the label the value
//!   happens to carry. So an edit that renames or reshapes a class takes
//!   effect on the values that predate it.
//! - Where the receiver *cannot* be pinned down, dispatch still reads the
//!   label, and an edit can leave a call with nothing to dispatch to. That is
//!   reported against the class named on the value.
//!
//! Deliberately **not** state migration: no field is invented on an instance
//! built before the field existed, and no value is rewritten when a
//! declaration changes. A value outliving its declaration is the same contract
//! as changing a state variable's type on reload (`state x = 0` → `state x =
//! "a"` keeps `0`).

use petal::env::Env;

/// Run `v1`, transfer onto `v2`, run again, and return the second run's output
/// (or its error).
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

// ── The new code wins ──────────────────────────────────────────────────────

/// The headline case. `c` holds an instance built by v1 and tagged `C`; v2
/// renames the class to `D`. Dispatching on the tag would look for `C.get` and
/// find nothing, so the annotation is what makes the edit land: the call binds
/// to `D.get`, which reads a field the old instance still has.
#[test]
fn an_annotated_receiver_runs_the_new_classs_method() {
    let out = after_reload(
        "class C\n  x: int,\nend\nfn C.get(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.get())\n",
        "class D\n  x: int,\nend\nfn D.get(d: D)\n  d.x + 100\nend\nstate c: D = D(1)\nprint(c.get())\n",
    )
    .expect("the annotation binds the call to the new class");
    assert_eq!(out, ["101"], "the new method ran on the old value");
}

/// A record that predates the class entirely. v2 turns it into a `class` and
/// calls a method on it — with no tag to dispatch on, only static resolution
/// can find the method. This used to surface as the *global* builtin's
/// complaint (`get() expects 2 arguments`) from a call that named no builtin.
#[test]
fn a_plain_record_reaches_a_method_added_by_the_edit() {
    let out = after_reload(
        "state c = {a: 1}\nprint(c.a)\n",
        "class C\n  a: int,\nend\nfn C.get(c: C)\n  c.a\nend\nstate c: C = C(1)\nprint(c.get())\n",
    )
    .expect("an untagged record still reaches the method");
    assert_eq!(out, ["1"]);
}

/// Editing a method's body is picked up on reload — the call is bound to the
/// declaration, so it follows the declaration's new definition.
#[test]
fn an_edited_method_body_takes_effect() {
    let out = after_reload(
        "class C\n  x: int,\nend\nfn C.get(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.get())\n",
        "class C\n  x: int,\nend\nfn C.get(c: C)\n  c.x + 100\nend\nstate c = C(1)\nprint(c.get())\n",
    )
    .expect("runs");
    assert_eq!(out, ["101"]);
}

/// Fields the instance still has keep working across a reshape in either
/// direction: adding a field to the class does not disturb the old ones, and
/// removing one leaves the surviving fields readable.
#[test]
fn surviving_fields_keep_working_across_a_reshape() {
    let added = after_reload(
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(c.x)\n",
        "class C\n  x: int,\n  y: int,\nend\nstate c = C(1, 2)\nprint(c.x)\n",
    )
    .expect("runs");
    assert_eq!(added, ["1"]);

    let removed = after_reload(
        "class C\n  x: int,\n  y: int,\nend\nstate c = C(1, 2)\nprint(c.x)\n",
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(c.x)\n",
    )
    .expect("runs");
    assert_eq!(removed, ["1"]);
}

// ── A label, not a link ────────────────────────────────────────────────────

/// A state value outlives the declaration that produced it, and what rides
/// along is only the *name*. An instance of a class the program no longer
/// declares still reports it, and still behaves as the record it is.
#[test]
fn an_instance_outlives_its_class_declaration() {
    let out = after_reload(
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(type(c))\n",
        "state c = {x: 1}\nprint(type(c))\nprint(c.x)\n",
    )
    .expect("the record still works");
    assert_eq!(out, ["C", "1"], "the vanished class's tag is just a label");
}

/// No migration: a field added by the edit is *not* invented on an instance
/// built before it existed. The message names the class, which is the
/// declaration to go and read.
#[test]
fn a_field_added_by_the_edit_is_not_invented_on_an_old_instance() {
    let err = after_reload(
        "class C\n  x: int,\nend\nstate c = C(1)\nprint(c.x)\n",
        "class C\n  x: int,\n  y: int,\nend\nstate c = C(1, 2)\nprint(c.x, c.y)\n",
    )
    .expect_err("the instance in state has no `y`");
    assert!(err.contains("No field 'y' on class C"), "{err}");
}

// ── What still dispatches on the label, and how it reports ─────────────────

/// Deleting `fn P.get` from a running program. The receiver is an
/// un-annotated `state`, so the call dispatches on the tag and finds nothing;
/// it used to fail with `get() expects 2 arguments` — the *global* builtin's
/// arity complaint, from a call that named no builtin.
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

/// The same rename as the headline test, but with nothing to pin the receiver
/// down: an un-annotated `state` is `any`, so the call still dispatches on the
/// tag the old value carries and reports the class it names.
#[test]
fn an_unpinned_receiver_still_dispatches_on_its_label() {
    let err = after_reload(
        "class C\n  x: int,\nend\nfn C.get(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.get())\n",
        "class D\n  x: int,\nend\nfn D.get(d: D)\n  d.x\nend\nstate c = D(1)\nprint(c.get())\n",
    )
    .expect_err("the instance in state is still a C");
    assert!(err.contains("No method 'get' on class C"), "{err}");
}
