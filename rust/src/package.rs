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
//! A package's *internal* imports keep working with plain flat names — a
//! module in `bloom` reaches its sibling with `import motion`, because the
//! sibling sits in the same directory (and, for an in-memory package, because
//! [`resolve_in_packages`] looks in the importer's own package first). The
//! `bloom/`-prefixed spelling works too.
//!
//! Scope, deliberately: this is a manifest and a reachability rule. There is
//! no registry, no fetching, no dependency resolution, no lockfile, and no
//! version constraints — `version` is metadata a host can print.
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
}

impl PackageInfo {
    /// `bloom 0.1.0` / `bloom` — the one-line identity used in listings.
    pub fn label(&self) -> String {
        match &self.version {
            Some(v) => format!("{} {}", self.name, v),
            None => self.name.clone(),
        }
    }
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
        let manifest = parse_manifest(&text, &manifest_path)?;

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
        let mut sources = BTreeMap::new();
        for (module, source) in modules {
            let module = module.as_ref();
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
        Ok(Package {
            info: PackageInfo {
                name: name.to_string(),
                version: None,
                module_dir: root.clone(),
                root,
                modules: names,
                in_memory: true,
            },
            sources: Some(sources),
        })
    }

    /// Load one module of this package by its package-relative name, or
    /// `None` when the package has no such module.
    fn load(&self, module: &str) -> Option<ModuleSource> {
        if relative_module_error(module).is_some() {
            return None;
        }
        let full_name = format!("{}/{}", self.info.name, module);
        match &self.sources {
            Some(sources) => sources.get(module).map(|source| ModuleSource {
                name: full_name,
                source: source.clone(),
                // A synthetic path, so a flat sibling import from this module
                // lands back in this package (see `resolve_in_packages`).
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

    /// The module name an importing file inside this package would mean by a
    /// flat `import <name>`: the sibling directory it sits in, plus the name.
    /// `None` when the importer is not inside this package.
    fn sibling_of(&self, importer: &ModuleOrigin, name: &str) -> Option<String> {
        let ModuleOrigin::File(path) = importer else {
            return None;
        };
        let dir = path.parent()?;
        let rel = dir.strip_prefix(&self.info.module_dir).ok()?;
        let mut parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .collect();
        parts.push(name.to_string());
        Some(parts.join("/"))
    }
}

/// Resolve `name` against the registered packages, or `None`. Consulted by
/// `ModuleRegistry::resolve` after in-memory registrations and before the
/// filesystem search paths.
///
/// Three shapes resolve here:
/// 1. `bloom/menu` — a leading segment naming a package.
/// 2. `menu`, from a module of a package — its own package first, so a
///    library's internal flat imports keep working with no filesystem at all.
///    (An on-disk package would find the same file anyway, one step later,
///    through the importer-directory search.) Note this does not *rename* the
///    import: a module's identity is still the path the importing file wrote,
///    so a file reached as both `motion` and `bloom/motion` in one program is
///    two modules — a library should pick one spelling for itself.
/// 3. `bloom` — a package's facade module, `<module_dir>/bloom.ptl`.
pub fn resolve_in_packages(
    packages: &[Package],
    name: &str,
    importer: Option<&ModuleOrigin>,
) -> Option<ModuleSource> {
    if let Some((head, rest)) = name.split_once('/') {
        let pkg = packages.iter().find(|p| p.info.name == head)?;
        return pkg.load(rest);
    }

    if let Some(importer) = importer {
        for pkg in packages {
            if let Some(module) = pkg.sibling_of(importer, name)
                && let Some(found) = pkg.load(&module)
            {
                return Some(found);
            }
        }
    }

    packages
        .iter()
        .find(|p| p.info.name == name)
        .and_then(|pkg| pkg.load(name))
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

#[derive(Debug)]
struct Manifest {
    name: String,
    version: Option<String>,
    modules: Option<String>,
}

/// Parse the sliver of TOML a manifest needs: `[section]` headers, `key =
/// "string"` pairs, `#` comments, blank lines. Anything else is an error
/// naming the line — better a clear complaint than a silently ignored key.
/// (Petal has no TOML dependency and this is not the place to grow one.)
fn parse_manifest(text: &str, path: &Path) -> Result<Manifest, LoadError> {
    // A Windows editor may have written a UTF-8 BOM. It is invisible, so
    // leaving it in makes the first line fail with an error the author cannot
    // see the cause of ("found '\u{feff}[package]'").
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut section = String::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut modules: Option<String> = None;
    let mut saw_package_section = false;

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
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(manifest_error(
                path,
                format!("line {line_no}: expected 'key = \"value\"', found '{line}'"),
            ));
        };
        let key = key.trim();
        let value = parse_string_value(value.trim()).ok_or_else(|| {
            manifest_error(
                path,
                format!(
                    "line {line_no}: the value of '{key}' must be a quoted string, found '{}'",
                    value.trim()
                ),
            )
        })?;
        if section != "package" {
            // Other tables are somebody else's metadata; keep them legal so a
            // manifest can carry more than this version understands.
            continue;
        }
        match key {
            "name" => name = Some(value),
            "version" => version = Some(value),
            "modules" => modules = Some(value),
            // Same reasoning as above, one level down.
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
    Ok(Manifest {
        name,
        version,
        modules,
    })
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
    let ok = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
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
        parse_manifest(text, Path::new("petal.toml"))
    }

    /// A throwaway package directory: `<tmp>/petal-pkg-<tag>/lib/`, with the
    /// given manifest text and `src/menu.ptl`.
    fn temp_package(tag: &str, manifest_text: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("petal-pkg-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let lib = root.join("lib");
        std::fs::create_dir_all(lib.join("src")).unwrap();
        std::fs::write(lib.join(MANIFEST_FILE), manifest_text).unwrap();
        std::fs::write(lib.join("src/menu.ptl"), "export fn open() 1 end\n").unwrap();
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
        assert_eq!(pkg.load("menu").unwrap().name, "bloom/menu");
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
            [("menu.ptl", "export fn a() 1 end"), ("motion", "")],
        )
        .unwrap();
        assert_eq!(pkg.info.modules, vec!["menu", "motion"]);
        assert!(pkg.load("menu").is_some());
        assert!(pkg.load("nope").is_none());
        assert_eq!(pkg.load("menu").unwrap().name, "bloom/menu");
    }

    #[test]
    fn an_in_memory_module_finds_its_sibling_flat() {
        let pkg = Package::in_memory("bloom", [("menu", "a"), ("motion", "b")]).unwrap();
        let importer = pkg.load("menu").unwrap().origin;
        let found = resolve_in_packages(std::slice::from_ref(&pkg), "motion", Some(&importer));
        assert_eq!(found.map(|m| m.name), Some("bloom/motion".to_string()));
    }

    #[test]
    fn a_traversing_module_name_resolves_to_nothing() {
        let pkg = Package::in_memory("bloom", [("menu", "a")]).unwrap();
        assert!(resolve_in_packages(std::slice::from_ref(&pkg), "bloom/../x", None).is_none());
        assert!(Package::in_memory("bloom", [("../x", "a")]).is_err());
    }
}
