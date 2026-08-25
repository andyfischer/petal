//! State ↔ JSON: serializing a stack's committed state variables to a JSON map
//! keyed by variable name, and setting a named state variable from JSON.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors.

use super::*;
use crate::program::TermOp;
use crate::stack::PathPart;

impl Env {
    /// Serialize all state variables to a JSON map keyed by variable name.
    ///
    /// A **top-level** declaration runs on the root path and keys by its bare
    /// (module-qualified) name — `count`, `ui::theme` — which is every name a
    /// host inspects today and is unchanged from before call-path keying.
    ///
    /// A slot reached through a call path keys by that path, rendered
    /// root-to-leaf with `/` between steps and the variable name last:
    /// `counter/count`, `render/row#1/[3]/hovered`, `k1234…/leaf`. See
    /// [`render_state_key`] for the one renderer every part shape goes through.
    /// A pathed name always contains a `/`, which no bare name can, so the two
    /// namespaces cannot collide.
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
        let program = self.get_program(program_id);
        // Context for provenance-rich pending rendering: a pending state var
        // dumps as a structured `{ type:"pending", … }` object, not `"<pending>"`.
        let pending_ctx = program.map(|program| crate::value::PendingJsonCtx {
            resources: &ctx.resources,
            program,
            frame: ctx.frame(),
        });
        let mut map = serde_json::Map::new();
        if let Some(state) = self.get_all_state(stack_id) {
            // Callsite labels cost a scan over every term, so pay for them only
            // when a key actually carries a `Call` part — which no top-level-only
            // program has.
            let has_call_part = state
                .keys()
                .any(|k| k.path.iter().any(|p| matches!(p, PathPart::Call(_))));
            let labels = match program {
                Some(program) if has_call_part => call_site_labels(program),
                _ => HashMap::new(),
            };
            for (key, val) in state {
                let base_name = names
                    .get(&key.base)
                    .cloned()
                    .unwrap_or_else(|| format!("unknown_{}", key.base.0));
                map.insert(
                    render_state_key(&base_name, &key.path, &labels),
                    crate::value::value_to_json_ctx(val, heap, pending_ctx.as_ref()),
                );
            }
        }
        map
    }

    /// Set a top-level state variable by name from a JSON value.
    ///
    /// Top-level only: `name` is matched against the declaration names, so a
    /// pathed slot (`counter/count`, `[0]/xs`) has no address here — see
    /// [`Self::set_state_map_from_json`] for that documented limitation.
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
    /// - **Pathed entries are not addressable.** Anything `get_state_json`
    ///   rendered with a path — a slot inside a called function
    ///   (`counter/count`), a loop iteration (`[0]/xs`), an explicit
    ///   `state(key)` (`k1234…/leaf`) — is silently skipped, because
    ///   `set_state_from_json` matches top-level declaration names only. A slot
    ///   a host means to set from the outside belongs at top level (a
    ///   `state var` cell), which is also the model's answer for shared state.
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

/// Render one runtime state key as the name hosts see it: the call path that
/// reached the slot, root-to-leaf, `/`-separated, with the variable name last.
///
/// The single renderer for every part shape (plan §8.5 — the old code had one
/// special case per shape):
///
/// | Path | Renders as |
/// |------|------------|
/// | `[]` (top level) | `count` |
/// | `[Call(f)]` | `counter/count` |
/// | `[Call(f), Call(f)]` (recursion) | `counter/counter/count` |
/// | `[Index(3), Call(f)]` (called in a loop) | `[3]/row/hovered` |
/// | `[Index(0), Index(2)]` (top-level state in nested loops) | `[0]/[2]/xs` |
/// | `[Key(h)]` (explicit `state(expr)`) | `k1234…/leaf` |
///
/// A `Call` part is a hash, so it renders as the callee's spelling recovered
/// from the program (`counter`, `#1` and up when one function calls the same
/// callee more than once) and falls back to `c<hash>` when the program cannot
/// name it — hand-written IR, or a slot left behind by an edit that removed the
/// callsite. The label is for reading, not for addressing: nothing resolves a
/// name back to a slot (see [`Env::set_state_map_from_json`]).
fn render_state_key(base_name: &str, path: &[PathPart], labels: &HashMap<u64, String>) -> String {
    if path.is_empty() {
        return base_name.to_string();
    }
    let mut out = String::with_capacity(base_name.len() + path.len() * 8);
    for part in path {
        match part {
            PathPart::Call(h) => match labels.get(h) {
                Some(label) => out.push_str(label),
                None => out.push_str(&format!("c{h}")),
            },
            PathPart::Index(i) => out.push_str(&format!("[{i}]")),
            PathPart::Key(h) => out.push_str(&format!("k{h}")),
        }
        out.push('/');
    }
    out.push_str(base_name);
    out
}

