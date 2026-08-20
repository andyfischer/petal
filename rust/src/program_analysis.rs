//! Dataflow-graph analysis over a compiled [`Program`].
//!
//! Term lookup (`find_term`/`named_terms`) and the backward/forward slicing
//! queries (`trace_provenance`, `trace_dependents`, `slice`) that back the
//! `explain`, `show-provenance`, `show-dependents`, and `show-slice` tooling.
//! These are read-only graph walks, kept separate from the IR data structures
//! in [`crate::program`] and the import validator in [`crate::ir_validate`].

use std::collections::{HashMap, HashSet, VecDeque};

use crate::program::{FunctionId, Program, Term, TermId, TermOp};

// ---------------------------------------------------------------------------
// Cells: identity edges, and the frontier a backward walk stops at
// ---------------------------------------------------------------------------

/// How a forward edge got into the graph. Backward walks report their edges
/// untyped; the forward walk distinguishes the must-edges from the two *may*
/// kinds, because "what could this affect" answered with only the must-edges
/// would under-report every `set` (§6e) and every method call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A real value edge: `from` is an input whose value `to` consumed.
    Dataflow,
    /// A possible edge through a cell: `from` writes (or declares) the cell
    /// that `to` reads. Which write actually supplied a given read is a
    /// dynamic fact, so every write reaches every read.
    CellMay,
    /// A possible edge through method dispatch: `from` is a function declared
    /// as `fn Class.name`, `to` a `.name(…)` call it may dispatch to. Dispatch
    /// is by name at runtime, so every method of that name reaches every call
    /// on it. See [`Program::dispatch_targets`].
    DispatchMay,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Dataflow => "dataflow",
            EdgeKind::CellMay => "may",
            EdgeKind::DispatchMay => "dispatch",
        }
    }
}

/// A place a backward walk stopped because it reached a cell.
///
/// The walk is defined over *value* edges only (see [`CellIndex::value_inputs`]),
/// so it terminates at every `CellRead` and at every closure that captures a
/// cell. A frontier entry is the record of that stop: which var, where it was
/// declared, and the complete set of writes that could have supplied the value.
///
/// The write set is complete rather than approximate because of the §6d
/// containment invariant: no expression evaluates to a cell, so the only way
/// to reach one is a name lexically bound to its declaration, and the only op
/// that writes one is a `CellWrite` on such a name. `writes` is therefore
/// closed — "one of these" can be imprecise, but it can never be missing the
/// real writer. The one exception is `host_writable`.
#[derive(Debug, Clone)]
pub struct CellFrontier {
    /// The term the walk stopped at: a `CellRead`, or (when `captured`) the
    /// `MakeClosure` that handed the cell to a function.
    pub read_term: TermId,
    /// `CellNew`, or the `StateInit` that owns it for a `state var`.
    /// `None` when the cell operand could not be resolved statically.
    pub cell_decl: Option<TermId>,
    pub var_name: Option<String>,
    /// Every `CellWrite` on this cell's declaration, in program order (§6d).
    pub writes: Vec<TermId>,
    /// A `state var`'s slot is also writable by the host through `set_state`
    /// (`rust/src/env/mod.rs:517`), so its write set is *not* closed.
    pub host_writable: bool,
    /// True when the stop was a closure capture rather than a direct read —
    /// the value came back through a call, so not even the read site is here.
    pub captured: bool,
}

impl CellFrontier {
    /// `var 'x'`, or `an unresolved cell` when static resolution failed.
    pub fn describe(&self) -> String {
        match (&self.var_name, self.captured) {
            (Some(n), false) => format!("read of var '{}'", n),
            (Some(n), true) => format!("closure captures var '{}'", n),
            (None, false) => "read of an unresolved cell".to_string(),
            (None, true) => "closure captures an unresolved cell".to_string(),
        }
    }
}

/// Result of a backward walk. `frontier` is empty iff the answer is complete;
/// consumers cannot render the result without deciding what to do about it.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub ancestors: Vec<TermId>,
    pub edges: Vec<(TermId, TermId)>,
    pub frontier: Vec<CellFrontier>,
}

impl Provenance {
    pub fn is_complete(&self) -> bool {
        self.frontier.is_empty()
    }
}

/// Result of a forward walk. Edges carry their kind so a consumer can tell a
/// value edge from a may-edge through a cell.
#[derive(Debug, Clone)]
pub struct Dependents {
    pub dependents: Vec<TermId>,
    pub edges: Vec<(TermId, TermId, EdgeKind)>,
}

/// A backward slice that crossed a cell, and therefore is not minimal.
#[derive(Debug, Clone)]
pub struct IncompleteSlice {
    pub frontier: Vec<CellFrontier>,
}

/// A dataflow slice, with the two readings kept apart on purpose.
///
/// The failure directions are not symmetric: a slice that is too small
/// silently computes a *different value*, while one that is too big only
/// loses precision. So there is no "minimal" flag — a caller has to name
/// which of the two it can live with, via [`SliceResult::minimal`] or
/// [`SliceResult::conservative`].
#[derive(Debug, Clone)]
pub struct SliceResult {
    terms: Vec<TermId>,
    minimal_frontier: Vec<CellFrontier>,
    conservative_terms: Vec<TermId>,
    conservative_frontier: Vec<CellFrontier>,
}

impl SliceResult {
    /// The minimal subgraph, or `Err` the moment a cell read was crossed.
    /// Byte-identical to the pre-cells behaviour on cell-free programs.
    pub fn minimal(self) -> Result<Vec<TermId>, IncompleteSlice> {
        if self.minimal_frontier.is_empty() {
            Ok(self.terms)
        } else {
            Err(IncompleteSlice {
                frontier: self.minimal_frontier,
            })
        }
    }

