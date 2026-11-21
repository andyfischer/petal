//! Value - Runtime representation of data.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::fmt;

use crate::handle::HandleVal;
use crate::heap::{
    CellId, DictId, ElementId, EnumTag, F64ArrayId, ListId, MapId, StringId, Vec2Id, Vec3Id,
};
use crate::native_fn::NativeFnId;
use crate::program::{ClosureId, OverloadSetId};
use crate::symbol::SymbolId;

/// Runtime value. All variants are Copy — heap-allocated data is referenced by ID.
#[derive(Clone, Copy, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    /// Flat, unboxed contiguous array of f64 values.
    F64Array(F64ArrayId),
    /// A record (`{a: 1}`): string-keyed fields. `type()` is `"record"`.
    Map(MapId),
    /// A `map` (`Map()`): entries keyed by any hashable value, compared by
    /// content, in insertion order. `type()` is `"map"`. See `crate::dict`.
    Dict(DictId),
    Closure(ClosureId),
    /// Multi-arity function: dispatches to the right closure based on arg count.
    OverloadSet(OverloadSetId),
    NativeFunction(NativeFnId),
    /// A value of a declared `enum`: the variant's tag (see
    /// [`EnumTag`](crate::heap::EnumTag)) and its payload list.
    EnumVariant {
        tag: EnumTag,
        data: ListId,
    },
    Element(ElementId),
    /// A mutable one-value box behind a `var` binding. Never escapes into user
    /// code: every source-level read of a `var` dereferences it (`CellRead`),
    /// so no expression evaluates to a `Cell`. The only way two holders share
    /// one is closure capture, which is lexically visible. See the containment
    /// invariant in docs/syntax/var.md (Containment).
    Cell(CellId),
    /// 2D vector for creative coding (positions, velocities, forces). Its
    /// components are on the heap ([`Heap::get_vec2`](crate::heap::Heap::get_vec2)),
    /// immutable, so the id behaves like a value.
    Vec2(Vec2Id),
    /// 3D vector (positions, directions, colors in 3D code). Heap-allocated
    /// like `Vec2`; the components are immutable, so the id behaves like a
    /// value. See docs/dev/vec3.md.
    Vec3(Vec3Id),
    /// An interned symbol — a binding key shared with the embedding host.
    /// See `crate::symbol`.
    Symbol(SymbolId),
    /// An opaque reference to a host-owned foreign object. See `crate::handle`.
    Handle(HandleVal),
}

// A tag byte plus at most 12 bytes of 4-aligned payload, or 8 of 8-aligned:
// heap ids are 8 bytes (index + generation), the widest payloads are
// `EnumVariant` (a 4-byte tag and a payload id) and `Handle`, 12 bytes each.
// Keep it that way: every register, list slot, record field, map entry and
// closure capture is a `Value`. It is why `Vec2` and `Vec3` are heap
// ids rather than inline f64s. See docs/proposals/value-representation.md.
const _: () = assert!(std::mem::size_of::<Value>() == 16);
const _: () = assert!(std::mem::size_of::<Option<Value>>() == 16);

impl Value {
    /// Truthiness, when it can be told without the heap: `None` for a vec2,
    /// whose components are on the heap (a zero one is false). [`Heap::truthy`](crate::heap::Heap::truthy) answers for every
    /// value; this is its fast path.
    #[inline]
    pub fn truthy_without_heap(&self) -> Option<bool> {
        Some(match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Vec2(_) => return None,
            // Always truthy, like a list or record: a vec3 never had its
            // components inline. Test a zero vector explicitly
            // (`v == vec3(0, 0, 0)`).
            Value::Vec3(_) => true,
            _ => true,
        })
    }

    /// Whether a value is "present" for the `??` coalescing operator: anything
    /// other than `Nil`. Distinct from [`is_truthy`](Value::is_truthy) — `0`,
    /// `false`, and `""` are present.
    pub fn is_present(&self) -> bool {
        !matches!(self, Value::Nil)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::F64Array(_) => "f64_array",
            Value::Map(_) => "record",
            Value::Dict(_) => "map",
            Value::Closure(_) => "function",
            Value::OverloadSet(_) => "function",
            Value::NativeFunction(_) => "function",
            Value::EnumVariant { .. } => "enum",
            Value::Element(_) => "element",
            Value::Cell(_) => "cell",
            Value::Vec2(_) => "vec2",
            Value::Vec3(_) => "vec3",
            Value::Symbol(_) => "symbol",
            Value::Handle(_) => "handle",
        }
    }
}

