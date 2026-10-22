//! The `map` value type: `Map()` / `Map(pairs)`, keyed by any hashable value
//! compared by content (`core/src/dict.rs`, `Value::Dict`).
//!
//! These pin the script-facing contract — construction, content keys,
//! reading and writing (`m[k]`, `set m[k]`), the overloaded builtins,
//! iteration order, equality and printing — and every boundary a map has to
//! cross intact: the collector, JSON, `get_state_json`, `--state-storage`
//! documents and a hot reload.

use petal::env::Env;
use petal::heap::Heap;
use petal::policy::RunPolicy;
use petal::value::{Value, json_to_value, value_to_display_string, value_to_json};

fn eval(src: &str) -> String {
    let mut env = Env::new();
    let v = env
        .run_source(&format!("import json; {src}"))
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

/// What `code` printed, run once under `policy`.
fn printed(code: &str, policy: RunPolicy) -> String {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(code).unwrap_or_else(|e| panic!("{e}\n{code}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{e}\n{code}"));
    env.take_output().join("\n")
}

/// `code` prints `expected` with every optimization on and with none.
#[track_caller]
fn assert_prints(code: &str, expected: &str) {
    assert_eq!(printed(code, RunPolicy::FAST), expected, "optimized:\n{code}");
    assert_eq!(printed(code, RunPolicy::BASELINE), expected, "baseline:\n{code}");
}

#[test]
fn constructs_and_prints() {
    assert_eq!(eval("Map()"), "Map()");
    assert_eq!(eval("type(Map())"), "map");
    assert_eq!(eval("Map([[1, \"one\"], [\"k\", true]])"), "Map([[1, \"one\"], [\"k\", true]])");
    assert_eq!(eval("len(Map([[1, 2], [3, 4]]))"), "2");
    // A repeated key keeps its first place and its last value.
    assert_eq!(eval("Map([[1, \"a\"], [2, \"b\"], [1, \"c\"]])"), "Map([[1, \"c\"], [2, \"b\"]])");
    assert_eq!(eval("\"{Map([[1, 2]])}\""), "Map([[1, 2]])");
}

#[test]
fn keys_are_compared_by_content() {
    // `2 == 2.0`, so they are one key.
    assert_eq!(eval("let m = Map([[2, \"two\"]])\nm[2.0]"), "two");
    // A list, a record (any field order), a map and an enum variant, each
    // rebuilt rather than reused.
    assert_eq!(eval("let m = Map([[[1, 2], \"pair\"]])\nm[[1, 2]]"), "pair");
    assert_eq!(eval("let m = Map([[{a: 1, b: 2}, 7]])\nm[{b: 2, a: 1}]"), "7");
    assert_eq!(eval("let m = Map([[Map([[1, 2]]), \"inner\"]])\nm[Map([[1, 2]])]"), "inner");
    assert_eq!(
        eval("enum Dir\n  north,\n  south\nend\nlet m = Map([[Dir.north, 1]])\nm[Dir.north]"),
        "1"
    );
    // Different content, different keys: `1` and `"1"` are not equal.
    assert_eq!(eval("let m = Map([[1, \"int\"], [\"1\", \"str\"]])\n[m[1], m[\"1\"], len(m)]"), "[\"int\", \"str\", 2]");
}

#[test]
fn an_absent_key_reads_as_nil() {
    assert_eq!(eval("Map()[1]"), "nil");
    assert_eq!(eval("Map()[1] ?? \"fallback\""), "fallback");
    assert_eq!(eval("let m = Map()\nm?.[1]"), "nil");
    assert_eq!(eval("let m = nil\nm?.[1]"), "nil");
}

#[test]
fn unhashable_keys_are_rejected() {
    for (src, what) in [
        ("let m = Map()\nm[fn(x) -> x] = 1", "a function"),
        ("let m = Map()\nm[[1, fn(x) -> x]] = 1", "a function"),
        ("Map([[sqrt(-1.0), 1]])", "NaN"),
        ("Map()[sqrt(-1.0)]", "NaN"),
        ("contains(Map(), print)", "a function"),
    ] {
        let e = eval_err(src);
        assert!(e.contains(&format!("Cannot use {what} as a map key")), "{src:?}: {e}");
    }
    assert!(eval_err("Map([[1, 2], 3])").contains("element 1 is not a pair (got int)"));
    assert!(eval_err("Map(3)").contains("Map() expects a list of [key, value] pairs, got int"));
    assert!(eval_err("let m = Map()\nm.size").contains("Cannot access field 'size' on map"));
}

#[test]
fn a_local_write_rebinds_and_leaves_other_holders_alone() {
    assert_prints(
        "let m = Map()\nlet before = m\nm[1] = \"one\"\nm[[2]] = \"two\"\nm[1] = \"uno\"\n\
         print(m)\nprint(before)",
        "Map([[1, \"uno\"], [[2], \"two\"]])\nMap()",
    );
    // A function's write is to its own binding.
    assert_prints(
        "fn add(m, k)\n  m[k] = true\n  m\nend\nlet m = Map()\nlet n = add(m, 5)\nprint(len(m), len(n))",
        "0 1",
    );
}

#[test]
fn set_writes_a_shared_map_through_a_var() {
    assert_prints(
        "var counts = Map()\n\
         fn bump(k)\n  set counts[k] = (get counts[k] ?? 0) + 1\nend\n\
         for w in [\"a\", \"b\", \"a\", [1], \"a\", [1]] do\n  bump(w)\nend\n\
         print(get counts)",
        "Map([[\"a\", 3], [\"b\", 1], [[1], 2]])",
    );
    // A snapshot taken before the writes keeps what it read.
    assert_prints(
        "var m = Map([[1, 1]])\nlet snap = get m\nset m[2] = 2\nset m[1] = 9\nprint(snap, get m)",
        "Map([[1, 1]]) Map([[1, 9], [2, 2]])",
    );
}

#[test]
fn many_writes_stay_linear() {
    // Ten thousand inserts each through a `let`, a `var` written by a helper,
    // and a `state var`: all three write in place, so the whole run copies a
    // map a handful of times rather than once per insert.
    let code = "let a = Map()\nfor i in range(10000) do\n  a[i] = i\nend\n\
         var b = Map()\nfn put(i)\n  set b[i] = i * 2\nend\nfor i in range(10000) do\n  put(i)\nend\n\
         state var c = Map()\nfor i in range(10000) do\n  set c[[i]] = i\nend\n\
         print(len(a), a[9999], len(get b), (get b)[9999], len(get c), (get c)[[9999]])";
    let mut env = Env::new();
    env.set_policy(RunPolicy::FAST);
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().join("\n"), "10000 9999 10000 19998 10000 9999");
    if petal::stats::DUP_STATS_ENABLED {
        let copies = env.dup_stats().total_count();
        assert!(copies <= 10, "{copies} copies: a write is falling back to clone-and-alloc");
    }
    // The clone-and-alloc baseline agrees, on a size it can afford.
    assert_prints(
        "let a = Map()\nfor i in range(200) do\n  a[i] = i\nend\n\
         var b = Map()\nfn put(i)\n  set b[i] = i * 2\nend\nfor i in range(200) do\n  put(i)\nend\n\
         print(len(a), a[199], len(get b), (get b)[199])",
        "200 199 200 398",
    );
}

