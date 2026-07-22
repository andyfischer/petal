//! Module resolution and the import walk.
//!
//! Petal's module system merges all imported modules into a single `Program`
//! at compile time (see docs/module-system.md). This file owns the front half
//! of that pipeline: turning an entry source plus a resolver into the flat,
//! dependency-ordered list of parsed modules the compiler consumes.
//!
//! The language does not know about files — a module name is an identifier,
//! and what it resolves *to* is the resolver's business. The built-in
//! [`ModuleRegistry`] resolves, in order:
//! 1. embedder-registered in-memory modules (`Env::register_module`) — how a
//!    prelude ships and how wasm hosts with no filesystem work,
//! 2. the importing file's directory (`<dir>/<name>.ptl`),
//! 3. registered search paths (`Env::add_module_path`, CLI `-I`), then the
//!    directories in the `PETAL_PATH` environment variable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ast::{Expr, ExprKind, ExprVisitor, ImportDecl, Stmt, StmtKind, walk_expr};
use crate::source_map::{ENTRY_FILE, FileId};

/// Where a module's source came from.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleOrigin {
    /// Registered in memory via `Env::register_module`.
    Memory,
    /// Read from a file on disk.
    File(PathBuf),
}

/// A resolved module source, before parsing.
pub struct ModuleSource {
    pub name: String,
    pub source: String,
    pub origin: ModuleOrigin,
}

/// Pluggable module resolution: given a module name and the origin of the
/// importing file, produce source. [`ModuleRegistry`] is the built-in
/// implementation used by `Env`.
pub trait ModuleResolver {
    fn resolve(&self, name: &str, importer: Option<&ModuleOrigin>) -> Option<ModuleSource>;
}

/// The built-in resolver held by `Env`. See the module docs for the
/// resolution order.
#[derive(Default)]
pub struct ModuleRegistry {
    memory: std::collections::HashMap<String, String>,
    paths: Vec<PathBuf>,
    /// Modules every loaded entry file imports as if by `import m: <all>`,
    /// without ceremony. The entry file's own bindings still win, and an
    /// explicit `import m` is a no-op on top of this.
    pub implicit_imports: Vec<String>,
    /// Implicit imports that come *before* [`implicit_imports`] and are not
    /// touched by `set_implicit_imports`. The core prelude (`std`) lives here
    /// so a host that replaces its own implicit imports can't drop it. Lowest
    /// precedence of all, so both host preludes and user code shadow it.
    pub base_implicit_imports: Vec<String>,
}

impl ModuleRegistry {
    /// Register an in-memory module. Takes priority over the filesystem.
    pub fn register(&mut self, name: &str, source: &str) {
        self.memory.insert(name.to_string(), source.to_string());
    }

    /// Append a directory to the module search path.
    pub fn add_path(&mut self, dir: PathBuf) {
        self.paths.push(dir);
    }
}

impl ModuleResolver for ModuleRegistry {
    fn resolve(&self, name: &str, importer: Option<&ModuleOrigin>) -> Option<ModuleSource> {
        // 1. In-memory registrations.
        if let Some(source) = self.memory.get(name) {
            return Some(ModuleSource {
                name: name.to_string(),
                source: source.clone(),
                origin: ModuleOrigin::Memory,
            });
        }

        // 2. The importing file's directory, 3. search paths, 4. PETAL_PATH.
        let importer_dir = match importer {
            Some(ModuleOrigin::File(path)) => path.parent().map(Path::to_path_buf),
            _ => None,
        };
        let petal_path = std::env::var("PETAL_PATH").unwrap_or_default();
        let candidates = importer_dir
            .into_iter()
            .chain(self.paths.iter().cloned())
            .chain(
                petal_path
                    .split(':')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from),
            );
        for dir in candidates {
            let path = dir.join(format!("{name}.ptl"));
            if let Ok(source) = std::fs::read_to_string(&path) {
                return Some(ModuleSource {
                    name: name.to_string(),
                    source,
                    origin: ModuleOrigin::File(path),
                });
            }
        }
        None
    }
}