fn format_float(f: f64) -> String {
    if f == f.floor() && f.abs() < 1e15 {
        format!("{:.1}", f)
    } else {
        format!("{}", f)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "Nil"),
            Value::Bool(b) => write!(f, "Bool({b})"),
            Value::Int(n) => write!(f, "Int({n})"),
            Value::Float(v) => write!(f, "Float({v})"),
            Value::String(id) => write!(f, "String({:?})", id),
            Value::List(id) => write!(f, "List({:?})", id),
            Value::F64Array(id) => write!(f, "F64Array({})", id.index()),
            Value::Map(id) => write!(f, "Map({:?})", id),
            Value::Dict(id) => write!(f, "Dict({:?})", id),
            Value::Closure(id) => write!(f, "Closure({:?})", id),
            Value::OverloadSet(id) => write!(f, "OverloadSet({:?})", id),
            Value::NativeFunction(id) => write!(f, "NativeFunction({:?})", id),
            Value::EnumVariant { tag, data } => {
                write!(f, "EnumVariant({:?}, {:?})", tag, data)
            }
            Value::Element(id) => write!(f, "Element({:?})", id),
            Value::Cell(id) => write!(f, "Cell({:?})", id),
            // The components are on the heap, which `Debug` has no access to.
            Value::Vec2(id) => write!(f, "Vec2({:?})", id),
            Value::Vec3(id) => write!(f, "Vec3({:?})", id),
            Value::Symbol(id) => write!(f, "Symbol({})", id.0),
            Value::Handle(h) => write!(f, "{}", h),
        }
    }
}

/// Display helpers that need heap access. These are standalone functions
/// rather than methods because they need &Heap.
use crate::heap::Heap;
use crate::numeric::{self, Num};

