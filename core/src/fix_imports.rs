//! `petal fix-imports` — rewrite a file's imports for the current module
//! system (docs/module-system.md).
//!
//! Three rewrites, each safe to run again on its own output:
//!
//! 1. **A sibling file imported by a bare name becomes a relative import.**
//!    `import level` used to find `level.ptl` beside the importing file; a
//!    bare first segment now always names a package, so that line becomes
//!    `import ./level`. A path that *does* resolve as a package is left alone.
//!    A loose file that was found through `-I <dir>` becomes the relative path
//!    to it.
//! 2. **`import m: *` becomes `import m`.** The plain form binds every export
//!    now; the star form is gone.
//!    A selective import used to bind the module's qualifier as well
//!    (`import m: a` allowed `m.b`); a file that relies on that gains the
//!    plain `import m` it now needs.
//! 3. **A missing import is added.** A bare name that nothing in the file
//!    declares or imports, and that exactly one available package (or one
//!    module of a package with no module of its own name) exports, gets the
//!    `import <package>` line it needs. This is how the import for a host
//!    interface, or for a builtin that moved into a package, gets written.
//!
//! "Available" is whatever the [`Env`] the fixer holds can resolve: bundled
//! packages, `-I` / `PETAL_PATH` packages, and anything a host registered.
//!
//! The first two are edits to the import lines alone and need only a parse.
//! The third compiles the file (check-only), so it runs after every file of
//! the batch has had the first two applied — a file's siblings have to load.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, Stmt, StmtKind};
use crate::env::Env;
use crate::module::is_relative_import;
use crate::program::{Program, TermOp};
use crate::source_map::ENTRY_FILE;

/// What one pass over one file did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fixed {
    /// The file's new text (equal to the input when nothing changed).
    pub source: String,
    /// One line per edit, for a human.
    pub changes: Vec<String>,
    /// Things worth saying that are not edits: a name several packages
    /// export, a file that does not compile.
    pub notes: Vec<String>,
}

/// Rewrites imports against what one [`Env`] can resolve.
pub struct ImportFixer {
    env: Env,
    /// Directories old-style code found loose files in (`-I`).
    include_dirs: Vec<PathBuf>,
    /// Bare name → the import paths that export it. Built on first use.
    index: Option<HashMap<String, Vec<String>>>,
    /// Import paths whose exports could not be read while building `index`,
    /// each with the first line of why.
    unreadable: Vec<String>,
    /// Import path → every module importing it loads, itself included.
    loads: HashMap<String, HashSet<String>>,
    /// The project (manifest root) `index` was built for: a project's
    /// `[dependencies]` are importable only from its own files, so the index
    /// is rebuilt when a file of another project comes along.
    index_project: Option<PathBuf>,
}

/// The opt-out comment: a file carrying `// petal-fix-imports-ignore` on a
/// line of its own has its import lines rewritten like any other, but gets no
/// import *added* for a name nothing in it provides.
pub const IGNORE_FILE: &str = "petal-fix-imports-ignore";

impl ImportFixer {
    /// A fixer that knows the bundled packages and the packages under
    /// `include_dirs`.
    pub fn new(include_dirs: &[PathBuf]) -> Self {
        let mut env = Env::new();
        for dir in include_dirs {
            env.add_module_path(dir.clone());
        }
        Self::with_env(env, include_dirs)
    }

    /// A fixer over a host's own `Env` — one that has its interface packages
    /// registered, so their names can be imported.
    pub fn with_env(mut env: Env, include_dirs: &[PathBuf]) -> Self {
        env.set_check_only(true);
        Self {
            env,
            include_dirs: include_dirs.to_vec(),
            index: None,
            unreadable: Vec::new(),
            loads: HashMap::new(),
            index_project: None,
        }
    }