#[test]
fn a_key_stored_in_a_map_is_not_edited_under_it() {
    // `xs` is a fresh list and its last use would let the write after it
    // edit it in place; but the map kept it as a key, so that would change
    // the key under the map's hash index.
    assert_prints(
        "let xs = [1, 2]\nlet m = Map()\nm[xs] = \"a\"\nxs[0] = 9\nprint(m[[1, 2]], m[[9, 2]], xs)",
        "a nil [9, 2]",
    );
}

#[test]
fn the_record_builtins_take_a_map() {
    let m = "let m = Map([[1, \"one\"], [\"k\", true], [[2], nil]])\n";
    assert_eq!(eval(&format!("{m}keys(m)")), "[1, \"k\", [2]]");
    assert_eq!(eval(&format!("{m}values(m)")), "[\"one\", true, nil]");
    assert_eq!(eval(&format!("{m}entries(m)")), "[[1, \"one\"], [\"k\", true], [[2], nil]]");
    assert_eq!(eval(&format!("{m}Map(entries(m)) == m")), "true");
    // Membership is by key, and a key stored with a nil value is present.
    assert_eq!(eval(&format!("{m}[contains(m, [2]), contains(m, \"one\"), m.includes(1)]")), "[true, false, true]");
    assert_eq!(eval(&format!("{m}remove(m, \"k\")")), "Map([[1, \"one\"], [[2], nil]])");
    assert_eq!(eval(&format!("{m}remove(m, \"absent\") == m")), "true");
    assert_eq!(eval(&format!("{m}len(m)")), "3");
    // `entries` reads a record too.
    assert_eq!(eval("entries({a: 1, b: 2})"), "[[\"a\", 1], [\"b\", 2]]");
}

