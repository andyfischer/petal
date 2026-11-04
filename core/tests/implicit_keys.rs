//! Implicit keys and keyed loops (layer L8 of the retained optimization
//! plan; `docs/dev/retained-scene-output.md`).
//!
//! A native that declares something keyed can ask for its call's implicit
//! key (`PetalCxt::call_key`): a hash of the call path that keys `state`,
//! ending in the native's own call site. The oracle: every script runs frame
//! by frame with memoization off, on with a flat buffer and on with the
//! buffer segmented (drained through a `SegmentCache`), and all three must
//! declare the same things under the same keys and issue the same commands
//! with the same keys. Around it, what the key is: distinct per call site,
//! iteration and recursion depth, the same frame after frame and across a
//! reload that keeps the call path, following the entity under `for … key`,
//! and absent in a higher-order builtin's callback.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::output_segments::{SegmentCache, SegmentedFrame};
use petal::policy::RunPolicy;
use petal::stack::StackKey;
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

fn key_text(key: Option<u64>) -> String {
    match key {
        Some(k) => format!("{k:016x}"),
        None => "nokey".into(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Off,
    Flat,
    Segments,
}

#[derive(Default)]
struct Host {
    pointer: Cell<i64>,
    commands: RefCell<Vec<String>>,
}

struct World {
    env: Env,
    sid: StackKey,
    mode: Mode,
    host: Rc<Host>,
    cache: SegmentCache,
    frames: Vec<SegmentedFrame>,
}

/// `decl(v)` pushes `[key, v]` (the key as an int, or nil); `kick(v)` is a
/// command that hands the host its key and argument; `inp()` is a pointer
/// probe; `near(v)` a pointer probe that reads its key.
fn world(src: &str, mode: Mode) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = mode != Mode::Off;
    policy.gate = false;
    env.set_policy(policy);
    let host = Rc::new(Host::default());
    env.register_native(
        "decl",
        |c: &mut PetalCxt| -> NativeResult {
            let key = match c.call_key() {
                Some(k) => Value::Int(k as i64),
                None => Value::Nil,
            };
            let v = c.get_value(1)?;
            let pair = c.heap_mut().alloc_list(vec![key, v]);
            let sym = c.intern_symbol("decl");
            c.push_output(sym, Value::List(pair));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::DECLARES,
    );
    let h = host.clone();
    env.register_native_boxed(
        "kick",
        move |c| {
            let key = c.call_key();
            let v = show(c.heap(), c.get_value(1)?);
            h.commands
                .borrow_mut()
                .push(format!("kick {} {v}", key_text(key)));
            c.push_nil();
            Ok(1)
        },
        NativeEffects::COMMAND,
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
        "near",
        move |c| {
            let key = c.call_key().unwrap_or(0);
            c.push_bool((key as i64 ^ h.pointer.get()) & 1 == 0);
            Ok(1)
        },
        NativeEffects::probe(InputClasses::POINTER),
    );
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    if mode == Mode::Segments {
        let decl = env.intern_symbol("decl");
        env.set_output_segmented(decl, true);
    }
    World {
        env,
        sid,
        mode,
        host,
        cache: SegmentCache::default(),
        frames: vec![],
    }
}

impl World {
    /// One frame with the pointer at `pointer`: every declaration as
    /// `key value`, then every command.
    fn frame(&mut self, pointer: i64) -> Vec<String> {
        self.host.pointer.set(pointer);
        self.host.commands.borrow_mut().clear();
        self.env.reset_stack(self.sid).unwrap();
        self.env
            .run(self.sid)
            .unwrap_or_else(|e| panic!("run: {e}"));
        self.env.take_output();
        let sym = self.env.intern_symbol("decl");
        let values = if self.mode == Mode::Segments {
            let frame = self.env.take_output_segments(sym);
            let values = self.cache.apply(&frame);
            self.frames.push(frame);
            values
        } else {
            self.env.take_output_buffer(sym)
        };
        let heap = self.env.heap();
        let mut lines: Vec<String> = values
            .iter()
            .map(|v| {
                let Value::List(id) = v else {
                    panic!("not a declaration: {v:?}")
                };
                let pair = heap.get_list(*id);
                let key = match pair[0] {
                    Value::Int(k) => Some(k as u64),
                    _ => None,
                };
                format!("{} {}", key_text(key), show(heap, pair[1]))
            })
            .collect();
        lines.extend(self.host.commands.borrow().iter().cloned());
        lines
    }

    fn reload(&mut self, src: &str) {
        self.env
            .reload_program(self.sid, src, None)
            .unwrap_or_else(|e| panic!("reload: {e}"));
    }

    fn stats(&self) -> petal::memo::MemoStats {
        self.env.memo_stats(self.sid).unwrap()
    }
}

/// Run `src` under every mode for the given pointer positions and require
/// the same declarations, keys and commands on every frame. `edit`, if
/// given, is reloaded before frame `pointers.len() / 2`. Returns each
/// world and the memo-off frames.
fn same_everywhere(
    src: &str,
    pointers: &[i64],
    edit: Option<&str>,
) -> (Vec<World>, Vec<Vec<String>>) {
    let mut worlds = vec![
        world(src, Mode::Off),
        world(src, Mode::Flat),
        world(src, Mode::Segments),
    ];
    let mut reference = Vec::new();
    for (f, &p) in pointers.iter().enumerate() {
        if f == pointers.len() / 2
            && let Some(e) = edit
        {
            worlds.iter_mut().for_each(|w| w.reload(e));
        }
        let outs: Vec<Vec<String>> = worlds.iter_mut().map(|w| w.frame(p)).collect();
        for (w, o) in worlds.iter().zip(&outs).skip(1) {
            assert_eq!(&outs[0], o, "frame {f}: {:?} differs from memo off", w.mode);
        }
        reference.push(outs.into_iter().next().unwrap());
    }
    (worlds, reference)
}

/// The keys of a frame's declarations, by the value declared.
fn keys_by_value(frame: &[String]) -> HashMap<String, String> {
    frame
        .iter()
        .filter(|l| !l.starts_with("kick "))
        .map(|l| {
            let (k, v) = l.split_once(' ').unwrap();
            (v.to_string(), k.to_string())
        })
        .collect()
}

const PATHS: &str = r#"
native fn decl(v)

fn crate_at(i)
  decl("crate {i}")
  nil
end

fn crates(tag, n)
  for i in range(n) do crate_at("{tag}{i}") end
  nil
end

fn tower(d)
  decl("level {d}")
  if d > 0 then tower(d - 1) end
  nil
end

fn wrapped(v)
  decl(v)
  nil
end

crates("a", 3)
crates("b", 2)
tower(3)
decl("top one")
decl("top two")
var i = 0
while i < 2 do
  decl("while {i}")
  set i = i + 1
end
[1, 2].forEach(fn(x) decl("each {x}") end)
let made = [5, 6].map(fn(x) decl("mapped {x}") end)
for x in [7] do [8].forEach(fn(y) wrapped("nested {x} {y}") end) end
"#;

#[test]
fn every_call_path_gets_its_own_key_frame_after_frame() {
    let (_, frames) = same_everywhere(PATHS, &[0, 0, 0, 0], None);
    let first = keys_by_value(&frames[0]);
    for f in &frames[1..] {
        assert_eq!(&keys_by_value(f), &first, "the keys are the same every frame");
    }
    // A callback of a higher-order builtin has none: every element runs on
    // one path, as for `state`, which is an error there.
    for v in [
        "\"each 1\"",
        "\"each 2\"",
        "\"mapped 5\"",
        "\"mapped 6\"",
        "\"nested 7 8\"",
    ] {
        assert_eq!(first[v], "nokey", "{v}");
    }
    // Every other call has one, and no two share it: two call sites of one
    // helper, loop iterations, recursion depths, two calls at top level,
    // `while` iterations.
    let keyed: Vec<&String> = first.values().filter(|k| *k != "nokey").collect();
    assert_eq!(keyed.len(), 3 + 2 + 4 + 2 + 2, "{first:?}");
    let distinct: HashSet<&&String> = keyed.iter().collect();
    assert_eq!(distinct.len(), keyed.len(), "keys collide: {first:?}");
}

/// The keyed loop's entities change order every frame; the positional
/// loop's do too.
const ENTITIES: &str = r#"
native fn decl(v)

state n = 0
n += 1

let ents = if n % 2 == 0 then
  [{id: 1, x: 10.0}, {id: 2, x: 20.0}, {id: 3, x: 30.0}]
else
  [{id: 3, x: 30.0}, {id: 1, x: 10.0}, {id: 2, x: 20.0}]
end

fn ent(e)
  decl("ent {e.id} at {e.x}")
  for k in range(20) do decl("part {e.id}.{k}") end
  nil
end

fn row(e)
  decl("row {e.id}")
  nil
end

for e in ents key e.id do ent(e) end
for e in ents do row(e) end
"#;

#[test]
fn a_keyed_loop_keeps_each_entitys_key_and_scope() {
    let (worlds, frames) = same_everywhere(ENTITIES, &[0; 6], None);
    let a = keys_by_value(&frames[0]);
    let b = keys_by_value(&frames[1]);
    // Under `key e.id` a declaration's key follows its entity...
    for id in 1..=3 {
        let v = format!("\"ent {id} at {}.0\"", id * 10);
        assert_eq!(a[&v], b[&v], "{v}");
    }
    // ...and in the positional loop it stays with the position: the entity
    // that moved has a different key, the position has the same.
    for id in 1..=3 {
        let v = format!("\"row {id}\"");
        assert_ne!(a[&v], b[&v], "{v} moved, and so did its key");
    }
    let pos_keys = |f: &[String]| -> Vec<String> {
        f.iter()
            .filter(|l| l.contains("\"row "))
            .map(|l| l.split_once(' ').unwrap().0.to_string())
            .collect()
    };
    assert_eq!(pos_keys(&frames[0]), pos_keys(&frames[1]));

    // The memo scopes follow the entities too: once both orders have been
    // seen, every `ent` replays whatever the order, and the segmented host
    // is never handed a new segment for them.
    let flat = &worlds[1];
    let s = flat.stats();
    assert!(s.hits >= 3 * 4, "every ent replays from frame 2 on: {s:?}");
    // (An `ent` segment holds its 21 declarations; the positional rows
    // change arguments every frame and are recorded again.)
    let seg = &worlds[2];
    let ent_defs = |f: &SegmentedFrame| f.defined.iter().filter(|d| d.items.len() == 21).count();
    assert_eq!(ent_defs(&seg.frames[0]), 3);
    let late: usize = seg.frames[2..].iter().map(ent_defs).sum();
    assert_eq!(late, 0, "no ent segment is defined again after the first two frames");
}

/// A command issued from a scope that replays, and one deferred by a
/// re-execution, carry the keys their calls had.
const COMMANDS: &str = r#"
native fn decl(v)
native fn kick(v)
native fn inp()

fn pusher(i)
  kick(i)
  decl("pusher {i}")
  nil
end

fn gate(i)
  if inp() > 5 then
    kick(100 + i)
  end
  inp() > 3
end

fn panel()
  let a = gate(1)
  decl("panel {a}")
  nil
end

for i in range(3) do pusher(i) end
panel()
"#;

#[test]
fn commands_are_reissued_and_deferred_with_their_keys() {
    let pointers = [0, 0, 0, 4, 4, 6, 6, 7, 2, 2, 9];
    let (worlds, frames) = same_everywhere(COMMANDS, &pointers, None);
    let kicks: Vec<&String> = frames[1].iter().filter(|l| l.starts_with("kick ")).collect();
    assert_eq!(kicks.len(), 3, "{:?}", frames[1]);
    assert!(kicks.iter().all(|l| !l.contains("nokey")), "{kicks:?}");
    let s = worlds[1].stats();
    assert!(s.commands > 0, "the pushers replayed and re-issued: {s:?}");
    assert!(s.reexecs > 0, "gate was re-executed: {s:?}");
}

/// A probe that reads its key cannot be re-evaluated (validation has no
/// call path), so its scope is not recorded; the answer is still right.
const PROBE: &str = r#"
native fn decl(v)
native fn near(v)

fn spot(i)
  decl("spot {i} {near(i)}")
  nil
end

for i in range(4) do spot(i) end
"#;

#[test]
fn a_probe_that_reads_its_key_is_never_replayed() {
    let (worlds, _) = same_everywhere(PROBE, &[0, 1, 1, 2, 3, 3], None);
    let s = worlds[1].stats();
    assert_eq!(s.hits, 0, "{s:?}");
}

const RELOAD_BEFORE: &str = r#"
native fn decl(v)

fn crate_at(i)
  decl("crate {i}")
  nil
end

fn scene()
  for i in range(3) do crate_at(i) end
  decl("lamp")
  nil
end

scene()
"#;

/// The same call paths with code added around them.
const RELOAD_KEEPS: &str = r#"
native fn decl(v)

fn unrelated(x)
  x * 2
end

fn crate_at(i)
  let twice = unrelated(i)
  decl("crate {i}")
  nil
end

fn scene()
  for i in range(3) do crate_at(i) end
  decl("lamp")
  nil
end

let ignored = unrelated(4)
scene()
"#;

/// A call of `decl` added in front of the lamp's in the same function: the
/// lamp's call is now the second `decl` in `scene`, so its key moves, as a
/// `state` slot would.
const RELOAD_SHIFTS: &str = r#"
native fn decl(v)

fn crate_at(i)
  decl("crate {i}")
  nil
end

fn scene()
  for i in range(3) do crate_at(i) end
  decl("sign")
  decl("lamp")
  nil
end

scene()
"#;

#[test]
fn a_reload_that_keeps_the_call_path_keeps_the_keys() {
    let (_, frames) = same_everywhere(RELOAD_BEFORE, &[0; 4], Some(RELOAD_KEEPS));
    assert_eq!(keys_by_value(&frames[0]), keys_by_value(&frames[3]));

    let (_, frames) = same_everywhere(RELOAD_BEFORE, &[0; 4], Some(RELOAD_SHIFTS));
    let (before, after) = (keys_by_value(&frames[0]), keys_by_value(&frames[3]));
    for i in 0..3 {
        let v = format!("\"crate {i}\"");
        assert_eq!(before[&v], after[&v], "{v}");
    }
    assert_ne!(before["\"lamp\""], after["\"lamp\""]);
    assert_eq!(before["\"lamp\""], after["\"sign\""], "the first call keeps its key");
}

#[test]
fn a_native_that_asks_for_no_key_pays_nothing_and_sees_nothing() {
    // `decl` asks; a plain emitter does not, and its values carry no key.
    let mut w = world(
        "native fn decl(v)\nfn f() decl(1) end\nf()\nf()\n",
        Mode::Flat,
    );
    let out = w.frame(0);
    assert_eq!(out.len(), 2);
    let keys: HashSet<&str> = out.iter().map(|l| l.split_once(' ').unwrap().0).collect();
    assert_eq!(keys.len(), 2, "two call sites of `f`, two keys: {out:?}");
}
