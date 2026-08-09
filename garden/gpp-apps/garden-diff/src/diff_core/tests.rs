use super::*;
use std::process::Command;

/// A scratch repo on `main` with one committed file, then a working-tree change:
/// line 2 edited, line 3 removed, a line appended.
fn temp_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@e.com"]);
    run(&["config", "user.name", "T"]);
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "init"]);
    std::fs::write(dir.path().join("f.txt"), "one\nTWO\nfour\n").unwrap();
    dir
}

/// Run `git -C <repo> …`, asserting it succeeded.
fn git(repo: &tempfile::TempDir, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A binary file carries no hunks, so its section used to be a bare `@@@ file:`
/// marker butted straight against the next file's — a file that looked empty for
/// no stated reason. Every projection now says why, on a chrome line: it is not
/// content, so it can never be edited into a source or inherit a span.
#[test]
fn binary_files_carry_an_explanatory_chrome_line() {
    let repo = temp_repo();
    std::fs::write(repo.path().join("bin.dat"), [0x00u8, 0x01, 0x02, 0xff]).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "bin"]);
    std::fs::write(repo.path().join("bin.dat"), [0xffu8, 0xfe, 0x00, 0x01]).unwrap();

    let doc = build(repo.path(), "main", "main").unwrap();
    let bin = doc
        .files
        .iter()
        .find(|f| f.path == "bin.dat")
        .expect("binary file listed");
    assert!(bin.binary);

    // Every projection: the marker, then the explanation, on the line after it.
    for (proj, marker) in [
        (&doc.unified, bin.line_unified),
        (&doc.after, bin.line_after),
        (&doc.before, bin.line_before),
    ] {
        let lines: Vec<&str> = proj.text.lines().collect();
        assert_eq!(lines[marker], "@@@ file: bin.dat");
        assert!(
            lines[marker + 1].contains("binary file"),
            "expected an explanation under the marker, got {:?}",
            lines[marker + 1]
        );
    }
    // …and in the editable projections it is chrome carrying no span, so no edit
    // to it can ever reach a file.
    for proj in [&doc.unified, &doc.after] {
        let kinds: Vec<char> = proj.kinds.chars().collect();
        let at = proj
            .text
            .lines()
            .position(|l| l.contains("binary file"))
            .unwrap();
        assert_eq!(kinds[at], 'c');
        assert_eq!(proj.line_spans[at], -1);
        assert_eq!(kinds.len(), proj.text.lines().count());
        assert_eq!(proj.line_spans.len(), kinds.len());
    }
    // The binary file contributes no editable span at all.
    let bin_source = doc
        .sources
        .iter()
        .position(|s| s.ends_with("bin.dat"))
        .unwrap();
    assert!(doc
        .unified_spans
        .iter()
        .chain(doc.after_spans.iter())
        .all(|s| s.source != bin_source));
}

/// Outside a repository the real problem is the directory, not the ref — saying
/// `unknown base ref: main` sent the user hunting for a branch that was never
/// the issue.
#[test]
fn resolve_base_names_a_non_repository() {
    let plain = tempfile::tempdir().unwrap();
    let err = resolve_base(plain.path(), "main").unwrap_err();
    assert!(
        err.starts_with("not a git repository:"),
        "unexpected error: {err}"
    );
    // The no-arg form reports the same thing, rather than "no base ref found".
    assert!(resolve_base(plain.path(), "")
        .unwrap_err()
        .starts_with("not a git repository:"));
    // In a real repo a bad ref keeps its own message, and a good one resolves.
    let repo = temp_repo();
    assert_eq!(
        resolve_base(repo.path(), "nope-not-a-ref").unwrap_err(),
        "unknown base ref: nope-not-a-ref"
    );
    assert_eq!(resolve_base(repo.path(), "main").unwrap(), "main");
    // The fallback chain still finds `main` with no argument.
    assert_eq!(resolve_base(repo.path(), "").unwrap(), "main");
}

/// `git diff <base>` cannot see a brand-new file, so a review of pending work
/// silently omitted the files most likely to need reviewing. The local diff now
/// synthesises an all-added file for each untracked, non-ignored path.
#[test]
fn untracked_files_appear_as_all_added() {
    let repo = temp_repo();
    std::fs::write(repo.path().join("a_new.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(repo.path().join("z_new.bin"), [0x00u8, 0xff, 0x00]).unwrap();
    std::fs::write(repo.path().join("ignored.txt"), "nope\n").unwrap();
    std::fs::write(repo.path().join(".gitignore"), "ignored.txt\n").unwrap();
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "-qm", "ignore"]);

    let doc = build(repo.path(), "main", "main").unwrap();
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    // Merged into git's own path order, ignored files left out.
    assert_eq!(paths, ["a_new.txt", "f.txt", "z_new.bin"]);
    let new = &doc.files[0];
    assert_eq!((new.added, new.removed, new.binary), (2, 0, false));
    let bin = &doc.files[2];
    assert!(bin.binary);
    assert_eq!((bin.added, bin.removed), (0, 0));

    // Its lines are in the editable views, and its span covers the whole file so
    // `dd` on them actually removes them from disk.
    assert!(doc.after.text.contains("alpha"));
    // Undecorated in the unified stream too — the `+` is gutter chrome, and the
    // `kinds` track is what records that this line is an addition.
    assert!(doc.unified.text.lines().any(|l| l == "beta"));
    let source = doc
        .sources
        .iter()
        .position(|s| s.ends_with("a_new.txt"))
        .unwrap();
    let span = doc
        .unified_spans
        .iter()
        .find(|s| s.source == source)
        .expect("untracked file has an editable span");
    assert_eq!((span.start, span.end), (0, 2));

    // A PR review is exactly what the PR contains — no working-tree extras.
    let pr_doc = build_reviewed(repo.path(), "main", "main", &[]).unwrap();
    assert_eq!(
        pr_doc
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["f.txt"]
    );
}

