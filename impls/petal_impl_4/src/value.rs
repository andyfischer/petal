//! Runtime values for Petal

use std::collections::HashMap;
use crate::parser::FunctionId;
use crate::heap::{StringId, ListId, MapId};

/// Runtime representation of a value
#[derive(Clone, Debug, PartialEq)]
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
    /// Check if the value is truthy
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            _ => true, // Strings, lists, maps, functions are truthy
        }
    }

    /// Get the type name of this value
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
        }
    }

    /// Try to convert to an integer
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    /// Try to convert to a float
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// Try to convert to a bool
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl Eq for Value {}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Nil => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(n) => n.hash(state),
            Value::Float(f) => f.to_bits().hash(state),
            Value::String(s) => s.hash(state),
            Value::List(l) => l.hash(state),
            Value::Map(m) => m.hash(state),
            Value::Function(f) => f.hash(state),
        }
    }
}

impl Default for Value {
    fn default() -> Self {
        Value::Nil
    }
}

// Arithmetic operations
impl Value {
    /// Add two values
    pub fn add(&self, other: &Value, heap: &mut crate::heap::Heap) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
            (Value::String(a), Value::String(b)) => {
                let s1 = heap.get_string(*a).unwrap_or("");
                let s2 = heap.get_string(*b).unwrap_or("");
                let result = format!("{}{}", s1, s2);
                Ok(Value::String(heap.alloc_string(&result)))
            }
            (Value::List(a), Value::List(b)) => {
                let list_a = heap.get_list(*a).cloned().unwrap_or_default();
                let list_b = heap.get_list(*b).cloned().unwrap_or_default();
                let new_list = heap.alloc_list();
                for item in list_a {
                    heap.push_to_list(new_list, item);
                }
                for item in list_b {
                    heap.push_to_list(new_list, item);
                }
                Ok(Value::List(new_list))
            }
            _ => Err(format!("Cannot add {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Subtract two values
    pub fn sub(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - *b as f64)),
            _ => Err(format!("Cannot subtract {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Multiply two values
    pub fn mul(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * *b as f64)),
            _ => Err(format!("Cannot multiply {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Divide two values
    pub fn div(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Int(a / b))
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Float(a / b))
                }
            }
            (Value::Int(a), Value::Float(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Float(*a as f64 / b))
                }
            }
            (Value::Float(a), Value::Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Float(a / *b as f64))
                }
            }
            _ => Err(format!("Cannot divide {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Modulo operation
    pub fn modulo(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Int(a % b))
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Value::Float(a % b))
                }
            }
            _ => Err(format!("Cannot compute modulo of {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Power operation
    pub fn pow(&self, other: &Value) -> Result<Value, String> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => {
                if *b >= 0 {
                    Ok(Value::Int(a.pow(*b as u32)))
                } else {
                    Ok(Value::Float((*a as f64).powf(*b as f64)))
                }
            }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(*b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float((*a as f64).powf(*b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a.powf(*b as f64))),
            _ => Err(format!("Cannot compute power of {} and {}", self.type_name(), other.type_name())),
        }
    }

    /// Negate a value
    pub fn neg(&self) -> Result<Value, String> {
        match self {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            _ => Err(format!("Cannot negate {}", self.type_name())),
        }
    }

    /// Logical not
    pub fn not(&self) -> Value {
        Value::Bool(!self.is_truthy())
    }

    /// Logical and
    pub fn and(&self, other: &Value) -> Value {
        Value::Bool(self.is_truthy() && other.is_truthy())
    }

    /// Logical or
    pub fn or(&self, other: &Value) -> Value {
        Value::Bool(self.is_truthy() || other.is_truthy())
    }

    /// Compare values
    pub fn compare(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::String(a), Value::String(b)) => {
                // Strings would need heap access - simplified
                Some(std::cmp::Ordering::Equal)
            }
            _ => None,
        }
    }
}
