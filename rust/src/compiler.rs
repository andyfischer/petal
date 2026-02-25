//! Compiler - Transforms AST into term-graph IR.
//!
//! Single recursive pass over the AST, emitting terms and blocks.

use std::collections::HashMap;

use smallvec::{smallvec, SmallVec};

use crate::ast::*;
use crate::builtins::BuiltinTable;
use crate::constant_table::{ConstantTable, ConstantValue};
use crate::program::*;
use crate::source_map::SourceMap;

/// Info about a captured variable in the current function being compiled.
struct CaptureInfo {
    /// Term in the outer scope providing the value
    outer_tid: TermId,
    /// Phantom term in the function body that holds the capture value
    local_phantom: TermId,
    /// Variable name
    name: String,
}

/// Compiler state for a single compilation.
pub struct Compiler {
    terms: Vec<Term>,
    blocks: Vec<Block>,
    constants: ConstantTable,
    source_map: SourceMap,
    functions: Vec<FunctionDef>,
    match_arms: HashMap<TermId, Vec<MatchArmMeta>>,

    // Current compilation state
    current_block: BlockId,
    last_term_in_block: HashMap<BlockId, TermId>,
    scopes: Vec<HashMap<String, TermId>>,
    enum_variants: HashMap<String, usize>, // variant name -> field count
    next_register: HashMap<BlockId, u16>,

    // Builtin table for resolving builtin names at compile time
    builtins: BuiltinTable,

    // Function scope depth tracking for closure capture
    function_boundaries: Vec<usize>, // scope indices that are function boundaries

    // Capture tracking for the current function being compiled (stack for nesting)
    capture_stack: Vec<Vec<CaptureInfo>>,

    // Track function body blocks so capture phantoms are created in the right block
    function_body_blocks: Vec<BlockId>,
}

impl Compiler {
    pub fn new() -> Self {
        Self {
            terms: Vec::new(),
            blocks: Vec::new(),
            constants: ConstantTable::new(),
            source_map: SourceMap::new(),
            functions: Vec::new(),
            match_arms: HashMap::new(),
            current_block: BlockId(0),
            last_term_in_block: HashMap::new(),
            scopes: Vec::new(),
            enum_variants: HashMap::new(),
            next_register: HashMap::new(),
            builtins: BuiltinTable::new(),
            function_boundaries: Vec::new(),
            capture_stack: Vec::new(),
            function_body_blocks: Vec::new(),
        }
    }

    /// Compile a list of statements into a Program.
    pub fn compile(
        mut self,
        stmts: &[Stmt],
        source: String,
        program_id: ProgramId,
    ) -> Program {
        // Create root block
        let root_block = self.new_block(None);
        self.current_block = root_block;

        // Push global scope
        self.push_scope(false);

        // Register builtins in global scope as phantom terms.
        let builtin_count = self.builtins.count();
        let builtin_names: Vec<String> = (0..builtin_count)
            .map(|i| self.builtins.get_name(BuiltinId(i as u16)).to_string())
            .collect();
        for name in builtin_names {
            let tid = self.emit_phantom_term(name.clone());
            self.scope_bind(name, tid);
        }

        // Pre-scan for fn and enum declarations to allow forward references
        self.prescan_declarations(stmts);

        // Compile all statements
        for stmt in stmts {
            self.compile_stmt(stmt);
        }

        // Finalize root block
        self.finalize_block(root_block);

        self.pop_scope();

        Program {
            id: program_id,
            source,
            terms: self.terms,
            blocks: self.blocks,
            root_block,
            constants: self.constants,
            source_map: self.source_map,
            has_errors: false,
            functions: self.functions,
            match_arms: self.match_arms,
        }
    }

    // -----------------------------------------------------------------------
    // Block management
    // -----------------------------------------------------------------------