#[test]
fn parse_patch_tracks_kinds_and_counts() {
    let out = "\
diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,3 @@
 one
-two
-three
+TWO
+four
";
    let files = parse_patch(out);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "f.txt");
    assert_eq!((files[0].added, files[0].removed), (2, 2));
}

#[test]
fn build_emits_aligned_before_after_with_spans() {
    let repo = temp_repo();
    let doc = build(repo.path(), "main", "main").unwrap();
    assert_eq!(doc.base, "main");
    assert_eq!(doc.files.len(), 1);
    assert_eq!(doc.files[0].path, "f.txt");
    // The after side carries the working-tree lines (context + added) and one
    // span per hunk; the before side carries the base lines.
    assert!(doc.after.text.contains("@@@ file: f.txt"));
    assert!(doc.after.text.contains("TWO"));
    assert!(doc.after.text.contains("four"));
    assert!(doc.before.text.contains("two"));
    assert!(doc.before.text.contains("three"));
    assert_eq!(doc.after_spans.len(), 1);
    // Styles align to text lines on both sides.
    assert_eq!(doc.after.styles.len(), doc.after.text.lines().count());
    assert_eq!(doc.before.styles.len(), doc.before.text.lines().count());
    // The added lines are styled on the after side.
    assert!(doc.after.styles.iter().any(|s| s == "added"));
    assert!(doc.before.styles.iter().any(|s| s == "removed"));
}

/// The after column is editable as a projection in its own right: it is a picture
/// of the new file, so every content line is `Live` (` ` context / `+` added) and
/// belongs to its hunk's span, which names the file range it replaces.
///
/// Its markers are *locked* chrome, not span/group headers as the unified view's
/// are: this column shows only the new side, so it holds nothing to revert a hunk
/// back to. Deleting a marker is refused rather than half-reverting the hunk.
#[test]
fn the_after_column_is_an_undecorated_projection() {
    let repo = temp_repo();
    let doc = build(repo.path(), "main", "main").unwrap();
    // title, file marker, hunk marker, context `one`, `TWO`, `four`.
    assert_eq!(doc.after.kinds, "lll ++");
    assert_eq!(doc.after.line_spans, vec![-1, -1, -1, 0, 0, 0]);
    assert_eq!(
        doc.after.kinds.chars().count(),
        doc.after.text.lines().count()
    );
    // The lines themselves are the file's, undecorated — no `+`/`-`/space.
    let lines: Vec<&str> = doc.after.text.lines().collect();
    assert_eq!(&lines[3..], ["one", "TWO", "four"]);
    // One span per hunk, over the whole 3-line new file, in the file's group.
    assert_eq!(
        doc.after_spans,
        vec![SpanSpec {
            source: 0,
            start: 0,
            end: 3,
            group: 0,
        }]
    );
    // Both projections address the same source list.
    assert_eq!(doc.sources.len(), 1);
    assert!(doc.sources[0].ends_with("f.txt"));
}

