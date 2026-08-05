//! Observations ↔ JSON: reading the last value bound to every named IR term out
//! of a running program, keyed by a function-qualified source name.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors, and [`crate::observe`] for the buffer this reads.

use super::*;

use std::collections::HashMap;

use crate::program::{BlockId, FunctionId, TermId, TermOp, base_fn_name};

impl Env {
    /// Every observed value, keyed by function-qualified source name.
    ///
    /// # The naming rule
    /// A term keys under its source name, prefixed by the name of each function
    /// whose body encloses it, outermost first:
    ///
    /// - a `let` at the top level, *including* one inside a top-level `if` /
    ///   `else` / loop body, keys as `body_top` — only function bodies qualify,
    ///   because only they introduce a name scope a reader would confuse;
    /// - the same `let` inside `fn foo` keys as `foo.body_top`, and inside a
    ///   `fn inner` nested in a `fn outer` as `outer.inner.body_top`.
    ///
    /// So a function-local name and a same-named top-level one are two distinct
    /// keys and both are readable — the collision this facility exists to fix.
    /// The internal `#arity` overload suffix is stripped throughout, so an
    /// overloaded `fn greet` reads as `greet.…` regardless of which overload
    /// bound the value. An anonymous function contributes `fn<id>`.
    ///
    /// # Last write wins
    /// One slot per term, overwritten on every write: a binding inside a loop
    /// or a repeatedly-called function reports its **final** value, not its
    /// history (use [`Env::trace`] for that). Where two *different* terms
    /// qualify to the same key — the same name bound in both arms of an `if`,
    /// say — the later term in program order wins, so the answer is at least
    /// deterministic.
    ///
    /// A binding whose term never executed is absent from the map rather than
    /// present as null: "the `else` arm didn't run" and "the `else` arm bound
    /// nil" are different facts.
    ///
    /// # Contract: the values are a snapshot, not a handle
    /// The same caveat as [`Env::get_state`]: the JSON reflects the moment it is
    /// read. A container id observed here may be mutated *in place* by a later
    /// run (see the escape analysis), so read the map when you want the answer —
    /// don't compute it once and hold it across a run.
    ///
    /// Empty when observation is disabled (the default), when nothing has run
    /// yet, when `program_id` is unknown, or when the buffer holds another
    /// execution context's values — a fork's ids index the fork's heap, and
    /// decoding them against this stack's would yield plausible nonsense.
    pub fn get_observations_json(
        &self,
        program_id: ProgramId,
        stack_id: StackKey,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        if self.observations.is_empty() {
            return map;
        }
        let Some(program) = self.get_program(program_id) else {
            return map;
        };
        // Resolve the stack's *own* context heap: a fork's observed ids index
        // its forked heap, not the default context's.
        let ck = self.ctx_for(stack_id).unwrap_or(self.default_context);
        if self.observations.context() != Some(ck) {
            return map;
        }
        let ctx = self.ctx(ck);
        // Context for provenance-rich pending rendering: an observed pending
        // value dumps as a structured `{ type:"pending", … }` object rather
        // than `"<pending>"`, exactly as `get_state_json` does.
        let pending_ctx = crate::value::PendingJsonCtx {
            resources: &ctx.resources,
            program,
            frame: ctx.frame(),
        };

        let scopes = FunctionScopes::build(program);
        // Ascending term order makes the tie-break for two terms sharing one
        // qualified name deterministic: the later term wins.
        let mut entries: Vec<(TermId, Value)> = self.observations.iter().collect();
        entries.sort_by_key(|(t, _)| t.0);
        for (term_id, val) in entries {
            let Some(name) = scopes.qualified_name(program, term_id) else {
                continue;
            };
            map.insert(
                name,
                crate::value::value_to_json_ctx(&val, &ctx.heap, Some(&pending_ctx)),
            );
        }
        map
    }
}

/// The static half of the naming rule: which blocks are function bodies, and
/// where each such function was *defined* so the walk can continue outward.
///
/// A function body block carries `parent_term_id: None` (the compiler never
/// links it — see `compiler::function::begin_function_scope`), so the block
/// chain alone stops dead at the innermost function. The `MakeClosure` term
/// that mints the closure does live in the enclosing block, and that is the
/// edge this table supplies.
struct FunctionScopes {
    /// Body block → the function it belongs to.
    by_body: HashMap<BlockId, FunctionId>,
    /// Function → the `MakeClosure` term that defines it, if one was emitted.
    def_term: HashMap<FunctionId, TermId>,
}

impl FunctionScopes {
    fn build(program: &Program) -> Self {
        let mut by_body = HashMap::new();
        for f in &program.functions {
            by_body.insert(f.body_block, f.id);
        }
        let mut def_term = HashMap::new();
        for term in &program.terms {
            if let TermOp::MakeClosure(fid) = term.op {
                def_term.insert(fid, term.id);
            }
        }
        Self { by_body, def_term }
    }

    /// The function-qualified key for a term, or `None` if it has no name.
    fn qualified_name(&self, program: &Program, term_id: TermId) -> Option<String> {
        let term = program.get_term(term_id);
        let leaf = base_fn_name(term.name.as_deref()?);

        let mut prefixes: Vec<String> = Vec::new();
        let mut block = term.block_id;
        // A malformed program could in principle cycle; the visit count is
        // bounded by the block count, so cap the walk rather than hang a host.
        for _ in 0..program.blocks.len() + 1 {
            if let Some(&fid) = self.by_body.get(&block) {
                let def = program.functions.get(fid.0 as usize);
                prefixes.push(match def.and_then(|f| f.name.as_deref()) {
                    Some(n) => base_fn_name(n).to_string(),
                    // An anonymous function still needs a distinct scope, or its
                    // locals would collide with the top level's — and with each
                    // other's, hence the id.
                    None => format!("fn{}", fid.0),
                });
                match self.def_term.get(&fid) {
                    Some(&t) => {
                        block = program.get_term(t).block_id;
                        continue;
                    }
                    None => break,
                }
            }
            match program.get_block(block).parent_term_id {
                Some(pt) => block = program.get_term(pt).block_id,
                None => break,
            }
        }

        if prefixes.is_empty() {
            return Some(leaf.to_string());
        }
        prefixes.reverse();
        prefixes.push(leaf.to_string());
        Some(prefixes.join("."))
    }
}
