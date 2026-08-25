//! How `Env::get_state_json` names a slot now that a slot is a declaration
//! plus the call path that reached it (docs/dev/state-callsite-keying-plan.md
//! §3.6). One renderer covers every path shape: the parts root-to-leaf,
//! `/`-separated, variable name last.
//!
//! The contract these pin, in order of how much depends on it:
//!
//!   1. **A top-level slot still renders as its bare name.** Every host that
//!      inspects state by name — the debug protocol, web-canvas props↔state
//!      sync, `petal-ui-run` traces — reads top-level names, so this is the
//!      one shape that must not move.
//!   2. A pathed slot renders its path, readably where the program can name
//!      the callee.
//!   3. A pathed name can never be mistaken for a top-level one, which is what
//!      keeps `set_state_from_json`'s top-level-only matching sound.

use petal::env::Env;

/// Run `src` once and return its state map with keys sorted, as `k=v` pairs —
/// `get_state_json` returns a `serde_json::Map`, whose iteration order is not
/// meaningful.
fn state_pairs(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let json = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = json.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    pairs
}

#[test]
fn a_top_level_slot_keeps_its_bare_name() {
    assert_eq!(state_pairs("state top = 1\ntop = top + 1"), ["top=2"]);
}

#[test]
fn a_module_state_keeps_its_qualified_name() {
    // The module qualifier is part of the *declaration* name, not the path, so
    // it stays where it has always been: in front, with no path separator.
    let mut env = Env::new();
    env.register_module("counters", "state hits = 0\nhits = hits + 1\n");
    let pid = env
        .load_program("import counters\nstate own = 5\n")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    let json = env.get_state_json(pid, sid);
    let mut keys: Vec<&String> = json.keys().collect();
    keys.sort();
    assert_eq!(keys, ["counters::hits", "own"]);
}

#[test]
fn each_callsite_renders_as_its_own_path() {
    // Two calls to one function are two slots, and the rendering distinguishes
    // them: the callee's spelling, then `#1` and up for repeats of the same
    // spelling in one function (the compiler's own callsite ordinal).
    assert_eq!(
        state_pairs(
            "fn counter()\n  state count = 0\n  count = count + 1\n  count\nend\ncounter()\ncounter()"
        ),
        ["counter#1/count=1", "counter/count=1"]
    );
}

#[test]
fn a_path_reads_root_to_leaf() {
    // `b` calls `a` twice; the whole chain from the root shows up, outermost
    // callsite first, so a reader can trace how the slot was reached.
    assert_eq!(
        state_pairs("fn a()\n  state x = 1\n  x\nend\nfn b()\n  a()\n  a()\nend\nb()"),
        ["b/a#1/x=1", "b/a/x=1"]
    );
}

#[test]
fn a_loop_iteration_renders_as_a_bracketed_index() {
    // A widget function called in a `for` gets one slot per iteration; the
    // caller's Index part precedes the callee's Call part, which is the order
    // the frames were pushed in.
    assert_eq!(
        state_pairs(
            "fn w(n)\n  state seen = 0\n  seen = seen + n\n  seen\nend\nfor i in [1, 2, 3] do\n  w(i)\nend"
        ),
        ["[0]/w/seen=1", "[1]/w/seen=2", "[2]/w/seen=3"]
    );
}

#[test]
fn nested_loop_indices_are_separate_path_steps() {
    // The pre-path rendering packed loop indices into one suffix (`inner[0,1]`);
    // they are ordinary path parts now, one step each.
    assert_eq!(
        state_pairs(
            "for i in range(0, 2) do\n  for j in range(0, 2) do\n    state inner = i * 10 + j\n  end\nend"
        ),
        [
            "[0]/[0]/inner=0",
            "[0]/[1]/inner=1",
            "[1]/[0]/inner=10",
            "[1]/[1]/inner=11"
        ]
    );
}

#[test]
fn an_explicit_key_renders_as_k_hash_and_nothing_else() {
    // `state(expr)` is absolute (plan §2.2): the path is exactly the key, so
    // the rendering shows the key and no callsite — which is the point, since
    // two callsites reach the same slot.
    let pairs = state_pairs("fn leaf(id)\n  state(id) v = id\n  v\nend\nleaf(7)\nleaf(9)");
    assert_eq!(
        pairs.len(),
        2,
        "one slot per key, not per callsite: {pairs:?}"
    );
    for pair in &pairs {
        let (path, _) = pair.split_once('=').unwrap();
        let (part, name) = path.split_once('/').unwrap();
        assert_eq!(name, "v");
        assert!(
            part.starts_with('k') && part[1..].parse::<u64>().is_ok(),
            "an explicit key renders as k<hash>, got {part}"
        );
    }
}

#[test]
fn a_top_level_slot_survives_next_to_pathed_ones() {
    // The mixed case a host actually sees: its own top-level vars addressable
    // as always, with in-function slots alongside under pathed names.
    assert_eq!(
        state_pairs(
            "state total = 0\nfn bump()\n  state n = 0\n  n = n + 1\n  n\nend\ntotal = total + bump()"
        ),
        ["bump/n=1", "total=1"]
    );
}

#[test]
fn every_pathed_name_is_distinguishable_from_a_top_level_one() {
    // `set_state_from_json` matches top-level declaration names only. That is
    // sound because a path always contributes a `/`, and a declaration name
    // never contains one.
    let pairs = state_pairs(
        "state plain = 1\n\
         fn f()\n  state a = 1\n  a\nend\n\
         fn g(id)\n  state(id) b = id\n  b\nend\n\
         for i in range(0, 2) do\n  state c = i\n  f()\n  g(i)\nend",
    );
    for pair in &pairs {
        let (name, _) = pair.split_once('=').unwrap();
        assert_eq!(
            name.contains('/'),
            name != "plain",
            "exactly the pathed names carry a separator: {pair}"
        );
    }
}

#[test]
fn a_pathed_entry_is_skipped_by_a_state_map_restore() {
    // The documented not-addressable class: a snapshot round-trip applies the
    // top-level entries and silently drops the pathed ones, rather than
    // erroring on them or inventing a slot.
    let src =
        "state total = 0\nfn bump()\n  state n = 0\n  n = n + 1\n  n\nend\ntotal = total + bump()";
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let snapshot = env.get_state_json(pid, sid);
    assert_eq!(snapshot.len(), 2, "one top-level and one pathed entry");
    let applied = env.set_state_map_from_json(pid, sid, &snapshot);
    assert_eq!(applied, 1, "only the top-level entry is addressable");

    assert!(
        env.set_state_from_json(pid, sid, "bump/n", &serde_json::json!(9))
            .is_err(),
        "a pathed name is not a settable address"
    );
}
