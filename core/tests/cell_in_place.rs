//! In-place writes through a `var` cell: `set xs[i] = v`, `set r.f = v` and
//! `set xs = append(xs, v)`, from the scope that declared the cell and — the
//! case that used to copy the whole container on every write — from a function
//! that did not.
//!
//! Three things are pinned here, each end to end:
//!
//! * **cost** — a loop of writes through a helper duplicates its container a
//!   constant number of times, not once per write (`DupStats`, as in
//!   `copy_elision.rs`);
//! * **value semantics** — whatever took a copy of the cell's contents before
//!   a write (a snapshot, another cell, a caller that read a helper's result)
//!   still sees what it took, and the result is the clone-and-alloc baseline's
//!   to the character;
//! * **the readers outside the script** — memoized scopes, the frame gate and
//!   the observation buffer all keep a cell's contents across a write, and
//!   each has to notice one that changed them under the same id.
//!
//! The mechanism and its soundness argument are in
//! `src/backend/bytecode/cells.rs`; `cell_fuzz.rs` beside it is the randomized
//! half of this file.

use petal::env::Env;
use petal::policy::RunPolicy;
use petal::stats::DUP_STATS_ENABLED;

/// Writes per shape: enough that a copy per write is unmistakable.
const N: usize = 2000;

/// Run `code` once under `policy`: what it printed, or the error.
fn run_with(code: &str, policy: RunPolicy) -> Result<String, String> {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(code)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

/// Run `code` for `frames` runs of one stack: everything printed, then the
/// persistent state. An error ends the frames and is part of the answer.
fn run_frames(code: &str, policy: RunPolicy, frames: usize) -> (Vec<String>, String) {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = env.load_program(code).expect("load");
    let sid = env.create_stack(pid).expect("stack");
    let mut out = Vec::new();
    for frame in 0..frames {
        if frame > 0 {
            env.reset_stack(sid).expect("reset");
        }
        let ran = env.run(sid);
        out.extend(env.take_output());
        if let Err(e) = ran {
            out.push(format!("error: {e}"));
        }
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    (out, pairs.join(","))
}

/// `code` prints `expected` with everything on, and the baseline agrees.
#[track_caller]
fn assert_prints(code: &str, expected: &str) {
    let fast = run_with(code, RunPolicy::FAST).unwrap_or_else(|e| panic!("{e}\n{code}"));
    assert_eq!(fast, expected, "with in-place cell writes:\n{code}");
    let baseline = run_with(code, RunPolicy::BASELINE).unwrap_or_else(|e| panic!("{e}\n{code}"));
    assert_eq!(baseline, expected, "clone-and-alloc baseline:\n{code}");
}

/// `(copies, bytes)` the heap recorded while running `code` once.
fn copy_cost(code: &str) -> (u64, u64) {
    let mut env = Env::new();
    let pid = env
        .load_program(code)
        .unwrap_or_else(|e| panic!("load failed: {e}\n--- source ---\n{code}"));
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid)
        .unwrap_or_else(|e| panic!("run failed: {e}\n--- source ---\n{code}"));
    let stats = env.dup_stats();
    (stats.total_count(), stats.total_bytes())
}

/// Assert `code` duplicates a backing store at most `max` times. Not zero:
/// the first write through a cell copies, since whatever initialized the cell
/// may still hold what it stored.
#[track_caller]
fn assert_copies_at_most(shape: &str, max: u64, code: &str) {
    if !DUP_STATS_ENABLED {
        return;
    }
    let (count, bytes) = copy_cost(code);
    assert!(
        count <= max,
        "{shape}: expected at most {max} copies, got {count} ({bytes} bytes). \
         The write is falling back to clone-and-alloc, making each one O(len).\n\
         --- source ---\n{code}"
    );
}

// ── Cost ────────────────────────────────────────────────────────────────────

#[test]
fn a_helper_writing_a_module_level_cell_does_not_copy_per_write() {
    assert_copies_at_most(
        "top-level fn, indexed write to a module-level var",
        2,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn put(i, v)\n  set xs[i] = v\nend\n\
             for i in range(0, {N}) do\n  put(i % 4, i)\nend\nprint(xs[3])"
        ),
    );
}

