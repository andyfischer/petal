//! Import-path validation for the IR term graph.
//!
//! [`Program::from_json`] and its helpers reconstruct a [`Program`] from the
//! JSON IR form emitted by `show-ir --json` and enforce the structural-integrity
//! contract that a hand-written or third-party emitter must satisfy (the M3
//! contract in `docs/ir-as-target.md`). This lives apart from the IR data
//! structures in [`crate::program`] so the type definitions stay separate from
//! the (import-only) validator.

use std::collections::{HashMap, HashSet};

use crate::program::{
    BlockId, IR_SCHEMA_VERSION, Program, REGISTER_UNSET, RegisterIndex, Term, TermId, TermOp,
};

/// Is this term a binding phantom — an unexecuted `Copy` that only names a
/// register (builtin bindings, params, captures, self-refs, match-pattern
/// variables)? Structural check only; "not listed in any block's `terms`
/// array" is enforced separately by `validate`.
pub(crate) fn is_binding_phantom(term: &Term) -> bool {
    matches!(term.op, TermOp::Copy) && term.inputs.is_empty() && term.name.is_some()
}

impl Program {
    /// Deserialize a Program from its JSON IR form (the shape emitted by
    /// `show-ir --json`). Normalizes the wire form back into the in-memory
    /// one — rebuilds the intra-block linked list from the ordered `terms`
    /// arrays (or the reverse, for legacy v0 documents), recomputes registers
    /// when the document omits them, rebuilds the `#[serde(skip)]` indexes —
    /// and validates the graph's structural integrity. See
    /// `docs/dev/ir-as-target.md`.
    pub fn from_json(json: &str) -> Result<Program, String> {
        let mut program: Program =
            serde_json::from_str(json).map_err(|e| format!("invalid IR JSON: {}", e))?;
        if program.schema != IR_SCHEMA_VERSION {
            return Err(format!(
                "unsupported IR schema {:?}: this build reads schema {} documents \
                 (or pre-schema v0 documents with no \"schema\" field)",
                program.schema, IR_SCHEMA_VERSION
            ));
        }
        // Reject a flagged-broken document before touching its structure —
        // its graph may be arbitrarily incomplete.
        if program.has_errors {
            return Err("program has has_errors=true".to_string());
        }
        program.relink_block_terms()?;
        program.rebuild_indexes();
        if program.registers_missing() {
            program.recompute_registers()?;
        }
        program.fill_register_counts();
        program.validate()?;
        Ok(program)
    }

