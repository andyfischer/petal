//! Function compilation: fn declarations (incl. arity overloads), lambdas,
//! enum constructors, and closure-capture tracking.

use super::*;

impl Compiler {
    /// `fn name(params) { body }`. Overloaded functions (same name declared
    /// with several arities) compile each variant under an internal
    /// "name#arity" and are joined into an overload set once all variants
    /// have been seen.
    /// Returns the term the name is now bound to, or `None` when this was one
    /// variant of an overload set that is not complete yet (the set's term
    /// appears only once every arity has been compiled). Callers that need the
    /// *callable value* — method registration — must use the returned term, not
    /// the individual variant.
    pub(super) fn compile_fn_decl(
        &mut self,
        name: &str,
        params: &[String],
        body: &[Stmt],
    ) -> Option<TermId> {
        let Some(&expected_count) = self.overloaded_fns.get(name) else {
            let closure_tid = self.compile_function(Some(name.to_string()), params, body);
            // Module functions carry a qualified display name ("ui::button")
            // so root-frame harvesting exposes them to `Env::call_function`
            // without colliding with the entry file's names. The scope
            // binding stays bare — in-module references are unqualified.
            self.terms[closure_tid.0 as usize].name = Some(self.qualified_name(name));
            self.scope_bind(name.to_string(), closure_tid);
            return Some(closure_tid);
        };

        // Overloaded function: compile with internal name "name#arity"
        let internal_name = format!("{}#{}", name, params.len());
        let closure_tid = self.compile_function(Some(internal_name), params, body);
        self.overload_variants
            .entry(name.to_string())
            .or_default()
            .push(closure_tid);

        // Once all variants are compiled, emit the overload set
        let compiled_count = self.overload_variants[name].len();
        if compiled_count == expected_count {
            let inputs: SmallVec<[TermId; 4]> =
                self.overload_variants[name].clone().into_iter().collect();
            let set_tid = self.emit_term(
                TermOp::MakeOverloadSet,
                inputs,
                Some(self.qualified_name(name)),
            );
            self.scope_bind(name.to_string(), set_tid);
            return Some(set_tid);
        }
        None
    }

    /// `class Name` compiles to a constructor function taking one parameter per
    /// declared field, in order, and allocating the record tagged with the
    /// class name. Deliberately the same shape as an enum variant's
    /// constructor ([`Compiler::compile_enum_constructor`]) — a class is a
    /// named product type, and `Point(1, 2)` is an ordinary call.
    ///
    /// Built-in classes take the other road: their constructors are natives
    /// (`crate::builtins::classes`), so a program that never mentions `Rect`
    /// pays nothing for it.
    pub(super) fn compile_class_constructor(
        &mut self,
        name: &str,
        fields: &[ClassFieldDecl],
    ) -> TermId {
        let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
        let (body_block, saved_block) = self.begin_function_scope(&field_names);

        let inputs: SmallVec<[TermId; 4]> = field_names
            .iter()
            .map(|f| self.scope_lookup(f).expect("constructor parameter"))
            .collect();
        let keys: Vec<ConstantId> = field_names
            .iter()
            .map(|f| self.constants.intern(ConstantValue::String(f.clone())))
            .collect();
        let class_const = self
            .constants
            .intern(ConstantValue::String(name.to_string()));
        self.emit_term(
            TermOp::AllocMap {
                fields: keys,
                class: Some(class_const),
            },
            inputs,
            None,
        );

        self.end_function_scope(
            Some(name.to_string()),
            &field_names,
            body_block,
            saved_block,
            None,
        )
    }

    /// Emit the root-block statement that publishes `fn Class.method` into the
    /// VM's per-run method table, which is what `value.method(...)` consults.
    /// A builtin call rather than an instruction of its own: registration is a
    /// side effect on runtime state, exactly what the native boundary is for.
    pub(super) fn emit_declare_method(&mut self, class: &str, method: &str, func: TermId) {
        let class_c = self
            .constants
            .intern(ConstantValue::String(class.to_string()));
        let method_c = self
            .constants
            .intern(ConstantValue::String(method.to_string()));
        let class_tid = self.emit_term(TermOp::Constant(class_c), smallvec![], None);
        let method_tid = self.emit_term(TermOp::Constant(method_c), smallvec![], None);
        let name_c = self
            .constants
            .intern(ConstantValue::String(DECLARE_METHOD_BUILTIN.to_string()));
        self.emit_term(
            TermOp::BuiltinCall(name_c),
            smallvec![class_tid, method_tid, func],
            None,
        );
    }

