//! `garden-diff` — a **panel-mode** GPP app for reviewing a git diff as an
//! editable before/after split, the unified successor to the read-only `git-diff`
//! viewer, the `pr-browser`, and the in-app `:Review`/`:Review2` projection.
//!
//! Launched over local refs (`garden diff [base]` → base branch vs the working
//! tree). The host runs the colocated `garden_diff.ptl` drawer in-process and
//! answers its `query("doc")` with the projected before/after documents; editing
//! happens in the drawer's `edit_view` regions (real vim, host-side), each of
//! which carries an **editable projection** — so `^S` sends the *edits* the host
//! resolved from the line origins it tracked (`mutate("apply", …)`), and this
//! process only has to splice them into the working-tree files
//! (`diff_core::apply_edits`).

mod diff_core;
mod file_outline;

use std::path::PathBuf;

use petal_query::gpp::{self, PanelUi};
use petal_query::{Provider, Reply};
use serde_json::{json, Value};

use diff_core::Doc;

/// The `garden_diff.ptl` drawer, embedded and pushed to the host.
const GARDEN_DIFF_VIEW: &str = include_str!("garden_diff.ptl");

/// What to diff: a plain local diff against a base ref (default upstream/`main`),
/// or a GitHub PR (resolved via `gh`, then diffed against its merge-base — the
/// PR's head branch must be checked out, see [`diff_core::pr_diff_base`]). A PR
/// number of `0` means "the PR for the current branch" (let `gh` resolve it).
enum Spec {
    Local(String),
    /// `explicit` records whether the user *named* the PR. It decides what an
    /// unresolvable PR means: the bare `garden pr` form is a question ("what
    /// have I got pending?") that degrades to the local diff, while
    /// `garden pr 123` names one thing, and reviewing anything else instead of
    /// it would be answering a question nobody asked.
    Pr {
        number: u64,
        explicit: bool,
    },
}

/// Per-run state: what to diff, and the lazily-built projection (rebuilt after a
/// save so the panes reflect the new working tree).
struct State {
    dir: PathBuf,
    spec: Spec,
    /// Which view the drawer opens in — `"stat"` when launched with `--stat`
    /// (`garden diff --stat` / `:Diff --stat`), else `"unified"`. The drawer applies
    /// it once, on the first loaded doc; the pills switch freely after that.
    initial_mode: &'static str,
    /// The projection, built lazily so a bad ref / PR surfaces as a soft error the
    /// drawer renders (not a handshake failure). Reset to `None` after a save so
    /// the next query re-reads the new working tree.
    doc: Option<Result<Doc, String>>,
    /// A non-fatal banner the drawer shows above the diff — set when PR mode falls
    /// back to a plain local diff because no PR could be resolved (see
    /// [`build_doc`]). Empty otherwise.
    notice: String,
}

/// What the drawer asked the `doc` query for — the review's **scope**, parsed
/// from the query argument so the host caches each scope separately and
/// switching back to one already seen is free.
///
/// | argument | scope |
/// |---|---|
/// | `""` | the whole review: the base against the working tree (the default) |
/// | `"commit:<sha>"` | that commit alone — `<sha>^` against `<sha>` |
/// | `"since:<sha>"` | that commit's parent against the working tree |
///
/// Only the first is editable; see [`diff_core::Doc::editable`].
enum Scope {
    Whole,
    Commit(String),
    Since(String),
}

impl Scope {
    /// Parse a `doc` query argument. Anything unrecognised — an old drawer, a
    /// typo — is the whole review, which is the safe answer: it shows more than
    /// was asked for rather than silently showing the wrong commit.
    fn parse(arg: &str) -> Scope {
        match arg.split_once(':') {
            Some(("commit", sha)) if !sha.is_empty() => Scope::Commit(sha.to_string()),
            Some(("since", sha)) if !sha.is_empty() => Scope::Since(sha.to_string()),
            _ => Scope::Whole,
        }
    }
}

/// What the review is *of*, resolved once from the [`Spec`]: the rev everything
/// is diffed against, the name that rev goes by in the titles, and — in PR mode
/// — the pull request's metadata.
///
/// `base` and `label` differ in PR mode, where the base is an opaque merge-base
/// SHA but the label is the base branch name (`main`), which reads far better.
struct Review {
    base: String,
    label: String,
    pr: Option<diff_core::PrMeta>,
}

