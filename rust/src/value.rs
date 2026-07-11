//! Value - Runtime representation of data.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::fmt;

use crate::handle::HandleVal;
use crate::heap::{ElementId, F64ArrayId, ListId, MapId, StringId};
use crate::native_fn::NativeFnId;
use crate::program::{ClosureId, OverloadSetId, Program, TermId};
use crate::resource_table::{ResourceState, ResourceTable};
use crate::symbol::SymbolId;

/// Opaque index into an [`ExecutionContext`](crate::execution_context)'s resource
/// table. Kept a thin `Copy` id (like the heap ids) so [`Value`] stays `Copy`;
/// the resolution state and provenance live in the table entry it points at.
/// See docs/dev/pending-values-plan.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PendingId(pub u32);

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
    Map(MapId),
    Closure(ClosureId),
    /// Multi-arity function: dispatches to the right closure based on arg count.
    OverloadSet(OverloadSetId),
    NativeFunction(NativeFnId),
    EnumVariant { tag: StringId, data: ListId },
    Element(ElementId),
    /// Dual number for forward-mode automatic differentiation.
    /// Carries a primal value and its derivative (tangent).
    Dual { value: f64, derivative: f64 },
    /// 2D vector for creative coding (positions, velocities, forces).
    Vec2(f64, f64),
    /// An interned symbol — a binding key shared with the embedding host.
    /// See `crate::symbol`.
    Symbol(SymbolId),
    /// An opaque reference to a host-owned foreign object. See `crate::handle`.
    Handle(HandleVal),
    /// An unresolved (pending or errored) resource. A thin id into the owning
    /// context's resource table, where state/provenance live. Ordinary ops are
    /// strict in Pending (they absorb and return it); a small non-strict meta set
    /// inspects it. See docs/dev/pending-values-plan.md.
    // TODO(pending): operator/native absorption + meta builtins land in later chunks.
    Pending(PendingId),
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Dual { value, .. } => *value != 0.0,
            Value::Vec2(x, y) => *x != 0.0 || *y != 0.0,
            _ => true,
        }
    }

    /// Whether a value is "present" for the `??` coalescing operator: anything
    /// other than `Nil` or a `Pending` (loading OR errored). Distinct from
    /// [`is_truthy`](Value::is_truthy) — `0`, `false`, and `""` are present.
    pub fn is_present(&self) -> bool {
        !matches!(self, Value::Nil | Value::Pending(_))
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
            Value::Closure(_) => "function",
            Value::OverloadSet(_) => "function",
            Value::NativeFunction(_) => "function",
            Value::EnumVariant { .. } => "enum",
            Value::Element(_) => "element",
            Value::Dual { .. } => "dual",
            Value::Vec2(_, _) => "vec2",
            Value::Symbol(_) => "symbol",
            Value::Handle(_) => "handle",
            Value::Pending(_) => "pending",
        }
    }

    /// Extract the numeric value as f64 (for arithmetic with Dual numbers).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            Value::Dual { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Extract the derivative component (0.0 for non-Dual values).
    pub fn derivative(&self) -> f64 {
        match self {
            Value::Dual { derivative, .. } => *derivative,
            _ => 0.0,
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
            Value::F64Array(id) => write!(f, "F64Array({})", id.0),
            Value::Map(id) => write!(f, "Map({:?})", id),
            Value::Closure(id) => write!(f, "Closure({:?})", id),
            Value::OverloadSet(id) => write!(f, "OverloadSet({:?})", id),
            Value::NativeFunction(id) => write!(f, "NativeFunction({:?})", id),
            Value::EnumVariant { tag, data } => {
                write!(f, "EnumVariant({:?}, {:?})", tag, data)
            }
            Value::Element(id) => write!(f, "Element({:?})", id),
            Value::Dual { value, derivative } => {
                write!(f, "Dual({}, {})", format_float(*value), format_float(*derivative))
            }
            Value::Vec2(x, y) => {
                write!(f, "Vec2({}, {})", format_float(*x), format_float(*y))
            }
            Value::Symbol(id) => write!(f, "Symbol({})", id.0),
            Value::Handle(h) => write!(f, "{}", h),
            Value::Pending(id) => write!(f, "Pending({})", id.0),
        }
    }
}

