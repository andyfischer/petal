//! Prototype: can an immediate-mode scene API stay immediate-mode and still
//! cost O(what changed)?
//!
//! ```text
//! cargo run --release --example scene_frame [frames] [city] [crates] [ships]
//! MODES="effect (today),emit + memo + retained" cargo run ...   # a subset
//! SHIPS=math-only|orbit cargo run ...                            # ship variants
//! ```
//!
//! `scene.ptl` declares a world every frame the way a Cheesecake game does:
//! 2,000 static city meshes, 400 static crate bodies and 300 moving ships
//! (a kinematic body plus a mesh each). A mock engine host on the Rust side
//! reproduces the costs of Cheesecake's C++ host as the survey found them:
//!
//! - decode every argument tree, copying each key and string value
//!   (`view.rs` `push_shallow`),
//! - `check_options` against an allow-list (O(fields x allowed) compares),
//! - read options by linear key scan (`petal::Value::get`),
//! - key bodies by `std::string` in a hash map, diff field by field,
//!   reap with a full scan,
//! - build a ~250 byte `MeshDraw` (a `String` + matrix + material) per draw.
//!
//! The same script runs under several host strategies:
//!
//! | mode | natives | host |
//! |---|---|---|
//! | `effect`          | `EFFECT`, applied inside the call (Cheesecake today) | decode + reconcile per call |
//! | `effect+memo`     | same, memo on (it can do nothing: every scope is effectful) | |
//! | `emit+memo`       | `EMITS` into an output buffer; memo replays unchanged helpers | drains, decode + reconcile per item |
//! | `emit+memo+ident` | same | skips any item whose opts record is the *same heap object* as last frame |
//! | `emit+memo+plan`  | same | + per-shape option plans (validate once per shape) and field-level value diffs for keyed items |
//! | `emit+memo+seg`   | same | retained segments: reuse last frame's result for an item by position (`SegHost`), int keys, slot array |
//!
//! `SHIPS=orbit` declares each ship's motion (`orbit: {radius, speed, ...}`)
//! instead of this frame's position, so the declaration reads no clock, is
//! static, and replays; the host samples the motion natively. `SHIPS=math-only`
//! keeps the ships' math and drops their declarations (the VM's share).
//!
//! Every mode must produce the same world (checked: a digest of the bodies
//! and draws each frame).

#![allow(clippy::disallowed_methods, reason = "examples run natively")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use petal::env::Env;
use petal::heap::{Heap, MapId, StringId};
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::policy::RunPolicy;
use petal::value::Value;

// ─── The mock engine ────────────────────────────────────────────────────────

const BODY_ALLOWED: &[&str] = &[
    "shape", "size", "radius", "height", "mass", "static", "kinematic", "pos", "rot", "vel", "spin",
    "restitution", "friction", "rolling_friction", "damping", "angular_damping", "gravity_scale",
    "sleep", "layer", "mask", "visible", "shadows", "mesh", "material", "impact_sound",
    "impact_volume", "color", "roughness", "metallic", "emissive", "opacity", "pattern",
    "pattern_scale", "texture", "shader", "params", "unlit", "double_sided", "tint", "orbit",
];
const DRAW_ALLOWED: &[&str] = &[
    "pos", "rot", "scale", "material", "shadows", "max_distance", "min_distance", "color",
    "roughness", "metallic", "emissive", "opacity", "pattern", "pattern_scale", "texture", "shader",
    "params", "unlit", "double_sided", "tint",
];

/// What the bridge hands C++: a flattened view with owned key strings.
#[derive(Clone, Debug)]
enum DVal {
    Num(f64),
    Bool(bool),
    Str(String),
    V3([f64; 3]),
    Other,
}

struct View {
    fields: Vec<(String, DVal)>,
}

impl View {
    fn get(&self, k: &str) -> Option<&DVal> {
        self.fields.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }
    fn num(&self, k: &str, d: f64) -> f64 {
        match self.get(k) {
            Some(DVal::Num(n)) => *n,
            _ => d,
        }
    }
    fn v3(&self, k: &str) -> [f64; 3] {
        match self.get(k) {
            Some(DVal::V3(v)) => *v,
            _ => [0.0; 3],
        }
    }
}