/// Resolve the spec into a [`Review`]. Shared by the `doc` and `commits`
/// queries, which have to agree on the range or the commit list would describe
/// a different review from the diff beside it.
///
/// In PR mode, a PR that can't be resolved (no PR for this branch, no `gh`, not
/// authenticated) is *not* an error **when no number was given**: `garden pr`
/// asks "what have I got pending?", so it degrades to the plain local diff
/// against the default base — the working-tree changes, staged and unstaged —
/// with `state.notice` explaining why. A PR the user *named* is different: it
/// asks for one specific thing, so an unresolvable `garden pr 123` is an error
/// carrying `gh`'s reason rather than a silent review of unrelated changes.
/// A resolved PR whose branch isn't checked out is always an error, since that
/// one is actionable.
fn resolve_review(state: &mut State) -> Result<Review, String> {
    match &state.spec {
        Spec::Local(arg) => {
            let base = diff_core::resolve_base(&state.dir, arg)?;
            Ok(Review {
                label: base.clone(),
                base,
                pr: None,
            })
        }
        Spec::Pr { number, explicit } => {
            let (number, explicit) = (*number, *explicit);
            let meta = match diff_core::resolve_pr(&state.dir, number) {
                Ok(meta) => meta,
                Err(why) if explicit => {
                    return Err(format!(
                        "could not review PR #{number}: {}",
                        gh_reason(&why)
                    ))
                }
                Err(why) => return local_fallback(state, &why),
            };
            let base = diff_core::pr_diff_base(&state.dir, &meta)?;
            Ok(Review {
                base,
                label: meta.base_ref.clone(),
                pr: Some(meta),
            })
        }
    }
}

/// Build the diff the drawer asked for: the [`Review`]'s range, narrowed by
/// `scope`.
///
/// The whole review is the editable working-tree diff this app has always
/// shown. A **commit** scope is read-only — `<sha>^` against `<sha>` describes
/// files as they were, not as they are (see [`diff_core::Doc::editable`]) — and
/// carries no PR comments, since their anchors are the PR's diff and not this
/// one. A **since** scope still ends at the working tree, so it stays editable:
/// it is the same kind of diff as the whole review, just from a nearer base.
fn build_doc(state: &mut State, scope: &Scope) -> Result<Doc, String> {
    let review = resolve_review(state)?;
    let mut doc = match scope {
        Scope::Whole => match &review.pr {
            Some(meta) => {
                diff_core::build_reviewed(&state.dir, &review.base, &review.label, &meta.inline)?
            }
            None => diff_core::build(&state.dir, &review.base, &review.label)?,
        },
        Scope::Commit(sha) => {
            let parent = parent_of(&state.dir, sha)?;
            diff_core::build_range(
                &state.dir,
                &parent,
                sha,
                &format!("commit {}", short_sha(sha)),
            )?
        }
        Scope::Since(sha) => {
            let parent = parent_of(&state.dir, sha)?;
            diff_core::build(
                &state.dir,
                &parent,
                &format!("the parent of {}", short_sha(sha)),
            )?
        }
    };
    // The PR block is about the review, not about the slice of it on screen, so
    // it stays attached however the diff is scoped.
    doc.pr = review.pr;
    Ok(doc)
}

/// The commit a scoped diff starts from: `<sha>^`, or git's empty-tree hash
/// when `sha` is a root commit and has no parent. Without the fallback, "show
/// me just this commit" would fail outright on the first commit in a
/// repository — the one case where the whole commit *is* the diff.
fn parent_of(dir: &std::path::Path, sha: &str) -> Result<String, String> {
    let parent = format!("{sha}^");
    match diff_core::run_git(dir, &["rev-parse", "--verify", "--quiet", &parent]) {
        Ok(_) => Ok(parent),
        Err(_) => Ok(EMPTY_TREE.to_string()),
    }
}

