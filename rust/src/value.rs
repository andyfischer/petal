//! Value - Runtime representation of data.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::fmt;

use crate::handle::HandleVal;
use crate::heap::{CellId, ElementId, F64ArrayId, ListId, MapId, StringId, Vec3Id};
use crate::native_fn::NativeFnId;
use crate::program::{ClosureId, OverloadSetId, Program, TermId};
use crate::resource_table::{ResourceState, ResourceTable};
use crate::symbol::SymbolId;

/// Opaque index into an [`ExecutionContext`](crate::execution_context)'s resource
/// table. Kept a thin `Copy` id (like the heap ids) so [`Value`] stays `Copy`;
/// the resolution state and provenance live in the table entry it points at.
/// Unlike heap ids it needs no generation: table entries are never removed, so
/// an index is never reused. Resolved payloads are GC roots.
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
    EnumVariant {
        tag: StringId,
        data: ListId,
    },
    Element(ElementId),
    /// A mutable one-value box behind a `var` binding. Never escapes into user
    /// code: every source-level read of a `var` dereferences it (`CellRead`),
    /// so no expression evaluates to a `Cell`. The only way two holders share
    /// one is closure capture, which is lexically visible. See the containment
    /// invariant in docs/var.md (Containment).
    Cell(CellId),
    /// Dual number for forward-mode automatic differentiation.
    /// Carries a primal value and its derivative (tangent).
    Dual {
        value: f64,
        derivative: f64,
    },
    /// 2D vector for creative coding (positions, velocities, forces).
    Vec2(f64, f64),
    /// 3D vector (positions, directions, colors in 3D code). Heap-allocated
    /// because three `f64`s would widen every `Value`; the components are
    /// immutable, so the id behaves like a value. See docs/dev/vec3.md.
    Vec3(Vec3Id),
    /// An interned symbol — a binding key shared with the embedding host.
    /// See `crate::symbol`.
    Symbol(SymbolId),
    /// An opaque reference to a host-owned foreign object. See `crate::handle`.
    Handle(HandleVal),
    /// An unresolved (pending or errored) resource. A thin id into the owning
    /// context's resource table, where state/provenance live. Ordinary ops are
    /// strict in Pending (they absorb and return it); a small non-strict meta set
    /// inspects it. See docs/dev/pending-values-plan.md.
    Pending(PendingId),
}

