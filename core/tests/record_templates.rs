//! Record templates (`petal::record_template`): a record literal whose fields
//! are mostly static is tagged with a template naming its static part, and
//! nothing a program can do tells a tagged record from an untagged one.
//!
//! The oracle: every script runs frame by frame with templates on and off
//! (`Env::set_record_templates`), with the static-value cache off, and with
//! it on and checking itself; all must print the same and emit the same
//! values. The tags themselves are checked against what they promise: two
//! records with one template id agree on their keys, their hole positions
//! and every field that is not a hole.

use petal::env::Env;
use petal::heap::Heap;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
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
        other => other.type_name().to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// Templates on (the default).
    On,
    Off,
    /// No static-value cache: nothing is static, no literal is marked.
    NoCache,
    /// The cache checks the values it keeps in function bodies, so they are
    /// built on every call: a static field is a new, equal object each time.
    Checking,
    /// Memoization on as well.
    Memo,
}

const MODES: [Mode; 5] = [
    Mode::On,
    Mode::Off,
    Mode::NoCache,
    Mode::Checking,
    Mode::Memo,
];

struct World {
    env: Env,
    sid: StackKey,
}

fn world(src: &str, mode: Mode) -> World {
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.gate = false;
    policy.memo = mode == Mode::Memo;
    if mode == Mode::NoCache {
        policy = policy.with_static_cache(false);
    }
    env.set_policy(policy);
    env.set_record_templates(mode != Mode::Off);
    env.set_static_check(mode == Mode::Checking);
    env.register_native("put", put, NativeEffects::EMITS);
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, sid }
}

/// One record a frame emitted: its text, and its template (id, holes) if
/// tagged.
type Emitted = (String, Option<(u64, Vec<u32>)>);

impl World {
    fn frame(&mut self) -> (Vec<String>, Vec<Emitted>) {
        self.env.reset_stack(self.sid).unwrap();
        self.env
            .run(self.sid)
            .unwrap_or_else(|e| panic!("run: {e}"));
        let out = self.env.intern_symbol("out");
        let vals = self.env.take_output_buffer(out);
        let heap = self.env.heap();
        let emitted = vals
            .iter()
            .map(|v| {
                let tmpl = match v {
                    Value::Map(id) => heap
                        .get_map(*id)
                        .template()
                        .map(|t| (t.id, t.holes.to_vec())),
                    _ => None,
                };
                (show(heap, *v), tmpl)
            })
            .collect();
        let printed = self.env.take_output();
        (printed, emitted)
    }

    fn reload(&mut self, src: &str) {
        self.env
            .reload_program(self.sid, src, None)
            .unwrap_or_else(|e| panic!("reload: {e}"));
    }
}

/// Run `src` for `frames` frames (then each of `reloads` for as many) in
/// every mode; require the same printed text and emitted values in all, and
/// return the templated run's emitted records.
fn same_in_every_mode(src: &str, frames: usize, reloads: &[&str]) -> Vec<Vec<Emitted>> {
    let mut reference: Option<Vec<(Vec<String>, Vec<String>)>> = None;
    let mut tagged = vec![];
    for mode in MODES {
        let mut w = world(src, mode);
        let mut seen = vec![];
        let mut emitted_all = vec![];
        for stage in std::iter::once(None).chain(reloads.iter().map(Some)) {
            if let Some(next) = stage {
                w.reload(next);
            }
            for _ in 0..frames {
                let (printed, emitted) = w.frame();
                seen.push((printed, emitted.iter().map(|(s, _)| s.clone()).collect()));
                emitted_all.push(emitted);
            }
        }
        if mode == Mode::Checking {
            let stats = w.env.static_values(w.sid);
            assert!(
                stats.as_ref().is_none_or(|s| s.mismatches.is_empty()),
                "static check found mismatches"
            );
        }
        match &reference {
            None => reference = Some(seen),
            Some(r) => assert_eq!(r, &seen, "mode {mode:?} differs from {:?}", MODES[0]),
        }
        if mode != Mode::On && mode != Mode::Checking && mode != Mode::Memo {
            assert!(
                emitted_all.iter().flatten().all(|(_, t)| t.is_none()),
                "mode {mode:?} tagged a record"
            );
        }
        check_promises(&emitted_all);
        if mode == Mode::On {
            tagged = emitted_all;
        }
    }
    tagged
}

/// Records with one template id agree on everything but their holes.
fn check_promises(frames: &[Vec<Emitted>]) {
    let mut by_id: std::collections::HashMap<u64, (String, Vec<u32>)> = Default::default();
    for (text, t) in frames.iter().flatten() {
        let Some((id, holes)) = t else { continue };
        let masked = mask(text, holes);
        match by_id.get(id) {
            None => {
                by_id.insert(*id, (masked, holes.clone()));
            }
            Some((m, h)) => {
                assert_eq!(h, holes, "template {id:x}: holes differ");
                assert_eq!(m, &masked, "template {id:x}: static fields differ");
            }
        }
    }
}

