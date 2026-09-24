//! Source watching while the program is broken.
//!
//! A healthy program is watched through [`SourceWatch`] over its own source
//! files (`Env::watch_program_sources`). A broken one cannot be: after a
//! failed first load there is no program to ask, and after a failed reload
//! the old program's file list is stale — the broken edit may import a
//! module that does not exist yet, or one that exists but fails to compile.
//! The fix can land in any of those, so until a load or reload succeeds the
//! bridge watches, next to the entry file and the last good program's files,
//! every `.ptl` file under the entry file's directory, and counts a `.ptl`
//! file that appears there as a change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use petal::source_watch::SourceWatch;

/// Directory entries a scan looks at, so an entry file sitting in a huge
/// tree stays cheap to poll.
const MAX_SCANNED: usize = 4096;

/// What the bridge watches after a failed load or reload.
pub(crate) struct BrokenWatch {
    /// The entry file that failed.
    pub(crate) entry: PathBuf,
    /// The failure was a first `load_file` (there is no program for it yet):
    /// `reload()` retries the load instead of recompiling a loaded program.
    pub(crate) retry_load: bool,
    /// Directory scanned for `.ptl` files (the entry file's).
    root: PathBuf,
    /// Stamps of the entry, `known` and scanned files at the failed attempt.
    watch: SourceWatch,
    /// Identities of the watched files, to tell a newly appeared one.
    seen: HashSet<PathBuf>,
}

impl BrokenWatch {
    /// Stamp `entry`, `known` (the last good program's files) and every
    /// `.ptl` file under `entry`'s directory as they are now.
    pub(crate) fn new(entry: &Path, known: Vec<PathBuf>, retry_load: bool) -> BrokenWatch {
        let root = match entry.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        let candidates = std::iter::once(entry.to_path_buf())
            .chain(known)
            .chain(scan_ptl(&root));
        for p in candidates {
            if seen.insert(identity(&p)) {
                files.push(p);
            }
        }
        BrokenWatch {
            entry: entry.to_path_buf(),
            retry_load,
            root,
            watch: SourceWatch::new(files),
            seen,
        }
    }

    /// A watched file changed, appeared or vanished, or a new `.ptl` file
    /// appeared under the root.
    pub(crate) fn changed(&self) -> bool {
        self.watch.changed() || self.appeared().next().is_some()
    }

    /// The watched files that changed (in watch order), then the new ones.
    pub(crate) fn changed_paths(&self) -> Vec<PathBuf> {
        let mut out = self.watch.changed_paths();
        out.extend(self.appeared());
        out
    }

    fn appeared(&self) -> impl Iterator<Item = PathBuf> + '_ {
        scan_ptl(&self.root)
            .into_iter()
            .filter(|p| !self.seen.contains(&identity(p)))
    }
}

/// Filesystem identity of a path: its canonical form. A missing file (one
/// deleted since, or not created yet) takes its directory's canonical form,
/// so it matches the same file once it exists again.
fn identity(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => p.to_path_buf(),
    }
}

/// Every `.ptl` file under `root`, skipping hidden directories, in a stable
/// (sorted) order. Bounded by [`MAX_SCANNED`] directory entries.
fn scan_ptl(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut scanned = 0usize;
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            scanned += 1;
            if scanned > MAX_SCANNED {
                out.sort();
                return out;
            }
            let path = e.path();
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                if !e.file_name().to_string_lossy().starts_with('.') {
                    dirs.push(path);
                }
            } else if path.extension().is_some_and(|x| x == "ptl") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("petal-c-bridge-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_broken_watch_sees_siblings_new_files_and_skips_hidden_dirs() {
        let dir = scratch("broken-watch");
        let main = dir.join("main.ptl");
        std::fs::write(&main, "import util\n").unwrap();
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/deep.ptl"), "export let A = 1\n").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("notes.txt"), "not petal\n").unwrap();

        let w = BrokenWatch::new(&main, vec![main.clone()], true);
        assert!(!w.changed());
        // A nested module is watched.
        std::fs::write(dir.join("lib/deep.ptl"), "export let A = 22\n").unwrap();
        assert!(w.changed());
        assert_eq!(w.changed_paths(), vec![dir.join("lib/deep.ptl")]);

        let w = BrokenWatch::new(&main, vec![], true);
        // Non-.ptl files and hidden directories are not.
        std::fs::write(dir.join("notes.txt"), "still not petal\n").unwrap();
        std::fs::write(dir.join(".git/x.ptl"), "hidden\n").unwrap();
        assert!(!w.changed());
        // A module created after the failure (the import that was missing) is.
        std::fs::write(dir.join("util.ptl"), "export let B = 2\n").unwrap();
        assert!(w.changed());
        assert_eq!(w.changed_paths(), vec![dir.join("util.ptl")]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
