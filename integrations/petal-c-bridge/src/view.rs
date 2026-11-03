//! Decoding Petal heap values into flat, C-readable `pb_value` trees.
//!
//! A drained `Value` is only a heap id: it stays meaningful until the next run
//! mutates or collects the heap. The host therefore gets a *decoded copy* — a
//! [`ViewArena`] of `pb_value` nodes plus one string buffer — that the bridge
//! owns until the host's next run/call/clear.
//!
//! Layout: nodes are laid out breadth-first, so the roots occupy the first
//! `n` slots and every container's children are contiguous (`items[count]`).
//! While decoding, child and string positions are recorded as offsets (the
//! vectors may still reallocate); [`ViewArena::finish`] converts them to
//! pointers once, after which the arena is frozen.
//!
//! Record keys are not copied per record. A record's key list is its shape
//! (`petal::record::Shape`), shared by every record a literal makes, so the
//! arena copies a shape's keys once into a [`KeyCache`] that outlives
//! [`ViewArena::reset`], and every record of that shape points its entries'
//! `key` at the cached text. A decoded record also carries its shape id
//! (`integer` of a `PB_MAP`), which is what a host's option plan keys on
//! (`crate::plan`).

use std::collections::HashMap;
use std::ffi::c_char;
use std::ptr;

use petal::fxhash::FxBuildHasher;
use petal::heap::Heap;
use petal::record::RecordMap;
use petal::symbol::SymbolId;
use petal::value::Value;

/// Mirrors `pb_kind`.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Nil = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    Vec2 = 4,
    String = 5,
    List = 6,
    Map = 7,
    Enum = 8,
    Symbol = 9,
    Handle = 10,
    Pending = 11,
    Other = 12,
    Vec3 = 13,
    Dict = 14,
}

/// Mirrors `pb_value`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PbValue {
    pub kind: u32,
    pub count: u32,
    pub items: *const PbValue,
    pub str_: *const c_char,
    pub str_len: usize,
    pub key: *const c_char,
    pub key_len: usize,
    pub integer: i64,
    pub number: f64,
    pub y: f64,
    pub z: f64,
}

impl PbValue {
    fn scalar(kind: Kind) -> PbValue {
        PbValue {
            kind: kind as u32,
            count: 0,
            items: ptr::null(),
            str_: ptr::null(),
            str_len: 0,
            key: ptr::null(),
            key_len: 0,
            integer: 0,
            number: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

/// A statically allocated nil, handed out where a view is required but the
/// value is absent.
pub static NIL_VALUE: SyncValue = SyncValue(PbValue {
    kind: Kind::Nil as u32,
    count: 0,
    items: ptr::null(),
    str_: ptr::null(),
    str_len: 0,
    key: ptr::null(),
    key_len: 0,
    integer: 0,
    number: 0.0,
    y: 0.0,
    z: 0.0,
});

/// `PbValue` holds raw pointers; the static nil holds only nulls.
pub struct SyncValue(pub PbValue);
unsafe impl Sync for SyncValue {}

/// Name lookups the heap alone cannot answer (symbols and handle classes live
/// on the `Env`). Decoding inside a native call has neither, hence `Option`s.
pub struct Names<'a> {
    pub symbol: Option<&'a dyn Fn(SymbolId) -> Option<String>>,
    pub handle_class: Option<&'a dyn Fn(u16) -> Option<String>>,
}

impl Names<'_> {
    pub const NONE: Names<'static> = Names {
        symbol: None,
        handle_class: None,
    };
}

/// Offsets recorded during decoding, resolved to pointers by `finish`.
#[derive(Clone, Copy, Default)]
struct Pending {
    first_child: Option<u32>,
    str_off: Option<u32>,
    key: Option<KeyAt>,
}

/// Where a map entry's key text is: in the arena's string buffer (copied for
/// this record) or in the [`KeyCache`] (shared by its shape).
#[derive(Clone, Copy)]
enum KeyAt {
    Strings(u32),
    Cache(u32),
}

/// Shapes with more keys than this are not cached (a dictionary-style
/// record); their keys are copied per record, as before.
const CACHE_MAX_KEYS: usize = 64;
/// Past either bound the cache is emptied at the next `reset`, when nothing
/// points into it.
const CACHE_MAX_SHAPES: usize = 4096;
const CACHE_MAX_BYTES: usize = 1 << 20;

/// Key text of the record shapes an arena has decoded, by shape id.
///
/// A shape id names one key list for the life of the process with one
/// exception: a shape held by a single record may have keys appended without
/// a new id (`record.rs`, `Shape::push`). Keys are never changed or removed
/// under an id, so an entry is valid for a shape with as many keys as it
/// recorded; a longer shape replaces it.
#[derive(Default)]
struct KeyCache {
    /// NUL-terminated key text.
    text: Vec<u8>,
    /// `(offset into text, length)` of each cached key.
    spans: Vec<(u32, u32)>,
    /// shape id → (first span, key count).
    by_shape: HashMap<u64, (u32, u32), FxBuildHasher>,
}

