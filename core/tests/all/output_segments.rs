//! Segmented output (`petal::output_segments`): a replayed memo scope
//! reaches the host as one segment reference, and a host that keeps the
//! definitions it was given rebuilds exactly the buffer an unsegmented run
//! would have filled.
//!
//! The oracle: every script below is run frame by frame with memoization off
//! (the flat buffer is the reference), with it on and the buffer flat, and
//! with it on and the buffer segmented, drained through a
//! [`SegmentCache`] and also through the flattening `take_output_buffer`. All
//! must agree on every frame, through collections, re-recordings, cutoffs,
//! effectful scopes and reloads.

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::output_segments::{SegItem, SegmentCache, SegmentDef, SegmentedFrame};
use petal::policy::RunPolicy;
use petal::stack::StackKey;
use petal::value::Value;

fn put(c: &mut PetalCxt) -> NativeResult {
    let v = c.get_value(1)?;
    let sym = c.intern_symbol("out");
    c.push_output(sym, v);
    c.push_nil();
    Ok(1)
}

/// A value as text, from the heap it lives on (the runs compared do not
/// share a heap).
fn show(heap: &Heap, v: Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f:?}"),
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".into(),
        Value::String(id) => format!("{:?}", heap.get_string(id)),
        Value::Vec3(id) => format!("{:?}", heap.get_vec3(id)),
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drain {
    Flat,
    /// Segmented, drained through a `SegmentCache`.
    Segments,
    /// Segmented, drained flat with `take_output_buffer`.
    SegmentedFlat,
}

struct World {
    env: Env,
    sid: StackKey,
    drain: Drain,
    cache: SegmentCache,
    /// Every drain, for the tests that look at the segments themselves.
    frames: Vec<SegmentedFrame>,
}

fn world(src: &str, memo: bool, drain: Drain) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = memo;
    policy.gate = false;
    env.set_policy(policy);
    env.register_native("put", put, NativeEffects::EMITS);
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    if drain != Drain::Flat {
        let out = env.intern_symbol("out");
        env.set_output_segmented(out, true);
    }
    World {
        env,
        sid,
        drain,
        cache: SegmentCache::default(),
        frames: vec![],
    }
}

impl World {
    fn frame(&mut self) -> Vec<String> {
        self.env.reset_stack(self.sid).unwrap();
        self.env
            .run(self.sid)
            .unwrap_or_else(|e| panic!("run: {e}"));
        self.env.take_output();
        let out = self.env.intern_symbol("out");
        let values = match self.drain {
            Drain::Flat | Drain::SegmentedFlat => self.env.take_output_buffer(out),
            Drain::Segments => {
                let frame = self.env.take_output_segments(out);
                let values = self.cache.apply(&frame);
                self.frames.push(frame);
                values
            }
        };
        let heap = self.env.heap();
        values
            .iter()
            .map(|v| {
                assert!(heap.is_live(*v), "a drained value was collected");
                show(heap, *v)
            })
            .collect()
    }

    fn reload(&mut self, src: &str) {
        self.env
            .reload_program(self.sid, src, None)
            .unwrap_or_else(|e| panic!("reload: {e}"));
    }
}

/// Run `src` for `frames` frames under every drain and compare. `edit`, if
/// given, is reloaded before frame `frames / 2`.
fn same_everywhere(src: &str, frames: usize, edit: Option<&str>) -> Vec<World> {
    let mut worlds = vec![
        world(src, false, Drain::Flat),
        world(src, true, Drain::Flat),
        world(src, true, Drain::Segments),
        world(src, true, Drain::SegmentedFlat),
    ];
    for f in 0..frames {
        if f == frames / 2
            && let Some(e) = edit
        {
            worlds.iter_mut().for_each(|w| w.reload(e));
        }
        let outs: Vec<Vec<String>> = worlds.iter_mut().map(World::frame).collect();
        for (w, o) in worlds.iter().zip(&outs).skip(1) {
            assert_eq!(
                &outs[0],
                o,
                "frame {f}: {:?} with memo {} differs from the flat unmemoized buffer",
                w.drain,
                w.env.memo_enabled()
            );
        }
    }
    worlds
}

/// A static helper, a nested one, a dynamic one, top-level emits between
/// them, a scope that prints (effectful) around an emitting child, and
/// enough garbage per frame that the collector runs.
const SCENE: &str = r#"
import vec2
native fn put(v)

const CITY = map(range(0, 300), fn(i) {name: "block{i % 8}", pos: vec3(float(i), 0.0, 1.0)} end)

fn block(b)
  put({mesh: b.name, pos: b.pos})
  nil
end

fn district(bs)
  for b in bs do block(b) end
  nil
end

fn city(c)
  put("city start")
  district(slice(c, 0, 100))
  put("between")
  district(slice(c, 100, 300))
  nil
