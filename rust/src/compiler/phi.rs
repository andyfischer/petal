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
use crate::ast::{self, ExprVisitor};

impl Compiler {
    /// Record a cross-block rebinding of `name` to `new_tid` (a term in the
    /// current block). Updates the current scope and the per-block rebind
    /// log so the enclosing conditional can emit a phi term.
    pub(super) fn rebind_name_in_current_block(&mut self, name: String, new_tid: TermId) {
        self.scope_rebind(name.clone(), new_tid);
        self.block_rebinds
            .entry(self.current_block)
            .or_default()
            .insert(name, new_tid);
    }

    /// Rebind `name` to `new_tid` in the current (parent-of-loop-or-branch)
    /// scope, selecting between plain scope_bind and the cross-block rebind
    /// log based on whether the prior outer binding lives in this block.
    /// Shared between phi join emission and carry-phi emission.
    fn rebind_parent(&mut self, name: String, new_tid: TermId, outer_tid: TermId) {
        let outer_block = self.terms[outer_tid.0 as usize].block_id;
        if outer_block == self.current_block {
            self.scope_rebind(name, new_tid);
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
    /// order. Names shadowed by a declaration inside the body are filtered by
    /// the scan itself — see [`AssignedNames`].
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
        let mut v = AssignedNames::new(out);
        for s in stmts {
            v.visit_stmt(s);
        }
    }

    fn collect_assigned_names_expr(e: &Expr, out: &mut Vec<String>) {
        AssignedNames::new(out).visit_expr(e);
    }

    /// Walk an index/field assignment-target object expression down to its
    /// root variable name, if the chain is rooted at a plain variable.
    fn assign_target_root(object: &Expr) -> Option<&str> {
        match &object.kind {
            ExprKind::Ident(n) => Some(n),
            ExprKind::FieldAccess { object, .. } => Self::assign_target_root(object),
            ExprKind::IndexAccess { object, .. } => Self::assign_target_root(object),
            _ => None,
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
    pub(super) fn emit_phis(
        &mut self,
        names: &[String],
        span: SourceSpan,
    ) -> Vec<(String, TermId)> {
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let outer_tid = match self.scope_lookup(name) {
                Some(t) => t,
                None => continue,
            };
            // Before the phi takes over the binding, note whether this name
            // belongs to an enclosing function — afterwards every lookup finds
            // the phi and the fact is unrecoverable (see `cross_fn_terms`).
            let crosses = self.is_outer_function_binding(name);
            let phi_tid = self.emit_term(TermOp::Phi, smallvec![outer_tid], Some(name.clone()));
            self.source_map.add(phi_tid, span);
            if crosses {
                self.cross_fn_terms.insert(phi_tid);
            } else {
                self.propagate_cross_fn(outer_tid, phi_tid);
            }
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
    ///
    /// A name the block shadowed with its own `let`/`state` carries out the
    /// value frozen at the declaration, not whatever the shadowed local ended
    /// up holding (see `note_shadow`).
    pub(super) fn wire_phi_outs(&mut self, body_block: BlockId, phis: &[(String, TermId)]) {
        for (name, phi_tid) in phis {
            if let Some(frozen) = self
                .block_shadowed
                .get(&body_block)
                .and_then(|m| m.get(name))
                .copied()
            {
                if let Some(src_tid) = frozen {
                    self.blocks[body_block.0 as usize].phi_outs.push(PhiOut {
                        src_term: src_tid,
                        dest_term: *phi_tid,
                    });
                }
                continue;
            }
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
                    if blk == body_block { Some(tid) } else { None }
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
    /// names assigned anywhere in `body`, plus any outer-bound names assigned
    /// inside an optional condition expression (for `while` loops). Names the
    /// body declares for itself are already excluded by the scan.
    pub(super) fn detect_loop_carries(
        &self,
        body: &[Stmt],
        extra_cond: Option<&Expr>,
    ) -> Vec<String> {
        let mut carries: Vec<String> = self.detect_rebinds_stmts(&[body]);
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
            self.propagate_cross_fn(*phi_tid, in_tid);
            // If this carried name is a `state` variable, tag the body-entry
            // Copy with its state key. Without this, a reassignment inside the
            // loop body (`s = append(s, x)`) resolves via `find_state_init` to
            // a plain Copy and never emits a `StateWrite`, so the accumulated
            // value lives only in loop registers and is lost when the run ends.
            // Propagating the key makes in-loop reassignment persist to the
            // base state slot, matching how `let` accumulators carry.
            if let Some(init_tid) = self.find_state_init(*phi_tid) {
                self.terms[in_tid.0 as usize].state_key = self.terms[init_tid.0 as usize].state_key;
            }
            self.scope_rebind(name.clone(), in_tid);
            let reg = self.terms[in_tid.0 as usize].register;
            slots.insert(name.clone(), reg);
        }
        slots
    }

    /// Seed a branch/match arm block with an entry `Copy` per phi'd name —
    /// the arm-block counterpart of [`Self::emit_body_phi_ins`]. The copy
    /// (a) initializes the name's carry slot in this block from the parent
    /// phi, and (b) is logged as the block's initial rebind so
    /// `wire_phi_outs` always wires a carry-out. Later in-arm rebinds share
    /// the slot register (via `carry_slots`), so the phi-out src register
    /// holds the name's latest value even on a mid-block exit
    /// (break/continue) where trailing rebinds never executed.
    ///
    /// Names already bound in the innermost scope (match-pattern bindings
    /// that shadow an outer name) are skipped: assignments to them are
    /// arm-local and must not carry out.
    ///
    /// Pushes a `carry_slots` entry for `block`; the caller pops it after
    /// compiling the arm body.
    pub(super) fn seed_arm_entry_copies(&mut self, block: BlockId, phis: &[(String, TermId)]) {
        let mut slots = HashMap::new();
        for (name, phi_tid) in phis {
            if self.scopes.last().is_some_and(|s| s.contains_key(name)) {
                continue;
            }
            let in_tid = self.emit_term(TermOp::Copy, smallvec![*phi_tid], Some(name.clone()));
            self.propagate_cross_fn(*phi_tid, in_tid);
            // Keep state-variable reassignment resolvable through the seed
            // (same reasoning as the loop-body path above).
            if let Some(init_tid) = self.find_state_init(*phi_tid) {
                self.terms[in_tid.0 as usize].state_key = self.terms[init_tid.0 as usize].state_key;
            }
            self.rebind_name_in_current_block(name.clone(), in_tid);
            slots.insert(name.clone(), self.terms[in_tid.0 as usize].register);
        }
        self.carry_slots.push((block, slots));
    }

    /// Look up the carry slot register for `name` in the innermost carrying
    /// block (loop body or seeded branch/match arm) we're currently
    /// compiling, but only when the new term is being emitted directly into
    /// that block. Rebinds in nested sub-blocks get their own seeded slots
    /// (see `seed_arm_entry_copies`) and flow back to this one via
    /// `phi_outs` on child-frame pop / arm exit.
    pub(super) fn carry_slot_for_current_block(&self, name: &str) -> Option<RegisterIndex> {
        let (body_block, slots) = self.carry_slots.last()?;
        if self.current_block != *body_block || self.is_shadowed_in_current_block(name) {
            return None;
        }
        slots.get(name).copied()
    }

    /// Carry the "stands in for an enclosing function's binding" mark from one
    /// term to the term that replaces it as a name's binding.
    pub(super) fn propagate_cross_fn(&mut self, from: TermId, to: TermId) {
        if self.cross_fn_terms.contains(&from) {
            self.cross_fn_terms.insert(to);
        }
    }

    /// Has the current block redeclared `name` with its own `let`/`state`?
    /// Once it has, the name is block-local: rebinds neither share the carry
    /// slot nor update `block_rebinds`.
    pub(super) fn is_shadowed_in_current_block(&self, name: &str) -> bool {
        self.block_shadowed
            .get(&self.current_block)
            .is_some_and(|m| m.contains_key(name))
    }

    /// Record that a `let`/`state` in the current block is about to shadow
    /// `name`, freezing the value the block should carry out for it.
    ///
    /// The pre-scan is lexical, so an assignment *preceding* the declaration is
    /// detected as a rebind of the outer name and gets a phi. Without freezing,
    /// `wire_phi_outs` would then read the block's final binding — the shadowed
    /// local — and carry *its* value out to the outer name. Freezing here means
    /// the pre-shadow assignment carries out and everything after the
    /// declaration stays local, which is what the source says.
    /// See docs/lowering-confusion-20260726.md §3a.
    pub(super) fn note_shadow(&mut self, name: &str) {
        // Only the value that lives in this block can be carried out of it; a
        // binding from an enclosing block means the block contributed nothing
        // before the shadow, so there is no phi_out to wire.
        let frozen = self
            .scope_lookup(name)
            .filter(|tid| self.terms[tid.0 as usize].block_id == self.current_block);
        self.block_shadowed
            .entry(self.current_block)
            .or_default()
            // The first shadow wins: a second `let` of the same name in the
            // block is redeclaring an already-local binding.
            .entry(name.to_string())
            .or_insert(frozen);
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

/// Collects the names an assignment binds — plain reassignments and the *root*
/// variable of a field/index write — for loop-carry / rebind detection. Uses
/// the shared total traversal but overrides two nodes: `fn` declarations and
/// lambda bodies are skipped because they open their own scopes, so their
/// assignments must not be reported as carries of the enclosing scope.
///
/// The walk is **scope-aware**: a name declared by a `let`/`state`, a loop
/// variable, or a match pattern anywhere inside the scanned region shadows the
/// enclosing binding, so assignments to it are local and must not be reported.
/// Without this, a nested `let` whose name happens to collide with an outer
/// binding produces a phi initialized from that outer term — and when the outer
/// term belongs to another function (the core prelude, say), lowering fails
/// with `term tNN in block bN not in this function`. `petal-ui`'s
/// `_wrap_segment` hit exactly that by naming a local `take`, which collides
/// with `std::take`. See docs/lowering-confusion-20260726.md §2.
struct AssignedNames<'a> {
    out: &'a mut Vec<String>,
    /// Names shadowed by a declaration in an enclosing block of the scanned
    /// region. Innermost scope last; the region itself is the first scope.
    shadowed: Vec<Vec<String>>,
}

impl<'a> AssignedNames<'a> {
    fn new(out: &'a mut Vec<String>) -> Self {
        AssignedNames {
            out,
            shadowed: vec![Vec::new()],
        }
    }

    fn push_scope(&mut self) {
        self.shadowed.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.shadowed.pop();
    }

    /// Declare `name` in the innermost scope. Assignments to it from here on
    /// are local and are not reported.
    fn bind(&mut self, name: &str) {
        if let Some(scope) = self.shadowed.last_mut() {
            scope.push(name.to_string());
        }
    }

    fn is_shadowed(&self, name: &str) -> bool {
        self.shadowed.iter().flatten().any(|n| n == name)
    }

    fn push_unique(&mut self, name: &str) {
        if self.is_shadowed(name) || self.out.iter().any(|n| n == name) {
            return;
        }
        self.out.push(name.to_string());
    }

    /// Visit a statement list as its own scope.
    ///
    /// Declarations take effect **from their own line onward**, so an
    /// assignment that lexically precedes a `let` of the same name still
    /// targets the outer binding and is reported. The compiler freezes the
    /// carry-out value at the declaration (`Compiler::note_shadow`), so the
    /// shadowed local's final value never leaks back to the outer name.
    fn visit_stmts(&mut self, stmts: &[Stmt]) {
        self.push_scope();
        for s in stmts {
            self.visit_stmt(s);
        }
        self.pop_scope();
    }

    /// Declare every name a match pattern binds.
    fn bind_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Variable(n) => self.bind(n),
            Pattern::Variant { fields, .. } => {
                for f in fields {
                    self.bind_pattern(f);
                }
            }
            Pattern::List { elements, rest } => {
                for e in elements {
                    self.bind_pattern(e);
                }
                if let Some(r) = rest {
                    self.bind(r);
                }
            }
            Pattern::Record(fields) => {
                for (_, p) in fields {
                    self.bind_pattern(p);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }
}

impl ExprVisitor for AssignedNames<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            // The initializer is walked in the *pre*-declaration scope (it can
            // legitimately read the outer binding), then the name shadows for
            // the rest of this block.
            StmtKind::Let { name, value, .. } => {
                self.visit_expr(value);
                self.bind(name);
            }
            StmtKind::State {
                name, init, key, ..
            } => {
                self.visit_expr(init);
                if let Some(k) = key {
                    self.visit_expr(k);
                }
                self.bind(name);
            }
            // `set` writes a cell, and a cell never needs a phi: the binding
            // holds the cell's *identity*, which no write changes, so there is
            // nothing for a join to reconcile. This is exact rather than
            // approximate — `set` writes `var`s and only `var`s, `=` writes
            // everything else, and `check_write_keyword` makes both directions
            // a hard error. Leaving `set` in here would put a phi on a name the
            // compiler never rebinds, and for a `set` inside a lambda that phi
            // would be initialized from a term in another function: exactly the
            // lowering failure `var` exists to fix.
            // See docs/lowering-confusion-20260726.md §6c.
            StmtKind::Set { value, .. } => {
                self.visit_expr(value);
            }
            StmtKind::Assign {
                target: AssignTarget::Name(n),
                value,
            } => {
                self.push_unique(n);
                self.visit_expr(value);
            }
            StmtKind::Assign {
                target: AssignTarget::Field(object, _) | AssignTarget::Index(object, _),
                value,
            } => {
                // Under value semantics, `obj.f = v` / `xs[i] = v` desugars to a
                // functional rebuild + rebind of the ROOT variable, so the root
                // is reassigned just like a plain `name = v`. It must be detected
                // as a loop carry / rebind, otherwise the in-loop write never
                // reaches the base (state) slot.
                if let Some(root) = Compiler::assign_target_root(object) {
                    self.push_unique(root);
                }
                self.visit_expr(value);
            }
            // A nested `fn` opens its own scope; its assignments don't carry out.
            StmtKind::FnDecl { .. } => {}
            StmtKind::For { var, iter, body } => {
                self.visit_expr(iter);
                self.push_scope();
                self.bind(var);
                for s in body {
                    self.visit_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.visit_stmts(body);
            }
            _ => ast::walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            // Lambdas have their own scope — don't descend into their bodies.
            ExprKind::Lambda { .. } => {}
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expr(condition);
                self.visit_stmts(then_body);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.visit_stmts(stmts),
                    Some(ElseBranch::ElseIf(e)) => self.visit_expr(e),
                    None => {}
                }
            }
            ExprKind::Match { subject, arms } => {
                self.visit_expr(subject);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                    self.pop_scope();
                }
            }
            ExprKind::For { var, iter, body } => {
                self.visit_expr(iter);
                self.push_scope();
                self.bind(var);
                for s in body {
                    self.visit_stmt(s);
                }
                self.pop_scope();
            }
            ExprKind::Block(stmts) => self.visit_stmts(stmts),
            _ => ast::walk_expr(self, e),
        }
    }
}

#[cfg(test)]
mod walker_tests {
    //! Characterization tests for the assigned-name pre-scan walker. These lock
    //! in the exact set of names `collect_assigned_names_stmts` reports — plain
    //! reassignments, the *root* variable of a field/index assignment, dedup in
    //! insertion order, and descent through `if`/`for`/`while` bodies — so a
    //! refactor of the traversal can't silently change loop-carry detection.

    use super::*;
    use crate::rewrite::parse_ast;

    fn assigned(src: &str) -> Vec<String> {
        let (_, stmts) = parse_ast(src).expect("parse");
        let mut out = Vec::new();
        Compiler::collect_assigned_names_stmts(&stmts, &mut out);
        out
    }

    #[test]
    fn plain_name_assignments_collected_in_order_and_deduped() {
        assert_eq!(assigned("x = f(x)\ny = g(y)\nx = h(x)\n"), vec!["x", "y"]);
    }

    #[test]
    fn index_and_field_assignments_collect_the_root_variable() {
        assert_eq!(assigned("xs[0] = double(xs[0])\n"), vec!["xs"]);
        assert_eq!(assigned("p.x = double(p.x)\n"), vec!["p"]);
    }

    #[test]
    fn descends_into_if_for_and_while_bodies() {
        assert_eq!(assigned("if c then\n  x = f(x)\nend\n"), vec!["x"]);
        assert_eq!(
            assigned("for i in range(0, 3) do\n  s = add(s, i)\nend\n"),
            vec!["s"]
        );
        assert_eq!(
            assigned("while gt(n, 0) do\n  n = dec(n)\nend\n"),
            vec!["n"]
        );
    }

    // ---- shadowing (docs/lowering-confusion-20260726.md §2) ----

    #[test]
    fn a_let_in_the_scanned_region_shadows_the_outer_name() {
        assert!(assigned("let x = 1\nx = f(x)\n").is_empty());
    }

    #[test]
    fn a_let_in_a_nested_block_shadows_the_outer_name() {
        // The ui.ptl `_wrap_segment` shape: the `let` is nested one block deep
        // and the assignment one deeper still. Before the scope-aware walk this
        // reported `take`, producing a phi against the outer binding.
        assert!(
            assigned("while c do\n  let take = 2\n  while d do\n    take = take + 1\n  end\nend\n")
                .is_empty()
        );
    }

    #[test]
    fn a_nested_shadow_does_not_hide_a_genuine_carry_of_the_same_name() {
        // `s` is assigned unshadowed in the outer body *and* shadowed inside the
        // `if`. It must still be reported — over-filtering would silently drop a
        // real loop carry, which is worse than the bug being fixed.
        assert_eq!(
            assigned(
                "for i in xs do\n  s = f(s)\n  if c then\n    let s = 0\n    s = g(s)\n  end\nend\n"
            ),
            vec!["s"]
        );
    }

    #[test]
    fn a_declaration_shadows_only_from_its_own_line_onward() {
        // `y = f(y)` lexically targets the OUTER y — the shadow starts on the
        // next line — so it is reported. `Compiler::note_shadow` freezes the
        // carry-out at the `let`, so the shadowed local's later value cannot
        // leak back out. See docs/lowering-confusion-20260726.md §3a.
        assert_eq!(assigned("y = f(y)\nlet y = 1\n"), vec!["y"]);
        assert_eq!(assigned("y = f(y)\nlet y = 1\ny = g(y)\n"), vec!["y"]);
        // A declaration on the first line still shadows everything after it.
        assert!(assigned("let y = 1\ny = f(y)\n").is_empty());
    }

    #[test]
    fn loop_variables_shadow() {
        assert!(assigned("for w in xs do\n  w = f(w)\nend\n").is_empty());
    }

    #[test]
    fn match_pattern_bindings_shadow() {
        // `n` is bound by the arm pattern, so assigning it is arm-local.
        // `outer` is not bound anywhere here, so it is still reported.
        // (An arm body is an expression, so the assignments sit inside `if`s.)
        assert_eq!(
            assigned(
                "match v\n  when Ok(n) -> if c then n = f(n) end\n  when Error(e) -> if c then outer = g(e) end\nend\n"
            ),
            vec!["outer"]
        );
    }

    #[test]
    fn state_declarations_shadow_too() {
        assert!(assigned("state k = 0\nk = k + 1\n").is_empty());
    }
}