    /// Rewrites 1 and 2: the import lines of `source`, which lives at `file`
    /// (`None` for text with no path, where only rewrite 2 applies). `Err`
    /// when the text does not parse.
    pub fn rewrite_import_lines(&self, source: &str, file: Option<&Path>) -> Result<Fixed, String> {
        let (_, stmts) = crate::cst::parse_source(source, ENTRY_FILE)?;
        let chars: Vec<char> = source.chars().collect();
        // (start, end, replacement) in char offsets, in source order.
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        let mut changes = Vec::new();
        // Qualifiers a selective import used to bind and no longer does,
        // that the file still reaches through.
        let lost = lost_qualifiers(&stmts);
        let mut restored: HashSet<&str> = HashSet::new();
        for stmt in &stmts {
            let StmtKind::Import(decl) = &stmt.kind else {
                continue;
            };
            let Some(site) = ImportSite::find(&chars, stmt) else {
                continue;
            };
            let line = stmt.span.start.line;
            let new_path = file.and_then(|f| self.relative_spelling(&decl.module, f));
            let qualifier = import_qualifier(decl);
            if decl.names.is_some() && lost.contains(qualifier) && restored.insert(qualifier) {
                let path = new_path.as_deref().unwrap_or(&decl.module);
                let alias = if qualifier == crate::ast::module_local_name(&decl.module) {
                    String::new()
                } else {
                    format!(" as {qualifier}")
                };
                changes.push(format!(
                    "line {line}: added `import {path}{alias}` — `{qualifier}.` is used, and a \
                     selective import no longer binds it"
                ));
                let end = (stmt.span.end.offset as usize).min(chars.len());
                edits.push((end, end, format!("\nimport {path}{alias}")));
            }
            if let Some(new_path) = new_path {
                changes.push(format!(
                    "line {line}: `import {}` -> `import {new_path}`",
                    decl.module
                ));
                edits.push((site.path.0, site.path.1, new_path));
            }
            if decl.star
                && let Some((start, end)) = site.star
            {
                changes.push(format!(
                    "line {line}: `import {m}: *` -> `import {m}`",
                    m = decl.module
                ));
                edits.push((start, end, String::new()));
            }
        }
        Ok(Fixed {
            source: apply_edits(&chars, edits),
            changes,
            notes: Vec::new(),
        })
    }

    /// The relative path `import <path>` in `file` should be written as, when
    /// it is a bare path that names no package but does name a file.
    fn relative_spelling(&self, path: &str, file: &Path) -> Option<String> {
        if is_relative_import(path) {
            return None;
        }
        if self.env.resolve_import(path, file).is_ok() {
            return None;
        }
        let dir = file.parent()?;
        if module_file(dir, path).is_file() {
            return Some(format!("./{path}"));
        }
        let dir = crate::module::canonical_path(dir);
        self.include_dirs.iter().find_map(|include| {
            let target = module_file(include, path);
            target.is_file().then(|| {
                let rel = relative_path(&dir, &crate::module::canonical_path(&target));
                if rel.starts_with("../") {
                    rel
                } else {
                    format!("./{rel}")
                }
            })
        })
    }

    /// Make every later compile see `source` as the contents of `file`,
    /// whatever is on disk. A batch registers each file's rewritten text
    /// before [`add_missing_imports`](Self::add_missing_imports) runs, so the
    /// result does not depend on what has been written yet.
    pub fn stage(&mut self, file: &Path, source: &str) {
        self.env.override_file_source(file, source);
    }

    /// Rewrite 3: add the imports `source` needs for the bare names nothing
    /// in it provides. Never fails: a file that does not compile, or whose
    /// additions would not compile, comes back unchanged with a note.
    pub fn add_missing_imports(&mut self, source: &str, file: Option<&Path>) -> Fixed {
        let unchanged = |notes: Vec<String>| Fixed {
            source: source.to_string(),
            changes: Vec::new(),
            notes,
        };
        let program = match self.env.compile_program_diag(crate::program::ProgramId(0), source, file) {
            Ok(program) => program,
            Err(e) => {
                let first = e.to_string().lines().next().unwrap_or_default().to_string();
                return unchanged(vec![format!(
                    "does not compile, so no missing imports were looked for: {first}"
                )]);
            }
        };
        // `// petal-fix-imports-ignore` on a line of its own: the file names
        // things no package is meant to provide (a shader's environment, a
        // host this tool cannot see), so a package that happens to export
        // one of them is not what it meant.
        if source.lines().any(|line| {
            line.trim()
                .strip_prefix("//")
                .is_some_and(|rest| rest.trim() == IGNORE_FILE)
        }) {
            return unchanged(Vec::new());
        }
        let missing = unresolved_names(&program);
        // What the file already imports is never added again, and a name a
        // module the file imports would provide is not this rewrite's job.
        let already: HashSet<String> = program
            .imports
            .entry
            .iter()
            .map(|i| i.module.clone())
            .collect();
        let mut add: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut notes = Vec::new();
        // `x.name()` used to fall back to any builtin `name(x)`; for a
        // builtin that now lives in a package it does so only where the
        // package is imported, and nothing fails until the line runs.
        for (name, package) in builtin_method_calls(source, &program) {
            if already.contains(package) {
                continue;
            }
            add.entry(package.to_string())
                .or_default()
                .push(format!(".{name}()"));
        }
        if missing.is_empty() && add.is_empty() {
            return unchanged(Vec::new());
        }
        self.export_index(file);
        let index = self.index.as_ref().expect("just built");
        let mut unprovided = false;
        // Names with one provider first: the import one of them adds may be
        // what a name with several providers is waiting for.
        let mut contested: Vec<(&String, &[String])> = Vec::new();
        for name in &missing {
            match index.get(name).map(Vec::as_slice) {
                Some([only]) if !already.contains(only) => {
                    add.entry(only.clone()).or_default().push(name.clone());
                }
                Some([_]) => {}
                None => unprovided = true,
                Some(several) => contested.push((name, several)),
            }
        }
        for (name, several) in contested {
            // One of the providers is on its way in already.
            if several.iter().any(|path| add.contains_key(path)) {
                continue;
            }
            // Several packages export the name, but all the others are built
            // on one of them (bloom's `button` over ui's): the file had no
            // import of the others, so the base is the one it meant.
            let base = several.iter().find(|base| {
                several.iter().all(|other| {
                    other == *base
                        || self
                            .loads
                            .get(other)
                            .is_some_and(|loaded| loaded.contains(*base))
                })
            });
            match base {
                Some(base) if !already.contains(base) => {
                    add.entry(base.clone()).or_default().push(name.clone());
                }
                Some(_) => {}
                None => notes.push(format!(
                    "`{name}` is exported by {} — no import added; write the one you mean",
                    several.join(", ")
                )),
            }
        }
        // A package that would not load may be the one a still-missing name
        // is waiting for.
        if unprovided {
            notes.extend(self.unreadable.iter().map(|problem| {
                format!("{problem} — its exports were not considered")
            }));
        }
        if add.is_empty() {
            return unchanged(notes);
        }
        let lines: Vec<String> = add.keys().map(|path| format!("import {path}")).collect();
        let fixed = insert_import_lines(source, &lines);
        // The additions have to leave a program that still loads — an import
        // of the package a file is itself part of would be a cycle.
        if let Err(e) = self
            .env
            .compile_program_diag(crate::program::ProgramId(0), &fixed, file)
        {
            let first = e.to_string().lines().next().unwrap_or_default().to_string();
            notes.push(format!(
                "adding {} would not compile ({first}); left as it was",
                lines.join(", ")
            ));
            return unchanged(notes);
        }
        let changes = add
            .iter()
            .map(|(path, names)| {
                format!("added `import {path}` (for {})", some_names(names))
            })
            .collect();
        Fixed {
            source: fixed,
            changes,
            notes,
        }
    }