impl KeyCache {
    /// The first span of `map`'s keys, caching them if needed; `None` for a
    /// shape too big to cache.
    fn spans_of(&mut self, map: &RecordMap) -> Option<u32> {
        let shape = map.shape();
        let n = shape.len();
        if n > CACHE_MAX_KEYS {
            return None;
        }
        if let Some(&(first, count)) = self.by_shape.get(&shape.id()) {
            if count as usize == n {
                return Some(first);
            }
        }
        let first = self.spans.len() as u32;
        for k in shape.keys() {
            let off = self.text.len() as u32;
            self.text.extend_from_slice(k.as_bytes());
            self.text.push(0);
            self.spans.push((off, k.len() as u32));
        }
        self.by_shape.insert(shape.id(), (first, n as u32));
        Some(first)
    }

    fn over_bounds(&self) -> bool {
        self.by_shape.len() > CACHE_MAX_SHAPES || self.text.len() > CACHE_MAX_BYTES
    }

    fn clear(&mut self) {
        self.text.clear();
        self.spans.clear();
        self.by_shape.clear();
    }
}

/// Owns a decoded value forest. See the module docs.
#[derive(Default)]
pub struct ViewArena {
    nodes: Vec<PbValue>,
    fix: Vec<Pending>,
    strings: Vec<u8>,
    /// `decode`'s breadth-first work list, kept for its capacity.
    queue: Vec<(usize, Value)>,
    /// Key text per record shape; survives `reset`.
    keys: KeyCache,
    finished: bool,
}

impl ViewArena {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty the arena for another decode, keeping its buffers' capacity.
    /// Every pointer handed out from it before is dangling afterwards.
    pub fn reset(&mut self) {
        self.nodes.clear();
        self.fix.clear();
        self.strings.clear();
        self.queue.clear();
        if self.keys.over_bounds() {
            self.keys.clear();
        }
        self.finished = false;
    }

    /// Append `s` (NUL-terminated) to the string buffer; returns its offset.
    fn add_str(&mut self, s: &str) -> u32 {
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        off
    }

    fn push(&mut self, node: PbValue, fix: Pending) -> usize {
        self.nodes.push(node);
        self.fix.push(fix);
        self.nodes.len() - 1
    }

    /// Push one node for `v` without its children. `key` is where its field
    /// name is, with the name's length, when the node is a map entry.
    fn push_shallow(
        &mut self,
        v: &Value,
        key: Option<(KeyAt, u32)>,
        heap: &Heap,
        names: &Names,
    ) -> usize {
        let mut fix = Pending::default();
        let mut key_len = 0;
        if let Some((at, len)) = key {
            fix.key = Some(at);
            key_len = len as usize;
        }
        let mut node;
        // Text the heap owns is copied straight into the string buffer;
        // `text` is for names that have to be looked up.
        let mut text: Option<String> = None;
        let mut heap_text: Option<&str> = None;
        match *v {
            Value::Nil => node = PbValue::scalar(Kind::Nil),
            Value::Bool(b) => {
                node = PbValue::scalar(Kind::Bool);
                node.integer = b as i64;
                node.number = b as i64 as f64;
            }
            Value::Int(i) => {
                node = PbValue::scalar(Kind::Int);
                node.integer = i;
                node.number = i as f64;
            }
            Value::Float(f) => {
                node = PbValue::scalar(Kind::Float);
                node.number = f;
                node.integer = f as i64;
            }
            Value::Dual { value, .. } => {
                node = PbValue::scalar(Kind::Float);
                node.number = value;
                node.integer = value as i64;
            }
            Value::Vec2(x, y) => {
                node = PbValue::scalar(Kind::Vec2);
                node.number = x;
                node.y = y;
            }
            Value::Vec3(id) => {
                node = PbValue::scalar(Kind::Vec3);
                // A stale id (collected since it was drained) decodes as zero
                // rather than panicking, like the other heap kinds.
                if heap.is_live(*v) {
                    let [x, y, z] = heap.get_vec3(id);
                    node.number = x;
                    node.y = y;
                    node.z = z;
                }
            }
            Value::String(id) => {
                node = PbValue::scalar(Kind::String);
                heap_text = Some(heap.try_get_string(id).unwrap_or(""));
            }
            Value::List(_) | Value::F64Array(_) => node = PbValue::scalar(Kind::List),
            Value::Map(id) => {
                node = PbValue::scalar(Kind::Map);
                if let Some(map) = heap.try_get_map(id) {
                    // Shape ids are never 0, so a decoded record's is nonzero.
                    node.integer = map.shape_id() as i64;
                    heap_text = heap.map_class_name(id);
                }
            }
            Value::Dict(_) => node = PbValue::scalar(Kind::Dict),
            Value::EnumVariant { tag, .. } => {
                node = PbValue::scalar(Kind::Enum);
                heap_text = Some(heap.try_get_string(tag).unwrap_or(""));
            }
            Value::Symbol(sym) => {
                node = PbValue::scalar(Kind::Symbol);
                node.integer = sym.0 as i64;
                text = names.symbol.and_then(|f| f(sym));
            }
            Value::Handle(h) => {
                node = PbValue::scalar(Kind::Handle);
                node.integer = h.slot as i64;
                node.count = h.serial;
                text = names.handle_class.and_then(|f| f(h.class.0));
            }
            Value::Pending(_) => node = PbValue::scalar(Kind::Pending),
            ref other => {
                node = PbValue::scalar(Kind::Other);
                text = Some(other.type_name().to_string());
            }
        }
        if let Some(t) = heap_text.or(text.as_deref()) {
            node.str_len = t.len();
            fix.str_off = Some(self.add_str(t));
        }
        node.key_len = key_len;
        self.push(node, fix)
    }

