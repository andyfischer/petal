//! Test-only pending-resource builtins: `__pending`, `__resolve`, `__reject`.
//!
//! These deterministically drive the resource table (no real I/O) so the
//! pending-value semantics can be exercised in tests. Names are `__`-prefixed to
//! read as internal. See docs/dev/pending-values-plan.md (roadmap step 1) — real
//! fetchers arrive with `petal-query`.

use std::hash::{Hash, Hasher};

use crate::native_fn::PetalCxt;
use crate::value::Value;

/// Hash a string cache key to the `u64` the resource table is keyed by.
fn hash_key(key: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// `__pending(key)` — fetch a resource by string key. If its entry is already
/// `Ready`, return the real value (models a resource that landed between
/// frames). Otherwise create/return a `Loading` entry as a `Value::Pending`. An
/// `Errored` entry stays pending (errored is a pending-kind value).
pub(super) fn native_pending(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "__pending")?;
    let key = hash_key(&state.get_string(1)?);
    // Ready → the resolved value; Loading/Errored → the Pending id.
    let ready = {
        let table = state.resources();
        table.pending_for_key(key).and_then(|id| table.value_for(id))
    };
    if let Some(v) = ready {
        state.push_value(v);
        return Ok(1);
    }
    let id = state.resources_mut().get_or_create_loading(key);
    state.push_value(Value::Pending(id));
    Ok(1)
}

/// `__resolve(key, value)` — mark `key`'s resource `Ready(value)` (creating the
/// entry if needed). Returns nil.
pub(super) fn native_resolve(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "__resolve")?;
    let key = hash_key(&state.get_string(1)?);
    let value = state.get_value(2)?;
    state.resources_mut().resolve(key, value);
    state.push_nil();
    Ok(1)
}

/// `__reject(key, error)` — mark `key`'s resource `Errored(error)` (creating the
/// entry if needed). Returns nil.
pub(super) fn native_reject(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "__reject")?;
    let key = hash_key(&state.get_string(1)?);
    let error = state.get_value(2)?;
    state.resources_mut().reject(key, error);
    state.push_nil();
    Ok(1)
}
