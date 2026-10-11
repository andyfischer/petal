//! The package manager: git dependencies, `petal.lock`, `vendor/`, bundles.
//!
//! A project is a directory with a `petal.toml` ([`crate::package`]). Its
//! `[dependencies]` — and those of every package they reach — form a graph
//! this module resolves into exactly **one version of each package**:
//!
//! - A `path` dependency is the directory it names, as it is on disk.
//! - A `git` dependency is fetched (by shelling out to `git`) into a cache
//!   under `~/.petal/cache` (`PETAL_HOME` overrides `~/.petal`). The checked-
//!   out tree is stored by a hash of its contents: `cache/trees/<sha256>`.
//! - When two packages ask for the same git repository at different tags,
//!   the higher tag wins — *minimal version selection*: the version used is
//!   the highest minimum anyone asked for, and no solver runs. Requests that
//!   cannot be ordered (two different `rev`s) are an error.
//!
//! The result is written to `petal.lock` beside the project's `petal.toml`:
//! each package's source, the commit a tag or rev resolved to, and the tree
//! hash. Later installs reuse the locked commit and verify what they fetch
//! against the locked hash. A `vendor/<name>` directory at the project root,
//! when present, is used in preference to the cache.
//!
//! The loader ([`crate::module`]) never fetches: it reads `petal.lock` and
//! opens the vendored or cached tree ([`locate_git`]). The CLI turns on
//! install-on-demand, which runs [`install`] the first time a program imports
//! a git dependency that is not there.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::package::{DependencySource, MANIFEST_FILE, Manifest, Requirement, Version};

/// The lockfile's name, beside the project's `petal.toml`.
pub const LOCK_FILE: &str = "petal.lock";

/// The directory at the project root whose `<name>/` subdirectories stand in
/// for cached packages.
pub const VENDOR_DIR: &str = "vendor";

// ── where things live ────────────────────────────────────────────

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

/// `<petal home>/cache`: git databases under `git/`, checked-out trees under
/// `trees/<sha256>`, and the commit-to-tree index under `commits/`.
pub fn cache_dir() -> PathBuf {
    petal_home().join("cache")
}

fn tree_dir(tree: &str) -> PathBuf {
    cache_dir()
        .join("trees")
        .join(tree.strip_prefix("sha256:").unwrap_or(tree))
}

fn canonical(path: &Path) -> PathBuf {
    crate::module::canonical_path(path)
}

/// A git url as the cache and the lockfile key it: a relative path that
/// exists beside the manifest naming it becomes absolute, and a trailing `/`
/// is dropped. Anything else is left as written.
pub fn normalize_git_url(owner_root: &Path, url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let looks_remote = trimmed.contains("://") || trimmed.contains('@');
    if !looks_remote && Path::new(trimmed).is_relative() {
        let joined = owner_root.join(trimmed);
        if joined.exists() {
            return canonical(&joined).display().to_string();
        }
    }
    trimmed.to_string()
}

// ── sha-256 ──────────────────────────────────────────────────────

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256, small and dependency-free: the lockfile's tree hashes.
struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Sha256 {
    fn new() -> Sha256 {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (i, chunk) in self.block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(v);
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.block[56..64].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash of a directory's contents, `sha256:<hex>`: every file under
/// `root` (a `.git` entry excepted), in order of its `/`-joined relative
/// path, contributes that path and the SHA-256 of its bytes. Empty
/// directories and file modes do not count.
pub fn hash_tree(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    list_files(root, "", &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut outer = Sha256::new();
    for (rel, path) in files {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        outer.update(rel.as_bytes());
        outer.update(&[0]);
        outer.update(&sha256(&bytes));
    }
    Ok(format!("sha256:{}", hex(&outer.finish())))
}

fn list_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            list_files(&path, &rel, out)?;
        } else {
            out.push((rel, path));
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    list_files(from, "", &mut files)?;
    for (rel, path) in files {
        let dest = to.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::copy(&path, &dest)
            .map_err(|e| format!("cannot copy {} to {}: {e}", path.display(), dest.display()))?;
    }
    Ok(())
}

/// `target` relative to `base` with `/` separators (`../lib`), or `target`
/// itself when the two share no root.
fn relative_path(base: &Path, target: &Path) -> String {
    let base_parts: Vec<_> = base.components().collect();
    let target_parts: Vec<_> = target.components().collect();
    let common = base_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return target.display().to_string();
    }
    let mut parts: Vec<String> = vec!["..".to_string(); base_parts.len() - common];
    parts.extend(
        target_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

// ── the lockfile ─────────────────────────────────────────────────

/// A parsed `petal.lock`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lock {
    /// Every package of the graph except the project itself, by name.
    pub packages: Vec<LockedPackage>,
}

/// One package as the lockfile pins it.
#[derive(Debug, Clone, PartialEq)]
pub struct LockedPackage {
    /// The package's own `[package] name`.
    pub name: String,
    pub version: Option<String>,
    pub source: LockedSource,
    /// `(import name, package name)` for each of its `[dependencies]`.
    pub dependencies: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LockedSource {
    /// A directory on disk. `path` is relative to the project root — or, when
    /// `within` names a git package, to that package's root (a path
    /// dependency inside a fetched repository).
    Path { path: String, within: Option<String> },
    /// A git repository pinned to a commit, and the hash of its tree.
    Git {
        url: String,
        tag: Option<String>,
        rev: Option<String>,
        commit: String,
        tree: String,
    },
}

impl Lock {
    /// Read `<root>/petal.lock`. `Ok(None)` when there is none.
    pub fn load(root: &Path) -> Result<Option<Lock>, String> {
        let path = root.join(LOCK_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Lock::parse(&text)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Lock, String> {
        let mut tables: Vec<BTreeMap<String, String>> = Vec::new();
        let mut in_package = false;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[[package]]" {
                tables.push(BTreeMap::new());
                in_package = true;
                continue;
            }
            let bad = || format!("line {}: expected 'key = \"value\"', found '{line}'", index + 1);
            let (key, value) = line.split_once('=').ok_or_else(bad)?;
            let value = value
                .trim()
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or_else(bad)?;
            if in_package {
                tables
                    .last_mut()
                    .expect("a [[package]] was opened")
                    .insert(key.trim().to_string(), value.to_string());
            } else if key.trim() == "version" && value != "1" {
                return Err(format!(
                    "lockfile version {value} is newer than this petal understands"
                ));
            }
        }
        let mut packages = Vec::new();
        for mut table in tables {
            let name = table
                .remove("name")
                .ok_or("a [[package]] entry has no name")?;
            let dependencies = table
                .remove("dependencies")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| match d.split_once('=') {
                    Some((key, package)) => (key.trim().to_string(), package.trim().to_string()),
                    None => (d.to_string(), d.to_string()),
                })
                .collect();
            let source = match (table.remove("git"), table.remove("path")) {
                (Some(url), None) => LockedSource::Git {
                    url,
                    tag: table.remove("tag"),
                    rev: table.remove("rev"),
                    commit: table
                        .remove("commit")
                        .ok_or_else(|| format!("package '{name}' has no commit"))?,
                    tree: table
                        .remove("tree")
                        .ok_or_else(|| format!("package '{name}' has no tree hash"))?,
                },
                (None, Some(path)) => LockedSource::Path {
                    path,
                    within: table.remove("within"),
                },
                _ => return Err(format!("package '{name}' needs exactly one of 'git' or 'path'")),
            };
            packages.push(LockedPackage {
                name,
                version: table.remove("version"),
                source,
                dependencies,
            });
        }
        Ok(Lock { packages })
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "# Written by `petal install`. Commit this file; do not edit it by hand.\n\
             version = \"1\"\n",
        );
        for p in &self.packages {
            out.push_str("\n[[package]]\n");
            let mut line = |key: &str, value: &str| {
                out.push_str(&format!("{key} = \"{value}\"\n"));
            };
            line("name", &p.name);
            if let Some(v) = &p.version {
                line("version", v);
            }
            match &p.source {
                LockedSource::Path { path, within } => {
                    line("path", path);
                    if let Some(w) = within {
                        line("within", w);
                    }
                }
                LockedSource::Git {
                    url,
                    tag,
                    rev,
                    commit,
                    tree,
                } => {
                    line("git", url);
                    if let Some(t) = tag {
                        line("tag", t);
                    }
                    if let Some(r) = rev {
                        line("rev", r);
                    }
                    line("commit", commit);
                    line("tree", tree);
                }
            }
            if !p.dependencies.is_empty() {
                let deps: Vec<String> = p
                    .dependencies
                    .iter()
                    .map(|(key, name)| {
                        if key == name {
                            key.clone()
                        } else {
                            format!("{key}={name}")
                        }
                    })
                    .collect();
                line("dependencies", &deps.join(", "));
            }
        }
        out
    }

    /// The entry pinning the git repository at `url` (normalized).
    pub fn git(&self, url: &str) -> Option<&LockedPackage> {
        self.packages
            .iter()
            .find(|p| matches!(&p.source, LockedSource::Git { url: u, .. } if u == url))
    }

    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }
}