    pub(super) fn compile_function(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Stmt],
    ) -> TermId {
        let (body_block, saved_block) = self.begin_function_scope(params);

        // Self-reference phantom for recursion (if named)
        let self_ref_register = if let Some(ref fn_name) = name {
            let self_ref = self.emit_phantom_term(fn_name.clone());
            self.scope_bind(fn_name.clone(), self_ref);
            Some(self.terms[self_ref.0 as usize].register)
        } else {
            None
        };

        // Compile body (this may discover captures)
        for s in body {
            self.compile_stmt(s);
        }

        self.end_function_scope(name, params, body_block, saved_block, self_ref_register)
    }

    /// An enum variant with fields compiles to a constructor function whose
    /// body emits the variant from its parameters.
    pub(super) fn compile_enum_constructor(&mut self, variant: &EnumVariant) -> TermId {
        let (body_block, saved_block) = self.begin_function_scope(&variant.fields);

        // Collect phantom term IDs for the fields (already created by begin_function_scope)
        let field_tids: SmallVec<[TermId; 4]> = variant
            .fields
            .iter()
            .map(|f| self.scope_lookup(f).unwrap())
            .collect();

        // Emit MakeEnumVariant
        let name_const = self
            .constants
            .intern(ConstantValue::String(variant.name.clone()));
        self.emit_term(TermOp::MakeEnumVariant(name_const), field_tids, None);

        self.end_function_scope(
            Some(variant.name.clone()),
            &variant.fields,
            body_block,
            saved_block,
            None,
        )
    }

    /// Enter a new function body scope. Returns (body_block, saved_block).
    /// After calling this, compile the body, then call `end_function_scope`.
    fn begin_function_scope(&mut self, params: &[String]) -> (BlockId, BlockId) {
        let body_block = self.new_block(None);
        self.blocks[body_block.0 as usize].param_names = params.to_vec();

        let saved_block = self.set_block(body_block);
        self.push_scope(true); // function boundary
        self.capture_stack.push(Vec::new());
        self.function_body_blocks.push(body_block);

        // Bind params as phantom terms
        for param in params {
            let param_tid = self.emit_phantom_term(param.clone());
            self.scope_bind(param.clone(), param_tid);
        }

        (body_block, saved_block)
    }

    /// End a function scope, collect captures, create FunctionDef, and emit
    /// MakeClosure. Returns the TermId of the MakeClosure term.
    fn end_function_scope(
        &mut self,
        name: Option<String>,
        params: &[String],
        body_block: BlockId,
        saved_block: BlockId,
        self_ref_register: Option<RegisterIndex>,
    ) -> TermId {
        self.finalize_block(body_block);
        let body_reg_count = self.blocks[body_block.0 as usize].register_count;

        self.function_body_blocks.pop();
        let captures = self.capture_stack.pop().unwrap_or_default();
        let capture_names: Vec<String> = captures.iter().map(|c| c.name.clone()).collect();
        let capture_outer_tids: SmallVec<[TermId; 4]> =
            captures.iter().map(|c| c.outer_tid).collect();
        let capture_registers: Vec<RegisterIndex> = captures
            .iter()
            .map(|c| self.terms[c.local_phantom.0 as usize].register)
            .collect();

        self.pop_scope();
        self.set_block(saved_block);

        // Compute fn_id now, after body compilation so inner functions have
        // already been added to self.functions.
        let fn_id = FunctionId(self.functions.len() as u32);

        self.functions.push(FunctionDef {
            id: fn_id,
            name: name.clone(),
            params: params.to_vec(),
            body_block,
            capture_names,
            capture_registers,
            self_ref_register,
            register_count: body_reg_count,
        });

        self.emit_term(TermOp::MakeClosure(fn_id), capture_outer_tids, name)
    }

    // -----------------------------------------------------------------------
    // Capture tracking
    // -----------------------------------------------------------------------

    /// Check if a name's binding is from an outer function scope (needs capture).
    pub(super) fn needs_capture(&self, name: &str) -> bool {
        if self.function_boundaries.is_empty() {
            return false;
        }
        let current_fn_boundary = *self.function_boundaries.last().unwrap();
        // Search from innermost scope outward
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                // Found it — is it below the current function boundary?
                return i < current_fn_boundary;
            }
        }
        false
    }

    /// Is `name` bound outside the function currently being compiled?
    ///
    /// Unlike [`Self::needs_capture`] this stays true after the capture has
    /// been created. A capture phantom is bound *in* the function's boundary
    /// scope (so later reads resolve to it directly), which makes
    /// `needs_capture` report `false` from the second reference onward — fine
    /// for capture bookkeeping, wrong for diagnosing assignment, which must
    /// fire at every site regardless of what the body read first.
    pub(super) fn is_outer_function_binding(&self, name: &str) -> bool {
        let Some(&boundary) = self.function_boundaries.last() else {
            return false;
        };
        // A phi / seed copy / reassignment standing in for the outer binding
        // (see `cross_fn_terms`) is bound locally but is still that binding.
        if self
            .scope_lookup(name)
            .is_some_and(|tid| self.cross_fn_terms.contains(&tid))
        {
            return true;
        }
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return i < boundary
                    // At the boundary scope itself, params and locals sit
                    // alongside capture phantoms; only the latter stand in for
                    // a binding from an enclosing function.
                    || (i == boundary
                        && self
                            .capture_stack
                            .last()
                            .is_some_and(|caps| caps.iter().any(|c| c.name == name)));
            }
        }
        false
    }

    /// Get or create a capture phantom for a cross-function variable reference.
    ///
    /// When the binding lives several function boundaries out from the
    /// reference (e.g. a lambda nested in a `fn` that references a top-level
    /// function), a single capture on the innermost function is not enough:
    /// its `outer_tid` would point at a term living in a block that the
    /// intermediate functions don't own, which the graph runtime reads as
    /// nil and the bytecode lowerer rejects outright. Instead we thread the
    /// value through *every* enclosing function between the binding and the
    /// reference, chaining each level's capture source to the previous
    /// level's local phantom. Returns the innermost function's phantom.
    pub(super) fn get_or_add_capture(&mut self, name: &str, outer_tid: TermId) -> TermId {
        // Locate the scope that binds `name` (innermost binding). The caller
        // has already confirmed via `scope_lookup`/`needs_capture` that it
        // exists and crosses at least one function boundary.
        let Some(binding_scope_idx) = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, scope)| scope.contains_key(name))
            .map(|(i, _)| i)
        else {
            return outer_tid;
        };

        // A captured `var` stays a `var` inside the capturing function: the
        // captured value is the cell id, so the phantom denotes the same box.
        // Without this the phantom would bind as an ordinary `let` and `set x`
        // inside the closure would be rejected as "not a `var`" — and, worse,
        // a read would forward the raw cell instead of dereferencing it.
        let is_var = self.var_scopes[binding_scope_idx].contains(name);

        // Every enclosing function whose boundary sits *inside* the binding
        // scope must capture the value and forward it inward.
        let mut source_tid = outer_tid;
        for level in 0..self.function_boundaries.len() {
            if self.function_boundaries[level] <= binding_scope_idx {
                // At or above the binding: this function sees it directly.
                continue;
            }
            source_tid = self.add_capture_at_level(level, name, source_tid, is_var);
        }
        source_tid
    }

    /// Register a capture of `name` (sourced from `source_tid` in the
    /// enclosing frame) on the function at stack `level`, returning the local
    /// phantom that stands in for the value inside that function. Reuses an
    /// existing capture for the same name when one is already present.
    fn add_capture_at_level(
        &mut self,
        level: usize,
        name: &str,
        source_tid: TermId,
        is_var: bool,
    ) -> TermId {
        if let Some(cap) = self.capture_stack[level].iter().find(|c| c.name == name) {
            return cap.local_phantom;
        }

        // Create the capture phantom in this function's body block, where the
        // runtime deposits captured values at call time — capture_registers
        // must reference registers in that block.
        let body_block = self.function_body_blocks[level];
        let saved_block = self.current_block;
        self.current_block = body_block;
        let phantom = self.emit_phantom_term(name.to_string());
        self.current_block = saved_block;

        self.capture_stack[level].push(CaptureInfo {
            outer_tid: source_tid,
            local_phantom: phantom,
            name: name.to_string(),
        });
        // Bind in this function's boundary scope so later references (and
        // deeper captures) resolve to the local phantom rather than
        // re-reaching the outer term.
        let boundary_scope = self.function_boundaries[level];
        self.scopes[boundary_scope].insert(name.to_string(), phantom);
        if is_var {
            self.var_scopes[boundary_scope].insert(name.to_string());
        }
        phantom
    }
}
