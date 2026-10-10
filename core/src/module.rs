//! Module resolution and the import walk.
//!
//! Petal's module system merges all imported modules into a single `Program`
//! at compile time (see docs/module-system.md). This file owns the front half
//! of that pipeline: turning an entry source plus a resolver into the flat,
//! dependency-ordered list of parsed modules the compiler consumes.
//!
//! An import path is either *relative* — `./level`, `../shared/rng`, resolved
//! against the importing file — or starts with a *package name* (`ui`,
//! `bloom/menu`). A bare first segment is never a sibling file. The built-in
//! [`ModuleRegistry`] looks a package name up in this order:
//! 1. what the host registered in memory (`Env::register_module`, keyed by
//!    the full path, and `Env::register_package` / `Env::add_package`),
//! 2. the package the importing file belongs to — the nearest `petal.toml`
//!    above it — when the name is that package's own,
//! 3. packages found on the search path (`Env::add_module_path`, CLI `-I`,
//!    `PETAL_PATH`),
//! 4. packages bundled in the binary ([`crate::bundled`]).
//!
//! A module's *identity* is canonical, not the spelling an importer used:
//! `<package>/<path within it>` for a file under a package's module
//! directory, the registered name for an in-memory module, and the canonical
//! file path for a loose file (named, for diagnostics and state keys, by its
//! path relative to the entry file). `./motion` inside `bloom` and
//! `bloom/motion` from outside are one module with one set of state.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Expr, ExprKind, ExprVisitor, ImportDecl, Stmt, StmtKind, walk_expr};
use crate::error::{LoadError, Phase};
use crate::source_map::{ENTRY_FILE, FileId, SourceSpan};

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
    /// The module's canonical name (`bloom/menu`, `ui`), or `None` for a
    /// loose file — one that belongs to no package — which the loader names
    /// by its path relative to the entry file.
    pub name: Option<String>,
    pub source: String,
    pub origin: ModuleOrigin,
}

/// The file an import statement was written in.
#[derive(Clone, Copy)]
pub struct Importer<'a> {
    /// Its canonical module name; `None` for the entry file.
    pub name: Option<&'a str>,
    /// Where its source came from; `None` for an entry with no path.
    pub origin: Option<&'a ModuleOrigin>,
}

impl Importer<'_> {
    /// An entry file with no path: nothing to resolve a relative import
    /// against but the host's in-memory modules.
    pub const NONE: Importer<'static> = Importer {
        name: None,
        origin: None,
    };
}

/// Is `path` a relative import path (`./x`, `../x`)?
pub fn is_relative_import(path: &str) -> bool {
    path.starts_with("./") || path.starts_with("../")
}

/// Pluggable module resolution: given an import path as written and the file
/// that wrote it, produce source. [`ModuleRegistry`] is the built-in
/// implementation used by `Env`. An `Err` is the reason nothing was found,
/// ready to follow "cannot find module 'x' (imported by y): ".
pub trait ModuleResolver {
    fn resolve(&self, path: &str, importer: &Importer) -> Result<ModuleSource, String>;

    /// Where the loader may keep the parse of each source file between loads
    /// (see [`ParseCache`]). `None`, the default, parses everything every
    /// time.
    fn parse_cache(&self) -> Option<&ParseCache> {
        None
    }

    /// Called once at the start of every load, before the first `resolve`:
    /// the point to drop anything remembered about the filesystem.
    fn begin_load(&self) {}
}

/// The last parse of each source file a resolver's programs were loaded
/// from, so a reload re-parses only the files whose text changed.
///
/// A parse is a function of the source text and the file id stamped on its
/// spans, and nothing else; an entry is reused only when both are identical,
/// so a hit returns exactly what parsing again would. One entry is kept per
/// file (keyed by the module's name and file id), replaced when its text
/// changes. Parsing is about half of what compiling a program costs, and an
/// edit usually touches one file of many.
#[derive(Default)]
pub struct ParseCache {
    #[allow(clippy::type_complexity)]
    entries: std::cell::RefCell<std::collections::HashMap<(String, u16), (String, Vec<Stmt>)>>,
}

impl ParseCache {
    fn get(&self, key: &str, file: FileId, source: &str) -> Option<Vec<Stmt>> {
        let entries = self.entries.borrow();
        let (cached, stmts) = entries.get(&(key.to_string(), file.0))?;
        (cached == source).then(|| stmts.clone())
    }

    fn put(&self, key: &str, file: FileId, source: &str, stmts: &[Stmt]) {
        self.entries
            .borrow_mut()
            .insert((key.to_string(), file.0), (source.to_string(), stmts.to_vec()));
    }
}

/// The built-in resolver held by `Env`. See the module docs for the
/// resolution order.
#[derive(Default)]
pub struct ModuleRegistry {
    memory: HashMap<String, String>,
    /// Modules every loaded entry file imports as if by a plain `import m`,
    /// without ceremony, at the lowest precedence above the core prelude.
    pub implicit_imports: Vec<String>,
    /// Implicit imports that come *before* [`implicit_imports`] and are not
    /// touched by `set_implicit_imports`. The core prelude (`std`) lives here
    /// so a host that replaces its own implicit imports can't drop it. Lowest
    /// precedence of all, so both host preludes and user code shadow it.
    pub base_implicit_imports: Vec<String>,
    /// Packages the host registered (`Env::add_package`,
    /// `Env::register_package`): the first place a package name is looked up.
    packages: Vec<crate::package::Package>,
    /// Packages discovered on a search path (`-I`, `PETAL_PATH`): consulted
    /// after the importing file's own package.
    search_packages: Vec<crate::package::Package>,
    /// Text that stands in for a file's contents on disk, keyed by canonical
    /// path: a module that resolves to one of these files loads this source
    /// instead. This is how a tool compiles an edit it has not written yet
    /// (`petal apply-change` proving a multi-file rewrite before it touches
    /// anything); which file a name resolves to is unaffected.
    file_overrides: HashMap<PathBuf, String>,
    /// Parses of the files loaded through this registry (see [`ParseCache`]).
    parse_cache: ParseCache,
    /// The directory a program with no file path of its own (`petal run -e`,
    /// stdin) resolves `./x` against, once the in-memory modules have been
    /// tried. `None` — the default, and what an embedding host gets — leaves
    /// such a program with in-memory modules only.
    pub inline_import_dir: Option<PathBuf>,
    /// The package each directory belongs to (the nearest `petal.toml` at or
    /// above it), remembered for the length of one load.
    enclosing: std::cell::RefCell<HashMap<PathBuf, Option<std::sync::Arc<crate::package::Package>>>>,
}

impl ModuleRegistry {
    /// Register an in-memory module. Takes priority over the filesystem.
    pub fn register(&mut self, name: &str, source: &str) {
        self.memory.insert(name.to_string(), source.to_string());
    }

    /// Register a package the host named. A package with the same name is
    /// replaced — the most recent registration of a name wins.
    pub fn add_package(&mut self, package: crate::package::Package) {
        let name = package.info.name.clone();
        self.packages.retain(|p| p.info.name != name);
        self.packages.push(package);
    }

    /// Register a package found on a search path. The first one found under a
    /// name wins, matching search-path precedence.
    pub fn add_search_package(&mut self, package: crate::package::Package) {
        if !self
            .search_packages
            .iter()
            .any(|p| p.info.name == package.info.name)
        {
            self.search_packages.push(package);
        }
    }

    /// Is a package with this name registered or on a search path?
    pub fn has_package(&self, name: &str) -> bool {
        self.all_packages().any(|p| p.info.name == name)
    }

    /// Every package a program can name, host registrations first, then the
    /// search path. A search-path package a host registration shadows is left
    /// out. Bundled packages are not listed.
    pub fn packages(&self) -> Vec<&crate::package::Package> {
        self.all_packages().collect()
    }

    fn all_packages(&self) -> impl Iterator<Item = &crate::package::Package> {
        self.packages.iter().chain(
            self.search_packages
                .iter()
                .filter(|s| !self.packages.iter().any(|p| p.info.name == s.info.name)),
        )
    }

    /// Load `source` in place of whatever `path` holds on disk, for every
    /// later resolution that lands on that file.
    pub fn override_file(&mut self, path: &Path, source: &str) {
        self.file_overrides
            .insert(canonical_path(path), source.to_string());
    }