/// Does the locked selection `(locked_tag, locked_rev, commit)` answer a
/// request for `(tag, rev)`? The same pin does; so does a locked tag at
/// least as high as the tag asked for (minimal version selection chose it),
/// and a locked commit the requested rev abbreviates.
fn lock_answers(
    locked_tag: &Option<String>,
    locked_rev: &Option<String>,
    commit: &str,
    tag: &Option<String>,
    rev: &Option<String>,
) -> bool {
    if locked_tag == tag && locked_rev == rev {
        return true;
    }
    if let (Some(a), Some(b)) = (locked_tag, tag)
        && let (Some(a), Some(b)) = (Version::parse(a), Version::parse(b))
    {
        return a >= b;
    }
    matches!(rev, Some(r) if !r.is_empty() && commit.starts_with(r.as_str()))
}

/// Where the loader finds the git dependency `url` (as the manifest of the
/// package rooted at `owner_root` writes it) for the project at
/// `project_root`: `vendor/<name>` when that is there, else the cached tree
/// `petal.lock` names. Never fetches. `Err` says what is missing; `trust`
/// accepts whatever the lockfile pins for the url without comparing the
/// request (used right after an install).
pub fn locate_git(
    project_root: &Path,
    owner_root: &Path,
    url: &str,
    tag: &Option<String>,
    rev: &Option<String>,
    trust: bool,
) -> Result<PathBuf, String> {
    let url = normalize_git_url(owner_root, url);
    let lock = Lock::load(project_root)?
        .ok_or_else(|| format!("there is no {LOCK_FILE} in {}", project_root.display()))?;
    let entry = lock
        .git(&url)
        .ok_or_else(|| format!("{LOCK_FILE} does not pin {url}"))?;
    let LockedSource::Git {
        tag: locked_tag,
        rev: locked_rev,
        commit,
        tree,
        ..
    } = &entry.source
    else {
        unreachable!("Lock::git returns git entries")
    };
    if !trust && !lock_answers(locked_tag, locked_rev, commit, tag, rev) {
        return Err(format!(
            "{LOCK_FILE} pins {url} at {}, which does not answer {}",
            describe_pin(locked_tag, locked_rev),
            describe_pin(tag, rev)
        ));
    }
    let vendored = project_root.join(VENDOR_DIR).join(&entry.name);
    if vendored.join(MANIFEST_FILE).is_file() {
        return Ok(vendored);
    }
    let cached = tree_dir(tree);
    if cached.join(MANIFEST_FILE).is_file() {
        return Ok(cached);
    }
    Err(format!(
        "package '{}' ({url}) is in neither {VENDOR_DIR}/ nor the cache ({})",
        entry.name,
        cache_dir().display()
    ))
}

/// The root of the fetched repository `root` lies in — a cached tree or a
/// `vendor/<name>` of the project — or `None` for any other directory. A path
/// dependency of a fetched package may not leave it.
pub fn fetched_tree_of(project_root: &Path, root: &Path) -> Option<PathBuf> {
    for base in [
        canonical(&cache_dir().join("trees")),
        canonical(&project_root.join(VENDOR_DIR)),
    ] {
        if let Ok(inside) = root.strip_prefix(&base)
            && let Some(first) = inside.components().next()
        {
            return Some(base.join(first));
        }
    }
    None
}

fn describe_pin(tag: &Option<String>, rev: &Option<String>) -> String {
    match (tag, rev) {
        (Some(t), _) => format!("tag '{t}'"),
        (None, Some(r)) => format!("rev '{r}'"),
        (None, None) => "no pin".to_string(),
    }
}

// ── git ──────────────────────────────────────────────────────────

fn git(args: &[&str], index_file: Option<&Path>) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::null());
    if let Some(index) = index_file {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run git ({e}) — git dependencies need git on the PATH"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn git_db(url: &str) -> PathBuf {
    cache_dir()
        .join("git")
        .join(&hex(&sha256(url.as_bytes()))[..24])
}