end

fn ship(i, t)
  put({ship: i, at: t * float(i)})
  nil
end

fn loud(n)
  print("loud")
  put("loud {n}")
  block({name: "inner", pos: vec3(0.0, 0.0, 0.0)})
  nil
end

state frame = 0
frame += 1
let t = float(frame)

put("top")
city(CITY)
for i in range(0, 5) do ship(i, t) end
loud(frame % 2)
if frame % 4 < 2 then city(CITY) end
put("end {frame % 3}")
let garbage = map(range(0, 3000), fn(i) "g{i}{frame}" end)
"#;

#[test]
fn segmented_drains_rebuild_the_flat_buffer() {
    let worlds = same_everywhere(SCENE, 24, None);
    assert!(
        worlds[2].env.heap().collections() > 0,
        "the collector never ran"
    );
    let seg = &worlds[2];
    let last = seg.frames.last().unwrap();
    // `city` replays: one reference for its 302 values.
    assert!(
        last.items.len() < 40,
        "a replayed frame is mostly segment references, got {} items",
        last.items.len()
    );
    assert!(last.items.iter().any(|i| matches!(i, SegItem::Segment(_))));
    // The first `city` call is the same recording from the first frame on;
    // the second comes and goes (it is evicted on the frames that skip it),
    // so it is recorded again, under a new serial, each time it comes back.
    let first_city: Vec<SegItem> = seg.frames.iter().map(|f| f.items[1]).collect();
    assert!(matches!(first_city[0], SegItem::Segment(_)));
    assert!(
        first_city.iter().all(|i| *i == first_city[0]),
        "{first_city:?}"
    );
}

#[test]
fn a_replayed_scope_is_one_reference_and_is_defined_once() {
    let src = "native fn put(v)\n\
               fn wall(n)\n  for i in range(0, n) do put(i) end\n  nil\nend\n\
               wall(1000)\nput(\"tail\")\n";
    let mut w = world(src, true, Drain::Segments);
    w.frame();
    w.frame();
    w.frame();
    let (first, second, third) = (&w.frames[0], &w.frames[1], &w.frames[2]);
    assert_eq!(first.defined.len(), 1);
    assert_eq!(first.defined[0].items.len(), 1000);
    assert_eq!(first.items.len(), 2, "the segment and the fresh tail");
    assert!(second.defined.is_empty() && third.defined.is_empty());
    assert_eq!(
        second.items,
        first.items[..1]
            .iter()
            .copied()
            .chain([second.items[1]])
            .collect::<Vec<_>>()
    );
    assert!(
        matches!(second.items[0], SegItem::Segment(s) if Some(s) == first.defined.first().map(|d| d.serial))
    );
    assert!(second.dropped.is_empty());
}

#[test]
fn a_rerecorded_scope_keeps_its_id_and_drops_the_old_serial() {
    let src = "native fn put(v)\n\
               state n = 0\nn += 1\n\
               fn row(k)\n  put(\"row {k}\")\n  for i in range(0, 50) do put(i) end\n  nil\nend\n\
               row(n / 3)\n";
    let mut w = world(src, true, Drain::Segments);
    for _ in 0..7 {
        w.frame();
    }
    let defs: Vec<_> = w.frames.iter().flat_map(|f| f.defined.iter()).collect();
    assert!(defs.len() >= 3, "row re-recorded when n / 3 moved");
    assert!(defs.iter().all(|d| d.id == defs[0].id), "one scope, one id");
    let mut serials: Vec<u64> = defs.iter().map(|d| d.serial).collect();
    serials.dedup();
    assert_eq!(serials.len(), defs.len(), "a new serial per recording");
    let dropped: Vec<u64> = w
        .frames
        .iter()
        .flat_map(|f| f.dropped.iter().copied())
        .collect();
    assert!(
        dropped.contains(&defs[0].serial),
        "the first recording is dropped once replaced"
    );
}

