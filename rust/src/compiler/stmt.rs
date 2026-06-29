//! Statement compilation: let / assign / loops / state / declarations.

use super::*;

/// One step of an assignment-target path, borrowing from the AST.
enum AssignStep<'a> {
    Field(&'a str),
    Index(&'a Expr),
}

/// A path step after compilation: a field name interned as a constant, or an
/// index expression compiled to a term.
enum CompiledStep {
    Field(crate::constant_table::ConstantId),
    Index(TermId),
}

impl Compiler {
    pub(super) fn compile_stmt(&mut self, stmt: &Stmt) {
        let stmt_span = stmt.span;
        match &stmt.kind {
            StmtKind::Let { name, value } => {
                let val_tid = self.compile_expr(value);
                self.terms[val_tid.0 as usize].name = Some(name.clone());
                self.scope_bind(name.clone(), val_tid);
            }

            StmtKind::Assign { target, value } => {
                self.compile_assign(target, value);
            }

            StmtKind::Expr(expr) => {
                self.compile_expr(expr);
            }

            StmtKind::FnDecl { name, params, body } => {
                self.compile_fn_decl(name, params, body);
            }

            StmtKind::EnumDecl { name: _, variants } => {
                for variant in variants {
                    if variant.fields.is_empty() {
                        // Fieldless variant — store as a constant enum value
                        let name_const = self
                            .constants
                            .intern(ConstantValue::String(variant.name.clone()));
                        let tid = self.emit_term(
                            TermOp::MakeEnumVariant(name_const),
                            smallvec![],
                            Some(variant.name.clone()),
                        );
                        self.scope_bind(variant.name.clone(), tid);
                    } else {
                        // Variant with fields — create a constructor function
                        let constructor_tid = self.compile_enum_constructor(variant);
                        self.scope_bind(variant.name.clone(), constructor_tid);
                    }
                }
            }

            StmtKind::For { var, iter, body } => {
                // Fast path: `for i in range(a, b)` / `for i in range(n)`
                // lowers to a NumericForLoop that iterates an integer counter
                // with no list allocation. Everything after the op selection
                // is identical to the generic ForLoop path, so per-iteration
                // state, loop-carried phis, break, and continue behave the
                // same on both.
                let (op, loop_inputs) = match self.try_range_bounds(iter) {
                    Some((start_tid, end_tid)) => {
                        (TermOp::NumericForLoop, smallvec![start_tid, end_tid])
                    }
                    None => (TermOp::ForLoop, smallvec![self.compile_expr(iter)]),
                };

                let carries = self.detect_loop_carries(body, None);
                let phis = self.emit_phis(&carries, stmt_span);

                let body_block = self.new_block(None);
                self.blocks[body_block.0 as usize].param_names = vec![var.clone()];

                let for_tid =
                    self.emit_term_with_children(op, loop_inputs, None, smallvec![body_block]);
                self.blocks[body_block.0 as usize].parent_term_id = Some(for_tid);

                self.compile_loop_body(body_block, body, &phis, Some(var));
            }

            StmtKind::While { condition, body } => {
                let carries = self.detect_loop_carries(body, Some(condition));
                let phis = self.emit_phis(&carries, stmt_span);

                let cond_block = self.new_block(None);
                let body_block = self.new_block(None);

                let while_tid = self.emit_term_with_children(
                    TermOp::WhileLoop,
                    smallvec![],
                    None,
                    smallvec![cond_block, body_block],
                );
                self.blocks[cond_block.0 as usize].parent_term_id = Some(while_tid);
                self.blocks[body_block.0 as usize].parent_term_id = Some(while_tid);

                // Condition reads carry names via parent_frame walk to the
                // phi's register; nothing carry-specific to set up here.
                self.compile_in_block(cond_block, |c| {
                    c.compile_expr(condition);
                });

                self.compile_loop_body(body_block, body, &phis, None);
            }

            StmtKind::Return(expr) => {
                if let Some(e) = expr {
                    let val_tid = self.compile_expr(e);
                    self.emit_term(TermOp::Return, smallvec![val_tid], None);
                } else {
                    self.emit_term(TermOp::Return, smallvec![], None);
                }
            }

            StmtKind::Break => {
                self.emit_term(TermOp::Break, smallvec![], None);
            }

            StmtKind::Continue => {
                self.emit_term(TermOp::Continue, smallvec![], None);
            }

            StmtKind::State { name, init, id: _, key } => {
                self.compile_state_decl(name, init, key.as_ref());
            }
        }
    }

    /// `state name = init` / `state(key) name = init`.
    ///
    /// Lazy initialization: the init expression lives in a child block that
    /// is only entered the first time the (state_key, loop_indices) tuple is
    /// encountered. The explicit key (if any) is computed eagerly in the
    /// parent block — its value determines which slot to consult.
    fn compile_state_decl(&mut self, name: &str, init: &Expr, key: Option<&Expr>) {
        let state_key_const = StateKey(Self::hash_state_name(name));
        let key_tid = key.map(|key_expr| self.compile_expr(key_expr));

        // StateInit term sits in the current block. Inputs hold only
        // the (optional) explicit key. The init value is delivered
        // via the child block's last term value (see eval).
        let mut inputs: SmallVec<[TermId; 4]> = smallvec![];
        if let Some(k) = key_tid {
            inputs.push(k);
        }
        let state_tid = self.emit_term(TermOp::StateInit, inputs, Some(name.to_string()));
        self.terms[state_tid.0 as usize].state_key = Some(state_key_const);
        self.terms[state_tid.0 as usize].in_loop = self.loop_depth > 0;
        self.state_inits.insert(state_key_const, state_tid);

        // Compile the init expression into a fresh child block. The
        // init block's last term register is read on pop and copied
        // to StateInit's register (return_term mechanism).
        let init_block = self.new_block(Some(state_tid));
        self.terms[state_tid.0 as usize].child_blocks = smallvec![init_block];
        self.compile_in_block(init_block, |c| {
            c.compile_expr(init);
        });

        self.scope_bind(name.to_string(), state_tid);
    }

    /// If `iter` is literally a call to `range(...)` with 1 or 2 arguments,
    /// compile its bound expressions and return `(start_tid, end_tid)` for a
    /// NumericForLoop. For `range(n)` the start is a synthesized `Constant(0)`.
    /// Returns `None` for any other iterable (the caller falls back to the
    /// generic ForLoop path). Only the for-loop-iterable position is special-
    /// cased — `range` used anywhere else still goes through the builtin.
    fn try_range_bounds(&mut self, iter: &Expr) -> Option<(TermId, TermId)> {
        let ExprKind::Call { function, args } = &iter.kind else {
            return None;
        };
        let ExprKind::Ident(name) = &function.kind else {
            return None;
        };
        if name != "range" {
            return None;
        }
        match args.len() {
            1 => {
                let end_tid = self.compile_expr(&args[0]);
                let zero = self.constants.intern(ConstantValue::Int(0));
                let start_tid = self.emit_term(TermOp::Constant(zero), smallvec![], None);
                Some((start_tid, end_tid))
            }
            2 => {
                let start_tid = self.compile_expr(&args[0]);
                let end_tid = self.compile_expr(&args[1]);
                Some((start_tid, end_tid))
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Assignment compilation
    // -----------------------------------------------------------------------

    fn compile_assign(&mut self, target: &AssignTarget, value: &Expr) {
        match target {
            AssignTarget::Name(name) => self.compile_assign_name(name, value),
            AssignTarget::Field(object, field) => {
                match Self::resolve_assign_target(object) {
                    Some((root, mut steps)) => {
                        steps.push(AssignStep::Field(field));
                        self.compile_path_assign(root, steps, value);
                    }
                    None => self.emit_dead_store_error(),
                }
            }
            AssignTarget::Index(object, index) => match Self::resolve_assign_target(object) {
                Some((root, mut steps)) => {
                    steps.push(AssignStep::Index(index));
                    self.compile_path_assign(root, steps, value);
                }
                None => self.emit_dead_store_error(),
            },
        }
    }

    /// Walk an assignment-target object expression into a (root variable name,
    /// steps) pair, where each step is a field or index applied left-to-right
    /// from the root. Returns `None` if the chain is not rooted at a plain
    /// variable (e.g. `foo()[0] = v`), which is a dead store under value
    /// semantics.
    fn resolve_assign_target(object: &Expr) -> Option<(&str, Vec<AssignStep<'_>>)> {
        match &object.kind {
            ExprKind::Ident(n) => Some((n, vec![])),
            ExprKind::FieldAccess { object, field } => {
                let (root, mut steps) = Self::resolve_assign_target(object)?;
                steps.push(AssignStep::Field(field));
                Some((root, steps))
            }
            ExprKind::IndexAccess { object, index } => {
                let (root, mut steps) = Self::resolve_assign_target(object)?;
                steps.push(AssignStep::Index(index));
                Some((root, steps))
            }
            _ => None,
        }
    }

    fn emit_dead_store_error(&mut self) {
        let msg = "Assignment target must be rooted at a variable; assigning into a \
                   temporary value (e.g. the result of a call) has no effect under \
                   value semantics"
            .to_string();
        let msg_cid = self.constants.intern(ConstantValue::String(msg));
        self.emit_term(TermOp::Error(msg_cid), smallvec![], None);
    }

    /// Compile `root.<steps> = value` as a functional update + rebind of the
    /// root variable (value semantics): rebuild each collection along the path
    /// bottom-up, then rebind `root` to the new top-level collection.
    fn compile_path_assign(&mut self, root: &str, steps: Vec<AssignStep>, value: &Expr) {
        let n = steps.len();
        debug_assert!(n >= 1);

        let val_tid = self.compile_expr(value);

        // Compile each step once: field name -> constant, index expr -> term.
        let csteps: Vec<CompiledStep> = steps
            .iter()
            .map(|step| match step {
                AssignStep::Field(name) => {
                    CompiledStep::Field(self.constants.intern(ConstantValue::String((*name).to_string())))
                }
                AssignStep::Index(expr) => CompiledStep::Index(self.compile_expr(expr)),
            })
            .collect();

        // Reads for the intermediate collections: read[0] is the root variable
        // (resolved through scope, like an `Ident` reference), read[i] is the
        // value obtained by applying step[i-1] to read[i-1]. We only need
        // reads for levels 0..n-1 (the leaf level is overwritten, not read).
        let mut reads: Vec<TermId> = Vec::with_capacity(n);
        reads.push(self.compile_ident(root));
        for i in 0..n - 1 {
            let prev = reads[i];
            let read = match &csteps[i] {
                CompiledStep::Field(cid) => {
                    self.emit_term(TermOp::GetField(*cid), smallvec![prev], None)
                }
                CompiledStep::Index(idx) => {
                    self.emit_term(TermOp::GetIndex, smallvec![prev, *idx], None)
                }
            };
            reads.push(read);
        }

        // Build the new collections bottom-up. The leaf write replaces the
        // element at the deepest level; each enclosing level is rebuilt with
        // the freshly-built inner collection.
        let mut new_val = self.emit_set(&csteps[n - 1], reads[n - 1], val_tid);
        for i in (0..n - 1).rev() {
            new_val = self.emit_set(&csteps[i], reads[i], new_val);
        }

        // Rebind the root variable to the new top-level collection, routing
        // through the same machinery as plain name assignment so state writes
        // and loop-carry phis are handled identically.
        self.rebind_name(root, new_val);
    }

    /// Emit a functional-update term for one path step: `SetField`/`SetIndex`
    /// of `val` into `obj` at the step's field/index.
    fn emit_set(&mut self, step: &CompiledStep, obj: TermId, val: TermId) -> TermId {
        match step {
            CompiledStep::Field(cid) => {
                self.emit_term(TermOp::SetField(*cid), smallvec![obj, val], None)
            }
            CompiledStep::Index(idx) => {
                self.emit_term(TermOp::SetIndex, smallvec![obj, *idx, val], None)
            }
        }
    }

    fn compile_assign_name(&mut self, name: &str, value: &Expr) {
        let val_tid = self.compile_expr(value);
        self.rebind_name(name, val_tid);
    }

    /// Rebind variable `name` to the already-compiled value `val_tid`.
    ///
    /// Shared by plain name assignment (`x = v`) and index/field assignment
    /// (`x[i] = v`, `x.f = v`), which under value semantics desugars to a
    /// functional rebuild followed by a rebind of the root variable. Emits a
    /// `StateWrite` when the root is a state variable so in-loop reassignment
    /// persists across runs, shares the loop carry slot, and records the
    /// rebind so an enclosing conditional / loop can emit a phi join.
    pub(super) fn rebind_name(&mut self, name: &str, val_tid: TermId) {
        // Check if this is a state variable — if so, emit StateWrite.
        // Walk through Phi/Copy nodes so an assignment inside an
        // `if` / loop body, or a chain of repeat reassignments at
        // the top level, still finds the underlying StateInit.
        let mut state_init_for_copy: Option<StateKey> = None;
        if let Some(existing_tid) = self.scope_lookup(name)
            && let Some(init_tid) = self.find_state_init(existing_tid)
        {
            let state_key = self.terms[init_tid.0 as usize].state_key;
            let in_loop = self.terms[init_tid.0 as usize].in_loop;
            // StateInit's inputs are [explicit_key]? (the init value
            // lives in a child block for lazy evaluation). Forward the
            // key to StateWrite so the runtime resolves to the same
            // RuntimeStateKey.
            let mut write_inputs: SmallVec<[TermId; 4]> = smallvec![val_tid];
            if let Some(&key_input) = self.terms[init_tid.0 as usize].inputs.first() {
                write_inputs.push(key_input);
            }
            let write_tid = self.emit_term(TermOp::StateWrite, write_inputs, None);
            self.terms[write_tid.0 as usize].state_key = state_key;
            self.terms[write_tid.0 as usize].in_loop = in_loop;
            // Propagate the state key onto the Copy below so the
            // next reassignment can still resolve to the StateInit
            // (the Copy replaces the existing scope binding).
            state_init_for_copy = state_key;
        }

        // Always emit a fresh Copy term + rebind. If the name was
        // bound in an outer block, record the rebind so the enclosing
        // conditional / loop can emit a phi join.
        let assign_tid = self.emit_term(TermOp::Copy, smallvec![val_tid], Some(name.to_string()));
        if let Some(key) = state_init_for_copy {
            self.terms[assign_tid.0 as usize].state_key = Some(key);
        }
        // Carry-slot share: when this assign is the body of a loop
        // that carries `name`, rewrite its register to the shared
        // slot so every body-level rebind writes to the same
        // register (see `carry_slots`). This keeps the slot up to
        // date even if `break` fires before a later rebind.
        if let Some(slot) = self.carry_slot_for_current_block(name) {
            self.terms[assign_tid.0 as usize].register = slot;
        }
        if let Some(existing_tid) = self.scope_lookup(name) {
            let existing_block = self.terms[existing_tid.0 as usize].block_id;
            // A name that already has a rebind logged in this block crossed a
            // block boundary on its first reassignment here. Subsequent
            // in-block reassignments must keep `block_rebinds` pointing at the
            // *latest* binding, otherwise the enclosing conditional's phi-out
            // wires from the first rebind and later writes are dropped (e.g.
            // two `append`s to a loop-carried var inside an `if`).
            let already_rebound_here = self
                .block_rebinds
                .get(&self.current_block)
                .is_some_and(|m| m.contains_key(name));
            if existing_block == self.current_block && !already_rebound_here {
                self.scope_bind(name.to_string(), assign_tid);
            } else {
                self.rebind_name_in_current_block(name.to_string(), assign_tid);
            }
        } else {
            self.scope_bind(name.to_string(), assign_tid);
        }
    }

    /// Walk through `Phi` terms (following `inputs[0]`, which points to the
    /// pre-control-flow binding) to find an underlying `StateInit` term, if
    /// any. Used by `compile_assign` so that assignments to a state variable
    /// inside an `if` / loop body still emit a `StateWrite` — the scope
    /// lookup returns the phi that was installed by the enclosing control
    /// flow, not the original `StateInit`.
    pub(super) fn find_state_init(&self, tid: TermId) -> Option<TermId> {
        let term = &self.terms[tid.0 as usize];
        match &term.op {
            TermOp::StateInit => Some(tid),
            TermOp::Phi => {
                let input = *term.inputs.first()?;
                self.find_state_init(input)
            }
            // A `Copy` produced by reassignment of a state variable carries
            // the same `state_key` as the original `StateInit`. Use it to
            // jump back to the init term — walking `inputs[0]` would lead
            // to the assigned value, not the previous binding.
            TermOp::Copy => {
                let key = term.state_key?;
                self.state_inits.get(&key).copied()
            }
            _ => None,
        }
    }
}