pub fn value_to_display_string(val: &Value, heap: &Heap) -> String {
    match val {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format_float(*f),
        Value::String(id) => heap.get_string(*id).to_string(),
        Value::List(id) => {
            let elems = heap.get_list(*id);
            let parts: Vec<String> = elems
                .iter()
                .map(|v| value_to_debug_string(v, heap))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::F64Array(id) => {
            let data = heap.get_f64_array(*id);
            let parts: Vec<String> = data.iter().map(|f| format_float(*f)).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Map(id) => {
            let map = heap.get_map(*id);
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", display_key(k), value_to_debug_string(v, heap)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        // As the call that rebuilds it: `Map([[1, "one"], ["k", true]])`.
        Value::Dict(id) => {
            let d = heap.get_dict(*id);
            if d.is_empty() {
                return "Map()".to_string();
            }
            let parts: Vec<String> = d
                .iter()
                .map(|(k, v)| {
                    format!(
                        "[{}, {}]",
                        value_to_debug_string(&k, heap),
                        value_to_debug_string(&v, heap)
                    )
                })
                .collect();
            format!("Map([{}])", parts.join(", "))
        }
        Value::Element(id) => element_to_display_string(*id, heap),
        // Unreachable under the containment invariant (§6d): a `var` read
        // dereferences, so no cell reaches a display path. Printed rather than
        // panicked so a hypothetical leak shows up as a visible wart in output
        // instead of taking down the run.
        Value::Cell(id) => format!("<cell {}#{}>", id.index(), id.generation()),
        Value::Closure(_) => "<function>".to_string(),
        Value::OverloadSet(_) => "<function>".to_string(),
        Value::NativeFunction(_) => "<native>".to_string(),
        Value::EnumVariant { tag, data } => {
            // `Phase.play`, whichever module declared `Phase`: the module
            // part of the tag (`./level::`) is identity, not display.
            let name = crate::program::strip_module(heap.tag_str(*tag));
            let fields = heap.get_list(*data);
            if fields.is_empty() {
                name.to_string()
            } else {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|v| value_to_debug_string(v, heap))
                    .collect();
                format!("{}({})", name, parts.join(", "))
            }
        }
        Value::Vec2(id) => {
            let (x, y) = heap.get_vec2(*id);
            format!("vec2({}, {})", format_float(x), format_float(y))
        }
        Value::Vec3(id) => {
            let [x, y, z] = heap.get_vec3(*id);
            format!(
                "vec3({}, {}, {})",
                format_float(x),
                format_float(y),
                format_float(z)
            )
        }
        Value::Symbol(id) => format!("symbol#{}", id.0),
        Value::Handle(h) => h.to_string(),
    }
}

/// A record key as printed: bare when it is an identifier (and not a
/// keyword), else quoted and escaped like a string literal, so
/// `{[k]: 1}` with `k = "X-Req"` prints `{ "X-Req": 1 }` rather than the
/// unreadable `{ X-Req: 1 }`.
pub fn display_key(key: &str) -> std::borrow::Cow<'_, str> {
    if crate::static_value::is_identifier(key) {
        std::borrow::Cow::Borrowed(key)
    } else {
        std::borrow::Cow::Owned(crate::static_value::render_string_literal(key))
    }
}

pub fn value_to_debug_string(val: &Value, heap: &Heap) -> String {
    match val {
        Value::String(id) => format!("\"{}\"", heap.get_string(*id)),
        other => value_to_display_string(other, heap),
    }
}

fn element_to_display_string(id: crate::heap::ElementId, heap: &Heap) -> String {
    let tag_id = heap.get_element_tag(id);
    let tag = heap.get_string(tag_id);
    let props_id = heap.get_element_props(id);
    let children_id = heap.get_element_children(id);
    let props = heap.get_map(props_id);
    let children = heap.get_list(children_id);

    let mut s = format!("<{}", tag);
    // As written in source: a string in quotes, anything else in braces.
    for (k, v) in props {
        s.push(' ');
        s.push_str(k);
        let shown = value_to_display_string(v, heap);
        match v {
            Value::String(_) => s.push_str(&format!("=\"{shown}\"")),
            _ => s.push_str(&format!("={{{shown}}}")),
        }
    }

    if children.is_empty() {
        s.push_str(" />");
    } else {
        s.push('>');
        for child in children {
            s.push_str(&value_to_display_string(child, heap));
        }
        s.push_str(&format!("</{}>", tag));
    }
    s
}

/// Convert a Value to serde_json::Value — for data that leaves the process:
/// a saved state, a file, a host's output.
///
/// Nil→null, Bool→bool, Int/Float→number, String→string, List→array
/// (recursive), Map→object (recursive), others→string via display. Every
/// value converts; the `Result` is kept so a caller that handles a failure
/// (`.ok()`, `.map_err(..)`) needs no change.
pub fn value_to_json(val: &Value, heap: &Heap) -> Result<serde_json::Value, std::convert::Infallible> {
    Ok(to_json(val, heap))
}

/// [`value_to_json`] without the `Result`, for debug dumps.
pub fn value_to_json_lossy(val: &Value, heap: &Heap) -> serde_json::Value {
    to_json(val, heap)
}

fn to_json(val: &Value, heap: &Heap) -> serde_json::Value {
    match val {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::json!(*n),
        Value::Float(f) => serde_json::json!(*f),
        Value::String(id) => serde_json::Value::String(heap.get_string(*id).to_string()),
        Value::List(id) => {
            let mut arr = Vec::new();
            for v in heap.get_list(*id).iter() {
                arr.push(to_json(v, heap));
            }
            serde_json::Value::Array(arr)
        }
        Value::F64Array(id) => {
            let data = heap.get_f64_array(*id);
            let arr: Vec<serde_json::Value> = data.iter().map(|f| serde_json::json!(*f)).collect();
            serde_json::Value::Array(arr)
        }
        Value::Map(id) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in heap.get_map(*id).iter() {
                let json = to_json(v, heap);
                obj.insert(k.to_string(), json);
            }
            serde_json::Value::Object(obj)
        }
        // Keys are arbitrary values, so not object keys: a tagged list of
        // `[key, value]` pairs, in order. `json_to_value` reads it back.
        Value::Dict(id) => {
            let mut entries = Vec::new();
            for (k, v) in heap.get_dict(*id).iter() {
                let key = to_json(&k, heap);
                let value = to_json(&v, heap);
                entries.push(serde_json::Value::Array(vec![key, value]));
            }
            serde_json::json!({ "type": "map", "entries": entries })
        }
        Value::Vec2(id) => {
            let (x, y) = heap.get_vec2(*id);
            serde_json::json!({ "type": "vec2", "x": x, "y": y })
        }
        Value::Vec3(id) => {
            let [x, y, z] = heap.get_vec3(*id);
            serde_json::json!({ "type": "vec3", "x": x, "y": y, "z": z })
        }
        Value::EnumVariant { tag, data } => {
            let name = heap.tag_str(*tag).to_string();
            let mut arr = Vec::new();
            for v in heap.get_list(*data).iter() {
                arr.push(to_json(v, heap));
            }
            serde_json::json!({ "type": "enum", "tag": name, "data": arr })
        }
        Value::Element(id) => element_to_json(*id, heap),
        // A `state var`'s slot holds the cell, so this is the one place a cell
        // reaches a serialization surface. Report the *contents*: the box is an
        // implementation detail, and someone inspecting `hits` wants `3`.
        Value::Cell(id) => to_json(&heap.cell_read(*id), heap),
        Value::Symbol(id) => serde_json::json!({ "type": "symbol", "id": id.0 }),
        // Closures, native functions → string representation
        other => serde_json::Value::String(value_to_display_string(other, heap)),
    }
}

