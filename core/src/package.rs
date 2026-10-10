//! Packages: `petal.toml` manifests and the reachability they buy.
//!
//! A *package* is a directory with a `petal.toml` at its root:
//!
//! ```toml
//! [package]
//! name = "bloom"
//! version = "0.1.0"
//! modules = "src"      # optional; defaults to "src" when it exists,
//!                      # else the manifest's own directory
//! ```
//!
//! The manifest records what "copy this directory somewhere reachable" never
//! could: the library's *name*, its version, and the fact that the `.ptl`
//! files under it belong together. Once a package is known to an [`Env`], its
//! modules are importable under the package name, composing with namespaced
//! module paths (docs/module-system.md):
//!
//! ```petal ignore
//! import bloom/menu
//! import bloom/menu: open, close
//! ```
//!
//! Inside a package, modules reach each other with a relative import —
//! `import ./motion` — or by the package's own name, `import bloom/motion`.
//! Both name the same module (see [`Package::canonical_name`]).
//!
//! A manifest also names what the package is built on:
//!
//! ```toml
//! [dependencies]            # the KEY is the import name
//! text_layout = { path = "../text-layout" }
//! tween       = { git = "https://example.org/tween", tag = "v1.2.0" }
//! shapes      = "0.3"       # a registry version: parsed, not supported yet
//!
//! [requires]                # host interfaces: provided by the runner
//! ui = ">=0.1"
//! ```
//!
//! A `path` dependency is resolved when a file of the package imports it. A
//! `git` dependency is parsed here and fetched by the package manager; a
//! registry version is a clear "not supported yet" error where it is
//! imported. `[requires]` names host interfaces — what a host announces with
//! `Env::provides` — and is checked when a program is loaded to run.
//!
//! [`Env`]: crate::env::Env

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use crate::error::{LoadError, Phase};
use crate::module::{ModuleOrigin, ModuleSource};

/// The manifest file name at a package root.
pub const MANIFEST_FILE: &str = "petal.toml";

/// The directory a manifest uses for modules when it does not say — but only
/// when it exists; otherwise the manifest's own directory is the module dir.
const DEFAULT_MODULE_DIR: &str = "src";

/// How deep the module scan walks under a package's module directory. A
/// package is a library, not a filesystem; the cap keeps a stray symlink loop
/// from turning registration into a hang.
const MAX_SCAN_DEPTH: usize = 8;

/// Directories the module scan never enters: build output, never Petal source.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules"];

/// What a package is, as a host or a script should see it. Returned by
/// `Env::add_package` / `Env::register_package` and listed by `Env::packages`.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageInfo {
    /// The `[package] name`. Also the first segment of every import that
    /// reaches into this package (`bloom/menu`).
    pub name: String,
    /// The `[package] version`, verbatim. Metadata — nothing resolves on it.
    pub version: Option<String>,
    /// The directory holding `petal.toml`, or the synthetic
    /// `<package:NAME>` marker for an in-memory package.
    pub root: PathBuf,
    /// The directory the modules live in (`<root>/src` by default).
    pub module_dir: PathBuf,
    /// Module names relative to `module_dir`, `/`-joined and sorted — the
    /// names that follow `<package>/` in an import.
    pub modules: Vec<String>,
    /// True when the sources were handed over in memory rather than read
    /// from disk (`Env::register_package`).
    pub in_memory: bool,
    /// The manifest's `[dependencies]`, in the order written.
    pub dependencies: Vec<Dependency>,
    /// The manifest's `[requires]`: the host interfaces this package needs.
    pub requires: Vec<Requirement>,
}

impl PackageInfo {
    /// `bloom 0.1.0` / `bloom` — the one-line identity used in listings.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {}", self.name, v),
            None => self.name.clone(),
        }
    }

    /// The `[dependencies]` entry whose key (the import name) is `name`.
    pub fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.dependencies.iter().find(|d| d.name == name)
    }
}

/// One `[dependencies]` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// The key: the first segment a script writes after `import`, whatever
    /// the package calls itself.
    pub name: String,
    pub source: DependencySource,
}

/// Where a dependency comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum DependencySource {
    /// `{ path = "../lib" }`, relative to the manifest's directory.
    Path(String),
    /// `{ git = "url", tag = "v1" }` or `{ git = "url", rev = "abc123" }`.
    Git {
        url: String,
        tag: Option<String>,
        rev: Option<String>,
    },
    /// `"0.3"`: a registry version. Parsed; registries are not supported yet.
    Registry(String),
}

impl std::fmt::Display for DependencySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencySource::Path(path) => write!(f, "path {path}"),
            DependencySource::Git { url, tag: Some(tag), .. } => write!(f, "git {url} tag {tag}"),
            DependencySource::Git { url, rev: Some(rev), .. } => write!(f, "git {url} rev {rev}"),
            DependencySource::Git { url, .. } => write!(f, "git {url}"),
            DependencySource::Registry(version) => write!(f, "registry {version}"),
        }
    }
}

/// One `[requires]` entry: a host interface and the versions of it that do.
#[derive(Debug, Clone, PartialEq)]
pub struct Requirement {
    pub interface: String,
    pub version: VersionReq,
}

