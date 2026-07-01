//! Expression compilation, including `if`/`match` phi joins and pattern
//! resolution.

use super::*;

impl Compiler {
    pub(super) fn compile_expr(&mut self, expr: &Expr) -> TermId {
        let span = expr.span;
        let tid = self.compile_expr_kind(&expr.kind, span);
        // Record source span for the primary term emitted by this expression
        self.source_map.add(tid, span);
        tid
    }

    fn compile_expr_kind(&mut self, expr: &ExprKind, span: SourceSpan) -> TermId {
        match expr {
            ExprKind::Literal(lit) => {
                let cv = match lit {
                    Literal::Nil => ConstantValue::Nil,
                    Literal::Bool(b) => ConstantValue::Bool(*b),
                    Literal::Int(n) => ConstantValue::Int(*n),
                    Literal::Float(f) => ConstantValue::from_f64(*f),
                    Literal::String(s) => ConstantValue::String(s.clone()),
                };
                let cid = self.constants.intern(cv);
                self.emit_term(TermOp::Constant(cid), smallvec![], None)
            }

            ExprKind::Ident(name) => self.compile_ident(name),

            // `@x` that the desugar pass could not lift (it wasn't a call
            // argument at statement level). Compile to a deferred error so it
            // only fires if actually executed, matching undefined-variable
            // handling.
            ExprKind::AtVar(name) => {
                let msg = format!(
                    "`@{}` can only be used as an argument to a call at statement level",
                    name
                );
                let msg_cid = self.constants.intern(ConstantValue::String(msg));
                self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
            }

            ExprKind::BinaryOp { op, left, right } => {
                // Short-circuit ops
                if *op == BinOp::And {
                    return self.compile_short_circuit(left, right, true);
                }
                if *op == BinOp::Or {
                    return self.compile_short_circuit(left, right, false);
                }

                let l = self.compile_expr(left);
                let r = self.compile_expr(right);
                let term_op = match op {
                    BinOp::Add => TermOp::Add,
                    BinOp::Sub => TermOp::Sub,
                    BinOp::Mul => TermOp::Mul,
                    BinOp::Div => TermOp::Div,
                    BinOp::Mod => TermOp::Mod,
                    BinOp::Eq => TermOp::Eq,
                    BinOp::Ne => TermOp::Ne,
                    BinOp::Lt => TermOp::Lt,
                    BinOp::Le => TermOp::Le,
                    BinOp::Gt => TermOp::Gt,
                    BinOp::Ge => TermOp::Ge,
                    BinOp::Concat => TermOp::Concat,
                    BinOp::And | BinOp::Or => unreachable!(),
                };
                self.emit_term(term_op, smallvec![l, r], None)
            }

            ExprKind::UnaryOp { op, operand } => {
                let val = self.compile_expr(operand);
                let term_op = match op {
                    UnaryOp::Neg => TermOp::Neg,
                    UnaryOp::Not => TermOp::Not,
                };
                self.emit_term(term_op, smallvec![val], None)
            }

            ExprKind::Call { function, args } => {
                // Detect method syntax: obj.method(args...)
                if let ExprKind::FieldAccess { object, field } = &function.kind {
                    // `ui.button(...)` where `ui` is an unshadowed module
                    // alias is not a method call: the callee resolves
                    // statically in the module's exports.
                    if let Some(func_tid) = self.try_module_member(object, field) {
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![func_tid];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        return self.emit_term(TermOp::Call, inputs, None);
                    }
                    let obj_tid = self.compile_expr(object);
                    let mut inputs: SmallVec<[TermId; 4]> = smallvec![obj_tid];
                    for arg in args {
                        inputs.push(self.compile_expr(arg));
                    }
                    let field_const = self.constants.intern(ConstantValue::String(field.clone()));
                    self.emit_term(TermOp::MethodCall(field_const), inputs, None)
                } else {
                    // A bare identifier that currently resolves in scope to
                    // exactly the builtin's phantom term (i.e. not shadowed by a
                    // later user binding) compiles to a static BuiltinCall.
                    let builtin_name: Option<String> = match &function.kind {
                        ExprKind::Ident(name)
                            if self.builtin_phantoms.get(name) == self.scope_lookup(name).as_ref() =>
                        {
                            Some(name.clone())
                        }
                        _ => None,
                    };

                    if let Some(name) = builtin_name {
                        // Direct builtin call: inputs are just the args (no callable).
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        let name_cid = self.constants.intern(ConstantValue::String(name));
                        self.emit_term(TermOp::BuiltinCall(name_cid), inputs, None)
                    } else {
                        let func_tid = self.compile_expr(function);
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![func_tid];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        self.emit_term(TermOp::Call, inputs, None)
                    }
                }
            }

            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => self.compile_if(condition, then_body, else_body.as_ref(), span),

            ExprKind::Match { subject, arms } => self.compile_match(subject, arms, span),

            ExprKind::List(elements) => {
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                for elem in elements {
                    inputs.push(self.compile_expr(elem));
                }
                self.emit_term(TermOp::AllocList, inputs, None)
            }

            ExprKind::Record(fields) => self.compile_record(fields),

            ExprKind::FieldAccess { object, field } => {
                // `ui.palette` where `ui` is an unshadowed module alias
                // resolves statically in the module's exports.
                if let Some(tid) = self.try_module_member(object, field) {
                    return tid;
                }
                let obj_tid = self.compile_expr(object);
                let field_const = self.constants.intern(ConstantValue::String(field.clone()));
                self.emit_term(TermOp::GetField(field_const), smallvec![obj_tid], None)
            }

            ExprKind::IndexAccess { object, index } => {
                let obj_tid = self.compile_expr(object);
                let idx_tid = self.compile_expr(index);
                self.emit_term(TermOp::GetIndex, smallvec![obj_tid, idx_tid], None)
            }

            ExprKind::Block(stmts) => {
                // Compile in a new scope but same block (inline block)
                self.push_scope(false);
                let nil_cid = self.constants.intern(ConstantValue::Nil);
                let mut last_tid = self.emit_term(TermOp::Constant(nil_cid), smallvec![], None);
                for s in stmts {
                    match &s.kind {
                        StmtKind::Expr(e) => {
                            last_tid = self.compile_expr(e);
                        }
                        _ => {
                            self.compile_stmt(s);
                        }
                    }
                }
                self.pop_scope();
                last_tid
            }

            ExprKind::Lambda { params, body } => self.compile_function(None, params, body),

            ExprKind::Element { tag, props, children } => {
                self.compile_element(tag, props, children)
            }

            ExprKind::StringInterp { parts, exprs } => self.compile_string_interp(parts, exprs),
        }
    }