/// The unified stream itself, plus the provenance tracks that make it editable.
///
/// The semantics of *editing* a projection — what deleting a `+` line or a `-`
/// line means — are not tested here any more: they no longer live in this crate.
/// The host tracks each line's origin and folds the edits back
/// (`garden_core::projection`, tested there and end-to-end in `garden-app`'s
/// panel tests). What this crate still owns, and what these tests cover, is
/// *describing* the projection correctly.
#[test]
fn build_emits_a_unified_projection() {
    let repo = temp_repo();
    let doc = build(repo.path(), "main", "main").unwrap();
    // The unified side is one styled stream: file header, hunk header, then the
    // change's lines. The `+`/`-`/space markers are NOT in the text — they are
    // drawn in the region's gutter from the `kinds` track below — so each line
    // carries the file's own content and nothing else. That is what lets the
    // view be edited (and searched) like a file rather than like a patch.
    assert!(doc.unified.text.contains("@@@ file: f.txt"));
    let body: Vec<&str> = doc.unified.text.lines().collect();
    assert!(body.contains(&"two"), "deletion, undecorated: {body:?}");
    assert!(body.contains(&"TWO"), "addition, undecorated: {body:?}");
    assert!(body.contains(&"one"), "context, undecorated: {body:?}");
    assert!(
        !body
            .iter()
            .any(|l| l.starts_with('+') || l.starts_with('-')),
        "no line wears a diff marker: {body:?}"
    );
    assert_eq!(doc.unified.styles.len(), doc.unified.text.lines().count());
    assert!(doc.unified.styles.iter().any(|s| s == "added"));
    assert!(doc.unified.styles.iter().any(|s| s == "removed"));
    // The hunk header names its file as well as its range, so a reader who has
    // scrolled the file heading off the top still knows where they are.
    assert!(doc.unified.text.contains("@@@ hunk: f.txt @@ "));

    // The provenance tracks are exactly parallel to the text's lines — the
    // contract the host's `edit_view_projection` relies on.
    let n = doc.unified.text.lines().count();
    assert_eq!(doc.unified.kinds.chars().count(), n);
    assert_eq!(doc.unified.line_spans.len(), n);
    // title, file marker, hunk marker, context, two deletions, two additions.
    assert_eq!(doc.unified.kinds, "lgh --++");
    // Everything but the title belongs to the file's one span.
    assert_eq!(doc.unified.line_spans, vec![-1, 0, 0, 0, 0, 0, 0, 0]);

    // One editable span per hunk, naming the file range its content replaces.
    assert_eq!(doc.sources.len(), 1);
    assert!(doc.sources[0].ends_with("f.txt"));
    assert_eq!(
        doc.unified_spans,
        vec![SpanSpec {
            source: 0,
            start: 0,
            end: 3, // the whole 3-line new file
            group: 0,
        }]
    );
}

/// The two structural markers are chrome that *names* what deleting it should
/// revert: the hunk header its own span, the file header the group of spans that
/// file owns. That is what backs `dd`-on-a-header in the reviewer.
#[test]
fn marker_lines_are_chrome_naming_their_span_and_group() {
    let repo = temp_repo();
    let doc = build(repo.path(), "main", "main").unwrap();
    let lines: Vec<&str> = doc.unified.text.lines().collect();
    let kinds: Vec<char> = doc.unified.kinds.chars().collect();
    let at = |needle: &str| lines.iter().position(|l| l.starts_with(needle)).unwrap();

    // The title is locked: it belongs to the view, not to the change.
    assert_eq!(kinds[0], 'l');
    assert_eq!(doc.unified.line_spans[0], -1);
    // The file marker heads the group; the hunk marker heads the span. Both point
    // at a real span, which is where the group id is discoverable.
    assert_eq!(kinds[at("@@@ file:")], 'g');
    assert_eq!(kinds[at("@@@ hunk:")], 'h');
    assert_eq!(doc.unified.line_spans[at("@@@ file:")], 0);
    assert_eq!(doc.unified.line_spans[at("@@@ hunk:")], 0);
}

/// Woven PR comments sit inside a hunk but are not file content, so they are
/// recorded as chrome: editing or deleting one writes nothing, whatever the user
/// does to it.
#[test]
fn woven_review_comments_are_recorded_as_chrome() {
    let repo = temp_repo();
    let comments = [Comment {
        author: "octocat".to_string(),
        date: "2026-07-20".to_string(),
        body: "why shout?".to_string(),
        path: Some("f.txt".to_string()),
        line: Some(2),
        left_side: false,
    }];
    let doc = build_reviewed(repo.path(), "main", "main", &comments).unwrap();
    let kinds: Vec<char> = doc.unified.kinds.chars().collect();
    let at = |needle: &str| {
        doc.unified
            .text
            .lines()
            .position(|l| l.contains(needle))
            .unwrap()
    };
    assert_eq!(kinds[at("why shout?")], 'c');
    // …and the tracks are still line-aligned with the comments woven in.
    assert_eq!(kinds.len(), doc.unified.text.lines().count());
    assert_eq!(doc.unified.line_spans.len(), kinds.len());
}

/// Every unified hunk header names the file it belongs to, so the stream stays
/// self-describing once the `@@@ file:` heading has scrolled away. The split
/// view's headers deliberately do not — it has the FILES column beside it.
#[test]
fn unified_hunk_headers_name_their_file() {
    let repo = temp_repo();
    // `temp_repo` leaves f.txt modified in the working tree; g.txt joins it, so
    // the diff genuinely spans two files. Commit g.txt on its own — `add -A`
    // would sweep up f.txt's pending edit and leave nothing to diff.
    std::fs::write(repo.path().join("g.txt"), "alpha\nbeta\n").unwrap();
    git(&repo, &["add", "g.txt"]);
    git(&repo, &["commit", "-qm", "g"]);
    std::fs::write(repo.path().join("g.txt"), "alpha\nBETA\n").unwrap();

    let doc = build(repo.path(), "HEAD", "HEAD").unwrap();
    let headers: Vec<&str> = doc
        .unified
        .text
        .lines()
        .filter(|l| l.starts_with(HUNK_PREFIX))
        .collect();
    // One per file, each carrying its own path ahead of the range — not the
    // first file's path repeated, which is what a hoisted label would give.
    assert_eq!(headers.len(), 2, "{headers:?}");
    assert!(headers[0].starts_with("@@@ hunk: f.txt @@ "), "{headers:?}");
    assert!(headers[1].starts_with("@@@ hunk: g.txt @@ "), "{headers:?}");

    // The split view's headers are unchanged: no path, just the range.
    let split: Vec<&str> = doc
        .after
        .text
        .lines()
        .filter(|l| l.starts_with(HUNK_PREFIX))
        .collect();
    assert!(split.iter().all(|l| !l.contains("g.txt")), "{split:?}");

    // Naming the file is cosmetic: the header is still chrome heading its span,
    // so `dd` on it still reverts the hunk and nothing of it reaches disk.
    let i = doc
        .unified
        .text
        .lines()
        .position(|l| l.starts_with(HUNK_PREFIX))
        .unwrap();
    assert_eq!(doc.unified.kinds.chars().nth(i), Some('h'));
}