/// Bring the cache's copy of the repository at `url` up to date.
fn git_fetch(url: &str) -> Result<PathBuf, String> {
    let db = git_db(url);
    let db_str = db.display().to_string();
    if !db.join("HEAD").is_file() {
        std::fs::create_dir_all(&db).map_err(|e| format!("cannot create {db_str}: {e}"))?;
        git(&["init", "--bare", "--quiet", &db_str], None)?;
    }
    git(
        &[
            "--git-dir",
            &db_str,
            "fetch",
            "--quiet",
            "--force",
            "--prune",
            url,
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
        ],
        None,
    )
    .map_err(|e| format!("cannot fetch {url}: {e}"))?;
    Ok(db)
}

fn git_has_commit(db: &Path, commit: &str) -> bool {
    db.join("HEAD").is_file()
        && git(
            &[
                "--git-dir",
                &db.display().to_string(),
                "cat-file",
                "-e",
                &format!("{commit}^{{commit}}"),
            ],
            None,
        )
        .is_ok()
}

fn git_resolve(
    db: &Path,
    url: &str,
    tag: &Option<String>,
    rev: &Option<String>,
) -> Result<String, String> {
    let db_str = db.display().to_string();
    let parse = |spec: &str| {
        git(
            &["--git-dir", &db_str, "rev-parse", "--verify", "--quiet", spec],
            None,
        )
    };
    match (tag, rev) {
        (Some(tag), _) => parse(&format!("refs/tags/{tag}^{{commit}}"))
            .map_err(|_| format!("{url} has no tag '{tag}'")),
        (None, Some(rev)) => parse(&format!("{rev}^{{commit}}")).or_else(|_| {
            // A commit no branch or tag reaches: ask for it by name.
            let _ = git(
                &["--git-dir", &db_str, "fetch", "--quiet", url, rev],
                None,
            );
            parse(&format!("{rev}^{{commit}}")).map_err(|_| format!("{url} has no commit '{rev}'"))
        }),
        (None, None) => Err(format!("the dependency on {url} has neither a tag nor a rev")),
    }
}

/// Check `commit` out of the cache's database into `cache/trees/<hash>`.
/// Returns the tree hash.
fn git_extract(db: &Path, commit: &str) -> Result<String, String> {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let trees = cache_dir().join("trees");
    std::fs::create_dir_all(&trees).map_err(|e| format!("cannot create {}: {e}", trees.display()))?;
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = trees.join(format!(".tmp-{}-{n}", std::process::id()));
    let index = trees.join(format!(".tmp-{}-{n}.index", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
    let checked_out = git(
        &[
            "--git-dir",
            &db.display().to_string(),
            "--work-tree",
            &tmp.display().to_string(),
            "-c",
            "core.autocrlf=false",
            "checkout",
            "--quiet",
            "--force",
            commit,
            "--",
            ".",
        ],
        Some(&index),
    );
    let _ = std::fs::remove_file(&index);
    let result = checked_out
        .map_err(|e| format!("cannot check out {commit}: {e}"))
        .and_then(|_| hash_tree(&tmp))
        .and_then(|tree| {
            let dest = tree_dir(&tree);
            if !dest.is_dir() {
                std::fs::rename(&tmp, &dest)
                    .map_err(|e| format!("cannot move the checkout into the cache: {e}"))?;
            }
            Ok(tree)
        });
    let _ = std::fs::remove_dir_all(&tmp);
    let tree = result?;
    let commits = cache_dir().join("commits");
    let _ = std::fs::create_dir_all(&commits);
    let _ = std::fs::write(commits.join(commit), &tree);
    Ok(tree)
}

// ── resolving the graph ──────────────────────────────────────────

/// How a package of the graph is obtained.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeSource {
    /// The project itself.
    Project,
    Path { path: String, within: Option<String> },
    Git {
        url: String,
        tag: Option<String>,
        rev: Option<String>,
        commit: String,
        tree: String,
        /// True when `root` is the project's `vendor/<name>`.
        vendored: bool,
    },
}

impl std::fmt::Display for NodeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeSource::Project => write!(f, "project"),
            NodeSource::Path { path, within: None } => write!(f, "path {path}"),
            NodeSource::Path {
                path,
                within: Some(w),
            } => write!(f, "path {path} in {w}"),
            NodeSource::Git {
                url,
                tag,
                rev,
                commit,
                vendored,
                ..
            } => {
                write!(f, "git {url}")?;
                if let Some(t) = tag {
                    write!(f, " tag {t}")?;
                } else if let Some(r) = rev {
                    write!(f, " rev {r}")?;
                }
                write!(f, " @ {}", &commit[..commit.len().min(10)])?;
                if *vendored {
                    write!(f, " (vendored)")?;
                }
                Ok(())
            }
        }
    }
}

/// One package of a resolved graph.
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub version: Option<String>,
    /// The directory holding its `petal.toml`.
    pub root: PathBuf,
    pub source: NodeSource,
    /// `(import name, package name)` per `[dependencies]` entry.
    pub dependencies: Vec<(String, String)>,
    pub requires: Vec<Requirement>,
}

/// A project's dependency graph, one version of each package. The project is
/// the first node; the rest are sorted by name.
#[derive(Debug, Clone)]
pub struct Graph {
    pub root: PathBuf,
    pub packages: Vec<Node>,
}

impl Graph {
    pub fn project(&self) -> &Node {
        &self.packages[0]
    }

    pub fn dependencies(&self) -> &[Node] {
        &self.packages[1..]
    }

    pub fn package(&self, name: &str) -> Option<&Node> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// The lockfile that pins this graph.
    pub fn lock(&self) -> Lock {
        Lock {
            packages: self
                .dependencies()
                .iter()
                .map(|n| LockedPackage {
                    name: n.name.clone(),
                    version: n.version.clone(),
                    source: match &n.source {
                        NodeSource::Project => unreachable!("the project is the first node"),
                        NodeSource::Path { path, within } => LockedSource::Path {
                            path: path.clone(),
                            within: within.clone(),
                        },
                        NodeSource::Git {
                            url,
                            tag,
                            rev,
                            commit,
                            tree,
                            ..
                        } => LockedSource::Git {
                            url: url.clone(),
                            tag: tag.clone(),
                            rev: rev.clone(),
                            commit: commit.clone(),
                            tree: tree.clone(),
                        },
                    },
                    dependencies: n.dependencies.clone(),
                })
                .collect(),
        }
    }