    /// See [`crate::env::Env::importable_paths`].
    pub fn importable_paths(&self) -> Vec<String> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out = Vec::new();
        for pkg in self.all_packages().chain(bundled_packages().iter()) {
            let name = pkg.info.name.as_str();
            if name == crate::bundled::CORE_PACKAGE || !seen.insert(name) {
                continue;
            }
            if pkg.info.modules.iter().any(|m| m == name) {
                out.push(name.to_string());
            } else {
                out.extend(pkg.info.modules.iter().map(|m| format!("{name}/{m}")));
            }
        }
        for name in self.memory.keys() {
            let package = name.split('/').next().unwrap_or(name);
            if name != crate::env::STD_MODULE
                && package != crate::bundled::CORE_PACKAGE
                && !seen.contains(package)
            {
                out.push(name.clone());
            }
        }
        out.sort();
        out
    }

    /// The source registered in memory under `name`.
    pub fn memory_module(&self, name: &str) -> Option<&str> {
        self.memory.get(name).map(String::as_str)
    }
}

impl ModuleRegistry {
    /// The text a source file of a loaded program holds *now*: `origin`'s
    /// contents (or its [override](Self::override_file)) for a module that
    /// came from disk, the current registration of `name` for one held in
    /// memory. `None` when it can no longer be read or is no longer
    /// registered. What a reload compares against the text a program was
    /// compiled from.
    pub fn current_source(&self, name: &str, origin: Option<&Path>) -> Option<String> {
        match origin {
            Some(path) => {
                if !self.file_overrides.is_empty()
                    && let Some(source) = self.file_overrides.get(&canonical_path(path))
                {
                    return Some(source.clone());
                }
                // A package registered from memory stamps its modules with a
                // path that names no file.
                if let Some(source) = self
                    .all_packages()
                    .chain(bundled_packages().iter())
                    .find_map(|p| p.memory_source_at(path))
                {
                    return Some(source.to_string());
                }
                std::fs::read_to_string(path).ok()
            }
            None => self.memory.get(name).cloned(),
        }
    }
}

/// `path` with symlinks and `..` resolved, so two spellings of one file
/// compare equal. A path that cannot be canonicalized (it does not exist) is
/// returned as written.
pub fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `path` with `.` and `..` components folded away without touching the
/// filesystem. `None` when a `..` would climb above the path's root.
fn normalize_lexical(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return None;
                }
                out.pop();
                depth -= 1;
            }
            Component::Normal(seg) => {
                out.push(seg);
                depth += 1;
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// `target` as a `/`-joined path relative to the directory `base`, without
/// its `.ptl` suffix: `level`, `ui/hud`, `../shared/rng`. How a loose file is
/// named. Both paths are expected canonical.
fn relative_module_name(base: &Path, target: &Path) -> String {
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

/// The packages compiled into the binary, in the form the registry resolves
/// against.
fn bundled_packages() -> &'static [crate::package::Package] {
    static BUNDLED: std::sync::OnceLock<Vec<crate::package::Package>> = std::sync::OnceLock::new();
    BUNDLED
        .get_or_init(|| {
            crate::bundled::BUNDLED_PACKAGES
                .iter()
                .filter_map(|b| crate::package::Package::bundled(b).ok())
                .collect()
        })
}

impl ModuleResolver for ModuleRegistry {
    fn parse_cache(&self) -> Option<&ParseCache> {
        Some(&self.parse_cache)
    }

    fn begin_load(&self) {
        self.enclosing.borrow_mut().clear();
    }

    fn resolve(&self, path: &str, importer: &Importer) -> Result<ModuleSource, String> {
        let mut found = if is_relative_import(path) {
            self.resolve_relative(path, importer)?
        } else {
            self.resolve_package_path(path, importer)?
        };
        if !self.file_overrides.is_empty()
            && let ModuleOrigin::File(path) = &found.origin
            && let Some(source) = self.file_overrides.get(&canonical_path(path))
        {
            found.source = source.clone();
        }
        Ok(found)
    }
}

impl ModuleRegistry {
    /// The package the directory `dir` belongs to: the one whose `petal.toml`
    /// is nearest at or above it. `Err` when that manifest will not load.
    fn enclosing_package(
        &self,
        dir: &Path,
    ) -> Result<Option<std::sync::Arc<crate::package::Package>>, String> {
        if let Some(hit) = self.enclosing.borrow().get(dir) {
            return Ok(hit.clone());
        }
        let found = if dir.join(crate::package::MANIFEST_FILE).is_file() {
            Some(std::sync::Arc::new(
                crate::package::Package::from_dir(dir).map_err(|e| e.to_string())?,
            ))
        } else {
            match dir.parent() {
                Some(parent) if parent != dir => self.enclosing_package(parent)?,
                _ => None,
            }
        };
        self.enclosing
            .borrow_mut()
            .insert(dir.to_path_buf(), found.clone());
        Ok(found)
    }

    /// The package an in-memory module file (one stamped with a synthetic
    /// path) belongs to.
    fn memory_package_of(&self, path: &Path) -> Option<&crate::package::Package> {
        self.all_packages()
            .chain(bundled_packages().iter())
            .find(|p| p.info.in_memory && path.starts_with(&p.info.module_dir))
    }

    /// `./x` / `../x`: against the importing file.
    fn resolve_relative(&self, path: &str, importer: &Importer) -> Result<ModuleSource, String> {
        let rel = Path::new(path);
        match importer.origin {
            Some(ModuleOrigin::File(file)) => {
                let dir = file.parent().unwrap_or(Path::new(""));
                // A module of an in-memory package: its "directory" is the
                // package itself.
                if let Some(pkg) = self.memory_package_of(file) {
                    let leaves = || {
                        format!(
                            "the path leaves the package '{}' — a relative import may not \
                             climb out of its package",
                            pkg.info.name
                        )
                    };
                    let target = normalize_lexical(&dir.join(rel)).ok_or_else(leaves)?;
                    let inside = target
                        .strip_prefix(&pkg.info.module_dir)
                        .map_err(|_| leaves())?;
                    let module = path_to_module(inside);
                    return pkg.load(&module).ok_or_else(|| {
                        format!("package '{}' has no module '{module}'", pkg.info.name)
                    });
                }
                let dir = canonical_path(dir);
                let target = normalize_lexical(&dir.join(format!("{path}.ptl")))
                    .ok_or_else(|| "the path climbs above the filesystem root".to_string())?;
                // A file inside a package may not reach outside it.
                if let Some(pkg) = self.enclosing_package(&dir)? {
                    let root = canonical_path(&pkg.info.root);
                    if !target.starts_with(&root) {
                        return Err(format!(
                            "the path leaves the package '{}' (rooted at {}) — a relative \
                             import may not climb out of its package",
                            pkg.info.name,
                            root.display()
                        ));
                    }
                }
                let source = match self.file_overrides.get(&target) {
                    Some(source) => source.clone(),
                    None => std::fs::read_to_string(&target)
                        .map_err(|_| format!("no file {}", target.display()))?,
                };
                let target = canonical_path(&target);
                Ok(ModuleSource {
                    name: self.package_name_of(&target)?,
                    source,
                    origin: ModuleOrigin::File(target),
                })
            }
            // An in-memory module, or an entry with no path: the path is
            // resolved against the importer's own registered name.
            Some(ModuleOrigin::Memory) | None => {
                let mut segments: Vec<&str> = importer
                    .name
                    .map(|n| n.split('/').collect())
                    .unwrap_or_default();
                segments.pop();
                for seg in path.split('/') {
                    match seg {
                        "." => {}
                        ".." => {
                            if segments.pop().is_none() {
                                return Err(
                                    "the path climbs above the modules the host registered"
                                        .to_string(),
                                );
                            }
                        }
                        seg => segments.push(seg),
                    }
                }
                let name = segments.join("/");
                match self.memory.get(&name) {
                    Some(source) => Ok(ModuleSource {
                        name: Some(name),
                        source: source.clone(),
                        origin: ModuleOrigin::Memory,
                    }),
                    None if importer.origin.is_none() && self.inline_import_dir.is_some() => {
                        let dir = self.inline_import_dir.as_deref().expect("just checked");
                        let stand_in = ModuleOrigin::File(dir.join(INLINE_ENTRY_NAME));
                        self.resolve_relative(
                            path,
                            &Importer {
                                name: None,
                                origin: Some(&stand_in),
                            },
                        )
                    }
                    None if importer.origin.is_none() => Err(format!(
                        "the importing program has no file path, and no module '{name}' is \
                         registered in memory"
                    )),
                    None => Err(format!("no module '{name}' is registered in memory")),
                }
            }
        }
    }

    /// The canonical name of the module at `file` (a canonical path) when it
    /// sits under a package's module directory; `None` for a loose file.
    fn package_name_of(&self, file: &Path) -> Result<Option<String>, String> {
        let Some(dir) = file.parent() else {
            return Ok(None);
        };
        let Some(pkg) = self.enclosing_package(dir)? else {
            return Ok(None);
        };
        let module_dir = canonical_path(&pkg.info.module_dir);
        Ok(file
            .strip_prefix(&module_dir)
            .ok()
            .map(|inside| pkg.canonical_name(&path_to_module(inside))))
    }

    /// The canonical name the *entry* file would have as a module, when it is
    /// part of a package. What stops a package's own modules importing the
    /// entry back.
    pub fn module_name_of_file(&self, file: &Path) -> Option<String> {
        self.package_name_of(&canonical_path(file)).ok().flatten()
    }

    /// A path whose first segment is a package name.
    fn resolve_package_path(
        &self,
        path: &str,
        importer: &Importer,
    ) -> Result<ModuleSource, String> {
        if let Some(problem) = module_path_error(path) {
            return Err(problem);
        }
        // 1. What the host registered in memory, by full path…
        if let Some(source) = self.memory.get(path) {
            return Ok(ModuleSource {
                name: Some(path.to_string()),
                source: source.clone(),
                origin: ModuleOrigin::Memory,
            });
        }
        let (package, rest) = match path.split_once('/') {
            Some((head, rest)) => (head, Some(rest)),
            None => (path, None),
        };
        let load = |pkg: &crate::package::Package| -> Result<ModuleSource, String> {
            let module = rest.unwrap_or(&pkg.info.name);
            pkg.load(module).ok_or_else(|| match rest {
                Some(rest) => format!("package '{package}' has no module '{rest}'"),
                None => format!(
                    "package '{package}' has no module of its own name ({package}.ptl) to \
                     import as `{package}` — import one of its modules: {}",
                    list_some(&pkg.info.modules, package)
                ),
            })
        };
        // …and the packages it registered.
        if let Some(pkg) = self.packages.iter().find(|p| p.info.name == package) {
            return load(pkg);
        }
        // 2. The package the importing file is part of, by its own name.
        if let Some(ModuleOrigin::File(file)) = importer.origin
            && self.memory_package_of(file).is_none()
            && let Some(dir) = file.parent()
            && let Some(pkg) = self.enclosing_package(&canonical_path(dir))?
            && pkg.info.name == package
        {
            return load(&pkg);
        }
        // 3. The search path, 4. the binary.
        if let Some(pkg) = self
            .search_packages
            .iter()
            .chain(bundled_packages().iter())
            .find(|p| p.info.name == package)
        {
            return load(pkg);
        }

        // Nothing by that name. The commonest cause is a sibling file
        // imported the old way.
        let beside = match importer.origin {
            Some(ModuleOrigin::File(file)) => file.parent(),
            Some(ModuleOrigin::Memory) => None,
            None => self.inline_import_dir.as_deref(),
        };
        if let Some(dir) = beside
            && module_file_path(dir, path).is_file()
        {
            return Err(format!(
                "there is no package named '{package}' — a bare name is always a package. \
                 The file beside this one is `import ./{path}` (`petal fix-imports` rewrites \
                 these)"
            ));
        }
        Err(format!(
            "there is no package named '{package}': not registered by the host, not the \
             importing file's own package, not on a search path (-I, PETAL_PATH), and not \
             bundled. A file beside this one is imported as `./{path}`"
        ))
    }
}

/// `a, b, c, +N more`, each prefixed with the package name.
fn list_some(modules: &[String], package: &str) -> String {
    if modules.is_empty() {
        return "(it has none)".to_string();
    }
    let mut out: Vec<String> = modules
        .iter()
        .take(4)
        .map(|m| format!("{package}/{m}"))
        .collect();
    if modules.len() > 4 {
        out.push(format!("+{} more", modules.len() - 4));
    }
    out.join(", ")
}

/// `a/b.ptl` → `a/b`.
fn path_to_module(path: &Path) -> String {
    path.with_extension("")
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The file a module path names under `dir`: each segment is a directory, the
/// last one the `.ptl` file (`bloom/menu` → `<dir>/bloom/menu.ptl`).
fn module_file_path(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    let mut segments = name.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_some() {
            path.push(segment);
        } else {
            path.push(format!("{segment}.ptl"));
        }
    }
    path
}

/// How a module is named in diagnostics. A flat module keeps today's bare file
/// name (`ui.ptl`); a nested one keeps its path (`bloom/menu.ptl`), because two
/// namespaces may well ship the same file name and an error has to say which
/// one it means.
fn display_name_for(name: &str, origin: &ModuleOrigin) -> String {
    match origin {
        ModuleOrigin::File(path) if !name.contains('/') => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{name}.ptl")),
        ModuleOrigin::File(_) => format!("{name}.ptl"),
        ModuleOrigin::Memory => name.to_string(),
    }
}

/// Why a module path is unusable, or `None` when it is fine. A path is one or
/// more identifier-shaped segments joined by `/`; `.` and `..` are rejected so
/// an import can never climb out of the root it is resolved against, and the
/// separator characters are rejected so a path cannot smuggle in an absolute
/// or Windows-style location. The parser rejects most of this at the syntax
/// level; this is the backstop for paths that arrive another way (a host's
/// `register_module` name, an implicit-import list, a hand-built AST).
fn module_path_error(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("a module path may not be empty".to_string());
    }
    for segment in name.split('/') {
        if segment.is_empty() {
            return Some(format!("module path '{name}' has an empty segment"));
        }
        if segment == "." || segment == ".." {
            return Some(format!(
                "module path '{name}' may not contain '.' or '..' — a path segment names a \
                 directory under the module root"
            ));
        }
        if segment.contains(['\\', ':']) {
            return Some(format!("module path '{name}' may not contain '\\' or ':'"));
        }
    }
    None
}

