//! Expression compilation, including `if`/`match` phi joins and pattern
//! resolution.

use super::*;

impl Compiler {
    pub(super) fn compile_expr(&mut self, expr: &Expr) -> TermId {
        let span = expr.span;
        // A negated number in a `config let`'s data: one constant holding the
        // signed value (see `Compiler::config_folds`).
        if !self.config_folds.is_empty()
            && self.config_folds.contains(&span)
            && let Some(lit) = crate::source_diff::signed_number(expr)
        {
            self.value_used = true;
            let cv = match lit {
                Literal::Int(n) => ConstantValue::Int(n),
                Literal::Float(f) => ConstantValue::from_f64(f),
                _ => unreachable!("signed_number yields a number"),
            };
            let cid = self.constants.intern(cv);
            let tid = self.emit_term(TermOp::Constant(cid), smallvec![], None);
            self.source_map.add(tid, span);
            return tid;
        }
        let tid = self.compile_expr_kind(&expr.kind, span);
        // Record source span for the primary term emitted by this expression
        self.source_map.add(tid, span);
        tid
    }

    fn compile_expr_kind(&mut self, expr: &ExprKind, span: SourceSpan) -> TermId {
        // Taken here, so every sub-expression compiled below is back in value
        // position; only a discarded statement-level expression sees false.
        let value_used = std::mem::replace(&mut self.value_used, true);
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

            ExprKind::Ident(name) => {
                // A bare read of a cell that lives outside this function is
                // the one position where "snapshot" and "live" disagree, so it
                // has to say which it means. Checked here rather than in
                // `compile_ident` because the internal root-read for a nested
                // write target (`set r.f = v`) goes through that helper too and
                // is not a user-written read.
                self.check_cell_read_says_get(name, span);
                self.check_capture_is_not_rebound(name, span);
                self.check_import_use(name, span);
                self.compile_ident(name)
            }

            // `get x` — the explicit read of a cell's current contents.
            ExprKind::CellGet(name) => self.compile_cell_get(name, span),

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
                if *op == BinOp::Coalesce {
                    return self.compile_coalesce(left, right);
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
                    BinOp::And | BinOp::Or | BinOp::Coalesce => unreachable!(),
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

            ExprKind::Call {
                function,
                args,
                arg_names,
            } => {
                // Each written name, interned, parallel to the op's argument
                // slice (see `Term::arg_names`). Empty stays empty: the
                // all-positional path allocates nothing and behaves exactly as
                // it did before named arguments existed.
                let names = self.intern_arg_names(arg_names);
                // A spread call (`f(...args)`) takes the same paths as any
                // other: its last argument carries the spread marker as its
                // name (`ast::SPREAD_ARG`), and what that binds — positions
                // for a list, parameter names for a record — is the VM's to
                // decide when the value is in hand (`Vm::do_spread_call`).
                // Detect method syntax: obj.method(args...)
                if let ExprKind::FieldAccess { object, field } = &function.kind {
                    // `Color.custom(1, 2, 3)` where `Color` is an enum:
                    // construct the variant in place.
                    if let Some(enum_name) = self.static_enum(object) {
                        return self.compile_enum_call(&enum_name, field, args, names, span);
                    }
                    // `ui.button(...)` where `ui` is an unshadowed module
                    // alias is not a method call: the callee resolves
                    // statically in the module's exports.
                    if let Some(func_tid) = self.try_module_member(object, field) {
                        // A member the module does not export is a deferred
                        // error term. Give it the callee's position: the term
                        // this arm returns is the call, so nothing else would,
                        // and an error with no position is one neither `petal
                        // check` nor the run's own report can point at.
                        if matches!(self.terms[func_tid.0 as usize].op, TermOp::Error(_)) {
                            self.source_map.add(func_tid, function.span);
                        }
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![func_tid];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        let callee = super::callee_text(function);
                        return self.emit_call_term(TermOp::Call, inputs, &callee, names);
                    }
                    // A site the checker pinned to one class calls that class's
                    // method directly, as an ordinary `Call` whose callee is the
                    // `fn Class.method` binding — the same term a plain function
                    // call would name. What this buys:
                    //
                    //  - **Live editing.** Runtime dispatch reads the class tag
                    //    the *receiver* carries, and a value in `state` outlives
                    //    the edit that reshaped its class. Binding the call to
                    //    the declaration means the code now running decides what
                    //    runs, which is the whole point of a live edit; the tag
                    //    stays a label, never a link back to the old program.
                    //  - **Dataflow.** A `Call` names its callee in its inputs,
                    //    so a slice gets the exact function instead of the
                    //    over-approximating may-edge `dispatch_targets` recovers
                    //    for a dispatched call.
                    //
                    // Only a declaration already *compiled* is a candidate.
                    // Nothing in Petal hoists: the name a `fn` binds holds nil
                    // until its declaration runs, so binding a call to a
                    // declaration written below it would turn a program that
                    // works today (dispatch looks the method up by name at call
                    // time, by which point it is registered) into `Cannot call
                    // nil` — or, from inside a function body, into a term the
                    // callee's block cannot even reference. Resolving only
                    // backwards keeps this change a pure optimization of what
                    // already runs.
                    //
                    // A *built-in* class method has no declaration statement to
                    // wait for — it is a native, registered under the same
                    // dotted name before the program starts — so it is always a
                    // candidate.
                    let static_callee = self
                        .method_dispatch
                        .pinned
                        .get(&span)
                        .cloned()
                        .filter(|class| {
                            let qname = crate::classes::qualified_method_name(class, field);
                            self.declared_methods.contains(&qname)
                                || self.is_builtin_method(class, field)
                        })
                        .map(|class| crate::classes::qualified_method_name(&class, field))
                        // Resolve the name the way any other reference to it
                        // would be, so a call inside a function body captures
                        // the declaration rather than pointing at a term in
                        // someone else's block.
                        .and_then(|qname| self.resolve_local_term(&qname));
                    let obj_tid = self.compile_expr(object);
                    let callee_text = super::callee_text(function);
                    if let Some(callee) = static_callee {
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![callee, obj_tid];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        // `inputs` is [callee, receiver, args…], so the
                        // argument slice starts at the receiver — which is
                        // never named, and takes the leading `None`.
                        return self.emit_call_term(
                            TermOp::Call,
                            inputs,
                            &callee_text,
                            shift_arg_names(names),
                        );
                    }
                    // `x.name(…)` falls back to the native `name(x, …)` when
                    // nothing else answers — but a host's native is reachable
                    // only through a declaration in scope, and this is where
                    // the scope is known. A `core` builtin needs no entry.
                    if let Some(tid) = self.scope_lookup(field)
                        && self.native_terms.get(&tid).is_some_and(|host| host == field)
                    {
                        self.note_native_use(tid);
                        let files = self.method_natives.entry(field.clone()).or_default();
                        if !files.contains(&span.file) {
                            files.push(span.file);
                        }
                    }
                    // A plain value (a number, a list, a vector) has nothing
                    // else to answer with, so a packaged builtin the file has
                    // not imported is a line that fails whenever it runs.
                    else if self.scope_lookup(field).is_none()
                        && let Some(what) = self.method_dispatch.plain.get(&span)
                        && let Some(package) = crate::native_decl::builtin_package_of(field)
                    {
                        self.warnings.push(crate::diagnostic::Diagnostic::error(
                            span,
                            format!(
                                "no method `{field}` on a `{what}` here — `{field}` is a builtin \
                                 of the `{package}` package: add `import {package}` to call it \
                                 as a method"
                            ),
                        ));
                    }
                    let mut inputs: SmallVec<[TermId; 4]> = smallvec![obj_tid];
                    for arg in args {
                        inputs.push(self.compile_expr(arg));
                    }
                    let field_const = self.constants.intern(ConstantValue::String(field.clone()));
                    // The call still dispatches on the receiver, but carries the
                    // class its declaration named, for the case where the label
                    // the receiver arrives with names nothing in this program.
                    let hint = self
                        .method_dispatch
                        .hints
                        .get(&span)
                        .cloned()
                        .map(|class| self.constants.intern(ConstantValue::String(class)));
                    self.emit_call_term(
                        TermOp::MethodCall {
                            name: field_const,
                            hint,
                        },
                        inputs,
                        &callee_text,
                        names,
                    )
                } else {
                    // A bare identifier that currently resolves in scope to
                    // exactly the builtin's phantom term (i.e. not shadowed by a
                    // later user binding) compiles to a static BuiltinCall.
                    //
                    // "The builtin" is whatever host native the name is bound
                    // to: a core builtin's own name, or the name a
                    // `native fn` declaration gave one (`_native_rect` for
                    // `draw_rect`) — the call names the *host* native either
                    // way.
                    //
                    // A name nothing binds is not looked up among the host's
                    // natives: a native is reachable only through a
                    // declaration in scope. The call compiles to a deferred
                    // error that fires if the line runs ("Unknown function"),
                    // which `petal check` reports first
                    // (`typecheck::globals`).
                    if let ExprKind::Ident(name) = &function.kind {
                        self.check_import_call(
                            name,
                            args.len(),
                            arg_names.iter().all(Option::is_none),
                            span,
                        );
                    }
                    let builtin_name: Option<String> = match &function.kind {
                        ExprKind::Ident(name) => match self.scope_lookup(name) {
                            Some(tid) => {
                                self.note_native_use(tid);
                                self.native_terms.get(&tid).cloned()
                            }
                            None if self.module_aliases.contains_key(name) => None,
                            None => {
                                for arg in args {
                                    self.compile_expr(arg);
                                }
                                let mut msg = format!(
                                    "{}{name}",
                                    crate::typecheck::globals::UNKNOWN_FUNCTION_PREFIX
                                );
                                if let Some(hint) = self
                                    .bare_variant_hint(name, Some(!args.is_empty()))
                                    .or_else(|| {
                                    crate::native_decl::missing_builtin_import_hint(name)
                                }) {
                                    msg.push_str(" — ");
                                    msg.push_str(&hint);
                                }
                                let msg_cid = self.constants.intern(ConstantValue::String(msg));
                                return self.emit_term(TermOp::Error(msg_cid), smallvec![], None);
                            }
                        },
                        _ => None,
                    };

                    if let Some(name) = builtin_name {
                        // Direct builtin call: inputs are just the args (no callable).
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        let name_cid = self.constants.intern(ConstantValue::String(name.clone()));
                        self.emit_call_term(TermOp::BuiltinCall(name_cid), inputs, &name, names)
                    } else {
                        let func_tid = self.compile_expr(function);
                        let mut inputs: SmallVec<[TermId; 4]> = smallvec![func_tid];
                        for arg in args {
                            inputs.push(self.compile_expr(arg));
                        }
                        let callee = super::callee_text(function);
                        self.emit_call_term(TermOp::Call, inputs, &callee, names)
                    }
                }
            }

            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => self.compile_if(condition, then_body, else_body.as_ref(), value_used, span),

            ExprKind::Match { subject, arms } => {
                self.compile_match(subject, arms, value_used, span)
            }

            // Value-position `for`: collect each iteration's last expression
            // into a list (see `compile_for`). `while` has no expression form.
            // An expression-form `for` whose value is discarded — the arm of a
            // statement-level `match`, the last line of a discarded block —
            // stays a side-effect loop.
            ExprKind::For { var, iter, body } => {
                self.compile_for(var, iter, body, value_used, span)
            }

            ExprKind::List(elements) => {
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                for elem in elements {
                    inputs.push(self.compile_expr(elem));
                }
                self.emit_term(TermOp::AllocList, inputs, None)
            }

            ExprKind::Record(fields) => self.compile_record(fields),

            ExprKind::FieldAccess { object, field } => {
                // `Color.red` where `Color` is an enum is the variant itself.
                if let Some(enum_name) = self.static_enum(object) {
                    return self.compile_enum_member(&enum_name, field, span);
                }
                // `ui.palette` where `ui` is an unshadowed module alias
                // resolves statically in the module's exports.
                if let Some(tid) = self.try_module_member(object, field) {
                    return tid;
                }
                let obj_tid = self.compile_expr(object);
                let field_const = self.constants.intern(ConstantValue::String(field.clone()));
                let op = if spine_is_optional(object) {
                    TermOp::GetFieldOpt(field_const)
                } else {
                    TermOp::GetField(field_const)
                };
                self.emit_term(op, smallvec![obj_tid], None)
            }

            ExprKind::IndexAccess { object, index } => {
                let obj_tid = self.compile_expr(object);
                let idx_tid = self.compile_expr(index);
                let op = if spine_is_optional(object) {
                    TermOp::GetIndexOpt
                } else {
                    TermOp::GetIndex
                };
                self.emit_term(op, smallvec![obj_tid, idx_tid], None)
            }

            // `a?.b` — the same absence-tolerant spine a `??` left-hand side
            // gets, requested explicitly.
            ExprKind::OptionalAccess(inner) => self.compile_expr_optional(inner),

            ExprKind::Lambda { params, body } => {
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                let optional = params.iter().filter(|p| p.has_default()).count();
                // A lambda has no return-type slot: its tail is always its value.
                self.compile_function(None, &param_names, optional, body, None, true, Some(span))
            }

            ExprKind::Element {
                tag,
                props,
                children,
            } => self.compile_element(tag, props, children),

            ExprKind::StringInterp { parts, exprs } => self.compile_string_interp(parts, exprs),
        }
    }