fn decode_val(heap: &Heap, v: Value) -> DVal {
    match v {
        Value::Int(i) => DVal::Num(i as f64),
        Value::Float(f) => DVal::Num(f),
        Value::Bool(b) => DVal::Bool(b),
        Value::String(id) => DVal::Str(heap.get_string(id).to_string()),
        Value::Vec3(id) => DVal::V3(heap.get_vec3(id)),
        _ => DVal::Other,
    }
}

fn decode(heap: &Heap, v: Value) -> View {
    let Value::Map(id) = v else { return View { fields: vec![] } };
    let m = heap.get_map(id);
    View { fields: m.iter().map(|(k, v)| (k.as_str().to_string(), decode_val(heap, *v))).collect() }
}

fn check_options(v: &View, allowed: &[&str]) -> Result<(), String> {
    for (k, _) in &v.fields {
        if !allowed.iter().any(|a| a == k) {
            return Err(format!("unknown option `{k}`"));
        }
    }
    Ok(())
}

#[derive(Clone, Default, PartialEq, Debug)]
struct BodyDesc {
    shape: String,
    size: f64,
    mass: f64,
    is_static: bool,
    kinematic: bool,
    color: f64,
    roughness: f64,
    metallic: f64,
    pos: [f64; 3],
}

fn parse_body(v: &View) -> Result<BodyDesc, String> {
    check_options(v, BODY_ALLOWED)?;
    let flag = |k| matches!(v.get(k), Some(DVal::Bool(true)));
    Ok(BodyDesc {
        shape: match v.get("shape") {
            Some(DVal::Str(s)) => s.clone(),
            _ => "box".into(),
        },
        size: v.num("size", 1.0),
        mass: v.num("mass", 1.0),
        is_static: flag("static"),
        kinematic: flag("kinematic"),
        color: v.num("color", 0.0),
        roughness: v.num("roughness", 0.5),
        metallic: v.num("metallic", 0.0),
        pos: v.v3("pos"),
    })
}

struct BodyEntry {
    desc: BodyDesc,
    declared: u64,
    /// For `ident`: the opts object last applied.
    last_opts: Option<MapId>,
    /// For `plan`: the raw values last applied, by field position.
    last_vals: Vec<Value>,
    last_shape: u64,
}

#[derive(Clone, PartialEq)]
struct MeshDraw {
    mesh: String,
    xform: [f32; 16],
    material: [f32; 36],
}

fn parse_draw(name: &str, v: &View) -> Result<MeshDraw, String> {
    check_options(v, DRAW_ALLOWED)?;
    let s = match v.get("scale") {
        Some(DVal::V3(s)) => *s,
        _ => [1.0; 3],
    };
    let f = DrawFields {
        pos: v.v3("pos"),
        scale: s,
        rot: v.num("rot", 0.0),
        color: v.num("color", 0.0),
        roughness: v.num("roughness", 0.5),
        metallic: v.num("metallic", 0.0),
        emissive: v.num("emissive", 0.0),
    };
    Ok(compute_draw(name, &f))
}

struct DrawFields {
    pos: [f64; 3],
    scale: [f64; 3],
    rot: f64,
    color: f64,
    roughness: f64,
    metallic: f64,
    emissive: f64,
}

fn compute_draw(name: &str, f: &DrawFields) -> MeshDraw {
    let (p, s, rot) = (f.pos, f.scale, f.rot);
    let mut xform = [0f32; 16];
    let (c, sn) = (rot.cos() as f32, rot.sin() as f32);
    xform[0] = c * s[0] as f32;
    xform[2] = -sn * s[0] as f32;
    xform[5] = s[1] as f32;
    xform[8] = sn * s[2] as f32;
    xform[10] = c * s[2] as f32;
    xform[12] = p[0] as f32;
    xform[13] = p[1] as f32;
    xform[14] = p[2] as f32;
    xform[15] = 1.0;
    let mut material = [0f32; 36];
    material[0] = f.color as f32;
    material[1] = f.roughness as f32;
    material[2] = f.metallic as f32;
    material[3] = f.emissive as f32;
    MeshDraw { mesh: name.to_string(), xform, material }
}