fn element_to_json(id: crate::heap::ElementId, heap: &Heap) -> serde_json::Value {
    let tag_id = heap.get_element_tag(id);
    let tag = heap.get_string(tag_id).to_string();
    let props = heap.get_map(heap.get_element_props(id));
    let children = heap.get_list(heap.get_element_children(id));

    let mut props_obj = serde_json::Map::new();
    for (k, v) in props.iter() {
        let json = to_json(v, heap);
        props_obj.insert(k.to_string(), json);
    }
    let mut children_arr = Vec::new();
    for child in children.iter() {
        children_arr
            .push(to_json(child, heap));
    }

    serde_json::json!({
        "type": "element",
        "tag": tag,
        "props": props_obj,
        "children": children_arr
    })
}

/// Convert a JSON value to a Petal Value.
/// Supports null, bool, number (int/float), string, array and object — plus
/// the tagged objects [`value_to_json`] writes for the types JSON has no shape
/// for: vectors (`{"type": "vec2", "x", "y"}` and `{"type": "vec3", "x", "y",
/// "z"}`) and maps (`{"type": "map", "entries": [[k, v], ...]}`). Those come
/// back as what they were, so they survive a JSON round trip
/// (`json_parse(json_stringify(v))`, a state dump and restore).
pub fn json_to_value(json: &serde_json::Value, heap: &mut Heap) -> Result<Value, String> {
    if let Some(v) = json_to_vector(json, heap) {
        return Ok(v);
    }
    if let Some(pairs) = json_map_entries(json) {
        let mut kvs = Vec::with_capacity(pairs.len());
        for pair in pairs {
            kvs.push((json_to_value(&pair[0], heap)?, json_to_value(&pair[1], heap)?));
        }
        return Ok(Value::Dict(heap.dict_from_pairs(kvs)?));
    }
    match json {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err("Invalid number".to_string())
            }
        }
        serde_json::Value::String(s) => {
            let id = heap.alloc_string(s.clone());
            Ok(Value::String(id))
        }
        serde_json::Value::Array(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for item in items {
                elems.push(json_to_value(item, heap)?);
            }
            let id = heap.alloc_list(elems);
            Ok(Value::List(id))
        }
        serde_json::Value::Object(obj) => {
            let mut entries = crate::heap::record_map_with_capacity(obj.len());
            for (key, item) in obj {
                entries.insert(key.clone(), json_to_value(item, heap)?);
            }
            let id = heap.alloc_map(entries);
            Ok(Value::Map(id))
        }
    }
}

/// The vector a tagged JSON object denotes, if it is exactly one of the shapes
/// [`value_to_json`] writes for `vec2` / `vec3`: a `"type"` tag plus one
/// numeric field per component and nothing else. Anything looser stays a
/// record, so an ordinary `{type: "vec3", ...}` record with extra fields is
/// never reinterpreted.
fn json_to_vector(json: &serde_json::Value, heap: &mut Heap) -> Option<Value> {
    let obj = json.as_object()?;
    let num = |k: &str| obj.get(k).and_then(serde_json::Value::as_f64);
    match obj.get("type")?.as_str()? {
        "vec2" if obj.len() == 3 => {
            let (x, y) = (num("x")?, num("y")?);
            Some(heap.vec2_value(x, y))
        }
        "vec3" if obj.len() == 4 => {
            let (x, y, z) = (num("x")?, num("y")?, num("z")?);
            Some(heap.vec3_value(x, y, z))
        }
        _ => None,
    }
}

/// The `[key, value]` pairs of a tagged map object, if `json` is exactly the
/// shape [`value_to_json`] writes for a map: `"type": "map"` and an
/// `"entries"` list of two-element lists, and no other field. Anything looser
/// stays a record.
fn json_map_entries(json: &serde_json::Value) -> Option<Vec<&[serde_json::Value]>> {
    let obj = json.as_object()?;
    if obj.len() != 2 || obj.get("type")?.as_str()? != "map" {
        return None;
    }
    obj.get("entries")?
        .as_array()?
        .iter()
        .map(|e| e.as_array().filter(|p| p.len() == 2).map(Vec::as_slice))
        .collect()
}

/// The numeric view of a value that `==` and `<` compare by: ints and floats.
/// `None` for everything else.
pub fn as_num(v: &Value, _heap: &Heap) -> Option<Num> {
    match *v {
        Value::Int(n) => Some(Num::Int(n)),
        Value::Float(f) => Some(Num::Float(f)),
        _ => None,
    }
}

