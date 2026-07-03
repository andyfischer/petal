// Failing tests for chunk 1 of the handle feature: `Value::Handle`, an opaque
// foreign-object handle (docs/dev/unreal-ffi-proposal.md §1, §5).
//
// Scope: minting handles host-side, passing them into scripts via bindings,
// round-tripping, `type()`, equality, truthiness, and host-side eq/hash.
// NOT in scope (later chunks): method dispatch through `call_method`, the
// `is_valid` builtin, `state(h)`, JSON state encoding.
//
// Target API (agreed design, in the petal crate root / rust/src/handle.rs):
//
//   pub struct HandleVal { pub class: HandleClassId, pub slot: u32, pub serial: u32 }
//   pub struct HandleClassId(pub u16);
//   pub struct HandleClass { name, is_valid, describe, call_method }
//   Env::register_handle_class(&mut self, HandleClass) -> HandleClassId
//   Env::make_handle(&self, HandleClassId, slot, serial) -> Value  // Value::Handle(HandleVal)

use std::collections::HashSet;

use petal::backend::Backend;
use petal::env::Env;
use petal::value::Value;
use petal::{HandleClass, HandleClassId, HandleVal};

/// A trivial test handle class. Nothing in chunk 1 calls `is_valid` /
/// `describe` / `call_method`; they just have to be registrable.
fn test_entity_class() -> HandleClass {
    HandleClass {
        name: "TestEntity".to_string(),
        is_valid: Box::new(|_slot, serial| serial != 0),
        describe: Box::new(|slot, serial| format!("TestEntity(slot={slot}, serial={serial})")),
        call_method: Box::new(|_cxt, method| {
            Err(format!("no methods in chunk 1 (called '{method}')"))
        }),
    }
}

/// Fresh Env with a registered "TestEntity" handle class.
fn env_with_test_class() -> (Env, HandleClassId) {
    let mut env = Env::new();
    let class = env.register_handle_class(test_entity_class());
    (env, class)
}

/// Run `source` under one backend with the given named handle bindings
/// (bound as uniforms, read in-script via `binding(symbol("name"))`).
/// Returns (program result, print output).
fn run_with_handles(
    backend: Backend,
    bindings: &[(&str, u32, u32)], // (name, slot, serial) minted on the TestEntity class
    source: &str,
) -> (Value, Vec<String>) {
    let (mut env, class) = env_with_test_class();
    env.set_backend(backend);
    let pid = env.load_program(source).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for (name, slot, serial) in bindings {
        let sym = env.intern_symbol(name);
        let handle = env.make_handle(class, *slot, *serial);
        env.set_binding(sym, handle);
    }
    let result = env.run(sid).unwrap();
    (result, env.take_output())
}

/// Assert both backends produce the same program result for `source`.
fn check_result_both_backends(bindings: &[(&str, u32, u32)], source: &str, expect: Value) {
    for backend in [Backend::Graph, Backend::Bytecode] {
        let (result, _out) = run_with_handles(backend, bindings, source);
        assert_eq!(result, expect, "[{backend:?}] source: {source}");
    }
}

/// Assert both backends produce the same print output for `source`.
fn check_output_both_backends(bindings: &[(&str, u32, u32)], source: &str, expect: &[&str]) {
    for backend in [Backend::Graph, Backend::Bytecode] {
        let (_result, out) = run_with_handles(backend, bindings, source);
        assert_eq!(out, expect, "[{backend:?}] source: {source}");
    }
}

// ── 1. Host-side registration & minting ──────────────────────────

#[test]
fn register_handle_class_returns_distinct_ids() {
    let mut env = Env::new();
    let a = env.register_handle_class(test_entity_class());
    let b = env.register_handle_class(HandleClass {
        name: "OtherClass".to_string(),
        ..test_entity_class()
    });
    assert_ne!(a, b, "two registered classes must get distinct ids");
}

#[test]
fn make_handle_returns_handle_value_with_fields() {
    let (env, class) = env_with_test_class();
    let v = env.make_handle(class, 7, 42);
    match v {
        Value::Handle(h) => {
            assert_eq!(h.class, class);
            assert_eq!(h.slot, 7);
            assert_eq!(h.serial, 42);
        }
        other => panic!("expected Value::Handle, got {:?}", other.type_name()),
    }
}

// ── 2. Round-trip through a script ───────────────────────────────

#[test]
fn handle_round_trips_through_script() {
    for backend in [Backend::Graph, Backend::Bytecode] {
        let (mut env, class) = env_with_test_class();
        env.set_backend(backend);
        let pid = env
            .load_program("let h = binding(symbol(\"h\"))\nh")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        let handle = env.make_handle(class, 3, 9);
        let sym = env.intern_symbol("h");
        env.set_binding(sym, handle);
        let result = env.run(sid).unwrap();
        assert_eq!(result, handle, "[{backend:?}] handle must round-trip intact");
    }
}

// ── 3. type(h) ───────────────────────────────────────────────────

#[test]
fn type_of_handle_is_handle() {
    check_output_both_backends(
        &[("h", 1, 1)],
        "let h = binding(symbol(\"h\"))\nprint(type(h))",
        &["handle"],
    );
}

// ── 4. Equality in-script ────────────────────────────────────────

#[test]
fn handles_with_same_triple_are_equal_in_script() {
    // a and b are minted separately with the same (class, slot, serial).
    check_result_both_backends(
        &[("a", 5, 8), ("b", 5, 8)],
        "binding(symbol(\"a\")) == binding(symbol(\"b\"))",
        Value::Bool(true),
    );
}

#[test]
fn handles_with_different_serial_are_not_equal_in_script() {
    check_result_both_backends(
        &[("a", 5, 8), ("b", 5, 9)],
        "binding(symbol(\"a\")) == binding(symbol(\"b\"))",
        Value::Bool(false),
    );
}

// ── 5. Truthiness & nil ──────────────────────────────────────────

#[test]
fn handle_is_truthy_host_side() {
    let (env, class) = env_with_test_class();
    let v = env.make_handle(class, 0, 0);
    assert!(v.is_truthy(), "a handle is truthy regardless of slot/serial");
}

#[test]
fn handle_is_truthy_in_script() {
    check_result_both_backends(
        &[("h", 2, 2)],
        "let h = binding(symbol(\"h\"))\nif h then \"truthy\" else \"falsy\" end == \"truthy\"",
        Value::Bool(true),
    );
}

#[test]
fn handle_is_not_equal_to_nil() {
    check_result_both_backends(
        &[("h", 2, 2)],
        "binding(symbol(\"h\")) == nil",
        Value::Bool(false),
    );
}

// ── 6. Host-side HandleVal equality & hash ───────────────────────

#[test]
fn handle_val_equality_and_hash() {
    let class = HandleClassId(1);
    let a = HandleVal { class, slot: 4, serial: 10 };
    let b = HandleVal { class, slot: 4, serial: 10 };
    let c = HandleVal { class, slot: 4, serial: 11 }; // different serial
    let d = HandleVal { class: HandleClassId(2), slot: 4, serial: 10 }; // different class

    assert_eq!(a, b, "same (class, slot, serial) must be equal");
    assert_ne!(a, c, "different serial must not be equal");
    assert_ne!(a, d, "different class must not be equal");

    // HandleVal is Copy + Hash: usable as a set/map key.
    let mut set = HashSet::new();
    set.insert(a);
    set.insert(b); // duplicate of a
    set.insert(c);
    set.insert(d);
    assert_eq!(set.len(), 3, "a and b must hash/compare as one entry");
}
