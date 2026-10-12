//! The static-value cache (`petal::static_cache`,
//! docs/tasks/static-value-optimization.md, S1): top-level values that
//! depend on no dynamic data are built once and kept on the stack.
//!
//! The cache must never change what a program does, only how much of it
//! runs. So nearly every test here drives the same frames twice, with the
//! cache on and with it off, and compares everything a host could tell
//! apart: prints, the run's value or error, and state. The first four are
//! the defects the design review found in the first draft, each of which
//! would have been a wrong value or a missing error.

use petal::env::{Env, StaticSlotState};
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::Value;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

fn world_with(src: &str, policy: RunPolicy, setup: impl FnOnce(&mut Env)) -> World {
    let mut env = Env::new();
    setup(&mut env);
    env.set_policy(policy);
    env.set_echo(false);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("compile: {e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

fn world(src: &str, policy: RunPolicy) -> World {
    world_with(src, policy, |_| {})
}

/// One frame the way a host drives it: reset, run, collect.
fn frame(w: &mut World) -> String {
    w.env.reset_stack(w.sid).unwrap();
    run_frame(w)
}

fn run_frame(w: &mut World) -> String {
    let result = match w.env.run(w.sid) {
        Ok(v) => format!("ok {}", petal::value::value_to_display_string(&v, w.env.heap())),
        Err(e) => format!("error {e}"),
    };
    let out = w.env.take_output().join("\n");
    let state = serde_json::Value::Object(w.env.get_state_json(w.pid, w.sid));
    format!("{out}\n=> {result}\nstate {state}")
}

/// The policies the cache is held against: each pair differs in the cache
/// alone. Memoization and the optimizer both change what a run executes, so
/// the cache is checked over each combination.
fn pairs() -> Vec<(RunPolicy, RunPolicy)> {
    ["replay", "replay-memo", "baseline+memo", "baseline"]
        .iter()
        .map(|name| {
            let p = RunPolicy::parse(name).unwrap();
            (p.with_static_cache(true), p.with_static_cache(false))
        })
        .collect()
}

/// Run `frames` frames of `src` with the cache on and off under every
/// policy pair and require the same output. Returns the frames of the
/// default policy, and how many runs the cache skipped there.
fn same_on_and_off(src: &str, frames: usize) -> (Vec<String>, u64) {
    let mut first = None;
    for (on, off) in pairs() {
        let (mut a, mut b) = (world(src, on), world(src, off));
        let mut out = Vec::new();
        for i in 0..frames {
            let (fa, fb) = (frame(&mut a), frame(&mut b));
            assert_eq!(fa, fb, "frame {i} differs with the cache on ({:?})\n{src}", on.name());
            out.push(fa);
        }
        let skips = a.env.static_values(a.sid).map_or(0, |v| v.stats.skips);
        assert_eq!(
            b.env.static_values(b.sid).map_or(0, |v| v.slots.len()),
            0,
            "the cache is off"
        );
        first.get_or_insert((out, skips));
    }
    first.unwrap()
}

const COUNTER: &str = "state n = 0\nn = n + 1\n";

#[test]
fn a_static_table_is_built_once_and_read_from_its_slot() {
    let src = format!(
        "let LANES = [\n  {{name: \"BACKLOG\", limit: 0}},\n  {{name: \"REVIEW\", limit: 2}},\n]\n{COUNTER}print(n, LANES[n % 2].name)\n"
    );
    let (frames, skips) = same_on_and_off(&src, 4);
    assert!(frames[0].starts_with("1 REVIEW"), "{}", frames[0]);
    assert!(frames[3].starts_with("4 BACKLOG"), "{}", frames[3]);
    assert_eq!(skips, 3, "built on the first frame, skipped on the other three");

    // The value in the slot is the same heap object on every frame.
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    let slot = |w: &World| {
        let v = w.env.static_values(w.sid).unwrap();
        let (info, state) = v.slots.iter().find(|(i, _)| i.name.as_deref() == Some("LANES")).unwrap().clone();
        assert_eq!(info.at.as_deref(), Some("line 1"));
        state
    };
    let StaticSlotState::Filled(first) = slot(&w) else { panic!("LANES is kept") };
    frame(&mut w);
    assert_eq!(slot(&w), StaticSlotState::Filled(first));
    assert!(w.env.heap().is_immutable(first), "a kept value is marked immutable");
}

// --- the four defects of the first draft -----------------------------------

/// A value built inside a loop from the loop variable is a different value
/// on every iteration. Caching it per term would serve iteration 0's value
/// to the rest.
#[test]
fn a_loop_variant_value_is_not_served_from_the_first_iteration() {
    let src = "for i in range(0, 4) do\n  let sq = [i, i * i]\n  print(sq)\nend\n\
               let ROWS = for i in range(0, 3) do [i, i * 10] end\nprint(ROWS)\n\
               let TOTAL = 0\nfor r in ROWS do\n  let cell = {at: r[0], v: r[1] + 1}\n  TOTAL = TOTAL + cell.v\nend\nprint(TOTAL)\n";
    let (frames, _) = same_on_and_off(src, 3);
    assert!(
        frames[2].starts_with("[0, 0]\n[1, 1]\n[2, 4]\n[3, 9]\n[[0, 0], [1, 10], [2, 20]]\n33"),
        "{}",
        frames[2]
    );
}

/// `let by_sec = [0, 0, 0]` followed by a write is lowered to an in-place
/// edit of the literal's own storage. A kept literal would carry one
/// frame's edits into the next.
#[test]
fn a_literal_that_is_written_to_is_rebuilt_every_run() {
    let cases = [
        // The settings app's shape: an indexed write to the binding itself.
        "let by_sec = [0, 0, 0, 0, 0]\nby_sec[n % 5] = n\nprint(by_sec)\n",
        // A loop-carried accumulator rooted at a literal.
        "let acc = [100]\nfor i in range(0, n) do\n  acc = append(acc, i)\nend\nprint(acc)\n",
        // The write goes through another name.
        "let BASE = [[1, 2], [3, 4], [5, 6]]\nlet cur = append(BASE, [n])\nprint(BASE, cur)\n",
        "let BASE = [[1, 2], [3, 4], [5, 6]]\nlet alias = BASE\nlet cur = drop_last(alias)\nprint(len(cur), cur[0])\n",
        // A temporary that only exists to be written to.
        "let cur = append([[1], [2], [3]], n)\nprint(cur)\n",
        "let rec = {a: [1, 2], b: [3, 4]}\nrec.a = n\nprint(rec)\n",
        // A function that writes its parameter, handed a kept table.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nfn bump(xs, v)\n  xs[0] = v\n  xs\nend\nprint(bump(TABLE, n), TABLE)\n",
        // An element of a kept table, edited through an alias.
        "let NEST = [[1, 2], [3, 4], [5, 6]]\nlet INNER = NEST[0]\nlet grown = append(INNER, n)\nprint(NEST, grown)\n",
        // A kept table put in a `var` cell and written through it.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nvar c = TABLE\nset c[1] = n\nset c = append(get c, n)\nprint(get c, TABLE)\n",
        // A kept table copied into state and edited there.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nstate held = TABLE\nheld = append(held, n)\nprint(len(held), TABLE)\n",
        // Either arm of a branch may be the one that is written to.
        "let A = [[1], [2], [3]]\nlet B = [[4], [5], [6]]\nlet pick = if n % 2 == 0 then A else B end\nlet out = append(pick, n)\nprint(A, B, out)\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        same_on_and_off(&src, 6);
    }
}

/// A top-level `assert` over a static table is itself static. It must go
/// on being checked: a table edited so the assert fails has to fail on
/// every run, not only on the run that built it.
#[test]
fn a_top_level_assert_keeps_running() {
    let failing = "let TABLE = [[1, 2], [3, 4], [5, 6]]\nassert(len(TABLE) == 12)\nprint(\"unreachable\")\n";
    let (frames, _) = same_on_and_off(failing, 4);
    for f in &frames {
        assert!(f.contains("=> error") && f.contains("ssert"), "{f}");
    }
    assert_eq!(frames[0], frames[3]);

    let passing = format!(
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nassert(len(TABLE) == 3)\n{COUNTER}print(n, TABLE)\n"
    );
    let (frames, skips) = same_on_and_off(&passing, 3);
    assert!(frames[2].starts_with("3 [[1, 2], [3, 4], [5, 6]]"), "{}", frames[2]);
    assert!(skips > 0);
}

/// An unused binding whose initializer raises must keep raising, at its
/// own line, on every run.
#[test]
fn an_unused_failing_binding_keeps_raising() {
    let src = "let XS = [[1], [2], [3]]\nprint(\"before\")\nlet BAD = XS[7]\nprint(\"after\")\n";
    let (frames, _) = same_on_and_off(src, 4);
    for f in &frames {
        assert!(f.starts_with("before\n=> error"), "{f}");
        assert!(f.contains("[line 3, column 11]"), "the error is at the binding's line: {f}");
    }
    assert_eq!(frames[0], frames[3]);

    // The same when the failing binding sits in the middle of a run of
    // tables that would otherwise be kept together.
    let src = "let A = [[1], [2]]\nlet BAD = A[9]\nlet B = [[3], [4]]\nprint(A, B)\n";
    let (frames, skips) = same_on_and_off(src, 3);
    assert!(frames.iter().all(|f| f.contains("=> error")), "{frames:?}");
    assert_eq!(skips, 0, "a run that raised stored nothing");
}

// --- lifetime ---------------------------------------------------------------

/// The frames around a reload, cache on against cache off, for a literal
/// edit (patched in place: the same lowering, a moved constant-table
/// revision) and a structural one (recompiled).
#[test]
fn a_reload_rebuilds_every_static_value() {
    let old = format!(
        "let PAD = 4\nlet ROWS = [[1, PAD], [2, PAD * 2], [3, PAD * 3]]\nfn total(rows)\n  let t = 0\n  for r in rows do t = t + r[1] end\n  t\nend\nlet SUM = total(ROWS)\n{COUNTER}print(n, ROWS, SUM)\n"
    );
    let literal = old.replace("let PAD = 4", "let PAD = 5");
    let structural = old.replace("[3, PAD * 3]]", "[3, PAD * 3], [4, PAD * 4]]");
    for (edit, new) in [("literal", &literal), ("structural", &structural)] {
        for (on, off) in pairs() {
            let (mut a, mut b) = (world(&old, on), world(&old, off));
            for _ in 0..2 {
                assert_eq!(frame(&mut a), frame(&mut b));
            }
            let kept = a.env.static_values(a.sid).unwrap();
            assert!(kept.stats.skips > 0, "the tables were kept before the edit");
            for w in [&mut a, &mut b] {
                w.env.reload_program(w.sid, new, None).unwrap();
            }
            // The run straight after the reload, then ordinary frames.
            let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
            assert_eq!(fa, fb, "{edit}: the run after the reload");
            let expect = if edit == "literal" { "3 [[1, 5], [2, 10], [3, 15]] 30" } else { "3 [[1, 4], [2, 8], [3, 12], [4, 16]] 40" };
            assert!(fa.starts_with(expect), "{edit}: state was kept and the tables rebuilt: {fa}");
            for i in 0..3 {
                assert_eq!(frame(&mut a), frame(&mut b), "{edit}: frame {i} after the reload");
            }
            let after = a.env.static_values(a.sid).unwrap().stats;
            assert_eq!(after.clears, 1, "{edit}: the reload emptied the table once");
            assert!(after.skips > kept.stats.skips, "{edit}: and it is kept again afterwards");

            // A reloaded run is a cold start of the new source with the
            // state carried over.
            let mut cold = world(new, on);
            let program = cold.env.compile_program(cold.pid, new).unwrap();
            for _ in 0..2 {
                frame(&mut cold);
            }
            cold.env.transfer_state(cold.sid, program).unwrap();
            assert_eq!(run_frame(&mut cold), fa, "{edit}: against a cold start");
        }
    }
}

/// A literal patched in place changes no lowering, so a second stack on the
/// same program has nothing to tell it that its tables are stale except the
/// constant table's revision.
#[test]
fn a_patched_literal_empties_every_stack_of_the_program() {
    let old = format!("config let K = 3\nlet T = [[K, K + 1], [K * 2, K * 3]]\n{COUNTER}print(n, T)\n");
    let new = old.replace("K = 3", "K = 7");
    let mut w = world(&old, RunPolicy::REPLAY);
    let other = w.env.create_stack(w.pid).unwrap();
    frame(&mut w);
    w.env.run(other).unwrap();
    w.env.reset_stack(other).unwrap();
    w.env.run(other).unwrap();
    assert!(w.env.take_output().iter().all(|l| l.ends_with("[[3, 4], [6, 9]]")));
    let report = w.env.reload_program(w.sid, &new, None).unwrap();
    assert_eq!(report.outcome, petal::env::ReloadOutcome::Patched);
    w.env.reset_stack(other).unwrap();
    w.env.run(other).unwrap();
    assert_eq!(w.env.take_output(), ["3 [[7, 8], [14, 21]]"], "the other stack rebuilt its table");
    assert!(run_frame(&mut w).starts_with("2 [[7, 8], [14, 21]]"));
}

/// A fork carries the table and the heap it points into; each side then
/// goes its own way.
#[test]
fn a_fork_keeps_its_own_copy_of_the_table() {
    let src = format!("let T = [[1, 2], [3, 4]]\n{COUNTER}print(n, T)\n");
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    frame(&mut w);
    let fork = w.env.fork_execution(w.sid).unwrap();
    for _ in 0..3 {
        w.env.reset_stack(fork).unwrap();
        w.env.run(fork).unwrap();
    }
    assert_eq!(w.env.take_output_for(fork).last().unwrap(), "5 [[1, 2], [3, 4]]");
    assert!(w.env.static_values(fork).unwrap().stats.skips >= 4, "the fork skipped from its first run");
    w.env.drop_fork(fork);
    assert!(frame(&mut w).starts_with("3 [[1, 2], [3, 4]]"));

    // A snapshot restored onto the stack starts with an empty table, like a
    // reload: its heap is a copy, and the program may have changed since.
    let snapshot = w.env.fork_execution(w.sid).unwrap();
    frame(&mut w);
    w.env.restore_execution(w.sid, snapshot).unwrap();
    assert!(run_frame(&mut w).starts_with("4 [[1, 2], [3, 4]]"));
    assert!(frame(&mut w).starts_with("5 [[1, 2], [3, 4]]"));
}

/// Kept values are roots: frames that allocate enough to collect many times
/// over leave them intact.
#[test]
fn kept_values_survive_collections() {
    let src = format!(
        "let WORDS = [\"alpha\", \"beta\", \"gamma\"]\nlet T = map(WORDS, fn(w) {{word: w, parts: [w, w ++ \"!\"]}} end)\n{COUNTER}\
         let junk = for i in range(0, 30000) do [i, \"x{{i}}\", {{k: i}}] end\nprint(n, len(junk), T[n % 3].parts)\n"
    );
    let mut w = world(&src, RunPolicy::REPLAY);
    let mut b = world(&src, RunPolicy::REPLAY.with_static_cache(false));
    let before = w.env.heap().collections();
    for i in 0..12 {
        assert_eq!(frame(&mut w), frame(&mut b), "frame {i}");
    }
    assert!(w.env.heap().collections() > before + 1, "the frames collected");
    assert!(w.env.static_values(w.sid).unwrap().stats.skips >= 11);
}

// --- when it is off ---------------------------------------------------------

#[test]
fn the_cache_is_off_where_it_cannot_help_or_must_not_interfere() {
    let src = format!("let T = [[1, 2], [3, 4]]\n{COUNTER}print(n, T)\n");
    let lowered = |setup: &dyn Fn(&mut Env)| {
        let mut w = world_with(&src, RunPolicy::FAST, |_| {});
        setup(&mut w.env);
        frame(&mut w);
        frame(&mut w);
        let text = w.env.bytecode_text(w.pid).unwrap();
        (w.env.static_cache_enabled(), text.contains("static_skip"), w.env.static_values(w.sid).unwrap().stats.skips)
    };
    assert_eq!(lowered(&|_| {}), (true, true, 1), "on by default");
    assert_eq!(lowered(&|e| e.set_policy(RunPolicy::BASELINE)), (false, false, 0));
    assert_eq!(lowered(&|e| e.set_policy(RunPolicy::EXPLAIN)), (false, false, 0));
    assert_eq!(lowered(&|e| e.set_policy(RunPolicy::parse("fast-static").unwrap())), (false, false, 0));
    assert_eq!(lowered(&|e| e.set_static_cache(false)), (false, false, 0));
    assert_eq!(lowered(&|e| e.observations_mut().enable()), (false, false, 0), "observations are preserved");
    assert_eq!(lowered(&|e| e.trace_mut().enable()), (false, false, 0), "the per-term trace is preserved");
    assert_eq!(
        lowered(&|e| e.set_static_audit(petal::static_audit::StaticAuditMode::Watch)),
        (false, false, 0),
        "the static audit watches static code run"
    );
    // The optimizer is not what it depends on.
    assert_eq!(lowered(&|e| e.set_policy(RunPolicy::parse("baseline+static").unwrap())), (true, true, 1));

    // Switching it mid-session re-lowers, and the stack starts over.
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    frame(&mut w);
    w.env.set_static_cache(false);
    assert!(frame(&mut w).starts_with("3 [[1, 2], [3, 4]]"));
    w.env.set_static_cache(true);
    assert!(frame(&mut w).starts_with("4 [[1, 2], [3, 4]]"));
    assert!(frame(&mut w).starts_with("5 [[1, 2], [3, 4]]"));
    let stats = w.env.static_values(w.sid).unwrap().stats;
    assert_eq!((stats.skips, stats.clears), (2, 1));
}

/// `petal run` runs a script once: nothing kept would ever be read. With
/// `--runs` it is a frame loop.
#[test]
fn petal_run_leaves_the_cache_off_unless_it_runs_more_than_once() {
    let dir = std::env::temp_dir().join(format!("petal-static-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("t.ptl");
    std::fs::write(&file, "let T = [[1, 2], [3, 4]]\nprint(T)\n").unwrap();
    let petal = env!("CARGO_BIN_EXE_petal");
    let show = |policy: Option<&str>| {
        let mut cmd = std::process::Command::new(petal);
        cmd.arg("show-bytecode").arg(&file).env_remove("PETAL_POLICY").env_remove("PETAL_OPT");
        if let Some(p) = policy {
            cmd.env("PETAL_POLICY", p);
        }
        String::from_utf8(cmd.output().unwrap().stdout).unwrap()
    };
    assert!(!show(None).contains("static_skip"));
    assert!(show(Some("fast")).contains("static_skip"));
    assert!(!show(Some("fast-static")).contains("static_skip"));
    assert!(!show(Some("baseline")).contains("static_skip"));

    // `--runs n` is a host's frame loop on one stack: state carries, and
    // the cache (on by default there) changes nothing that is printed.
    std::fs::write(&file, "let T = [[1, 2], [3, 4]]\nstate n = 0\nn = n + 1\nprint(n, T)\n").unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(petal)
            .arg("run")
            .args(args)
            .arg(&file)
            .env_remove("PETAL_POLICY")
            .env_remove("PETAL_OPT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let three = "1 [[1, 2], [3, 4]]\n2 [[1, 2], [3, 4]]\n3 [[1, 2], [3, 4]]\n";
    assert_eq!(run(&["--runs", "3"]), three);
    assert_eq!(run(&["--runs", "3", "--policy", "replay-static"]), three);
    assert_eq!(run(&["--runs", "3", "--policy", "baseline"]), three);
    assert_eq!(run(&[]), "1 [[1, 2], [3, 4]]\n");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- what may be kept -------------------------------------------------------

fn lying_pending(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_value(Value::Pending(petal::value::PendingId(7)));
    Ok(1)
}

/// A value that holds a pending result is not kept, however the native that
/// made it was registered: the run it belongs to executes on every run and
/// the table says which declaration and why.
#[test]
fn a_value_that_cannot_be_kept_is_rejected_and_reported() {
    let src = format!(
        "native fn later()\nlet GOOD = [[1, 2], [3, 4]]\nlet BAD = [[1], [later()]]\n{COUNTER}print(n, GOOD, len(BAD[n % 2]))\n"
    );
    let setup = |env: &mut Env| {
        env.register_native("later", lying_pending, NativeEffects::CONST);
    };
    let mut on = world_with(&src, RunPolicy::REPLAY, setup);
    let mut off = world_with(&src, RunPolicy::REPLAY.with_static_cache(false), setup);
    for i in 0..4 {
        assert_eq!(frame(&mut on), frame(&mut off), "frame {i}");
    }
    let values = on.env.static_values(on.sid).unwrap();
    assert_eq!(values.stats.skips, 0, "the run with the pending value never skips");
    assert_eq!(
        values.warnings(),
        ["BAD (line 3) holds a pending result, so it is rebuilt on every run"]
    );
    let good = values.slots.iter().find(|(i, _)| i.name.as_deref() == Some("GOOD")).unwrap();
    assert_eq!(good.1, StaticSlotState::NotKept, "built by the same run");
    assert_eq!(values.stats.rejected, 2);
}

/// The guard of last resort: whatever reaches a kept value, an in-place
/// heap operation on it copies.
#[test]
fn an_in_place_write_to_a_kept_value_copies() {
    let src = format!("let T = [[1, 2], [3, 4]]\n{COUNTER}print(n, T)\n");
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    let values = w.env.static_values(w.sid).unwrap();
    let StaticSlotState::Filled(Value::List(outer)) = values.slots[0].1 else { panic!("T is kept") };
    let inner = match w.env.heap().get_list(outer)[0] {
        Value::List(l) => l,
        other => panic!("{other:?}"),
    };
    let heap = w.env.heap_mut();
    assert_ne!(heap.list_append_in_place(outer, Value::Int(9)), outer);
    assert_ne!(heap.list_set_in_place(inner, 0, Value::Int(9)), inner, "containers under it too");
    assert_ne!(heap.list_drop_last_in_place(inner), inner);
    assert!(frame(&mut w).starts_with("2 [[1, 2], [3, 4]]"));
}

// --- `const` and the lint ----------------------------------------------------

/// A `const` is checked to be static when it is compiled and kept like the
/// `let` it is; the classifier that answers both runs once.
#[test]
fn a_const_is_classified_once_and_kept() {
    let src = format!(
        "fn row(n)\n  {{n: n, sq: n * n}}\nend\nconst ROWS = [row(1), row(2), row(3)]\nlet PLAIN = [[1, 2], [3, 4]]\n{COUNTER}print(n, ROWS[n % 3].sq, PLAIN)\n"
    );
    let before = petal::static_classify::analyses_run();
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    frame(&mut w);
    assert_eq!(
        petal::static_classify::analyses_run() - before,
        1,
        "the `const` check and the lowering share one analysis"
    );
    let values = w.env.static_values(w.sid).unwrap();
    let slot = |name: &str| values.slots.iter().find(|(i, _)| i.name.as_deref() == Some(name)).unwrap().clone();
    assert!(slot("ROWS").0.is_const && matches!(slot("ROWS").1, StaticSlotState::Filled(_)));
    assert!(!slot("PLAIN").0.is_const);
    assert!(values.stats.skips >= 1);

    // A native registered after the compile may change what is constant,
    // so the kept analysis is not served to a lowering that follows it.
    let before = petal::static_classify::analyses_run();
    w.env.register_native("later", lying_pending, NativeEffects::CONST);
    w.env.set_static_cache(false);
    frame(&mut w);
    w.env.set_static_cache(true);
    frame(&mut w);
    assert_eq!(petal::static_classify::analyses_run() - before, 1, "classified again");

    // Without a `const` nothing is classified at compile time.
    let before = petal::static_classify::analyses_run();
    let mut plain = world(&src.replace("const ", "let "), RunPolicy::REPLAY.with_static_cache(false));
    frame(&mut plain);
    assert_eq!(petal::static_classify::analyses_run() - before, 0);
}

/// What `petal lint --fix --rules-include=state-as-constant` writes is kept:
/// the cell that was seeded once per stack becomes a value built once per
/// load.
#[test]
fn the_lint_rewrite_of_a_state_table_is_kept() {
    use petal::lint::{LintOptions, RuleFilter, STATE_AS_CONSTANT, lint_source};
    let src = format!(
        "state TILES = [\n  {{name: \"grass\", cost: 1}},\n  {{name: \"road\", cost: 0}},\n]\nstate RAW = [3, 1, 2]\nstate ORDER = sort(RAW)\n{COUNTER}print(n, TILES[n % 2].name, ORDER)\n"
    );
    let options = LintOptions {
        rules: RuleFilter { include: Some(vec![STATE_AS_CONSTANT.to_string()]), exclude: Vec::new() },
        ..LintOptions::default()
    };
    let fixed = lint_source(&src, &options).unwrap().output;
    assert!(fixed.contains("const TILES = [") && fixed.contains("const ORDER = sort(RAW)"), "{fixed}");

    // The same prints before and after the rewrite, cache on and off.
    let (mut was, mut on, mut off) = (
        world(&src, RunPolicy::REPLAY),
        world(&fixed, RunPolicy::REPLAY),
        world(&fixed, RunPolicy::REPLAY.with_static_cache(false)),
    );
    let prints = |f: String| f.split("\n=>").next().unwrap().to_string();
    for i in 0..4 {
        let (a, b, c) = (frame(&mut was), frame(&mut on), frame(&mut off));
        assert_eq!(b, c, "frame {i}");
        assert_eq!(prints(a), prints(b), "frame {i}: the rewrite prints what the cells did");
    }
    let values = on.env.static_values(on.sid).unwrap();
    for name in ["TILES", "ORDER"] {
        let (info, state) = values.slots.iter().find(|(i, _)| i.name.as_deref() == Some(name)).unwrap();
        assert!(info.is_const && matches!(state, StaticSlotState::Filled(_)), "{name}: {state:?}");
    }
    assert!(values.stats.skips >= 3);
    assert!(values.warnings().is_empty());
}

/// A `const` passes its check when it is static, and a static value can
/// still be one the stack cannot keep. It is then built on every run, and
/// the host is told once per load, in words that say it was a `const`.
#[test]
fn a_const_that_cannot_be_kept_warns_once_per_load() {
    let src = format!(
        "native fn later()\nconst BAD = [[1], [later()]]\nlet ALSO = [[2], [later()], [3]]\n{COUNTER}print(n, len(BAD[n % 2]), len(ALSO))\n"
    );
    let setup = |env: &mut Env| {
        env.register_native("later", lying_pending, NativeEffects::CONST);
    };
    let mut on = world_with(&src, RunPolicy::REPLAY, setup);
    let mut off = world_with(&src, RunPolicy::REPLAY.with_static_cache(false), setup);
    assert_eq!(frame(&mut on), frame(&mut off));
    let warnings = on.env.take_static_warnings(on.sid);
    assert!(
        warnings.contains(
            &"const BAD (line 2) holds a pending result, which cannot be kept between runs, so it is rebuilt on every run"
                .to_string()
        ),
        "{warnings:?}"
    );
    assert!(off.env.take_static_warnings(off.sid).is_empty());
    for i in 0..3 {
        assert_eq!(frame(&mut on), frame(&mut off), "frame {i}");
        assert!(on.env.take_static_warnings(on.sid).is_empty(), "said once");
    }
    // After a reload the table is new, and so is the warning.
    let edited = src.replace("[[1], [later()]]", "[[1], [later()], [4]]");
    on.env.reload_program(on.sid, &edited, None).unwrap();
    run_frame(&mut on);
    assert_eq!(on.env.take_static_warnings(on.sid).len(), warnings.len());
}

// --- enum constant data -----------------------------------------------------

const KINDS: &str = "enum Kind\n  road(code: 1, label: \"Road\", cost: [10, 20]),\n  park(code: 2, label: \"Park\", cost: [5, 5 + 1]),\n  lake(code: 3, label: \"Lake\", cost: []),\nend\n";

/// A variant's constant data is published into the stack's enum table by a
/// call the declaration makes on every run. Where the data is static the
/// record is a static value: published by the first run and left where it
/// is by the rest.
#[test]
fn static_enum_data_is_published_once() {
    let src = format!(
        "{KINDS}{COUNTER}let k = Kind.all[n % 3]\nprint(n, k.code, k.label, k.cost, Kind.park.cost[1], Kind.from(\"lake\").code)\n"
    );
    let (frames, skips) = same_on_and_off(&src, 5);
    assert!(frames[0].starts_with("1 2 Park [5, 6] 6 3"), "{}", frames[0]);
    assert!(frames[4].starts_with("5 3 Lake [] 6 3"), "{}", frames[4]);
    assert!(skips >= 4, "the declarations ran once: {skips}");

    let (mut on, mut off) = (world(&src, RunPolicy::REPLAY), world(&src, RunPolicy::REPLAY.with_static_cache(false)));
    for w in [&mut on, &mut off] {
        frame(w);
        frame(w);
    }
    let spent = |w: &mut World| {
        let before = w.env.instructions_retired(w.sid).unwrap();
        frame(w);
        w.env.instructions_retired(w.sid).unwrap() - before
    };
    let (with, without) = (spent(&mut on), spent(&mut off));
    assert!(with + 9 <= without, "three declarations fewer a frame: {with} against {without}");

    let values = on.env.static_values(on.sid).unwrap();
    let names: Vec<&str> = values.slots.iter().filter_map(|(i, _)| i.name.as_deref()).collect();
    for tag in ["Kind.road", "Kind.park", "Kind.lake"] {
        let name = format!("enum data of {tag}");
        assert!(names.contains(&name.as_str()), "{name} in {names:?}");
    }
    for (info, state) in &values.slots {
        let StaticSlotState::Filled(v) = state else { panic!("{info:?} is {state:?}") };
        if info.name.as_deref().is_some_and(|n| n.starts_with("enum data of")) {
            assert!(on.env.heap().is_immutable(*v), "a kept record is marked");
        }
    }
}

/// Data that is not static is published on every run, as before, and the
/// variants beside it are still kept.
#[test]
fn dynamic_enum_data_is_published_every_run() {
    let src = format!(
        "fn seen()\n  state s = 0\n  s = s + 2\n  s\nend\n{COUNTER}\
         enum Lamp\n  off(level: 0, tint: [0, 0]),\n  on(level: seen() * 10, tint: [seen(), 1]),\n  dim(level: 5, tint: [2, 2]),\nend\n\
         for l in Lamp.all do print(n, l, l.level, l.tint) end\n"
    );
    let (frames, _) = same_on_and_off(&src, 4);
    assert!(frames[3].contains("4 Lamp.on 80 [8, 1]"), "{}", frames[3]);
    assert!(frames[3].contains("4 Lamp.dim 5 [2, 2]"), "{}", frames[3]);
}

/// A write through a value read out of kept enum data must not reach the
/// record, whichever route the write takes.
#[test]
fn kept_enum_data_cannot_be_written_through() {
    let src = format!(
        "{KINDS}{COUNTER}fn bump(xs)\n  xs[0] = xs[0] + 100\n  xs\nend\n\
         let a = Kind.road.cost\nlet b = bump(a)\nlet c = push(Kind.park.cost, n)\n\
         var d = Kind.road.cost\nset d[1] = n\n\
         print(n, a, b, c, d, Kind.road.cost, Kind.park.cost)\n"
    );
    let (frames, skips) = same_on_and_off(&src, 4);
    assert!(
        frames[3].starts_with("4 [10, 20] [110, 20] [5, 6, 4] [10, 4] [10, 20] [5, 6]"),
        "{}",
        frames[3]
    );
    assert!(skips >= 3);
}

/// An edit to a literal inside enum data is patched in place and recompiles
/// nothing; the kept record goes with the rest of the table and the next
/// run publishes the new one. A structural edit likewise, and a variant in
/// `state` reads the new data either way.
#[test]
fn a_reload_republishes_enum_data() {
    let old = format!(
        "{KINDS}{COUNTER}state held = Kind.park\nprint(n, held.code, held.cost, Kind.road.label, for k in Kind.all do k.code end)\n"
    );
    let literal = old.replace("code: 2", "code: 20").replace("\"Road\"", "\"Street\"");
    let structural = old.replace("cost: [5, 5 + 1]", "cost: [5, 5 + 1, 7]");
    for (edit, new, expect) in [
        ("literal", &literal, "3 20 [5, 6] Street [1, 20, 3]"),
        ("structural", &structural, "3 2 [5, 6, 7] Road [1, 2, 3]"),
    ] {
        for (on, off) in pairs() {
            let (mut a, mut b) = (world(&old, on), world(&old, off));
            for _ in 0..2 {
                assert_eq!(frame(&mut a), frame(&mut b));
            }
            let before = a.env.static_values(a.sid).unwrap().stats;
            assert!(before.skips > 0, "{edit}: the data was kept before the edit");
            for w in [&mut a, &mut b] {
                let report = w.env.reload_program(w.sid, new, None).unwrap();
                let patched = report.outcome == petal::env::ReloadOutcome::Patched;
                assert_eq!(patched, edit == "literal", "{edit}: {:?}", report.outcome);
            }
            let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
            assert_eq!(fa, fb, "{edit}: the run after the reload");
            assert!(fa.starts_with(expect), "{edit}: {fa}");
            for i in 0..3 {
                assert_eq!(frame(&mut a), frame(&mut b), "{edit}: frame {i} after the reload");
            }
            assert!(a.env.static_values(a.sid).unwrap().stats.skips > before.skips, "{edit}: kept again");
        }
    }
}

/// Two stacks of one program each publish and keep their own records, and
/// a fork carries its parent's.
#[test]
fn enum_data_is_kept_per_stack() {
    let src = format!("{KINDS}{COUNTER}print(n, Kind.all[n % 3].cost)\n");
    let mut w = world(&src, RunPolicy::REPLAY);
    let other = w.env.create_stack(w.pid).unwrap();
    frame(&mut w);
    frame(&mut w);
    w.env.run(other).unwrap();
    assert_eq!(w.env.take_output_for(other), ["1 [5, 6]"]);
    let fork = w.env.fork_execution(w.sid).unwrap();
    w.env.reset_stack(fork).unwrap();
    w.env.run(fork).unwrap();
    assert_eq!(w.env.take_output_for(fork).last().unwrap(), "3 [10, 20]");
    assert!(w.env.static_values(fork).unwrap().stats.skips > w.env.static_values(other).unwrap().stats.skips);
    w.env.drop_fork(fork);
    w.env.clear_static_values(w.sid);
    assert!(frame(&mut w).starts_with("3 [10, 20]"));
    assert!(frame(&mut w).starts_with("4 [5, 6]"));
}

// --- the benchmark the plan names -------------------------------------------

/// A 400-record `let` table handed to a memoized function. Rebuilt every
/// frame it is a new object each time, too big for the memo to compare
/// (512 nodes), so the call misses on every frame. Kept, it is the same
/// object, and the call is replayed.
#[test]
fn a_large_table_as_a_memo_argument_stops_missing() {
    let rows: String = (0..400).map(|i| format!("  {{id: {i}, w: {}}},\n", i % 7)).collect();
    let src = format!(
        "let TABLE = [\n{rows}]\nfn weigh(rows, k)\n  let t = 0\n  for r in rows do t = t + r.w * k end\n  t\nend\n{COUNTER}print(n, weigh(TABLE, 1 + n * 0))\n"
    );
    let run = |policy: RunPolicy| {
        let mut w = world(&src, policy);
        let first = frame(&mut w);
        let warm = w.env.memo_stats(w.sid).unwrap();
        for _ in 0..29 {
            frame(&mut w);
        }
        let end = w.env.memo_stats(w.sid).unwrap();
        (first, end.misses - warm.misses, end.hits - warm.hits)
    };
    let (out_on, misses_on, hits_on) = run(RunPolicy::REPLAY);
    let (out_off, misses_off, _) = run(RunPolicy::REPLAY.with_static_cache(false));
    assert_eq!(out_on, out_off);
    assert_eq!(misses_on, 0, "with the table kept the call is replayed on every frame");
    assert_eq!(hits_on, 29);
    assert_eq!(misses_off, 29, "rebuilt, it misses on every frame");
}

// --- the example corpus ------------------------------------------------------

/// Every console example and every small script under `test/`, three runs
/// on one stack, with the cache on and off under each policy pair. These
/// are the scripts `tools/test-examples.ts` holds against the golden
/// corpus; that tool runs the optimizer-on pair through the CLI.
#[test]
fn the_example_corpus_prints_the_same_with_the_cache_on_and_off() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut walk = vec![root.join("examples/console"), root.join("test")];
    while let Some(dir) = walk.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                // The benchmarks take seconds a run unoptimized; vitest
                // fixtures are fragments, not programs.
                if !matches!(name.as_str(), "benchmarks" | "vitest" | "example-golden" | "ui-golden") {
                    walk.push(path);
                }
            } else if name.ends_with(".ptl") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() >= 50, "found only {} scripts", files.len());

    let (mut ran, mut kept, mut skips) = (0, 0, 0u64);
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        let load = |policy: RunPolicy| {
            let mut env = Env::new();
            env.set_policy(policy);
            env.set_echo(false);
            env.set_seed(1);
            let pid = env.load_program(&src).ok()?;
            let sid = env.create_stack(pid).ok()?;
            Some(World { env, pid, sid })
        };
        let mut compiled = false;
        for (on, off) in pairs() {
            let (Some(mut a), Some(mut b)) = (load(on), load(off)) else { break };
            compiled = true;
            for i in 0..3 {
                let (fa, fb) = (frame(&mut a), frame(&mut b));
                assert_eq!(fa, fb, "{}: run {i} differs with the cache on ({:?})", file.display(), on.name());
            }
            if on == RunPolicy::REPLAY {
                let values = a.env.static_values(a.sid).unwrap();
                assert_eq!(values.warnings(), Vec::<String>::new(), "{}", file.display());
                kept += (!values.slots.is_empty()) as usize;
                skips += values.stats.skips;
            }
        }
        ran += compiled as usize;
    }
    assert!(ran * 10 >= files.len() * 9, "only {ran} of {} scripts compile here", files.len());
    assert!(kept >= 10 && skips > 0, "{kept} scripts keep a static value, {skips} runs skipped");
    eprintln!("{ran} of {} scripts ran; {kept} keep a static value; {skips} runs skipped", files.len());
}
