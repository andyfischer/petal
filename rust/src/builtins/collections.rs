//! List and record manipulation: range, len, push, append, pop, keys, values,
//! contains, sort, reverse, join, split, enumerate, zip, slice, flat.

use crate::native_fn::PetalCxt;
use crate::value::{self, Value};

use super::require_args;

/// The leftmost `Value::Pending` element of `items`, if any. Aggregates that
/// must read every element (`sort`, `join`) absorb it and return that Pending —
/// the whole result is unknown while one element is unresolved. Element-wise
/// operations (`len`, indexing, `map`) leave Pending elements in place instead.
fn leftmost_pending_element(items: &[Value]) -> Option<Value> {
    items.iter().copied().find(|v| matches!(v, Value::Pending(_)))
}

/// Bounds-check a signed index against an f64-array length, returning the
/// validated `usize` or the standard out-of-bounds error.
fn checked_f64_index(i: i64, len: usize) -> Result<usize, String> {
    if i < 0 || i as usize >= len {
        return Err(format!(
            "Index {} out of bounds for f64_array of length {}",
            i, len
        ));
    }
    Ok(i as usize)
}

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
        Value::F64Array(id) => {
            state.push_int(state.heap().f64_array_len(id) as i64);
            Ok(1)
        }
        Value::String(id) => {
            state.push_int(state.heap().get_string(id).len() as i64);
            Ok(1)
        }
        _ => Err(format!("Cannot get length of {}", v.type_name())),
    }
}

pub(super) fn native_f64_array(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "f64_array")?;
    let n = state.get_int(1)?;
    if n < 0 {
        return Err("f64_array() expects a non-negative length".to_string());
    }
    let id = state.heap_mut().alloc_f64_array(vec![0.0_f64; n as usize]);
    state.push_value(Value::F64Array(id));
    Ok(1)
}

pub(super) fn native_get(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "get")?;
    let container = state.get_value(1)?;
    match container {
        Value::F64Array(id) => {
            let i = state.get_int(2)?;
            let data = state.heap().get_f64_array(id);
            let idx = checked_f64_index(i, data.len())?;
            let v = data[idx];
            state.push_float(v);
            Ok(1)
        }
        _ => Err(format!("Cannot get() from {}", container.type_name())),
    }
}

/// `set(arr, i, v)` returns a NEW f64-array with index `i` set to `v`. The
/// input array is never mutated (value semantics) — callers must rebind:
/// `a = set(a, i, v)`. (Equivalent to the `a[i] = v` index-assign form.)
pub(super) fn native_set(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "set")?;
    let container = state.get_value(1)?;
    match container {
        Value::F64Array(id) => {
            let i = state.get_int(2)?;
            let v = match state.get_value(3)? {
                Value::Float(f) => f,
                Value::Int(n) => n as f64,
                other => {
                    return Err(format!(
                        "set() expects a number value, got {}",
                        other.type_name()
                    ))
                }
            };
            let idx = checked_f64_index(i, state.heap().f64_array_len(id))?;
            let new_id = if state.in_place() {
                state.heap_mut().f64_array_set_in_place(id, idx, v)
            } else {
                state.heap_mut().f64_array_set(id, idx, v)
            };
            state.push_value(Value::F64Array(new_id));
            Ok(1)
        }
        _ => Err(format!("Cannot set() on {}", container.type_name())),
    }
}

/// `swap(arr, i, j)` returns a NEW f64-array with elements `i` and `j` swapped.
/// The input array is never mutated (value semantics) — callers must rebind:
/// `a = swap(a, i, j)`.
pub(super) fn native_swap(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "swap")?;
    let container = state.get_value(1)?;
    match container {
        Value::F64Array(id) => {
            let i = state.get_int(2)?;
            let j = state.get_int(3)?;
            let len = state.heap().f64_array_len(id);
            let i = checked_f64_index(i, len)?;
            let j = checked_f64_index(j, len)?;
            let new_id = if state.in_place() {
                state.heap_mut().f64_array_swap_in_place(id, i, j)
            } else {
                state.heap_mut().f64_array_swap(id, i, j)
            };
            state.push_value(Value::F64Array(new_id));
            Ok(1)
        }
        _ => Err(format!("Cannot swap() on {}", container.type_name())),
    }
}

/// `append(list, val)` returns a NEW list with `val` added to the end. The
/// input list is never mutated (value semantics): `let b = append(a, x)` leaves
/// `a` unchanged. Use it as `xs = append(xs, x)` to grow an accumulator.
pub(super) fn native_append(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "append")?;
    let list = state.get_value(1)?;
    let val = state.get_value(2)?;
    match list {
        Value::List(id) => {
            let new_id = if state.in_place() {
                state.heap_mut().list_append_in_place(id, val)
            } else {
                state.heap_mut().list_append(id, val)
            };
            state.push_value(Value::List(new_id));
            Ok(1)
        }
        _ => Err("append() expects a list".into()),
    }
}

