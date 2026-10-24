//! Direct-manipulation hints: `tunable` and `rigid`
//! (docs/proposals/dm-v2-hints.md, docs/dev/direct-manipulation-v2-plan.md
//! §2.6).
//!
//! Both are the identity on their first argument at run time: a dual number
//! passes through unchanged, and the bounds of `tunable` are evaluated by the
//! call and ignored. What they mean is static: `crate::dm_v2::hints` reads
//! them from the IR to decide which literals a drag may move. In a
//! speculative run the gate (`vm::native::call_native_spec`) replays the
//! baseline's answer of `rigid`, so nothing upstream of it receives feedback
//! through it.

use crate::native_fn::PetalCxt;

/// `tunable(value, min?, max?, step?)`: `value`.
pub(super) fn native_tunable(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if !(1..=4).contains(&n) {
        return Err(format!("tunable() expects 1 to 4 arguments, got {n}"));
    }
    let v = state.get_value(1)?;
    state.push_value(v);
    Ok(1)
}

/// `rigid(value)`: `value`.
pub(super) fn native_rigid(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "rigid")?;
    let v = state.get_value(1)?;
    state.push_value(v);
    Ok(1)
}
