// `state` inside `map`, `forEach` and `sort_by`'s key: one slot per element,
// keyed by index or by `key:` (docs/tasks/state-in-map.md).
//
// A callback that a builtin runs once per element, in element order, gets an
// element part on its path, as a loop iteration does: `[Call(map), Index(i)]`
// or `[Call(map), Key(h)]`. The other builtins (`filter`, `reduce`, `any`,
// `all`, `find`, comparator `sort`) keep the error; their tests are in
// state_call_paths.rs.

use std::collections::HashSet;

use petal::compiler::Compiler;
use petal::env::Env;
use petal::policy::RunPolicy;
use petal::program::StateKey;
use petal::stack::{PathPart, StackKey};

/// Run `src` `times` times on one stack under `policy` and return all output.
fn run_n_with(src: &str, times: usize, policy: RunPolicy) -> Vec<String> {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for i in 0..times {
        if i > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap_or_else(|e| panic!("run {i}: {e}"));
    }
    env.take_output()
}

/// Run under both policies, assert they agree, and return the output.
fn run_n(src: &str, times: usize) -> Vec<String> {
    let base = run_n_with(src, times, RunPolicy::BASELINE);
    let fast = run_n_with(src, times, RunPolicy::FAST);
    assert_eq!(base, fast, "baseline and fast policies disagree\n{src}");
    base
}

fn run_err(src: &str) -> String {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_err()
}

/// A stateful `seen`, then `rest`.
fn seen_then(rest: &str) -> String {
    format!("fn seen(x)\n  state n = 0\n  n += 1\n  n\nend\n{rest}\n")
}

fn base(name: &str) -> StateKey {
    StateKey(Compiler::hash_state_name(name))
}

fn paths_of(env: &Env, sid: StackKey, name: &str) -> HashSet<Vec<PathPart>> {
    let b = base(name);
    env.snapshot_state(sid)
        .unwrap()
        .keys()
        .filter(|k| k.base == b)
        .map(|k| k.path.to_vec())
        .collect()
}

/// Run `src` once and return the env and stack, for inspecting slots.
fn run_once(src: &str) -> (Env, StackKey) {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    (env, sid)
}

// ── Per element, by index ────────────────────────────────────────

#[test]
fn map_keeps_one_slot_per_element_like_a_for_loop() {
    let by_map = run_n(&seen_then("print(map([10, 20, 30], seen))"), 3);
    let by_for = run_n(&seen_then("print(for x in [10, 20, 30] do seen(x) end)"), 3);
    assert_eq!(by_map, ["[1, 1, 1]", "[2, 2, 2]", "[3, 3, 3]"]);
    assert_eq!(by_map, by_for);
}

#[test]
fn a_lambda_declaring_state_keeps_it_per_element() {
    let out = run_n(
        "print(map([1, 2, 3], fn(x)\n  state total = 0\n  total += x\n  total\nend))",
        3,
    );
    assert_eq!(out, ["[1, 2, 3]", "[2, 4, 6]", "[3, 6, 9]"]);
}

#[test]
fn map_slots_have_the_shape_of_a_loop_iteration() {
    let (env, sid) = run_once(&seen_then("let r = map([1, 2], seen)"));
    let paths = paths_of(&env, sid, "seen/n");
    assert_eq!(paths.len(), 2, "{paths:?}");
    for p in &paths {
        assert!(
            // `seen` is the callback: its frame is `[Call(map), Index(i)]`.
            matches!(p[..], [PathPart::Call(_), PathPart::Index(_)]),
            "{p:?}"
        );
    }
}

#[test]
fn for_each_and_sort_by_keep_one_slot_per_element() {
    let out = run_n(
        &seen_then(
            "state var log = []\n\
             forEach([5, 6], fn(x) set log = append(get log, seen(x)) end)\n\
             print(get log)\n\
             print(sort_by([3, 1, 2], fn(x) x * 10 + seen(x) end))",
        ),
        2,
    );
    assert_eq!(out, ["[1, 1]", "[1, 2, 3]", "[1, 1, 2, 2]", "[1, 2, 3]"]);
}

