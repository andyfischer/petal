//! Which `recv.method()` sites are bound at compile time, and which keep
//! runtime dispatch.
//!
//! When the checker can pin a receiver to one class, the compiler binds the
//! call straight to `fn Class.method` — an ordinary `Call` naming its callee —
//! instead of emitting a `MethodCall` that looks the method up by the tag the
//! receiver carries. Two things ride on it: a live edit takes effect on values
//! that predate it (`class_live_edit.rs`), and a dataflow slice gets the exact
//! function instead of a may-edge over every method of that name
//! (`ts/test/slicing.test.ts`).
//!
//! The interesting content of this file is the *guards*: resolving a site the
//! two mechanisms would disagree about would change what a working program
//! does, so each guard below has a test.

use petal::env::Env;
use petal::program::TermOp;

/// The class and method every case below shares.
const CLS: &str = "class C\n  a: int,\nend\nfn C.get(c: C)\n  c.a\nend\n";

/// Whether `src` still contains a dispatched-at-runtime method call.
fn dispatches(src: &str) -> bool {
    let mut env = Env::new();
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("compiles: {e}\n{src}"));
    env.get_program(pid)
        .expect("program")
        .terms
        .iter()
        .any(|t| matches!(t.op, TermOp::MethodCall(_)))
}

fn resolves(src: &str) -> bool {
    !dispatches(src)
}

/// Run `src` and return its output, so a resolved call is checked to still
/// compute what the dispatched one did.
fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).unwrap_or_else(|e| panic!("runs: {e}\n{src}"));
    env.take_output()
}

// ── Receivers the checker can pin down ─────────────────────────────────────

#[test]
fn a_constructor_call_receiver_resolves() {
    let src = format!("{CLS}print(C(1).get())\n");
    assert!(resolves(&src));
    assert_eq!(run(&src), ["1"]);
}

#[test]
fn a_let_bound_to_a_constructor_resolves() {
    let src = format!("{CLS}let c = C(1)\nprint(c.get())\n");
    assert!(resolves(&src));
    assert_eq!(run(&src), ["1"]);
}

/// The annotation is the point: it pins a binding the initializer alone would
/// leave as `any`, which is what makes a live edit land on a `state` value.
#[test]
fn an_annotation_resolves_every_binding_form() {
    for form in ["let c: C = C(1)", "var c: C = C(1)", "state c: C = C(1)"] {
        let src = format!("{CLS}{form}\nprint(c.get())\n");
        assert!(resolves(&src), "{form}");
        assert_eq!(run(&src), ["1"], "{form}");
    }
}

#[test]
fn an_annotated_parameter_resolves() {
    let src = format!("{CLS}fn f(c: C)\n  c.get()\nend\nprint(f(C(1)))\n");
    assert!(resolves(&src));
    assert_eq!(run(&src), ["1"]);
}

/// A built-in class method is a native registered under the same dotted name
/// before the program starts, so it needs no declaration to wait for.
#[test]
fn a_builtin_class_method_resolves() {
    let src = "let r = Rect(0, 0, 100, 40)\nprint(r.center_x())\n";
    assert!(resolves(src));
    assert_eq!(run(src), ["50"]);
}

/// A user declaration of a built-in's method still wins, as it does at
/// runtime: the declaration shadows the native under the same name.
#[test]
fn a_user_method_overrides_the_builtin_it_shadows() {
    let src =
        "fn Rect.center_x(r: Rect)\n  999\nend\nlet r = Rect(0, 0, 100, 40)\nprint(r.center_x())\n";
    assert!(resolves(src));
    assert_eq!(run(src), ["999"]);
}

// ── Guards: sites that must keep runtime dispatch ──────────────────────────

/// An un-annotated binding is `any`, mutable or not — the checker's rule, not
/// this pass's. Nothing to pin down, so the tag decides.
#[test]
fn an_unpinned_receiver_keeps_dispatching() {
    for form in ["state c = C(1)", "var c = C(1)"] {
        let src = format!("{CLS}{form}\nprint(c.get())\n");
        assert!(dispatches(&src), "{form}");
        assert_eq!(run(&src), ["1"], "{form}");
    }
    let param = format!("{CLS}fn f(c)\n  c.get()\nend\nprint(f(C(1)))\n");
    assert!(dispatches(&param));
    assert_eq!(run(&param), ["1"]);
}

/// A field outranks a method of the same name at runtime. Binding the call to
/// the method would invert that, so a class declaring both keeps dispatching.
#[test]
fn a_field_of_the_same_name_keeps_dispatching() {
    let src = "class C\n  get: any,\nend\nfn C.get(c: C)\n  1\nend\nlet c = C(fn() -> 9)\nprint(c.get())\n";
    assert!(dispatches(src));
    assert_eq!(run(src), ["9"], "the field wins, as it does at runtime");
}

/// Nothing in Petal hoists: the name a `fn` binds holds nil until its
/// declaration runs. Binding a call to a declaration written below it would
/// turn a program that works today into `Cannot call nil` — or, from inside a
/// function body, into a term the caller's block cannot reference at all.
#[test]
fn a_method_declared_below_the_call_keeps_dispatching() {
    let src = "class C\n  a: int,\nend\nfn f(c: C)\n  c.get()\nend\nfn C.get(x: C)\n  x.a\nend\nprint(f(C(1)))\n";
    assert!(dispatches(src));
    assert_eq!(run(src), ["1"], "dispatch still finds it at call time");
}

/// A method the class does not declare falls through to a global native with
/// the receiver prepended (`r.keys()`), which this resolution knows nothing
/// about and must not intercept.
#[test]
fn a_global_native_reached_through_a_receiver_keeps_dispatching() {
    let src = "class C\n  a: int,\nend\nlet c = C(1)\nprint(c.keys())\n";
    assert!(dispatches(src));
    assert_eq!(run(src), ["[\"a\"]"]);
}

/// An arity no declared overload accepts is already a warning; resolving it
/// would replace the method-shaped arity error with a plain function's.
#[test]
fn a_call_matching_no_overload_keeps_dispatching() {
    let src = format!("{CLS}let c = C(1)\nprint(c.get(1, 2, 3))\n");
    assert!(dispatches(&src));
}

/// Overloading by arity picks the overload the call site matches.
#[test]
fn an_arity_overload_resolves_to_the_matching_variant() {
    let src = "class C\n  a: int,\nend\nfn C.plus(c: C)\n  c.a\nend\nfn C.plus(c: C, n)\n  c.a + n\nend\nlet c = C(1)\nprint(c.plus())\nprint(c.plus(10))\n";
    assert!(resolves(src));
    assert_eq!(run(src), ["1", "11"]);
}
