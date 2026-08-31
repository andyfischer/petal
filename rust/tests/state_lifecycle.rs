// Lifecycle tests for the persistent state map across multiple top-level
// runs of the same Stack — the petal-sdl / petal-fps "every frame is a
// new run, state survives" model.
//
// These cover behavior that pure .ptl tests can't exercise from a single
// process invocation:
//   - State retention across `run` → `reset_stack` → `run` cycles
//   - Lazy init: the init RHS only fires the first time the runtime key is
//     visited, even across multiple runs
//   - GC: state entries whose source-level declaration was not visited on
//     a run are dropped at run-completion time

use petal::env::Env;
use petal::program::StateKey;

#[test]
fn state_survives_reset_and_rerun() {
    let mut env = Env::new();
    let pid = env
        .load_program("state count = 0\ncount = count + 1")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    let count_key = StateKey(petal::compiler::Compiler::hash_state_name("count"));
    let val = env.get_state(sid, count_key).unwrap();
    // Three runs, each adds 1 → final value is 3 (not 1, which is what
    // we'd see if the init expression re-ran each time).
    assert_eq!(format!("{:?}", val), "Int(3)");
}

#[test]
fn state_list_accumulated_in_loop_persists_across_runs() {
    // A `state` list grown by in-loop reassignment (`xs = append(xs, i)`)
    // must persist across runs, the same way `let` accumulators carry within
    // a run. Under value semantics this only works if the in-loop reassignment
    // emits a StateWrite to the base slot (see compiler/phi.rs) — otherwise the
    // appended value lives only in loop registers and is lost when the run ends.
    //
    // Call-path keying does not change this. The *declaration* is what the path
    // keys, and this one is reached at the root, on the empty path
    // (docs/dev/state-call-paths.md §2.3: top-level `state` is untouched). The
    // writes sit one loop level below it and pop back to it
    // (`Term::path_pop`). What per-iteration keying looks like — a declaration
    // *inside* the loop — is the next test.
    let mut env = Env::new();
    let pid = env
        .load_program("state items = []\nfor i in range(0, 3) do\n  items = append(items, i)\nend")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    let items_key = StateKey(petal::compiler::Compiler::hash_state_name("items"));
    let list_len = |env: &Env| match env.get_state(sid, items_key).unwrap() {
        petal::value::Value::List(id) => env.heap().list_len(id),
        other => panic!("expected a list, got {:?}", other),
    };

    env.run(sid).unwrap();
    assert_eq!(list_len(&env), 3, "first run should build [0, 1, 2]");

    // Second run: init is a cache hit (not reset to []), so the loop appends
    // three more → length 6. Confirms the base slot, not a per-iteration slot,
    // received the writes.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        list_len(&env),
        6,
        "second run should accumulate onto the persisted list"
    );
    assert_eq!(
        env.snapshot_state(sid).unwrap().len(),
        1,
        "one declaration on the root path is one slot — the loop it is written \
         from adds no per-iteration strays"
    );
}

#[test]
fn state_declared_inside_a_loop_is_one_slot_per_iteration() {
    // The other half of the rule, and the companion to the test above: move the
    // declaration *into* the loop and the path that reaches it now carries the
    // iteration, so each iteration owns a slot and each accumulates on its own
    // across runs (§2.1). This is the keying `examples/console/particles.
    // ptl` always claimed to have.
    let mut env = Env::new();
    let pid = env
        .load_program("for i in range(0, 3) do\n  state seen = 0\n  seen += 1\nend")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();
    let after_one = env.get_all_state(sid).unwrap();
    assert_eq!(after_one.len(), 3, "three iterations, three slots");
    assert!(
        after_one.values().all(|v| format!("{v:?}") == "Int(1)"),
        "each iteration's own slot starts at 0, got {after_one:?}"
    );

    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    let after_two = env.get_all_state(sid).unwrap();
    assert_eq!(after_two.len(), 3, "the same three slots, not three more");
    assert!(
        after_two.values().all(|v| format!("{v:?}") == "Int(2)"),
        "each slot resumed where its own iteration left it, got {after_two:?}"
    );
}

