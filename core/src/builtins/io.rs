//! I/O and type-name builtins: print, str, type, assert, assert_eq.

use crate::native_fn::PetalCxt;
use crate::value;
use crate::value::Value;

pub(super) fn native_print(state: &mut PetalCxt) -> Result<u32, String> {
    let parts: Vec<String> = (1..=state.arg_count())
        .map(|i| {
            let v = state.get_value(i).unwrap();
            value::value_to_display_string(&v, state.heap())
        })
        .collect();
    let line = parts.join(" ");
    state.print(line);
    state.push_nil();
    Ok(1)
}

pub(super) fn native_str(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "str")?;
    let v = state.get_value(1)?;
    let s = value::value_to_display_string(&v, state.heap());
    state.push_string(s);
    Ok(1)
}

pub(super) fn native_type(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "type")?;
    let v = state.get_value(1)?;
    let name = type_name_id(state.heap_mut(), v);
    state.push_value(Value::String(name));
    Ok(1)
}

/// What `type(v)` answers, as an interned string. Shared with the VM, which
/// answers `type` without a native call (`Inst::Intrinsic`).
pub(crate) fn type_name_id(heap: &mut crate::heap::Heap, v: Value) -> crate::heap::StringId {
    // A class instance reports its class rather than the underlying `record`:
    // the class name is the more specific true answer, and it is what a
    // `type(x) == "Rect"` test wants. Untagged records still say "record".
    match v {
        Value::Map(id) => match heap.map_class(id) {
            Some(class) => class,
            None => heap.intern_str(v.type_name()),
        },
        // A variant reports its enum, as a class instance reports its class.
        Value::EnumVariant { tag, .. } => {
            match crate::program::split_enum_tag(heap.get_string(tag)).0 {
                Some(name) => {
                    let name = name.to_string();
                    heap.alloc_string(name)
                }
                None => heap.intern_str(v.type_name()),
            }
        }
        _ => heap.intern_str(v.type_name()),
    }
}

pub(super) fn native_assert(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if n != 1 && n != 2 {
        return Err("assert() expects 1 or 2 arguments".into());
    }
    let cond = state.get_value(1)?;
    let ok = match cond {
        crate::value::Value::Bool(b) => b,
        crate::value::Value::Nil => false,
        _ => true,
    };
    if !ok {
        return Err(if n == 2 {
            let m = state.get_value(2)?;
            format!(
                "assertion failed: {}",
                value::value_to_display_string(&m, state.heap())
            )
        } else {
            "assertion failed".to_string()
        });
    }
    state.push_nil();
    Ok(1)
}

pub(super) fn native_assert_eq(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "assert_eq")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    if !value::values_equal(&a, &b, state.heap()) {
        let a_str = value::value_to_display_string(&a, state.heap());
        let b_str = value::value_to_display_string(&b, state.heap());
        return Err(format!(
            "assertion failed: assert_eq: left={} right={}",
            a_str, b_str
        ));
    }
    state.push_nil();
    Ok(1)
}
