//! Diff computation, before/after projection, and edit write-back for
//! `garden-diff`.
//!
//! Forked from `garden-app`'s `diff.rs` + `projection/` (which live in the host
//! binary with no lib target, so a subprocess crate can't import them — the same
//! reimplement-per-app pattern as `git-viewers`).
//!
//! Both editable views — the unified stream and the split's after column — are
//! described here as **projections** (`garden_core::projection`): every line
//! records where it came from, and the host folds the user's edits back into the
//! files itself. Nothing in this crate re-reads the edited text to work out what
//! was meant, so the `@@@ file:` / `@@@ hunk:` markers are ordinary chrome: they
//! label the stream for a reader and can no longer corrupt a write-back.

use std::path::{Path, PathBuf};
use std::process::Command;

/// First line of every file's section: names the (repo-relative) file.
pub const FILE_PREFIX: &str = "@@@ file: ";
/// First line of every hunk's section: heads that hunk's lines.
pub const HUNK_PREFIX: &str = "@@@ hunk: ";

// ---------------------------------------------------------------------------
// git plumbing + unified-diff parse
// ---------------------------------------------------------------------------

/// Run `git -C <dir> <args...>`, returning stdout or a trimmed stderr error.
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

/// The role of one unified-diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Add,
    Del,
    Context,
}

/// One line of a file's unified diff (leading `+`/`-`/space stripped; a hunk
/// header kept whole).
#[derive(Clone, Debug, PartialEq)]
pub struct PatchLine {
    pub kind: LineKind,
    pub text: String,
}

/// One changed file's line diff plus its add/remove counts.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    /// A binary file — git reports `Binary files … differ` instead of hunks, so
    /// the line diff is empty and the stat view labels it rather than drawing a
    /// zero-length bar.
    pub binary: bool,
    pub lines: Vec<PatchLine>,
}

/// Parse `git diff` **unified diff** output into per-file line diffs. Splits on
/// `diff --git`, keeps hunk/add/del/context lines, derives counts, and resolves
/// the path from the `+++`/`---` headers (handling `/dev/null` add/delete).
pub fn parse_patch(out: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut minus_path: Option<String> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            files.push(FileDiff {
                path,
                added: 0,
                removed: 0,
                binary: false,
                lines: Vec::new(),
            });
            minus_path = None;
            continue;
        }
        let Some(cur) = files.last_mut() else {
            continue;
        };
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            cur.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
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
    }
    files
}

/// The working tree's untracked, non-ignored files, each synthesised as an
/// all-added [`FileDiff`] — `git diff` cannot see them, so a review of pending
/// work used to omit exactly the files most likely to need reviewing.
///
/// Listed from `root` (not the caller's `dir`): `git ls-files` reports paths
/// relative to its working directory and only walks the subtree below it, while
/// the diff's paths are repo-root-relative, and the two must agree.
///
/// Each file gets one synthetic `@@ -0,0 +1,n @@` hunk header so it projects like
/// any other file: its span then covers the whole file, which is what makes `dd`
/// on its lines actually remove them from disk. A file that isn't valid UTF-8 is
/// marked `binary` rather than mangled into text (its section is then the
/// explanatory chrome line, as for any binary file); one that can't be read at
/// all is skipped, since there is nothing to show.
fn untracked_files(root: &Path) -> Vec<FileDiff> {
    let Ok(list) = run_git(root, &["ls-files", "--others", "--exclude-standard", "-z"]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in list.split('\0').filter(|p| !p.is_empty()) {
        let Ok(bytes) = std::fs::read(root.join(path)) else {
            continue;
        };
        let mut file = FileDiff {
            path: path.to_string(),
            added: 0,
            removed: 0,
            binary: false,
            lines: Vec::new(),
        };
        // A NUL byte is git's own binary tell, and `from_utf8` catches the rest.
        match std::str::from_utf8(&bytes) {
            Ok(text) if !bytes.contains(&0) => {
                let lines: Vec<&str> = text.lines().collect();
                file.added = lines.len() as u32;
                file.lines.push(PatchLine {
                    kind: LineKind::Hunk,
                    text: format!("@@ -0,0 +1,{} @@", lines.len()),
                });
                file.lines.extend(lines.into_iter().map(|l| PatchLine {
                    kind: LineKind::Add,
                    text: l.to_string(),
                }));
            }
            _ => file.binary = true,
        }
        out.push(file);
    }
    out
}

fn header_path(token: &str, prefix: &str) -> Option<String> {
    let token = token.trim();
    if token == "/dev/null" {
        return None;
    }
    Some(token.strip_prefix(prefix).unwrap_or(token).to_string())
}

/// Resolve the base ref for a diff: an explicit `arg` (verified to exist), else
/// the current branch's upstream, else `main`/`master`.
///
/// The not-a-repository case is separated out first, and deliberately: every ref
/// lookup below fails outside a repository, so folding the two together reported
/// `unknown base ref: main` from a directory that has no refs at all — sending
/// the user to look for a branch when the directory was the problem.
pub fn resolve_base(dir: &Path, arg: &str) -> Result<String, String> {
    if run_git(dir, &["rev-parse", "--git-dir"]).is_err() {
        return Err(format!("not a git repository: {}", dir.display()));
    }
    let arg = arg.trim();
    if !arg.is_empty() {
        if run_git(dir, &["rev-parse", "--verify", "--quiet", arg]).is_err() {
            return Err(format!("unknown base ref: {arg}"));
        }
        return Ok(arg.to_string());
    }
    if let Ok(up) = run_git(
        dir,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        let up = up.trim();
        if !up.is_empty() {
            return Ok(up.to_string());
        }
    }
    for cand in ["main", "master"] {
        if run_git(dir, &["rev-parse", "--verify", "--quiet", cand]).is_ok() {
            return Ok(cand.to_string());
        }
    }
    Err("no base ref found; pass one (e.g. `garden diff main`)".to_string())
}

// ---------------------------------------------------------------------------
// Projection: before/after/unified documents with per-line styles, plus the
// provenance the two editable ones carry
// ---------------------------------------------------------------------------

/// A projected document: the marker+body `text` and a per-line `styles` name
/// (aligned to `text`'s lines).
///
/// The two *editable* projections — unified and after — additionally fill the
/// provenance tracks `kinds` and `line_spans`, which are what the host needs to
/// make the view editable as a projection (`garden_core::projection`): they say
/// where each line came from, so the host can fold the user's edits back into the
/// files rather than anyone having to re-derive the intent from the edited text
/// afterwards. The read-only before side leaves them empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projected {
    pub text: String,
    pub styles: Vec<String>,
    /// One origin character per line — see [`ProjectionSpec::kinds`] in
    /// `garden-script` for the alphabet.
    pub kinds: String,
    /// The span each line belongs to (`-1` = none), parallel to `kinds`.
    pub line_spans: Vec<i64>,
}

