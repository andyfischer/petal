//! Cross-block rebind detection and phi joins.
//!
//! Petal IR blocks are SSA-ish: a name reassigned inside an `if`/`match`
//! branch or a loop body lands in a *child* block, so the parent block needs
//! a `Phi` term to join the possible values. The compiler pre-scans bodies
//! for assigned names, emits phis in the parent block before the control-flow
//! term, and wires `phi_outs` so a popping child frame copies its final
//! binding back into the phi's register.
//!
//! Loops add one more wrinkle: a "carry" name reassigned in the body must be
//! visible to the next iteration and to a `break` mid-body, which is handled
//! by sharing a single body-block register (the carry slot) across rebinds.

use std::collections::HashSet;

use super::*;

impl Compiler {
    /// Record a cross-block rebinding of `name` to `new_tid` (a term in the
    /// current block). Updates the current scope and the per-block rebind
    /// log so the enclosing conditional can emit a phi term.
    pub(super) fn rebind_name_in_current_block(&mut self, name: String, new_tid: TermId) {
        self.scope_bind(name.clone(), new_tid);
        self.block_rebinds
            .entry(self.current_block)
            .or_insert_with(HashMap::new)
            .insert(name, new_tid);
    }

    /// Rebind `name` to `new_tid` in the current (parent-of-loop-or-branch)
    /// scope, selecting between plain scope_bind and the cross-block rebind
    /// log based on whether the prior outer binding lives in this block.
    /// Shared between phi join emission and carry-phi emission.
    fn rebind_parent(&mut self, name: String, new_tid: TermId, outer_tid: TermId) {
        let outer_block = self.terms[outer_tid.0 as usize].block_id;
        if outer_block == self.current_block {
            self.scope_bind(name, new_tid);
        } else {
            self.rebind_name_in_current_block(name, new_tid);
        }
    }

    // -----------------------------------------------------------------------
    // Rebind detection (pre-scan)
    // -----------------------------------------------------------------------

