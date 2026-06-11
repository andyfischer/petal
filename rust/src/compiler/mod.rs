//! Compiler - Transforms AST into term-graph IR.
//!
//! Single recursive pass over the AST, emitting terms and blocks.
//!
//! The compiler is split by concern:
//! - `mod.rs`     — compiler state, blocks, scopes, term emission, prescan
//! - `stmt`       — statement compilation (let/assign/loops/state/decls)
//! - `expr`       — expression compilation (incl. match patterns)
//! - `function`   — function bodies, closures, capture tracking
//! - `phi`        — cross-block rebind detection, phi joins, loop carries

mod expr;
mod function;
mod phi;
mod stmt;

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

    // Map from a state variable's StateKey back to its `StateInit` term. Used
    // by `compile_assign` to emit a `StateWrite` even after the state name has
    // been rebound (which replaces its scope binding with a `Copy` term, so a
    // simple scope_lookup chain can no longer reach the StateInit).
    state_inits: HashMap<StateKey, TermId>,

    // Builtin name → the phantom Copy TermId created for that builtin during
    // `compile()`. Used at call sites to detect a bare, unshadowed builtin call
    // and compile it to a static `BuiltinCall` instead of a dynamic `Call`.
    builtin_phantoms: HashMap<String, TermId>,
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
            state_inits: HashMap::new(),
            builtin_phantoms: HashMap::new(),
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
            let name = native_fns
                .get_name(crate::native_fn::NativeFnId(i as u32))
                .to_string();
            let tid = self.emit_phantom_term(name.clone());
            self.builtin_phantoms.insert(name.clone(), tid);
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
        let mut block_terms: HashMap<BlockId, Vec<TermId>> = HashMap::new();
        for term in &self.terms {
            block_terms.entry(term.block_id).or_default().push(term.id);
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

    /// Compute a stable hash for a state variable name. This ensures state
    /// keys are based on name, not declaration order, so reordering state
    /// declarations doesn't break hot reload.
    pub fn hash_state_name(name: &str) -> u64 {
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

    fn scope_lookup(&self, name: &str) -> Option<TermId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&tid) = scope.get(name) {
                return Some(tid);
            }
        }
        None
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
                fn_arities
                    .entry(name.clone())
                    .or_default()
                    .insert(params.len());
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
}