    /// The minimal slice closed over cells: every reachable declaration, every
    /// one of its static writes, and each write's own value-edge ancestors,
    /// iterated to a fixed point.
    ///
    /// Sufficient *in terms*, not faithful *in order*: a pure-dataflow slice
    /// never carried the control flow that selects among the writes, or their
    /// sequencing, so the returned frontier still reports the loss.
    pub fn conservative(self) -> (Vec<TermId>, Vec<CellFrontier>) {
        (self.conservative_terms, self.conservative_frontier)
    }
}

/// Static index of every cell declaration in a program and the reads and
/// writes that reach it. Built in one pass; the sole owner of the question
/// "which input of which op names a *box* rather than a *value*".
#[derive(Debug, Default, Clone)]
pub struct CellIndex {
    /// Cell-operand term -> the declaration it resolves to. Keyed by the
    /// declaration itself as well, so a direct reference is a hit.
    decl_of: HashMap<TermId, TermId>,
    reads: HashMap<TermId, Vec<TermId>>,
    writes: HashMap<TermId, Vec<TermId>>,
    host_writable: HashSet<TermId>,
    var_name: HashMap<TermId, String>,
    decls: HashSet<TermId>,
    /// A `CellRead`/`CellWrite`/capturing `MakeClosure` term -> its declaration.
    /// (`decl_of` is keyed by the cell *operand*; this is keyed by the site.)
    site_decl: HashMap<TermId, TermId>,
    /// Method name constant -> the function terms a `MethodCall` on that name
    /// may dispatch to. See [`Program::dispatch_targets`].
    dispatch: HashMap<crate::constant_table::ConstantId, Vec<TermId>>,
}

impl CellIndex {
    /// Sub-rule 1, and the only place it is stated.
    ///
    /// `inputs[0]` of a `CellRead`/`CellWrite` is an *identity* edge — it says
    /// which box, not which value — and so is a `MakeClosure` capture input
    /// that resolves to a cell declaration. Every other input of every op is a
    /// value edge, including `CellNew`'s initializer and `CellWrite`'s value
    /// operand at `inputs[1]`.
    fn is_identity_input(&self, term: &Term, pos: usize) -> bool {
        match term.op {
            TermOp::CellRead | TermOp::CellWrite => pos == 0,
            // A capture is by value, and the captured value *is* the cell —
            // which is exactly why closures can mutate a `var` (§6c). It is
            // therefore an identity edge, and a walk that crossed it would
            // claim the closure's result came from the cell's initializer.
            TermOp::MakeClosure(_) => self.decl_of.contains_key(&term.inputs[pos]),
            _ => false,
        }
    }