/// Two hunks in one file get two spans, each naming its own range of the file —
/// what lets them be written back independently.
#[test]
fn multiple_hunks_in_one_file_get_independent_spans() {
    let repo = temp_repo();
    let long: String = (1..=20).map(|i| format!("line{i}\n")).collect();
    std::fs::write(repo.path().join("g.txt"), &long).unwrap();
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .status()
            .unwrap();
    };
    run(&["add", "-A"]);
    run(&["commit", "-qm", "g"]);
    let changed = long.replace("line2\n", "LINE2\n").replace("line19\n", "");
    std::fs::write(repo.path().join("g.txt"), &changed).unwrap();

    let doc = build(repo.path(), "HEAD", "HEAD").unwrap();
    assert_eq!(doc.unified_spans.len(), 2);
    // Both write to the same file, in the same group, at disjoint ranges.
    assert_eq!(doc.unified_spans[0].source, doc.unified_spans[1].source);
    assert_eq!(doc.unified_spans[0].group, doc.unified_spans[1].group);
    assert!(doc.unified_spans[0].end <= doc.unified_spans[1].start);
}

/// The write-back itself: the host resolved these edits from the line origins it
/// tracked, so applying them is a splice and nothing more.
#[test]
fn apply_edits_writes_the_resolved_lines() {
    let repo = temp_repo();
    let f = repo.path().join("f.txt");
    // Replace the whole 3-line file, as a reverted deletion would.
    let written = apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 0,
        end: 3,
        lines: vec!["one".into(), "three".into(), "TWO".into(), "four".into()],
        expected: None,
    }])
    .unwrap()
    .written;
    assert_eq!(written, vec!["f.txt"]);
    assert_eq!(read(&repo, "f.txt"), "one\nthree\nTWO\nfour\n");
}

/// Several spans of one file are applied together, high-to-low, so an edit in the
/// first never shifts where the second lands.
#[test]
fn apply_edits_splices_disjoint_ranges_of_one_file() {
    let repo = temp_repo();
    let f = repo.path().join("f.txt");
    std::fs::write(&f, "a\nb\nc\nd\ne\n").unwrap();
    apply_edits(vec![
        SourceEdit {
            source: f.clone(),
            start: 0,
            end: 1,
            lines: vec!["A".into()],
            expected: None,
        },
        SourceEdit {
            source: f.clone(),
            start: 3,
            end: 5,
            lines: vec!["D".into()],
            expected: None,
        },
    ])
    .unwrap();
    assert_eq!(read(&repo, "f.txt"), "A\nb\nc\nD\n");
}

/// An edit that changes nothing writes nothing — the reviewer reports "no changes
/// to write" rather than touching mtimes.
#[test]
fn apply_edits_leaves_an_unchanged_file_alone() {
    let repo = temp_repo();
    let written = apply_edits(vec![SourceEdit {
        source: repo.path().join("f.txt"),
        start: 0,
        end: 3,
        lines: vec!["one".into(), "TWO".into(), "four".into()],
        expected: None,
    }])
    .unwrap()
    .written;
    assert!(written.is_empty());
    assert_eq!(read(&repo, "f.txt"), "one\nTWO\nfour\n");
}

// ── staleness: did the file change under the view? ─────────────────────────

/// The ordinary save: the span still holds what the view projected, so the write
/// is silent. `expected` is the file as it was when the review opened — the same
/// lines the edit replaces.
#[test]
fn apply_edits_reports_nothing_stale_when_the_file_still_matches() {
    let repo = temp_repo();
    let applied = apply_edits(vec![SourceEdit {
        source: repo.path().join("f.txt"),
        start: 0,
        end: 3,
        lines: vec!["one".into(), "EDITED".into(), "four".into()],
        expected: Some(vec!["one".into(), "TWO".into(), "four".into()]),
    }])
    .unwrap();
    assert_eq!(applied.written, vec!["f.txt"]);
    assert_eq!(applied.stale, vec![]);
    assert_eq!(read(&repo, "f.txt"), "one\nEDITED\nfour\n");
}