/// One import binding the compiler must materialize in the importing file's
/// scope. `implicit` imports bind every export bare but *weakly*: the file's
/// own declarations shadow them silently, and they never raise collision
/// errors (mirroring how builtins behave).
#[derive(Clone)]
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
    /// Where this file spells the `pub` modifier the deprecated way,
    /// `export`. Collected here, before `imports` is split off `stmts`, so an
    /// `export import …` is seen too; the compiler turns each into a warning.
    pub deprecated_exports: Vec<SourceSpan>,
}

/// The display name of an entry "file" that has no path (`-e` code, a host
/// string).
pub const INLINE_ENTRY_NAME: &str = "<entry>";

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
/// - `implicit_imports` — host preludes (e.g. `ui`), always merged, and bound
///   inside every module of the program, not just the entry (a prelude module
///   and its own dependencies excepted — see below).
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
) -> Result<Vec<LoadedModule>, LoadError> {
    let entry_display = entry_origin
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| INLINE_ENTRY_NAME.to_string());
    let stmts = parse_module(entry_source, ENTRY_FILE, None, resolver.parse_cache(), "")?;
    let entry_deprecated_exports = deprecated_exports(&stmts, entry_source);
    let (explicit_imports, stmts) = split_imports(stmts);
    let (mut explicit_imports, stmts) = (explicit_imports, stmts);
    for import in &explicit_imports {
        reject_star(&import.decl, &entry_display)?;
    }
    let entry_module_origin = entry_origin.map(|p| ModuleOrigin::File(p.to_path_buf()));
    resolver.begin_load();

    // Loose files are named by their path from the entry file's directory.
    let entry_canonical = entry_origin.map(canonical_path);
    let base_dir = entry_canonical
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok().map(|d| canonical_path(&d)))
        .unwrap_or_default();
    let mut walker = Walker {
        resolver,
        loaded: HashMap::new(),
        in_progress: Vec::new(),
        out: Vec::new(),
        next_file: 1,
        resolved: HashMap::new(),
        base_dir,
        entry_file: entry_canonical,
    };
    let entry_importer = Importer {
        name: None,
        origin: entry_module_origin.as_ref(),
    };

    // The always-merged imports, in precedence order (host implicit first, then
    // the entry's explicit imports). The entry's own bindings land on top.
    let mut implicit_decls: Vec<ResolvedImport> = implicit_imports
        .iter()
        .map(|name| ResolvedImport {
            decl: import_all(name),
            implicit: true,
        })
        .collect();

    // Walk the host preludes first, and keep them (plus whatever *they* import)
    // in their own list: those modules are emitted before every other module, so
    // they are exactly the ones that must not receive an implicit import back —
    // that would be a self-import, or a forward reference to a module that has
    // not run yet.
    for import in &mut implicit_decls {
        import.decl.module =
            walker.visit(&import.decl.module, &entry_importer, &entry_display)?;
    }
    let prelude_modules = std::mem::take(&mut walker.out);

    // Then the entry's own explicit imports. A module reached only from here can
    // safely see the host prelude bare.
    for import in &mut explicit_imports {
        import.decl.module =
            walker.visit(&import.decl.module, &entry_importer, &entry_display)?;
    }
    let imported_modules = std::mem::take(&mut walker.out);

    let mut ungated: Vec<ResolvedImport> = implicit_decls.clone();
    ungated.extend(explicit_imports);

    // Reference-gate the core prelude: include a gated module only when the
    // entry or one of the always-merged modules names one of its exports. An
    // included prelude module's own references can pull in a further one, so
    // iterate to a fixpoint.
    let mut refs: HashSet<String> = HashSet::new();
    collect_module_refs(&stmts, &mut refs);
    for m in prelude_modules.iter().chain(imported_modules.iter()) {
        collect_module_refs(&m.stmts, &mut refs);
    }
    let scanned: Vec<(String, Vec<String>, HashSet<String>)> = gated_imports
        .iter()
        .map(|name| {
            let (exports, idents) = scan_gated_module(resolver, name, &entry_importer)?;
            Ok((name.clone(), exports, idents))
        })
        .collect::<Result<_, LoadError>>()?;
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
        let canonical = walker.visit(name, &entry_importer, &entry_display)?;
        gated_decls.push(ResolvedImport {
            decl: import_all(&canonical),
            implicit: true,
        });
    }
    let gated_modules = std::mem::take(&mut walker.out);

    // The gated prelude binds inside *every* module, not just the entry. A host
    // that registers its own prelude module (petal-ui's `ui`, the fantasy
    // console's `nes_sound`) writes ordinary Petal in it and reasonably expects
    // `has_field`/`sum`/… to resolve there exactly as they do in a script; when
    // they only reached the entry, such a module compiled fine and then raised
    // "Unknown builtin" the first time the offending line ran, which is both
    // late and confusing. Gated decls stay lowest-precedence, so a module's own
    // declarations still shadow them.
    //
    // The host's own implicit imports (`Env::set_implicit_imports`, e.g.
    // petal-ui's `ui`) reach every module for the same reason: a host promises
    // "scripts get this prelude for free", and that promise used to stop at the
    // entry file — the moment a script grew a second file, a bare `draw_rect`
    // there silently meant the raw native instead of the prelude's overload.
    // They bind just above the gated prelude and below the module's own
    // explicit imports and declarations, and stay weak (shadowed silently,
    // never a collision error). Only modules reached from the entry's own
    // imports get them: the prelude modules themselves, and anything they
    // import, are emitted first and would be importing themselves or a module
    // that has not run yet.
    let mut prelude_modules = prelude_modules;
    for m in &mut prelude_modules {
        let mut imports = gated_decls.clone();
        imports.extend(std::mem::take(&mut m.imports));
        m.imports = imports;
    }
    let mut imported_modules = imported_modules;
    for m in &mut imported_modules {
        let mut imports = gated_decls.clone();
        imports.extend(implicit_decls.iter().cloned());
        imports.extend(std::mem::take(&mut m.imports));
        m.imports = imports;
    }

    // Assemble: prelude modules run first, then the always-merged modules, then
    // the entry. The entry's import list mirrors that precedence order.
    let mut modules = gated_modules;
    modules.extend(prelude_modules);
    modules.extend(imported_modules);
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
        deprecated_exports: entry_deprecated_exports,
    });
    Ok(modules)
}