    /// Every `[requires]` in the graph, by interface then package.
    pub fn requires(&self) -> Vec<crate::package::RequiredInterface> {
        let mut out: Vec<crate::package::RequiredInterface> = self
            .packages
            .iter()
            .flat_map(|n| {
                n.requires.iter().map(|r| crate::package::RequiredInterface {
                    interface: r.interface.clone(),
                    version: r.version.clone(),
                    package: n.name.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| (&a.interface, &a.package).cmp(&(&b.interface, &b.package)));
        out
    }
}

/// Which locked packages a resolution re-resolves from their source.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Update {
    #[default]
    None,
    All,
    Names(Vec<String>),
}

/// How [`resolve`] may go about it.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Fetch what is missing. Off, a package that is in neither `vendor/`
    /// nor the cache is [`ResolveError::NotInstalled`].
    pub online: bool,
    pub update: Update,
    /// Re-hash vendored and cached trees and compare them to the lockfile.
    pub verify: bool,
    /// Say on stderr what is being fetched.
    pub announce: bool,
    /// Resolve as though there were no `vendor/` (what `petal vendor` does
    /// before it rewrites the directory).
    pub ignore_vendor: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolveError {
    /// Something has to be fetched (or the lockfile is out of date) and the
    /// resolution was offline. `petal install` is the cure.
    NotInstalled(String),
    Failed(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotInstalled(m) => write!(f, "{m} — run `petal install`"),
            ResolveError::Failed(m) => f.write_str(m),
        }
    }
}

fn failed(message: impl Into<String>) -> ResolveError {
    ResolveError::Failed(message.into())
}

/// The version of a git repository chosen for the program.
#[derive(Debug, Clone)]
struct Selection {
    tag: Option<String>,
    rev: Option<String>,
    /// Who asked for it.
    by: String,
    commit: String,
    tree: String,
    root: PathBuf,
    vendored: bool,
    /// True while this stands in for a lower tag than the tree it points at
    /// (see `Resolver::select`).
    provisional: bool,
}

struct Resolver<'a> {
    project_root: PathBuf,
    lock: Option<Lock>,
    options: &'a Options,
    selected: BTreeMap<String, Selection>,
    /// Every pin already materialized this resolution, so a restart of the
    /// walk does not fetch again.
    materialized: HashMap<(String, Option<String>, Option<String>), Selection>,
    fetched: std::collections::HashSet<String>,
    /// Repositories whose requests are resolved exactly as asked, the
    /// lockfile's different pin notwithstanding.
    settled: std::collections::HashSet<String>,
    changed: bool,
}

/// Resolve the dependency graph of the project rooted at `project_root`.
pub fn resolve(project_root: &Path, options: &Options) -> Result<Graph, ResolveError> {
    let project_root = canonical(project_root);
    let lock = Lock::load(&project_root).map_err(failed)?;
    let mut resolver = Resolver {
        project_root: project_root.clone(),
        lock,
        options,
        selected: BTreeMap::new(),
        materialized: HashMap::new(),
        fetched: Default::default(),
        settled: Default::default(),
        changed: false,
    };
    let mut nodes = loop {
        // Selecting a higher version of a package changes what *it* depends
        // on, so walk until a walk selects nothing new.
        let nodes = loop {
            resolver.changed = false;
            let nodes = resolver.walk()?;
            if !resolver.changed {
                break nodes;
            }
        };
        // A locked higher tag that nobody asks for any more does not stand:
        // resolve those repositories as asked, from the top.
        let unasked: Vec<String> = resolver
            .selected
            .iter()
            .filter(|(_, sel)| sel.provisional)
            .map(|(url, _)| url.clone())
            .collect();
        if unasked.is_empty() {
            break nodes;
        }
        resolver.settled.extend(unasked);
        resolver.selected.clear();
    };
    // Exactly one package per name.
    for (i, a) in nodes.iter().enumerate() {
        if let Some(b) = nodes[i + 1..].iter().find(|b| b.name == a.name) {
            return Err(failed(format!(
                "two different packages are both named '{}': {} ({}) and {} ({}) — a program \
                 holds exactly one version of each package; point every dependency on '{}' at \
                 the same source",
                a.name,
                a.source,
                a.root.display(),
                b.source,
                b.root.display(),
                a.name
            )));
        }
    }
    let project = nodes.remove(0);
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    nodes.insert(0, project);
    Ok(Graph {
        root: project_root,
        packages: nodes,
    })
}