    /// A variable reference: resolve through scope, inserting a capture
    /// phantom when the binding crosses a function boundary. Unresolved
    /// names compile to an `Error` term (with a hint for common slips from
    /// other languages) that only fires if actually executed.
    pub(super) fn compile_ident(&mut self, name: &str) -> TermId {
        if let Some(tid) = self.scope_lookup(name) {
            // Check if this reference crosses a function boundary (needs capture)
            if self.needs_capture(name) {
                let local_tid = self.get_or_add_capture(name, tid);
                self.emit_term(TermOp::Copy, smallvec![local_tid], None)
            } else {
                self.emit_term(TermOp::Copy, smallvec![tid], None)
            }
        } else if let Some(module) = self.module_aliases.get(name) {
            // A module alias is not a runtime value.
            let msg = format!(
                "'{}' is a module, not a value — use {}.<name>, or `import {}: <name>` \
                 to bind a member directly",
                name, name, module
            );
            let msg_cid = self.constants.intern(ConstantValue::String(msg));
            self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
        } else {
            let hint = match name {
                "var" | "const" => Some("use 'let' to declare variables in Petal"),
                "def" | "func" | "function" => Some("use 'fn' to define functions in Petal"),
                "elif" | "elseif" | "elsif" => Some("use 'else if' in Petal"),
                "switch" | "case" => Some("use 'match' for pattern matching in Petal"),
                "lambda" => Some("use 'fn' for anonymous functions, e.g. fn(x) { x + 1 }"),
                "null" | "undefined" | "None" => {
                    Some("use 'nil' for null/empty values in Petal")
                }
                "console" => Some("use 'print()' for output in Petal"),
                "typeof" => Some("use 'type()' to get the type of a value in Petal"),
                "Math" => Some(
                    "math functions are top-level in Petal: abs(), sqrt(), floor(), ceil(), round()",
                ),
                "require" => Some("use 'import <module>' at the top of the file"),
                _ => None,
            };
            let msg = match hint {
                Some(hint) => format!("Undefined variable: {} — {}", name, hint),
                None => format!("Undefined variable: {}", name),
            };
            let msg_cid = self.constants.intern(ConstantValue::String(msg));
            self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
        }
    }

