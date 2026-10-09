//! Polling change detection for a program's source files.
//!
//! A host that hot-reloads a script has to answer two questions: *which*
//! files does the program depend on, and *has any of them changed* since it
//! was compiled. The first is [`Env::program_source_paths`] — the entry file
//! plus every module in the program's manifest that came from disk, deduped.
//! The second is a [`SourceWatch`]: a stat snapshot of those files that
//! reports when any of them no longer matches.
//!
//! This is polling, deliberately: one `stat` per file per check, no threads,
//! no platform notification API, and it works the same inside any host loop.
//! A host that wants OS notifications (petal-desktop-sdl uses `notify`) can
//! still take the file list from [`Env::program_source_paths`] and watch
//! their directories.
//!
//! [`Env::program_source_paths`]: crate::env::Env::program_source_paths

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file looked like when the snapshot was taken: modification time
/// and length. Length catches a rewrite that lands within the filesystem's
/// mtime granularity. `None` in a [`SourceWatch`] entry means the file could
/// not be stat'ed (missing, unreadable) — and a file that *appears* later
/// counts as a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: SystemTime,
    pub len: u64,
}

impl FileStamp {
    /// Stat `path` now. `None` if it cannot be read.
    pub fn of(path: &Path) -> Option<FileStamp> {
        let meta = std::fs::metadata(path).ok()?;
        Some(FileStamp {
            modified: meta.modified().ok()?,
            len: meta.len(),
        })
    }
}

/// A stat snapshot of a set of files, for "has anything changed since?"
///
/// Build one from [`Env::watch_program_sources`] right after compiling (or
/// from any path list with [`SourceWatch::new`]), then poll
/// [`changed`](Self::changed) each frame. After a reload, replace it with a
/// fresh snapshot of the *new* program — its imports may differ — or call
/// [`refresh`](Self::refresh) to re-stamp the same files.
///
/// [`Env::watch_program_sources`]: crate::env::Env::watch_program_sources
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceWatch {
    files: Vec<(PathBuf, Option<FileStamp>)>,
}

impl SourceWatch {
    /// Snapshot `paths` as they are now.
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> SourceWatch {
        SourceWatch {
            files: paths
                .into_iter()
                .map(|p| {
                    let stamp = FileStamp::of(&p);
                    (p, stamp)
                })
                .collect(),
        }
    }

    /// The watched files, in snapshot order.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|(p, _)| p.as_path())
    }

    /// The watched files with the stamp each had at snapshot time.
    pub fn entries(&self) -> &[(PathBuf, Option<FileStamp>)] {
        &self.files
    }

    /// Whether no files are watched.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Whether any watched file differs from its snapshot: modified, resized,
    /// deleted, or (if it was missing) created. Stops at the first change.
    pub fn changed(&self) -> bool {
        self.files.iter().any(|(p, stamp)| FileStamp::of(p) != *stamp)
    }

    /// Every watched file that differs from its snapshot.
    pub fn changed_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(p, stamp)| FileStamp::of(p) != *stamp)
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Re-stamp every watched file as it is now, so the next
    /// [`changed`](Self::changed) compares against this moment.
    pub fn refresh(&mut self) {
        for (p, stamp) in &mut self.files {
            *stamp = FileStamp::of(p);
        }
    }
}

/// Dedupe `paths` by filesystem identity, keeping the first spelling of
/// each: two spellings of one file (a relative and an absolute path, a
/// symlinked directory) collapse to one entry. Paths that cannot be
/// canonicalized (missing files) are compared as written.
pub(crate) fn dedup_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for p in paths {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        if !seen.contains(&key) {
            seen.push(key);
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "petal-source-watch-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_a_resize_a_delete_and_a_creation() {
        let dir = temp_dir("changes");
        let a = dir.join("a.ptl");
        let b = dir.join("b.ptl");
        std::fs::write(&a, "1").unwrap();
        let mut watch = SourceWatch::new([a.clone(), b.clone()]);
        assert!(!watch.changed());

        std::fs::write(&a, "12").unwrap();
        assert_eq!(watch.changed_paths(), vec![a.clone()]);
        watch.refresh();
        assert!(!watch.changed());

        std::fs::write(&b, "x").unwrap();
        assert_eq!(watch.changed_paths(), vec![b.clone()]);
        watch.refresh();

        std::fs::remove_file(&a).unwrap();
        assert!(watch.changed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dedup_collapses_two_spellings_of_one_file() {
        let dir = temp_dir("dedup");
        let a = dir.join("a.ptl");
        std::fs::write(&a, "1").unwrap();
        let other = dir.join(".").join("a.ptl");
        let missing = dir.join("missing.ptl");
        let out = dedup_paths([a.clone(), other, missing.clone(), missing.clone()]);
        assert_eq!(out, vec![a, missing]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
