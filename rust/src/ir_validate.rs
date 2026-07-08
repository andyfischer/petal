//! Import-path validation for the IR term graph.
//!
//! [`Program::from_json`] and its helpers reconstruct a [`Program`] from the
//! JSON IR form emitted by `show-ir --json` and enforce the structural-integrity
//! contract that a hand-written or third-party emitter must satisfy (the M3
//! contract in `docs/ir-as-target.md`). This lives apart from the IR data
//! structures in [`crate::program`] so the type definitions stay separate from
//! the (import-only) validator.

use std::collections::{HashMap, HashSet};

use crate::program::{BlockId, Program, TermId, TermOp};

impl Program {
    /// Deserialize a Program from its JSON IR form (the shape emitted by
    /// `show-ir --json`). Rebuilds the derived indexes (`block_terms`, the
    /// constant-table dedup map) that are skipped during serialization, and
    /// validates the graph's structural integrity. See `docs/ir-as-target.md`.
    pub fn from_json(json: &str) -> Result<Program, String> {
        let mut program: Program =
            serde_json::from_str(json).map_err(|e| format!("invalid IR JSON: {}", e))?;
        program.rebuild_indexes();
        program.validate()?;
        Ok(program)
    }

    /// Recompute the indexes that `#[serde(skip)]` drops on load, so a
    /// deserialized Program matches a freshly-compiled one. Built in term-id
    /// order, identical to the compiler (see `Compiler::compile`).
    pub fn rebuild_indexes(&mut self) {
        let mut block_terms: HashMap<BlockId, Vec<TermId>> = HashMap::new();
        for term in &self.terms {
            block_terms.entry(term.block_id).or_default().push(term.id);
        }
        self.block_terms = block_terms;
        self.constants.rebuild_dedup();
    }

    /// Structural validation for an imported IR graph (the M3 contract in
    /// `docs/ir-as-target.md`). Catches the malformations a hand-written or
    /// third-party emitter is most likely to produce before they become
    /// confusing runtime panics.
    pub fn validate(&self) -> Result<(), String> {
        if self.has_errors {
            return Err("program has has_errors=true".to_string());
        }
        let n_terms = self.terms.len() as u32;
        let n_blocks = self.blocks.len() as u32;
        let n_consts = self.constants.len() as u32;
        let n_fns = self.functions.len() as u32;

        // terms[i].id == i, and every reference resolves.
        for (i, term) in self.terms.iter().enumerate() {
            if term.id.0 as usize != i {
                return Err(format!("terms[{}] has id {} (must equal index)", i, term.id.0));
            }
            if matches!(term.op, TermOp::Error(_)) {
                return Err(format!("t{}: Error terms are not valid in an import", i));
            }
            for inp in &term.inputs {
                if inp.0 >= n_terms {
                    return Err(format!("t{}: input t{} out of range", i, inp.0));
                }
            }
            for cb in &term.child_blocks {
                if cb.0 >= n_blocks {
                    return Err(format!("t{}: child_block b{} out of range", i, cb.0));
                }
            }
            if term.block_id.0 >= n_blocks {
                return Err(format!("t{}: block_id b{} out of range", i, term.block_id.0));
            }
            // Constant references inside ops.
            for c in term.op.constant_ids() {
                if c.0 >= n_consts {
                    return Err(format!("t{}: constant c{} out of range", i, c.0));
                }
            }
            if let TermOp::MakeClosure(f) = &term.op
                && f.0 >= n_fns
            {
                return Err(format!("t{}: function f{} out of range", i, f.0));
            }
            // A BuiltinCall's name must resolve to a String constant (the constant
            // was already range-checked above via `cids`).
            if let TermOp::BuiltinCall(c) = &term.op
                && self.get_string_constant(*c).is_none()
            {
                return Err(format!("t{}: BuiltinCall name is not a string constant", i));
            }
            // State ops require a state_key. Other ops *may* also carry one:
            // a `Copy` produced by a state-tracking reassignment references its
            // StateInit's key (see compile_assign).
            let is_state = matches!(
                term.op,
                TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite
            );
            if is_state && term.state_key.is_none() {
                return Err(format!("t{}: state op missing state_key", i));
            }
        }

        // Block back-references and phi_outs targets.
        for (i, block) in self.blocks.iter().enumerate() {
            if block.id.0 as usize != i {
                return Err(format!("blocks[{}] has id {} (must equal index)", i, block.id.0));
            }
            if let Some(entry) = block.entry
                && entry.0 >= n_terms
            {
                return Err(format!("b{}: entry t{} out of range", i, entry.0));
            }
            if let Some(pt) = block.parent_term_id
                && pt.0 >= n_terms
            {
                return Err(format!("b{}: parent_term t{} out of range", i, pt.0));
            }
            for po in &block.phi_outs {
                if po.src_term.0 >= n_terms || po.dest_term.0 >= n_terms {
                    return Err(format!("b{}: phi_out references out-of-range term", i));
                }
                if !matches!(self.terms[po.dest_term.0 as usize].op, TermOp::Phi) {
                    return Err(format!(
                        "b{}: phi_out dest t{} is not a Phi term",
                        i, po.dest_term.0
                    ));
                }
            }
        }

        if self.root_block.0 >= n_blocks {
            return Err(format!("root_block b{} out of range", self.root_block.0));
        }

        // Every StateRead/StateWrite key must have a matching StateInit.
        let inits: HashSet<u64> = self
            .terms
            .iter()
            .filter(|t| matches!(t.op, TermOp::StateInit))
            .filter_map(|t| t.state_key.map(|k| k.0))
            .collect();
        for term in &self.terms {
            if matches!(term.op, TermOp::StateRead | TermOp::StateWrite)
                && let Some(k) = term.state_key
                && !inits.contains(&k.0)
            {
                return Err(format!(
                    "t{}: state key {} has no StateInit",
                    term.id.0, k.0
                ));
            }
        }
        Ok(())
    }
}