    /// Decode `roots` (and everything reachable from them) breadth-first.
    /// Returns the index of the first root; the roots are contiguous.
    pub fn decode(&mut self, roots: &[Value], heap: &Heap, names: &Names) -> usize {
        assert!(!self.finished, "ViewArena is frozen");
        let start = self.nodes.len();
        let mut queue = std::mem::take(&mut self.queue);
        queue.clear();
        for v in roots {
            let idx = self.push_shallow(v, None, heap, names);
            queue.push((idx, *v));
        }
        let mut qi = 0;
        while qi < queue.len() {
            let (idx, v) = queue[qi];
            qi += 1;
            let first = self.nodes.len() as u32;
            let count = match v {
                Value::List(id) => match heap.try_get_list(id) {
                    Some(items) => {
                        for it in items {
                            let c = self.push_shallow(it, None, heap, names);
                            queue.push((c, *it));
                        }
                        items.len()
                    }
                    None => 0,
                },
                Value::EnumVariant { data, .. } => match heap.try_get_list(data) {
                    Some(items) => {
                        for it in items {
                            let c = self.push_shallow(it, None, heap, names);
                            queue.push((c, *it));
                        }
                        items.len()
                    }
                    None => 0,
                },
                Value::Map(id) => match heap.try_get_map(id) {
                    Some(map) => {
                        let cached = self.keys.spans_of(map);
                        for (j, (k, it)) in map.iter().enumerate() {
                            let key = match cached {
                                Some(first) => {
                                    let (off, len) = self.keys.spans[first as usize + j];
                                    (KeyAt::Cache(off), len)
                                }
                                None => (KeyAt::Strings(self.add_str(k)), k.len() as u32),
                            };
                            let c = self.push_shallow(it, Some(key), heap, names);
                            queue.push((c, *it));
                        }
                        map.len()
                    }
                    None => 0,
                },
                // Two children per entry, key then value.
                Value::Dict(id) => match heap.try_get_dict(id) {
                    Some(dict) => {
                        for it in dict.all_values() {
                            let c = self.push_shallow(&it, None, heap, names);
                            queue.push((c, it));
                        }
                        2 * dict.len()
                    }
                    None => 0,
                },
                Value::F64Array(id) => {
                    let floats = heap.get_f64_array(id);
                    for f in floats {
                        let mut n = PbValue::scalar(Kind::Float);
                        n.number = *f;
                        n.integer = *f as i64;
                        self.push(n, Pending::default());
                    }
                    floats.len()
                }
                _ => continue,
            };
            self.nodes[idx].count = count as u32;
            if count > 0 {
                self.fix[idx].first_child = Some(first);
            }
        }
        self.queue = queue;
        start
    }

    /// Resolve every recorded offset into a pointer and freeze the arena.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let nodes_base = self.nodes.as_ptr();
        let str_base = self.strings.as_ptr() as *const c_char;
        let key_base = self.keys.text.as_ptr() as *const c_char;
        for (node, fix) in self.nodes.iter_mut().zip(&self.fix) {
            if let Some(first) = fix.first_child {
                // SAFETY: `first` indexes into `nodes`, which no longer grows.
                node.items = unsafe { nodes_base.add(first as usize) };
            }
            if let Some(off) = fix.str_off {
                node.str_ = unsafe { str_base.add(off as usize) };
            }
            // SAFETY (both): offsets into buffers that no longer grow; the
            // key cache only grows during a decode, before `finish`.
            match fix.key {
                Some(KeyAt::Strings(off)) => node.key = unsafe { str_base.add(off as usize) },
                Some(KeyAt::Cache(off)) => node.key = unsafe { key_base.add(off as usize) },
                None => {}
            }
        }
        self.fix.clear();
    }

    /// Pointer to node `index` (valid once finished).
    pub fn node_ptr(&self, index: usize) -> *const PbValue {
        debug_assert!(self.finished);
        if index < self.nodes.len() {
            // SAFETY: in bounds.
            unsafe { self.nodes.as_ptr().add(index) }
        } else {
            &NIL_VALUE.0
        }
    }
}
