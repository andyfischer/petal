//! Option plans: read a record's options by position, checking its field
//! names once per record shape (`pb_plan`), and compare a keyed
//! declaration's options with last frame's (`pb_snapshot`).
//!
//! A host native that takes an options record (`body(id, {shape: "box",
//! pos: p})`) used to check every field against an allow-list and then find
//! each option with a linear scan by name, on every call. But a record
//! literal's keys are fixed when the program is lowered: every record the
//! literal makes shares one shape, and a decoded `PB_MAP` carries that
//! shape's id (`integer`). A plan lists the options a native understands;
//! the first time it sees a shape it maps each option to the field position
//! that holds it (or none) and notes the first field that is not an option.
//! After that, binding a record of that shape is a lookup by shape id and
//! one pointer per option.
//!
//! Shape ids are unique for the life of the process, and keys are only ever
//! appended under an id (`petal::record`), so an entry stays valid for a
//! record with as many fields as it was built for; a record with more fields
//! under the same id is planned again.
//!
//! A snapshot keeps an encoding of the option values it was last given, so a
//! host that keeps one per keyed declaration (a body) learns which options
//! changed since the last frame and applies only those: a moving ship whose
//! record differs from last frame's only in `pos` updates one field.

use std::collections::HashMap;
use std::ffi::{CString, c_char};

use petal::fxhash::FxBuildHasher;

use crate::ffi::guard;
use crate::view::{Kind, PbValue};

/// No such field.
const NONE: u32 = u32::MAX;
/// Past this many planned shapes the plan starts over.
const MAX_ENTRIES: usize = 4096;

/// What a plan knows about one shape.
struct Entry {
    /// The field count it was built for.
    count: u32,
    /// (option, position of its field) for each option the shape has.
    present: Box<[(u32, u32)]>,
    /// The first field that is not an option, or `NONE`.
    unknown: u32,
}

/// The state behind a `pb_plan*`.
pub struct Plan {
    names: Vec<CString>,
    by_name: HashMap<Box<[u8]>, u32, FxBuildHasher>,
    entries: Vec<Entry>,
    by_shape: HashMap<u64, u32, FxBuildHasher>,
    /// The last shape bound: (shape id, field count, entry).
    last: (u64, u32, u32),
    /// For records without a shape id (not decoded by this bridge).
    scratch: Option<Entry>,
}

/// The field name of a map entry, or `None`.
fn key_of(field: &PbValue) -> Option<&[u8]> {
    if field.key.is_null() {
        return None;
    }
    // SAFETY: a decoded view's key points at `key_len` bytes.
    Some(unsafe { std::slice::from_raw_parts(field.key as *const u8, field.key_len) })
}

/// The fields of a `PB_MAP` view.
fn fields_of(rec: &PbValue) -> &[PbValue] {
    if rec.items.is_null() || rec.count == 0 {
        return &[];
    }
    // SAFETY: a decoded view's items point at `count` contiguous nodes.
    unsafe { std::slice::from_raw_parts(rec.items, rec.count as usize) }
}

