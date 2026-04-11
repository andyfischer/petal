//! List and record manipulation: range, len, push, append, pop, keys, values,
//! contains, sort, reverse, join, split, enumerate, zip, slice, flat.

use crate::native_fn::PetalCxt;
use crate::value::{self, Value};

use super::require_args;

pub(super) fn native_range(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    let (start, end) = match argc {
        1 => {
            let end = state.get_int(1)?;
            (0, end)
        }
        2 => {
            let start = state.get_int(1)?;
            let end = state.get_int(2)?;
            (start, end)
        }
        _ => {
            return Err("range() expects 1 or 2 arguments".to_string());
        }
    };
    let items: Vec<Value> = (start..end).map(Value::Int).collect();
    state.push_list(items);
    Ok(1)
}

pub(super) fn native_len(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "len")?;
    let v = state.get_value(1)?;
    match v {
        Value::List(id) => {
            state.push_int(state.heap().list_len(id) as i64);
            Ok(1)
        }
        Value::String(id) => {
            state.push_int(state.heap().get_string(id).len() as i64);
            Ok(1)
        }
        _ => Err(format!("Cannot get length of {}", v.type_name())),
    }
}

pub(super) fn native_push(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "push")?;
    let list = state.get_value(1)?;
    let val = state.get_value(2)?;
    match list {
        Value::List(id) => {
            state.heap_mut().get_list_mut(id).push(val);
            state.push_nil();
            Ok(1)
        }
        _ => Err("push() expects a list as first argument".into()),
    }
}

pub(super) fn native_append(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "append")?;
    let list = state.get_value(1)?;
    let val = state.get_value(2)?;
    match list {
        Value::List(id) => {
            state.heap_mut().get_list_mut(id).push(val);
            state.push_nil();
            Ok(1)
        }
        _ => Err("append() expects a list".into()),
    }
}

pub(super) fn native_pop(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "pop")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let v = state.heap_mut().get_list_mut(id).pop().unwrap_or(Value::Nil);
            state.push_value(v);
            Ok(1)
        }
        _ => Err("pop() expects a list".into()),
    }
}

pub(super) fn native_keys(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "keys")?;
    match state.get_value(1)? {
        Value::Map(id) => {
            let key_strings: Vec<String> = state.heap().get_map(id).keys().cloned().collect();
            let keys: Vec<Value> = key_strings
                .into_iter()
                .map(|k| {
                    let sid = state.heap_mut().alloc_string(k);
                    Value::String(sid)
                })
                .collect();
            state.push_list(keys);
            Ok(1)
        }
        _ => Err("keys() expects a record".into()),
    }
}

pub(super) fn native_values(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "values")?;
    match state.get_value(1)? {
        Value::Map(id) => {
            let vals: Vec<Value> = state.heap().get_map(id).values().copied().collect();
            state.push_list(vals);
            Ok(1)
        }
        _ => Err("values() expects a record".into()),
    }
}

pub(super) fn native_contains(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "contains")?;
    let container = state.get_value(1)?;
    let needle = state.get_value(2)?;
    match container {
        Value::List(id) => {
            let elems = state.heap().get_list(id);
            let found = elems
                .iter()
                .any(|v| value::values_equal(v, &needle, state.heap()));
            state.push_bool(found);
            Ok(1)
        }
        Value::String(id) => match needle {
            Value::String(sub_id) => {
                let s = state.heap().get_string(id).to_string();
                let sub = state.heap().get_string(sub_id).to_string();
                state.push_bool(s.contains(&sub));
                Ok(1)
            }
            _ => Err("contains() on string expects a string".into()),
        },
        _ => Err("contains() expects a list or string".into()),
    }
}

/// Sort key extracted from Values so sorting doesn't need heap access.
#[derive(PartialEq, PartialOrd)]
enum SortKey {
    Num(f64),
    Str(String),
    Other,
}

impl Eq for SortKey {}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

pub(super) fn native_sort(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "sort")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            // Build sort keys: extract string content and numeric values up front
            // so the sort closure doesn't need heap access.
            let mut keyed: Vec<(SortKey, Value)> = items.into_iter()
                .map(|v| {
                    let key = match v {
                        Value::Int(n) => SortKey::Num(n as f64),
                        Value::Float(f) => SortKey::Num(f),
                        Value::Dual { value, .. } => SortKey::Num(value),
                        Value::String(sid) => SortKey::Str(state.heap().get_string(sid).to_string()),
                        _ => SortKey::Other,
                    };
                    (key, v)
                })
                .collect();
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
            let sorted: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
            state.push_list(sorted);
            Ok(1)
        }
        _ => Err("sort() expects a list".into()),
    }
}

