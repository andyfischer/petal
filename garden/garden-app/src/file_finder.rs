//! The fuzzy file finder: a pure subsequence matcher, the [`FileFinder`] state
//! machine the overlay drives, and the project-file walk that seeds it.
//!
//! Opened with `Cmd`/`Ctrl`+`P` (see [`crate::app::input`]), the finder overlays
//! a query line and a scored, filtered list of the project's files over the
//! panes; `Enter` opens the selected file in the focused pane. Everything here
//! is window-free and unit-tested: [`fuzzy_match`] and [`FileFinder`] are pure,
//! and [`gather_files`] is exercised against temporary directories.

use std::path::Path;

/// Score awarded for each matched query character (so a longer query that still
/// matches outscores a shorter prefix of it).
const BASE: i32 = 1;
/// Bonus when a matched character sits on a word boundary — the start of the
/// text, just after a separator (`/ \ _ - . space`), or a lower→upper camelCase
/// hump. Boundary hits are what make `mr` favour `main.rs` over `comautomate`.
const BOUNDARY: i32 = 10;
/// Bonus when a matched character immediately follows the previous match, so a
/// contiguous run ("comp" inside "compile") beats the same letters scattered.
const CONSECUTIVE: i32 = 6;
/// Per-character penalty for the gap between two matches, so a contiguous run
/// is preferred over the same letters scattered (even across word boundaries).
/// The gap *distance* is capped at [`GAP_CAP`] first, so one big jump costs the
/// same as a medium one rather than swamping everything.
const GAP_PENALTY: i32 = 3;
const GAP_CAP: usize = 4;
/// Bonus added when the query also matches within the basename alone (the part
/// after the last `/`). Filenames are what people usually type, so a basename
/// hit should outrank an equally-good match that only lands in the directory
/// part of the path.
const BASENAME_BONUS: i32 = 15;

/// Score `text` against the fuzzy `query`, or `None` when `query` is not a
/// subsequence of `text`. A higher score is a better match.
///
/// Matching is **smartcase** (mirroring [`crate::search`]): an all-lowercase
/// query matches case-insensitively; any uppercase character makes the whole
/// query case-sensitive. Folding is ASCII-only, so multi-byte characters keep
/// their own case.
///
/// The score is the better of two greedy passes — one over the full path and
/// one over just the basename (plus [`BASENAME_BONUS`]) — so a filename hit is
/// preferred without losing the ability to match against directory segments.
pub fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let case_sensitive = query.chars().any(|c| c.is_uppercase());
    let q: Vec<char> = query.chars().collect();
    let t: Vec<char> = text.chars().collect();

    // Full-path membership is the necessary condition: if the query isn't a
    // subsequence of the whole text, it can't be one of the basename either.
    let path_score = greedy_score(&q, &t, case_sensitive)?;

    let basename_start = t.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let best = match greedy_score(&q, &t[basename_start..], case_sensitive) {
        Some(base) => path_score.max(base + BASENAME_BONUS),
        None => path_score,
    };

    // Mild length penalty so that, among equally good matches, the shorter path
    // sorts first (a top-level file over a deeply nested namesake).
    Some(best - t.len() as i32 / 16)
}

/// Greedy left-to-right scoring of `q` against `t`. Each query character is
/// matched at its first available position; the running score rewards boundary
/// and consecutive hits and penalises the gaps between them. `None` if `q` is
/// not a subsequence of `t`.
fn greedy_score(q: &[char], t: &[char], case_sensitive: bool) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let eq = |a: char, b: char| {
        if case_sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(&b)
        }
    };

    let mut score = 0;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (ti, &ch) in t.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if !eq(ch, q[qi]) {
            continue;
        }
        score += BASE;
        let boundary = ti == 0 || {
            let p = t[ti - 1];
            matches!(p, '/' | '\\' | '_' | '-' | '.' | ' ')
                || (p.is_ascii_lowercase() && ch.is_ascii_uppercase())
        };
        if boundary {
            score += BOUNDARY;
        }
        match prev {
            Some(pi) if pi + 1 == ti => score += CONSECUTIVE,
            Some(pi) => score -= (ti - pi - 1).min(GAP_CAP) as i32 * GAP_PENALTY,
            None => {}
        }
        prev = Some(ti);
        qi += 1;
    }
    (qi == q.len()).then_some(score)
}