#[test]
fn removal_keeps_order_and_lookups_intact() {
    assert_prints(
        "let m = Map()\nfor i in range(6) do\n  m[i] = i * 10\nend\n\
         m = remove(m, 0)\nm = remove(m, 3)\nm[0] = \"back\"\n\
         print(keys(m))\nprint(m[1], m[2], m[3], m[4], m[5], m[0])",
        "[1, 2, 4, 5, 0]\n10 20 nil 40 50 back",
    );
}

#[test]
fn for_visits_keys_in_insertion_order() {
    assert_prints(
        "let m = Map([[\"z\", 1], [\"a\", 2], [[3], 3]])\nm[\"z\"] = 10\n\
         for k in m do\n  print(k, m[k])\nend\n\
         print(for k in m do str(k) end)",
        "z 10\na 2\n[3] 3\n[\"z\", \"a\", \"[3]\"]",
    );
}

#[test]
fn equality_and_hashing_ignore_insertion_order() {
    assert_eq!(eval("Map([[1, 2], [3, 4]]) == Map([[3, 4], [1, 2]])"), "true");
    assert_eq!(eval("Map([[1, 2]]) == Map([[1, 3]])"), "false");
    assert_eq!(eval("Map([[1, 2]]) == Map([[1, 2], [3, 4]])"), "false");
    assert_eq!(eval("Map([[\"a\", 1]]) == {a: 1}"), "false");
    // Equal maps are one key.
    assert_eq!(
        eval("let outer = Map([[Map([[1, 2], [3, 4]]), \"x\"]])\nouter[Map([[3, 4], [1, 2]])]"),
        "x"
    );
}

#[test]
fn a_map_annotation_is_checked() {
    assert_eq!(eval("fn size(m: map) -> int\n  len(m)\nend\nsize(Map([[1, 1]]))"), "1");
    let mut env = Env::new();
    let pid = env
        .load_program("let m: map = Map()\nlet r: record = Map()\nkeys([1])\n")
        .unwrap();
    let warnings = env.get_program(pid).unwrap().warnings.iter().map(|d| d.message.clone());
    let warnings = warnings.collect::<Vec<_>>().join("\n");
    assert!(warnings.contains("`r` declared `record` but assigned `map`"), "{warnings}");
    assert!(warnings.contains("expected a record or map, found `list`"), "{warnings}");
    assert!(!warnings.contains("`m` declared"), "{warnings}");
}

