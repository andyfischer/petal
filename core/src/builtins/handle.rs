//! Handle-related builtins (docs/game-engine-features/handles.md).

use crate::native_fn::{NativeResult, PetalCxt};
use crate::value::Value;

/// `is_valid(v)` — true iff `v` is a handle of a registered class whose
/// liveness predicate passes. Never errors: nil, non-handles, and stale
/// handles are all simply not valid (that's the builtin's purpose — scripts
/// use it to guard against host-side object churn).
///
/// Liveness is host data the runtime cannot see change, so the call is
/// declared a host-data read (`NativeEffects::reads(HOST_DATA)`) and says so
/// at runtime too: a frame that asked is re-run when the host reports a
/// change, and a memoized scope that asked is not replayed across one.
pub fn native_is_valid(cxt: &mut PetalCxt) -> NativeResult {
    cxt.note_host_read();
    let valid = match cxt.get_value(1) {
        Ok(Value::Handle(h)) => cxt
            .handle_class(h.class)
            .map(|class| (class.is_valid)(h.slot, h.serial))
            .unwrap_or(false),
        _ => false,
    };
    cxt.push_bool(valid);
    Ok(1)
}
