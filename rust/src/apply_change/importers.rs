//! Which other files import a module-level binding of the target file.
//!
//! A change to an exported binding has to follow it into its importers, and
//! nothing in the language says where those are: an import names a module,
//! and the resolver ([`crate::module`]) decides which file that is. So this
//! works the other way round. It gathers candidate files, resolves each of
//! their imports the way a compile would, and keeps the ones that land on
//! the target file.
//!
//! Candidates come from one of two places ([`discover`]):
//!
//! - **`--from <file>`** (repeatable): the entry files of the programs that
//!   use the module. Each one's import graph is followed, and every file in
//!   it is a candidate. This is the same flag, with the same meaning, as
//!   `petal suggest --from`.
//! - **A directory scan**, when no `--from` is given: every `.ptl` file under
//!   the target's *project root* — the nearest directory at or above the
//!   target that holds a `petal.toml` ([`crate::package`]), or the target's
//!   own directory when there is none.
//!
//! A facade is followed too: a file that re-exports the name
//! (`pub import tally: hits`, `pub import tally: *`) hands it on to its own
//! importers, so they are importers of the binding as well.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{ImportDecl, Stmt, StmtKind};
use crate::env::Env;
use crate::module::canonical_path;

/// One file that imports the target binding's module.
pub(super) struct Importer {
    pub path: PathBuf,
    pub source: String,
    pub stmts: Vec<Stmt>,
    /// The import(s) that reach the binding, as written.
    pub form: String,
    /// Whether the import binds the name bare in this file (`import m: x`,
    /// `import m: *`). A qualified import reaches it only as `m.x`.
    pub binds_bare: bool,
    /// The names the module itself is bound under here (`import m` → `m`,
    /// `import m as u` → `u`), through which the binding is `m.x`.
    pub aliases: Vec<String>,
}

pub(super) struct Discovered {
    /// How the candidates were gathered, for the report.
    pub how: String,
    pub importers: Vec<Importer>,
    /// Candidates that could not be read or parsed, and the like.
    pub notes: Vec<String>,
}