/// The bug this exists for: the file changed on disk *inside* the span after the
/// view loaded. The user's decision is to write anyway — refusing would strand
/// their edits — but to say so, naming the file and how many hunks went over the
/// top of it.
#[test]
fn apply_edits_writes_a_changed_file_but_reports_it_stale() {
    let repo = temp_repo();
    let f = repo.path().join("f.txt");
    // Someone (another editor, a rebase) rewrote line 2 since the view loaded.
    std::fs::write(&f, "one\nEXTERNAL\nfour\n").unwrap();
    let applied = apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 0,
        end: 3,
        lines: vec!["one".into(), "MINE".into(), "four".into()],
        expected: Some(vec!["one".into(), "TWO".into(), "four".into()]),
    }])
    .unwrap();
    assert_eq!(applied.written, vec!["f.txt"]);
    assert_eq!(applied.stale, vec![("f.txt".to_string(), 1)]);
    assert_eq!(read(&repo, "f.txt"), "one\nMINE\nfour\n");
}

/// Only the spans that drifted count, and only the files that own them: a second
/// hunk of the same file that still matches is not reported, and neither is an
/// untouched second file.
#[test]
fn staleness_is_counted_per_span_and_reported_per_file() {
    let repo = temp_repo();
    let f = repo.path().join("f.txt");
    let g = repo.path().join("g.txt");
    std::fs::write(&f, "a\nb\nc\nd\ne\n").unwrap();
    std::fs::write(&g, "x\n").unwrap();
    let applied = apply_edits(vec![
        // Matches: `a`.
        SourceEdit {
            source: f.clone(),
            start: 0,
            end: 1,
            lines: vec!["A".into()],
            expected: Some(vec!["a".into()]),
        },
        // Drifted: the view thought line 5 was `E`.
        SourceEdit {
            source: f.clone(),
            start: 4,
            end: 5,
            lines: vec!["EE".into()],
            expected: Some(vec!["E".into()]),
        },
        SourceEdit {
            source: g.clone(),
            start: 0,
            end: 1,
            lines: vec!["X".into()],
            expected: Some(vec!["x".into()]),
        },
    ])
    .unwrap();
    assert_eq!(applied.stale, vec![("f.txt".to_string(), 1)]);
    assert_eq!(read(&repo, "f.txt"), "A\nb\nc\nd\nEE\n");
    assert_eq!(read(&repo, "g.txt"), "X\n");
}

/// A file the working tree deleted reads as empty so the reviewer can revert the
/// deletion. There is nothing on disk to overwrite, so recreating it is never a
/// stale write — however much content the span expected.
#[test]
fn reverting_a_deleted_file_is_never_stale() {
    let repo = temp_repo();
    let gone = repo.path().join("gone.txt");
    let applied = apply_edits(vec![SourceEdit {
        source: gone.clone(),
        start: 0,
        end: 2,
        lines: vec!["back".into()],
        expected: Some(vec!["was".into(), "here".into()]),
    }])
    .unwrap();
    assert_eq!(applied.written, vec!["gone.txt"]);
    assert_eq!(applied.stale, vec![]);
    assert_eq!(std::fs::read_to_string(&gone).unwrap(), "back\n");
}

/// The comparison uses the same line splitting the splice does, so a CRLF file
/// is not falsely accused: the expectation the host captured carries no `\r`,
/// and neither do the lines it is compared with.
#[test]
fn a_crlf_file_is_not_reported_stale() {
    let repo = temp_repo();
    let f = repo.path().join("crlf.txt");
    std::fs::write(&f, "c1\r\nc2\r\nc3\r\n").unwrap();
    let applied = apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 1,
        end: 2,
        lines: vec!["C2".into()],
        expected: Some(vec!["c2".into()]),
    }])
    .unwrap();
    assert_eq!(applied.stale, vec![]);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "c1\r\nC2\r\nc3\r\n");
}

/// An edit that records no expectation is checked against nothing — an older
/// host, or a projection with no captured baseline.
#[test]
fn an_edit_without_an_expectation_is_never_stale() {
    let repo = temp_repo();
    let applied = apply_edits(vec![SourceEdit {
        source: repo.path().join("f.txt"),
        start: 0,
        end: 3,
        lines: vec!["whatever".into()],
        expected: None,
    }])
    .unwrap();
    assert_eq!(applied.stale, vec![]);
}

fn read(repo: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(repo.path().join(name)).unwrap()
}

/// A file deleted in the working tree still has a span — its `-` lines are ghosts
/// the reviewer may want to revert. The source is gone from disk, so it reads as
/// empty and the revert *recreates* it, parent directories and all.
#[test]
fn apply_edits_recreates_a_deleted_file() {
    let repo = temp_repo();
    let gone = repo.path().join("nested/dir/gone.txt");
    let written = apply_edits(vec![SourceEdit {
        source: gone.clone(),
        start: 0,
        end: 0,
        lines: vec!["back".into(), "again".into()],
        expected: None,
    }])
    .unwrap()
    .written;
    assert_eq!(written, vec!["gone.txt"]);
    assert_eq!(std::fs::read_to_string(&gone).unwrap(), "back\nagain\n");
}