#[derive(Default)]
struct Counters {
    decoded: u64,
    skipped_ident: u64,
    field_diffs: u64,
    body_changes: u64,
    reaped: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum HostMode {
    Effect,
    Emit,
    EmitIdent,
    EmitPlan,
    /// Retained segments: see `SegHost`.
    EmitSeg,
}

/// A planned decode for one record shape: validated once, then fields are
/// read by position.
struct ShapePlan {
    /// Positions of the fields this native reads (`usize::MAX` if absent):
    /// pos, scale, rot, color, roughness, metallic, emissive.
    at: [usize; 7],
}

#[derive(Default)]
struct World {
    frame: u64,
    bodies: HashMap<String, BodyEntry>,
    draws: Vec<MeshDraw>,
    /// `ident` / `plan` for unkeyed draws: last frame's draw per opts object.
    draw_cache_prev: HashMap<(MapId, StringId), MeshDraw>,
    draw_cache_next: HashMap<(MapId, StringId), MeshDraw>,
    plans: HashMap<u64, ShapePlan>,
    c: Counters,
}

fn id_to_key(heap: &Heap, id: Value) -> String {
    match id {
        Value::Int(i) => i.to_string(),
        Value::String(s) => heap.get_string(s).to_string(),
        _ => "?".into(),
    }
}

impl World {
    fn begin(&mut self) {
        self.frame += 1;
        self.draws.clear();
        std::mem::swap(&mut self.draw_cache_prev, &mut self.draw_cache_next);
        self.draw_cache_next.clear();
    }

    fn body(&mut self, heap: &Heap, id: Value, opts: Value, mode: HostMode) -> Result<(), String> {
        let key = id_to_key(heap, id);
        let frame = self.frame;
        let Value::Map(mid) = opts else { return Err("body: opts must be a record".into()) };
        if matches!(mode, HostMode::EmitIdent | HostMode::EmitPlan) {
            if let Some(e) = self.bodies.get_mut(&key) {
                if e.last_opts == Some(mid) {
                    e.declared = frame;
                    self.c.skipped_ident += 1;
                    return Ok(());
                }
                if mode == HostMode::EmitPlan {
                    // Same shape as the record applied last time: it was
                    // validated then. Compare raw values; if only `pos`
                    // moved, apply just that, without decoding the rest.
                    let m = heap.get_map(mid);
                    if m.shape_id() == e.last_shape {
                        let vals = m.values_slice();
                        let mut only_pos: Option<Option<usize>> = Some(None);
                        for (i, (a, b)) in vals.iter().zip(e.last_vals.iter()).enumerate() {
                            if !same_value(heap, *a, *b) {
                                self.c.field_diffs += 1;
                                let is_pos = m.get_index(i).map(|(k, _)| k.as_str()) == Some("pos");
                                only_pos = if is_pos { only_pos.map(|_| Some(i)) } else { None };
                                if only_pos.is_none() {
                                    break;
                                }
                            }
                        }
                        if let Some(moved) = only_pos {
                            if let Some(i) = moved {
                                if let Value::Vec3(p) = vals[i] {
                                    e.desc.pos = heap.get_vec3(p);
                                    self.c.body_changes += 1;
                                }
                            }
                            e.last_vals.clear();
                            e.last_vals.extend_from_slice(vals);
                            e.last_opts = Some(mid);
                            e.declared = frame;
                            return Ok(());
                        }
                    }
                }
            }
        }
        // The full path: what Cheesecake's `body()` does on every call.
        self.c.decoded += 1;
        let v = decode(heap, opts);
        let d = parse_body(&v)?;
        let m = heap.get_map(mid);
        let e = self.bodies.entry(key).or_insert_with(|| BodyEntry {
            desc: BodyDesc::default(),
            declared: 0,
            last_opts: None,
            last_vals: vec![],
            last_shape: 0,
        });
        if e.desc != d {
            self.c.body_changes += 1;
            e.desc = d;
        }
        e.declared = frame;
        e.last_opts = Some(mid);
        e.last_shape = m.shape_id();
        e.last_vals.clear();
        e.last_vals.extend_from_slice(m.values_slice());
        Ok(())
    }

