// Call-path keyed `state` — the Phase 2 semantics of
// docs/dev/state-callsite-keying-plan.md.
//
// A slot is `(declaration id, path)`, where the path is the chain of callsites
// and loop iterations that reached the declaration. Two executions land in the
// same slot exactly when they arrive the same way. What that buys, and what it
// costs, is pinned here:
//
//   - each callsite of a helper holds its own value (§2.1);
//   - a widget called inside a `for` gets per-iteration slots, so the list's
//     positions are its keys — and reordering the list moves the values (§2.1);
//   - recursion is one slot per depth;
//   - `state(expr)` is **absolute**: it ignores the path entirely, which is
//     what makes lineage keying and shared button-repeat phases work (§2.2);
//   - top-level `state` is untouched, including the accumulator idiom whose
//     writes sit inside a loop (§2.3);
//   - a host call runs on a root path of its own (§2.5).
//
// Declaration *ids* — the other half of the key — are pinned in
// tests/state_decl_ids.rs.

use std::collections::{HashMap, HashSet};

use petal::compiler::Compiler;
use petal::env::Env;
use petal::program::{StateKey, TermOp};
use petal::stack::{PathPart, RuntimeStateKey};
use petal::value::Value;

/// Run `src` once and return its print output.
fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

/// Run `src` `times` times on one stack — the frame-loop model, where each run
/// reaches the same declarations by the same paths — and return all the output.
fn run_n(src: &str, times: usize) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for i in 0..times {
        if i > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
    }
    env.take_output()
}

fn base(name: &str) -> StateKey {
    StateKey(Compiler::hash_state_name(name))
}

/// Every live path for the declaration named `name`, as a set so ordering of
/// the state map never decides a test.
fn paths_of(env: &Env, sid: petal::stack::StackKey, name: &str) -> HashSet<Vec<PathPart>> {
    let b = base(name);
    env.snapshot_state(sid)
        .unwrap()
        .keys()
        .filter(|k| k.base == b)
        .map(|k| k.path.to_vec())
        .collect()
}

// ── Per-callsite isolation ───────────────────────────────────────

#[test]
fn two_callsites_of_one_helper_hold_independent_values() {
    // The headline change. Under the old name-keyed model this printed
    // 1, 2, 1, 2 — one shared counter, incremented four times.
    let out = run_n(
        "\
fn counter()
  state n = 0
  n += 1
  n
end
print(counter())
print(counter())
",
        2,
    );
    assert_eq!(out, ["1", "1", "2", "2"]);
}

