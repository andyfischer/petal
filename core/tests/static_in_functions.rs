//! Static values inside function bodies (`petal::static_cache`, the `in_fn`
//! module; docs/tasks/static-value-optimization.md, "Later: constants
//! inside function bodies").
//!
//! A value a function builds that depends on nothing of the call is built
//! by the first call that reaches it and loaded by every later one. That
//! must change nothing a program does. Every case here drives the same
//! frames three ways under each policy: the cache on, the cache off, and
//! the cache on and checking itself (`Env::set_static_check`: the code
//! runs on every call and what it built is compared with what is kept).
//! All three must print, return and store the same, and the check must
//! find nothing.
//!
//! A stale or wrongly shared kept value is the worst bug this could have,
//! so most cases are about what must *not* be kept, and assert that it is
//! not as well as that the output agrees.

use petal::env::{Env, StaticSlotState};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;

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
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("compile: {e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

fn world(src: &str, policy: RunPolicy) -> World {
    world_with(src, policy, |_| {})
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

/// One frame the way a host drives it: reset, run, collect.
fn frame(w: &mut World) -> String {
    w.env.reset_stack(w.sid).unwrap();
    run_frame(w)
}

const POLICIES: [&str; 5] = ["replay", "replay-memo", "fast", "baseline+memo", "baseline"];

/// What the stack keeps for function bodies: (slots, slots that hold a
/// value, the lines of the slots that hold one).
fn kept(w: &World) -> (usize, usize, Vec<String>) {
    let values = w.env.static_values(w.sid).unwrap_or_default();
    let filled: Vec<String> = values
        .in_functions
        .iter()
        .filter(|(_, s)| matches!(s, StaticSlotState::Filled(_)))
        .map(|(info, _)| format!("{} {}", info.function.clone().unwrap_or_default(), info.at.clone().unwrap_or_default()))
        .collect();
    (values.in_functions.len(), filled.len(), filled)
}

fn mismatches(w: &World) -> Vec<String> {
    w.env.static_values(w.sid).map(|v| v.mismatches).unwrap_or_default()
}

/// Run `frames` frames of `src` with the cache on, off, and on and
/// checking, under every policy, and require the same output. Returns the
/// frames of the default policy and the world that ran them with the
/// cache on.
fn same_on_and_off(src: &str, frames: usize) -> (Vec<String>, World) {
    let mut first = None;
    for name in POLICIES {
        let p = RunPolicy::parse(name).unwrap();
        let mut on = world(src, p.with_static_cache(true));
        let mut off = world(src, p.with_static_cache(false));
        let mut checked = world_with(src, p.with_static_cache(true), |e| e.set_static_check(true));
        let mut out = Vec::new();
        for i in 0..frames {
            let (a, b, c) = (frame(&mut on), frame(&mut off), frame(&mut checked));
            assert_eq!(a, b, "frame {i} differs with the cache on under {name}\n{src}");
            assert_eq!(c, b, "frame {i} differs with the cache checking itself under {name}\n{src}");
            out.push(a);
        }
        assert_eq!(mismatches(&checked), Vec::<String>::new(), "under {name}\n{src}");
        assert_eq!(kept(&off).0, 0, "the cache is off");
        first.get_or_insert((out, on));
    }
    // Where a host observes, nothing is kept for function bodies, and the
    // program still runs the same.
    let watch = |e: &mut Env| e.observations_mut().enable();
    let mut on = world_with(src, RunPolicy::REPLAY, watch);
    let mut off = world_with(src, RunPolicy::REPLAY.with_static_cache(false), watch);
    for i in 0..frames {
        assert_eq!(frame(&mut on), frame(&mut off), "frame {i} differs, observed\n{src}");
    }
    assert_eq!(kept(&on).0, 0, "nothing is kept for function bodies under observation");
    first.unwrap()
}

/// Instructions and allocations of each of `frames` frames of `src`.
fn costs(src: &str, policy: RunPolicy, frames: usize) -> Vec<(u64, u64)> {
    let mut w = world_with(src, policy, |e| e.set_heap_stats(true));
    let mut out = Vec::new();
    let read = |w: &World| {
        (
            w.env.instructions_retired(w.sid).unwrap_or(0),
            w.env.alloc_stats_for(w.sid).map_or(0, |s| s.total()),
        )
    };
    for _ in 0..frames {
        w.env.reset_stack(w.sid).unwrap();
        let before = read(&w);
        w.env.run(w.sid).unwrap();
        w.env.take_output();
        let after = read(&w);
        out.push((after.0 - before.0, after.1 - before.1));
    }
    out
}

const COUNTER: &str = "state n = 0\nn = n + 1\n";

// ---------------------------------------------------------------------------
// What is kept
// ---------------------------------------------------------------------------

#[test]
fn a_record_a_function_builds_from_literals_is_built_once() {
    let src = format!(
        "fn match_at(s, i)\n  let none = {{kind: \"none\", ml: 0, close: -1}}\n  if i > 1 then {{kind: \"code\", ml: 1, close: i}} else none end\nend\n\
         {COUNTER}for i in range(0, 4) do\n  print(n, match_at(\"ab\", i))\nend\n"
    );
    let (frames, w) = same_on_and_off(&src, 4);
    assert!(frames[0].contains("{ kind: \"none\", ml: 0, close: -1 }"), "{}", frames[0]);
    let (slots, filled, lines) = kept(&w);
    assert_eq!((slots, filled), (1, 1), "{lines:?}");
    assert!(lines[0].starts_with("match_at"), "{lines:?}");
    // Four calls a frame, one record each: the first call of the first
    // frame builds it, and no later call allocates it.
    let on = costs(&src, RunPolicy::REPLAY, 3);
    let off = costs(&src, RunPolicy::REPLAY.with_static_cache(false), 3);
    assert_eq!(off[2].1 - on[2].1, 4, "allocations a frame, off {off:?} on {on:?}");
    assert!(on[2].0 < off[2].0, "instructions a frame, off {off:?} on {on:?}");
}

#[test]
fn constant_arguments_of_a_call_are_kept_and_the_call_is_not() {
    // The shape of a heat map: colour literals handed to a function with a
    // value of the call.
    let src = format!(
        "fn mix(a, b, t)\n  [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]\nend\n\
         fn heat(v)\n  let t = v / 10.0\n  if t < 0.5 then\n    mix([1, 2], [3, 4], t)\n  else\n    mix([3, 4], [9, 9], t)\n  end\nend\n\
         {COUNTER}for i in range(0, 10) do\n  print(n, heat(i))\nend\n"
    );
    let (_, w) = same_on_and_off(&src, 3);
    let (slots, filled, lines) = kept(&w);
    assert_eq!((slots, filled), (4, 4), "{lines:?}");
    assert!(lines.iter().all(|l| l.starts_with("heat")), "{lines:?}");
}

#[test]
fn a_call_of_a_pure_function_on_constants_is_kept_with_what_it_calls() {
    let src = format!(
        "fn fc(r, g, b)\n  {{r: r, g: g, b: b}}\nend\n\
         fn sky(t)\n  let lo = fc(0.9, 0.93, 0.96)\n  let hi = fc(0.45, 0.57, 0.72)\n  lo.r + (hi.r - lo.r) * t\nend\n\
         {COUNTER}for i in range(0, 6) do\n  print(n, sky(i / 6))\nend\n"
    );
    let (_, w) = same_on_and_off(&src, 3);
    let (_, filled, lines) = kept(&w);
    assert_eq!(filled, 2, "{lines:?}");
    let on = costs(&src, RunPolicy::REPLAY, 3);
    let off = costs(&src, RunPolicy::REPLAY.with_static_cache(false), 3);
    assert_eq!(off[2].1 - on[2].1, 12, "two records a call, six calls: off {off:?} on {on:?}");
}

#[test]
fn a_constant_built_from_a_kept_top_level_value_holds_that_value() {
    let src = format!(
        "let PALETTE = [[1, 2, 3], [4, 5, 6]]\n\
         fn row(i)\n  let both = [PALETTE[0], PALETTE[1], len(PALETTE)]\n  both[i % 2]\nend\n\
         {COUNTER}for i in range(0, 4) do\n  print(n, row(i))\nend\n"
    );
    let (_, w) = same_on_and_off(&src, 4);
    assert_eq!(kept(&w).1, 1, "{:?}", kept(&w).2);
}

#[test]
fn a_loop_over_constants_is_kept_whole() {
    let src = format!(
        "fn table(k)\n  let squares = for i in range(0, 5) do i * i end\n  var total = 0\n  for s in squares do\n    set total = total + s\n  end\n  squares[k % 5] + total\nend\n\
         {COUNTER}for i in range(0, 7) do\n  print(n, table(i))\nend\n"
    );
    let (frames, _) = same_on_and_off(&src, 3);
    assert!(frames[0].contains("1 30"), "{}", frames[0]);
}

// ---------------------------------------------------------------------------
// What is not
// ---------------------------------------------------------------------------

/// Each of these builds something the classifier calls static (it takes a
/// function's parameters to be data) that is a different value on every
/// call, iteration or run. None of those may be kept; the number beside
/// each case is how many values in it really are constant.
#[test]
fn a_value_of_the_call_is_never_kept() {
    let cases = [
        // A parameter.
        ("fn f(a)\n  let t = [a, 1]\n  t\nend\n", 0),
        // A parameter through a local and a pure call.
        ("fn g(x)\n  [x * 2]\nend\nfn f(a)\n  let b = a + 1\n  let t = {k: g(b)}\n  t\nend\n", 0),
        // A loop variable, inside a loop that is not itself invariant.
        ("fn f(a)\n  let out = []\n  for i in range(0, a + 2) do\n    let cell = [i, 0]\n    out = append(out, cell)\n  end\n  out\nend\n", 0),
        // A loop variable of a loop over constants, read by code that
        // stays in the loop with a parameter.
        ("fn f(a)\n  var s = 0\n  for i in range(0, 3) do\n    let pair = [i, i + 1]\n    set s = s + pair[0] * a + pair[1]\n  end\n  s\nend\n", 0),
        // A pattern variable.
        ("fn f(a)\n  match [a, 2]\n    when [x, y] -> [x, y, 9]\n    when _ -> [0]\n  end\nend\n", 0),
        // A name a branch on a parameter rebinds.
        ("fn f(a)\n  let t = [1]\n  if a > 1 then\n    t = [2]\n  end\n  let u = [t, 0]\n  u\nend\n", 2),
        // An accumulator of a loop whose length is a parameter.
        ("fn f(a)\n  let acc = [0]\n  for i in range(0, a) do\n    acc = [acc[0] + 1]\n  end\n  let u = {v: acc}\n  u\nend\n", 1),
        // `state`, and a `var`.
        ("fn f(a)\n  state hits = 0\n  hits = hits + 1\n  let t = [hits]\n  t\nend\n", 0),
        ("fn f(a)\n  var c = 0\n  set c = c + a\n  let t = [c]\n  t\nend\n", 0),
        // The default of a parameter that was passed.
        ("fn f(a, extra = [7, 7])\n  let t = [a, extra]\n  t\nend\n", 1),
    ];
    for (body, constants) in cases {
        let src = format!("{body}{COUNTER}for i in range(0, 4) do\n  print(n, f(i + n))\nend\nprint(f(n))\n");
        let (frames, w) = same_on_and_off(&src, 4);
        assert_ne!(frames[0], frames[1], "the frames differ, or the case proves nothing\n{src}");
        // What is kept in each is the literal that really is constant
        // (`[1]` and `[2]` on the two sides of the branch, the `[0]` the
        // accumulator starts from, the default's `[7, 7]`), and nothing
        // built from the call. `same_on_and_off` checked the values; this
        // is the count.
        let (_, filled, lines) = kept(&w);
        assert_eq!(filled, constants, "{lines:?}\n{src}");
    }
}

#[test]
fn a_closure_made_per_call_keeps_only_what_no_call_gave_it() {
    // `scale` is made once per call of `make`, capturing a value of that
    // call. `[k, 1]` reads the capture; `[10, 20]` does not.
    let src = format!(
        "fn make(k)\n  fn(x) -> do\n    let own = [k, 1]\n    let shared = [10, 20]\n    own[0] * x + shared[1]\n  end\nend\n\
         {COUNTER}let a = make(n)\nlet b = make(n + 5)\nprint(n, a(2), b(2), a(3))\n"
    );
    let alt = src.replace("fn(x) -> do\n", "fn(x)\n").replace("  end\nend\n", "  end\nend\n");
    for src in [src, alt] {
        // Whichever spelling of a multi-line lambda the language takes.
        if Env::new().load_program(&src).is_err() {
            continue;
        }
        let (frames, w) = same_on_and_off(&src, 3);
        assert!(frames[0].contains("1 22 32 23"), "{}", frames[0]);
        let (_, filled, lines) = kept(&w);
        assert_eq!(filled, 1, "{lines:?}");
        return;
    }
    panic!("neither spelling of the lambda compiled");
}

#[test]
fn a_capture_is_invariant_only_if_it_is_at_every_place_the_closure_is_made() {
    // `pick` captures `base`, which is a kept top-level table in one
    // closure and a per-call list in... the same function made elsewhere
    // cannot be written; what can is one function whose capture is a
    // top-level value that is not static.
    let src = format!(
        "{COUNTER}let base = [n, n + 1]\nlet FIXED = [5, 6]\n\
         fn pick(i)\n  let a = [base[0], 0]\n  let b = [FIXED[0], 0]\n  a[0] * 10 + b[0] + i\nend\n\
         print(pick(0), pick(1))\n"
    );
    let (frames, w) = same_on_and_off(&src, 4);
    assert_ne!(frames[0], frames[1]);
    let (_, filled, lines) = kept(&w);
    assert_eq!(filled, 1, "only the one built from FIXED: {lines:?}");
}

#[test]
fn a_value_that_is_written_to_is_never_kept() {
    let cases = [
        // The accumulator every loop starts from.
        ("fn f(a)\n  let out = []\n  for i in range(0, 3) do\n    out = append(out, i * a)\n  end\n  out\nend\n", 0),
        // Written by index, and through a field.
        ("fn f(a)\n  let grid = [0, 0, 0]\n  grid[1] = a\n  grid\nend\n", 0),
        ("fn f(a)\n  let r = {x: 0, y: 0}\n  r.x = a\n  r\nend\n", 0),
        // Through an alias.
        ("fn f(a)\n  let base = [1, 2, 3]\n  let alias = base\n  alias[0] = a\n  [base, alias]\nend\n", 0),
    ];
    for (body, expect) in cases {
        let src = format!("{body}{COUNTER}print(n, f(n), f(n + 1))\n");
        let (_, w) = same_on_and_off(&src, 4);
        let (_, filled, lines) = kept(&w);
        assert_eq!(filled, expect, "{lines:?}\n{src}");
    }
}

/// A kept value leaves the function. Whoever writes to it gets a copy,
/// and the next call gets the value as it was built.
#[test]
fn no_caller_or_callee_writes_through_to_a_kept_value() {
    let prelude = "fn bump(xs, v)\n  xs[0] = v\n  xs\nend\nfn poke(r, v)\n  r.x = v\n  r\nend\n";
    let cases = [
        // The caller edits what the function returned (the function hands
        // back a "fresh" list as far as the escape analysis can tell).
        "fn mk()\n  [0, 0, 0]\nend\nfn use(a)\n  let g = mk()\n  g[0] = a\n  g[1] = a + 1\n  g\nend\nprint(use(n), mk(), use(n + 1), mk())\n",
        "fn mk()\n  let t = [0, 0, 0]\n  t\nend\nlet g = mk()\ng[2] = n\nprint(g, mk())\n",
        // The same through another name for the function, which is how a
        // kept value reaches a caller that writes: the first write copies.
        "fn mk()\n  [0, 0, 0]\nend\nlet other = mk\nlet g = other()\ng[0] = n\ng[1] = n + 1\nprint(g, mk(), other())\n",
        // An appending caller.
        "fn mk()\n  [1, 2]\nend\nlet g = mk()\ng = append(g, n)\ng = append(g, n)\nprint(g, mk())\n",
        // A callee that writes its parameter.
        "fn mk()\n  [7, 8, 9]\nend\nprint(bump(mk(), n), mk())\n",
        "fn mk()\n  {x: 1, y: [1, 2]}\nend\nprint(poke(mk(), n), mk())\n",
        // An element taken out of it.
        "fn mk()\n  [[1, 2], [3, 4]]\nend\nlet row = mk()[0]\nrow[0] = n\nprint(row, mk())\nprint(bump(mk()[1], n), mk())\n",
        // Through a `var` cell and a `state` cell seeded from it.
        "fn mk()\n  [4, 5, 6]\nend\nvar c = mk()\nset c[0] = n\nprint(c, mk())\n",
        "fn mk()\n  [4, 5, 6]\nend\nstate kept = mk()\nkept[0] = kept[0] + n\nprint(kept, mk())\n",
        // Sorted, reversed and mapped in place.
        "fn mk()\n  [3, 1, 2]\nend\nlet s = sort(mk())\nlet m = map(mk(), fn(x) -> x * n)\nprint(s, m, mk())\n",
        // Inside the function itself, behind the value's last use.
        "fn mk(a)\n  let t = [1, 2, 3]\n  let u = [t, t]\n  bump(u[0], a)\nend\nprint(mk(n), mk(n + 1))\n",
    ];
    for body in cases {
        let src = format!("{prelude}{COUNTER}{body}");
        if Env::new().load_program(&src).is_err() {
            panic!("does not compile:\n{src}");
        }
        same_on_and_off(&src, 4);
    }
}

/// A function that hands back a container for its caller to fill is left
/// alone: the caller fills it in place because it is new, and a kept one
/// would be copied by the first write of every call.
#[test]
fn what_a_function_returns_is_not_kept_when_a_caller_writes_to_it() {
    let filled = format!(
        "fn build()\n  let a = [0, 0, 0, 0]\n  a\nend\n\
         fn table()\n  [[1, 2], [3, 4]]\nend\n\
         {COUNTER}let a = build()\nfor i in range(0, 4) do\n  a[i] = i * n\nend\nprint(a, table()[n % 2])\n"
    );
    let (_, w) = same_on_and_off(&filled, 3);
    let (_, kept_count, lines) = kept(&w);
    assert_eq!(kept_count, 1, "{lines:?}");
    assert!(lines[0].starts_with("table"), "{lines:?}");
    // No copy where there was none: the writes are in place, as without
    // the cache.
    if petal::stats::DUP_STATS_ENABLED {
        let copies = |policy: RunPolicy| {
            let mut w = world(&filled, policy);
            for _ in 0..3 {
                frame(&mut w);
            }
            w.env.dup_stats().total_count()
        };
        assert_eq!(copies(RunPolicy::REPLAY), copies(RunPolicy::REPLAY.with_static_cache(false)));
    }
}

#[test]
fn code_that_raises_is_not_kept_and_raises_on_every_call() {
    let src = format!(
        "fn bad(a)\n  let xs = [1, 2, 3]\n  let y = xs[7]\n  y + a\nend\n{COUNTER}print(n)\nif n > 1 then\n  print(bad(n))\nend\n"
    );
    let (frames, _) = same_on_and_off(&src, 4);
    assert!(frames[0].contains("=> ok"), "{}", frames[0]);
    for f in &frames[1..] {
        assert!(f.contains("error"), "{f}");
    }
}

// ---------------------------------------------------------------------------
// Recursion, memoized calls, forks, collections
// ---------------------------------------------------------------------------

#[test]
fn a_recursive_function_shares_one_slot_across_its_calls() {
    let src = format!(
        "fn walk(depth)\n  let step = {{dx: 1, dy: 2}}\n  if depth == 0 then\n    [step.dx]\n  else\n    let rest = walk(depth - 1)\n    append(rest, step.dy * depth)\n  end\nend\n\
         fn fib(k)\n  let base = [0, 1]\n  if k < 2 then base[k] else fib(k - 1) + fib(k - 2) end\nend\n\
         {COUNTER}print(n, walk(n + 2), fib(8))\n"
    );
    let (frames, w) = same_on_and_off(&src, 4);
    assert!(frames[0].contains("1 [1, 2, 4, 6] 21"), "{}", frames[0]);
    // `step` and `base`. Not the `[step.dx]` that `walk` returns: its
    // caller appends to it.
    assert_eq!(kept(&w).1, 2, "{:?}", kept(&w).2);
}

#[test]
fn a_function_that_reaches_its_own_constant_while_building_it() {
    // `seed` is built by calling a function that calls back in, on a path
    // that returns before the constant. Both calls are building it at once.
    let src = format!(
        "fn inner(k)\n  if k == 0 then return [9] end\n  let seed = inner(0)\n  [seed[0], k]\nend\n\
         {COUNTER}print(n, inner(1), inner(2), inner(0))\n"
    );
    same_on_and_off(&src, 3);
}

#[test]
fn memoized_calls_and_kept_constants_agree() {
    // The same arguments every frame, so a memoized call replays and never
    // reaches the code; new ones, so it records and does.
    let src = format!(
        "fn shade(v)\n  let ramp = [[0, 0], [5, 5], [9, 9]]\n  let pick = ramp[v % 3]\n  [pick[0] + v, len(ramp)]\nend\n\
         fn row(k)\n  let cells = [shade(1), shade(2), shade(k)]\n  cells\nend\n\
         {COUNTER}print(row(1), row(n), shade(4), shade(n % 2))\n"
    );
    let (_, w) = same_on_and_off(&src, 5);
    assert!(kept(&w).1 >= 1, "{:?}", kept(&w).2);
}

#[test]
fn a_fork_starts_without_the_constants_and_builds_its_own() {
    let src = format!(
        "fn mk(a)\n  let base = [[1, 2], [3, 4]]\n  [base[a % 2], a]\nend\n{COUNTER}print(n, mk(n), mk(n + 1))\n"
    );
    let drive = |policy: RunPolicy| {
        let mut w = world(&src, policy);
        let mut log = vec![frame(&mut w), frame(&mut w)];
        let fork = w.env.fork_execution(w.sid).unwrap();
        let mut f = World { env: std::mem::replace(&mut w.env, Env::new()), pid: w.pid, sid: fork };
        log.push(frame(&mut f));
        log.push(frame(&mut f));
        // Back on the original stack.
        let mut w = World { env: f.env, pid: w.pid, sid: w.sid };
        log.push(frame(&mut w));
        log.push(frame(&mut w));
        (log, kept(&w).1)
    };
    let (on, filled) = drive(RunPolicy::REPLAY);
    let (off, _) = drive(RunPolicy::REPLAY.with_static_cache(false));
    assert_eq!(on, off);
    assert_eq!(filled, 1);
}

#[test]
fn kept_constants_survive_collections() {
    let src = format!(
        "fn mk(a)\n  let table = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]\n  let label = {{name: \"row\", cells: table}}\n  [label.cells[a % 3], label.name]\nend\n\
         {COUNTER}let junk = for i in range(0, 30000) do [i, \"x{{i}}\", {{k: i}}] end\nprint(n, mk(n), len(junk))\n"
    );
    let mut w = world(&src, RunPolicy::REPLAY);
    let mut b = world(&src, RunPolicy::REPLAY.with_static_cache(false));
    let before = w.env.heap().collections();
    for i in 0..12 {
        assert_eq!(frame(&mut w), frame(&mut b), "frame {i}");
    }
    assert!(w.env.heap().collections() > before + 1, "the frames collected");
    assert_eq!(kept(&w).1, 1, "{:?}", kept(&w).2);
}

// ---------------------------------------------------------------------------
// Reloads
// ---------------------------------------------------------------------------

/// Drive `texts` in turn on one stack: three frames of the first, then for
/// each later text a reload and three more (the first of them not reset,
/// as a host's is not).
fn through_reloads(texts: &[String], policy: RunPolicy, check: bool) -> (Vec<String>, World) {
    let mut w = world_with(&texts[0], policy, |e| e.set_static_check(check));
    let mut log = Vec::new();
    for _ in 0..3 {
        log.push(frame(&mut w));
    }
    for text in &texts[1..] {
        match w.env.reload_program(w.sid, text, None) {
            Ok(report) => log.push(format!("reload {:?} {:?}", report.outcome, report.static_lines())),
            Err(e) => log.push(format!("reload error {e}")),
        }
        log.push(run_frame(&mut w));
        log.push(frame(&mut w));
        log.push(frame(&mut w));
    }
    (log, w)
}

fn same_through_reloads(texts: &[String]) -> Vec<String> {
    let mut first = None;
    for name in POLICIES {
        let p = RunPolicy::parse(name).unwrap();
        let (on, w) = through_reloads(texts, p.with_static_cache(true), false);
        let (off, _) = through_reloads(texts, p.with_static_cache(false), false);
        let (checked, c) = through_reloads(texts, p.with_static_cache(true), true);
        // The reload report says what the cache did, so it is only the
        // same between the two sides that have one.
        let frames = |log: &[String]| -> Vec<String> {
            log.iter().filter(|l| !l.starts_with("reload ")).cloned().collect()
        };
        assert_eq!(frames(&on), frames(&off), "under {name}\n{}", texts.join("\n---\n"));
        assert_eq!(on, checked, "under {name}, checking\n{}", texts.join("\n---\n"));
        assert_eq!(mismatches(&c), Vec::<String>::new(), "under {name}");
        // And against the reload that keeps nothing.
        let mut all = world_with(&texts[0], p.with_static_cache(true), |e| e.set_static_follow(false));
        let mut log = Vec::new();
        for _ in 0..3 {
            log.push(frame(&mut all));
        }
        for text in &texts[1..] {
            let _ = all.env.reload_program(all.sid, text, None);
            log.push(run_frame(&mut all));
            log.push(frame(&mut all));
            log.push(frame(&mut all));
        }
        assert_eq!(frames(&on), log, "against a reload that rebuilds everything, under {name}");
        let _ = w;
        first.get_or_insert(on);
    }
    first.unwrap()
}

#[test]
fn a_literal_edited_inside_a_kept_constant_is_the_new_value_on_the_next_call() {
    let v0 = format!(
        "fn tint(a)\n  let ramp = [[10, 20], [30, 40]]\n  ramp[a % 2][0] + a\nend\n{COUNTER}print(n, tint(0), tint(1))\n"
    );
    let v1 = v0.replace("[10, 20]", "[11, 20]");
    let v2 = v1.replace("[30, 40]", "[35, 40]");
    let log = same_through_reloads(&[v0.clone(), v1, v2, v0]);
    assert!(log[2].contains("3 10 31"), "{}", log[2]);
    assert!(log.iter().any(|l| l.starts_with("reload Patched")), "{log:?}");
    assert!(log[4].contains("4 11 31"), "the frame after the edit: {}", log[4]);
    assert!(log[8].contains("7 11 36"), "{}", log[8]);
}

#[test]
fn a_constant_built_from_an_edited_top_level_value_is_rebuilt() {
    let v0 = format!(
        "let BASE = [[1, 2], [3, 4]]\nlet K = 5\n\
         fn pick(a)\n  let both = [BASE[0], BASE[1], [K, K]]\n  both[a % 3]\nend\n{COUNTER}print(n, pick(0), pick(1), pick(2))\n"
    );
    // The table, the number, a comment (moved text only), a new
    // declaration (a recompile), and a function the constant calls.
    let v1 = v0.replace("[1, 2]", "[1, 9]");
    let v2 = v1.replace("let K = 5", "let K = 6");
    let v3 = format!("// a comment\n{v2}");
    let v4 = v3.replace("let K = 6", "let K = 6\nlet EXTRA = [K]");
    let log = same_through_reloads(&[v0.clone(), v1, v2, v3, v4, v0]);
    assert!(log[4].contains("[1, 9]") && log[8].contains("[6, 6]"), "{log:?}");
}

#[test]
fn a_constant_that_calls_an_edited_function_is_rebuilt() {
    let v0 = format!(
        "fn mk(a, b)\n  {{a: a, b: b, sum: a + b}}\nend\n\
         fn use(k)\n  let fixed = mk(1, 2)\n  fixed.sum + k\nend\n{COUNTER}print(n, use(0), use(n))\n"
    );
    let v1 = v0.replace("sum: a + b", "sum: a * 10 + b");
    let v2 = v1.replace("mk(1, 2)", "mk(3, 2)");
    let log = same_through_reloads(&[v0.clone(), v1, v2, v0]);
    assert!(log[4].contains("4 12 16"), "{}", log[4]);
    assert!(log[8].contains("7 32 39"), "{}", log[8]);
}

#[test]
fn the_reload_report_counts_top_level_values_only() {
    let v0 = format!(
        "let BASE = [[1, 2], [3, 4]]\nfn pick(a)\n  let both = [BASE[0], [7, 7]]\n  both[a % 2]\nend\n{COUNTER}print(n, pick(n))\n"
    );
    let v1 = v0.replace("[7, 7]", "[8, 7]");
    let (log, w) = through_reloads(&[v0, v1], RunPolicy::REPLAY, false);
    let reload = log.iter().find(|l| l.starts_with("reload ")).unwrap();
    assert!(!reload.contains("rebuilt"), "nothing at the top level changed: {reload}");
    assert_eq!(kept(&w).1, 1);
}

#[test]
fn a_dragged_value_rebuilds_the_constants_built_from_it_when_the_drag_pauses() {
    use petal::static_cache::StaticDebounce;
    use std::time::Duration;
    // `SLOW` is built from `K` and is slow enough to be put off; `pick`
    // builds a constant from `SLOW`.
    let src = "config let K = 3\n\
         fn build(k)\n  for i in range(0, 4000) do (i * k) % 97 end\nend\n\
         let SLOW = build(K)\n\
         fn pick(a)\n  let pair = [SLOW[1], SLOW[2], len(SLOW)]\n  pair[a % 3]\nend\n\
         print(K, pick(0), pick(1), pick(2))\n";
    let drive = |policy: RunPolicy, debounce: Option<StaticDebounce>| {
        let mut w = world_with(src, policy, |e| e.set_static_debounce(debounce));
        let mut log = vec![frame(&mut w), frame(&mut w)];
        for v in [4, 5, 6] {
            w.env.set_config_value(w.sid, None, "K", &petal::static_value::StaticValue::Int(v)).unwrap();
            log.push(frame(&mut w));
        }
        // The drag pauses: whatever was put off is built now.
        w.env.flush_static_rebuilds(w.sid);
        log.push(frame(&mut w));
        log.push(frame(&mut w));
        log
    };
    // A threshold of nothing puts every rebuild off after the first tick.
    let lazy = StaticDebounce { threshold: Duration::ZERO, quiet: Duration::from_secs(3600) };
    let on = drive(RunPolicy::REPLAY, Some(lazy));
    let off = drive(RunPolicy::REPLAY.with_static_cache(false), None);
    // While the drag lasts the cache serves the old table on purpose; what
    // must agree is the frames before it and the frames after it paused.
    assert_eq!(on[..2], off[..2]);
    assert_eq!(on[5..], off[5..], "after the drag paused\non  {on:?}\noff {off:?}");
    assert!(on[6].contains("6 6 12 4000"), "{}", on[6]);
}

// ---------------------------------------------------------------------------
// Known differences, pinned
// ---------------------------------------------------------------------------

/// `freeze` and `frozen` are about the heap object, so they can tell a
/// kept value from one built on every call, as they can at the top level
/// (`static_cache.rs`, `freezing_a_kept_value_freezes_it_once`).
#[test]
fn freezing_what_a_function_returned_freezes_the_kept_value() {
    let src = "fn mk()\n  [[1, 2], [3, 4]]\nend\nprint(frozen(mk()))\nfreeze(mk())\nprint(frozen(mk()), mk())\n";
    let run = |policy: RunPolicy| {
        let mut w = world(src, policy);
        (0..2).map(|_| frame(&mut w).lines().take(2).collect::<Vec<_>>().join(" / ")).collect::<Vec<_>>()
    };
    let off = run(RunPolicy::REPLAY.with_static_cache(false));
    let on = run(RunPolicy::REPLAY);
    assert_eq!(off[1], "false / false [[1, 2], [3, 4]]", "a new list every call");
    assert_eq!(on[1], "true / true [[1, 2], [3, 4]]", "one list");
}

#[test]
fn the_switch_turns_function_constants_off_alone() {
    let src = format!(
        "let TOP = [[1, 2], [3, 4]]\nfn mk(a)\n  let t = [5, 6, 7]\n  t[a % 3] + TOP[0][0]\nend\n{COUNTER}print(n, mk(n))\n"
    );
    let without = RunPolicy::parse("replay-fnstatic").unwrap();
    assert!(without.opts.static_cache && !without.opts.static_in_functions);
    assert_eq!(without.name().as_deref(), Some("replay-fnstatic"));
    let mut w = world(&src, without);
    let mut full = world(&src, RunPolicy::REPLAY);
    for _ in 0..3 {
        assert_eq!(frame(&mut w), frame(&mut full));
    }
    let values = w.env.static_values(w.sid).unwrap();
    assert_eq!((values.slots.len(), values.in_functions.len()), (1, 0));
    let values = full.env.static_values(full.sid).unwrap();
    assert_eq!((values.slots.len(), values.in_functions.len()), (1, 1));
}