    fn draw_mesh(&mut self, heap: &Heap, name: Value, opts: Value, mode: HostMode) -> Result<(), String> {
        let Value::Map(mid) = opts else { return Err("draw_mesh: opts must be a record".into()) };
        let Value::String(name_key) = name else { return Err("draw_mesh: name".into()) };
        if matches!(mode, HostMode::EmitIdent | HostMode::EmitPlan) {
            if let Some(d) = self.draw_cache_prev.remove(&(mid, name_key)) {
                self.c.skipped_ident += 1;
                self.draws.push(d.clone());
                self.draw_cache_next.insert((mid, name_key), d);
                return Ok(());
            }
        }
        let d = if mode == HostMode::EmitPlan {
            // Validate a shape once; afterwards read fields by position:
            // no key copies, no allow-list scan, no linear lookups.
            let m = heap.get_map(mid);
            let sid = m.shape_id();
            if !self.plans.contains_key(&sid) {
                check_options(&decode(heap, opts), DRAW_ALLOWED)?;
                let at = ["pos", "scale", "rot", "color", "roughness", "metallic", "emissive"]
                    .map(|k| m.get_index_of(k).unwrap_or(usize::MAX));
                self.plans.insert(sid, ShapePlan { at });
            }
            let at = self.plans[&sid].at;
            let vals = m.values_slice();
            let num = |i: usize, d: f64| match vals.get(i) {
                Some(Value::Float(f)) => *f,
                Some(Value::Int(n)) => *n as f64,
                _ => d,
            };
            let v3 = |i: usize, d: [f64; 3]| match vals.get(i) {
                Some(Value::Vec3(id)) => heap.get_vec3(*id),
                _ => d,
            };
            let f = DrawFields {
                pos: v3(at[0], [0.0; 3]),
                scale: v3(at[1], [1.0; 3]),
                rot: num(at[2], 0.0),
                color: num(at[3], 0.0),
                roughness: num(at[4], 0.5),
                metallic: num(at[5], 0.0),
                emissive: num(at[6], 0.0),
            };
            self.c.decoded += 1;
            compute_draw(heap.get_string(name_key), &f)
        } else {
            self.c.decoded += 1;
            parse_draw(heap.get_string(name_key), &decode(heap, opts))?
        };
        if matches!(mode, HostMode::EmitIdent | HostMode::EmitPlan) {
            self.draw_cache_next.insert((mid, name_key), d.clone());
        }
        self.draws.push(d);
        Ok(())
    }

    fn end(&mut self) {
        let frame = self.frame;
        let before = self.bodies.len();
        self.bodies.retain(|_, e| e.declared == frame);
        self.c.reaped += (before - self.bodies.len()) as u64;
    }

    fn digest(&self) -> u64 {
        let mut h: u64 = self.bodies.len() as u64;
        let mut keys: Vec<_> = self.bodies.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (k, e) in keys {
            for b in k.bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u64);
            }
            for p in e.desc.pos {
                h = h.wrapping_mul(31).wrapping_add(p.to_bits());
            }
        }
        for d in &self.draws {
            for x in d.xform {
                h = h.wrapping_mul(31).wrapping_add(x.to_bits() as u64);
            }
            h = h.wrapping_mul(31).wrapping_add(d.material[0].to_bits() as u64);
        }
        h
    }
}

fn same_value(heap: &Heap, a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Vec3(x), Value::Vec3(y)) => x == y || heap.get_vec3(x) == heap.get_vec3(y),
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::String(x), Value::String(y)) => x == y || heap.get_string(x) == heap.get_string(y),
        (Value::Map(x), Value::Map(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        _ => false,
    }
}

// ─── Retained segments ──────────────────────────────────────────────────────
//
// A replayed memo scope appends the *same* item objects it emitted last
// frame, in the same order. The host keeps last frame's items and what it
// made of each (a draw, or the slot of a body) in parallel arrays, and walks
// the new frame with a cursor: an item identical to the one under the cursor
// reuses its result by index (no hashing, no decoding, no validation). A
// miss falls back to a lookup by item id, then to the planned decode. Mesh
// names are resolved to an index once, so a draw is `Copy`. Bodies live in a
// slot array; the reap is a scan of that array.

#[derive(Clone, Copy)]
struct DrawC {
    #[allow(dead_code, reason = "the renderer would read it")]
    mesh: u32,
    xform: [f32; 16],
    material: [f32; 36],
}