impl Resolver<'_> {
    fn updating(&self, name: &str) -> bool {
        match &self.options.update {
            Update::None => false,
            Update::All => true,
            Update::Names(names) => names.iter().any(|n| n == name),
        }
    }

    fn walk(&mut self) -> Result<Vec<Node>, ResolveError> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut seen: HashMap<PathBuf, String> = HashMap::new();
        // (root, source, the fetched tree it lies in and that tree's package)
        let mut queue: Vec<(PathBuf, NodeSource, Option<(PathBuf, String)>)> =
            vec![(self.project_root.clone(), NodeSource::Project, None)];
        let mut next = 0;
        while next < queue.len() {
            let (root, source, tree) = queue[next].clone();
            next += 1;
            if seen.contains_key(&root) {
                continue;
            }
            let manifest = Manifest::load(&root).map_err(|e| failed(e.to_string()))?;
            seen.insert(root.clone(), manifest.name.clone());
            // The tree a git package's own path dependencies must stay in.
            let tree = match (&source, tree) {
                (NodeSource::Git { .. }, _) => Some((root.clone(), manifest.name.clone())),
                (_, tree) => tree,
            };
            let mut dependencies = Vec::new();
            for dep in &manifest.dependencies {
                let (dep_root, dep_source) = match &dep.source {
                    DependencySource::Path(rel) => {
                        let joined = root.join(rel);
                        let dep_root = canonical(
                            &crate::module::normalize_lexical(&joined).unwrap_or(joined),
                        );
                        let source = match &tree {
                            Some((tree_root, tree_name)) => {
                                if !dep_root.starts_with(tree_root) {
                                    return Err(failed(format!(
                                        "dependency '{}' of package '{}' (path = \"{rel}\") \
                                         leaves the repository the package was fetched from — a \
                                         path dependency of a git package must stay inside it",
                                        dep.name, manifest.name
                                    )));
                                }
                                NodeSource::Path {
                                    path: relative_path(tree_root, &dep_root),
                                    within: Some(tree_name.clone()),
                                }
                            }
                            None => NodeSource::Path {
                                path: relative_path(&self.project_root, &dep_root),
                                within: None,
                            },
                        };
                        if !dep_root.join(MANIFEST_FILE).is_file() {
                            return Err(failed(format!(
                                "dependency '{}' of package '{}' (path = \"{rel}\"): there is no \
                                 {MANIFEST_FILE} in {}",
                                dep.name,
                                manifest.name,
                                dep_root.display()
                            )));
                        }
                        (dep_root, source)
                    }
                    DependencySource::Git { url, tag, rev } => {
                        let url = normalize_git_url(&root, url);
                        let sel = self.select(&url, tag, rev, &manifest.name)?;
                        (
                            sel.root.clone(),
                            NodeSource::Git {
                                url,
                                tag: sel.tag,
                                rev: sel.rev,
                                commit: sel.commit,
                                tree: sel.tree,
                                vendored: sel.vendored,
                            },
                        )
                    }
                    DependencySource::Registry(version) => {
                        return Err(failed(format!(
                            "dependency '{} = \"{version}\"' of package '{}' names a registry \
                             version, and registries are not supported yet — use {{ path = \"…\" \
                             }} or {{ git = \"…\", tag = \"…\" }}",
                            dep.name, manifest.name
                        )));
                    }
                };
                let dep_name = match seen.get(&dep_root) {
                    Some(name) => name.clone(),
                    None => Manifest::load(&dep_root)
                        .map_err(|e| failed(e.to_string()))?
                        .name,
                };
                dependencies.push((dep.name.clone(), dep_name));
                queue.push((dep_root, dep_source, tree.clone()));
            }
            nodes.push(Node {
                name: manifest.name,
                version: manifest.version,
                root,
                source,
                dependencies,
                requires: manifest.requires,
            });
        }
        Ok(nodes)
    }

    /// The one version of the repository at `url` this program uses, given
    /// that `by` asks for `(tag, rev)`.
    fn select(
        &mut self,
        url: &str,
        tag: &Option<String>,
        rev: &Option<String>,
        by: &str,
    ) -> Result<Selection, ResolveError> {
        let Some(current) = self.selected.get(url).cloned() else {
            // The lockfile pins this repository at something other than what
            // is asked for here, but which answers it — typically a higher
            // tag another package asks for. Take the locked tree for now
            // without fetching the one asked for; whether that stands is
            // settled once the whole graph has been walked (`resolve`).
            let locked = self.lock.as_ref().and_then(|l| l.git(url)).and_then(|p| {
                let LockedSource::Git {
                    tag: t,
                    rev: r,
                    commit,
                    ..
                } = &p.source
                else {
                    return None;
                };
                (!self.settled.contains(url)
                    && !self.updating(&p.name)
                    && (t, r) != (tag, rev)
                    && lock_answers(t, r, commit, tag, rev))
                .then(|| (t.clone(), r.clone()))
            });
            if let Some((locked_tag, locked_rev)) = locked {
                let mut sel = self.materialize(url, &locked_tag, &locked_rev, by)?;
                // A rev that abbreviates the locked commit is that commit;
                // a lower tag is a guess until someone asks for the higher.
                if tag.is_some() {
                    sel.tag = tag.clone();
                    sel.rev = rev.clone();
                    sel.provisional = true;
                }
                self.selected.insert(url.to_string(), sel.clone());
                return Ok(sel);
            }
            let sel = self.materialize(url, tag, rev, by)?;
            self.selected.insert(url.to_string(), sel.clone());
            return Ok(sel);
        };
        if &current.tag == tag && &current.rev == rev {
            return Ok(current);
        }
        // Two tags that read as versions: the higher one is the version.
        if let (Some(a), Some(b)) = (&current.tag, tag)
            && let (Some(va), Some(vb)) = (Version::parse(a), Version::parse(b))
        {
            if vb <= va {
                return Ok(current);
            }
            let sel = self.materialize(url, tag, rev, by)?;
            self.selected.insert(url.to_string(), sel.clone());
            self.changed = true;
            return Ok(sel);
        }
        // Otherwise they must simply be the same commit.
        let other = self.materialize(url, tag, rev, by)?;
        if other.commit == current.commit {
            return Ok(current);
        }
        Err(failed(format!(
            "{url} is wanted at {} (by '{}') and at {} (by '{by}'), and those are different \
             commits that cannot be ordered — a program holds exactly one version of each \
             package; pin both to the same tag or rev",
            describe_pin(&current.tag, &current.rev),
            current.by,
            describe_pin(tag, rev),
        )))
    }

    /// Make the tree for one pin available and say where it is.
    fn materialize(
        &mut self,
        url: &str,
        tag: &Option<String>,
        rev: &Option<String>,
        by: &str,
    ) -> Result<Selection, ResolveError> {
        let key = (url.to_string(), tag.clone(), rev.clone());
        if let Some(done) = self.materialized.get(&key) {
            return Ok(done.clone());
        }
        let options = self.options;
        let not_installed = |what: String| ResolveError::NotInstalled(what);
        // What the lockfile pins for this exact request.
        let locked = self.lock.as_ref().and_then(|l| l.git(url)).and_then(|p| {
            let LockedSource::Git {
                tag: t,
                rev: r,
                commit,
                tree,
                ..
            } = &p.source
            else {
                return None;
            };
            (t == tag && r == rev && !self.updating(&p.name))
                .then(|| (p.name.clone(), commit.clone(), tree.clone()))
        });

        let finish = |this: &mut Self, sel: Selection| {
            this.materialized.insert(key.clone(), sel.clone());
            Ok(sel)
        };
        let selection = |commit: String, tree: String, root: PathBuf, vendored: bool| Selection {
            tag: tag.clone(),
            rev: rev.clone(),
            by: by.to_string(),
            commit,
            tree,
            root,
            vendored,
            provisional: false,
        };

        if let Some((name, commit, tree)) = &locked {
            // vendor/ first.
            let vendored = self.project_root.join(VENDOR_DIR).join(name);
            if !options.ignore_vendor && vendored.join(MANIFEST_FILE).is_file() {
                if options.verify {
                    let found = hash_tree(&vendored).map_err(failed)?;
                    if &found != tree {
                        return Err(failed(format!(
                            "{VENDOR_DIR}/{name} does not match {LOCK_FILE}: the lockfile says \
                             {tree}, the directory hashes to {found} — `petal vendor` rewrites it \
                             from the locked source"
                        )));
                    }
                }
                let sel = selection(commit.clone(), tree.clone(), canonical(&vendored), true);
                return finish(self, sel);
            }
            let cached = tree_dir(tree);
            if cached.join(MANIFEST_FILE).is_file() {
                let intact = !options.verify || hash_tree(&cached).map_err(failed)? == *tree;
                if intact {
                    let sel = selection(commit.clone(), tree.clone(), canonical(&cached), false);
                    return finish(self, sel);
                }
                // A damaged cache entry is thrown away and fetched again.
                let _ = std::fs::remove_dir_all(&cached);
            }
            if !options.online {
                return Err(not_installed(format!(
                    "package '{name}' ({url}) is locked but not in the cache"
                )));
            }
        } else if !options.online {
            return Err(not_installed(format!(
                "the dependency of '{by}' on {url} at {} is not in {LOCK_FILE}",
                describe_pin(tag, rev)
            )));
        }

        // Fetch.
        let db = git_db(url);
        let commit = match &locked {
            Some((_, commit, _)) => {
                if !git_has_commit(&db, commit) {
                    self.fetch(url)?;
                }
                commit.clone()
            }
            None => {
                self.fetch(url)?;
                git_resolve(&db, url, tag, rev).map_err(failed)?
            }
        };
        // A commit this cache has already checked out.
        let known = std::fs::read_to_string(cache_dir().join("commits").join(&commit))
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| {
                tree_dir(t).join(MANIFEST_FILE).is_file()
                    && (!options.verify || hash_tree(&tree_dir(t)).is_ok_and(|h| h == *t))
            });
        let tree = match known {
            Some(tree) => tree,
            None => git_extract(&db, &commit).map_err(failed)?,
        };
        if let Some((name, _, locked_tree)) = &locked
            && *locked_tree != tree
        {
            return Err(failed(format!(
                "package '{name}' ({url}) does not match {LOCK_FILE}: commit {commit} is locked \
                 with tree {locked_tree}, and what was fetched hashes to {tree}. The source may \
                 have been tampered with; `petal update {name}` re-pins it if the change is \
                 expected"
            )));
        }
        let root = tree_dir(&tree);
        if !root.join(MANIFEST_FILE).is_file() {
            return Err(failed(format!(
                "{url} at {} is not a Petal package: there is no {MANIFEST_FILE} at its root",
                describe_pin(tag, rev)
            )));
        }
        // A vendored copy that already holds exactly this tree stays in use.
        if !options.ignore_vendor
            && let Ok(manifest) = Manifest::load(&root)
        {
            let vendored = self.project_root.join(VENDOR_DIR).join(&manifest.name);
            if vendored.join(MANIFEST_FILE).is_file()
                && hash_tree(&vendored).is_ok_and(|h| h == tree)
            {
                let sel = selection(commit, tree, canonical(&vendored), true);
                return finish(self, sel);
            }
        }
        let sel = selection(commit, tree, canonical(&root), false);
        finish(self, sel)
    }

    fn fetch(&mut self, url: &str) -> Result<(), ResolveError> {
        if !self.fetched.insert(url.to_string()) {
            return Ok(());
        }
        if std::env::var_os("PETAL_OFFLINE").is_some() {
            return Err(failed(format!(
                "{url} has to be fetched, and PETAL_OFFLINE is set"
            )));
        }
        if self.options.announce {
            eprintln!("    Fetching {url}");
        }
        git_fetch(url).map(|_| ()).map_err(failed)
    }
}