/// Hash a value to a u64 for use as a key: a `memo(key)`, a `for … key`
/// loop iteration, a `Map` entry.
///
/// Consistent with [`values_equal`]: values that are `==` hash equally, so
/// `memo(key)` finds the same slot for every key that is `==` — including a
/// record rebuilt each frame, and `2` vs `2.0`. Containers hash their content,
/// never their heap id (ids differ between two equal values).
pub fn hash_value(val: &Value, heap: &Heap) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_into(val, heap, &mut hasher);
    hasher.finish()
}

/// A number, a string or a bool, as [`hash_value`] hashes the Petal value
/// of it: what a host's `StatePart::key` builds with no heap at hand.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarKey<'a> {
    Int(i64),
    Str(&'a str),
    Bool(bool),
}

/// [`hash_value`] of the value `key` stands for, computed without a heap.
pub fn hash_scalar_key(key: ScalarKey<'_>) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match key {
        ScalarKey::Int(i) => hash_num_into(Num::Int(i), &mut h),
        ScalarKey::Str(s) => hash_str_into(s, &mut h),
        ScalarKey::Bool(b) => hash_bool_into(b, &mut h),
    }
    h.finish()
}

fn hash_num_into(n: Num, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    2u8.hash(h);
    numeric::num_key(n).hash(h);
}

fn hash_str_into(s: &str, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    4u8.hash(h);
    s.hash(h);
}

fn hash_bool_into(b: bool, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    1u8.hash(h);
    b.hash(h);
}

fn hash_into(val: &Value, heap: &Heap, h: &mut impl std::hash::Hasher) {
    use std::hash::{Hash, Hasher};
    if let Some(n) = as_num(val, heap) {
        hash_num_into(n, h);
        return;
    }
    match val {
        Value::Nil => 0u8.hash(h),
        Value::Bool(b) => hash_bool_into(*b, h),
        Value::String(id) => hash_str_into(heap.get_string(*id), h),
        Value::List(id) => {
            5u8.hash(h);
            let elems = heap.get_list(*id);
            elems.len().hash(h);
            for elem in elems {
                hash_into(elem, heap, h);
            }
        }
        Value::Vec2(id) => {
            7u8.hash(h);
            let (x, y) = heap.get_vec2(*id);
            numeric::num_key(Num::Float(x)).hash(h);
            numeric::num_key(Num::Float(y)).hash(h);
        }
        Value::Vec3(id) => {
            13u8.hash(h);
            for f in heap.get_vec3(*id) {
                numeric::num_key(Num::Float(f)).hash(h);
            }
        }
        Value::F64Array(id) => {
            8u8.hash(h);
            for f in heap.get_f64_array(*id) {
                numeric::num_key(Num::Float(*f)).hash(h);
            }
        }
        Value::Handle(hv) => {
            9u8.hash(h);
            hv.class.0.hash(h);
            hv.slot.hash(h);
            hv.serial.hash(h);
        }
        Value::Map(id) => {
            10u8.hash(h);
            heap.map_class_name(*id).hash(h);
            // Record equality ignores key order, so the hash must too: combine
            // per-entry hashes with a commutative sum.
            let map = heap.get_map(*id);
            map.len().hash(h);
            let mut sum = 0u64;
            for (k, v) in map {
                let mut eh = std::collections::hash_map::DefaultHasher::new();
                k.hash(&mut eh);
                hash_into(v, heap, &mut eh);
                sum = sum.wrapping_add(eh.finish());
            }
            sum.hash(h);
        }
        Value::Dict(id) => {
            14u8.hash(h);
            // Map equality ignores insertion order, like a record's.
            let d = heap.get_dict(*id);
            d.len().hash(h);
            let mut sum = 0u64;
            for (k, v) in d.iter() {
                let mut eh = std::collections::hash_map::DefaultHasher::new();
                hash_into(&k, heap, &mut eh);
                hash_into(&v, heap, &mut eh);
                sum = sum.wrapping_add(eh.finish());
            }
            sum.hash(h);
        }
        Value::EnumVariant { tag, data } => {
            11u8.hash(h);
            heap.tag_str(*tag).hash(h);
            hash_into(&Value::List(*data), heap, h);
        }
        Value::Element(id) => {
            12u8.hash(h);
            heap.get_string(heap.get_element_tag(*id)).hash(h);
            hash_into(&Value::Map(heap.get_element_props(*id)), heap, h);
            hash_into(&Value::List(heap.get_element_children(*id)), heap, h);
        }
        // Identity-compared values (functions, symbols, cells, handles): the
        // debug form is the id, which is exactly what equality compares.
        other => {
            6u8.hash(h);
            format!("{:?}", other).hash(h);
        }
    }
}