/// One import binding the compiler must materialize in the importing file's
/// scope. `implicit` imports bind every export bare but *weakly*: the file's
/// own declarations shadow them silently, and they never raise collision
/// errors (mirroring how builtins behave).
pub struct ResolvedImport {
    pub decl: ImportDecl,
    pub implicit: bool,
}

/// One parsed source file, ready for compilation. `name` is `None` for the
/// entry file. `stmts` no longer contain the leading `import` statements —
/// those are extracted into `imports`.
pub struct LoadedModule {
    pub name: Option<String>,
    /// Display name used in diagnostics (`ui.ptl`, or the module name for
    /// in-memory modules).
    pub display_name: String,
    pub source: String,
    pub origin: Option<PathBuf>,
    pub stmts: Vec<Stmt>,
    pub imports: Vec<ResolvedImport>,
    pub file_id: FileId,
}

/// Load the entry source and every transitively imported module. Returns the
/// modules in dependency post-order with the entry file **last** — the order
/// the compiler emits them into the merged root block, so each module's
/// top-level statements execute exactly once, before its importers.
///
/// A module name resolves at most once per load (diamond imports dedupe).
/// Cycles and unresolvable names are errors.
///
/// Two flavours of implicit import compose here, both binding every export bare
/// and weakly (the entry's own declarations shadow them):
/// - `implicit_imports` — host preludes (e.g. `ui`), always merged.
/// - `gated_imports` — the core prelude (`std`), merged *only when the program
///   references one of its exports*. This keeps the prelude zero-cost: a script
///   that never calls `sum`/`first`/… compiles byte-for-byte as if `std` didn't
///   exist, so the standard library can grow without bloating every program.
///   Gated modules sit at the lowest precedence (a host prelude and user code
///   both shadow them).
pub fn load_modules(
    entry_source: &str,
    entry_origin: Option<&Path>,
    resolver: &dyn ModuleResolver,
    implicit_imports: &[String],
    gated_imports: &[String],
) -> Result<Vec<LoadedModule>, String> {
    let entry_display = entry_origin
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<entry>".to_string());
    let stmts = parse_module(entry_source, ENTRY_FILE, None)?;
    let (explicit_imports, stmts) = split_imports(stmts);
    let entry_module_origin = entry_origin.map(|p| ModuleOrigin::File(p.to_path_buf()));

    let mut walker = Walker {
        resolver,
        loaded: HashSet::new(),
        in_progress: Vec::new(),
        out: Vec::new(),
        next_file: 1,
    };

    // The always-merged imports, in precedence order (host implicit first, then
    // the entry's explicit imports). The entry's own bindings land on top.
    let mut ungated: Vec<ResolvedImport> = implicit_imports
        .iter()
        .map(|name| ResolvedImport {
            decl: import_all(name),
            implicit: true,
        })
        .collect();
    ungated.extend(explicit_imports);
    for import in &ungated {
        walker.visit(
            &import.decl.module,
            entry_module_origin.as_ref(),
            &entry_display,
        )?;
    }
    let ungated_modules = std::mem::take(&mut walker.out);

    // Reference-gate the core prelude: include a gated module only when the
    // entry or one of the always-merged modules names one of its exports. An
    // included prelude module's own references can pull in a further one, so
    // iterate to a fixpoint.
    let mut refs: HashSet<String> = HashSet::new();
    collect_module_refs(&stmts, &mut refs);
    for m in &ungated_modules {
        collect_module_refs(&m.stmts, &mut refs);
    }
    let scanned: Vec<(String, Vec<String>, HashSet<String>)> = gated_imports
        .iter()
        .map(|name| {
            let (exports, idents) =
                scan_gated_module(resolver, name, entry_module_origin.as_ref())?;
            Ok((name.clone(), exports, idents))
        })
        .collect::<Result<_, String>>()?;
    let mut included: HashSet<String> = HashSet::new();
    loop {
        let mut progressed = false;
        for (name, exports, idents) in &scanned {
            if included.contains(name) {
                continue;
            }
            if exports.iter().any(|e| refs.contains(e)) {
                included.insert(name.clone());
                refs.extend(idents.iter().cloned());
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // Walk the referenced gated modules and bind them ahead of everything else
    // (lowest precedence). Preserve the caller's order for determinism.
    let mut gated_decls: Vec<ResolvedImport> = Vec::new();
    for name in gated_imports.iter().filter(|n| included.contains(*n)) {
        walker.visit(name, entry_module_origin.as_ref(), &entry_display)?;
        gated_decls.push(ResolvedImport {
            decl: import_all(name),
            implicit: true,
        });
    }
    let gated_modules = std::mem::take(&mut walker.out);

    // Assemble: prelude modules run first, then the always-merged modules, then
    // the entry. The entry's import list mirrors that precedence order.
    let mut modules = gated_modules;
    modules.extend(ungated_modules);
    let mut entry_imports = gated_decls;
    entry_imports.extend(ungated);
    modules.push(LoadedModule {
        name: None,
        display_name: entry_display,
        source: entry_source.to_string(),
        origin: entry_origin.map(Path::to_path_buf),
        stmts,
        imports: entry_imports,
        file_id: ENTRY_FILE,
    });
    Ok(modules)
}

/// `import <name>` binding all of the module's exports (no alias, no selection).
fn import_all(name: &str) -> ImportDecl {
    ImportDecl {
        module: name.to_string(),
        alias: None,
        names: None,
    }
}

/// The top-level names a statement declares (`fn f`, `let x`, `state s`,
/// `enum E`), for reading a module's exported surface.
fn declared_name(kind: &StmtKind) -> Option<&str> {
    match kind {
        StmtKind::Let { name, .. }
        | StmtKind::FnDecl { name, .. }
        | StmtKind::State { name, .. }
        | StmtKind::EnumDecl { name, .. } => Some(name),
        _ => None,
    }
}

/// Resolve and parse a gated (core-prelude) module without loading it, to learn
/// its exported names (what a reference must name to pull it in) and the
/// identifiers it itself references (so one prelude module can pull in another).
fn scan_gated_module(
    resolver: &dyn ModuleResolver,
    name: &str,
    importer: Option<&ModuleOrigin>,
) -> Result<(Vec<String>, HashSet<String>), String> {
    let resolved = resolver.resolve(name, importer).ok_or_else(|| {
        format!("cannot find implicit prelude module '{name}': not registered or on a search path")
    })?;
    let display = match &resolved.origin {
        ModuleOrigin::File(path) => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{name}.ptl")),
        ModuleOrigin::Memory => name.to_string(),
    };
    let stmts = parse_module(&resolved.source, ENTRY_FILE, Some(&display))?;
    let (_imports, stmts) = split_imports(stmts);
    let exports = stmts
        .iter()
        .filter(|s| s.exported)
        .filter_map(|s| declared_name(&s.kind).map(str::to_string))
        .collect();
    let mut idents = HashSet::new();
    collect_module_refs(&stmts, &mut idents);
    Ok((exports, idents))
}

/// The names a module references that could resolve to a *prelude* export: every
/// bare identifier used (`ExprKind::Ident`), minus the names the module declares
/// at its own top level. A top-level `let`/`fn`/`state`/`enum` of name `N`
/// shadows the (weak) prelude binding of `N` throughout the module, so a use of
/// `N` there is never the prelude's — dropping it keeps a variable that merely
/// shares a stdlib name (e.g. `state count`) from pulling the prelude in.
/// Nested/local declarations are deliberately *not* subtracted: they shadow only
/// their own scope, so a sibling use of `N` may still want the prelude.
fn collect_module_refs(stmts: &[Stmt], refs: &mut HashSet<String>) {
    struct Collector<'a> {
        names: &'a mut HashSet<String>,
    }
    impl ExprVisitor for Collector<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::Ident(name) = &e.kind {
                self.names.insert(name.clone());
            }
            walk_expr(self, e);
        }
    }
    let mut used = HashSet::new();
    let mut c = Collector { names: &mut used };
    for s in stmts {
        c.visit_stmt(s);
    }
    for s in stmts {
        if let Some(n) = declared_name(&s.kind) {
            used.remove(n);
        }
    }
    refs.extend(used);
}