#[derive(Clone, Copy)]
enum Made {
    Draw(DrawC),
    Body(u32),
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
enum BodyKey {
    Int(i64),
    Str(String),
}

impl BodyKey {
    fn digest_str(&self) -> String {
        match self {
            BodyKey::Int(i) => i.to_string(),
            BodyKey::Str(s) => s.clone(),
        }
    }
}

struct Slot {
    /// A declared motion (radius, speed, phase, y, bob, bob_phase) the host
    /// samples each frame, instead of a position the script samples.
    orbit: Option<[f64; 6]>,
    key: BodyKey,
    desc: BodyDesc,
    declared: u64,
    live: bool,
    last_vals: Vec<Value>,
    last_shape: u64,
}

#[derive(Default)]
pub struct SegHost {
    frame: u64,
    prev_items: Vec<Value>,
    prev_made: Vec<Made>,
    items: Vec<Value>,
    made: Vec<Made>,
    meshes: HashMap<StringId, u32>,
    mesh_names: Vec<String>,
    slots: Vec<Slot>,
    by_key: HashMap<BodyKey, u32>,
    plans: HashMap<u64, ShapePlan>,
    free: Vec<u32>,
    draws: Vec<DrawC>,
    decoded: u64,
    reused: u64,
}

impl SegHost {
    fn frame(&mut self, heap: &Heap, items: Vec<Value>) -> Result<(), String> {
        self.frame += 1;
        self.draws.clear();
        self.items.clear();
        self.made.clear();
        let mut cursor = 0usize;
        for it in items {
            let Value::List(l) = it else { continue };
            // Replayed scopes come back in order, so a short look-ahead
            // from the cursor finds them; a fresh item misses in O(1).
            let mut hit = None;
            for k in 0..4 {
                if cursor + k < self.prev_items.len() && self.prev_items[cursor + k] == it {
                    hit = Some(cursor + k);
                    break;
                }
            }
            let _ = l;
            let made = if let Some(i) = hit {
                cursor = i + 1;
                self.reused += 1;
                let m = self.prev_made[i];
                match m {
                    Made::Draw(d) => self.draws.push(d),
                    Made::Body(s) => self.slots[s as usize].declared = self.frame,
                }
                m
            } else {
                let xs = heap.get_list(l);
                match xs[0] {
                    Value::Int(0) => Made::Body(self.body(heap, xs[1], xs[2])?),
                    _ => {
                        let d = self.draw(heap, xs[1], xs[2])?;
                        self.draws.push(d);
                        Made::Draw(d)
                    }
                }
            };
            self.items.push(it);
            self.made.push(made);
        }
        // Sample declared motion natively: what the script's per-ship math
        // did, as a tight loop over the slots that have one.
        let t = self.frame as f64 / 60.0;
        for s in self.slots.iter_mut() {
            if let (true, Some([r, sp, ph, y, bob, bph])) = (s.live, s.orbit) {
                let a = t * sp + ph;
                s.desc.pos = [a.cos() * r, y + (t + bph).sin() * bob, a.sin() * r];
            }
        }
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.live && s.declared != self.frame {
                s.live = false;
                self.by_key.remove(&s.key);
                self.free.push(i as u32);
            }
        }
        std::mem::swap(&mut self.items, &mut self.prev_items);
        std::mem::swap(&mut self.made, &mut self.prev_made);
        Ok(())
    }

    fn body(&mut self, heap: &Heap, id: Value, opts: Value) -> Result<u32, String> {
        let key = match id {
            Value::Int(i) => BodyKey::Int(i),
            other => BodyKey::Str(id_to_key(heap, other)),
        };
        let Value::Map(mid) = opts else { return Err("body: opts".into()) };
        let m = heap.get_map(mid);
        if let Some(&si) = self.by_key.get(&key) {
            let s = &mut self.slots[si as usize];
            if m.shape_id() == s.last_shape {
                let vals = m.values_slice();
                let mut only_pos = true;
                let mut moved = None;
                for (i, (a, b)) in vals.iter().zip(s.last_vals.iter()).enumerate() {
                    if !same_value(heap, *a, *b) {
                        if m.get_index(i).map(|(k, _)| k.as_str()) == Some("pos") {
                            moved = Some(i);
                        } else {
                            only_pos = false;
                            break;
                        }
                    }
                }
                if only_pos {
                    if let Some(i) = moved {
                        if let Value::Vec3(p) = vals[i] {
                            s.desc.pos = heap.get_vec3(p);
                        }
                    }
                    s.last_vals.clear();
                    s.last_vals.extend_from_slice(vals);
                    s.declared = self.frame;
                    return Ok(si);
                }
            }
        }
        self.decoded += 1;
        let d = parse_body(&decode(heap, opts))?;
        let si = match self.by_key.get(&key) {
            Some(&si) => si,
            None => {
                let si = match self.free.pop() {
                    Some(i) => i,
                    None => {
                        self.slots.push(Slot {
                            orbit: None,
                            key: BodyKey::Int(0),
                            desc: BodyDesc::default(),
                            declared: 0,
                            live: false,
                            last_vals: vec![],
                            last_shape: 0,
                        });
                        (self.slots.len() - 1) as u32
                    }
                };
                self.by_key.insert(key.clone(), si);
                si
            }
        };
        let orbit = match m.get("orbit") {
            Some(Value::Map(o)) => {
                let o = heap.get_map(*o);
                let f = |k: &str| match o.get(k) {
                    Some(Value::Float(x)) => *x,
                    _ => 0.0,
                };
                Some([f("radius"), f("speed"), f("phase"), f("y"), f("bob"), f("bob_phase")])
            }
            _ => None,
        };
        let s = &mut self.slots[si as usize];
        s.orbit = orbit;
        s.key = key;
        s.desc = d;
        s.live = true;
        s.declared = self.frame;
        s.last_shape = m.shape_id();
        s.last_vals.clear();
        s.last_vals.extend_from_slice(m.values_slice());
        Ok(si)
    }