/// A dotted numeric version (`0.4`, `1.2.3`). Missing components count as
/// zero, so `0.4` and `0.4.0` are equal.
#[derive(Debug, Clone)]
pub struct Version {
    parts: Vec<u64>,
    text: String,
}

impl Version {
    /// `1.2.3`, `0.4`, `v2`. A pre-release or build suffix (`-rc1`, `+x`) is
    /// accepted and ignored. `None` when no leading number is there.
    pub fn parse(text: &str) -> Option<Version> {
        let trimmed = text.trim();
        let body = trimmed.strip_prefix('v').unwrap_or(trimmed);
        let body = body.split(['-', '+']).next().unwrap_or(body);
        let parts: Option<Vec<u64>> = body.split('.').map(|p| p.parse().ok()).collect();
        let parts = parts?;
        Some(Version {
            parts,
            text: trimmed.to_string(),
        })
    }

    fn part(&self, i: usize) -> u64 {
        self.parts.get(i).copied().unwrap_or(0)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for Version {}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let n = self.parts.len().max(other.parts.len());
        (0..n)
            .map(|i| self.part(i).cmp(&other.part(i)))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Which versions satisfy a requirement.
///
/// - `">=0.4"`, or a bare `"0.4"`: that version or any later one.
/// - `"^1.2"`: at least 1.2, and the same major version.
/// - `"=1.2.0"`: exactly that version.
/// - `"*"`: any version.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionReq {
    op: VersionOp,
    /// The floor (or the exact version). `None` for `*`.
    version: Option<Version>,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum VersionOp {
    Any,
    AtLeast,
    Compatible,
    Exact,
}

impl VersionReq {
    pub fn parse(text: &str) -> Result<VersionReq, String> {
        let trimmed = text.trim();
        let (op, rest) = if trimmed == "*" {
            return Ok(VersionReq {
                op: VersionOp::Any,
                version: None,
                text: "*".to_string(),
            });
        } else if let Some(rest) = trimmed.strip_prefix(">=") {
            (VersionOp::AtLeast, rest)
        } else if let Some(rest) = trimmed.strip_prefix('^') {
            (VersionOp::Compatible, rest)
        } else if let Some(rest) = trimmed.strip_prefix('=') {
            (VersionOp::Exact, rest)
        } else {
            (VersionOp::AtLeast, trimmed)
        };
        let version = Version::parse(rest).ok_or_else(|| {
            format!(
                "'{trimmed}' is not a version requirement — write \">=0.4\", \"^1.2\", \
                 \"=1.2.0\" or \"*\""
            )
        })?;
        Ok(VersionReq {
            op,
            version: Some(version),
            text: if op == VersionOp::AtLeast && !trimmed.starts_with(">=") {
                format!(">={trimmed}")
            } else {
                trimmed.to_string()
            },
        })
    }

    /// Does `version` satisfy this requirement?
    pub fn matches(&self, version: &Version) -> bool {
        let Some(floor) = &self.version else {
            return true;
        };
        match self.op {
            VersionOp::Any => true,
            VersionOp::AtLeast => version >= floor,
            VersionOp::Compatible => version >= floor && version.part(0) == floor.part(0),
            VersionOp::Exact => version == floor,
        }
    }

    /// The lowest version that satisfies this (`None` for `*`).
    pub fn minimum(&self) -> Option<&Version> {
        self.version.as_ref()
    }
}

impl std::fmt::Display for VersionReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// A host interface some package of a program requires, and which one asked.
/// Collected onto [`crate::program::Program::requires`].
#[derive(Debug, Clone, PartialEq)]
pub struct RequiredInterface {
    pub interface: String,
    pub version: VersionReq,
    /// The package whose manifest says so.
    pub package: String,
}

impl std::fmt::Display for RequiredInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} ({})", self.interface, self.version, self.package)
    }
}

