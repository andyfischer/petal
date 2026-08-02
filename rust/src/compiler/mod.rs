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

use std::collections::{HashMap, HashSet};

use smallvec::{SmallVec, smallvec};

use crate::ast::*;
use crate::classes::DECLARE_METHOD_BUILTIN;
use crate::constant_table::{ConstantId, ConstantTable, ConstantValue};
use crate::error::{LoadError, Phase};
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
    /// Span of the assignment statement being compiled, so the `CellWrite` a
    /// `set` emits carries a source location. Provenance names the write site
    /// at every cell boundary (§6e), and a spanless write renders as
    /// "writer unknown" — the exact silence the boundary exists to break.
    assign_span: Option<SourceSpan>,
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

    // Classes visible to this compilation: the built-ins, plus every `class`
    // declaration found by `prescan_declarations` (so a `fn f(p: Point)` above
    // `class Point` still resolves). Also the checker's source of truth for
    // class-typed annotations and method lookup. Compile-time only.
    classes: crate::classes::ClassTable,
    // Method-call sites the checker pinned to a single class, keyed by the call
    // expression's span. Replaced per module (spans are file-local). Read by
    // `compile_expr` to bind `r.m()` straight to `fn Class.m` — see
    // `crate::typecheck::check_module`.
    method_dispatch: crate::typecheck::MethodDispatch,
    // Qualified names (`"Rect.inset"`) of the `fn Class.method` declarations
    // compiled so far. Nothing hoists, so a call may only bind statically to a
    // declaration that already ran; see `compile_expr`'s method-call arm.
    declared_methods: HashSet<String>,

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

    // Names declared `var` (mutable cells), one set per entry in `scopes` and
    // pushed/popped in lockstep with it. Kept beside `scopes` rather than in
    // it so that binding kind is looked up exactly like a binding is, and so
    // shadowing works by construction: a `let x` inside a `var x` reports
    // non-var, and vice versa. Rebinds propagate the flag explicitly via
    // `scope_rebind` — a missed propagation loses var-ness and produces a loud
    // "not a var" error rather than silently accepting `=` on a cell.
    var_scopes: Vec<HashSet<String>>,

    // Imported `var`s visible in the file being compiled, bare name → (owning
    // module, the term that holds the cell). Populated by `bind_imports` for
    // the two forms that bind a bare name (selective and implicit imports);
    // alias access (`m.x`) resolves through the qualified name instead and is
    // not recorded. Reads of these names dereference like any other `var`;
    // the map exists so a `set` on one can say which module owns it, since
    // only the declaring module may write a cell it exports.
    imported_vars: HashMap<String, (String, TermId)>,

    // Fatal compile errors, collected with their spans and drained into the
    // `Err` of `compile_modules`. Distinct from `warnings`: these abort. The
    // compiler's statement/expression walk returns `()`/`TermId` rather than
    // `Result`, so errors accumulate here instead of threading `?` through it.
    // Each is paired with the module it was raised in (`None` for the entry
    // file). A span's line/column are file-local, so an error carried out of a
    // module unattributed renders a caret under the *entry* file's line of
    // that number — a different file than the message names.
    errors: Vec<(crate::diagnostic::Diagnostic, Option<String>)>,

    // Display name of the module whose statements are being compiled, which is
    // what the pairs above are tagged with. `None` while the entry file is.
    error_file: Option<String>,

    // Terms that stand in, inside some function, for a binding owned by an
    // enclosing function: the phis and seed copies that control-flow
    // compilation installs for a name assigned across a function boundary.
    // They keep `is_outer_function_binding` honest after the name has been
    // rebound locally — without them only the first assignment in a branch
    // would be diagnosed, since every later lookup finds the phi.
    // See docs/dev/var-next-steps.md (Why the feature exists).
    cross_fn_terms: HashSet<TermId>,

    // Per-block local-shadow log: block → (name → the binding that was live in
    // this block immediately before a `let`/`state` in it redeclared the name,
    // or `None` if there was no such in-block binding). Populated by
    // `note_shadow` when a declaration shadows a name the enclosing control
    // flow is carrying. From that point on the name is block-local: rebinds
    // stop sharing the carry slot and stop updating `block_rebinds`, and
    // `wire_phi_outs` carries the *frozen* pre-shadow value out instead of the
    // shadowed local's final value. See docs/dev/var-next-steps.md (Lexical shadowing).
    block_shadowed: HashMap<BlockId, HashMap<String, Option<TermId>>>,

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
            assign_span: None,
            last_term_in_block: HashMap::new(),
            scopes: Vec::new(),
            enum_variants: HashMap::new(),
            next_register: HashMap::new(),
            fn_signatures: HashMap::new(),
            classes: crate::classes::ClassTable::new(),
            method_dispatch: crate::typecheck::MethodDispatch::new(),
            declared_methods: HashSet::new(),
            warnings: Vec::new(),
            function_boundaries: Vec::new(),
            capture_stack: Vec::new(),
            function_body_blocks: Vec::new(),
            loop_depth: 0,
            overloaded_fns: HashMap::new(),
            overload_variants: HashMap::new(),
            block_rebinds: HashMap::new(),
            carry_slots: Vec::new(),
            block_shadowed: HashMap::new(),
            cross_fn_terms: HashSet::new(),
            var_scopes: Vec::new(),
            imported_vars: HashMap::new(),
            errors: Vec::new(),
            state_inits: HashMap::new(),
            builtin_phantoms: HashMap::new(),
            current_module: None,
            error_file: None,
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
    ) -> Result<Program, LoadError> {
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

        // Fatal errors abort here, after the whole program has been walked so
        // every one of them has been found. `LoadError`'s `Display` renders
        // each one as `msg [line N, column M]`, joined by newlines — the same
        // bytes this site used to format by hand, which the error-position
        // tests pin.
        if !self.errors.is_empty() {
            return Err(LoadError::from_attributed_diagnostics(
                Phase::Compile,
                &self.errors,
            ));
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
            class_names: self
                .classes
                .iter()
                .map(|(_, def)| def.name.clone())
                .collect(),
        })
    }

    /// Compile one module's statements into the root block. For the entry
    /// file (`module.name == None`) bindings land in the global scope frame,
    /// exactly as single-file compilation always has; for an imported module
    /// they land in a dedicated scope frame that is popped afterwards, its
    /// surviving bindings becoming the module's exports.
    fn compile_module(&mut self, module: &LoadedModule) -> Result<(), LoadError> {
        // Rewrite `@`-arguments (`f(@x)` → `x = f(x)`) before anything else, so
        // prescan and compilation only ever see the desugared form.
        let mut stmts = module.stmts.to_vec();
        crate::desugar::desugar(&mut stmts);

        let is_entry = module.name.is_none();
        self.current_module = module.name.clone();
        // Attribute this module's errors to it, so the caret is drawn against
        // its own source rather than the entry file's.
        self.error_file = (!is_entry).then(|| module.display_name.clone());
        // Aliases are file-scoped; overload grouping is per-compile and must
        // not leak across module boundaries (prescan counts a module's own
        // declarations only).
        self.module_aliases.clear();
        self.imported_vars.clear();
        self.overloaded_fns.clear();
        self.overload_variants.clear();

        if !is_entry {
            self.push_scope(false); // module scope frame
        }

        self.bind_imports(module, &stmts)?;
        Self::check_overload_export_consistency(&stmts).map_err(|mut e| {
            // Same bytes as the old `format!("{display_name}: {e}")`.
            for item in &mut e.items {
                item.file = Some(module.display_name.clone());
            }
            e
        })?;
        self.prescan_declarations(module, &stmts);
        let (diags, dispatch) =
            crate::typecheck::check_module(&stmts, &self.fn_signatures, &self.classes);
        self.warnings.extend(diags);
        // Spans are file-local, so this must be *replaced* per module rather
        // than accumulated — two modules' spans collide freely.
        self.method_dispatch = dispatch;
        self.warnings
            .extend(crate::typecheck::unused::check_unused(&stmts));
        for stmt in &stmts {
            self.compile_stmt(stmt);
        }

        if !is_entry {
            // Pop both halves of the frame in lockstep — `binding_is_var`
            // indexes `var_scopes` by `scopes` position, so leaving one behind
            // would hand the next module's frame this module's binding kinds.
            let scope = self.scopes.pop().expect("module scope frame");
            let vars = self.var_scopes.pop().expect("module scope var frame");
            self.capture_exports(module, scope, vars);
        }
        self.current_module = None;
        self.error_file = None;
        Ok(())
    }

    /// Materialize a module's resolved imports into the current scope:
    /// aliases become compile-time alias bindings, selective names become
    /// direct term bindings (loud on collision), implicit imports bind every
    /// export bare but weakly (the file's own bindings win, like builtins).
    fn bind_imports(&mut self, module: &LoadedModule, stmts: &[Stmt]) -> Result<(), LoadError> {
        let declared = Self::declared_top_level_names(stmts);
        // Selectively-imported name → module it came from, for collision
        // provenance within this one file.
        let mut selective: HashMap<String, String> = HashMap::new();

        for import in &module.imports {
            let m = &import.decl.module;
            let Some(exports) = self.module_exports.get(m).cloned() else {
                // load_modules compiles dependencies first; a miss is a bug.
                return Err(LoadError::message(
                    Phase::Compile,
                    format!("internal error: module '{m}' was not compiled before its importer"),
                ));
            };

            if import.implicit {
                // Bind every export bare, silently — the file's own imports
                // and declarations land on top of these.
                for name in &exports {
                    let tid = self
                        .scope_lookup(&format!("{m}::{name}"))
                        .expect("export is bound under its qualified name");
                    self.bind_imported_name(m, name, tid);
                }
                self.module_aliases.insert(m.clone(), m.clone());
                continue;
            }

            // Alias binding (`import ui` / `import ui as u`).
            let alias = import.decl.alias.clone().unwrap_or_else(|| m.clone());
            if let Some(existing) = self.module_aliases.get(&alias)
                && existing != m
            {
                return Err(LoadError::message(
                    Phase::Compile,
                    format!(
                        "{}: '{}' is already an alias for module '{}' and cannot also \
                         alias '{}'",
                        module.display_name, alias, existing, m
                    ),
                ));
            }
            self.module_aliases.insert(alias, m.clone());

            // Selective bindings (`import ui: button, clicked`).
            let Some(names) = &import.decl.names else {
                continue;
            };
            for name in names {
                if !exports.contains(name) {
                    return Err(LoadError::message(
                        Phase::Compile,
                        format!(
                            "{}: module '{}' has no export '{}' (exports: {})",
                            module.display_name,
                            m,
                            name,
                            if exports.is_empty() {
                                "none".to_string()
                            } else {
                                exports.join(", ")
                            }
                        ),
                    ));
                }
                if let Some(other) = selective.get(name) {
                    return Err(LoadError::message(
                        Phase::Compile,
                        format!(
                            "{}: '{}' is imported from both '{}' and '{}'",
                            module.display_name, name, other, m
                        ),
                    ));
                }
                if declared.contains(name) {
                    return Err(LoadError::message(
                        Phase::Compile,
                        format!(
                            "{}: '{}' is imported from '{}' but is also declared in this \
                             file",
                            module.display_name, name, m
                        ),
                    ));
                }
                let tid = self
                    .scope_lookup(&format!("{m}::{name}"))
                    .expect("export is bound under its qualified name");
                self.bind_imported_name(m, name, tid);
                selective.insert(name.clone(), m.clone());
            }
        }
        Ok(())
    }

    /// Bind an import under its bare name, carrying the binding kind over from
    /// the qualified name. An exported `var` has to stay a `var` here: the
    /// bound term holds the *cell*, so a binding that lost its kind would
    /// forward the raw cell id to every read and break the containment
    /// invariant (no expression ever evaluates to a cell — §6d).
    fn bind_imported_name(&mut self, module: &str, name: &str, tid: TermId) {
        if self.binding_is_var(&format!("{module}::{name}")) {
            self.scope_bind_var(name.to_string(), tid);
            self.imported_vars
                .insert(name.to_string(), (module.to_string(), tid));
        } else {
            self.scope_bind(name.to_string(), tid);
        }
    }

    /// Record a finished module's exports: every top-level binding declared
    /// with the `export` modifier that the module didn't itself import (imports
    /// are not re-exported). Each export is also bound in the global scope under
    /// its qualified name (`"ui::button"`), which is how alias access, later
    /// importers, and `Env::call_function` reach it. A module with no `export`
    /// declarations exports nothing — the default is private.
    fn capture_exports(
        &mut self,
        module: &LoadedModule,
        scope: HashMap<String, TermId>,
        vars: HashSet<String>,
    ) {
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
                global.insert(qualified.clone(), tid);
            }
            // An exported `var` stays a `var` under its qualified name, so an
            // importer's read of it dereferences the cell rather than
            // forwarding the raw cell id. Only the owning module can `set` it:
            // `set m.x = 1` is rooted at the module alias, which is not a
            // binding, so there is no cross-module write syntax.
            if vars.contains(name)
                && let Some(global_vars) = self.var_scopes.first_mut()
            {
                global_vars.insert(qualified);
            }
        }
        self.module_exports.insert(module_name.to_string(), names);
    }

    /// Top-level names a module declares (fn, enum variants, let, state, and a
    /// class's constructor) — the set a selective import may collide with.
    fn declared_top_level_names(stmts: &[Stmt]) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FnDecl { name, .. }
                | StmtKind::Let { name, .. }
                | StmtKind::State { name, .. }
                | StmtKind::ClassDecl { name, .. } => {
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
    /// state, class) — the set that importers may see. Everything else is private.
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
                | StmtKind::State { name, .. }
                // A class exports *one* name covering both of its positions:
                // the constructor `Point(…)` and the type `Point`. Importers
                // get this set as their class scope too (see
                // `visible_class_names`), so an unexported class is private in
                // type position exactly as it is in call position.
                | StmtKind::ClassDecl { name, .. } => {
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
        self.var_scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.var_scopes.pop();
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

    /// Bind `name` as a `var` — a mutable cell, written with `set`.
    pub(super) fn scope_bind_var(&mut self, name: String, term_id: TermId) {
        self.scope_bind(name.clone(), term_id);
        if let Some(vars) = self.var_scopes.last_mut() {
            vars.insert(name);
        }
    }

    /// Rebind an existing name, preserving whether it was declared `var`.
    /// Used by the paths that replace a binding without redeclaring it:
    /// assignment `Copy`s, phis, and the loop/arm entry seeds.
    pub(super) fn scope_rebind(&mut self, name: String, term_id: TermId) {
        if self.binding_is_var(&name) {
            self.scope_bind_var(name, term_id);
        } else {
            self.scope_bind(name, term_id);
        }
    }

    /// Resolve `name` to the term that carries its value *inside the function
    /// currently being compiled*, adding closure captures along the way when
    /// the binding lives in an enclosing function. `None` if unbound.
    ///
    /// For a `var` the resolved term holds the cell, not the contents — the
    /// caller decides whether to dereference (a read) or write through it.
    pub(super) fn resolve_local_term(&mut self, name: &str) -> Option<TermId> {
        let tid = self.scope_lookup(name)?;
        if self.needs_capture(name) {
            Some(self.get_or_add_capture(name, tid))
        } else {
            Some(tid)
        }
    }

    /// Was the innermost binding of `name` declared `var`? Answered against
    /// the scope that actually binds the name, so an inner `let` shadowing an
    /// outer `var` (or the reverse) reports its own kind.
    pub(super) fn binding_is_var(&self, name: &str) -> bool {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return self.var_scopes[i].contains(name);
            }
        }
        false
    }

    /// Record a fatal compile error at `span`. Compilation continues so a
    /// single run can report more than one, but `compile_modules` will fail.
    pub(super) fn error_at(&mut self, span: SourceSpan, message: String) {
        let file = self.error_file.clone();
        self.errors
            .push((crate::diagnostic::Diagnostic { span, message }, file));
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
    fn check_overload_export_consistency(stmts: &[Stmt]) -> Result<(), LoadError> {
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
                return Err(LoadError::message(
                    Phase::Compile,
                    format!(
                        "overloaded function '{name}' has mixed export markers: \
                         mark all overloads 'export' or none"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Register this module's `class` declarations and the methods declared on
    /// them, before anything is compiled. Every diagnostic here is *fatal*
    /// (unlike the type checker's warnings): a duplicate field or a method on
    /// a type that does not exist has no reasonable code to generate.
    fn prescan_classes(&mut self, stmts: &[Stmt], module: Option<&str>) {
        let diags = collect_classes(&mut self.classes, stmts, module);
        let file = self.error_file.clone();
        self.errors
            .extend(diags.into_iter().map(|d| (d, file.clone())));
    }

    /// The class names the file being compiled may spell — in an annotation or
    /// in call position, which are the same name. A file sees the classes it
    /// declares plus the ones it imports; a module-private class is invisible
    /// outside its own file, exactly as its constructor is. (Built-in classes
    /// need no import and are added by [`ClassTable::lookup`] itself.)
    ///
    /// An *alias* import (`import shapes`) counts too, even though it binds no
    /// bare name: the constructor is reachable as `shapes.Circle(…)` while a
    /// type annotation has no qualified spelling, so restricting the type to
    /// selective imports would make an exported class unusable in a signature.
    fn visible_class_names(&self, module: &LoadedModule, stmts: &[Stmt]) -> HashSet<String> {
        let mut names: HashSet<String> = stmts
            .iter()
            .filter_map(|s| match &s.kind {
                StmtKind::ClassDecl { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        for import in &module.imports {
            let Some(exports) = self.module_exports.get(&import.decl.module) else {
                continue;
            };
            match &import.decl.names {
                // `import m: A, B` — only the names asked for.
                Some(selected) if !import.implicit => names.extend(selected.iter().cloned()),
                // An alias or implicit import carries the module's whole
                // exported surface.
                _ => names.extend(exports.iter().cloned()),
            }
        }
        names
    }

    fn prescan_declarations(&mut self, module: &LoadedModule, stmts: &[Stmt]) {
        // Scope first: `collect_classes` resolves `fn Circle.diameter(...)`
        // against what this file can see, so a method on another module's
        // private class is an error rather than a silent extension.
        let visible = self.visible_class_names(module, stmts);
        self.classes.set_scope(visible);

        // Classes next: a signature may name one (`fn f(p: Point)`), and a
        // method declaration has to find its class whichever order the file
        // declares them in.
        self.prescan_classes(stmts, Some(module.display_name.as_str()));

        // Record declared signatures so the checker can verify call sites even
        // across forward references. Accumulates across modules.
        self.fn_signatures
            .extend(collect_fn_signatures(stmts, &self.classes));

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
                // A class is *hoisted*: its constructor is emitted here, ahead
                // of every statement in the file, and `compile_stmt` leaves the
                // declaration alone. The type name is already file-wide (the
                // prescan above put it in the table), so the constructor has to
                // be too — otherwise `Point(1, 2)` above `class Point` type-
                // checks clean and then calls nil at runtime. A class body has
                // nothing to evaluate, so there is no order to get wrong.
                StmtKind::ClassDecl { name, fields } => {
                    let ctor = self.compile_class_constructor(name, fields);
                    // The constructor's term is where the class is written.
                    // Hoisting moves *when* it is emitted, never where it came
                    // from — without this every `Point(...)` in a provenance
                    // chain reads `[no location]`, or worse, line 1.
                    self.source_map.add(ctor, stmt.span);
                    self.scope_bind(name.clone(), ctor);
                }
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

/// Fold every `class` declaration in `stmts`, and every method declared on one,
/// into `classes`. `module` is the declaring file's display name, used to name
/// both sides of a cross-module duplicate; `None` for a single-file
/// compilation. Returns the fatal diagnostics found — a nested `class`, a
/// duplicate field, a redeclared class, a name that collides with a built-in
/// type, a duplicate method, or a method on a type that does not exist. Pure
/// over the class table so it is unit-testable without a live [`Compiler`], and
/// so the checker's tests can build the same table the compiler would.
pub fn collect_classes(
    classes: &mut crate::classes::ClassTable,
    stmts: &[Stmt],
    module: Option<&str>,
) -> Vec<crate::diagnostic::Diagnostic> {
    let mut diags = Vec::new();
    let mut err = |span: SourceSpan, message: String| {
        diags.push(crate::diagnostic::Diagnostic { span, message });
    };

    // A `class` is a top-level declaration, so a nested one is rejected before
    // anything else — it is never registered, and never becomes a second,
    // invisible meaning for a name the table already holds.
    for (span, name) in nested_class_decls(stmts) {
        err(
            span,
            format!(
                "class `{name}` must be declared at the top level of a file, not inside \
                 a function or block"
            ),
        );
    }

    for stmt in stmts {
        let StmtKind::ClassDecl { name, fields } = &stmt.kind else {
            continue;
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut defs = Vec::new();
        for f in fields {
            if !seen.insert(f.name.as_str()) {
                // The *second* `x`, not the class — a field-level mistake is
                // reported at the field (see `ClassFieldDecl::span`).
                err(
                    if f.span.start.line > 0 {
                        f.span
                    } else {
                        stmt.span
                    },
                    format!("duplicate field `{}` in class `{}`", f.name, name),
                );
                continue;
            }
            defs.push(crate::classes::ClassField {
                name: f.name.clone(),
                ty: f.ty.as_ref().and_then(|t| t.resolved),
            });
        }
        if let Err(msg) = classes.declare(
            crate::classes::ClassDef {
                name: name.clone(),
                fields: defs,
                methods: Vec::new(),
                builtin: false,
            },
            module,
        ) {
            err(stmt.span, msg);
        }
    }

    // Methods second: a class declared anywhere in the file is now known, so
    // `fn Point.shifted(...)` above `class Point` is fine.
    for stmt in stmts {
        let StmtKind::FnDecl {
            class: Some(class),
            params,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        let Some(id) = classes.lookup(class) else {
            err(
                stmt.span,
                format!(
                    "cannot declare a method on `{class}`: no class of that name \
                     (declare it with `class {class} … end`)"
                ),
            );
            continue;
        };
        let method = method_base_name(&stmt.kind);
        // The receiver is the first parameter and the call site supplies it, so
        // a method with no parameters can never be called: `c.f()` passes one
        // argument to a function that takes none. Reject the declaration rather
        // than let every call site report a baffling arity mismatch.
        let Some(recv) = params.first() else {
            err(
                stmt.span,
                format!(
                    "method `{class}.{method}` declares no receiver parameter \
                     (write `fn {class}.{method}(self: {class}, …)`)"
                ),
            );
            continue;
        };
        // That receiver is whatever `recv.method(...)` dispatched on, so it is
        // always an instance of the class the method is declared on. An
        // annotation that such an instance cannot fill describes a call that
        // can never happen: the declaration is malformed, not merely suspect,
        // so this is fatal rather than one of the checker's warnings.
        if let Some(declared) = resolve_ann(recv.ty.as_ref(), classes)
            && !crate::types::Type::Class(id).is_assignable_to(&declared)
        {
            err(
                stmt.span,
                format!(
                    "method `{class}.{method}` declares its receiver `{}` as `{}`, \
                     but a method on `{class}` always receives an instance of `{class}`",
                    recv.name,
                    declared.display(classes),
                ),
            );
        }
        if let Err(msg) = classes.declare_method(id, method, params.len()) {
            err(stmt.span, msg);
        }
    }
    diags
}

/// Every `class` declaration in `stmts` that is *not* at the top level, with
/// the span to blame and the name it tried to declare.
///
/// Classes are top-level-only. The alternative — genuinely scoped, distinct
/// nested classes — would need a scoped class table, per-scope heap tags and a
/// scoped `type()`; without all of that a nested `class Inner` was simply a
/// second, unregistered meaning for a name whose tag, dispatch and annotations
/// all still pointed at the top-level `Inner`.
fn nested_class_decls(stmts: &[Stmt]) -> Vec<(SourceSpan, String)> {
    struct Collect(Vec<(SourceSpan, String)>);
    impl crate::ast::ExprVisitor for Collect {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::ClassDecl { name, .. } = &s.kind {
                self.0.push((s.span, name.clone()));
            }
            crate::ast::walk_stmt(self, s);
        }
    }
    let mut v = Collect(Vec::new());
    // `walk_stmt` visits a statement's *children*, so a top-level `class` is
    // skipped and everything nested underneath one is not.
    for stmt in stmts {
        crate::ast::walk_stmt(&mut v, stmt);
    }
    v.0
}

/// Collect declared function signatures from a statement list, keyed by
/// `(name, arity)`. Only the *resolved* types are kept — an un-annotated or
/// unrecognized-name parameter/return becomes `None` (checked as `any`). Later
/// declarations of the same `(name, arity)` win. Pure so it is unit-testable
/// without a live [`Compiler`]; `prescan_declarations` folds the result into
/// [`Compiler::fn_signatures`].
pub(crate) fn collect_fn_signatures(
    stmts: &[Stmt],
    classes: &crate::classes::ClassTable,
) -> HashMap<(String, usize), FnSignature> {
    let mut sigs = HashMap::new();
    for stmt in stmts {
        if let StmtKind::FnDecl {
            name, params, ret, ..
        } = &stmt.kind
        {
            let sig = FnSignature {
                params: params
                    .iter()
                    .map(|p| resolve_ann(p.ty.as_ref(), classes))
                    .collect(),
                ret: resolve_ann(ret.as_ref(), classes),
            };
            sigs.insert((name.clone(), params.len()), sig);
        }
    }
    sigs
}

/// Resolve a written annotation, falling back to the class table for a name the
/// parser could not resolve on its own (class names need context — see
/// [`crate::types::Type::resolve`]). `None` means "no annotation, or a name
/// nothing recognizes", which the checker treats as `any`.
pub(crate) fn resolve_ann(
    ann: Option<&crate::ast::TypeAnn>,
    classes: &crate::classes::ClassTable,
) -> Option<crate::types::Type> {
    let ann = ann?;
    ann.resolved
        .or_else(|| classes.lookup(&ann.name).map(crate::types::Type::Class))
}

/// The bare method name of a method declaration — `center_x` for
/// `fn Rect.center_x(...)`, whose [`StmtKind::FnDecl::name`] is the qualified
/// `Rect.center_x`. Returns the whole name for a plain function.
pub(crate) fn method_base_name(kind: &StmtKind) -> &str {
    let StmtKind::FnDecl { name, class, .. } = kind else {
        return "";
    };
    match class {
        Some(c) => &name[c.len() + 1..],
        None => name,
    }
}

#[cfg(test)]
mod prescan_tests {
    use super::collect_fn_signatures;
    use crate::rewrite::parse_ast;
    use crate::types::{FnSignature, Type};

    fn sigs(src: &str) -> std::collections::HashMap<(String, usize), FnSignature> {
        let (_, stmts) = parse_ast(src).expect("parse");
        collect_fn_signatures(&stmts, &crate::classes::ClassTable::new())
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
        let table =
            sigs("fn g(x: int) -> int\n  x\nend\nfn g(x: int, y: int) -> int\n  x + y\nend");
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
