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
//! - `state_ids`  — the name-derived declaration and callsite ids that key
//!                  `state` slots

mod capture_lag;
mod expr;
mod function;
mod phi;
mod state_ids;
mod stmt;

pub(crate) use state_ids::{append_ordinal, callee_text, shift_arg_names};

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
    /// Top-level rebindings in the module being compiled, for the
    /// capture-lag rule (`compiler::capture_lag`).
    module_rebinds: capture_lag::ModuleRebinds,
    /// Source offset just past each open closure's declaration, outermost
    /// first. A module binding captured by the *outermost* closure is frozen
    /// at that point, so that is the offset a later rebinding is measured
    /// against — an inner lambda only ever re-captures what the enclosing
    /// function already froze.
    ///
    /// `None` for a lambda, which exempts it: an anonymous closure written
    /// inline is overwhelmingly a callback (`map(xs, fn(a) … end)`) that runs
    /// and is discarded inside the statement that created it, so it cannot
    /// outlive a later rebinding. Flagging those would reject a core idiom
    /// with no fix available — a `map` callback's parameter list is not the
    /// author's to extend. A lambda that really is stored and called later
    /// keeps today's behaviour; see `capture_lag`'s note on the gap.
    closure_def_ends: Vec<Option<u32>>,

    // Capture tracking for the current function being compiled (stack for nesting)
    capture_stack: Vec<Vec<CaptureInfo>>,

    // Track function body blocks so capture phantoms are created in the right block
    function_body_blocks: Vec<BlockId>,

    // Value-position tracking, so a `for` in a position whose value is used
    // collects into a list (see `compile_stmts`).
    //
    // `stmt_value_used` is set by `compile_stmts` for the final statement of a
    // value-producing list and taken by `compile_stmt`. `value_used` says the
    // expression about to be compiled has its value consumed; it defaults to
    // true (nearly every expression is compiled in value position) and is
    // taken — reset to true — at the top of `compile_expr_kind`, so only a
    // discarded statement-level expression ever sees it false.
    stmt_value_used: bool,
    value_used: bool,

    // Declared type signatures of named functions, keyed by (name, arity) so
    // arity overloads keep distinct entries. Populated by `prescan_declarations`
    // and consulted by the type checker at call sites. Compile-time only.
    fn_signatures: HashMap<(String, usize), FnSignature>,

    // The parameter *names* of those same functions, keyed the same way and
    // kept beside `fn_signatures` rather than inside it: a signature is about
    // types, and only the named-argument check needs the names. Empty for a
    // name nothing declares, which is what keeps that check conservative.
    fn_param_names: HashMap<(String, usize), Vec<String>>,

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

    // What the compiler knows about the *shape* of every function value it has
    // built: which arity a closure term takes, which variants an overload-set
    // term joins, and which module each was declared in. Unlike
    // `overloaded_fns`/`overload_variants` it is **not** cleared per module —
    // it is what lets a module merge its own arity into a set another module
    // owns (see `merge_overload_binding`).
    overload_index: function::OverloadIndex,

    // The function cells *this* module's prescan created, so a declaration
    // writes only a box its own file hoisted. A name that arrives as an
    // imported hoisted `fn` is also a cell binding, and writing that one would
    // reach into the exporting module's own recursion; such a declaration
    // shadows (or merges) instead, like any other redeclaration. Cleared per
    // module.
    own_fn_cells: HashSet<TermId>,

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

    // Names bound to a *function cell* — the hoisting mechanism for top-level
    // `fn`s that are referenced before they are declared (mutual recursion, a
    // helper called from a function written above it). One set per entry in
    // `scopes`, pushed/popped in lockstep with it, exactly like `var_scopes`.
    //
    // The binding holds a cell whose contents the declaration writes when it
    // runs, so a reference compiled *before* the declaration still resolves to
    // the same box and reads the closure once it is there. A closure capturing
    // such a name captures the cell, not the (still nil) value — which is what
    // makes `fn a` above `fn b` able to call `b` at all.
    //
    // Only forward-referenced names get one (see `forward_referenced_fns`), so
    // an ordinary declare-then-call program keeps its direct binding and pays
    // nothing.
    fn_cell_scopes: Vec<HashSet<String>>,

    // Spans of the `fn` declarations this module's prescan already compiled
    // (see `hoistable_fn_names`). `compile_stmt` skips them so a hoisted
    // declaration is not compiled twice. Reset per module — spans are
    // file-local, so two files' spans collide freely.
    hoisted_fn_decls: HashSet<SourceSpan>,

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
    // See docs/var.md (Cross-function assignment).
    cross_fn_terms: HashSet<TermId>,

    // Per-block local-shadow log: block → (name → the binding that was live in
    // this block immediately before a `let`/`state` in it redeclared the name,
    // or `None` if there was no such in-block binding). Populated by
    // `note_shadow` when a declaration shadows a name the enclosing control
    // flow is carrying. From that point on the name is block-local: rebinds
    // stop sharing the carry slot and stop updating `block_rebinds`, and
    // `wire_phi_outs` carries the *frozen* pre-shadow value out instead of the
    // shadowed local's final value. See docs/var.md (Lexical shadowing).
    block_shadowed: HashMap<BlockId, HashMap<String, Option<TermId>>>,

    // Map from a state variable's StateKey back to its `StateInit` term. Used
    // by `compile_assign` to emit a `StateWrite` even after the state name has
    // been rebound (which replaces its scope binding with a `Copy` term, so a
    // simple scope_lookup chain can no longer reach the StateInit).
    state_inits: HashMap<StateKey, TermId>,

    // ── State slot identity (see `state_ids.rs`) ─────────────────────
    //
    // The lexically enclosing functions of the code being compiled, outermost
    // first; empty at module scope. A named `fn` contributes its name (the
    // internal `"name#arity"` for an overload variant), a lambda its binding
    // name when it has one and a per-enclosing-function ordinal otherwise.
    fn_name_chain: Vec<String>,
    // Unnamed-lambda counter, one per level of `fn_name_chain` plus one for
    // module scope at index 0 — so `lambda_counts.len() == fn_name_chain.len() + 1`.
    lambda_counts: Vec<u32>,
    // Next shadow ordinal for each state-declaration base string. Bumped once
    // per compiled `state` declaration, which is what makes two declarations
    // of one name in one function (or a lambda whose binding name collides
    // with a sibling `fn`) land in distinct slots.
    state_decl_ordinals: HashMap<String, u32>,
    // Loop-body nesting depth of the code being compiled, reset at every
    // function boundary. Only its *difference* between a `state` declaration
    // and a later assignment to that variable is used: that difference is the
    // number of loop `Index` parts the write has to drop to reach the
    // declaration's slot (`Term::path_pop`).
    loop_depth: u32,
    // Loop-body nesting depth each `state` declaration was compiled at, so a
    // reassignment can compute its `path_pop`. Keyed by declaration id, which
    // is unique per declaration site.
    state_decl_depths: HashMap<StateKey, u32>,
    // Next ordinal for each callsite base string (module + enclosing-function
    // chain + canonical callee text). Bumped once per compiled call term, so
    // two calls to `f()` in one function get distinct callsite ids while the
    // same call in a different function is counted separately. See
    // `call_site_for`.
    call_site_ordinals: HashMap<String, u32>,
    // Set just before compiling the initializer of `let f = x -> …`, and taken
    // by the lambda's `compile_function` so it joins the name chain as `f`.
    pending_lambda_name: Option<String>,

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
            fn_param_names: HashMap::new(),
            classes: crate::classes::ClassTable::new(),
            method_dispatch: crate::typecheck::MethodDispatch::new(),
            declared_methods: HashSet::new(),
            warnings: Vec::new(),
            function_boundaries: Vec::new(),
            module_rebinds: capture_lag::ModuleRebinds::default(),
            closure_def_ends: Vec::new(),
            capture_stack: Vec::new(),
            function_body_blocks: Vec::new(),
            stmt_value_used: false,
            value_used: true,
            overloaded_fns: HashMap::new(),
            overload_variants: HashMap::new(),
            overload_index: function::OverloadIndex::default(),
            own_fn_cells: HashSet::new(),
            block_rebinds: HashMap::new(),
            carry_slots: Vec::new(),
            block_shadowed: HashMap::new(),
            cross_fn_terms: HashSet::new(),
            var_scopes: Vec::new(),
            fn_cell_scopes: Vec::new(),
            hoisted_fn_decls: HashSet::new(),
            imported_vars: HashMap::new(),
            errors: Vec::new(),
            state_inits: HashMap::new(),
            fn_name_chain: Vec::new(),
            lambda_counts: vec![0],
            loop_depth: 0,
            state_decl_depths: HashMap::new(),
            state_decl_ordinals: HashMap::new(),
            call_site_ordinals: HashMap::new(),
            pending_lambda_name: None,
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
            schema: crate::program::IR_SCHEMA_VERSION.to_string(),
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
        self.own_fn_cells.clear();

        if !is_entry {
            self.push_scope(false); // module scope frame
        }

        // Top-level rebindings, for the capture-lag rule. Per module: a
        // closure can only capture names from its own module scope.
        self.module_rebinds = capture_lag::ModuleRebinds::collect(&stmts);

        self.bind_imports(module, &stmts)?;
        Self::check_overload_export_consistency(&stmts).map_err(|mut e| {
            // Same bytes as the old `format!("{display_name}: {e}")`.
            for item in &mut e.items {
                item.file = Some(module.display_name.clone());
            }
            e
        })?;
        self.prescan_declarations(module, &stmts);
        let (diags, dispatch) = crate::typecheck::check_module(
            &stmts,
            &self.fn_signatures,
            &self.fn_param_names,
            &self.classes,
        );
        self.warnings.extend(diags);
        // Spans are file-local, so this must be *replaced* per module rather
        // than accumulated — two modules' spans collide freely.
        self.method_dispatch = dispatch;
        self.warnings
            .extend(crate::typecheck::unused::check_unused(&stmts));
        // After the checker, before the file's statements: a hoisted body is
        // compiled here and wants the dispatch table the checker just built.
        self.prescan_emit(&stmts);
        for stmt in &stmts {
            self.compile_stmt(stmt);
        }

        if !is_entry {
            // Pop both halves of the frame in lockstep — `binding_is_var`
            // indexes `var_scopes` by `scopes` position, so leaving one behind
            // would hand the next module's frame this module's binding kinds.
            let scope = self.scopes.pop().expect("module scope frame");
            let vars = self.var_scopes.pop().expect("module scope var frame");
            let fn_cells = self
                .fn_cell_scopes
                .pop()
                .expect("module scope fn-cell frame");
            self.capture_exports(module, scope, vars, fn_cells);
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
        // An import over a name some *other* module already put in this scope
        // joins the two overload sets: the higher-precedence import (this one)
        // wins every arity it defines, and the arities only the outgoing
        // binding had stay reachable. Non-functions, and two bindings from the
        // same module, shadow exactly as before.
        if let Some(existing) = self.scope_lookup(name)
            && let Some(merged) = self.merge_overload_binding(name, existing, tid)
        {
            // The merged value is an ordinary function value, not a cell — the
            // exporting module has already run and filled any cell of its own.
            self.scope_bind(name.to_string(), merged);
            return;
        }
        if self.binding_is_var(&format!("{module}::{name}")) {
            self.scope_bind_var(name.to_string(), tid);
            self.imported_vars
                .insert(name.to_string(), (module.to_string(), tid));
        } else if self.binding_is_fn_cell(&format!("{module}::{name}")) {
            // A hoisted `fn` binds its cell; the bare alias has to know that,
            // or a read would forward the cell id instead of the closure.
            self.scope_bind_fn_cell(name.to_string(), tid);
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
        fn_cells: HashSet<String>,
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
                global_vars.insert(qualified.clone());
            }
            // A hoisted `fn` exports its *cell* for the same reason: an
            // importer's read has to dereference it, not forward the cell id.
            if fn_cells.contains(name)
                && let Some(global_cells) = self.fn_cell_scopes.first_mut()
            {
                global_cells.insert(qualified);
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
            terms: Vec::new(),
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
        self.fn_cell_scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.var_scopes.pop();
        self.fn_cell_scopes.pop();
        if let Some(&boundary) = self.function_boundaries.last()
            && boundary >= self.scopes.len()
        {
            self.function_boundaries.pop();
        }
    }

    fn scope_bind(&mut self, name: String, term_id: TermId) {
        // A plain rebind in the same scope drops function-cell-ness: whatever
        // is bound now is an ordinary value term, and a read of it must not
        // compile to a `CellRead` of a non-cell.
        if let Some(cells) = self.fn_cell_scopes.last_mut() {
            cells.remove(&name);
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, term_id);
        }
    }

    /// Bind `name` to a hoisted function cell: reads dereference it and a
    /// closure capturing it captures the cell, so the binding is live before
    /// the declaration that fills it has run. See `fn_cell_scopes`.
    pub(super) fn scope_bind_fn_cell(&mut self, name: String, term_id: TermId) {
        self.scope_bind(name.clone(), term_id);
        if let Some(cells) = self.fn_cell_scopes.last_mut() {
            cells.insert(name);
        }
    }

    /// Is the innermost binding of `name` a hoisted function cell? Answered
    /// against the scope that actually binds the name, so a local `let f`
    /// shadowing a hoisted top-level `fn f` reports its own kind.
    pub(super) fn binding_is_fn_cell(&self, name: &str) -> bool {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return self.fn_cell_scopes[i].contains(name);
            }
        }
        false
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

    /// Is the scope that binds `name` below index `limit`? Used to ask whether
    /// a binding is module-level: `limit` is the outermost function boundary,
    /// so anything below it was declared outside every function.
    pub(super) fn binding_is_below_scope(&self, name: &str, limit: usize) -> bool {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return i < limit;
            }
        }
        false
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

    /// Record a non-fatal diagnostic at `span`. Compilation succeeds; the
    /// warning rides along on the compiled program, like the type checker's.
    pub(super) fn warn_at(&mut self, span: SourceSpan, message: String) {
        self.warnings
            .push(crate::diagnostic::Diagnostic { span, message });
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

        let mut term = Term::new(term_id, op, inputs, block_id, name, reg);
        term.block_prev = prev;
        self.terms.push(term);

        // Link prev -> this
        if let Some(prev_id) = prev {
            self.terms[prev_id.0 as usize].block_next = Some(term_id);
        } else {
            // First term in block — set as entry
            self.blocks[block_id.0 as usize].entry = Some(term_id);
        }

        // The ordered terms array is the wire form of this linked list
        // (schema v0.2); this is its single append site, so the two stay in
        // lockstep by construction.
        self.blocks[block_id.0 as usize].terms.push(term_id);

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
        self.terms.push(Term::new(
            term_id,
            TermOp::Copy,
            SmallVec::new(),
            block_id,
            Some(name),
            reg,
        ));
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
        self.fn_param_names.extend(collect_fn_param_names(stmts));

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
    }

    /// The emitting half of the prescan: the declarations that are *hoisted* —
    /// class constructors, the cells of forward-referenced functions, enum
    /// phantoms, and the hoisted `fn` bodies themselves — all emitted into the
    /// root block ahead of the file's first statement.
    ///
    /// Split from [`Self::prescan_declarations`] so the type checker can run in
    /// between: a hoisted body's `c.first()` binds statically only if
    /// `method_dispatch` already says which class the receiver is, and that
    /// table is the checker's output.
    fn prescan_emit(&mut self, stmts: &[Stmt]) {
        // Which top-level `fn`s are hoisted, and which of them need a cell.
        // Both are decided before anything is emitted, because the cells have
        // to exist before the first hoisted body is compiled.
        // A `fn` that *shadows* a name already in scope — a builtin, or an
        // import — is left exactly where it is. Reading the old meaning before
        // the shadow lands is a deliberate idiom (`let _draw_line = draw_line`
        // above `fn draw_line`), and both hoisting the declaration and binding
        // the name to a cell would silently turn that read into nil. That is
        // handed to `hoistable_fn_names` rather than filtered out afterwards,
        // so a caller of such a `fn` is held back with it — a hoisted caller
        // would bind to the shadowed meaning.
        let hoistable = hoistable_fn_names(stmts, |n| self.scope_lookup(n).is_some());
        let mut forward = forward_referenced_fns(stmts);
        forward.extend(late_bound_fn_refs(stmts, &hoistable));
        // Same two exclusions as hoisting: a name already in scope keeps its
        // meaning until the declaration shadows it, and a name a `let`/`state`
        // also declares is rebound in source order, which a cell would fight.
        let shadowed = top_level_value_names(stmts);
        forward.retain(|n| self.scope_lookup(n).is_none() && !shadowed.contains(n));
        let late: Vec<crate::diagnostic::Diagnostic> = late_declaration_warnings(stmts, &hoistable)
            .into_iter()
            .filter(|(name, _)| self.scope_lookup(name).is_none())
            .map(|(_, d)| d)
            .collect();
        self.warnings.extend(late);
        self.hoisted_fn_decls.clear();

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
                    if forward.contains(name) {
                        // Hoisted: bind the name to a cell now, ahead of every
                        // statement in the file, so a reference compiled before
                        // the declaration — including one inside a function
                        // written above it — resolves to the same box the
                        // declaration will fill. This is what makes mutual
                        // recursion possible: `a`'s closure captures `b`'s cell
                        // rather than `b`'s (still nil) value.
                        let nil = self.constants.intern(ConstantValue::Nil);
                        let nil_tid = self.emit_term(TermOp::Constant(nil), smallvec![], None);
                        let cell = self.emit_term(TermOp::CellNew, smallvec![nil_tid], None);
                        self.source_map.add(cell, stmt.span);
                        self.own_fn_cells.insert(cell);
                        self.scope_bind_fn_cell(name.clone(), cell);
                    } else if self.scope_lookup(name).is_none() {
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

        // Emit the hoisted declarations, in source order, ahead of the file's
        // first statement. A hoisted `fn` closes over nothing this file
        // computes at run time (see `hoistable_fn_names`), so moving *when* its
        // closure is created cannot change what it sees — and it makes
        // `main()` above `fn main` work, not just calls between functions.
        // `compile_stmt` skips the declarations recorded here.
        for stmt in stmts {
            let StmtKind::FnDecl {
                name,
                class,
                params,
                body,
                ..
            } = &stmt.kind
            else {
                continue;
            };
            if !hoistable.contains(name) {
                continue;
            }
            let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
            // `def_end` is where the declaration *is written*, not where it is
            // emitted: the capture-lag check compares it against the source
            // position of a later rebind, which hoisting must not move.
            let bound = self.compile_fn_decl(name, &param_names, body, stmt.span.end.offset);
            if let Some(tid) = bound {
                self.source_map.add(tid, stmt.span);
            }
            // Methods are hoisted with everything else, in source order.
            // They have to be: a method's registration is what lets a pinned
            // `c.first()` bind to the declaration, and a hoisted function whose
            // body contains such a call is now compiled *here*. Registering in
            // this same pass keeps the two in the order the file wrote them.
            if let (Some(class), Some(tid)) = (class, bound) {
                let method = crate::compiler::method_base_name(&stmt.kind).to_string();
                self.emit_declare_method(class, &method, tid);
            }
            self.hoisted_fn_decls.insert(stmt.span);
        }
    }
}