    /// All three rewrites on one file that stands alone.
    pub fn fix(&mut self, source: &str, file: Option<&Path>) -> Result<Fixed, String> {
        let mut first = self.rewrite_import_lines(source, file)?;
        if let Some(file) = file {
            self.stage(file, &first.source);
        }
        let second = self.add_missing_imports(&first.source, file);
        first.source = second.source;
        first.changes.extend(second.changes);
        first.notes.extend(second.notes);
        Ok(first)
    }

    /// Bare name → every import path that would bring it in. A package with a
    /// module of its own name is offered as `import <package>`; one without,
    /// module by module. `core` and `std` are in scope already and are left
    /// out.
    fn export_index(&mut self, file: Option<&Path>) {
        let project = file
            .and_then(|f| self.env.project_of(f).ok().flatten())
            .map(|p| p.root);
        if self.index.is_some() && self.index_project != project {
            self.index = None;
            self.loads.clear();
            self.unreadable.clear();
        }
        if self.index.is_none() {
            self.index_project = project;
            let mut index: HashMap<String, Vec<String>> = HashMap::new();
            let paths = match file {
                Some(file) => self.env.importable_paths_at(file),
                None => self.env.importable_paths(),
            };
            for path in paths {
                let exports = match self.env.exports_and_modules_of_at(&path, file) {
                    Ok((exports, loaded)) => {
                        self.loads.insert(path.clone(), loaded.into_iter().collect());
                        exports
                    }
                    Err(e) => {
                        let first = e.to_string().lines().next().unwrap_or_default().to_string();
                        self.unreadable
                            .push(format!("`import {path}` does not load ({first})"));
                        continue;
                    }
                };
                for name in exports {
                    index.entry(name).or_default().push(path.clone());
                }
            }
            for paths in index.values_mut() {
                paths.sort();
                paths.dedup();
            }
            self.index = Some(index);
        }
    }
}

/// The bare names the entry file of `program` calls or reads that nothing
/// defines: not a declaration, not an import, not a `core`/`std` builtin.
pub fn unresolved_names(program: &Program) -> BTreeSet<String> {
    use crate::typecheck::globals::{UNDEFINED_VARIABLE_PREFIX, UNKNOWN_FUNCTION_PREFIX};
    let mut out = BTreeSet::new();
    for term in &program.terms {
        // Only what the file itself wrote: a module's own gaps are that
        // module's to fix.
        if program
            .source_map
            .get(term.id)
            .is_none_or(|span| span.file != ENTRY_FILE)
        {
            continue;
        }
        let TermOp::Error(cid) = term.op else {
            continue;
        };
        if let Some(rest) = program.get_string_constant(cid).and_then(|m| {
            m.strip_prefix(UNKNOWN_FUNCTION_PREFIX)
                .or_else(|| m.strip_prefix(UNDEFINED_VARIABLE_PREFIX))
        }) {
            out.insert(rest.split(' ').next().unwrap_or(rest).to_string());
        }
    }
    out
}