    /// The `enums` table, the variants built against it, and `match_arms`.
    ///
    /// A variant value carries its tag and its payload and nothing else, so
    /// a field read by name and a variant pattern both trust the table: a
    /// construction or a pattern with the wrong number of fields for the
    /// variant it names would read past the payload or never match. A tag
    /// whose enum the document does not declare (a host's own, or a legacy
    /// document's bare `Circle`) is taken as it comes.
    fn validate_enums_and_matches(&self) -> Result<(), String> {
        use crate::ast::Pattern;
        use crate::program::split_enum_tag;

        for (name, def) in &self.enums {
            let mut seen: HashSet<&str> = HashSet::new();
            for v in &def.variants {
                if crate::program::ENUM_MEMBERS.contains(&v.name.as_str()) {
                    return Err(format!(
                        "enum {name}: a variant may not be named `{}`",
                        v.name
                    ));
                }
                if !seen.insert(v.name.as_str()) {
                    return Err(format!("enum {name}: variant `{}` is declared twice", v.name));
                }
                let mut members: HashSet<&str> = HashSet::new();
                for member in v.fields.iter().chain(&v.data) {
                    if member == crate::program::VARIANT_NAME_FIELD || !members.insert(member.as_str()) {
                        return Err(format!(
                            "enum {name}: variant `{}` has a field or data key `{member}` that is \
                             reserved or repeated",
                            v.name
                        ));
                    }
                }
            }
        }

        // The payload size the table gives a tag: `Ok(None)` for a tag of no
        // declared enum, an error for a variant its enum does not declare.
        let payload_of = |tag: &str| -> Result<Option<usize>, String> {
            let (Some(enum_name), variant) = split_enum_tag(tag) else {
                return Ok(None);
            };
            let Some(def) = self.enums.get(enum_name) else {
                return Ok(None);
            };
            match def.variant(variant) {
                Some(v) => Ok(Some(v.fields.len())),
                None => Err(format!("enum `{enum_name}` declares no variant `{variant}`")),
            }
        };

        for term in &self.terms {
            let TermOp::MakeEnumVariant(c) = &term.op else {
                continue;
            };
            let Some(tag) = self.get_string_constant(*c) else {
                return Err(format!(
                    "t{}: MakeEnumVariant tag is not a string constant",
                    term.id.0
                ));
            };
            match payload_of(tag) {
                Err(e) => return Err(format!("t{}: MakeEnumVariant({tag}): {e}", term.id.0)),
                Ok(Some(n)) if n != term.inputs.len() => {
                    return Err(format!(
                        "t{}: MakeEnumVariant({tag}) has {} input(s), but the variant declares \
                         {n} payload field(s)",
                        term.id.0,
                        term.inputs.len()
                    ));
                }
                Ok(_) => {}
            }
        }

        fn check_pattern(
            pattern: &Pattern,
            payload_of: &dyn Fn(&str) -> Result<Option<usize>, String>,
        ) -> Result<(), String> {
            match pattern {
                Pattern::Wildcard | Pattern::Literal(_) | Pattern::Variable(_) => Ok(()),
                Pattern::Variant { name, fields } => {
                    match payload_of(name)? {
                        Some(n) if n != fields.len() => {
                            return Err(format!(
                                "pattern `{name}` has {} field(s), but the variant declares {n}",
                                fields.len()
                            ));
                        }
                        _ => {}
                    }
                    fields.iter().try_for_each(|f| check_pattern(f, payload_of))
                }
                Pattern::List { elements, .. } => {
                    elements.iter().try_for_each(|e| check_pattern(e, payload_of))
                }
                Pattern::Record(fields) => {
                    fields.iter().try_for_each(|(_, p)| check_pattern(p, payload_of))
                }
                Pattern::Or(alternatives) => {
                    if alternatives.len() < 2 {
                        return Err("an or-pattern needs at least two alternatives".to_string());
                    }
                    if pattern.or_binding_mismatch().is_some() {
                        return Err(
                            "the alternatives of an or-pattern bind different names".to_string()
                        );
                    }
                    alternatives.iter().try_for_each(|a| check_pattern(a, payload_of))
                }
            }
        }

        let n_blocks = self.blocks.len() as u32;
        let mut matches: Vec<&TermId> = self.match_arms.keys().collect();
        matches.sort_by_key(|t| t.0);
        for &t in matches {
            if !self.terms.get(t.0 as usize).is_some_and(|term| matches!(term.op, TermOp::Match)) {
                return Err(format!("match_arms: t{} is not a Match term", t.0));
            }
            for (i, arm) in self.match_arms[&t].iter().enumerate() {
                let blocks = arm.guard_block.iter().chain(std::iter::once(&arm.body_block));
                if let Some(b) = blocks.into_iter().find(|b| b.0 >= n_blocks) {
                    return Err(format!("t{}: arm {i}: block b{} out of range", t.0, b.0));
                }
                check_pattern(&arm.pattern, &payload_of)
                    .map_err(|e| format!("t{}: arm {i}: {e}", t.0))?;
            }
        }
        Ok(())
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

    /// Reconcile the two forms of intra-block ordering after deserialization.
    ///
    /// Schema v0.2 documents carry each block's ordered `terms` array; the
    /// `entry`/`block_next`/`block_prev` linked list the engine walks is
    /// in-memory only and is rebuilt here. Legacy v0 documents carry the
    /// linked list and no `terms` arrays; those are walked once to
    /// reconstruct the arrays, then relinked from them like any other
    /// document (so both forms load to an identical Program).
    fn relink_block_terms(&mut self) -> Result<(), String> {
        let n_terms = self.terms.len();
        // Legacy direction: terms array absent, linked list present.
        for b in 0..self.blocks.len() {
            if !self.blocks[b].terms.is_empty() || self.blocks[b].entry.is_none() {
                continue;
            }
            let mut list = Vec::new();
            let mut cur = self.blocks[b].entry;
            while let Some(t) = cur {
                if t.0 as usize >= n_terms {
                    return Err(format!(
                        "b{}: linked list references t{} out of range",
                        b, t.0
                    ));
                }
                if list.len() >= n_terms {
                    return Err(format!("b{}: term linked list does not terminate", b));
                }
                list.push(t);
                cur = self.terms[t.0 as usize].block_next;
            }
            self.blocks[b].terms = list;
        }
        // Canonical direction: rebuild the linked list from the arrays.
        for term in &mut self.terms {
            term.block_next = None;
            term.block_prev = None;
        }
        for b in 0..self.blocks.len() {
            let list = self.blocks[b].terms.clone();
            for t in &list {
                if t.0 as usize >= n_terms {
                    return Err(format!(
                        "b{}: terms array references t{} out of range",
                        b, t.0
                    ));
                }
            }
            self.blocks[b].entry = list.first().copied();
            for pair in list.windows(2) {
                self.terms[pair[0].0 as usize].block_next = Some(pair[1]);
                self.terms[pair[1].0 as usize].block_prev = Some(pair[0]);
            }
        }
        Ok(())
    }

    /// Did the wire form omit any register assignment? (Absent registers
    /// deserialize to the [`REGISTER_UNSET`] sentinel.)
    fn registers_missing(&self) -> bool {
        self.terms.iter().any(|t| t.register == REGISTER_UNSET)
    }

    /// Assign the whole register file from scratch, for documents that omit
    /// registers (schema v0.2 makes them optional). Requires `block_terms`
    /// (call `rebuild_indexes` first).
    ///
    /// The assignment mirrors the compiler's: registers are per-block, dense,
    /// in term-id order — except that a block's parameter binding phantoms
    /// are seated first, at registers `0..N-1` in parameter order, because
    /// the engine addresses params positionally. Function capture registers
    /// and self-ref registers are then re-derived from the body block's
    /// binding phantoms by name (any values the document carried for them
    /// would describe an assignment that no longer exists).
    fn recompute_registers(&mut self) -> Result<(), String> {
        let mut counts: HashMap<BlockId, u16> = HashMap::new();
        // Params first: block.param_names is set for function bodies and
        // loop bodies alike, so one rule covers both.
        for b in 0..self.blocks.len() {
            let params = self.blocks[b].param_names.clone();
            let block_id = self.blocks[b].id;
            for param in &params {
                let tid = self.find_binding_phantom(block_id, param).ok_or_else(|| {
                    format!(
                        "b{}: registers are omitted, but parameter `{}` has no binding \
                         phantom (an unlisted `Copy` with no inputs and that name) in the block",
                        b, param
                    )
                })?;
                let next = counts.entry(block_id).or_insert(0);
                self.terms[tid.0 as usize].register = RegisterIndex(*next);
                *next += 1;
            }
        }
        // Everything else in term-id order, skipping the param phantoms
        // seated above.
        let seated: HashSet<TermId> = self
            .blocks
            .iter()
            .flat_map(|b| {
                b.param_names
                    .iter()
                    .filter_map(|p| self.find_binding_phantom(b.id, p))
            })
            .collect();
        for i in 0..self.terms.len() {
            if seated.contains(&self.terms[i].id) {
                continue;
            }
            let block_id = self.terms[i].block_id;
            let next = counts.entry(block_id).or_insert(0);
            self.terms[i].register = RegisterIndex(*next);
            *next = next
                .checked_add(1)
                .ok_or_else(|| format!("b{}: register assignment overflows u16", block_id.0))?;
        }
        for block in &mut self.blocks {
            block.register_count = counts.get(&block.id).copied().unwrap_or(0);
        }
        // Re-derive the function binding registers from the recomputed
        // assignment.
        for f in 0..self.functions.len() {
            let body = self.functions[f].body_block;
            self.functions[f].register_count = self
                .blocks
                .get(body.0 as usize)
                .map(|b| b.register_count)
                .unwrap_or(0);
            let capture_names = self.functions[f].capture_names.clone();
            let mut capture_registers = Vec::with_capacity(capture_names.len());
            for cap in &capture_names {
                let tid = self.find_binding_phantom(body, cap).ok_or_else(|| {
                    format!(
                        "fn{}: registers are omitted, but capture `{}` has no binding \
                         phantom in body block b{}",
                        f, cap, body.0
                    )
                })?;
                capture_registers.push(self.terms[tid.0 as usize].register);
            }
            self.functions[f].capture_registers = capture_registers;
            // Self-reference: a phantom named after the function that isn't
            // one of its params or captures.
            self.functions[f].self_ref_register = match self.functions[f].name.clone() {
                Some(name) => {
                    let claimed: HashSet<&String> = self.functions[f]
                        .params
                        .iter()
                        .chain(self.functions[f].capture_names.iter())
                        .collect();
                    if claimed.contains(&name) {
                        None
                    } else {
                        self.find_binding_phantom(body, &name)
                            .map(|tid| self.terms[tid.0 as usize].register)
                    }
                }
                None => None,
            };
        }
        Ok(())
    }

    /// The unlisted binding phantom named `name` in `block`, if any. Uses
    /// the rebuilt `block_terms` index (all terms of the block, in id order)
    /// minus the block's executed `terms` list.
    fn find_binding_phantom(&self, block: BlockId, name: &str) -> Option<TermId> {
        let all = self.block_terms.get(&block)?;
        let listed: HashSet<TermId> = self
            .blocks
            .get(block.0 as usize)
            .map(|b| b.terms.iter().copied().collect())
            .unwrap_or_default();
        all.iter().copied().find(|tid| {
            let term = &self.terms[tid.0 as usize];
            !listed.contains(tid) && is_binding_phantom(term) && term.name.as_deref() == Some(name)
        })
    }

    /// Fill in any block `register_count` the wire form omitted (absent
    /// deserializes to 0) from the registers actually used. A block whose
    /// document *explicitly* claims a too-small nonzero count is left alone
    /// so `validate` can reject it. A no-op for compiled programs and for
    /// documents that went through `recompute_registers`.
    fn fill_register_counts(&mut self) {
        let mut max_reg: HashMap<BlockId, u16> = HashMap::new();
        for term in &self.terms {
            let slot = max_reg.entry(term.block_id).or_insert(0);
            *slot = (*slot).max(term.register.0.saturating_add(1));
        }
        for block in &mut self.blocks {
            if block.register_count == 0 {
                block.register_count = max_reg.get(&block.id).copied().unwrap_or(0);
            }
        }
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
                return Err(format!(
                    "terms[{}] has id {} (must equal index)",
                    i, term.id.0
                ));
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
                return Err(format!(
                    "t{}: block_id b{} out of range",
                    i, term.block_id.0
                ));
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
            // Named-argument metadata: only calls carry it, it is either empty
            // or exactly as long as the op's argument slice, and every name
            // resolves to a String constant.
            if !term.arg_names.is_empty() {
                let Some(offset) = term.op.arg_offset() else {
                    return Err(format!("t{}: arg_names on a non-call op", i));
                };
                let n_args = term.inputs.len().saturating_sub(offset);
                if term.arg_names.len() != n_args {
                    return Err(format!(
                        "t{}: arg_names has {} entries for {} arguments",
                        i,
                        term.arg_names.len(),
                        n_args
                    ));
                }
                for c in term.arg_names.iter().flatten() {
                    if c.0 >= n_consts {
                        return Err(format!("t{}: constant c{} out of range", i, c.0));
                    }
                    if self.get_string_constant(*c).is_none() {
                        return Err(format!("t{}: argument name is not a string constant", i));
                    }
                }
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
            // `is_memo` marks a `memo(key)` declaration: a `StateInit` whose one
            // input is the key the slot is addressed by.
            if term.is_memo && !(matches!(term.op, TermOp::StateInit) && term.inputs.len() == 1) {
                return Err(format!(
                    "t{}: is_memo is only valid on a StateInit with exactly one input (its key)",
                    i
                ));
            }
            // Cell ops have fixed arity: the cell always comes first, so a
            // malformed IR can't turn a `CellWrite` into a read of whatever
            // happens to sit in input 0.
            let cell_arity = match term.op {
                TermOp::CellNew | TermOp::CellRead => Some(1),
                TermOp::CellWrite => Some(2),
                _ => None,
            };
            if let Some(n) = cell_arity
                && term.inputs.len() != n
            {
                return Err(format!(
                    "t{}: {} takes {} input(s), found {}",
                    i,
                    match term.op {
                        TermOp::CellNew => "CellNew",
                        TermOp::CellRead => "CellRead",
                        _ => "CellWrite",
                    },
                    n,
                    term.inputs.len()
                ));
            }
        }

        // Block back-references, terms-array consistency, and phi_outs targets.
        let mut listed_in: HashMap<TermId, BlockId> = HashMap::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if block.id.0 as usize != i {
                return Err(format!(
                    "blocks[{}] has id {} (must equal index)",
                    i, block.id.0
                ));
            }
            if let Some(entry) = block.entry
                && entry.0 >= n_terms
            {
                return Err(format!("b{}: entry t{} out of range", i, entry.0));
            }
            for &tid in &block.terms {
                if tid.0 >= n_terms {
                    return Err(format!(
                        "b{}: terms array references t{} out of range",
                        i, tid.0
                    ));
                }
                if self.terms[tid.0 as usize].block_id != block.id {
                    return Err(format!(
                        "b{}: lists t{}, whose block_id is b{}",
                        i, tid.0, self.terms[tid.0 as usize].block_id.0
                    ));
                }
                if let Some(other) = listed_in.insert(tid, block.id) {
                    return Err(format!(
                        "t{} is listed twice (b{} and b{})",
                        tid.0, other.0, i
                    ));
                }
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

        // Default parameter values: only trailing parameters have them, and
        // each one's was-it-passed flag is a body-block parameter slot after
        // the real ones (`FunctionDef::optional_params`).
        for (i, func) in self.functions.iter().enumerate() {
            let optional = func.optional_params as usize;
            if optional == 0 {
                continue;
            }
            if optional > func.params.len() {
                return Err(format!(
                    "fn{}: optional_params is {} but there are only {} params",
                    i,
                    optional,
                    func.params.len()
                ));
            }
            let seated = self
                .blocks
                .get(func.body_block.0 as usize)
                .map(|b| b.param_names.len())
                .unwrap_or(0);
            if seated != func.bound_arg_count() {
                return Err(format!(
                    "fn{}: {} params with {} optional need {} body-block param_names \
                     (each optional parameter is followed, after the last parameter, by \
                     its `name#given` flag), found {}",
                    i,
                    func.params.len(),
                    optional,
                    func.bound_arg_count(),
                    seated
                ));
            }
        }

        // A `Copy` on a block's execution list is a value copy and takes
        // exactly one input; an empty-inputs `Copy` is a binding phantom and
        // must stay OFF the list (it only names a register — executing it
        // would read nothing).
        for term in &self.terms {
            if matches!(term.op, TermOp::Copy)
                && listed_in.contains_key(&term.id)
                && term.inputs.len() != 1
            {
                return Err(format!(
                    "t{}: a Copy on a block's terms list takes exactly 1 input, found {} \
                     (binding phantoms must not be listed)",
                    term.id.0,
                    term.inputs.len()
                ));
            }
        }

        // Registers: every term's register fits its block's frame.
        for term in &self.terms {
            let count = self
                .blocks
                .get(term.block_id.0 as usize)
                .map(|b| b.register_count)
                .unwrap_or(0);
            if term.register.0 >= count {
                return Err(format!(
                    "t{}: register r{} out of range for b{} (register_count {})",
                    term.id.0, term.register.0, term.block_id.0, count
                ));
            }
        }

        if self.root_block.0 >= n_blocks {
            return Err(format!("root_block b{} out of range", self.root_block.0));
        }

        self.validate_enums_and_matches()?;

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
        // A memo entry is read only by its own declaration and never
        // reassigned, so nothing else may address its key.
        let memos: HashSet<u64> = self
            .terms
            .iter()
            .filter(|t| t.is_memo)
            .filter_map(|t| t.state_key.map(|k| k.0))
            .collect();
        for term in &self.terms {
            if matches!(term.op, TermOp::StateRead | TermOp::StateWrite)
                && let Some(k) = term.state_key
                && memos.contains(&k.0)
            {
                return Err(format!(
                    "t{}: state key {} belongs to a memo, which cannot be read or written \
                     outside its StateInit",
                    term.id.0, k.0
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::program::{Program, RegisterIndex};

    /// A minimal schema-v0.2 document: ordered `terms` arrays, no registers,
    /// no linked list, defaults omitted. `let x = 40 + 2` with a named
    /// binding Copy.
    const V02_MINIMAL: &str = r#"{
        "schema": "0.2",
        "id": 0,
        "terms": [
            {"id": 0, "op": {"Constant": 0}, "block_id": 0},
            {"id": 1, "op": {"Constant": 1}, "block_id": 0},
            {"id": 2, "op": "Add", "inputs": [0, 1], "block_id": 0},
            {"id": 3, "op": "Copy", "inputs": [2], "block_id": 0, "name": "x"}
        ],
        "blocks": [{"id": 0, "terms": [0, 1, 2, 3]}],
        "root_block": 0,
        "constants": {"values": [{"Int": 40}, {"Int": 2}]}
    }"#;

    /// The same graph in the legacy v0 shape: explicit linked list, entry,
    /// registers, and nulls.
    const V0_LEGACY: &str = r#"{
        "id": 0,
        "terms": [
            {"id": 0, "op": {"Constant": 0}, "inputs": [], "block_id": 0,
             "block_next": 1, "block_prev": null, "name": null, "register": 0,
             "state_key": null, "child_blocks": []},
            {"id": 1, "op": {"Constant": 1}, "inputs": [], "block_id": 0,
             "block_next": 2, "block_prev": 0, "name": null, "register": 1,
             "state_key": null, "child_blocks": []},
            {"id": 2, "op": "Add", "inputs": [0, 1], "block_id": 0,
             "block_next": 3, "block_prev": 1, "name": null, "register": 2,
             "state_key": null, "child_blocks": []},
            {"id": 3, "op": "Copy", "inputs": [2], "block_id": 0,
             "block_next": null, "block_prev": 2, "name": "x", "register": 3,
             "state_key": null, "child_blocks": []}
        ],
        "blocks": [{"id": 0, "parent_term_id": null, "entry": 0,
                    "param_names": [], "register_count": 4}],
        "root_block": 0,
        "constants": {"values": [{"Int": 40}, {"Int": 2}]},
        "has_errors": false,
        "functions": [],
        "match_arms": {}
    }"#;

    fn block_order(p: &Program) -> Vec<u32> {
        let mut out = Vec::new();
        let mut cur = p.get_block(p.root_block).entry;
        while let Some(t) = cur {
            out.push(t.0);
            cur = p.get_term(t).block_next;
        }
        out
    }

    #[test]
    fn v02_document_loads_and_relinks() {
        let p = Program::from_json(V02_MINIMAL).expect("loads");
        assert_eq!(block_order(&p), vec![0, 1, 2, 3]);
        // Registers were omitted and recomputed: dense per block, id order.
        let regs: Vec<u16> = p.terms.iter().map(|t| t.register.0).collect();
        assert_eq!(regs, vec![0, 1, 2, 3]);
        assert_eq!(p.get_block(p.root_block).register_count, 4);
    }

    #[test]
    fn legacy_v0_document_loads_identically() {
        let legacy = Program::from_json(V0_LEGACY).expect("legacy loads");
        let modern = Program::from_json(V02_MINIMAL).expect("v0.2 loads");
        assert_eq!(block_order(&legacy), block_order(&modern));
        assert_eq!(legacy.blocks[0].terms, modern.blocks[0].terms);
        for (a, b) in legacy.terms.iter().zip(modern.terms.iter()) {
            assert_eq!(a.register, b.register);
        }
    }

    #[test]
    fn loaded_ir_reserializes_in_v02_shape() {
        let p = Program::from_json(V0_LEGACY).expect("loads");
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["schema"], "0.2");
        assert_eq!(json["blocks"][0]["terms"], serde_json::json!([0, 1, 2, 3]));
        assert!(
            json["blocks"][0].get("entry").is_none(),
            "entry is in-memory only"
        );
        assert!(json["terms"][0].get("block_next").is_none());
        assert!(json["terms"][0].get("name").is_none(), "null name omitted");
        assert!(
            json["terms"][0].get("inputs").is_none(),
            "empty inputs omitted"
        );
        assert!(json.get("functions").is_none(), "empty functions omitted");
        assert!(json.get("has_errors").is_none(), "false has_errors omitted");
    }

    /// The IR of a compiled program, as a JSON value to tamper with.
    fn compiled_json(src: &str) -> serde_json::Value {
        let mut env = crate::env::Env::new();
        let pid = env.load_program(src).expect("compiles");
        let json = serde_json::to_string(env.get_program(pid).unwrap()).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    const ENUM_SRC: &str = "enum Op\n  fight(place, won, tone: \"bad\"),\n  rest(tone: \"ok\"),\nend\n\
        let op = Op.fight(\"x\", true)\n\
        let r = match op\n  when Op.fight(p, _) | Op.fight(_, p) -> p\n  when Op.rest -> nil\nend\nprint(r)\n";

    #[test]
    fn a_program_with_enums_and_or_patterns_round_trips() {
        let doc = compiled_json(ENUM_SRC);
        let program = Program::from_json(&doc.to_string()).expect("loads");
        let op = &program.enums["Op"];
        assert_eq!(op.variants[0].fields, ["place", "won"]);
        assert_eq!(op.variants[0].data, ["tone"]);
    }

    #[test]
    fn a_variant_built_or_matched_against_the_wrong_shape_is_rejected() {
        let load = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut doc = compiled_json(ENUM_SRC);
            edit(&mut doc);
            Program::from_json(&doc.to_string()).err().unwrap_or_default()
        };
        // The table says `fight` carries three fields; the construction and
        // the pattern in the document still carry two.
        let e = load(&|doc| {
            doc["enums"]["Op"]["variants"][0]["fields"] = serde_json::json!(["place", "won", "odds"]);
        });
        assert!(e.contains("MakeEnumVariant(Op.fight) has 2 input(s)"), "{e}");
        // A variant the enum does not declare.
        let e = load(&|doc| {
            doc["enums"]["Op"]["variants"][0]["name"] = serde_json::json!("brawl");
        });
        assert!(e.contains("declares no variant `fight`"), "{e}");
        // A table that breaks the declaration rules.
        let e = load(&|doc| {
            doc["enums"]["Op"]["variants"][1]["name"] = serde_json::json!("fight");
        });
        assert!(e.contains("variant `fight` is declared twice"), "{e}");
        let e = load(&|doc| {
            doc["enums"]["Op"]["variants"][1]["data"] = serde_json::json!(["name"]);
        });
        assert!(e.contains("reserved or repeated"), "{e}");
        // An arm whose block does not exist.
        let e = load(&|doc| {
            let arms = doc["match_arms"].as_object_mut().unwrap();
            let arm = arms.values_mut().next().unwrap();
            arm[0]["body_block"] = serde_json::json!(9999);
        });
        assert!(e.contains("block b9999 out of range"), "{e}");
        // A document that declares no enum at all is taken as it comes: its
        // tags belong to no table.
        let e = load(&|doc| {
            doc.as_object_mut().unwrap().remove("enums");
        });
        assert_eq!(e, "");
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let doc = V02_MINIMAL.replace("\"0.2\"", "\"9.9\"");
        let err = Program::from_json(&doc).err().expect("must be rejected");
        assert!(err.contains("unsupported IR schema"), "{err}");
    }

    #[test]
    fn registers_recomputed_for_functions_with_captures_and_self_ref() {
        // fn make(n): fn adder(x) -> x + n + adder-ness... simplified:
        //   f0 "adder" params=[x] captures=[n], self-recursive by name.
        // Body block b1: phantoms x (param), n (capture), adder (self).
        let doc = r#"{
            "schema": "0.2",
            "id": 0,
            "terms": [
                {"id": 0, "op": {"Constant": 0}, "block_id": 0, "name": "n"},
                {"id": 1, "op": {"MakeClosure": 0}, "inputs": [0], "block_id": 0, "name": "adder"},
                {"id": 2, "op": "Copy", "block_id": 1, "name": "x"},
                {"id": 3, "op": "Copy", "block_id": 1, "name": "n"},
                {"id": 4, "op": "Copy", "block_id": 1, "name": "adder"},
                {"id": 5, "op": "Add", "inputs": [2, 3], "block_id": 1}
            ],
            "blocks": [
                {"id": 0, "terms": [0, 1]},
                {"id": 1, "terms": [5], "param_names": ["x"]}
            ],
            "root_block": 0,
            "constants": {"values": [{"Int": 7}]},
            "functions": [
                {"id": 0, "name": "adder", "params": ["x"], "body_block": 1,
                 "capture_names": ["n"]}
            ]
        }"#;
        let p = Program::from_json(doc).expect("loads");
        // Param phantom seated first at r0.
        assert_eq!(p.terms[2].register, RegisterIndex(0));
        let f = &p.functions[0];
        assert_eq!(f.capture_registers, vec![p.terms[3].register]);
        assert_eq!(f.self_ref_register, Some(p.terms[4].register));
        assert_eq!(f.register_count, p.blocks[1].register_count);
    }

    #[test]
    fn omitted_registers_require_param_phantoms() {
        let doc = r#"{
            "schema": "0.2",
            "id": 0,
            "terms": [{"id": 0, "op": {"Constant": 0}, "block_id": 0}],
            "blocks": [{"id": 0, "terms": [0], "param_names": ["x"]}],
            "root_block": 0,
            "constants": {"values": [{"Int": 1}]}
        }"#;
        let err = Program::from_json(doc).err().expect("must be rejected");
        assert!(err.contains("has no binding phantom"), "{err}");
    }

    #[test]
    fn listed_phantom_copy_is_rejected() {
        let doc = r#"{
            "schema": "0.2",
            "id": 0,
            "terms": [{"id": 0, "op": "Copy", "block_id": 0, "name": "print"}],
            "blocks": [{"id": 0, "terms": [0]}],
            "root_block": 0,
            "constants": {"values": []}
        }"#;
        let err = Program::from_json(doc).err().expect("must be rejected");
        assert!(err.contains("exactly 1 input"), "{err}");
    }

    #[test]
    fn explicit_register_out_of_range_is_rejected() {
        let doc = r#"{
            "schema": "0.2",
            "id": 0,
            "terms": [
                {"id": 0, "op": {"Constant": 0}, "block_id": 0, "register": 5}
            ],
            "blocks": [{"id": 0, "terms": [0], "register_count": 1}],
            "root_block": 0,
            "constants": {"values": [{"Int": 1}]}
        }"#;
        let err = Program::from_json(doc).err().expect("must be rejected");
        assert!(err.contains("register r5 out of range"), "{err}");
    }

