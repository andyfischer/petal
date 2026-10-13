//! Review of the phase after S2 (docs/tasks/static-value-optimization.md,
//! "Review of the phase after S2"): a store per value, the kept parse and
//! classification, the cache under observation, and constants inside
//! function bodies.
//!
//! Every case drives one list of steps (frames, reloads, host calls)
//! through four worlds: the cache on, the cache off, the cache on and
//! checking itself, and the cache on with every reload rebuilding
//! everything. What a host could tell apart must be the same in all four.

use petal::env::Env;
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::static_value::StaticValue;
use petal::value::Value;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

#[derive(Clone)]
enum Step {
    /// Reset and run, as a host's frame.
    Frame,
    /// Run without a reset, as the first run after a reload is.
    Run,
    Reload(String),
    /// `Env::set_config_value` of a top-level name to a number.
    Set(&'static str, i64),
    /// A host call of a function that takes no arguments.
    Call(&'static str),
    /// A host call with one integer argument.
    CallWith(&'static str, i64),
}

fn world(src: &str, policy: RunPolicy, setup: &dyn Fn(&mut Env)) -> World {
    let mut env = Env::new();
    setup(&mut env);
    env.set_policy(policy);
    env.set_echo(false);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("compile: {e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

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

fn after(w: &mut World, result: Result<Value, String>) -> String {
    let result = match result {
        Ok(v) => format!("ok {}", petal::value::value_to_display_string(&v, w.env.heap_for(w.sid).unwrap())),
        Err(e) => format!("error {e}"),
    };
    let out = w.env.take_output().join("\n");
    let state = serde_json::Value::Object(w.env.get_state_json(w.pid, w.sid));
    format!("{out}\n=> {result}\nstate {state}{}", observed(w))
}

fn step(w: &mut World, s: &Step) -> Option<String> {
    Some(match s {
        Step::Frame => {
            w.env.reset_stack(w.sid).unwrap();
            let r = w.env.run(w.sid).map_err(|e| e.to_string());
            after(w, r)
        }
        Step::Run => {
            let r = w.env.run(w.sid).map_err(|e| e.to_string());
            after(w, r)
        }
        Step::Reload(text) => {
            // What the report says is the cache's own business; that the
            // reload took or failed is not.
            return Some(match w.env.reload_program(w.sid, text, None) {
                Ok(r) => format!("reload {:?}", r.outcome),
                Err(e) => format!("reload error {e}"),
            });
        }
        Step::Set(name, v) => {
            return Some(match w.env.set_config_value(w.sid, None, name, &StaticValue::Int(*v)) {
                Ok(r) => format!("set {:?}", r.outcome),
                Err(e) => format!("set error {e:?}"),
            });
        }
        Step::Call(name) => {
            let r = w.env.call_function(w.sid, name, &[]);
            after(w, r)
        }
        Step::CallWith(name, v) => {
            let r = w.env.call_function(w.sid, name, &[Value::Int(*v)]);
            after(w, r)
        }
    })
}

const POLICIES: [&str; 5] = ["replay", "replay-memo", "fast", "baseline+memo", "baseline+static"];

fn mismatches(w: &World) -> Vec<String> {
    w.env.static_values(w.sid).map(|v| v.mismatches).unwrap_or_default()
}

/// Drive `steps` through the four worlds under every policy, with and
/// without a host observing. Returns the log of the default policy.
fn same(src: &str, steps: &[Step]) -> Vec<String> {
    let mut first = None;
    for observe in [false, true] {
        for name in POLICIES {
            let p = RunPolicy::parse(name).unwrap();
            let watch = move |e: &mut Env| {
                if observe {
                    e.observations_mut().enable();
                }
            };
            let mut on = world(src, p.with_static_cache(true), &watch);
            let mut off = world(src, p.with_static_cache(false), &watch);
            let mut checked = world(src, p.with_static_cache(true), &|e: &mut Env| {
                watch(e);
                e.set_static_check(true);
            });
            let mut all = world(src, p.with_static_cache(true), &|e: &mut Env| {
                watch(e);
                e.set_static_follow(false);
            });
            let mut log = Vec::new();
            for (i, s) in steps.iter().enumerate() {
                let what = format!("step {i} under {name}{}\n{src}", if observe { ", observed" } else { "" });
                let a = step(&mut on, s);
                assert_eq!(a, step(&mut off, s), "the cache on against off, {what}");
                assert_eq!(a, step(&mut checked, s), "the cache checking itself, {what}");
                assert_eq!(a, step(&mut all, s), "against a reload that rebuilds everything, {what}");
                log.extend(a);
            }
            assert_eq!(mismatches(&checked), Vec::<String>::new(), "under {name}\n{src}");
            first.get_or_insert(log);
        }
    }
    first.unwrap()
}

fn frames(n: usize) -> Vec<Step> {
    vec![Step::Frame; n]
}

/// How many function-body slots hold a value after `n` frames.
fn kept_in_functions(src: &str, n: usize) -> usize {
    let mut w = world(src, RunPolicy::REPLAY, &|_| {});
    for _ in 0..n {
        step(&mut w, &Step::Frame);
    }
    let values = w.env.static_values(w.sid).unwrap_or_default();
    values
        .in_functions
        .iter()
        .filter(|(_, s)| matches!(s, petal::env::StaticSlotState::Filled(_)))
        .count()
}

const COUNTER: &str = "state n = 0\nn = n + 1\n";

// ---------------------------------------------------------------------------
// Constants inside function bodies
// ---------------------------------------------------------------------------

/// Every way a value of the call reaches code that reads no parameter by
/// name: through a closure made per call, at one or two removes, through a
/// function handed in or read out of a table, through a top-level function
/// that captured something dynamic.
#[test]
fn a_value_of_the_call_reached_through_a_function_is_never_kept() {
    let cases = [
        // A nested declaration that captures the parameter, called here.
        "fn outer(x)\n  fn inner()\n    [x, 1]\n  end\n  let v = inner()\n  [v, [5, 6]]\nend\nprint(n, outer(n), outer(n + 1), outer(0))\n",
        // The same through a lambda bound to a name, and through `map`.
        "fn outer(x)\n  let g = fn() -> [x, 1]\n  let v = g()\n  [v, [5, 6]]\nend\nprint(n, outer(n), outer(n + 1))\n",
        "fn outer(x)\n  let v = map([1, 2, 3], fn(e) -> [e + x])\n  [v, [5, 6]]\nend\nprint(n, outer(n), outer(n + 1))\n",
        // Two removes: `user` captures `inner`, which captured `k`.
        "fn make(k)\n  fn inner()\n    [k, 0]\n  end\n  fn user()\n    let got = [inner(), [7, 7]]\n    got\n  end\n  user\nend\nlet a = make(n)\nlet b = make(n + 10)\nprint(n, a(), b(), a())\n",
        // The same with constant arguments: one function, two closures,
        // one slot.
        "fn make(k)\n  fn inner()\n    [k, 0]\n  end\n  fn user()\n    let got = [inner(), [7, 7]]\n    got\n  end\n  user\nend\nlet a = make(1)\nlet b = make(2)\nprint(n, a(), b(), a())\n",
        // A function handed in.
        "fn apply(g)\n  let v = [g(), 1]\n  v\nend\nprint(n, apply(fn() -> n), apply(fn() -> [n]), apply(fn() -> 3))\n",
        // A top-level function that captured a value of the frame.
        "let now = n * 2\nfn cur()\n  [now, 1]\nend\nfn f()\n  let xs = [cur(), [1, 2]]\n  xs\nend\nprint(n, f(), f())\n",
        // A top-level name that is one function or another by the frame.
        "fn one()\n  [1]\nend\nfn two()\n  [2]\nend\nlet pick = if n % 2 == 0 then one else two end\nfn f()\n  let xs = [pick(), [3]]\n  xs\nend\nprint(n, f(), f())\n",
        // A function read out of a table.
        "let OPS = {a: fn() -> [n, 1], b: fn() -> [4, 4]}\nfn f()\n  let xs = [OPS.a(), OPS.b()]\n  xs\nend\nprint(n, f())\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        if let Err(e) = Env::new().load_program(&src) {
            panic!("does not compile: {e}\n{src}");
        }
        same(&src, &frames(4));
    }
}

/// A name inside a function is whatever is nearest: a parameter, a local
/// or a loop variable of the same name as a kept top-level table is not
/// that table.
#[test]
fn a_parameter_or_local_named_like_a_kept_table_is_not_the_table() {
    let cases = [
        "let K = [1, 2]\nfn f(K)\n  let v = [K, 0]\n  v\nend\nprint(n, f(n), f([n]), K)\n",
        "let K = [1, 2]\nfn f(a)\n  let K = [a, a]\n  let v = [K, 0]\n  v\nend\nprint(n, f(n), f(n + 1), K)\n",
        "let K = [1, 2]\nfn f(a)\n  let out = for K in range(0, 2) do [K + a, [9]] end\n  out\nend\nprint(n, f(n), K)\n",
        "let K = [1, 2]\nfn f(a, k = K)\n  let v = [k, a]\n  v\nend\nprint(n, f(n), f(n, [n]))\n",
        "fn f(a, k = [0, 0])\n  k[0] = a\n  k\nend\nprint(n, f(n), f(n + 1), f(n, [7, 7]))\n",
        "fn f(a, k = [0, 0])\n  let v = [k, [1, 1]]\n  v\nend\nprint(n, f(n), f(n, [n]))\n",
        // Rebound by a branch the call decides, and by a loop it bounds.
        "let K = [1, 2]\nfn f(a)\n  let k = K\n  if a % 2 == 0 then\n    k = [a]\n  end\n  let v = [k, [3]]\n  v\nend\nprint(n, f(n), f(n + 1))\n",
        "fn f(a)\n  let acc = [0]\n  for i in range(0, a % 3) do\n    acc = [acc, i]\n  end\n  let v = [acc, [3]]\n  v\nend\nprint(n, f(n), f(n + 1))\n",
        "fn f(a)\n  let i = 0\n  let acc = [[0]]\n  while i < a % 4 do\n    acc = append(acc, [i])\n    i = i + 1\n  end\n  [acc, [8, 8]]\nend\nprint(n, f(n), f(n + 1))\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        if let Err(e) = Env::new().load_program(&src) {
            panic!("does not compile: {e}\n{src}");
        }
        same(&src, &frames(4));
    }
}

/// More routes by which something writes to a value a function keeps.
#[test]
fn more_routes_that_write_to_what_a_function_keeps() {
    let cases = [
        // An element of a kept list is the loop variable, and is written.
        "fn f(a)\n  let out = []\n  for c in [[1, 2], [3, 4]] do\n    c[0] = a\n    out = append(out, c)\n  end\n  out\nend\nprint(n, f(n), f(n + 1))\n",
        // A kept list as a loop's first accumulator value.
        "fn base()\n  [[0, 0], [1, 1]]\nend\nfn f(a)\n  let acc = base()\n  for i in range(0, 2) do\n    acc[i][0] = a + i\n  end\n  acc\nend\nprint(n, f(n), base(), f(n + 1), base())\n",
        // Held by state across frames and written there.
        "fn mk()\n  {hits: [0, 0], name: \"a\"}\nend\nstate held = mk()\nheld.hits[n % 2] = held.hits[n % 2] + n\nprint(n, held, mk())\n",
        "fn mk()\n  [[1], [2]]\nend\nstate rows = []\nrows = append(rows, mk()[n % 2])\nrows[0][0] = n\nprint(n, rows, mk())\n",
        // Held by a `var` and written through it inside a function.
        "fn mk()\n  [4, 5, 6]\nend\nvar c = mk()\nfn poke(v)\n  set c[1] = v\nend\npoke(n)\nprint(n, get c, mk())\n",
        // Written through a second name for an element, in a callee.
        "fn mk()\n  [[1, 2], [3, 4]]\nend\nfn edit(rows, v)\n  let r = rows[1]\n  r[1] = v\n  [rows, r]\nend\nprint(n, edit(mk(), n), mk())\n",
        // Concatenated onto, twice, from the kept value.
        "fn mk()\n  [1, 2]\nend\nlet a = mk() ++ [n]\nlet b = mk()\nb = b ++ [n]\nb = b ++ [n]\nprint(n, a, b, mk())\n",
        // A record spread and a field written after.
        "fn mk()\n  {x: 1, inner: {y: 2}}\nend\nlet r = mk()\nr.inner.y = n\nlet q = mk()\nq.x = n\nprint(n, r, q, mk())\n",
        // A constant of one function built from the constant of another,
        // and the outer one's caller writes.
        "fn leaf()\n  [0, 0]\nend\nfn tree()\n  let t = [leaf(), leaf()]\n  t\nend\nfn use(a)\n  let t = tree()\n  let l = t[0]\n  l[0] = a\n  [t, l, leaf()]\nend\nprint(n, use(n), tree())\n",
    ];
    for body in cases {
        let src = format!("{COUNTER}{body}");
        if let Err(e) = Env::new().load_program(&src) {
            panic!("does not compile: {e}\n{src}");
        }
        same(&src, &frames(4));
    }
}

/// A host calls a function directly, between frames and straight after a
/// reload, before the root code has run under the new text.
#[test]
fn a_host_call_after_a_reload_does_not_read_a_constant_of_the_old_text() {
    let v0 = format!(
        "let BASE = 10\nfn ramp()\n  let r = [[BASE, 20], [30, 40]]\n  r\nend\nfn at(i)\n  let r = [[1, 2], [3, 4]]\n  r[i % 2]\nend\n{COUNTER}print(n, ramp(), at(n))\n"
    );
    let patched = v0.replace("[30, 40]", "[30, 41]");
    let patched2 = patched.replace("[3, 4]", "[3, 5]");
    let recompiled = patched2.replace("print(n, ramp()", "print(n + 100, ramp()");
    let steps = vec![
        Step::Frame,
        Step::Call("ramp"),
        Step::CallWith("at", 1),
        Step::Reload(patched),
        Step::Call("ramp"),
        Step::CallWith("at", 1),
        Step::Reload(patched2),
        Step::CallWith("at", 1),
        Step::Call("ramp"),
        Step::Run,
        Step::Frame,
        Step::Set("BASE", 11),
        Step::Call("ramp"),
        Step::Frame,
        Step::Reload(recompiled),
        Step::Call("ramp"),
        Step::CallWith("at", 3),
        Step::Run,
        Step::Frame,
        Step::Set("BASE", 12),
        Step::Call("ramp"),
        Step::Frame,
        Step::Reload(v0.clone()),
        Step::Call("ramp"),
        Step::Frame,
    ];
    let log = same(&v0, &steps);
    assert!(log.iter().any(|l| l.contains("[[10, 20], [30, 41]]")), "{log:#?}");
    assert!(log.iter().any(|l| l.contains("[[12, 20], [30, 41]]")), "{log:#?}");
    assert!(kept_in_functions(&v0, 2) >= 2);
}

/// A failed reload between two good ones, of a function's own constant.
#[test]
fn a_failed_reload_leaves_no_constant_of_a_text_that_never_loaded() {
    let v0 = format!(
        "let TILES = [[1, 2], [3, 4]]\nfn pick(i)\n  let t = [TILES[0], [5, 6]]\n  t[i % 2]\nend\n{COUNTER}print(n, pick(n), TILES)\n"
    );
    let broken = v0.replace("[5, 6]", "[5, 7");
    let v1 = v0.replace("[5, 6]", "[5, 7]");
    let v2 = v1.replace("[[1, 2], [3, 4]]", "[[1, 9], [3, 4]]");
    let steps = vec![
        Step::Frame,
        Step::Frame,
        Step::Reload(broken.clone()),
        Step::Frame,
        Step::Reload(v1.clone()),
        Step::Run,
        Step::Frame,
        Step::Reload(broken),
        Step::Frame,
        Step::Reload(v2),
        Step::Run,
        Step::Frame,
        Step::Reload(v0.clone()),
        Step::Run,
        Step::Frame,
        Step::Reload(v1),
        Step::Run,
        Step::Frame,
    ];
    let log = same(&v0, &steps);
    assert!(log.iter().any(|l| l.starts_with("reload error")), "{log:#?}");
    assert!(log.iter().any(|l| l.contains("[[1, 9], [3, 4]]")), "{log:#?}");
}

/// A reload that edits the function itself: its parameters, what it
/// captures, which of its values depend on the call.
#[test]
fn a_reload_that_makes_a_constant_depend_on_the_call() {
    let v0 = format!(
        "let K = [1, 2]\nfn f(a)\n  let t = [K, [0, 0]]\n  let u = [t, a]\n  u\nend\n{COUNTER}print(n, f(n), f(n + 1))\n"
    );
    let texts = [
        v0.replace("let t = [K, [0, 0]]", "let t = [K, [a, 0]]"),
        v0.replace("fn f(a)", "fn f(K)").replace("let u = [t, a]", "let u = [t, K]"),
        v0.replace("let K = [1, 2]", "let K = [n, 2]").replace(COUNTER, "").replacen("let K", &format!("{COUNTER}let K"), 1),
        v0.replace("fn f(a)\n", "fn f(a)\n  let K = [a]\n"),
        v0.clone(),
    ];
    let mut steps = frames(3);
    for t in texts {
        steps.push(Step::Reload(t));
        steps.push(Step::Run);
        steps.push(Step::Frame);
        steps.push(Step::Frame);
    }
    same(&v0, &steps);
}

// ---------------------------------------------------------------------------
// A store per value
// ---------------------------------------------------------------------------

/// A kept table inside a rebuilt one, then written through every name the
/// program has for it.
#[test]
fn a_kept_table_inside_a_rebuilt_one_is_written_through_no_name() {
    let v0 = format!(
        "let B = [1, 2]\nlet C = [B, 1]\nlet D = [C, B]\nfn bump(xs, v)\n  xs[0] = v\n  xs\nend\n{COUNTER}\
         let c0 = C[0]\nc0[0] = n\nlet d = D[0][0]\nd[1] = n\nstate s = D\ns[1][0] = s[1][0] + n\n\
         print(n, B, C, D, c0, d, s, bump(D[1], n), bump(C[0], n))\n"
    );
    let mut steps = frames(2);
    for text in [
        v0.replace("let C = [B, 1]", "let C = [B, 7]"),
        v0.replace("let C = [B, 1]", "let C = [B, 7]").replace("let D = [C, B]", "let D = [C, B, 3]"),
        v0.replace("let B = [1, 2]", "let B = [1, 5]"),
        v0.clone(),
    ] {
        steps.push(Step::Reload(text));
        steps.push(Step::Run);
        steps.push(Step::Frame);
        steps.push(Step::Frame);
    }
    steps.push(Step::Set("C[1]", 9));
    steps.push(Step::Frame);
    steps.push(Step::Frame);
    same(&v0, &steps);
}

/// A frame raises between two values, a reload edits the one in front,
/// and the one behind was never built.
#[test]
fn a_raise_between_two_values_and_then_an_edit_to_the_first() {
    let v0 = format!(
        "let A = [1, 2, 3]\nlet I = 5\nlet BAD = A[I]\nlet C = [A, BAD]\n{COUNTER}print(n, A, C)\n"
    );
    let steps = vec![
        Step::Frame,
        Step::Frame,
        Step::Reload(v0.replace("let A = [1, 2, 3]", "let A = [1, 2, 4]")),
        Step::Run,
        Step::Frame,
        Step::Set("I", 2),
        Step::Frame,
        Step::Frame,
        Step::Reload(v0.replace("let I = 5", "let I = 1")),
        Step::Run,
        Step::Frame,
        Step::Set("I", 9),
        Step::Frame,
        Step::Reload(v0.replace("let I = 5", "let I = 0").replace("[1, 2, 3]", "[8, 2, 3]")),
        Step::Run,
        Step::Frame,
    ];
    same(&v0, &steps);
}

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

/// One function called from several kept segments and from dynamic code:
/// a host reads the locals of whichever call came last, and a skipped
/// segment has to put its own back in the same place.
const CALLS: &str = "\
fn widths(ts, k)
  let scale = k * 2
  let out = map(ts, fn(t) -> t.w * scale)
  let total = reduce(out, 0, fn(a, b) -> a + b)
  [out, total]
end
let T1 = [{w: 1}, {w: 2}]
let W1 = widths(T1, 1)
let T2 = [{w: 5}, {w: 6}]
let W2 = widths(T2, 2)
let BOTH = [widths(T1, 1), 3]
state n = 0
n = n + 1
let live = widths([{w: n}], n)
let AGAIN = widths(T2, 2)
print(n, W1, W2, BOTH, live, AGAIN)
";

#[test]
fn the_locals_of_a_function_called_from_several_segments_are_those_of_the_last_call() {
    let mut steps = frames(3);
    for text in [
        CALLS.replace("[{w: 1}, {w: 2}]", "[{w: 1}, {w: 3}]"),
        CALLS.replace("[widths(T1, 1), 3]", "[widths(T1, 1), 4]"),
        CALLS.replace("let scale = k * 2", "let scale = k * 3"),
        CALLS.replace("let AGAIN = widths(T2, 2)", "let AGAIN = widths(T1, 2)"),
        CALLS.replace("print(n, W1", "print(n + 1, W1"),
        CALLS.to_string(),
    ] {
        steps.push(Step::Reload(text));
        steps.push(Step::Run);
        steps.push(Step::Frame);
        steps.push(Step::Frame);
    }
    steps.push(Step::Set("T2[0].w", 9));
    steps.push(Step::Frame);
    steps.push(Step::Frame);
    let log = same(CALLS, &steps);
    assert!(log.iter().any(|l| l == "set Patched"), "{log:#?}");
}

/// A host starts and stops observing while a stack is alive: a segment
/// built while nobody watched has written nothing down, and must not be
/// skipped as if it had.
#[test]
fn a_host_that_starts_observing_later_reads_what_one_that_always_did_reads() {
    for name in POLICIES {
        let p = RunPolicy::parse(name).unwrap();
        let mut on = world(CALLS, p.with_static_cache(true), &|_| {});
        let mut off = world(CALLS, p.with_static_cache(false), &|_| {});
        let mut log = Vec::new();
        for round in 0..4 {
            for w in [&mut on, &mut off] {
                if round % 2 == 1 {
                    w.env.observations_mut().enable();
                } else {
                    w.env.observations_mut().disable();
                }
            }
            for i in 0..3 {
                let a = step(&mut on, &Step::Frame);
                assert_eq!(a, step(&mut off, &Step::Frame), "round {round} frame {i} under {name}");
                log.extend(a);
            }
            if round == 2 {
                let text = CALLS.replace("[{w: 5}, {w: 6}]", "[{w: 5}, {w: 7}]");
                for w in [&mut on, &mut off] {
                    w.env.reload_program(w.sid, &text, None).unwrap();
                }
            }
        }
        assert!(log.iter().any(|l| l.contains("observed")), "{log:#?}");
    }
}

/// Under a name the program can no longer reach, a host still reads the
/// container, edits made in place included. Every shape here hands a table
/// nothing reads again to something that may edit it where it is.
#[test]
fn a_table_edited_in_place_under_a_dead_name_is_observed_the_same() {
    let prelude = "fn bump(xs, v)\n  xs[0] = v\n  xs\nend\nfn outer(xs)\n  bump(xs, 7)\nend\nfn add(xs)\n  append(xs, 4)\nend\n\
fn cat(xs)\n  xs ++ [4]\nend\nfn setx(r)\n  r.x = 5\n  r\nend\nfn fill(xs)\n  for i in range(0, len(xs)) do\n    xs[i] = i * 10\n  end\n  xs\nend\n\
fn deep(rows)\n  rows[0][0] = 99\n  rows\nend\nfn first(rows)\n  let r = rows[0]\n  r[0] = 42\n  r\nend\n";
    let cases = [
        "let A = [1, 2, 3]\nlet B = bump(A, 9)\n",
        "let A = [1, 2, 3]\nlet B = outer(A)\n",
        "let A = [1, 2, 3]\nlet B = add(A)\n",
        "let A = [1, 2, 3]\nlet B = cat(A)\n",
        "let A = {x: 1, y: 2}\nlet B = setx(A)\n",
        "let A = [1, 2, 3]\nlet B = fill(A)\n",
        "let A = [[1, 2], [3, 4]]\nlet B = deep(A)\n",
        "let A = [[1, 2], [3, 4]]\nlet B = first(A)\n",
        "let A = [3, 1, 2]\nlet B = sort(A)\n",
        "let A = [3, 1, 2]\nlet B = reverse(A)\n",
        "let A = [3, 1, 2]\nlet B = map(A, fn(x) -> x * 2)\n",
        "let A = [3, 1, 2]\nlet B = filter(A, fn(x) -> x > 1)\n",
        "let A = [3, 1, 2]\nlet B = append(A, 4)\n",
        "let A = [3, 1, 2]\nlet B = A ++ [4]\n",
        "let A = [3, 1, 2]\nlet B = bump(bump(A, 1), 2)\n",
        "let A = [3, 1, 2]\nlet H = [A]\nlet B = bump(H[0], 5)\n",
        "let A = [3, 1, 2]\nlet H = {a: A}\nlet B = deep([H.a])\n",
        "let A = [3, 1, 2]\nlet C = A\nlet B = bump(C, 5)\n",
        "let A = [3, 1, 2]\nlet B = bump(if len(A) > 2 then A else [0] end, 5)\n",
        "let A = [3, 1, 2]\nlet B = for i in range(0, 2) do bump(A, i) end\n",
        "let A = [3, 1, 2]\nlet B = A\nB[0] = 8\n",
        "let A = [[3], [1]]\nlet B = A[0]\nB[0] = 8\n",
        "let A = [[3], [1]]\nlet B = map(A, fn(r) -> bump(r, 0))\n",
        "let A = [[3], [1]]\nlet B = for r in A do bump(r, 0) end\n",
    ];
    for body in cases {
        for tail in ["print(n, B)\n", "print(n, B, len(B))\nlet late = B\n"] {
            let src = format!("{prelude}{body}{COUNTER}{tail}");
            if let Err(e) = Env::new().load_program(&src) {
                panic!("does not compile: {e}\n{src}");
            }
            let mut steps = frames(3);
            steps.push(Step::Reload(src.replace("print(n, B", "print(n + 1, B")));
            steps.push(Step::Run);
            steps.push(Step::Frame);
            steps.push(Step::Frame);
            same(&src, &steps);
        }
    }
}

/// Classes, enum data, methods, overloads, cells read with `get`, a name
/// rebound below the function that captured it, patterns.
#[test]
fn constants_built_from_classes_enums_cells_and_rebound_names() {
    let prelude = "class Box\n  w,\n  tags,\nend\nfn Box.area(b) b.w * 3 end\nfn Box.grow(b, v)\n  b.w = b.w + v\n  b\nend\n\
enum Kind\n  small(cost: 3, at: [1, 2]),\n  large(cost: 30, at: [3, 4]),\nend\nfn pick(x) [x] end\nfn pick(x, y) [x, y] end\n";
    let cases = [
        // An instance built from constants, then grown by a method.
        "fn mk()\n  let b = Box(2, [1, 2])\n  b\nend\nfn use(a)\n  let b = mk()\n  [b.area(), b.grow(a).w, mk().w, mk().tags]\nend\nprint(n, use(n), use(n + 1))\n",
        "fn use(a)\n  let boxes = [Box(1, [0]), Box(2, [0])]\n  let first = boxes[0]\n  first.tags[0] = a\n  [first, boxes, map(boxes, fn(b) -> b.area())]\nend\nprint(n, use(n), use(n + 1))\n",
        // Enum data read inside a function, and written to through it.
        "fn costs(a)\n  let all = [Kind.small, Kind.large]\n  let at = all[a % 2].at\n  at[0] = a\n  [at, map(all, fn(k) -> k.cost), Kind.small.at]\nend\nprint(n, costs(n), costs(n + 1))\n",
        "fn name(k)\n  let table = [[Kind.small, \"s\"], [Kind.large, \"l\"]]\n  match k\n    when Kind.small -> table[0][1]\n    when _ -> table[1][1]\n  end\nend\nprint(n, name(Kind.small), name(if n % 2 == 0 then Kind.small else Kind.large end))\n",
        // Overloads.
        "fn both(a)\n  let one = pick([1, 2])\n  let two = pick([1, 2], [a])\n  [one, two, pick(a)]\nend\nprint(n, both(n), both(n + 1))\n",
        // A cell read with `get`, beside a constant.
        "var live = n\nfn f()\n  let t = [[1, 2], get live]\n  let u = [[3, 4], [5]]\n  [t, u]\nend\nprint(n, f())\nset live = n * 10\nprint(n, f())\n",
        // A name rebound below the function that captured it.
        "let K = [1, 2]\nfn f()\n  let v = [K, [0]]\n  v\nend\nprint(n, f())\nK = [n, 2]\nprint(n, f(), K)\n",
        "let K = [n, 2]\nfn f()\n  let v = [K, [0]]\n  v\nend\nprint(n, f())\nK = [1, 2]\nfn g()\n  let v = [K, [0]]\n  v\nend\nprint(n, f(), g(), K)\n",
        // A pattern's variables, and a guard.
        "fn f(p)\n  match p\n    when [a, b] if a > 1 -> [[a, b], [7, 7]]\n    when [a, b] -> [[b, a], [8, 8]]\n    when _ -> [[0], [9, 9]]\n  end\nend\nprint(n, f([n, 1]), f([1, n]), f(n))\n",
        "fn f(a)\n  let fixed = match [1, 2]\n    when [x, y] -> [[y, x], [x]]\n    when _ -> []\n  end\n  [fixed, a]\nend\nprint(n, f(n), f(n + 1))\n",
        // `config let` read inside a function.
    ];
    for body in cases {
        let src = format!("{prelude}{COUNTER}{body}");
        if let Err(e) = Env::new().load_program(&src) {
            panic!("does not compile: {e}\n{src}");
        }
        same(&src, &frames(4));
    }
    let src = format!(
        "config let SIZE = 4\nlet GRID = [SIZE, SIZE * 2]\nfn cell(i)\n  let dims = [[SIZE, SIZE], GRID]\n  let pad = [SIZE + 1]\n  [dims[i % 2], pad]\nend\n{COUNTER}print(n, cell(n))\n"
    );
    let steps = vec![
        Step::Frame,
        Step::Frame,
        Step::Set("SIZE", 5),
        Step::Frame,
        Step::Frame,
        Step::Set("SIZE", 6),
        Step::Call("cell_is_not_there"),
        Step::CallWith("cell", 1),
        Step::Frame,
        Step::Reload(src.replace("config let SIZE = 4", "config let SIZE = 7")),
        Step::CallWith("cell", 0),
        Step::Run,
        Step::Frame,
    ];
    let log = same(&src, &steps);
    assert!(log.iter().any(|l| l.contains("[[6, 12], [7]]")), "{log:#?}");
}

/// Two stacks of one program, a fork, and a restore: what one stack's
/// reload or patch does reaches the constants every other stack kept.
#[test]
fn a_patch_through_one_stack_reaches_the_constants_of_the_others() {
    let v0 = format!(
        "let BASE = [10, 20]\nlet PAIR = [BASE, 1]\nfn ramp(i)\n  let r = [[BASE[0], 2], [30, 40]]\n  r[i % 2]\nend\n{COUNTER}print(n, ramp(n), ramp(n + 1), PAIR)\n"
    );
    let v1 = v0.replace("[30, 40]", "[30, 41]");
    let v2 = v1.replace("let BASE = [10, 20]", "let BASE = [11, 20]");
    let v3 = v2.replace("print(n, ramp(n)", "print(n + 100, ramp(n)");
    for name in POLICIES {
        for observe in [false, true] {
            let p = RunPolicy::parse(name).unwrap();
            let drive = |policy: RunPolicy| {
                let mut env = Env::new();
                if observe {
                    env.observations_mut().enable();
                }
                env.set_policy(policy);
                env.set_echo(false);
                let pid = env.load_program(&v0).unwrap();
                let a = env.create_stack(pid).unwrap();
                let b = env.create_stack(pid).unwrap();
                let mut w = World { env, pid, sid: a };
                let mut log = Vec::new();
                let both = |w: &mut World, log: &mut Vec<String>, s: &Step| {
                    for sid in [a, b] {
                        w.sid = sid;
                        log.extend(step(w, s));
                    }
                };
                both(&mut w, &mut log, &Step::Frame);
                both(&mut w, &mut log, &Step::Frame);
                // A fork of `b`, taken while everything is kept.
                let fork = w.env.fork_execution(b).unwrap();
                for (text, through) in [(&v1, a), (&v2, b), (&v3, a), (&v0, b)] {
                    w.sid = through;
                    log.extend(step(&mut w, &Step::Reload(text.to_string())));
                    both(&mut w, &mut log, &Step::Frame);
                    both(&mut w, &mut log, &Step::Frame);
                    w.sid = fork;
                    log.extend(step(&mut w, &Step::Frame));
                    w.sid = b;
                    log.extend(step(&mut w, &Step::Set("BASE[1]", 21)));
                    both(&mut w, &mut log, &Step::Frame);
                    w.sid = fork;
                    log.extend(step(&mut w, &Step::Frame));
                }
                w.env.restore_execution(a, fork).unwrap();
                both(&mut w, &mut log, &Step::Frame);
                both(&mut w, &mut log, &Step::Frame);
                log
            };
            let on = drive(p.with_static_cache(true));
            let off = drive(p.with_static_cache(false));
            assert_eq!(on.len(), off.len());
            for (i, (x, y)) in on.iter().zip(&off).enumerate() {
                assert_eq!(x, y, "step {i} under {name}, observed {observe}\n{on:#?}");
            }
        }
    }
}

/// A function that captured a value of the frame, handed on as a value:
/// as an argument, inside a table, returned by another function, at the
/// top level and inside a function body.
#[test]
fn a_function_with_a_dynamic_capture_is_dynamic_wherever_it_is_handed() {
    let src = format!(
        "{COUNTER}let now = n * 2\nfn cur()\n  [now, 1]\nend\nfn apply(g)\n  g()\nend\nfn twice(g)\n  apply(g)\nend\n\
         fn give()\n  cur\nend\nfn wrap(g)\n  fn() -> [g(), 5]\nend\n\
         let TOP = [apply(cur), 0]\nlet TOP2 = map([1, 2], fn(x) -> cur())\nlet TABLE = {{f: cur, k: [1, 2]}}\n\
         let TOP3 = [TABLE.f(), 1]\nlet TOP4 = [give()(), wrap(cur)(), TABLE.k]\nlet LIST = [cur, give]\nlet TOP5 = [LIST[0](), LIST[1]()()]\n\
         fn f()\n  let v = [apply(cur), twice(cur), [1, 2]]\n  let w = [TABLE.f(), [3]]\n  let x = [give()(), wrap(cur)(), LIST[0](), TABLE.k]\n  [v, w, x]\nend\n\
         print(n, TOP, TOP2, TOP3, TOP4, TOP5, f())\n"
    );
    if let Err(e) = Env::new().load_program(&src) {
        panic!("does not compile: {e}\n{src}");
    }
    let mut steps = frames(4);
    steps.push(Step::Reload(src.replace("print(n, TOP", "print(n + 1, TOP")));
    steps.push(Step::Run);
    steps.push(Step::Frame);
    steps.push(Step::Frame);
    let log = same(&src, &steps);
    assert_ne!(log[1], log[2]);
}

/// What each kind of reload does to the constants of function bodies, as
/// the table shows it straight after the reload: text that only moved
/// leaves them, and anything else empties every one.
#[test]
fn a_reload_that_only_moves_text_leaves_function_constants_and_any_other_empties_them() {
    let v0 = format!(
        "let BASE = [10, 20]\nfn ramp(i)\n  let r = [[BASE[0], 2], [30, 40]]\n  r[i % 2]\nend\n{COUNTER}print(n, ramp(n))\n"
    );
    let filled = |w: &World| {
        let values = w.env.static_values(w.sid).unwrap_or_default();
        values
            .in_functions
            .iter()
            .filter(|(_, s)| matches!(s, petal::env::StaticSlotState::Filled(_)))
            .count()
    };
    let cases = [
        (format!("// a comment\n{v0}"), "Relocated", 1),
        (v0.replace("{COUNTER}", COUNTER).replace("print(n, ramp(n))", "print(n,  ramp(n))  // moved"), "Relocated", 1),
        (v0.replace("[30, 40]", "[30, 41]"), "Patched", 0),
        (v0.replace("let BASE = [10, 20]", "let BASE = [10, 21]"), "Patched", 0),
        (v0.replace("print(n, ramp(n))", "print(n + 1, ramp(n))"), "Recompiled", 0),
    ];
    for (text, outcome, left) in cases {
        let mut w = world(&v0, RunPolicy::REPLAY, &|_| {});
        step(&mut w, &Step::Frame);
        step(&mut w, &Step::Frame);
        assert_eq!(filled(&w), 1);
        let report = w.env.reload_program(w.sid, &text, None).unwrap();
        assert_eq!(format!("{:?}", report.outcome), outcome, "{text}");
        assert_eq!(filled(&w), left, "after a reload that {outcome}\n{text}");
        let mut steps = frames(2);
        steps.push(Step::Reload(text.clone()));
        steps.push(Step::Run);
        steps.push(Step::Frame);
        same(&v0, &steps);
    }
}

// ---------------------------------------------------------------------------
// The optimizer fix that came with the function-body fuzzer (4dbced6)
// ---------------------------------------------------------------------------

/// Relatives of the shape that fix was for: a name some control flow
/// rebinds to (part of) another value, in front of code that writes to it.
/// The optimizer on must print what `baseline` prints.
#[test]
fn a_name_rebound_to_another_value_by_control_flow_is_not_written_in_place() {
    let loop_ = "  for i in range(0, len(b)) do\n    b[i] = (b[i] + 17) % 97\n  end\n";
    let bodies = [
        // An element of the other value, a field of it, and a call on it.
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = other[0]\n  end\n",
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = {k: other[0]}.k\n  end\n",
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = same(other[0])\n  end\n",
        // Both arms, an else arm only, and a nested branch.
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = other[0]\n  else\n    b = other[1]\n  end\n",
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = sort([9, 8])\n  else\n    b = other[1]\n  end\n",
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    if p % 4 == 0 then\n      b = other[0]\n    end\n  end\n",
        // A branch as an expression, and a match.
        "let b = if p % 2 == 0 then other[0] else sort([3, 1, 2]) end\n",
        "let b = match p % 3\n    when 0 -> other[0]\n    when 1 -> sort([5, 4])\n    when _ -> other[1]\n  end\n",
        // A loop that rebinds it before the loop that writes.
        "let b = sort([3, 1, 2])\n  for j in range(0, p % 3) do\n    b = other[j % 2]\n  end\n",
        "let b = sort([3, 1, 2])\n  let j = 0\n  while j < p % 3 do\n    b = other[j % 2]\n    j = j + 1\n  end\n",
        // Through a second name.
        "let b = sort([3, 1, 2])\n  let c = other[0]\n  if p % 2 == 0 then\n    b = c\n  end\n",
        "let c = other[0]\n  let b = c\n  if p % 2 == 0 then\n    b = sort([3, 1, 2])\n  end\n",
        // Appended to and concatenated onto instead of written by index.
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = other[0]\n  end\n  b = append(b, p)\n  b = append(b, p)\n",
        "let b = sort([3, 1, 2])\n  if p % 2 == 0 then\n    b = other[0]\n  end\n  b = b ++ [p]\n  b = b ++ [p]\n",
    ];
    for body in bodies {
        let src = format!(
            "fn same(x)\n  x\nend\nfn f(p, other)\n  {body}{loop_}  b\nend\n\
             let ROWS = [[1, 2, 3], [4, 5, 6]]\nstate n = 0\nn = n + 1\n\
             let mine = [[7, 8, 9], [10, 11, 12]]\n\
             print(f(n, ROWS), f(n + 1, ROWS), ROWS)\nprint(f(n, mine), f(n + 1, mine), f(n + 2, mine), mine)\n"
        );
        if let Err(e) = Env::new().load_program(&src) {
            panic!("does not compile: {e}\n{src}");
        }
        let run = |name: &str| {
            let mut w = world(&src, RunPolicy::parse(name).unwrap(), &|_| {});
            (0..4).map(|_| step(&mut w, &Step::Frame).unwrap()).collect::<Vec<_>>()
        };
        let base = run("baseline");
        for name in ["fast", "replay", "replay-memo", "baseline+static", "fast-static"] {
            if RunPolicy::parse(name).is_err() {
                continue;
            }
            assert_eq!(run(name), base, "under {name}\n{src}");
        }
    }
}