/// Compare two values for equality. Needs heap access for deep comparison
/// of lists and maps.
///
/// This is `==`. It is symmetric and transitive, and reflexive on everything
/// but NaN (checked by the `proofs` harnesses for numbers and by the
/// exhaustive tests in this module for containers):
/// - numbers compare exactly across Int and Float (no rounding to f64);
/// - lists, enum variants and elements compare element-wise; records compare
///   by class and by key set and values, ignoring key order; maps compare by
///   key set and values, ignoring insertion order;
/// - functions, symbols, cells and handles compare by identity.
pub fn values_equal(a: &Value, b: &Value, heap: &Heap) -> bool {
    if let (Some(x), Some(y)) = (as_num(a, heap), as_num(b, heap)) {
        return numeric::num_eq(x, y);
    }
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::String(a), Value::String(b)) => {
            // With string interning, equal content means equal IDs
            a == b || heap.get_string(*a) == heap.get_string(*b)
        }
        (Value::EnumVariant { tag: at, data: ad }, Value::EnumVariant { tag: bt, data: bd }) => {
            (at == bt || heap.tag_str(*at) == heap.tag_str(*bt))
                && values_equal(&Value::List(*ad), &Value::List(*bd), heap)
        }
        (Value::List(a), Value::List(b)) => {
            let a_elems = heap.get_list(*a);
            let b_elems = heap.get_list(*b);
            a_elems.len() == b_elems.len()
                && a_elems
                    .iter()
                    .zip(b_elems.iter())
                    .all(|(a, b)| values_equal(a, b, heap))
        }
        (Value::Map(a), Value::Map(b)) => {
            if heap.map_class_name(*a) != heap.map_class_name(*b) {
                return false;
            }
            let (xs, ys) = (heap.get_map(*a), heap.get_map(*b));
            // Same shape: the keys match position by position.
            if std::sync::Arc::ptr_eq(xs.shape(), ys.shape()) {
                return xs.values().zip(ys.values()).all(|(x, y)| values_equal(x, y, heap));
            }
            xs.len() == ys.len()
                && xs.iter().all(|(k, x)| match ys.get(k) {
                    Some(y) => values_equal(x, y, heap),
                    None => false,
                })
        }
        (Value::Dict(a), Value::Dict(b)) => {
            if a == b {
                return true;
            }
            let (xs, ys) = (heap.get_dict(*a), heap.get_dict(*b));
            // Every key of a map is hashable, so looking one up cannot fail.
            xs.len() == ys.len()
                && xs.iter().all(|(k, x)| match heap.dict_get(*b, k) {
                    Ok(Some(y)) => values_equal(&x, &y, heap),
                    _ => false,
                })
        }
        (Value::F64Array(a), Value::F64Array(b)) => {
            let a_data = heap.get_f64_array(*a);
            let b_data = heap.get_f64_array(*b);
            a_data == b_data
        }
        (Value::NativeFunction(a), Value::NativeFunction(b)) => a == b,
        (Value::Closure(a), Value::Closure(b)) => a == b,
        (Value::OverloadSet(a), Value::OverloadSet(b)) => a == b,
        (Value::Cell(a), Value::Cell(b)) => a == b,
        (Value::Symbol(a), Value::Symbol(b)) => a == b,
        // Componentwise, with no same-id shortcut: a vec2 holding a NaN is
        // not `==` to itself, as when it was inline.
        (Value::Vec2(a), Value::Vec2(b)) => heap.get_vec2(*a) == heap.get_vec2(*b),
        (Value::Vec3(a), Value::Vec3(b)) => a == b || heap.get_vec3(*a) == heap.get_vec3(*b),
        (Value::Handle(a), Value::Handle(b)) => a == b,
        (Value::Element(a), Value::Element(b)) => {
            let a_tag = heap.get_string(heap.get_element_tag(*a));
            let b_tag = heap.get_string(heap.get_element_tag(*b));
            a_tag == b_tag
                && values_equal(
                    &Value::Map(heap.get_element_props(*a)),
                    &Value::Map(heap.get_element_props(*b)),
                    heap,
                )
                && values_equal(
                    &Value::List(heap.get_element_children(*a)),
                    &Value::List(heap.get_element_children(*b)),
                    heap,
                )
        }
        _ => false,
    }
}