/// `{a: 1, b: 2}` with field `holes` replaced by `_`. Fields here never
/// hold a nested record with commas at the top level of the text.
fn mask(text: &str, holes: &[u32]) -> String {
    let inner = &text[1..text.len() - 1];
    let mut fields = vec![];
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            ',' if depth == 0 => {
                fields.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    fields.push(inner[start..].trim().to_string());
    for &h in holes {
        if let Some(f) = fields.get_mut(h as usize) {
            let key = f.split(':').next().unwrap_or("").to_string();
            *f = format!("{key}: _");
        }
    }
    fields.join(", ")
}

const SHIPS: &str = r#"
import vec2
native fn put(v)

const SIZE = 2.0

fn ship(i, t)
  let a = t * 0.3 + float(i)
  let p = vec3(cos(a), 1.0, sin(a))
  put({shape: "box", size: SIZE, mass: 5.0, kinematic: true, color: #ff8800, pos: p})
  put({mesh: "hull", pos: p, rot: a, tint: [1.0, 0.5, 0.0], emissive: 0.0})
  nil
end

state frame = 0
frame += 1
let t = float(frame) / 60.0
for i in range(0, 3) do ship(i, t) end
put({kind: "hud", label: "frame", value: frame})
"#;

#[test]
fn mostly_static_literals_are_tagged_and_mean_the_same() {
    let frames = same_in_every_mode(SHIPS, 4, &[]);
    // Every ship body and mesh is tagged, and one id per literal.
    let mut ids = std::collections::HashSet::new();
    for f in &frames {
        let tagged: Vec<_> = f.iter().filter_map(|(_, t)| t.clone()).collect();
        assert_eq!(tagged.len(), 7, "{f:?}");
        for (id, _) in tagged {
            ids.insert(id);
        }
    }
    assert_eq!(ids.len(), 3, "body, mesh and hud literals");
    // The body's only hole is `pos`; the mesh's are `pos` and `rot`.
    let first = &frames[0];
    assert_eq!(first[0].1.as_ref().unwrap().1, vec![5]);
    assert_eq!(first[1].1.as_ref().unwrap().1, vec![1, 2]);
    // The top-level hud literal: `value` reads state.
    assert_eq!(first[6].1.as_ref().unwrap().1, vec![2]);
}

#[test]
fn a_mostly_dynamic_literal_is_not_tagged() {
    let src = r#"
native fn put(v)
fn f(a, b, c)
  put({a: a, b: b, c: c, d: 1})
  put({a: a, b: b, c: 1, d: 1})
end
for i in range(0, 3) do f(i, i * 2, "x{i}") end
"#;
    let frames = same_in_every_mode(src, 2, &[]);
    for f in &frames {
        assert!(f[0].1.is_none(), "three holes of four: untagged");
        assert!(f[1].1.is_some(), "two of four: tagged");
    }
}

#[test]
fn a_write_drops_the_template_and_a_copy_keeps_it() {
    let src = r#"
native fn put(v)
fn mk(p)
  {shape: "box", size: 1.0, pos: p}
end
let a = mk(1)
var b = mk(2)
set b.size = 3.0
var c = mk(3)
set c.pos = 9
let d = {...mk(4), extra: 1}
let e = a
put(a)
put(b)
put(c)
put(d)
put(e)
print(a == mk(1))
print(b)
print(c)
"#;
    let frames = same_in_every_mode(src, 2, &[]);
    let f = &frames[0];
    assert!(f[0].1.is_some(), "a: as built");
    assert!(f[1].1.is_none(), "b: a static field written");
    assert!(f[2].1.is_none(), "c: any write drops it");
    assert!(f[3].1.is_none(), "d: a spread builds a new record");
    assert_eq!(f[4].1, f[0].1, "e is a");
}

#[test]
fn the_id_survives_a_reload_that_leaves_the_literal_alone_and_changes_with_it() {
    let v1 = r#"
native fn put(v)
fn ship(i)
  put({shape: "box", size: 2.0, pos: i})
  put({shape: "ball", radius: 1.0, pos: i})
end
for i in range(0, 2) do ship(i) end
"#;
    // A line added elsewhere: the literals are the same.
    let v2 = v1.replace("for i in range", "print(\"hi\")\nfor i in range");
    // The second literal's size changes.
    let v3 = v2.replace("radius: 1.0", "radius: 1.5");
    let frames = same_in_every_mode(v1, 2, &[&v2, &v3]);
    let id = |f: usize, i: usize| frames[f][i].1.as_ref().unwrap().0;
    assert_eq!(id(0, 0), id(1, 0));
    assert_eq!(id(0, 0), id(2, 0), "after a reload that moved text");
    assert_eq!(id(0, 1), id(2, 1));
    assert_eq!(
        id(0, 0),
        id(4, 0),
        "after a reload that changed the other literal"
    );
    assert_ne!(id(0, 1), id(4, 1), "the changed literal has a new id");
    // A fresh Env (another run of the program) gets the same ids.
    let mut w = world(v1, Mode::On);
    let (_, e) = w.frame();
    assert_eq!(e[0].1.as_ref().unwrap().0, id(0, 0));
    assert_eq!(e[1].1.as_ref().unwrap().0, id(0, 1));
}

#[test]
fn ints_and_floats_and_nested_values_are_named_exactly() {
    let src = r#"
native fn put(v)
fn a(p)
  put({n: 1, s: "x", l: [1, 2], r: {k: "v"}, pos: p})
end
fn b(p)
  put({n: 1.0, s: "x", l: [1, 2], r: {k: "v"}, pos: p})
end
fn c(p)
  put({n: 1, s: "x", l: [1, 2.0], r: {k: "v"}, pos: p})
end
a(1)
b(1)
c(1)
"#;
    let frames = same_in_every_mode(src, 2, &[]);
    let ids: Vec<u64> = frames[0]
        .iter()
        .map(|(_, t)| t.as_ref().unwrap().0)
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
    assert_ne!(ids[1], ids[2]);
}

#[test]
fn a_field_the_classifier_calls_static_is_still_checked() {
    // A top-level loop variable is not static: only the literal fields are.
    // A field that reads a top-level `var` changes between frames.
    let src = r#"
native fn put(v)
var k = 0
fn f(i, k)
  put({shape: "box", size: 1.0, mass: 2.0, k: k, pos: i})
end
state frame = 0
frame += 1
set k = frame % 2
for i in range(0, 3) do
  put({shape: "s", size: 1.0, n: i})
  f(i, k)
end
"#;
    let frames = same_in_every_mode(src, 4, &[]);
    for f in &frames {
        for (_, t) in f {
            assert!(t.is_some());
        }
    }
}

#[test]
fn a_closure_in_a_static_field_rejects_the_site() {
    let src = r#"
native fn put(v)
fn g(x)
  x + 1
end
fn f(p)
  put({on_hit: g, size: 1.0, pos: p})
end
f(1)
f(2)
"#;
    let frames = same_in_every_mode(src, 2, &[]);
    for f in &frames {
        for (_, t) in f {
            assert!(t.is_none());
        }
    }
}

#[test]
fn tags_follow_a_record_through_memo_replays() {
    let src = r#"
native fn put(v)
fn crates(n)
  for i in range(0, n) do put({shape: "box", size: 1.0, static: true, pos: i}) end
  nil
end
crates(4)
"#;
    let mut w = world(src, Mode::Memo);
    let mut first = None;
    for _ in 0..4 {
        let (_, e) = w.frame();
        let ids: Vec<_> = e.iter().map(|(_, t)| t.as_ref().map(|t| t.0)).collect();
        assert!(ids.iter().all(|i| i.is_some()));
        match &first {
            None => first = Some(ids),
            Some(f) => assert_eq!(f, &ids),
        }
    }
    let stats = w.env.record_template_stats(w.sid).unwrap();
    assert_eq!(stats.rejected, 0);
}

#[test]
fn a_patched_config_value_gives_its_literals_a_new_id() {
    // An edit of a `config let` literal is patched in place (no recompile):
    // the literal's static field changes under the same code.
    let src = r#"
native fn put(v)
config let SIZE = 2.0
config let SHAPE = "box"
fn ship(i)
  put({shape: SHAPE, size: SIZE, mass: 1.0, pos: i})
end
for i in range(0, 3) do ship(i) end
"#;
    let v2 = src.replace("SIZE = 2.0", "SIZE = 3.0");
    let v3 = v2.replace("SHAPE = \"box\"", "SHAPE = \"ball\"");
    let frames = same_in_every_mode(src, 2, &[&v2, &v3]);
    assert!(
        frames[5][0].0.contains("ball") && frames[5][0].0.contains("3.0"),
        "{:?}",
        frames[5][0]
    );
    let id = |f: usize| frames[f][0].1.as_ref().expect("tagged").0;
    assert_eq!(id(0), id(1));
    assert_ne!(id(1), id(2));
    assert_eq!(id(2), id(3));
    assert_ne!(id(3), id(4));
    assert_ne!(id(4), id(0));
}