/// A missing source no longer aborts the whole apply: the other files in the set
/// are written exactly as they would have been on their own. `f.txt` sorts before
/// `zz_gone.txt`, and `a.txt` after it, so both sides of the BTreeMap order are
/// covered.
#[test]
fn apply_edits_survives_a_missing_source() {
    let repo = temp_repo();
    std::fs::write(repo.path().join("a.txt"), "x\n").unwrap();
    let edit = |name: &str, lines: Vec<String>| SourceEdit {
        source: repo.path().join(name),
        start: 0,
        end: 1,
        lines,
        expected: None,
    };
    let mut written = apply_edits(vec![
        edit("f.txt", vec!["ONE".into()]),
        edit("zz_gone.txt", vec!["revived".into()]),
        edit("a.txt", vec!["X".into()]),
    ])
    .unwrap()
    .written;
    written.sort();
    assert_eq!(written, vec!["a.txt", "f.txt", "zz_gone.txt"]);
    assert_eq!(read(&repo, "f.txt"), "ONE\nTWO\nfour\n");
    assert_eq!(read(&repo, "a.txt"), "X\n");
    assert_eq!(read(&repo, "zz_gone.txt"), "revived\n");
}

/// The apply is transactional: every file's new contents are resolved first, and
/// the writes only happen once all of them succeeded. A target that cannot be read
/// (here, a directory standing where a file should be — the reliable failure on
/// macOS) fails the whole apply and leaves every other file byte-identical, even
/// the ones that sort *before* it and would have been written first.
#[test]
fn apply_edits_writes_nothing_when_one_source_fails() {
    let repo = temp_repo();
    std::fs::write(repo.path().join("a.txt"), "x\n").unwrap();
    std::fs::create_dir(repo.path().join("m_dir")).unwrap();
    let edit = |name: &str| SourceEdit {
        source: repo.path().join(name),
        start: 0,
        end: 1,
        lines: vec!["CHANGED".into()],
        expected: None,
    };
    // `a.txt` < `f.txt` < `m_dir`: both would-be writes precede the failure.
    let err = apply_edits(vec![edit("a.txt"), edit("f.txt"), edit("m_dir")]).unwrap_err();
    assert!(err.contains("m_dir"), "unexpected error: {err}");
    assert_eq!(read(&repo, "a.txt"), "x\n");
    assert_eq!(read(&repo, "f.txt"), "one\nTWO\nfour\n");
}

/// A binary source is still an error — `read_to_string` rejects it as invalid
/// data, and that must not be laundered into "the file was empty", which would
/// truncate it.
#[test]
fn apply_edits_refuses_a_non_utf8_source() {
    let repo = temp_repo();
    let bin = repo.path().join("b.bin");
    std::fs::write(&bin, [0x00u8, 0xff, 0xfe, 0x00]).unwrap();
    let err = apply_edits(vec![SourceEdit {
        source: bin.clone(),
        start: 0,
        end: 1,
        lines: vec!["nope".into()],
        expected: None,
    }])
    .unwrap_err();
    assert!(err.contains("b.bin"), "unexpected error: {err}");
    assert_eq!(std::fs::read(&bin).unwrap(), [0x00u8, 0xff, 0xfe, 0x00]);
}

/// A CRLF file keeps CRLF. Splicing one hunk used to rewrite the endings of every
/// line in the file, because the split dropped the `\r` and the join put back only
/// `\n`; the file's dominant ending is now what the rejoin uses, so untouched
/// lines survive and the inserted line follows the file's convention.
#[test]
fn apply_edits_preserves_crlf_line_endings() {
    let repo = temp_repo();
    let f = repo.path().join("crlf.txt");
    std::fs::write(&f, "c1\r\nc2\r\nc3\r\n").unwrap();
    apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 1,
        end: 2,
        lines: vec!["C2".into()],
        expected: None,
    }])
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "c1\r\nC2\r\nc3\r\n",
        "untouched lines must keep CRLF and the new line must adopt it"
    );
}

/// An LF file is byte-identical to what it always was — the dominant-ending rule
/// resolves to `\n` when nothing is CRLF.
#[test]
fn apply_edits_leaves_lf_line_endings_alone() {
    let repo = temp_repo();
    let f = repo.path().join("lf.txt");
    std::fs::write(&f, "l1\nl2\nl3\n").unwrap();
    apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 1,
        end: 2,
        lines: vec!["L2".into()],
        expected: None,
    }])
    .unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "l1\nL2\nl3\n");
}

/// A missing trailing newline still round-trips, in either convention.
#[test]
fn apply_edits_preserves_a_missing_trailing_newline() {
    let repo = temp_repo();
    let lf = repo.path().join("nolf.txt");
    std::fs::write(&lf, "a\nb").unwrap();
    let crlf = repo.path().join("nocrlf.txt");
    std::fs::write(&crlf, "a\r\nb").unwrap();
    apply_edits(vec![
        SourceEdit {
            source: lf.clone(),
            start: 0,
            end: 1,
            lines: vec!["A".into()],
            expected: None,
        },
        SourceEdit {
            source: crlf.clone(),
            start: 0,
            end: 1,
            lines: vec!["A".into()],
            expected: None,
        },
    ])
    .unwrap();
    assert_eq!(std::fs::read_to_string(&lf).unwrap(), "A\nb");
    assert_eq!(std::fs::read_to_string(&crlf).unwrap(), "A\r\nb");
}