    fn draw(&mut self, heap: &Heap, name: Value, opts: Value) -> Result<DrawC, String> {
        let Value::String(name_id) = name else { return Err("draw_mesh: name".into()) };
        let Value::Map(mid) = opts else { return Err("draw_mesh: opts".into()) };
        let mesh = match self.meshes.get(&name_id) {
            Some(&i) => i,
            None => {
                let i = self.mesh_names.len() as u32;
                self.mesh_names.push(heap.get_string(name_id).to_string());
                self.meshes.insert(name_id, i);
                i
            }
        };
        let m = heap.get_map(mid);
        let sid = m.shape_id();
        if !self.plans.contains_key(&sid) {
            check_options(&decode(heap, opts), DRAW_ALLOWED)?;
            let at = ["pos", "scale", "rot", "color", "roughness", "metallic", "emissive"]
                .map(|k| m.get_index_of(k).unwrap_or(usize::MAX));
            self.plans.insert(sid, ShapePlan { at });
        }
        let at = self.plans[&sid].at;
        let vals = m.values_slice();
        let num = |i: usize, d: f64| match vals.get(i) {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(n)) => *n as f64,
            _ => d,
        };
        let v3 = |i: usize, d: [f64; 3]| match vals.get(i) {
            Some(Value::Vec3(id)) => heap.get_vec3(*id),
            _ => d,
        };
        let f = DrawFields {
            pos: v3(at[0], [0.0; 3]),
            scale: v3(at[1], [1.0; 3]),
            rot: num(at[2], 0.0),
            color: num(at[3], 0.0),
            roughness: num(at[4], 0.5),
            metallic: num(at[5], 0.0),
            emissive: num(at[6], 0.0),
        };
        self.decoded += 1;
        let d = compute_draw("", &f);
        Ok(DrawC { mesh, xform: d.xform, material: d.material })
    }

    fn digest(&self) -> u64 {
        let mut h: u64 = 0;
        let mut live: Vec<(String, &Slot)> =
            self.slots.iter().filter(|s| s.live).map(|s| (s.key.digest_str(), s)).collect();
        h = h.wrapping_add(live.len() as u64);
        live.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, s) in live {
            for b in k.bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u64);
            }
            for p in s.desc.pos {
                h = h.wrapping_mul(31).wrapping_add(p.to_bits());
            }
        }
        for d in &self.draws {
            for x in d.xform {
                h = h.wrapping_mul(31).wrapping_add(x.to_bits() as u64);
            }
            h = h.wrapping_mul(31).wrapping_add(d.material[0].to_bits() as u64);
        }
        h
    }
}

thread_local! {
    static SEG: RefCell<SegHost> = RefCell::new(SegHost::default());
}

thread_local! {
    static WORLD: RefCell<World> = RefCell::new(World::default());
    static MODE: RefCell<HostMode> = const { RefCell::new(HostMode::Effect) };
}