#[test]
fn state_list_mutated_by_index_in_loop_persists_across_runs() {
    // A `state` list updated by in-loop index assignment (`grid[i] = grid[i] + 1`)
    // must persist across runs. Under value semantics the index assignment desugars
    // to a functional rebuild + rebind of `grid`; routing the rebind through the
    // same machinery as plain name assignment means the StateWrite to the base slot
    // fires, so the per-iteration writes survive when the run ends. Routing it
    // there also gives it the declaration's `path_pop`, so under call-path keying
    // it still addresses the root-path slot rather than the iteration's.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "state grid = [0, 0, 0]\nfor i in range(0, 3) do\n  grid[i] = grid[i] + 1\nend",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    let grid_key = StateKey(petal::compiler::Compiler::hash_state_name("grid"));
    let cell0 = |env: &Env| match env.get_state(sid, grid_key).unwrap() {
        petal::value::Value::List(id) => env.heap().get_list(id)[0],
        other => panic!("expected a list, got {:?}", other),
    };

    env.run(sid).unwrap();
    // First run: each cell incremented once → grid == [1, 1, 1].
    assert_eq!(
        format!("{:?}", cell0(&env)),
        "Int(1)",
        "first run should set cell 0 to 1"
    );

    // Second run: init is a cache hit (not reset to [0,0,0]), so cells accumulate
    // → grid == [2, 2, 2]. Confirms the index-assign rebind wrote to the base slot.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        format!("{:?}", cell0(&env)),
        "Int(2)",
        "second run should accumulate onto the persisted list"
    );
}

#[test]
fn explicit_key_state_survives_reset_and_rerun() {
    let mut env = Env::new();
    let pid = env
        .load_program("for v in [\"a\", \"b\"] do\n  state(v) hits = 0\n  hits += 1\nend")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    // After two runs, hits["a"] == 2 and hits["b"] == 2.
    // Both keys were visited each run so neither is GC'd.
    let map = env.get_all_state(sid).unwrap();
    assert_eq!(
        map.len(),
        2,
        "expected 2 entries, got {}: {:?}",
        map.len(),
        map
    );
    for (_k, v) in map.iter() {
        assert_eq!(format!("{:?}", v), "Int(2)");
    }
}

#[test]
fn untouched_state_keys_are_swept_after_run() {
    let mut env = Env::new();

    // First program touches both "a" and "b".
    let pid1 = env
        .load_program("for v in [\"a\", \"b\"] do\n  state(v) hits = 0\n  hits += 1\nend")
        .unwrap();
    let sid = env.create_stack(pid1).unwrap();
    env.run(sid).unwrap();

    // Now load a new program (simulating hot reload) that only touches "a".
    // We swap the program but keep the state map; on the next run, "b"
    // should be swept.
    let pid2 = env
        .load_program("for v in [\"a\"] do\n  state(v) hits = 0\n  hits += 1\nend")
        .unwrap();
    // Reuse the same Stack with a fresh program: snapshot the state, build
    // a stack on the new program, restore the snapshot.
    let snapshot = env.snapshot_state(sid).unwrap();
    let sid2 = env.create_stack(pid2).unwrap();
    env.restore_state(sid2, snapshot);

    let before = env.get_all_state(sid2).unwrap().len();
    assert_eq!(before, 2, "expected 2 entries pre-sweep, got {}", before);

    env.run(sid2).unwrap();

    let after = env.get_all_state(sid2).unwrap().len();
    assert_eq!(
        after, 1,
        "expected 1 entry after GC sweep (only \"a\" was visited), got {}",
        after
    );
}

#[test]
fn a_callsite_that_stops_running_has_its_slot_swept() {
    // The sweep is keyed on the whole runtime key, so it reaches call paths as
    // well as loop indices: one declaration reached through two callsites is two
    // entries, and the one whose callsite is skipped this run is collected.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn widget()
  state n = 0
  n += 1
  n
end
state var both = true
widget()
if get both then
  widget()
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let widget_slots = |env: &Env| {
        let b = StateKey(petal::compiler::Compiler::hash_state_name("widget/n"));
        env.get_all_state(sid)
            .unwrap()
            .keys()
            .filter(|k| k.base == b)
            .count()
    };
    assert_eq!(widget_slots(&env), 2, "two callsites, two slots");

    // Turn the second callsite off; its slot is unvisited on the next run.
    env.set_state(
        sid,
        StateKey(petal::compiler::Compiler::hash_state_name("both")),
        petal::value::Value::Bool(false),
    );
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        widget_slots(&env),
        1,
        "the skipped callsite's path was not visited, so it is swept"
    );
}

