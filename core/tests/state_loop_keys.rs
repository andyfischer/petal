// `for x in xs key k do`: a loop whose iterations key their `state` by an
// item's identity instead of its position (docs/features/inline-state.md,
// Keyed loops; docs/dev/state-call-paths.md §2.1).
//
// The key replaces the loop's `Index` path part with `Key(hash(k))` and leaves
// the rest of the path alone, so it is relative to the loop at its call path:
//
//   - a reorder, an insert ahead or a removal keeps each item's slots;
//   - a removed item's slots are swept like any path not taken;
//   - two callers of one keyed loop still hold separate slots;
//   - an equal key twice in one run is an error, not silent sharing;
//   - a top-level accumulator written in a keyed loop still has one slot;
//   - a memoized row replays after a reorder, because its path did not move.

use std::collections::HashSet;

use petal::compiler::Compiler;
use petal::env::Env;
use petal::program::StateKey;
use petal::stack::{PathPart, StackKey};
use petal::value::Value;

fn base(name: &str) -> StateKey {
    StateKey(Compiler::hash_state_name(name))
}

/// A program whose rows count their own runs, over a top-level `order` the
/// tests rewrite between runs. `keyed` picks `for … key id` or a plain `for`.
fn rows(keyed: bool) -> String {
    let key = if keyed { " key id" } else { "" };
    format!(
        "\
fn row(id)
  state seen = 0
  seen += 1
  id ++ \"=\" ++ str(seen)
end
state var order = [\"a\", \"b\"]
for id in get order{key} do
  print(row(id))
end
"
    )
}

/// Load `src` and run it once.
fn start(src: &str) -> (Env, StackKey) {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    (env, sid)
}

/// Set the top-level `order` to `items` and run again.
fn rerun_with(env: &mut Env, sid: StackKey, items: &[&str]) {
    let vals = items
        .iter()
        .map(|s| Value::String(env.heap_mut().alloc_string(s.to_string())))
        .collect();
    let list = env.heap_mut().alloc_list(vals);
    env.set_state(sid, base("order"), Value::List(list));
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
}

/// Every live path for the declaration named `name`.
fn paths_of(env: &Env, sid: StackKey, name: &str) -> HashSet<Vec<PathPart>> {
    let b = base(name);
    env.snapshot_state(sid)
        .unwrap()
        .keys()
        .filter(|k| k.base == b)
        .map(|k| k.path.to_vec())
        .collect()
}

fn run_err(src: &str) -> String {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_err()
}

// ── Identity through a change to the list ───────────────────────────

#[test]
fn a_reorder_keeps_each_items_state() {
    let (mut env, sid) = start(&rows(true));
    assert_eq!(env.take_output(), ["a=1", "b=1"]);
    rerun_with(&mut env, sid, &["b", "a"]);
    assert_eq!(env.take_output(), ["b=2", "a=2"]);
}

#[test]
fn an_unkeyed_loop_still_keys_by_position() {
    // The behaviour `key` exists to change, pinned so the two stay distinct.
    let (mut env, sid) = start(&rows(false));
    assert_eq!(env.take_output(), ["a=1", "b=1"]);
    rerun_with(&mut env, sid, &["c", "a", "b"]);
    assert_eq!(env.take_output(), ["c=2", "a=2", "b=1"]);
}

#[test]
fn an_item_inserted_ahead_starts_fresh_and_the_rest_keep_theirs() {
    let (mut env, sid) = start(&rows(true));
    env.take_output();
    rerun_with(&mut env, sid, &["c", "a", "b"]);
    assert_eq!(env.take_output(), ["c=1", "a=2", "b=2"]);
}

#[test]
fn a_removed_items_state_is_swept() {
    let (mut env, sid) = start(&rows(true));
    env.take_output();
    assert_eq!(paths_of(&env, sid, "row/seen").len(), 2);
    rerun_with(&mut env, sid, &["b"]);
    assert_eq!(env.take_output(), ["b=2"]);
    assert_eq!(paths_of(&env, sid, "row/seen").len(), 1, "a's slot is gone");
    // And it does not come back: a returning item starts over.
    rerun_with(&mut env, sid, &["a", "b"]);
    assert_eq!(env.take_output(), ["a=1", "b=3"]);
}

#[test]
fn a_numeric_range_loop_takes_a_key_too() {
    let src = "\
state var base = 0
for i in range(0, 2) key get base + i do
  state seen = 0
  seen += 1
  print(str(get base + i) ++ \"=\" ++ str(seen))
end
";
    let (mut env, sid) = start(src);
    assert_eq!(env.take_output(), ["0=1", "1=1"]);
    env.set_state(sid, base("base"), Value::Int(1));
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1=2", "2=1"], "key 1 kept its slot from position 1");
}

// ── Path shape ───────────────────────────────────────────────────────

#[test]
fn the_key_takes_the_place_of_the_index_part() {
    let (env, sid) = start(&rows(true));
    let paths = paths_of(&env, sid, "row/seen");
    assert_eq!(paths.len(), 2);
    for p in &paths {
        assert!(
            matches!(p[..], [PathPart::Key(_), PathPart::Call(_)]),
            "expected [Key, Call], got {p:?}"
        );
    }
}

#[test]
fn a_keyed_loop_in_a_function_called_twice_keeps_two_sets_of_slots() {
    // The key is relative to the loop at its call path, which is what keeps
    // stateful code safe to duplicate — unlike the absolute `state(key)`.
    let src = "\
fn list(tag)
  for id in [\"a\"] key id do
    state seen = 0
    seen += 1
    print(tag ++ id ++ \"=\" ++ str(seen))
  end
end
list(\"x\")
list(\"y\")
";
    let (env, sid) = start(src);
    let mut env = env;
    assert_eq!(env.take_output(), ["xa=1", "ya=1"]);
    assert_eq!(paths_of(&env, sid, "list/seen").len(), 2);
}

#[test]
fn nested_keyed_loops_key_by_both_items() {
    let src = "\
for g in [\"p\", \"q\"] key g do
  for id in [1, 2] key id do
    state seen = 0
    seen += 1
  end
end
";
    let (env, sid) = start(src);
    let paths = paths_of(&env, sid, "seen");
    assert_eq!(paths.len(), 4);
    for p in &paths {
        assert!(matches!(p[..], [PathPart::Key(_), PathPart::Key(_)]), "got {p:?}");
    }
}

#[test]
fn a_top_level_accumulator_written_in_a_keyed_loop_keeps_one_slot() {
    // `path_pop` drops the loop's part whether it is an `Index` or a `Key`.
    let (env, sid) = start("state items = []\nfor x in [5, 6] key x do\n  items = append(items, x)\nend");
    let items = env.get_state(sid, base("items")).unwrap();
    let Value::List(id) = items else {
        panic!("expected a list, got {items:?}")
    };
    assert_eq!(env.heap().list_len(id), 2);
    assert_eq!(env.snapshot_state(sid).unwrap().len(), 1);
}

#[test]
fn break_and_continue_leave_no_stray_key_part() {
    let src = "\
for id in [1, 2, 3] key id do
  if id == 2 then
    continue
  end
  if id == 3 then
    break
  end
end
state after = 0
after += 1
";
    let (env, sid) = start(src);
    assert_eq!(paths_of(&env, sid, "after"), HashSet::from([vec![]]));
}

// ── Keys that cannot work ────────────────────────────────────────────

#[test]
fn an_equal_key_twice_in_one_run_is_an_error() {
    let err = run_err("for t in [{id: 1}, {id: 1}] key t.id do\n  state s = 0\nend\n");
    assert!(err.contains("Duplicate loop key 1"), "{err}");
}

#[test]
fn a_key_must_be_hashable_like_a_map_key() {
    let err = run_err("for t in [1] key fn() -> t do\nend\n");
    assert!(err.contains("Cannot use a function as a loop key"), "{err}");
}

#[test]
fn the_same_key_in_two_runs_of_the_loop_is_fine() {
    // Duplicates are counted per run of the loop, not per program run.
    let src = "\
for round in [1, 2] do
  for id in [\"a\"] key id do
    state seen = 0
    seen += 1
  end
end
";
    let (env, sid) = start(src);
    assert_eq!(paths_of(&env, sid, "seen").len(), 2, "one per round");
}

// ── Hosts and tools ──────────────────────────────────────────────────

#[test]
fn a_state_dump_names_a_keyed_slot_by_its_key_value() {
    let mut env = Env::new();
    let pid = env.load_program(&rows(true)).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let json = env.get_state_json(pid, sid);
    let mut keys: Vec<&str> = json.keys().map(|k| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["[key=\"a\"]/row/seen", "[key=\"b\"]/row/seen", "order"]);
}

#[test]
fn a_keyed_slot_survives_an_unrelated_edit() {
    let v1 = rows(true);
    let v2 = format!("let unrelated = 1\n{v1}");
    let mut env = Env::new();
    let pid = env.load_program(&v1).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output();
    let new_program = env.compile_program(pid, &v2).unwrap();
    let result = env.transfer_state(sid, new_program).unwrap();
    assert_eq!(result.state_dropped, 0);
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["a=2", "b=2"]);
}

#[test]
fn a_memoized_row_replays_after_a_reorder() {
    // The payoff: a row's scope is addressed by its path, and a keyed row's
    // path does not move when the list does. The row is big enough to be
    // recorded and does nothing a replay cannot reproduce.
    let src = |key: &str| {
        format!(
            "\
fn row(id)
  state open = false
  let acc = 0
  for k in range(0, 40) do acc = acc + k end
  if open then acc else id end
end
state var order = [\"a\", \"b\", \"c\"]
let out = for id in get order{key} do
  row(id)
end
"
        )
    };
    let misses_after_reorder = |key: &str| {
        let mut env = Env::new();
        env.set_static_cache(false);
        let pid = env.load_program(&src(key)).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        rerun_with(&mut env, sid, &["a", "b", "c"]);
        let before = env.memo_stats(sid).unwrap();
        assert!(before.hits >= 3, "an unchanged run replays every row: {before:?}");
        rerun_with(&mut env, sid, &["c", "a", "b"]);
        let after = env.memo_stats(sid).unwrap();
        (after.hits - before.hits, after.misses - before.misses)
    };
    let (hits, misses) = misses_after_reorder(" key id");
    assert_eq!((hits, misses), (3, 0), "keyed rows replay where they moved to");
    let (_, misses) = misses_after_reorder("");
    assert!(misses > 0, "positional rows see a different argument at each slot");
}