// Immediate natives (Cheesecake today): apply inside the call.
fn body_now(c: &mut PetalCxt) -> NativeResult {
    let (id, opts) = (c.get_value(1)?, c.get_value(2)?);
    c.note_effect();
    WORLD.with(|w| w.borrow_mut().body(c.heap(), id, opts, HostMode::Effect))?;
    // Cheesecake returns a fresh state record; a vec3 stands in for it.
    let p = c.heap_mut().alloc_vec3([0.0, 0.0, 0.0]);
    c.push_value(Value::Vec3(p));
    Ok(1)
}
fn draw_now(c: &mut PetalCxt) -> NativeResult {
    let (name, opts) = (c.get_value(1)?, c.get_value(2)?);
    c.note_effect();
    WORLD.with(|w| w.borrow_mut().draw_mesh(c.heap(), name, opts, HostMode::Effect))?;
    c.push_nil();
    Ok(1)
}

// Emitters: push the request; the host applies it after the run.
fn body_emit(c: &mut PetalCxt) -> NativeResult {
    let (id, opts) = (c.get_value(1)?, c.get_value(2)?);
    let sym = c.intern_symbol("scene");
    let item = c.heap_mut().alloc_list(vec![Value::Int(0), id, opts]);
    c.push_output(sym, Value::List(item));
    c.push_nil();
    Ok(1)
}
fn draw_emit(c: &mut PetalCxt) -> NativeResult {
    let (name, opts) = (c.get_value(1)?, c.get_value(2)?);
    let sym = c.intern_symbol("scene");
    let item = c.heap_mut().alloc_list(vec![Value::Int(1), name, opts]);
    c.push_output(sym, Value::List(item));
    c.push_nil();
    Ok(1)
}

struct Result_ {
    name: &'static str,
    script_us: f64,
    host_us: f64,
    total_us: f64,
    digests: Vec<u64>,
    decoded_per_frame: f64,
    skipped_per_frame: f64,
    memo_hits: u64,
}