#[test]
fn a_helper_read_modify_write_does_not_copy_per_write() {
    // The read is what used to give the game away: `get xs[i]` handed the
    // list to a register, so the write after it could never be the only
    // holder.
    assert_copies_at_most(
        "top-level fn, read-modify-write",
        2,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn bump(i)\n  set xs[i] = get xs[i] + 1\nend\n\
             for i in range(0, {N}) do\n  bump(i % 4)\nend\nprint(xs[3])"
        ),
    );
    assert_copies_at_most(
        "top-level fn, compound write",
        2,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn bump(i)\n  set xs[i] += 1\nend\n\
             for i in range(0, {N}) do\n  bump(i % 4)\nend\nprint(xs[3])"
        ),
    );
}

#[test]
fn a_closure_writing_its_enclosing_functions_cell_does_not_copy_per_write() {
    assert_copies_at_most(
        "nested closure over a local var",
        2,
        &format!(
            "fn go()\n  var xs = [0, 0, 0, 0]\n  let bump = fn(i)\n    set xs[i] = get xs[i] + 1\n  end\n\
             \x20 for i in range(0, {N}) do\n    bump(i % 4)\n  end\n  xs[3]\nend\nprint(go())"
        ),
    );
}

#[test]
fn a_helper_writing_a_state_var_does_not_copy_per_write() {
    assert_copies_at_most(
        "top-level fn, state var",
        2,
        &format!(
            "state var xs = [0, 0, 0, 0]\nfn bump(i)\n  set xs[i] += 1\nend\n\
             for i in range(0, {N}) do\n  bump(i % 4)\nend\nprint(xs[3])"
        ),
    );
}

#[test]
fn a_helper_writing_a_record_field_does_not_copy_per_write() {
    assert_copies_at_most(
        "top-level fn, field write",
        2,
        &format!(
            "var r = {{hits: 0, misses: 0}}\nfn hit()\n  set r.hits = get r.hits + 1\nend\n\
             for i in range(0, {N}) do\n  hit()\nend\nprint(r.hits)"
        ),
    );
}

#[test]
fn a_conditional_write_in_a_helper_does_not_copy_per_write() {
    // The `set` is an `if` arm's result and the `if` is the function's: the
    // value is only dropped because every caller here drops it.
    assert_copies_at_most(
        "top-level fn, write under an if",
        2,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn put(i, v)\n  if i >= 0 then\n    set xs[i] = v\n  end\nend\n\
             for i in range(0, {N}) do\n  put(i % 4, i)\nend\nprint(xs[3])"
        ),
    );
}

#[test]
fn a_helper_calling_a_helper_does_not_copy_per_write() {
    // `outer`'s result is `put`'s, so whether it is read is `outer`'s
    // caller's business — and this caller drops it.
    assert_copies_at_most(
        "write in the tail of a tail call",
        2,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn put(i, v)\n  set xs[i] = v\nend\nfn outer(i)\n  put(i, i * 2)\nend\n\
             for i in range(0, {N}) do\n  outer(i % 4)\nend\nprint(xs[3])"
        ),
    );
}

#[test]
fn the_cell_accumulator_does_not_copy_per_append() {
    assert_copies_at_most(
        "set xs = append(xs, v) in the declaring scope",
        2,
        &format!("var xs = []\nfor i in range(0, {N}) do\n  set xs = append(xs, i)\nend\nprint(len(xs))"),
    );
    assert_copies_at_most(
        "set xs = append(get xs, v) from a helper",
        2,
        &format!(
            "var xs = []\nfn add(v)\n  set xs = append(get xs, v)\nend\n\
             for i in range(0, {N}) do\n  add(i)\nend\nprint(len(xs))"
        ),
    );
}