/// Git's hash of the empty tree — the parent every root commit is diffed
/// against. Constant across every git repository ever made.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// A full hash shortened for a title. Not `git rev-parse --short`: this is a
/// label, so a fixed width beats a round trip for something nobody diffs on.
fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// The PR-mode fallback: review the default base (upstream / `main` / `master`)
/// against the working tree, and record why the PR lookup was abandoned so the
/// drawer can say so. `why` is the raw [`diff_core::resolve_pr`] failure.
fn local_fallback(state: &mut State, why: &str) -> Result<Review, String> {
    let base = diff_core::resolve_base(&state.dir, "")?;
    // `gh`'s own "no pull requests found …" needs no parenthetical restatement;
    // anything else (no `gh`, not authenticated) does, so it is quoted.
    let reason = gh_reason(why);
    state.notice = if reason.starts_with("no pull request") {
        format!("no pull request found — showing pending changes vs {base}")
    } else {
        format!("no pull request found ({reason}) — showing pending changes vs {base}")
    };
    Ok(Review {
        label: base.clone(),
        base,
        pr: None,
    })
}

/// The readable half of a [`diff_core::resolve_pr`] error: its first line, minus
/// the `gh pr view … failed:` framing the drawer's own wording already covers.
fn gh_reason(why: &str) -> &str {
    let first = why.lines().next().unwrap_or(why);
    match first.split_once("failed: ") {
        Some((_, rest)) => rest.trim(),
        None => first.trim(),
    }
}

/// Build the projection on first use (see [`build_doc`]); cached until a save
/// invalidates it.
fn ensure<'a>(state: &'a mut State, scope: &Scope) -> &'a Result<Doc, String> {
    if state.doc.is_none() {
        let built = build_doc(state, scope);
        state.doc = Some(built);
    }
    state.doc.as_ref().unwrap()
}

/// The `doc` answer: the header, file list, and all three projected views. The
/// read-only before side ships text + per-line styles for its `text_view`; the
/// two editable sides ship text + a projection, whose own style track the host
/// paints (it follows the lines through edits, which a positional list cannot).
/// `pr` is null for a plain local diff, else the PR metadata block.
fn doc_json(doc: &Doc, initial_mode: &str, notice: &str) -> Value {
    // Always an object (never JSON null) so the drawer reads fields without
    // nil-guards; `present` distinguishes PR mode from a plain local diff.
    // `discussion` is the description plus the conversation comments, prebuilt
    // as one scrollable block so the drawer only has to show text.
    let pr = match &doc.pr {
        Some(m) => json!({
            "present": true,
            "number": m.number,
            "title": m.title,
            "author": m.author,
            "base_ref": m.base_ref,
            "head_ref": m.head_ref,
            // `state` is a Petal keyword, so the drawer sees it as `status`.
            "status": m.state,
            "url": m.url,
            "body": m.body,
            "discussion": discussion_text(m),
            "comments": m.conversation.len() + m.inline.len(),
        }),
        None => json!({ "present": false, "number": 0, "title": "", "author": "",
                        "base_ref": "", "head_ref": "", "status": "", "url": "",
                        "body": "", "discussion": "", "comments": 0 }),
    };
    json!({
        // Null on success; the drawer reads it null-safely.
        "error": Value::Null,
        // Whether this diff can be edited and saved back — false for a
        // commit-scoped one, which describes files as they were rather than as
        // they are. The drawer renders those with read-only `text_view`s.
        "editable": doc.editable,
        // Whether the `+`/`-` markers are already in the line text (a read-only
        // scoped view) or belong in the region's gutter (an editable one).
        "markers_in_text": doc.markers_in_text,
        // Non-fatal banner (empty when there is nothing to say) — e.g. PR mode
        // that fell back to the local diff because no PR was found.
        "notice": notice,
        "base": doc.base,
        "initial_mode": initial_mode,
        "files": doc
            .files
            .iter()
            .map(|f| json!({
                "path": f.path,
                "added": f.added,
                "removed": f.removed,
                "binary": f.binary,
            }))
            .collect::<Vec<_>>(),
        // The same file set, grouped into the stat view's hierarchical rows: the
        // drawer draws these verbatim, one per line, indented by `depth`.
        "outline": file_outline::build(&doc.files)
            .iter()
            .map(|r| json!({
                "dir": r.kind == file_outline::RowKind::Dir,
                "depth": r.depth,
                "label": r.label,
                "added": r.added,
                "removed": r.removed,
                "binary": r.binary,
                // Where clicking this row should scroll each view. A dir row
                // carries its first file's lines, so clicking a heading jumps to
                // the top of that subtree.
                "line_before": r.line_before,
                "line_after": r.line_after,
                "line_unified": r.line_unified,
            }))
            .collect::<Vec<_>>(),
        "before": { "text": doc.before.text, "styles": doc.before.styles },
        // Each editable view carries its own provenance, in the flat shape
        // `edit_view_projection` takes. The after column is undecorated — it is
        // the new file, line for line — so its decor has no prefixes and a line
        // typed into it is taken literally. The unified stream carries the
        // `+`/`-`/space markers in its **gutter**: the host draws them beside
        // the text from the line origins, and the buffer holds the files' own
        // text, so nothing typed, joined, selected or searched in that view has
        // to step around a marker.
        "after": {
            "text": doc.after.text,
            // Positional styles as well as the projection's own track: a
            // read-only (commit-scoped) doc is rendered as a plain `text_view`,
            // which has no projection to carry them.
            "styles": doc.after.styles,
            "projection": projection_json(
                &doc.sources, &doc.after_spans, &doc.after,
                json!({
                    "same": "", "added": "", "removed": "",
                    "same_style": "", "added_style": "added", "removed_style": "removed",
                    "diff_markers": false,
                }),
            ),
        },
        "unified": {
            "text": doc.unified.text,
            "styles": doc.unified.styles,
            "projection": projection_json(
                &doc.sources, &doc.unified_spans, &doc.unified,
                json!({
                    "same": " ", "added": "+", "removed": "-",
                    "same_style": "", "added_style": "added", "removed_style": "removed",
                    // The markers are display, drawn in the gutter — so nothing
                    // is stripped on fold and a typed line is taken literally,
                    // which `diff_markers` would otherwise override.
                    "diff_markers": false,
                    "gutter": true,
                }),
            ),
        },
        "pr": pr,
    })
}