fn run_mode(name: &'static str, source: &str, emit: bool, memo: bool, host: HostMode, frames: usize) -> Result_ {
    WORLD.with(|w| *w.borrow_mut() = World::default());
    SEG.with(|w| *w.borrow_mut() = SegHost::default());
    MODE.with(|m| *m.borrow_mut() = host);
    let mut env = Env::new();
    env.set_echo(false);
    let mut policy = RunPolicy::FAST;
    policy.memo = memo;
    policy.gate = false;
    env.set_policy(policy);
    if emit {
        env.register_native("body", body_emit, NativeEffects::EMITS);
        env.register_native("draw_mesh", draw_emit, NativeEffects::EMITS);
    } else {
        env.register_native("body", body_now, NativeEffects::EFFECT);
        env.register_native("draw_mesh", draw_now, NativeEffects::EFFECT);
    }
    let pid = env.load_program(source).unwrap_or_else(|e| panic!("{e}"));
    let sid = env.create_stack(pid).unwrap();
    let scene = env.intern_symbol("scene");
    let (mut script, mut hostt, mut times) = (vec![], vec![], vec![]);
    let mut digests = vec![];
    let warm = 30;
    for f in 0..frames + warm {
        WORLD.with(|w| w.borrow_mut().begin());
        let t0 = Instant::now();
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap_or_else(|e| panic!("{e}"));
        let t1 = Instant::now();
        if emit {
            let items = env.take_output_buffer(scene);
            let heap = env.heap();
            if host == HostMode::EmitSeg {
                SEG.with(|w| w.borrow_mut().frame(heap, items)).unwrap();
            } else {
            WORLD.with(|w| {
                let mut w = w.borrow_mut();
                for it in items {
                    let Value::List(l) = it else { continue };
                    let xs = heap.get_list(l);
                    match xs[0] {
                        Value::Int(0) => w.body(heap, xs[1], xs[2], host).unwrap(),
                        _ => w.draw_mesh(heap, xs[1], xs[2], host).unwrap(),
                    }
                }
            });
            }
        }
        WORLD.with(|w| w.borrow_mut().end());
        let t2 = Instant::now();
        env.take_output();
        if f >= warm {
            script.push((t1 - t0).as_secs_f64() * 1e6);
            hostt.push((t2 - t1).as_secs_f64() * 1e6);
            times.push((t2 - t0).as_secs_f64() * 1e6);
            digests.push(if host == HostMode::EmitSeg {
                SEG.with(|w| w.borrow().digest())
            } else {
                WORLD.with(|w| w.borrow().digest())
            });
        }
        if f == warm {
            WORLD.with(|w| w.borrow_mut().c = Counters::default());
            SEG.with(|w| {
                let mut w = w.borrow_mut();
                w.decoded = 0;
                w.reused = 0;
            });
        }
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    let (decoded, skipped) = if host == HostMode::EmitSeg {
        SEG.with(|w| {
            let w = w.borrow();
            (w.decoded as f64 / frames as f64, w.reused as f64 / frames as f64)
        })
    } else {
        WORLD.with(|w| {
            let w = w.borrow();
            (w.c.decoded as f64 / frames as f64, w.c.skipped_ident as f64 / frames as f64)
        })
    };
    let memo_hits = env.memo_stats(sid).map(|s| s.hits).unwrap_or(0);
    Result_ {
        name,
        script_us: med(&mut script),
        host_us: med(&mut hostt),
        total_us: med(&mut times),
        digests,
        decoded_per_frame: decoded,
        skipped_per_frame: skipped,
        memo_hits,
    }
}

fn main() {
    let frames: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(300);
    let arg = |i: usize, d: usize| std::env::args().nth(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (city, crates, ships) = (arg(2, 2000), arg(3, 400), arg(4, 300));
    println!("world: {city} static meshes, {crates} static bodies, {ships} moving ships (body + mesh); {frames} frames, median");
    let source = include_str!("scene.ptl")
        .replace("const N_CITY = 2000", &format!("const N_CITY = {city}"))
        .replace("const N_CRATES = 400", &format!("const N_CRATES = {crates}"))
        .replace("const N_SHIPS = 300", &format!("const N_SHIPS = {ships}"));
    let variant = std::env::var("SHIPS").unwrap_or_default();
    let source = match variant.as_str() {
        // Ships do their math but declare nothing: the VM's share.
        "math-only" => source
            .replace("  body(1000 + i, {shape: \"box\", size: 2.0, mass: 5.0, kinematic: true, color: #ff8800,\n                  roughness: 0.4, metallic: 0.2, pos: p})\n", "")
            .replace("  draw_mesh(\"ship_hull\", {pos: p, rot: a, color: #ff8800, metallic: 0.2, emissive: 0.0})\n", "  p\n"),
        // Ships declare their motion, not this frame's sample: the
        // declaration no longer reads the clock, so it is static and replays.
        "orbit" => source.replace("for i in range(0, N_SHIPS) do ship(i, t) end", "declare_fleet(N_SHIPS)")
            + "\nfn declare_fleet(n)\n  for i in range(0, n) do\n    body(1000 + i, {shape: \"box\", size: 2.0, mass: 5.0, kinematic: true, color: #ff8800,\n                    roughness: 0.4, metallic: 0.2, orbit: {radius: 60.0, speed: 0.3, phase: float(i) * 0.021, y: 12.0, bob: 2.0, bob_phase: float(i)}})\n  end\n  nil\nend\n",
        _ => source,
    };
    if !variant.is_empty() {
        println!("ship variant: {variant}");
    }
    let source = source.as_str();
    let only: Option<String> = std::env::var("MODES").ok();
    let modes: [(&'static str, bool, bool, HostMode); 7] = [
        ("effect (today)", false, false, HostMode::Effect),
        ("effect + memo", false, true, HostMode::Effect),
        ("emit (no memo)", true, false, HostMode::Emit),
        ("emit + memo", true, true, HostMode::Emit),
        ("emit + memo + identity", true, true, HostMode::EmitIdent),
        ("emit + memo + identity + shape plans", true, true, HostMode::EmitPlan),
        ("emit + memo + retained segments", true, true, HostMode::EmitSeg),
    ];
    let results: Vec<Result_> = modes
        .iter()
        .filter(|m| only.as_deref().map(|o| o.split(',').any(|x| m.0.starts_with(x))).unwrap_or(true))
        .map(|(n, e, m, h)| run_mode(n, source, *e, *m, *h, frames))
        .collect();
    println!(
        "{:<40} {:>10} {:>10} {:>10} {:>12} {:>12} {:>10}",
        "mode", "script µs", "host µs", "total µs", "decoded/fr", "skipped/fr", "memo hits"
    );
    for r in &results {
        println!(
            "{:<40} {:>10.0} {:>10.0} {:>10.0} {:>12.0} {:>12.0} {:>10}",
            r.name, r.script_us, r.host_us, r.total_us, r.decoded_per_frame, r.skipped_per_frame, r.memo_hits
        );
    }
    let same = results.iter().all(|r| r.digests == results[0].digests);
    println!("worlds identical across modes, every frame: {same}");
}