/// A mixed file picks the ending the majority of its terminators use, and a lone
/// `\r` in the middle of a line is content, not a terminator, so it survives
/// untouched.
#[test]
fn apply_edits_normalises_a_mixed_file_to_its_dominant_ending() {
    let repo = temp_repo();
    let f = repo.path().join("mixed.txt");
    // Three CRLF terminators, one LF: CRLF wins. `m3` carries an interior `\r`.
    std::fs::write(&f, "m1\r\nm2\r\nm3a\rm3b\nm4\r\n").unwrap();
    apply_edits(vec![SourceEdit {
        source: f.clone(),
        start: 0,
        end: 1,
        lines: vec!["M1".into()],
        expected: None,
    }])
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "M1\r\nm2\r\nm3a\rm3b\r\nm4\r\n"
    );
}

/// PR review comments are threaded into the unified view under the line they
/// were left on (new-file line for a RIGHT comment, old-file line for LEFT), and
/// an anchor that no longer exists in the diff still shows, at the file's end.
/// The split sides stay comment-free — that column is the new file, verbatim.
#[test]
fn unified_weaves_inline_review_comments() {
    let repo = temp_repo();
    let comment = |line: usize, left: bool, body: &str| Comment {
        author: "octocat".to_string(),
        date: "2026-07-20".to_string(),
        body: body.to_string(),
        path: Some("f.txt".to_string()),
        line: Some(line),
        left_side: left,
    };
    // The diff is `one / -two / -three / +TWO / +four`: new line 2 is `TWO`,
    // old line 3 is `three`, and line 99 exists on neither side.
    let doc = build_reviewed(
        repo.path(),
        "main",
        "main",
        &[
            comment(2, false, "why shout?"),
            comment(3, true, "good riddance"),
            comment(99, false, "stale thread"),
        ],
    )
    .unwrap();

    let lines: Vec<&str> = doc.unified.text.lines().collect();
    let at = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
    // Anchored directly under their target lines. The diff lines are
    // undecorated (the markers are gutter chrome), so they are matched whole
    // rather than by their old `+`/`-` prefix.
    let row = |text: &str| lines.iter().position(|l| *l == text).unwrap();
    assert_eq!(at("why shout?"), row("TWO") + 2);
    assert_eq!(at("good riddance"), row("three") + 2);
    // The stale thread lands in the trailing outside-this-diff block.
    assert!(at("stale thread") > row("four"));
    assert!(lines
        .iter()
        .any(|l| l.contains("comments on lines outside this diff")));
    // Comment lines carry the `comment` style, and styles stay line-aligned.
    assert_eq!(doc.unified.styles.len(), doc.unified.text.lines().count());
    assert_eq!(doc.unified.styles[at("why shout?")], "comment");
    // The editable split sides are untouched by comments.
    assert!(!doc.after.text.contains("why shout?"));
    assert!(!doc.before.text.contains("good riddance"));
    assert_eq!(doc.after_spans.len(), 1);
}

/// Each summary records the line its `@@@ file:` marker landed on in every
/// projection — the anchor the file list scrolls a view to. The before and
/// after sides are emitted in lockstep, so their numbers match.
#[test]
fn summaries_record_each_projection_s_marker_line() {
    let repo = temp_repo();
    let doc = build(repo.path(), "main", "main").unwrap();
    let f = &doc.files[0];
    let line_of = |text: &str| {
        text.lines()
            .position(|l| l == "@@@ file: f.txt")
            .expect("marker present")
    };
    assert_eq!(f.line_before, line_of(&doc.before.text));
    assert_eq!(f.line_after, line_of(&doc.after.text));
    assert_eq!(f.line_unified, line_of(&doc.unified.text));
    assert_eq!(f.line_before, f.line_after);
}

/// The same anchors across a *multi-file* diff, which is where an off-by-one in
/// the running line count would show up: every summary's recorded line must be
/// its own file's marker in every projection, not the one before or after it.
#[test]
fn every_file_s_marker_line_is_its_own_in_a_multi_file_diff() {
    let repo = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@e.com"]);
    run(&["config", "user.name", "T"]);
    let names = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"];
    for (i, n) in names.iter().enumerate() {
        let body: String = (1..=40)
            .map(|l| format!("{}{l}\n", n.chars().next().unwrap()))
            .collect();
        std::fs::write(repo.path().join(n), &body).unwrap();
        let _ = i;
    }
    run(&["add", "-A"]);
    run(&["commit", "-qm", "init"]);
    for n in names {
        let body: String = (1..=40)
            .map(|l| {
                let c = n.chars().next().unwrap();
                if l == 5 {
                    format!("{c}{l}-edited\n")
                } else {
                    format!("{c}{l}\n")
                }
            })
            .collect();
        std::fs::write(repo.path().join(n), &body).unwrap();
    }

    let doc = build(repo.path(), "main", "main").unwrap();
    assert_eq!(doc.files.len(), 5);
    for f in &doc.files {
        let marker = format!("{FILE_PREFIX}{}", f.path);
        for (proj, line) in [
            (&doc.unified, f.line_unified),
            (&doc.after, f.line_after),
            (&doc.before, f.line_before),
        ] {
            let lines: Vec<&str> = proj.text.lines().collect();
            assert_eq!(
                lines[line], marker,
                "{} anchored at {line}, which reads {:?}",
                f.path, lines[line]
            );
        }
    }
}