    fn new_block(&mut self, parent_term: Option<TermId>) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(Block {
            id,
            parent_term_id: parent_term,
            entry: None,
            param_names: Vec::new(),
            register_count: 0,
        });
        self.next_register.insert(id, 0);
        id
    }

    fn set_block(&mut self, block_id: BlockId) -> BlockId {
        let old = self.current_block;
        self.current_block = block_id;
        old
    }

    /// Finalize a block's register count after compilation.
    fn finalize_block(&mut self, block_id: BlockId) {
        let reg_count = self.next_register.get(&block_id).copied().unwrap_or(0);
        self.blocks[block_id.0 as usize].register_count = reg_count;
    }

    /// Switch to a block, push a new scope, run the compilation closure,
    /// then finalize, pop scope, and restore the previous block.
    fn compile_in_block<F>(&mut self, block_id: BlockId, f: F)
    where
        F: FnOnce(&mut Self),
    {
        let saved = self.set_block(block_id);
        self.push_scope(false);
        f(self);
        self.finalize_block(block_id);
        self.pop_scope();
        self.set_block(saved);
    }

    // -----------------------------------------------------------------------
    // Scope management
    // -----------------------------------------------------------------------

    fn push_scope(&mut self, is_function_boundary: bool) {
        if is_function_boundary {
            self.function_boundaries.push(self.scopes.len());
        }
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        if let Some(&boundary) = self.function_boundaries.last() {
            if boundary >= self.scopes.len() {
                self.function_boundaries.pop();
            }
        }
    }

    fn scope_bind(&mut self, name: String, term_id: TermId) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, term_id);
        }
    }

    /// Bind a name in the current function's scope (not the innermost scope).
    /// Used for captures so nested functions see the local phantom.
    fn bind_in_function_scope(&mut self, name: String, tid: TermId) {
        if let Some(&boundary_idx) = self.function_boundaries.last() {
            if boundary_idx < self.scopes.len() {
                self.scopes[boundary_idx].insert(name, tid);
                return;
            }
        }
        // Fallback: bind in current scope
        self.scope_bind(name, tid);
    }

    fn scope_lookup(&self, name: &str) -> Option<TermId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&tid) = scope.get(name) {
                return Some(tid);
            }
        }
        None
    }

    /// Check if a name's binding is from an outer function scope (needs capture).
    fn needs_capture(&self, name: &str) -> bool {
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

    /// Get or create a capture phantom for a cross-function variable reference.
    fn get_or_add_capture(&mut self, name: &str, outer_tid: TermId) -> TermId {
        // Check if already captured
        if let Some(captures) = self.capture_stack.last() {
            for cap in captures {
                if cap.name == name {
                    return cap.local_phantom;
                }
            }
        }
        // Create capture phantom in the FUNCTION BODY block (not current block).
        // This is critical because capture_registers must reference registers in the
        // function body block, where the evaluator places captured values at call time.
        let saved_block = self.current_block;
        if let Some(&fn_body_block) = self.function_body_blocks.last() {
            self.current_block = fn_body_block;
        }
        let phantom = self.emit_phantom_term(name.to_string());
        self.current_block = saved_block;

        if let Some(captures) = self.capture_stack.last_mut() {
            captures.push(CaptureInfo {
                outer_tid,
                local_phantom: phantom,
                name: name.to_string(),
            });
        }
        // Bind in function scope so nested functions see the local phantom
        self.bind_in_function_scope(name.to_string(), phantom);
        phantom
    }

    // -----------------------------------------------------------------------
    // Term emission
    // -----------------------------------------------------------------------

    fn emit_term(
        &mut self,
        op: TermOp,
        inputs: SmallVec<[TermId; 4]>,
        name: Option<String>,
    ) -> TermId {
        let block_id = self.current_block;
        let reg = self.alloc_register(block_id);
        let term_id = TermId(self.terms.len() as u32);

        let prev = self.last_term_in_block.get(&block_id).copied();

        let term = Term {
            id: term_id,
            op,
            inputs,
            block_id,
            block_next: None,
            block_prev: prev,
            name,
            register: reg,
            state_key: None,
            child_blocks: SmallVec::new(),
        };

        self.terms.push(term);

        // Link prev -> this
        if let Some(prev_id) = prev {
            self.terms[prev_id.0 as usize].block_next = Some(term_id);
        } else {
            // First term in block — set as entry
            self.blocks[block_id.0 as usize].entry = Some(term_id);
        }

        self.last_term_in_block.insert(block_id, term_id);
        term_id
    }

    fn emit_term_with_children(
        &mut self,
        op: TermOp,
        inputs: SmallVec<[TermId; 4]>,
        name: Option<String>,
        child_blocks: SmallVec<[BlockId; 2]>,
    ) -> TermId {
        let tid = self.emit_term(op, inputs, name);
        self.terms[tid.0 as usize].child_blocks = child_blocks;
        tid
    }

    /// Create a phantom term — allocates a register and creates a term for scope
    /// resolution, but does NOT link it into the block's execution list.
    fn emit_phantom_term(&mut self, name: String) -> TermId {
        let block_id = self.current_block;
        let reg = self.alloc_register(block_id);
        let term_id = TermId(self.terms.len() as u32);
        self.terms.push(Term {
            id: term_id,
            op: TermOp::Copy,
            inputs: SmallVec::new(),
            block_id,
            block_next: None,
            block_prev: None,
            name: Some(name),
            register: reg,
            state_key: None,
            child_blocks: SmallVec::new(),
        });
        term_id
    }

    fn alloc_register(&mut self, block_id: BlockId) -> RegisterIndex {
        let reg = self.next_register.get(&block_id).copied().unwrap_or(0);
        self.next_register.insert(block_id, reg + 1);
        RegisterIndex(reg)
    }

    // -----------------------------------------------------------------------
    // Prescan for forward references
    // -----------------------------------------------------------------------

    fn prescan_declarations(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::FnDecl { name, .. } => {
                    let tid = self.emit_phantom_term(name.clone());
                    self.scope_bind(name.clone(), tid);
                }
                Stmt::EnumDecl { variants, .. } => {
                    for variant in variants {
                        self.enum_variants
                            .insert(variant.name.clone(), variant.fields.len());
                        let tid = self.emit_phantom_term(variant.name.clone());
                        self.scope_bind(variant.name.clone(), tid);
                    }
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Statement compilation
    // -----------------------------------------------------------------------

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value } => {
                let val_tid = self.compile_expr(value);
                self.terms[val_tid.0 as usize].name = Some(name.clone());
                self.scope_bind(name.clone(), val_tid);
            }

            Stmt::Assign { target, value } => {
                self.compile_assign(target, value);
            }

            Stmt::Expr(expr) => {
                self.compile_expr(expr);
            }

            Stmt::FnDecl { name, params, body } => {
                let closure_tid = self.compile_function(Some(name.clone()), params, body);
                // Overwrite the placeholder binding
                self.scope_bind(name.clone(), closure_tid);
            }

            Stmt::EnumDecl { name: _, variants } => {
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

            Stmt::For { var, iter, body } => {
                let iter_tid = self.compile_expr(iter);
                let body_block = self.new_block(None);
                self.blocks[body_block.0 as usize].param_names = vec![var.clone()];

                let for_tid = self.emit_term_with_children(
                    TermOp::ForLoop,
                    smallvec![iter_tid],
                    None,
                    smallvec![body_block],
                );
                self.blocks[body_block.0 as usize].parent_term_id = Some(for_tid);

                // Compile body in body_block
                self.compile_in_block(body_block, |c| {
                    // Bind loop variable as phantom — evaluator populates register 0
                    let var_tid = c.emit_phantom_term(var.clone());
                    c.scope_bind(var.clone(), var_tid);
                    for s in body {
                        c.compile_stmt(s);
                    }
                });
            }

            Stmt::While { condition, body } => {
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

                // Compile condition in cond_block
                self.compile_in_block(cond_block, |c| {
                    c.compile_expr(condition);
                });

                // Compile body in body_block
                self.compile_in_block(body_block, |c| {
                    for s in body {
                        c.compile_stmt(s);
                    }
                });
            }

            Stmt::Return(expr) => {
                if let Some(e) = expr {
                    let val_tid = self.compile_expr(e);
                    self.emit_term(TermOp::Return, smallvec![val_tid], None);
                } else {
                    self.emit_term(TermOp::Return, smallvec![], None);
                }
            }

            Stmt::Break => {
                self.emit_term(TermOp::Break, smallvec![], None);
            }

            Stmt::Continue => {
                self.emit_term(TermOp::Continue, smallvec![], None);
            }

            Stmt::State { name, init, id } => {
                let init_tid = self.compile_expr(init);
                let state_key = StateKey(*id as u64);
                let state_tid = self.emit_term(
                    TermOp::StateInit,
                    smallvec![init_tid],
                    Some(name.clone()),
                );
                self.terms[state_tid.0 as usize].state_key = Some(state_key);
                self.scope_bind(name.clone(), state_tid);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Assignment compilation
    // -----------------------------------------------------------------------

    fn compile_assign(&mut self, target: &AssignTarget, value: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                let val_tid = self.compile_expr(value);

                // Check if this is a state variable — if so, emit StateWrite
                if let Some(existing_tid) = self.scope_lookup(name) {
                    if let TermOp::StateInit = &self.terms[existing_tid.0 as usize].op {
                        let state_key = self.terms[existing_tid.0 as usize].state_key;
                        let write_tid = self.emit_term(
                            TermOp::StateWrite,
                            smallvec![val_tid],
                            None,
                        );
                        self.terms[write_tid.0 as usize].state_key = state_key;
                    }
                }

                // Determine if this is a same-block or cross-block assignment
                if let Some(existing_tid) = self.scope_lookup(name) {
                    let existing_block = self.terms[existing_tid.0 as usize].block_id;
                    if existing_block == self.current_block {
                        // Same block — emit Copy and rebind (SSA-like)
                        let assign_tid = self.emit_term(
                            TermOp::Copy,
                            smallvec![val_tid],
                            Some(name.clone()),
                        );
                        self.scope_bind(name.clone(), assign_tid);
                    } else {
                        // Cross-block — write to outer register, don't rebind
                        self.emit_term(
                            TermOp::Assign(existing_tid),
                            smallvec![val_tid],
                            None,
                        );
                    }
                } else {
                    // Variable not found — treat as new binding
                    let assign_tid = self.emit_term(
                        TermOp::Copy,
                        smallvec![val_tid],
                        Some(name.clone()),
                    );
                    self.scope_bind(name.clone(), assign_tid);
                }
            }
            AssignTarget::Field(object, field) => {
                let obj_tid = self.compile_expr(object);
                let val_tid = self.compile_expr(value);
                let field_const = self
                    .constants
                    .intern(ConstantValue::String(field.clone()));
                self.emit_term(
                    TermOp::SetField(field_const),
                    smallvec![obj_tid, val_tid],
                    None,
                );
            }
            AssignTarget::Index(object, index) => {
                let obj_tid = self.compile_expr(object);
                let idx_tid = self.compile_expr(index);
                let val_tid = self.compile_expr(value);
                self.emit_term(
                    TermOp::SetIndex,
                    smallvec![obj_tid, idx_tid, val_tid],
                    None,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expression compilation
    // -----------------------------------------------------------------------

    fn compile_expr(&mut self, expr: &Expr) -> TermId {
        match expr {
            Expr::Literal(lit) => {
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

            Expr::Ident(name) => {
                if let Some(tid) = self.scope_lookup(name) {
                    // Check if this reference crosses a function boundary (needs capture)
                    if self.needs_capture(name) {
                        let local_tid = self.get_or_add_capture(name, tid);
                        self.emit_term(TermOp::Copy, smallvec![local_tid], None)
                    } else {
                        self.emit_term(TermOp::Copy, smallvec![tid], None)
                    }
                } else {
                    let msg_cid = self
                        .constants
                        .intern(ConstantValue::String(format!("Undefined variable: {}", name)));
                    self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
                }
            }

            Expr::BinaryOp { op, left, right } => {
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

            Expr::UnaryOp { op, operand } => {
                let val = self.compile_expr(operand);
                let term_op = match op {
                    UnaryOp::Neg => TermOp::Neg,
                    UnaryOp::Not => TermOp::Not,
                };
                self.emit_term(term_op, smallvec![val], None)
            }

            Expr::Call { function, args } => {
                // Detect method syntax: obj.method(args...)
                if let Expr::FieldAccess { object, field } = function.as_ref() {
                    let obj_tid = self.compile_expr(object);
                    let mut inputs: SmallVec<[TermId; 4]> = smallvec![obj_tid];
                    for arg in args {
                        inputs.push(self.compile_expr(arg));
                    }
                    let field_const = self
                        .constants
                        .intern(ConstantValue::String(field.clone()));
                    self.emit_term(TermOp::MethodCall(field_const), inputs, None)
                } else {
                    let func_tid = self.compile_expr(function);
                    let mut inputs: SmallVec<[TermId; 4]> = smallvec![func_tid];
                    for arg in args {
                        inputs.push(self.compile_expr(arg));
                    }
                    self.emit_term(TermOp::Call, inputs, None)
                }
            }

            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                let cond_tid = self.compile_expr(condition);
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

                // Compile then body
                self.compile_in_block(then_block, |c| {
                    for s in then_body {
                        c.compile_stmt(s);
                    }
                });

                // Compile else body
                self.compile_in_block(else_block, |c| {
                    if let Some(else_br) = else_body {
                        match else_br {
                            ElseBranch::Block(stmts) => {
                                for s in stmts {
                                    c.compile_stmt(s);
                                }
                            }
                            ElseBranch::ElseIf(expr) => {
                                c.compile_expr(expr);
                            }
                        }
                    } else {
                        // No else — emit Nil
                        let nil_cid = c.constants.intern(ConstantValue::Nil);
                        c.emit_term(TermOp::Constant(nil_cid), smallvec![], None);
                    }
                });

                branch_tid
            }

            Expr::Match { subject, arms } => {
                let subj_tid = self.compile_expr(subject);
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

                    // Compile body with pattern variable bindings
                    self.compile_in_block(body_block, |c| {
                        for var_name in &pattern_vars {
                            let phantom = c.emit_phantom_term(var_name.clone());
                            c.scope_bind(var_name.clone(), phantom);
                        }
                        c.compile_expr(&arm.body);
                    });

                    arm_metas.push(MatchArmMeta {
                        pattern,
                        guard_block,
                        body_block,
                    });
                }

                let match_tid = self.emit_term_with_children(
                    TermOp::Match,
                    smallvec![subj_tid],
                    None,
                    child_blocks,
                );

                // Set parent_term_id on all child blocks
                for meta in &arm_metas {
                    self.blocks[meta.body_block.0 as usize].parent_term_id = Some(match_tid);
                    if let Some(gb) = meta.guard_block {
                        self.blocks[gb.0 as usize].parent_term_id = Some(match_tid);
                    }
                }

                self.match_arms.insert(match_tid, arm_metas);
                match_tid
            }

            Expr::List(elements) => {
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                for elem in elements {
                    inputs.push(self.compile_expr(elem));
                }
                self.emit_term(TermOp::AllocList, inputs, None)
            }

            Expr::Record(fields) => {
                let mut field_names = Vec::new();
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                for (key, value) in fields {
                    field_names.push(
                        self.constants
                            .intern(ConstantValue::String(key.clone())),
                    );
                    inputs.push(self.compile_expr(value));
                }
                self.emit_term(
                    TermOp::AllocMap {
                        fields: field_names,
                    },
                    inputs,
                    None,
                )
            }

            Expr::FieldAccess { object, field } => {
                let obj_tid = self.compile_expr(object);
                let field_const = self
                    .constants
                    .intern(ConstantValue::String(field.clone()));
                self.emit_term(TermOp::GetField(field_const), smallvec![obj_tid], None)
            }

            Expr::IndexAccess { object, index } => {
                let obj_tid = self.compile_expr(object);
                let idx_tid = self.compile_expr(index);
                self.emit_term(TermOp::GetIndex, smallvec![obj_tid, idx_tid], None)
            }

            Expr::Block(stmts) => {
                // Compile in a new scope but same block (inline block)
                self.push_scope(false);
                let nil_cid = self.constants.intern(ConstantValue::Nil);
                let mut last_tid = self.emit_term(
                    TermOp::Constant(nil_cid),
                    smallvec![],
                    None,
                );
                for s in stmts {
                    match s {
                        Stmt::Expr(e) => {
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

            Expr::Lambda { params, body } => {
                self.compile_function(None, params, body)
            }

            Expr::Element { tag, props, children } => {
                let tag_cid = self.constants.intern(ConstantValue::String(tag.clone()));
                let mut prop_keys = Vec::new();
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();

                // Compile prop values
                for (key, value) in props {
                    prop_keys.push(
                        self.constants.intern(ConstantValue::String(key.clone())),
                    );
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
                    TermOp::AllocElement { tag: tag_cid, prop_keys },
                    inputs,
                    None,
                )
            }

            Expr::StringInterp { parts, exprs } => {
                // Build: str(parts[0]) ++ str(exprs[0]) ++ str(parts[1]) ++ str(exprs[1]) ++ ... ++ str(parts[N])
                // Start with the first string part
                let first_cid = self.constants.intern(ConstantValue::String(parts[0].clone()));
                let mut result = self.emit_term(TermOp::Constant(first_cid), smallvec![], None);

                for (i, expr) in exprs.iter().enumerate() {
                    // Compile the expression and convert to string via Concat
                    // (Concat already handles str conversion in the evaluator)
                    let expr_tid = self.compile_expr(expr);
                    result = self.emit_term(TermOp::Concat, smallvec![result, expr_tid], None);

                    // Add the next string part
                    let part_cid = self.constants.intern(ConstantValue::String(parts[i + 1].clone()));
                    let part_tid = self.emit_term(TermOp::Constant(part_cid), smallvec![], None);
                    result = self.emit_term(TermOp::Concat, smallvec![result, part_tid], None);
                }

                result
            }
        }
    }

    // -----------------------------------------------------------------------
    // Short-circuit And/Or
    // -----------------------------------------------------------------------

    fn compile_short_circuit(
        &mut self,
        left: &Expr,
        right: &Expr,
        is_and: bool,
    ) -> TermId {
        let left_tid = self.compile_expr(left);
        let rhs_block = self.new_block(None);

        // Compile RHS in its own block
        self.compile_in_block(rhs_block, |c| {
            c.compile_expr(right);
        });

        let op = if is_and { TermOp::And } else { TermOp::Or };
        let tid = self.emit_term_with_children(
            op,
            smallvec![left_tid],
            None,
            smallvec![rhs_block],
        );
        self.blocks[rhs_block.0 as usize].parent_term_id = Some(tid);
        tid
    }

    // -----------------------------------------------------------------------
    // Function compilation
    // -----------------------------------------------------------------------

    fn compile_function(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Stmt],
    ) -> TermId {
        // NOTE: fn_id is computed later (after body compilation) because inner
        // functions get compiled first and added to self.functions.
        let body_block = self.new_block(None);
        self.blocks[body_block.0 as usize].param_names = params.to_vec();

        // Save outer state and start capture tracking
        let saved_block = self.set_block(body_block);
        self.push_scope(true); // function boundary
        self.capture_stack.push(Vec::new());
        self.function_body_blocks.push(body_block);

        // Bind params as phantom terms — evaluator populates these registers
        for param in params {
            let param_tid = self.emit_phantom_term(param.clone());
            self.scope_bind(param.clone(), param_tid);
        }

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

        self.finalize_block(body_block);
        let body_reg_count = self.blocks[body_block.0 as usize].register_count;

        // Collect captures
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

        // Compute fn_id now — after body compilation, so inner functions have
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

        // Emit MakeClosure in the outer block with capture inputs
        self.emit_term(
            TermOp::MakeClosure(fn_id),
            capture_outer_tids,
            name,
        )
    }

    // -----------------------------------------------------------------------
    // Enum constructor
    // -----------------------------------------------------------------------

    fn compile_enum_constructor(&mut self, variant: &EnumVariant) -> TermId {
        let body_block = self.new_block(None);
        self.blocks[body_block.0 as usize].param_names = variant.fields.clone();

        let saved_block = self.set_block(body_block);
        self.push_scope(true);
        self.capture_stack.push(Vec::new());
        self.function_body_blocks.push(body_block);

        // Bind params as phantom terms
        let mut field_tids: SmallVec<[TermId; 4]> = SmallVec::new();
        for field in &variant.fields {
            let tid = self.emit_phantom_term(field.clone());
            self.scope_bind(field.clone(), tid);
            field_tids.push(tid);
        }

        // Emit MakeEnumVariant
        let name_const = self
            .constants
            .intern(ConstantValue::String(variant.name.clone()));
        self.emit_term(TermOp::MakeEnumVariant(name_const), field_tids, None);

        self.finalize_block(body_block);
        let body_reg_count = self.blocks[body_block.0 as usize].register_count;

        self.function_body_blocks.pop();
        let _captures = self.capture_stack.pop().unwrap_or_default();

        self.pop_scope();
        self.set_block(saved_block);

        let fn_id = FunctionId(self.functions.len() as u32);

        self.functions.push(FunctionDef {
            id: fn_id,
            name: Some(variant.name.clone()),
            params: variant.fields.clone(),
            body_block,
            capture_names: Vec::new(),
            capture_registers: Vec::new(),
            self_ref_register: None,
            register_count: body_reg_count,
        });

        self.emit_term(
            TermOp::MakeClosure(fn_id),
            smallvec![],
            Some(variant.name.clone()),
        )
    }

    // -----------------------------------------------------------------------
    // Pattern variable extraction
    // -----------------------------------------------------------------------

    /// Convert Pattern::Variable to Pattern::Variant for known enum variant names.
    /// This ensures pattern matching only matches the actual variant, not any value.
    fn resolve_pattern(&self, pattern: &Pattern) -> Pattern {
        match pattern {
            Pattern::Variable(name) => {
                if let Some(&field_count) = self.enum_variants.get(name) {
                    if field_count == 0 {
                        return Pattern::Variant {
                            name: name.clone(),
                            fields: vec![],
                        };
                    }
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
            Pattern::Record(fields) => {
                fields.iter().flat_map(|(_, p)| Self::extract_pattern_vars(p)).collect()
            }
        }
    }
}