/// The nearest `petal.toml` at or above `start` (a directory, or a file whose
/// directory is the first one tried): the manifest every command and the
/// language server use for the file. `None` when no directory up to the
/// filesystem root holds one.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let start = crate::module::canonical_path(start);
    let mut dir: Option<&Path> = if start.is_dir() {
        Some(&start)
    } else {
        start.parent()
    };
    while let Some(d) = dir {
        let candidate = d.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// A registered package: its manifest facts plus, for an in-memory package,
/// the sources themselves.
#[derive(Debug)]
pub struct Package {
    pub info: PackageInfo,
    /// `Some` for an in-memory package, keyed by module name relative to the
    /// package (`menu`, `ui/menu`). `None` means "read from `module_dir`".
    sources: Option<BTreeMap<String, String>>,
}

impl Package {
    /// Read the manifest at `root/petal.toml` and index the modules under it.
    ///
    /// Every failure is a [`LoadError`] naming the file — a missing manifest,
    /// a manifest that does not parse, a missing `name`, a name that is not
    /// an identifier, or a `modules` directory that is not there.
    pub fn from_dir(root: &Path) -> Result<Package, LoadError> {
        let manifest_path = root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            manifest_error(
                &manifest_path,
                format!("cannot read package manifest: {}", e),
            )
        })?;
        let manifest = Manifest::parse(&text, &manifest_path)?;

        let module_dir = match &manifest.modules {
            Some(rel) => {
                if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|s| s == "..") {
                    return Err(manifest_error(
                        &manifest_path,
                        format!(
                            "modules = \"{rel}\" must be a relative directory under the package \
                             root (no leading '/', no '..')"
                        ),
                    ));
                }
                let dir = rel.split('/').filter(|s| !s.is_empty() && *s != ".").fold(
                    root.to_path_buf(),
                    |mut acc, seg| {
                        acc.push(seg);
                        acc
                    },
                );
                if !dir.is_dir() {
                    return Err(manifest_error(
                        &manifest_path,
                        format!(
                            "modules = \"{rel}\" names '{}', which is not a directory",
                            dir.display()
                        ),
                    ));
                }
                dir
            }
            None => {
                let default = root.join(DEFAULT_MODULE_DIR);
                if default.is_dir() {
                    default
                } else {
                    root.to_path_buf()
                }
            }
        };

        let mut modules = Vec::new();
        scan_modules(&module_dir, "", 0, &mut modules);
        modules.sort();

        Ok(Package {
            info: PackageInfo {
                name: manifest.name,
                version: manifest.version,
                root: root.to_path_buf(),
                module_dir,
                modules,
                in_memory: false,
                dependencies: manifest.dependencies,
                requires: manifest.requires,
            },
            sources: None,
        })
    }

    /// A package whose sources are handed over directly — the `include_str!`
    /// / `include_dir!` / wasm case. `modules` yields `(module name, source)`
    /// pairs, the module name relative to the package (`menu`, `ui/menu`,
    /// with or without a `.ptl` suffix, which is stripped).
    pub fn in_memory<N: AsRef<str>, S: AsRef<str>>(
        name: &str,
        modules: impl IntoIterator<Item = (N, S)>,
    ) -> Result<Package, LoadError> {
        check_package_name(name, &PathBuf::from(format!("<package:{name}>")))?;
        let root = PathBuf::from(format!("<package:{name}>"));
        let modules = modules.into_iter();
        let mut sources = BTreeMap::new();
        let mut manifest: Option<Manifest> = None;
        for (module, source) in modules {
            let module = module.as_ref();
            // The manifest may ride along with the sources, under its file
            // name: it carries the version, `[dependencies]` and `[requires]`.
            if module == MANIFEST_FILE {
                let parsed = Manifest::parse(source.as_ref(), &root.join(MANIFEST_FILE))?;
                if parsed.name != name {
                    return Err(LoadError::message(
                        Phase::Module,
                        format!(
                            "package '{name}': its {MANIFEST_FILE} names the package '{}'",
                            parsed.name
                        ),
                    ));
                }
                manifest = Some(parsed);
                continue;
            }
            let module = module.strip_suffix(".ptl").unwrap_or(module);
            if let Some(problem) = relative_module_error(module) {
                return Err(LoadError::message(
                    Phase::Module,
                    format!("package '{name}': {problem}"),
                ));
            }
            sources.insert(module.to_string(), source.as_ref().to_string());
        }
        let names: Vec<String> = sources.keys().cloned().collect();
        let (version, dependencies, requires) = match manifest {
            Some(m) => (m.version, m.dependencies, m.requires),
            None => (None, Vec::new(), Vec::new()),
        };
        Ok(Package {
            info: PackageInfo {
                name: name.to_string(),
                version,
                module_dir: root.clone(),
                root,
                modules: names,
                in_memory: true,
                dependencies,
                requires,
            },
            sources: Some(sources),
        })
    }

    /// Load one module of this package by its package-relative name, or
    /// `None` when the package has no such module.
    pub(crate) fn load(&self, module: &str) -> Option<ModuleSource> {
        if relative_module_error(module).is_some() {
            return None;
        }
        let full_name = Some(self.canonical_name(module));
        match &self.sources {
            Some(sources) => sources.get(module).map(|source| ModuleSource {
                name: full_name,
                source: source.clone(),
                // A synthetic path, so a relative import from this module
                // lands back in this package.
                origin: ModuleOrigin::File(module_file_path(&self.info.module_dir, module)),
            }),
            None => {
                let path = module_file_path(&self.info.module_dir, module);
                std::fs::read_to_string(&path)
                    .ok()
                    .map(|source| ModuleSource {
                        name: full_name,
                        source,
                        origin: ModuleOrigin::File(path),
                    })
            }
        }
    }

    /// The source of the in-memory module whose synthetic origin is `path`
    /// (the one [`load`](Self::load) stamps on it). `None` for a package read
    /// from disk, whose modules are simply files.
    pub(crate) fn memory_source_at(&self, path: &Path) -> Option<&str> {
        let sources = self.sources.as_ref()?;
        sources
            .iter()
            .find(|(module, _)| module_file_path(&self.info.module_dir, module) == path)
            .map(|(_, source)| source.as_str())
    }

    /// The canonical name of this package's module `module` (its path under
    /// the module directory): `bloom/menu`, and plain `bloom` for the module
    /// named after the package — the one `import bloom` loads.
    pub fn canonical_name(&self, module: &str) -> String {
        if module == self.info.name {
            module.to_string()
        } else {
            format!("{}/{}", self.info.name, module)
        }
    }

    /// A package compiled into the binary (see [`crate::bundled`]).
    pub fn bundled(bundled: &crate::bundled::BundledPackage) -> Result<Package, LoadError> {
        let marker = PathBuf::from(format!("<bundled:{}>", bundled.name));
        let manifest = Manifest::parse(bundled.manifest(), &marker.join(MANIFEST_FILE))?;
        let prefix = match &manifest.modules {
            Some(dir) => format!("{}/", dir.trim_matches('/')),
            None if bundled.sources().any(|(p, _)| p.starts_with("src/")) => "src/".to_string(),
            None => String::new(),
        };
        let mut package = Package::in_memory(
            &manifest.name,
            bundled
                .sources()
                .filter_map(|(path, source)| Some((path.strip_prefix(prefix.as_str())?, source))),
        )?;
        package.info.version = manifest.version;
        package.info.dependencies = manifest.dependencies;
        package.info.requires = manifest.requires;
        package.info.root = marker.clone();
        package.info.module_dir = marker;
        Ok(package)
    }
}