/// Method calls in the entry file that can only mean a domain builtin
/// (`v.normalize()` → the `vec2` package's `normalize`): `(method, package)`,
/// sorted, one per method name.
///
/// "Can only mean" is a judgement made from the outside, so it is a careful
/// one: the name must be declared by a bundled builtin package, not be bound
/// in the file already (then the call reaches it today), and nothing in the
/// program may define something else it could be — a function or a class
/// method of that name exported by a module the file imports, or a `fn`, a
/// class method or a record field of that name written in the file itself.
fn builtin_method_calls(source: &str, program: &Program) -> Vec<(String, &'static str)> {
    use crate::ast::{Expr, ExprKind, ExprVisitor, walk_expr};
    #[derive(Default)]
    struct Scan {
        methods: BTreeSet<String>,
        /// Names the file gives a meaning of its own: record fields,
        /// functions and class methods.
        own: HashSet<String>,
    }
    impl ExprVisitor for Scan {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Call { function, .. } => {
                    if let ExprKind::FieldAccess { field, .. } = &function.kind {
                        self.methods.insert(field.clone());
                    }
                }
                ExprKind::Record(fields) => {
                    for field in fields {
                        if let crate::ast::RecordField::Named(name, _) = field {
                            self.own.insert(name.clone());
                        }
                    }
                }
                _ => {}
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::FnDecl { name, .. } = &s.kind {
                let method = name.rsplit('.').next().unwrap_or(name);
                self.own.insert(method.to_string());
            }
            crate::ast::walk_stmt(self, s);
        }
    }
    let Ok((_, stmts)) = crate::cst::parse_source(source, ENTRY_FILE) else {
        return Vec::new();
    };
    let mut scan = Scan::default();
    for stmt in &stmts {
        scan.visit_stmt(stmt);
    }
    // What the modules the program loads export, bare or as `Class.method`.
    let exported: HashSet<&str> = program
        .imports
        .exports
        .iter()
        .filter(|(module, _)| {
            crate::native_decl::builtin_package_of_module(module).is_none()
        })
        .flat_map(|(_, names)| names.iter())
        .map(|name| name.rsplit('.').next().unwrap_or(name))
        .collect();
    let unresolved = unresolved_names(program);
    scan.methods
        .iter()
        .filter(|name| !scan.own.contains(*name) && !exported.contains(name.as_str()))
        // A name the file also uses bare and unresolved is handled, with its
        // import, by the bare-name rewrite.
        .filter(|name| !unresolved.contains(*name))
        .filter_map(|name| {
            let package = crate::native_decl::builtin_package_of(name)?;
            Some((name.clone(), package))
        })
        .collect()
}

/// The qualifier an import statement binds — or, for a selective import,
/// used to bind.
fn import_qualifier(decl: &ImportDecl) -> &str {
    decl.alias
        .as_deref()
        .unwrap_or_else(|| crate::ast::module_local_name(&decl.module))
}