/// Display helpers that need heap access. These are standalone functions
/// rather than methods because they need &Heap.
use crate::heap::Heap;

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
                .map(|(k, v)| format!("{}: {}", k, value_to_debug_string(v, heap)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Element(id) => element_to_display_string(*id, heap),
        Value::Closure(_) => "<function>".to_string(),
        Value::OverloadSet(_) => "<function>".to_string(),
        Value::NativeFunction(_) => "<native>".to_string(),
        Value::EnumVariant { tag, data } => {
            let name = heap.get_string(*tag);
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
        Value::Dual { value, derivative } => {
            format!("dual({}, {})", format_float(*value), format_float(*derivative))
        }
        Value::Vec2(x, y) => {
            format!("vec2({}, {})", format_float(*x), format_float(*y))
        }
        Value::Symbol(id) => format!("symbol#{}", id.0),
        Value::Handle(h) => h.to_string(),
        // Context-free fallback: unambiguous but bare (no state/origin/age, which
        // need the resource table + program). Provenance-rich rendering lives in
        // `pending_to_display` / `value_to_json_ctx`.
        Value::Pending(id) => format!("<pending {}>", id.0),
    }
}

pub fn value_to_debug_string(val: &Value, heap: &Heap) -> String {
    match val {
        Value::String(id) => format!("\"{}\"", heap.get_string(*id)),
        other => value_to_display_string(other, heap),
    }
}

/// Lower-case name of a resource's resolution state — the token debug surfaces
/// show (`loading` / `errored` / `ready`).
fn resource_state_name(state: &ResourceState) -> &'static str {
    match state {
        ResourceState::Loading => "loading",
        ResourceState::Errored(_) => "errored",
        ResourceState::Ready(_) => "ready",
    }
}

/// The source-text slice an origin `TermId` points at (e.g. `__pending("k")`),
/// used to attribute a pending value in debug output. Resolves the term's span
/// through the program's source map and slices the owning file's source by byte
/// offset (falling back to `Program::source` for entry-file spans). `None` when
/// there is no usable span or the slice would be empty / out of bounds.
fn origin_text(program: &Program, term_id: TermId) -> Option<String> {
    let span = program.source_map.get(term_id)?;
    let src = program
        .source_map
        .source_for_span(span)
        .unwrap_or(&program.source);
    let text = src.get(span.start.offset as usize..span.end.offset as usize)?;
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// The `{ line, col, text }` origin object for a pending entry's origin term, or
/// `null` when the entry has no origin (a resource a native created without a
/// reachable call site). Shared by the JSON pending rendering here and the
/// frame pending report.
fn pending_origin_json(program: &Program, origin: Option<TermId>) -> serde_json::Value {
    let Some(term_id) = origin else {
        return serde_json::Value::Null;
    };
    let (line, col) = match program.source_map.get(term_id) {
        Some(s) if s.start.line > 0 => (Some(s.start.line), Some(s.start.column)),
        _ => (None, None),
    };
    serde_json::json!({
        "line": line,
        "col": col,
        "text": origin_text(program, term_id),
    })
}

/// Provenance-rich rendering of a pending value for human-facing debug surfaces:
/// `<pending __pending("k") loading 2f>` — the origin call site's source text,
/// the resource's resolution state, and its age in frames. Falls back to `?` for
/// the origin when the entry has no origin term. Unlike the context-free
/// [`Display`](fmt::Display)/[`Debug`](fmt::Debug) (which can only show the bare
/// id), this needs the resource table (state + provenance), the program (origin
/// source text), and the current frame (age).
pub fn pending_to_display(
    id: PendingId,
    resources: &ResourceTable,
    program: &Program,
    current_frame: u64,
) -> String {
    let entry = resources.entry(id);
    let origin = entry
        .origin
        .and_then(|t| origin_text(program, t))
        .unwrap_or_else(|| "?".to_string());
    let state = resource_state_name(&entry.state);
    let age = entry.age_frames(current_frame);
    format!("<pending {origin} {state} {age}f>")
}

fn element_to_display_string(id: crate::heap::ElementId, heap: &Heap) -> String {
    let tag_id = heap.get_element_tag(id);
    let tag = heap.get_string(tag_id);
    let props_id = heap.get_element_props(id);
    let children_id = heap.get_element_children(id);
    let props = heap.get_map(props_id);
    let children = heap.get_list(children_id);

    let mut s = format!("<{}", tag);
    for (k, v) in props {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(&value_to_display_string(v, heap));
        s.push('"');
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

fn element_to_json(
    id: crate::heap::ElementId,
    heap: &Heap,
    ctx: Option<&PendingJsonCtx>,
) -> serde_json::Value {
    let tag_id = heap.get_element_tag(id);
    let tag = heap.get_string(tag_id).to_string();
    let props_id = heap.get_element_props(id);
    let children_id = heap.get_element_children(id);
    let props = heap.get_map(props_id);
    let children = heap.get_list(children_id);

    let props_obj: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json_ctx(v, heap, ctx)))
        .collect();

    let children_arr: Vec<serde_json::Value> = children
        .iter()
        .map(|child| value_to_json_ctx(child, heap, ctx))
        .collect();

    serde_json::json!({
        "type": "element",
        "tag": tag,
        "props": props_obj,
        "children": children_arr
    })
}

/// Rendering context for provenance-rich pending values in JSON dumps: the
/// resource table (state + provenance), the program (origin source text), and
/// the current frame (age). Threaded through [`value_to_json_ctx`] so a
/// `Value::Pending` — including one nested in a list, map, or element — dumps as
/// a structured object instead of the context-free `"<pending N>"` fallback.
pub struct PendingJsonCtx<'a> {
    pub resources: &'a ResourceTable,
    pub program: &'a Program,
    pub frame: u64,
}

