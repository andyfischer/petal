//! The native `vec3` type: a heap-allocated three-f64 vector (docs/dev/vec3.md).
//!
//! These pin the script-facing contract — construction, fields, the operator
//! set, equality, printing, the shared vector builtins — and the two places the
//! heap representation could leak: garbage collection (a live vec3 must
//! survive a sweep, a dead one must not) and the JSON boundary (a vec3 must
//! come back as a vec3, not as a `{type, x, y, z}` record).

use petal::env::Env;
use petal::heap::Heap;
use petal::value::{value_to_display_string, value_to_json, Value};

/// Run `src` and render its final value the way `print` would.
fn eval(src: &str) -> String {
    let mut env = Env::new();
    let v = env
        .run_source(src)
        .unwrap_or_else(|e| panic!("{src:?} failed: {e}"));
    value_to_display_string(&v, env.heap())
}

fn eval_err(src: &str) -> String {
    let mut env = Env::new();
    match env.run_source(src) {
        Ok(v) => panic!("{src:?} succeeded with {}", value_to_display_string(&v, env.heap())),
        Err(e) => e,
    }
}

#[test]
fn constructs_and_prints() {
    assert_eq!(eval("vec3(1, 2, 3)"), "vec3(1.0, 2.0, 3.0)");
    assert_eq!(eval("vec3(0.5, -1.25, 1e3)"), "vec3(0.5, -1.25, 1000.0)");
    assert_eq!(eval("type(vec3(1, 2, 3))"), "vec3");
    assert_eq!(eval("\"at {vec3(1, 2, 3)}\""), "at vec3(1.0, 2.0, 3.0)");
    assert_eq!(eval("[vec3(1, 2, 3)]"), "[vec3(1.0, 2.0, 3.0)]");
}

#[test]
fn keeps_full_f64_precision() {
    // The reason vec3 is on the heap rather than three inline f32s: a
    // component reads back exactly what was written.
    assert_eq!(eval("vec3(0.1, 0, 0).x == 0.1"), "true");
    assert_eq!(eval("vec3(1, 2, 16777217).z"), "16777217.0");
}

#[test]
fn fields() {
    assert_eq!(eval("let v = vec3(1, 2, 3)\nv.x + v.y * 10 + v.z * 100"), "321.0");
    let e = eval_err("vec3(1, 2, 3).w");
    assert!(e.contains("No field 'w' on vec3 (available: x, y, z)"), "{e}");
}

#[test]
fn arithmetic_matches_vec2_rules() {
    let a = "let a = vec3(1, 2, 3)\nlet b = vec3(4, 5, 6)\n";
    assert_eq!(eval(&format!("{a}a + b")), "vec3(5.0, 7.0, 9.0)");
    assert_eq!(eval(&format!("{a}b - a")), "vec3(3.0, 3.0, 3.0)");
    assert_eq!(eval(&format!("{a}a * b")), "vec3(4.0, 10.0, 18.0)");
    assert_eq!(eval(&format!("{a}b / a")), "vec3(4.0, 2.5, 2.0)");
    assert_eq!(eval(&format!("{a}a * 2")), "vec3(2.0, 4.0, 6.0)");
    assert_eq!(eval(&format!("{a}2 * a")), "vec3(2.0, 4.0, 6.0)");
    assert_eq!(eval(&format!("{a}a / 2")), "vec3(0.5, 1.0, 1.5)");
    assert_eq!(eval(&format!("{a}a + 1")), "vec3(2.0, 3.0, 4.0)");
    assert_eq!(eval(&format!("{a}10 - a")), "vec3(9.0, 8.0, 7.0)");
    assert_eq!(eval(&format!("{a}-a")), "vec3(-1.0, -2.0, -3.0)");
    // Operands are values: arithmetic never changes its inputs.
    assert_eq!(eval(&format!("{a}let c = a + b\na")), "vec3(1.0, 2.0, 3.0)");
    // Compound assignment goes through the same operators.
    assert_eq!(eval("var p = vec3(0, 0, 0)\nset p += vec3(1, 2, 3)\nset p *= 2\nget p"), "vec3(2.0, 4.0, 6.0)");
}

