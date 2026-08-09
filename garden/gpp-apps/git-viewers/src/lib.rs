//! Shared plumbing for the panel-mode GPP app in this crate — `git-log`, the
//! `:Git` history browser. (Its sibling `git-diff` viewer was retired: the one
//! diff/review tool is now `gpp-apps/garden-diff`.)
//!
//! The app pushes a **Petal UI drawer colocated in this crate**
//! (`git_panel.ptl`, `include_str!`'d by the bin), which the host runs
//! in-process, then answers the drawer's `query(kind, arg)` requests over the
//! pipe by shelling out to `git`. This is the sole `:Git` viewer — the host
//! launches the app and runs its drawer; there is no separate in-process copy.
//!
//! This module holds:
//! - [`run_git`] and the pure patch parser ([`parse_patch`], [`number_lines`],
//!   [`diff_record`]),
//! - the `log` / `commit` value shapers the app calls.
//!
//! The stdio protocol loop lives in [`petal_query::App`]: each bin declares its
//! query kinds (with a [`CachePolicy`](petal_query::CachePolicy) per answer) and
//! calls [`serve`](petal_query::App::serve). See `src/bin/git-log.rs`.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

/// The `commit`/`diff` argument selecting the uncommitted working-tree diff (the
/// synthetic row the git-log drawer shows above the history when the tree is dirty).
pub const WORKTREE_ARG: &str = "@worktree";

/// Prefix on a `commit` argument requesting the **full-context** diff — the whole
/// file around every hunk, so a clicked hunk header can uncollapse to the
/// surrounding source (`@full:<hash>`, `@full:@worktree`).
pub const FULL_PREFIX: &str = "@full:";

/// The `-U<n>` context passed to git for a full-context diff — large enough to
/// swallow any real file into a single hunk.
const FULL_CONTEXT: &str = "-U100000";

/// How many commits the log query returns. Full history would be wasteful; 400
/// two-line rows is far more than a pane usefully scrolls.
const LOG_LIMIT: &str = "-400";

/// Caps on one file's / one commit's fetched diff, so a pathological commit can't
/// balloon the panel heap. Overflow collapses to a truncation marker.
const MAX_LINES_PER_FILE: usize = 3000;
const MAX_FILES_PER_COMMIT: usize = 400;

