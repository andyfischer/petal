//! The static-value cache under observation (`petal::static_cache`,
//! "Observations"; docs/tasks/static-value-optimization.md, "Later:
//! Garden").
//!
//! A host that observes reads the last value bound to every named term
//! after each frame. A frame that skips a static segment executes none of
//! its bindings, so the segment keeps a log of what it observed and the
//! skip records it again. The oracle is the same run with the cache off:
//! what a host reads from the buffer, by name and term by term, must be the
//! same after every frame and across every kind of reload.
//!
//! The wide differentials are elsewhere and run every case twice, once
//! observed: `same_on_and_off` in `static_cache.rs`, `follow_modules` in
//! `static_reload.rs`, both fuzzers in `src/backend/bytecode`, and the
//! panel corpus in `core-libs/petal-ui/tests`. Here are the cases that say
//! what the mechanism is.

use petal::env::{Env, ReloadOutcome, StaticSlotState};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::static_cache::{StaticDebounce, StaticStats};
use petal::static_value::StaticValue;
use petal::value::Value;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

fn world(src: &str, policy: RunPolicy) -> World {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    env.set_static_debounce(None);
    env.observations_mut().enable();
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("compile: {e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

/// What a host reads after a frame: by qualified name, as
/// `Env::get_observations_json` orders it, and the buffer term by term.
fn observed(w: &World) -> String {
    let by_name = serde_json::Value::Object(w.env.get_observations_json(w.pid, w.sid));
    let heap = w.env.heap_for(w.sid).unwrap();
    let mut by_term: Vec<(u32, String)> = w
        .env
        .observations()
        .iter()
        .map(|(t, v)| (t.0, petal::value::value_to_json(&v, heap).to_string()))
        .collect();
    by_term.sort();
    format!("observed {by_name}\nterms {by_term:?}")
}

fn run_frame(w: &mut World) -> String {
    let result = match w.env.run(w.sid) {
        Ok(v) => format!("ok {}", petal::value::value_to_display_string(&v, w.env.heap())),
        Err(e) => format!("error {e}"),
    };
    let out = w.env.take_output().join("\n");
    let state = serde_json::Value::Object(w.env.get_state_json(w.pid, w.sid));
    format!("{out}\n=> {result}\nstate {state}\n{}", observed(w))
}

fn frame(w: &mut World) -> String {
    w.env.reset_stack(w.sid).unwrap();
    run_frame(w)
}

/// One observed value, by qualified name.
fn seen(w: &World, name: &str) -> Option<serde_json::Value> {
    w.env.get_observations_json(w.pid, w.sid).get(name).cloned()
}

fn stats(w: &World) -> StaticStats {
    w.env.static_values(w.sid).map(|v| v.stats).unwrap_or_default()
}

fn held(w: &World, name: &str) -> Option<Value> {
    let values = w.env.static_values(w.sid)?;
    values.slots.iter().find_map(|(info, state)| match state {
        StaticSlotState::Filled(v) if info.name.as_deref() == Some(name) => Some(*v),
        _ => None,
    })
}

fn policies() -> Vec<RunPolicy> {
    ["replay", "replay-memo", "fast", "baseline+static"].iter().map(|n| RunPolicy::parse(n).unwrap()).collect()
}

/// `frames` frames with the cache on and off under every policy; what is
/// observed after each must be the same. Returns the cache's counters
/// under the first policy.
fn same_on_and_off(src: &str, frames: usize) -> StaticStats {
    let mut first = None;
    for policy in policies() {
        let (mut on, mut off) = (world(src, policy.with_static_cache(true)), world(src, policy.with_static_cache(false)));
        assert!(on.env.static_cache_enabled() && !off.env.static_cache_enabled());
        for i in 0..frames {
            assert_eq!(frame(&mut on), frame(&mut off), "frame {i} under {:?}\n{src}", policy.name());
        }
        first.get_or_insert(stats(&on));
    }
    first.unwrap()
}

const COUNTER: &str = "state n = 0\nn = n + 1\n";

/// Three tables, one built by a function with locals of its own, a name
/// nothing reads, and an alias: every kind of binding a segment can make.
const TABLES: &str = "\
let PAD = 2
let TILES = [{id: 1, w: 2}, {id: 2, w: 3}, {id: 3, w: 5}]
let NAMES = [\"a\", \"b\", \"c\"]
fn widths(ts)
  let scale = PAD * 2
  let out = map(ts, fn(t) t.w * scale end)
  out
end
let WIDTHS = widths(TILES)
let unread = [len(NAMES), PAD]
let alias = WIDTHS
state n = 0
n = n + 1
print(n, TILES[n % 3], NAMES[n % 3], WIDTHS)
";

#[test]
fn a_skipped_segment_reports_what_it_observed() {
    let s = same_on_and_off(TABLES, 5);
    assert!(s.skips >= 4, "the tables are kept: {s:?}");
    assert!(s.replayed > 0, "and what they observed is recorded again: {s:?}");
    assert_eq!(s.relogged, 0, "nothing ran twice for it: {s:?}");

    // By name, on a frame that executed none of these bindings.
    let mut w = world(TABLES, RunPolicy::REPLAY);
    for _ in 0..3 {
        frame(&mut w);
    }
    let json = |v: &str| Some(serde_json::from_str::<serde_json::Value>(v).unwrap());
    assert_eq!(seen(&w, "WIDTHS"), json("[8, 12, 20]"));
    assert_eq!(seen(&w, "alias"), json("[8, 12, 20]"), "a name nothing reads but the buffer");
    assert_eq!(seen(&w, "unread"), json("[3, 2]"), "a value nothing reads at all");
    assert_eq!(seen(&w, "widths.scale"), json("4"), "a local of a function the segment called");
    assert_eq!(seen(&w, "widths.out"), json("[8, 12, 20]"));
    assert_eq!(seen(&w, "n"), json("3"), "beside what the frame did execute");
}

/// A function called by a kept segment and again by dynamic code: the
/// buffer is last write wins, and the skip records its log where the
/// segment stood, before the dynamic call.
#[test]
fn a_replayed_value_does_not_overwrite_a_later_one() {
    let src = format!(
        "fn twice(x)\n  let doubled = x * 2\n  [doubled, x]\nend\nlet FIXED = twice(21)\n{COUNTER}let live = twice(n)\nlet after = [FIXED, live]\nprint(after)\n"
    );
    let s = same_on_and_off(&src, 4);
    assert!(s.skips > 0 && s.replayed > 0, "{s:?}");
    let mut w = world(&src, RunPolicy::REPLAY);
    for _ in 0..3 {
        frame(&mut w);
    }
    assert_eq!(seen(&w, "twice.doubled"), Some(6.into()), "the dynamic call's, which came last");
}

/// The optimizer edits a container in place where the program can no
/// longer reach the old value. The buffer still names it and shows the
/// edit, so a statement that edits what another statement built is not
/// left out under observation: the first case is the one that differed.
#[test]
fn a_container_edited_in_place_is_observed_the_same() {
    let cases = [
        "let BASE = [[1, 2], [3, 4], [5, 6]]\nlet alias = BASE\nlet cur = drop_last(alias)\nprint(len(cur), cur[0], n)\n",
        "let A = [1, 2, 3]\nlet B = push(A, 4)\nlet C = push(B, 5)\nprint(C, n)\n",
        "let A = [1, 2, 3]\nlet B = if len(A) > 2 then A else [0] end\nlet C = push(B, 5)\nprint(C, n)\n",
        "let A = {x: 1, y: [1]}\nlet B = A\nB.x = 2\nlet C = B\nC.y = [2]\nprint(C, n)\n",
        "let T = [3, 1, 2]\nlet S = reverse(T)\nlet W = push(S, 9)\nprint(W, n)\n",
        "let T = [3, 1, 2]\nlet S = sort(T)\nlet out = push(S, n)\nprint(out)\n",
        "let T = [[1, 2], [3, 4]]\nlet r = T[0]\nlet out = push(r, n)\nprint(out)\n",
        "let R = {a: [1, 2], b: 2}\nlet q = R\nq.b = n\nprint(q)\n",
        "let R = {a: [1, 2], b: 2}\nlet inner = R.a\nlet out = push(inner, n)\nprint(out)\n",
        "fn bump(xs, v)\n  xs[0] = v\n  push(xs, v)\nend\nlet T = [[1, 2], [3, 4]]\nlet out = bump(T, n)\nprint(out)\n",
        "fn mk(k)\n  let a = f64_array(4)\n  for i in range(0, 4) do\n    a = set_at(a, i, i * k)\n  end\n  a\nend\n\
         fn fill(a, v)\n  set_at(a, 0, v)\nend\nlet ARR = mk(2)\nlet out = fill(ARR, n)\nlet out2 = set_at(ARR, 1, n)\nprint(out, out2)\n",
        "let T = [[1, 2], [3, 4]]\nfor row in T do\n  let r2 = push(row, n)\n  print(r2)\nend\n",
        "let T = [[1, 2], [3, 4]]\nlet m = map(T, fn(r) push(r, 1) end)\nlet m2 = push(m, n)\nprint(m2)\n",
        "enum K\n  a(xs: [1, 2]),\n  b(xs: [3]),\nend\nlet d = K.a.xs\nlet e = push(d, n)\nprint(e)\n",
        "let T = [1, 2, 3]\nvar c = T\nset c = push(get c, n)\nprint(get c)\n",
        "let T = [1, 2, 3]\nstate s = T\ns = push(s, n)\nprint(len(s))\n",
    ];
    for case in cases {
        same_on_and_off(&format!("{COUNTER}{case}"), 4);
    }
    // The statement that edits in place runs on every frame; the one it
    // edits was never kept, with or without observation.
    let src = format!("{COUNTER}{}", cases[0]);
    let mut w = world(&src, RunPolicy::REPLAY);
    frame(&mut w);
    frame(&mut w);
    assert_eq!(seen(&w, "BASE"), seen(&w, "cur"), "one list under both names, as the cache off shows it");
    assert!(held(&w, "cur").is_none() && held(&w, "BASE").is_none());
}

/// A reload that recompiles renumbers every term, so no log survives it.
/// The values do: each kept segment executes once more to write its log,
/// and the store hands back the kept values.
#[test]
fn a_recompile_keeps_the_values_and_writes_the_logs_again() {
    let new = TABLES.replace("n = n + 1", "n = n + 1 + n % 2");
    for policy in policies() {
        let what = policy.name().unwrap_or_default();
        let mut on = world(TABLES, policy);
        let mut off = world(TABLES, policy.with_static_cache(false));
        // The same reload with observations off: the report must not
        // change because a host is watching.
        let mut plain = world(TABLES, policy);
        plain.env.observations_mut().disable();
        for w in [&mut on, &mut off, &mut plain] {
            frame(w);
            frame(w);
        }
        let before = (held(&on, "TILES"), held(&on, "NAMES"), held(&on, "WIDTHS"));
        assert!(before.0.is_some() && before.1.is_some() && before.2.is_some(), "[{what}] kept");
        let stored = stats(&on).stored;

        let report = on.env.reload_program(on.sid, &new, None).unwrap();
        off.env.reload_program(off.sid, &new, None).unwrap();
        let unwatched = plain.env.reload_program(plain.sid, &new, None).unwrap();
        assert_eq!(report.outcome, ReloadOutcome::Recompiled);
        assert_eq!(report.statics, unwatched.statics, "[{what}] the report is the one without observations");
        let s = report.statics.as_ref().unwrap();
        assert!(s.kept >= 3 && s.rebuilt_names.is_empty(), "[{what}] {s:?}");

        assert_eq!(run_frame(&mut on), run_frame(&mut off), "[{what}] the run after the reload");
        let after = stats(&on);
        assert!(after.relogged >= 3, "[{what}] each kept segment ran once for its log: {after:?}");
        assert_eq!(after.stored, stored, "[{what}] and stored nothing: {after:?}");
        assert_eq!(
            (held(&on, "TILES"), held(&on, "NAMES"), held(&on, "WIDTHS")),
            before,
            "[{what}] the very values, not copies"
        );
        // And the buffer names those, not the copies the segments built
        // on the way: a log would otherwise hold a second TILES for good.
        for name in ["TILES", "NAMES", "WIDTHS"] {
            let kept = held(&on, name).unwrap();
            let json = petal::value::value_to_json(&kept, on.env.heap());
            let named: Vec<Value> =
                on.env.observations().iter().map(|(_, v)| v).filter(|v| petal::value::value_to_json(v, on.env.heap()) == json).collect();
            assert!(!named.is_empty() && named.iter().all(|v| *v == kept), "[{what}] {name}: {named:?} vs {kept:?}");
        }
        for i in 0..3 {
            assert_eq!(frame(&mut on), frame(&mut off), "[{what}] frame {i} after the reload");
        }
        let later = stats(&on);
        assert_eq!(later.relogged, after.relogged, "[{what}] once, not on every frame");
        assert!(later.skips > after.skips, "[{what}] skipped again: {later:?}");
    }
}

/// A reload that leaves the lowering in place hands on the logs of the
/// segments the edit did not reach, and only those.
#[test]
fn a_patched_literal_hands_on_the_logs_the_edit_did_not_reach() {
    let new = TABLES.replace("\"b\"", "\"B\"");
    let mut on = world(TABLES, RunPolicy::REPLAY);
    let mut off = world(TABLES, RunPolicy::REPLAY.with_static_cache(false));
    for w in [&mut on, &mut off] {
        frame(w);
        frame(w);
    }
    let before = stats(&on);
    let report = on.env.reload_program(on.sid, &new, None).unwrap();
    off.env.reload_program(off.sid, &new, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    // `unread` reads NAMES, has no slot, and is in the segment of WIDTHS,
    // which is rebuilt with what its segment read.
    assert_eq!(report.statics.as_ref().unwrap().rebuilt_names, ["NAMES", "WIDTHS"]);
    assert_eq!(run_frame(&mut on), run_frame(&mut off));
    let after = stats(&on);
    assert_eq!(after.relogged, 0, "PAD and TILES kept their logs, and the rest was built: {after:?}");
    assert!(after.stored > before.stored, "NAMES was rebuilt: {after:?}");
    let longer = new.replace("[\"a\", \"B\", \"c\"]", "[\"a\", \"B\", \"c\", \"d\"]");
    on.env.reload_program(on.sid, &longer, None).unwrap();
    off.env.reload_program(off.sid, &longer, None).unwrap();
    assert_eq!(run_frame(&mut on), run_frame(&mut off));
    assert_eq!(seen(&on, "unread"), Some(serde_json::json!([4, 2])));
    for i in 0..2 {
        assert_eq!(frame(&mut on), frame(&mut off), "frame {i}");
    }
}

/// The case the fuzzer found: a statement kept in the code for its name
/// alone (`let K10 = K9`) has no slot, sits in the segment of a value the
/// edit does not reach (`G`, at whose front it is), and reads a number the
/// edit does change.
#[test]
fn a_name_with_no_slot_is_not_served_from_a_stale_log() {
    let old = "\
config let C = 16
let T = [11, 9, 7, C]
let K9 = sum(T) % 97
let K10 = K9
let G = map([1, 2], fn(i) [i, i * 3] end)
state n = 0
n = n + 1
print(n, G, T, K9)
";
    for new in [old.replace("[11, 9, 7, C]", "[12, 9, 7, C]"), old.replace("config let C = 16", "config let C = 17")] {
        for policy in policies() {
            let mut on = world(old, policy);
            let mut off = world(old, policy.with_static_cache(false));
            for w in [&mut on, &mut off] {
                frame(w);
                frame(w);
            }
            assert_eq!(on.env.reload_program(on.sid, &new, None).unwrap().outcome, ReloadOutcome::Patched);
            off.env.reload_program(off.sid, &new, None).unwrap();
            assert_eq!(run_frame(&mut on), run_frame(&mut off), "{:?}\n{new}", policy.name());
            assert_eq!(seen(&on, "K10"), seen(&on, "K9"));
            assert_eq!(frame(&mut on), frame(&mut off));
        }
    }
    // What the edit reaches need not be a value at all. `Q` is read by
    // nothing and sits in front of the list that is kept; the edit is
    // inside the function it calls, which no slot depends on.
    let plain = "\
fn five()
  5
end
state n = 0
n = n + 1
let Q = five()
let G = map([1, 2], fn(i) [i, i * 3] end)
print(n, G)
";
    for policy in policies() {
        let mut on = world(plain, policy);
        let mut off = world(plain, policy.with_static_cache(false));
        for w in [&mut on, &mut off] {
            frame(w);
            frame(w);
        }
        assert_eq!(seen(&on, "Q"), Some(5.into()));
        let new = plain.replace("  5\n", "  6\n");
        let report = on.env.reload_program(on.sid, &new, None).unwrap();
        off.env.reload_program(off.sid, &new, None).unwrap();
        assert_eq!(report.outcome, ReloadOutcome::Patched);
        assert!(report.statics.as_ref().unwrap().rebuilt_names.is_empty(), "no slot is reached: {:?}", report.statics);
        assert_eq!(run_frame(&mut on), run_frame(&mut off), "under {:?}", policy.name());
        assert_eq!(seen(&on, "Q"), Some(6.into()));
        assert_eq!(frame(&mut on), frame(&mut off));
        assert!(stats(&on).skips > 0);
    }
    // The same through `set_config_value`.
    let mut on = world(old, RunPolicy::REPLAY);
    let mut off = world(old, RunPolicy::REPLAY.with_static_cache(false));
    for k in [20, 21, 30] {
        for w in [&mut on, &mut off] {
            frame(w);
            w.env.set_config_value(w.sid, None, "C", &StaticValue::Int(k)).unwrap();
        }
        assert_eq!(frame(&mut on), frame(&mut off), "C = {k}");
        assert_eq!(seen(&on, "K10"), Some(serde_json::json!((11 + 9 + 7 + k) % 97)));
    }
}

/// An enum's data is in the stack's enum table as well as in its slot. The
/// segment that executes again for its log publishes a new record; the
/// store publishes the kept one over it, or a neighbour kept from the old
/// record would hold another object than the one the program reads.
#[test]
fn enum_data_kept_through_a_recompile_stays_the_published_record() {
    let old = "\
enum Kind
  road(code: 1, cost: [10, 20]),
  park(code: 2, cost: [5, 6]),
end
let COST = Kind.road.cost
state n = 0
n = n + 1
if n == 4 then
  freeze(Kind.road.cost)
end
print(n, frozen(COST), frozen(Kind.road.cost), COST, Kind.park.cost)
";
    let new = old.replace("n = n + 1", "n = n + 1 + 0");
    let drive = |observe: bool| {
        let mut w = world(old, RunPolicy::REPLAY);
        if !observe {
            w.env.observations_mut().disable();
        }
        let mut out = Vec::new();
        for _ in 0..2 {
            out.push(frame(&mut w).lines().next().unwrap().to_string());
        }
        let report = w.env.reload_program(w.sid, &new, None).unwrap();
        assert_eq!(report.outcome, ReloadOutcome::Recompiled);
        assert!(report.statics.as_ref().unwrap().rebuilt_names.is_empty(), "{:?}", report.statics);
        out.push(run_frame(&mut w).lines().next().unwrap().to_string());
        for _ in 0..3 {
            out.push(frame(&mut w).lines().next().unwrap().to_string());
        }
        (out, stats(&w))
    };
    let (watched, s) = drive(true);
    let (plain, _) = drive(false);
    assert!(s.relogged > 0, "the data's segment ran again for its log: {s:?}");
    assert_eq!(watched, plain, "observing changes nothing the program can tell");
    assert_eq!(watched[4], "5 true true [10, 20] [5, 6]", "one record under both names");
}

/// A log holds values nothing else does: the locals of a function a
/// segment called. They are roots for as long as the segment is kept.
#[test]
fn logged_values_survive_collections() {
    let src = format!(
        "fn build(ws)\n  let parts = map(ws, fn(w) {{word: w, twice: [w, w ++ \"!\"]}} end)\n  let lens = map(parts, fn(p) len(p.word) end)\n  sum(lens)\nend\n\
         {COUNTER}let junk = for i in range(0, 30000 + n % 2) do [i, \"x{{i}}\", {{k: i}}] end\n\
         let WORDS = [\"alpha\", \"beta\", \"gamma\"]\nlet TOTAL = [build(WORDS)]\nprint(n, len(junk), TOTAL)\n"
    );
    // The junk is built first: while it is, the frame has cleared the
    // buffer and not yet reached the skip, and the log is all that holds
    // what `build` bound.
    let mut on = world(&src, RunPolicy::REPLAY);
    let mut off = world(&src, RunPolicy::REPLAY.with_static_cache(false));
    let before = on.env.heap().collections();
    for i in 0..12 {
        assert_eq!(frame(&mut on), frame(&mut off), "frame {i}");
        for (term, v) in on.env.observations().iter() {
            assert!(on.env.heap().is_live(v), "frame {i}: term {} names a collected value", term.0);
        }
    }
    assert!(on.env.heap().collections() > before + 1, "the frames collected");
    assert!(stats(&on).skips >= 11);
    let parts = seen(&on, "build.parts").expect("held by the log alone");
    assert_eq!(parts[2]["twice"], serde_json::json!(["gamma", "gamma!"]));
}

/// A segment that raises is not kept and neither is its half-written log;
/// the segments before it are, with theirs.
#[test]
fn a_segment_that_raises_leaves_no_log() {
    let src = format!(
        "fn pick(xs, i)\n  let got = xs[i]\n  got\nend\nlet GOOD = [pick([4, 5, 6], 1), 7]\n{COUNTER}let BAD = [pick([1, 2], 2 + 5)]\nprint(n, GOOD, BAD)\n"
    );
    same_on_and_off(&src, 4);
}

/// While a drag puts a slow rebuild off, the segment keeps its old values
/// and its old log: it is not executed for the log on every tick, which is
/// the cost the debounce exists to avoid.
#[test]
fn a_rebuild_put_off_keeps_its_log_with_its_values() {
    let src = "\
config let K = 2
fn times(xs)
  let factor = K
  map(xs, fn(i) i * factor end)
end
let BIG = times([1, 2, 3, 4])
let OTHER = [1, 2, 3]
state n = 0
n = n + 1
print(n, K, BIG, OTHER)
";
    let every = StaticDebounce { threshold: std::time::Duration::ZERO, quiet: std::time::Duration::from_secs(60) };
    let mut w = world(src, RunPolicy::REPLAY);
    w.env.set_static_debounce(Some(every));
    frame(&mut w);
    w.env.set_config_value(w.sid, None, "K", &StaticValue::Int(3)).unwrap();
    frame(&mut w);
    let built = stats(&w);
    let report = w.env.set_config_value(w.sid, None, "K", &StaticValue::Int(4)).unwrap();
    assert_eq!(report.statics.as_ref().unwrap().deferred, ["BIG"]);
    assert!(frame(&mut w).starts_with("3 4 [3, 6, 9, 12] [1, 2, 3]"));
    let waiting = stats(&w);
    // K is given the literal's new value without a run, so its segment (one
    // instruction) executes for its log; BIG's does not execute at all.
    assert_eq!(
        (waiting.relogged - built.relogged, waiting.stored - built.stored),
        (1, 0),
        "only K itself: {built:?} then {waiting:?}"
    );
    assert_eq!(seen(&w, "BIG"), Some(serde_json::json!([3, 6, 9, 12])), "what the program is using");
    assert_eq!(seen(&w, "times.factor"), Some(3.into()), "and what it was built from");
    assert_eq!(seen(&w, "K"), Some(4.into()));

    // Asked for, the rebuild writes the log with the values.
    assert_eq!(w.env.flush_static_rebuilds(w.sid), 1);
    assert!(frame(&mut w).starts_with("4 4 [4, 8, 12, 16] [1, 2, 3]"));
    assert_eq!(seen(&w, "BIG"), Some(serde_json::json!([4, 8, 12, 16])));
    assert_eq!(seen(&w, "times.factor"), Some(4.into()));
}

/// The per-term trace still turns the cache off: it records every
/// instruction, not one value per name.
#[test]
fn the_trace_still_turns_the_cache_off() {
    let mut w = world(TABLES, RunPolicy::FAST);
    frame(&mut w);
    assert!(w.env.static_cache_enabled());
    w.env.trace_mut().enabled = true;
    assert!(!w.env.static_cache_enabled());
    frame(&mut w);
    assert!(w.env.static_values(w.sid).unwrap().slots.is_empty());
    w.env.trace_mut().enabled = false;
    frame(&mut w);
    frame(&mut w);
    assert!(stats(&w).skips > 0);
    let mut explain = world(TABLES, RunPolicy::EXPLAIN);
    frame(&mut explain);
    assert!(!explain.env.static_cache_enabled());
}