    /// Detect names that will be rebound in one or more child-block bodies
    /// of an enclosing control-flow construct (if/match/for/while). A name
    /// qualifies if it's assigned inside any branch and is already bound in
    /// the current (parent) scope. Returns deduplicated names in insertion
    /// order. Callers filter let-shadowed names per body if needed.
    pub(super) fn detect_rebinds_stmts(&self, bodies: &[&[Stmt]]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for body in bodies {
            let mut assigned: Vec<String> = Vec::new();
            Self::collect_assigned_names_stmts(body, &mut assigned);
            for n in assigned {
                if self.scope_lookup(&n).is_some() && seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        out
    }

    /// Same as `detect_rebinds_stmts` but for expression-shaped bodies
    /// (match arm expressions and while conditions).
    pub(super) fn detect_rebinds_exprs(&self, bodies: &[&Expr]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for e in bodies {
            let mut assigned: Vec<String> = Vec::new();
            Self::collect_assigned_names_expr(e, &mut assigned);
            for n in assigned {
                if self.scope_lookup(&n).is_some() && seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        out
    }

    fn collect_assigned_names_stmts(stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            match &s.kind {
                StmtKind::Assign {
                    target: AssignTarget::Name(n),
                    value,
                } => {
                    if !out.contains(n) {
                        out.push(n.clone());
                    }
                    Self::collect_assigned_names_expr(value, out);
                }
                StmtKind::Assign { value, .. } => {
                    Self::collect_assigned_names_expr(value, out);
                }
                StmtKind::Let { value, .. } => {
                    Self::collect_assigned_names_expr(value, out);
                }
                StmtKind::Expr(e) => Self::collect_assigned_names_expr(e, out),
                StmtKind::For { iter, body, .. } => {
                    Self::collect_assigned_names_expr(iter, out);
                    Self::collect_assigned_names_stmts(body, out);
                }
                StmtKind::While { condition, body } => {
                    Self::collect_assigned_names_expr(condition, out);
                    Self::collect_assigned_names_stmts(body, out);
                }
                StmtKind::Return(Some(e)) => Self::collect_assigned_names_expr(e, out),
                StmtKind::State { init, key, .. } => {
                    Self::collect_assigned_names_expr(init, out);
                    if let Some(k) = key {
                        Self::collect_assigned_names_expr(k, out);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_assigned_names_expr(e: &Expr, out: &mut Vec<String>) {
        match &e.kind {
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                Self::collect_assigned_names_expr(condition, out);
                Self::collect_assigned_names_stmts(then_body, out);
                if let Some(eb) = else_body {
                    match eb {
                        ElseBranch::Block(stmts) => Self::collect_assigned_names_stmts(stmts, out),
                        ElseBranch::ElseIf(e) => Self::collect_assigned_names_expr(e, out),
                    }
                }
            }
            ExprKind::Match { subject, arms } => {
                Self::collect_assigned_names_expr(subject, out);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        Self::collect_assigned_names_expr(g, out);
                    }
                    Self::collect_assigned_names_expr(&arm.body, out);
                }
            }
            ExprKind::Block(stmts) => Self::collect_assigned_names_stmts(stmts, out),
            // Don't descend into lambdas — they have their own scope.
            ExprKind::Lambda { .. } => {}
            ExprKind::BinaryOp { left, right, .. } => {
                Self::collect_assigned_names_expr(left, out);
                Self::collect_assigned_names_expr(right, out);
            }
            ExprKind::UnaryOp { operand, .. } => {
                Self::collect_assigned_names_expr(operand, out);
            }
            ExprKind::Call { function, args } => {
                Self::collect_assigned_names_expr(function, out);
                for a in args {
                    Self::collect_assigned_names_expr(a, out);
                }
            }
            ExprKind::List(elems) => {
                for el in elems {
                    Self::collect_assigned_names_expr(el, out);
                }
            }
            ExprKind::Record(fields) => {
                for f in fields {
                    match f {
                        RecordField::Named(_, e) => Self::collect_assigned_names_expr(e, out),
                        RecordField::Spread(e) => Self::collect_assigned_names_expr(e, out),
                    }
                }
            }
            ExprKind::FieldAccess { object, .. } => {
                Self::collect_assigned_names_expr(object, out);
            }
            ExprKind::IndexAccess { object, index } => {
                Self::collect_assigned_names_expr(object, out);
                Self::collect_assigned_names_expr(index, out);
            }
            ExprKind::StringInterp { exprs, .. } => {
                for e in exprs {
                    Self::collect_assigned_names_expr(e, out);
                }
            }
            ExprKind::Element { props, children, .. } => {
                for (_, e) in props {
                    Self::collect_assigned_names_expr(e, out);
                }
                for c in children {
                    if let JsxChild::Expr(e) = c {
                        Self::collect_assigned_names_expr(e, out);
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_let_names(stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            if let StmtKind::Let { name, .. } = &s.kind {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Phi emission and wiring
    // -----------------------------------------------------------------------

    /// Emit a `Phi` term in the current (parent) block for each name to be
    /// joined. Placed *before* the upcoming control-flow term so the phi's
    /// own exec initializes its register from the pre-control-flow value;
    /// child frames that rebind the name will overwrite via `phi_outs` on
    /// pop. Rebinds the parent-scope binding of the name to the phi term.
    /// Returns `(name, phi_tid)` pairs for later wiring via `wire_phi_outs`.
    pub(super) fn emit_phis(&mut self, names: &[String], span: SourceSpan) -> Vec<(String, TermId)> {
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let outer_tid = match self.scope_lookup(name) {
                Some(t) => t,
                None => continue,
            };
            let phi_tid = self.emit_term(TermOp::Phi, smallvec![outer_tid], Some(name.clone()));
            self.source_map.add(phi_tid, span);
            // If this phi is landing in an enclosing loop's body block and
            // joins an outer carry name, rewrite its register to the shared
            // carry slot so nested-branch rebinds propagate through to the
            // loop's own phi via a single register.
            if let Some(slot) = self.carry_slot_for_current_block(name) {
                self.terms[phi_tid.0 as usize].register = slot;
            }
            self.rebind_parent(name.clone(), phi_tid, outer_tid);
            out.push((name.clone(), phi_tid));
        }
        out
    }

    /// Wire `phi_outs` for a child block: for each phi, if the body
    /// rebound the name, its popping frame copies the final binding back
    /// to the phi's register. Handles both conditional-branch callers
    /// (scope already popped → read from `block_rebinds`) and loop-body
    /// callers (scope still live → read via `scope_lookup`). Branches
    /// that don't rebind a phi'd name don't get a phi_out, so the phi
    /// keeps its init value.
    pub(super) fn wire_phi_outs(&mut self, body_block: BlockId, phis: &[(String, TermId)]) {
        for (name, phi_tid) in phis {
            let src = self
                .block_rebinds
                .get(&body_block)
                .and_then(|m| m.get(name).copied())
                .or_else(|| {
                    // Loop-body path: scope_lookup finds the final body
                    // binding, but only if it lives in the body block
                    // (not the parent-scope phi we just rebound to).
                    let tid = self.scope_lookup(name)?;
                    let blk = self.terms[tid.0 as usize].block_id;
                    if blk == body_block {
                        Some(tid)
                    } else {
                        None
                    }
                });
            if let Some(src_tid) = src {
                self.blocks[body_block.0 as usize].phi_outs.push(PhiOut {
                    src_term: src_tid,
                    dest_term: *phi_tid,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Loop carries
    // -----------------------------------------------------------------------

    /// Compute the set of loop-carry names for a for/while body: outer-bound
    /// names assigned anywhere in `body`, minus those shadowed by a top-level
    /// `let` in the body, plus any outer-bound names assigned inside an
    /// optional condition expression (for `while` loops).
    pub(super) fn detect_loop_carries(
        &self,
        body: &[Stmt],
        extra_cond: Option<&Expr>,
    ) -> Vec<String> {
        let mut let_bound: Vec<String> = Vec::new();
        Self::collect_let_names(body, &mut let_bound);
        let mut carries: Vec<String> = self
            .detect_rebinds_stmts(&[body])
            .into_iter()
            .filter(|n| !let_bound.contains(n))
            .collect();
        if let Some(cond) = extra_cond {
            for n in self.detect_rebinds_exprs(&[cond]) {
                if !carries.contains(&n) {
                    carries.push(n);
                }
            }
        }
        carries
    }

    /// Seed body-local read terms at the start of a loop body block for
    /// each phi. Each iteration re-runs these Copy terms to snapshot the
    /// current phi register value; subsequent body rebindings chain off
    /// these as same-block SSA rebinds. Returns `(name, slot_register)`
    /// pairs so the caller can install a carry-slot entry that rewrites
    /// later body-block rebinds of each name to share this register.
    fn emit_body_phi_ins(&mut self, phis: &[(String, TermId)]) -> HashMap<String, RegisterIndex> {
        let mut slots = HashMap::new();
        for (name, phi_tid) in phis {
            let in_tid = self.emit_term(TermOp::Copy, smallvec![*phi_tid], Some(name.clone()));
            self.scope_bind(name.clone(), in_tid);
            let reg = self.terms[in_tid.0 as usize].register;
            slots.insert(name.clone(), reg);
        }
        slots
    }

    /// Look up the carry slot register for `name` in the innermost loop
    /// body we're currently compiling, but only when the new term is being
    /// emitted directly into that body block. Rebinds in nested sub-blocks
    /// (conditional branches inside the body) keep their own registers and
    /// flow back to the slot via `phi_outs` on child-frame pop.
    pub(super) fn carry_slot_for_current_block(&self, name: &str) -> Option<RegisterIndex> {
        let (body_block, slots) = self.carry_slots.last()?;
        if self.current_block != *body_block {
            return None;
        }
        slots.get(name).copied()
    }

    /// Compile the body of a for/while loop. Manages loop-depth tracking,
    /// scope lifecycle, carry-slot bookkeeping, phi-out wiring, and block
    /// finalization. Optionally binds a loop variable phantom at the start
    /// of the body so `for` loops can name their iterator binding — pass
    /// `None` for `while` bodies.
    pub(super) fn compile_loop_body(
        &mut self,
        body_block: BlockId,
        body: &[Stmt],
        phis: &[(String, TermId)],
        loop_var: Option<&str>,
    ) {
        self.loop_depth += 1;
        let saved = self.set_block(body_block);
        self.push_scope(false);

        if let Some(name) = loop_var {
            let var_tid = self.emit_phantom_term(name.to_string());
            self.scope_bind(name.to_string(), var_tid);
        }

        let slots = self.emit_body_phi_ins(phis);
        self.carry_slots.push((body_block, slots));

        for s in body {
            self.compile_stmt(s);
        }

        self.wire_phi_outs(body_block, phis);
        self.carry_slots.pop();

        self.finalize_block(body_block);
        self.pop_scope();
        self.set_block(saved);
        self.loop_depth -= 1;
    }
}
