// Declaration ids for `state` (Compiler::state_key_for).
//
// A declaration's slot key is derived from its *name path* — module ‖
// enclosing-function chain ‖ variable name ‖ shadow ordinal — never from its
// position, so it survives the edits hot reload has to tolerate. Two rules
// are pinned here:
//
//   - every declaration site owns its own slot (before this, two functions
//     that happened to declare the same state name shared one, silently);
//   - top-level declarations hash exactly the string they always did, so
//     persisted state carries across this change.
//
// Under call-path keying (docs/dev/state-call-paths.md), a declaration's
// *slot* is that id under the call path that reached it — so the way to watch
// one declaration accumulate is to reach it the same way twice, which is what
// `run_twice` does (the frame-loop model: same callsites, same paths, next
// run). Per-callsite splitting itself is pinned in tests/state_call_paths.rs.

use petal::compiler::Compiler;
use petal::env::Env;
use petal::program::StateKey;

/// Run `src` once and return its print output.
fn run(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

/// Run `src` twice on one stack and return both runs' print output. Every
/// callsite is reached by the same path on both runs, so a declaration that
/// owns its own slot shows its second value on the second run — and one that
/// wrongly shares a slot with another declaration shows *that* one's.
fn run_twice(src: &str) -> Vec<String> {
    let mut env = Env::new();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

fn key(name: &str) -> StateKey {
    StateKey(Compiler::hash_state_name(name))
}

// ── Distinct declarations get distinct slots ─────────────────────

#[test]
fn same_state_name_in_two_functions_does_not_collide() {
    // The historical bug: both `count`s hashed to `hash("count")`, so `b`
    // resumed `a`'s counter and the printed sequence was 1, 2, 3, 4.
    let out = run_twice(
        "\
fn a()
  state count = 0
  count += 1
  count
end
fn b()
  state count = 100
  count += 1
  count
end
print(a())
print(b())
",
    );
    assert_eq!(out, ["1", "101", "2", "102"]);
}

#[test]
fn a_nested_fn_does_not_collide_with_its_parent() {
    let out = run_twice(
        "\
fn outer()
  fn inner()
    state n = 100
    n += 1
    n
  end
  state n = 0
  n += 1
  print(n)
  print(inner())
end
outer()
",
    );
    assert_eq!(out, ["1", "101", "2", "102"]);
}

#[test]
fn two_declarations_of_one_name_in_one_function_do_not_collide() {
    // The shadow ordinal: the `else` branch's `n` is a different declaration
    // from the `then` branch's, even though both spell the same name in the
    // same function.
    let out = run_twice(
        "\
fn f(which)
  if which then
    state n = 0
    n += 1
    n
  else
    state n = 50
    n += 1
    n
  end
end
print(f(true))
print(f(false))
",
    );
    assert_eq!(out, ["1", "51", "2", "52"]);
}

#[test]
fn lambdas_bound_to_different_names_do_not_collide() {
    // A bound lambda contributes its binding name to the chain.
    let out = run_twice(
        "\
let g = fn(x)
  state c = 0
  c += x
  c
end
let h = fn(x)
  state c = 0
  c += x
  c
end
print(g(1))
print(h(10))
",
    );
    assert_eq!(out, ["1", "10", "2", "20"]);
}

#[test]
fn unbound_lambdas_do_not_collide_with_each_other() {
    // No binding name to borrow, so each lambda takes an ordinal among the
    // unnamed lambdas of its enclosing function.
    let out = run("\
fn apply(f)
  f(1)
end
print(apply(fn(x)
  state c = 0
  c += x
  c
end))
print(apply(fn(x)
  state c = 900
  c += x
  c
end))
");
    assert_eq!(out, ["1", "901"]);
}

#[test]
fn a_module_function_does_not_collide_with_an_entry_function() {
    let mut env = Env::new();
    env.register_module(
        "m",
        "export fn tick()\n  state count = 100\n  count += 1\n  count\nend",
    );
    let pid = env
        .load_program(
            "import m\nfn tick()\n  state count = 0\n  count += 1\n  count\nend\nprint(tick())\nprint(m.tick())",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1", "101", "2", "102"]);
}

// ── Key shape ────────────────────────────────────────────────────

#[test]
fn top_level_keys_hash_the_bare_name() {
    // The compatibility guarantee: an entry-file top-level declaration still
    // hashes its bare name, so state persisted by an older build is found.
    let mut env = Env::new();
    let pid = env.load_program("state count = 0\ncount += 7").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        format!("{:?}", env.get_state(sid, key("count")).unwrap()),
        "Int(7)"
    );
}

#[test]
fn an_in_function_key_is_not_the_bare_name_hash() {
    let mut env = Env::new();
    let pid = env
        .load_program("fn f()\n  state count = 0\n  count += 7\nend\nf()")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    // The declaration ran (one slot exists) but it is keyed by "f/count",
    // not by "count".
    assert_eq!(env.snapshot_state(sid).unwrap().len(), 1);
    assert!(
        env.get_state(sid, key("count")).is_none(),
        "an in-function declaration must not claim the top-level slot"
    );
}

#[test]
fn an_in_function_declaration_leaves_the_top_level_key_alone() {
    // Ordinals are per name path, so a function-scoped `count` compiled first
    // does not push the top-level `count` onto a `#1` key.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "fn f()\n  state count = 0\n  count += 1\nend\nf()\nstate count = 0\ncount += 7",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(
        format!("{:?}", env.get_state(sid, key("count")).unwrap()),
        "Int(7)"
    );
}

// ── Unchanged behavior ───────────────────────────────────────────

#[test]
fn one_declaration_gets_one_slot_per_callsite() {
    // The Phase 2 flip: three callsites of one declaration, three slots. Before
    // it, this printed 1, 2, 3.
    let out = run("\
fn bump()
  state count = 0
  count += 1
  count
end
print(bump())
print(bump())
print(bump())
");
    assert_eq!(out, ["1", "1", "1"]);
}

#[test]
fn an_explicit_key_still_slots_by_its_value() {
    // An explicit key is absolute — it ignores the call path — so the two
    // callsites of `cell` per id share a slot even though their paths differ.
    let out = run("\
fn cell(id, amount)
  state(id) total = 0
  total += amount
  total
end
print(cell(\"a\", 1))
print(cell(\"b\", 10))
print(cell(\"a\", 1))
print(cell(\"b\", 10))
");
    assert_eq!(out, ["1", "10", "2", "20"]);
}
