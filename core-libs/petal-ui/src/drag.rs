//! The host side of `ui/drag` (`prelude/drag.ptl`, `docs/drag.md`): a drop is
//! announced on the [`DRAG_DROPS_CHANNEL`] output channel as
//! `{key, model, drop}`, and a host that commits drops (Garden writes a dropped
//! model back to the source) drains it with [`take_drops`] after each run.
//! Reading a drag in flight from the outside is headless tooling, in the
//! `petal-ui-headless` crate's `drag_trace`.

use petal::env::Env;
use serde_json::Value as Json;

/// The output channel `ui/drag` announces a drop on (`DRAG_DROPS_CHANNEL` in
/// the module).
pub const DRAG_DROPS_CHANNEL: &str = "drag_drops";

/// Drain the drops announced since the last call: `{key, model, drop}` each.
pub fn take_drops(env: &mut Env) -> Vec<Json> {
    let sym = env.intern_symbol(DRAG_DROPS_CHANNEL);
    let values = env.take_output_buffer(sym);
    let heap = env.heap();
    values
        .iter()
        .map(|v| petal::value::value_to_json(v, heap))
        .collect()
}
