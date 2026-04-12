//! Compiler - Transforms AST into term-graph IR.
//!
//! Single recursive pass over the AST, emitting terms and blocks.

use std::collections::HashMap;

use smallvec::{smallvec, SmallVec};

use crate::ast::*;
use crate::constant_table::{ConstantTable, ConstantValue};
use crate::native_fn::NativeFnTable;
use crate::program::*;
use crate::source_map::{SourceMap, SourceSpan};

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

    // Function scope depth tracking for closure capture
    function_boundaries: Vec<usize>, // scope indices that are function boundaries

    // Capture tracking for the current function being compiled (stack for nesting)
    capture_stack: Vec<Vec<CaptureInfo>>,

    // Track function body blocks so capture phantoms are created in the right block
    function_body_blocks: Vec<BlockId>,

    // Track loop nesting depth so state terms know if they're inside a loop
    loop_depth: u32,

    // Overloaded function tracking: name → number of unique arities expected
    overloaded_fns: HashMap<String, usize>,
    // Compiled overload variants: name → vec of closure term IDs (one per arity)
    overload_variants: HashMap<String, Vec<TermId>>,

    // Per-block rebinding log: block → (name → latest rebind term in that
    // block). Populated by `compile_assign` when a name bound in an outer
    // block is reassigned inside a child block. Consumed by `wire_phi_outs`
    // during if/match compilation to join each branch's candidate value.
    block_rebinds: HashMap<BlockId, HashMap<String, TermId>>,

    // Loop-carry slot stack: one entry per currently-open loop body. Each
    // entry maps a carry name to a shared register in that loop body block.
    // When the inner rebinds (plain assigns or phis from nested conditionals)
    // land in the body block, their registers are rewritten to the slot, so
    // every rebind writes to the same register. This makes `break` mid-body
    // leave the slot with whatever the most recent rebind stored — the
    // loop's `phi_out` always reads the up-to-date value, even when the
    // compile-time "latest" rebind term never ran in that iteration.
    carry_slots: Vec<(BlockId, HashMap<String, RegisterIndex>)>,
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
            function_boundaries: Vec::new(),
            capture_stack: Vec::new(),
            function_body_blocks: Vec::new(),
            loop_depth: 0,
            overloaded_fns: HashMap::new(),
            overload_variants: HashMap::new(),
            block_rebinds: HashMap::new(),
            carry_slots: Vec::new(),
        }
    }

    /// Compile a list of statements into a Program.
    pub fn compile(
        mut self,
        stmts: &[Stmt],
        source: String,
        program_id: ProgramId,
        native_fns: &NativeFnTable,
    ) -> Program {
        // Create root block
        let root_block = self.new_block(None);
        self.current_block = root_block;

        // Push global scope
        self.push_scope(false);

        // Register native functions (including builtins) as phantom terms.
        for i in 0..native_fns.count() {
            let name = native_fns.get_name(crate::native_fn::NativeFnId(i as u32)).to_string();
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

        // Build block→terms index
        let mut block_terms: std::collections::HashMap<BlockId, Vec<TermId>> =
            std::collections::HashMap::new();
        for term in &self.terms {
            block_terms
                .entry(term.block_id)
                .or_default()
                .push(term.id);
        }

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
            block_terms,
        }
    }

    // -----------------------------------------------------------------------
    // Utility
    // -----------------------------------------------------------------------

    /// Compute a stable hash for a state variable name. This ensures state
    /// keys are based on name, not declaration order, so reordering state
    /// declarations doesn't break hot reload.
    fn hash_state_name(name: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        hasher.finish()
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
            phi_outs: Vec::new(),
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

    /// Record a cross-block rebinding of `name` to `new_tid` (a term in the
    /// current block). Updates the current scope and the per-block rebind
    /// log so the enclosing conditional can emit a phi term.
    fn rebind_name_in_current_block(&mut self, name: String, new_tid: TermId) {
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

    /// Detect names that will be rebound in one or more child-block bodies
    /// of an enclosing control-flow construct (if/match/for/while). A name
    /// qualifies if it's assigned inside any branch and is already bound in
    /// the current (parent) scope. Returns deduplicated names in insertion
    /// order. Callers filter let-shadowed names per body if needed.
    fn detect_rebinds_stmts(&self, bodies: &[&[Stmt]]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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
    fn detect_rebinds_exprs(&self, bodies: &[&Expr]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
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

    /// Emit a `Phi` term in the current (parent) block for each name to be
    /// joined. Placed *before* the upcoming control-flow term so the phi's
    /// own exec initializes its register from the pre-control-flow value;
    /// child frames that rebind the name will overwrite via `phi_outs` on
    /// pop. Rebinds the parent-scope binding of the name to the phi term.
    /// Returns `(name, phi_tid)` pairs for later wiring via `wire_phi_outs`.
    fn emit_phis(&mut self, names: &[String], span: SourceSpan) -> Vec<(String, TermId)> {
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let outer_tid = match self.scope_lookup(name) {
                Some(t) => t,
                None => continue,
            };
            let phi_tid = self.emit_term(
                TermOp::Phi,
                smallvec![outer_tid],
                Some(name.clone()),
            );
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

    /// Seed body-local read terms at the start of a loop body block for
    /// each phi. Each iteration re-runs these Copy terms to snapshot the
    /// current phi register value; subsequent body rebindings chain off
    /// these as same-block SSA rebinds. Returns `(name, slot_register)`
    /// pairs so the caller can install a carry-slot entry that rewrites
    /// later body-block rebinds of each name to share this register.
    fn emit_body_phi_ins(
        &mut self,
        phis: &[(String, TermId)],
    ) -> HashMap<String, RegisterIndex> {
        let mut slots = HashMap::new();
        for (name, phi_tid) in phis {
            let in_tid = self.emit_term(
                TermOp::Copy,
                smallvec![*phi_tid],
                Some(name.clone()),
            );
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
    fn carry_slot_for_current_block(&self, name: &str) -> Option<RegisterIndex> {
        let (body_block, slots) = self.carry_slots.last()?;
        if self.current_block != *body_block {
            return None;
        }
        slots.get(name).copied()
    }

    /// Wire `phi_outs` for a child block: for each phi, if the body
    /// rebound the name, its popping frame copies the final binding back
    /// to the phi's register. Handles both conditional-branch callers
    /// (scope already popped → read from `block_rebinds`) and loop-body
    /// callers (scope still live → read via `scope_lookup`). Branches
    /// that don't rebind a phi'd name don't get a phi_out, so the phi
    /// keeps its init value.
    fn wire_phi_outs(&mut self, body_block: BlockId, phis: &[(String, TermId)]) {
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

    fn collect_assigned_names_stmts(stmts: &[Stmt], out: &mut Vec<String>) {
        for s in stmts {
            match &s.kind {
                StmtKind::Assign { target: AssignTarget::Name(n), value } => {
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
            ExprKind::If { condition, then_body, else_body } => {
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
                use crate::ast::RecordField;
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
            in_loop: false,
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
            in_loop: false,
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
        // Detect overloaded function names (same name, different arities)
        let mut fn_arities: HashMap<String, std::collections::HashSet<usize>> = HashMap::new();
        for stmt in stmts {
            if let StmtKind::FnDecl { name, params, .. } = &stmt.kind {
                fn_arities.entry(name.clone()).or_default().insert(params.len());
            }
        }
        for (name, arities) in fn_arities {
            if arities.len() > 1 {
                self.overloaded_fns.insert(name, arities.len());
            }
        }

        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FnDecl { name, .. } => {
                    if self.scope_lookup(name).is_none() {
                        let tid = self.emit_phantom_term(name.clone());
                        self.scope_bind(name.clone(), tid);
                    }
                }
                StmtKind::EnumDecl { variants, .. } => {
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
                if let Some(&expected_count) = self.overloaded_fns.get(name) {
                    // Overloaded function: compile with internal name "name#arity"
                    let internal_name = format!("{}#{}", name, params.len());
                    let closure_tid =
                        self.compile_function(Some(internal_name), params, body);
                    self.overload_variants
                        .entry(name.clone())
                        .or_default()
                        .push(closure_tid);

                    // Once all variants are compiled, emit the overload set
                    let compiled_count = self.overload_variants[name].len();
                    if compiled_count == expected_count {
                        let inputs: SmallVec<[TermId; 4]> = self.overload_variants[name]
                            .clone().into_iter().collect();
                        let set_tid = self.emit_term(
                            TermOp::MakeOverloadSet,
                            inputs,
                            Some(name.clone()),
                        );
                        self.scope_bind(name.clone(), set_tid);
                    }
                } else {
                    let closure_tid =
                        self.compile_function(Some(name.clone()), params, body);
                    self.scope_bind(name.clone(), closure_tid);
                }
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
                let iter_tid = self.compile_expr(iter);

                // Detect loop-carry names: outer-bound names assigned in the
                // body. Filter out names that `let`-shadow at the top level of
                // the body — those are fresh locals per iteration, not carries.
                let mut let_bound: Vec<String> = Vec::new();
                Self::collect_let_names(body, &mut let_bound);
                let carries: Vec<String> = self
                    .detect_rebinds_stmts(&[body])
                    .into_iter()
                    .filter(|n| !let_bound.contains(n))
                    .collect();

                let phis = self.emit_phis(&carries, stmt_span);

                let body_block = self.new_block(None);
                self.blocks[body_block.0 as usize].param_names = vec![var.clone()];

                let for_tid = self.emit_term_with_children(
                    TermOp::ForLoop,
                    smallvec![iter_tid],
                    None,
                    smallvec![body_block],
                );
                self.blocks[body_block.0 as usize].parent_term_id = Some(for_tid);

                // Compile body manually so we can capture the final scope
                // binding for each phi'd name before the body scope pops.
                self.loop_depth += 1;
                let saved = self.set_block(body_block);
                self.push_scope(false);

                // Loop variable phantom — evaluator populates register 0.
                let var_tid = self.emit_phantom_term(var.clone());
                self.scope_bind(var.clone(), var_tid);

                let slots = self.emit_body_phi_ins(&phis);
                self.carry_slots.push((body_block, slots));

                for s in body {
                    self.compile_stmt(s);
                }

                self.wire_phi_outs(body_block, &phis);
                self.carry_slots.pop();

                self.finalize_block(body_block);
                self.pop_scope();
                self.set_block(saved);
                self.loop_depth -= 1;
            }

            StmtKind::While { condition, body } => {
                // Carry names: outer-bound names assigned in the body, plus
                // any outer-bound names assigned inside the condition expr.
                let mut let_bound: Vec<String> = Vec::new();
                Self::collect_let_names(body, &mut let_bound);
                let mut carries: Vec<String> = self
                    .detect_rebinds_stmts(&[body])
                    .into_iter()
                    .filter(|n| !let_bound.contains(n))
                    .collect();
                for n in self.detect_rebinds_exprs(&[condition]) {
                    if !carries.contains(&n) {
                        carries.push(n);
                    }
                }

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

                self.loop_depth += 1;
                let saved = self.set_block(body_block);
                self.push_scope(false);

                let slots = self.emit_body_phi_ins(&phis);
                self.carry_slots.push((body_block, slots));

                for s in body {
                    self.compile_stmt(s);
                }

                self.wire_phi_outs(body_block, &phis);
                self.carry_slots.pop();

                self.finalize_block(body_block);
                self.pop_scope();
                self.set_block(saved);
                self.loop_depth -= 1;
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
                let init_tid = self.compile_expr(init);
                let state_key = StateKey(Self::hash_state_name(name));
                let mut inputs: SmallVec<[TermId; 4]> = smallvec![init_tid];
                if let Some(key_expr) = key {
                    let key_tid = self.compile_expr(key_expr);
                    inputs.push(key_tid);
                }
                let state_tid = self.emit_term(
                    TermOp::StateInit,
                    inputs,
                    Some(name.clone()),
                );
                self.terms[state_tid.0 as usize].state_key = Some(state_key);
                self.terms[state_tid.0 as usize].in_loop = self.loop_depth > 0;
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
                        let in_loop = self.terms[existing_tid.0 as usize].in_loop;
                        // If StateInit has an explicit key (2nd input), pass it to StateWrite too
                        let has_explicit_key = self.terms[existing_tid.0 as usize].inputs.len() > 1;
                        let mut write_inputs: SmallVec<[TermId; 4]> = smallvec![val_tid];
                        if has_explicit_key {
                            let key_input = self.terms[existing_tid.0 as usize].inputs[1];
                            write_inputs.push(key_input);
                        }
                        let write_tid = self.emit_term(
                            TermOp::StateWrite,
                            write_inputs,
                            None,
                        );
                        self.terms[write_tid.0 as usize].state_key = state_key;
                        self.terms[write_tid.0 as usize].in_loop = in_loop;
                    }
                }

                // Always emit a fresh Copy term + rebind. If the name was
                // bound in an outer block, record the rebind so the enclosing
                // conditional / loop can emit a phi join.
                let assign_tid = self.emit_term(
                    TermOp::Copy,
                    smallvec![val_tid],
                    Some(name.clone()),
                );
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
                    if existing_block == self.current_block {
                        self.scope_bind(name.clone(), assign_tid);
                    } else {
                        self.rebind_name_in_current_block(name.clone(), assign_tid);
                    }
                } else {
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

            ExprKind::Ident(name) => {
                if let Some(tid) = self.scope_lookup(name) {
                    // Check if this reference crosses a function boundary (needs capture)
                    if self.needs_capture(name) {
                        let local_tid = self.get_or_add_capture(name, tid);
                        self.emit_term(TermOp::Copy, smallvec![local_tid], None)
                    } else {
                        self.emit_term(TermOp::Copy, smallvec![tid], None)
                    }
                } else {
                    let hint = match name.as_str() {
                        "var" | "const" => Some("use 'let' to declare variables in Petal"),
                        "def" | "func" | "function" => Some("use 'fn' to define functions in Petal"),
                        "elif" | "elseif" | "elsif" => Some("use 'else if' in Petal"),
                        "switch" | "case" => Some("use 'match' for pattern matching in Petal"),
                        "lambda" => Some("use 'fn' for anonymous functions, e.g. fn(x) { x + 1 }"),
                        "null" | "undefined" | "None" => Some("use 'nil' for null/empty values in Petal"),
                        "console" => Some("use 'print()' for output in Petal"),
                        "typeof" => Some("use 'type()' to get the type of a value in Petal"),
                        "Math" => Some("math functions are top-level in Petal: abs(), sqrt(), floor(), ceil(), round()"),
                        "require" | "import" => Some("Petal does not have a module system yet"),
                        _ => None,
                    };
                    let msg = if let Some(hint) = hint {
                        format!("Undefined variable: {} — {}", name, hint)
                    } else {
                        format!("Undefined variable: {}", name)
                    };
                    let msg_cid = self.constants.intern(ConstantValue::String(msg));
                    self.emit_term(TermOp::Error(msg_cid), smallvec![], None)
                }
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

            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let cond_tid = self.compile_expr(condition);

                // Pre-scan both branches to decide which names need a phi.
                // Emit phi terms in the parent block *before* the Branch so
                // each phi's exec initializes its register to the pre-if
                // value; popping branches then overwrite via phi_outs.
                let else_stmts_for_scan: Vec<&[Stmt]> = match else_body {
                    Some(ElseBranch::Block(stmts)) => vec![stmts.as_slice()],
                    _ => Vec::new(),
                };
                let else_expr_for_scan: Vec<&Expr> = match else_body {
                    Some(ElseBranch::ElseIf(e)) => vec![e.as_ref()],
                    _ => Vec::new(),
                };
                let mut bodies: Vec<&[Stmt]> = vec![then_body];
                bodies.extend(else_stmts_for_scan);
                let mut names = self.detect_rebinds_stmts(&bodies);
                for n in self.detect_rebinds_exprs(&else_expr_for_scan) {
                    if !names.contains(&n) {
                        names.push(n);
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

                // Wire phi_outs from each branch's rebinds.
                self.wire_phi_outs(then_block, &phis);
                self.wire_phi_outs(else_block, &phis);

                branch_tid
            }

            ExprKind::Match { subject, arms } => {
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

                // Wire phi_outs for each arm body's rebinds.
                let arm_bodies: Vec<BlockId> =
                    arm_metas.iter().map(|m| m.body_block).collect();
                self.match_arms.insert(match_tid, arm_metas);
                for body_block in &arm_bodies {
                    self.wire_phi_outs(*body_block, &phis);
                }
                match_tid
            }

            ExprKind::List(elements) => {
                let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                for elem in elements {
                    inputs.push(self.compile_expr(elem));
                }
                self.emit_term(TermOp::AllocList, inputs, None)
            }

            ExprKind::Record(fields) => {
                use crate::ast::RecordField;
                let has_spread = fields.iter().any(|f| matches!(f, RecordField::Spread(_)));
                if !has_spread {
                    // Simple case: no spread, use AllocMap
                    let mut field_names = Vec::new();
                    let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
                    for field in fields {
                        if let RecordField::Named(key, value) = field {
                            field_names.push(
                                self.constants
                                    .intern(ConstantValue::String(key.clone())),
                            );
                            inputs.push(self.compile_expr(value));
                        }
                    }
                    self.emit_term(
                        TermOp::AllocMap {
                            fields: field_names,
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
                                let cid = self.constants
                                    .intern(ConstantValue::String(key.clone()));
                                let idx = inputs.len();
                                inputs.push(self.compile_expr(value));
                                entries.push(MapSpreadEntry::Named(cid, idx));
                            }
                        }
                    }
                    self.emit_term(
                        TermOp::AllocMapSpread { entries },
                        inputs,
                        None,
                    )
                }
            }

            ExprKind::FieldAccess { object, field } => {
                let obj_tid = self.compile_expr(object);
                let field_const = self
                    .constants
                    .intern(ConstantValue::String(field.clone()));
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
                let mut last_tid = self.emit_term(
                    TermOp::Constant(nil_cid),
                    smallvec![],
                    None,
                );
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

            ExprKind::Lambda { params, body } => {
                self.compile_function(None, params, body)
            }

            ExprKind::Element { tag, props, children } => {
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

            ExprKind::StringInterp { parts, exprs } => {
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

    fn compile_function(
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

    // -----------------------------------------------------------------------
    // Enum constructor
    // -----------------------------------------------------------------------

    fn compile_enum_constructor(&mut self, variant: &EnumVariant) -> TermId {
        let (body_block, saved_block) = self.begin_function_scope(&variant.fields);

        // Collect phantom term IDs for the fields (already created by begin_function_scope)
        let field_tids: SmallVec<[TermId; 4]> = variant.fields.iter()
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
