//! Value - Runtime representation of data.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::fmt;

use crate::heap::{ElementId, F64ArrayId, ListId, MapId, StringId};
use crate::native_fn::NativeFnId;
use crate::program::{ClosureId, OverloadSetId};

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

fn element_to_json(id: crate::heap::ElementId, heap: &Heap) -> serde_json::Value {
    let tag_id = heap.get_element_tag(id);
    let tag = heap.get_string(tag_id).to_string();
    let props_id = heap.get_element_props(id);
    let children_id = heap.get_element_children(id);
    let props = heap.get_map(props_id);
    let children = heap.get_list(children_id);

    let props_obj: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v, heap)))
        .collect();

    let children_arr: Vec<serde_json::Value> = children
        .iter()
        .map(|child| value_to_json(child, heap))
        .collect();

    serde_json::json!({
        "type": "element",
        "tag": tag,
        "props": props_obj,
        "children": children_arr
    })
}

/// Convert a Value to serde_json::Value for JSON serialization.
/// Nil→null, Bool→bool, Int/Float→number, String→string,
/// List→array (recursive), Map→object (recursive), others→string via display.
pub fn value_to_json(val: &Value, heap: &Heap) -> serde_json::Value {
    match val {
        Value::Nil => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::json!(*n),
        Value::Float(f) => serde_json::json!(*f),
        Value::String(id) => serde_json::Value::String(heap.get_string(*id).to_string()),
        Value::List(id) => {
            let elems = heap.get_list(*id);
            let arr: Vec<serde_json::Value> = elems.iter().map(|v| value_to_json(v, heap)).collect();
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
                .map(|(k, v)| (k.clone(), value_to_json(v, heap)))
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
            let arr: Vec<serde_json::Value> = fields.iter().map(|v| value_to_json(v, heap)).collect();
            serde_json::json!({ "type": "enum", "tag": name, "data": arr })
        }
        Value::Element(id) => element_to_json(*id, heap),
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
