//! I/O and type-name builtins: print, str, type.

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
