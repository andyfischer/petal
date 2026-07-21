//! State ↔ JSON: serializing a stack's committed state variables to a JSON map
//! keyed by variable name, and setting a named state variable from JSON.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors.

use super::*;

impl Env {
    /// Serialize all state variables to a JSON map keyed by variable name.
    /// Per-iteration state entries are suffixed with their loop indices.
    pub fn get_state_json(
        &self,
        program_id: ProgramId,
        stack_id: StackKey,
    ) -> serde_json::Map<String, serde_json::Value> {
        let names = self.state_key_names(program_id);
        // Resolve the stack's *own* context heap: a fork's state ids index its
        // forked heap, not the default context's.
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let ctx = self.ctx(ck);
        let heap = &ctx.heap;
        // Context for provenance-rich pending rendering: a pending state var
        // dumps as a structured `{ type:"pending", … }` object, not `"<pending>"`.
        let pending_ctx =
            self.get_program(program_id)
                .map(|program| crate::value::PendingJsonCtx {
                    resources: &ctx.resources,
                    program,
                    frame: ctx.frame(),
                });
        let mut map = serde_json::Map::new();
        if let Some(state) = self.get_all_state(stack_id) {
            for (key, val) in state {
                let base_name = names
                    .get(&key.base)
                    .cloned()
                    .unwrap_or_else(|| format!("unknown_{}", key.base.0));
                let name = if key.loop_indices.is_empty() {
                    base_name
                } else {
                    let suffix: Vec<String> = key
                        .loop_indices
                        .iter()
                        .map(|p| match p {
                            crate::stack::LoopKeyPart::Index(i) => i.to_string(),
                            crate::stack::LoopKeyPart::Explicit(h) => format!("k{}", h),
                        })
                        .collect();
                    format!("{}[{}]", base_name, suffix.join(","))
                };
                map.insert(
                    name,
                    crate::value::value_to_json_ctx(val, heap, pending_ctx.as_ref()),
                );
            }
        }
        map
    }

    /// Set a top-level state variable by name from a JSON value.
    pub fn set_state_from_json(
        &mut self,
        program_id: ProgramId,
        stack_id: StackKey,
        name: &str,
        json_val: &serde_json::Value,
    ) -> Result<(), String> {
        let names = self.state_key_names(program_id);
        let state_key = names
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(k, _)| *k)
            .ok_or_else(|| format!("No state variable named '{}'", name))?;

        // Allocate the value into the stack's own context heap so a fork's
        // state stays self-consistent (its ids must index its forked heap).
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        let val = crate::value::json_to_value(json_val, &mut self.ctx_mut(ck).heap)?;
        self.set_state(stack_id, state_key, val);
        Ok(())
    }

    /// Restore a whole state map (as produced by [`Self::get_state_json`]) into a
    /// stack, applying each entry via [`Self::set_state_from_json`]. Keys that
    /// fail — an unknown top-level name, or a value that cannot be reconstructed —
    /// are skipped and the rest still apply, so a partially-compatible screen
    /// restores what it can. Returns the number of keys successfully applied.
    ///
    /// Two documented v1 limitations:
    /// - Loop-indexed entries (serialized as `name[i]` by `get_state_json`) are
    ///   not addressable: `set_state_from_json` matches top-level names only, so
    ///   they are silently skipped.
    /// - Non-serializable values (closures, native fns, handles, `Pending`) that
    ///   `get_state_json` degraded to a display `String` or a structured `Map`
    ///   round-trip *verbatim* rather than erroring — they are applied as those
    ///   surrogate values, a known silent-corruption limitation, not an error.
    pub fn set_state_map_from_json(
        &mut self,
        program_id: ProgramId,
        stack_id: StackKey,
        map: &serde_json::Map<String, serde_json::Value>,
    ) -> usize {
        let mut applied = 0;
        for (name, json_val) in map {
            if self
                .set_state_from_json(program_id, stack_id, name, json_val)
                .is_ok()
            {
                applied += 1;
            }
        }
        applied
    }
}
