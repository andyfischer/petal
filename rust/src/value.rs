//! Value - Runtime representation of data.
//!
//! See docs/tech_outline/data_structures/Value.md

use std::fmt;

use crate::heap::{ListId, MapId, StringId};
use crate::program::{BuiltinId, ClosureId};

/// Runtime value. All variants are Copy — heap-allocated data is referenced by ID.
#[derive(Clone, Copy, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    Map(MapId),
    Closure(ClosureId),
    BuiltinFunction(BuiltinId),
    EnumVariant { tag: StringId, data: ListId },
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
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
            Value::Map(_) => "record",
            Value::Closure(_) => "function",
            Value::BuiltinFunction(_) => "function",
            Value::EnumVariant { .. } => "enum",
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
            Value::Map(id) => write!(f, "Map({:?})", id),
            Value::Closure(id) => write!(f, "Closure({:?})", id),
            Value::BuiltinFunction(id) => write!(f, "BuiltinFunction({:?})", id),
            Value::EnumVariant { tag, data } => {
                write!(f, "EnumVariant({:?}, {:?})", tag, data)
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
        Value::Map(id) => {
            let map = heap.get_map(*id);
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_debug_string(v, heap)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Value::Closure(_) => "<function>".to_string(),
        Value::BuiltinFunction(_) => "<builtin>".to_string(),
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
    }
}

pub fn value_to_debug_string(val: &Value, heap: &Heap) -> String {
    match val {
        Value::String(id) => format!("\"{}\"", heap.get_string(*id)),
        other => value_to_display_string(other, heap),
    }
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
            heap.get_string(*a) == heap.get_string(*b)
        }
        (
            Value::EnumVariant { tag: at, data: ad },
            Value::EnumVariant { tag: bt, data: bd },
        ) => {
            heap.get_string(*at) == heap.get_string(*bt) && {
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
        _ => false,
    }
}