// ── commits + commit-scoped diffs ──────────────────────────────────────────

/// A repo whose `main` has one commit and whose branch adds two more on top,
/// plus an uncommitted working-tree edit — the shape a real review has.
fn branched_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let ok = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@e.com"]);
    run(&["config", "user.name", "T"]);
    std::fs::write(repo.path().join("f.txt"), "one\ntwo\nthree\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "base commit"]);

    run(&["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    run(&["commit", "-qam", "shout the second line"]);
    std::fs::write(repo.path().join("g.txt"), "gamma\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "add g.txt"]);

    // Uncommitted on top, so "since" and "whole" can be told apart from "commit".
    std::fs::write(repo.path().join("f.txt"), "one\nTWO\nTHREE\n").unwrap();
    repo
}

/// The review's commits, newest first, with the fields the drawer draws.
#[test]
fn commits_lists_the_range_newest_first() {
    let repo = branched_repo();
    let list = commits(repo.path(), "main").unwrap();
    let subjects: Vec<&str> = list.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["add g.txt", "shout the second line"]);
    assert_eq!(list[0].author, "T");
    assert_eq!(list[0].short.len(), 7);
    assert!(list[0].sha.starts_with(&list[0].short));
    assert_eq!(list[0].date.len(), 10, "YYYY-MM-DD: {}", list[0].date);

    // A branch level with its base has no commits of its own — an empty list,
    // not an error: that is a normal state for `garden diff`, not a failure.
    assert!(commits(repo.path(), "feature").unwrap().is_empty());
}

/// A commit-scoped diff shows that commit and nothing else — not the commit
/// before it, and not the uncommitted work on top.
#[test]
fn build_range_scopes_the_diff_to_one_commit() {
    let repo = branched_repo();
    let list = commits(repo.path(), "main").unwrap();
    let newest = &list[0]; // "add g.txt"
    let older = &list[1]; // "shout the second line"

    let doc = build_range(
        repo.path(),
        &format!("{}^", newest.sha),
        &newest.sha,
        "newest",
    )
    .unwrap();
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["g.txt"], "only the newest commit's file");
    assert!(doc.unified.text.contains("gamma"));
    // The earlier commit's change and the uncommitted one are both out of scope.
    assert!(!doc.unified.text.contains("TWO"));
    assert!(!doc.unified.text.contains("THREE"));

    let doc = build_range(repo.path(), &format!("{}^", older.sha), &older.sha, "older").unwrap();
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["f.txt"]);
    assert!(doc.unified.text.contains("TWO"));
    assert!(
        !doc.unified.text.contains("THREE"),
        "uncommitted, so absent"
    );
}

/// A commit-scoped diff is read-only, and says so in the two flags the drawer
/// reads: its after side describes a blob, not the checkout, so writing it back
/// would splice the past into the present. Its markers are in the text, because
/// a read-only `text_view` has no projection and so no gutter to draw them in.
#[test]
fn a_commit_scoped_diff_is_read_only_and_prefixed() {
    let repo = branched_repo();
    let newest = &commits(repo.path(), "main").unwrap()[0];
    let scoped = build_range(
        repo.path(),
        &format!("{}^", newest.sha),
        &newest.sha,
        "newest",
    )
    .unwrap();
    assert!(!scoped.editable);
    assert!(scoped.markers_in_text);
    assert!(
        scoped.unified.text.lines().any(|l| l == "+gamma"),
        "a read-only view wears its markers: {}",
        scoped.unified.text
    );

    // The working-tree diff beside it is the opposite on both counts.
    let whole = build(repo.path(), "main", "main").unwrap();
    assert!(whole.editable);
    assert!(!whole.markers_in_text);
    assert!(!whole
        .unified
        .text
        .lines()
        .any(|l| l.starts_with('+') || l.starts_with('-')));
}

/// The first commit in a repository has no parent, and "show me just this
/// commit" must still work there — it is the one case where the commit *is*
/// the whole diff. Diffing against git's empty tree is what makes that true.
#[test]
fn a_root_commit_diffs_against_the_empty_tree() {
    let repo = branched_repo();
    let root = run_git(repo.path(), &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
    let root = root.trim();
    // Its parent does not exist, which is what `main.rs::parent_of` detects.
    assert!(run_git(
        repo.path(),
        &["rev-parse", "--verify", "--quiet", &format!("{root}^")]
    )
    .is_err());

    let doc = build_range(
        repo.path(),
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        root,
        "root",
    )
    .unwrap();
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["f.txt"]);
    assert_eq!((doc.files[0].added, doc.files[0].removed), (3, 0));
}