    /// The inputs of `term` a backward walk may follow.
    ///
    /// For a `MethodCall` this includes the method's *function* term, which is
    /// not an operand — user-method dispatch is by name, so without it a slice
    /// over class-using code would omit the code that computes the value. See
    /// [`Program::dispatch_targets`].
    pub fn value_inputs(&self, term: &Term) -> Vec<TermId> {
        let mut out: Vec<TermId> = term
            .inputs
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.is_identity_input(term, *i))
            .map(|(_, &id)| id)
            .collect();
        out.extend(self.dispatch_inputs(term));
        out
    }

    /// The function terms a `MethodCall` may dispatch to; empty for every other
    /// op. A may-edge, and the one dataflow edge that is not an operand.
    pub fn dispatch_inputs(&self, term: &Term) -> Vec<TermId> {
        let TermOp::MethodCall { name, .. } = term.op else {
            return Vec::new();
        };
        self.dispatch.get(&name).cloned().unwrap_or_default()
    }

    /// The inputs of `term` that name a cell. Empty for every op but
    /// `CellRead`, `CellWrite` and a capturing `MakeClosure`.
    pub fn cell_operands(&self, term: &Term) -> Vec<TermId> {
        term.inputs
            .iter()
            .enumerate()
            .filter(|(i, _)| self.is_identity_input(term, *i))
            .map(|(_, &id)| id)
            .collect()
    }

    /// The declaration a cell operand resolves to, if it resolves.
    pub fn decl_of(&self, operand: TermId) -> Option<TermId> {
        self.decl_of.get(&operand).copied()
    }

    pub fn is_decl(&self, term: TermId) -> bool {
        self.decls.contains(&term)
    }

    /// The declaration a read/write/capture *site* belongs to.
    pub fn decl_for_site(&self, term: TermId) -> Option<TermId> {
        if self.decls.contains(&term) {
            return Some(term);
        }
        self.site_decl.get(&term).copied()
    }

    pub fn writes_of(&self, decl: TermId) -> &[TermId] {
        self.writes.get(&decl).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn reads_of(&self, decl: TermId) -> &[TermId] {
        self.reads.get(&decl).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn var_name(&self, decl: TermId) -> Option<&str> {
        self.var_name.get(&decl).map(|s| s.as_str())
    }

    pub fn host_writable(&self, decl: TermId) -> bool {
        self.host_writable.contains(&decl)
    }

    /// Build the frontier record for a walk that stopped at `at_term`'s cell
    /// operand `operand`.
    fn frontier_for(&self, at_term: TermId, operand: TermId, captured: bool) -> CellFrontier {
        let decl = self.decl_of(operand);
        CellFrontier {
            read_term: at_term,
            cell_decl: decl,
            var_name: decl.and_then(|d| self.var_name(d)).map(|s| s.to_string()),
            writes: decl.map(|d| self.writes_of(d).to_vec()).unwrap_or_default(),
            host_writable: decl.map(|d| self.host_writable(d)).unwrap_or(false),
            captured,
        }
    }
}

impl Program {
    /// Find a term by name (e.g. variable name like "x") or by id string (e.g. "t24").
    pub fn find_term(&self, query: &str) -> Option<TermId> {
        // Try "tN" id format first
        if let Some(id_str) = query.strip_prefix('t')
            && let Ok(id) = id_str.parse::<u32>()
            && (id as usize) < self.terms.len()
        {
            return Some(TermId(id));
        }
        // Try a bare numeric ID (e.g. `--term 72`)
        if let Ok(id) = query.parse::<u32>()
            && (id as usize) < self.terms.len()
        {
            return Some(TermId(id));
        }
        // Search by name (last match wins — like variable shadowing)
        let mut found = None;
        for term in &self.terms {
            if term.name.as_deref() == Some(query) {
                found = Some(term.id);
            }
        }
        found
    }

    /// Return the list of distinct user-visible names bound to terms in this
    /// program. Filters out phantom builtin terms by requiring a real source
    /// span (line > 0). Used for "did you mean?" hints on `--term` misses.
    pub fn named_terms(&self) -> Vec<String> {
        use std::collections::BTreeSet;
        let mut set = BTreeSet::new();
        for term in &self.terms {
            let Some(name) = &term.name else { continue };
            match self.source_map.get(term.id) {
                Some(span) if span.start.line > 0 => {
                    set.insert(name.clone());
                }
                _ => {}
            }
        }
        set.into_iter().collect()
    }

    /// Index every cell declaration and the reads/writes that reach it.
    ///
    /// One pass to find the declarations (a `CellNew`, or the `StateInit` that
    /// owns it for a `state var`), then one pass to attach every `CellRead`,
    /// `CellWrite` and closure capture to the declaration its cell operand
    /// resolves to.
    pub fn cell_index(&self) -> CellIndex {
        let mut idx = CellIndex {
            dispatch: self.dispatch_targets(),
            ..CellIndex::default()
        };

        // Pass 1 — declarations. A `state var` puts its `CellNew` inside the
        // `StateInit`'s init block so the cell is created once and persists
        // (§7 4b), which makes the *slot* the thing every read names.
        for term in &self.terms {
            if !matches!(term.op, TermOp::CellNew) {
                continue;
            }
            let owner = self
                .get_block(term.block_id)
                .parent_term_id
                .filter(|p| matches!(self.get_term(*p).op, TermOp::StateInit));
            let decl = owner.unwrap_or(term.id);
            idx.decls.insert(decl);
            idx.decl_of.insert(decl, decl);
            idx.decl_of.insert(term.id, decl);
            if owner.is_some() {
                idx.host_writable.insert(decl);
            }
            let name = self
                .get_term(decl)
                .name
                .clone()
                .or_else(|| term.name.clone());
            if let Some(name) = name {
                idx.var_name.insert(decl, name);
            }
        }

        // A `MakeClosure` per function id, for resolving capture phantoms.
        let mut closures: HashMap<FunctionId, Vec<TermId>> = HashMap::new();
        for term in &self.terms {
            if let TermOp::MakeClosure(fid) = term.op {
                closures.entry(fid).or_default().push(term.id);
            }
        }

        // Pass 2 — reads, writes and captures.
        for term in &self.terms {
            let positions: &[usize] = match term.op {
                TermOp::CellRead | TermOp::CellWrite => &[0],
                TermOp::MakeClosure(_) => &[usize::MAX], // "all of them"
                _ => continue,
            };
            let all = positions == [usize::MAX];
            for pos in 0..term.inputs.len() {
                if !all && !positions.contains(&pos) {
                    continue;
                }
                let operand = term.inputs[pos];
                let Some(decl) = self.resolve_cell_operand(&idx, &closures, operand, 0) else {
                    continue;
                };
                idx.decl_of.insert(operand, decl);
                idx.site_decl.insert(term.id, decl);
                match term.op {
                    TermOp::CellRead => idx.reads.entry(decl).or_default().push(term.id),
                    TermOp::CellWrite => idx.writes.entry(decl).or_default().push(term.id),
                    _ => {}
                }
            }
        }
        for v in idx.reads.values_mut() {
            v.sort_by_key(|t| t.0);
        }
        for v in idx.writes.values_mut() {
            v.sort_by_key(|t| t.0);
        }
        idx
    }

    /// Resolve a cell operand to its declaration, walking out through closure
    /// capture phantoms.
    ///
    /// A capture phantom is a `TermOp::Copy` with **empty inputs** that is not
    /// linked into its block's execution list (`Compiler::emit_phantom_term`);
    /// there is no edge from it back to the cell, so there is no chain to
    /// follow. The only recoverable path is structural: phantom -> the
    /// `FunctionDef` whose `body_block` is the phantom's block -> the capture
    /// slot whose register is the phantom's register -> the `MakeClosure` for
    /// that function -> the corresponding input. Iterated, that handles nested
    /// closures.
    fn resolve_cell_operand(
        &self,
        idx: &CellIndex,
        closures: &HashMap<FunctionId, Vec<TermId>>,
        operand: TermId,
        depth: usize,
    ) -> Option<TermId> {
        if depth > 16 {
            return None;
        }
        if let Some(&decl) = idx.decl_of.get(&operand) {
            return Some(decl);
        }
        let term = self.get_term(operand);
        if !matches!(term.op, TermOp::Copy) || !term.inputs.is_empty() {
            return None;
        }
        let func = self
            .functions
            .iter()
            .find(|f| f.body_block == term.block_id)?;
        let slot = func
            .capture_registers
            .iter()
            .position(|r| *r == term.register)
            .or_else(|| {
                let name = term.name.as_deref()?;
                func.capture_names.iter().position(|n| n == name)
            })?;
        // Several `MakeClosure`s can share a `FunctionId` (a `fn` inside a
        // loop). Only accept an unambiguous answer.
        let sites = closures.get(&func.id)?;
        let mut resolved: Option<TermId> = None;
        for &site in sites {
            let outer = *self.get_term(site).inputs.get(slot)?;
            let decl = self.resolve_cell_operand(idx, closures, outer, depth + 1)?;
            match resolved {
                Some(prev) if prev != decl => return None,
                _ => resolved = Some(decl),
            }
        }
        resolved
    }

    /// Trace provenance: collect all transitive input ancestors of a term,
    /// following *value* edges only. The walk stops at every cell and records
    /// a [`CellFrontier`] there; `Provenance::is_complete` is the caller's
    /// gate (§6e).
    pub fn trace_provenance(&self, root_id: TermId) -> Provenance {
        let index = self.cell_index();
        self.trace_provenance_with(&index, root_id)
    }

    pub fn trace_provenance_with(&self, index: &CellIndex, root_id: TermId) -> Provenance {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut ancestors = Vec::new();
        let mut edges = Vec::new();
        let mut frontier: Vec<CellFrontier> = Vec::new();
        let mut stopped_at: HashSet<(TermId, TermId)> = HashSet::new();

        // Collect all `src_term` entries from `phi_outs` blocks across the
        // program that target a given phi term — these are the branch/loop
        // rebind candidates that update the phi's register on child-frame
        // pop, and must be treated as ancestors for provenance purposes.
        let phi_sources = |dest: TermId| -> Vec<TermId> {
            let mut out = Vec::new();
            for block in &self.blocks {
                for po in &block.phi_outs {
                    if po.dest_term == dest {
                        out.push(po.src_term);
                    }
                }
            }
            out
        };

        let mut push_term_inputs =
            |term_id: TermId,
             visited: &mut HashSet<TermId>,
             queue: &mut VecDeque<TermId>,
             edges: &mut Vec<(TermId, TermId)>| {
                let term = self.get_term(term_id);
                for input_id in index.value_inputs(term) {
                    edges.push((input_id, term_id));
                    if visited.insert(input_id) {
                        queue.push_back(input_id);
                    }
                }
                // An identity edge says which box, not which value. Cross it
                // and the answer becomes "y came from the cell's initializer",
                // which is the §6e lie. Stop, and say so — except at a
                // `CellWrite`, where no value *arrives* from the cell: the
                // write's own value chain is at `inputs[1]` and is entirely
                // present, so nothing about the answer is missing.
                if !matches!(term.op, TermOp::CellWrite) {
                    for operand in index.cell_operands(term) {
                        if stopped_at.insert((term_id, operand)) {
                            let captured = matches!(term.op, TermOp::MakeClosure(_));
                            frontier.push(index.frontier_for(term_id, operand, captured));
                        }
                    }
                }
                if matches!(term.op, TermOp::Phi) {
                    let srcs = phi_sources(term_id);
                    for src_id in srcs {
                        edges.push((src_id, term_id));
                        if visited.insert(src_id) {
                            queue.push_back(src_id);
                        }
                    }
                }
            };

        push_term_inputs(root_id, &mut visited, &mut queue, &mut edges);

        while let Some(term_id) = queue.pop_front() {
            ancestors.push(term_id);
            push_term_inputs(term_id, &mut visited, &mut queue, &mut edges);
        }

        Provenance {
            ancestors,
            edges,
            frontier,
        }
    }

    /// Forward slice: collect all terms that transitively depend on the given
    /// term. The complement of `trace_provenance`, but deliberately *not* its
    /// mirror image: "what could this affect" is a may-question, so the graph
    /// includes `CellMay` edges from a declaration and from every write to
    /// every read of the same cell. Over-approximating here is correct;
    /// under-approximating would make `show-dependents` on a `set` claim the
    /// mutation affects nothing.
    pub fn trace_dependents(&self, root_id: TermId) -> Dependents {
        let index = self.cell_index();

        // Reverse index over *value* edges: a read is not a "user" of the
        // declaration by identity, it is a user by may-edge (added below).
        let mut users: HashMap<TermId, Vec<(TermId, EdgeKind)>> = HashMap::new();
        for term in &self.terms {
            for (pos, &input_id) in term.inputs.iter().enumerate() {
                if index.is_identity_input(term, pos) {
                    continue;
                }
                users
                    .entry(input_id)
                    .or_default()
                    .push((term.id, EdgeKind::Dataflow));
            }
            // A user method reaches its call sites the same way a cell's
            // writes reach its reads: by name, not by operand.
            for target in index.dispatch_inputs(term) {
                users
                    .entry(target)
                    .or_default()
                    .push((term.id, EdgeKind::DispatchMay));
            }
        }
        for &decl in &index.decls {
            let reads = index.reads_of(decl);
            let writes = index.writes_of(decl);
            // The declaration reaches its writes as well as its reads —
            // without this, `show-dependents` on a `var` would not list the
            // `set` sites that are the whole point of declaring it.
            for &w in writes {
                users.entry(decl).or_default().push((w, EdgeKind::CellMay));
            }
            for &r in reads {
                users.entry(decl).or_default().push((r, EdgeKind::CellMay));
                for &w in writes {
                    users.entry(w).or_default().push((r, EdgeKind::CellMay));
                }
            }
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut dependents = Vec::new();
        let mut edges = Vec::new();

        // Seed with the root's direct users
        if let Some(direct_users) = users.get(&root_id) {
            for &(user_id, kind) in direct_users {
                if visited.insert(user_id) {
                    queue.push_back(user_id);
                }
                edges.push((root_id, user_id, kind));
            }
        }

        // BFS forward through users
        while let Some(term_id) = queue.pop_front() {
            dependents.push(term_id);
            if let Some(term_users) = users.get(&term_id) {
                for &(user_id, kind) in term_users {
                    edges.push((term_id, user_id, kind));
                    if visited.insert(user_id) {
                        queue.push_back(user_id);
                    }
                }
            }
        }

        Dependents { dependents, edges }
    }

    /// Compute a dataflow slice: the subgraph needed to compute the given
    /// target terms from their transitive inputs, in topological order
    /// (inputs before outputs). See [`SliceResult`] for why the caller has to
    /// choose between the minimal and the conservative reading.
    pub fn slice(&self, targets: &[TermId]) -> SliceResult {
        let index = self.cell_index();

        let mut needed: HashSet<TermId> = HashSet::new();
        let mut minimal_frontier: Vec<CellFrontier> = Vec::new();
        for &target in targets {
            needed.insert(target);
            let prov = self.trace_provenance_with(&index, target);
            needed.extend(prov.ancestors);
            minimal_frontier.extend(prov.frontier);
        }
        let terms = sorted(&needed);

        // Conservative closure. A value-edge walk from a write stops at the
        // next cell read, so pulling in one level of writes is not enough —
        // `set b = a + 1` would drag in b's writes but not a's, and the slice
        // would evaluate to a different value again. Iterate to a fixed point.
        let mut conservative_frontier = minimal_frontier.clone();
        let mut pending: Vec<TermId> = minimal_frontier
            .iter()
            .filter_map(|f| f.cell_decl)
            .collect();
        let mut seen_decls: HashSet<TermId> = HashSet::new();
        while let Some(decl) = pending.pop() {
            if !seen_decls.insert(decl) {
                continue;
            }
            needed.insert(decl);
            let decl_prov = self.trace_provenance_with(&index, decl);
            needed.extend(decl_prov.ancestors);
            for f in decl_prov.frontier {
                if let Some(d) = f.cell_decl {
                    pending.push(d);
                }
                conservative_frontier.push(f);
            }
            for &w in index.writes_of(decl) {
                needed.insert(w);
                let prov = self.trace_provenance_with(&index, w);
                needed.extend(prov.ancestors);
                for f in prov.frontier {
                    if let Some(d) = f.cell_decl {
                        pending.push(d);
                    }
                    conservative_frontier.push(f);
                }
            }
        }

        SliceResult {
            terms,
            minimal_frontier,
            conservative_terms: sorted(&needed),
            conservative_frontier,
        }
    }
}

/// Term ids in program order, which is topological for a well-formed IR.
fn sorted(ids: &HashSet<TermId>) -> Vec<TermId> {
    let mut out: Vec<TermId> = ids.iter().copied().collect();
    out.sort_by_key(|id| id.0);
    out
}

#[cfg(test)]
mod tests {
    use super::EdgeKind;
    use crate::constant_table::{ConstantId, ConstantTable, ConstantValue};
    use crate::program::*;
    use crate::source_map::SourceMap;
    use smallvec::SmallVec;
    use std::collections::HashMap;

    /// Build a minimal program with the given terms for testing.
    fn test_program(terms: Vec<Term>) -> Program {
        let root_block = BlockId(0);
        let blocks = vec![Block {
            id: root_block,
            parent_term_id: None,
            entry: terms.first().map(|t| t.id),
            terms: terms.iter().map(|t| t.id).collect(),
            param_names: vec![],
            register_count: terms.len() as u16,
            phi_outs: vec![],
        }];
        Program {
            schema: crate::program::IR_SCHEMA_VERSION.to_string(),
            id: ProgramId(0),
            source: String::new(),
            terms,
            blocks,
            root_block,
            constants: ConstantTable::new(),
            source_map: SourceMap::new(),
            has_errors: false,
            functions: vec![],
            match_arms: HashMap::new(),
            block_terms: HashMap::new(),
            warnings: Vec::new(),
            class_names: Default::default(),
        }
    }

    fn make_term(id: u32, op: TermOp, inputs: Vec<u32>, name: Option<&str>) -> Term {
        Term {
            id: TermId(id),
            op,
            inputs: inputs.into_iter().map(TermId).collect(),
            block_id: BlockId(0),
            block_next: None,
            block_prev: None,
            name: name.map(|s| s.to_string()),
            register: RegisterIndex(id as u16),
            state_key: None,
            child_blocks: SmallVec::new(),
            in_loop: false,
            collect: false,
            is_config: false,
        }
    }

    /// A `MethodCall` reaches the function it dispatches to through the
    /// registration statement, not through an operand. See
    /// [`Program::dispatch_targets`].
    #[test]
    fn method_dispatch_is_a_backward_dataflow_edge() {
        let mut constants = ConstantTable::new();
        let declare = constants.intern(ConstantValue::String(
            crate::classes::DECLARE_METHOD_BUILTIN.to_string(),
        ));
        let class = constants.intern(ConstantValue::String("Point".to_string()));
        let method = constants.intern(ConstantValue::String("dist2".to_string()));

        let mut prog = test_program(vec![
            // t0..t2: the declaration and its registration.
            make_term(
                0,
                TermOp::MakeClosure(FunctionId(0)),
                vec![],
                Some("Point.dist2"),
            ),
            make_term(1, TermOp::Constant(class), vec![], None),
            make_term(2, TermOp::Constant(method), vec![], None),
            make_term(3, TermOp::BuiltinCall(declare), vec![1, 2, 0], None),
            // t4: the receiver; t5: `recv.dist2()`.
            make_term(4, TermOp::Constant(class), vec![], Some("base")),
            make_term(
                5,
                TermOp::MethodCall {
                    name: method,
                    hint: None,
                },
                vec![4],
                Some("d"),
            ),
        ]);
        prog.constants = constants;

        assert_eq!(
            prog.dispatch_targets().get(&method).map(Vec::as_slice),
            Some([TermId(0)].as_slice())
        );

        let index = prog.cell_index();
        let call = prog.get_term(TermId(5));
        assert!(
            index.value_inputs(call).contains(&TermId(0)),
            "the method's function term is a backward edge of the call"
        );
        // …and nothing else gains one.
        assert!(index.dispatch_inputs(prog.get_term(TermId(3))).is_empty());

        let prov = prog.trace_provenance(TermId(5));
        assert!(prov.ancestors.contains(&TermId(0)), "{:?}", prov.ancestors);

        let deps = prog.trace_dependents(TermId(0));
        assert!(deps.dependents.contains(&TermId(5)));
        assert!(
            deps.edges
                .iter()
                .any(|&(f, t, k)| f == TermId(0) && t == TermId(5) && k == EdgeKind::DispatchMay)
        );
    }

    #[test]
    fn find_term_by_name() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(0)));
        assert_eq!(prog.find_term("y"), Some(TermId(1)));
        assert_eq!(prog.find_term("z"), None);
    }

    #[test]
    fn find_term_by_id_string() {
        let prog = test_program(vec![make_term(
            0,
            TermOp::Constant(ConstantId(0)),
            vec![],
            None,
        )]);
        assert_eq!(prog.find_term("t0"), Some(TermId(0)));
        assert_eq!(prog.find_term("t99"), None);
    }

    #[test]
    fn find_term_last_name_wins() {
        // Like variable shadowing: last definition with same name is found
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Constant(ConstantId(1)), vec![], Some("x")),
        ]);
        assert_eq!(prog.find_term("x"), Some(TermId(1)));
    }

    #[test]
    fn trace_provenance_leaf_has_no_ancestors() {
        let prog = test_program(vec![make_term(
            0,
            TermOp::Constant(ConstantId(0)),
            vec![],
            Some("x"),
        )]);
        let prov = prog.trace_provenance(TermId(0));
        assert!(prov.ancestors.is_empty());
        assert!(prov.edges.is_empty());
        assert!(prov.is_complete());
    }

    #[test]
    fn trace_provenance_single_input() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let prov = prog.trace_provenance(TermId(1));
        assert_eq!(prov.ancestors, vec![TermId(0)]);
        assert_eq!(prov.edges, vec![(TermId(0), TermId(1))]);
    }

    #[test]
    fn trace_provenance_diamond() {
        // c depends on a and b, both depend on const
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], None),
            make_term(1, TermOp::Copy, vec![0], Some("a")),
            make_term(2, TermOp::Copy, vec![0], Some("b")),
            make_term(3, TermOp::Add, vec![1, 2], Some("c")),
        ]);
        let prov = prog.trace_provenance(TermId(3));
        // BFS order: 1, 2, 0 (1 and 2 are direct inputs, 0 is shared ancestor)
        assert_eq!(prov.ancestors.len(), 3);
        assert!(prov.ancestors.contains(&TermId(1)));
        assert!(prov.ancestors.contains(&TermId(2)));
        assert!(prov.ancestors.contains(&TermId(0)));
        // Should have 4 edges: (1,3), (2,3), (0,1), (0,2)
        assert_eq!(prov.edges.len(), 4);
    }

    #[test]
    fn trace_dependents_leaf_has_no_dependents() {
        // Terminal node with no users
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("x")),
            make_term(1, TermOp::Copy, vec![0], Some("y")),
        ]);
        let deps = prog.trace_dependents(TermId(1));
        assert!(deps.dependents.is_empty());
        assert!(deps.edges.is_empty());
    }

    #[test]
    fn trace_dependents_single_user() {
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
        ]);
        let deps = prog.trace_dependents(TermId(0));
        assert_eq!(deps.dependents, vec![TermId(1)]);
        assert_eq!(deps.edges, vec![(TermId(0), TermId(1), EdgeKind::Dataflow)]);
    }

    #[test]
    fn trace_dependents_transitive() {
        // a -> b -> c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![1], Some("c")),
        ]);
        let deps = prog.trace_dependents(TermId(0));
        assert_eq!(deps.dependents.len(), 2);
        assert!(deps.dependents.contains(&TermId(1)));
        assert!(deps.dependents.contains(&TermId(2)));
    }

    #[test]
    fn trace_dependents_fan_out() {
        // a used by both b and c
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Copy, vec![0], Some("c")),
        ]);
        let deps = prog.trace_dependents(TermId(0));
        assert_eq!(deps.dependents.len(), 2);
        assert!(deps.dependents.contains(&TermId(1)));
        assert!(deps.dependents.contains(&TermId(2)));
        assert_eq!(deps.edges.len(), 2);
    }

    #[test]
    fn slice_returns_minimal_subgraph() {
        // a(0) -> b(1), c(2) -> d(3) = b + c, e(4) = unrelated
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Add, vec![1, 2], Some("d")),
            make_term(4, TermOp::Constant(ConstantId(2)), vec![], Some("e")),
        ]);
        let slice = prog.slice(&[TermId(3)]).minimal().expect("no cells");
        // Should include a, b, c, d but NOT e
        assert!(slice.contains(&TermId(0))); // a
        assert!(slice.contains(&TermId(1))); // b
        assert!(slice.contains(&TermId(2))); // c
        assert!(slice.contains(&TermId(3))); // d
        assert!(!slice.contains(&TermId(4))); // e is unrelated
        assert_eq!(slice.len(), 4);
        // Should be in topological order
        assert_eq!(slice, vec![TermId(0), TermId(1), TermId(2), TermId(3)]);
    }

    #[test]
    fn slice_multiple_targets() {
        // a(0) -> b(1), c(2) -> d(3)
        let prog = test_program(vec![
            make_term(0, TermOp::Constant(ConstantId(0)), vec![], Some("a")),
            make_term(1, TermOp::Copy, vec![0], Some("b")),
            make_term(2, TermOp::Constant(ConstantId(1)), vec![], Some("c")),
            make_term(3, TermOp::Copy, vec![2], Some("d")),
        ]);
        // Slice for both b and d should include a, b, c, d
        let slice = prog
            .slice(&[TermId(1), TermId(3)])
            .minimal()
            .expect("no cells");
        assert_eq!(slice.len(), 4);
    }
}