/// `import m: *` no longer exists: a plain `import m` is that form now.
fn reject_star(decl: &ImportDecl, display: &str) -> Result<(), LoadError> {
    if !decl.star {
        return Ok(());
    }
    let keyword = if decl.exported { "pub import" } else { "import" };
    Err(LoadError::message(
        Phase::Module,
        format!(
            "{display}: `{keyword} {m}: *` is no longer a form — a plain `{keyword} {m}` \
             binds every export of the module (`petal fix-imports` rewrites these)",
            m = decl.module
        ),
    ))
}

/// `import <name>` binding all of the module's exports (no alias, no selection).
fn import_all(name: &str) -> ImportDecl {
    ImportDecl {
        module: name.to_string(),
        alias: None,
        names: None,
        star: false,
        exported: false,
    }
}

/// The top-level names a statement declares (`fn f`, `let x`, `state s`,
/// `enum E`), for reading a module's exported surface.
fn declared_name(kind: &StmtKind) -> Option<&str> {
    match kind {
        StmtKind::Let { name, .. }
        | StmtKind::FnDecl { name, .. }
        | StmtKind::NativeFn(crate::ast::NativeFnDecl { name, .. })
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
    importer: &Importer,
) -> Result<(Vec<String>, HashSet<String>), LoadError> {
    let resolved = resolver.resolve(name, importer).map_err(|why| {
        LoadError::message(
            Phase::Module,
            format!("cannot find implicit prelude module '{name}': {why}"),
        )
    })?;
    let display = display_name_for(name, &resolved.origin);
    // Keyed apart from the load of the same module (`visit`), which stamps
    // its spans with the module's own file id.
    let stmts = parse_module(
        &resolved.source,
        ENTRY_FILE,
        Some(&display),
        resolver.parse_cache(),
        &format!("scan {name}"),
    )?;
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
    /// Canonical name → where the module loaded under it came from.
    loaded: HashMap<String, ModuleOrigin>,
    /// DFS stack of canonical module names currently being loaded, for cycle
    /// reporting.
    in_progress: Vec<String>,
    /// Finished modules in dependency post-order.
    out: Vec<LoadedModule>,
    next_file: u16,
    /// (import path as written, the importing module or directory) → the
    /// canonical name it resolved to, so a module imported from many files is
    /// read once.
    resolved: HashMap<(String, String), String>,
    /// The directory loose files are named relative to: the entry file's.
    base_dir: PathBuf,
    /// The entry file's canonical path, which nothing may import.
    entry_file: Option<PathBuf>,
}

impl Walker<'_> {
    /// Load the module `path` names when written in `importer`, and everything
    /// it imports. Returns the module's canonical name.
    fn visit(
        &mut self,
        path: &str,
        importer: &Importer,
        importer_display: &str,
    ) -> Result<String, LoadError> {
        // What the same spelling resolved to from the same place before.
        let memo_key = (
            path.to_string(),
            match importer.origin {
                Some(ModuleOrigin::File(file)) => file
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                _ => format!("<{}>", importer.name.unwrap_or("")),
            },
        );
        if let Some(name) = self.resolved.get(&memo_key) {
            let name = name.clone();
            self.check_cycle(&name)?;
            return Ok(name);
        }

        let resolved = self.resolver.resolve(path, importer).map_err(|why| {
            LoadError::message(
                Phase::Module,
                format!("cannot find module '{path}' (imported by {importer_display}): {why}"),
            )
        })?;
        if let (ModuleOrigin::File(file), Some(entry)) = (&resolved.origin, &self.entry_file)
            && (file == entry || canonical_path(file) == *entry)
        {
            return Err(LoadError::message(
                Phase::Module,
                format!(
                    "import cycle: '{path}' (imported by {importer_display}) is the program's \
                     entry file"
                ),
            ));
        }
        let name = match (&resolved.name, &resolved.origin) {
            (Some(name), _) => name.clone(),
            (None, ModuleOrigin::File(file)) => relative_module_name(&self.base_dir, file),
            (None, ModuleOrigin::Memory) => path.to_string(),
        };
        self.resolved.insert(memo_key, name.clone());

        if let Some(origin) = self.loaded.get(&name) {
            let same = match (origin, &resolved.origin) {
                (ModuleOrigin::File(a), ModuleOrigin::File(b)) => {
                    a == b || canonical_path(a) == canonical_path(b)
                }
                (a, b) => a == b,
            };
            if !same {
                return Err(LoadError::message(
                    Phase::Module,
                    format!(
                        "two different sources are both the module '{name}': {} and {} \
                         (imported by {importer_display})",
                        describe_origin(origin),
                        describe_origin(&resolved.origin)
                    ),
                ));
            }
            return Ok(name);
        }
        self.check_cycle(&name)?;

        let display_name = display_name_for(&name, &resolved.origin);
        let file_id = FileId(self.next_file);
        self.next_file = self.next_file.checked_add(1).ok_or_else(|| {
            LoadError::message(
                Phase::Module,
                "too many modules (file table limit is 65535)",
            )
        })?;

        let stmts = parse_module(
            &resolved.source,
            file_id,
            Some(&display_name),
            self.resolver.parse_cache(),
            &format!("module {name}"),
        )?;
        let deprecated_exports = deprecated_exports(&stmts, &resolved.source);
        let (mut imports, stmts) = split_imports(stmts);

        self.in_progress.push(name.clone());
        for import in &mut imports {
            reject_star(&import.decl, &display_name)?;
            import.decl.module = self.visit(
                &import.decl.module,
                &Importer {
                    name: Some(&name),
                    origin: Some(&resolved.origin),
                },
                &display_name,
            )?;
        }
        self.in_progress.pop();

        self.loaded.insert(name.clone(), resolved.origin.clone());
        self.out.push(LoadedModule {
            name: Some(name.clone()),
            display_name,
            source: resolved.source,
            origin: match resolved.origin {
                ModuleOrigin::File(path) => Some(path),
                ModuleOrigin::Memory => None,
            },
            stmts,
            imports,
            file_id,
            deprecated_exports,
        });
        Ok(name)
    }

    fn check_cycle(&self, name: &str) -> Result<(), LoadError> {
        if let Some(pos) = self.in_progress.iter().position(|n| n == name) {
            let mut cycle: Vec<&str> = self.in_progress[pos..].iter().map(String::as_str).collect();
            cycle.push(name);
            return Err(LoadError::message(
                Phase::Module,
                format!("import cycle: {}", cycle.join(" -> ")),
            ));
        }
        Ok(())
    }
}

