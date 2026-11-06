//! `get_in(value, path)` and `set_in(value, path, v)`: reading and replacing a
//! value nested inside records, lists and maps, with the path given as data.
//!
//! `set_in` is the expression form of a path assignment: `set_in(st,
//! ["meetings", 2, "room"], 3)` is the value `st` would hold after
//! `st.meetings[2].room = 3`, and `st` itself is unchanged (value semantics).
//! It exists for code that builds many candidate states from one model — a
//! drag library's `moves`, a solver's levers — where a `for` loop rebinding a
//! copy is easy to get wrong (docs/proposals/declarative-dragging.md, P3).
//!
//! Each step is a field name (a string) or a list index (an int; negative
//! counts from the end), or any key for a `Map`. Every step goes through the
//! same `ops::get_index` / `ops::set_index` the `x[k]` and `x[k] = v` forms
//! use, so the two cannot drift apart. An error names the step that failed,
//! both by its position and as the path written in Petal syntax up to it.

use crate::backend::ops;
use crate::heap::Heap;
use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

/// The path argument's steps, or the standard "expects a list" error.
fn path_steps(state: &PetalCxt, name: &str) -> Result<Vec<Value>, String> {
    match state.get_value(2)? {
        Value::List(id) => Ok(state.heap().get_list(id).to_vec()),
        other => Err(format!(
            "{name}() expects its path to be a list of field names and indices, got {}",
            other.type_name()
        )),
    }
}

/// The path up to and including step `upto` (0-based), written the way a
/// program reads it: `.meetings[2].room`. A string that is not a plain name
/// is written as a computed key (`["two words"]`).
fn describe(heap: &Heap, steps: &[Value], upto: usize) -> String {
    let mut out = String::new();
    for step in &steps[..=upto] {
        match step {
            Value::String(id) => {
                let s = heap.get_string(*id);
                let plain = !s.is_empty()
                    && !s.starts_with(|c: char| c.is_ascii_digit())
                    && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if plain {
                    out.push('.');
                    out.push_str(s);
                } else {
                    out.push_str(&format!("[{s:?}]"));
                }
            }
            Value::Int(i) => out.push_str(&format!("[{i}]")),
            other => out.push_str(&format!("[<{}>]", other.type_name())),
        }
    }
    out
}

/// A step's error, naming the step: `set_in(): path step 2 of 3
/// (.meetings[5]): Index 5 out of bounds (len 3)`.
fn step_error(name: &str, heap: &Heap, steps: &[Value], i: usize, err: String) -> String {
    format!(
        "{name}(): path step {} of {} ({}): {err}",
        i + 1,
        steps.len(),
        describe(heap, steps, i)
    )
}

/// The error for stepping into nil at step `i`.
fn nil_at(heap: &Heap, steps: &[Value], i: usize) -> String {
    if i == 0 {
        "the value is nil".to_string()
    } else {
        format!("{} is nil", describe(heap, steps, i - 1))
    }
}

/// Only strings and ints address a record or a list; anything else is a
/// `Map` key, which `get_index` / `set_index` judge for themselves.
fn read_step(
    name: &str,
    heap: &mut Heap,
    steps: &[Value],
    i: usize,
    obj: Value,
) -> Result<Value, String> {
    if matches!(obj, Value::Nil) {
        return Err(step_error(name, heap, steps, i, nil_at(heap, steps, i)));
    }
    ops::get_index(heap, obj, steps[i], false).map_err(|e| step_error(name, heap, steps, i, e))
}

/// `get_in(value, path)`: the value at `path`. An empty path is `value`.
pub(super) fn native_get_in(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "get_in")?;
    let mut cur = state.get_value(1)?;
    let steps = path_steps(state, "get_in")?;
    for i in 0..steps.len() {
        if matches!(cur, Value::Pending(_)) {
            break;
        }
        cur = read_step("get_in", state.heap_mut(), &steps, i, cur)?;
    }
    state.push_value(cur);
    Ok(1)
}

/// `set_in(value, path, v)`: a copy of `value` with the slot at `path`
/// replaced by `v`. Every container on the way must exist; the last step may
/// add a field to a record or a key to a map (as `r.f = v` does), but a list
/// index must be in bounds. An empty path is `v`.
pub(super) fn native_set_in(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "set_in")?;
    let root = state.get_value(1)?;
    let steps = path_steps(state, "set_in")?;
    let v = state.get_value(3)?;
    if steps.is_empty() {
        state.push_value(v);
        return Ok(1);
    }
    let val = rebuild(state.heap_mut(), root, &steps, v)?;
    state.push_value(val);
    Ok(1)
}

fn rebuild(heap: &mut Heap, root: Value, steps: &[Value], v: Value) -> Result<Value, String> {
    // The spine: spine[k] is the container step k indexes into.
    let mut spine = Vec::with_capacity(steps.len());
    let mut cur = root;
    for i in 0..steps.len() {
        if let Value::Pending(_) = cur {
            // Not there yet: the result is not known either.
            return Ok(cur);
        }
        spine.push(cur);
        if i + 1 < steps.len() {
            cur = read_step("set_in", heap, steps, i, cur)?;
        }
    }
    // Rebuild from the leaf up, each level a new container (value semantics).
    let mut val = v;
    for i in (0..steps.len()).rev() {
        let obj = spine[i];
        if matches!(obj, Value::Nil) {
            return Err(step_error("set_in", heap, steps, i, nil_at(heap, steps, i)));
        }
        val = ops::set_index(heap, obj, steps[i], val)
            .map_err(|e| step_error("set_in", heap, steps, i, e))?;
    }
    Ok(val)
}