// ── the commands ─────────────────────────────────────────────────

/// The directory of the nearest `petal.toml` at or above `start`.
pub fn project_root(start: &Path) -> Result<PathBuf, String> {
    crate::package::find_manifest(start)
        .and_then(|m| m.parent().map(Path::to_path_buf))
        .ok_or_else(|| {
            format!(
                "there is no {MANIFEST_FILE} at or above {} — `petal init` starts a project",
                start.display()
            )
        })
}

/// What an install did.
#[derive(Debug, Clone)]
pub struct Installed {
    pub graph: Graph,
    /// True when `petal.lock` was written (it was missing or out of date).
    pub lock_changed: bool,
    /// The packages copied into `vendor/`.
    pub vendored: Vec<String>,
}

/// How [`install_with`] goes about it.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub update: Update,
    /// Fail rather than change `petal.lock`.
    pub locked: bool,
    /// Copy every git package into `vendor/`, creating it. Without this an
    /// existing `vendor/` is still kept in step with the lockfile.
    pub vendor: bool,
    pub announce: bool,
}

/// `petal install`: resolve the graph, fetch what is missing, verify what is
/// there against `petal.lock`, and write the lockfile.
pub fn install(project_root: &Path) -> Result<Installed, String> {
    install_with(project_root, &InstallOptions::default())
}