    /// A variable reference: resolve through scope, inserting a capture
    /// phantom when the binding crosses a function boundary. Unresolved
    /// names compile to an `Error` term (with a hint for common slips from
    /// other languages) that only fires if actually executed.
    /// `get name` — read a cell's contents, explicitly.
    ///
    /// The mirror of `set`: `set` is the only way to write a cell, `get` is the
    /// unambiguous way to read one. It is an error on anything that is not a
    /// `var`, so the keyword never appears on a binding whose reads are already
    /// plain dataflow edges — seeing `get` in a body always means a cell.
    fn compile_cell_get(&mut self, name: &str, span: SourceSpan) -> TermId {
        if self.scope_lookup(name).is_none() {
            // Same shape as a bare undefined name: a deferred error that only
            // fires if the expression actually runs.
            return self.compile_ident(name);
        }
        self.check_import_use(name, span);
        if !self.binding_is_var(name) {
            self.error_at(
                span,
                format!(
                    "`{name}` is not a `var`, so `get {name}` has nothing to read — \
                     a `let` or `state` read is already a plain value. Drop the `get`."
                ),
            );
        }
        self.compile_ident(name)
    }

    /// Reject a bare read of a cell that is bound outside this function.
    ///
    /// Inside the declaring scope a bare read is fine — there is no captured
    /// snapshot there for it to be confused with. Across a function boundary
    /// there is, and the two answers differ by a frame in a script that re-runs,
    /// so the read has to say which one it wants.
    fn check_cell_read_says_get(&mut self, name: &str, span: SourceSpan) {
        if self.binding_is_var(name) && self.is_outer_function_binding(name) {
            self.error_at(
                span,
                format!(
                    "`{name}` is a `var` declared outside this function; write `get {name}` \
                     to read the cell's current value. A bare name here would read the value \
                     captured when this function was defined, which is a different thing."
                ),
            );
        }
    }