    /// Resolve `alias.member` when `alias` is a module alias that no term
    /// binding shadows. Returns `None` when this isn't module member access
    /// (the caller falls back to field-access / method-call compilation).
    /// A hit compiles to an ordinary reference to the exported term — the
    /// scope-lookup/capture machinery applies, since exports are bound in the
    /// global scope under their qualified name — or to a deferred error term
    /// for an unknown or private member.
    fn try_module_member(&mut self, object: &Expr, member: &str) -> Option<TermId> {
        let ExprKind::Ident(name) = &object.kind else {
            return None;
        };
        // A term binding (local, param, capture, builtin) shadows the alias:
        // `ui` is then an ordinary value and `.member` is field access.
        if self.scope_lookup(name).is_some() {
            return None;
        }
        let module = self.module_aliases.get(name)?.clone();

        let qualified = format!("{module}::{member}");
        if !member.starts_with('_') && self.scope_lookup(&qualified).is_some() {
            return Some(self.compile_ident(&qualified));
        }
        let msg = if member.starts_with('_') {
            format!(
                "'{}' in module '{}' is private (names starting with '_' are \
                 module-private)",
                member, module
            )
        } else {
            format!("module '{}' has no export '{}'", module, member)
        };
        let msg_cid = self.constants.intern(ConstantValue::String(msg));
        Some(self.emit_term(TermOp::Error(msg_cid), smallvec![], None))
    }