/// Every package directly under `dir`, plus `dir` itself when it is a package
/// root. Returns what it found *and* what it could not read, so a caller can
/// choose between reporting a broken manifest and ignoring it — a search path
/// is allowed to hold directories that are not packages, but a directory that
/// claims to be one by having a `petal.toml` and then fails to parse is worth
/// hearing about.
pub fn discover_packages(dir: &Path) -> (Vec<Package>, Vec<LoadError>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    let consider = |root: PathBuf, found: &mut Vec<Package>, errors: &mut Vec<LoadError>| {
        if !root.join(MANIFEST_FILE).is_file() {
            return;
        }
        match Package::from_dir(&root) {
            Ok(pkg) => found.push(pkg),
            Err(e) => errors.push(e),
        }
    };

    consider(dir.to_path_buf(), &mut found, &mut errors);
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && !file_name_of(p).starts_with('.'))
            .collect();
        subdirs.sort();
        for sub in subdirs {
            consider(sub, &mut found, &mut errors);
        }
    }
    (found, errors)
}

// ── manifest parsing ─────────────────────────────────────────────

/// A parsed `petal.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: Option<String>,
    pub modules: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub requires: Vec<Requirement>,
}

/// A manifest value: a quoted string, or an inline table of them.
enum TomlValue {
    Str(String),
    Table(Vec<(String, String)>),
}

impl Manifest {
    /// Read and parse `<dir>/petal.toml`.
    pub fn load(dir: &Path) -> Result<Manifest, LoadError> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| manifest_error(&path, format!("cannot read package manifest: {e}")))?;
        Manifest::parse(&text, &path)
    }

    /// Parse the sliver of TOML a manifest needs: `[section]` headers, `key =
    /// "string"` pairs, `key = { k = "v", … }` inline tables, `#` comments,
    /// blank lines. Anything else is an error naming the line — better a
    /// clear complaint than a silently ignored key. (Petal has no TOML
    /// dependency and this is not the place to grow one.) `path` is only for
    /// error messages.
    pub fn parse(text: &str, path: &Path) -> Result<Manifest, LoadError> {
        // A Windows editor may have written a UTF-8 BOM. It is invisible, so
        // leaving it in makes the first line fail with an error the author
        // cannot see the cause of ("found '\u{feff}[package]'").
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut section = String::new();
        let mut name: Option<String> = None;
        let mut version: Option<String> = None;
        let mut modules: Option<String> = None;
        let mut saw_package_section = false;
        // Dependencies in the order written; each is the keys seen so far
        // (an inline table arrives whole, a `[dependencies.x]` section key by
        // key) or a bare version string.
        let mut deps: Vec<(String, usize, Result<Vec<(String, String)>, String>)> = Vec::new();
        let mut requires: Vec<Requirement> = Vec::new();

        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(header) = line.strip_prefix('[') {
                let Some(header) = header.strip_suffix(']') else {
                    return Err(manifest_error(
                        path,
                        format!("line {line_no}: unterminated section header '{line}'"),
                    ));
                };
                section = header.trim().to_string();
                if section == "package" {
                    saw_package_section = true;
                }
                if let Some(dep) = section.strip_prefix("dependencies.") {
                    let dep = dep.trim().to_string();
                    check_import_name(&dep, "dependency", path, line_no)?;
                    if deps.iter().any(|(n, _, _)| *n == dep) {
                        return Err(manifest_error(
                            path,
                            format!("line {line_no}: dependency '{dep}' is listed twice"),
                        ));
                    }
                    deps.push((dep, line_no, Ok(Vec::new())));
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(manifest_error(
                    path,
                    format!("line {line_no}: expected 'key = \"value\"', found '{line}'"),
                ));
            };
            let key = key.trim();
            let value = parse_value(value.trim()).ok_or_else(|| {
                manifest_error(
                    path,
                    format!(
                        "line {line_no}: the value of '{key}' must be a quoted string or an \
                         inline table of them ({{ path = \"…\" }}), found '{}'",
                        value.trim()
                    ),
                )
            })?;
            let string_only = |value: TomlValue| match value {
                TomlValue::Str(s) => Ok(s),
                TomlValue::Table(_) => Err(manifest_error(
                    path,
                    format!("line {line_no}: the value of '{key}' must be a quoted string"),
                )),
            };
            match section.as_str() {
                "package" => match key {
                    "name" => name = Some(string_only(value)?),
                    "version" => version = Some(string_only(value)?),
                    "modules" => modules = Some(string_only(value)?),
                    // Somebody else's metadata; keep it legal so a manifest
                    // can carry more than this version understands.
                    _ => {}
                },
                "dependencies" => {
                    check_import_name(key, "dependency", path, line_no)?;
                    if deps.iter().any(|(n, _, _)| n == key) {
                        return Err(manifest_error(
                            path,
                            format!("line {line_no}: dependency '{key}' is listed twice"),
                        ));
                    }
                    deps.push((
                        key.to_string(),
                        line_no,
                        match value {
                            TomlValue::Str(version) => Err(version),
                            TomlValue::Table(pairs) => Ok(pairs),
                        },
                    ));
                }
                "requires" => {
                    check_import_name(key, "interface", path, line_no)?;
                    let text = string_only(value)?;
                    let req = VersionReq::parse(&text).map_err(|e| {
                        manifest_error(path, format!("line {line_no}: [requires] {key}: {e}"))
                    })?;
                    if requires.iter().any(|r| r.interface == key) {
                        return Err(manifest_error(
                            path,
                            format!("line {line_no}: interface '{key}' is required twice"),
                        ));
                    }
                    requires.push(Requirement {
                        interface: key.to_string(),
                        version: req,
                    });
                }
                other if other.starts_with("dependencies.") => {
                    let text = string_only(value)?;
                    if let Some((_, _, Ok(pairs))) = deps.last_mut() {
                        pairs.push((key.to_string(), text));
                    }
                }
                // Other tables are somebody else's metadata.
                _ => {}
            }
        }

        if !saw_package_section {
            return Err(manifest_error(path, "missing the [package] section"));
        }
        let Some(name) = name else {
            return Err(manifest_error(path, "[package] has no 'name'"));
        };
        check_package_name(&name, path)?;
        let dependencies = deps
            .into_iter()
            .map(|(dep, line_no, spec)| {
                let source = dependency_source(&dep, spec)
                    .map_err(|e| manifest_error(path, format!("line {line_no}: {e}")))?;
                Ok(Dependency { name: dep, source })
            })
            .collect::<Result<Vec<_>, LoadError>>()?;
        Ok(Manifest {
            name,
            version,
            modules,
            dependencies,
            requires,
        })
    }
}