fn describe_origin(origin: &ModuleOrigin) -> String {
    match origin {
        ModuleOrigin::File(path) => path.display().to_string(),
        ModuleOrigin::Memory => "a module registered in memory".to_string(),
    }
}

/// Lex + parse one module's source, tagging spans with `file_id`. Errors from
/// non-entry modules are prefixed with the module's display name (entry-file
/// errors keep today's format).
fn parse_module(
    source: &str,
    file_id: FileId,
    display_name: Option<&str>,
    cache: Option<&ParseCache>,
    cache_key: &str,
) -> Result<Vec<Stmt>, LoadError> {
    if let Some(stmts) = cache.and_then(|c| c.get(cache_key, file_id, source)) {
        return Ok(stmts);
    }
    let stmts = parse_module_uncached(source, file_id, display_name)?;
    if let Some(cache) = cache {
        cache.put(cache_key, file_id, source, &stmts);
    }
    Ok(stmts)
}

fn parse_module_uncached(
    source: &str,
    file_id: FileId,
    display_name: Option<&str>,
) -> Result<Vec<Stmt>, LoadError> {
    let (_tree, stmts) = crate::cst::parse_source_phased(source, file_id).map_err(|mut e| {
        // Tagging each item with the module's display name reproduces the
        // old `format!("{name}: {e}")` annotation byte for byte; entry-file
        // errors carry no file and so are unchanged.
        if let Some(name) = display_name {
            for item in &mut e.items {
                item.file = Some(name.to_string());
            }
        }
        e
    })?;
    Ok(stmts)
}

