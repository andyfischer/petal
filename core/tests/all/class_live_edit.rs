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
/// renames the class to `D`. Dispatching on the tag would look for `C.first` and
/// find nothing, so the annotation is what makes the edit land: the call binds
/// to `D.first`, which reads a field the old instance still has.
#[test]
fn an_annotated_receiver_runs_the_new_classs_method() {
    let out = after_reload(
        "class C\n  x: int,\nend\nfn C.first(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.first())\n",
        "class D\n  x: int,\nend\nfn D.first(d: D)\n  d.x + 100\nend\nstate c: D = D(1)\nprint(c.first())\n",
    )
    .expect("the annotation binds the call to the new class");
    assert_eq!(out, ["101"], "the new method ran on the old value");
}

/// A record that predates the class entirely. v2 turns it into a `class` and
/// calls a method on it — with no tag to dispatch on, only static resolution
/// can find the method. This used to surface as the *global* builtin's
/// complaint (`first() expects 1 argument`) from a call that named no builtin.
#[test]
fn a_plain_record_reaches_a_method_added_by_the_edit() {
    let out = after_reload(
        "state c = {a: 1}\nprint(c.a)\n",
        "class C\n  a: int,\nend\nfn C.first(c: C)\n  c.a\nend\nstate c: C = C(1)\nprint(c.first())\n",
    )
    .expect("an untagged record still reaches the method");
    assert_eq!(out, ["1"]);
}

/// Editing a method's body is picked up on reload — the call is bound to the
/// declaration, so it follows the declaration's new definition.
#[test]
fn an_edited_method_body_takes_effect() {
    let out = after_reload(
        "class C\n  x: int,\nend\nfn C.first(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.first())\n",
        "class C\n  x: int,\nend\nfn C.first(c: C)\n  c.x + 100\nend\nstate c = C(1)\nprint(c.first())\n",
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

/// Deleting `fn P.first` from a running program. The receiver is an
/// un-annotated `state`, so the call dispatches on the tag and finds nothing;
/// it used to fail with `first() expects 1 argument` — the *global* builtin's
/// arity complaint, from a call that named no builtin.
#[test]
fn deleting_a_method_reports_the_class_not_the_builtin_it_collides_with() {
    let err = after_reload(
        "class P\n  a: int,\nend\nfn P.first(p: P)\n  p.a\nend\nstate p = P(1)\nprint(p.first())\n",
        "class P\n  a: int,\nend\nstate p = P(1)\nprint(p.first())\n",
    )
    .expect_err("the method is gone");
    assert!(err.contains("No method 'first' on class P"), "{err}");
    assert!(!err.contains("expects 2 arguments"), "{err}");
}

/// The same rename with nothing to pin the receiver down. An un-annotated
/// `state` is `any`, so the call still dispatches on the label — which names a
/// class this program no longer has. Rather than dead-end there, dispatch
/// falls back to the class the *declaration* named, and the edit lands.
#[test]
fn a_stale_label_falls_back_to_the_declarations_class() {
    let out = after_reload(
        "class C\n  x: int,\nend\nfn C.first(c: C)\n  c.x\nend\nstate c = C(1)\nprint(c.first())\n",
        "class D\n  x: int,\nend\nfn D.first(d: D)\n  d.x + 100\nend\nstate c = D(1)\nprint(c.first())\n",
    )
    .expect("the label is stale, so the declaration answers");
    assert_eq!(out, ["101"]);
}

/// The fallback is a *last* resort, not a preference. A label naming a class
/// that really is here wins, so one binding holding different classes over
/// time keeps dispatching to each one's own method — the behaviour that ruled
/// out simply pinning an un-annotated binding to its initializer.
#[test]
fn a_live_label_still_wins_over_the_declarations_class() {
    let out = after_reload(
        "class Circle\n  r: int,\nend\nclass Square\n  s: int,\nend\nfn Circle.area(c: Circle)\n  3 * c.r * c.r\nend\nfn Square.area(q: Square)\n  q.s * q.s\nend\nstate shape = Circle(2)\nshape = Square(3)\nprint(shape.area())\n",
        "class Circle\n  r: int,\nend\nclass Square\n  s: int,\nend\nfn Circle.area(c: Circle)\n  3 * c.r * c.r\nend\nfn Square.area(q: Square)\n  q.s * q.s\nend\nstate shape = Circle(2)\nprint(shape.area())\n",
    )
    .expect("runs");
    // `shape` survives as the Square the first run left there. Its declaration
    // says Circle, but the label is live, so `Square.area` runs — 3*3, not
    // Circle.area reading a field a Square does not have.
    assert_eq!(out, ["9"]);
}

/// A method genuinely deleted still reports against the class on the value:
/// the label resolves (class `P` is still declared), so the fallback is never
/// consulted and the diagnostic is the class-aware one.
#[test]
fn a_live_label_missing_the_method_still_reports_its_own_class() {
    let err = after_reload(
        "class P\n  a: int,\nend\nfn P.first(p: P)\n  p.a\nend\nstate p = P(1)\nprint(p.first())\n",
        "class P\n  a: int,\nend\nfn P.other(p: P)\n  p.a\nend\nstate p = P(1)\nprint(p.first())\n",
    )
    .expect_err("the method is gone and the class is not");
    assert!(err.contains("No method 'first' on class P"), "{err}");
}