#[test]
fn a_nested_write_copies_the_inner_container_only() {
    if !DUP_STATS_ENABLED {
        return;
    }
    // `set g[i][j] = v` rewrites `g` in place and copies row `i`: the cell
    // owns its top-level container, not the rows `get g[i]` hands out. Rows
    // are 4 wide and `g` is 500 long, so a copy of `g` per write would cost
    // two orders of magnitude more than this allows.
    let rows = 500;
    let code = format!(
        "var g = []\nfor i in range(0, {rows}) do\n  set g = append(g, [0, 0, 0, 0])\nend\n\
         fn put(i, j, v)\n  set g[i][j] = v\nend\n\
         for i in range(0, {N}) do\n  put(i % {rows}, i % 4, i)\nend\nprint(g[3][3])"
    );
    let (count, bytes) = copy_cost(&code);
    let row_bytes = 4 * std::mem::size_of::<petal::value::Value>() as u64;
    assert!(
        count <= N as u64 + 4 && bytes <= (N as u64 + 4) * row_bytes + 64,
        "nested write copied more than one row per write: {count} copies, {bytes} bytes"
    );
}

#[test]
fn a_snapshot_costs_one_copy_not_one_per_write() {
    // Value semantics has a price, and it is paid once: the first write after
    // the snapshot copies, and the cell owns the copy from then on.
    assert_copies_at_most(
        "snapshot, then a loop of writes",
        3,
        &format!(
            "var xs = [0, 0, 0, 0]\nfn bump(i)\n  set xs[i] += 1\nend\nbump(0)\nlet snap = xs\n\
             for i in range(0, {N}) do\n  bump(i % 4)\nend\nprint(snap[0], xs[0])"
        ),
    );
}

// ── Value semantics ─────────────────────────────────────────────────────────

#[test]
fn a_snapshot_taken_before_a_write_does_not_see_it() {
    assert_prints(
        "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\n\
         put(0, 10)\nlet snap = xs\nput(1, 20)\nput(2, 30)\nprint(snap, xs)",
        "[10, 2, 3] [10, 20, 30]",
    );
    // The same from inside a function, where the read needs `get`.
    assert_prints(
        "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\n\
         fn both()\n  put(0, 10)\n  let snap = get xs\n  put(1, 20)\n  [snap, get xs]\nend\nprint(both())",
        "[[10, 2, 3], [10, 20, 3]]",
    );
}

#[test]
fn a_list_stored_in_two_places_is_two_lists() {
    // One cell's list becomes a row of another's grid; then each is written.
    assert_prints(
        "var xs = [1, 2, 3]\nvar g = [[0, 0, 0], [0, 0, 0]]\n\
         fn go()\n  set xs[0] = 7\n  set g[0] = get xs\n  set xs[1] = 8\n  set g[0][2] = 9\nend\n\
         go()\nprint(xs, g)",
        "[7, 8, 3] [[7, 2, 9], [0, 0, 0]]",
    );
    // A grid's row becomes a cell's own list.
    assert_prints(
        "var xs = [1, 2, 3]\nvar g = [[4, 5, 6], [0, 0, 0]]\n\
         fn go()\n  set g[1][0] = 1\n  set xs = get g[0]\n  set xs[0] = 40\n  set g[0][1] = 50\nend\n\
         go()\nprint(xs, g)",
        "[40, 5, 6] [[4, 50, 6], [1, 0, 0]]",
    );
    // Two cells share one list.
    assert_prints(
        "var xs = [1, 2, 3]\nvar ys = []\n\
         fn go()\n  set xs[0] = 5\n  set ys = get xs\n  set xs[1] = 6\n  set ys[2] = 7\nend\n\
         go()\nprint(xs, ys)",
        "[5, 6, 3] [5, 2, 7]",
    );
}

#[test]
fn an_element_read_out_before_a_write_does_not_see_it() {
    // `get g[0]` leaves `g` owning its top level but hands the row out, so a
    // write into that row must copy it.
    assert_prints(
        "var g = [[1, 2], [3, 4]]\nfn go()\n  set g[1][0] = 30\n  let row = get g[0]\n  set g[0][1] = 20\n  [row, get g]\nend\nprint(go())",
        "[[1, 2], [[1, 20], [30, 4]]]",
    );
    assert_prints(
        "var ps = [{x: 1}, {x: 2}]\nfn go()\n  set ps[1].x = 20\n  let p = get ps[0]\n  set ps[0].x = 10\n  [p, get ps]\nend\nprint(go())",
        "[{ x: 1 }, [{ x: 10 }, { x: 20 }]]",
    );
}

