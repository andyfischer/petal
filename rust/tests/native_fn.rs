use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};

fn petal_string_repeat(state: &mut PetalCxt) -> NativeResult {
    let s = state.get_string(1)?;
    let n = state.get_int(2)?;
    state.push_string(s.repeat(n as usize));
    Ok(1)
}

fn petal_add_ints(state: &mut PetalCxt) -> NativeResult {
    let a = state.get_int(1)?;
    let b = state.get_int(2)?;
    state.push_int(a + b);
    Ok(1)
}

fn petal_greet(state: &mut PetalCxt) -> NativeResult {
    let name = state.get_string(1)?;
    state.print(format!("Hello, {}!", name));
    Ok(0)
}

fn petal_no_args(_state: &mut PetalCxt) -> NativeResult {
    Ok(0)
}

fn petal_multi_type(state: &mut PetalCxt) -> NativeResult {
    let count = state.arg_count();
    state.push_int(count as i64);
    Ok(1)
}

#[test]
fn native_string_repeat() {
    let mut env = Env::new();
    env.register_native("string_repeat", petal_string_repeat, NativeEffects::PURE);
    let result = env.run_source(r#"print(string_repeat("abc", 3))"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_add_ints() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints, NativeEffects::PURE);
    let result = env.run_source("print(add_ints(10, 20))");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_greet_with_output() {
    let mut env = Env::new();
    env.register_native("greet", petal_greet, NativeEffects::EFFECT);
    let result = env.run_source(r#"greet("World")"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_no_args_returns_nil() {
    let mut env = Env::new();
    env.register_native("noop", petal_no_args, NativeEffects::PURE);
    let result = env.run_source("let x = noop()\nprint(x)");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_arg_count() {
    let mut env = Env::new();
    env.register_native("count_args", petal_multi_type, NativeEffects::PURE);
    let result = env.run_source(r#"print(count_args(1, "two", true))"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_used_in_expression() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints, NativeEffects::PURE);
    let result = env.run_source("let x = add_ints(3, 4) + 1\nprint(x)");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_error_on_wrong_type() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints, NativeEffects::PURE);
    let result = env.run_source(r#"add_ints("not", "ints")"#);
    assert!(result.is_err());
}

#[test]
fn native_multiple_registrations() {
    let mut env = Env::new();
    env.register_native("string_repeat", petal_string_repeat, NativeEffects::PURE);
    env.register_native("add_ints", petal_add_ints, NativeEffects::PURE);
    let result = env.run_source(
        r#"
        let s = string_repeat("x", add_ints(2, 3))
        print(s)
    "#,
    );
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

// ---------------------------------------------------------------------------
// Boxed natives: natives that own captured state
// ---------------------------------------------------------------------------

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use petal::native_fn::NativeClass;
use petal::value::Value;

/// A closure's captures travel with the native: here a shared counter the
/// host reads back after the run.
#[test]
fn boxed_native_owns_captured_state() {
    let mut env = Env::new();
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    env.register_native_boxed(
        "tick",
        move |cxt: &mut PetalCxt| {
            seen.set(seen.get() + 1);
            cxt.push_int(seen.get());
            Ok(1)
        },
        NativeEffects::EFFECT,
    );
    let result = env.run_source("tick()\ntick()\ntick()").unwrap();
    assert_eq!(result, Value::Int(3));
    assert_eq!(calls.get(), 3);
}

/// One Rust function serving many natives, each with its own userdata — the
/// case a C bridge has, and the one a bare `fn` pointer cannot express
/// without a trampoline per slot.
#[test]
fn one_closure_body_serves_many_natives() {
    fn make(scale: i64) -> impl Fn(&mut PetalCxt) -> NativeResult {
        move |cxt| {
            let x = cxt.get_int(1)?;
            cxt.push_int(x * scale);
            Ok(1)
        }
    }
    let mut env = Env::new();
    for scale in 1..=50 {
        env.register_native_boxed(&format!("times{scale}"), make(scale), NativeEffects::PURE);
    }
    let result = env.run_source("times3(2) + times50(1) + times1(7)").unwrap();
    assert_eq!(result, Value::Int(6 + 50 + 7));
}

/// Boxed and bare natives share one id space and one call path.
#[test]
fn boxed_and_bare_natives_coexist() {
    let mut env = Env::new();
    let bare = env.register_native("add_ints", petal_add_ints, NativeEffects::PURE);
    let offset = 100;
    let boxed = env.register_native_boxed(
        "add_offset",
        move |cxt: &mut PetalCxt| {
            let x = cxt.get_int(1)?;
            cxt.push_int(x + offset);
            Ok(1)
        },
        NativeEffects::PURE,
    );
    assert_ne!(bare, boxed);
    assert_eq!(env.native_fn_name(boxed.0), "add_offset");
    assert!(env.has_native("add_offset"));
    let result = env.run_source("add_offset(add_ints(1, 2))").unwrap();
    assert_eq!(result, Value::Int(103));
}

/// A boxed native's error surfaces as a runtime error like any native's.
#[test]
fn boxed_native_error_is_a_runtime_error() {
    let mut env = Env::new();
    let label = String::from("host says no");
    env.register_native_boxed(
        "refuse",
        move |_cxt: &mut PetalCxt| Err(label.clone()),
        NativeEffects::PURE,
    );
    let err = env.run_source("refuse()").unwrap_err();
    assert!(err.contains("host says no"), "got: {err}");
}

/// The row and the Pending policy apply to a boxed native exactly as to a
/// bare one: `set_native_class(Effectful)` turns a Pending argument into a
/// no-op without invoking the closure.
#[test]
fn boxed_native_respects_its_pending_class() {
    let mut env = Env::new();
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let id = env.register_native_boxed(
        "emit_it",
        move |_cxt: &mut PetalCxt| {
            seen.set(seen.get() + 1);
            Ok(0)
        },
        NativeEffects::EFFECT,
    );
    env.set_native_class(id, NativeClass::Effectful);
    assert_eq!(env.native_effects(id).pending, NativeClass::Effectful);
    let result = env
        .run_source("let p = __pending(\"k\")\nemit_it(p)\nemit_it(1)\n7")
        .unwrap();
    assert_eq!(result, Value::Int(7));
    assert_eq!(calls.get(), 1, "the Pending call must not reach the closure");
}

/// The effect audit holds a boxed native to its row like any other: one that
/// prints while declaring itself pure is reported under-declared.
#[test]
fn effect_audit_covers_boxed_natives() {
    let mut env = Env::new();
    env.set_effect_audit(true);
    let greeting = String::from("hi");
    env.register_native_boxed(
        "shout",
        move |cxt: &mut PetalCxt| {
            cxt.print(greeting.clone());
            Ok(0)
        },
        NativeEffects::PURE,
    );
    env.run_source("shout()").unwrap();
    let report = env.effect_audit_report();
    let finding = report
        .findings
        .iter()
        .find(|f| f.name == "shout")
        .expect("shout is audited");
    assert_eq!(finding.kind, petal::effect_audit::FindingKind::UnderDeclared);
    assert!(finding.facets.iter().any(|f| f == "effect"), "{:?}", finding.facets);
}

/// Captures belong to the Env, not to an execution: a speculative run (a
/// fork) calls the same closure.
#[test]
fn forked_executions_share_a_boxed_natives_captures() {
    let mut env = Env::new();
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    env.register_native_boxed(
        "record",
        move |cxt: &mut PetalCxt| {
            sink.borrow_mut().push(cxt.get_int(1)?);
            Ok(0)
        },
        NativeEffects::EFFECT,
    );
    let pid = env.load_program("record(1)").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.run_speculative(sid).unwrap();
    assert_eq!(*log.borrow(), vec![1, 1]);
}

/// Dropping the Env drops the closures, and with them their captures — the
/// hook a host uses to free userdata.
#[test]
fn dropping_the_env_drops_boxed_captures() {
    let token = Rc::new(());
    let held = token.clone();
    let mut env = Env::new();
    env.register_native_boxed(
        "hold",
        move |_cxt: &mut PetalCxt| {
            let _ = &held;
            Ok(0)
        },
        NativeEffects::PURE,
    );
    assert_eq!(Rc::strong_count(&token), 2);
    drop(env);
    assert_eq!(Rc::strong_count(&token), 1);
}