#[test]
fn a_cutoff_keeps_the_serial_the_host_has() {
    // `label` reads a cell that moves every frame, so it fails validation;
    // it is pure, so `panel` re-executes it alone and, seeing the same
    // output, keeps going. Every third frame `panel` then fails on `slow`
    // and runs, and `label` (validated just now) replays inside it: the
    // host must get the body it already has, not an equal one under a new
    // serial.
    let src = "native fn put(v)\n\
               state var tick = 0\n\
               set tick = get tick + 1\n\
               state var slow = 0\n\
               if get tick % 3 == 0 then\n  set slow = get slow + 1\nend\n\
               fn label()\n  put(if get tick > 100000 then \"big\" else \"small\" end)\n  for i in range(0, 20) do put(i) end\n  nil\nend\n\
               fn panel(k)\n  put(\"panel {k}\")\n  label()\n  put(\"slow {get slow}\")\n  for i in range(0, 20) do put(i * 2) end\n  nil\nend\n\
               panel(1)\n";
    let worlds = same_everywhere(src, 10, None);
    let seg = &worlds[2];
    let stats = seg.env.memo_stats(seg.sid).unwrap();
    assert!(stats.cutoffs > 0, "no cutoff happened: {stats:?}");
    let first = &seg.frames[0].defined;
    let label = first
        .iter()
        .find(|d| d.items.len() == 21)
        .expect("label's segment");
    let panels: Vec<&SegmentDef> = seg.frames[1..]
        .iter()
        .flat_map(|f| f.defined.iter())
        .collect();
    assert!(!panels.is_empty(), "panel never ran again");
    for p in panels {
        assert_eq!(p.items.len(), 23, "only panel is redefined: {p:?}");
        assert_eq!(
            p.items[1],
            SegItem::Segment(label.serial),
            "panel holds the label the host has"
        );
    }
}

#[test]
fn a_reload_drops_every_segment() {
    let src = "native fn put(v)\n\
               fn wall(n)\n  for i in range(0, n) do put(i) end\n  nil\nend\n\
               wall(100)\n";
    let edit = "native fn put(v)\n\
                fn wall(n)\n  for i in range(0, n) do put(i) end\n  nil\nend\n\
                fn other() 1 end\n\
                wall(100)\nput(other())\n";
    let worlds = same_everywhere(src, 6, Some(edit));
    let seg = &worlds[2];
    let before = seg.frames[2].items.clone();
    let SegItem::Segment(old) = before[0] else {
        panic!("{before:?}")
    };
    let after_reload = &seg.frames[3];
    assert!(after_reload.dropped.contains(&old) || seg.frames[4].dropped.contains(&old));
}

#[test]
fn turning_segmentation_off_flattens_what_is_undrained() {
    let src = "native fn put(v)\n\
               fn wall(n)\n  for i in range(0, n) do put(i) end\n  nil\nend\n\
               wall(10)\nput(\"tail\")\n";
    let mut w = world(src, true, Drain::Segments);
    w.frame();
    w.env.reset_stack(w.sid).unwrap();
    w.env.run(w.sid).unwrap();
    let out = w.env.intern_symbol("out");
    w.env.set_output_segmented(out, false);
    assert_eq!(w.env.take_output_buffer(out).len(), 11);
    // Records made while segmented replay flat once it is off.
    w.drain = Drain::Flat;
    assert_eq!(w.frame().len(), 11);
}

#[test]
fn a_host_that_lost_its_cache_is_given_everything_again() {
    let src = "native fn put(v)\n\
               fn wall(n)\n  for i in range(0, n) do put(i) end\n  nil\nend\n\
               wall(10)\n";
    let mut w = world(src, true, Drain::Segments);
    w.frame();
    w.frame();
    assert!(w.frames[1].defined.is_empty());
    let out = w.env.intern_symbol("out");
    w.env.reset_output_segments(out);
    w.cache = SegmentCache::default();
    assert_eq!(w.frame().len(), 10);
    assert_eq!(w.frames[2].defined.len(), 1);
}

// ── Errors, mixed buffers, deep trees, liveness ─────────────────────

/// Deep recursion (past the memo frame-depth bound), a leaf that changes
/// every few frames under a chain of recorded parents, closures passed in,
/// and a state-writing scope that replays.
#[test]
fn deep_trees_closures_and_state_writes_match() {
    let src = r#"
native fn put(v)
state frame = 0
frame += 1

fn tree(d, k)
  put("enter {d}")
  if d == 0 then
    put("leaf {k}")
  else
    tree(d - 1, k)
    tree(d - 1, 0)
  end
  put("exit {d}")
  nil
end

fn deep(n)
  put(n)
  if n > 0 then deep(n - 1) end
  nil
end

fn apply(f, xs)
  for x in xs do put(f(x)) end
  nil
end

fn counter(k)
  state hits = 0
  hits += 1
  put("counter {k} {hits}")
  for i in range(0, 12) do put(i) end
  nil
end

tree(5, frame / 3)
deep(120)
apply(fn(x) x * 2 end, range(0, 30))
counter(frame / 4)
"#;
    same_everywhere(src, 16, None);
}