struct Walker<'a> {
    resolver: &'a dyn ModuleResolver,
    loaded: HashSet<String>,
    /// DFS stack of module names currently being loaded, for cycle reporting.
    in_progress: Vec<String>,
    /// Finished modules in dependency post-order.
    out: Vec<LoadedModule>,
    next_file: u16,
}

impl Walker<'_> {
    fn visit(
        &mut self,
        name: &str,
        importer_origin: Option<&ModuleOrigin>,
        importer_display: &str,
    ) -> Result<(), String> {
        if self.loaded.contains(name) {
            return Ok(());
        }
        if let Some(pos) = self.in_progress.iter().position(|n| n == name) {
            let mut cycle: Vec<&str> = self.in_progress[pos..].iter().map(String::as_str).collect();
            cycle.push(name);
            return Err(format!("import cycle: {}", cycle.join(" -> ")));
        }

        let resolved = self
            .resolver
            .resolve(name, importer_origin)
            .ok_or_else(|| {
                format!(
                    "cannot find module '{}' (imported by {}): not registered, and no \
                 {}.ptl in the importing file's directory, module paths, or PETAL_PATH",
                    name, importer_display, name
                )
            })?;

        let display_name = match &resolved.origin {
            ModuleOrigin::File(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{name}.ptl")),
            ModuleOrigin::Memory => name.to_string(),
        };
        let file_id = FileId(self.next_file);
        self.next_file = self
            .next_file
            .checked_add(1)
            .ok_or("too many modules (file table limit is 65535)")?;

        let stmts = parse_module(&resolved.source, file_id, Some(&display_name))?;
        let (imports, stmts) = split_imports(stmts);

        self.in_progress.push(name.to_string());
        for import in &imports {
            self.visit(&import.decl.module, Some(&resolved.origin), &display_name)?;
        }
        self.in_progress.pop();

        self.loaded.insert(name.to_string());
        self.out.push(LoadedModule {
            name: Some(name.to_string()),
            display_name,
            source: resolved.source,
            origin: match resolved.origin {
                ModuleOrigin::File(path) => Some(path),
                ModuleOrigin::Memory => None,
            },
            stmts,
            imports,
            file_id,
        });
        Ok(())
    }
}

/// Lex + parse one module's source, tagging spans with `file_id`. Errors from
/// non-entry modules are prefixed with the module's display name (entry-file
/// errors keep today's format).
fn parse_module(
    source: &str,
    file_id: FileId,
    display_name: Option<&str>,
) -> Result<Vec<Stmt>, String> {
    let annotate = |e: String| match display_name {
        Some(name) => format!("{name}: {e}"),
        None => e,
    };
    let (_tree, stmts) = crate::cst::parse_source(source, file_id).map_err(annotate)?;
    Ok(stmts)
}

/// Split a parsed statement list into its leading imports and the rest.
/// The parser guarantees imports form a prefix.
fn split_imports(stmts: Vec<Stmt>) -> (Vec<ResolvedImport>, Vec<Stmt>) {
    let mut imports = Vec::new();
    let mut rest = Vec::new();
    for stmt in stmts {
        match stmt.kind {
            StmtKind::Import(decl) => imports.push(ResolvedImport {
                decl,
                implicit: false,
            }),
            _ => rest.push(stmt),
        }
    }
    (imports, rest)
}
