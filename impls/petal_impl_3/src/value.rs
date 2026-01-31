//! Runtime values

use slotmap::new_key_type;

new_key_type! {
    pub struct StringId;
    pub struct ListId;
    pub struct MapId;
    pub struct FunctionId;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(StringId),
    List(ListId),
    Map(MapId),
    Function(FunctionId),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "Nil",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Function(_) => "Function",
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            _ => true,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(i) => write!(f, "{}", i),
            Value::Float(fl) => {
                if fl.fract() == 0.0 {
                    write!(f, "{}.", fl)
                } else {
                    write!(f, "{}", fl)
                }
            }
            Value::String(id) => write!(f, "<String {:?}>", id),
            Value::List(id) => write!(f, "<List {:?}>", id),
            Value::Map(id) => write!(f, "<Map {:?}>", id),
            Value::Function(id) => write!(f, "<Function {:?}>", id),
        }
    }
}

#[derive(Debug, Clone)]
pub enum HeapValue {
    String(String),
    List(Vec<Value>),
    Map(std::collections::HashMap<String, Value>),
}
