use petal::env::Env;
use petal::native_fn::{NativeResult, PetalCxt};

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
    env.register_native("string_repeat", petal_string_repeat);
    let result = env.run_source(r#"print(string_repeat("abc", 3))"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_add_ints() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints);
    let result = env.run_source("print(add_ints(10, 20))");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_greet_with_output() {
    let mut env = Env::new();
    env.register_native("greet", petal_greet);
    let result = env.run_source(r#"greet("World")"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_no_args_returns_nil() {
    let mut env = Env::new();
    env.register_native("noop", petal_no_args);
    let result = env.run_source("let x = noop()\nprint(x)");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_arg_count() {
    let mut env = Env::new();
    env.register_native("count_args", petal_multi_type);
    let result = env.run_source(r#"print(count_args(1, "two", true))"#);
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_used_in_expression() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints);
    let result = env.run_source("let x = add_ints(3, 4) + 1\nprint(x)");
    assert!(result.is_ok(), "Error: {:?}", result.err());
}

#[test]
fn native_error_on_wrong_type() {
    let mut env = Env::new();
    env.register_native("add_ints", petal_add_ints);
    let result = env.run_source(r#"add_ints("not", "ints")"#);
    assert!(result.is_err());
}

#[test]
fn native_multiple_registrations() {
    let mut env = Env::new();
    env.register_native("string_repeat", petal_string_repeat);
    env.register_native("add_ints", petal_add_ints);
    let result = env.run_source(
        r#"
        let s = string_repeat("x", add_ints(2, 3))
        print(s)
    "#,
    );
    assert!(result.is_ok(), "Error: {:?}", result.err());
}