// Heap ids are 8 bytes (index + generation); the widest payloads are
// `Dual`/`Vec2` and `EnumVariant`'s two ids, all 16 bytes. Keep it that way —
// it is why `Vec3` is a heap id rather than three inline f64s.
const _: () = assert!(std::mem::size_of::<Value>() == 24);

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Dual { value, .. } => *value != 0.0,
            Value::Vec2(x, y) => *x != 0.0 || *y != 0.0,
            // Always truthy, like a list or record: its components are on the
            // heap, which this heap-free check cannot read. Test a zero vector
            // explicitly (`v == vec3(0, 0, 0)`).
            Value::Vec3(_) => true,
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
            Value::Cell(_) => "cell",
            Value::Dual { .. } => "dual",
            Value::Vec2(_, _) => "vec2",
            Value::Vec3(_) => "vec3",
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
            Value::F64Array(id) => write!(f, "F64Array({})", id.index()),
            Value::Map(id) => write!(f, "Map({:?})", id),
            Value::Closure(id) => write!(f, "Closure({:?})", id),
            Value::OverloadSet(id) => write!(f, "OverloadSet({:?})", id),
            Value::NativeFunction(id) => write!(f, "NativeFunction({:?})", id),
            Value::EnumVariant { tag, data } => {
                write!(f, "EnumVariant({:?}, {:?})", tag, data)
            }
            Value::Element(id) => write!(f, "Element({:?})", id),
            Value::Cell(id) => write!(f, "Cell({:?})", id),
            Value::Dual { value, derivative } => {
                write!(
                    f,
                    "Dual({}, {})",
                    format_float(*value),
                    format_float(*derivative)
                )
            }
            Value::Vec2(x, y) => {
                write!(f, "Vec2({}, {})", format_float(*x), format_float(*y))
            }
            Value::Vec3(id) => write!(f, "Vec3({:?})", id),
            Value::Symbol(id) => write!(f, "Symbol({})", id.0),
            Value::Handle(h) => write!(f, "{}", h),
            Value::Pending(id) => write!(f, "Pending({})", id.0),
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
                .map(|(k, v)| format!("{}: {}", k, value_to_debug_string(v, heap)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
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
            format!(
                "dual({}, {})",
                format_float(*value),
                format_float(*derivative)
            )
        }
        Value::Vec2(x, y) => {
            format!("vec2({}, {})", format_float(*x), format_float(*y))
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
            let arr: Vec<serde_json::Value> = elems
                .iter()
                .map(|v| value_to_json_ctx(v, heap, ctx))
                .collect();
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
        Value::Vec3(id) => {
            let [x, y, z] = heap.get_vec3(*id);
            serde_json::json!({ "type": "vec3", "x": x, "y": y, "z": z })
        }
        Value::EnumVariant { tag, data } => {
            let name = heap.get_string(*tag).to_string();
            let fields = heap.get_list(*data);
            let arr: Vec<serde_json::Value> = fields
                .iter()
                .map(|v| value_to_json_ctx(v, heap, ctx))
                .collect();
            serde_json::json!({ "type": "enum", "tag": name, "data": arr })
        }
        Value::Element(id) => element_to_json(*id, heap, ctx),
        // A `state var`'s slot holds the cell, so this is the one place a cell
        // reaches a serialization surface. Report the *contents*: the box is an
        // implementation detail, and someone inspecting `hits` wants `3`.
        Value::Cell(id) => value_to_json_ctx(&heap.cell_read(*id), heap, ctx),
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
/// Supports null, bool, number (int/float), string, array and object — plus
/// the tagged vector objects [`value_to_json`] writes (`{"type": "vec2", "x",
/// "y"}` and `{"type": "vec3", "x", "y", "z"}`), which come back as vectors so
/// a vector survives a JSON round trip (`json_parse(json_stringify(v))`, a
/// state dump and restore).
pub fn json_to_value(json: &serde_json::Value, heap: &mut Heap) -> Result<Value, String> {
    if let Some(v) = json_to_vector(json, heap) {
        return Ok(v);
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
        "vec2" if obj.len() == 3 => Some(Value::Vec2(num("x")?, num("y")?)),
        "vec3" if obj.len() == 4 => {
            let (x, y, z) = (num("x")?, num("y")?, num("z")?);
            Some(heap.vec3_value(x, y, z))
        }
        _ => None,
    }
}

/// The numeric view of a value that `==` and `<` compare by: ints, floats, and
/// a dual number's primal. `None` for everything else.
pub fn as_num(v: &Value) -> Option<Num> {
    match *v {
        Value::Int(n) => Some(Num::Int(n)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Dual { value, .. } => Some(Num::Float(value)),
        _ => None,
    }
}

/// Hash a value to a u64 for use as an explicit state key.
///
/// Consistent with [`values_equal`]: values that are `==` hash equally, so
/// `state(key)` finds the same slot for every key that is `==` — including a
/// record rebuilt each frame, and `2` vs `2.0`. Containers hash their content,
/// never their heap id (ids differ between two equal values).
pub fn hash_value(val: &Value, heap: &Heap) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hash_into(val, heap, &mut hasher);
    hasher.finish()
}

fn hash_into(val: &Value, heap: &Heap, h: &mut impl std::hash::Hasher) {
    use std::hash::{Hash, Hasher};
    if let Some(n) = as_num(val) {
        2u8.hash(h);
        numeric::num_key(n).hash(h);
        return;
    }
    match val {
        Value::Nil => 0u8.hash(h),
        Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        Value::String(id) => {
            4u8.hash(h);
            heap.get_string(*id).hash(h);
        }
        Value::List(id) => {
            5u8.hash(h);
            let elems = heap.get_list(*id);
            elems.len().hash(h);
            for elem in elems {
                hash_into(elem, heap, h);
            }
        }
        Value::Vec2(x, y) => {
            7u8.hash(h);
            numeric::num_key(Num::Float(*x)).hash(h);
            numeric::num_key(Num::Float(*y)).hash(h);
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
        Value::EnumVariant { tag, data } => {
            11u8.hash(h);
            heap.get_string(*tag).hash(h);
            hash_into(&Value::List(*data), heap, h);
        }
        Value::Element(id) => {
            12u8.hash(h);
            heap.get_string(heap.get_element_tag(*id)).hash(h);
            hash_into(&Value::Map(heap.get_element_props(*id)), heap, h);
            hash_into(&Value::List(heap.get_element_children(*id)), heap, h);
        }
        // Identity-compared values (functions, symbols, cells, pendings): the
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
/// - numbers compare exactly across Int and Float (no rounding to f64), and a
///   dual number compares by its primal value, against anything numeric;
/// - lists, enum variants and elements compare element-wise; records compare
///   by class and by key set and values, ignoring key order;
/// - functions, symbols, cells, handles and pendings compare by identity.
pub fn values_equal(a: &Value, b: &Value, heap: &Heap) -> bool {
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
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
            (at == bt || heap.get_string(*at) == heap.get_string(*bt))
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
            xs.len() == ys.len()
                && xs.iter().all(|(k, x)| match ys.get(k) {
                    Some(y) => values_equal(x, y, heap),
                    None => false,
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
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => ax == bx && ay == by,
        (Value::Vec3(a), Value::Vec3(b)) => a == b || heap.get_vec3(*a) == heap.get_vec3(*b),
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

/// Order two Values (used by the `<`/`<=`/`>`/`>=` operators and the
/// `min`/`max` builtins). Numeric kinds compare exactly by value (dual numbers
/// by their primal); strings compare lexically; mismatched non-numeric kinds
/// are an error. `Ok(None)` when the two are unordered — a NaN is involved —
/// so every ordering operator on it is false, as in IEEE 754.
pub fn compare_values_partial(
    a: &Value,
    b: &Value,
    heap: &Heap,
) -> Result<Option<std::cmp::Ordering>, String> {
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
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

    // ── Exhaustive laws of `==`, hashing and ordering ──────────────
    //
    // Small-scope exhaustive check: every pair and triple drawn from a universe
    // of values up to nesting depth 2 — numeric edge cases (±0, NaN, 2^53 + 1
    // vs 2^53 as float, duals), strings allocated twice, and every container
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
            Value::Dual {
                value: 1.0,
                derivative: 5.0,
            },
            Value::Dual {
                value: 1.0,
                derivative: 0.0,
            },
            Value::Vec2(0.0, 1.0),
            Value::Vec2(-0.0, 1.0),
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
            all.push(Value::EnumVariant { tag: key, data });
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
            Value::Dual { value, .. } => value.is_nan(),
            Value::List(id) => heap.get_list(id).iter().any(|x| contains_nan(x, heap)),
            Value::Map(id) => heap.get_map(id).values().any(|x| contains_nan(x, heap)),
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
}