/// The qualifiers of the file's selective imports that the file uses as
/// `q.member` while nothing else — a plain import, a declaration, a
/// parameter — binds `q`. Under the old rules a selective import bound its
/// module's qualifier too.
fn lost_qualifiers(stmts: &[Stmt]) -> HashSet<String> {
    use crate::ast::{Expr, ExprKind, ExprVisitor, walk_expr, walk_stmt};
    #[derive(Default)]
    struct Scan {
        /// Names used as the object of a field access or method call.
        reached_through: HashSet<String>,
        /// Names the file binds as values, anywhere.
        bound: HashSet<String>,
    }
    impl ExprVisitor for Scan {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::FieldAccess { object, .. } => {
                    if let ExprKind::Ident(name) = &object.kind {
                        self.reached_through.insert(name.clone());
                    }
                }
                ExprKind::Lambda { params, .. } => {
                    self.bound.extend(params.iter().map(|p| p.name.clone()));
                }
                ExprKind::For { var, .. } => {
                    self.bound.insert(var.clone());
                }
                _ => {}
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. }
                | StmtKind::State { name, .. }
                | StmtKind::ClassDecl { name, .. } => {
                    self.bound.insert(name.clone());
                }
                StmtKind::For { var, .. } => {
                    self.bound.insert(var.clone());
                }
                StmtKind::FnDecl { name, params, .. } => {
                    self.bound.insert(name.clone());
                    self.bound.extend(params.iter().map(|p| p.name.clone()));
                }
                StmtKind::Import(decl) => {
                    // A selectively imported *name* is a value binding.
                    self.bound.extend(decl.names.iter().flatten().cloned());
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut scan = Scan::default();
    for stmt in stmts {
        scan.visit_stmt(stmt);
    }
    let imports = || {
        stmts.iter().filter_map(|s| match &s.kind {
            StmtKind::Import(decl) => Some(decl),
            _ => None,
        })
    };
    let plain: HashSet<&str> = imports()
        .filter(|d| d.names.is_none())
        .map(import_qualifier)
        .collect();
    imports()
        .filter(|d| d.names.is_some())
        .map(import_qualifier)
        .filter(|q| {
            scan.reached_through.contains(*q) && !scan.bound.contains(*q) && !plain.contains(q)
        })
        .map(str::to_string)
        .collect()
}

/// Where the pieces of one import statement sit in the file, in char
/// offsets.
struct ImportSite {
    /// The module path.
    path: (usize, usize),
    /// From the `:` through the `*` of a star import.
    star: Option<(usize, usize)>,
}

impl ImportSite {
    fn find(chars: &[char], stmt: &Stmt) -> Option<ImportSite> {
        let StmtKind::Import(ImportDecl { star, .. }) = &stmt.kind else {
            return None;
        };
        let (start, end) = (
            stmt.span.start.offset as usize,
            (stmt.span.end.offset as usize).min(chars.len()),
        );
        // `import` is the first word, or the second after `pub` / `export`.
        let keyword: Vec<char> = "import".chars().collect();
        let at = (start..end.saturating_sub(keyword.len() - 1)).find(|&i| {
            chars[i..].starts_with(&keyword)
                && (i == start || chars[i - 1].is_whitespace())
                && chars.get(i + keyword.len()).is_some_and(|c| c.is_whitespace())
        })?;
        let mut i = at + keyword.len();
        while i < end && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        let path_start = i;
        while i < chars.len()
            && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '/' | '.'))
        {
            i += 1;
        }
        let path_end = i;
        if path_end == path_start {
            return None;
        }
        let star = if *star {
            // `: *`, with whatever blanks the author put around the colon.
            let mut j = path_end;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
                j += 1;
            }
            if chars.get(j) == Some(&':') {
                let mut k = j + 1;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                (chars.get(k) == Some(&'*')).then_some((path_end, k + 1))
            } else {
                None
            }
        } else {
            None
        };
        Some(ImportSite {
            path: (path_start, path_end),
            star,
        })
    }
}

/// Apply non-overlapping `(start, end, text)` replacements, given in any
/// order.
fn apply_edits(chars: &[char], mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| e.0);
    let mut out = String::with_capacity(chars.len());
    let mut at = 0;
    for (start, end, text) in edits {
        out.extend(&chars[at..start]);
        out.push_str(&text);
        at = end;
    }
    out.extend(&chars[at..]);
    out
}

/// `source` with `lines` added to its imports: after the last import it
/// already has, else ahead of the first statement — below a leading comment
/// block that a blank line sets apart from the code, since that is a file
/// header and not a comment on the import.
fn insert_import_lines(source: &str, lines: &[String]) -> String {
    let block = lines.join("\n");
    let stmts = crate::cst::parse_source(source, ENTRY_FILE)
        .map(|(_, stmts)| stmts)
        .unwrap_or_default();
    let last_import = stmts
        .iter()
        .filter(|s| matches!(s.kind, StmtKind::Import(_)))
        .map(|s| s.span.end.line as usize)
        .max();
    let src_lines: Vec<&str> = source.split('\n').collect();
    let (at, blank_after) = match last_import {
        // `end.line` is 1-based, so it is also the index of the next line.
        Some(line) => (line.min(src_lines.len()), false),
        None => {
            let mut i = 0;
            while i < src_lines.len() && src_lines[i].trim_start().starts_with("//") {
                i += 1;
            }
            if i > 0 && i < src_lines.len() && src_lines[i].trim().is_empty() {
                (i + 1, true)
            } else {
                (0, true)
            }
        }
    };
    let mut out: Vec<String> = src_lines[..at].iter().map(|l| l.to_string()).collect();
    out.push(block);
    if blank_after && src_lines.get(at).is_some_and(|l| !l.trim().is_empty()) {
        out.push(String::new());
    }
    out.extend(src_lines[at..].iter().map(|l| l.to_string()));
    out.join("\n")
}

/// `a, b, c` / `a, b, c, +N more`.
fn some_names(names: &[String]) -> String {
    let mut out: Vec<String> = names.iter().take(4).map(|n| format!("`{n}`")).collect();
    if names.len() > 4 {
        out.push(format!("+{} more", names.len() - 4));
    }
    out.join(", ")
}