    #[test]
    fn term_listed_in_wrong_block_is_rejected() {
        let doc = r#"{
            "schema": "0.2",
            "id": 0,
            "terms": [{"id": 0, "op": {"Constant": 0}, "block_id": 1}],
            "blocks": [
                {"id": 0, "terms": [0]},
                {"id": 1}
            ],
            "root_block": 0,
            "constants": {"values": [{"Int": 1}]}
        }"#;
        let err = Program::from_json(doc).err().expect("must be rejected");
        assert!(err.contains("whose block_id"), "{err}");
    }

    #[test]
    fn compact_and_verbose_spans_both_load() {
        let with_spans = V02_MINIMAL.replace(
            "\"root_block\": 0,",
            r#""root_block": 0,
               "source_map": {"term_spans": {
                   "2": [1, 9, 8, 1, 15, 14],
                   "3": {"start": {"line": 1, "column": 1, "offset": 0},
                         "end": {"line": 1, "column": 15, "offset": 14}}
               }},"#,
        );
        let p = Program::from_json(&with_spans).expect("loads");
        let s2 = p.source_map.get(crate::program::TermId(2)).unwrap();
        assert_eq!((s2.start.line, s2.start.column, s2.start.offset), (1, 9, 8));
        assert_eq!((s2.end.line, s2.end.column, s2.end.offset), (1, 15, 14));
        let s3 = p.source_map.get(crate::program::TermId(3)).unwrap();
        assert_eq!((s3.start.line, s3.end.column), (1, 15));
    }
}
