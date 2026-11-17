//! A value that holds a pending value somewhere inside it: the strict JSON
//! conversion refuses it, and an `EffectfulDeep` native no-ops on it, while
//! every other policy keeps the element-by-element rule.

use std::cell::RefCell;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{NativeClass, NativeEffects, PetalCxt};
use petal::value::{Value, find_pending, value_to_json, value_to_json_lossy};

/// Register `name` as an emitter with `effects` that records the JSON of each
/// argument it is called with (lossy, so a pending field shows up).
fn recorder(env: &mut Env, name: &str, effects: NativeEffects) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    env.register_native_boxed(
        name,
        move |cxt: &mut PetalCxt| {
            let v = cxt.get_value(1)?;
            log.borrow_mut().push(value_to_json_lossy(&v, cxt.heap()).to_string());
            Ok(0)
        },
        effects,
    );
    seen
}

#[test]
fn strict_json_refuses_a_pending_field_and_names_it() {
    let mut env = Env::new();
    let v = env
        .run_source("let d = __pending(\"d\")\n{title: \"t\", rows: [1, {date: d}]}\n")
        .unwrap();
    let err = value_to_json(&v, env.heap()).unwrap_err();
    assert_eq!(err.path, ".rows[1].date");
    assert_eq!(Some(err.pending), find_pending(&v, env.heap()));
    let msg = err.to_string();
    assert!(msg.contains(".rows[1].date") && msg.contains("pending"), "got: {msg}");

    // The lossy form still renders it, for a debug dump.
    let lossy = value_to_json_lossy(&v, env.heap());
    assert_eq!(lossy["rows"][1]["date"], format!("<pending {}>", err.pending.0));
}

#[test]
fn strict_json_refuses_a_pending_value_itself() {
    let mut env = Env::new();
    let v = env.run_source("__pending(\"k\")\n").unwrap();
    let err = value_to_json(&v, env.heap()).unwrap_err();
    assert_eq!(err.path, "");
    assert!(matches!(v, Value::Pending(id) if id == err.pending));
}

#[test]
fn strict_json_converts_a_loaded_value() {
    let mut env = Env::new();
    let v = env
        .run_source("__resolve(\"d\", 3)\n{date: __pending(\"d\"), xs: [1, 2]}\n")
        .unwrap();
    assert_eq!(find_pending(&v, env.heap()), None);
    assert_eq!(
        value_to_json(&v, env.heap()).unwrap(),
        serde_json::json!({"date": 3, "xs": [1, 2]})
    );
}

#[test]
fn json_stringify_of_a_record_with_a_pending_field_is_pending() {
    let mut env = Env::new();
    let v = env
        .run_source("import json\njson_stringify({a: 1, b: [__pending(\"k\")]})\n")
        .unwrap();
    assert!(matches!(v, Value::Pending(_)), "got {v:?}");
}

#[test]
fn effectful_deep_emitter_skips_a_record_with_a_pending_field() {
    let mut env = Env::new();
    let seen = recorder(&mut env, "emit", NativeEffects::EMITS_DEEP);
    assert_eq!(NativeEffects::EMITS_DEEP.pending, NativeClass::EffectfulDeep);
    let v = env
        .run_source(
            "native fn emit(x)\n\
             let d = __pending(\"d\")\n\
             emit({title: \"a\", date: d})\n\
             emit([[d]])\n\
             emit(d)\n\
             emit({title: \"b\", date: 2})\n\
             7\n",
        )
        .unwrap();
    assert_eq!(v, Value::Int(7));
    assert_eq!(*seen.borrow(), vec![r#"{"title":"b","date":2}"#.to_string()]);
}

#[test]
fn effectful_deep_runs_once_the_field_loads() {
    let mut env = Env::new();
    let seen = recorder(&mut env, "emit", NativeEffects::EMITS_DEEP);
    let src = "native fn emit(x)\nemit({date: __pending(\"d\")})\n";
    env.run_source(src).unwrap();
    assert!(seen.borrow().is_empty());
    env.run_source("__resolve(\"d\", 5)\n").unwrap();
    env.run_source(src).unwrap();
    assert_eq!(*seen.borrow(), vec![r#"{"date":5}"#.to_string()]);
}

/// The other policies keep the element-by-element rule: an `EMITS` native
/// handed a record with a pending field runs and sees the field.
#[test]
fn shallow_effectful_still_sees_a_partly_loaded_record() {
    let mut env = Env::new();
    let seen = recorder(&mut env, "emit", NativeEffects::EMITS);
    env.run_source("native fn emit(x)\nemit({date: __pending(\"d\")})\nemit(__pending(\"d\"))\n")
        .unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1, "a top-level pending still no-ops: {seen:?}");
    assert!(seen[0].contains("<pending"), "got {seen:?}");
}