/// Run `git -C <dir> <args…>`, returning stdout or a trimmed stderr error.
pub fn run_git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr);
        let msg = msg.trim();
        let msg = if msg.is_empty() {
            "git command failed"
        } else {
            msg
        };
        return Err(format!("git: {msg}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// `log` — the git-log drawer's commit history query.
// ---------------------------------------------------------------------------

/// `query("log", "")` → `{ repo, branch, worktree_dirty, commits: [ { hash, short,
/// author, date, subject } … ] }`. A repo with no commits yet yields an empty
/// list, not an error, so the panel still opens.
pub fn git_log(dir: &Path) -> Result<Value, String> {
    let toplevel = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    let repo = Path::new(toplevel.trim())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| toplevel.trim().to_string());
    let branch = run_git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let commits = match run_git(
        dir,
        &[
            "log",
            LOG_LIMIT,
            "--date=format:%Y-%m-%d",
            "--format=%H%x09%h%x09%an%x09%ad%x09%s",
        ],
    ) {
        Ok(out) => parse_log(&out),
        // A repo whose HEAD has no commits yet: empty history, not a failure.
        Err(_) => Vec::new(),
    };
    let worktree_dirty = run_git(dir, &["status", "--porcelain", "-uno"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    Ok(json!({
        "repo": repo,
        "branch": branch,
        "worktree_dirty": worktree_dirty,
        "commits": commits,
    }))
}

/// Parse the tab-separated `git log` format above. Malformed lines are skipped.
fn parse_log(out: &str) -> Vec<Value> {
    let mut commits = Vec::new();
    for line in out.lines() {
        let mut cols = line.splitn(5, '\t');
        let (Some(hash), Some(short), Some(author), Some(date), Some(subject)) = (
            cols.next(),
            cols.next(),
            cols.next(),
            cols.next(),
            cols.next(),
        ) else {
            continue;
        };
        if hash.is_empty() {
            continue;
        }
        commits.push(json!({
            "hash": hash,
            "short": short,
            "author": author,
            "date": date,
            "subject": subject,
        }));
    }
    commits
}

// ---------------------------------------------------------------------------
// `commit` / `diff` — one target's numbered unified diff.
// ---------------------------------------------------------------------------

/// `query("commit", arg)` → the diff record for one commit (`arg` = a hash),
/// the working tree ([`WORKTREE_ARG`]), or a [`FULL_PREFIX`]-prefixed full-context
/// request. Shape: `{ error, body, files: [ { path, added, removed, lines: [ {
/// kind, text, old, new } … ] } … ] }`. A git failure folds into `error`, so the
/// drawer renders it inline instead of the app dying.
pub fn git_commit(dir: &Path, arg: &str) -> Value {
    let (target, ctx): (&str, &[&str]) = match arg.strip_prefix(FULL_PREFIX) {
        Some(rest) => (rest, &[FULL_CONTEXT]),
        None => (arg, &[]),
    };
    let (patch, body) = if target == WORKTREE_ARG {
        let mut args = vec!["diff"];
        args.extend_from_slice(ctx);
        args.push("HEAD");
        (run_git(dir, &args), Ok(String::new()))
    } else {
        let mut args = vec!["show"];
        args.extend_from_slice(ctx);
        args.push(target);
        args.push("--format=");
        (
            run_git(dir, &args),
            run_git(dir, &["show", "-s", "--format=%B", target]),
        )
    };
    let patch = match patch {
        Ok(p) => p,
        Err(e) => return error_data(&e),
    };
    diff_record(&patch, &body.unwrap_or_default())
}

/// The `{ error, body, files }` record for a failed fetch. The `error` field is
/// the soft-error channel a drawer surfaces: a non-empty string here, `null` on
/// the success path. Drawers coalesce it (`data.error ?? ""`) so `null` and `""`
/// both mean "no error" — the reply never has to carry an empty placeholder.
pub fn error_data(msg: &str) -> Value {
    json!({ "error": msg, "body": "", "files": [] })
}

/// The role of one line in a unified diff.
#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Hunk,
    Add,
    Del,
    Context,
}

/// One parsed unified-diff line: its role and text (marker stripped; hunk header
/// kept whole).
struct PatchLine {
    kind: LineKind,
    text: String,
}

/// One changed file's parsed line diff.
struct FileDetail {
    path: String,
    added: u32,
    removed: u32,
    lines: Vec<PatchLine>,
}

/// Shape a raw unified `patch` (+ optional commit `body`) into the `{ error, body,
/// files: [ { path, added, removed, lines } … ] }` record the drawers read.
/// Applies the [`MAX_FILES_PER_COMMIT`] / [`MAX_LINES_PER_FILE`] caps, collapsing
/// overflow to a truncation marker.
pub fn diff_record(patch: &str, body: &str) -> Value {
    let files = parse_patch(patch);
    let mut file_data = Vec::new();
    for f in files.iter().take(MAX_FILES_PER_COMMIT) {
        let numbers = number_lines(&f.lines);
        let mut lines = Vec::new();
        for (line, (old, new)) in f.lines.iter().zip(numbers).take(MAX_LINES_PER_FILE) {
            lines.push(line_record(line, old, new));
        }
        if f.lines.len() > MAX_LINES_PER_FILE {
            let more = f.lines.len() - MAX_LINES_PER_FILE;
            lines.push(line_record(
                &PatchLine {
                    kind: LineKind::Context,
                    text: format!("… {more} more lines (truncated)"),
                },
                0,
                0,
            ));
        }
        file_data.push(json!({
            "path": f.path,
            "added": f.added,
            "removed": f.removed,
            "lines": lines,
        }));
    }
    if files.len() > MAX_FILES_PER_COMMIT {
        let more = files.len() - MAX_FILES_PER_COMMIT;
        file_data.push(json!({
            "path": format!("… {more} more files (truncated)"),
            "added": 0,
            "removed": 0,
            "lines": [],
        }));
    }
    json!({
        // No error on the success path: `null`, not `""`. Drawers read the
        // field null-safely (`data.error ?? ""`), so absence is the natural
        // "no error" — see the protocol note in error_data.
        "error": Value::Null,
        "body": body.trim_end(),
        "files": file_data,
    })
}

/// One diff line as the record the drawer reads.
fn line_record(line: &PatchLine, old: u32, new: u32) -> Value {
    let kind = match line.kind {
        LineKind::Hunk => "hunk",
        LineKind::Add => "add",
        LineKind::Del => "del",
        LineKind::Context => "context",
    };
    json!({ "kind": kind, "text": line.text, "old": old, "new": new })
}

/// Parse `git diff`/`git show` **unified diff** output into per-file line diffs.
/// Splits on `diff --git` headers; within each file keeps hunk headers, additions,
/// deletions, and context lines, deriving added/removed counts. Non-content lines
/// (`index`, mode, rename, `\ No newline`, any commit header before the first
/// `diff --git`) are skipped. Ported from `garden-app`'s `diff::parse_patch`.
fn parse_patch(out: &str) -> Vec<FileDetail> {
    let mut files: Vec<FileDetail> = Vec::new();
    let mut minus_path: Option<String> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            files.push(FileDetail {
                path,
                added: 0,
                removed: 0,
                lines: Vec::new(),
            });
            minus_path = None;
            continue;
        }
        let Some(cur) = files.last_mut() else {
            continue;
        };
        if let Some(p) = line.strip_prefix("--- ") {
            minus_path = header_path(p, "a/");
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if let Some(p) = header_path(p, "b/").or_else(|| minus_path.take()) {
                cur.path = p;
            }
        } else if line.starts_with("@@") {
            cur.lines.push(PatchLine {
                kind: LineKind::Hunk,
                text: line.to_string(),
            });
        } else if let Some(t) = line.strip_prefix('+') {
            cur.added += 1;
            cur.lines.push(PatchLine {
                kind: LineKind::Add,
                text: t.to_string(),
            });
        } else if let Some(t) = line.strip_prefix('-') {
            cur.removed += 1;
            cur.lines.push(PatchLine {
                kind: LineKind::Del,
                text: t.to_string(),
            });
        } else if let Some(t) = line.strip_prefix(' ') {
            cur.lines.push(PatchLine {
                kind: LineKind::Context,
                text: t.to_string(),
            });
        }
        // Everything else (index, mode, rename, `\ No newline…`) is skipped.
    }
    files
}