impl Plan {
    /// A plan for these option names (comma-separated, whitespace ignored).
    /// `None` when a name is empty or repeats.
    pub fn new(options: &str) -> Option<Plan> {
        let mut names = Vec::new();
        let mut by_name = HashMap::default();
        if !options.trim().is_empty() {
            for n in options.split(',') {
                let n = n.trim();
                if n.is_empty() || n.contains('\0') {
                    return None;
                }
                if by_name
                    .insert(n.as_bytes().into(), names.len() as u32)
                    .is_some()
                {
                    return None;
                }
                names.push(CString::new(n).ok()?);
            }
        }
        Some(Plan {
            names,
            by_name,
            entries: Vec::new(),
            by_shape: HashMap::default(),
            last: (0, 0, 0),
            scratch: None,
        })
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn build(&self, rec: &PbValue) -> Entry {
        let mut present = Vec::new();
        let mut unknown = NONE;
        for (j, f) in fields_of(rec).iter().enumerate() {
            match key_of(f).and_then(|k| self.by_name.get(k)) {
                Some(&i) => present.push((i, j as u32)),
                None if unknown == NONE => unknown = j as u32,
                None => {}
            }
        }
        Entry {
            count: rec.count,
            present: present.into_boxed_slice(),
            unknown,
        }
    }

    /// The entry for `rec`'s shape, planning it if needed.
    fn entry(&mut self, rec: &PbValue) -> &Entry {
        let shape = rec.integer as u64;
        if shape == 0 {
            let e = self.build(rec);
            return self.scratch.insert(e);
        }
        let (ls, lc, le) = self.last;
        if ls == shape && lc == rec.count {
            return &self.entries[le as usize];
        }
        let idx = match self.by_shape.get(&shape) {
            Some(&i) if self.entries[i as usize].count == rec.count => i,
            found => {
                let e = self.build(rec);
                match found {
                    Some(&i) => {
                        self.entries[i as usize] = e;
                        i
                    }
                    None => {
                        if self.entries.len() >= MAX_ENTRIES {
                            self.entries.clear();
                            self.by_shape.clear();
                        }
                        self.entries.push(e);
                        let i = (self.entries.len() - 1) as u32;
                        self.by_shape.insert(shape, i);
                        i
                    }
                }
            }
        };
        self.last = (shape, rec.count, idx);
        &self.entries[idx as usize]
    }

    /// Fill `out` (one slot per option) with `rec`'s fields. `Ok` when every
    /// field is an option (or `rec` is nil); `Err(Some(j))` names the first
    /// field that is not; `Err(None)` when `rec` is not a record.
    pub fn bind(
        &mut self,
        rec: Option<&PbValue>,
        out: &mut [*const PbValue],
    ) -> Result<(), Option<u32>> {
        out.fill(std::ptr::null());
        let rec = match rec {
            None => return Ok(()),
            Some(r) if r.kind == Kind::Nil as u32 => return Ok(()),
            Some(r) if r.kind != Kind::Map as u32 => return Err(None),
            Some(r) => r,
        };
        let items = rec.items;
        let e = self.entry(rec);
        for &(i, j) in e.present.iter() {
            if let Some(o) = out.get_mut(i as usize) {
                // SAFETY: j < rec.count: the entry was built for this shape.
                *o = unsafe { items.add(j as usize) };
            }
        }
        if e.unknown == NONE {
            Ok(())
        } else {
            Err(Some(e.unknown))
        }
    }
}

// ─── Snapshots ──────────────────────────────────────────────────────────────

/// What a snapshot holds for one option. A scalar is held inline, so an
/// unchanged scalar costs a compare and no copy; text and containers are
/// encoded into the snapshot's byte buffer.
#[derive(Clone, Copy, PartialEq)]
struct Held {
    /// The `pb_kind` of a scalar, or one of the `HELD_*` tags.
    tag: u32,
    words: [u64; 3],
}

const HELD_ABSENT: u32 = u32::MAX;
/// Bytes `words[0]..words[1]` of the buffer.
const HELD_BLOB: u32 = u32::MAX - 1;
/// Something whose contents a view does not carry (a closure, a pending
/// value): never equal to anything.
const HELD_OPAQUE: u32 = u32::MAX - 2;

const ABSENT: Held = Held {
    tag: HELD_ABSENT,
    words: [0; 3],
};

/// The state behind a `pb_snapshot*`.
#[derive(Default)]
pub struct Snapshot {
    held: Vec<Held>,
    bytes: Vec<u8>,
    next_bytes: Vec<u8>,
    stack: Vec<*const PbValue>,
}

fn put_u64(out: &mut Vec<u8>, x: u64) {
    out.extend_from_slice(&x.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, p: *const c_char, len: usize) {
    put_u64(out, len as u64);
    if !p.is_null() && len > 0 {
        // SAFETY: a decoded view's text points at `len` bytes.
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(p as *const u8, len) });
    }
}