#[test]
fn arithmetic_errors() {
    assert!(eval_err("vec3(1, 2, 3) / 0").contains("Division by zero"));
    assert!(eval_err("vec3(1, 2, 3) / vec3(1, 0, 1)").contains("Division by zero in vec3"));
    let e = eval_err("vec3(1, 2, 3) + vec2(1, 2)");
    assert!(e.contains("vec3") && e.contains("vec2"), "{e}");
    let e = eval_err("vec3(1, 2, 3) + \"s\"");
    assert!(e.contains("Cannot perform arithmetic on vec3 and string"), "{e}");
    assert!(eval_err("1 / vec3(1, 2, 3)").contains("Unsupported vec3 operation"));
    assert!(eval_err("vec3(1, 2, 3) % 2").contains("Unsupported vec3 operation"));
}

#[test]
fn equality_compares_components() {
    assert_eq!(eval("vec3(1, 2, 3) == vec3(1.0, 2.0, 3.0)"), "true");
    assert_eq!(eval("vec3(1, 2, 3) == vec3(1, 2, 4)"), "false");
    assert_eq!(eval("vec3(1, 2, 3) != vec3(1, 2, 4)"), "true");
    assert_eq!(eval("vec3(1, 2, 3) == vec2(1, 2)"), "false");
    assert_eq!(eval("vec3(1, 2, 3) == {x: 1, y: 2, z: 3}"), "false");
    assert_eq!(eval("[vec3(1, 2, 3)] == [vec3(1, 2, 3)]"), "true");
    // Always truthy, like a record (a zero vec2 is falsy; see the docs).
    assert_eq!(eval("if vec3(0, 0, 0) then 1 else 2 end"), "1");
}

#[test]
fn vector_builtins() {
    assert_eq!(eval("mag(vec3(2, 3, 6))"), "7.0");
    assert_eq!(eval("distance(vec3(1, 1, 1), vec3(3, 4, 7))"), "7.0");
    assert_eq!(eval("dot(vec3(1, 2, 3), vec3(4, 5, 6))"), "32.0");
    assert_eq!(eval("cross(vec3(1, 0, 0), vec3(0, 1, 0))"), "vec3(0.0, 0.0, 1.0)");
    assert_eq!(eval("cross(vec3(0, 1, 0), vec3(1, 0, 0))"), "vec3(0.0, 0.0, -1.0)");
    assert_eq!(eval("normalize(vec3(0, 3, 4))"), "vec3(0.0, 0.6, 0.8)");
    assert_eq!(eval("normalize(vec3(0, 0, 0))"), "vec3(0.0, 0.0, 0.0)");
    assert_eq!(eval("limit(vec3(0, 6, 8), 5)"), "vec3(0.0, 3.0, 4.0)");
    assert_eq!(eval("limit(vec3(0, 1, 0), 5)"), "vec3(0.0, 1.0, 0.0)");
    assert_eq!(eval("lerp(vec3(0, 0, 0), vec3(10, 20, 30), 0.25)"), "vec3(2.5, 5.0, 7.5)");
    // lerp learned vec2 at the same time, and still does numbers.
    assert_eq!(eval("lerp(vec2(0, 0), vec2(10, 20), 0.5)"), "vec2(5.0, 10.0)");
    assert_eq!(eval("lerp(10, 20, 0.5)"), "15.0");

    assert!(eval_err("cross(vec2(1, 0), vec2(0, 1))").contains("cross() expects two vec3 values"));
    assert!(eval_err("dot(vec3(1, 0, 0), vec2(0, 1))").contains("dot() expects two vec2 or two vec3"));
    assert!(eval_err("lerp(vec3(0, 0, 0), 1, 0.5)").contains("two vectors of the same kind"));
    assert!(eval_err("rotate(vec3(1, 0, 0), 1.0)").contains("rotate() expects a vec2"));
}

