//! Builtins - Built-in function registry and implementations.

use crate::heap::Heap;
use crate::program::BuiltinId;
use crate::value::{self, Value};

/// Registry of built-in functions.
pub struct BuiltinTable {
    names: Vec<&'static str>,
}

// Builtin IDs (constants for fast dispatch)
pub const BUILTIN_PRINT: BuiltinId = BuiltinId(0);
pub const BUILTIN_RANGE: BuiltinId = BuiltinId(1);
pub const BUILTIN_LEN: BuiltinId = BuiltinId(2);
pub const BUILTIN_PUSH: BuiltinId = BuiltinId(3);
pub const BUILTIN_STR: BuiltinId = BuiltinId(4);
pub const BUILTIN_ABS: BuiltinId = BuiltinId(5);
pub const BUILTIN_SQRT: BuiltinId = BuiltinId(6);
pub const BUILTIN_FLOOR: BuiltinId = BuiltinId(7);
pub const BUILTIN_CEIL: BuiltinId = BuiltinId(8);
pub const BUILTIN_FLOAT: BuiltinId = BuiltinId(9);
pub const BUILTIN_INT: BuiltinId = BuiltinId(10);
pub const BUILTIN_RANDOM: BuiltinId = BuiltinId(11);
pub const BUILTIN_TYPE: BuiltinId = BuiltinId(12);
pub const BUILTIN_APPEND: BuiltinId = BuiltinId(13);
pub const BUILTIN_POP: BuiltinId = BuiltinId(14);
pub const BUILTIN_KEYS: BuiltinId = BuiltinId(15);
pub const BUILTIN_VALUES: BuiltinId = BuiltinId(16);
pub const BUILTIN_CONTAINS: BuiltinId = BuiltinId(17);
pub const BUILTIN_MIN: BuiltinId = BuiltinId(18);
pub const BUILTIN_MAX: BuiltinId = BuiltinId(19);
pub const BUILTIN_ROUND: BuiltinId = BuiltinId(20);
pub const BUILTIN_MAP: BuiltinId = BuiltinId(21);
pub const BUILTIN_FILTER: BuiltinId = BuiltinId(22);
pub const BUILTIN_REDUCE: BuiltinId = BuiltinId(23);

const BUILTIN_NAMES: &[&str] = &[
    "print", "range", "len", "push", "str", "abs", "sqrt", "floor", "ceil",
    "float", "int", "random", "type", "append", "pop", "keys", "values",
    "contains", "min", "max", "round", "map", "filter", "reduce",
];

impl BuiltinTable {
    pub fn new() -> Self {
        Self {
            names: BUILTIN_NAMES.to_vec(),
        }
    }

    pub fn lookup_name(&self, name: &str) -> Option<BuiltinId> {
        self.names
            .iter()
            .position(|&n| n == name)
            .map(|i| BuiltinId(i as u16))
    }

    pub fn get_name(&self, id: BuiltinId) -> &str {
        self.names[id.0 as usize]
    }

    pub fn count(&self) -> usize {
        self.names.len()
    }
}

