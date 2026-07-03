//! Handle-related builtins (docs/dev/unreal-ffi-proposal.md §5).

use crate::native_fn::{NativeResult, PetalCxt};
use crate::value::Value;

/// `is_valid(v)` — true iff `v` is a handle of a registered class whose
/// liveness predicate passes. Never errors: nil, non-handles, and stale
/// handles are all simply not valid (that's the builtin's purpose — scripts
/// use it to guard against host-side object churn).
pub fn native_is_valid(cxt: &mut PetalCxt) -> NativeResult {
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