#[test]
fn json_round_trips_as_a_map() {
    assert_eq!(
        eval("json_stringify(Map([[1, \"a\"], [[2], true]]))"),
        r#"{"entries":[[1,"a"],[[2],true]],"type":"map"}"#
    );
    assert_eq!(
        eval("let m = Map([[1, \"a\"], [{x: 1}, Map([[2, 3]])]])\njson_parse(json_stringify(m)) == m"),
        "true"
    );
    // A look-alike with any other field stays a record.
    assert_eq!(
        eval(r#"type(json_parse("\{\"type\":\"map\",\"entries\":[],\"x\":1}"))"#),
        "record"
    );

    let mut heap = Heap::new();
    let one = heap.alloc_string("one".into());
    let id = heap.dict_from_pairs([(Value::Int(1), Value::String(one))]).unwrap();
    let json = value_to_json(&Value::Dict(id), &heap);
    assert_eq!(json, serde_json::json!({ "type": "map", "entries": [[1, "one"]] }));
    let back = json_to_value(&json, &mut heap).unwrap();
    assert!(petal::value::values_equal(&back, &Value::Dict(id), &heap));
}

#[test]
fn state_json_round_trips_as_a_map() {
    let src = "state var hits = Map()\nset hits[[1, 2]] = (get hits[[1, 2]] ?? 0) + 1\n";
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let saved = env.get_state_json(pid, sid)["hits"].clone();
    assert_eq!(saved, serde_json::json!({ "type": "map", "entries": [[[1, 2], 1]] }));

    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.set_state_from_json(pid, sid, "hits", &saved).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.get_state_json(pid, sid)["hits"],
        serde_json::json!({ "type": "map", "entries": [[[1, 2], 2]] })
    );
}

#[test]
fn state_storage_round_trips_a_map() {
    let src = "state var seen = Map()\nset seen[len(get seen)] = {at: len(get seen)}\nprint(get seen)\n";
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let saved = env.save_state_storage(pid, sid);
    assert!(saved.skipped.is_empty(), "{:?}", saved.skipped);
    let text = saved.document.to_string();
    assert!(text.contains(r#""$petal":"map""#), "{text}");

    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.load_state_storage(pid, sid, &saved.document).unwrap();
    env.take_output();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().join("\n"), "Map([[0, { at: 0 }], [1, { at: 1 }]])");
}

#[test]
fn a_hot_reload_keeps_a_map_in_state() {
    let old = "state var m = Map()\nset m[[len(get m)]] = true\n";
    let new = "state var m = Map()\nset m[[len(get m)]] = true\nprint(keys(get m))\n";
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(old).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    env.reload_program(sid, new, None).unwrap();
    env.reset_stack(sid).unwrap();
    env.take_output();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().join("\n"), "[[0], [1], [2]]");
}

#[test]
fn the_collector_keeps_keys_and_values_alive() {
    let mut heap = Heap::new();
    let k = heap.alloc_list(vec![Value::Int(1)]);
    let v = heap.alloc_string("kept".into());
    let id = heap.dict_from_pairs([(Value::List(k), Value::String(v))]).unwrap();
    let dead = heap.dict_from_pairs([(Value::Int(1), Value::Int(2))]).unwrap();
    heap.mark_value(Value::Dict(id));
    heap.sweep();
    assert!(heap.is_live(Value::Dict(id)));
    assert!(heap.is_live(Value::List(k)), "a key is reachable from its map");
    assert!(heap.is_live(Value::String(v)), "a value is reachable from its map");
    assert!(!heap.is_live(Value::Dict(dead)));
    assert_eq!(heap.dict_get(id, Value::List(k)).unwrap(), Some(Value::String(v)));
}

#[test]
fn churning_maps_through_a_long_loop_survives_collection() {
    assert_prints(
        "let keep = Map([[\"k\", [1, 2, 3]]])\nvar total = 0\n\
         for i in range(30000) do\n  let m = Map([[i, [i]], [[i], i]])\n  set total = total + m[[i]] + m[i][0]\nend\n\
         print(get total, keep)",
        "899970000 Map([[\"k\", [1, 2, 3]]])",
    );
}

#[test]
fn freeze_reaches_keys_and_values() {
    assert_eq!(
        eval("let m = freeze(Map([[[1], [2]]]))\n[frozen(m), frozen(keys(m)[0]), frozen(values(m)[0])]"),
        "[true, true, true]"
    );
}