/// Turn a dependency's keys (or its bare version string) into a source.
fn dependency_source(
    name: &str,
    spec: Result<Vec<(String, String)>, String>,
) -> Result<DependencySource, String> {
    let pairs = match spec {
        Err(version) => {
            VersionReq::parse(&version).map_err(|e| format!("dependency '{name}': {e}"))?;
            return Ok(DependencySource::Registry(version));
        }
        Ok(pairs) => pairs,
    };
    let mut path = None;
    let mut git = None;
    let mut tag = None;
    let mut rev = None;
    let mut version = None;
    for (key, value) in pairs {
        let slot = match key.as_str() {
            "path" => &mut path,
            "git" => &mut git,
            "tag" => &mut tag,
            "rev" => &mut rev,
            "version" => &mut version,
            other => {
                return Err(format!(
                    "dependency '{name}' has an unknown key '{other}' — a dependency is \
                     {{ path = \"…\" }}, {{ git = \"…\", tag = \"…\" }}, {{ git = \"…\", rev = \
                     \"…\" }} or a version string"
                ));
            }
        };
        if slot.replace(value).is_some() {
            return Err(format!("dependency '{name}' gives '{key}' twice"));
        }
    }
    match (path, git, version) {
        (Some(path), None, _) => {
            if tag.is_some() || rev.is_some() {
                return Err(format!(
                    "dependency '{name}': 'tag' and 'rev' belong to a git dependency, not a path"
                ));
            }
            if path.is_empty() {
                return Err(format!("dependency '{name}' has an empty path"));
            }
            Ok(DependencySource::Path(path))
        }
        (None, Some(url), _) => match (&tag, &rev) {
            (Some(_), Some(_)) => Err(format!(
                "dependency '{name}' gives both 'tag' and 'rev' — pin a git dependency with one"
            )),
            (None, None) => Err(format!(
                "git dependency '{name}' needs a 'tag' or a 'rev' to pin it"
            )),
            _ => Ok(DependencySource::Git { url, tag, rev }),
        },
        (Some(_), Some(_), _) => Err(format!(
            "dependency '{name}' gives both 'path' and 'git' — it comes from one place"
        )),
        (None, None, Some(version)) => {
            VersionReq::parse(&version).map_err(|e| format!("dependency '{name}': {e}"))?;
            Ok(DependencySource::Registry(version))
        }
        (None, None, None) => Err(format!(
            "dependency '{name}' says nothing about where it comes from — give it a 'path', a \
             'git' url with a 'tag' or 'rev', or a version string"
        )),
    }
}