/// Cell-aware walks (§6e). These compile real source rather than hand-building
/// terms: the shapes under test — a capture phantom, a `state var`'s init
/// block — are produced by the compiler and a hand-built approximation of them
/// would pass without proving anything.
#[cfg(test)]
mod cell_tests {
    use super::EdgeKind;
    use crate::env::Env;
    use crate::program::{Program, TermId, TermOp};

    fn compiled(source: &str) -> Env {
        let mut env = Env::new();
        env.load_program(source).expect("compiles");
        env
    }

    fn program(env: &Env) -> &Program {
        env.get_program(crate::program::ProgramId(0))
            .or_else(|| (0..8u32).find_map(|i| env.get_program(crate::program::ProgramId(i))))
            .expect("program")
    }

    fn ops(program: &Program, op: fn(&TermOp) -> bool) -> Vec<TermId> {
        program
            .terms
            .iter()
            .filter(|t| op(&t.op))
            .map(|t| t.id)
            .collect()
    }

    fn only(ids: Vec<TermId>) -> TermId {
        assert_eq!(ids.len(), 1, "expected exactly one term, got {:?}", ids);
        ids[0]
    }

    fn line_of(program: &Program, id: TermId) -> Option<u32> {
        program
            .source_map
            .get(id)
            .filter(|s| s.start.line > 0)
            .map(|s| s.start.line)
    }