/// `<dir>/a/b.ptl` for the path `a/b`.
fn module_file(dir: &Path, path: &str) -> PathBuf {
    let mut out = dir.to_path_buf();
    for segment in path.split('/') {
        out.push(segment);
    }
    out.set_extension("ptl");
    out
}

/// `target` (a `.ptl` file) relative to the directory `base`, `/`-joined and
/// without the suffix. Both canonical.
fn relative_path(base: &Path, target: &Path) -> String {
    let target = target.with_extension("");
    let base_parts: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();
    let common = base_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = vec!["..".to_string(); base_parts.len() - common];
    parts.extend(
        target_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory holding `files`, removed on drop.
    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str, files: &[(&str, &str)]) -> Dir {
            let root = std::env::temp_dir().join(format!(
                "petal-fix-imports-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            for (path, text) in files {
                let path = root.join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            Dir(crate::module::canonical_path(&root))
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn lines(fixer: &ImportFixer, source: &str, file: &Path) -> String {
        fixer.rewrite_import_lines(source, Some(file)).unwrap().source
    }

    #[test]
    fn a_sibling_bare_import_becomes_relative_in_every_form() {
        let dir = Dir::new(
            "sibling",
            &[("level.ptl", "pub let a = 1\n"), ("ui/hud.ptl", "pub let b = 1\n")],
        );
        let fixer = ImportFixer::new(&[]);
        let main = dir.path("main.ptl");
        assert_eq!(
            lines(
                &fixer,
                "// header\nimport level\nimport level as l   // why\npub import level: a\n\
                 import ui/hud: b,\n  b\nprint(a)\n",
                &main
            ),
            "// header\nimport ./level\nimport ./level as l   // why\npub import ./level: a\n\
             import ./ui/hud: b,\n  b\nprint(a)\n"
        );
    }

    #[test]
    fn a_star_import_becomes_a_plain_one() {
        let dir = Dir::new("star", &[("level.ptl", "pub let a = 1\n")]);
        let fixer = ImportFixer::new(&[]);
        assert_eq!(
            lines(
                &fixer,
                "import level: *\npub import level : *  // all\nexport import ./level:*\n",
                &dir.path("main.ptl")
            ),
            "import ./level\npub import ./level  // all\nexport import ./level\n"
        );
        // With no file there is nothing to resolve against, but the star goes.
        assert_eq!(
            fixer.rewrite_import_lines("import m: *\n", None).unwrap().source,
            "import m\n"
        );
    }

    #[test]
    fn a_selective_import_whose_qualifier_is_still_used_gains_a_plain_import() {
        let dir = Dir::new("lost-qualifier", &[("tally.ptl", "pub let hits = 1\npub let n = 2\n")]);
        let fixer = ImportFixer::new(&[]);
        let main = dir.path("main.ptl");
        // `tally.n` only ever worked because the selective import also bound
        // the qualifier.
        let once = lines(&fixer, "import tally: hits\nprint(hits + tally.n)\n", &main);
        assert_eq!(once, "import ./tally: hits\nimport ./tally\nprint(hits + tally.n)\n");
        assert_eq!(lines(&fixer, &once, &main), once);
        // Not when the name is a value of the file's own, or a plain import
        // already binds it, or nothing reaches through it.
        for src in [
            "import ./tally: hits\nlet tally = {n: 1}\nprint(hits + tally.n)\n",
            "import ./tally: hits\nfn f(tally)\n  tally.n\nend\nprint(hits)\n",
            "import ./tally: hits\nimport ./tally\nprint(hits + tally.n)\n",
            "import ./tally: hits\nprint(hits)\n",
        ] {
            assert_eq!(lines(&fixer, src, &main), src);
        }
        // A wrapped list: the new line goes after the whole statement.
        assert_eq!(
            lines(&fixer, "import ./tally: hits,\n  n\nprint(tally.n)\n", &main),
            "import ./tally: hits,\n  n\nimport ./tally\nprint(tally.n)\n"
        );
    }

    #[test]
    fn a_package_path_and_an_unknown_name_are_left_alone() {
        let dir = Dir::new(
            "package",
            &[
                ("libs/kit/petal.toml", "[package]\nname = \"kit\"\n"),
                ("libs/kit/src/kit.ptl", "pub let k = 1\n"),
                ("libs/kit/src/menu.ptl", "import motion\npub let m = 1\n"),
                ("libs/kit/src/motion.ptl", "pub let e = 1\n"),
                // A sibling that shares the package's name loses to it.
                ("app/kit.ptl", "pub let nope = 1\n"),
            ],
        );
        let fixer = ImportFixer::new(&[dir.path("libs")]);
        let src = "import kit\nimport kit/menu\nimport core/math\nimport host_thing\n";
        assert_eq!(lines(&fixer, src, &dir.path("app/main.ptl")), src);
        // Inside the package, the flat sibling import is what gets rewritten;
        // the package's own name keeps working.
        assert_eq!(
            lines(
                &fixer,
                "import motion\nimport kit/motion as km\n",
                &dir.path("libs/kit/src/menu.ptl")
            ),
            "import ./motion\nimport kit/motion as km\n"
        );
    }

    #[test]
    fn a_loose_file_on_an_include_dir_becomes_the_relative_path_to_it() {
        let dir = Dir::new(
            "include",
            &[("shared/rng.ptl", "pub let r = 1\n"), ("game/main.ptl", "")],
        );
        let fixer = ImportFixer::new(&[dir.path("shared")]);
        assert_eq!(
            lines(&fixer, "import rng\n", &dir.path("game/main.ptl")),
            "import ../shared/rng\n"
        );
    }

    #[test]
    fn it_is_idempotent_and_leaves_migrated_files_alone() {
        let dir = Dir::new(
            "idem",
            &[("level.ptl", "pub fn go()\n  1\nend\n"), ("main.ptl", "")],
        );
        let mut fixer = ImportFixer::new(&[]);
        let main = dir.path("main.ptl");
        let once = fixer.fix("import level: *\nprint(go())\n", Some(&main)).unwrap();
        assert_eq!(once.source, "import ./level\nprint(go())\n");
        assert_eq!(once.changes.len(), 2, "{:?}", once.changes);
        let twice = fixer.fix(&once.source, Some(&main)).unwrap();
        assert_eq!(twice.source, once.source);
        assert!(twice.changes.is_empty(), "{:?}", twice.changes);
    }

    /// A fixer whose `Env` has the package `kit` (one module, `kit`) and the
    /// package `parts` (no module of its own name).
    fn fixer_with_packages() -> ImportFixer {
        let mut env = Env::new();
        env.register_package(
            "kit",
            [("kit", "pub fn knob(x)\n  x\nend\npub let dial = 2\npub fn shared()\n  1\nend\n")],
        )
        .unwrap();
        env.register_package(
            "parts",
            [
                ("gear", "pub fn teeth()\n  8\nend\n"),
                ("cog", "pub fn shared()\n  2\nend\n"),
            ],
        )
        .unwrap();
        ImportFixer::with_env(env, &[])
    }

    #[test]
    fn a_name_one_package_exports_gets_its_import() {
        let mut fixer = fixer_with_packages();
        let fixed = fixer.add_missing_imports("print(knob(dial))\nprint(teeth())\n", None);
        assert_eq!(
            fixed.source,
            "import kit\nimport parts/gear\n\nprint(knob(dial))\nprint(teeth())\n"
        );
        assert_eq!(fixed.changes.len(), 2, "{:?}", fixed.changes);
        // Again: nothing left to add.
        let again = fixer.add_missing_imports(&fixed.source, None);
        assert_eq!(again.source, fixed.source);
        assert!(again.changes.is_empty());
    }

    // ---- builtins that moved into packages ------------------------------

    #[test]
    fn a_moved_builtin_gets_the_import_of_its_package() {
        let mut fixer = ImportFixer::new(&[]);
        let src = "print(lerp(0, 10, 0.5))\nlet v = vec3(1, 2, 3)\nprint(noise(0.5), hsv(0, 1, 1))\n\
                   print(json_stringify(dual(1, 1)))\npush_output(symbol(\"out\"), 1)\nprint(clamp(5, 0, 1))\n";
        let fixed = fixer.add_missing_imports(src, None);
        assert_eq!(
            fixed.source,
            format!(
                "import autodiff\nimport color\nimport creative\nimport io\nimport json\n\
                 import noise\nimport vec2\n\n{src}"
            )
        );
        // Idempotent, and a file that already imports is left alone.
        let again = fixer.add_missing_imports(&fixed.source, None);
        assert_eq!(again.source, fixed.source);
        assert!(again.changes.is_empty(), "{:?}", again.changes);
    }

    #[test]
    fn a_name_the_file_defines_is_not_a_moved_builtin() {
        let mut fixer = ImportFixer::new(&[]);
        let src = "fn lerp(a, b)\n  a + b\nend\nprint(lerp(1, 2))\n";
        let fixed = fixer.add_missing_imports(src, None);
        assert_eq!(fixed.source, src);
    }

    #[test]
    fn a_method_call_that_can_only_be_a_moved_builtin_gets_the_import() {
        let mut fixer = ImportFixer::new(&[]);
        // Nothing fails to compile here: `v.mag()` only fails when it runs.
        let src = "let v = [3, 4]\nprint(v.mag())\n";
        let fixed = fixer.add_missing_imports(src, None);
        assert_eq!(fixed.source, format!("import creative\n\n{src}"));
        assert_eq!(fixed.changes, ["added `import creative` (for `.mag()`)"]);
        assert!(fixer.add_missing_imports(&fixed.source, None).changes.is_empty());
    }

    #[test]
    fn a_method_call_something_else_may_answer_adds_nothing() {
        let mut fixer = ImportFixer::new(&[]);
        // A record field, a function and a class method of the name.
        for src in [
            "let r = {limit: fn(x) x end}\nprint(r.limit(1))\n",
            "fn rotate(shape, by)\n  shape\nend\nprint([1].rotate(2))\n",
            "class Body\n  x: num,\nend\nfn Body.normalize(b)\n  b\nend\nprint(Body(1).normalize())\n",
        ] {
            assert_eq!(fixer.add_missing_imports(src, None).source, src, "{src}");
        }
        // … or a module the file imports exports it.
        let mut env = Env::new();
        env.register_package("phys", [("phys", "pub fn limit(v, m)\n  v\nend\n")])
            .unwrap();
        let mut fixer = ImportFixer::with_env(env, &[]);
        let src = "import phys\nprint([1].limit(2))\n";
        assert_eq!(fixer.add_missing_imports(src, None).source, src);
    }

    #[test]
    fn the_import_goes_after_the_ones_already_there_and_below_a_header() {
        let mut fixer = fixer_with_packages();
        assert_eq!(
            fixer
                .add_missing_imports("import parts/gear\nprint(knob(teeth()))\n", None)
                .source,
            "import parts/gear\nimport kit\nprint(knob(teeth()))\n"
        );
        assert_eq!(
            fixer
                .add_missing_imports("// A header.\n// More.\n\nprint(knob(1))\n", None)
                .source,
            "// A header.\n// More.\n\nimport kit\n\nprint(knob(1))\n"
        );
        // A comment that sits on the first statement stays on it.
        assert_eq!(
            fixer.add_missing_imports("// about x\nlet x = knob(1)\n", None).source,
            "import kit\n\n// about x\nlet x = knob(1)\n"
        );
    }

    #[test]
    fn a_name_the_file_provides_or_two_packages_export_adds_nothing() {
        let mut fixer = fixer_with_packages();
        // Declared here, a builtin, and a plain undefined name.
        for src in [
            "fn knob(x)\n  x\nend\nprint(knob(1))\n",
            "print(len([1]))\n",
            "print(nothing_has_this(1))\n",
        ] {
            let fixed = fixer.add_missing_imports(src, None);
            assert_eq!(fixed.source, src);
            assert!(fixed.notes.is_empty(), "{:?}", fixed.notes);
        }
        let fixed = fixer.add_missing_imports("print(shared())\n", None);
        assert_eq!(fixed.source, "print(shared())\n");
        assert_eq!(fixed.notes.len(), 1);
        assert!(
            fixed.notes[0].contains("`shared` is exported by kit, parts/cog"),
            "{:?}",
            fixed.notes
        );
    }

    #[test]
    fn a_package_that_does_not_load_is_named_when_a_name_is_still_missing() {
        let mut env = Env::new();
        env.register_package("old", [("old", "pub import nowhere: *\n")]).unwrap();
        let mut fixer = ImportFixer::with_env(env, &[]);
        let fixed = fixer.add_missing_imports("print(knob(1))\n", None);
        assert_eq!(fixed.source, "print(knob(1))\n");
        assert_eq!(fixed.notes.len(), 1, "{:?}", fixed.notes);
        assert!(
            fixed.notes[0].contains("`import old` does not load"),
            "{:?}",
            fixed.notes
        );
        // Nothing missing, nothing to say.
        assert!(fixer.add_missing_imports("print(1)\n", None).notes.is_empty());
    }

    #[test]
    fn a_file_that_does_not_compile_is_reported_not_edited() {
        let mut fixer = fixer_with_packages();
        let src = "import nowhere\nprint(knob(1))\n";
        let fixed = fixer.add_missing_imports(src, None);
        assert_eq!(fixed.source, src);
        assert!(fixed.notes[0].contains("does not compile"), "{:?}", fixed.notes);
    }
    #[test]
    fn the_ignore_comment_keeps_added_imports_out_of_a_file() {
        let mut fixer = ImportFixer::new(&[]);
        // `clear` is exported by `ui`, but this file says not to guess.
        let src = "// petal-fix-imports-ignore\nlet f = fn() clear(0, 0, 0) end\n";
        let fixed = fixer.fix(src, None).unwrap();
        assert_eq!(fixed.source, src);
        assert!(fixed.changes.is_empty(), "{:?}", fixed.changes);
        // Without the comment the import is added.
        let fixed = fixer.fix("let f = fn() clear(0, 0, 0) end\n", None).unwrap();
        assert!(fixed.source.starts_with("import ui\n"), "{}", fixed.source);
    }
}
