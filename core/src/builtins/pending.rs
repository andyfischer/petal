//! Test-only pending-resource builtins: `__pending`, `__resolve`, `__reject`.
//!
//! These deterministically drive the resource table (no real I/O) so the
//! pending-value semantics can be exercised in tests. Names are `__`-prefixed to
//! read as internal. See docs/dev/pending-values-plan.md (roadmap step 1) — real
//! fetchers arrive with `petal-query`.

use std::hash::{Hash, Hasher};

use crate::native_fn::PetalCxt;
use crate::resource_table::ResourceState;
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
        table
            .pending_for_key(key)
            .and_then(|id| table.value_for(id))
    };
    if let Some(v) = ready {
        state.push_value(v);
        return Ok(1);
    }
    // Stamp the resource with the requesting call site and current frame so the
    // observability tooling can render its provenance and age.
    let origin = state.origin();
    let frame = state.frame();
    let id = state
        .resources_mut()
        .get_or_create_loading(key, origin, frame);
    state.push_value(Value::Pending(id));
    Ok(1)
}

/// `__resolve(key, value)` — mark `key`'s resource `Ready(value)` (creating the
/// entry if needed). Returns nil.
pub(super) fn native_resolve(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "__resolve")?;
    let key = hash_key(&state.get_string(1)?);
    let value = state.get_value(2)?;
    let frame = state.frame();
    state.resources_mut().resolve(key, value, frame);
    state.push_nil();
    Ok(1)
}

/// `__reject(key, error)` — mark `key`'s resource `Errored(error)` (creating the
/// entry if needed). Returns nil.
pub(super) fn native_reject(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "__reject")?;
    let key = hash_key(&state.get_string(1)?);
    let error = state.get_value(2)?;
    let frame = state.frame();
    state.resources_mut().reject(key, error, frame);
    state.push_nil();
    Ok(1)
}

// --- Chunk D: pending meta builtins ---
//
// These are the ONLY sanctioned way to inspect pending-ness (everything else
// absorbs a Pending). Each MUST be registered `AllowPending` (see builtins/mod.rs)
// or a Strict registration would absorb the Pending arg before it ever reached
// the native. A `Value::Pending` at the language level is always Loading or
// Errored — a Ready resource surfaces as its real value via `__pending`.

/// `is_loading(x)` — true iff `x` is a `Value::Pending` whose entry is `Loading`.
pub(super) fn native_is_loading(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "is_loading")?;
    let x = state.get_value(1)?;
    let result = match x {
        Value::Pending(id) => matches!(state.resources().entry(id).state, ResourceState::Loading),
        _ => false,
    };
    state.push_bool(result);
    Ok(1)
}

/// `is_error(x)` — true iff `x` is a `Value::Pending` whose entry is `Errored`.
pub(super) fn native_is_error(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "is_error")?;
    let x = state.get_value(1)?;
    let result = match x {
        Value::Pending(id) => {
            matches!(state.resources().entry(id).state, ResourceState::Errored(_))
        }
        _ => false,
    };
    state.push_bool(result);
    Ok(1)
}

/// `is_pending(x)` — true iff `x` is a `Value::Pending` at all (Loading OR
/// Errored). The general "not resolved" check.
pub(super) fn native_is_pending(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "is_pending")?;
    let x = state.get_value(1)?;
    state.push_bool(matches!(x, Value::Pending(_)));
    Ok(1)
}

/// `is_ready(x)` — true iff `x` is NOT a `Value::Pending` (a usable resolved
/// value). Equivalent to `!is_pending(x)`.
pub(super) fn native_is_ready(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "is_ready")?;
    let x = state.get_value(1)?;
    state.push_bool(!matches!(x, Value::Pending(_)));
    Ok(1)
}

/// `error_of(x)` — the stored error `Value` if `x` is a `Pending`+`Errored`;
/// otherwise `Nil`.
pub(super) fn native_error_of(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "error_of")?;
    let x = state.get_value(1)?;
    let result = match x {
        Value::Pending(id) => match state.resources().entry(id).state {
            ResourceState::Errored(v) => v,
            _ => Value::Nil,
        },
        _ => Value::Nil,
    };
    state.push_value(result);
    Ok(1)
}

/// `or_else(x, default)` — eager fallback: return `default` if `x` is a
/// `Value::Pending` (loading OR errored); otherwise return `x`. Both args are
/// already evaluated (this is NOT the short-circuit `??` operator).
pub(super) fn native_or_else(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "or_else")?;
    let x = state.get_value(1)?;
    let default = state.get_value(2)?;
    let result = match x {
        Value::Pending(_) => default,
        _ => x,
    };
    state.push_value(result);
    Ok(1)
}

/// `resource_key(x)` — for tooling/dedup. If `x` is a `Value::Pending`, its
/// resource cache key as an `Int` (the `u64` key cast to `i64`); otherwise
/// `Nil`. Two pendings for the same key return an equal `resource_key`.
pub(super) fn native_resource_key(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "resource_key")?;
    let x = state.get_value(1)?;
    match x {
        Value::Pending(id) => {
            let key = state.resources().entry(id).key;
            state.push_int(key as i64);
        }
        _ => state.push_nil(),
    }
    Ok(1)
}