/// The structured JSON object for a pending value:
/// `{ type:"pending", id, key, state, age_frames, origin }` — the shape debug
/// surfaces and the frame report consume.
fn pending_json(id: PendingId, ctx: &PendingJsonCtx) -> serde_json::Value {
    let entry = ctx.resources.entry(id);
    serde_json::json!({
        "type": "pending",
        "id": id.0,
        "key": entry.key,
        "state": resource_state_name(&entry.state),
        "age_frames": entry.age_frames(ctx.frame),
        "origin": pending_origin_json(ctx.program, entry.origin),
    })
}

/// The frame pending report: a structured summary over **every** live resource
/// in `resources`, as a JSON array of
/// `{ id, key, state, age_frames, origin, absorbed_count }` objects (origin is
/// `{ line, col, text }` or `null`). This is the data the debug-protocol
/// `pending_report` query, the petal-ui overlay hook, and `--trace-pending`
/// consume: state + provenance + this-frame absorption count for the whole
/// table. `current_frame` supplies each entry's age; `program` resolves origin
/// source text, reusing [`pending_origin_json`] (the same resolution the Chunk-M
/// per-value rendering uses).
pub fn pending_report_json(
    resources: &ResourceTable,
    program: &Program,
    current_frame: u64,
) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = resources
        .iter()
        .map(|(id, entry)| {
            serde_json::json!({
                "id": id.0,
                "key": entry.key,
                "state": resource_state_name(&entry.state),
                "age_frames": entry.age_frames(current_frame),
                "origin": pending_origin_json(program, entry.origin),
                "absorbed_count": entry.absorbed_count,
            })
        })
        .collect();
    serde_json::Value::Array(entries)
}

/// Convert a Value to serde_json::Value for JSON serialization, without pending
/// provenance. A `Value::Pending` renders as its context-free `"<pending N>"`
/// string; callers that can supply a [`PendingJsonCtx`] should use
/// [`value_to_json_ctx`] so pending values dump as structured objects.
///
/// Nil→null, Bool→bool, Int/Float→number, String→string, List→array
/// (recursive), Map→object (recursive), others→string via display.
pub fn value_to_json(val: &Value, heap: &Heap) -> serde_json::Value {
    value_to_json_ctx(val, heap, None)
}