    /// Both halves of the §6e contract in one test, deliberately: stopping
    /// without announcing is the same lie, shorter.
    #[test]
    fn provenance_stops_at_cell_read() {
        let env = compiled("var x = 0\nlet y = x * 2\n");
        let p = program(&env);
        let decl = only(ops(p, |o| matches!(o, TermOp::CellNew)));
        let read = only(ops(p, |o| matches!(o, TermOp::CellRead)));
        let init = p.get_term(decl).inputs[0];

        let prov = p.trace_provenance(p.find_term("y").unwrap());
        // Stops: neither the declaration nor its initializer is claimed as an
        // ancestor of `y`.
        assert!(!prov.ancestors.contains(&decl));
        assert!(!prov.ancestors.contains(&init));
        // Announces.
        assert!(!prov.is_complete());
        assert_eq!(prov.frontier.len(), 1);
        assert_eq!(prov.frontier[0].read_term, read);
        assert_eq!(prov.frontier[0].cell_decl, Some(decl));
        assert_eq!(prov.frontier[0].var_name.as_deref(), Some("x"));
    }

    #[test]
    fn provenance_frontier_lists_every_write() {
        let env =
            compiled("var x = 0\nset x = 1\nfn bump()\n  set x = 2\nend\nbump()\nlet y = x * 2\n");
        let p = program(&env);
        let writes = ops(p, |o| matches!(o, TermOp::CellWrite));
        assert_eq!(writes.len(), 2);

        let prov = p.trace_provenance(p.find_term("y").unwrap());
        let f = prov
            .frontier
            .iter()
            .find(|f| f.var_name.as_deref() == Some("x"))
            .expect("frontier for x");
        // Complete and in program order — including the write in another
        // function, which is the whole reason `var` exists.
        assert_eq!(f.writes, writes);
    }