/// A string, or `{ k = "v", k2 = "v2" }`.
fn parse_value(text: &str) -> Option<TomlValue> {
    if let Some(s) = parse_string_value(text) {
        return Some(TomlValue::Str(s));
    }
    let inner = text.strip_prefix('{')?.strip_suffix('}')?.trim();
    let mut pairs = Vec::new();
    if inner.is_empty() {
        return Some(TomlValue::Table(pairs));
    }
    for part in split_outside_quotes(inner, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=')?;
        pairs.push((key.trim().to_string(), parse_string_value(value.trim())?));
    }
    Some(TomlValue::Table(pairs))
}

/// Split on `sep` wherever it is not inside a quoted string.
fn split_outside_quotes(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == sep => {
                out.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            None => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// A dependency key or an interface name is the first segment of an import
/// path (or names a package), so it must be an identifier.
fn check_import_name(name: &str, what: &str, path: &Path, line_no: usize) -> Result<(), LoadError> {
    if is_identifier(name) {
        Ok(())
    } else {
        Err(manifest_error(
            path,
            format!(
                "line {line_no}: {what} name '{name}' is not an identifier (letters, digits \
                 and '_', not starting with a digit)"
            ),
        ))
    }
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Drop a trailing `#` comment, respecting quotes.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' => return &line[..i],
            None => {}
        }
    }
    line
}

/// A bare quoted TOML string (no escapes — a manifest holds a name, a version
/// and a directory).
fn parse_string_value(text: &str) -> Option<String> {
    for quote in ['"', '\''] {
        if let Some(rest) = text.strip_prefix(quote)
            && let Some(inner) = rest.strip_suffix(quote)
            && !inner.contains(quote)
        {
            return Some(inner.to_string());
        }
    }
    None
}

/// A package name becomes the first segment of an import path, so it must be
/// spellable as an identifier.
fn check_package_name(name: &str, path: &Path) -> Result<(), LoadError> {
    if is_identifier(name) {
        Ok(())
    } else {
        Err(manifest_error(
            path,
            format!(
                "package name '{name}' is not an identifier — a package name is the first \
                 segment of an import path (letters, digits and '_', not starting with a digit)"
            ),
        ))
    }
}

/// Why a package-relative module name is unusable, or `None`. Mirrors the
/// module path rules: no empty segment, no `.`/`..`, no other separators.
fn relative_module_error(module: &str) -> Option<String> {
    if module.is_empty() {
        return Some("a module name may not be empty".to_string());
    }
    for segment in module.split('/') {
        if segment.is_empty() {
            return Some(format!("module name '{module}' has an empty segment"));
        }
        if segment == "." || segment == ".." {
            return Some(format!(
                "module name '{module}' may not contain '.' or '..'"
            ));
        }
        if segment.contains(['\\', ':']) {
            return Some(format!(
                "module name '{module}' may not contain '\\' or ':'"
            ));
        }
    }
    None
}

fn manifest_error(path: &Path, message: impl Display) -> LoadError {
    LoadError::message(Phase::Module, format!("{}: {message}", path.display()))
}