    /// Warn about a capture of a reactive (`state`) module binding that is
    /// rebound after this closure was defined.
    ///
    /// The capture is by value at the closure's own position, so a rebinding
    /// below it produces a term the closure never sees — and for a `state`, in
    /// a script that re-runs each frame, that reads as one frame of lag. A
    /// `let` is left alone: capture-at-definition is its defined meaning. See
    /// [`crate::compiler::capture_lag`].
    fn check_capture_is_not_rebound(&mut self, name: &str, span: SourceSpan) {
        // Cells are exempt: their reads are live, and `get` marks them.
        if self.binding_is_var(name) || !self.is_outer_function_binding(name) {
            return;
        }
        // Only module bindings. A capture of an enclosing *function's* local
        // has the same shape, but its rebindings are not in the module-level
        // scan, so it is out of scope here and left for a follow-up.
        let Some(&outermost_fn) = self.function_boundaries.first() else {
            return;
        };
        if !self.binding_is_below_scope(name, outermost_fn) {
            return;
        }
        // The freeze point is the *outermost* open closure: an inner lambda
        // only re-captures what the enclosing function already froze. `None`
        // there means the outermost closure is an inline lambda, which the
        // rule exempts.
        let Some(Some(frozen_at)) = self.closure_def_ends.first().copied() else {
            return;
        };
        let Some(rebind) = self.module_rebinds.lagging_rebind(name, frozen_at) else {
            return;
        };
        self.warnings.push(crate::diagnostic::Diagnostic::citing(
            span,
            format!(
                "`{name}` is a `state` binding written on line {{line}}, after this function \
                 was defined, so this reads the value captured at the definition — one run \
                 behind the write. Pass it in as a parameter if you want the current value."
            ),
            rebind,
        ));
    }