/// The finder's modal state: the immutable candidate list gathered when it
/// opened, the live query, the indices of the current matches (best first), and
/// the selected row. Pure — it owns no filesystem or window state, so the App
/// resolves a selected relative path against the project root it gathered from.
pub struct FileFinder {
    /// All candidate paths, project-relative and `/`-separated.
    files: Vec<String>,
    /// The query typed so far.
    query: String,
    /// Indices into [`files`](Self::files) of the current matches, best first.
    /// With an empty query this is every index, in gathered (sorted) order.
    matches: Vec<usize>,
    /// Selected row within [`matches`](Self::matches).
    selected: usize,
}

/// A windowed view of the matches for rendering: the paths visible in a list of
/// at most `max` rows, scrolled to keep the selection on screen.
pub struct Visible<'a> {
    /// The visible paths, in order.
    pub paths: Vec<&'a str>,
    /// Row of the selected path within [`paths`](Self::paths), if it is visible.
    pub selected_row: Option<usize>,
    /// Total number of matches (may exceed `paths.len()`).
    pub total: usize,
}

impl FileFinder {
    /// Build a finder over `files`. The list is sorted so the empty-query order
    /// is deterministic regardless of how the caller gathered it; the query
    /// starts empty, so every file is a match and the first is selected.
    pub fn new(mut files: Vec<String>) -> FileFinder {
        files.sort();
        let matches = (0..files.len()).collect();
        FileFinder {
            files,
            query: String::new(),
            matches,
            selected: 0,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Append a character to the query and re-filter.
    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.recompute();
    }

    /// Remove the last query character and re-filter. Returns `false` when the
    /// query was already empty (the finder stays open regardless; the caller
    /// closes it on `Escape`).
    pub fn backspace(&mut self) -> bool {
        let popped = self.query.pop().is_some();
        if popped {
            self.recompute();
        }
        popped
    }

    /// Move the selection one row toward the top (clamped).
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection one row toward the bottom (clamped to the last match).
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.matches.len() {
            self.selected += 1;
        }
    }

    /// The selected match's path, or `None` when nothing matches the query.
    pub fn selected_path(&self) -> Option<&str> {
        self.matches
            .get(self.selected)
            .map(|&i| self.files[i].as_str())
    }

    /// The selected row within the current match list.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// How many files currently match the query.
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// The first `n` matching paths, best first — used by the debug `/state`
    /// endpoint so integration tests can assert on the ranking.
    pub fn match_paths(&self, n: usize) -> Vec<&str> {
        self.matches
            .iter()
            .take(n)
            .map(|&i| self.files[i].as_str())
            .collect()
    }

    /// The window of matches to display in a list `max` rows tall, scrolled just
    /// enough to keep the selection visible (it only scrolls once the selection
    /// would fall past the bottom row).
    pub fn visible(&self, max: usize) -> Visible<'_> {
        let total = self.matches.len();
        if max == 0 || total == 0 {
            return Visible {
                paths: Vec::new(),
                selected_row: None,
                total,
            };
        }
        let offset = if self.selected >= max {
            self.selected - max + 1
        } else {
            0
        };
        let end = (offset + max).min(total);
        let paths = self.matches[offset..end]
            .iter()
            .map(|&i| self.files[i].as_str())
            .collect();
        let selected_row =
            (self.selected >= offset && self.selected < end).then(|| self.selected - offset);
        Visible {
            paths,
            selected_row,
            total,
        }
    }

    /// Re-rank the candidates against the current query, keeping a stable order
    /// (best score first, then the gathered order for ties), and reset the
    /// selection to the top. An empty query keeps every file in gathered order.
    fn recompute(&mut self) {
        if self.query.is_empty() {
            self.matches = (0..self.files.len()).collect();
            self.selected = 0;
            return;
        }
        let mut scored: Vec<(i32, usize)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| fuzzy_match(&self.query, f).map(|s| (s, i)))
            .collect();
        // Higher score first; ties break by original index so the order is
        // stable and deterministic (the gathered list is already sorted).
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }
}

/// Directory names never descended into during a project walk: version-control
/// metadata and common build/dependency output. Hidden directories (a leading
/// `.`) are skipped too, which also covers `.git` and friends.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build"];