    /// Direct regression on the measured bogus `t92 -> t96` ancestor: a write's
    /// cell operand is an identity edge, its value operand is not.
    #[test]
    fn provenance_of_cell_write_excludes_the_identity_edge() {
        let env = compiled("var x = 0\nset x = x + 1\n");
        let p = program(&env);
        let decl = only(ops(p, |o| matches!(o, TermOp::CellNew)));
        let init = p.get_term(decl).inputs[0];
        let write = only(ops(p, |o| matches!(o, TermOp::CellWrite)));
        let add = only(ops(p, |o| matches!(o, TermOp::Add)));

        let prov = p.trace_provenance(write);
        assert!(!prov.ancestors.contains(&decl));
        assert!(!prov.ancestors.contains(&init));
        assert!(!prov.edges.contains(&(decl, write)));
        // The value operand's chain is untouched.
        assert!(prov.ancestors.contains(&add));
        assert!(prov.edges.contains(&(add, write)));
    }

    /// The shape `var` exists for. A capture phantom is a `Copy` with empty
    /// inputs, so there is no edge to follow back to the cell — resolution has
    /// to go phantom -> `FunctionDef` -> capture slot -> `MakeClosure` input.
    /// If this fails the frontier degrades to "unresolved" exactly where the
    /// feature matters.
    #[test]
    fn cell_decl_resolves_through_capture_phantom() {
        let env =
            compiled("var x = 0\nfn bump()\n  set x = get x + 1\n  get x\nend\nlet y = bump()\n");
        let p = program(&env);
        let decl = only(ops(p, |o| matches!(o, TermOp::CellNew)));
        let index = p.cell_index();

        let read = ops(p, |o| matches!(o, TermOp::CellRead))[0];
        let operand = p.get_term(read).inputs[0];
        assert!(
            p.get_term(operand).inputs.is_empty(),
            "phantom has no inputs"
        );
        assert_eq!(index.decl_of(operand), Some(decl));
        assert_eq!(index.writes_of(decl).len(), 1);

        // Fix A: the value comes back through a call, so the `MakeClosure`
        // capture is the stop. Without it `complete: true` would be a lie.
        let prov = p.trace_provenance(p.find_term("y").unwrap());
        assert!(!prov.is_complete());
        assert!(!prov.ancestors.contains(&decl));
        assert!(!prov.ancestors.contains(&p.get_term(decl).inputs[0]));
        let f = &prov.frontier[0];
        assert!(f.captured);
        assert_eq!(f.var_name.as_deref(), Some("x"));
        assert_eq!(f.cell_decl, Some(decl));
    }