    pub(super) fn compile_ident(&mut self, name: &str) -> TermId {
        // An enum's name read as a value is the record of its members.
        if let Some(enum_name) = self.enum_bound_to(name) {
            return self.emit_enum_record(&enum_name);
        }
        // In an enum's constant data, a function of this file that is not
        // hoisted is still the empty box its declaration will fill: calling
        // it would be `Cannot call nil` at run time, with nothing to say why.
        // A lambda in the data runs later, when the box is full.
        if let Some(scope) = &self.enum_data_scope
            && let Some(tid) = self.scope_lookup(name)
            && scope.late_fns.contains(&tid)
            && self.closure_def_ends.is_empty()
        {
            let msg = format!(
                "`{name}` cannot be used in an enum's constant data: the data is evaluated \
                 before the file's statements run, and `{name}` reads something the file \
                 computes as it runs, so it is not defined yet"
            );
            let msg_cid = self.constants.intern(ConstantValue::String(msg));
            return self.emit_term(TermOp::Error(msg_cid), smallvec![], None);
        }
        if let Some(tid) = self.scope_lookup(name) {
            self.note_native_use(tid);
        }
        if let Some(local_tid) = self.resolve_local_term(name) {
            // A `var` binds the *cell*, so reading the name dereferences it.
            // This is the only place reads of a var are turned into `CellRead`,
            // which is what makes the containment invariant hold by
            // construction: nothing else forwards a cell into the value domain.
            // A hoisted `fn` binds its cell for the same reason a `var` does —
            // the box exists before the value — so a read dereferences it too.
            // Unlike a `var` this needs no `get`: the cell is an implementation
            // detail of declaration order, written exactly once, so a bare read
            // has only one possible meaning.
            let op = if self.binding_is_var(name) || self.binding_is_fn_cell(name) {
                TermOp::CellRead
            } else {
                TermOp::Copy
            };
            self.emit_term(op, smallvec![local_tid], None)
        } else if let Some(module) = self.module_aliases.get(name) {
            // A module alias is not a runtime value.
            let written = self.alias_written.get(name).unwrap_or(module);
            let msg = format!(
                "'{}' is a module, not a value — use {}.<name>, or `import {}: <name>` \
                 to bind a member directly",
                name, name, written
            );
            let msg_cid = self.constants.intern(ConstantValue::String(msg));
            self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
        } else {
            let hint = match name {
                "var" => Some("use 'let' to declare variables in Petal"),
                "def" | "func" | "function" => Some("use 'fn' to define functions in Petal"),
                "elif" | "elseif" | "elsif" => Some("use 'else if' in Petal"),
                "switch" | "case" => Some("use 'match' for pattern matching in Petal"),
                "lambda" => Some("use 'fn' for anonymous functions, e.g. fn(x) { x + 1 }"),
                "null" | "undefined" | "None" => Some("use 'nil' for null/empty values in Petal"),
                "console" => Some("use 'print()' for output in Petal"),
                "typeof" => Some("use 'type()' to get the type of a value in Petal"),
                "Math" => Some(
                    "math functions are top-level in Petal: abs(), sqrt(), floor(), ceil(), round()",
                ),
                "require" => Some("use 'import <module>' at the top of the file"),
                "and" => Some("Petal spells logical and as `&&`"),
                "or" => Some("Petal spells logical or as `||`"),
                "not" => Some("Petal spells logical not as `!`"),
                _ => None,
            };
            if self
                .enum_data_scope
                .as_ref()
                .is_some_and(|scope| scope.unavailable.contains(name))
            {
                let msg = format!(
                    "`{name}` cannot be used in an enum's constant data: the data is evaluated \
                     before the file's statements run, so it may use literals, enum variants, \
                     constant `let`s and functions, but not a `let`, `var` or `state` the file \
                     computes as it runs"
                );
                let msg_cid = self.constants.intern(ConstantValue::String(msg));
                return self.emit_term(TermOp::Error(msg_cid), smallvec![], None);
            }
            let builtin_hint = crate::native_decl::missing_builtin_import_hint(name);
            // A variant of that name in scope is likelier than a habit from
            // another language (`None`).
            let variant_hint = self.bare_variant_hint(name, None);
            let hint = variant_hint.as_deref().or(hint).or(builtin_hint.as_deref());
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
        let module = self.module_aliases.get(name)?.clone();
        let qualified = format!("{module}::{member}");
        // A binding of the file's own (a local, a parameter, a declaration)
        // shadows the qualifier: `ui` is then an ordinary value and `.member`
        // is field access. A value the name had *before* the file declared
        // anything does not — a builtin called `noise` beside `import
        // ./noise`, or the function `noise` that `import noise` itself binds
        // bare. There the module's member wins when it has one, and the value
        // is what is left otherwise.
        if let Some(bound) = self.scope_lookup(name) {
            let unshadowed = self.alias_floor.get(name) == Some(&Some(bound));
            if !unshadowed || self.scope_lookup(&qualified).is_none() {
                return None;
            }
        }
        if let Some(tid) = self.scope_lookup(&qualified) {
            if let Some(ambiguous) = self.ambiguous_imports.get(&tid) {
                let message = ambiguous.message();
                self.error_at(object.span, message);
            }
            return Some(self.compile_ident(&qualified));
        }
        // A non-exported name is absent from the global scope, so the two cases
        // (declared-but-private vs. never declared) look the same here; the
        // message points at the likely cause — a missing `pub`.
        let msg = format!(
            "module '{}' has no export '{}' (declarations are private unless \
             marked `pub`)",
            module, member
        );
        let msg_cid = self.constants.intern(ConstantValue::String(msg));
        Some(self.emit_term(TermOp::Error(msg_cid), smallvec![], None))
    }

    /// `value_used` is threaded down to the branch bodies: an `if` whose value
    /// is consumed puts each branch's last statement in value position.
    fn compile_if(
        &mut self,
        condition: &Expr,
        then_body: &[Stmt],
        else_body: Option<&ElseBranch>,
        value_used: bool,
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
            c.compile_stmts(then_body, value_used);
            c.carry_slots.pop();
        });

