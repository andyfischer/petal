//! I/O and type-name builtins: print, str, type, assert, assert_eq.

use crate::native_fn::PetalCxt;
use crate::value;

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
    state.push_string(v.type_name().to_string());
    Ok(1)
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