/// Gather the candidate files for a project rooted at `root`: `git ls-files`
/// when the root is a git work tree — tracked plus untracked-unignored, so
/// `.gitignore` is honored with full fidelity (nested files, negations, the
/// global excludes) — falling back to the plain [`gather_files`] walk outside
/// git or when git itself is unavailable.
pub fn gather_project_files(root: &Path, limit: usize) -> Vec<String> {
    git_ls_files(root, limit).unwrap_or_else(|| gather_files(root, limit))
}

/// The `git ls-files` gatherer behind [`gather_project_files`], or `None` when
/// `root` is not inside a git work tree (or git is missing/failing), so the
/// caller falls back to walking. `-z` framing keeps unusual filenames intact
/// (no quoting), and the same `limit` bounds the result.
fn git_ls_files(root: &Path, limit: usize) -> Option<Vec<String>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut out: Vec<String> = text
        .split('\0')
        .filter(|p| !p.is_empty())
        .take(limit)
        .map(str::to_string)
        .collect();
    out.sort();
    Some(out)
}

/// Walk `root` recursively and collect project-relative file paths
/// (`/`-separated), skipping [`SKIP_DIRS`] and hidden directories and never
/// following directory symlinks (so a self-referential link can't loop). The
/// walk stops once `limit` files have been collected. The result is sorted for a
/// stable empty-query ordering.
pub fn gather_files(root: &Path, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        // Collect this directory's child dirs separately so the traversal order
        // is deterministic regardless of read_dir's order.
        let mut child_dirs = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Never follow symlinks: a link pointing at an ancestor would loop,
            // and links in a source tree are rare enough to simply skip.
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                child_dirs.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    out.push(
                        rel.to_string_lossy()
                            .replace(std::path::MAIN_SEPARATOR, "/"),
                    );
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        // Push in reverse so sibling directories pop in lexical-ish order; the
        // final sort makes the listing fully deterministic anyway.
        child_dirs.sort();
        for child in child_dirs.into_iter().rev() {
            stack.push(child);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_match("xyz", "main.rs"), None);
        // All letters present but out of order is still not a subsequence.
        assert_eq!(fuzzy_match("sr", "rs"), None);
    }

    #[test]
    fn subsequence_matches() {
        assert!(fuzzy_match("mn", "main.rs").is_some());
        assert!(fuzzy_match("main", "src/main.rs").is_some());
    }

    #[test]
    fn smartcase_lowercase_is_insensitive() {
        assert!(fuzzy_match("cargo", "Cargo.toml").is_some());
    }

    #[test]
    fn smartcase_uppercase_is_sensitive() {
        assert!(fuzzy_match("Cargo", "Cargo.toml").is_some());
        // A capital in the query forces case-sensitivity, so a lowercase
        // candidate no longer matches.
        assert_eq!(fuzzy_match("Cargo", "cargo.lock"), None);
    }

    /// Helper: is `a` a strictly better match than `b` for `query`?
    fn better(query: &str, a: &str, b: &str) -> bool {
        match (fuzzy_match(query, a), fuzzy_match(query, b)) {
            (Some(sa), Some(sb)) => sa > sb,
            _ => false,
        }
    }

    #[test]
    fn basename_hit_beats_directory_hit() {
        // "main" lands on the basename of main.rs but only on a scattered
        // interior run of the other path.
        assert!(better("main", "src/main.rs", "src/domain/helpers.rs"));
    }

    #[test]
    fn consecutive_beats_scattered() {
        assert!(better("ab", "ab.txt", "a_x_b.txt"));
    }

    #[test]
    fn boundary_beats_interior() {
        // "fb" hits two word boundaries in foo_bar; in foobar only the start.
        assert!(better("fb", "foo_bar", "foobXrXb"));
    }

    #[test]
    fn shorter_path_wins_on_a_tie() {
        // Same basename match quality; the shallower path should edge ahead.
        assert!(better("rs", "a.rs", "deep/nested/dir/a.rs"));
    }

    #[test]
    fn finder_filters_and_selects() {
        let files = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "README.md".to_string(),
        ];
        let mut ff = FileFinder::new(files);
        assert_eq!(ff.match_count(), 3);
        assert_eq!(ff.selected_path(), Some("README.md")); // sorted-order first

        ff.push('m');
        ff.push('a');
        ff.push('i');
        ff.push('n');
        assert_eq!(ff.match_count(), 1);
        assert_eq!(ff.selected_path(), Some("src/main.rs"));

        ff.backspace();
        ff.backspace();
        ff.backspace();
        ff.backspace();
        assert!(ff.query().is_empty());
        assert_eq!(ff.match_count(), 3);
    }

    #[test]
    fn finder_ranks_basename_match_first() {
        let files = vec![
            "src/domain/widget.rs".to_string(),
            "src/widget.rs".to_string(),
        ];
        let mut ff = FileFinder::new(files);
        ff.push('w');
        ff.push('i');
        ff.push('d');
        // Both contain "wid"; the shorter path should rank first.
        assert_eq!(ff.selected_path(), Some("src/widget.rs"));
    }

    #[test]
    fn selection_movement_clamps() {
        let mut ff = FileFinder::new(vec!["a".to_string(), "b".to_string()]);
        ff.move_up(); // already at top
        assert_eq!(ff.selected_index(), 0);
        ff.move_down();
        assert_eq!(ff.selected_index(), 1);
        ff.move_down(); // already at bottom
        assert_eq!(ff.selected_index(), 1);
    }

    #[test]
    fn typing_resets_selection_to_top() {
        let mut ff = FileFinder::new(vec!["aa".to_string(), "ab".to_string()]);
        ff.move_down();
        assert_eq!(ff.selected_index(), 1);
        ff.push('a');
        assert_eq!(ff.selected_index(), 0);
    }

    #[test]
    fn visible_window_scrolls_to_keep_selection() {
        let files: Vec<String> = (0..10).map(|i| format!("file{i}.rs")).collect();
        let mut ff = FileFinder::new(files);
        let v = ff.visible(3);
        assert_eq!(v.paths, vec!["file0.rs", "file1.rs", "file2.rs"]);
        assert_eq!(v.selected_row, Some(0));
        assert_eq!(v.total, 10);

        for _ in 0..5 {
            ff.move_down();
        }
        let v = ff.visible(3);
        // Selected row 5, window of 3 → scrolled to show 3..6, selection last.
        assert_eq!(v.paths, vec!["file3.rs", "file4.rs", "file5.rs"]);
        assert_eq!(v.selected_row, Some(2));
    }

    #[test]
    fn no_match_has_no_selected_path() {
        let mut ff = FileFinder::new(vec!["main.rs".to_string()]);
        ff.push('z');
        assert_eq!(ff.match_count(), 0);
        assert_eq!(ff.selected_path(), None);
        let v = ff.visible(5);
        assert!(v.paths.is_empty());
        assert_eq!(v.selected_row, None);
    }

    #[test]
    fn gather_files_walks_and_skips() {
        let dir = std::env::temp_dir().join(format!("garden-ff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        std::fs::write(dir.join("src/main.rs"), "x").unwrap();
        std::fs::write(dir.join("target/debug/app"), "x").unwrap();
        std::fs::write(dir.join(".git/config"), "x").unwrap();

        let files = gather_files(&dir, 1000);
        assert_eq!(
            files,
            vec!["README.md".to_string(), "src/main.rs".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gather_project_files_honors_gitignore_in_a_repo() {
        let dir = std::env::temp_dir().join(format!("garden-ff-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("__pycache__")).unwrap();
        std::fs::write(dir.join(".gitignore"), "__pycache__/\n").unwrap();
        std::fs::write(dir.join("kept.rs"), "x").unwrap();
        std::fs::write(dir.join("__pycache__/junk.pyc"), "x").unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());

        // Untracked-but-unignored files show; ignored ones don't.
        let files = gather_project_files(&dir, 1000);
        assert_eq!(files, vec![".gitignore".to_string(), "kept.rs".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gather_project_files_falls_back_to_walking_outside_git() {
        let dir = std::env::temp_dir().join(format!("garden-ff-nogit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(gather_project_files(&dir, 1000), vec!["a.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gather_files_respects_limit() {
        let dir = std::env::temp_dir().join(format!("garden-ff-lim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "x").unwrap();
        }
        let files = gather_files(&dir, 5);
        assert_eq!(files.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
