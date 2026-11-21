//! `StaticValue::to_plain_json` gives a literal the JSON `value_to_json` gives
//! the value it evaluates to, and `from_plain_json` reads it back.

use petal::env::Env;
use petal::static_value::{StaticValue, get_static_value};
use serde_json::json;

/// `literal`'s plain JSON read statically, and the JSON of its value.
fn both(literal: &str, prelude: &str) -> (serde_json::Value, serde_json::Value) {
    let source = format!("{prelude}\nlet x = {literal}\nx\n");
    let stat = get_static_value(&source, "x").unwrap_or_else(|e| panic!("{literal}: {e:?}"));
    let mut env = Env::new();
    let pid = env.load_program(&source).unwrap();
    let stack = env.create_stack(pid).unwrap();
    let v = env.run(stack).unwrap();
    (stat.to_plain_json(), petal::value::value_to_json(&v, env.heap()).unwrap())
}

#[test]
fn a_literal_and_its_value_are_the_same_plain_json() {
    let enums = "enum Shape\n  circle(r),\n  dot,\nend";
    for lit in [
        r#"{name: "a", n: 3, f: 1.5, on: true, none: nil}"#,
        "[1, [2, 3], \"x\"]",
        ":ff0066",
        ":ff006680",
        "<card x={10} label=\"hi\">text <b>bold</b></card>",
        "Shape.circle(2)",
        "Shape.dot",
    ] {
        let (stat, eval) = both(lit, enums);
        assert_eq!(stat, eval, "{lit}");
    }
}

#[test]
fn plain_json_reads_back() {
    for lit in [
        r#"{name: "a", n: 3, f: 1.5, on: true}"#,
        "[1, [2, 3], \"x\"]",
        ":ff0066",
        ":ff006680",
        "<card x={10}>text</card>",
        "vec3(1, 2, 3)",
    ] {
        let v = get_static_value(&format!("let x = {lit}\n"), "x").unwrap();
        assert_eq!(StaticValue::from_plain_json(&v.to_plain_json()), v, "{lit}");
    }
}

#[test]
fn plain_json_shapes() {
    let call = get_static_value("let x = vec3(1, 2, 3)\n", "x").unwrap();
    assert_eq!(call.to_plain_json(), json!({"type": "call", "function": "vec3", "args": [1, 2, 3]}));
    // Not a colour: a fifth key, or a channel out of range.
    assert!(matches!(StaticValue::from_plain_json(&json!({"r": 1, "g": 2, "b": 3, "x": 4})), StaticValue::Record(_)));
    assert!(matches!(StaticValue::from_plain_json(&json!({"r": 1, "g": 2, "b": 300})), StaticValue::Record(_)));
    assert_eq!(StaticValue::from_plain_json(&json!(2.0)), StaticValue::Float(2.0));
    assert_eq!(StaticValue::from_plain_json(&json!(2)), StaticValue::Int(2));
}