/// A scalar's exact contents, or `None` for text, containers and opaque
/// values.
fn scalar(n: &PbValue) -> Option<Held> {
    let words = match n.kind {
        k if k == Kind::Nil as u32 => [0; 3],
        k if k == Kind::Bool as u32 || k == Kind::Int as u32 || k == Kind::Symbol as u32 => {
            [n.integer as u64, 0, 0]
        }
        k if k == Kind::Float as u32 => [n.number.to_bits(), 0, 0],
        k if k == Kind::Vec2 as u32 => [n.number.to_bits(), n.y.to_bits(), 0],
        k if k == Kind::Vec3 as u32 => [n.number.to_bits(), n.y.to_bits(), n.z.to_bits()],
        // Slot, serial and class index: slots and serials are per class.
        k if k == Kind::Handle as u32 => [n.integer as u64, n.count as u64, n.number.to_bits()],
        _ => return None,
    };
    Some(Held { tag: n.kind, words })
}

impl Snapshot {
    /// Append an exact encoding of `v` (contents, the keys of map entries
    /// below it, children) to `next_bytes`. False when part of it is opaque.
    fn encode(&mut self, v: *const PbValue) -> bool {
        let out = &mut self.next_bytes;
        self.stack.clear();
        self.stack.push(v);
        let mut root = true;
        while let Some(p) = self.stack.pop() {
            // SAFETY: `p` is the host's view or a child of one.
            let n = unsafe { &*p };
            out.push(n.kind as u8);
            // The root's key is the option's name, the same every time.
            if !root && !n.key.is_null() {
                put_bytes(out, n.key, n.key_len);
            }
            root = false;
            if let Some(h) = scalar(n) {
                for w in h.words {
                    put_u64(out, w);
                }
                continue;
            }
            match n.kind {
                k if k == Kind::String as u32 => put_bytes(out, n.str_, n.str_len),
                k if k == Kind::List as u32
                    || k == Kind::Map as u32
                    || k == Kind::Enum as u32
                    || k == Kind::Dict as u32 =>
                {
                    // Map: class name; enum: tag. Keys are on the children.
                    put_bytes(out, n.str_, n.str_len);
                    put_u64(out, n.count as u64);
                    if !n.items.is_null() {
                        for i in (0..n.count as usize).rev() {
                            // SAFETY: `items` points at `count` nodes.
                            self.stack.push(unsafe { n.items.add(i) });
                        }
                    }
                }
                // Pending, Other (closures, native functions…), anything newer.
                _ => return false,
            }
        }
        true
    }

    /// What to hold for `v`.
    fn hold(&mut self, v: *const PbValue) -> Held {
        // SAFETY: a non-null slot is a view the host was handed.
        let Some(n) = (unsafe { v.as_ref() }) else {
            return ABSENT;
        };
        if let Some(h) = scalar(n) {
            return h;
        }
        let start = self.next_bytes.len();
        if !self.encode(v) {
            self.next_bytes.truncate(start);
            return Held {
                tag: HELD_OPAQUE,
                words: [0; 3],
            };
        }
        Held {
            tag: HELD_BLOB,
            words: [start as u64, self.next_bytes.len() as u64, 0],
        }
    }

    fn same(&self, old: &Held, new: &Held) -> bool {
        match (old.tag, new.tag) {
            (HELD_OPAQUE, _) | (_, HELD_OPAQUE) => false,
            (HELD_BLOB, HELD_BLOB) => {
                let (a, b) = (old.words, new.words);
                self.bytes[a[0] as usize..a[1] as usize]
                    == self.next_bytes[b[0] as usize..b[1] as usize]
            }
            _ => old == new,
        }
    }