#[test]
fn the_accessor_slot_idiom_no_longer_launders_one_slot() {
    // The negative test for the pattern the preludes used to reach for: a
    // single function wrapping the declaration so "there is exactly one
    // `state`". It no longer shares, because the two callers are two paths.
    // The migration is a top-level cell, checked in the next test.
    let out = run("\
fn slot(writing, v)
  state cell = 0
  if writing then
    cell = v
  end
  cell
end
slot(true, 42)
print(slot(false, 0))
");
    assert_eq!(out, ["0"], "the reader must not see the writer's slot");
}

#[test]
fn a_top_level_state_var_is_the_shared_cell() {
    // §2.4's migration idiom: one cell, one path, `get`/`set` at the seams.
    let out = run("\
state var shared = 0
fn put(v)
  set shared = v
end
fn look()
  get shared
end
put(42)
print(look())
");
    assert_eq!(out, ["42"]);
}

// ── Recursion ────────────────────────────────────────────────────

#[test]
fn recursion_gets_one_slot_per_depth_that_persists_across_runs() {
    // Each recursive call adds a path part, so the three depths are three
    // slots; a second run walks the same three and finds each where it left it.
    let out = run_n(
        "\
fn down(n)
  state hits = 0
  hits += 1
  if n > 0 then
    down(n - 1)
  end
  print(str(n) ++ \":\" ++ str(hits))
end
down(2)
",
        2,
    );
    assert_eq!(out, ["0:1", "1:1", "2:1", "0:2", "1:2", "2:2"]);
}

// ── Caller-loop keying ───────────────────────────────────────────

#[test]
fn a_widget_called_in_a_loop_gets_per_iteration_slots() {
    // React's positional list keying, for free: the loop iteration around the
    // callsite is part of the path, so each list position has its own counter.
    let out = run_n(
        "\
fn widget(label)
  state seen = 0
  seen += 1
  label ++ \":\" ++ str(seen)
end
for x in [\"a\", \"b\"] do
  print(widget(x))
end
",
        2,
    );
    assert_eq!(out, ["a:1", "b:1", "a:2", "b:2"]);
}

#[test]
fn reordering_the_list_moves_positional_slots_but_not_keyed_ones() {
    // The documented cost of positional keying, and its escape hatch. Both
    // widgets run over ["a","b"] and then over ["b","a"]: the unkeyed counter
    // follows the *position*, the `state(key)` one follows the *item*.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn positional(label)
  state seen = 0
  seen += 1
  seen
end
fn keyed(label)
  state(label) seen = 0
  seen += 1
  seen
end
state var order = [\"a\", \"b\"]
for x in get order do
  print(x ++ \" pos=\" ++ str(positional(x)) ++ \" key=\" ++ str(keyed(x)))
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["a pos=1 key=1", "b pos=1 key=1"]);

    // Swap the list in through the state cell, then run again.
    let order = base("order");
    let a = env.heap_mut().alloc_string("b".to_string());
    let b = env.heap_mut().alloc_string("a".to_string());
    let swapped = env
        .heap_mut()
        .alloc_list(vec![Value::String(a), Value::String(b)]);
    env.set_state(sid, order, Value::List(swapped));
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.take_output(),
        // Position 0 is now "b": it inherits the counter "a" left there.
        // The keyed counters travelled with their items.
        ["b pos=2 key=2", "a pos=2 key=2"]
    );
}

// ── Explicit keys are absolute ───────────────────────────────────

#[test]
fn one_explicit_key_is_one_slot_across_different_callsites() {
    // The plant.ptl lineage / nes.ptl `btn_repeat` contract: same key ⇒ same
    // slot, no matter who asks or how deep they are.
    let out = run("\
fn bump(id)
  state(id) n = 0
  n += 1
  n
end
fn indirect(id)
  bump(id)
end
print(bump(\"x\"))
print(bump(\"x\"))
print(indirect(\"x\"))
");
    assert_eq!(out, ["1", "2", "3"]);
}

#[test]
fn an_explicit_key_ignores_the_loop_it_is_reached_from() {
    let out = run("\
fn tally(id)
  state(id) n = 0
  n += 1
  n
end
for i in range(0, 3) do
  print(tally(\"shared\"))
end
");
    assert_eq!(out, ["1", "2", "3"]);
}

// ── Top-level state is untouched ─────────────────────────────────

#[test]
fn a_top_level_accumulator_written_inside_a_loop_keeps_one_slot() {
    // The write sits one loop level below its declaration, so it pops that
    // level and commits to the declaration's slot (`Term::path_pop`). Without
    // that, each iteration would write a slot of its own and the persisted
    // list would stay empty.
    let mut env = Env::new();
    let pid = env
        .load_program("state items = []\nfor i in range(0, 3) do\n  items = append(items, i)\nend")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let items = env.get_state(sid, base("items")).unwrap();
    let Value::List(id) = items else {
        panic!("expected a list, got {items:?}")
    };
    assert_eq!(env.heap().list_len(id), 3);
    assert_eq!(
        env.snapshot_state(sid).unwrap().len(),
        1,
        "one declaration, one slot — no per-iteration strays"
    );
}

#[test]
fn a_state_declared_inside_a_nested_loop_is_keyed_per_iteration_pair() {
    let out = run("\
for i in range(0, 2) do
  for j in range(0, 2) do
    state hits = 0
    hits += 1
    print(str(i) ++ str(j) ++ \"=\" ++ str(hits))
  end
end
");
    assert_eq!(out, ["00=1", "01=1", "10=1", "11=1"]);
}

// ── Host entry points (§2.5) ─────────────────────────────────────

#[test]
fn host_calls_share_a_root_path_with_each_other_but_not_with_the_program() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn tick()
  state n = 0
  n += 1
  n
end
print(tick())
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"], "the in-program callsite's slot");

    // Repeated host calls run on one root path derived from the name, so they
    // accumulate with each other — and start from 0, not from the program's 1.
    assert_eq!(env.call_function(sid, "tick", &[]).unwrap(), Value::Int(1));
    assert_eq!(env.call_function(sid, "tick", &[]).unwrap(), Value::Int(2));
}

// ── Garbage collection ───────────────────────────────────────────

#[test]
fn a_path_not_taken_this_run_is_swept() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn widget(x)
  state seen = 0
  seen += 1
  seen
end
state var items = [1, 2, 3]
for x in get items do
  widget(x)
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    // Three per-iteration slots plus the `items` cell.
    assert_eq!(env.snapshot_state(sid).unwrap().len(), 4);

    let shorter = env.heap_mut().alloc_list(vec![Value::Int(1)]);
    env.set_state(sid, base("items"), Value::List(shorter));
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.snapshot_state(sid).unwrap().len(),
        2,
        "the two iterations that no longer happen are swept"
    );
}

// ── Key shape ────────────────────────────────────────────────────

#[test]
fn the_runtime_key_carries_call_and_index_parts_and_a_key_part() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn widget(id)
  state seen = 0
  seen += 1
  state(id) tagged = 0
  tagged += 1
end
state plain = 0
plain += 1
for x in [\"a\"] do
  widget(x)
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let state = env.snapshot_state(sid).unwrap();
    let shape = |name: &str| -> Vec<Vec<PathPart>> {
        let b = base(name);
        state
            .keys()
            .filter(|k| k.base == b)
            .map(|k| k.path.to_vec())
            .collect()
    };

    assert_eq!(shape("plain"), [vec![]], "top level runs on the root path");

    // `widget/seen` is reached through one loop iteration and one call.
    let seen = shape("widget/seen");
    assert_eq!(seen.len(), 1);
    assert!(
        matches!(seen[0][..], [PathPart::Index(0), PathPart::Call(_)]),
        "expected [Index, Call], got {:?}",
        seen[0]
    );

    // The keyed declaration next to it is absolute: one `Key` part, nothing else.
    let tagged = shape("widget/tagged");
    assert_eq!(tagged.len(), 1);
    assert!(
        matches!(tagged[0][..], [PathPart::Key(_)]),
        "expected [Key], got {:?}",
        tagged[0]
    );
}

// ── Hot reload ───────────────────────────────────────────────────

#[test]
fn an_unrelated_edit_preserves_pathed_state() {
    let v1 = "\
fn counter()
  state n = 0
  n += 1
  n
end
print(counter())
";
    let v2 = "\
fn counter()
  state n = 0
  n += 1
  n
end
let unrelated = 1 + 1
print(counter())
";
    let mut env = Env::new();
    let pid = env.load_program(v1).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"]);

    let new_program = env.compile_program(pid, v2).unwrap();
    let result = env.transfer_state(sid, new_program).unwrap();
    assert_eq!(result.state_preserved, 1);
    assert_eq!(result.state_dropped, 0);

    env.run(sid).unwrap();
    assert_eq!(
        env.take_output(),
        ["2"],
        "the callsite id is name-derived, so the edit above it changed nothing"
    );
}

#[test]
fn inserting_an_earlier_call_to_the_same_callee_reshuffles_its_paths() {
    // The accepted loss of §3.1: a callsite's id is its ordinal among
    // identically-spelled callees in the function, so inserting an earlier
    // `counter()` renumbers the one below it. The base declaration survives
    // (nothing is dropped by the transfer) but the reader lands on a fresh
    // slot, and the orphan is swept after the run.
    let v1 = "\
fn counter()
  state n = 0
  n += 1
  n
end
print(counter())
";
    let v2 = "\
fn counter()
  state n = 0
  n += 1
  n
end
counter()
print(counter())
";
    let mut env = Env::new();
    let pid = env.load_program(v1).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1", "2"]);

    let new_program = env.compile_program(pid, v2).unwrap();
    let result = env.transfer_state(sid, new_program).unwrap();
    assert_eq!(
        (result.state_preserved, result.state_dropped),
        (1, 0),
        "the declaration still exists, so its entries are kept by the transfer"
    );

    env.run(sid).unwrap();
    assert_eq!(
        env.take_output(),
        ["1"],
        "the surviving print now reads the second callsite's fresh slot"
    );
    let paths: HashSet<Vec<PathPart>> = env
        .snapshot_state(sid)
        .unwrap()
        .keys()
        .map(|k| k.path.to_vec())
        .collect();
    assert_eq!(paths.len(), 2, "one slot per callsite, the orphan swept");
}

// ── `state var` cells are pathed too ─────────────────────────────

#[test]
fn a_state_var_in_a_function_holds_one_cell_per_callsite() {
    // A `state var`'s slot holds the *cell*; `set` writes through it, so no
    // `StateWrite` is ever emitted. Path keying applies to the slot that holds
    // the cell, so each callsite gets a cell of its own — and each keeps its
    // value across runs, because the init block only runs on a path's first
    // visit.
    let src = "\
fn c()
  state var n = 0
  set n = n + 1
  get n
end
print(c())
print(c())
";
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();

    // No `StateWrite`: `set` goes through the cell (plan §7's last bullet).
    let program = env.get_program(pid).unwrap();
    assert!(
        !program
            .terms
            .iter()
            .any(|t| matches!(t.op, TermOp::StateWrite)),
        "a `state var` needs no StateWrite to persist"
    );

    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1", "1"], "two callsites, two cells");
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.take_output(),
        ["2", "2"],
        "each cell persists on its own path"
    );

    // Two cells, both reached through one call part.
    let paths = paths_of(&env, sid, "c/n");
    assert_eq!(paths.len(), 2);
    for p in &paths {
        assert!(
            matches!(p[..], [PathPart::Call(_)]),
            "expected [Call], got {p:?}"
        );
        // and the slot really does hold a cell, not the value
        let key = RuntimeStateKey {
            base: base("c/n"),
            path: p.iter().copied().collect(),
        };
        let Some(Value::Cell(cell)) = env.snapshot_state(sid).unwrap().get(&key).copied() else {
            panic!("a `state var` slot holds a cell");
        };
        assert_eq!(env.heap().cell_read(cell), Value::Int(2));
    }
}

// ── Upgrade compatibility for top-level slots (§2.3) ─────────────

#[test]
fn a_top_level_slot_written_by_an_older_build_is_picked_up_verbatim() {
    // The upgrade contract: a top-level declaration's key is still
    // `hash(name)` on the empty path, byte-identical to the pre-path builds.
    // A snapshot taken then — modelled here by synthesizing the key by hand —
    // restores into this build and the program resumes from it rather than
    // re-running its init.
    let mut env = Env::new();
    let pid = env.load_program("state count = 0\ncount += 1").unwrap();
    let sid = env.create_stack(pid).unwrap();

    let mut old_snapshot: HashMap<RuntimeStateKey, Value> = HashMap::new();
    old_snapshot.insert(
        RuntimeStateKey {
            base: base("count"),
            path: Default::default(),
        },
        Value::Int(41),
    );
    env.restore_state(sid, old_snapshot);

    env.run(sid).unwrap();
    assert_eq!(
        env.get_state(sid, base("count")).unwrap(),
        Value::Int(42),
        "the persisted 41 was resumed, not re-initialized to 0"
    );
    assert_eq!(
        env.snapshot_state(sid).unwrap().len(),
        1,
        "and no second slot appeared beside it"
    );
}

// ── The path unwinds correctly on every exit ─────────────────────

#[test]
fn breaking_out_of_a_loop_leaves_no_stale_index_part() {
    // `break` jumps to the loop's exit label, which sits *before* `LoopPop` —
    // so the iteration's `Index` part is popped on the way out. A leftover part
    // would be invisible in the printed output and would quietly give every
    // later call in the same frame a different slot each run.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn widget()
  state n = 0
  n += 1
  n
end
for i in range(0, 3) do
  if i == 1 then
    break
  end
end
print(widget())
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"]);

    let paths = paths_of(&env, sid, "widget/n");
    assert_eq!(paths.len(), 1);
    assert!(
        matches!(paths.iter().next().unwrap()[..], [PathPart::Call(_)]),
        "the call after the loop runs on the root path, got {paths:?}"
    );

    // And a second run lands in the same slot rather than a fresh one.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["2"]);
    assert_eq!(paths_of(&env, sid, "widget/n").len(), 1);
}

#[test]
fn continuing_a_loop_keeps_one_index_part_per_iteration() {
    // `continue` jumps to the loop's `*Next`, which resets the innermost
    // `Index` part rather than pushing a second one — so a skipped iteration
    // costs one path, not one path per `continue`.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn widget()
  state n = 0
  n += 1
  n
end
for i in range(0, 4) do
  if i == 1 then
    continue
  end
  widget()
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let paths = paths_of(&env, sid, "widget/n");
    assert_eq!(paths.len(), 3, "iterations 0, 2 and 3 called the widget");
    let mut indices: Vec<usize> = paths
        .iter()
        .map(|p| match p[..] {
            [PathPart::Index(i), PathPart::Call(_)] => i,
            ref other => panic!("expected [Index, Call], got {other:?}"),
        })
        .collect();
    indices.sort_unstable();
    assert_eq!(indices, [0, 2, 3], "the skipped iteration left no slot");
}

#[test]
fn returning_from_inside_nested_loops_leaves_no_stale_index_part() {
    // The frame that returns is recycled, and `recycle` clears its path, so an
    // early `return` out of two loops cannot leak `Index` parts into the next
    // call's path.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn search()
  for i in range(0, 3) do
    for j in range(0, 3) do
      if j == 1 then
        return i + j
      end
    end
  end
  -1
end
fn widget()
  state n = 0
  n += 1
  n
end
search()
print(widget())
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"]);

    let paths = paths_of(&env, sid, "widget/n");
    assert_eq!(paths.len(), 1);
    assert!(
        matches!(paths.iter().next().unwrap()[..], [PathPart::Call(_)]),
        "expected [Call] after the early return, got {paths:?}"
    );
}

// ── Intrinsic higher-order calls (documented v1 gap) ─────────────

#[test]
fn a_closure_run_by_map_shares_one_slot_across_the_elements() {
    // §2.1 defines `Index` parts as coming from `for`/`while` only, so the
    // per-element calls `map` makes all carry the *`map` callsite's* part and
    // nothing per element: one slot, incremented three times. Two `map`
    // callsites are still two slots. Pinned so a future decision to key
    // intrinsic iteration per element is a deliberate, visible change.
    let out = run("\
fn w(x)
  state n = 0
  n += 1
  n
end
print(map([1, 2, 3], w))
print(map([1, 2, 3], w))
");
    assert_eq!(out, ["[1, 2, 3]", "[1, 2, 3]"]);
}

// ── Garbage collection, continued ────────────────────────────────

#[test]
fn a_visited_explicit_key_survives_a_sweep_that_takes_its_positional_neighbour() {
    // Two declarations reached through the same loop: one positional, one
    // keyed by the item. Shrinking the list drops the positional slot for the
    // iteration that no longer happens, while the keyed slot for an item that
    // is still visited stays — even though the *path* it was first reached by
    // is gone, because an explicit key is absolute (§2.2).
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
state var items = [\"a\", \"b\"]
for x in get items do
  state pos = 0
  pos += 1
  state(x) tagged = 0
  tagged += 1
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(paths_of(&env, sid, "pos").len(), 2);
    assert_eq!(paths_of(&env, sid, "tagged").len(), 2);

    // Drop "a": "b" moves from index 1 to index 0.
    let b = env.heap_mut().alloc_string("b".to_string());
    let shorter = env.heap_mut().alloc_list(vec![Value::String(b)]);
    env.set_state(sid, base("items"), Value::List(shorter));
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    assert_eq!(
        paths_of(&env, sid, "pos").len(),
        1,
        "the second iteration no longer happens, so its positional slot is swept"
    );
    let tagged = paths_of(&env, sid, "tagged");
    assert_eq!(
        tagged.len(),
        1,
        "\"a\" was not visited, so its keyed slot is swept too"
    );
    // "b"'s keyed slot was visited from a different position and survived with
    // its count: 1 from the first run plus 1 from the second.
    let key = RuntimeStateKey {
        base: base("tagged"),
        path: tagged.iter().next().unwrap().iter().copied().collect(),
    };
    assert_eq!(
        env.snapshot_state(sid).unwrap().get(&key).copied(),
        Some(Value::Int(2)),
        "the keyed slot followed its item across the reindex"
    );
}