    fn compile_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: Option<&ElseBranch>,
        span: SourceSpan,
    ) -> TermId {
        let cond_tid = self.compile_expr(condition);

        // Pre-scan both branches to decide which names need a phi.
        // Emit phi terms in the parent block *before* the Branch so
        // each phi's exec initializes its register to the pre-if
        // value; popping branches then overwrite via phi_outs.
        let mut bodies: Vec<&[Stmt]> = vec![then_body];
        if let Some(ElseBranch::Block(stmts)) = else_body {
            bodies.push(stmts.as_slice());
        }
        let mut names = self.detect_rebinds_stmts(&bodies);
        if let Some(ElseBranch::ElseIf(e)) = else_body {
            for n in self.detect_rebinds_exprs(&[e.as_ref()]) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        let phis = self.emit_phis(&names, span);

        let then_block = self.new_block(None);
        let else_block = self.new_block(None);

        let branch_tid = self.emit_term_with_children(
            TermOp::Branch,
            smallvec![cond_tid],
            None,
            smallvec![then_block, else_block],
        );
        self.blocks[then_block.0 as usize].parent_term_id = Some(branch_tid);
        self.blocks[else_block.0 as usize].parent_term_id = Some(branch_tid);

        // Compile then body. Each arm is seeded with carry-slot entry copies
        // for the phi'd names so a mid-arm exit (break/continue) still
        // carries the names' latest values out (see `seed_arm_entry_copies`).
        self.compile_in_block(then_block, |c| {
            c.seed_arm_entry_copies(then_block, &phis);
            for s in then_body {
                c.compile_stmt(s);
            }
            c.carry_slots.pop();
        });

        // Compile else body
        self.compile_in_block(else_block, |c| {
            c.seed_arm_entry_copies(else_block, &phis);
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    for s in stmts {
                        c.compile_stmt(s);
                    }
                }
                Some(ElseBranch::ElseIf(expr)) => {
                    c.compile_expr(expr);
                }
                None => {
                    // No else — emit Nil
                    let nil_cid = c.constants.intern(ConstantValue::Nil);
                    c.emit_term(TermOp::Constant(nil_cid), smallvec![], None);
                }
            }
            c.carry_slots.pop();
        });

        // Wire phi_outs from each branch's rebinds.
        self.wire_phi_outs(then_block, &phis);
        self.wire_phi_outs(else_block, &phis);

        branch_tid
    }

    fn compile_match(&mut self, subject: &Expr, arms: &[MatchArm], span: SourceSpan) -> TermId {
        let subj_tid = self.compile_expr(subject);

        // Pre-scan all arm bodies for names that will be rebound,
        // emit phis in the parent block before the Match term.
        let arm_body_refs: Vec<&Expr> = arms.iter().map(|a| &a.body).collect();
        let names = self.detect_rebinds_exprs(&arm_body_refs);
        let phis = self.emit_phis(&names, span);

        let mut child_blocks: SmallVec<[BlockId; 2]> = SmallVec::new();
        let mut arm_metas = Vec::new();

        for arm in arms {
            // Body block
            let body_block = self.new_block(None);
            child_blocks.push(body_block);

            // Resolve pattern: convert known enum variant names to Variant patterns
            let pattern = self.resolve_pattern(&arm.pattern);

            // Extract pattern variables (after resolution, so enum names aren't bindings)
            let pattern_vars = Self::extract_pattern_vars(&pattern);

            // Compile guard if present (with pattern vars in scope)
            let guard_block = arm.guard.as_ref().map(|guard_expr| {
                let gb = self.new_block(None);
                self.compile_in_block(gb, |c| {
                    for var_name in &pattern_vars {
                        let phantom = c.emit_phantom_term(var_name.clone());
                        c.scope_bind(var_name.clone(), phantom);
                    }
                    c.compile_expr(guard_expr);
                });
                gb
            });

            // Compile body with pattern variable bindings. Seeding runs after
            // the pattern bindings so pattern-shadowed names are skipped
            // (assignments to them are arm-local and must not carry out).
            self.compile_in_block(body_block, |c| {
                for var_name in &pattern_vars {
                    let phantom = c.emit_phantom_term(var_name.clone());
                    c.scope_bind(var_name.clone(), phantom);
                }
                c.seed_arm_entry_copies(body_block, &phis);
                c.compile_expr(&arm.body);
                c.carry_slots.pop();
            });

            arm_metas.push(MatchArmMeta {
                pattern,
                guard_block,
                body_block,
            });
        }

        let match_tid =
            self.emit_term_with_children(TermOp::Match, smallvec![subj_tid], None, child_blocks);

        // Set parent_term_id on all child blocks
        for meta in &arm_metas {
            self.blocks[meta.body_block.0 as usize].parent_term_id = Some(match_tid);
            if let Some(gb) = meta.guard_block {
                self.blocks[gb.0 as usize].parent_term_id = Some(match_tid);
            }
        }

        // Wire phi_outs for each arm body's rebinds.
        let arm_bodies: Vec<BlockId> = arm_metas.iter().map(|m| m.body_block).collect();
        self.match_arms.insert(match_tid, arm_metas);
        for body_block in &arm_bodies {
            self.wire_phi_outs(*body_block, &phis);
        }
        match_tid
    }

    fn compile_record(&mut self, fields: &[RecordField]) -> TermId {
        let has_spread = fields.iter().any(|f| matches!(f, RecordField::Spread(_)));
        if !has_spread {
            // Simple case: no spread, use AllocMap
            let mut field_names = Vec::new();
            let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
            for field in fields {
                if let RecordField::Named(key, value) = field {
                    field_names.push(self.constants.intern(ConstantValue::String(key.clone())));
                    inputs.push(self.compile_expr(value));
                }
            }
            self.emit_term(TermOp::AllocMap { fields: field_names }, inputs, None)
        } else {
            // Spread case: compile all inputs and build entry list
            let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
            let mut entries = Vec::new();
            for field in fields {
                match field {
                    RecordField::Spread(expr) => {
                        let idx = inputs.len();
                        inputs.push(self.compile_expr(expr));
                        entries.push(MapSpreadEntry::Spread(idx));
                    }
                    RecordField::Named(key, value) => {
                        let cid = self.constants.intern(ConstantValue::String(key.clone()));
                        let idx = inputs.len();
                        inputs.push(self.compile_expr(value));
                        entries.push(MapSpreadEntry::Named(cid, idx));
                    }
                }
            }
            self.emit_term(TermOp::AllocMapSpread { entries }, inputs, None)
        }
    }

    fn compile_element(
        &mut self,
        tag: &str,
        props: &[(String, Expr)],
        children: &[JsxChild],
    ) -> TermId {
        let tag_cid = self.constants.intern(ConstantValue::String(tag.to_string()));
        let mut prop_keys = Vec::new();
        let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();

        // Compile prop values
        for (key, value) in props {
            prop_keys.push(self.constants.intern(ConstantValue::String(key.clone())));
            inputs.push(self.compile_expr(value));
        }

        // Compile children
        for child in children {
            match child {
                JsxChild::Text(text) => {
                    let cid = self.constants.intern(ConstantValue::String(text.clone()));
                    inputs.push(self.emit_term(TermOp::Constant(cid), smallvec![], None));
                }
                JsxChild::Expr(expr) => {
                    inputs.push(self.compile_expr(expr));
                }
            }
        }

        self.emit_term(TermOp::AllocElement { tag: tag_cid, prop_keys }, inputs, None)
    }

    /// Build: str(parts[0]) ++ str(exprs[0]) ++ str(parts[1]) ++ ... ++ str(parts[N]).
    /// Concat already handles string conversion in the evaluator.
    fn compile_string_interp(&mut self, parts: &[String], exprs: &[Expr]) -> TermId {
        // Start with the first string part
        let first_cid = self.constants.intern(ConstantValue::String(parts[0].clone()));
        let mut result = self.emit_term(TermOp::Constant(first_cid), smallvec![], None);

        for (i, expr) in exprs.iter().enumerate() {
            let expr_tid = self.compile_expr(expr);
            result = self.emit_term(TermOp::Concat, smallvec![result, expr_tid], None);

            // Add the next string part
            let part_cid = self
                .constants
                .intern(ConstantValue::String(parts[i + 1].clone()));
            let part_tid = self.emit_term(TermOp::Constant(part_cid), smallvec![], None);
            result = self.emit_term(TermOp::Concat, smallvec![result, part_tid], None);
        }

        result
    }

    /// Compile `&&` / `||`: the RHS lives in a child block that only runs
    /// when the LHS doesn't decide the result.
    fn compile_short_circuit(&mut self, left: &Expr, right: &Expr, is_and: bool) -> TermId {
        let left_tid = self.compile_expr(left);
        let rhs_block = self.new_block(None);

        // Compile RHS in its own block
        self.compile_in_block(rhs_block, |c| {
            c.compile_expr(right);
        });

        let op = if is_and { TermOp::And } else { TermOp::Or };
        let tid = self.emit_term_with_children(op, smallvec![left_tid], None, smallvec![rhs_block]);
        self.blocks[rhs_block.0 as usize].parent_term_id = Some(tid);
        tid
    }

    // -----------------------------------------------------------------------
    // Pattern resolution
    // -----------------------------------------------------------------------

    /// Convert Pattern::Variable to Pattern::Variant for known enum variant names.
    /// This ensures pattern matching only matches the actual variant, not any value.
    fn resolve_pattern(&self, pattern: &Pattern) -> Pattern {
        match pattern {
            Pattern::Variable(name) => {
                if self.enum_variants.get(name) == Some(&0) {
                    return Pattern::Variant {
                        name: name.clone(),
                        fields: vec![],
                    };
                }
                pattern.clone()
            }
            Pattern::Variant { name, fields } => Pattern::Variant {
                name: name.clone(),
                fields: fields.iter().map(|f| self.resolve_pattern(f)).collect(),
            },
            Pattern::List { elements, rest } => Pattern::List {
                elements: elements.iter().map(|e| self.resolve_pattern(e)).collect(),
                rest: rest.clone(),
            },
            Pattern::Record(fields) => Pattern::Record(
                fields
                    .iter()
                    .map(|(k, p)| (k.clone(), self.resolve_pattern(p)))
                    .collect(),
            ),
            _ => pattern.clone(),
        }
    }

    fn extract_pattern_vars(pattern: &Pattern) -> Vec<String> {
        match pattern {
            Pattern::Wildcard | Pattern::Literal(_) => vec![],
            Pattern::Variable(name) => vec![name.clone()],
            Pattern::Variant { fields, .. } => {
                fields.iter().flat_map(Self::extract_pattern_vars).collect()
            }
            Pattern::List { elements, rest } => {
                let mut vars: Vec<String> =
                    elements.iter().flat_map(Self::extract_pattern_vars).collect();
                if let Some(rest_name) = rest {
                    vars.push(rest_name.clone());
                }
                vars
            }
            Pattern::Record(fields) => fields
                .iter()
                .flat_map(|(_, p)| Self::extract_pattern_vars(p))
                .collect(),
        }
    }
}
