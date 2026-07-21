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

use smallvec::{SmallVec, smallvec};

use crate::ast::*;
use crate::constant_table::{ConstantTable, ConstantValue};
use crate::module::LoadedModule;
use crate::native_fn::NativeFnTable;
use crate::program::*;
use crate::source_map::{ENTRY_FILE, SourceFile, SourceMap, SourceSpan};
use crate::types::FnSignature;

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

    // Declared type signatures of named functions, keyed by (name, arity) so
    // arity overloads keep distinct entries. Populated by `prescan_declarations`
    // and consulted by the type checker at call sites. Compile-time only.
    fn_signatures: HashMap<(String, usize), FnSignature>,

    // Non-fatal type-checker diagnostics, accumulated during compilation and
    // surfaced alongside the compiled program (a later chunk consumes them).
    warnings: Vec<crate::diagnostic::Diagnostic>,

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

    // ── Module system state (see docs/module-system.md) ──────────────
    //
    // The module the statements currently being compiled belong to; `None`
    // for the entry file. Drives state-key qualification and the qualified
    // display names on module-level closures.
    current_module: Option<String>,
    // Module alias → module name, for the file currently being compiled
    // (`import ui` binds "ui"→"ui", `import ui as u` binds "u"→"ui").
    // Cleared at every module boundary — aliases are file-scoped. A module
    // alias is a compile-time binding kind, not a runtime value; `ui.button`
    // resolves through it statically (see `try_module_member`).
    pub(super) module_aliases: HashMap<String, String>,
    // Export names of every module compiled so far. The exported terms
    // themselves are bound in the global scope under qualified names
    // ("ui::button"), so references to them ride the ordinary scope-lookup /
    // closure-capture machinery.
    module_exports: HashMap<String, Vec<String>>,
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
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
            fn_signatures: HashMap::new(),
            warnings: Vec::new(),
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
            current_module: None,
            module_aliases: HashMap::new(),
            module_exports: HashMap::new(),
        }
    }

    /// Compile a list of statements into a Program (single-file form: no
    /// imports). Kept as the simple entry point for tools that already hold
    /// parsed statements; the module-aware pipeline is [`compile_modules`].
    pub fn compile(
        self,
        stmts: &[Stmt],
        source: String,
        program_id: ProgramId,
        native_fns: &NativeFnTable,
    ) -> Program {
        let entry = LoadedModule {
            name: None,
            display_name: "<entry>".to_string(),
            source,
            origin: None,
            stmts: stmts.to_vec(),
            imports: Vec::new(),
            file_id: ENTRY_FILE,
        };
        self.compile_modules(&[entry], program_id, native_fns)
            .expect("import-free compilation cannot fail")
    }

    /// Compile a dependency-ordered module list (imports first, entry file
    /// last — the shape [`crate::module::load_modules`] produces) into one
    /// merged Program.
    ///
    /// Each module compiles inside its own scope frame: its top-level
    /// bindings become its exports, qualified-bound in the global scope as
    /// `"module::name"` so importer references ride the ordinary scope-lookup
    /// and closure-capture machinery. Top-level statements of every module are
    /// emitted into the single root block, in dependency order, ahead of the
    /// entry file's — an imported module's body executes exactly once, before
    /// its importers. Errors are import-binding problems (unknown export,
    /// selective-import collisions, private names).
    pub fn compile_modules(
        mut self,
        modules: &[LoadedModule],
        program_id: ProgramId,
        native_fns: &NativeFnTable,
    ) -> Result<Program, String> {
        // Create root block
        let root_block = self.new_block(None);
        self.current_block = root_block;

        // Push global scope
        self.push_scope(false);

        // Register native functions (including builtins) as phantom terms.
        // Natives are global: visible in every module scope.
        for i in 0..native_fns.count() {
            let name = native_fns
                .get_name(crate::native_fn::NativeFnId(i as u32))
                .to_string();
            let tid = self.emit_phantom_term(name.clone());
            self.builtin_phantoms.insert(name.clone(), tid);
            self.scope_bind(name, tid);
        }

        for module in modules {
            self.compile_module(module)?;
        }

        // Finalize root block
        self.finalize_block(root_block);

        self.pop_scope();

        // Build block→terms index
        let mut block_terms: HashMap<BlockId, Vec<TermId>> = HashMap::new();
        for term in &self.terms {
            block_terms.entry(term.block_id).or_default().push(term.id);
        }

        let entry = modules
            .iter()
            .find(|m| m.name.is_none())
            .expect("module list contains the entry file");

        // File table for multi-file programs, indexed by FileId (entry = 0,
        // modules at their load-order ids). Single-file programs keep an
        // empty table so their IR serialization stays in the v0 shape.
        if modules.len() > 1 {
            let mut files = vec![
                SourceFile {
                    name: String::new(),
                    source: String::new(),
                    origin: None
                };
                modules.len()
            ];
            for m in modules {
                files[m.file_id.0 as usize] = SourceFile {
                    name: m.display_name.clone(),
                    source: m.source.clone(),
                    origin: m.origin.clone(),
                };
            }
            self.source_map.files = files;
        }

        Ok(Program {
            id: program_id,
            source: entry.source.clone(),
            terms: self.terms,
            blocks: self.blocks,
            root_block,
            constants: self.constants,
            source_map: self.source_map,
            has_errors: false,
            functions: self.functions,
            match_arms: self.match_arms,
            block_terms,
            warnings: self.warnings,
        })
    }

    /// Compile one module's statements into the root block. For the entry
    /// file (`module.name == None`) bindings land in the global scope frame,
    /// exactly as single-file compilation always has; for an imported module
    /// they land in a dedicated scope frame that is popped afterwards, its
    /// surviving bindings becoming the module's exports.
    fn compile_module(&mut self, module: &LoadedModule) -> Result<(), String> {
        // Rewrite `@`-arguments (`f(@x)` → `x = f(x)`) before anything else, so
        // prescan and compilation only ever see the desugared form.
        let mut stmts = module.stmts.to_vec();
        crate::desugar::desugar(&mut stmts);

        let is_entry = module.name.is_none();
        self.current_module = module.name.clone();
        // Aliases are file-scoped; overload grouping is per-compile and must
        // not leak across module boundaries (prescan counts a module's own
        // declarations only).
        self.module_aliases.clear();
        self.overloaded_fns.clear();
        self.overload_variants.clear();

        if !is_entry {
            self.push_scope(false); // module scope frame
        }

        self.bind_imports(module, &stmts)?;
        Self::check_overload_export_consistency(&stmts)
            .map_err(|e| format!("{}: {}", module.display_name, e))?;
        self.prescan_declarations(&stmts);
        let diags = crate::typecheck::check_module(&stmts, &self.fn_signatures);
        self.warnings.extend(diags);
        for stmt in &stmts {
            self.compile_stmt(stmt);
        }

        if !is_entry {
            let scope = self.scopes.pop().expect("module scope frame");
            self.capture_exports(module, scope);
        }
        self.current_module = None;
        Ok(())
    }

    /// Materialize a module's resolved imports into the current scope:
    /// aliases become compile-time alias bindings, selective names become
    /// direct term bindings (loud on collision), implicit imports bind every
    /// export bare but weakly (the file's own bindings win, like builtins).
    fn bind_imports(&mut self, module: &LoadedModule, stmts: &[Stmt]) -> Result<(), String> {
        let declared = Self::declared_top_level_names(stmts);
        // Selectively-imported name → module it came from, for collision
        // provenance within this one file.
        let mut selective: HashMap<String, String> = HashMap::new();

        for import in &module.imports {
            let m = &import.decl.module;
            let Some(exports) = self.module_exports.get(m).cloned() else {
                // load_modules compiles dependencies first; a miss is a bug.
                return Err(format!(
                    "internal error: module '{}' was not compiled before its importer",
                    m
                ));
            };

            if import.implicit {
                // Bind every export bare, silently — the file's own imports
                // and declarations land on top of these.
                for name in &exports {
                    let tid = self
                        .scope_lookup(&format!("{m}::{name}"))
                        .expect("export is bound under its qualified name");
                    self.scope_bind(name.clone(), tid);
                }
                self.module_aliases.insert(m.clone(), m.clone());
                continue;
            }

            // Alias binding (`import ui` / `import ui as u`).
            let alias = import.decl.alias.clone().unwrap_or_else(|| m.clone());
            if let Some(existing) = self.module_aliases.get(&alias)
                && existing != m
            {
                return Err(format!(
                    "{}: '{}' is already an alias for module '{}' and cannot also \
                     alias '{}'",
                    module.display_name, alias, existing, m
                ));
            }
            self.module_aliases.insert(alias, m.clone());

            // Selective bindings (`import ui: button, clicked`).
            let Some(names) = &import.decl.names else {
                continue;
            };
            for name in names {
                if !exports.contains(name) {
                    return Err(format!(
                        "{}: module '{}' has no export '{}' (exports: {})",
                        module.display_name,
                        m,
                        name,
                        if exports.is_empty() {
                            "none".to_string()
                        } else {
                            exports.join(", ")
                        }
                    ));
                }
                if let Some(other) = selective.get(name) {
                    return Err(format!(
                        "{}: '{}' is imported from both '{}' and '{}'",
                        module.display_name, name, other, m
                    ));
                }
                if declared.contains(name) {
                    return Err(format!(
                        "{}: '{}' is imported from '{}' but is also declared in this \
                         file",
                        module.display_name, name, m
                    ));
                }
                let tid = self
                    .scope_lookup(&format!("{m}::{name}"))
                    .expect("export is bound under its qualified name");
                self.scope_bind(name.clone(), tid);
                selective.insert(name.clone(), m.clone());
            }
        }
        Ok(())
    }

    /// Record a finished module's exports: every top-level binding declared
    /// with the `export` modifier that the module didn't itself import (imports
    /// are not re-exported). Each export is also bound in the global scope under
    /// its qualified name (`"ui::button"`), which is how alias access, later
    /// importers, and `Env::call_function` reach it. A module with no `export`
    /// declarations exports nothing — the default is private.
    fn capture_exports(&mut self, module: &LoadedModule, scope: HashMap<String, TermId>) {
        let module_name = module.name.as_deref().expect("not the entry file");
        let imported: std::collections::HashSet<&str> = module
            .imports
            .iter()
            .flat_map(|i| i.decl.names.iter().flatten())
            .map(String::as_str)
            .collect();

        let exported = Self::exported_top_level_names(&module.stmts);

        let mut names: Vec<String> = scope
            .keys()
            .filter(|n| exported.contains(n.as_str()) && !imported.contains(n.as_str()))
            .cloned()
            .collect();
        names.sort_unstable(); // deterministic export order for messages

        for name in &names {
            let tid = scope[name];
            let qualified = format!("{module_name}::{name}");
            if let Some(global) = self.scopes.first_mut() {
                global.insert(qualified, tid);
            }
        }
        self.module_exports.insert(module_name.to_string(), names);
    }

    /// Top-level names a module declares (fn, enum variants, let, state) —
    /// the set a selective import may collide with.
    fn declared_top_level_names(stmts: &[Stmt]) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FnDecl { name, .. }
                | StmtKind::Let { name, .. }
                | StmtKind::State { name, .. } => {
                    names.insert(name.clone());
                }
                StmtKind::EnumDecl { variants, .. } => {
                    for v in variants {
                        names.insert(v.name.clone());
                    }
                }
                _ => {}
            }
        }
        names
    }

    /// Top-level names a module explicitly `export`s (fn, enum variants, let,
    /// state) — the set that importers may see. Everything else is private.
    /// `export` is the single privacy rule: a name is exported iff its
    /// declaration is marked `export`, regardless of a leading underscore
    /// (`export fn _helper` exports normally).
    fn exported_top_level_names(stmts: &[Stmt]) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        for stmt in stmts {
            if !stmt.exported {
                continue;
            }
            match &stmt.kind {
                StmtKind::FnDecl { name, .. }
                | StmtKind::Let { name, .. }
                | StmtKind::State { name, .. } => {
                    names.insert(name.clone());
                }
                StmtKind::EnumDecl { variants, .. } => {
                    for v in variants {
                        names.insert(v.name.clone());
                    }
                }
                _ => {}
            }
        }
        names
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

    /// The state key for `name` declared in the current compilation context:
    /// module state keys are qualified (`"ui::scroll"`) so two modules'
    /// same-named `state` decls get distinct slots; the entry file keeps
    /// bare-name hashing so existing programs' hot-reload state survives.
    /// Consequence (documented in docs/module-system.md): moving a `state`
    /// decl between files, or renaming a module, changes its key and drops
    /// that state on reload — same class of event as renaming the variable.
    pub(super) fn state_key_for(&self, name: &str) -> StateKey {
        match &self.current_module {
            Some(m) => StateKey(Self::hash_state_name(&format!("{m}::{name}"))),
            None => StateKey(Self::hash_state_name(name)),
        }
    }

    /// Display name for a term declared at module scope: qualified for module
    /// code (`"ui::button"`) so host-facing surfaces (`Env::call_function`,
    /// state JSON, `--term` lookup) can address it unambiguously.
    pub(super) fn qualified_name(&self, name: &str) -> String {
        match &self.current_module {
            Some(m) => format!("{m}::{name}"),
            None => name.to_string(),
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
        if let Some(&boundary) = self.function_boundaries.last()
            && boundary >= self.scopes.len()
        {
            self.function_boundaries.pop();
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
            collect: false,
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
            collect: false,
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

    /// Reject overload groups (same top-level name, 2+ `fn` declarations) whose
    /// members carry inconsistent `export` markers. Export visibility is tracked
    /// per *name*, and all arities of an overloaded fn share one name binding, so
    /// marking a single arity `export` would silently export the whole set (and
    /// leak the unmarked arities). Rather than pick a winner, require the author
    /// to be explicit: mark every overload `export`, or none.
    fn check_overload_export_consistency(stmts: &[Stmt]) -> Result<(), String> {
        // name -> (any exported, any not exported), in first-seen order.
        let mut groups: Vec<String> = Vec::new();
        let mut seen: HashMap<String, (bool, bool)> = HashMap::new();
        for stmt in stmts {
            if let StmtKind::FnDecl { name, .. } = &stmt.kind {
                let entry = seen.entry(name.clone()).or_insert_with(|| {
                    groups.push(name.clone());
                    (false, false)
                });
                if stmt.exported {
                    entry.0 = true;
                } else {
                    entry.1 = true;
                }
            }
        }
        for name in groups {
            let (any_exported, any_plain) = seen[&name];
            if any_exported && any_plain {
                return Err(format!(
                    "overloaded function '{name}' has mixed export markers: \
                     mark all overloads 'export' or none"
                ));
            }
        }
        Ok(())
    }

    fn prescan_declarations(&mut self, stmts: &[Stmt]) {
        // Record declared signatures so the checker can verify call sites even
        // across forward references. Accumulates across modules.
        self.fn_signatures.extend(collect_fn_signatures(stmts));

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

/// Collect declared function signatures from a statement list, keyed by
/// `(name, arity)`. Only the *resolved* types are kept — an un-annotated or
/// unrecognized-name parameter/return becomes `None` (checked as `any`). Later
/// declarations of the same `(name, arity)` win. Pure so it is unit-testable
/// without a live [`Compiler`]; `prescan_declarations` folds the result into
/// [`Compiler::fn_signatures`].
pub(crate) fn collect_fn_signatures(stmts: &[Stmt]) -> HashMap<(String, usize), FnSignature> {
    let mut sigs = HashMap::new();
    for stmt in stmts {
        if let StmtKind::FnDecl {
            name, params, ret, ..
        } = &stmt.kind
        {
            let sig = FnSignature {
                params: params
                    .iter()
                    .map(|p| p.ty.as_ref().and_then(|t| t.resolved))
                    .collect(),
                ret: ret.as_ref().and_then(|t| t.resolved),
            };
            sigs.insert((name.clone(), params.len()), sig);
        }
    }
    sigs
}

#[cfg(test)]
mod prescan_tests {
    use super::collect_fn_signatures;
    use crate::rewrite::parse_ast;
    use crate::types::{FnSignature, Type};

    fn sigs(src: &str) -> std::collections::HashMap<(String, usize), FnSignature> {
        let (_, stmts) = parse_ast(src).expect("parse");
        collect_fn_signatures(&stmts)
    }

    #[test]
    fn collects_param_and_return_types() {
        let table = sigs("fn area(r: float) -> float\n  r * r\nend");
        assert_eq!(
            table.get(&("area".to_string(), 1)),
            Some(&FnSignature {
                params: vec![Some(Type::Float)],
                ret: Some(Type::Float),
            })
        );
    }

    #[test]
    fn un_annotated_and_unknown_slots_are_none() {
        // `b` un-annotated, `banana` unrecognized, no return annotation.
        let table = sigs("fn f(a: int, b, c: banana)\n  a\nend");
        assert_eq!(
            table.get(&("f".to_string(), 3)),
            Some(&FnSignature {
                params: vec![Some(Type::Int), None, None],
                ret: None,
            })
        );
    }

    #[test]
    fn arity_overloads_get_distinct_entries() {
        let table = sigs(
            "fn g(x: int) -> int\n  x\nend\nfn g(x: int, y: int) -> int\n  x + y\nend",
        );
        assert_eq!(table.len(), 2);
        assert_eq!(table[&("g".to_string(), 1)].params, vec![Some(Type::Int)]);
        assert_eq!(
            table[&("g".to_string(), 2)].params,
            vec![Some(Type::Int), Some(Type::Int)]
        );
    }

    #[test]
    fn no_functions_yields_empty_table() {
        assert!(sigs("let x: int = 5\nprint(x)").is_empty());
    }
}