    /// Compare `values` (one per option, null = absent) with what the
    /// snapshot holds, set bit `i` of `changed` for each option `i` that
    /// differs, and hold `values` from now on. Returns whether any differs.
    pub fn update(&mut self, values: &[*const PbValue], mut changed: Option<&mut [u64]>) -> bool {
        if let Some(c) = changed.as_deref_mut() {
            c.fill(0);
        }
        self.next_bytes.clear();
        let mut any = false;
        let mut mark = |i: usize, changed: &mut Option<&mut [u64]>| {
            any = true;
            if let Some(word) = changed.as_deref_mut().and_then(|c| c.get_mut(i / 64)) {
                *word |= 1 << (i % 64);
            }
        };
        // An option held before but not passed now went absent.
        for i in values.len()..self.held.len() {
            if self.held[i].tag != HELD_ABSENT {
                mark(i, &mut changed);
            }
        }
        self.held.resize(values.len(), ABSENT);
        for (i, &v) in values.iter().enumerate() {
            if v.is_null() && self.held[i].tag == HELD_ABSENT {
                continue; // most options of a record are absent, and stay so
            }
            let new = self.hold(v);
            let old = self.held[i];
            if !self.same(&old, &new) {
                mark(i, &mut changed);
            }
            self.held[i] = new;
        }
        std::mem::swap(&mut self.bytes, &mut self.next_bytes);
        any
    }

    pub fn clear(&mut self) {
        self.held.clear();
        self.bytes.clear();
    }
}

