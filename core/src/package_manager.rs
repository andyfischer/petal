//! The seam between module loading and the package manager.
//!
//! Loading a module never needs the package manager for what a host hands
//! over in memory (`register_module`, `register_package`), for the search
//! path, for path dependencies or for a hand-vendored `vendor/<name>`. Only a
//! **git** dependency of a package on disk does: its location comes from
//! `petal.lock` and the package cache, and with install-on-demand it may have
//! to be fetched first. That lives in the `petal-pkgman` crate
//! (`core-libs/petal-pkgman`), which the `petal` CLI links and registers with
//! [`register`]. Embedders — the browser runtimes, the SDL and C hosts — do
//! not link it; for them a git dependency is an error that says to bundle or
//! vendor the package instead.
//!
//! The registration is process-wide: every [`crate::env::Env`] made after it
//! (the CLI's, the language server's, the test runner's) resolves git
//! dependencies through it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::package::Requirement;

/// The lockfile beside a project's `petal.toml`.
pub const LOCK_FILE: &str = "petal.lock";

/// The directory, beside a project's `petal.toml`, of packages vendored into
/// it: `vendor/<name>`. The loader reads it without a package manager.
pub const VENDOR_DIR: &str = "vendor";

/// `$PETAL_HOME`, or `~/.petal`.
pub fn petal_home() -> PathBuf {
    if let Some(home) = std::env::var_os("PETAL_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".petal")
}

/// `<petal home>/cache`, where the package manager keeps fetched packages:
/// git databases under `git/`, checked-out trees under `trees/<sha256>`, and
/// the commit-to-tree index under `commits/`. Code under it is library code
/// to tools such as direct manipulation, with or without a package manager.
pub fn cache_dir() -> PathBuf {
    petal_home().join("cache")
}

/// A git dependency as a manifest writes it.
pub struct GitDependency<'a> {
    pub url: &'a str,
    pub tag: &'a Option<String>,
    pub rev: &'a Option<String>,
}

/// Why [`PackageManager::locate_git`] found no package.
pub enum LocateError {
    /// Installing the project's dependencies failed.
    InstallFailed(String),
    /// The package is not installed: what is missing.
    NotInstalled(String),
}

/// What the module loader asks of a package manager.
pub trait PackageManager: Send + Sync {
    /// Where the git dependency `dep` — as the manifest of the package rooted
    /// at `owner_root` writes it — is installed for the project at
    /// `project_root`. With `install`, a project whose lockfile is behind its
    /// manifests is installed first (fetching); without it, nothing is
    /// fetched and a missing package is [`LocateError::NotInstalled`].
    fn locate_git(
        &self,
        project_root: &Path,
        owner_root: &Path,
        dep: &GitDependency,
        install: bool,
    ) -> Result<PathBuf, LocateError>;

    /// The root of the fetched repository that `root` lies in (a cached tree
    /// or a `vendor/<name>` of the project), or `None` for any other
    /// directory. A path dependency of a fetched package may not leave it.
    fn fetched_tree_of(&self, project_root: &Path, root: &Path) -> Option<PathBuf>;

    /// The project's dependency graph as installed (git packages at their
    /// locked commits), for `petal packages`. Fetches nothing.
    fn installed_graph(&self, project_root: &Path) -> Result<InstalledGraph, String>;
}

/// A project's resolved dependency graph, as [`PackageManager::installed_graph`]
/// reports it.
pub struct InstalledGraph {
    /// The project first, then its dependencies.
    pub nodes: Vec<InstalledNode>,
}

impl InstalledGraph {
    pub fn project(&self) -> &InstalledNode {
        &self.nodes[0]
    }

    /// The node of the package called `name`.
    pub fn package(&self, name: &str) -> Option<&InstalledNode> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

/// One package of an [`InstalledGraph`].
pub struct InstalledNode {
    pub name: String,
    pub version: Option<String>,
    /// The directory holding its `petal.toml`.
    pub root: PathBuf,
    pub requires: Vec<Requirement>,
    /// How the package was reached, for display.
    pub source: String,
    /// Reached by a path dependency (so its manifest's spelling reads best).
    pub is_path: bool,
    /// For a git package: `(commit, tree hash, vendored)`.
    pub locked: Option<(String, String, bool)>,
    /// `(import name, package name)` for each of its dependencies.
    pub dependencies: Vec<(String, String)>,
}

static REGISTERED: OnceLock<Box<dyn PackageManager>> = OnceLock::new();

/// Make `manager` answer for git dependencies in this process. Only the first
/// registration takes effect; a later one returns it back.
pub fn register(manager: Box<dyn PackageManager>) -> Result<(), Box<dyn PackageManager>> {
    REGISTERED.set(manager)
}

/// The registered package manager, if this process has one.
pub fn registered() -> Option<&'static dyn PackageManager> {
    REGISTERED.get().map(|b| b.as_ref())
}