/// Find the files that import `name` from the file at `target`.
pub(super) fn discover(env: &Env, target: &Path, name: &str, from: &[PathBuf]) -> Discovered {
    let target = canonical_path(target);
    let mut files = Files::default();
    let mut notes = Vec::new();

    let (how, candidates) = if from.is_empty() {
        let root = scan_root(&target);
        let found = ptl_files_under(&root);
        (
            format!(
                "scanned {} ({} .ptl file{})",
                root.display(),
                found.len(),
                if found.len() == 1 { "" } else { "s" }
            ),
            found.iter().map(|p| canonical_path(p)).collect::<Vec<_>>(),
        )
    } else {
        let graph = follow_imports(env, from, &mut files, &mut notes);
        if !graph.contains(&target) {
            notes.push(format!(
                "{} is not imported by any --from entry",
                target.display()
            ));
        }
        (
            format!(
                "followed the imports of {}",
                from.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            graph,
        )
    };

    // The files that export the name: the target, then every facade found to
    // re-export it. A facade's own importers only show up once it is known to
    // be one, so iterate to a fixed point.
    let mut sources: HashSet<PathBuf> = HashSet::from([target.clone()]);
    let mut found: HashMap<PathBuf, (Vec<String>, bool, Vec<String>)> = HashMap::new();
    loop {
        let mut grew = false;
        for path in &candidates {
            if *path == target {
                continue;
            }
            let Some(file) = files.load(path, from.is_empty(), &mut notes) else {
                continue;
            };
            let mut forms = Vec::new();
            let mut bare = false;
            let mut aliases = Vec::new();
            let mut re_exports = false;
            for decl in imports(&file.stmts) {
                let Some(resolved) = env.resolve_module_file(&decl.module, path) else {
                    continue;
                };
                if !sources.contains(&canonical_path(&resolved)) {
                    continue;
                }
                let named = decl
                    .names
                    .as_ref()
                    .is_some_and(|n| n.iter().any(|n| n == name));
                // A star import binds weakly: a top-level declaration of the
                // same name anywhere in the file wins, and the star binds
                // nothing under that name.
                let starred = decl.star && !declares(&file.stmts, name);
                forms.push(import_text(decl));
                bare |= named || starred;
                // A qualified import binds the module, not the name. Only an
                // import of the target itself counts: a facade re-exports
                // names, and `facade.x` is not a spelling of the binding.
                if decl.names.is_none() && !decl.star && canonical_path(&resolved) == target {
                    let alias = decl
                        .alias
                        .as_deref()
                        .unwrap_or_else(|| crate::ast::module_local_name(&decl.module));
                    aliases.push(alias.to_string());
                }
                re_exports |= decl.exported && (named || decl.star);
            }
            if forms.is_empty() {
                continue;
            }
            found.insert(path.clone(), (forms, bare, aliases));
            if re_exports && sources.insert(path.clone()) {
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut importers: Vec<Importer> = found
        .into_iter()
        .filter_map(|(path, (forms, binds_bare, aliases))| {
            let file = files.take(&path)?;
            Some(Importer {
                path,
                source: file.source,
                stmts: file.stmts,
                form: forms.join("; "),
                binds_bare,
                aliases,
            })
        })
        .collect();
    importers.sort_by(|a, b| a.path.cmp(&b.path));
    Discovered {
        how,
        importers,
        notes,
    }
}

/// A candidate file, read and parsed once.
struct Parsed {
    source: String,
    stmts: Vec<Stmt>,
}

/// The candidates read so far. `None` records a file that could not be used,
/// so it is reported once.
#[derive(Default)]
struct Files {
    cache: HashMap<PathBuf, Option<Parsed>>,
}

impl Files {
    /// Read and parse `path`. With `quiet`, a file that does not parse is
    /// skipped without a note unless it mentions `import` at all — a scan
    /// passes over plenty of files that have nothing to do with the target.
    fn load(&mut self, path: &Path, quiet: bool, notes: &mut Vec<String>) -> Option<&Parsed> {
        if !self.cache.contains_key(path) {
            let parsed = match std::fs::read_to_string(path) {
                Err(e) => {
                    notes.push(format!("{}: skipped, {e}", path.display()));
                    None
                }
                // No import statement, no importer: skip the parse.
                Ok(source) if quiet && !source.contains("import") => None,
                Ok(source) => match crate::rewrite::parse_ast(&source) {
                    Ok((_tree, stmts)) => Some(Parsed { source, stmts }),
                    Err(e) => {
                        notes.push(format!(
                            "{}: skipped, it does not parse ({e})",
                            path.display()
                        ));
                        None
                    }
                },
            };
            self.cache.insert(path.to_path_buf(), parsed);
        }
        self.cache.get(path).and_then(Option::as_ref)
    }

    fn take(&mut self, path: &Path) -> Option<Parsed> {
        self.cache.remove(path).flatten()
    }
}

/// Every file reachable from `entries` by imports, the entries included.
fn follow_imports(
    env: &Env,
    entries: &[PathBuf],
    files: &mut Files,
    notes: &mut Vec<String>,
) -> Vec<PathBuf> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut queue: Vec<PathBuf> = entries.iter().rev().map(|p| canonical_path(p)).collect();
    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        order.push(path.clone());
        let Some(file) = files.load(&path, false, notes) else {
            continue;
        };
        for decl in imports(&file.stmts) {
            if let Some(next) = env.resolve_module_file(&decl.module, &path) {
                queue.push(canonical_path(&next));
            }
        }
    }
    order
}

fn imports(stmts: &[Stmt]) -> impl Iterator<Item = &ImportDecl> {
    stmts.iter().filter_map(|s| match &s.kind {
        StmtKind::Import(decl) => Some(decl),
        _ => None,
    })
}

/// Does the file declare `name` at its top level?
fn declares(stmts: &[Stmt], name: &str) -> bool {
    stmts.iter().any(|s| match &s.kind {
        StmtKind::Let { name: n, .. }
        | StmtKind::State { name: n, .. }
        | StmtKind::FnDecl { name: n, .. }
        | StmtKind::EnumDecl { name: n, .. }
        | StmtKind::ClassDecl { name: n, .. } => n == name,
        _ => false,
    })
}

/// An import as it is written, for the report.
fn import_text(decl: &ImportDecl) -> String {
    let mut out = String::new();
    if decl.exported {
        out.push_str("pub ");
    }
    out.push_str("import ");
    out.push_str(&decl.module);
    if let Some(names) = &decl.names {
        out.push_str(": ");
        out.push_str(&names.join(", "));
    } else if decl.star {
        out.push_str(": *");
    } else if let Some(alias) = &decl.alias
        && alias != crate::ast::module_local_name(&decl.module)
    {
        out.push_str(" as ");
        out.push_str(alias);
    }
    out
}

/// The directory a scan for importers covers: the nearest one at or above
/// `file` holding a package manifest, else the file's own directory.
pub(super) fn scan_root(file: &Path) -> PathBuf {
    let dir = match file.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut at = Some(dir.as_path());
    while let Some(d) = at {
        if d.join(crate::package::MANIFEST_FILE).is_file() {
            return d.to_path_buf();
        }
        at = d.parent();
    }
    dir
}

/// Every `.ptl` file under `dir`, sorted for stable output. Dot-directories,
/// `node_modules` and `target` are skipped.
pub fn ptl_files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() {
                if !name.starts_with('.') && name != "node_modules" && name != "target" {
                    walk(&path, out);
                }
            } else if name.ends_with(".ptl") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}
