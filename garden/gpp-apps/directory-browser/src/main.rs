//! directory-browser — the netrw-style directory listing behind `garden <dir>`
//! and `:E` / `-`.
//!
//! A GPP app (protocol v2, panel-only): it pushes the colocated `browser.ptl`
//! drawer, which the host runs in its in-process panel runtime, and answers the
//! drawer's `query("list", dir)` calls by reading the filesystem. Opening a
//! file is not this app's doing — the drawer calls `mutate("open_path", …)`,
//! which the host answers itself, swapping the pane back to a normal editor
//! (and shutting this process down).
//!
//! The file is split into a pure listing core ([`list_entries`] /
//! [`listing_value`] — sorting, `..`/parent resolution, unit-tested with no
//! stdio) and the tiny [`main`] that hands a [`Provider`] to
//! [`petal_query::gpp::serve`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use petal_query::gpp::{self, PanelUi};
use petal_query::{CachePolicy, Provider, Reply};
use serde_json::json;

const UI_SCRIPT: &str = include_str!("browser.ptl");

/// One entry in a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    is_dir: bool,
}

/// Read `dir` and build its sorted listing.
///
/// Directories sort before files; within each group entries sort
/// case-insensitively. Hidden files (leading `.`) are shown. On a read error
/// the listing is empty (the drawer still shows the `..` row, so the user can
/// navigate back out). The `..` row itself is the *drawer's* to add — this is
/// just the directory's own entries.
fn list_entries(dir: &Path) -> Vec<Entry> {
    let mut dirs: Vec<Entry> = Vec::new();
    let mut files: Vec<Entry> = Vec::new();

    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                dirs.push(Entry { name, is_dir: true });
            } else {
                files.push(Entry {
                    name,
                    is_dir: false,
                });
            }
        }
    }

    let by_name = |a: &Entry, b: &Entry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    dirs.sort_by(by_name);
    files.sort_by(by_name);

    dirs.extend(files);
    dirs
}

/// The `query("list", dir)` answer: the canonical path, its parent (absent at
/// the filesystem root), the user's home (for the `~` key), and the sorted
/// entries — each carrying its own absolute `path` so the drawer never does
/// path surgery.
fn listing_value(dir: &Path) -> serde_json::Value {
    let dir = canonicalize_or_keep(dir);
    let entries: Vec<serde_json::Value> = list_entries(&dir)
        .into_iter()
        .map(|e| {
            json!({
                "name": e.name,
                "is_dir": e.is_dir,
                "path": dir.join(&e.name).display().to_string(),
            })
        })
        .collect();
    json!({
        "path": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "home": std::env::var("HOME").ok(),
        "entries": entries,
    })
}

/// Canonicalize a path for clean display and stable query keys; if that fails
/// (e.g. it doesn't exist), keep the path as given.
fn canonicalize_or_keep(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

fn main() {
    // The launch dir (the first arg, else the pane cwd) is the per-run state:
    // the drawer's `query("list", "")` means "list that".
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg())).query(
        "list",
        |root: &mut PathBuf, ctx| {
            let arg = ctx.arg_str();
            let dir = if arg.is_empty() {
                root.clone()
            } else {
                PathBuf::from(arg)
            };
            // A directory changes under us without notice, so a short max_age
            // with a stale window keeps the listing current without a spinner
            // on revisit.
            Reply::json(listing_value(&dir)).cache(
                CachePolicy::max_age(Duration::from_secs(2))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        },
    );

    // The pane is named by the directory it browses, like a buffer is by its
    // path.
    let ui = PanelUi::new("directory-browser", UI_SCRIPT)
        .title(|root: &PathBuf| canonicalize_or_keep(root).display().to_string());

    if let Err(err) = gpp::serve(provider, ui) {
        eprintln!("directory-browser: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A self-cleaning temporary directory tree for tests.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        /// Create a fresh, uniquely-named directory under the system temp dir.
        fn new() -> TempTree {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = format!(
                "directory-browser-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let root = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&root).unwrap();
            // Canonicalize so paths match what listing_value reports (macOS
            // /tmp is a symlink to /private/tmp).
            let root = std::fs::canonicalize(&root).unwrap();
            TempTree { root }
        }

        fn dir(&self, name: &str) -> &Self {
            std::fs::create_dir_all(self.root.join(name)).unwrap();
            self
        }

        fn file(&self, name: &str) -> &Self {
            std::fs::write(self.root.join(name), b"x").unwrap();
            self
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn listing_sorts_dirs_before_files_then_alphabetical() {
        let tree = TempTree::new();
        tree.file("zebra.txt")
            .file("Apple.txt")
            .dir("src")
            .dir("Assets")
            .file("banana.txt");

        let entries = list_entries(&tree.root);
        assert_eq!(
            names(&entries),
            vec!["Assets", "src", "Apple.txt", "banana.txt", "zebra.txt"]
        );
        assert!(entries[0].is_dir && entries[1].is_dir);
        assert!(!entries[2].is_dir);
    }

    #[test]
    fn hidden_files_are_shown() {
        let tree = TempTree::new();
        tree.file(".hidden").file("visible");
        assert!(names(&list_entries(&tree.root)).contains(&".hidden".to_string()));
    }

    #[test]
    fn unreadable_directory_lists_nothing() {
        let tree = TempTree::new();
        let missing = tree.root.join("does-not-exist");
        assert!(list_entries(&missing).is_empty());
    }

    #[test]
    fn listing_value_carries_paths_parent_and_flags() {
        let tree = TempTree::new();
        tree.dir("sub").file("a.txt");
        let v = listing_value(&tree.root);

        assert_eq!(v["path"], tree.root.display().to_string());
        assert_eq!(
            v["parent"],
            tree.root.parent().unwrap().display().to_string()
        );
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries[0]["name"], "sub");
        assert_eq!(entries[0]["is_dir"], true);
        assert_eq!(
            entries[0]["path"],
            tree.root.join("sub").display().to_string()
        );
        assert_eq!(entries[1]["name"], "a.txt");
        assert_eq!(entries[1]["is_dir"], false);
    }

    #[test]
    fn listing_value_at_the_root_has_no_parent() {
        let v = listing_value(Path::new("/"));
        assert_eq!(v["path"], "/");
        assert!(v["parent"].is_null());
    }

    #[test]
    fn a_missing_directory_still_reports_its_parent() {
        // The drawer can always navigate back out of a bad path.
        let tree = TempTree::new();
        let missing = tree.root.join("gone");
        let v = listing_value(&missing);
        assert_eq!(v["entries"].as_array().unwrap().len(), 0);
        assert_eq!(v["parent"], tree.root.display().to_string());
    }
}