    #[test]
    fn cell_decl_resolves_state_var_to_state_init() {
        let env = compiled("state var h = 0\nset h = h + 1\nlet y = h\n");
        let p = program(&env);
        let state_init = only(ops(p, |o| matches!(o, TermOp::StateInit)));

        let prov = p.trace_provenance(p.find_term("y").unwrap());
        let f = &prov.frontier[0];
        assert_eq!(f.cell_decl, Some(state_init));
        assert_eq!(f.var_name.as_deref(), Some("h"));
        // `set_state` writes through the slot, so the static write set is not
        // closed for a `state var` and the difference has to be visible.
        assert!(f.host_writable);
    }

    /// Direct regression on the measured `Downstream (0)`: a `set` that
    /// affects nothing is the forward direction's version of the §6e lie.
    #[test]
    fn dependents_of_cell_write_reaches_later_reads() {
        let env = compiled("var x = 0\nset x = x + 1\nlet y = x * 2\n");
        let p = program(&env);
        let write = only(ops(p, |o| matches!(o, TermOp::CellWrite)));
        let decl = only(ops(p, |o| matches!(o, TermOp::CellNew)));
        let reads = ops(p, |o| matches!(o, TermOp::CellRead));
        let y = p.find_term("y").unwrap();

        let deps = p.trace_dependents(write);
        assert!(deps.dependents.contains(&y));
        for r in &reads {
            assert!(deps.dependents.contains(r));
            assert!(deps.edges.contains(&(write, *r, EdgeKind::CellMay)));
        }

        // Fix D: the declaration still reaches its `set` sites.
        let from_decl = p.trace_dependents(decl);
        assert!(from_decl.dependents.contains(&write));
        assert!(from_decl.edges.contains(&(decl, write, EdgeKind::CellMay)));
    }

