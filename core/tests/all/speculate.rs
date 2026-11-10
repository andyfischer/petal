//! `speculate(f, ...args)`: a hypothetical call (docs/features/speculate.md).
//!
//! The central property: a program with a `speculate` call in it behaves,
//! outside the call, exactly as the same program without it — the same
//! output, the same final value and the same persistent state, frame after
//! frame — under the baseline policy and under the fast one (optimizer,
//! in-place edits, memo, static cache), which must also agree with each
//! other. Around it, one test per rule: effects, emits, random draws,
//! nesting, memo, places.

use std::cell::Cell;

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::value::{self, Value};

/// Run `code` for `runs` frames on one stack, returning each frame's output
/// and value, and the final state rendered as sorted `key=value` pairs.
fn frames(code: &str, policy: RunPolicy, runs: usize) -> Result<(Vec<String>, String), String> {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_seed(11);
    let pid = env.load_program(code)?;
    let sid = env.create_stack(pid)?;
    let mut out = Vec::new();
    for i in 0..runs {
        if i > 0 {
            env.reset_stack(sid)?;
        }
        let v = env.run(sid)?;
        out.extend(env.take_output());
        out.push(format!("=> {}", value::value_to_display_string(&v, env.heap())));
    }
    let state = env.get_state_json(pid, sid);
    let mut pairs: Vec<String> = state.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    Ok((out, pairs.join(",")))
}

/// The programs of the no-leak property. `SPEC` is replaced by a
/// `speculate` call (or several) in one copy and by nothing in the other.
/// Each touches a different kind of mutable thing from inside the call:
/// slots first reached there, slots that already exist, `state var` cells
/// edited in place, plain `var` cells captured from outside, keyed loops,
/// `memo(key)`, collecting loops, random draws, records edited by path.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "function state and a top-level cell",
        "state var log = []
state frame = 0
fn view(k)
  state hits = 0
  hits += 1
  set log = append(get log, k)
  for i in range(0, 3) do
    state per = i
    per = per + k
  end
  hits
end
frame += 1
SPEC
let live = view(frame)
print(\"{frame} {live} {get log}\")",
    ),
    (
        "in-place edits through state var cells",
        "state var xs = [1, 2, 3]
state var rec = {a: 1, b: [1, 2]}
fn edit(v)
  set xs[0] = v
  set xs = append(get xs, v)
  set rec.b[1] = v
  set rec.a = v
  len(get xs)
end
SPEC
set xs[1] = (get xs)[1] + 1
set xs = append(get xs, len(get xs))
print(\"{get xs} {get rec}\")",
    ),
    (
        "a captured var and the call's own cells",
        "var counter = 0
fn bump(by)
  set counter = get counter + by
  var local = []
  for i in range(0, by) do set local = append(get local, i) end
  len(get local)
end
SPEC
bump(2)
print(get counter)",
    ),
    (
        "keyed loops, memo and a collecting loop",
        "state items = [{id: 1, n: 3}, {id: 2, n: 5}]
fn view(items)
  let rows = for it in items key it.id do
    state seen = 0
    seen += it.n
    memo(it.n) sq = it.n * it.n
    seen + sq
  end
  rows
end
SPEC
print(view(items))
items = [{id: 2, n: 5}, {id: 3, n: 1}]",
    ),
    (
        "random draws",
        "fn noisy(n)
  var acc = 0
  for i in range(0, n) do set acc = get acc + random() end
  get acc
end
SPEC
print(random())
print(random_int(1, 100))",
    ),
    (
        "a slot the live program reaches later",
        "fn widget(dx)
  state x = 10
  x = x + dx
  x
end
fn panel(dx)
  widget(dx)
end
SPEC
print(panel(1))",
    ),
    (
        "nested records and a state var written by path",
        "state var doc = {meetings: [{start: 1, room: 0}, {start: 4, room: 1}]}
fn move(i, room)
  set doc.meetings[i].room = room
  set doc.meetings[i].start = (get doc).meetings[i].start + 1
  get doc
end
SPEC
set doc.meetings[0].start = (get doc).meetings[0].start + 10
print(get doc)",
    ),
];

