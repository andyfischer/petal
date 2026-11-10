//! `freeze(v)` / `frozen(v)`: the frozen (permanent) heap generation, end to
//! end through scripts.
//!
//! `Heap::freeze`'s own invariants (the marker skips a frozen object, the
//! sweep keeps it, the in-place operations copy) are unit-tested at the end
//! of `src/heap.rs`. This file holds what a script sees:
//!
//! * **value semantics survive freezing** — an update of a frozen value
//!   yields a new value and leaves the frozen one as it was, on every path
//!   the compiler would otherwise have planned in place (a fresh local, a
//!   `var` cell, a function's last use), with the optimizer on and off;
//! * **frozen data outlives collections** that no longer trace it, including
//!   data nothing roots;
//! * **what cannot be frozen** is refused, and nothing is half-frozen;
//! * **a fork and a restore carry the frozen set**.

use petal::env::Env;
use petal::policy::RunPolicy;

/// Run `code` once under `policy`: what it printed, or the error.
fn run_with(code: &str, policy: RunPolicy) -> Result<String, String> {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(code)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

/// Run `code` with the optimizer on and off; both must print `expected`.
fn expect_both(code: &str, expected: &str) {
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        let out = run_with(code, policy).unwrap_or_else(|e| panic!("run failed: {e}\n{code}"));
        assert_eq!(out, expected, "policy {policy:?}\n{code}");
    }
}

#[test]
fn freeze_returns_its_argument_and_frozen_reports_it() {
    expect_both(
        "let t = [1, 2, 3]\n\
         print(frozen(t))\n\
         let u = freeze(t)\n\
         print(frozen(t), frozen(u), u == t)\n\
         print(frozen(1), frozen(nil), frozen(\"never frozen: a fresh string \" ++ str(1)))\n\
         print(freeze(7))\n",
        "false\ntrue true true\nfalse false false\n7",
    );
}

/// Freezing reaches everything the value references.
#[test]
fn freezing_is_transitive() {
    expect_both(
        "import vec2\n\
         let inner = [1, 2]\n\
         let rec = { name: \"tower\", parts: inner, pos: vec3(1, 2, 3) }\n\
         let table = [rec, rec]\n\
         freeze(table)\n\
         print(frozen(table), frozen(rec), frozen(inner), frozen(rec.name), frozen(rec.pos))\n",
        "true true true true true",
    );
}

/// `frozen` on an enum variant asks about the variant, not about the payload
/// list every payload-free variant shares: freezing a table that holds
/// `Kind.park` must not make `Kind.lake` read as frozen.
#[test]
fn a_variant_is_frozen_only_if_it_was_frozen() {
    expect_both(
        "enum Kind\n  park,\n  lake,\n  tower(h),\nend\n\
         let t = freeze([Kind.park, Kind.tower(3)])\n\
         print(frozen(t[0]), frozen(t[1]), frozen(Kind.lake), frozen(Kind.tower(3)))\n\
         let l = freeze(Kind.lake)\n\
         print(frozen(l), l == Kind.lake)\n",
        "true true false false\ntrue true",
    );
}

/// The escape analysis plans `append` / `drop_last` / a field update on a fresh
/// local in place. On a frozen value each must copy instead: the frozen
/// value is unchanged and the result is a new, unfrozen value.
#[test]
fn updating_a_frozen_local_copies() {
    expect_both(
        "let a = [1, 2, 3]\n\
         let keep = freeze(a)\n\
         let b = append(a, 4)\n\
         let c = drop_last(a)\n\
         print(keep, a, b, c)\n\
         print(frozen(a), frozen(b), frozen(c))\n",
        "[1, 2, 3] [1, 2, 3] [1, 2, 3, 4] [1, 2]\ntrue false false",
    );
    expect_both(
        "let r = freeze({ x: 1, y: 2 })\n\
         let s = { ...r, x: 5 }\n\
         print(r, s, frozen(r), frozen(s))\n",
        "{ x: 1, y: 2 } { x: 5, y: 2 } true false",
    );
}

/// A `var` cell owns its container and writes through it in place
/// (`src/backend/bytecode/cells.rs`). A frozen container in a cell is copied
/// by the first write; whoever else holds the frozen value still sees it.
#[test]
fn writing_through_a_cell_that_holds_a_frozen_value_copies() {
    expect_both(
        "let base = freeze([1, 2, 3])\n\
         var xs = base\n\
         set xs[0] = 9\n\
         set xs = append(xs, 4)\n\
         print(base, xs, frozen(base), frozen(xs))\n\
         var r = freeze({ hp: 3 })\n\
         let before = r\n\
         set r.hp = 2\n\
         print(before, r, frozen(before), frozen(r))\n",
        "[1, 2, 3] [9, 2, 3, 4] true false\n{ hp: 3 } { hp: 2 } true false",
    );
}

