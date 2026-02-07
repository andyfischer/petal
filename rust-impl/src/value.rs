use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use crate::ast::Stmt;

#[derive(Debug, Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    Record(Rc<RefCell<BTreeMap<String, Value>>>),
    Function {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
        closure: crate::env_scope::Environment,
    },
    BuiltinFunction(String),
    EnumVariant {
        name: String,
        data: Vec<Value>,
    },
    /// Represents a lambda/anonymous function
    Lambda {
        params: Vec<String>,
        body: Vec<Stmt>,
        closure: crate::env_scope::Environment,
    },
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
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
            Value::Record(_) => "record",
            Value::Function { .. } => "function",
            Value::BuiltinFunction(_) => "function",
            Value::EnumVariant { .. } => "enum",
            Value::Lambda { .. } => "function",
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Value::Nil => "nil".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => format_float(*f),
            Value::String(s) => s.clone(),
            Value::List(items) => {
                let items = items.borrow();
                let parts: Vec<String> = items.iter().map(|v| v.to_debug_string()).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Record(fields) => {
                let fields = fields.borrow();
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_debug_string()))
                    .collect();
                format!("{{ {} }}", parts.join(", "))
            }
            Value::Function { name, .. } => format!("<fn {}>", name),
            Value::BuiltinFunction(name) => format!("<builtin {}>", name),
            Value::EnumVariant { name, data } => {
                if data.is_empty() {
                    name.clone()
                } else {
                    let parts: Vec<String> = data.iter().map(|v| v.to_debug_string()).collect();
                    format!("{}({})", name, parts.join(", "))
                }
            }
            Value::Lambda { .. } => "<lambda>".to_string(),
        }
    }

    pub fn to_debug_string(&self) -> String {
        match self {
            Value::String(s) => format!("\"{}\"", s),
            other => other.to_display_string(),
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

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::String(a), Value::String(b)) => a == b,
            (Value::EnumVariant { name: a, data: ad }, Value::EnumVariant { name: b, data: bd }) => {
                a == b && ad == bd
            }
            (Value::List(a), Value::List(b)) => *a.borrow() == *b.borrow(),
            _ => false,
        }
    }
}