/// Order two Values (used by the `<`/`<=`/`>`/`>=` operators and the
/// `min`/`max` builtins). Numeric kinds compare exactly by value; strings compare lexically; mismatched non-numeric kinds
/// are an error. `Ok(None)` when the two are unordered — a NaN is involved —
/// so every ordering operator on it is false, as in IEEE 754.
pub fn compare_values_partial(
    a: &Value,
    b: &Value,
    heap: &Heap,
) -> Result<Option<std::cmp::Ordering>, String> {
    if let (Some(x), Some(y)) = (as_num(a, heap), as_num(b, heap)) {
        return Ok(numeric::num_cmp(x, y));
    }
    match (a, b) {
        (Value::String(a), Value::String(b)) => {
            Ok(Some(heap.get_string(*a).cmp(heap.get_string(*b))))
        }
        _ => Err(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

/// [`compare_values_partial`] with unordered (NaN) pairs reported as `Equal`,
/// for callers that must pick one side (`min`/`max` keep the first argument).
pub fn compare_values(a: &Value, b: &Value, heap: &Heap) -> Result<std::cmp::Ordering, String> {
    Ok(compare_values_partial(a, b, heap)?.unwrap_or(std::cmp::Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn nil_is_falsy() {
        assert!(!Value::Nil.truthy_without_heap().unwrap());
    }

    #[test]
    fn false_is_falsy() {
        assert!(!Value::Bool(false).truthy_without_heap().unwrap());
    }

    #[test]
    fn true_is_truthy() {
        assert!(Value::Bool(true).truthy_without_heap().unwrap());
    }

    #[test]
    fn zero_int_is_falsy() {
        assert!(!Value::Int(0).truthy_without_heap().unwrap());
    }

    #[test]
    fn nonzero_int_is_truthy() {
        assert!(Value::Int(42).truthy_without_heap().unwrap());
    }

    #[test]
    fn zero_float_is_falsy() {
        assert!(!Value::Float(0.0).truthy_without_heap().unwrap());
    }

    #[test]
    fn nonzero_float_is_truthy() {
        assert!(Value::Float(3.25).truthy_without_heap().unwrap());
    }

    #[test]
    fn type_names() {
        assert_eq!(Value::Nil.type_name(), "nil");
        assert_eq!(Value::Bool(true).type_name(), "bool");
        assert_eq!(Value::Int(1).type_name(), "int");
        assert_eq!(Value::Float(1.0).type_name(), "float");
    }

    #[test]
    fn format_float_whole_numbers() {
        assert_eq!(format_float(5.0), "5.0");
        assert_eq!(format_float(0.0), "0.0");
    }

    #[test]
    fn format_float_fractional() {
        assert_eq!(format_float(3.25), "3.25");
    }

    // ── Exhaustive laws of `==`, hashing and ordering ──────────────
    //
    // Small-scope exhaustive check: every pair and triple drawn from a universe
    // of values up to nesting depth 2 — numeric edge cases (±0, NaN, 2^53 + 1
    // vs 2^53 as float), strings allocated twice, and every container
    // kind built from them. The numeric kernels are proven for all inputs by
    // the Kani harnesses in `crate::proofs`; this covers how containers
    // compose them. See docs/dev/formal-verification.md.

    fn universe(heap: &mut Heap) -> Vec<Value> {
        let mut atoms = vec![
            Value::Nil,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(0),
            Value::Int(1),
            Value::Int(9007199254740993),
            Value::Float(0.0),
            Value::Float(-0.0),
            Value::Float(1.0),
            Value::Float(9007199254740992.0),
            Value::Float(f64::NAN),
            heap.vec2_value(0.0, 1.0),
            heap.vec2_value(-0.0, 1.0),
        ];
        // Equal strings with distinct ids, and a different one.
        for s in ["a", "a", "b"] {
            atoms.push(Value::String(heap.alloc_string(s.to_string())));
        }
        let mut all = atoms.clone();
        let small: Vec<Value> = atoms.iter().copied().step_by(2).collect();
        let key = heap.intern_str("Some");
        let class = heap.intern_str("Point");
        for &x in &small {
            all.push(Value::List(heap.alloc_list(vec![x])));
            all.push(Value::List(heap.alloc_list(vec![x, Value::Int(1)])));
            let mut m = crate::heap::RecordMap::default();
            m.insert("a".to_string(), x);
            all.push(Value::Map(heap.alloc_map(m.clone())));
            all.push(Value::Map(heap.alloc_class_instance(m, class)));
            let data = heap.alloc_list(vec![x]);
            all.push(heap.enum_variant(key, data));
            // A map holding `x` as a value, and (unless it is NaN, which no
            // map may hold as a key) as a key.
            let one = Value::Int(1);
            all.push(Value::Dict(heap.dict_from_pairs([(one, x)]).unwrap()));
            if let Ok(d) = heap.dict_from_pairs([(x, one)]) {
                all.push(Value::Dict(d));
            }
        }
        // A map's same entries, both insertion orders.
        for pairs in [[(1, 2.0), (3, 4.0)], [(3, 4.0), (1, 2.0)]] {
            let pairs = pairs.map(|(k, v)| (Value::Int(k), Value::Float(v)));
            all.push(Value::Dict(heap.dict_from_pairs(pairs).unwrap()));
        }
        all.push(Value::List(heap.alloc_list(vec![])));
        all.push(Value::List(heap.alloc_list(vec![])));
        // Same entries, both key orders.
        for (k1, k2) in [("a", "b"), ("b", "a")] {
            let mut m = crate::heap::RecordMap::default();
            m.insert(k1.to_string(), Value::Int(1));
            m.insert(k2.to_string(), Value::Float(2.0));
            all.push(Value::Map(heap.alloc_map(m)));
        }
        // Depth 2: a list of each depth-1 container.
        let depth1: Vec<Value> = all[atoms.len()..].to_vec();
        for &c in depth1.iter().step_by(3) {
            all.push(Value::List(heap.alloc_list(vec![c])));
        }
        all
    }

    fn contains_nan(v: &Value, heap: &Heap) -> bool {
        match *v {
            Value::Float(f) => f.is_nan(),
            Value::List(id) => heap.get_list(id).iter().any(|x| contains_nan(x, heap)),
            Value::Map(id) => heap.get_map(id).values().any(|x| contains_nan(x, heap)),
            Value::Dict(id) => heap.get_dict(id).all_values().any(|x| contains_nan(&x, heap)),
            Value::EnumVariant { data, .. } => contains_nan(&Value::List(data), heap),
            _ => false,
        }
    }

    #[test]
    fn equality_is_a_partial_equivalence_consistent_with_hash() {
        let mut heap = Heap::new();
        let u = universe(&mut heap);
        let eq = |a: &Value, b: &Value| values_equal(a, b, &heap);
        for a in &u {
            assert!(
                eq(a, a) || contains_nan(a, &heap),
                "== is not reflexive on {}",
                value_to_display_string(a, &heap)
            );
            for b in &u {
                let ab = eq(a, b);
                assert_eq!(ab, eq(b, a), "== is not symmetric");
                if ab {
                    assert_eq!(
                        hash_value(a, &heap),
                        hash_value(b, &heap),
                        "{} == {} but they hash differently",
                        value_to_display_string(a, &heap),
                        value_to_display_string(b, &heap),
                    );
                }
                if !ab {
                    continue;
                }
                for c in &u {
                    if eq(b, c) {
                        assert!(
                            eq(a, c),
                            "== is not transitive: {} == {} == {}",
                            value_to_display_string(a, &heap),
                            value_to_display_string(b, &heap),
                            value_to_display_string(c, &heap),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ordering_is_antisymmetric_and_agrees_with_equality() {
        let mut heap = Heap::new();
        let u = universe(&mut heap);
        for a in &u {
            for b in &u {
                let ab = compare_values_partial(a, b, &heap);
                let ba = compare_values_partial(b, a, &heap);
                match (ab, ba) {
                    (Ok(x), Ok(y)) => {
                        assert_eq!(x, y.map(std::cmp::Ordering::reverse));
                        if let Some(o) = x {
                            assert_eq!(
                                o == std::cmp::Ordering::Equal,
                                values_equal(a, b, &heap),
                                "ordering says Equal but == disagrees"
                            );
                        }
                    }
                    (Err(_), Err(_)) => {}
                    _ => panic!("compare_values_partial errs on one side only"),
                }
            }
        }
    }

    #[test]
    fn display_key_quotes_only_non_identifiers() {
        assert_eq!(display_key("name"), "name");
        assert_eq!(display_key("_x1"), "_x1");
        assert_eq!(display_key("X-Req"), "\"X-Req\"");
        assert_eq!(display_key("a b"), "\"a b\"");
        assert_eq!(display_key(""), "\"\"");
        assert_eq!(display_key("1st"), "\"1st\"");
        // A keyword is not a bare key.
        assert_eq!(display_key("if"), "\"if\"");
        // Escaped like a string literal.
        assert_eq!(display_key("say \"hi\"\n{x}"), "\"say \\\"hi\\\"\\n\\{x}\"");
    }

    #[test]
    fn record_display_quotes_non_identifier_keys() {
        let mut heap = Heap::new();
        let mut m = crate::heap::RecordMap::default();
        m.insert("X-Req".to_string(), Value::Int(1));
        m.insert("ok".to_string(), Value::Bool(true));
        let rec = Value::Map(heap.alloc_map(m));
        assert_eq!(value_to_display_string(&rec, &heap), "{ \"X-Req\": 1, ok: true }");
        assert_eq!(value_to_debug_string(&rec, &heap), "{ \"X-Req\": 1, ok: true }");
        let list = Value::List(heap.alloc_list(vec![rec]));
        assert_eq!(value_to_display_string(&list, &heap), "[{ \"X-Req\": 1, ok: true }]");
    }
}