#[test]
fn a_helpers_result_is_the_container_when_the_caller_reads_it() {
    // `fn put` ends on a `set`, so it evaluates to the whole list. A caller
    // that keeps that holds a list the next write must not touch.
    assert_prints(
        "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\n\
         put(0, 10)\nlet a = put(1, 20)\nput(2, 30)\nlet b = put(0, 11)\nput(1, 21)\nprint(a, b, xs)",
        "[10, 20, 3] [11, 20, 30] [11, 21, 30]",
    );
    // Through a second function, whose own result is the first one's.
    assert_prints(
        "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\nfn via(i, v)\n  put(i, v)\nend\n\
         via(0, 10)\nlet a = via(1, 20)\nvia(2, 30)\nprint(a, xs)",
        "[10, 20, 3] [10, 20, 30]",
    );
    // As a callback: every result is collected.
    assert_prints(
        "var xs = [0, 0]\nlet seen = map([0, 1], fn(i)\n  set xs[i] = i + 1\nend)\nprint(seen, xs)",
        "[[1, 0], [1, 2]] [1, 2]",
    );
}

#[test]
fn a_forwarded_result_is_read_or_dropped_per_call_not_per_function() {
    // `g` ends on a write to its own cell and `k` forwards it, so whether the
    // list is handed out is decided by `k`'s caller, call by call — also when
    // the calls before it dropped the result and memoization had every reason
    // to think it knew what `g(3)` evaluates to.
    let code = "fn g(n)\n  var xs = [1, 2, n]\n  set xs[0] = 9\nend\nfn k(n) g(n) end\n\
                fn use(n, keep)\n  if keep then print(k(n)) else k(n) end\n  nil\nend\n\
                state c = 0\nc = c + 1\nuse(3, false)\nuse(3, false)\nuse(3, true)\nuse(3, c % 2 == 0)\nnil";
    let baseline = run_frames(code, RunPolicy::BASELINE, 4);
    assert_eq!(baseline.0, ["[9, 2, 3]"; 6]);
    assert_eq!(run_frames(code, RunPolicy::FAST, 4), baseline);
    assert_eq!(run_frames(code, RunPolicy::REPLAY, 4), baseline);
}

#[test]
fn every_way_of_calling_a_writing_helper_gets_the_list_it_asked_for() {
    // A method call, a pipe and a list literal all read their callee's
    // result; only the plain statement calls in between drop it.
    assert_prints(
        "class Box\n  v: int,\nend\nvar xs = [0, 0, 0]\nfn Box.put(b: Box, i: int)\n  set xs[i] = b.v\nend\n\
         fn put(i, v)\n  set xs[i] = v\nend\n\
         let r1 = Box(5).put(0)\nBox(6).put(1)\nlet r2 = 2 |> put(9)\n1 |> put(4)\n\
         let r3 = [put(0, 3), put(1, 3)]\nput(2, 2)\nprint(r1, r2, r3, xs)",
        "[5, 0, 0] [5, 6, 9] [[3, 4, 9], [3, 3, 9]] [3, 3, 2]",
    );
}

#[test]
fn a_write_in_value_position_hands_its_container_out() {
    assert_prints(
        "var xs = [0, 0, 0]\nlet each = for i in range(0, 3) do\n  set xs[i] = i + 1\nend\nprint(each, xs)",
        "[[1, 0, 0], [1, 2, 0], [1, 2, 3]] [1, 2, 3]",
    );
    assert_prints(
        "var xs = [0, 0]\nfn go(c)\n  let a = if c then\n    set xs[0] = 1\n  else\n    set xs[1] = 2\n  end\n  set xs[0] = 9\n  a\nend\nprint(go(true), go(false), xs)",
        "[1, 0] [9, 2] [9, 2]",
    );
}

