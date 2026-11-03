//! A record crosses into JSON with its keys in the order it holds them,
//! which is the order they were written.
//!
//! Hosts read script values through `value_to_json` (natives' arguments,
//! element `props`, state dumps), and a host that writes a value back to
//! source (goal-based editing, petal-images) must not reorder the user's
//! file. serde_json sorts object keys unless its `preserve_order` feature is
//! on, which `core/Cargo.toml` turns on. Parsing keeps the order too.

use petal::env::Env;
use petal::value::{json_to_value, value_to_display_string, value_to_json};

fn run(src: &str) -> (Env, petal::value::Value) {
    let mut env = Env::new();
    let v = env
        .run_source(src)
        .unwrap_or_else(|e| panic!("{src:?} failed: {e}"));
    (env, v)
}

fn keys(json: &serde_json::Value) -> Vec<&str> {
    json.as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn a_record_keeps_its_written_key_order() {
    let (env, v) = run("{x: 1, y: 2, w: 3, h: 4, nested: {b: 1, a: 2}}");
    let json = value_to_json(&v, env.heap());
    assert_eq!(keys(&json), ["x", "y", "w", "h", "nested"]);
    assert_eq!(keys(&json["nested"]), ["b", "a"]);
}

#[test]
fn json_stringify_keeps_it_too() {
    let (env, v) = run("import json\njson_stringify({z: 1, a: 2, m: 3})");
    assert_eq!(value_to_display_string(&v, env.heap()), r#"{"z":1,"a":2,"m":3}"#);
}

#[test]
fn element_props_keep_their_attribute_order() {
    let (env, v) = run(r#"<text stroke="red" drop_shadow={2} align="left" />"#);
    let json = value_to_json(&v, env.heap());
    assert_eq!(keys(&json["props"]), ["stroke", "drop_shadow", "align"]);
}

#[test]
fn parsing_keeps_the_order_and_round_trips() {
    let (mut env, _) = run("nil");
    let json: serde_json::Value = serde_json::from_str(r#"{"w": 3, "h": 4, "x": 1}"#).unwrap();
    let v = json_to_value(&json, env.heap_mut()).unwrap();
    assert_eq!(value_to_display_string(&v, env.heap()), "{ w: 3, h: 4, x: 1 }");
    assert_eq!(value_to_json(&v, env.heap()), json);
    assert_eq!(keys(&value_to_json(&v, env.heap())), ["w", "h", "x"]);
}
