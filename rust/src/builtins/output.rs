//! Symbol and buffered-output builtins.
//!
//! `symbol(name)` interns a name into a `Value::Symbol`, the binding key shared
//! with the embedding host. `push_output(sym, value)` appends a value into the
//! host-visible buffer bound to that symbol — the script-side counterpart to
//! `Env::take_output_buffer`. Native host functions (e.g. draw commands) use the
//! same channel via `PetalCxt::push_output`/`emit`.

use crate::native_fn::PetalCxt;
use crate::value::Value;

pub(super) fn native_symbol(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "symbol")?;
    let name = state.get_string(1)?;
    let id = state.intern_symbol(&name);
    state.push_value(Value::Symbol(id));
    Ok(1)
}

pub(super) fn native_push_output(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "push_output")?;
    let sym = state.get_symbol(1)?;
    let value = state.get_value(2)?;
    state.push_output(sym, value);
    state.push_nil();
    Ok(1)
}