#[test]
fn a_call_in_the_written_value_runs_before_the_cell_is_read() {
    // `bump` writes the cell that the enclosing `set` is in the middle of
    // writing. Index and value are evaluated first, so its write is kept.
    assert_prints(
        "var xs = [1, 2, 3]\nfn bump()\n  set xs[0] += 10\n  5\nend\nfn go()\n  set xs[1] = bump()\nend\ngo()\nprint(xs)",
        "[11, 5, 3]",
    );
    // An accumulator's argument is evaluated after the read of the list, so
    // a callee's write is overwritten — in every configuration alike.
    assert_prints(
        "var xs = [1]\nfn add()\n  set xs = append(get xs, 7)\n  8\nend\nfn go()\n  set xs = append(get xs, add())\nend\ngo()\nprint(xs)",
        "[1, 8]",
    );
    // A call between reading the cell and indexing what was read.
    assert_prints(
        "var xs = [1, 2, 3]\nfn shift()\n  set xs[1] = 99\n  1\nend\nfn go()\n  get xs[shift()]\nend\nprint(go(), xs)",
        "2 [1, 99, 3]",
    );
}

#[test]
fn iterating_a_cell_while_writing_it_iterates_what_it_held() {
    assert_prints(
        "var xs = [1, 2, 3]\nfn go()\n  for x in get xs do\n    set xs[0] = get xs[0] + x\n    set xs = append(get xs, x)\n  end\nend\ngo()\nprint(xs)",
        "[7, 2, 3, 1, 2, 3]",
    );
}

#[test]
fn every_kind_of_container_a_cell_can_hold_is_written_through() {
    assert_prints(
        "var a = f64_array(3)\nfn put(i, v)\n  set a[i] = v\nend\n\
         put(0, 1.5)\nlet snap = a\nput(1, 2.5)\nprint(snap[0], snap[1], a[0], a[1])",
        "1.5 0.0 1.5 2.5",
    );
    assert_prints(
        "var r = {a: 1}\nfn put(k, v)\n  set r[k] = v\nend\n\
         put(\"b\", 2)\nlet snap = r\nput(\"c\", 3)\nprint(snap, r)",
        "{ a: 1, b: 2 } { a: 1, b: 2, c: 3 }",
    );
    // A cell holding something that is not a container at all.
    assert_prints(
        "var n = 1\nfn bump()\n  set n += 1\nend\nbump()\nbump()\nprint(n)",
        "3",
    );
}

#[test]
fn a_failed_write_leaves_the_cell_as_it_was() {
    // The error is the same error, and a `state var` that outlives the failed
    // run holds what the baseline would have left in it.
    let code = "state var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\n\
                put(0, xs[0] + 1)\nput(1, xs[1] + 1)\nif xs[0] > 2 then\n  put(7, 0)\nend\nprint(xs)";
    let fast = run_frames(code, RunPolicy::FAST, 4);
    let baseline = run_frames(code, RunPolicy::BASELINE, 4);
    assert_eq!(fast, baseline);
    assert!(
        fast.0.iter().any(|l| l.contains("out of bounds")),
        "the write was supposed to fail on a later frame: {:?}",
        fast.0
    );
}

#[test]
fn a_state_var_written_in_place_carries_across_frames() {
    let code = "state var xs = [0, 0, 0]\nstate var log = []\n\
                fn bump(i)\n  set xs[i] += 1\nend\nfn note(v)\n  set log = append(get log, v)\nend\n\
                bump(0)\nbump(2)\nnote(xs[0] * 10)\nprint(xs, log)";
    let fast = run_frames(code, RunPolicy::FAST, 3);
    assert_eq!(fast, run_frames(code, RunPolicy::BASELINE, 3));
    assert_eq!(
        fast.0,
        ["[1, 0, 1] [10]", "[2, 0, 2] [10, 20]", "[3, 0, 3] [10, 20, 30]"]
    );
}

// ── The readers outside the script ──────────────────────────────────────────

#[test]
fn a_memoized_read_is_not_replayed_over_an_in_place_write() {
    // `peek` records what it read, and the record's copy of the list *is* the
    // list `bump` goes on to edit — so comparing the two would find them
    // equal forever. On the second frame `peek` has to run again.
    let code = "state var xs = [1, 2, 3]\nfn peek()\n  get xs[0] * 10\nend\nfn bump()\n  set xs[0] = get xs[0] + 1\nend\n\
                bump()\nlet a = peek()\nbump()\nprint(a)";
    let replay = run_frames(code, RunPolicy::REPLAY, 4);
    assert_eq!(replay, run_frames(code, RunPolicy::BASELINE, 4));
    assert_eq!(replay.0, ["20", "40", "60", "80"]);
}