/// A cell that *owns* its container (built in place by the function that
/// declared it) and is then frozen: the writes after the freeze must land in
/// a copy, or a frozen list would come to hold unfrozen objects the
/// collector no longer reaches through it.
///
/// Two things keep that from happening. Statically, `freeze` has no effect
/// property (`builtins/effects.rs`), so passing a value to it ends the
/// value's unique ownership and the compiler stops planning writes to it in
/// place; in every script shape tried while porting this, that alone was
/// enough. At run time, the heap's in-place operations copy when their
/// target is frozen, whatever the compiler planned; `src/heap.rs`'s unit
/// tests hold that half.
#[test]
fn writes_after_freezing_an_owned_cell_copy() {
    expect_both(
        "fn build()\n\
           var xs = [[0]]\n\
           freeze(xs)\n\
           set xs = append(xs, [1, 2])\n\
           set xs[0] = [5]\n\
           var r = { tags: [1] }\n\
           freeze(r)\n\
           set r.tags = [2, 3]\n\
           var a = set_at(f64_array(2), 0, 1.5)\n\
           freeze(a)\n\
           set a[1] = 2.5\n\
           print(frozen(xs), frozen(r), frozen(a))\n\
           [xs, r, a]\n\
         end\n\
         let kept = build()\n\
         for i in range(0, 20000) do\n\
           let junk = [i, \"junk \" ++ str(i)]\n\
         end\n\
         print(kept)\n",
        "false false false\n[[[5], [1, 2]], { tags: [2, 3] }, [1.5, 2.5]]",
    );
}

/// A loop that grows a list seeded from a frozen one: the first append
/// copies, the rest are in place on the copy, and the seed never changes.
#[test]
fn a_loop_growing_from_a_frozen_seed_leaves_the_seed_alone() {
    expect_both(
        "let seed = freeze([0])\n\
         fn grow(xs, n)\n\
           var out = xs\n\
           for i in range(1, n) do\n\
             set out = append(out, i)\n\
           end\n\
           out\n\
         end\n\
         let big = grow(seed, 500)\n\
         print(len(seed), len(big), frozen(seed), frozen(big))\n",
        "1 500 true false",
    );
}

/// f64 arrays: `set_at` and `swap` are the in-place candidates.
#[test]
fn a_frozen_f64_array_is_never_written_in_place() {
    expect_both(
        "let a = f64_array(3)\n\
         let a1 = set_at(a, 0, 1.5)\n\
         freeze(a1)\n\
         let b = set_at(a1, 1, 2.5)\n\
         let c = swap(a1, 0, 2)\n\
         print(a1[0], a1[1], a1[2], b[1], c[2], frozen(a1), frozen(b), frozen(c))\n",
        "1.5 0.0 0.0 2.5 1.5 true false false",
    );
}

/// The collector no longer traces a frozen table, so everything it holds has
/// to have been frozen with it or it would be swept. Churn enough garbage
/// for many collections and read the whole table back.
#[test]
fn frozen_data_survives_collections_that_do_not_trace_it() {
    let code = "\
import vec2
enum Kind
  park,
  tower(height),
end
state table = nil
if table == nil then
  var rows = []
  for i in range(0, 2000) do
    set rows = append(rows, {
      id: i,
      name: \"row \" ++ str(i),
      kind: if i % 2 == 0 then Kind.park else Kind.tower(i) end,
      pos: vec3(i, 0, 1),
      tags: [i, i + 1],
    })
  end
  table = freeze(rows)
end
var sum = 0
for round in range(0, 200) do
  for i in range(0, 500) do
    let junk = { a: i, b: \"junk \" ++ str(i), c: [i, round] }
    set sum = sum + junk.a
  end
end
var check = 0
var towers = 0
for row in table do
  set check = check + row.id + row.tags[1] + row.pos.x + len(row.name)
  match row.kind
    when Kind.park -> nil
    when Kind.tower(h) -> set towers = towers + h
  end
end
print(frozen(table), len(table), check, towers, table[1999].name)
";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut outputs = Vec::new();
    for frame in 0..3 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        outputs.push(env.take_output().join("\n").trim().to_string());
    }
    assert!(
        env.heap().collections() >= 6,
        "the garbage loop should have collected many times, got {}",
        env.heap().collections()
    );
    // id: 0..2000 sums to 1_999_000; tags[1] = id + 1 adds 2_001_000; pos.x
    // = id adds 1_999_000; the names are "row N": 4 + digits(N).
    let name_len: i64 = (0..2000).map(|i: i64| 4 + i.to_string().len() as i64).sum();
    let check = 1_999_000 + 2_001_000 + 1_999_000 + name_len;
    let towers: i64 = (0..2000).filter(|i| i % 2 == 1).sum();
    let expected = format!("true 2000 {check}.0 {towers} row 1999");
    for (frame, out) in outputs.iter().enumerate() {
        assert_eq!(out, &expected, "frame {frame}");
    }
}