/// The calls each program's `SPEC` becomes, by program index.
const CALLS: &[&str] = &[
    "let r = speculate(view, 100)\nlet r2 = speculate(view, frame + 7)",
    "let r = speculate(edit, 99)",
    "let r = speculate(bump, 5)",
    "let r = speculate(view, [{id: 1, n: 9}, {id: 4, n: 2}])",
    "let r = speculate(noisy, 4)",
    "let r = speculate(panel, 100)",
    "let r = speculate(move, 1, 7)\nlet r2 = speculate(move, 0, 3)",
];

#[test]
fn nothing_a_speculative_call_does_outlives_it() {
    assert_eq!(PROGRAMS.len(), CALLS.len());
    for ((name, program), call) in PROGRAMS.iter().zip(CALLS) {
        let with = program.replace("SPEC", call);
        let without = program.replace("SPEC", "");
        for policy in [RunPolicy::BASELINE, RunPolicy::FAST] {
            let a = frames(&with, policy, 4).unwrap_or_else(|e| panic!("{name}: {e}"));
            let b = frames(&without, policy, 4).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(a, b, "{name}: the speculative call leaked (policy {policy:?})");
        }
    }
}

/// The differential oracle: the speculative call's *own* result (what it
/// sees and returns) is the same with the optimizer and memo on as with
/// both off.
#[test]
fn the_call_itself_agrees_with_the_optimizer_off() {
    for ((name, program), call) in PROGRAMS.iter().zip(CALLS) {
        let with = format!("{}\nprint(r)", program.replace("SPEC", call));
        let base = frames(&with, RunPolicy::BASELINE, 3).unwrap_or_else(|e| panic!("{name}: {e}"));
        let fast = frames(&with, RunPolicy::FAST, 3).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(base, fast, "{name}: optimizer on and off disagree");
    }
}

#[test]
fn state_reached_first_in_the_call_is_read_back_inside_it() {
    let mut env = Env::new();
    let v = env
        .run_source(
            "fn f()
  state a = 1
  a = a + 10
  state b = a * 2
  [a, b]
end
speculate(f).value",
        )
        .unwrap();
    assert_eq!(value::value_to_display_string(&v, env.heap()), "[11, 22]");
}

