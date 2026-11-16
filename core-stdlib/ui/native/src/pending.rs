//! Pending-resource observability hook for hosts.
//!
//! petal-ui does not render a pending overlay itself — that UI is an
//! integration / sample-app concern — but it exposes the per-frame pending
//! report in a structured form an integration can paint (e.g. "3 pending:
//! fetch(/api/user/7) · 12 frames · absorbed by 4 draw calls"). See
//! docs/dev/pending-values-plan.md §Observability.

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

/// The per-frame pending report for `stack`'s run: a JSON array of
/// `{ id, key, state, age_frames, origin, absorbed_count }` objects, one per
/// live resource (see [`petal::env::Env::pending_report`]). A host renders this
/// as a dev overlay; the overlay UI itself lives in the integration, not here.
pub fn pending_report(env: &Env, program: ProgramId, stack: StackKey) -> serde_json::Value {
    env.pending_report(program, stack)
}