// ─── C API ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_plan_new(options: *const c_char) -> *mut Plan {
    if options.is_null() {
        return std::ptr::null_mut();
    }
    let text = unsafe { std::ffi::CStr::from_ptr(options) }
        .to_string_lossy()
        .into_owned();
    guard(std::ptr::null_mut(), || match Plan::new(&text) {
        Some(p) => Box::into_raw(Box::new(p)),
        None => std::ptr::null_mut(),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_plan_free(plan: *mut Plan) {
    if !plan.is_null() {
        guard((), || drop(unsafe { Box::from_raw(plan) }));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_plan_count(plan: *const Plan) -> usize {
    unsafe { plan.as_ref() }.map_or(0, Plan::len)
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_plan_name(plan: *const Plan, i: usize) -> *const c_char {
    unsafe { plan.as_ref() }
        .and_then(|p| p.names.get(i))
        .map_or(std::ptr::null(), |n| n.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_plan_bind(
    plan: *mut Plan,
    record: *const PbValue,
    out: *mut *const PbValue,
    out_unknown: *mut *const PbValue,
) -> bool {
    guard(false, || {
        if !out_unknown.is_null() {
            unsafe { out_unknown.write(std::ptr::null()) };
        }
        let Some(plan) = (unsafe { plan.as_mut() }) else {
            return false;
        };
        let rec = unsafe { record.as_ref() };
        let mut none: [*const PbValue; 0] = [];
        let out: &mut [*const PbValue] = if out.is_null() {
            &mut none
        } else {
            // SAFETY: the caller passes pb_plan_count slots.
            unsafe { std::slice::from_raw_parts_mut(out, plan.len()) }
        };
        if out.len() < plan.len() {
            // No slots: bind into a scratch array for the verdict.
            let mut tmp = vec![std::ptr::null(); plan.len()];
            return finish_bind(plan.bind(rec, &mut tmp), rec, out_unknown);
        }
        finish_bind(plan.bind(rec, out), rec, out_unknown)
    })
}

fn finish_bind(
    r: Result<(), Option<u32>>,
    rec: Option<&PbValue>,
    out_unknown: *mut *const PbValue,
) -> bool {
    match r {
        Ok(()) => true,
        Err(Some(j)) => {
            if let (false, Some(rec)) = (out_unknown.is_null(), rec) {
                // SAFETY: j < rec.count.
                unsafe { out_unknown.write(rec.items.add(j as usize)) };
            }
            false
        }
        Err(None) => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_snapshot_new() -> *mut Snapshot {
    guard(std::ptr::null_mut(), || Box::into_raw(Box::default()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_snapshot_free(snap: *mut Snapshot) {
    if !snap.is_null() {
        guard((), || drop(unsafe { Box::from_raw(snap) }));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_snapshot_clear(snap: *mut Snapshot) {
    if let Some(s) = unsafe { snap.as_mut() } {
        guard((), || s.clear());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_snapshot_update(
    snap: *mut Snapshot,
    values: *const *const PbValue,
    n: usize,
    changed: *mut u64,
) -> bool {
    guard(true, || {
        let Some(s) = (unsafe { snap.as_mut() }) else {
            return true;
        };
        let values: &[*const PbValue] = if values.is_null() || n == 0 {
            &[]
        } else {
            // SAFETY: the caller passes `n` slots.
            unsafe { std::slice::from_raw_parts(values, n) }
        };
        let changed = if changed.is_null() {
            None
        } else {
            // SAFETY: the caller passes (n + 63) / 64 words.
            Some(unsafe { std::slice::from_raw_parts_mut(changed, n.div_ceil(64)) })
        };
        s.update(values, changed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Names, ViewArena};
    use petal::env::Env;
    use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
    use petal::value::Value;

    fn put(c: &mut PetalCxt) -> NativeResult {
        let v = c.get_value(1)?;
        let sym = c.intern_symbol("out");
        c.push_output(sym, v);
        c.push_nil();
        Ok(1)
    }

    /// Run `src` and decode the values it leaves in the `out` buffer.
    fn decode(env: &mut Env, src: &str) -> (ViewArena, usize, usize) {
        let src = format!("import vec2\nnative fn put(v)\n{src}");
        let pid = env.load_program(&src).unwrap_or_else(|e| panic!("{e}"));
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap_or_else(|e| panic!("{e}"));
        let out = env.intern_symbol("out");
        let vals: Vec<Value> = env.take_output_buffer(out);
        let mut arena = ViewArena::new();
        let first = arena.decode(&vals, env.heap(), &Names::NONE);
        arena.finish();
        (arena, first, vals.len())
    }

    fn node(arena: &ViewArena, i: usize) -> &PbValue {
        unsafe { &*arena.node_ptr(i) }
    }

    fn env() -> Env {
        let mut env = Env::new();
        env.set_echo(false);
        env.register_native("put", put, NativeEffects::EMITS);
        env
    }

    #[test]
    fn a_plan_reads_by_position_and_names_the_first_unknown_field() {
        let mut env = env();
        let (arena, first, n) = decode(
            &mut env,
            "for i in range(0, 3) do put({pos: i, size: 2}) end\n\
             put({size: 1, colour: 3, bogus: 4})\nput(5)",
        );
        assert_eq!(n, 5);
        let mut plan = Plan::new("shape, size, pos").unwrap();
        let mut out = [std::ptr::null(); 3];
        let shape = node(&arena, first).integer;
        assert_ne!(shape, 0);
        for i in 0..3 {
            let rec = node(&arena, first + i);
            assert_eq!(rec.integer, shape, "one literal, one shape");
            assert_eq!(plan.bind(Some(rec), &mut out), Ok(()));
            assert!(out[0].is_null());
            assert_eq!(unsafe { &*out[1] }.integer, 2);
            assert_eq!(unsafe { &*out[2] }.integer, i as i64);
        }
        assert_eq!(plan.entries.len(), 1, "planned once");
        let odd = node(&arena, first + 3);
        assert_eq!(
            plan.bind(Some(odd), &mut out),
            Err(Some(1)),
            "`colour` is the first unknown"
        );
        assert_eq!(
            unsafe { &*out[1] }.integer,
            1,
            "known options are still bound"
        );
        assert_eq!(
            plan.bind(Some(node(&arena, first + 4)), &mut out),
            Err(None)
        );
        assert_eq!(plan.bind(None, &mut out), Ok(()));
        assert!(Plan::new("a, , b").is_none());
        assert!(Plan::new("a, b, a").is_none());
        assert_eq!(Plan::new("").unwrap().len(), 0);
    }

    #[test]
    fn a_grown_shape_is_planned_again() {
        // A view with a shape id the plan knows but more fields: keys are
        // only appended under an id, so the plan must look again.
        let mut plan = Plan::new("a").unwrap();
        let key_a = c"a";
        let key_b = c"b";
        let mut fields = [
            PbValue {
                ..crate::view::NIL_VALUE.0
            },
            PbValue {
                ..crate::view::NIL_VALUE.0
            },
        ];
        fields[0].key = key_a.as_ptr();
        fields[0].key_len = 1;
        fields[1].key = key_b.as_ptr();
        fields[1].key_len = 1;
        let mut rec = PbValue {
            ..crate::view::NIL_VALUE.0
        };
        rec.kind = Kind::Map as u32;
        rec.integer = 77;
        rec.items = fields.as_ptr();
        rec.count = 1;
        let mut out = [std::ptr::null(); 1];
        assert_eq!(plan.bind(Some(&rec), &mut out), Ok(()));
        rec.count = 2;
        assert_eq!(plan.bind(Some(&rec), &mut out), Err(Some(1)));
    }

    #[test]
    fn a_snapshot_reports_exactly_the_options_that_changed() {
        let mut env = env();
        let (arena, first, _) = decode(
            &mut env,
            "let o = {shape: \"box\", size: 2.0, mat: {color: 1}}\n\
             put({...o, pos: vec3(1, 2, 3)})\n\
             put({...o, pos: vec3(1, 2, 4)})\n\
             put({shape: \"box\", size: 2.0, mat: {color: 2}, pos: vec3(1, 2, 4)})\n\
             put({shape: \"box\", mat: {color: 2}, pos: vec3(1, 2, 4)})\n\
             put({shape: \"box\", mat: {color: 2}, pos: vec3(1, 2, 4)})\n\
             put({shape: \"box\", mat: {colour: 2}, pos: vec3(1, 2, 4), f: fn(x) -> x})",
        );
        let mut plan = Plan::new("shape, size, mat, pos, f").unwrap();
        let mut snap = Snapshot::default();
        let mut out = [std::ptr::null(); 5];
        let mut step = |i: usize| {
            let _ = plan.bind(Some(node(&arena, first + i)), &mut out);
            let mut changed = [0u64];
            let any = snap.update(&out, Some(&mut changed));
            (any, changed[0])
        };
        assert_eq!(
            step(0),
            (true, 0b01111),
            "a fresh snapshot: every present option"
        );
        assert_eq!(step(1), (true, 0b01000), "only pos");
        assert_eq!(step(2), (true, 0b00100), "a nested record's contents");
        assert_eq!(step(3), (true, 0b00010), "size went absent");
        assert_eq!(step(4), (false, 0), "nothing");
        assert_eq!(
            step(5),
            (true, 0b10100),
            "a nested key; a closure is never equal"
        );
    }

    #[test]
    fn a_snapshot_tells_handles_of_different_classes_apart() {
        // Slots and serials are per class: the first object of two classes
        // can both be (0, 1).
        let handle = |class: u16| {
            let mut h = PbValue {
                ..crate::view::NIL_VALUE.0
            };
            h.kind = Kind::Handle as u32;
            h.integer = 0;
            h.count = 1;
            h.number = class as f64;
            h
        };
        let (a, b) = (handle(0), handle(1));
        let mut snap = Snapshot::default();
        let mut changed = [0u64];
        assert!(snap.update(&[&a], Some(&mut changed)));
        assert!(!snap.update(&[&a], Some(&mut changed)));
        assert!(
            snap.update(&[&b], Some(&mut changed)),
            "another class's object"
        );
        assert_eq!(changed[0], 1);
    }

    fn key(v: &PbValue) -> &str {
        std::str::from_utf8(key_of(v).unwrap()).unwrap()
    }

    #[test]
    fn records_point_at_their_shapes_cached_key_text() {
        let mut env = env();
        let (arena, first, n) = decode(
            &mut env,
            "for i in range(0, 2) do put({pos: i, name: \"n{i}\"}) end\nput({alpha: 1, beta: {pos: 2}})",
        );
        assert_eq!(n, 3);
        let (a, b, c) = (
            node(&arena, first),
            node(&arena, first + 1),
            node(&arena, first + 2),
        );
        let (fa, fb, fc) = (fields_of(a), fields_of(b), fields_of(c));
        assert_eq!((key(&fa[0]), key(&fa[1])), ("pos", "name"));
        assert_eq!(fa[0].key, fb[0].key, "one copy of a shape's keys");
        assert_eq!(unsafe { *fa[0].key.add(3) }, 0, "keys stay NUL-terminated");
        assert_eq!(fb[1].str_len, 2);
        assert_eq!((key(&fc[0]), key(&fc[1])), ("alpha", "beta"));
        assert_eq!(key(&fields_of(&fc[1])[0]), "pos");
        assert_ne!(
            fields_of(&fc[1])[0].key,
            fa[0].key,
            "another shape, its own text"
        );
    }

    #[test]
    fn the_key_cache_survives_a_reset() {
        let mut env = env();
        let pid = env
            .load_program("native fn put(v)\nput({a: 1, bb: 2})\nput({bb: 1, a: 2})")
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        let out = env.intern_symbol("out");
        let vals = env.take_output_buffer(out);
        let mut arena = ViewArena::new();
        let mut keys = Vec::new();
        for v in vals.iter().chain(vals.iter()) {
            arena.reset();
            let i = arena.decode(std::slice::from_ref(v), env.heap(), &Names::NONE);
            arena.finish();
            let f = fields_of(node(&arena, i));
            keys.push((key(&f[0]).to_string(), key(&f[1]).to_string()));
        }
        let ab = ("a".to_string(), "bb".to_string());
        let ba = ("bb".to_string(), "a".to_string());
        assert_eq!(keys, vec![ab.clone(), ba.clone(), ab, ba]);
    }

    #[test]
    fn host_built_records_share_a_shape() {
        use crate::builder::{self, HostValue};
        let mut env = env();
        let rec = |x: f64| {
            HostValue::Map(vec![
                ("pos".into(), HostValue::Vec3(x, 0.0, 0.0)),
                ("sleeping".into(), HostValue::Bool(false)),
            ])
        };
        let dup = HostValue::Map(vec![
            ("a".into(), HostValue::Int(1)),
            ("a".into(), HostValue::Int(2)),
        ]);
        let vals = [rec(1.0), rec(2.0), dup];
        let syms = builder::intern_symbols(&vals, |s| env.intern_symbol(s));
        let out = builder::materialize(&vals, env.heap_mut(), &syms);
        let shape = |env: &Env, v: Value| match v {
            Value::Map(id) => env.heap().get_map(id).shape_id(),
            _ => 0,
        };
        assert_eq!(shape(&env, out[0]), shape(&env, out[1]));
        let Value::Map(id) = out[2] else { panic!() };
        let m = env.heap().get_map(id);
        assert_eq!(
            (m.len(), m.get("a")),
            (1, Some(&Value::Int(2))),
            "a repeated key: last value wins"
        );
        // A script that adds a field gets a shape of its own; the cached
        // one keeps its keys.
        let Value::Map(id0) = out[0] else { panic!() };
        let mut grown = env.heap().get_map(id0).clone();
        grown.insert("extra", Value::Int(1));
        assert_ne!(grown.shape_id(), shape(&env, out[0]));
        let again = builder::materialize(&vals[..1], env.heap_mut(), &syms);
        assert_eq!(shape(&env, again[0]), shape(&env, out[0]));
        let Value::Map(id) = again[0] else { panic!() };
        assert_eq!(env.heap().get_map(id).len(), 2);
    }
}