/// Names this file binds to a *computed* value at the top level: `let`, `var`,
/// `state`, and enum variants (whose constructors are built where the `enum`
/// statement stands). A function body that mentions one of these is tied to the
/// order the file runs in, and so is a `fn` whose own name is one of them.
fn top_level_value_names(stmts: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => {
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

/// Top-level `fn` names whose declarations can be emitted ahead of the file's
/// statements — those whose bodies mention nothing the file *computes*: no
/// top-level `let`/`var`/`state`, and no enum variant (its constructor is built
/// where the `enum` statement stands).
///
/// Such a function captures nothing from the file's run-time work, so creating
/// its closure early cannot change what the closure sees; a function that does
/// capture one stays exactly where it was written, capturing the same value it
/// captures today. Over-approximate on purpose — a body that merely reuses a
/// top-level name as a local is treated as depending on it — because the
/// failure mode of a false negative is only "this one is not hoisted".
///
/// An overloaded name is hoisted only if every arity is: the overload set is
/// emitted once the last variant compiles, so a split would gain nothing.
///
/// Blocking is **transitive**. A hoisted body is compiled up front, so every
/// name it calls resolves against the scope as it stands *there* — and a
/// function that stayed behind has not bound its name yet. When the stay-behind
/// also shadows something (`let _rect = draw_rect` above
/// `fn draw_rect(r, c)`, the shape every prelude uses), the early call does not
/// fail loudly: it binds to the *old* meaning, the 7-argument native, and the
/// record-form call dies at run time. So a function that references a
/// non-hoistable top-level function is itself non-hoistable, to a fixpoint.
///
/// `shadows_existing` reports the names already bound where the file starts —
/// a builtin or an import the file redeclares. Those declarations are left
/// where they are for the same reason (see `prescan_emit`), which makes them
/// blocked seeds here so their callers are held back too.
fn hoistable_fn_names(stmts: &[Stmt], shadows_existing: impl Fn(&str) -> bool) -> HashSet<String> {
    let computed = top_level_value_names(stmts);

    // Every top-level `fn`, with the names its body mentions. Kept so the
    // fixpoint below can re-ask "does this one reach anything blocked?"
    // without re-walking the AST.
    let mut fn_refs: Vec<(&str, HashSet<String>)> = Vec::new();
    // A name a `let`/`state` also declares is never hoisted: the two
    // declarations shadow each other in source order, and moving one of them
    // would change which meaning the statements between them see.
    let mut blocked: HashSet<String> = computed.clone();
    for stmt in stmts {
        // Methods (`fn C.first`, bound under `"C.first"`) are hoisted too:
        // their registration is what a pinned `c.first()` binds to, and the
        // functions containing such calls are hoisted now.
        let StmtKind::FnDecl { name, body, .. } = &stmt.kind else {
            continue;
        };
        let refs = idents_in_stmts(body);
        if refs.iter().any(|id| computed.contains(id.as_str())) || shadows_existing(name) {
            blocked.insert(name.clone());
        }
        fn_refs.push((name.as_str(), refs));
    }

    // Fixpoint: a `fn` that reaches a blocked name is blocked. Terminates
    // because `blocked` only grows and is bounded by the file's `fn` names.
    loop {
        let mut changed = false;
        for (name, refs) in &fn_refs {
            if blocked.contains(*name) {
                continue;
            }
            if refs.iter().any(|id| blocked.contains(id.as_str())) {
                blocked.insert((*name).to_string());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    fn_refs
        .iter()
        .map(|(name, _)| (*name).to_string())
        .filter(|n| !blocked.contains(n))
        .collect()
}

/// Warn about the one forward reference hoisting cannot fix: a top-level
/// statement that *runs* a function whose declaration is below it and which
/// could not be hoisted, because its body reads something the file computes.
/// The call reaches a name that is still nil, and without this the only
/// symptom is `Cannot call nil` at run time with no mention of declaration
/// order — the thing that actually went wrong.
///
/// References from inside a function or lambda body are never reported: those
/// run later, by which point the declaration has executed. That is the whole
/// point of the cell, and it is why the visitor stops at every function
/// boundary.
fn late_declaration_warnings(
    stmts: &[Stmt],
    hoistable: &HashSet<String>,
) -> Vec<(String, crate::diagnostic::Diagnostic)> {
    let mut decl_at: HashMap<&str, usize> = HashMap::new();
    for (i, stmt) in stmts.iter().enumerate() {
        if let StmtKind::FnDecl {
            name, class: None, ..
        } = &stmt.kind
        {
            decl_at.entry(name.as_str()).or_insert(i);
        }
    }

    struct Refs<'a> {
        decl_at: &'a HashMap<&'a str, usize>,
        hoistable: &'a HashSet<String>,
        here: usize,
        out: &'a mut Vec<(String, crate::diagnostic::Diagnostic)>,
    }
    impl crate::ast::ExprVisitor for Refs<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            // A lambda body runs later, like a `fn` body.
            if matches!(e.kind, ExprKind::Lambda { .. }) {
                return;
            }
            // Call position only. A bare mention (`let f2 = f`) is sometimes
            // deliberate — capturing a name's *current* meaning before a
            // declaration below shadows it — and the reported failure is
            // always a call.
            if let ExprKind::Call { function, .. } = &e.kind
                && let ExprKind::Ident(name) = &function.kind
                && let Some(&d) = self.decl_at.get(name.as_str())
                && d > self.here
                && !self.hoistable.contains(name)
            {
                self.out.push((
                    name.clone(),
                    crate::diagnostic::Diagnostic {
                        span: function.span,
                        message: format!(
                            "call to `{name}` before its declaration, which is further down \
                             this file and cannot be hoisted: its body reads a value the file \
                             computes at run time, so at this point `{name}` is still nil. \
                             Move this call below the declaration of `{name}`."
                        ),
                    },
                ));
            }
            crate::ast::walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            if matches!(s.kind, StmtKind::FnDecl { .. }) {
                return;
            }
            crate::ast::walk_stmt(self, s);
        }
    }

    let mut out = Vec::new();
    for (i, stmt) in stmts.iter().enumerate() {
        if matches!(stmt.kind, StmtKind::FnDecl { .. }) {
            continue;
        }
        let mut v = Refs {
            decl_at: &decl_at,
            hoistable,
            here: i,
            out: &mut out,
        };
        crate::ast::ExprVisitor::visit_stmt(&mut v, stmt);
    }
    out
}

/// Top-level `fn` names that a *hoisted* body references but that are not
/// themselves hoisted — they are bound where they are written, which is after
/// every hoisted declaration, so the reference has to go through a cell to
/// reach them at all. (A hoisted body referencing another hoisted function
/// declared later is already covered by `forward_referenced_fns`.)
fn late_bound_fn_refs(stmts: &[Stmt], hoistable: &HashSet<String>) -> HashSet<String> {
    let top_level_fns: HashSet<&str> = stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::FnDecl {
                name, class: None, ..
            } => Some(name.as_str()),
            _ => None,
        })
        .collect();

    let mut needed = HashSet::new();
    for stmt in stmts {
        // Every hoisted body, methods included — each is compiled before the
        // file's first statement, so each one's references are subject to the
        // same rule.
        let StmtKind::FnDecl { name, body, .. } = &stmt.kind else {
            continue;
        };
        if !hoistable.contains(name) {
            continue;
        }
        for id in idents_in_stmts(body) {
            if top_level_fns.contains(id.as_str()) && !hoistable.contains(&id) {
                needed.insert(id);
            }
        }
    }
    needed
}