/// The `commits` answer: the review's commits, newest first, in the flat shape
/// the drawer's list draws — one record per row, nothing to compute script-side.
fn commits_json(list: &[diff_core::CommitInfo]) -> Value {
    json!({
        "commits": list
            .iter()
            .map(|c| json!({
                "sha": c.sha,
                "short": c.short,
                "subject": c.subject,
                "author": c.author,
                "date": c.date,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One editable view's projection, in the flat parallel-array form
/// `edit_view_projection(id, spec)` takes: where each line came from
/// (`kinds` / `line_spans` / `styles`, all parallel to the text's lines), which
/// file range each span rewrites, and how the lines are decorated. This is what
/// makes a view editable as a projection — the host folds the edits back itself,
/// and `^S` sends the result rather than the text.
fn projection_json(
    sources: &[String],
    spans: &[diff_core::SpanSpec],
    projected: &diff_core::Projected,
    decor: Value,
) -> Value {
    json!({
        "sources": sources,
        "span_source": spans.iter().map(|s| s.source).collect::<Vec<_>>(),
        "span_start": spans.iter().map(|s| s.start).collect::<Vec<_>>(),
        "span_end": spans.iter().map(|s| s.end).collect::<Vec<_>>(),
        "span_group": spans.iter().map(|s| s.group).collect::<Vec<_>>(),
        "kinds": projected.kinds,
        "line_spans": projected.line_spans,
        "styles": projected.styles,
        "decor": decor,
    })
}

/// The PR discussion block the drawer scrolls under the header: the description
/// body followed by the conversation comments (oldest first). Inline review
/// comments are not here — they are threaded into the unified diff itself.
fn discussion_text(meta: &diff_core::PrMeta) -> String {
    let mut out = meta.body.replace('\r', "");
    for c in &meta.conversation {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "─── @{} · {} ───\n{}",
            c.author,
            c.date,
            c.body.replace('\r', "")
        ));
    }
    if out.trim().is_empty() {
        "(no description)".to_string()
    } else {
        out
    }
}

/// Read the `edits` payload `^S` sends — the list of write-backs the host's
/// projection resolved (`edit_view_edits(id)`). Anything malformed is an error
/// rather than a partial write: this arrives from the host, not from a user, so
/// a bad shape means a protocol mismatch worth reporting.
fn parse_edits(arg: &Value) -> Result<Vec<diff_core::SourceEdit>, String> {
    let items = arg["edits"]
        .as_array()
        .ok_or_else(|| "save payload has no `edits` list".to_string())?;
    items
        .iter()
        .map(|e| {
            let source = e["source"]
                .as_str()
                .ok_or_else(|| "an edit has no source".to_string())?;
            let start = e["start"].as_u64().unwrap_or(0) as usize;
            let end = e["end"].as_u64().unwrap_or(0) as usize;
            let lines = e["lines"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|l| l.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            // `expected` is the host's record of what this span held when the
            // view loaded. Absent/null means "no expectation" — an older host,
            // or a projection with no captured baseline — and the apply then
            // checks nothing rather than inventing an expectation.
            let expected = e["expected"].as_array().map(|a| {
                a.iter()
                    .map(|l| l.as_str().unwrap_or_default().to_string())
                    .collect()
            });
            Ok(diff_core::SourceEdit {
                source: PathBuf::from(source),
                start,
                end,
                lines,
                expected,
            })
        })
        .collect()
}

/// The tail of a save: report what was written, and drop the lazy projection
/// cache so the next `doc` query re-reads the new working tree.
///
/// A save that found a file changed on disk since the view loaded is still a
/// save — the edits are written — but the reviewer has to be told, and told
/// *first*: they may have just overwritten someone else's (or their own
/// out-of-band) change, and the only remedy is to reload and look. See
/// [`stale_warning`].
fn finish_save(state: &mut State, result: Result<diff_core::Applied, String>) -> Reply {
    match result {
        Ok(a) => {
            if !a.written.is_empty() {
                state.doc = None;
            }
            Reply::json(save_status(&a))
        }
        Err(e) => Reply::error(e),
    }
}

/// What a save says. Nothing written and nothing stale is the quiet case; a
/// stale one leads with the warning, since the success text is not the news.
fn save_status(applied: &diff_core::Applied) -> String {
    let n = applied.written.len();
    let wrote = format!("wrote {n} file{}", plural(n));
    match stale_warning(&applied.stale) {
        Some(warning) => format!("{warning} Wrote {n} file{}. Press ⟳ to reload.", plural(n)),
        None if n == 0 => "no changes to write".to_string(),
        None => wrote,
    }
}

/// The loud half of a stale save: which files had drifted, and how many of their
/// hunks were written over regardless. `None` when nothing was stale, which is
/// the overwhelmingly common case and must read exactly as it always did.
fn stale_warning(stale: &[(String, usize)]) -> Option<String> {
    if stale.is_empty() {
        return None;
    }
    let files: Vec<&str> = stale.iter().map(|(name, _)| name.as_str()).collect();
    let hunks: usize = stale.iter().map(|(_, n)| n).sum();
    Some(format!(
        "WARNING: {} changed on disk since this view loaded; {hunks} hunk{} overwritten.",
        files.join(", "),
        plural(hunks)
    ))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn run() -> std::io::Result<()> {
    let provider = Provider::new(|init| {
        let (dir, spec, initial_mode) = parse_args(&init.args, &init.cwd);
        State {
            dir,
            spec,
            initial_mode,
            doc: None,
            notice: String::new(),
        }
    })
    // The projected before/after documents. Cached fresh-forever (the host caches
    // it and drives editing/scroll locally); a save drops the cache below and the
    // drawer `invalidate`s it, so the panes refresh to the new working tree.
    .query("doc", |state: &mut State, ctx| {
        let initial_mode = state.initial_mode;
        // The query argument is the review's *scope* — empty for the whole
        // thing, `commit:<sha>` / `since:<sha>` for a slice of it (see
        // [`Scope`]). The host caches per argument, so switching back to a
        // scope already looked at costs nothing.
        let scope = Scope::parse(ctx.arg);
        // Rebuild every time the host asks. The host answers `doc` from its own
        // query cache and only re-requests after an `invalidate`, so caching it
        // here as well meant a refresh — the ⟳ pill, the staleness probe, a save
        // — was served the stale copy this process built on startup and could
        // never see the working tree change.
        state.doc = None;
        state.notice.clear();
        ensure(state, &scope);
        let notice = state.notice.clone();
        match state.doc.as_ref().expect("ensure populates doc") {
            Ok(doc) => Reply::json(doc_json(doc, initial_mode, &notice)),
            Err(e) => Reply::error(e.clone()),
        }
    })
    // The commits the review covers, newest first — what the drawer's COMMITS
    // column lists and scopes the diff by. Answered from the same [`Review`]
    // the `doc` query resolves, so the list and the diff always describe the
    // same range.
    //
    // A review with no commits of its own (uncommitted work against the base)
    // is an empty list, not an error: the drawer says "no commits yet" and the
    // whole-review diff beside it is still the answer to what changed.
    .query("commits", |state: &mut State, _ctx| {
        let base = match resolve_review(state) {
            Ok(review) => review.base,
            Err(e) => return Reply::error(e),
        };
        match diff_core::commits(&state.dir, &base) {
            Ok(list) => Reply::json(commits_json(&list)),
            Err(e) => Reply::error(e),
        }
    })
    // The write-back, for either editable view. The drawer sends the *edits* the
    // host's projection resolved — `[{source, start, end, lines}]` — not the
    // region's text, so nothing here has to work out what the user meant by an
    // edit (see [`diff_core::apply_edits`]).
    .on_mutation("apply", |state: &mut State, ctx| {
        if state.doc.is_none() {
            return Reply::error("no diff loaded yet".to_string());
        }
        let edits = match parse_edits(&ctx.arg) {
            Ok(edits) => edits,
            Err(e) => return Reply::error(e),
        };
        finish_save(state, diff_core::apply_edits(edits))
    });
    gpp::serve(provider, PanelUi::new("garden-diff", GARDEN_DIFF_VIEW))
}

fn main() {
    if let Err(err) = run() {
        eprintln!("garden-diff: {err}");
        std::process::exit(1);
    }
}

/// Parse the client args (positional `[dir, spec]` plus the optional `--pr` /
/// `--stat` flags, the launcher's contract) into (repo dir, [`Spec`], initial view
/// mode). A missing dir falls back to `cwd`. PR mode is selected either explicitly
/// by `--pr` (the `:PR` command path, where the second positional is an optional
/// number and an absent one means the current branch's PR) or implicitly by an
/// all-digit second positional (`garden diff 123`). Otherwise the second positional
/// is a **base ref**, which may itself contain `/` (`origin/main`); a missing one
/// means an empty local base (resolved to upstream/main/master at build time).
/// `--stat` only picks the view the drawer opens in — the diff itself is the same.
fn parse_args(args: &[String], cwd: &str) -> (PathBuf, Spec, &'static str) {
    let pr_flag = args.iter().any(|a| a == "--pr");
    let initial_mode = if args.iter().any(|a| a == "--stat") {
        "stat"
    } else {
        "unified"
    };
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let dir = positionals
        .first()
        .map(|s| PathBuf::from(s.as_str()))
        .unwrap_or_else(|| PathBuf::from(cwd));
    let second = positionals.get(1);
    let spec = if pr_flag {
        // `--pr [number]` — an absent/blank number resolves the current branch's PR.
        match second.and_then(|s| s.parse().ok()) {
            Some(number) => Spec::Pr {
                number,
                explicit: true,
            },
            None => Spec::Pr {
                number: 0,
                explicit: false,
            },
        }
    } else {
        match second {
            Some(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => Spec::Pr {
                number: s.parse().unwrap_or(0),
                explicit: true,
            },
            Some(s) => Spec::Local(s.to_string()),
            None => Spec::Local(String::new()),
        }
    };
    (dir, spec, initial_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use diff_core::{Comment, PrMeta};

    #[test]
    fn parse_args_splits_dir_and_spec() {
        // The launcher's contract: [dir, spec] positionally; a ref base may contain `/`.
        let (dir, spec, mode) =
            parse_args(&["/repo".to_string(), "origin/main".to_string()], "/cwd");
        assert_eq!(dir, PathBuf::from("/repo"));
        assert!(matches!(spec, Spec::Local(ref b) if b == "origin/main"));
        assert_eq!(mode, "unified");
        // An all-digit second positional is a PR number, not a ref.
        let (_, spec, _) = parse_args(&["/repo".to_string(), "123".to_string()], "/cwd");
        assert!(matches!(
            spec,
            Spec::Pr {
                number: 123,
                explicit: true
            }
        ));
        // `--pr` selects PR mode explicitly; a number may follow, else 0 (current).
        let (_, spec, _) = parse_args(&["/repo".to_string(), "--pr".to_string()], "/cwd");
        assert!(matches!(
            spec,
            Spec::Pr {
                number: 0,
                explicit: false
            }
        ));
        let (_, spec, _) = parse_args(
            &["/repo".to_string(), "--pr".to_string(), "7".to_string()],
            "/cwd",
        );
        assert!(matches!(
            spec,
            Spec::Pr {
                number: 7,
                explicit: true
            }
        ));
        // Dir only → empty local base (resolved later); no args → cwd.
        let (dir, spec, _) = parse_args(&["/repo".to_string()], "/cwd");
        assert_eq!(dir, PathBuf::from("/repo"));
        assert!(matches!(spec, Spec::Local(ref b) if b.is_empty()));
        assert_eq!(parse_args(&[], "/cwd").0, PathBuf::from("/cwd"));
    }

    /// `--stat` (from `garden diff --stat` / `:Diff --stat`) only chooses the
    /// opening view; it is not a positional and must not be read as a base ref.
    #[test]
    fn stat_flag_selects_the_stat_view() {
        let (dir, spec, mode) = parse_args(&["/repo".to_string(), "--stat".to_string()], "/cwd");
        assert_eq!(dir, PathBuf::from("/repo"));
        assert!(matches!(spec, Spec::Local(ref b) if b.is_empty()));
        assert_eq!(mode, "stat");
        // With a base ref alongside it, both survive.
        let (_, spec, mode) = parse_args(
            &[
                "/repo".to_string(),
                "HEAD~2".to_string(),
                "--stat".to_string(),
            ],
            "/cwd",
        );
        assert!(matches!(spec, Spec::Local(ref b) if b == "HEAD~2"));
        assert_eq!(mode, "stat");
    }

    /// A PR number the user typed is *explicit*; the bare `garden pr` form is
    /// not. Only the second may quietly degrade to the local diff.
    #[test]
    fn parse_args_marks_an_explicit_pr_number() {
        let explicit = |args: &[&str]| {
            let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            match parse_args(&owned, "/cwd").1 {
                Spec::Pr { explicit, .. } => explicit,
                Spec::Local(_) => panic!("expected PR mode for {args:?}"),
            }
        };
        assert!(explicit(&["/repo", "--pr", "123"]));
        assert!(explicit(&["/repo", "123"]));
        assert!(!explicit(&["/repo", "--pr"]));
    }

    /// A named PR that cannot be resolved is an error carrying the reason — not
    /// a silent review of something else. The no-arg form still falls back to
    /// the pending working-tree changes, with its notice.
    #[test]
    fn an_explicit_pr_never_falls_back_to_the_local_diff() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@e.com"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.path().join("f.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        let state = |spec| State {
            dir: repo.path().to_path_buf(),
            spec,
            initial_mode: "unified",
            doc: None,
            notice: String::new(),
        };
        // No remote, so no PR can resolve however `gh` is configured.
        let mut explicit = state(Spec::Pr {
            number: 4242,
            explicit: true,
        });
        assert!(build_doc(&mut explicit, &Scope::Whole).is_err());
        assert!(explicit.notice.is_empty());

        let mut implicit = state(Spec::Pr {
            number: 0,
            explicit: false,
        });
        assert!(build_doc(&mut implicit, &Scope::Whole).is_ok());
        assert!(implicit.notice.starts_with("no pull request found"));
    }

    /// The discussion block is description + conversation comments, and never
    /// empty (the drawer shows it in a fixed-height region).
    #[test]
    fn discussion_text_appends_conversation() {
        let meta = PrMeta {
            body: "does the thing".to_string(),
            conversation: vec![Comment {
                author: "octocat".to_string(),
                date: "2026-07-20".to_string(),
                body: "looks good".to_string(),
                ..Comment::default()
            }],
            ..PrMeta::default()
        };
        let text = discussion_text(&meta);
        assert!(text.starts_with("does the thing"));
        assert!(text.contains("@octocat · 2026-07-20"));
        assert!(text.contains("looks good"));
        assert_eq!(discussion_text(&PrMeta::default()), "(no description)");
    }

    /// A save that overwrote a file which had changed on disk says so *before*
    /// the success text, names the files, and counts the hunks — the reviewer's
    /// only clue that something of theirs may be gone. A clean save reads exactly
    /// as it always did.
    #[test]
    fn a_stale_save_leads_with_the_warning() {
        assert_eq!(stale_warning(&[]), None);
        assert_eq!(
            stale_warning(&[("a.txt".to_string(), 1)]).unwrap(),
            "WARNING: a.txt changed on disk since this view loaded; 1 hunk overwritten."
        );
        assert_eq!(
            stale_warning(&[("a.txt".to_string(), 2), ("b.txt".to_string(), 1)]).unwrap(),
            "WARNING: a.txt, b.txt changed on disk since this view loaded; 3 hunks overwritten."
        );

        assert_eq!(
            save_status(&diff_core::Applied {
                written: vec!["a.txt".into(), "b.txt".into()],
                stale: vec![("a.txt".to_string(), 1)],
            }),
            "WARNING: a.txt changed on disk since this view loaded; 1 hunk overwritten. \
             Wrote 2 files. Press ⟳ to reload."
        );
        assert_eq!(
            save_status(&diff_core::Applied {
                written: vec!["a.txt".into()],
                stale: vec![],
            }),
            "wrote 1 file"
        );
        // The clean, unedited `^S`: with dirty-span tracking there is nothing to
        // write at all, which is the whole point of the fix.
        assert_eq!(
            save_status(&diff_core::Applied::default()),
            "no changes to write"
        );

        // A save that wrote something drops the lazy doc cache so the next query
        // re-reads the working tree; one that wrote nothing leaves it alone.
        let mut state = State {
            dir: PathBuf::from("/repo"),
            spec: Spec::Local(String::new()),
            initial_mode: "unified",
            doc: Some(Err("built".to_string())),
            notice: String::new(),
        };
        finish_save(&mut state, Ok(diff_core::Applied::default()));
        assert!(state.doc.is_some());
        finish_save(
            &mut state,
            Ok(diff_core::Applied {
                written: vec!["a.txt".into()],
                stale: vec![],
            }),
        );
        assert!(state.doc.is_none(), "a save drops the projection cache");
    }

    /// The host's `expected` track survives the wire: a null (or absent) one is
    /// "no expectation", a list is the lines the span held when the view loaded.
    #[test]
    fn parse_edits_reads_the_expected_lines() {
        let edits = parse_edits(&json!({
            "edits": [
                { "source": "a.txt", "start": 0, "end": 2,
                  "lines": ["x"], "expected": ["one", "two"] },
                { "source": "b.txt", "start": 1, "end": 1,
                  "lines": [], "expected": Value::Null },
                { "source": "c.txt", "start": 0, "end": 0, "lines": [] },
            ]
        }))
        .unwrap();
        assert_eq!(
            edits[0].expected.as_deref(),
            Some(["one".to_string(), "two".to_string()].as_slice())
        );
        assert_eq!(edits[1].expected, None);
        assert_eq!(edits[2].expected, None);
        assert_eq!(edits[0].source, PathBuf::from("a.txt"));
        assert_eq!((edits[0].start, edits[0].end), (0, 2));
    }

    /// The no-PR notice quotes only `gh`'s reason, not the command framing, and
    /// keeps multi-line stderr to its first line.
    #[test]
    fn gh_reason_strips_the_command_framing() {
        assert_eq!(
            gh_reason(
                "gh pr view (current branch) failed: no pull requests found for branch \"main\"\nusage: …"
            ),
            "no pull requests found for branch \"main\""
        );
        assert_eq!(
            gh_reason("could not run gh (is the GitHub CLI installed?): not found"),
            "could not run gh (is the GitHub CLI installed?): not found"
        );
    }
}
