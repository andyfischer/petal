//! Runtime values for Petal

use std::fmt;

use crate::heap::{ListId, MapId, StringId};
use crate::program::FunctionId;

/// Runtime representation of data
#[derive(Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    Map(MapId),
    Function(FunctionId),
    /// Native/builtin function
    NativeFunction(String),
    /// Range for iteration
    Range { start: i64, end: i64 },
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Function(_) => "function",
            Value::NativeFunction(_) => "native_function",
            Value::Range { .. } => "range",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::String(_) => true,
            Value::List(_) => true,
            Value::Map(_) => true,
            Value::Function(_) => true,
            Value::NativeFunction(_) => true,
            Value::Range { .. } => true,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::Nil
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(n) => write!(f, "{}", n),
            Value::String(id) => write!(f, "String({:?})", id),
            Value::List(id) => write!(f, "List({:?})", id),
            Value::Map(id) => write!(f, "Map({:?})", id),
            Value::Function(id) => write!(f, "Function({:?})", id),
            Value::NativeFunction(name) => write!(f, "<native:{}>", name),
            Value::Range { start, end } => write!(f, "{}..{}", start, end),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(n) => {
                if n.fract() == 0.0 {
                    write!(f, "{:.1}", n)
                } else {
                    write!(f, "{}", n)
                }
            }
            Value::String(id) => write!(f, "<string:{}>", id.0),
            Value::List(id) => write!(f, "<list:{}>", id.0),
            Value::Map(id) => write!(f, "<map:{}>", id.0),
            Value::Function(id) => write!(f, "<fn:{}>", id.0),
            Value::NativeFunction(name) => write!(f, "<native:{}>", name),
            Value::Range { start, end } => write!(f, "{}..{}", start, end),
        }
    }
}