#[test]
fn a_memoized_reader_of_an_untouched_cell_is_still_replayed() {
    // The other half: reading a cell does not, by itself, cost a scope its
    // record. Without this the test above would pass by never memoizing.
    let code = "state var xs = [1, 2, 3]\nfn peek()\n  get xs[0] * 10 + len(get xs)\nend\nprint(peek())";
    let mut env = Env::new();
    env.set_policy(RunPolicy::REPLAY);
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for frame in 0..3 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        assert_eq!(env.take_output().join(""), "13");
    }
    let stats = env.memo_stats(sid).unwrap();
    assert!(stats.hits >= 2, "peek() was not replayed: {stats:?}");
}

#[test]
fn a_scope_that_writes_a_cell_in_place_is_not_replayed() {
    // `bump`'s arguments never change, and neither does anything a record
    // could compare, since the cell holds the edited list rather than a new
    // one. It still has to run every frame.
    let code = "state var xs = [0, 0]\nfn bump()\n  set xs[0] += 1\nend\nfn frame()\n  bump()\n  bump()\nend\nframe()\nprint(xs)";
    let replay = run_frames(code, RunPolicy::REPLAY, 3);
    assert_eq!(replay, run_frames(code, RunPolicy::BASELINE, 3));
    assert_eq!(replay.0, ["[2, 0]", "[4, 0]", "[6, 0]"]);
}

#[test]
fn a_scope_writing_its_own_cell_in_place_can_still_be_replayed() {
    // A function's local `var` dies with the call, so writing it in place is
    // nothing a replay has to reproduce.
    let code = "fn total(n)\n  var acc = [0]\n  for i in range(0, n) do\n    set acc[0] += i\n  end\n  acc[0]\nend\nprint(total(50))";
    let mut env = Env::new();
    env.set_policy(RunPolicy::REPLAY);
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for frame in 0..3 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        assert_eq!(env.take_output().join(""), "1225");
    }
    let stats = env.memo_stats(sid).unwrap();
    assert!(stats.hits >= 2, "total() was not replayed: {stats:?}");
}

#[test]
fn the_frame_gate_sees_an_in_place_write_to_a_state_var() {
    // The gate snapshots every `state var` when a run starts and compares at
    // the end. The snapshot's list is the one the run edits, so it is the
    // cell's mutation count that says the run changed it — and a run that
    // changed state has not settled.
    // (The trailing `nil` keeps `bump()` from being the program's result,
    // which would hand the list out and make every frame's write a copy.)
    let mut env = Env::new();
    let pid = env
        .load_program("state var xs = [0, 0]\nfn bump()\n  set xs[0] += 1\nend\nbump()\nnil")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    for frame in 0..4 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        assert!(
            env.run_needed(sid),
            "frame {frame}: a run that wrote a state var reads as settled"
        );
    }
    // And one that writes nothing does settle.
    let pid = env
        .load_program("state var xs = [0, 0]\nfn peek()\n  get xs[0]\nend\npeek()\nnil")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert!(!env.run_needed(sid), "a run that only read never settles");
}

#[test]
fn observation_reports_a_cells_container_after_an_in_place_write() {
    let code = "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\nput(0, 10)\nlet kept = put(1, 20)\nput(2, 30)\nprint(xs)";
    let mut env = Env::new();
    env.observations_mut().enable();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output().join(""), "[10, 20, 30]");
    let seen = env.get_observations_json(pid, sid);
    // A binding nothing else reads is still a binding: it holds what `put`
    // returned, which the later write must not have touched.
    assert_eq!(seen["kept"], serde_json::json!([10, 20, 3]), "{seen:?}");
    // The write inside `put` reports the list as it now stands.
    assert_eq!(seen["put.xs"], serde_json::json!([10, 20, 30]), "{seen:?}");
}