#[test]
fn std_helpers_built_on_map_keep_one_slot_per_element() {
    let out = run_n(
        &seen_then(
            "print(flat_map([1, 2], fn(x) [seen(x)] end))\n\
             print(min_by([3, 1, 2], fn(x) x + seen(x) end))\n\
             print(group_by([1, 2, 3], fn(x) seen(x) end))",
        ),
        2,
    );
    assert_eq!(out, [
            "[1, 1]",
            "1",
            r#"{ "1": [1, 2, 3] }"#,
            "[2, 2]",
            "1",
            r#"{ "2": [1, 2, 3] }"#,
        ]);
}

#[test]
fn a_loop_inside_a_stateful_callback_keeps_its_own_parts() {
    // The callback's own loop pushes its part above the element part and pops
    // only its own (the fast path's `set_loop_idx` too).
    let src = "\
fn row(x)
  let cells = for i in range(0, 2) do
    state c = 0
    c += x
    c
  end
  state after = 0
  after += 1
  [cells, after]
end
print(map([1, 10], row))
";
    let out = run_n(src, 2);
    assert_eq!(out, ["[[[1, 1], 1], [[10, 10], 1]]", "[[[2, 2], 2], [[20, 20], 2]]"]);
    let (env, sid) = run_once(src);
    let paths = paths_of(&env, sid, "row/c");
    assert_eq!(paths.len(), 4);
    for p in &paths {
        assert!(
            matches!(
                p[..],
                [PathPart::Call(_), PathPart::Index(_), PathPart::Index(_)]
            ),
            "{p:?}"
        );
    }
    let after = paths_of(&env, sid, "row/after");
    assert_eq!(after.len(), 2, "{after:?}");
}

#[test]
fn nested_maps_and_a_for_inside_map() {
    let out = run_n(
        &seen_then("print(map([1, 2], fn(x) map([1, 2, 3], seen) end))"),
        2,
    );
    assert_eq!(out, ["[[1, 1, 1], [1, 1, 1]]", "[[2, 2, 2], [2, 2, 2]]"]);
    let (env, sid) = run_once(&seen_then("let r = map([1, 2], fn(x) map([1, 2, 3], seen) end)"));
    assert_eq!(paths_of(&env, sid, "seen/n").len(), 6);

    let (env, sid) =
        run_once(&seen_then("let r = map([1, 2], fn(x) for y in [1, 2, 3] do seen(y) end end)"));
    let paths = paths_of(&env, sid, "seen/n");
    assert_eq!(paths.len(), 6);
    for p in &paths {
        assert!(
            matches!(
                p[..],
                [PathPart::Call(_), PathPart::Index(_), PathPart::Index(_), PathPart::Call(_)]
            ),
            "{p:?}"
        );
    }
}

#[test]
fn stable_id_from_a_map_callback_is_one_id_per_element_and_stable() {
    let out = run_n("let ids = map([1, 2, 3], fn(x) stable_id() end)\nprint(len(unique(ids)), ids)", 2);
    assert_eq!(out[0].split(' ').next(), Some("3"), "{out:?}");
    assert_eq!(out[0], out[1], "ids must persist across runs");
}

#[test]
fn a_memoized_function_called_from_map_sees_the_element() {
    // Its memo scope is keyed under the element part, so two elements do not
    // share an entry and each keeps its state.
    let src = "\
fn cell(x)
  state n = 0
  n += 1
  n * 100 + x
end
fn row(x)
  cell(x)
end
print(map([1, 2, 1], row))
";
    assert_eq!(run_n(src, 3), ["[101, 102, 101]", "[201, 202, 201]", "[301, 302, 301]"]);
}

#[test]
fn a_shrinking_list_sweeps_the_slots_of_gone_elements() {
    let mut env = Env::new();
    let pid = env.load_program(&seen_then("state xs = [1, 2, 3]\nlet r = map(xs, seen)\nxs = [1]")).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(paths_of(&env, sid, "seen/n").len(), 3);
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(paths_of(&env, sid, "seen/n").len(), 1);
}