/// Every name spelled anywhere inside `stmts`, nesting included: plain
/// identifiers, `get x` / `@x`, and the root of an assignment target. Bindings
/// are not tracked, so this is a superset of the free variables — every caller
/// here wants exactly that conservatism, and every *spelling* of a name has to
/// be in it or a body that reads `get hits` looks independent of `var hits`.
fn idents_in_stmts(stmts: &[Stmt]) -> HashSet<String> {
    struct Idents<'a>(&'a mut HashSet<String>);
    impl crate::ast::ExprVisitor for Idents<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Ident(name) | ExprKind::AtVar(name) | ExprKind::CellGet(name) => {
                    self.0.insert(name.clone());
                }
                _ => {}
            }
            crate::ast::walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            // An assignment's target root is a name the statement touches but
            // never an `Ident` expression, so `walk_stmt` would not show it.
            // (`walk_stmt` covers the object/index expressions of the other
            // two target forms, whose roots are ordinary identifiers.)
            if let StmtKind::Assign {
                target: crate::ast::AssignTarget::Name(name),
                ..
            }
            | StmtKind::Set {
                target: crate::ast::AssignTarget::Name(name),
                ..
            } = &s.kind
            {
                self.0.insert(name.clone());
            }
            crate::ast::walk_stmt(self, s);
        }
    }
    let mut names = HashSet::new();
    let mut v = Idents(&mut names);
    for stmt in stmts {
        crate::ast::ExprVisitor::visit_stmt(&mut v, stmt);
    }
    names
}