/// The `export` modifiers in a freshly parsed file (see
/// [`crate::export_keyword`]).
pub(crate) fn deprecated_exports(stmts: &[Stmt], source: &str) -> Vec<SourceSpan> {
    let chars: Vec<char> = source.chars().collect();
    crate::export_keyword::deprecated_export_spans(stmts, &chars)
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

#[cfg(test)]
mod tests {
    use super::module_path_error;
    use crate::env::Env;

    /// Run `entry` against a set of in-memory modules with `implicit` registered
    /// as the host's implicit imports, returning its print output.
    fn run(modules: &[(&str, &str)], implicit: &[&str], entry: &str) -> Vec<String> {
        let mut env = Env::new();
        for (name, source) in modules {
            env.register_module(name, source);
        }
        env.set_implicit_imports(implicit);
        let pid = env.load_program(entry).unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        env.take_output()
    }

    /// Like [`run`], but for a program expected to fail to load: returns the
    /// error text.
    fn load_err(modules: &[(&str, &str)], entry: &str) -> String {
        let mut env = Env::new();
        for (name, source) in modules {
            env.register_module(name, source);
        }
        env.load_program(entry)
            .expect_err("expected a load error")
            .to_string()
    }

    const UI: &str = "pub fn button(l)\n  \"[\" ++ l ++ \"]\"\nend\n\
                      pub let theme = { fg: 15 }";

    #[test]
    fn host_implicit_imports_reach_an_imported_module() {
        // The gap: a library module calling a host prelude function bare used to
        // see the raw builtin (or nothing) — implicit imports stopped at the
        // entry file.
        assert_eq!(
            run(
                &[("ui", UI), ("lib", "pub fn box(l)\n  button(l)\nend")],
                &["ui"],
                "import lib\nprint(lib.box(\"z\"))",
            ),
            vec!["[z]"]
        );
    }

    #[test]
    fn host_implicit_imports_reach_a_transitively_imported_module() {
        assert_eq!(
            run(
                &[
                    ("ui", UI),
                    ("mid", "import deep\npub fn go(l)\n  deep.go(l)\nend"),
                    ("deep", "pub fn go(l)\n  button(l) ++ str(theme.fg)\nend"),
                ],
                &["ui"],
                "import mid\nprint(mid.go(\"z\"))",
            ),
            vec!["[z]15"]
        );
    }

    #[test]
    fn a_module_declaration_shadows_a_host_implicit_import() {
        // Implicit imports are the weakest binding above the core prelude: the
        // module's own declaration wins, silently, with no collision error.
        assert_eq!(
            run(
                &[
                    ("ui", UI),
                    (
                        "lib",
                        "fn button(l)\n  \"<\" ++ l ++ \">\"\nend\n\
                         pub fn box(l)\n  button(l)\nend",
                    ),
                ],
                &["ui"],
                "import lib\nprint(lib.box(\"z\"))",
            ),
            vec!["<z>"]
        );
    }

    #[test]
    fn a_modules_explicit_import_wins_over_a_host_implicit_import() {
        assert_eq!(
            run(
                &[
                    ("ui", UI),
                    ("other", "pub fn button(l)\n  \"(\" ++ l ++ \")\"\nend"),
                    (
                        "lib",
                        "import other: button\npub fn box(l)\n  button(l)\nend",
                    ),
                ],
                &["ui"],
                "import lib\nprint(lib.box(\"z\"))",
            ),
            vec!["(z)"]
        );
    }

    #[test]
    fn an_imported_module_may_also_import_the_prelude_explicitly() {
        // Belt and braces (what bloom does today): the explicit import is a
        // no-op on top of the implicit one, not a redefinition error.
        assert_eq!(
            run(
                &[
                    ("ui", UI),
                    (
                        "lib",
                        "import ui: button\npub fn box(l)\n  button(l) ++ str(theme.fg)\nend",
                    ),
                ],
                &["ui"],
                "import lib\nprint(lib.box(\"z\"))",
            ),
            vec!["[z]15"]
        );
    }

    #[test]
    fn a_prelude_module_does_not_implicitly_import_itself() {
        // `ui` declares `button` and also calls it; injecting an implicit import
        // of `ui` into `ui` would be a self-reference before it has run.
        assert_eq!(
            run(
                &[(
                    "ui",
                    "fn wrap(l)\n  \"[\" ++ l ++ \"]\"\nend\npub fn button(l)\n  wrap(l)\nend",
                )],
                &["ui"],
                "print(button(\"z\"))",
            ),
            vec!["[z]"]
        );
    }

    #[test]
    fn several_prelude_modules_do_not_import_each_other_implicitly() {
        // Two host preludes, one explicitly importing the other: neither gets an
        // implicit import back, so there is no cycle and no forward reference.
        assert_eq!(
            run(
                &[
                    ("core", "pub fn wrap(l)\n  \"[\" ++ l ++ \"]\"\nend"),
                    (
                        "ui",
                        "import core: wrap\npub fn button(l)\n  wrap(l)\nend",
                    ),
                ],
                &["core", "ui"],
                "print(button(wrap(\"z\")))",
            ),
            vec!["[[z]]"]
        );
    }

    #[test]
    fn a_memory_module_registers_under_its_full_path() {
        // The path is the module's identity, so two namespaces may ship the
        // same last segment; each binds under that segment locally.
        assert_eq!(
            run(
                &[
                    ("bloom/menu", "pub fn open()\n  \"bloom\"\nend"),
                    ("petal/menu", "pub fn open()\n  \"petal\"\nend"),
                ],
                &[],
                "import bloom/menu\nimport petal/menu as pm\nprint(menu.open())\nprint(pm.open())",
            ),
            vec!["bloom", "petal"]
        );
    }

    #[test]
    fn a_nested_module_may_import_another_nested_module() {
        assert_eq!(
            run(
                &[
                    (
                        "bloom/menu",
                        "import bloom/motion\npub fn open()\n  motion.ease()\nend",
                    ),
                    ("bloom/motion", "pub fn ease()\n  \"eased\"\nend"),
                ],
                &[],
                "import bloom/menu\nprint(menu.open())",
            ),
            vec!["eased"]
        );
    }

    #[test]
    fn a_traversing_path_is_rejected_before_the_filesystem_is_touched() {
        // The parser rejects a written `a/../b`; this is the backstop for a
        // path that arrives some other way (here, a hand-built import).
        assert!(module_path_error("bloom/../etc").is_some());
        assert!(module_path_error("bloom/./menu").is_some());
        assert!(module_path_error("/bloom/menu").is_some());
        assert!(module_path_error("").is_some());
        assert!(module_path_error("bloom/menu").is_none());
        assert!(module_path_error("ui").is_none());
    }

    #[test]
    fn a_module_path_becomes_directories_under_the_root() {
        assert_eq!(
            super::module_file_path(std::path::Path::new("/root"), "bloom/menu"),
            std::path::Path::new("/root/bloom/menu.ptl")
        );
        assert_eq!(
            super::module_file_path(std::path::Path::new("/root"), "ui"),
            std::path::Path::new("/root/ui.ptl")
        );
    }

    #[test]
    fn the_core_prelude_stays_below_a_host_implicit_import() {
        // Precedence inside an imported module: std < host implicit < the
        // module's own declarations. A host prelude that redefines a `std` name
        // wins there just as it does in the entry file.
        assert_eq!(
            run(
                &[
                    ("ui", "pub fn sum(xs)\n  \"host sum\"\nend"),
                    ("lib", "pub fn go()\n  sum([1, 2])\nend"),
                ],
                &["ui"],
                "import lib\nprint(lib.go())",
            ),
            vec!["host sum"]
        );
    }

    // ---- re-exports (`pub import`) -------------------------------------

    /// The overload set is the property the hand-written facade
    /// (`pub let f = m.f`) already had, and the one a declarative re-export
    /// must not lose.
    const BUTTON: &str = "pub fn button(l)\n  \"[\" ++ l ++ \"]\"\nend\n\
                          pub fn button(l, w)\n  \"[\" ++ l ++ str(w) ++ \"]\"\nend";

    #[test]
    fn a_plain_re_export_carries_a_whole_overload_set() {
        assert_eq!(
            run(
                &[
                    ("bloom/button", BUTTON),
                    ("bloom", "pub import bloom/button"),
                ],
                &[],
                "import bloom: button\nprint(button(\"a\"))\nprint(button(\"a\", 2))",
            ),
            vec!["[a]", "[a2]"]
        );
    }

    #[test]
    fn a_plain_re_export_also_binds_the_names_locally() {
        // The facade can use what it re-exports.
        assert_eq!(
            run(
                &[
                    ("bloom/button", BUTTON),
                    (
                        "bloom",
                        "pub import bloom/button\npub fn twice(l)\n  button(l) ++ button(l)\nend",
                    ),
                ],
                &[],
                "import bloom: twice\nprint(twice(\"a\"))",
            ),
            vec!["[a][a]"]
        );
    }

    #[test]
    fn a_selective_re_export_names_exactly_what_it_passes_on() {
        assert_eq!(
            run(
                &[
                    (
                        "impl",
                        "pub fn a()\n  \"a\"\nend\npub fn b()\n  \"b\"\nend",
                    ),
                    ("facade", "pub import impl: a"),
                ],
                &[],
                "import facade\nprint(facade.a())",
            ),
            vec!["a"]
        );
        let err = load_err(
            &[
                (
                    "impl",
                    "pub fn a()\n  \"a\"\nend\npub fn b()\n  \"b\"\nend",
                ),
                ("facade", "pub import impl: a"),
            ],
            "import facade: b\nprint(b())",
        );
        assert!(err.contains("module 'facade' has no export 'b'"), "{err}");
    }

    #[test]
    fn re_exporting_a_name_the_module_does_not_have_is_an_error() {
        let err = load_err(
            &[
                ("impl", "pub fn a()\n  1\nend"),
                ("facade", "pub import impl: a, nope"),
            ],
            "import facade\nprint(facade.a())",
        );
        assert!(err.contains("module 'impl' has no export 'nope'"), "{err}");
    }

    #[test]
    fn a_local_declaration_wins_over_a_plain_re_export() {
        assert_eq!(
            run(
                &[
                    ("impl", "pub fn tag(l)\n  \"impl\" ++ l\nend"),
                    (
                        "facade",
                        "pub import impl\npub fn tag(l)\n  \"facade\" ++ l\nend",
                    ),
                ],
                &[],
                "import facade: tag\nprint(tag(\"!\"))",
            ),
            vec!["facade!"]
        );
    }

    #[test]
    fn two_plain_re_exports_of_one_function_name_merge_by_arity() {
        assert_eq!(
            run(
                &[
                    ("one", "pub fn f(a)\n  \"one\" ++ a\nend"),
                    ("two", "pub fn f(a, b)\n  \"two\" ++ a ++ b\nend"),
                    ("facade", "pub import one\npub import two"),
                ],
                &[],
                "import facade: f\nprint(f(\"x\"))\nprint(f(\"x\", \"y\"))",
            ),
            vec!["onex", "twoxy"]
        );
    }

    #[test]
    fn two_plain_re_exports_of_one_value_name_are_ambiguous_where_used() {
        let modules = [
            ("one", "pub let v = 1\npub let only_one = 10"),
            ("two", "pub let v = 2"),
            ("facade", "pub import one\npub import two"),
        ];
        // Nothing is wrong until someone uses the name.
        assert_eq!(
            run(&modules, &[], "import facade\nprint(only_one)"),
            vec!["10"]
        );
        let err = load_err(&modules, "import facade: v\nprint(v)");
        assert!(
            err.contains("`v` is ambiguous: 'one' and 'two' export it"),
            "{err}"
        );
        let err = load_err(&modules, "import facade\nprint(facade.v)");
        assert!(err.contains("`v` is ambiguous"), "{err}");
    }

    #[test]
    fn a_nested_path_prelude_binds_its_alias_by_its_last_segment() {
        // An implicit import binds the module alias the way an explicit
        // `import bloom/menu` would: `menu`, not the whole path (which no
        // expression can spell).
        assert_eq!(
            run(
                &[("bloom/menu", "pub fn open()\n  \"opened\"\nend")],
                &["bloom/menu"],
                "print(open())\nprint(menu.open())",
            ),
            vec!["opened", "opened"]
        );
    }

    #[test]
    fn a_re_export_chain_passes_a_name_along() {
        assert_eq!(
            run(
                &[
                    ("deep", "pub fn go()\n  \"deep\"\nend"),
                    ("mid", "pub import deep"),
                    ("top", "pub import mid"),
                ],
                &[],
                "import top: go\nprint(go())",
            ),
            vec!["deep"]
        );
    }

    #[test]
    fn a_re_export_cycle_is_an_error_not_a_hang() {
        let err = load_err(
            &[("a", "pub import b"), ("b", "pub import a")],
            "import a\nprint(1)",
        );
        assert!(err.contains("import cycle"), "{err}");
    }

    #[test]
    fn a_plain_pub_import_re_exports_every_export_not_the_qualifier() {
        let modules = [
            ("bloom/menu", "pub fn open()\n  \"open\"\nend"),
            ("bloom", "pub import ./bloom/menu"),
        ];
        assert_eq!(
            run(&modules, &[], "import bloom\nprint(open())\nprint(bloom.open())"),
            vec!["open", "open"]
        );
        let err = load_err(&modules, "import bloom: menu\nprint(menu.open())");
        assert!(err.contains("module 'bloom' has no export 'menu'"), "{err}");
    }

    // ---- the new import forms ------------------------------------------

    const HUD: &str = "pub fn draw()\n  \"hud\"\nend\npub fn score(a)\n  \"hud1\"\nend\n\
                       pub let hud_only = 1";
    const LEVEL: &str = "pub fn draw()\n  \"level\"\nend\n\
                         pub fn score(a, b)\n  \"level2\"\nend";

    #[test]
    fn a_plain_import_binds_every_export_bare_and_the_qualifier() {
        assert_eq!(
            run(
                &[("hud", HUD)],
                &[],
                "import hud\nprint(draw())\nprint(hud.draw())\nprint(hud_only)",
            ),
            vec!["hud", "hud", "1"]
        );
    }

    #[test]
    fn an_alias_import_binds_bare_names_and_renames_the_qualifier() {
        assert_eq!(
            run(&[("hud", HUD)], &[], "import hud as h\nprint(draw())\nprint(h.draw())"),
            vec!["hud", "hud"]
        );
        // The module's own name is no longer a qualifier.
        let mut env = Env::new();
        env.register_module("hud", HUD);
        let pid = env.load_program("import hud as h\nprint(hud.draw())").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let err = format!("{:?}", env.run(sid));
        assert!(err.contains("Undefined variable: hud"), "{err}");
    }

    #[test]
    fn a_selective_import_binds_only_its_names_and_no_qualifier() {
        let mut env = Env::new();
        env.register_module("hud", HUD);
        // The qualifier is not bound: `hud` is an undefined name at run time.
        let pid = env.load_program("import hud: draw\nprint(draw())\nprint(hud.draw())").unwrap();
        let sid = env.create_stack(pid).unwrap();
        let err = format!("{:?}", env.run(sid));
        assert!(err.contains("Undefined variable: hud"), "{err}");
        assert_eq!(env.take_output(), vec!["hud"]);
        let pid = env.load_program("import hud: draw\nprint(hud_only)").unwrap();
        let sid = env.create_stack(pid).unwrap();
        assert!(env.run(sid).is_err());
    }

    #[test]
    fn a_star_import_is_an_error_pointing_at_the_plain_form() {
        let err = load_err(&[("hud", HUD)], "import hud: *\nprint(draw())");
        assert!(err.contains("`import hud: *` is no longer a form"), "{err}");
        let err = load_err(
            &[("hud", HUD), ("f", "pub import hud: *")],
            "import f\nprint(1)",
        );
        assert!(err.contains("`pub import hud: *` is no longer a form"), "{err}");
    }

    #[test]
    fn two_plain_imports_offering_one_name_are_fine_until_it_is_used_bare() {
        let modules = [("hud", HUD), ("level", LEVEL)];
        assert_eq!(
            run(
                &modules,
                &[],
                "import hud\nimport level\nprint(hud.draw())\nprint(level.draw())\nprint(hud_only)",
            ),
            vec!["hud", "level", "1"]
        );
        let err = load_err(&modules, "import hud\nimport level\nprint(draw())\nlet d = draw");
        assert!(
            err.contains(
                "`draw` is ambiguous: 'hud' and 'level' export it — write `hud.draw` or \
                 `level.draw`, or pin one with `import hud: draw` [line 3, column 7]"
            ),
            "{err}"
        );
        // Each bare use is its own error.
        assert!(err.contains("[line 4, column 9]"), "{err}");
    }

    #[test]
    fn a_selective_import_or_a_declaration_settles_an_ambiguity() {
        let modules = [("hud", HUD), ("level", LEVEL)];
        assert_eq!(
            run(
                &modules,
                &[],
                "import hud\nimport level\nimport level: draw\nprint(draw())",
            ),
            vec!["level"]
        );
        assert_eq!(
            run(
                &modules,
                &[],
                "import hud\nimport level\nfn draw()\n  \"mine\"\nend\nprint(draw())",
            ),
            vec!["mine"]
        );
    }

    #[test]
    fn precedence_is_declaration_then_selective_then_plain_then_prelude() {
        // `sum` is std's; a plain import hides it, a selective import hides
        // that, a declaration hides them all.
        let modules = [
            ("a", "pub fn sum(xs)\n  \"a\"\nend"),
            ("b", "pub fn sum(xs)\n  \"b\"\nend"),
        ];
        assert_eq!(run(&modules, &[], "print(sum([1, 2]))"), vec!["3"]);
        assert_eq!(run(&modules, &[], "import a\nprint(sum([1, 2]))"), vec!["a"]);
        assert_eq!(
            run(&modules, &[], "import a\nimport b: sum\nprint(sum([1, 2]))"),
            vec!["b"]
        );
        assert_eq!(
            run(
                &modules,
                &[],
                "import a\nprint(sum([1, 2]))\nlet sum = fn(xs) \"mine\" end\nprint(sum([1]))",
            ),
            vec!["a", "mine"]
        );
        // A selective import and a declaration of one name is still the loud
        // collision it always was: both are explicit.
        let err = load_err(&modules, "import b: sum\nfn sum(xs)\n  1\nend\nprint(sum([1]))");
        assert!(err.contains("is imported from 'b' but is also declared"), "{err}");
    }

    #[test]
    fn an_importers_export_shadows_the_export_of_what_it_imports() {
        // `kit` is built on `base` and redeclares `button` on purpose; a file
        // importing both gets kit's, whichever order it imports them in.
        let modules = [
            ("base", "pub fn button(l)\n  \"base\"\nend\npub let base_only = 1"),
            ("kit", "import base\npub fn button(l)\n  \"kit\"\nend"),
            ("deep", "import kit\npub fn button(l)\n  \"deep\"\nend"),
        ];
        for order in ["import base\nimport kit\n", "import kit\nimport base\n"] {
            assert_eq!(
                run(&modules, &[], &format!("{order}print(button(1))\nprint(base_only)")),
                vec!["kit", "1"]
            );
        }
        // Transitively too.
        assert_eq!(
            run(&modules, &[], "import base\nimport deep\nprint(button(1))"),
            vec!["deep"]
        );
    }

    #[test]
    fn one_declaration_reached_by_two_routes_is_not_ambiguous() {
        let modules = [
            ("impl", "pub fn go()\n  \"go\"\nend"),
            ("facade", "pub import impl"),
        ];
        assert_eq!(
            run(&modules, &[], "import impl\nimport facade\nprint(go())"),
            vec!["go"]
        );
    }

    #[test]
    fn disjoint_overload_sets_from_two_plain_imports_merge_with_a_warning() {
        let mut env = Env::new();
        env.register_module("hud", HUD);
        env.register_module("level", LEVEL);
        let pid = env
            .load_program("import hud\nimport level\nprint(score(1))\nprint(score(1, 2))")
            .unwrap();
        let warnings: Vec<String> = env
            .get_program(pid)
            .unwrap()
            .warnings
            .iter()
            .map(|w| w.message.clone())
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains(
                "`score` is exported by 'hud' and 'level' with different parameter counts"
            ),
            "{warnings:?}"
        );
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), vec!["hud1", "level2"]);
    }

    #[test]
    fn the_same_arity_from_two_unrelated_modules_is_ambiguous_at_the_call() {
        let err = load_err(
            &[
                ("a", "pub fn f(x)\n  1\nend\npub fn f(x, y)\n  2\nend"),
                ("b", "pub fn f(x)\n  3\nend"),
            ],
            "import a\nimport b\nprint(f(1))",
        );
        assert!(err.contains("`f` is ambiguous: 'a' and 'b' export it"), "{err}");
    }

    /// The warnings of a program that loads.
    fn warnings(modules: &[(&str, &str)], entry: &str) -> Vec<String> {
        let mut env = Env::new();
        for (name, source) in modules {
            env.register_module(name, source);
        }
        let pid = env.load_program(entry).unwrap();
        env.get_program(pid)
            .unwrap()
            .warnings
            .iter()
            .map(|w| w.message.clone())
            .collect()
    }

    #[test]
    fn a_plain_import_hiding_a_builtin_warns_once_where_it_is_used() {
        let tween = [("tween", "pub fn lerp(a)\n  a\nend\npub fn other()\n  1\nend")];
        let w = warnings(&tween, "import tween\nprint(lerp(1))\nprint(lerp(2))");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("`lerp` here is the one module 'tween' exports, which hides the builtin"),
            "{w:?}"
        );
        // Not used bare: nothing to say. Qualified, selective: on purpose.
        assert!(warnings(&tween, "import tween\nprint(other())").is_empty());
        assert!(warnings(&tween, "import tween\nprint(tween.lerp(1))").is_empty());
        assert!(warnings(&tween, "import tween: lerp\nprint(lerp(1))").is_empty());
        // A std name counts as a builtin too.
        let w = warnings(&[("m", "pub fn sum(xs)\n  0\nend")], "import m\nprint(sum([1]))");
        assert_eq!(w.len(), 1, "{w:?}");
    }

    #[test]
    fn a_first_party_package_hides_a_builtin_without_a_warning() {
        let w = warnings(
            &[("ui", "pub fn lerp(a)\n  a\nend")],
            "import ui\nprint(lerp(1))",
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn a_module_that_exports_its_own_name_is_still_a_qualifier() {
        // `import noise` binds the qualifier and, bare, the function.
        assert_eq!(
            run(
                &[("noise", "pub fn noise(x)\n  \"n\"\nend\npub fn fbm(x)\n  \"f\"\nend")],
                &[],
                "import noise\nprint(noise(1))\nprint(noise.fbm(1))\nprint(noise.noise(1))",
            ),
            vec!["n", "f", "n"]
        );
    }

    // ---- relative paths and canonical identity --------------------------

    #[test]
    fn a_relative_import_between_memory_modules_resolves_against_the_importers_name() {
        assert_eq!(
            run(
                &[
                    ("bloom/menu", "import ./motion\npub fn open()\n  ease()\nend"),
                    ("bloom/motion", "pub fn ease()\n  \"eased\"\nend"),
                    ("bloom/widgets/button", "import ../motion\npub fn b()\n  motion.ease()\nend"),
                ],
                &[],
                "import bloom/menu\nimport bloom/widgets/button\nprint(open())\nprint(b())",
            ),
            vec!["eased", "eased"]
        );
        let err = load_err(
            &[("a", "import ../x\npub let v = 1")],
            "import a\nprint(1)",
        );
        assert!(err.contains("climbs above"), "{err}");
    }

    #[test]
    fn a_relative_import_from_a_program_with_no_path_names_a_registered_module() {
        assert_eq!(
            run(&[("hud", HUD)], &[], "import ./hud\nprint(draw())"),
            vec!["hud"]
        );
        let err = load_err(&[], "import ./nope\nprint(1)");
        assert!(err.contains("has no file path"), "{err}");
    }

    #[test]
    fn a_package_module_is_one_module_however_it_is_reached() {
        // `./motion` inside the package and `bloom/motion` from outside: one
        // module, so its top-level code runs once and its state is shared.
        let mut env = Env::new();
        env.register_package(
            "bloom",
            [
                ("motion", "print(\"motion loaded\")\npub var n = 0\npub fn bump()\n  set n = get n + 1\n  get n\nend"),
                ("menu", "import ./motion\npub fn open()\n  motion.bump()\nend"),
                ("bar", "import bloom/motion\npub fn tick()\n  motion.bump()\nend"),
            ],
        )
        .unwrap();
        let pid = env
            .load_program(
                "import bloom/menu\nimport bloom/bar\nimport bloom/motion\n\
                 print(menu.open())\nprint(bar.tick())\nprint(motion.bump())",
            )
            .unwrap();
        let sid = env.create_stack(pid).unwrap();
        env.run(sid).unwrap();
        assert_eq!(env.take_output(), vec!["motion loaded", "1", "2", "3"]);
        let names: Vec<String> = env
            .module_manifest(pid)
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(names.iter().filter(|n| n.contains("motion")).count(), 1, "{names:?}");
        assert!(names.contains(&"bloom/motion.ptl".to_string()), "{names:?}");
    }

    #[test]
    fn a_relative_import_may_not_leave_an_in_memory_package() {
        let mut env = Env::new();
        env.register_module("secret", "pub let v = 1");
        env.register_package("bloom", [("menu", "import ../secret\npub let m = 1")])
            .unwrap();
        let err = env.load_program("import bloom/menu\nprint(1)").unwrap_err();
        assert!(err.contains("leaves the package 'bloom'"), "{err}");
    }

    #[test]
    fn a_bare_name_is_a_package_never_a_missing_file() {
        let err = load_err(&[], "import nope\nprint(1)");
        assert!(err.contains("there is no package named 'nope'"), "{err}");
    }

    #[test]
    fn a_bundled_package_is_the_last_place_a_name_is_looked_up() {
        // `core/math` is bundled; a host registration of the same path wins.
        assert_eq!(run(&[], &[], "import core/math\nprint(abs(-2))"), vec!["2"]);
        assert_eq!(
            run(
                &[("core/math", "pub fn mine()\n  \"host\"\nend")],
                &[],
                "import core/math\nprint(mine())",
            ),
            vec!["host"]
        );
    }

    #[test]
    fn normalizing_a_path_folds_dots_without_the_filesystem() {
        use std::path::Path;
        assert_eq!(
            super::normalize_lexical(Path::new("/a/b/./c/../d.ptl")),
            Some(Path::new("/a/b/d.ptl").to_path_buf())
        );
        assert_eq!(super::normalize_lexical(Path::new("a/../../x")), None);
        assert_eq!(
            super::relative_module_name(Path::new("/p/game"), Path::new("/p/game/ui/hud.ptl")),
            "ui/hud"
        );
        assert_eq!(
            super::relative_module_name(Path::new("/p/game"), Path::new("/p/shared/rng.ptl")),
            "../shared/rng"
        );
    }

    #[test]
    fn a_plain_import_without_pub_binds_locally_only() {
        let err = load_err(
            &[
                ("impl", "pub fn a()\n  \"a\"\nend"),
                ("plain", "import impl\npub fn use_a()\n  a()\nend"),
            ],
            "import plain: a\nprint(a())",
        );
        assert!(err.contains("module 'plain' has no export 'a'"), "{err}");
        assert_eq!(
            run(
                &[
                    ("impl", "pub fn a()\n  \"a\"\nend"),
                    ("plain", "import impl\npub fn use_a()\n  a()\nend"),
                ],
                &[],
                "import plain: use_a\nprint(use_a())",
            ),
            vec!["a"]
        );
    }

    #[test]
    fn a_re_exported_name_reaches_a_nested_path_facade() {
        assert_eq!(
            run(
                &[
                    ("bloom/button", BUTTON),
                    ("bloom/menu", "pub fn open()\n  \"m\"\nend"),
                    (
                        "bloom/all",
                        "pub import bloom/button\npub import bloom/menu",
                    ),
                ],
                &[],
                "import bloom/all: button, open\nprint(button(\"a\"))\nprint(open())",
            ),
            vec!["[a]", "m"]
        );
    }

    #[test]
    fn a_plain_re_export_composes_with_cross_module_overload_merging() {
        // The host prelude has `f(a)`; a library adds `f(a, b)`; the facade
        // star-re-exports the library. Both arities have to survive the two
        // hops — the merge inside the facade, and the merge at the importer.
        assert_eq!(
            run(
                &[
                    ("ui", "pub fn f(a)\n  \"ui\" ++ a\nend"),
                    ("ext", "pub fn f(a, b)\n  \"ext\" ++ a ++ b\nend"),
                    ("facade", "pub import ext"),
                ],
                &["ui"],
                "import facade: f\nprint(f(\"x\"))\nprint(f(\"x\", \"y\"))",
            ),
            vec!["uix", "extxy"]
        );
    }

    #[test]
    fn a_plain_re_export_carries_classes_and_vars() {
        assert_eq!(
            run(
                &[
                    (
                        "impl",
                        "pub class Point\n  x: int\nend\n\
                         pub var hits = 0\n\
                         pub fn bump()\n  set hits = get hits + 1\nend",
                    ),
                    ("facade", "pub import impl"),
                ],
                &[],
                "import facade: Point, hits, bump\n\
                 bump()\nlet p = Point(3)\nprint(str(p.x) ++ \"/\" ++ str(hits))",
            ),
            vec!["3/1"]
        );
    }
}
