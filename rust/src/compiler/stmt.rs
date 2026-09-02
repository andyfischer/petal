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
    /// Compile a statement list. `value_used` says the list's final statement
    /// produces the enclosing construct's value — a function body (implicit
    /// return), a branch of an `if` whose value is consumed, or the body of a
    /// collecting `for`. Only there does a trailing bare `for` collect into a
    /// list; everywhere else it stays a zero-allocation side-effect loop.
    pub(super) fn compile_stmts(&mut self, stmts: &[Stmt], value_used: bool) {
        let saved = self.stmt_value_used;
        for (i, s) in stmts.iter().enumerate() {
            self.stmt_value_used = value_used && i + 1 == stmts.len();
            self.compile_stmt(s);
        }
        self.stmt_value_used = saved;
    }

    pub(super) fn compile_stmt(&mut self, stmt: &Stmt) {
        let stmt_span = stmt.span;
        // Taken here so it applies only to this statement: any nested list
        // compiled below re-establishes its own value positions.
        let stmt_value_used = std::mem::take(&mut self.stmt_value_used);
        match &stmt.kind {
            StmtKind::Let {
                name,
                value,
                is_var,
                is_config,
                ..
            } => {
                let val_tid = self.compile_bound_expr(name, value);
                self.note_shadow(name);
                if *is_var {
                    // The binding is the *cell*, not the initial value. Every
                    // read of the name dereferences it and every `set` writes
                    // through it, so the name is never rebound to a new term —
                    // which is precisely why a `var` needs no phi and works
                    // across function and control-flow boundaries.
                    let cell_tid =
                        self.emit_term(TermOp::CellNew, smallvec![val_tid], Some(name.clone()));
                    self.scope_bind_var(name.clone(), cell_tid);
                } else {
                    self.terms[val_tid.0 as usize].name = Some(name.clone());
                    // The flag rides on the binding's value term — for a
                    // `config let` with a literal initializer that is exactly
                    // the term a proposal edits.
                    self.terms[val_tid.0 as usize].is_config = *is_config;
                    self.scope_bind(name.clone(), val_tid);
                }
            }

            StmtKind::Assign { target, value } => {
                self.compile_assign(target, value, stmt_span, false);
            }

            StmtKind::Set { target, value } => {
                self.compile_assign(target, value, stmt_span, true);
            }

            StmtKind::Expr(expr) => {
                // A statement-level expression's value is discarded unless the
                // statement is in tail position; an `if`/`match`/block passes
                // that on to its own branches.
                self.value_used = stmt_value_used;
                self.compile_expr(expr);
            }

            StmtKind::FnDecl {
                name,
                class,
                params,
                body,
                ..
            } => {
                // A hoisted declaration was already compiled by the prescan,
                // ahead of the file's first statement — compiling it again
                // would emit a second closure and, for an overloaded name, a
                // second variant.
                if self.hoisted_fn_decls.contains(&stmt_span) {
                    return;
                }
                // Declared parameter types are not yet used at compile time
                // (checking lands in a later chunk); the compiler only needs the
                // names.
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                let bound = self.compile_fn_decl(name, &param_names, body, stmt_span.end.offset);
                // A declaration's term is where the declaration is written.
                // Without this every `<function>` in a provenance chain reads
                // `[no location]` — the one entry a reader most needs to find.
                if let Some(tid) = bound {
                    self.source_map.add(tid, stmt_span);
                }
                // A method also has to be *findable* by the receiver's class at
                // a `r.center_x()` call site, where the name in scope is not
                // consulted at all. Registration is a statement in the root
                // block, so it happens in source order exactly as the binding
                // does — a method, like a function, is callable from the point
                // its declaration runs.
                if let (Some(class), Some(tid)) = (class, bound) {
                    let method = crate::compiler::method_base_name(&stmt.kind).to_string();
                    self.emit_declare_method(class, &method, tid);
                }
            }

            StmtKind::ClassDecl { .. } => {
                // Nothing to emit: the class name binds to its constructor
                // (`Point(1, 2)` is an ordinary call, its fields the positional
                // parameters), and `prescan_declarations` already compiled and
                // bound that constructor ahead of the file's first statement.
                // A class declaration is hoisted, like the type name it
                // introduces. The prescan records the constructor's span too,
                // so a provenance chain still points at the `class` line.
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
                        self.source_map.add(tid, stmt_span);
                        self.scope_bind(variant.name.clone(), tid);
                    } else {
                        // Variant with fields — create a constructor function
                        let constructor_tid = self.compile_enum_constructor(variant);
                        self.source_map.add(constructor_tid, stmt_span);
                        self.scope_bind(variant.name.clone(), constructor_tid);
                    }
                }
            }

            StmtKind::For { var, iter, body } => {
                // Statement form: a side-effect loop that allocates nothing,
                // unless it sits in tail position — an implicit return or the
                // last statement of a value-position branch — where the
                // documented value-position rule makes it a mapping.
                self.compile_for(var, iter, body, stmt_value_used, stmt_span);
            }

            StmtKind::While { condition, body } => {
                self.compile_while(condition, body, stmt_span);
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

            StmtKind::State {
                name,
                // Annotations are compile-time only; codegen drops them, as it
                // does for `let`/`fn`.
                ty: _,
                init,
                key,
                is_var,
            } => {
                self.compile_state_decl(name, init, key.as_ref(), *is_var, stmt_span);
            }

            // Imports are extracted and resolved by the module loader before
            // compilation (see `Compiler::bind_imports`); one reaching here
            // (legacy single-file `compile`) has nothing left to do.
            StmtKind::Import(_) => {}
        }
    }

    /// Compile a `for … in … do … end` loop, shared by the statement form
    /// (`compile_stmt`, `collect = false`) and the expression form
    /// (`compile_expr`, `collect = true`). When `collect` is set the loop term
    /// evaluates to a list built from each iteration's last expression (a
    /// mapping); otherwise it runs purely for side effects and allocates
    /// nothing. Returns the loop control term.
    ///
    /// Fast path: `for i in range(a, b)` / `for i in range(n)` lowers to a
    /// `NumericForLoop` that iterates an integer counter with no list
    /// allocation. Everything after the op selection is identical to the
    /// generic `ForLoop` path, so per-iteration state, loop-carried phis,
    /// break, continue, and collection behave the same on both.
    pub(super) fn compile_for(
        &mut self,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        collect: bool,
        span: SourceSpan,
    ) -> TermId {
        let (op, loop_inputs) = match self.try_range_bounds(iter) {
            Some((start_tid, end_tid)) => (TermOp::NumericForLoop, smallvec![start_tid, end_tid]),
            None => (TermOp::ForLoop, smallvec![self.compile_expr(iter)]),
        };

        let carries = self.detect_loop_carries(body, None);
        let phis = self.emit_phis(&carries, span);

        let body_block = self.new_block(None);
        self.blocks[body_block.0 as usize].param_names = vec![var.to_string()];

        let for_tid = self.emit_term_with_children(op, loop_inputs, None, smallvec![body_block]);
        self.terms[for_tid.0 as usize].collect = collect;
        self.blocks[body_block.0 as usize].parent_term_id = Some(for_tid);

        self.compile_loop_body(body_block, body, &phis, Some(var), collect);
        for_tid
    }

    /// Compile a `while … do … end` loop. `while` is statement-only (no
    /// collecting expression form), so it never yields a list.
    pub(super) fn compile_while(
        &mut self,
        condition: &Expr,
        body: &[Stmt],
        span: SourceSpan,
    ) -> TermId {
        let carries = self.detect_loop_carries(body, Some(condition));
        let phis = self.emit_phis(&carries, span);

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

        // Condition reads carry names via parent_frame walk to the phi's
        // register; nothing carry-specific to set up here.
        self.compile_in_block(cond_block, |c| {
            c.compile_expr(condition);
        });

        // `while` has no collecting form, so its body's last statement is never
        // in value position.
        self.compile_loop_body(body_block, body, &phis, None, false);
        while_tid
    }

    /// `state name = init` / `state(key) name = init`.
    ///
    /// Lazy initialization: the init expression lives in a child block that is
    /// only entered the first time this declaration is reached on a given call
    /// path (the `(decl id, path)` slot). The explicit key (if any) is computed
    /// eagerly in the parent block — its value alone determines which slot to
    /// consult, since a keyed slot ignores the path
    /// (docs/dev/state-call-paths.md §2.2).
    fn compile_state_decl(
        &mut self,
        name: &str,
        init: &Expr,
        key: Option<&Expr>,
        is_var: bool,
        span: SourceSpan,
    ) {
        // The declaration id folds in the module, the enclosing functions and a
        // shadow ordinal (see `state_key_for`), so every declaration site owns
        // its own slot. The term's display name is module-qualified so state
        // JSON / diffs stay unambiguous when two modules declare one name.
        let state_key_const = self.state_key_for(name);
        let key_tid = key.map(|key_expr| self.compile_expr(key_expr));

        // StateInit term sits in the current block. Inputs hold only
        // the (optional) explicit key. The init value is delivered
        // via the child block's last term value (see eval).
        let mut inputs: SmallVec<[TermId; 4]> = smallvec![];
        if let Some(k) = key_tid {
            inputs.push(k);
        }
        let state_tid = self.emit_term(TermOp::StateInit, inputs, Some(self.qualified_name(name)));
        self.terms[state_tid.0 as usize].state_key = Some(state_key_const);
        // Remember how deep in loops the declaration sits, so a later
        // reassignment can pop back to its slot (see `Term::path_pop`).
        self.state_decl_depths
            .insert(state_key_const, self.loop_depth);
        // Declaration ids are unique by construction (the shadow ordinal in
        // `state_key_for` separates otherwise identical sites), so a duplicate
        // here means two sites hashed to one slot and one of them would be
        // silently unreachable — `find_state_init` resolves a rebind through
        // this map, so the loser's writes would land on the winner's slot.
        if self
            .state_inits
            .insert(state_key_const, state_tid)
            .is_some()
        {
            self.error_at(
                span,
                format!(
                    "internal error: `state {name}` hashes to a slot already \
                     claimed by another declaration; rename the variable"
                ),
            );
        }

        // Compile the init expression into a fresh child block. The
        // init block's last term register is read on pop and copied
        // to StateInit's register (return_term mechanism).
        let init_block = self.new_block(Some(state_tid));
        self.terms[state_tid.0 as usize].child_blocks = smallvec![init_block];
        self.compile_in_block(init_block, |c| {
            let init_tid = c.compile_expr(init);
            if is_var {
                // `state var` persists the *cell*: the init block runs once, so
                // the cell is allocated once and the slot holds it from then
                // on. Reads and writes go through it, which is what makes
                // persistence fall out with no `StateWrite` at all.
                c.emit_term(TermOp::CellNew, smallvec![init_tid], None);
            }
        });

        self.note_shadow(name);
        if is_var {
            self.scope_bind_var(name.to_string(), state_tid);
        } else {
            self.scope_bind(name.to_string(), state_tid);
        }
    }

    /// If `iter` is literally a call to `range(...)` with 1 or 2 arguments,
    /// compile its bound expressions and return `(start_tid, end_tid)` for a
    /// NumericForLoop. For `range(n)` the start is a synthesized `Constant(0)`.
    /// Returns `None` for any other iterable (the caller falls back to the
    /// generic ForLoop path). Only the for-loop-iterable position is special-
    /// cased — `range` used anywhere else still goes through the builtin.
    fn try_range_bounds(&mut self, iter: &Expr) -> Option<(TermId, TermId)> {
        let ExprKind::Call {
            function,
            args,
            arg_names,
        } = &iter.kind
        else {
            return None;
        };
        // `range(end: 10)` binds by name; the positional bounds below would
        // read it as `range(start)`, so leave it to the generic builtin path.
        if !arg_names.is_empty() {
            return None;
        }
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

    /// Compile `target = value` (`is_set` false) or `set target = value`
    /// (`is_set` true). The two forms compile identically; what differs is
    /// which binding kind each accepts, which is checked here.
    fn compile_assign(
        &mut self,
        target: &AssignTarget,
        value: &Expr,
        span: SourceSpan,
        is_set: bool,
    ) {
        if !self.check_write_keyword(target, span, is_set) {
            return;
        }
        if !self.check_assign_to_outer_function_binding(target, span) {
            return;
        }
        let prev_assign_span = self.assign_span.replace(span);
        match target {
            AssignTarget::Name(name) => self.compile_assign_name(name, value),
            AssignTarget::Field(object, field) => match Self::resolve_assign_target(object) {
                Some((root, mut steps)) => {
                    steps.push(AssignStep::Field(field));
                    self.compile_path_assign(root, steps, value);
                }
                None => self.emit_dead_store_error(),
            },
            AssignTarget::Index(object, index) => match Self::resolve_assign_target(object) {
                Some((root, mut steps)) => {
                    steps.push(AssignStep::Index(index));
                    self.compile_path_assign(root, steps, value);
                }
                None => self.emit_dead_store_error(),
            },
        }
        self.assign_span = prev_assign_span;
    }

    /// Enforce that `=` and `set` are disjoint: `=` writes `let`/`state`
    /// bindings, `set` writes `var` cells, and each rejects the other. Erroring
    /// in only one direction would leave `=` meaning two different things
    /// depending on a distant declaration, which is exactly the ambiguity `set`
    /// exists to remove. See docs/var.md (Two write keywords).
    ///
    /// Returns false when the statement is in error and should not be compiled.
    fn check_write_keyword(
        &mut self,
        target: &AssignTarget,
        span: SourceSpan,
        is_set: bool,
    ) -> bool {
        let Some(root) = Self::assign_root_name(target) else {
            return true;
        };
        // `set` never introduces a binding (section 6b), so an unknown target
        // is an error rather than a declaration. A plain `=` on an unknown name
        // keeps its existing behaviour.
        if self.scope_lookup(root).is_none() {
            if is_set {
                self.error_at(span, format!("`{root}` is not defined"));
                return false;
            }
            return true;
        }
        let root = root.to_string();
        // A `var` another module exported is readable here but not writable:
        // its cell belongs to the declaring module, and the alias form
        // (`set m.x = ...`) has no way to say this at all. Checked before the
        // keyword match so the message names the owner instead of repeating
        // "use `set`" at a `set` that is already correct.
        if is_set
            && let Some((owner, tid)) = self.imported_vars.get(&root)
            && self.scope_lookup(&root) == Some(*tid)
        {
            let owner = owner.clone();
            self.error_at(
                span,
                format!(
                    "`{root}` is a `var` exported by module `{owner}`; only `{owner}` \
                     can write it — call a function it exports instead"
                ),
            );
            return false;
        }
        // A hoisted `fn` binds a cell that its declaration writes exactly once.
        // Rebinding that name would leave every already-compiled reference —
        // and every closure that captured the cell — pointing at a box nothing
        // updates, so reject the write instead of silently splitting the name
        // in two.
        if self.binding_is_fn_cell(&root) {
            self.error_at(
                span,
                format!(
                    "`{root}` is a function declared in this file and is used before its \
                     declaration, so its name cannot be reassigned. Bind a `var` to it \
                     instead (`var current = {root}`) and write that."
                ),
            );
            return false;
        }
        match (self.binding_is_var(&root), is_set) {
            (true, false) => {
                self.error_at(
                    span,
                    format!("`{root}` is a `var`; use `set {root} = ...` to write it"),
                );
                false
            }
            (false, true) => {
                self.error_at(
                    span,
                    format!("`{root}` is not a `var`; use `{root} = ...`, or declare it `var {root} = ...`"),
                );
                false
            }
            _ => true,
        }
    }

    /// The root variable name an assignment target writes, if it has one.
    fn assign_root_name(target: &AssignTarget) -> Option<&str> {
        match target {
            AssignTarget::Name(name) => Some(name.as_str()),
            AssignTarget::Field(object, _) | AssignTarget::Index(object, _) => {
                Self::resolve_assign_target(object).map(|(root, _)| root)
            }
        }
    }

    /// Reject an assignment targeting a name bound outside the function being
    /// compiled. Such an assignment does not modify that binding — it creates a
    /// function-local shadow — so the code reads as a dataflow edge that isn't
    /// there, and one control-flow step further it did not even lower (the phi
    /// would have had to initialize from a term in another function). The split
    /// was an implementation detail showing through as a language rule; now
    /// both halves fail, at the assignment site, and `var`/`set` is the escape
    /// hatch for code that genuinely wanted mutation.
    /// See docs/var.md (Cross-function assignment).
    ///
    /// Returns false when the statement is in error and should not be compiled
    /// — abandoning it is what keeps a rejected assignment from emitting the
    /// phi that would fail to lower, so the program stops at compile.
    ///
    /// Must run *before* the value expression is compiled: compiling it may
    /// create the capture phantom for the same name.
    fn check_assign_to_outer_function_binding(
        &mut self,
        target: &AssignTarget,
        span: SourceSpan,
    ) -> bool {
        // A `var` is exempt: writing an outer binding from inside a function is
        // exactly what the escape hatch is for, and a `set` really does modify
        // it. The error is about `=`'s silent local shadow.
        let Some(root) = Self::assign_root_name(target)
            .filter(|n| self.is_outer_function_binding(n) && !self.binding_is_var(n))
        else {
            return true;
        };
        let root = root.to_string();
        self.error_at(
            span,
            format!(
                "`{root}` is bound outside this function; this assignment creates a \
                 local shadow and does not modify `{root}`. Use `let` for a new local, \
                 return the value, or — if it really must be mutable — declare it \
                 `var {root} = ...` and write it with `set {root} = ...`"
            ),
        );
        false
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
                AssignStep::Field(name) => CompiledStep::Field(
                    self.constants
                        .intern(ConstantValue::String((*name).to_string())),
                ),
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
        // A `var` is written through its cell, not rebound. Nothing below
        // applies: no `StateWrite` (a `state var`'s slot already holds the
        // cell, so the write lands in the persisted box), no carry slot and no
        // `block_rebinds` entry (there is no phi to feed), and no
        // `scope_rebind` (the binding still names the same cell).
        if self.binding_is_var(name) {
            let cell = self
                .resolve_local_term(name)
                .expect("a `var` binding resolves; check_write_keyword ran first");
            let write_tid = self.emit_term(
                TermOp::CellWrite,
                smallvec![cell, val_tid],
                Some(name.to_string()),
            );
            // Only expressions record spans, so without this the write a
            // provenance boundary points at would have no location.
            if let Some(span) = self.assign_span {
                self.source_map.add(write_tid, span);
            }
            return;
        }

        // Check if this is a state variable — if so, emit StateWrite.
        // Walk through Phi/Copy nodes so an assignment inside an
        // `if` / loop body, or a chain of repeat reassignments at
        // the top level, still finds the underlying StateInit.
        let mut state_init_for_copy: Option<StateKey> = None;
        if let Some(existing_tid) = self.scope_lookup(name)
            && let Some(init_tid) = self.find_state_init(existing_tid)
        {
            let state_key = self.terms[init_tid.0 as usize].state_key;
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
            // A write nested deeper in loops than the declaration still belongs
            // to the declaration's slot: record how many loop levels to pop.
            let decl_depth = state_key
                .and_then(|k| self.state_decl_depths.get(&k).copied())
                .unwrap_or(self.loop_depth);
            self.terms[write_tid.0 as usize].path_pop = self.loop_depth.saturating_sub(decl_depth);
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
            self.propagate_cross_fn(existing_tid, assign_tid);
            let existing_block = self.terms[existing_tid.0 as usize].block_id;
            // A name that already has a rebind logged in this block crossed a
            // block boundary on its first reassignment here. Subsequent
            // in-block reassignments must keep `block_rebinds` pointing at the
            // *latest* binding, otherwise the enclosing conditional's phi-out
            // wires from the first rebind and later writes are dropped (e.g.
            // two `append`s to a loop-carried var inside an `if`).
            // …unless the block declared its own `let`/`state` for the name in
            // the meantime, in which case this write targets that local and
            // must not touch the block's carry-out (see `note_shadow`).
            let already_rebound_here = self
                .block_rebinds
                .get(&self.current_block)
                .is_some_and(|m| m.contains_key(name))
                && !self.is_shadowed_in_current_block(name);
            if existing_block == self.current_block && !already_rebound_here {
                self.scope_rebind(name.to_string(), assign_tid);
            } else {
                self.rebind_name_in_current_block(name.to_string(), assign_tid);
            }
        } else {
            self.scope_rebind(name.to_string(), assign_tid);
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