/// Display labels for the callsite ids ([`Term::call_site`](crate::program::Term::call_site))
/// a path's `Call` parts carry, recovered from the program's call terms.
///
/// Best-effort and display-only: the compiler hashes the callee's canonical
/// text (plus an ordinal and its lexical scope) into an opaque `u64`, so this
/// reads the callee back off the term graph rather than inverting the hash.
/// Two callsites that read alike are separated by a `#n` suffix in term order —
/// close to, but not derived from, the compiler's own per-function ordinal.
/// Nothing addresses a slot by these names, so a label that shifts costs a
/// changed string in a dump, never a changed slot.
fn call_site_labels(program: &crate::program::Program) -> HashMap<u64, String> {
    let mut labels: HashMap<u64, String> = HashMap::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    for term in &program.terms {
        let Some(site) = term.call_site else { continue };
        if labels.contains_key(&site) {
            continue;
        }
        let text = callee_display(program, term);
        let n = seen.entry(text.clone()).or_insert(0);
        let ordinal = std::mem::replace(n, *n + 1);
        let label = if ordinal == 0 {
            text
        } else {
            format!("{text}#{ordinal}")
        };
        labels.insert(site, label);
    }
    labels
}

/// The callee a call term names, for [`call_site_labels`]. A `Call`'s callee is
/// its first input, which is typically a `Copy` of the binding the `fn`
/// declared — so follow the copy chain to the term that carries the name.
fn callee_display(program: &crate::program::Program, term: &crate::program::Term) -> String {
    const UNNAMED: &str = "<expr>";
    match &term.op {
        TermOp::BuiltinCall(cid) => program
            .get_string_constant(*cid)
            .unwrap_or(UNNAMED)
            .to_string(),
        TermOp::MethodCall { name, .. } => match program.get_string_constant(*name) {
            Some(name) => format!(".{name}"),
            None => UNNAMED.to_string(),
        },
        TermOp::Call => {
            let Some(&callee) = term.inputs.first() else {
                return UNNAMED.to_string();
            };
            let mut cur = program.get_term(callee);
            // Bounded by the copy chain's length; every hop moves strictly
            // backwards through the term graph, which is acyclic.
            loop {
                if let Some(name) = &cur.name {
                    return name.clone();
                }
                match (&cur.op, cur.inputs.first()) {
                    (TermOp::Copy, Some(&src)) => cur = program.get_term(src),
                    _ => return UNNAMED.to_string(),
                }
            }
        }
        _ => UNNAMED.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(u64, &str)]) -> HashMap<u64, String> {
        pairs.iter().map(|(h, s)| (*h, s.to_string())).collect()
    }

    #[test]
    fn a_top_level_slot_renders_as_its_bare_name() {
        assert_eq!(render_state_key("count", &[], &HashMap::new()), "count");
        assert_eq!(
            render_state_key("ui::theme", &[], &HashMap::new()),
            "ui::theme"
        );
    }

    #[test]
    fn every_part_shape_goes_through_one_renderer() {
        let labels = labels(&[(7, "counter")]);
        assert_eq!(
            render_state_key("count", &[PathPart::Call(7)], &labels),
            "counter/count"
        );
        assert_eq!(
            render_state_key("count", &[PathPart::Call(7), PathPart::Call(7)], &labels),
            "counter/counter/count"
        );
        assert_eq!(
            render_state_key("hovered", &[PathPart::Index(3), PathPart::Call(7)], &labels),
            "[3]/counter/hovered"
        );
        assert_eq!(
            render_state_key("xs", &[PathPart::Index(0), PathPart::Index(2)], &labels),
            "[0]/[2]/xs"
        );
        assert_eq!(
            render_state_key("leaf", &[PathPart::Key(42)], &labels),
            "k42/leaf"
        );
    }

    #[test]
    fn an_unlabelled_call_falls_back_to_its_hash() {
        // Hand-written IR, or a slot whose callsite the last edit deleted: the
        // path still renders, just opaquely.
        assert_eq!(
            render_state_key("count", &[PathPart::Call(99)], &HashMap::new()),
            "c99/count"
        );
    }

    #[test]
    fn a_pathed_name_can_never_look_like_a_bare_one() {
        // What keeps the two namespaces disjoint for `set_state_from_json`:
        // every path has at least one part, and every part is followed by a
        // `/`, which no declaration name contains.
        for path in [
            vec![PathPart::Call(1)],
            vec![PathPart::Index(0)],
            vec![PathPart::Key(2)],
        ] {
            assert!(render_state_key("count", &path, &HashMap::new()).contains('/'));
        }
    }
}