/// One editable span of a projection: which file it writes to (an index into
/// [`Doc::sources`]), the 0-based `[start, end)` line range of that file its
/// content replaces, and the group (one per file) it reverts with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanSpec {
    pub source: usize,
    pub start: usize,
    pub end: usize,
    pub group: usize,
}

/// One resolved write-back, as the host's projection folds it: replace lines
/// `[start, end)` of `source` with `lines`. This is what `^S` now sends — edits,
/// not text — so nothing here has to work out what the user meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub source: PathBuf,
    pub start: usize,
    pub end: usize,
    pub lines: Vec<String>,
    /// What `[start, end)` of `source` held when the view was built — the host's
    /// projection captured it and sends it along. The apply compares it with the
    /// file's current lines: a mismatch means the file changed on disk since the
    /// review opened, and this write is about to overwrite that change. `None`
    /// when the host recorded no expectation, in which case nothing is checked.
    pub expected: Option<Vec<String>>,
}

/// What an [`apply_edits`] did: the files written (by file name, path order),
/// and the ones whose content had drifted from what the view expected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub written: Vec<String>,
    /// `(file name, how many of its spans were stale)`, in path order. Non-empty
    /// means content that was on disk has been overwritten — the write still
    /// happened (refusing would strand the user's edits in a buffer they cannot
    /// save), so this is what the status has to say out loud.
    pub stale: Vec<(String, usize)>,
}

/// A summary row for the file list (and the `stat` view's diagram).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSummary {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    /// 0-based line of this file's `@@@ file:` marker in each projection — where
    /// a view must scroll to put the file at the top. The before and after sides
    /// are emitted in lockstep so their numbers agree, but both are recorded
    /// rather than assumed.
    pub line_before: usize,
    pub line_after: usize,
    pub line_unified: usize,
}

/// The whole diff, projected three ways: the read-only `before` (base side)
/// beside the editable `after` (working-tree side) — sharing marker order so the
/// split panes align hunk-by-hunk — plus the `unified` single-column projection
/// (classic `+`/`-`/context stream) for the unified view mode.
///
/// Both editable sides are projections in the same sense, and differ only in what
/// they show: the unified stream carries the base's dropped lines too (so
/// deleting a `-` line can revive one), while the after column is a plain picture
/// of the new file, where every line is content the working tree currently holds.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub base: String,
    pub files: Vec<FileSummary>,
    pub before: Projected,
    pub after: Projected,
    pub unified: Projected,
    /// The files both projections write back to, indexed by [`SpanSpec::source`]
    /// — one per changed file, in file order.
    pub sources: Vec<String>,
    /// The after column's editable spans, in hunk order.
    pub after_spans: Vec<SpanSpec>,
    /// The unified projection's editable spans, in hunk order.
    pub unified_spans: Vec<SpanSpec>,
    /// Populated only in PR mode (`garden diff <PR#>`); `None` for a plain local
    /// diff. Rendered as the drawer's collapsible description block.
    pub pr: Option<PrMeta>,
    /// Whether this diff can be edited and saved back.
    ///
    /// True for every diff that ends at the **working tree** — the ones
    /// `build` / `build_reviewed` produce — because the after side's line
    /// numbers address the files that are actually on disk, which is what makes
    /// `^S` meaningful.
    ///
    /// False for a [`build_range`] diff scoped to a commit. Its after side is
    /// a picture of a file *as it was at that commit*, so its line numbers
    /// address a blob and not the checkout; writing them back would splice the
    /// past into the present at coincidentally-matching offsets. Such a diff is
    /// for reading, and the drawer renders it read-only.
    pub editable: bool,
    /// How the `+`/`-`/space markers reach the reader: in the region's gutter
    /// (the editable views, where keeping the buffer free of markers is what
    /// lets it be edited like a file) or baked into the line text (the
    /// read-only scoped views, which are plain `text_view`s with no projection
    /// and therefore no gutter to draw into).
    pub markers_in_text: bool,
}

/// One commit in the range under review, as the drawer's COMMITS list shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommitInfo {
    /// The full hash — what a scoped diff is built from, so it is never
    /// ambiguous the way an abbreviation can become in a growing repo.
    pub sha: String,
    /// The abbreviated hash, for display.
    pub short: String,
    /// The first line of the message.
    pub subject: String,
    pub author: String,
    /// Author date, `YYYY-MM-DD`.
    pub date: String,
}

