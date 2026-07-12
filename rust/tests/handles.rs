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

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use petal::env::Env;
use petal::native_fn::{NativeFn, NativeResult, PetalCxt};
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

/// Run `source` with the given named handle bindings (bound as uniforms, read
/// in-script via `binding(symbol("name"))`).
/// Returns (program result, print output).
fn run_with_handles(
    bindings: &[(&str, u32, u32)], // (name, slot, serial) minted on the TestEntity class
    source: &str,
) -> (Value, Vec<String>) {
    let (mut env, class) = env_with_test_class();
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

/// Assert `source` produces the expected program result.
fn check_result(bindings: &[(&str, u32, u32)], source: &str, expect: Value) {
    let (result, _out) = run_with_handles(bindings, source);
    assert_eq!(result, expect, "source: {source}");
}

/// Assert `source` produces the expected print output.
fn check_output(bindings: &[(&str, u32, u32)], source: &str, expect: &[&str]) {
    let (_result, out) = run_with_handles(bindings, source);
    assert_eq!(out, expect, "source: {source}");
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
    let (mut env, class) = env_with_test_class();
    let pid = env
        .load_program("let h = binding(symbol(\"h\"))\nh")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    let handle = env.make_handle(class, 3, 9);
    let sym = env.intern_symbol("h");
    env.set_binding(sym, handle);
    let result = env.run(sid).unwrap();
    assert_eq!(result, handle, "handle must round-trip intact");
}

// ── 3. type(h) ───────────────────────────────────────────────────

#[test]
fn type_of_handle_is_handle() {
    check_output(
        &[("h", 1, 1)],
        "let h = binding(symbol(\"h\"))\nprint(type(h))",
        &["handle"],
    );
}

// ── 4. Equality in-script ────────────────────────────────────────

#[test]
fn handles_with_same_triple_are_equal_in_script() {
    // a and b are minted separately with the same (class, slot, serial).
    check_result(
        &[("a", 5, 8), ("b", 5, 8)],
        "binding(symbol(\"a\")) == binding(symbol(\"b\"))",
        Value::Bool(true),
    );
}

#[test]
fn handles_with_different_serial_are_not_equal_in_script() {
    check_result(
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
    assert!(
        v.is_truthy(),
        "a handle is truthy regardless of slot/serial"
    );
}

#[test]
fn handle_is_truthy_in_script() {
    check_result(
        &[("h", 2, 2)],
        "let h = binding(symbol(\"h\"))\nif h then \"truthy\" else \"falsy\" end == \"truthy\"",
        Value::Bool(true),
    );
}

#[test]
fn handle_is_not_equal_to_nil() {
    check_result(
        &[("h", 2, 2)],
        "binding(symbol(\"h\")) == nil",
        Value::Bool(false),
    );
}

// ── 6. Host-side HandleVal equality & hash ───────────────────────

#[test]
fn handle_val_equality_and_hash() {
    let class = HandleClassId(1);
    let a = HandleVal {
        class,
        slot: 4,
        serial: 10,
    };
    let b = HandleVal {
        class,
        slot: 4,
        serial: 10,
    };
    let c = HandleVal {
        class,
        slot: 4,
        serial: 11,
    }; // different serial
    let d = HandleVal {
        class: HandleClassId(2),
        slot: 4,
        serial: 10,
    }; // different class

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

// ═════════════════════════════════════════════════════════════════
// CHUNK 2: handle method dispatch + the `is_valid` builtin
// (docs/dev/unreal-ffi-proposal.md §5).
//
// Contract under test:
//  - `h.method(args...)` on a `Value::Handle` dispatches through
//    `handle_classes[h.class].call_method(&mut cxt, "method")` with
//    cxt args = [receiver, args...]. This arm runs BEFORE the UFCS
//    native-table fallback.
//  - If the class's `is_valid` says the handle is stale, dispatch errors
//    with a message containing the class name and `describe()` output,
//    WITHOUT invoking `call_method`.
//  - `call_method` returning Err surfaces as a script runtime error.
//  - New builtin `is_valid(v)`: true for a live handle, false for a stale
//    handle (no error — that is its purpose), false for any non-handle
//    (nil, ints, ...), never an error.
// ═════════════════════════════════════════════════════════════════

/// State captured by a Counter test class, one set per Env/backend run.
struct CounterState {
    cell: Rc<RefCell<i64>>,
    alive: Rc<RefCell<bool>>,
    was_called: Rc<RefCell<bool>>,
}

impl CounterState {
    fn new(initial: i64) -> Self {
        Self {
            cell: Rc::new(RefCell::new(initial)),
            alive: Rc::new(RefCell::new(true)),
            was_called: Rc::new(RefCell::new(false)),
        }
    }
}

/// A "Counter" handle class over the captured state.
/// Methods (receiver is cxt arg 1, method args follow):
///   get()  -> current cell value
///   add(n) -> adds n to the cell, returns the new value
/// Anything else: Err("no method '{name}' on Counter").
fn counter_class(state: &CounterState) -> HandleClass {
    let cell = state.cell.clone();
    let alive = state.alive.clone();
    let was_called = state.was_called.clone();
    HandleClass {
        name: "Counter".to_string(),
        is_valid: Box::new(move |_slot, _serial| *alive.borrow()),
        describe: Box::new(|slot, serial| format!("Counter(slot={slot}, serial={serial})")),
        call_method: Box::new(move |cxt: &mut PetalCxt, method: &str| -> NativeResult {
            *was_called.borrow_mut() = true;
            match method {
                "get" => {
                    let v = *cell.borrow();
                    cxt.push_int(v);
                    Ok(1)
                }
                "add" => {
                    // Receiver is arg 1; the method argument is arg 2.
                    let n = cxt.get_int(2)?;
                    *cell.borrow_mut() += n;
                    let v = *cell.borrow();
                    cxt.push_int(v);
                    Ok(1)
                }
                other => Err(format!("no method '{other}' on Counter")),
            }
        }),
    }
}

/// Run `source` in a fresh Env with `class` registered and a
/// handle of that class (slot=1, serial=1) bound as the uniform "h".
/// Extra global natives (for UFCS-shadowing tests) are registered before load.
fn run_with_class(
    class: HandleClass,
    natives: &[(&str, NativeFn)],
    source: &str,
) -> Result<Value, String> {
    let mut env = Env::new();
    for (name, func) in natives {
        env.register_native(name, *func);
    }
    let class_id = env.register_handle_class(class);
    let pid = env.load_program(source)?;
    let sid = env.create_stack(pid)?;
    let handle = env.make_handle(class_id, 1, 1);
    let sym = env.intern_symbol("h");
    env.set_binding(sym, handle);
    env.run(sid)
}

/// Run `source` in a fresh Env with no handle classes or bindings.
fn run_plain(source: &str) -> Result<Value, String> {
    let mut env = Env::new();
    let pid = env.load_program(source)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)
}

// ── 7. Method dispatch through the handle class ──────────────────

#[test]
fn handle_method_dispatch_add_then_get() {
    let state = CounterState::new(0);
    let result = run_with_class(
        counter_class(&state),
        &[],
        "let h = binding(symbol(\"h\"))\nh.add(5)\nh.get()",
    );
    assert_eq!(
        result,
        Ok(Value::Int(5)),
        "h.add(5) then h.get() must dispatch through call_method"
    );
    assert_eq!(*state.cell.borrow(), 5, "add(5) must mutate host state");
    assert!(*state.was_called.borrow(), "call_method must be invoked");
}

#[test]
fn handle_method_add_returns_new_value() {
    let state = CounterState::new(10);
    let result = run_with_class(
        counter_class(&state),
        &[],
        "let h = binding(symbol(\"h\"))\nh.add(7)",
    );
    assert_eq!(
        result,
        Ok(Value::Int(17)),
        "add returns the post-mutation value"
    );
}

// ── 8. Stale handles: dispatch is blocked before call_method ─────

#[test]
fn stale_handle_method_errors_with_describe_and_skips_call_method() {
    let state = CounterState::new(0);
    *state.alive.borrow_mut() = false; // handle goes stale before the run
    let result = run_with_class(
        counter_class(&state),
        &[],
        "let h = binding(symbol(\"h\"))\nh.get()",
    );
    let err = result.expect_err(&format!(
        "calling a method on a stale handle must be a runtime error"
    ));
    assert!(
        err.contains("Counter"),
        "stale-handle error must name the class, got: {err}"
    );
    assert!(
        err.contains("Counter(slot=1, serial=1)"),
        "stale-handle error must include describe() output, got: {err}"
    );
    assert!(
        !*state.was_called.borrow(),
        "call_method must NOT be invoked for a stale handle"
    );
}

// ── 9. Unknown method: call_method's Err surfaces to the script ──

#[test]
fn unknown_handle_method_surfaces_call_method_error() {
    let state = CounterState::new(0);
    let result = run_with_class(
        counter_class(&state),
        &[],
        "let h = binding(symbol(\"h\"))\nh.frobnicate()",
    );
    let err = result.expect_err(&format!("unknown handle method must be a runtime error"));
    assert!(
        err.contains("frobnicate"),
        "error must name the missing method, got: {err}"
    );
    // The error must come from the class's own dispatcher (it says
    // "on Counter"), not from the generic no-method fallback.
    assert!(
        err.contains("Counter"),
        "error must come from the Counter dispatcher, got: {err}"
    );
    assert!(
        *state.was_called.borrow(),
        "the class dispatcher must have been consulted"
    );
}

// ── 10. Handle-class dispatch wins over the UFCS native fallback ─

fn native_get_999(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_int(999);
    Ok(1)
}

#[test]
fn handle_class_method_shadows_ufcs_native() {
    let state = CounterState::new(5);
    // Note: a builtin `get(container, key)` also already exists in the
    // native table, and UFCS name lookup returns the first match — so
    // today `h.get()` resolves to that builtin (arity error). Either way,
    // after chunk 2 the handle class's own `get` must win over the table.
    let result = run_with_class(
        counter_class(&state),
        &[("get", native_get_999)], // global native `get` would return 999
        "let h = binding(symbol(\"h\"))\nh.get()",
    );
    assert_eq!(
        result,
        Ok(Value::Int(5)),
        "the handle class's 'get' must win over the global native 'get'"
    );
}

// ── 11. The `is_valid` builtin ───────────────────────────────────

#[test]
fn is_valid_true_for_live_handle() {
    let state = CounterState::new(0);
    let result = run_with_class(
        counter_class(&state),
        &[],
        "is_valid(binding(symbol(\"h\")))",
    );
    assert_eq!(result, Ok(Value::Bool(true)), "live handle is valid");
}

#[test]
fn is_valid_false_for_stale_handle_without_error() {
    let state = CounterState::new(0);
    *state.alive.borrow_mut() = false;
    let result = run_with_class(
        counter_class(&state),
        &[],
        "is_valid(binding(symbol(\"h\")))",
    );
    assert_eq!(
        result,
        Ok(Value::Bool(false)),
        "is_valid must return false (not error) on a stale handle"
    );
}

#[test]
fn is_valid_false_for_nil() {
    let result = run_plain("is_valid(nil)");
    assert_eq!(result, Ok(Value::Bool(false)), "nil is not a valid handle");
}

#[test]
fn is_valid_false_for_non_handle_value() {
    let result = run_plain("is_valid(5)");
    assert_eq!(
        result,
        Ok(Value::Bool(false)),
        "a non-handle value is simply not a valid handle (no error)"
    );
}

// ── 12. Regression guard: UFCS on non-handles keeps working ──────
// (Passes today; must still pass once the handle-dispatch arm is added.
//  UFCS is confirmed in ts/test/method-syntax.test.ts, e.g. `[1,2,3].len()`.)

fn native_double(cxt: &mut PetalCxt) -> NativeResult {
    let n = cxt.get_int(1)?;
    cxt.push_int(n * 2);
    Ok(1)
}

#[test]
fn ufcs_on_non_handle_receiver_still_works() {
    let mut env = Env::new();
    env.register_native("double", native_double);
    let pid = env.load_program("let x = 4\nx.double()").unwrap();
    let sid = env.create_stack(pid).unwrap();
    let result = env.run(sid);
    assert_eq!(
        result,
        Ok(Value::Int(8)),
        "UFCS native fallback on non-handle receivers must keep working"
    );
}