pub(super) fn native_reverse(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "reverse")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let mut items: Vec<Value> = state.heap().get_list(id).to_vec();
            items.reverse();
            state.push_list(items);
            Ok(1)
        }
        Value::String(id) => {
            let s: String = state.heap().get_string(id).chars().rev().collect();
            state.push_string(s);
            Ok(1)
        }
        _ => Err("reverse() expects a list or string".into()),
    }
}

pub(super) fn native_join(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "join")?;
    let list = state.get_value(1)?;
    let sep = state.get_value(2)?;
    match (list, sep) {
        (Value::List(list_id), Value::String(sep_id)) => {
            let separator = state.heap().get_string(sep_id).to_string();
            let elements = state.heap().get_list(list_id);
            let parts: Vec<String> = elements.iter()
                .map(|v| value::value_to_display_string(v, state.heap()))
                .collect();
            let result = parts.join(&separator);
            state.push_string(result);
            Ok(1)
        }
        _ => Err("join() expects (list, string)".into()),
    }
}

pub(super) fn native_split(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "split")?;
    let s = state.get_value(1)?;
    let sep = state.get_value(2)?;
    match (s, sep) {
        (Value::String(s_id), Value::String(sep_id)) => {
            let string = state.heap().get_string(s_id).to_string();
            let separator = state.heap().get_string(sep_id).to_string();
            let parts: Vec<Value> = string.split(&separator)
                .map(|part| {
                    let id = state.heap_mut().alloc_string(part.to_string());
                    Value::String(id)
                })
                .collect();
            state.push_list(parts);
            Ok(1)
        }
        _ => Err("split() expects (string, string)".into()),
    }
}

pub(super) fn native_enumerate(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "enumerate")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            let pairs: Vec<Value> = items.into_iter().enumerate()
                .map(|(i, v)| {
                    let pair = vec![Value::Int(i as i64), v];
                    let pair_id = state.heap_mut().alloc_list(pair);
                    Value::List(pair_id)
                })
                .collect();
            state.push_list(pairs);
            Ok(1)
        }
        _ => Err("enumerate() expects a list".into()),
    }
}

pub(super) fn native_zip(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "zip")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match (a, b) {
        (Value::List(a_id), Value::List(b_id)) => {
            let a_items = state.heap().get_list(a_id).to_vec();
            let b_items = state.heap().get_list(b_id).to_vec();
            let pairs: Vec<Value> = a_items.into_iter().zip(b_items)
                .map(|(x, y)| {
                    let pair = vec![x, y];
                    let pair_id = state.heap_mut().alloc_list(pair);
                    Value::List(pair_id)
                })
                .collect();
            state.push_list(pairs);
            Ok(1)
        }
        _ => Err("zip() expects two lists".into()),
    }
}

pub(super) fn native_slice(state: &mut PetalCxt) -> Result<u32, String> {
    if !(2..=3).contains(&state.arg_count()) {
        return Err("slice() expects 2-3 arguments".into());
    }
    let list = state.get_value(1)?;
    let start = state.get_int(2)?;
    match list {
        Value::List(id) => {
            let items = state.heap().get_list(id);
            let len = items.len() as i64;
            let start_idx = if start < 0 { (len + start).max(0) as usize } else { start.min(len) as usize };
            let end_idx = if state.arg_count() == 3 {
                let end = state.get_int(3)?;
                if end < 0 { (len + end).max(0) as usize } else { end.min(len) as usize }
            } else {
                len as usize
            };
            let sliced = if start_idx <= end_idx {
                items[start_idx..end_idx].to_vec()
            } else {
                Vec::new()
            };
            state.push_list(sliced);
            Ok(1)
        }
        Value::String(id) => {
            let s = state.heap().get_string(id);
            let len = s.len() as i64;
            let start_idx = if start < 0 { (len + start).max(0) as usize } else { start.min(len) as usize };
            let end_idx = if state.arg_count() == 3 {
                let end = state.get_int(3)?;
                if end < 0 { (len + end).max(0) as usize } else { end.min(len) as usize }
            } else {
                len as usize
            };
            let sliced = if start_idx <= end_idx {
                s[start_idx..end_idx].to_string()
            } else {
                String::new()
            };
            state.push_string(sliced);
            Ok(1)
        }
        _ => Err("slice() expects a list or string".into()),
    }
}

pub(super) fn native_flat(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "flat")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            let mut result = Vec::new();
            for item in items {
                match item {
                    Value::List(inner_id) => {
                        let inner = state.heap().get_list(inner_id).to_vec();
                        result.extend(inner);
                    }
                    _ => result.push(item),
                }
            }
            state.push_list(result);
            Ok(1)
        }
        _ => Err("flat() expects a list".into()),
    }
}