/// Like [`value_to_json`], but with an optional [`PendingJsonCtx`] so a
/// `Value::Pending` dumps as the structured `{ type:"pending", … }` object
/// (state + provenance + age) instead of the bare `"<pending N>"` string. Pass
/// `None` to match [`value_to_json`] exactly.
pub fn value_to_json_ctx(
    val: &Value,
    heap: &Heap,
    ctx: Option<&PendingJsonCtx>,
) -> serde_json::Value {
    match val {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::json!(*n),
        Value::Float(f) => serde_json::json!(*f),
        Value::String(id) => serde_json::Value::String(heap.get_string(*id).to_string()),
        Value::List(id) => {
            let elems = heap.get_list(*id);
            let arr: Vec<serde_json::Value> =
                elems.iter().map(|v| value_to_json_ctx(v, heap, ctx)).collect();
            serde_json::Value::Array(arr)
        }
        Value::F64Array(id) => {
            let data = heap.get_f64_array(*id);
            let arr: Vec<serde_json::Value> = data.iter().map(|f| serde_json::json!(*f)).collect();
            serde_json::Value::Array(arr)
        }
        Value::Map(id) => {
            let map = heap.get_map(*id);
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json_ctx(v, heap, ctx)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Dual { value, derivative } => {
            serde_json::json!({ "type": "dual", "value": *value, "derivative": *derivative })
        }
        Value::Vec2(x, y) => {
            serde_json::json!({ "type": "vec2", "x": *x, "y": *y })
        }
        Value::EnumVariant { tag, data } => {
            let name = heap.get_string(*tag).to_string();
            let fields = heap.get_list(*data);
            let arr: Vec<serde_json::Value> =
                fields.iter().map(|v| value_to_json_ctx(v, heap, ctx)).collect();
            serde_json::json!({ "type": "enum", "tag": name, "data": arr })
        }
        Value::Element(id) => element_to_json(*id, heap, ctx),
        Value::Symbol(id) => serde_json::json!({ "type": "symbol", "id": id.0 }),
        // A pending value renders richly when a context is available, else falls
        // back to its context-free string — never `null` or a bare handle.
        Value::Pending(id) => match ctx {
            Some(c) => pending_json(*id, c),
            None => serde_json::Value::String(value_to_display_string(val, heap)),
        },
        // Closures, native functions → string representation
        other => serde_json::Value::String(value_to_display_string(other, heap)),
    }
}

/// Convert a JSON value to a Petal Value.
/// Supports null, bool, number (int/float), and string.
pub fn json_to_value(json: &serde_json::Value, heap: &mut Heap) -> Result<Value, String> {
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
        _ => Err("Only null, bool, number, and string values are supported".to_string()),
    }
}

/// Hash a value to a u64 for use as an explicit state key.
/// Uses the value's content directly — no heap needed for primitives.
pub fn hash_value(val: &Value, heap: &Heap) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match val {
        Value::Nil => 0u8.hash(&mut hasher),
        Value::Bool(b) => { 1u8.hash(&mut hasher); b.hash(&mut hasher); }
        Value::Int(n) => { 2u8.hash(&mut hasher); n.hash(&mut hasher); }
        Value::Float(f) => { 3u8.hash(&mut hasher); f.to_bits().hash(&mut hasher); }
        Value::String(id) => { 4u8.hash(&mut hasher); heap.get_string(*id).hash(&mut hasher); }
        Value::List(id) => {
            5u8.hash(&mut hasher);
            let elems = heap.get_list(*id);
            for elem in elems {
                hash_value(elem, heap).hash(&mut hasher);
            }
        }
        Value::Vec2(x, y) => {
            7u8.hash(&mut hasher);
            x.to_bits().hash(&mut hasher);
            y.to_bits().hash(&mut hasher);
        }
        Value::F64Array(id) => {
            8u8.hash(&mut hasher);
            for f in heap.get_f64_array(*id) {
                f.to_bits().hash(&mut hasher);
            }
        }
        Value::Handle(h) => {
            9u8.hash(&mut hasher);
            h.class.0.hash(&mut hasher);
            h.slot.hash(&mut hasher);
            h.serial.hash(&mut hasher);
        }
        // For other types, hash the debug representation
        other => { 6u8.hash(&mut hasher); format!("{:?}", other).hash(&mut hasher); }
    }
    hasher.finish()
}