#[test]
fn the_explain_policy_does_not_write_cells_in_place() {
    // The trace is a history of values; an in-place write would edit it. So
    // under `explain` every cell write copies, and the answer is unchanged.
    let code = "var xs = [1, 2, 3]\nfn put(i, v)\n  set xs[i] = v\nend\nput(0, 10)\nlet snap = xs\nput(1, 20)\nprint(snap, xs)";
    assert_eq!(
        run_with(code, RunPolicy::EXPLAIN).unwrap(),
        "[10, 2, 3] [10, 20, 3]"
    );
}

#[test]
fn the_frame_gate_settles_when_in_place_writes_change_nothing() {
    // A script that stores the same values every frame — a layout cache, a
    // selection re-asserted — has to be able to go quiet. The gate cannot
    // compare the list it kept at run start, because that is the list being
    // rewritten, so it compares fingerprints taken around the rewrite.
    let code = "state var xs = [0, 0]\nstate var n = 0\nfn put(i, v)\n  set xs[i] = v\nend\nfn keep()\n  set n = get n * 1\nend\n\
                put(0, 7)\nput(1, 8)\nkeep()\nnil";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(env.run_needed(sid), "the first frame changed the list");
    for frame in 1..4 {
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert!(
            !env.run_needed(sid),
            "frame {frame}: writes that changed nothing read as a change: {:?}",
            env.run_needed_reason(sid)
        );
    }
    // And a write that does change something, after frames that did not.
    let code = "state var xs = [0, 0]\nstate tick = 0\nfn put(i, v)\n  set xs[i] = v\nend\n\
                tick = tick + 1\nput(0, 7)\nput(1, if tick > 3 then 9 else 8 end)\nnil";
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut seen = Vec::new();
    for frame in 0..6 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        seen.push(env.get_state_json(pid, sid)["xs"].to_string());
    }
    assert_eq!(seen[2], "[7,8]");
    assert_eq!(seen[5], "[7,9]");
}

#[test]
fn the_frame_gate_settles_on_module_level_in_place_writes_too() {
    // No function, so no memo scope is open and the VM's straight-line loop
    // retires the cell accesses itself. It still has to hand the first
    // mutation of a watched `state var` to the general executor, which takes
    // the fingerprint the gate compares.
    let code = "state var xs = [0, 0]\nset xs[0] = 7\nset xs[1] = xs[0] + 1\nnil";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(env.run_needed(sid), "the first frame changed the list");
    for frame in 1..4 {
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        assert!(
            !env.run_needed(sid),
            "frame {frame}: writes that changed nothing read as a change: {:?}",
            env.run_needed_reason(sid)
        );
    }
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[7,8]");
    // And one that keeps changing never settles.
    let pid = env
        .load_program("state var xs = [0, 0]\nset xs[0] = xs[0] + 1\nnil")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    for frame in 0..4 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        assert!(
            env.run_needed(sid),
            "frame {frame}: a changed list read as settled"
        );
    }
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[4,0]");
}

#[test]
fn a_scope_reading_its_own_cell_and_an_outer_one_depends_on_the_outer() {
    // The straight-line loop skips the recorder for a scope's own cells and
    // must not skip it for anyone else's: `total` is replayed while `scale`
    // holds still and runs again on the frame it changes.
    let code = "state var scale = [1]\nstate tick = 0\nfn total(n)\n  var acc = [0]\n  for i in range(0, n) do\n    set acc[0] = get acc[0] + i * get scale[0]\n  end\n  acc[0]\nend\n\
                tick = tick + 1\nif tick == 3 then set scale[0] = 2 end\nprint(total(10))";
    let replay = run_frames(code, RunPolicy::REPLAY, 5);
    assert_eq!(replay, run_frames(code, RunPolicy::BASELINE, 5));
    assert_eq!(replay.0, ["45", "45", "90", "90", "90"]);
}