pub fn install_with(project_root: &Path, options: &InstallOptions) -> Result<Installed, String> {
    let project_root = canonical(project_root);
    // A fetched tree is content-addressed: nothing is ever written into one.
    if project_root.starts_with(canonical(&cache_dir())) {
        return Err(format!(
            "{} is inside the package cache, which is read-only — install from the project \
             that depends on it",
            project_root.display()
        ));
    }
    let resolve_options = Options {
        online: true,
        update: options.update.clone(),
        verify: true,
        announce: options.announce,
        ignore_vendor: options.vendor,
    };
    let mut graph = resolve(&project_root, &resolve_options).map_err(|e| e.to_string())?;
    let text = graph.lock().render();
    let lock_path = project_root.join(LOCK_FILE);
    let lock_changed = std::fs::read_to_string(&lock_path).ok().as_deref() != Some(text.as_str());
    if lock_changed {
        if options.locked {
            return Err(format!(
                "{LOCK_FILE} is out of date with {MANIFEST_FILE} and --locked forbids changing \
                 it — run `petal install` without --locked"
            ));
        }
        std::fs::write(&lock_path, &text)
            .map_err(|e| format!("cannot write {}: {e}", lock_path.display()))?;
    }

    // vendor/: every git package when asked, and otherwise whatever an
    // existing vendor/ no longer has right.
    let vendor_dir = project_root.join(VENDOR_DIR);
    let mut vendored = Vec::new();
    if options.vendor || vendor_dir.is_dir() {
        for node in graph.dependencies() {
            let NodeSource::Git {
                tree,
                vendored: already,
                ..
            } = &node.source
            else {
                continue;
            };
            if *already {
                continue;
            }
            let dest = vendor_dir.join(&node.name);
            if !options.vendor && !dest.exists() {
                continue;
            }
            let _ = std::fs::remove_dir_all(&dest);
            copy_tree(&tree_dir(tree), &dest)?;
            vendored.push(node.name.clone());
        }
        if !vendored.is_empty() {
            // The copies are now where those packages live.
            graph = resolve(
                &project_root,
                &Options {
                    verify: true,
                    ..Options::default()
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(Installed {
        graph,
        lock_changed,
        vendored,
    })
}

/// The project's graph as it is installed now, fetching nothing.
pub fn installed_graph(project_root: &Path) -> Result<Graph, ResolveError> {
    resolve(project_root, &Options::default())
}

/// The project's graph, installing first when something is missing.
pub fn ensure_installed(project_root: &Path, announce: bool) -> Result<Graph, String> {
    match installed_graph(project_root) {
        Ok(graph) => Ok(graph),
        Err(ResolveError::NotInstalled(_)) => install_with(
            project_root,
            &InstallOptions {
                announce,
                ..InstallOptions::default()
            },
        )
        .map(|i| i.graph),
        Err(e) => Err(e.to_string()),
    }
}

/// `petal init`: write a fresh `petal.toml` into `dir`. Returns its path.
pub fn init(dir: &Path, name: Option<&str>) -> Result<PathBuf, String> {
    let path = dir.join(MANIFEST_FILE);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let name = match name {
        Some(name) => name.to_string(),
        None => package_name_from(
            &canonical(dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
    let text = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n");
    Manifest::parse(&text, &path).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// A directory name bent into a package name (an identifier).
fn package_name_from(dir_name: &str) -> String {
    let mut name: String = dir_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    name
}

/// Where `petal add` gets a dependency from.
#[derive(Debug, Clone, PartialEq)]
pub enum AddSource {
    Path(String),
    Git {
        url: String,
        tag: Option<String>,
        rev: Option<String>,
    },
}

impl AddSource {
    fn render(&self) -> String {
        match self {
            AddSource::Path(path) => format!("{{ path = \"{path}\" }}"),
            AddSource::Git {
                url,
                tag: Some(tag),
                ..
            } => format!("{{ git = \"{url}\", tag = \"{tag}\" }}"),
            AddSource::Git { url, rev, .. } => format!(
                "{{ git = \"{url}\", rev = \"{}\" }}",
                rev.as_deref().unwrap_or_default()
            ),
        }
    }
}

/// `petal add`: write (or replace) the dependency `name` in the project's
/// manifest, then install. A manifest the install rejects is put back.
pub fn add(project_root: &Path, name: &str, source: &AddSource) -> Result<Installed, String> {
    let path = project_root.join(MANIFEST_FILE);
    let before =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let after = manifest_set_dependency(&before, name, &source.render());
    Manifest::parse(&after, &path).map_err(|e| e.to_string())?;
    std::fs::write(&path, &after).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    install_with(
        project_root,
        &InstallOptions {
            update: Update::Names(vec![name.to_string()]),
            announce: true,
            ..InstallOptions::default()
        },
    )
    .inspect_err(|_| {
        let _ = std::fs::write(&path, &before);
    })
}

/// `petal remove`: drop the dependency `name` from the project's manifest,
/// then install (which prunes the lockfile).
pub fn remove(project_root: &Path, name: &str) -> Result<Installed, String> {
    let path = project_root.join(MANIFEST_FILE);
    let before =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let after = manifest_remove_dependency(&before, name)
        .ok_or_else(|| format!("{} has no dependency named '{name}'", path.display()))?;
    std::fs::write(&path, &after).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    install_with(
        project_root,
        &InstallOptions {
            announce: true,
            ..InstallOptions::default()
        },
    )
}

/// Which `[section]` a manifest line opens, if it is a header.
fn section_header(line: &str) -> Option<&str> {
    let line = line.trim();
    let line = line.split('#').next().unwrap_or(line).trim();
    line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

/// `text` without the dependency `name`, or `None` when it has none. Both
/// spellings go: a `name = …` line under `[dependencies]` and a
/// `[dependencies.name]` section.
pub fn manifest_remove_dependency(text: &str, name: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    let mut section = String::new();
    let mut removed = false;
    let table = format!("dependencies.{name}");
    for line in text.lines() {
        if let Some(header) = section_header(line) {
            section = header.to_string();
        }
        if section == table {
            removed = true;
            continue;
        }
        if section == "dependencies"
            && line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == name)
        {
            removed = true;
            continue;
        }
        out.push(line);
    }
    removed.then(|| {
        let mut text = out.join("\n");
        text.push('\n');
        text
    })
}

/// `text` with the dependency `name` set to `spec` (an inline table),
/// replacing an existing entry in place or adding one at the end of
/// `[dependencies]` — which is appended when the manifest has none.
pub fn manifest_set_dependency(text: &str, name: &str, spec: &str) -> String {
    let entry = format!("{name} = {spec}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut section = String::new();
    let mut replaced = false;
    let mut last_in_section: Option<usize> = None;
    let table = format!("dependencies.{name}");
    let mut kept: Vec<String> = Vec::new();
    for line in lines.drain(..) {
        if let Some(header) = section_header(&line) {
            section = header.to_string();
        }
        if section == table {
            continue;
        }
        if section == "dependencies" {
            if line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim() == name)
            {
                kept.push(entry.clone());
                replaced = true;
                continue;
            }
            if !line.trim().is_empty() {
                last_in_section = Some(kept.len());
            }
        }
        kept.push(line);
    }
    if !replaced {
        match last_in_section {
            Some(at) => kept.insert(at + 1, entry),
            None => {
                while kept.last().is_some_and(|l| l.trim().is_empty()) {
                    kept.pop();
                }
                kept.push(String::new());
                kept.push("[dependencies]".to_string());
                kept.push(entry);
            }
        }
    }
    let mut out = kept.join("\n");
    out.push('\n');
    out
}

/// `petal bundle <entry>`: every module of the installed graph in the form
/// `register_package` takes — for a host with no filesystem.
///
/// ```json
/// {
///   "format": 1,
///   "entry": { "file": "main.ptl", "path": "<package:game>/main.ptl", "source": "…" },
///   "packages": [
///     { "name": "tween", "version": "1.2.0",
///       "modules": { "petal.toml": "…", "tween": "…", "ease": "…" } }
///   ],
///   "requires": [ { "interface": "ui", "version": ">=0.1", "package": "tween" } ]
/// }
/// ```
///
/// A host registers each package (`Env::register_package(name, modules)`,
/// `pb_vm_register_package`, the wasm `register_package(name, json)`) and
/// loads `entry.source` at `entry.path`, which places the entry inside the
/// project's own in-memory package so its `./x` imports resolve. `path` is
/// null for an entry file outside the project's module directory. Packages
/// bundled in the petal binary are not repeated here.
pub fn bundle(entry: &Path) -> Result<serde_json::Value, String> {
    let source = std::fs::read_to_string(entry)
        .map_err(|e| format!("cannot read {}: {e}", entry.display()))?;
    let root = project_root(entry)?;
    let graph = ensure_installed(&root, true)?;
    let mut packages = Vec::new();
    let mut entry_path = serde_json::Value::Null;
    let entry_file = canonical(entry);
    for node in &graph.packages {
        if let Some((key, name)) = node.dependencies.iter().find(|(key, name)| key != name) {
            return Err(format!(
                "package '{}' imports the package '{name}' under the name '{key}', and a bundle \
                 registers each package under its own name — name the dependency '{name}' to \
                 bundle this project",
                node.name
            ));
        }
        let package =
            crate::package::Package::from_dir(&node.root).map_err(|e| e.to_string())?;
        let mut modules = serde_json::Map::new();
        let manifest_path = node.root.join(MANIFEST_FILE);
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
        modules.insert(MANIFEST_FILE.to_string(), manifest.into());
        for module in &package.info.modules {
            let loaded = package.load(module).ok_or_else(|| {
                format!("cannot read module '{module}' of package '{}'", node.name)
            })?;
            modules.insert(module.clone(), loaded.source.into());
        }
        if node.source == NodeSource::Project
            && let Ok(inside) = entry_file.strip_prefix(canonical(&package.info.module_dir))
        {
            let inside: Vec<String> = inside
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            entry_path = format!("<package:{}>/{}", node.name, inside.join("/")).into();
        }
        packages.push(serde_json::json!({
            "name": node.name,
            "version": node.version,
            "modules": modules,
        }));
    }
    let requires: Vec<serde_json::Value> = graph
        .requires()
        .iter()
        .map(|r| {
            serde_json::json!({
                "interface": r.interface,
                "version": r.version.to_string(),
                "package": r.package,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "format": 1,
        "entry": {
            "file": relative_path(&graph.root, &entry_file),
            "path": entry_path,
            "source": source,
        },
        "packages": packages,
        "requires": requires,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Longer than one block, fed in pieces.
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(b"a");
        }
        assert_eq!(
            hex(&h.finish()),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn a_lockfile_round_trips() {
        let lock = Lock {
            packages: vec![
                LockedPackage {
                    name: "ease".into(),
                    version: Some("1.0.0".into()),
                    source: LockedSource::Git {
                        url: "file:///tmp/ease".into(),
                        tag: Some("v1.0.0".into()),
                        rev: None,
                        commit: "abc123".into(),
                        tree: "sha256:00ff".into(),
                    },
                    dependencies: vec![("tw".into(), "tween".into()), ("shapes".into(), "shapes".into())],
                },
                LockedPackage {
                    name: "shapes".into(),
                    version: None,
                    source: LockedSource::Path {
                        path: "libs/shapes".into(),
                        within: Some("ease".into()),
                    },
                    dependencies: vec![],
                },
            ],
        };
        let text = lock.render();
        assert!(text.contains("dependencies = \"tw=tween, shapes\""), "{text}");
        assert_eq!(Lock::parse(&text).unwrap(), lock);
        assert_eq!(lock.git("file:///tmp/ease").unwrap().name, "ease");
    }

    #[test]
    fn a_malformed_lockfile_is_named_by_line() {
        let err = Lock::parse("version = \"1\"\n[[package]]\nname = ease\n").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        let err = Lock::parse("[[package]]\nname = \"x\"\n").unwrap_err();
        assert!(err.contains("exactly one of 'git' or 'path'"), "{err}");
        let err = Lock::parse("version = \"9\"\n").unwrap_err();
        assert!(err.contains("newer"), "{err}");
    }

    #[test]
    fn a_higher_locked_tag_answers_a_lower_request() {
        let tag = |t: &str| Some(t.to_string());
        assert!(lock_answers(&tag("v1.2.0"), &None, "abc", &tag("v1.2.0"), &None));
        assert!(lock_answers(&tag("v1.2.0"), &None, "abc", &tag("v1.0"), &None));
        assert!(!lock_answers(&tag("v1.2.0"), &None, "abc", &tag("v1.3"), &None));
        assert!(lock_answers(&tag("v1.2.0"), &None, "abcdef", &None, &tag("abc")));
        assert!(!lock_answers(&tag("v1.2.0"), &None, "abcdef", &None, &tag("fff")));
    }

    const MANIFEST: &str = "[package]\nname = \"game\"\n\n[dependencies]\n# libs\nbloom = { path = \"../bloom\" }\n\n[requires]\nui = \">=0.1\"\n";

    #[test]
    fn setting_a_dependency_adds_replaces_and_starts_the_section() {
        let added = manifest_set_dependency(MANIFEST, "tween", "{ path = \"../tween\" }");
        assert_eq!(
            added,
            "[package]\nname = \"game\"\n\n[dependencies]\n# libs\nbloom = { path = \"../bloom\" }\ntween = { path = \"../tween\" }\n\n[requires]\nui = \">=0.1\"\n"
        );
        let replaced = manifest_set_dependency(&added, "bloom", "{ git = \"u\", tag = \"v1\" }");
        assert!(replaced.contains("bloom = { git = \"u\", tag = \"v1\" }\ntween"), "{replaced}");
        assert!(!replaced.contains("../bloom"), "{replaced}");
        // Idempotent.
        assert_eq!(
            manifest_set_dependency(&added, "tween", "{ path = \"../tween\" }"),
            added
        );
        let fresh = manifest_set_dependency("[package]\nname = \"a\"\n", "x", "{ path = \"x\" }");
        assert_eq!(fresh, "[package]\nname = \"a\"\n\n[dependencies]\nx = { path = \"x\" }\n");
        // A table-section spelling is replaced by the inline one.
        let table = "[package]\nname = \"a\"\n\n[dependencies.x]\npath = \"old\"\n";
        let set = manifest_set_dependency(table, "x", "{ path = \"new\" }");
        assert!(!set.contains("old") && set.contains("x = { path = \"new\" }"), "{set}");
        Manifest::parse(&set, Path::new("petal.toml")).unwrap();
    }

    #[test]
    fn removing_a_dependency_takes_either_spelling() {
        let removed = manifest_remove_dependency(MANIFEST, "bloom").unwrap();
        assert!(!removed.contains("bloom"), "{removed}");
        assert!(removed.contains("[requires]\nui"), "{removed}");
        assert_eq!(manifest_remove_dependency(MANIFEST, "nope"), None);
        // `ui` is a requirement, not a dependency.
        assert_eq!(manifest_remove_dependency(MANIFEST, "ui"), None);
        let table = "[package]\nname = \"a\"\n\n[dependencies.x]\npath = \"old\"\n\n[requires]\nui = \"*\"\n";
        let removed = manifest_remove_dependency(table, "x").unwrap();
        assert_eq!(removed, "[package]\nname = \"a\"\n\n[requires]\nui = \"*\"\n");
    }

    #[test]
    fn a_directory_name_becomes_a_package_name() {
        assert_eq!(package_name_from("my-game"), "my_game");
        assert_eq!(package_name_from("3d"), "_3d");
        assert_eq!(package_name_from("ok_name"), "ok_name");
    }

    #[test]
    fn a_relative_path_climbs_and_descends() {
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/c/d")), "../c/d");
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/b/x")), "x");
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/b")), ".");
    }
}