/// The commits the review covers: everything reachable from `HEAD` but not from
/// `base`, newest first — the same set `git log base..HEAD` prints, which is the
/// set a PR contains.
///
/// A range that resolves to nothing (the branch is level with its base, or the
/// diff is of uncommitted work only) is an empty list, not an error: "no commits
/// of your own yet" is a normal state for `garden diff`, and the drawer says so
/// rather than showing a failure.
///
/// The unit separator (`%x1f`) delimits fields because every one of them can
/// contain the obvious alternatives — a subject with a tab, an author name with
/// a comma — and the record separator is the newline git already gives us.
pub fn commits(dir: &Path, base: &str) -> Result<Vec<CommitInfo>, String> {
    let range = format!("{base}..HEAD");
    let out = run_git(
        dir,
        &[
            "log",
            "--no-color",
            "--date=short",
            "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ad",
            &range,
        ],
    )?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            Some(CommitInfo {
                sha: parts.next()?.to_string(),
                short: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
                author: parts.next()?.to_string(),
                date: parts.next().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// GitHub PR metadata for PR-review mode, fetched once via `gh pr view`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrMeta {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub state: String,
    pub url: String,
    /// The PR description body (may be multi-line / empty).
    pub body: String,
    /// Conversation (issue-level) comments, oldest first. Rendered under the
    /// description block, not woven into the diff.
    pub conversation: Vec<Comment>,
    /// Inline review comments, oldest first. Woven into the unified view at the
    /// diff line they were left on (see [`weave_comments`]).
    pub inline: Vec<Comment>,
}

/// One GitHub comment — a conversation comment (no `path`/`line`) or an inline
/// review comment anchored to a file and diff line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comment {
    pub author: String,
    /// `YYYY-MM-DD` (the date portion of the ISO timestamp).
    pub date: String,
    pub body: String,
    /// The file an inline comment is anchored to.
    pub path: Option<String>,
    /// The 1-based line an inline comment targets — the new-file line, or the
    /// old-file line when [`left_side`](Self::left_side) is set.
    pub line: Option<usize>,
    /// True when the comment targets the old (deleted) side of the diff.
    pub left_side: bool,
}

/// The chrome line standing in for a binary file's (absent) hunks, so its
/// section says why it is empty instead of looking like a file with no changes.
pub const BINARY_NOTE: &str = "binary file — not shown";

/// Build the **local** before/after projection of `git diff <base>` (base →
/// working tree) rooted at `dir`, with no PR comments — see [`build_reviewed`].
///
/// This is the form behind `garden diff` and the no-arg `garden pr` fallback, so
/// it answers "what have I got pending?": untracked, non-ignored files are folded
/// in as all-added files, which `git diff` alone cannot see. A PR review must not
/// do that — see [`build_reviewed`].
pub fn build(dir: &Path, base: &str, label: &str) -> Result<Doc, String> {
    build_inner(dir, &[base], label, &[], true, false)
}

/// Build the before/after projection of `git diff <base>` (base → working tree)
/// rooted at `dir`. `base` is the git rev to diff against; `label` is how it is
/// named in the projected titles (they differ in PR mode, where `base` is an
/// opaque merge-base SHA but `label` is the base branch name). One `git diff`,
/// emitted twice: the after side (context + added, editable, with regions mapping
/// back to the files) and the before side (context + removed, read-only). Empty
/// diff → both sides carry a "(no changes)" note and no regions.
///
/// `comments` are inline PR review comments (empty for a plain local diff): they
/// are woven into the **unified** projection only, at the line they were left on.
/// The split sides deliberately stay comment-free — the after column is a picture
/// of the new file, and prose interleaved into it would be prose the reader has
/// to mentally subtract to see the file.
///
/// This is the **PR-review** form: the diff is exactly what `git diff <base>`
/// contains, with no working-tree extras. Untracked files are deliberately left
/// out — a reviewer reading PR #123 must see the PR, not whatever happens to be
/// lying around the checkout. [`build`] is the local form that folds them in.
pub fn build_reviewed(
    dir: &Path,
    base: &str,
    label: &str,
    comments: &[Comment],
) -> Result<Doc, String> {
    build_inner(dir, &[base], label, comments, false, false)
}

/// Build a **commit-scoped**, read-only diff: exactly `git diff <from> <to>`,
/// with no working tree involved.
///
/// This is what "show me just this commit" asks for — `from` is the commit's
/// parent and `to` the commit itself — and also what "everything since this
/// commit" asks for, with `to` left at `HEAD`. Untracked files are excluded
/// (they belong to no commit) and no PR comments are woven in (their anchors
/// are the PR's diff, not this one).
///
/// The result is deliberately **not** editable: see [`Doc::editable`] for why a
/// diff that does not end at the working tree has nothing to write back to. Its
/// markers are baked into the text, since a read-only view has no projection
/// and so no gutter to draw them in.
pub fn build_range(dir: &Path, from: &str, to: &str, label: &str) -> Result<Doc, String> {
    build_inner(dir, &[from, to], label, &[], false, true)
}

/// The shared body of [`build`], [`build_reviewed`] and [`build_range`].
///
/// `revs` is what follows `git diff`: one rev for a base-to-working-tree diff,
/// two for a commit-scoped one. `untracked` selects whether working-tree files
/// git has never seen are folded into the file list. `markers_in_text` bakes
/// the `+`/`-`/space markers into the line text instead of leaving them to the
/// region's gutter — see [`Doc::markers_in_text`].
fn build_inner(
    dir: &Path,
    revs: &[&str],
    label: &str,
    comments: &[Comment],
    untracked: bool,
    markers_in_text: bool,
) -> Result<Doc, String> {
    let toplevel = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(toplevel.trim());
    let mut diff_args = vec!["diff"];
    diff_args.extend_from_slice(revs);
    let patch = run_git(dir, &diff_args)?;
    let mut files = parse_patch(&patch);
    if untracked {
        files.extend(untracked_files(&root));
        // git emits its own diff in path order; the merged list keeps that order
        // so a new file sits where the reader expects to find it, not in a
        // separate tail block. The sort is stable, so a path that somehow appears
        // twice keeps git's copy first.
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    let mut after = Projected::default();
    let mut before = Projected::default();
    // What the projected views call themselves. An editable diff ends at the
    // working tree and says how to save it; a scoped one is a picture of a
    // commit, and telling its reader to press ^S would be a lie.
    let (after_title, unified_title) = if markers_in_text {
        (
            format!("review: {label}  (read-only — a commit's diff)"),
            format!("unified: {label}  (read-only — a commit's diff)"),
        )
    } else {
        (
            format!("review: {label} → working tree  (edit below; ^S saves)"),
            format!("unified: {label} → working tree  (edit below; ^S saves)"),
        )
    };
    // The after column's title is locked chrome for the same reason the unified
    // one is: it belongs to the view, not to the change.
    push_proj(&mut after, &after_title, "title", 'l', -1);
    push_line(&mut before, &format!("before: {label}"), "title");
    if files.is_empty() {
        push_proj(
            &mut after,
            &format!("(no changes vs {label})"),
            "dim",
            'c',
            -1,
        );
        push_line(&mut before, &format!("(no changes vs {label})"), "dim");
    }
    let mut unified = Projected::default();
    // The title belongs to the *view*, not to the change: it is locked chrome, so
    // `dd` on it is refused rather than quietly deleting a line from a file it has
    // nothing to do with.
    push_proj(&mut unified, &unified_title, "title", 'l', -1);
    if files.is_empty() {
        push_proj(
            &mut unified,
            &format!("(no changes vs {label})"),
            "dim",
            'c',
            -1,
        );
    }
    // Each file's marker line is recorded as it is written — the projections are
    // built by appending, so the current line count *is* the marker's index.
    let mut summaries = Vec::with_capacity(files.len());
    let mut sources = Vec::with_capacity(files.len());
    let mut after_spans = Vec::new();
    let mut unified_spans = Vec::new();
    for (index, f) in files.iter().enumerate() {
        summaries.push(FileSummary {
            path: f.path.clone(),
            added: f.added,
            removed: f.removed,
            binary: f.binary,
            line_before: before.styles.len(),
            line_after: after.styles.len(),
            line_unified: unified.styles.len(),
        });
        // On the after side the file marker is *locked* chrome, not a group
        // header: the column shows only the new file, so it holds nothing that
        // could restore a deleted line. Offering a revert there would drop the
        // file's additions and silently leave its deletions in place — a half
        // revert. That gesture lives in the unified view, which has both sides.
        push_proj(
            &mut after,
            &format!("{FILE_PREFIX}{}", f.path),
            "file",
            'l',
            -1,
        );
        push_line(&mut before, &format!("{FILE_PREFIX}{}", f.path), "file");
        // The file marker heads the *group* of this file's hunks: deleting it is
        // read as "drop this file's changes". It points at the first span the file
        // is about to push, which is where the group id is discoverable. A binary
        // file pushes no spans at all, so there is no group to head and the marker
        // is locked chrome — pointing it at the *next* file's span would make `dd`
        // revert a file the user was not looking at.
        if f.binary {
            push_proj(
                &mut unified,
                &format!("{FILE_PREFIX}{}", f.path),
                "file",
                'l',
                -1,
            );
        } else {
            push_proj(
                &mut unified,
                &format!("{FILE_PREFIX}{}", f.path),
                "file",
                'g',
                unified_spans.len() as i64,
            );
        }
        sources.push(root.join(&f.path).to_string_lossy().into_owned());
        // A binary file has no hunks, so without this its section is a bare
        // marker butted against the next file's — a file that looks unchanged for
        // no stated reason. The note is chrome carrying no span: it is not
        // content, so no edit to it can reach a file.
        if f.binary {
            for p in [&mut unified, &mut after] {
                push_proj(p, BINARY_NOTE, "dim", 'c', -1);
            }
            push_line(&mut before, BINARY_NOTE, "dim");
        }
        emit_file(&mut after, &mut before, &mut after_spans, index, f);
        emit_unified(
            &mut unified,
            &mut unified_spans,
            index,
            index,
            f,
            comments,
            markers_in_text,
        );
    }

    Ok(Doc {
        base: label.to_string(),
        files: summaries,
        before,
        after,
        unified,
        sources,
        after_spans,
        unified_spans,
        pr: None,
        editable: !markers_in_text,
        markers_in_text,
    })
}

/// Emit one file's hunks as a classic unified diff into `u`: a `@@@ hunk:` marker
/// carrying the `@@` header, then each line prefixed `+` (added), `-` (removed),
/// or ` ` (context), styled to match the split view's add/remove colors. Each
/// hunk also pushes a [`SpanSpec`] onto `spans` — the file range that hunk's
/// content replaces — and every emitted line records its origin in the
/// projection's `kinds` / `line_spans` tracks, which is what makes the view
/// editable (see [`Projected`]).
///
/// `comments` is the whole PR's inline comment set; the ones anchored to this file
/// are threaded in directly under the line they target (matched on the new-file
/// line number, or the old-file one for a `LEFT`-side comment). Comments whose
/// anchor line is no longer in the diff (outdated threads) are appended after the
/// file's hunks so they are never silently dropped. Comment lines are recorded as
/// chrome, so editing or deleting one changes no file.
fn emit_unified(
    u: &mut Projected,
    spans: &mut Vec<SpanSpec>,
    source: usize,
    group: usize,
    f: &FileDiff,
    comments: &[Comment],
    markers_in_text: bool,
) {
    // The marker a content line wears in its *text*. Empty in the editable
    // views, where the host draws it in the region's gutter instead (see
    // `Doc::markers_in_text`); the classic `+`/`-`/space in the read-only ones.
    let mark = |m: &'static str| -> &'static str {
        if markers_in_text {
            m
        } else {
            ""
        }
    };
    let mine: Vec<&Comment> = comments
        .iter()
        .filter(|c| c.path.as_deref() == Some(f.path.as_str()))
        .collect();
    let mut placed = vec![false; mine.len()];
    // 1-based line cursors into the old / new file, advanced as lines are emitted.
    let (mut old_no, mut new_no) = (0usize, 0usize);
    // The hunk being accumulated: its 0-based new-file start. `None` before the
    // file's first `@@` header, so a file with no hunks (a binary file with stale
    // comments) records no span and its comment lines carry none.
    let mut open: Option<usize> = None;

    for line in &f.lines {
        match line.kind {
            LineKind::Hunk => {
                close_hunk(spans, source, group, &mut open, new_no);
                old_no = hunk_old_start(&line.text).unwrap_or(1);
                new_no = hunk_new_start(&line.text).unwrap_or(1);
                // The header is chrome that *heads* the span it opens, so
                // deleting it is read as "revert this hunk". Its span index is
                // the one `close_hunk` will push.
                //
                // It names its file as well as its line range. The unified view
                // is one long stream, so a reader deep inside a multi-hunk file
                // has scrolled the `@@@ file:` heading off the top and has to
                // scroll back to answer "which file am I in?". Repeating the
                // path on every hunk header answers it in place. The split view
                // does not do this: it has the FILES column beside it.
                push_proj(
                    u,
                    &format!("{HUNK_PREFIX}{} {}", f.path, line.text),
                    "hunk",
                    'h',
                    spans.len() as i64,
                );
                open = Some(new_no.saturating_sub(1));
                continue;
            }
            // The `+`/`-`/space marker is NOT part of the line's text: the
            // host draws it in the region's gutter from the origin (`kind`)
            // recorded here. That is what keeps the buffer holding the file's
            // own text, so a join, a column selection or a search in the
            // unified view behaves exactly as it would in the file itself —
            // rather than one character out of step with it.
            LineKind::Add => push_block_line(
                u,
                spans.len(),
                &open,
                '+',
                format!("{}{}", mark("+"), line.text),
                "added",
            ),
            LineKind::Del => push_block_line(
                u,
                spans.len(),
                &open,
                '-',
                format!("{}{}", mark("-"), line.text),
                "removed",
            ),
            LineKind::Context => push_block_line(
                u,
                spans.len(),
                &open,
                ' ',
                format!("{}{}", mark(" "), line.text),
                "",
            ),
        }
        let (at_old, at_new) = match line.kind {
            LineKind::Add => (None, Some(new_no)),
            LineKind::Del => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        for (i, c) in mine.iter().enumerate() {
            if placed[i] {
                continue;
            }
            let hit = match (c.left_side, c.line) {
                (true, Some(l)) => at_old == Some(l),
                (false, Some(l)) => at_new == Some(l),
                _ => false,
            };
            if hit {
                push_comment(u, spans.len(), &open, c);
                placed[i] = true;
            }
        }
        match line.kind {
            LineKind::Add => new_no += 1,
            LineKind::Del => old_no += 1,
            _ => {
                old_no += 1;
                new_no += 1;
            }
        }
    }

    let orphans: Vec<&&Comment> = mine
        .iter()
        .enumerate()
        .filter(|(i, _)| !placed[*i])
        .map(|(_, c)| c)
        .collect();
    if !orphans.is_empty() {
        push_block_line(
            u,
            spans.len(),
            &open,
            'c',
            "  \u{250c} comments on lines outside this diff".to_string(),
            "comment",
        );
        for c in orphans {
            push_comment(u, spans.len(), &open, c);
        }
    }
    close_hunk(spans, source, group, &mut open, new_no);
}

/// Finish the open hunk (if any), recording it as an editable span. `new_no` is
/// the 1-based new-file cursor just past the hunk's last new-side line, so the
/// range it replaces is `[start, new_no - 1)`.
fn close_hunk(
    spans: &mut Vec<SpanSpec>,
    source: usize,
    group: usize,
    open: &mut Option<usize>,
    new_no: usize,
) {
    if let Some(start) = open.take() {
        spans.push(SpanSpec {
            source,
            start,
            end: new_no.saturating_sub(1).max(start),
            group,
        });
    }
}

/// Append one line of an open hunk: its text, its style, and its origin. A line
/// emitted outside any hunk belongs to no span (`-1`) and so contributes to no
/// file.
fn push_block_line(
    u: &mut Projected,
    span: usize,
    open: &Option<usize>,
    kind: char,
    display: String,
    style: &str,
) {
    let span = open.map(|_| span as i64).unwrap_or(-1);
    push_proj(u, &display, style, kind, span);
}

/// Thread one review comment into the unified stream as an indented block:
/// an author/date header line then the body, all styled `comment` so the host
/// paints it as a distinct band. Recorded as chrome — a comment is not file
/// content, so editing or deleting one writes nothing.
fn push_comment(u: &mut Projected, span: usize, open: &Option<usize>, c: &Comment) {
    let side = if c.left_side {
        " (on the base side)"
    } else {
        ""
    };
    let mut line = |text: String| push_block_line(u, span, open, 'c', text, "comment");
    line(format!("  \u{250c} @{} \u{b7} {}{side}", c.author, c.date));
    for body_line in c.body.replace('\r', "").lines() {
        line(format!("  \u{2502} {body_line}"));
    }
    line("  \u{2514}".to_string());
}

/// Append one line to a projected document *and* record its projection origin
/// (`kind`) and owning span. The two tracks stay exactly parallel to the text's
/// lines, which is the contract `edit_view_projection` relies on.
fn push_proj(p: &mut Projected, text: &str, style: &str, kind: char, span: i64) {
    push_line(p, text, style);
    p.kinds.push(kind);
    p.line_spans.push(span);
}

/// Append `text` as one line (plus its style) to a projected document.
fn push_line(p: &mut Projected, text: &str, style: &str) {
    p.text.push_str(text);
    p.text.push('\n');
    p.styles.push(style.to_string());
}

/// Emit one file's hunks to both sides of the split, pushing the after column's
/// editable [`SpanSpec`]s. Both sides share marker order so they stay aligned;
/// each side gets the per-line style track it paints with, and the after side
/// gets the provenance tracks that make it a projection.
///
/// The after column is a picture of the new file, so its origins are simple:
/// every content line is `Live` (` ` for context, `+` for a line this change
/// added — the distinction the styling uses), and the markers are locked chrome.
/// There are no ghosts, because a line the change deleted is exactly what this
/// column does *not* show; it is on the before side, read-only, opposite.
fn emit_file(
    after: &mut Projected,
    before: &mut Projected,
    spans: &mut Vec<SpanSpec>,
    source: usize,
    f: &FileDiff,
) {
    let mut hunk: Option<Hunk> = None;
    let flush =
        |h: Hunk, after: &mut Projected, before: &mut Projected, spans: &mut Vec<SpanSpec>| {
            let start0 = h.new_start.saturating_sub(1);
            // The span this hunk's lines belong to — pushed after they are emitted,
            // so its index is the current length.
            let span = spans.len() as i64;
            // Hunk marker on both sides (labelled with the other side's delta).
            let after_extra = if h.removed > 0 {
                format!("  ({} removed)", h.removed)
            } else {
                String::new()
            };
            let before_extra = if h.added > 0 {
                format!("  ({} added on the right)", h.added)
            } else {
                String::new()
            };
            push_proj(
                after,
                &format!("{HUNK_PREFIX}{}{after_extra}", h.header),
                "hunk",
                'l',
                -1,
            );
            push_line(
                before,
                &format!("{HUNK_PREFIX}{}{before_extra}", h.header),
                "hunk",
            );
            for (line, added) in &h.new_lines {
                let (style, kind) = if *added { ("added", '+') } else { ("", ' ') };
                push_proj(after, line, style, kind, span);
            }
            for (line, removed) in &h.base_lines {
                push_line(before, line, if *removed { "removed" } else { "" });
            }
            spans.push(SpanSpec {
                source,
                start: start0,
                end: start0 + h.new_lines.len(),
                // One group per file, as in the unified projection — nothing on this
                // side heads a group today, but the spans are still that file's.
                group: source,
            });
        };
    for line in &f.lines {
        match line.kind {
            LineKind::Hunk => {
                if let Some(h) = hunk.take() {
                    flush(h, after, before, spans);
                }
                hunk = Some(Hunk::start(&line.text));
            }
            LineKind::Context => {
                if let Some(h) = hunk.as_mut() {
                    h.new_lines.push((line.text.clone(), false));
                    h.base_lines.push((line.text.clone(), false));
                }
            }
            LineKind::Add => {
                if let Some(h) = hunk.as_mut() {
                    h.new_lines.push((line.text.clone(), true));
                    h.added += 1;
                }
            }
            LineKind::Del => {
                if let Some(h) = hunk.as_mut() {
                    h.base_lines.push((line.text.clone(), true));
                    h.removed += 1;
                }
            }
        }
    }
    if let Some(h) = hunk.take() {
        flush(h, after, before, spans);
    }
}

/// A hunk being accumulated. `new_lines`/`base_lines` carry `(text, is_change)`
/// so the after/before sides can style added/removed lines.
struct Hunk {
    header: String,
    new_lines: Vec<(String, bool)>,
    base_lines: Vec<(String, bool)>,
    new_start: usize,
    removed: usize,
    added: usize,
}

impl Hunk {
    fn start(header: &str) -> Hunk {
        Hunk {
            header: header.to_string(),
            new_lines: Vec::new(),
            base_lines: Vec::new(),
            new_start: hunk_new_start(header).unwrap_or(1),
            removed: 0,
            added: 0,
        }
    }
}

/// Parse the old-file start line `a` from a `@@ -a,b +c,d @@ …` hunk header.
fn hunk_old_start(header: &str) -> Option<usize> {
    let rest = header.strip_prefix("@@ -")?;
    let (old_part, _) = rest.split_once(" +")?;
    old_part.split(',').next()?.parse().ok()
}

/// Parse the new-file start line `c` from a `@@ -a,b +c,d @@ …` hunk header.
fn hunk_new_start(header: &str) -> Option<usize> {
    let rest = header.strip_prefix("@@ -")?;
    let (_old, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    new_part.split(',').next()?.parse().ok()
}

// ---------------------------------------------------------------------------
// Save: apply the edits the host's projection resolved
// ---------------------------------------------------------------------------

/// The line ending a spliced file is rejoined with: whichever of `\r\n` / `\n` the
/// majority of `original`'s existing terminators use (ties, and a file with no
/// terminator at all, go to `\n`).
///
/// A whole-file decision rather than a per-line one, deliberately: an edited hunk
/// mixes untouched lines with lines the user typed, and the typed ones have no
/// terminator of their own to preserve — so they have to adopt *something*, and
/// the file's own convention is the only answer that doesn't leave a file with
/// ragged endings. A lone `\r` is not a terminator (only `\r\n` and `\n` are), so
/// an old-Mac line stays intact as content.
fn dominant_line_ending(original: &str) -> &'static str {
    let (mut crlf, mut lf) = (0usize, 0usize);
    for seg in original.split_inclusive('\n') {
        if seg.ends_with("\r\n") {
            crlf += 1;
        } else if seg.ends_with('\n') {
            lf += 1;
        }
    }
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Splice `content` into `original`, replacing the 0-based line spans named by
/// each `(target, content)`. Spans applied high-to-low; the original's trailing
/// newline and its dominant line ending (see [`dominant_line_ending`]) are both
/// preserved, so editing one hunk of a CRLF file doesn't rewrite the endings of
/// every line in it. Pure — the caller does the I/O.
fn splice_lines(original: &str, mut edits: Vec<((usize, usize), Vec<String>)>) -> String {
    // An empty original is a file being created (a deletion reverted), which has
    // no convention of its own to keep: give it the conventional trailing newline
    // rather than the ragged last line "preserve what was there" would imply.
    let had_trailing_newline = original.is_empty() || original.ends_with('\n');
    let ending = dominant_line_ending(original);
    // `lines()` splits on `\n` and drops one trailing `\r`, which is exactly the
    // terminator set `dominant_line_ending` counts — an interior lone `\r` stays.
    let mut lines: Vec<String> = original.lines().map(|l| l.to_string()).collect();
    edits.sort_by(|a, b| b.0 .0.cmp(&a.0 .0));
    for ((start, end), content) in edits {
        let start = start.min(lines.len());
        let end = end.clamp(start, lines.len());
        lines.splice(start..end, content);
    }
    let mut out = lines.join(ending);
    if had_trailing_newline && !out.is_empty() {
        out.push_str(ending);
    }
    out
}

/// Read a write-back target. A file the working tree **deleted** still has a span
/// — its `-` lines are ghosts the reviewer may revert — but it is gone from disk,
/// so `NotFound` reads as empty content and the revert recreates it. Every other
/// error stays an error: a permission failure, and in particular a binary file
/// (`InvalidData` from the UTF-8 check), must not be laundered into "it was empty"
/// — that would truncate the file on write.
fn read_source(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// How many of `edits` expected something the file no longer holds.
///
/// The comparison is against `original` split the way [`splice_lines`] splits it,
/// so the line-ending handling can never make a CRLF file look stale. An edit
/// with no expectation is never stale (nothing was claimed), and neither is any
/// edit against a file that is **not on disk**: a missing file reads as empty so
/// that a reviewer can revert a deletion, and there is no content there to
/// overwrite — warning about it would make "revert this deletion" always shout.
fn stale_spans(
    path: &Path,
    original: &str,
    edits: &[((usize, usize), Vec<String>, Option<Vec<String>>)],
) -> usize {
    if !path.exists() {
        return 0;
    }
    let lines: Vec<&str> = original.lines().collect();
    edits
        .iter()
        .filter(|(_, _, expected)| expected.is_some())
        .filter(|((start, end), _, expected)| {
            let start = (*start).min(lines.len());
            let end = (*end).clamp(start, lines.len());
            let current = &lines[start..end];
            let expected = expected.as_ref().expect("filtered");
            current.len() != expected.len()
                || current.iter().zip(expected).any(|(a, b)| *a != b.as_str())
        })
        .count()
}

/// A file's short name for a status line — its base name, or the whole path when
/// it has none.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Apply `(file, span, replacement, expectation)` tuples: group by file, splice
/// all of that file's spans at once, write the ones that actually changed.
/// Returns the file names written and the ones found stale, both in path order.
///
/// Two passes, and that is the point: **every** file's new contents are resolved
/// first, and the writes only start once all of them succeeded. A single-pass
/// read-splice-write walked a `BTreeMap`, so a file that failed to read left the
/// path-order-earlier files already written — a partial save reported as a total
/// failure. Now a failure leaves the working tree untouched. The staleness check
/// rides on the same resolve pass, since that is where the file's current
/// content is in hand.
fn write_spans(edits: Vec<SourceEdit>) -> Result<Applied, String> {
    /// A file's pending replacements: `(0-based [start, end) span, new lines,
    /// the lines the view expected to find there)`.
    type Edits = Vec<((usize, usize), Vec<String>, Option<Vec<String>>)>;
    let mut by_file: std::collections::BTreeMap<PathBuf, Edits> = std::collections::BTreeMap::new();
    for e in edits {
        by_file
            .entry(e.source)
            .or_default()
            .push(((e.start, e.end), e.lines, e.expected));
    }
    // Pass 1 — resolve (and audit). Only the files that actually change carry
    // through; a file whose spans no longer match what was projected is still
    // written, but is reported.
    let mut pending: Vec<(PathBuf, String)> = Vec::new();
    let mut stale: Vec<(String, usize)> = Vec::new();
    for (path, edits) in by_file {
        let original = read_source(&path)?;
        let drifted = stale_spans(&path, &original, &edits);
        if drifted > 0 {
            stale.push((file_label(&path), drifted));
        }
        let updated = splice_lines(
            &original,
            edits.into_iter().map(|(t, c, _)| (t, c)).collect(),
        );
        if updated != original {
            pending.push((path, updated));
        }
    }
    // Pass 2 — write. A resolved file whose content is empty is written as an
    // empty file; nothing here deletes.
    let mut written = Vec::new();
    for (path, updated) in pending {
        // Reverting a deletion can recreate a file inside a directory the working
        // tree no longer has, so the parents come back with it.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
        }
        std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
        written.push(file_label(&path));
    }
    Ok(Applied { written, stale })
}

/// Apply the write-backs `^S` sent, from either editable view. Each names a file,
/// a line range, and the lines to put there — the host's projection worked all of
/// that out from the origins it tracked while the user edited, so there is
/// nothing to infer here.
///
/// This replaces the two write-back paths that used to live in this file. Both
/// recovered intent from edited *text* after the fact: the unified one by
/// LCS-aligning the buffer against what had been projected (it is not a picture
/// of the new file — it interleaves the base side with it), the split one by
/// re-parsing the `@@@` markers to find the blocks between them, so a stray edit
/// to a marker aborted the save. The projection records the intent as it happens
/// instead, which leaves the markers as ordinary chrome.
/// Each edit also says what it expected the file to hold, so an apply that lands
/// on top of an external change is reported (and still written — see
/// [`Applied::stale`]).
pub fn apply_edits(edits: Vec<SourceEdit>) -> Result<Applied, String> {
    write_spans(edits)
}

// ---------------------------------------------------------------------------
// PR mode: resolve a GitHub PR via `gh`, then diff its base against the working
// tree (which must be the PR's head branch, checked out)
// ---------------------------------------------------------------------------

/// Fetch a PR's metadata with `gh pr view` (run inside `dir`). `number` names the
/// PR, or `0` for the current branch's PR (let `gh` resolve it). Returns a
/// friendly error if `gh` is missing, unauthenticated, or the PR is unknown.
pub fn resolve_pr(dir: &Path, number: u64) -> Result<PrMeta, String> {
    // `gh` locates the repo from its working directory (it has no git-style `-C`
    // flag), so run it *inside* `dir` rather than passing the path as an arg.
    let mut args = vec!["pr".to_string(), "view".to_string()];
    if number != 0 {
        args.push(number.to_string());
    }
    args.push("--json".to_string());
    args.push("number,title,author,baseRefName,headRefName,state,url,body,comments".to_string());
    let out = Command::new("gh")
        .current_dir(dir)
        .args(&args)
        .output()
        .map_err(|e| format!("could not run gh (is the GitHub CLI installed?): {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.trim();
        let which = if number == 0 {
            "gh pr view (current branch)".to_string()
        } else {
            format!("gh pr view {number}")
        };
        return Err(format!(
            "{which} failed: {}",
            if msg.is_empty() {
                "unknown PR or not authenticated"
            } else {
                msg
            }
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("could not parse gh output: {e}"))?;
    let mut meta = PrMeta {
        number: v["number"].as_u64().unwrap_or(number),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        author: v["author"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        base_ref: v["baseRefName"].as_str().unwrap_or_default().to_string(),
        head_ref: v["headRefName"].as_str().unwrap_or_default().to_string(),
        state: v["state"].as_str().unwrap_or_default().to_string(),
        url: v["url"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
        conversation: conversation_comments(&v["comments"]),
        inline: Vec::new(),
    };
    // Inline review comments are a best-effort extra: a failure here (an older
    // `gh`, a repo the API call can't resolve) must not cost the user the diff.
    meta.inline = review_comments(dir, meta.number).unwrap_or_default();
    Ok(meta)
}

/// Parse the `comments` array of `gh pr view --json comments` (conversation, i.e.
/// issue-level, comments) into [`Comment`]s — oldest first, as `gh` returns them.
fn conversation_comments(v: &serde_json::Value) -> Vec<Comment> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| Comment {
                    author: c["author"]["login"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    date: iso_date(c["createdAt"].as_str().unwrap_or_default()),
                    body: c["body"].as_str().unwrap_or_default().to_string(),
                    path: None,
                    line: None,
                    left_side: false,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Inline review comments for PR `number` (the pulls-comments REST endpoint via
/// `gh api`), oldest first. Ported from the retired `pr-browser`, which is where
/// these threads used to be readable. `number == 0` (an unresolved PR) yields
/// none; every failure is the caller's to swallow.
fn review_comments(dir: &Path, number: u64) -> Result<Vec<Comment>, String> {
    if number == 0 {
        return Ok(Vec::new());
    }
    let slug = repo_slug(dir)?;
    let endpoint = format!("repos/{slug}/pulls/{number}/comments");
    let out = gh_stdout(dir, &["api", "--paginate", &endpoint])?;
    let mut comments = Vec::new();
    for chunk in split_json_arrays(&out) {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(&chunk)
        {
            for c in arr {
                comments.push(Comment {
                    author: c["user"]["login"].as_str().unwrap_or_default().to_string(),
                    date: iso_date(c["created_at"].as_str().unwrap_or_default()),
                    body: c["body"].as_str().unwrap_or_default().to_string(),
                    path: c["path"].as_str().map(str::to_string),
                    line: c["line"]
                        .as_u64()
                        .or_else(|| c["original_line"].as_u64())
                        .map(|n| n as usize),
                    left_side: c["side"].as_str().unwrap_or("RIGHT") == "LEFT",
                });
            }
        }
    }
    Ok(comments)
}

/// `gh repo view --json nameWithOwner` → `owner/repo`.
fn repo_slug(dir: &Path) -> Result<String, String> {
    let out = gh_stdout(
        dir,
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ],
    )?;
    let slug = out.trim().to_string();
    if slug.is_empty() {
        Err("could not resolve the repository (owner/name)".to_string())
    } else {
        Ok(slug)
    }
}

/// Run `gh` rooted at `dir` (via the child's cwd — `gh` has no `-C` flag),
/// returning stdout or a readable error.
fn gh_stdout(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("could not run gh: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.trim();
        return Err(if msg.is_empty() {
            format!("gh {} failed", args.first().copied().unwrap_or(""))
        } else {
            msg.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The `YYYY-MM-DD` date portion of an ISO-8601 timestamp.
fn iso_date(iso: &str) -> String {
    iso.chars().take(10).collect()
}

/// Split a `gh api --paginate` stream that may concatenate several JSON arrays
/// (`][` boundaries) into individual array strings.
fn split_json_arrays(s: &str) -> Vec<String> {
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    if !s.contains("][") {
        return vec![s.to_string()];
    }
    s.replace("][", "]\u{1}[")
        .split('\u{1}')
        .map(str::to_string)
        .collect()
}

/// The base ref to diff a PR against: the merge-base of its base branch and the
/// working tree's `HEAD`, so the projection matches GitHub's "Files changed"
/// (three-dot) view. The PR's head branch must be the current checkout — the
/// editable after side writes to the working tree, so reviewing a PR whose branch
/// isn't checked out would splice edits into the wrong files. When it isn't, this
/// returns the `gh pr checkout` hint instead of a base ref.
pub fn pr_diff_base(dir: &Path, meta: &PrMeta) -> Result<String, String> {
    let current = run_git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let current = current.trim();
    if current != meta.head_ref {
        return Err(format!(
            "PR #{} is branch `{}`, but `{}` is checked out — run `gh pr checkout {}` first",
            meta.number, meta.head_ref, current, meta.number
        ));
    }
    // Resolve the base branch locally, preferring the remote-tracking ref so the
    // merge-base reflects the PR's actual fork point even when local `main` lags.
    let base_ref = [format!("origin/{}", meta.base_ref), meta.base_ref.clone()]
        .into_iter()
        .find(|r| run_git(dir, &["rev-parse", "--verify", "--quiet", r]).is_ok())
        .ok_or_else(|| format!("PR base branch `{}` not found locally", meta.base_ref))?;
    let mb = run_git(dir, &["merge-base", &base_ref, "HEAD"])?;
    Ok(mb.trim().to_string())
}

#[cfg(test)]
mod tests;