#[test]
fn a_live_slot_is_read_and_the_calls_write_is_put_back() {
    // `root` is top-level state, read inside the call; the call's write to
    // a `state var` it reads back is visible inside and gone after.
    let code = "state var level = 3
fn peek()
  let before = get level
  set level = before * 10
  [before, get level]
end
let r = speculate(peek)
print(\"{r.value} {get level}\")
set level = get level + 1";
    let (out, _) = frames(code, RunPolicy::FAST, 2).unwrap();
    assert_eq!(out, ["[3, 30] 3", "=> 4", "[4, 40] 4", "=> 5"]);
}

thread_local! {
    static SAVED: Cell<u32> = const { Cell::new(0) };
}

fn native_save_file(cx: &mut PetalCxt) -> NativeResult {
    cx.note_effect();
    SAVED.with(|s| s.set(s.get() + 1));
    cx.push_int(1);
    Ok(1)
}

#[test]
fn an_escaping_effect_is_not_called_and_is_warned_about_once() {
    SAVED.with(|s| s.set(0));
    let mut env = Env::new();
    env.register_native("save_file", native_save_file, NativeEffects::EFFECT);
    let pid = env
        .load_program(
            "native fn save_file()
fn save()
  let a = save_file()
  let b = save_file()
  [a, b]
end
let r = speculate(save)
let s = speculate(save)
print(\"{r.value} {s.value} {save_file()}\")",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    // Only the live call ran.
    assert_eq!(SAVED.with(|s| s.get()), 1);
    assert_eq!(env.take_output(), ["[nil, nil] [nil, nil] 1"]);
    let warnings = env.take_runtime_warnings(sid);
    // One per call site: two sites in `save`, however many times it runs.
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].starts_with("speculate(): `save_file` was not called (its effect escapes the run)"),
        "{}",
        warnings[0]
    );
    assert!(warnings[0].contains(":3:11"), "{}", warnings[0]);
    // And not again on the next frame.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert!(env.take_runtime_warnings(sid).is_empty());
}

#[test]
fn a_contained_effect_runs_and_is_undone() {
    let mut env = Env::new();
    let v = env
        .run_source(
            "import noise
fn chatty()
  print(\"inside\")
  noise_seed(99)
  noise(0.5, 0.5)
end
let before = noise(0.25, 0.75)
let r = speculate(chatty)
let after = noise(0.25, 0.75)
print(\"same noise: {before == after}\")
r.value == noise(0.5, 0.5)",
        )
        .unwrap();
    // The print inside the call was discarded with the call.
    assert_eq!(env.take_output(), ["same noise: true"]);
    // Inside, the reseeded noise was what the call saw; outside it is not.
    assert_eq!(value::value_to_display_string(&v, env.heap()), "false");
}

#[test]
fn random_draws_do_not_advance_the_live_stream() {
    let run = |code: &str| {
        let mut env = Env::new();
        env.set_seed(7);
        let v = env.run_source(code).unwrap();
        value::value_to_display_string(&v, env.heap())
    };
    let plain = run("[random(), random(), random()]");
    let spec = run(
        "let a = random()
let r = speculate(fn() [random(), random()] end)
[a, random(), random()]",
    );
    assert_eq!(plain, spec);
    // The call draws from a copy of the stream: its first draw is the one
    // the live program makes next.
    let inside = run(
        "let a = random()
let r = speculate(fn() random() end)
r.value == random()",
    );
    assert_eq!(inside, "true");
}

#[test]
fn emits_are_handed_back_with_their_origins_not_drained() {
    let mut env = Env::new();
    let pid = env
        .load_program(
            "import io
fn draw(n)
  for i in range(0, n) do push_output(symbol(\"cmds\"), i * 10) end
  push_output(symbol(\"other\"), \"x\")
  n
end
push_output(symbol(\"cmds\"), \"before\")
let r = speculate(draw, 2)
push_output(symbol(\"cmds\"), \"after\")
r",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    let r = env.run(sid).unwrap();
    let text = value::value_to_display_string(&r, env.heap());
    assert!(text.contains("value: 2"), "{text}");
    assert!(
        text.contains("{ channel: \"cmds\", value: 0, at: { file: "),
        "{text}"
    );
    assert!(text.contains("line: 3, column: 27"), "{text}");
    // The channels come in name order, and each in emit order.
    let cmds = text.find("value: 10").unwrap();
    let other = text.find("channel: \"other\"").unwrap();
    assert!(cmds < other, "{text}");
    assert!(text.contains("places: []"), "{text}");
    // The host's buffers hold only the live emits.
    let cmds = env.intern_symbol("cmds");
    let live: Vec<String> = env
        .take_output_buffer(cmds)
        .iter()
        .map(|v| value::value_to_display_string(v, env.heap()))
        .collect();
    assert_eq!(live, ["before", "after"]);
    let other = env.intern_symbol("other");
    assert!(env.take_output_buffer(other).is_empty());
}

#[test]
fn emits_keep_their_origins_when_the_host_traces_too() {
    let mut env = Env::new();
    env.enable_emit_trace(true);
    let pid = env
        .load_program(
            "import io
push_output(symbol(\"cmds\"), 1)
let r = speculate(fn() push_output(symbol(\"cmds\"), 2) end)
push_output(symbol(\"cmds\"), 3)
len(r.emits)",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    let n = env.run(sid).unwrap();
    assert_eq!(n, Value::Int(1));
    let cmds = env.intern_symbol("cmds");
    let origins = env.take_output_origins(cmds);
    assert_eq!(env.take_output_buffer(cmds).len(), 2);
    assert_eq!(origins.len(), 2, "one origin per live value");
}

#[test]
fn a_nested_speculate_is_an_error_with_help() {
    let mut env = Env::new();
    let err = env
        .run_source(
            "fn inner() 1 end
fn outer()
  speculate(inner)
end
speculate(outer)",
        )
        .unwrap_err();
    assert!(
        err.contains("speculate() cannot call itself: a speculative call is already running"),
        "{err}"
    );
    assert!(err.contains("help: compute the candidate scenes first, then compare them outside"), "{err}");
    assert!(err.contains("line 3"), "{err}");
}

#[test]
fn a_failing_call_puts_everything_back_before_the_error() {
    let mut env = Env::new();
    env.set_policy(RunPolicy::FAST);
    let pid = env
        .load_program(
            "import io
state var level = 1
state tries = 0
fn boom()
  set level = 100
  push_output(symbol(\"cmds\"), 1)
  1 / 0
end
tries += 1
if tries == 1 then speculate(boom) end
print(get level)",
        )
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    assert!(env.run(sid).is_err());
    let cmds = env.intern_symbol("cmds");
    assert!(env.take_output_buffer(cmds).is_empty());
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1"]);
}

#[test]
fn places_are_recorded_inside_and_pass_through_outside() {
    let mut env = Env::new();
    let v = env
        .run_source(
            "fn place(key, r)
  speculate_place(key, r)
end
fn view(xs)
  for i in range(0, len(xs)) do place(xs[i], {x: i * 70}) end
  speculating()
end
let r = speculate(view, [\"b\", \"a\"])
let live = view([\"a\"])
[r.value, r.places, live, speculating()]",
        )
        .unwrap();
    assert_eq!(
        value::value_to_display_string(&v, env.heap()),
        "[true, [{ key: \"b\", value: { x: 0 } }, { key: \"a\", value: { x: 70 } }], false, false]"
    );
    // A key a keyed loop would refuse is refused here too.
    let err = Env::new().run_source("speculate_place(fn() 1 end, 1)").unwrap_err();
    assert!(err.contains("as a place key"), "{err}");
}

#[test]
fn state_is_allowed_in_a_call_made_from_a_callback() {
    let mut env = Env::new();
    let v = env
        .run_source(
            "fn view(n)
  state seen = 0
  seen += n
  seen
end
map([1, 2, 3], fn(n) speculate(view, n).value end)",
        )
        .unwrap();
    assert_eq!(value::value_to_display_string(&v, env.heap()), "[1, 2, 3]");
}

/// Memo stats after three frames of `code` under the fast policy, and its
/// output.
fn memo_frames(code: &str) -> (Vec<String>, petal::memo::MemoStats) {
    let mut env = Env::new();
    env.set_policy(RunPolicy::FAST);
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut out = Vec::new();
    for i in 0..3 {
        if i > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        out.extend(env.take_output());
    }
    (out, env.memo_stats(sid).unwrap())
}

#[test]
fn nothing_is_recorded_inside_and_the_scope_around_is_not_replayed() {
    // `expensive` is the kind of call the memo records: a pure function of
    // its argument that does real work. Called outside it is recorded and
    // then replayed; called only inside a speculative call it never is.
    let program = |call: &str| {
        format!(
            "state var level = 1
state frame = 0
fn expensive(n)
  var acc = 0
  for i in range(0, 300) do set acc = get acc + n end
  get acc
end
fn probe()
  expensive(get level)
end
fn panel(tag)
  {call}
end
frame += 1
print(panel(\"p\"))
if frame == 1 then set level = 2 end"
        )
    };
    let (live_out, live) = memo_frames(&program("probe()"));
    assert_eq!(live_out, ["300", "600", "600"]);
    assert!(live.records > 0, "the live program records: {live:?}");
    // In the speculative form `panel` reads `level` only inside the call; if
    // its record were replayed after `level` changed, frame 2 would print
    // 300.
    let (spec_out, spec) = memo_frames(&program("speculate(probe).value"));
    assert_eq!(spec_out, ["300", "600", "600"]);
    assert_eq!(spec.records, 0, "{spec:?}");
    assert_eq!(spec.hits, 0, "{spec:?}");
}

#[test]
fn the_checker_knows_the_builtins() {
    let mut env = Env::new();
    // A wrong call is a check error, a right one is not.
    let pid = env.load_program("let r = speculate(fn(x) x end, 1)\nr.value").unwrap();
    let sid = env.create_stack(pid).unwrap();
    assert_eq!(env.run(sid).unwrap(), Value::Int(1));
    let err = Env::new().run_source("speculate()").unwrap_err();
    assert!(err.contains("speculate"), "{err}");
}

/// What the call returns may share containers with the live program; the
/// live program's later in-place edits must not reach into it.
#[test]
fn a_returned_value_is_not_edited_by_the_live_program_afterwards() {
    let code = "state var xs = [1, 2, 3]
state ys = [4, 5]
fn grab()
  [get xs, ys]
end
let r = speculate(grab)
set xs[0] = 9
set xs = append(get xs, 10)
ys = append(ys, 6)
print(\"{r.value} {get xs} {ys}\")";
    for policy in [RunPolicy::BASELINE, RunPolicy::FAST] {
        let (out, _) = frames(code, policy, 2).unwrap();
        assert_eq!(out[0], "[[1, 2, 3], [4, 5]] [9, 2, 3, 10] [4, 5, 6]", "{policy:?}");
    }
}