// ── key: ─────────────────────────────────────────────────────────

const ITEMS: &str = "\
fn tally(t)
  state n = 0
  n += t.v
  \"{t.id}:{n}\"
end
state items = [{id: \"a\", v: 1}, {id: \"b\", v: 10}]
";

#[test]
fn a_keyed_map_follows_items_when_the_list_reorders() {
    for key in ["key: \"id\"", "key: fn(t) t.id end"] {
        let src = format!(
            "{ITEMS}print(map(items, tally, {key}))\n\
             items = [{{id: \"b\", v: 10}}, {{id: \"c\", v: 100}}, {{id: \"a\", v: 1}}]"
        );
        let out = run_n(&src, 3);
        assert_eq!(
            out,
            [r#"["a:1", "b:10"]"#, r#"["b:20", "c:100", "a:2"]"#, r#"["b:30", "c:200", "a:3"]"#],
            "{key}"
        );
    }
    // Unkeyed, the slots stay with the positions.
    let src = format!(
        "{ITEMS}print(map(items, tally))\nitems = [{{id: \"b\", v: 10}}, {{id: \"a\", v: 1}}]"
    );
    assert_eq!(run_n(&src, 2), [r#"["a:1", "b:10"]"#, r#"["b:11", "a:11"]"#]);
}

#[test]
fn both_key_forms_and_a_keyed_for_build_the_same_slots() {
    let shapes: Vec<HashSet<Vec<PathPart>>> = [
        "let r = map(items, tally, key: \"id\")",
        "let r = map(items, tally, key: fn(t) t.id end)",
        "let r = forEach(items, tally, key: \"id\")",
    ]
    .iter()
    .map(|call| {
        let (env, sid) = run_once(&format!("{ITEMS}{call}"));
        paths_of(&env, sid, "tally/n")
    })
    .collect();
    assert_eq!(shapes[0].len(), 2);
    assert_eq!(shapes[0], shapes[1]);
    // `forEach` and a `for … key` build the same element parts under a
    // different call part (the builtin's call site, or none for a loop at
    // top level), so compare the element parts.
    let (env, sid) = run_once(&format!("{ITEMS}let r = for t in items key t.id do tally(t) end"));
    let keys = |s: &HashSet<Vec<PathPart>>| -> HashSet<PathPart> {
        s.iter()
            .map(|p| *p.iter().find(|q| matches!(q, PathPart::Key(_))).unwrap())
            .collect()
    };
    assert_eq!(keys(&shapes[0]), keys(&shapes[2]));
    assert_eq!(keys(&shapes[0]), keys(&paths_of(&env, sid, "tally/n")));
}

#[test]
fn key_reads_a_field_of_a_record_or_a_map() {
    let out = run_n(
        "fn f(t)\n  state n = 0\n  n += 1\n  n\nend\n\
         let m1 = Map([[\"id\", 1]])\nlet m2 = Map([[\"id\", 2]])\n\
         print(map([m1, m2], f, key: \"id\"), map([{id: 1}, {id: 2}], f, key: \"id\"))",
        2,
    );
    assert_eq!(out, ["[1, 1] [1, 1]", "[2, 2] [2, 2]"]);
}

#[test]
fn key_errors() {
    let err = run_err("print(map([{id: 1}, {name: 2}], fn(x) x end, key: \"id\"))");
    assert!(err.contains("map(): element 1 has no field 'id'"), "{err}");
    for bad in ["3", "nil", "[1]"] {
        let err = run_err(&format!("print(map([1], fn(x) x end, key: {bad}))"));
        assert!(
            err.contains("map(): `key:` must be a field name (a string) or a function of the element"),
            "{bad}: {err}"
        );
    }
    let err = run_err("print(map([1, 2], fn(x) x end, key: \"id\"))");
    assert!(err.contains("reads a field, but element 0 is a"), "{err}");
    let err = run_err("print(forEach([1], fn(x) x end, key: fn(x) fn() x end end))");
    assert!(err.contains("as a forEach() key"), "{err}");
}

#[test]
fn duplicate_keys_fail_before_any_callback_runs() {
    let err = run_err(
        "state var calls = 0\n\
         print(map([{id: 1}, {id: 2}, {id: 1}], fn(x) set calls += 1 end, key: \"id\"))",
    );
    assert!(err.contains("Duplicate map key 1"), "{err}");
    let out = {
        let mut env = Env::new();
        let pid = env
            .load_program(
                "state var calls = 0\n\
                 fn go()\n  map([{id: 1}, {id: 1}], fn(x) set calls += 1 end, key: \"id\")\nend\n\
                 fn count_calls()\n  get calls\nend",
            )
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert!(env.call_function(sid, "go", &[]).is_err());
        env.call_function(sid, "count_calls", &[]).unwrap()
    };
    assert_eq!(out, petal::value::Value::Int(0));
}

#[test]
fn a_key_function_may_not_declare_state() {
    let err = run_err(&seen_then("print(map([1, 2], fn(x) x end, key: seen))"));
    assert!(err.starts_with("`state n` cannot be used inside a callback of `map`"), "{err}");
}

#[test]
fn stable_id_follows_items_under_key() {
    let src = "\
state items = [{id: 1}, {id: 2}]
let ids = map(items, fn(t) stable_id() end, key: \"id\")
print(ids[0], ids[1])
items = [{id: 2}, {id: 1}]
";
    let out = run_n(src, 2);
    let first: Vec<&str> = out[0].split(' ').collect();
    let second: Vec<&str> = out[1].split(' ').collect();
    assert_eq!((first[0], first[1]), (second[1], second[0]), "{out:?}");
}

// ── Still errors ─────────────────────────────────────────────────

#[test]
fn map_inside_a_callback_without_an_element_part_still_errors() {
    for (src, builtin) in [
        ("print(filter([1, 2], fn(x) len(map([x], seen)) > 0 end))", "filter"),
        ("print(sort([2, 1], fn(a, b) map([a], seen)[0] - b end))", "sort"),
        ("print(reduce([1, 2], 0, fn(acc, x) acc + map([x], seen)[0] end))", "reduce"),
    ] {
        let err = run_err(&seen_then(src));
        assert!(
            err.starts_with(&format!("`state n` cannot be used inside a callback of `{builtin}`")),
            "{src}: {err}"
        );
    }
}

// ── Hot reload ───────────────────────────────────────────────────

#[test]
fn hot_reload_keeps_per_element_slots() {
    let src = seen_then("print(map([1, 2], seen))");
    let mut env = Env::new();
    let pid = env.load_program(&src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    let edited = format!("{src}print(\"edited\")\n");
    env.reload_program(sid, &edited, None).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["[1, 1]", "[2, 2]", "[3, 3]", "edited"]);
}

// ── Host: state under parts the host chooses ─────────────────────

use petal::stack::StatePart;
use petal::value::Value;

/// A loaded, run stack of `seen_then(rest)`.
fn host(rest: &str) -> (Env, StackKey) {
    run_once(&seen_then(rest))
}

#[test]
fn host_keys_hash_as_petal_keys() {
    let (env, sid) = host("let a = for k in [\"x\", 7, true] key k do seen(k) end");
    let petal_keys: HashSet<PathPart> = paths_of(&env, sid, "seen/n")
        .iter()
        .map(|p| *p.iter().find(|q| matches!(q, PathPart::Key(_))).unwrap())
        .collect();
    let host_keys: HashSet<PathPart> = [StatePart::key("x"), StatePart::key(7), StatePart::key(true)]
        .iter()
        .map(StatePart::path_part)
        .collect();
    assert_eq!(petal_keys, host_keys);
    assert_eq!(StatePart::key("x"), StatePart::key(String::from("x")));
    assert_eq!(StatePart::key(7u8), StatePart::key(7i64));
    assert_ne!(StatePart::index(3), StatePart::key(3));
    // A Petal value the host holds hashes the same as the equal Rust value.
    let mut env = Env::new();
    let s = env.heap_mut().alloc_string("x".to_string());
    assert_eq!(StatePart::key_value(&env, Value::String(s)).unwrap(), StatePart::key("x"));
    assert_eq!(StatePart::key_value(&env, Value::Int(7)).unwrap(), StatePart::key(7));
}

#[test]
fn key_value_refuses_what_a_keyed_loop_refuses() {
    let (mut env, sid) = host("fn f(x) x end\nfn get_f() f end");
    let err = StatePart::key_value(&env, Value::Float(f64::NAN)).unwrap_err();
    assert!(err.contains("Cannot use NaN as a state key"), "{err}");
    let f = env.call_function(sid, "get_f", &[]).unwrap();
    let err = StatePart::key_value(&env, f).unwrap_err();
    assert!(err.contains("Cannot use a function as a state key"), "{err}");
}

#[test]
fn call_function_at_keeps_a_set_of_slots_per_key_across_calls_and_runs() {
    let (mut env, sid) = host("");
    let (a, b) = ([StatePart::key("row:1")], [StatePart::key("row:2")]);
    let call = |env: &mut Env, parts: &[StatePart]| env.call_function_at(sid, "seen", &[Value::Nil], parts).unwrap();
    assert_eq!(call(&mut env, &a), Value::Int(1));
    assert_eq!(call(&mut env, &a), Value::Int(2));
    assert_eq!(call(&mut env, &b), Value::Int(1));
    // A run in between does not sweep them: the host manages them.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(call(&mut env, &a), Value::Int(3));
    assert_eq!(call(&mut env, &b), Value::Int(2));
    // An index part and plain call_function are other places.
    assert_eq!(call(&mut env, &[StatePart::index(0)]), Value::Int(1));
    assert_eq!(env.call_function(sid, "seen", &[Value::Nil]).unwrap(), Value::Int(1));

    let under = env.state_under(sid, "seen", &a);
    assert_eq!(under.len(), 1, "{under:?}");
    assert_eq!(under[0].1, Value::Int(3));

    // A deleted row: its slots go now.
    assert_eq!(env.clear_state_under(sid, "seen", &a), 1);
    assert!(env.state_under(sid, "seen", &a).is_empty());
    assert_eq!(call(&mut env, &a), Value::Int(1));

    // Only row 2 is live: row 1 and the index-0 row go.
    let dropped = env.keep_state_under(sid, "seen", &[b.to_vec()]);
    assert_eq!(dropped, 3, "row 1, index 0 and the plain call's slot");
    assert!(env.state_under(sid, "seen", &a).is_empty());
    assert_eq!(env.state_under(sid, "seen", &b)[0].1, Value::Int(2));
}

#[test]
fn call_closure_at_runs_a_held_callback_under_a_named_scope() {
    let mut env = Env::new();
    let pid = env
        .load_program("fn make()\n  fn(x)\n    state total = 0\n    total += x\n    total\n  end\nend")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let cb = env.call_function(sid, "make", &[]).unwrap();
    let at = |env: &mut Env, i: usize, x: i64| {
        env.call_closure_at(sid, "table:rows", cb, &[Value::Int(x)], &[StatePart::index(i)]).unwrap()
    };
    assert_eq!(at(&mut env, 0, 5), Value::Int(5));
    assert_eq!(at(&mut env, 1, 7), Value::Int(7));
    assert_eq!(at(&mut env, 0, 5), Value::Int(10));
    assert_eq!(env.state_under(sid, "table:rows", &[]).len(), 2);
}

#[test]
fn a_keyed_host_pass_refuses_a_repeated_key() {
    let (mut env, sid) = host("");
    let mut pass = env.begin_keyed(sid);
    pass.call_at("seen", &[Value::Nil], &[StatePart::key("row:1")]).unwrap();
    pass.call_at("seen", &[Value::Nil], &[StatePart::key("row:2")]).unwrap();
    let err = pass.call_at("seen", &[Value::Nil], &[StatePart::key("row:1")]).unwrap_err();
    assert!(err.contains("Duplicate host key \"row:1\""), "{err}");
}
