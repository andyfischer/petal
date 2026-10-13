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
    format!("{out}\n=> {result}\nstate {state}{}", observed(w))
}

/// What a host that observes reads after a frame, in both of the ways one
/// can: the map by qualified name (as `Env::get_observations_json` orders
/// it), and the buffer itself term by term. Nothing when observation is
/// off.
fn observed(w: &World) -> String {
    if !w.env.observations().enabled {
        return String::new();
    }
    let by_name = serde_json::Value::Object(w.env.get_observations_json(w.pid, w.sid));
    let heap = w.env.heap_for(w.sid).unwrap();
    let mut by_term: Vec<(u32, String)> = w
        .env
        .observations()
        .iter()
        .map(|(t, v)| (t.0, petal::value::value_to_json(&v, heap).to_string()))
        .collect();
    by_term.sort();
    format!("\nobserved {by_name}\nterms {by_term:?}")
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
    // Each pair again with observations on: a frame that skips a segment
    // must report every named value the frame that ran it reports.
    for (observe, (on, off)) in [false, true].into_iter().flat_map(|o| pairs().into_iter().map(move |p| (o, p))) {
        let watch = |e: &mut Env| {
            if observe {
                e.observations_mut().enable()
            }
        };
        let (mut a, mut b) = (world_with(src, on, watch), world_with(src, off, watch));
        let mut out = Vec::new();
        for i in 0..frames {
            let (fa, fb) = (frame(&mut a), frame(&mut b));
            assert_eq!(
                fa, fb,
                "frame {i} differs with the cache on ({:?}, observations {observe})\n{src}",
                on.name()
            );
            out.push(fa);
        }
        assert!(a.env.static_cache_enabled(), "the cache is on (observations {observe})");
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

/// The value the slot of declaration `name` holds, if it holds one.
fn held(w: &World, name: &str) -> Option<Value> {
    let values = w.env.static_values(w.sid)?;
    values.slots.iter().find_map(|(info, state)| match state {
        StaticSlotState::Filled(v) if info.name.as_deref() == Some(name) => Some(*v),
        _ => None,
    })
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
    assert_eq!(frames[0], frames[2]);
    // `A` was stored before the binding raised, and is not built again;
    // the binding and what follows it are, on every run.
    assert_eq!(skips, 2, "one value kept, on each of the two later runs");
}

/// A function declaration ends a stretch (its closure is made on every run,
/// where it stands), so a table that stands alone between two of them is a
/// stretch of one allocation. It is kept all the same: one check a frame is
/// less than the allocation.
#[test]
fn a_single_table_between_two_function_declarations_is_kept() {
    let src = format!(
        "fn sq(x) x * x end\nlet K = sq(2)\nfn g() K end\nlet T = [K, 1]\nfn h() T[0] end\n\
         let U = {{k: K}}\nfn i() U.k end\nlet N = K + 1\nfn j() N end\n\
         {COUNTER}print(n, g(), h(), i(), j(), T, U)\n"
    );
    let (frames, _) = same_on_and_off(&src, 3);
    assert!(frames[2].starts_with("3 4 4 4 5 [4, 1]"), "{}", frames[2]);
    for policy in ["replay", "baseline+static"] {
        let mut w = world(&src, RunPolicy::parse(policy).unwrap());
        frame(&mut w);
        let (t, u) = (held(&w, "T"), held(&w, "U"));
        assert!(t.is_some() && u.is_some(), "{policy}: one allocation is enough");
        // A number alone is not worth a check: loading it costs what the
        // test costs.
        assert!(held(&w, "N").is_none(), "{policy}");
        frame(&mut w);
        assert_eq!((held(&w, "T"), held(&w, "U")), (t, u), "{policy}: the same objects on the next frame");
    }
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
    // Observations do not turn it off: a skipped segment reports what it
    // observed when it ran (`a_skipped_segment_reports_what_it_observed`).
    assert_eq!(lowered(&|e| e.observations_mut().enable()), (true, true, 1), "observations are preserved");
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
/// made it was registered: the code that builds it executes on every run and
/// the table says which declaration and why. A value built beside it is
/// stored on its own and is kept.
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
    assert_eq!(values.stats.skips, 3, "only `GOOD` is skipped, on each later run");
    assert_eq!(
        values.warnings(),
        ["BAD (line 3) holds a pending result, so it is rebuilt on every run"]
    );
    let good = values.slots.iter().find(|(i, _)| i.name.as_deref() == Some("GOOD")).unwrap();
    assert!(matches!(good.1, StaticSlotState::Filled(_)), "stored on its own: {:?}", good.1);
    assert_eq!(values.stats.rejected, 1);
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

// --- the static part of a frame, alone ---------------------------------------

/// The audit's static-only build jumps over every statement that is not
/// static, so it runs a script whose host is not here. With
/// `set_static_only_keeps` it is lowered with the cache as well: the static
/// part of a frame as a host with the cache runs it, which is how the cache
/// is measured on such a script (`core/examples/static_frame.rs`).
#[test]
fn the_static_only_build_can_keep_its_values() {
    use petal::static_audit::StaticAuditMode;
    let src = "native fn host_clock()\nnative fn host_draw(rows, t)\n\
               fn row(i)\n  {id: i, cells: [i, i * 2, i * 3]}\nend\n\
               let ROWS = for i in range(0, 20) do row(i) end\n\
               let t = host_clock()\n\
               let WIDE = map(ROWS, fn(r) -> len(r.cells))\n\
               host_draw(ROWS, t)\n\
               assert(len(WIDE) == 20)\n";
    let spent = |keeps: bool| {
        let mut env = Env::new();
        env.set_echo(false);
        env.set_check_only(true);
        env.set_policy(RunPolicy::FAST);
        env.set_static_audit(StaticAuditMode::StaticOnly);
        env.set_static_only_keeps(keeps);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let mut frames = Vec::new();
        for _ in 0..3 {
            env.reset_stack(sid).unwrap();
            let before = env.instructions_retired(sid).unwrap();
            env.run(sid).unwrap();
            frames.push(env.instructions_retired(sid).unwrap() - before);
        }
        assert!(!env.static_audit_report().has_failures());
        let values = env.static_values(sid).unwrap_or_default();
        (frames, values.slots.len(), values.stats.skips)
    };
    let (plain, slots, _) = spent(false);
    assert_eq!(slots, 0, "the audit runs with the cache off");
    assert_eq!(plain[1], plain[2]);
    let (kept, slots, skips) = spent(true);
    assert!(slots >= 2 && skips >= 2, "{slots} slots, {skips} skips");
    assert!(kept[0] >= plain[0], "the first frame builds everything, and stores it");
    assert_eq!(kept[1], kept[2]);
    assert!(kept[1] * 4 < plain[1], "later frames skip the tables: {kept:?} against {plain:?}");
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

// --- review: more ways to reach a kept value, or to need a skipped run -------

/// Every route found for writing through to a value the stack keeps, in one
/// place: a parameter written by a user function (flat, nested, through a
/// record), a loop variable, a match binding, a captured table, a `var`
/// cell, `state`, the callback of a higher-order builtin, f64 arrays, enum
/// payloads, `K.all` and the constant data of a variant. Each prints the
/// kept value after the write.
#[test]
fn no_route_writes_through_to_a_kept_value() {
    let cases = [
        // User functions that write their parameter, at three depths.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nlet REC = {a: [1, 2], b: {c: [3, 4]}, d: \"x\"}\n\
         fn bump(xs, v)\n  xs[0] = v\n  push(xs, v)\nend\n\
         fn deep(r, v)\n  r.b.c[0] = v\n  r.a = push(r.a, v)\n  r\nend\n\
         fn inner_edit(t, v)\n  let row = t[1]\n  row[0] = v\n  let t2 = t\n  t2[2] = row\n  t2\nend\n\
         print(n, bump(TABLE, n), TABLE)\nprint(deep(REC, n), REC)\nprint(inner_edit(TABLE, n), TABLE)\n",
        // A loop variable and a match binding are elements of the kept table.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\n\
         for row in TABLE do\n  row[0] = row[0] + n\n  let r2 = push(row, n)\n  print(row, r2)\nend\n\
         match TABLE\n  when [a, b, c] -> print(push(a, n), drop_last(b), TABLE)\n  when _ -> print(\"no\")\nend\nprint(TABLE)\n",
        // A closure that captured it, a cell, state, and builtin callbacks.
        "let TABLE = [[1, 2], [3, 4], [5, 6]]\nlet REC = {a: [1, 2], b: {c: [3, 4]}}\n\
         let f = fn(v) push(TABLE, v) end\nprint(f(n), TABLE)\n\
         var cell = REC\nset cell.a = [n]\nset cell.b.c[1] = n\nprint(get cell, REC)\n\
         state held = TABLE\nheld[0] = [n]\nheld = push(held, n)\nprint(len(held), TABLE)\n\
         let m = map(TABLE, fn(r) push(r, n) end)\nlet s = sort_by(TABLE, fn(r) -> 0 - r[0])\nprint(m, s, reverse(TABLE), TABLE)\n",
        // Tables built from each other share their rows.
        "let A = [[1], [2], [3]]\nlet B = map(A, fn(r) -> [r, r])\nlet C = {a: A, b: B, n: len(B)}\n\
         fn poke(x, v)\n  x[0][0] = v\n  x\nend\nprint(n, poke(A, n), poke(B[0], n), A, B)\n\
         var c = C\nset c.a[0][0] = n\nset c.b = push(get c.b, n)\nprint(get c, C)\n\
         state st = C\nst.a[1] = [n]\nst.b[0][0] = [n]\nprint(st.a, C)\n\
         let cl = fn() -> C\nlet cc = cl()\ncc.a = n\nprint(cc, C)\n\
         let big = for i in range(0, 3) do A end\nlet big2 = for b in big do push(b, n) end\nprint(big2, A)\n",
        // f64 arrays, whose only writes are in place when they can be.
        "fn mk(k)\n  let a = f64_array(4)\n  for i in range(0, 4) do\n    a = set_at(a, i, i * k)\n  end\n  a\nend\n\
         let ARR = mk(2)\nlet PAIR = [mk(1), mk(3)]\nfn fill(a, v)\n  set_at(a, 0, v)\nend\nfn sw(a)\n  swap(a, 0, 3)\nend\n\
         print(n, fill(ARR, n), sw(ARR), ARR)\nlet p0 = PAIR[0]\nlet q = set_at(p0, 1, n)\nlet q2 = swap(PAIR[1], 1, 2)\nprint(q, q2, PAIR)\n",
        // Enum payloads, `K.all`, and the data of a variant.
        "enum Shape\n  circle(r),\n  rect(w, h),\n  none\nend\n\
         enum Kind\n  road(code: 1, cost: [10, 20], tags: {a: [1]}),\n  park(code: 2, cost: [5, 6], tags: {a: [2]}),\nend\n\
         let SHAPES = [Shape.circle([1, 2]), Shape.rect([3], [4]), Shape.none]\n\
         fn grow(s, v)\n  match s\n    when Shape.circle(r) -> Shape.circle(push(r, v))\n    when Shape.rect(w, h) -> Shape.rect(push(w, v), drop_last(h))\n    when Shape.none -> Shape.none\n  end\nend\n\
         for s in SHAPES do\n  print(n, grow(s, n), s)\nend\n\
         match SHAPES[0]\n  when Shape.circle(r) -> print(push(r, n), drop_last(r), r)\n  when _ -> print(\"x\")\nend\nprint(SHAPES)\n\
         let ks = push(Kind.all, n)\nprint(ks, Kind.all)\n\
         for k in Kind.all do\n  let c = k.cost\n  c[0] = n\n  let t = k.tags\n  t.a = push(t.a, n)\n  t.b = n\n  print(k, c, t, k.cost, k.tags)\nend\n\
         let kk = Kind.from(\"park\")\nprint(kk.cost, drop_last(kk.cost), kk.cost)\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        let (frames, skips) = same_on_and_off(&src, 5);
        assert!(!frames[4].contains("=> error"), "the case runs: {}", frames[4]);
        assert!(skips > 0, "something was kept:\n{src}");
    }
}

/// A kept table directly in front of each construct dynamic code can open
/// with: nothing of the construct's own setup may be taken for part of the
/// run before it.
#[test]
fn a_run_ends_where_the_dynamic_statement_after_it_begins() {
    let src = format!(
        "{COUNTER}let xs = [n, n + 1, n + 2]\n\
         let T1 = [[1, 2], [3, 4]]\nfor x in xs do print(\"fe\", x, T1) end\n\
         let T2 = [[1, 2], [3, 5]]\nfor i in range(n, n + 2) do print(\"fr\", i, T2) end\n\
         let T3 = [[1, 2], [3, 6]]\nwhile len(xs) < 5 do\n  xs = push(xs, len(T3))\nend\n\
         let T4 = [[1, 2], [3, 7]]\nif n % 2 == 0 then print(\"even\", T4) else print(\"odd\", T4) end\n\
         let T5 = [[1, 2], [3, 8]]\nmatch n % 3\n  when 0 -> print(\"zero\", T5)\n  when 1 -> print(\"one\", T5)\n  when _ -> print(\"many\", T5)\nend\n\
         let T6 = [[1, 2], [3, 9]]\nlet a = n > 2 && T6\n\
         let T7 = [[1, 2], [3, 10]]\nlet b = n > 2 || T7\n\
         let T8 = [[1, 2], [3, 11]]\nlet c = (if n > 3 then nil else n end) ?? T8\nprint(a, b, c, xs)\n\
         let T9 = [[1, 2], [3, 12]]\nlet sq = for x in xs do [x, T9[0]] end\n\
         let T10 = [[1, 2], [3, 13]]\nlet w = for x in xs do\n  if x % 2 == 0 then continue end\n  [x, T10]\nend\n\
         let T11 = [[1, 2], [3, 14]]\nvar v = n\nset v = get v + len(T11)\n\
         let T12 = [[1, 2], [3, 15]]\nstate s2 = T12\n\
         let T13 = [[1, 2], [3, 16]]\nfn f(q) [q, T13] end\n\
         let T14 = [[1, 2], [3, 17]]\nprint(sq, w, get v, s2, f(n), T14)\n"
    );
    let (frames, skips) = same_on_and_off(&src, 7);
    assert!(!frames[6].contains("=> error"), "{}", frames[6]);
    assert!(skips > 0);
}

/// A frame that raises part of the way down has stored the runs above the
/// error and none below it. The frames after it, raising or not, are the
/// frames of the cache-off program.
#[test]
fn a_frame_that_raises_part_way_leaves_a_table_the_next_frame_can_use() {
    let src = format!(
        "{COUNTER}let XS = [[1], [2], [3]]\nlet YS = for x in XS do [x[0] * 2, [x]] end\nprint(\"before\", n)\n\
         let PICK = if n > 2 then XS[0] else XS[1] end\nlet Z = YS[5 - 5]\nlet W = [Z, YS]\n\
         if n % 3 == 1 then\n  let BOOM = XS[9]\nend\nprint(n, W, PICK)\n\
         let Q = [[W], [W]]\nassert(n % 5 != 0)\nlet LATE = [[Q], [XS]]\nprint(Q[n % 2], LATE)\n"
    );
    let (frames, skips) = same_on_and_off(&src, 9);
    assert!(frames[0].contains("=> error") && !frames[1].contains("=> error"), "{frames:?}");
    assert!(frames[4].contains("=> error") && frames[4].contains("ssert"), "{}", frames[4]);
    assert!(!frames[7].contains("=> error"), "{}", frames[7]);
    assert!(skips > 0);
}

/// Live tuning: `set_config_value` patches a literal in place. Every value
/// built from it is rebuilt by the next run, on every stack of the program,
/// whether the literal is in the declaration itself or in a function the
/// declaration calls.
#[test]
fn a_value_set_directly_rebuilds_what_was_built_from_it() {
    use petal::static_value::StaticValue;
    let src = format!(
        "config let GAP = 4\nconfig let STYLE = {{pad: 2, names: [\"a\", \"b\"]}}\n\
         fn cell(i)\n  {{at: i * GAP, pad: STYLE.pad}}\nend\n\
         let GRID = for i in range(0, 3) do cell(i) end\nlet LABELS = map(STYLE.names, fn(s) -> s ++ \"!\")\n\
         {COUNTER}print(n, GRID, LABELS)\n"
    );
    for (on, off) in pairs() {
        let (mut a, mut b) = (world(&src, on), world(&src, off));
        let other = a.env.create_stack(a.pid).unwrap();
        a.env.run(other).unwrap();
        a.env.take_output_for(other);
        for _ in 0..2 {
            assert_eq!(frame(&mut a), frame(&mut b));
        }
        let edits = [
            ("GAP", StaticValue::int(10)),
            ("STYLE.pad", StaticValue::int(7)),
            ("STYLE.names[1]", StaticValue::str("z")),
        ];
        for (path, value) in &edits {
            for w in [&mut a, &mut b] {
                w.env.set_config_value(w.sid, None, path, value).unwrap();
            }
            let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
            assert_eq!(fa, fb, "{path}: the run after the value was set ({:?})", on.name());
            assert_eq!(frame(&mut a), frame(&mut b), "{path}: the frame after that");
        }
        let last = frame(&mut a);
        assert_eq!(last, frame(&mut b));
        assert!(
            last.contains("{ at: 20, pad: 7 }") && last.contains("\"z!\"") || last.contains("at: 20, pad: 7") && last.contains("z!"),
            "{last}"
        );
        // The stack the values were not set through sees them too.
        a.env.reset_stack(other).unwrap();
        a.env.run(other).unwrap();
        let seen = a.env.take_output_for(other).join("\n");
        assert!(seen.contains("at: 20, pad: 7") && seen.contains("z!"), "{seen}");
    }
}

/// A literal inside the body of a function a kept value was built with is
/// patched in place like any other; the kept value goes with it.
#[test]
fn a_literal_edit_inside_a_function_a_kept_value_calls_rebuilds_it() {
    let old = format!(
        "fn row(i)\n  {{id: i, w: i * 10, tag: \"r\"}}\nend\nfn rows(n)\n  for i in range(0, n) do row(i) end\nend\n\
         let ROWS = rows(3)\nlet WIDE = map(ROWS, fn(r) -> r.w + 1)\n{COUNTER}print(n, ROWS, WIDE)\n"
    );
    let new = old.replace("i * 10", "i * 25").replace("\"r\"", "\"q\"");
    for (on, off) in pairs() {
        let (mut a, mut b) = (world(&old, on), world(&old, off));
        for _ in 0..2 {
            assert_eq!(frame(&mut a), frame(&mut b));
        }
        for w in [&mut a, &mut b] {
            let report = w.env.reload_program(w.sid, &new, None).unwrap();
            assert_eq!(report.outcome, petal::env::ReloadOutcome::Patched);
        }
        let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
        assert_eq!(fa, fb);
        assert!(fa.contains("w: 50") && fa.contains("51") && fa.contains("\"q\""), "{fa}");
        // And back, which is a second patch of the same slots.
        for w in [&mut a, &mut b] {
            w.env.reload_program(w.sid, &old, None).unwrap();
        }
        let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
        assert_eq!(fa, fb);
        assert!(fa.contains("w: 20") && fa.contains("\"r\""), "{fa}");
        assert_eq!(frame(&mut a), frame(&mut b));
    }
}

/// What `state` took from a kept value outlives the table that kept it: a
/// reload empties the table, the collector runs many times over, and the
/// cell still reads what it read. The rebuilt value is a new object, and
/// the old one is released when nothing else holds it.
#[test]
fn state_that_holds_part_of_a_kept_value_survives_the_table_being_emptied() {
    let old = format!(
        "let WORDS = [[\"alpha\", [1, 2]], [\"beta\", [3, 4]], [\"gamma\", [5, 6]]]\nlet BY = for w in WORDS do {{word: w[0], pair: w}} end\n\
         {COUNTER}state first = BY[0]\nstate picked = []\npicked = push(picked, WORDS[n % 3])\nif len(picked) > 4 then picked = [] end\n\
         let junk = for i in range(0, 20000) do [i, \"x{{i}}\", {{k: i}}] end\nprint(n, len(junk), first, picked, BY[n % 3])\n"
    );
    let new = old.replace("\"beta\", [3, 4]", "\"beta\", [3, 4, 9]").replace("word: w[0]", "word: w[0] ++ \"?\"");
    let (mut a, mut b) = (world(&old, RunPolicy::REPLAY), world(&old, RunPolicy::REPLAY.with_static_cache(false)));
    let before = a.env.heap().collections();
    for i in 0..4 {
        assert_eq!(frame(&mut a), frame(&mut b), "frame {i}");
    }
    let StaticSlotState::Filled(old_by) = a
        .env
        .static_values(a.sid)
        .unwrap()
        .slots
        .iter()
        .find(|(i, _)| i.name.as_deref() == Some("BY"))
        .unwrap()
        .1
        .clone()
    else {
        panic!("BY is kept")
    };
    for w in [&mut a, &mut b] {
        w.env.reload_program(w.sid, &new, None).unwrap();
    }
    assert_eq!(run_frame(&mut a), run_frame(&mut b), "the run after the reload");
    for i in 0..6 {
        assert_eq!(frame(&mut a), frame(&mut b), "frame {i} after the reload");
    }
    a.env.clear_static_values(a.sid);
    for i in 0..4 {
        assert_eq!(frame(&mut a), frame(&mut b), "frame {i} after the table was emptied by hand");
    }
    assert!(a.env.heap().collections() > before + 1, "the frames collected");
    let values = a.env.static_values(a.sid).unwrap();
    let StaticSlotState::Filled(new_by) =
        values.slots.iter().find(|(i, _)| i.name.as_deref() == Some("BY")).unwrap().1.clone()
    else {
        panic!("BY is kept again")
    };
    assert_ne!(old_by, new_by, "a rebuilt value is a new object");
    assert_eq!(values.stats.clears, 2);
}

/// A host calling a script function between frames runs it against the
/// values the last frame left, kept ones included, and cannot change them.
#[test]
fn a_host_call_between_frames_cannot_change_a_kept_value() {
    let src = format!(
        "let TABLE = [[1, 2], [3, 4]]\nfn poke(v)\n  let t = TABLE\n  t[0] = v\n  let row = TABLE[1]\n  row[0] = v\n  [push(TABLE, v), t, row]\nend\n\
         fn edit(xs, v)\n  xs[0][0] = v\n  xs\nend\n{COUNTER}print(n, TABLE, edit(TABLE, n))\n"
    );
    for (on, off) in pairs() {
        let (mut a, mut b) = (world(&src, on), world(&src, off));
        for i in 0..4 {
            assert_eq!(frame(&mut a), frame(&mut b), "frame {i}");
            let call = |w: &mut World| {
                let v = w.env.call_function(w.sid, "poke", &[Value::Int(90 + i)]).unwrap();
                petal::value::value_to_display_string(&v, w.env.heap())
            };
            let (ca, cb) = (call(&mut a), call(&mut b));
            assert_eq!(ca, cb);
            assert!(ca.starts_with("[[[1, 2], [3, 4], 9"), "{ca}");
        }
    }
}

/// Switching what the lowering preserves between frames (observations, the
/// audit, the optimizer, memoization) re-lowers the program. A table filled
/// under one lowering is never read by another.
#[test]
fn a_table_does_not_outlive_the_lowering_it_was_filled_under() {
    let src = format!(
        "let PAD = 2\nlet T = [[1, PAD], [3, PAD * 2]]\nlet U = for r in T do [r, r[1] + 1] end\n{COUNTER}print(n, T, U)\n"
    );
    let mut a = world(&src, RunPolicy::REPLAY);
    let mut b = world(&src, RunPolicy::BASELINE);
    let steps: [(&str, &dyn Fn(&mut Env)); 10] = [
        ("as loaded", &|_| {}),
        ("observations on", &|e| e.observations_mut().enable()),
        ("observations off", &|e| e.observations_mut().disable()),
        ("baseline+static", &|e| e.set_policy(RunPolicy::parse("baseline+static").unwrap())),
        ("fast", &|e| e.set_policy(RunPolicy::FAST)),
        ("audit on", &|e| e.set_static_audit(petal::static_audit::StaticAuditMode::Watch)),
        ("audit off", &|e| e.set_static_audit(petal::static_audit::StaticAuditMode::Off)),
        ("replay-memo", &|e| e.set_policy(RunPolicy::parse("replay-memo").unwrap())),
        ("cache off", &|e| e.set_static_cache(false)),
        ("cache on", &|e| e.set_static_cache(true)),
    ];
    for (what, step) in steps {
        step(&mut a.env);
        // What a host observes is part of a frame here.
        b.env.observations_mut().enabled = a.env.observations().enabled;
        for i in 0..2 {
            assert_eq!(frame(&mut a), frame(&mut b), "{what}: frame {i}");
        }
        let values = a.env.static_values(a.sid).unwrap();
        if a.env.static_cache_enabled() {
            assert_eq!(values.slots.iter().filter(|(_, s)| matches!(s, StaticSlotState::Filled(_))).count(), values.slots.len(), "{what}");
        } else {
            assert!(values.slots.is_empty(), "{what}: no slots with the cache off");
        }
    }
    // Under each of them a reload was still a reload.
    let new = src.replace("let PAD = 2", "let PAD = 5");
    for w in [&mut a, &mut b] {
        w.env.reload_program(w.sid, &new, None).unwrap();
    }
    let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
    assert_eq!(fa, fb);
    assert!(fa.contains("[[1, 5], [3, 10]]"), "{fa}");
}

/// A fork and its parent each reload, restore and run on their own; a
/// speculative run fills nothing the real stack then reads.
#[test]
fn forks_reloads_and_speculative_runs_do_not_share_a_table() {
    let old = format!("let T = [[1, 2], [3, 4]]\nlet U = map(T, fn(r) -> [r[1], r])\n{COUNTER}print(n, T, U)\n");
    let new = old.replace("[3, 4]", "[3, 44]");
    for (on, off) in pairs() {
        let (mut a, mut b) = (world(&old, on), world(&old, off));
        let drive = |w: &mut World| {
            let mut log = Vec::new();
            // A speculative run before anything is kept, and one after.
            log.push(format!("{:?}", w.env.run_speculative(w.sid).is_ok()));
            log.push(frame(w));
            log.push(format!("{:?}", w.env.run_speculative(w.sid).is_ok()));
            log.push(frame(w));
            let fork = w.env.fork_execution(w.sid).unwrap();
            w.env.reload_program(w.sid, &new, None).unwrap();
            log.push(run_frame(w));
            // The fork is a stack of the same program, which has just changed.
            w.env.reset_stack(fork).unwrap();
            w.env.run(fork).unwrap();
            log.push(w.env.take_output_for(fork).join("|"));
            log.push(frame(w));
            // Back to the snapshot: the old state, the program as it is now.
            w.env.restore_execution(w.sid, fork).unwrap();
            log.push(run_frame(w));
            log.push(frame(w));
            log
        };
        let (la, lb) = (drive(&mut a), drive(&mut b));
        assert_eq!(la, lb, "{:?}", on.name());
        assert!(la[4].starts_with("3 [[1, 2], [3, 44]] [[2, [1, 2]], [44, [3, 44]]]"), "{}", la[4]);
        assert!(la[5].contains("[3, 44]"), "the fork runs the new program: {}", la[5]);
    }
}

/// `freeze` and `frozen` are about the heap object, not the value, so they
/// can tell a kept table from one rebuilt every run: it is frozen once and
/// stays frozen, where the rebuilt one is a new unfrozen table each run.
/// A constant string and a payload-free variant already behave this way
/// without the cache. Pinned so the difference is a known one.
#[test]
fn freezing_a_kept_value_freezes_it_once() {
    let src = format!("{COUNTER}let T = [[1, 2], [3, 4]]\nlet s = \"abc\"\nprint(n, frozen(T), frozen(s))\nfreeze(T)\nfreeze(s)\nprint(frozen(T), T)\n");
    let run = |policy: RunPolicy| {
        let mut w = world(&src, policy);
        (0..3).map(|_| frame(&mut w).lines().take(2).collect::<Vec<_>>().join(" / ")).collect::<Vec<_>>()
    };
    let off = run(RunPolicy::REPLAY.with_static_cache(false));
    let on = run(RunPolicy::REPLAY);
    assert_eq!(off[0], on[0]);
    assert_eq!(off[2], "3 false true / true [[1, 2], [3, 4]]", "rebuilt, and the string is the constant");
    assert_eq!(on[2], "3 true true / true [[1, 2], [3, 4]]", "kept");
}

/// `restore_execution` replaces the whole context of the stack it restores,
/// heap included. Another stack bound to the same context then holds ids
/// the restored heap may never have had: what it kept after the snapshot
/// was taken must go, or its next run loads objects that are not there.
#[test]
fn a_restore_empties_the_table_of_every_stack_on_the_restored_context() {
    let src = format!("let T = [[1, 2], [3, 4]]\nlet U = map(T, fn(r) -> {{r: r, s: \"k{{r[0]}}\"}})\n{COUNTER}print(n, T, U)\n");
    let drive = |policy: RunPolicy| {
        let mut w = world(&src, policy);
        let other = w.env.create_stack(w.pid).unwrap();
        let mut log = vec![frame(&mut w)];
        // The snapshot is taken before the second stack has run at all.
        let snapshot = w.env.fork_execution(w.sid).unwrap();
        for _ in 0..3 {
            w.env.reset_stack(other).unwrap();
            w.env.run(other).unwrap();
        }
        log.push(w.env.take_output_for(other).join("|"));
        w.env.restore_execution(w.sid, snapshot).unwrap();
        log.push(run_frame(&mut w));
        // Allocate over whatever the second stack's ids now name.
        for _ in 0..3 {
            log.push(frame(&mut w));
            w.env.reset_stack(other).unwrap();
            w.env.run(other).unwrap();
            let seen = w.env.take_output_for(other);
            log.push(seen.last().map(|l| l.split_once(' ').unwrap().1.to_string()).unwrap_or_default());
        }
        log
    };
    for (on, off) in pairs() {
        assert_eq!(drive(on), drive(off), "{:?}", on.name());
    }
}

/// One table read by many later runs: past the cap on how many slots a run
/// may feed, the table gets a slot of its own instead. Frames that stop
/// half way down leave some of those runs stored and the rest not.
#[test]
fn a_table_that_feeds_more_runs_than_the_cap_is_still_served_right() {
    let mut src = format!("{COUNTER}let BASE = [[1, 2], [3, 4]]\nlet MID = [BASE, [5]]\n");
    for i in 0..75 {
        src.push_str(&format!("let T{i} = [BASE[{}], [{i}], MID[1]]\n", i % 2));
        if i % 3 == 0 {
            src.push_str(&format!("let d{i} = n + {i}\n"));
        } else {
            src.push_str(&format!("print(T{i}[n % 3])\n"));
        }
        if i == 40 {
            src.push_str("assert(n % 3 != 1)\n");
        }
    }
    src.push_str("print(n, T3, T74)\nlet TAIL = for i in range(0, 3) do [i, MID] end\nprint(TAIL[n % 3])\n");
    let (frames, skips) = same_on_and_off(&src, 6);
    assert!(frames[0].contains("=> error") && !frames[1].contains("=> error"), "{}", frames[0]);
    assert!(skips > 50, "{skips}");
}

/// Three more shapes from the review, kept as they were written: tables read
/// only through a capture, a lambda, a method, a default parameter and a
/// nested closure; top-level `return`s between runs that depend on each
/// other; and fresh containers that hold kept rows and are written two
/// levels down.
#[test]
fn captures_early_returns_and_nested_writes_agree_with_the_cache_off() {
    let cases = [
        "let SEEN = [[1, 2], [3, 4]]\nlet ONLY_CAPTURED = [[5, 6], [7, 8]]\nlet ONLY_LAMBDA = [[9], [10]]\nlet ONLY_METHOD = [[11], [12]]\nlet ONLY_DEFAULT = [[13], [14]]\nlet ONLY_NESTED = [[15], [16]]\nfn fetch(i) ONLY_CAPTURED[i % 2] end\nlet lam = fn(i) -> ONLY_LAMBDA[i % 2]\nfn Rect.pick(self, i) ONLY_METHOD[i % 2] end\nfn dflt(i, t = ONLY_DEFAULT) t[i % 2] end\nfn outer(i)\n  let inner = fn(j) -> ONLY_NESTED[j % 2]\n  inner(i)\nend\nprint(n, SEEN, fetch(n), lam(n), dflt(n), outer(n))\nlet r = Rect(0, 0, 1, 1)\nprint(r.pick(n))\n",
        "let A = [[1], [2]]\nlet HIDDEN = [[A], [3]]\nprint(\"top\", n, A)\nif n % 3 == 1 then\n  return n\nend\nlet B = [HIDDEN, [4]]\nprint(B)\nif n % 3 == 2 then return B end\nlet C = map(B, fn(x) -> [x, HIDDEN[1]])\nprint(C)\nC[n % 2]\n",
        "let ROW = [[5], [6]]\nlet FA = f64_array(3)\nlet REC = {xs: [1, 2], m: {k: [3]}}\nlet grid = []\nfor i in range(0, 3) do\n  grid = push(grid, ROW)\nend\nfor i in range(0, 3) do\n  grid[i][0] = [n + i]\n  grid[i][1][0] = n\nend\nprint(grid, ROW)\nlet web = [ROW, [FA, FA], REC]\nweb[0][0][0] = n\nweb[1][0] = set_at(web[1][0], 0, n)\nweb[2].xs[0] = n\nweb[2].m.k = push(web[2].m.k, n)\nprint(web, ROW, FA, REC)\nlet fresh = {a: REC, b: [REC.xs]}\nfresh.a.xs = push(fresh.a.xs, n)\nfresh.b[0][1] = n\nprint(fresh, REC)\nvar acc = [ROW]\nfor i in range(0, 2) do\n  set acc = push(get acc, ROW[i])\n  set acc[0][i] = n\nend\nprint(get acc, ROW)\nlet T = [[1], [2]]\nlet T2 = push(T, n)\nlet U = [[1], [2]]\nlet U = push(U, n)\nprint(T, T2, U)\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        let (frames, skips) = same_on_and_off(&src, 7);
        assert!(!frames[6].contains("=> error"), "the case runs: {}", frames[6]);
        assert!(skips > 0, "something was kept:\n{src}");
    }
}