/// The top-level `fn` names in `stmts` that are *used before they are
/// declared* — the ones that need hoisting.
///
/// A name qualifies when it appears as an identifier anywhere in a statement
/// that precedes its first declaration, which includes the body of a function
/// declared above it: that is exactly the mutual-recursion case (`fn a` calls
/// `b`, `fn b` calls `a` — `b` is forward-referenced, `a` is not).
///
/// Deliberately an over-approximation: it does not track shadowing, so a local
/// `b` inside an earlier function also marks the top-level `b`. The cost of a
/// false positive is one indirection on reads of that name; the cost of a false
/// negative is a call to nil, so the analysis errs toward hoisting. Names never
/// mentioned before their declaration keep their direct binding and are
/// completely unaffected.
///
/// Method declarations (`fn Point.scaled(...)`, whose bound name is
/// `"Point.scaled"`) can never match: that name has no identifier spelling.
/// Method registration therefore keeps its source-order behaviour.
fn forward_referenced_fns(stmts: &[Stmt]) -> HashSet<String> {
    // First declaration index per top-level fn name.
    let mut decl_at: HashMap<&str, usize> = HashMap::new();
    for (i, stmt) in stmts.iter().enumerate() {
        if let StmtKind::FnDecl {
            name, class: None, ..
        } = &stmt.kind
        {
            decl_at.entry(name.as_str()).or_insert(i);
        }
    }
    if decl_at.is_empty() {
        return HashSet::new();
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut forward = HashSet::new();
    for (i, stmt) in stmts.iter().enumerate() {
        for (name, &d) in &decl_at {
            if d == i && seen.contains(*name) {
                forward.insert((*name).to_string());
            }
        }
        seen.extend(idents_in_stmts(std::slice::from_ref(stmt)));
    }
    forward
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
            ret,
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
        // The same signature shape `collect_fn_signatures` builds for a free
        // function, receiver included — it is what lets a call site read the
        // method's return type instead of inferring `any`.
        let sig = FnSignature {
            params: params
                .iter()
                .map(|p| resolve_ann(p.ty.as_ref(), classes))
                .collect(),
            ret: resolve_ann(ret.as_ref(), classes),
        };
        if let Err(msg) = classes.declare_method(id, method, sig) {
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

/// Collect declared parameter *names*, keyed by `(name, arity)` exactly like
/// [`collect_fn_signatures`], so a call site can be checked against the names
/// the declaration wrote (`f(limit: 10)`). Later declarations of the same
/// `(name, arity)` win, as there too.
pub(crate) fn collect_fn_param_names(stmts: &[Stmt]) -> HashMap<(String, usize), Vec<String>> {
    let mut names = HashMap::new();
    for stmt in stmts {
        if let StmtKind::FnDecl { name, params, .. } = &stmt.kind {
            names.insert(
                (name.clone(), params.len()),
                params.iter().map(|p| p.name.clone()).collect(),
            );
        }
    }
    names
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

    fn param_names(src: &str) -> std::collections::HashMap<(String, usize), Vec<String>> {
        let (_, stmts) = parse_ast(src).expect("parse");
        super::collect_fn_param_names(&stmts)
    }

    #[test]
    fn collects_param_names_by_arity() {
        let table = param_names("fn g(x)\n  x\nend\nfn g(x, limit: int)\n  x\nend");
        assert_eq!(table[&("g".to_string(), 1)], vec!["x".to_string()]);
        assert_eq!(
            table[&("g".to_string(), 2)],
            vec!["x".to_string(), "limit".to_string()]
        );
    }

    #[test]
    fn collects_method_names_under_the_qualified_name() {
        let table = param_names("class R\n  w: int\nend\nfn R.grow(r, by)\n  r\nend");
        assert_eq!(
            table[&("R.grow".to_string(), 2)],
            vec!["r".to_string(), "by".to_string()]
        );
    }

    #[test]
    fn no_functions_yields_no_param_names() {
        assert!(param_names("let x = 5\nprint(x)").is_empty());
    }
}