#[test]
fn an_explicit_key_reached_by_a_new_path_keeps_its_value() {
    // The counterpart to the sweep: an explicit key is absolute, so visiting it
    // through a *different* call path on a later run finds the same slot rather
    // than allocating a second one — and nothing is left unvisited to collect.
    // This is the plant.ptl lineage contract, where the same leaf is reached one
    // recursion level deeper every frame.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "\
fn leaf(id)
  state(id) age = 0
  age += 1
  age
end
fn wrap(id)
  leaf(id)
end
state var deep = false
if get deep then
  print(wrap(\"x\"))
else
  print(leaf(\"x\"))
end
",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"]);

    // Same key, now reached one call deeper.
    env.set_state(
        sid,
        StateKey(petal::compiler::Compiler::hash_state_name("deep")),
        petal::value::Value::Bool(true),
    );
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        env.take_output(),
        ["2"],
        "the absolute key found its slot from the deeper path"
    );

    let b = StateKey(petal::compiler::Compiler::hash_state_name("leaf/age"));
    assert_eq!(
        env.get_all_state(sid)
            .unwrap()
            .keys()
            .filter(|k| k.base == b)
            .count(),
        1,
        "one key, one slot — the deeper path did not fork it"
    );
}

#[test]
fn deleted_top_level_state_is_swept_on_hot_reload() {
    let mut env = Env::new();

    // Old source has two state vars.
    let pid1 = env.load_program("state a = 1\nstate b = 2").unwrap();
    let sid = env.create_stack(pid1).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.get_all_state(sid).unwrap().len(), 2);

    // Hot reload: new source removes `b`. Snapshot, swap program, restore,
    // then run — the unvisited `b` should be GC'd.
    let snapshot = env.snapshot_state(sid).unwrap();
    let pid2 = env.load_program("state a = 1").unwrap();
    let sid2 = env.create_stack(pid2).unwrap();
    env.restore_state(sid2, snapshot);
    env.run(sid2).unwrap();

    assert_eq!(
        env.get_all_state(sid2).unwrap().len(),
        1,
        "expected only `a` to remain after GC; got {:?}",
        env.get_all_state(sid2).unwrap()
    );
}

#[test]
fn lazy_init_does_not_overwrite_existing_value_across_runs() {
    let mut env = Env::new();

    // Program with a state initializer that, if run on every frame, would
    // overwrite the value with 42. We mutate the value to 99 between runs
    // and verify the second run keeps 99 (the init didn't fire again).
    let pid = env.load_program("state x = 42").unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();

    // External mutation (simulating agent set_state).
    let key = StateKey(petal::compiler::Compiler::hash_state_name("x"));
    env.set_state(sid, key, petal::value::Value::Int(99));

    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    let val = env.get_state(sid, key).unwrap();
    assert_eq!(
        format!("{:?}", val),
        "Int(99)",
        "expected x = 99 (lazy init skipped), got {:?}",
        val
    );
}

// ---------------------------------------------------------------------------
// `state var` — the slot holds a cell (docs/var.md, Cells)
// ---------------------------------------------------------------------------

#[test]
fn state_var_persists_across_runs_without_a_state_write() {
    // A `state var`'s slot holds the *cell*, created once by the init block.
    // Every `set` writes through it, so persistence needs no `StateWrite` at
    // all — and works from inside a conditional, which is where the plain
    // `state` + `=` form has to emit one.
    let mut env = Env::new();
    let pid = env
        .load_program("state var count = 0\nif true then set count = count + 1 end")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    for _ in 0..3 {
        env.run(sid).unwrap();
        env.reset_stack(sid).unwrap();
    }

    let key = StateKey(petal::compiler::Compiler::hash_state_name("count"));
    let cell = match env.get_state(sid, key).unwrap() {
        petal::value::Value::Cell(id) => id,
        other => panic!("a `state var` slot holds a cell, got {:?}", other),
    };
    assert_eq!(format!("{:?}", env.heap().cell_read(cell)), "Int(3)");
}

#[test]
fn state_var_reads_and_writes_as_its_contents_over_json() {
    // The slot holds a cell, but every serialization surface must show the
    // *value*: a host inspecting `count` wants 2, and setting it must write
    // through the cell rather than replacing it — replacing would strand every
    // `CellRead` the script compiled against that slot.
    let mut env = Env::new();
    let pid = env
        .load_program("state var count = 0\nset count = count + 1")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    let state = env.get_state_json(pid, sid);
    assert_eq!(state.get("count"), Some(&serde_json::json!(2)));

    env.set_state_from_json(pid, sid, "count", &serde_json::json!(40))
        .unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    // 40 was written *through* the cell, so the next run incremented it.
    let state = env.get_state_json(pid, sid);
    assert_eq!(state.get("count"), Some(&serde_json::json!(41)));
}