/// Compare two values for equality. Needs heap access for deep comparison
/// of lists and maps.
pub fn values_equal(a: &Value, b: &Value, heap: &Heap) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        (Value::String(a), Value::String(b)) => {
            // With string interning, equal content means equal IDs
            a == b || heap.get_string(*a) == heap.get_string(*b)
        }
        (
            Value::EnumVariant { tag: at, data: ad },
            Value::EnumVariant { tag: bt, data: bd },
        ) => {
            (at == bt || heap.get_string(*at) == heap.get_string(*bt)) && {
                let a_fields = heap.get_list(*ad);
                let b_fields = heap.get_list(*bd);
                a_fields.len() == b_fields.len()
                    && a_fields
                        .iter()
                        .zip(b_fields.iter())
                        .all(|(a, b)| values_equal(a, b, heap))
            }
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
        (Value::F64Array(a), Value::F64Array(b)) => {
            let a_data = heap.get_f64_array(*a);
            let b_data = heap.get_f64_array(*b);
            a_data == b_data
        }
        (Value::NativeFunction(a), Value::NativeFunction(b)) => a == b,
        (Value::Dual { value: av, derivative: ad }, Value::Dual { value: bv, derivative: bd }) => {
            av == bv && ad == bd
        }
        // Dual compared with numeric: compare primal values only
        (Value::Dual { value, .. }, Value::Float(f)) | (Value::Float(f), Value::Dual { value, .. }) => {
            value == f
        }
        (Value::Dual { value, .. }, Value::Int(n)) | (Value::Int(n), Value::Dual { value, .. }) => {
            *value == *n as f64
        }
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => ax == bx && ay == by,
        (Value::Handle(a), Value::Handle(b)) => a == b,
        // Two Pendings are equal iff they reference the same resource entry.
        // (Ordinary `==` on Pending is strict — absorbs — in later chunks; this
        // structural equality is for tooling/tests.)
        (Value::Pending(a), Value::Pending(b)) => a == b,
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

/// Order two Values (used for sorting and by the `min`/`max` builtins). Numeric
/// kinds compare by their `f64` value (dual numbers by their primal); strings
/// compare lexically; mismatched non-numeric kinds are an error.
pub fn compare_values(a: &Value, b: &Value, heap: &Heap) -> Result<std::cmp::Ordering, String> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => Ok((*a as f64)
            .partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal)),
        (Value::Float(a), Value::Int(b)) => Ok(a
            .partial_cmp(&(*b as f64))
            .unwrap_or(std::cmp::Ordering::Equal)),
        (Value::String(a), Value::String(b)) => Ok(heap.get_string(*a).cmp(heap.get_string(*b))),
        // Dual comparisons use primal value only
        _ if a.as_f64().is_some() && b.as_f64().is_some() => {
            let af = a.as_f64().unwrap();
            let bf = b.as_f64().unwrap();
            Ok(af.partial_cmp(&bf).unwrap_or(std::cmp::Ordering::Equal))
        }
        _ => Err(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nil_is_falsy() {
        assert!(!Value::Nil.is_truthy());
    }

    #[test]
    fn false_is_falsy() {
        assert!(!Value::Bool(false).is_truthy());
    }

    #[test]
    fn true_is_truthy() {
        assert!(Value::Bool(true).is_truthy());
    }

    #[test]
    fn zero_int_is_falsy() {
        assert!(!Value::Int(0).is_truthy());
    }

    #[test]
    fn nonzero_int_is_truthy() {
        assert!(Value::Int(42).is_truthy());
    }

    #[test]
    fn zero_float_is_falsy() {
        assert!(!Value::Float(0.0).is_truthy());
    }

    #[test]
    fn nonzero_float_is_truthy() {
        assert!(Value::Float(3.25).is_truthy());
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
}