/// `<dir>/a/b.ptl` for the module `a/b`.
fn module_file_path(dir: &Path, module: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    let mut segments = module.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_some() {
            path.push(segment);
        } else {
            path.push(format!("{segment}.ptl"));
        }
    }
    path
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Collect `.ptl` files under `dir` as `/`-joined module names.
fn scan_modules(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = file_name_of(&path);
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            // Build output is not source, and a directory with a manifest of
            // its own is another package: its files are its modules, not
            // this one's.
            if SKIPPED_DIRS.contains(&name.as_str()) || path.join(MANIFEST_FILE).is_file() {
                continue;
            }
            let inner = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            scan_modules(&path, &inner, depth + 1, out);
        } else if let Some(stem) = name.strip_suffix(".ptl") {
            out.push(if prefix.is_empty() {
                stem.to_string()
            } else {
                format!("{prefix}/{stem}")
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Result<Manifest, LoadError> {
        Manifest::parse(text, Path::new("petal.toml"))
    }

    /// A throwaway package directory: `<tmp>/petal-pkg-<tag>/lib/`, with the
    /// given manifest text and `src/menu.ptl`.
    fn temp_package(tag: &str, manifest_text: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("petal-pkg-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let lib = root.join("lib");
        std::fs::create_dir_all(lib.join("src")).unwrap();
        std::fs::write(lib.join(MANIFEST_FILE), manifest_text).unwrap();
        std::fs::write(lib.join("src/menu.ptl"), "pub fn open() 1 end\n").unwrap();
        root
    }

    #[test]
    fn a_leading_utf8_bom_is_not_part_of_the_first_line() {
        // Written by a Windows editor. The BOM is invisible, so before this
        // the only symptom was `line 1: expected 'key = "value"', found
        // '\u{feff}[package]'`.
        let m = manifest("\u{feff}[package]\nname = \"bloom\"\n").unwrap();
        assert_eq!(m.name, "bloom");
    }

    #[test]
    fn a_directory_with_a_manifest_becomes_a_package() {
        let root = temp_package(
            "basic",
            "[package]\nname = \"bloom\"\nversion = \"0.1.0\"\n",
        );
        let pkg = Package::from_dir(&root.join("lib")).unwrap();
        assert_eq!(pkg.info.name, "bloom");
        assert_eq!(pkg.info.version.as_deref(), Some("0.1.0"));
        assert_eq!(pkg.info.modules, vec!["menu"]);
        assert_eq!(pkg.info.module_dir, root.join("lib/src"));
        assert_eq!(pkg.load("menu").unwrap().name.as_deref(), Some("bloom/menu"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn discovery_finds_a_package_one_level_down_and_reports_a_broken_one() {
        let root = temp_package("discover", "[package]\nname = \"bloom\"\n");
        let broken = root.join("nope");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(MANIFEST_FILE), "[package]\nname = bloom\n").unwrap();
        // A plain directory with no manifest is not a package and not an error.
        std::fs::create_dir_all(root.join("just-files")).unwrap();

        let (found, errors) = discover_packages(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].info.name, "bloom");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].to_string().contains("must be a quoted string"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_modules_directory_is_a_named_error() {
        let root = temp_package(
            "modules",
            "[package]\nname = \"bloom\"\nmodules = \"lib\"\n",
        );
        let err = Package::from_dir(&root.join("lib"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("petal.toml"), "{err}");
        assert!(err.contains("is not a directory"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_manifest_is_a_named_error() {
        let root = std::env::temp_dir().join("petal-pkg-absent");
        let err = Package::from_dir(&root).unwrap_err().to_string();
        assert!(err.contains("petal.toml"), "{err}");
        assert!(err.contains("cannot read package manifest"), "{err}");
    }

    #[test]
    fn parses_a_minimal_manifest() {
        let m = manifest("[package]\nname = \"bloom\"\nversion = \"0.1.0\"\n").unwrap();
        assert_eq!(m.name, "bloom");
        assert_eq!(m.version.as_deref(), Some("0.1.0"));
        assert_eq!(m.modules, None);
    }

    #[test]
    fn comments_and_blank_lines_are_fine() {
        let m = manifest("# a library\n\n[package]\nname = \"bloom\" # the name\n").unwrap();
        assert_eq!(m.name, "bloom");
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let m = manifest("[package]\nname = \"bloom\"\nversion = \"0.1#0\"\n").unwrap();
        assert_eq!(m.version.as_deref(), Some("0.1#0"));
    }

    #[test]
    fn unknown_keys_and_tables_are_carried_quietly() {
        let m = manifest("[package]\nname = \"bloom\"\nauthor = \"a\"\n[meta]\nx = \"y\"\n");
        assert_eq!(m.unwrap().name, "bloom");
    }

    #[test]
    fn malformed_lines_are_named() {
        let err = manifest("[package]\nname\n").unwrap_err().to_string();
        assert!(err.contains("line 2"), "{err}");
        assert!(err.contains("petal.toml"), "{err}");

        let err = manifest("[package]\nname = bloom\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be a quoted string"), "{err}");

        let err = manifest("[package\nname = \"b\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unterminated section header"), "{err}");
    }

    #[test]
    fn dependencies_come_in_three_forms() {
        let m = manifest(
            "[package]\nname = \"marble\"\n\n[dependencies]\n\
             bloom = \"0.3\"  # registry, later\n\
             text_layout = { path = \"../text-layout\" }\n\
             tween = { git = \"https://example.org/tween\", tag = \"v1.2.0\" }\n\
             pinned = { git = \"https://example.org/p#x\", rev = 'abc123' }\n",
        )
        .unwrap();
        let names: Vec<&str> = m.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["bloom", "text_layout", "tween", "pinned"]);
        assert_eq!(m.dependencies[0].source, DependencySource::Registry("0.3".into()));
        assert_eq!(
            m.dependencies[1].source,
            DependencySource::Path("../text-layout".into())
        );
        assert_eq!(
            m.dependencies[2].source,
            DependencySource::Git {
                url: "https://example.org/tween".into(),
                tag: Some("v1.2.0".into()),
                rev: None,
            }
        );
        assert_eq!(
            m.dependencies[3].source,
            DependencySource::Git {
                url: "https://example.org/p#x".into(),
                tag: None,
                rev: Some("abc123".into()),
            }
        );
    }

    #[test]
    fn a_dependency_may_be_a_table_section() {
        let m = manifest(
            "[package]\nname = \"marble\"\n[dependencies.tween]\n\
             git = \"https://example.org/tween\"\nrev = \"abc\"\n\
             [dependencies.kit]\npath = \"../kit\"\n",
        )
        .unwrap();
        assert_eq!(m.dependencies.len(), 2);
        assert_eq!(m.dependencies[1].source, DependencySource::Path("../kit".into()));
    }

    #[test]
    fn a_malformed_dependency_is_named_with_its_line() {
        let dep = |line: &str| {
            manifest(&format!("[package]\nname = \"m\"\n[dependencies]\n{line}\n"))
                .unwrap_err()
                .to_string()
        };
        assert!(dep("x = { git = \"u\" }").contains("needs a 'tag' or a 'rev'"));
        assert!(dep("x = { git = \"u\", tag = \"a\", rev = \"b\" }").contains("both 'tag' and 'rev'"));
        assert!(dep("x = { path = \"p\", git = \"u\", tag = \"t\" }").contains("both 'path' and 'git'"));
        assert!(dep("x = { path = \"p\", tag = \"t\" }").contains("belong to a git dependency"));
        assert!(dep("x = { branch = \"main\" }").contains("unknown key 'branch'"));
        assert!(dep("x = {}").contains("says nothing about where it comes from"));
        assert!(dep("x = \"latest\"").contains("is not a version requirement"));
        assert!(dep("my-lib = { path = \"p\" }").contains("is not an identifier"));
        assert!(dep("x = { path = \"a\" }\nx = { path = \"b\" }").contains("listed twice"));
        let err = dep("x = { path = 3 }");
        assert!(err.contains("line 4"), "{err}");
        assert!(err.contains("must be a quoted string"), "{err}");
    }

    #[test]
    fn requires_names_host_interfaces_and_version_ranges() {
        let m = manifest(
            "[package]\nname = \"bloom\"\n[requires]\nui = \">=0.4\"\nsdl = \"^1.2\"\nany = \"*\"\n",
        )
        .unwrap();
        let shown: Vec<String> = m
            .requires
            .iter()
            .map(|r| format!("{} {}", r.interface, r.version))
            .collect();
        assert_eq!(shown, ["ui >=0.4", "sdl ^1.2", "any *"]);
        let err = manifest("[package]\nname = \"b\"\n[requires]\nui = \"new\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("[requires] ui"), "{err}");
        assert!(manifest("[package]\nname = \"b\"\n[requires]\nui = { path = \"x\" }\n").is_err());
    }

    #[test]
    fn version_requirements_compare_numerically() {
        let v = |s: &str| Version::parse(s).unwrap();
        let req = |s: &str| VersionReq::parse(s).unwrap();
        assert!(v("0.10") > v("0.9"));
        assert_eq!(v("0.4"), v("0.4.0"));
        assert_eq!(v("v1.2.3-rc1"), v("1.2.3"));
        assert!(Version::parse("soon").is_none());
        assert!(req(">=0.4").matches(&v("0.4.0")));
        assert!(req(">=0.4").matches(&v("2")));
        assert!(!req(">=0.4").matches(&v("0.3.9")));
        // A bare version is a floor.
        assert!(req("0.4").matches(&v("0.5")));
        assert_eq!(req("0.4").to_string(), ">=0.4");
        assert!(req("^1.2").matches(&v("1.9")));
        assert!(!req("^1.2").matches(&v("2.0")));
        assert!(!req("^1.2").matches(&v("1.1")));
        assert!(req("=1.2.0").matches(&v("1.2")));
        assert!(!req("=1.2.0").matches(&v("1.2.1")));
        assert!(req("*").matches(&v("0")));
    }

    #[test]
    fn a_manifest_needs_a_package_name() {
        let err = manifest("[package]\nversion = \"1\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no 'name'"), "{err}");
        let err = manifest("name = \"b\"\n").unwrap_err().to_string();
        assert!(err.contains("[package]"), "{err}");
    }

    #[test]
    fn a_package_name_must_be_an_identifier() {
        let err = manifest("[package]\nname = \"my lib\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not an identifier"), "{err}");
        assert!(manifest("[package]\nname = \"bloom/menu\"\n").is_err());
        assert!(manifest("[package]\nname = \"2fast\"\n").is_err());
    }

    #[test]
    fn in_memory_packages_index_their_modules() {
        let pkg = Package::in_memory(
            "bloom",
            [("menu.ptl", "pub fn a() 1 end"), ("motion", "")],
        )
        .unwrap();
        assert_eq!(pkg.info.modules, vec!["menu", "motion"]);
        assert!(pkg.load("menu").is_some());
        assert!(pkg.load("nope").is_none());
        assert_eq!(pkg.load("menu").unwrap().name.as_deref(), Some("bloom/menu"));
    }

    #[test]
    fn the_module_named_after_the_package_is_the_package_itself() {
        let pkg = Package::in_memory("bloom", [("bloom", "a"), ("menu", "b")]).unwrap();
        assert_eq!(pkg.canonical_name("bloom"), "bloom");
        assert_eq!(pkg.canonical_name("menu"), "bloom/menu");
        assert_eq!(pkg.load("bloom").unwrap().name.as_deref(), Some("bloom"));
    }

    #[test]
    fn a_traversing_module_name_resolves_to_nothing() {
        let pkg = Package::in_memory("bloom", [("menu", "a")]).unwrap();
        assert!(pkg.load("../x").is_none());
        assert!(Package::in_memory("bloom", [("../x", "a")]).is_err());
    }

    #[test]
    fn a_bundled_package_is_an_in_memory_package() {
        let core = crate::bundled::package(crate::bundled::CORE_PACKAGE).unwrap();
        let pkg = Package::bundled(core).unwrap();
        assert_eq!(pkg.info.name, "core");
        assert!(pkg.info.in_memory);
        assert!(pkg.info.modules.contains(&"math".to_string()), "{:?}", pkg.info.modules);
        assert!(pkg.load("math").is_some());
    }
}