/// A scope emitting into a segmented and a flat buffer at once.
#[test]
fn a_scope_emitting_into_a_segmented_and_a_flat_buffer() {
    fn put2(c: &mut PetalCxt) -> NativeResult {
        let v = c.get_value(1)?;
        let a = c.intern_symbol("out");
        let b = c.intern_symbol("side");
        c.push_output(a, v);
        c.push_output(b, v);
        c.push_nil();
        Ok(1)
    }
    let src = "native fn put(v)\nnative fn put2(v)\nstate n = 0\nn += 1\n\
               fn row(k)\n  put2(\"row {k}\")\n  for i in range(0, 30) do put(i) end\n  put2(k)\n  nil\nend\n\
               row(1)\nrow(n / 2)\nput2(\"end\")\n";
    let run = |memo: bool, seg: bool| -> Vec<(Vec<String>, Vec<String>)> {
        let mut env = Env::new();
        env.set_echo(false);
        let mut policy = RunPolicy::FAST;
        policy.memo = memo;
        policy.gate = false;
        env.set_policy(policy);
        env.register_native("put", put, NativeEffects::EMITS);
        env.register_native("put2", put2, NativeEffects::EMITS);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let out = env.intern_symbol("out");
        let side = env.intern_symbol("side");
        if seg {
            env.set_output_segmented(out, true);
        }
        let mut cache = SegmentCache::default();
        let mut frames = vec![];
        for _ in 0..8 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            let a = if seg {
                let f = env.take_output_segments(out);
                cache.apply(&f)
            } else {
                env.take_output_buffer(out)
            };
            let b = env.take_output_buffer(side);
            let heap = env.heap();
            frames.push((
                a.iter().map(|v| show(heap, *v)).collect(),
                b.iter().map(|v| show(heap, *v)).collect(),
            ));
        }
        frames
    };
    let reference = run(false, false);
    assert_eq!(reference, run(true, false));
    assert_eq!(reference, run(true, true));
}

/// A run that fails part way, inside a scope that emitted: what the buffer
/// holds after the error must be the same segmented or not, and the next
/// frames too.
#[test]
fn a_run_that_fails_inside_a_scope_leaves_the_same_buffer() {
    let src = r#"
native fn put(v)
state frame = 0
frame += 1
fn wall(n)
  for i in range(0, n) do put(i) end
  nil
end
fn risky(k)
  put("risky {k}")
  wall(20)
  if k % 3 == 2 then
    let xs = [1, 2]
    put(xs[k])
  end
  put("after")
  nil
end
put("top")
wall(40)
risky(frame / 2)
put("end")
"#;
    let run = |memo: bool, seg: bool| -> Vec<(bool, Vec<String>)> {
        let mut env = Env::new();
        env.set_echo(false);
        let mut policy = RunPolicy::FAST;
        policy.memo = memo;
        policy.gate = false;
        env.set_policy(policy);
        env.register_native("put", put, NativeEffects::EMITS);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let out = env.intern_symbol("out");
        if seg {
            env.set_output_segmented(out, true);
        }
        let mut cache = SegmentCache::default();
        let mut frames = vec![];
        for _ in 0..14 {
            env.reset_stack(sid).unwrap();
            let ok = env.run(sid).is_ok();
            env.take_output();
            let a = if seg {
                let f = env.take_output_segments(out);
                cache.apply(&f)
            } else {
                env.take_output_buffer(out)
            };
            let heap = env.heap();
            frames.push((ok, a.iter().map(|v| show(heap, *v)).collect()));
        }
        frames
    };
    let reference = run(false, false);
    assert!(reference.iter().any(|(ok, _)| !ok), "no frame failed");
    assert_eq!(reference, run(true, false));
    assert_eq!(reference, run(true, true));
}

/// Values a host keeps from a definition stay live until the drain that
/// reports their serial dropped, even when the record holding them is
/// replaced and the collector runs before that drain.
#[test]
fn kept_definitions_stay_live_until_dropped() {
    let src = r#"
native fn put(v)
state n = 0
n += 1
fn row(k)
  for i in range(0, 40) do put("row {k} {i}") end
  nil
end
row(n)
let garbage = map(range(0, 20000), fn(i) "g{i}{n}" end)
"#;
    let mut w = world(src, true, Drain::Segments);
    let out = w.env.intern_symbol("out");
    w.frame();
    let kept: Vec<Value> = w.frames[0]
        .defined
        .iter()
        .flat_map(|d| d.items.iter())
        .filter_map(|i| match i {
            SegItem::Value(v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(kept.len(), 40);
    // The next run replaces `row`'s record; collect before draining.
    let before = w.env.heap().collections();
    // Runs that replace `row`'s record, with the collector running, all
    // before the next drain.
    for _ in 0..6 {
        w.env.reset_stack(w.sid).unwrap();
        w.env.run(w.sid).unwrap();
    }
    let heap = w.env.heap();
    assert!(heap.collections() > before, "the collector never ran");
    assert!(
        kept.iter().all(|v| heap.is_live(*v)),
        "a kept definition was collected before its serial was dropped"
    );
    let f = w.env.take_output_segments(out);
    assert!(f.dropped.contains(&w.frames[0].defined[0].serial));
}