/// A frozen value that nothing roots is still not reclaimed: freezing is
/// one-way, and the documented cost of freezing short-lived data.
#[test]
fn an_unreachable_frozen_value_is_not_reclaimed() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "fn make()\n  freeze([1, 2, 3])\n  nil\nend\n\
             make()\n\
             for i in range(0, 20000) do\n  let junk = [i, \"junk \" ++ str(i)]\nend\n",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(env.heap().collections() >= 1);
    let stats = env.heap().frozen_counts();
    assert_eq!(stats.lists, 1, "the unrooted frozen list is still allocated");
}

#[test]
fn values_holding_functions_or_var_cells_cannot_be_frozen() {
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        let err = run_with("let f = fn(x) -> x\nfreeze({ run: f })\n", policy).unwrap_err();
        assert!(
            err.contains("freeze: the value holds a function, which cannot be frozen"),
            "{err}"
        );
    }
    // Nothing was half-frozen: the list beside the function is still an
    // ordinary value.
    let mut env = Env::new();
    let pid = env
        .load_program(
            "state inner = [1, 2]\nstate outer = [inner, fn(x) -> x]\nfreeze(outer)\n",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    assert!(env.run(sid).is_err());
    assert_eq!(env.heap().frozen_counts().total(), 0);
}

/// `freeze` and `frozen` take exactly one argument.
#[test]
fn freeze_and_frozen_check_their_arity() {
    assert!(run_with("freeze()\n", RunPolicy::FAST).is_err());
    assert!(run_with("frozen(1, 2)\n", RunPolicy::FAST).is_err());
}

/// A fork deep-copies the heap, frozen bits included: the fork's collector
/// skips the same objects, and its in-place operations copy the same ones.
/// A restore puts the snapshot's frozen set back under the live context.
#[test]
fn fork_and_restore_carry_the_frozen_set() {
    let code = "\
state table = freeze([[1, 2], [3, 4]])
state var grown = []
state var n = 0
set n = n + 1
set grown = append(table, n)
for i in range(0, 20000) do
  let junk = [i, \"junk \" ++ str(i)]
end
print(frozen(table), frozen(table[0]), frozen(grown), len(table), len(grown))
";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let live = env.create_stack(pid).unwrap();
    env.run(live).unwrap();
    assert_eq!(env.take_output().join(""), "true true false 2 3");
    let frozen_live = env.heap().frozen_counts();
    assert!(frozen_live.lists >= 3, "{frozen_live:?}");

    // The fork: same frozen objects, same answers after its own run and
    // its own collections.
    let fork = env.fork_execution(live).unwrap();
    assert_eq!(env.heap_for(fork).unwrap().frozen_counts(), frozen_live);
    env.reset_stack(fork).unwrap();
    env.run(fork).unwrap();
    assert_eq!(env.take_output_for(fork).join(""), "true true false 2 3");
    assert!(env.heap_for(fork).unwrap().collections() >= 1);
    assert_eq!(env.heap_for(fork).unwrap().frozen_counts(), frozen_live);

    // Advance the live stack, freezing more on the way, then restore the
    // snapshot over it: the frozen set is the snapshot's again.
    let snap = env.fork_execution(live).unwrap();
    let edited = env
        .compile_program_at(
            pid,
            &code.replace("set n = n + 1", "set n = n + 1\nfreeze([n, n, n])"),
            std::path::Path::new("edited.ptl"),
        )
        .unwrap();
    env.transfer_state(live, edited).unwrap();
    env.reset_stack(live).unwrap();
    env.run(live).unwrap();
    env.take_output();
    assert!(env.heap().frozen_counts().lists > frozen_live.lists);

    env.restore_execution(live, snap).unwrap();
    assert_eq!(env.heap().frozen_counts(), frozen_live);
    env.reset_stack(live).unwrap();
    env.run(live).unwrap();
    assert_eq!(env.take_output().join(""), "true true false 2 3");
}