#[test]
fn a_user_definition_shadows_the_builtin() {
    // Scripts that predate the builtin (cheesecake's prelude defines
    // `fn vec3(x, y, z)` returning a record) keep their own meaning.
    assert_eq!(
        eval("fn vec3(x, y, z)\n  {x: x, y: y, z: z}\nend\ntype(vec3(1, 2, 3))"),
        "record"
    );
    assert_eq!(eval("fn cross(a, b)\n  a + b\nend\ncross(1, 2)"), "3");
}

#[test]
fn json_round_trips_as_a_vec3() {
    assert_eq!(
        eval("json_stringify(vec3(1, 2.5, -3))"),
        r#"{"type":"vec3","x":1.0,"y":2.5,"z":-3.0}"#
    );
    // Decoded back to a vec3 — operators work on it — both bare and nested.
    assert_eq!(
        eval("json_parse(json_stringify(vec3(1, 2, 3))) + vec3(1, 1, 1)"),
        "vec3(2.0, 3.0, 4.0)"
    );
    assert_eq!(
        eval("json_parse(json_stringify({p: vec3(1, 2, 3), q: vec2(4, 5)}))"),
        "{ p: vec3(1.0, 2.0, 3.0), q: vec2(4.0, 5.0) }"
    );
    // An object that merely resembles the tag, with extra fields, stays a record.
    assert_eq!(
        eval(r#"type(json_parse("\{\"type\":\"vec3\",\"x\":1,\"y\":2,\"z\":3,\"w\":4}"))"#),
        "record"
    );
}

#[test]
fn state_json_round_trips_as_a_vec3() {
    let src = "state p = vec3(1, 2, 3)\np = p + vec3(1, 1, 1)\n";

    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let saved = env.get_state_json(pid, sid)["p"].clone();
    assert_eq!(
        saved,
        serde_json::json!({ "type": "vec3", "x": 2.0, "y": 3.0, "z": 4.0 })
    );

    // Restore into a fresh environment: the slot must come back as a vec3,
    // or the `+` on the next run would fail on a record.
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.set_state_from_json(pid, sid, "p", &saved).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.get_state_json(pid, sid)["p"],
        serde_json::json!({ "type": "vec3", "x": 3.0, "y": 4.0, "z": 5.0 })
    );
}

#[test]
fn the_collector_keeps_live_vec3s_and_frees_dead_ones() {
    let mut heap = Heap::new();
    let live = heap.vec3_value(1.0, 2.0, 3.0);
    let dead = heap.vec3_value(4.0, 5.0, 6.0);
    let list = heap.alloc_list(vec![live]);
    heap.mark_value(Value::List(list));
    heap.sweep();
    assert!(heap.is_live(live), "a vec3 reachable from a root survives");
    assert!(!heap.is_live(dead), "an unreachable vec3 is reclaimed");
    assert_eq!(
        value_to_json(&live, &heap),
        serde_json::json!({ "type": "vec3", "x": 1.0, "y": 2.0, "z": 3.0 })
    );
    // The freed slot is reused, under a new generation: the stale id stays dead.
    let reused = heap.vec3_value(7.0, 8.0, 9.0);
    assert!(heap.is_live(reused));
    assert!(!heap.is_live(dead));
}

#[test]
fn churning_vec3s_through_a_long_loop_survives_collection() {
    // Enough temporaries to force several collections mid-loop; the running
    // value and one captured early must both come through intact.
    let src = "let keep = vec3(9, 9, 9)\nvar p = vec3(0, 0, 0)\n\
               for i in range(0, 60000) do\n  set p = p + vec3(1, 2, 3) * 0.5\nend\n\
               [get p, keep]";
    assert_eq!(
        eval(src),
        "[vec3(30000.0, 60000.0, 90000.0), vec3(9.0, 9.0, 9.0)]"
    );
}