        // Compile else body
        self.compile_in_block(else_block, |c| {
            c.seed_arm_entry_copies(else_block, &phis);
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    c.compile_stmts(stmts, value_used);
                }
                Some(ElseBranch::ElseIf(expr)) => {
                    c.value_used = value_used;
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

    /// `value_used` is threaded down to the arm bodies, as [`Self::compile_if`]
    /// does for its branches: a `match` whose value is consumed puts each arm's
    /// tail in value position, a discarded one does not.
    fn compile_match(
        &mut self,
        subject: &Expr,
        arms: &[MatchArm],
        value_used: bool,
        span: SourceSpan,
    ) -> TermId {
        let subj_tid = self.compile_expr(subject);

        // Pre-scan all arm bodies for names that will be rebound,
        // emit phis in the parent block before the Match term.
        let bodies: Vec<&[Stmt]> = arms.iter().map(|a| a.body.as_slice()).collect();
        let names = self.detect_rebinds_stmts(&bodies);
        let phis = self.emit_phis(&names, span);

        let mut child_blocks: SmallVec<[BlockId; 2]> = SmallVec::new();
        let mut arm_metas = Vec::new();

        // A pattern that did not resolve was reported; what the match covers
        // is not known then, and is not checked.
        let mut patterns_resolved = true;
        for arm in arms {
            // Body block
            let body_block = self.new_block(None);
            child_blocks.push(body_block);

            // Resolve each variant pattern (`Kind.park`) to the tag its value
            // carries; a pattern naming no variant is reported here.
            let errors_before = self.errors.len();
            let pattern = self.resolve_pattern(&arm.pattern, arm.pattern_span);
            patterns_resolved &= self.errors.len() == errors_before;

            // The names the pattern binds.
            let pattern_vars = pattern.bound_names();

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
                c.compile_stmts(&arm.body, value_used);
                c.carry_slots.pop();
            });

            arm_metas.push(MatchArmMeta {
                pattern,
                guard_block,
                body_block,
            });
        }

        if patterns_resolved {
            self.check_match(span, arms, &arm_metas);
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
            self.emit_term(
                TermOp::AllocMap {
                    fields: field_names,
                    class: None,
                },
                inputs,
                None,
            )
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
        let tag_cid = self
            .constants
            .intern(ConstantValue::String(tag.to_string()));
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

        self.emit_term(
            TermOp::AllocElement {
                tag: tag_cid,
                prop_keys,
            },
            inputs,
            None,
        )
    }

    /// Build: str(parts[0]) ++ str(exprs[0]) ++ str(parts[1]) ++ ... ++ str(parts[N]).
    /// Concat already handles string conversion in the evaluator.
    fn compile_string_interp(&mut self, parts: &[String], exprs: &[Expr]) -> TermId {
        // Start with the first string part
        let first_cid = self
            .constants
            .intern(ConstantValue::String(parts[0].clone()));
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

    /// Compile the left side of `??` in absence-tolerant mode: every link of a
    /// *field/index access spine* reads with the `Opt` op, so a record that
    /// simply does not carry the key produces `Nil` — which is exactly what the
    /// `??` then coalesces — instead of aborting the frame before the
    /// `Coalesce` term can run.
    ///
    /// Only the access spine is softened, and only under a `??`. A bare
    /// `rec.missing` keeps its hard error, as does a wrong-typed base (`3.x`)
    /// and an out-of-bounds list index, on either side of the operator. So
    /// `ragged.maybe ?? "default"` works, while a typo'd `rec.filed` still
    /// fails loudly unless the author writes the `??` that asks for tolerance.
    fn compile_expr_optional(&mut self, expr: &Expr) -> TermId {
        match &expr.kind {
            ExprKind::FieldAccess { object, field } => {
                // A module member is a static resolution, not a record read.
                if let Some(tid) = self.try_module_member(object, field) {
                    return tid;
                }
                let obj_tid = self.compile_expr_optional(object);
                let field_const = self.constants.intern(ConstantValue::String(field.clone()));
                self.emit_term(TermOp::GetFieldOpt(field_const), smallvec![obj_tid], None)
            }
            ExprKind::IndexAccess { object, index } => {
                let obj_tid = self.compile_expr_optional(object);
                let idx_tid = self.compile_expr(index);
                self.emit_term(TermOp::GetIndexOpt, smallvec![obj_tid, idx_tid], None)
            }
            _ => self.compile_expr(expr),
        }
    }

    /// Compile `x ?? y`: the RHS lives in a child block that only runs when the
    /// LHS is absent (`Nil`, `Pending`, or — via [`compile_expr_optional`] — a
    /// record field that is not there at all). Mirrors [`compile_short_circuit`].
    fn compile_coalesce(&mut self, left: &Expr, right: &Expr) -> TermId {
        let left_tid = self.compile_expr_optional(left);
        let rhs_block = self.new_block(None);

        // Compile RHS in its own block
        self.compile_in_block(rhs_block, |c| {
            c.compile_expr(right);
        });

        let tid = self.emit_term_with_children(
            TermOp::Coalesce,
            smallvec![left_tid],
            None,
            smallvec![rhs_block],
        );
        self.blocks[rhs_block.0 as usize].parent_term_id = Some(tid);
        tid
    }

}

/// Does this access chain already carry a written `?.` to its left?
///
/// `?.` short-circuits the **rest** of its chain, the way JavaScript's does: in
/// `cfg?.window.width` a missing `window` yields nil rather than erroring on the
/// `.width`. The parser marks only the link that was written `?.`
/// ([`ExprKind::OptionalAccess`]), so the plain `FieldAccess`/`IndexAccess` arms
/// of [`Compiler::compile_expr`] ask this of their object to decide between the
/// hard read and the tolerant one.
///
/// Only the spine is walked. A `?.` inside a call argument or an index
/// expression is a different chain and does not leak out of it.
fn spine_is_optional(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::OptionalAccess(_) => true,
        ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => {
            spine_is_optional(object)
        }
        _ => false,
    }
}
