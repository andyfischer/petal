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
    let pid = env.load_program("state count = 0\ncount = count + 1").unwrap();
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
fn explicit_key_state_survives_reset_and_rerun() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "for v in [\"a\", \"b\"] do\n  state(v) hits = 0\n  hits += 1\nend",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();

    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();

    // After two runs, hits["a"] == 2 and hits["b"] == 2.
    // Both keys were visited each run so neither is GC'd.
    let map = env.get_all_state(sid).unwrap();
    assert_eq!(map.len(), 2, "expected 2 entries, got {}: {:?}", map.len(), map);
    for (_k, v) in map.iter() {
        assert_eq!(format!("{:?}", v), "Int(2)");
    }
}

#[test]
fn untouched_state_keys_are_swept_after_run() {
    let mut env = Env::new();

    // First program touches both "a" and "b".
    let pid1 = env
        .load_program(
            "for v in [\"a\", \"b\"] do\n  state(v) hits = 0\n  hits += 1\nend",
        )
        .unwrap();
    let sid = env.create_stack(pid1).unwrap();
    env.run(sid).unwrap();

    // Now load a new program (simulating hot reload) that only touches "a".
    // We swap the program but keep the state map; on the next run, "b"
    // should be swept.
    let pid2 = env
        .load_program(
            "for v in [\"a\"] do\n  state(v) hits = 0\n  hits += 1\nend",
        )
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