#[test]
fn a_helper_that_always_writes_in_place_stops_opening_scopes() {
    // Such a helper can never be replayed, and a solver calls it thousands of
    // times a frame: after a few calls its site stops paying for a scope it
    // is only going to throw away (one probe a run aside).
    let code = "state var xs = [0, 0]\nfn bump(i)\n  set xs[i] += 1\nend\n\
                for k in range(0, 50) do\n  bump(k % 2)\nend\nprint(xs)";
    let mut env = Env::new();
    env.set_policy(RunPolicy::REPLAY);
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut out = Vec::new();
    for frame in 0..3 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        out.extend(env.take_output());
    }
    assert_eq!(out, ["[25, 25]", "[50, 50]", "[75, 75]"]);
    let stats = env.memo_stats(sid).unwrap();
    assert!(
        stats.effectful <= 8 && stats.tiny >= 130,
        "bump() kept opening scopes: {stats:?}"
    );
}

#[test]
fn a_fork_writes_its_own_copy_of_a_cells_container() {
    // The fork's heap is a deep copy, ownership bits and all: both sides go
    // on writing in place, each into its own list.
    let code = "state var xs = [0, 0]\nfn bump()\n  set xs[0] += 1\nend\nbump()\nnil";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    let fork = env.fork_execution(sid).unwrap();
    for _ in 0..3 {
        env.reset_stack(fork).unwrap();
        env.run(fork).unwrap();
    }
    assert_eq!(env.get_state_json(pid, fork)["xs"].to_string(), "[5,0]");
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[2,0]");
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[3,0]");
    assert_eq!(env.get_state_json(pid, fork)["xs"].to_string(), "[5,0]");
}

#[test]
fn a_host_write_to_a_state_var_is_not_written_through() {
    // `set_state_from_json` stores a value the host made; the next in-place
    // write must start from it, and must not reach back into anything else.
    let code = "state var xs = [0, 0]\nfn bump()\n  set xs[0] += 1\nend\nbump()\nnil";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    env.set_state_from_json(pid, sid, "xs", &serde_json::json!([40, 7]))
        .unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[41,7]");
}

#[test]
fn the_frame_gate_settles_on_equal_element_writes_inside_a_loop() {
    // The same store, issued from a loop body rather than straight-line code,
    // has to read the same way to the gate: equal values, no change.
    for code in [
        "state var xs = [0, 0]\nfor i in range(0, 2) do\n  set xs[i] = i + 7\nend\nnil",
        "state xs = [0, 0]\nfor i in range(0, 2) do\n  xs[i] = i + 7\nend\nnil",
        "state var xs = [0, 0]\nfn fill()\n  for i in range(0, 2) do\n    set xs[i] = i + 7\n  end\nend\nfill()\nnil",
        "state var xs = [0, 0]\nvar i = 0\nwhile i < 2 do\n  set xs[i] = i + 7\n  set i = i + 1\nend\nnil",
    ] {
        let mut env = Env::new();
        let pid = env.load_program(code).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert!(env.run_needed(sid), "the first frame changed the list");
        for frame in 1..4 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            assert!(
                !env.run_needed(sid),
                "frame {frame} of {code:?}: equal writes read as a change: {:?}",
                env.run_needed_reason(sid)
            );
        }
        assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[7,8]");
    }
    // A record field stored from a loop settles the same way.
    let code = "state r = { n: 0 }\nfor i in range(0, 2) do\n  r.n = 5\nend\nnil";
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert!(!env.run_needed(sid), "{:?}", env.run_needed_reason(sid));
    // And a loop whose stores do change something is still seen to: on every
    // frame, and on the one frame that differs after frames that did not.
    let code = "state xs = [0, 0]\nfor i in range(0, 2) do\n  xs[i] = xs[i] + 1\nend\nnil";
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    for frame in 0..3 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        assert!(env.run_needed(sid), "frame {frame}: a changed list settled");
    }
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[3,3]");
    let code = "state xs = [0, 0]\nstate tick = 0\ntick = min(tick + 1, 4)\n\
                for i in range(0, 2) do\n  xs[i] = if tick == 4 then 9 else 7 end\nend\nnil";
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut needed = Vec::new();
    for frame in 0..6 {
        if frame > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        needed.push(env.run_needed(sid));
    }
    // Frames 0-3 move `tick`; frame 3 also rewrites the list; 4 and 5 are quiet.
    assert_eq!(needed, [true, true, true, true, false, false]);
    assert_eq!(env.get_state_json(pid, sid)["xs"].to_string(), "[9,9]");
}