/// Extract a path from a `---`/`+++` header token: strip the `a/`/`b/` prefix;
/// `/dev/null` (add/delete sentinel) yields `None`.
fn header_path(token: &str, prefix: &str) -> Option<String> {
    let token = token.trim();
    if token == "/dev/null" {
        return None;
    }
    Some(token.strip_prefix(prefix).unwrap_or(token).to_string())
}

/// Derive per-line (old, new) 1-based source line numbers by walking hunk headers
/// (`@@ -a[,b] +c[,d] @@`). `0` means "no number on that side".
fn number_lines(lines: &[PatchLine]) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(lines.len());
    let mut old = 0u32;
    let mut new = 0u32;
    let mut counting = false;
    for line in lines {
        match line.kind {
            LineKind::Hunk => {
                counting = match parse_hunk_header(&line.text) {
                    Some((o, n)) => {
                        old = o;
                        new = n;
                        true
                    }
                    None => false,
                };
                out.push((0, 0));
            }
            LineKind::Context if counting => {
                out.push((old, new));
                old += 1;
                new += 1;
            }
            LineKind::Del if counting => {
                out.push((old, 0));
                old += 1;
            }
            LineKind::Add if counting => {
                out.push((0, new));
                new += 1;
            }
            _ => out.push((0, 0)),
        }
    }
    out
}

/// Parse `@@ -a[,b] +c[,d] @@ …` into the starting (old, new) line numbers.
fn parse_hunk_header(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    let first_num = |s: &str| -> Option<u32> { s.split(',').next()?.parse().ok() };
    Some((first_num(old_part)?, first_num(new_part)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn field<'d>(v: &'d Value, name: &str) -> &'d Value {
        v.get(name)
            .unwrap_or_else(|| panic!("missing field {name}"))
    }

    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first commit"]);
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "second commit\n\nwith a body line"]);
        std::fs::write(dir.path().join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        dir
    }

    #[test]
    fn log_shapes_history_and_dirty_flag() {
        let repo = temp_repo();
        let log = git_log(repo.path()).unwrap();
        assert_eq!(field(&log, "branch"), "main");
        assert_eq!(field(&log, "worktree_dirty"), true);
        let commits = field(&log, "commits").as_array().unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(field(&commits[0], "subject"), "second commit");
        assert_eq!(field(&commits[1], "subject"), "first commit");
    }

    #[test]
    fn commit_shapes_numbered_diff_and_body() {
        let repo = temp_repo();
        let head = git_log(repo.path()).unwrap()["commits"][0]["hash"]
            .as_str()
            .unwrap()
            .to_string();
        let data = git_commit(repo.path(), &head);
        // Success reports no error as `null` (the drawer coalesces it to "").
        assert!(field(&data, "error").is_null());
        assert!(field(&data, "body")
            .as_str()
            .unwrap()
            .contains("with a body line"));
        let files = field(&data, "files").as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(field(&files[0], "path"), "a.txt");
        assert_eq!(field(&files[0], "added"), 2);
        assert_eq!(field(&files[0], "removed"), 1);
        let lines = field(&files[0], "lines").as_array().unwrap();
        assert_eq!(field(&lines[0], "kind"), "hunk");
        assert_eq!(field(&lines[2], "kind"), "del");
        assert_eq!(field(&lines[2], "old"), 2);
        assert_eq!(field(&lines[3], "kind"), "add");
        assert_eq!(field(&lines[3], "new"), 2);
    }

    #[test]
    fn worktree_and_bad_rev() {
        let repo = temp_repo();
        let wt = git_commit(repo.path(), WORKTREE_ARG);
        assert!(field(&wt, "error").is_null());
        assert_eq!(field(&wt, "files").as_array().unwrap().len(), 1);
        let bad = git_commit(repo.path(), "no-such-rev");
        assert!(!field(&bad, "error").as_str().unwrap().is_empty());
    }

    #[test]
    fn full_context_spans_the_file() {
        let repo = temp_repo();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap()
        };
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "park"]);
        let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
        std::fs::write(repo.path().join("big.txt"), &long).unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "add big"]);
        let mut edited: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        edited[30] = "line 30 CHANGED".into();
        std::fs::write(repo.path().join("big.txt"), edited.join("\n") + "\n").unwrap();

        let count = |arg: &str| -> usize {
            let d = git_commit(repo.path(), arg);
            let big = field(&d, "files")
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["path"] == "big.txt")
                .unwrap()
                .clone();
            big["lines"].as_array().unwrap().len()
        };
        assert!(count(WORKTREE_ARG) < 15);
        assert!(count(&format!("{FULL_PREFIX}{WORKTREE_ARG}")) >= 60);
    }
}