impl Default for BuiltinTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a builtin function. Returns Ok(Value) or Err(error message).
pub fn call_builtin(
    id: BuiltinId,
    args: &[Value],
    heap: &mut Heap,
    output: &mut Vec<String>,
) -> Result<Value, String> {
    match id {
        BUILTIN_PRINT => {
            let parts: Vec<String> = args
                .iter()
                .map(|v| value::value_to_display_string(v, heap))
                .collect();
            let line = parts.join(" ");
            println!("{}", line);
            output.push(line);
            Ok(Value::Nil)
        }
        BUILTIN_RANGE => {
            if args.len() != 2 {
                return Err("range() expects 2 arguments".into());
            }
            let start = match args[0] {
                Value::Int(n) => n,
                _ => return Err("range() expects integer arguments".into()),
            };
            let end = match args[1] {
                Value::Int(n) => n,
                _ => return Err("range() expects integer arguments".into()),
            };
            let items: Vec<Value> = (start..end).map(Value::Int).collect();
            let id = heap.alloc_list(items);
            Ok(Value::List(id))
        }
        BUILTIN_LEN => {
            if args.len() != 1 {
                return Err("len() expects 1 argument".into());
            }
            match args[0] {
                Value::List(id) => Ok(Value::Int(heap.list_len(id) as i64)),
                Value::String(id) => Ok(Value::Int(heap.get_string(id).len() as i64)),
                _ => Err(format!("Cannot get length of {}", args[0].type_name())),
            }
        }
        BUILTIN_PUSH => {
            if args.len() != 2 {
                return Err("push() expects 2 arguments".into());
            }
            match args[0] {
                Value::List(id) => {
                    heap.get_list_mut(id).push(args[1]);
                    Ok(Value::Nil)
                }
                _ => Err("push() expects a list as first argument".into()),
            }
        }
        BUILTIN_STR => {
            if args.len() != 1 {
                return Err("str() expects 1 argument".into());
            }
            let s = value::value_to_display_string(&args[0], heap);
            let id = heap.alloc_string(s);
            Ok(Value::String(id))
        }
        BUILTIN_ABS => {
            if args.len() != 1 {
                return Err("abs() expects 1 argument".into());
            }
            match args[0] {
                Value::Int(n) => Ok(Value::Int(n.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                _ => Err("abs() expects a number".into()),
            }
        }
        BUILTIN_SQRT => {
            if args.len() != 1 {
                return Err("sqrt() expects 1 argument".into());
            }
            let n = to_float(&args[0])?;
            Ok(Value::Float(n.sqrt()))
        }
        BUILTIN_FLOOR => {
            if args.len() != 1 {
                return Err("floor() expects 1 argument".into());
            }
            match args[0] {
                Value::Int(n) => Ok(Value::Int(n)),
                Value::Float(f) => Ok(Value::Float(f.floor())),
                _ => Err("floor() expects a number".into()),
            }
        }
        BUILTIN_CEIL => {
            if args.len() != 1 {
                return Err("ceil() expects 1 argument".into());
            }
            match args[0] {
                Value::Int(n) => Ok(Value::Int(n)),
                Value::Float(f) => Ok(Value::Float(f.ceil())),
                _ => Err("ceil() expects a number".into()),
            }
        }
        BUILTIN_FLOAT => {
            if args.len() != 1 {
                return Err("float() expects 1 argument".into());
            }
            let f = to_float(&args[0])?;
            Ok(Value::Float(f))
        }
        BUILTIN_INT => {
            if args.len() != 1 {
                return Err("int() expects 1 argument".into());
            }
            match args[0] {
                Value::Int(n) => Ok(Value::Int(n)),
                Value::Float(f) => Ok(Value::Int(f as i64)),
                Value::String(id) => {
                    let s = heap.get_string(id);
                    s.parse::<i64>()
                        .map(Value::Int)
                        .map_err(|_| format!("Cannot convert '{}' to int", s))
                }
                _ => Err(format!("Cannot convert {} to int", args[0].type_name())),
            }
        }
        BUILTIN_RANDOM => {
            if args.len() != 2 {
                return Err("random() expects 2 arguments".into());
            }
            let min = to_float(&args[0])?;
            let max = to_float(&args[1])?;
            let pseudo = ((std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as f64)
                / 4_294_967_295.0)
                * (max - min)
                + min;
            Ok(Value::Float(pseudo))
        }
        BUILTIN_TYPE => {
            if args.len() != 1 {
                return Err("type() expects 1 argument".into());
            }
            let s = args[0].type_name().to_string();
            let id = heap.alloc_string(s);
            Ok(Value::String(id))
        }
        BUILTIN_APPEND => {
            if args.len() != 2 {
                return Err("append() expects 2 arguments".into());
            }
            match args[0] {
                Value::List(id) => {
                    heap.get_list_mut(id).push(args[1]);
                    Ok(Value::Nil)
                }
                _ => Err("append() expects a list".into()),
            }
        }
        BUILTIN_POP => {
            if args.len() != 1 {
                return Err("pop() expects 1 argument".into());
            }
            match args[0] {
                Value::List(id) => Ok(heap.get_list_mut(id).pop().unwrap_or(Value::Nil)),
                _ => Err("pop() expects a list".into()),
            }
        }
        BUILTIN_KEYS => {
            if args.len() != 1 {
                return Err("keys() expects 1 argument".into());
            }
            match args[0] {
                Value::Map(id) => {
                    let key_strings: Vec<String> =
                        heap.get_map(id).keys().cloned().collect();
                    let keys: Vec<Value> = key_strings
                        .into_iter()
                        .map(|k| {
                            let sid = heap.alloc_string(k);
                            Value::String(sid)
                        })
                        .collect();
                    let lid = heap.alloc_list(keys);
                    Ok(Value::List(lid))
                }
                _ => Err("keys() expects a record".into()),
            }
        }
        BUILTIN_VALUES => {
            if args.len() != 1 {
                return Err("values() expects 1 argument".into());
            }
            match args[0] {
                Value::Map(id) => {
                    let vals: Vec<Value> = heap.get_map(id).values().copied().collect();
                    let lid = heap.alloc_list(vals);
                    Ok(Value::List(lid))
                }
                _ => Err("values() expects a record".into()),
            }
        }
        BUILTIN_CONTAINS => {
            if args.len() != 2 {
                return Err("contains() expects 2 arguments".into());
            }
            match args[0] {
                Value::List(id) => {
                    let elems = heap.get_list(id);
                    let found = elems
                        .iter()
                        .any(|v| value::values_equal(v, &args[1], heap));
                    Ok(Value::Bool(found))
                }
                Value::String(id) => match args[1] {
                    Value::String(sub_id) => {
                        let s = heap.get_string(id);
                        let sub = heap.get_string(sub_id);
                        Ok(Value::Bool(s.contains(sub)))
                    }
                    _ => Err("contains() on string expects a string".into()),
                },
                _ => Err("contains() expects a list or string".into()),
            }
        }
        BUILTIN_MIN => {
            if args.len() != 2 {
                return Err("min() expects 2 arguments".into());
            }
            match compare_values(&args[0], &args[1], heap)? {
                std::cmp::Ordering::Less | std::cmp::Ordering::Equal => Ok(args[0]),
                std::cmp::Ordering::Greater => Ok(args[1]),
            }
        }
        BUILTIN_MAX => {
            if args.len() != 2 {
                return Err("max() expects 2 arguments".into());
            }
            match compare_values(&args[0], &args[1], heap)? {
                std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => Ok(args[0]),
                std::cmp::Ordering::Less => Ok(args[1]),
            }
        }
        BUILTIN_ROUND => {
            if args.len() != 1 {
                return Err("round() expects 1 argument".into());
            }
            match args[0] {
                Value::Int(n) => Ok(Value::Int(n)),
                Value::Float(f) => Ok(Value::Float(f.round())),
                _ => Err("round() expects a number".into()),
            }
        }
        _ => Err(format!("Unknown builtin ID: {:?}", id)),
    }
}

fn to_float(val: &Value) -> Result<f64, String> {
    match val {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(format!("Cannot convert {} to float", val.type_name())),
    }
}

pub fn compare_values(
    a: &Value,
    b: &Value,
    heap: &Heap,
) -> Result<std::cmp::Ordering, String> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => {
            Ok((*a as f64)
                .partial_cmp(b)
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Float(a), Value::Int(b)) => {
            Ok(a.partial_cmp(&(*b as f64))
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::String(a), Value::String(b)) => {
            Ok(heap.get_string(*a).cmp(heap.get_string(*b)))
        }
        _ => Err(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}
