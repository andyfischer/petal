//! Dependency classes and the refined native vocabulary (layer L5 of the
//! retained optimization plan; `docs/dev/retained-scene-output.md`).
//!
//! The oracle: one game-shaped script, run frame by frame three ways —
//! memoization off (the reference), memoization on with the compile-time
//! hints of the dependency classes, and on with the hints ignored — must
//! produce the same draws, the same declarations, the same printed lines and
//! the same commands issued to the host, on every frame. Around it, the
//! classes themselves, the replay rules of `command` and `query` natives,
//! and the counters that show the hints did something.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::stack::StackKey;
use petal::static_classify::{Deps, ReplaySeverity};
use petal::value::Value;

/// A value as text, from the heap it lives on.
fn show(heap: &Heap, v: Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".into(),
        Value::String(id) => format!("{:?}", heap.get_string(id)),
        Value::List(id) => {
            let items: Vec<String> = heap.get_list(id).iter().map(|x| show(heap, *x)).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Map(id) => {
            let fields: Vec<String> = heap
                .get_map(id)
                .iter()
                .map(|(k, x)| format!("{}: {}", k.as_str(), show(heap, *x)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// The host side of one world: the inputs the natives read and the
/// commands they were handed.
#[derive(Default)]
struct Host {
    clock: Cell<f64>,
    pointer: Cell<i64>,
    physics: Cell<i64>,
    commands: RefCell<Vec<String>>,
}

struct World {
    env: Env,
    pid: petal::program::ProgramId,
    sid: StackKey,
    host: Rc<Host>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Off,
    Hinted,
    Unhinted,
}

fn push(c: &mut PetalCxt, buffer: &str) -> NativeResult {
    let args: Vec<Value> = (1..=c.arg_count()).map(|i| c.get_value(i)).collect::<Result<_, _>>()?;
    let sym = c.intern_symbol(buffer);
    let v = if args.len() == 1 {
        args[0]
    } else {
        let id = c.heap_mut().alloc_list(args);
        Value::List(id)
    };
    c.push_output(sym, v);
    c.push_nil();
    Ok(1)
}

fn world(src: &str, mode: Mode) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = mode != Mode::Off;
    policy.gate = false;
    env.set_policy(policy);
    let host = Rc::new(Host::default());

    let h = host.clone();
    env.register_native_boxed(
        "now",
        move |c| {
            c.push_float(h.clock.get());
            Ok(1)
        },
        NativeEffects::probe(InputClasses::CLOCK).ticks(),
    );
    let h = host.clone();
    env.register_native_boxed(
        "inp",
        move |c| {
            c.push_int(h.pointer.get());
            Ok(1)
        },
        NativeEffects::probe(InputClasses::POINTER),
    );
    let h = host.clone();
    env.register_native_boxed(
        "ray",
        move |c| {
            c.note_class_read(InputClasses::PHYSICS);
            let x = c.get_int(1)?;
            c.push_int(h.physics.get() * 10 + x);
            Ok(1)
        },
        NativeEffects::query(InputClasses::PHYSICS),
    );
    let h = host.clone();
    env.register_native_boxed(
        "kick",
        move |c| {
            let id = c.get_int(1)?;
            let v = c.get_float(2)?;
            h.commands.borrow_mut().push(format!("kick {id} {v}"));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::COMMAND,
    );
    env.register_native("put", |c| push(c, "out"), NativeEffects::EMITS);
    env.register_native("decl", |c| push(c, "decl"), NativeEffects::DECLARES);

    let pid = env.load_program(src).unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    if mode == Mode::Unhinted {
        env.set_memo_hints(sid, false);
    }
    World { env, pid, sid, host }
}

impl World {
    /// One frame with the given inputs: what it drew, declared, printed and
    /// commanded.
    fn frame(&mut self, clock: f64, pointer: i64, physics: i64) -> Vec<String> {
        self.host.clock.set(clock);
        self.host.pointer.set(pointer);
        if physics != self.host.physics.get() {
            self.host.physics.set(physics);
            self.env.note_class_changed(self.sid, InputClasses::PHYSICS);
        }
        self.host.commands.borrow_mut().clear();
        self.env.reset_stack(self.sid).unwrap();
        self.env.run(self.sid).unwrap_or_else(|e| panic!("run: {e}"));
        let printed = self.env.take_output();
        let mut lines = Vec::new();
        for buffer in ["out", "decl"] {
            let sym = self.env.intern_symbol(buffer);
            let values = self.env.take_output_buffer(sym);
            let heap = self.env.heap();
            lines.extend(values.iter().map(|v| format!("{buffer} {}", show(heap, *v))));
        }
        lines.extend(printed.iter().map(|l| format!("print {l}")));
        lines.extend(self.host.commands.borrow().iter().map(|c| format!("cmd {c}")));
        lines
    }

    fn stats(&self) -> petal::memo::MemoStats {
        self.env.memo_stats(self.sid).unwrap()
    }
}

const GAME: &str = r#"
native fn now()
native fn inp()
native fn ray(x)
native fn kick(id, v)
native fn put(v)
native fn decl(id, v)

const CITY = map(range(0, 40), fn(i) {name: "b{i % 4}", x: float(i)} end)

fn block(b)
  put({mesh: b.name, x: b.x})
  nil
end

fn city(c)
  for b in c do block(b) end
  nil
end

fn bob(i)
  let t = now()
  put({bob: i, y: t * float(i)})
  nil
end

fn crate(i)
  decl("crate{i}", {x: float(i)})
  nil
end

fn crates(n)
  for i in range(n) do crate(i) end
  nil
end

fn pusher(i)
  kick(i, 1.5)
  put({pushed: i})
  nil
end

fn pushers()
  for i in range(3) do pusher(i) end
  nil
end

fn hit_test()
  ray(1)
end

fn aim()
  put({aim: hit_test()})
  nil
end

fn gate(i)
  if inp() > 5 then
    kick(100 + i, 2.0)
  end
  inp() > 3
end

fn panel()
  let a = gate(1)
  put({panel: a})
  nil
end

fn literal_arg(r)
  put({lit: r.w, n: len(r.h)})
  nil
end

fn caller()
  literal_arg({w: 3, h: [1, 2, 3]})
  nil
end

fn shout(n)
  print("shout {n}")
  put({shouted: n})
  nil
end

city(CITY)
for i in range(4) do bob(i) end
crates(5)
pushers()
aim()
panel()
caller()
shout(2)
"#;

/// The inputs of each frame: clock, pointer, physics revision.
const FRAMES: &[(f64, i64, i64)] = &[
    (0.0, 0, 0),
    (0.1, 0, 0),
    (0.2, 4, 0),
    (0.3, 4, 1),
    (0.4, 5, 1),
    (0.5, 6, 1),
    (0.6, 6, 1),
    (0.7, 7, 2),
    (0.8, 2, 2),
    (0.9, 2, 2),
    (1.0, 9, 3),
    (1.1, 9, 3),
];

fn run_all(src: &str) -> (World, World, World) {
    let mut off = world(src, Mode::Off);
    let mut hinted = world(src, Mode::Hinted);
    let mut unhinted = world(src, Mode::Unhinted);
    for (f, &(clock, pointer, physics)) in FRAMES.iter().enumerate() {
        let want = off.frame(clock, pointer, physics);
        let got = hinted.frame(clock, pointer, physics);
        assert_eq!(want, got, "frame {f}: memo with hints differs from memo off");
        let got = unhinted.frame(clock, pointer, physics);
        assert_eq!(want, got, "frame {f}: memo without hints differs from memo off");
    }
    (off, hinted, unhinted)
}

#[test]
fn hinted_memo_reproduces_every_frame() {
    let (_, hinted, unhinted) = run_all(GAME);
    let (h, u) = (hinted.stats(), unhinted.stats());
    // The ticking `bob` and the printing `shout`, called from top-level code,
    // open no scope at all with the hints.
    assert!(h.static_skips >= 5 * FRAMES.len() as u64, "{h:?}");
    assert_eq!(u.static_skips, 0, "{u:?}");
    // `city(CITY)` and `literal_arg({..})` have static arguments.
    assert!(h.static_args >= FRAMES.len() as u64 - 1, "{h:?}");
    assert_eq!(u.static_args, 0, "{u:?}");
    // `pushers` replays, re-issuing its three commands each time.
    assert!(h.commands >= 3 * (FRAMES.len() as u64 - 4), "{h:?}");
    assert!(h.hits > 0 && u.hits > 0);
}

#[test]
fn a_command_is_reissued_on_replay_and_deferred_in_a_reexecution() {
    let mut w = world(GAME, Mode::Hinted);
    let first = w.frame(0.0, 0, 0);
    assert!(first.contains(&"cmd kick 0 1.5".to_string()), "{first:?}");
    let second = w.frame(0.1, 0, 0);
    assert_eq!(
        second.iter().filter(|l| l.starts_with("cmd kick")).count(),
        3,
        "the replayed pushers issue their commands once each: {second:?}"
    );
    // The pointer crosses 5: `gate` is re-executed during `panel`'s
    // validation and now kicks. The kick is deferred, the change seen, and
    // `panel` runs, issuing it exactly once.
    w.frame(0.2, 4, 0);
    let reexecs = w.stats().reexecs;
    let crossed = w.frame(0.3, 6, 0);
    assert!(w.stats().reexecs > reexecs, "gate was re-executed: {:?}", w.stats());
    assert_eq!(
        crossed.iter().filter(|l| *l == "cmd kick 101 2").count(),
        1,
        "{crossed:?}"
    );
}

#[test]
fn a_query_stays_valid_while_its_class_revision_holds() {
    let mut w = world(GAME, Mode::Hinted);
    w.frame(0.0, 0, 0);
    w.frame(0.1, 0, 0);
    let misses = w.stats().misses;
    // Host data in general changing does not touch a PHYSICS read.
    w.env.note_host_data_changed(w.sid);
    let out = w.frame(0.2, 0, 0);
    assert!(out.contains(&"out {aim: 1}".to_string()), "{out:?}");
    assert_eq!(w.stats().misses, misses, "nothing missed: {:?}", w.stats());
    // A physics step does.
    let out = w.frame(0.3, 0, 7);
    assert!(out.contains(&"out {aim: 71}".to_string()), "{out:?}");
    assert!(w.stats().misses > misses, "{:?}", w.stats());
}

#[test]
fn the_gate_reruns_a_frame_once_a_read_class_changes() {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.gate = true;
    env.set_policy(policy);
    env.register_native(
        "ray",
        |c| {
            c.note_class_read(InputClasses::PHYSICS);
            c.push_int(1);
            Ok(1)
        },
        NativeEffects::query(InputClasses::PHYSICS),
    );
    let pid = env.load_program("native fn ray()\nprint(ray())\n").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert!(!env.run_needed(sid), "nothing changed");
    env.note_class_changed(sid, InputClasses::NET);
    assert!(!env.run_needed(sid), "a class it did not read");
    env.note_class_changed(sid, InputClasses::PHYSICS);
    assert!(env.run_needed(sid), "the class it read");
}

#[test]
fn the_classes_of_the_game() {
    let w = world(GAME, Mode::Hinted);
    let program = w.env.get_program(w.pid).unwrap();
    let analysis = w.env.static_analysis(program);
    let deps = analysis.dep_classes();
    let f = |name: &str| {
        deps.fns
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no fn {name}"))
    };
    assert!(f("block").hint().constant && f("city").hint().constant);
    assert_eq!(f("city").may, Deps::EMIT);
    assert!(f("bob").every.contains(Deps::TICKS) && f("bob").hint().ticks_every_call);
    assert!(f("bob").may.input_classes().contains(InputClasses::CLOCK));
    assert!(f("crate").may.contains(Deps::DECLARE) && f("crate").hint().constant);
    assert!(f("pusher").every.contains(Deps::COMMAND) && !f("pusher").hint().effect_every_call);
    assert!(!f("pusher").hint().constant, "a command is not a constant");
    assert!(f("hit_test").every.input_classes().contains(InputClasses::PHYSICS));
    assert!(f("aim").may.input_classes().contains(InputClasses::PHYSICS));
    assert!(
        !f("aim").every.input_classes().contains(InputClasses::PHYSICS),
        "a callee's query is not the caller's own"
    );
    assert!(f("gate").may.input_classes().contains(InputClasses::POINTER));
    assert!(f("gate").may.contains(Deps::COMMAND) && !f("gate").every.contains(Deps::COMMAND));
    assert!(f("shout").hint().effect_every_call);

    let findings = deps.findings(program, false);
    let reason = |name: &str| {
        findings
            .iter()
            .find(|x| x.name == name)
            .map(|x| (x.severity, x.reason.clone()))
    };
    let (sev, why) = reason("bob").unwrap();
    assert_eq!(sev, ReplaySeverity::Never);
    assert!(why.contains("`now`") && why.contains("CLOCK"), "{why}");
    let (sev, why) = reason("hit_test").unwrap();
    assert_eq!(sev, ReplaySeverity::Rarely);
    assert!(why.contains("`ray`") && why.contains("PHYSICS"), "{why}");
    let (_, why) = reason("shout").unwrap();
    assert!(why.contains("`print`") && why.contains("EFFECT"), "{why}");
    assert!(reason("city").is_none() && reason("pusher").is_none() && reason("aim").is_none());
}

/// An effect reached on every path through a chain of calls is the
/// caller's too; one behind a branch, or after an early return, is not.
#[test]
fn every_call_follows_calls_and_stops_at_branches() {
    let src = r#"
fn log_it(x)
  print("{x}")
end

fn middle(x)
  log_it(x)
  x
end

fn outer(x)
  middle(x) + 1
end

fn sometimes(x)
  if x > 1 then
    log_it(x)
  end
  x
end

fn early(x)
  if x > 1 then
    return 0
  end
  log_it(x)
  x
end

outer(1)
sometimes(1)
early(1)
"#;
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap();
    let program = env.get_program(pid).unwrap();
    let analysis = env.static_analysis(program);
    let deps = analysis.dep_classes();
    let every = |name: &str| deps.fns.iter().find(|f| f.name == name).unwrap().every;
    assert!(every("log_it").contains(Deps::EFFECT));
    assert!(every("middle").contains(Deps::EFFECT));
    assert!(every("outer").contains(Deps::EFFECT));
    assert!(!every("sometimes").contains(Deps::EFFECT));
    assert!(!every("early").contains(Deps::EFFECT));
    let findings = deps.findings(program, false);
    let outer = findings.iter().find(|x| x.name == "outer").unwrap();
    assert!(
        outer.reason.contains("`middle`") && outer.reason.contains("`log_it`") && outer.reason.contains("`print`"),
        "the chain down to the native: {}",
        outer.reason
    );
    let report = env.dep_classes_report(program, false);
    assert!(report.contains("never replays: outer"), "{report}");
}

/// A top-level plain `var` is a new cell on every run, so a function that
/// uses one on every call never validates; a `state var` is one cell.
#[test]
fn a_module_var_is_named_as_the_reason() {
    let src = r#"
var log = []
state var kept = []

fn note(x)
  set log = append(get log, x)
end

fn keep(x)
  set kept = append(get kept, x)
end

note(1)
keep(1)
"#;
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap();
    let program = env.get_program(pid).unwrap();
    let findings = env.replay_findings(program, false);
    let note = findings.iter().find(|x| x.name == "note").expect("note never replays");
    assert!(note.reason.contains("var `log`") && note.reason.contains("MODULE_VAR"), "{}", note.reason);
    assert!(findings.iter().all(|x| x.name != "keep"), "{findings:?}");
}

/// Calls `run_all` on a script without the game's natives declared at the
/// top: the natives are registered by `world` regardless.
fn same_three_ways(body: &str) -> (World, World, World) {
    let src = format!(
        "native fn now()\nnative fn inp()\nnative fn ray(x)\nnative fn kick(id, v)\nnative fn put(v)\nnative fn decl(id, v)\n{body}"
    );
    run_all(&src)
}

/// One top-level lambda, made once per iteration of a static loop, captures
/// a different static value each time; which of them is called is chosen by
/// input. Every argument at the call is static and the function is
/// constant, but the closure is not the same one every run: its captures
/// must still be compared.
#[test]
fn a_closure_made_in_a_top_level_loop_is_not_a_constant_call() {
    same_three_ways(
        r#"
var sel = fn(x) x end
for k in [1, 2, 3] do
  if inp() % 3 == k - 1 then
    set sel = fn(x)
      var acc = 0
      for j in range(40) do
        set acc = get acc + j * x
      end
      get acc * 10 + k
    end
  end
end
put((get sel)(1))
"#,
    );
}

/// A probe read at the bottom of a call chain deeper than any fixed number
/// of rounds of the forward walk still reaches the top: the walk runs to its
/// fixed point, so the top of the chain is not taken for a constant (whose
/// call with a static argument would be replayed unchecked, stale once the
/// pointer moves). Checked on the classes: running memo over a chain this
/// deep costs seconds per changed frame in a debug build, hints or not.
#[test]
fn a_deep_call_chain_carries_its_reads_to_the_top() {
    let depth = 80;
    let mut body = format!("native fn inp()\nnative fn put(v)\nfn f{depth}(x)\n  inp() + x\nend\n");
    for i in (1..depth).rev() {
        body.push_str(&format!("fn f{i}(x)\n  f{}(x)\nend\n", i + 1));
    }
    body.push_str("put(f1(1))\n");
    let w = world(&body, Mode::Hinted);
    let program = w.env.get_program(w.pid).unwrap();
    let analysis = w.env.static_analysis(program);
    let deps = analysis.dep_classes();
    let f1 = deps.fns.iter().find(|f| f.name == "f1").unwrap();
    assert!(f1.may.input_classes().contains(InputClasses::POINTER), "{:?}", f1.may.names());
    assert!(!f1.hint().constant);
}

/// A call in a top-level loop whose argument is the loop variable over
/// static data: static on every run at each iteration's own path.
#[test]
fn static_arguments_in_a_top_level_loop_are_per_iteration() {
    same_three_ways(
        r#"
fn sq(i)
  i * i
end
for i in range(4) do
  put(sq(i) + inp())
end
"#,
    );
}

/// The same, one level down: inside such a closure, an argument that is a
/// capture of top-level static data is not the same value on every run at
/// that call, so it must still be compared.
#[test]
fn a_capture_of_a_closure_made_in_a_loop_is_not_a_static_argument() {
    same_three_ways(
        r#"
fn heavy(x)
  var acc = 0
  for j in range(40) do
    set acc = get acc + j * x
  end
  get acc + inp() * 0
end
var sel = fn() 0 end
for k in [1, 2, 3] do
  if int(now() * 10.0 + 0.5) % 3 == k - 1 then
    set sel = fn() heavy(k) end
  end
end
put((get sel)())
"#,
    );
}