    #[test]
    fn dependents_edge_kinds_distinguish() {
        let env = compiled("var x = 0\nset x = x + 1\nlet y = x * 2\n");
        let p = program(&env);
        let deps = p.trace_dependents(only(ops(p, |o| matches!(o, TermOp::CellNew))));
        assert!(deps.edges.iter().any(|(_, _, k)| *k == EdgeKind::CellMay));
        assert!(deps.edges.iter().any(|(_, _, k)| *k == EdgeKind::Dataflow));
        // A pure value edge is never mislabelled.
        let add = only(ops(p, |o| matches!(o, TermOp::Add)));
        let write = only(ops(p, |o| matches!(o, TermOp::CellWrite)));
        assert!(deps.edges.contains(&(add, write, EdgeKind::Dataflow)));
    }

    /// Fix C: the closure has to be a fixed point. One level of writes gets
    /// b's chain but not a's, and the slice evaluates to 2 instead of 12.
    #[test]
    fn slice_conservative_includes_the_writes() {
        let env = compiled("var a = 0\nvar b = 0\nset a = 5\nset b = a + 1\nlet y = b * 2\n");
        let p = program(&env);
        let y = p.find_term("y").unwrap();
        let (terms, frontier) = p.slice(&[y]).conservative();

        for id in ops(p, |o| matches!(o, TermOp::CellWrite)) {
            assert!(terms.contains(&id), "missing write t{}", id.0);
        }
        for id in ops(p, |o| matches!(o, TermOp::CellNew)) {
            assert!(terms.contains(&id), "missing decl t{}", id.0);
        }
        // `set a = 5` is two cells deep from `y`.
        let five = ops(p, |o| matches!(o, TermOp::CellWrite))
            .into_iter()
            .find(|&w| p.get_term(w).name.as_deref() == Some("a"))
            .unwrap();
        assert_eq!(line_of(p, five), Some(3));
        assert!(terms.contains(&five));
        assert!(terms.contains(&p.get_term(five).inputs[1]));
        assert!(!frontier.is_empty());
        assert!(frontier.iter().any(|f| f.var_name.as_deref() == Some("a")));
        assert!(frontier.iter().any(|f| f.var_name.as_deref() == Some("b")));
    }

    #[test]
    fn slice_minimal_errors_when_a_cell_is_crossed() {
        let env = compiled("var x = 0\nset x = x + 1\nlet y = x * 2\n");
        let p = program(&env);
        let err = p
            .slice(&[p.find_term("y").unwrap()])
            .minimal()
            .expect_err("crosses a cell");
        assert_eq!(err.frontier.len(), 1);
        assert_eq!(err.frontier[0].var_name.as_deref(), Some("x"));
    }

    #[test]
    fn slice_minimal_unchanged_without_cells() {
        let env = compiled("let a = 1\nlet b = a + 2\nlet y = b * 3\n");
        let p = program(&env);
        let y = p.find_term("y").unwrap();
        let minimal = p.slice(&[y]).minimal().expect("no cells");
        let (conservative, frontier) = p.slice(&[y]).conservative();
        assert!(frontier.is_empty());
        assert_eq!(minimal, conservative);
        assert!(minimal.contains(&p.find_term("b").unwrap()));
    }
}