/// Deprecated alias for `append`. Kept temporarily so existing scripts keep
/// compiling while they migrate to `xs = append(xs, x)`. Like `append`, it is
/// immutable and returns a new list — statement-form `push(xs, x)` no longer
/// mutates `xs`, so callers must capture the result.
pub(super) fn native_push(state: &mut PetalCxt) -> Result<u32, String> {
    native_append(state)
}

/// Deprecated immutable alias for `drop_last`. Kept so existing scripts keep
/// compiling while they migrate. Under value semantics `pop` no longer mutates
/// the list nor returns the removed element — it returns a NEW list with the
/// last element dropped. Use `last(xs)` to read the final element and
/// `drop_last(xs)` (or `xs = drop_last(xs)`) to shorten the list.
pub(super) fn native_pop(state: &mut PetalCxt) -> Result<u32, String> {
    native_drop_last(state)
}

/// `last(list)` returns the final element of `list` (or Nil if empty). Pure
/// read — the list is never mutated.
pub(super) fn native_last(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "last")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let v = state.heap().get_list(id).last().copied().unwrap_or(Value::Nil);
            state.push_value(v);
            Ok(1)
        }
        _ => Err("last() expects a list".into()),
    }
}

/// `drop_last(list)` returns a NEW list equal to `list` without its last
/// element (value semantics). The input list is never mutated; an empty list
/// yields a new empty list.
pub(super) fn native_drop_last(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "drop_last")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let new_id = if state.in_place() {
                state.heap_mut().list_drop_last_in_place(id)
            } else {
                state.heap_mut().list_drop_last(id)
            };
            state.push_value(Value::List(new_id));
            Ok(1)
        }
        _ => Err("drop_last() expects a list".into()),
    }
}

/// `remove(map, key)` returns a NEW map equal to `map` without `key` (value
/// semantics). The input map is never mutated; removing an absent key yields an
/// equivalent new map.
pub(super) fn native_remove(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "remove")?;
    let container = state.get_value(1)?;
    match container {
        Value::Map(id) => {
            let key = match state.get_value(2)? {
                Value::String(sid) => state.heap().get_string(sid).to_string(),
                other => {
                    return Err(format!(
                        "remove() expects a string key, got {}",
                        other.type_name()
                    ))
                }
            };
            let new_id = if state.in_place() {
                state.heap_mut().map_remove_in_place(id, &key)
            } else {
                state.heap_mut().map_remove(id, &key)
            };
            state.push_value(Value::Map(new_id));
            Ok(1)
        }
        _ => Err(format!("Cannot remove() from {}", container.type_name())),
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
#[derive(PartialEq)]
enum SortKey {
    Num(f64),
    Str(String),
    Other,
}

impl Eq for SortKey {}

impl SortKey {
    /// Rank for ordering keys of different kinds: numbers, then strings, then
    /// everything else.
    fn rank(&self) -> u8 {
        match self {
            SortKey::Num(_) => 0,
            SortKey::Str(_) => 1,
            SortKey::Other => 2,
        }
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            // NaN is incomparable; treat it as equal so the sort stays total.
            (SortKey::Num(a), SortKey::Num(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) fn native_sort(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "sort")?;
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            // A Pending element has no orderable key; absorb it (element-wise
            // ops keep it, but sorting needs every key).
            if let Some(p) = leftmost_pending_element(&items) {
                if let Value::Pending(id) = p {
                    state.resources_mut().note_absorbed(id);
                }
                state.push_value(p);
                return Ok(1);
            }
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
            // A Pending element makes the whole joined string unknown; absorb it.
            let pending = leftmost_pending_element(state.heap().get_list(list_id));
            if let Some(p) = pending {
                if let Value::Pending(id) = p {
                    state.resources_mut().note_absorbed(id);
                }
                state.push_value(p);
                return Ok(1);
            }
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

/// Largest char boundary `<= i` (clamped to the string length). Used to snap
/// a byte index down onto a UTF-8 boundary so String slicing never panics.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary `>= i` (clamped to the string length).
fn ceil_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
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
            // Indices are byte offsets (matching byte-indexed len()). A byte
            // that lands inside a multi-byte char would panic String slicing,
            // so snap to char boundaries: start up, end down, keeping only
            // whole chars and never exceeding the requested range.
            let start_idx = ceil_char_boundary(s, start_idx);
            let end_idx = floor_char_boundary(s, end_idx);
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
