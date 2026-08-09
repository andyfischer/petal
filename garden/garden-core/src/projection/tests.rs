//! Unit tests for the editable-projection model.
//!
//! Every test drives a small unified-diff projection over one file the way the
//! editor would: a [`Doc`] holds the projected text beside the table, and each
//! edit changes both — the text as a buffer would, the table through
//! [`Projection::splice`]. What is asserted is the fold ([`Projection::resolve`]),
//! i.e. what would be written back to the file.

use super::*;

/// The projected text and its table, edited together. `undo` counts the
/// buffer's undo position the way a real buffer would move it: one step per
/// edit, so [`Projection::sync_to`] can be exercised against it.
struct Doc {
    lines: Vec<String>,
    proj: Projection,
    undo: usize,
}

/// The fixture: one file, one hunk, one line each of context / deletion /
/// addition / context, under a file header and a locked title.
///
/// ```text
///  0  unified: main → working tree     locked title
///  1  @@@ file: a.txt                  group header
///  2  @@ -1,3 +1,3 @@                  span header
///  3   one                             context
///  4  -two                             deletion (base line "two")
///  5  +TWO                             addition
///  6   three                           context
/// ```
fn doc() -> Doc {
    let decor = Decor {
        same: (" ".into(), String::new()),
        added: ("+".into(), "added".into()),
        removed: ("-".into(), "removed".into()),
        new_line: NewLine::DiffMarker,
        gutter: false,
    };
    let spans = vec![Span {
        source: 0,
        target: (0, 3),
        group: Some(0),
    }];
    let mut proj = Projection::new(vec!["a.txt".into()], spans, decor);
    proj.push(
        LineOrigin::Chrome {
            role: ChromeRole::Plain,
            locked: true,
        },
        "title",
        None,
    );
    proj.push(
        LineOrigin::Chrome {
            role: ChromeRole::GroupHeader,
            locked: false,
        },
        "file",
        Some(0),
    );
    proj.push(
        LineOrigin::Chrome {
            role: ChromeRole::SpanHeader,
            locked: false,
        },
        "hunk",
        Some(0),
    );
    proj.push(LineOrigin::Live { added: false }, "", Some(0));
    proj.push(LineOrigin::Ghost { text: "two".into() }, "removed", Some(0));
    proj.push(LineOrigin::Live { added: true }, "added", Some(0));
    proj.push(LineOrigin::Live { added: false }, "", Some(0));
    Doc {
        lines: [
            "unified: main → working tree",
            "@@@ file: a.txt",
            "@@ -1,3 +1,3 @@",
            " one",
            "-two",
            "+TWO",
            " three",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        proj,
        undo: 0,
    }
}

impl Doc {
    /// Replace rows `[at, at + removed)` with `inserted`, in both the text and
    /// the table — one buffer edit, as the editor would make it.
    fn splice(&mut self, at: usize, removed: usize, inserted: &[&str]) {
        self.undo += 1;
        self.lines.splice(
            at..at + removed,
            inserted.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        self.proj.splice(at, removed, inserted.len(), self.undo);
    }

    fn delete(&mut self, at: usize, count: usize) {
        self.splice(at, count, &[]);
    }

    /// Apply a claimed [`Outcome`]'s row operations, the way the editor does:
    /// bottom-up, so earlier rows keep their indices.
    fn apply(&mut self, outcome: Outcome) {
        let Outcome::Claimed { start, ops } = outcome else {
            panic!("expected a claimed outcome, got {outcome:?}");
        };
        for (i, op) in ops.iter().enumerate().rev() {
            match op {
                RowOp::Keep => {}
                RowOp::Delete => {
                    self.lines.remove(start + i);
                }
                RowOp::Set(text) => self.lines[start + i] = text.clone(),
            }
        }
    }

    /// Record the fixture's starting state as the projection's baseline, the way
    /// a host does the moment it builds one. Only the dirty-tracking tests want
    /// this: the fold tests assert on *every* span, so they leave the baseline
    /// uncaptured and see the raw fold.
    fn capture(&mut self) {
        self.proj.capture_baseline(&self.lines);
    }

    /// What would be written back to span `i`'s source.
    fn fold(&self, i: usize) -> Vec<String> {
        self.proj.resolve(&self.lines)[i].lines.clone()
    }

    /// What would be written back to the file: the single span's new content.
    fn folded(&self) -> Vec<String> {
        let edits = self.proj.resolve(&self.lines);
        assert_eq!(edits.len(), 1, "the fixture has exactly one span");
        assert_eq!(edits[0].source, "a.txt");
        assert_eq!((edits[0].start, edits[0].end), (0, 3));
        edits[0].lines.clone()
    }
}

/// A two-file fixture, so that "which file did this line land in?" is a
/// question with an answer. One woven review comment sits inside the first
/// file's block, to pin down that plain chrome does *not* end a block.
///
/// ```text
///  0  unified: main → working tree     locked title
///  1  @@@ file: aa.txt                 group header   span 0
///  2  @@ -1,3 +1,3 @@                  span header    span 0
///  3   a1                              context
///  4  -a2                              deletion
///  5  +A2                              addition
///  6   a3                              context
///  7  ## reads fine to me              plain chrome   span 0
///  8  @@@ file: bb.txt                 group header   span 1
///  9  @@ -1,3 +1,3 @@                  span header    span 1
/// 10   b1                              context
/// 11  +B2                              addition
/// 12   b3                              context
/// ```
fn two_files() -> Doc {
    let decor = Decor {
        same: (" ".into(), String::new()),
        added: ("+".into(), "added".into()),
        removed: ("-".into(), "removed".into()),
        new_line: NewLine::DiffMarker,
        gutter: false,
    };
    let spans = vec![
        Span {
            source: 0,
            target: (0, 3),
            group: Some(0),
        },
        Span {
            source: 1,
            target: (0, 3),
            group: Some(1),
        },
    ];
    let mut proj = Projection::new(vec!["aa.txt".into(), "bb.txt".into()], spans, decor);
    let chrome = |role, locked| LineOrigin::Chrome { role, locked };
    proj.push(chrome(ChromeRole::Plain, true), "title", None);
    proj.push(chrome(ChromeRole::GroupHeader, false), "file", Some(0));
    proj.push(chrome(ChromeRole::SpanHeader, false), "hunk", Some(0));
    proj.push(LineOrigin::Live { added: false }, "", Some(0));
    proj.push(LineOrigin::Ghost { text: "a2".into() }, "removed", Some(0));
    proj.push(LineOrigin::Live { added: true }, "added", Some(0));
    proj.push(LineOrigin::Live { added: false }, "", Some(0));
    proj.push(chrome(ChromeRole::Plain, false), "comment", Some(0));
    proj.push(chrome(ChromeRole::GroupHeader, false), "file", Some(1));
    proj.push(chrome(ChromeRole::SpanHeader, false), "hunk", Some(1));
    proj.push(LineOrigin::Live { added: false }, "", Some(1));
    proj.push(LineOrigin::Live { added: true }, "added", Some(1));
    proj.push(LineOrigin::Live { added: false }, "", Some(1));
    Doc {
        lines: [
            "unified: main → working tree",
            "@@@ file: aa.txt",
            "@@ -1,3 +1,3 @@",
            " a1",
            "-a2",
            "+A2",
            " a3",
            "## reads fine to me",
            "@@@ file: bb.txt",
            "@@ -1,3 +1,3 @@",
            " b1",
            "+B2",
            " b3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        proj,
        undo: 0,
    }
}

fn strs(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

// ── the fold ───────────────────────────────────────────────────────────────

#[test]
fn an_unedited_projection_folds_back_to_the_working_tree_content() {
    let d = doc();
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
    // Chrome and the ghost contribute nothing; the decoration is stripped.
    assert_eq!(d.proj.visible_len(), d.lines.len());
}

#[test]
fn deleting_an_added_line_drops_the_addition() {
    let mut d = doc();
    d.delete(5, 1); // dd on `+TWO`
    assert_eq!(strs(&d.folded()), ["one", "three"]);
}

#[test]
fn deleting_a_removed_line_reverts_the_deletion() {
    let mut d = doc();
    d.delete(4, 1); // dd on `-two` — the base line comes back
    assert_eq!(strs(&d.folded()), ["one", "two", "TWO", "three"]);
}

#[test]
fn deleting_a_context_line_removes_it_from_the_file() {
    let mut d = doc();
    d.delete(3, 1); // dd on ` one`
    assert_eq!(strs(&d.folded()), ["TWO", "three"]);
}

#[test]
fn retexting_an_added_line_changes_what_is_added() {
    let mut d = doc();
    d.splice(5, 1, &["+two point five"]);
    assert_eq!(strs(&d.folded()), ["one", "two point five", "three"]);
}

#[test]
fn typing_over_a_removed_line_makes_it_content() {
    let mut d = doc();
    // Typing `+…` over `-two`: the deletion stops being a deletion and the
    // typed text becomes file content.
    d.splice(4, 1, &["+two and a half"]);
    assert_eq!(strs(&d.folded()), ["one", "two and a half", "TWO", "three"]);
}

#[test]
fn retyping_a_context_line_as_a_deletion_deletes_it() {
    let mut d = doc();
    // Changing ` three` to `-three` is how one asks a diff to drop a line; the
    // decoration the user typed wins over the one the origin expected.
    d.splice(6, 1, &["-three"]);
    assert_eq!(strs(&d.folded()), ["one", "TWO"]);
}

#[test]
fn retyping_a_context_line_as_an_addition_keeps_its_new_text() {
    let mut d = doc();
    d.splice(6, 1, &["+three and a bit"]);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three and a bit"]);
}

#[test]
fn a_fresh_line_joins_the_span_it_was_typed_inside() {
    let mut d = doc();
    d.splice(6, 0, &["+inserted"]); // `O` above ` three`
    assert_eq!(strs(&d.folded()), ["one", "TWO", "inserted", "three"]);
}

#[test]
fn a_bare_line_typed_into_the_diff_is_taken_literally() {
    let mut d = doc();
    d.splice(6, 0, &["no marker here"]);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "no marker here", "three"]);
}

#[test]
fn a_multi_line_replacement_pairs_retexts_and_resolves_the_surplus() {
    let mut d = doc();
    // Visual-select the ghost, the addition and the trailing context, and type
    // two lines over them: two are retexts, the third is a surplus deletion.
    d.splice(4, 3, &["+alpha", "+beta"]);
    // `-two` was typed over (so it is content now) and `+TWO` retexted; ` three`
    // was deleted outright.
    assert_eq!(strs(&d.folded()), ["one", "alpha", "beta"]);
}

#[test]
fn editing_chrome_never_reaches_a_source() {
    let mut d = doc();
    d.splice(2, 1, &["@@ this header is now nonsense @@"]);
    d.splice(1, 1, &["not a file marker at all"]);
    // The old marker protocol aborted the save here. Provenance does not care.
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
}

#[test]
fn deleting_chrome_leaves_the_rest_of_the_span_intact() {
    let mut d = doc();
    d.delete(2, 1); // the hunk header, as a plain text deletion
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
}

// ── styles follow their lines ──────────────────────────────────────────────

#[test]
fn line_styles_track_insertions_instead_of_drifting() {
    let mut d = doc();
    assert_eq!(
        d.proj.line_styles(),
        ["title", "file", "hunk", "", "removed", "added", ""]
    );
    d.splice(3, 0, &["+new first line"]);
    // The `removed`/`added` bands moved down with their lines — the drift that
    // forced the old drawer to recompute styles from the text every frame.
    assert_eq!(
        d.proj.line_styles(),
        ["title", "file", "hunk", "", "", "removed", "added", ""]
    );
    assert_eq!(d.proj.line_styles().len(), d.lines.len());
}

// ── undo / redo ────────────────────────────────────────────────────────────

#[test]
fn undo_restores_the_table_not_just_the_text() {
    let mut d = doc();
    d.delete(4, 1); // revert the deletion
    assert_eq!(strs(&d.folded()), ["one", "two", "TWO", "three"]);

    // Undo the buffer edit, then the table.
    d.lines.insert(4, "-two".into());
    d.proj.sync_to(0);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
    // The ghost is a ghost again, not a line that happens to read "-two":
    // deleting it once more must revert the deletion, not drop an addition.
    d.delete(4, 1);
    assert_eq!(strs(&d.folded()), ["one", "two", "TWO", "three"]);
}

#[test]
fn redo_reapplies_the_table_change() {
    let mut d = doc();
    d.delete(5, 1);
    d.lines.insert(5, "+TWO".into());
    d.proj.sync_to(0);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
    d.lines.remove(5);
    d.proj.sync_to(1);
    assert_eq!(strs(&d.folded()), ["one", "three"]);
}

#[test]
fn edits_folded_into_one_undo_group_undo_together() {
    let mut d = doc();
    // Two splices at the same undo index — a change operator's delete plus the
    // insert session that follows it.
    d.proj.splice(5, 1, 0, 1);
    d.lines.remove(5);
    d.proj.splice(5, 0, 1, 1);
    d.lines.insert(5, "+replacement".into());
    assert_eq!(strs(&d.folded()), ["one", "replacement", "three"]);

    d.proj.sync_to(0);
    d.lines.splice(5..6, ["+TWO".to_string()]);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
}

// ── tier 2: intents ────────────────────────────────────────────────────────

#[test]
fn deleting_a_span_header_reverts_the_hunk() {
    let mut d = doc();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 2, count: 1 }, 1);
    d.apply(outcome);
    // The addition is gone, the deletion is back as context, and the file is
    // exactly what the base held.
    assert_eq!(strs(&d.folded()), ["one", "two", "three"]);
    assert_eq!(
        strs(&d.lines),
        [
            "unified: main → working tree",
            "@@@ file: a.txt",
            "@@ -1,3 +1,3 @@",
            " one",
            " two",
            " three",
        ]
    );
    assert_eq!(d.proj.visible_len(), d.lines.len());
    // The revived line reads as context now, not as a deletion.
    assert_eq!(d.proj.line_styles(), ["title", "file", "hunk", "", "", ""]);
}

#[test]
fn deleting_a_group_header_reverts_every_span_of_the_file() {
    let mut d = doc();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 1, count: 1 }, 1);
    d.apply(outcome);
    assert_eq!(strs(&d.folded()), ["one", "two", "three"]);
}

#[test]
fn a_reverted_hunk_can_be_undone() {
    let mut d = doc();
    let before = d.lines.clone();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 2, count: 1 }, 1);
    d.apply(outcome);
    assert_eq!(strs(&d.folded()), ["one", "two", "three"]);

    d.lines = before;
    d.proj.sync_to(0);
    assert_eq!(strs(&d.folded()), ["one", "TWO", "three"]);
}

#[test]
fn a_locked_line_refuses_the_delete() {
    let mut d = doc();
    match d.proj.intent(Intent::DeleteLines { start: 0, count: 1 }, 1) {
        Outcome::Refused(why) => assert!(why.contains("not the change")),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_ordinary_line_passes_to_the_plain_text_edit() {
    let mut d = doc();
    assert_eq!(
        d.proj.intent(Intent::DeleteLines { start: 5, count: 1 }, 1),
        Outcome::Pass
    );
    // …and a multi-line delete that merely starts on content passes too.
    assert_eq!(
        d.proj.intent(Intent::DeleteLines { start: 3, count: 4 }, 1),
        Outcome::Pass
    );
}

// ── which block a fresh line joins ─────────────────────────────────────────

#[test]
fn the_two_file_fixture_folds_back_to_each_working_tree() {
    let d = two_files();
    assert_eq!(strs(&d.fold(0)), ["a1", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["b1", "B2", "b3"]);
    assert_eq!(d.proj.visible_len(), d.lines.len());
}

#[test]
fn a_line_typed_under_a_group_header_joins_the_file_it_heads() {
    let mut d = two_files();
    d.splice(9, 0, &["+XY"]); // `o` on `@@@ file: bb.txt`
    assert_eq!(strs(&d.fold(0)), ["a1", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["XY", "b1", "B2", "b3"]);
}

#[test]
fn a_line_typed_under_a_span_header_joins_the_hunk_it_heads() {
    let mut d = two_files();
    d.splice(10, 0, &["+XY"]); // `o` on bb's `@@ -1,3 +1,3 @@`
    assert_eq!(strs(&d.fold(0)), ["a1", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["XY", "b1", "B2", "b3"]);
}

#[test]
fn a_line_typed_above_the_first_header_joins_the_first_span() {
    let mut d = two_files();
    d.splice(1, 0, &["+XY"]); // `o` on the locked title
    assert_eq!(strs(&d.fold(0)), ["XY", "a1", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["b1", "B2", "b3"]);
}

#[test]
fn a_line_typed_under_a_woven_comment_stays_in_the_block_around_it() {
    let mut d = two_files();
    // Plain chrome — a review comment — sits *inside* a file's block, so it
    // must not end the backward search the way a header does.
    d.splice(8, 0, &["+XY"]); // `o` on `## reads fine to me`
    assert_eq!(strs(&d.fold(0)), ["a1", "A2", "a3", "XY"]);
    assert_eq!(strs(&d.fold(1)), ["b1", "B2", "b3"]);
}

#[test]
fn a_line_typed_mid_block_still_joins_the_block_it_is_inside() {
    let mut d = two_files();
    d.splice(5, 0, &["+XY"]); // `O` above `+A2`
    assert_eq!(strs(&d.fold(0)), ["a1", "XY", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["b1", "B2", "b3"]);
}

// ── tier 2: chrome protection across a range ───────────────────────────────

#[test]
fn a_multi_line_delete_that_straddles_a_header_is_refused() {
    let mut d = two_files();
    // `3dd` on ` a3` — the run swallows bb's file header, which would leave
    // bb's hunks displayed under aa's heading.
    match d.proj.intent(Intent::DeleteLines { start: 6, count: 3 }, 1) {
        Outcome::Refused(why) => assert!(why.contains("not the change")),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_visual_range_spanning_a_file_boundary_is_refused() {
    let mut d = two_files();
    match d.proj.intent(
        Intent::DeleteLines {
            start: 3,
            count: 10,
        },
        1,
    ) {
        Outcome::Refused(why) => assert!(why.contains("not the change")),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_multi_line_delete_inside_one_hunk_still_passes() {
    let mut d = two_files();
    assert_eq!(
        d.proj.intent(Intent::DeleteLines { start: 3, count: 3 }, 1),
        Outcome::Pass
    );
}

#[test]
fn a_delete_that_starts_on_a_span_header_still_reverts_its_span() {
    let mut d = two_files();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 9, count: 1 }, 1);
    d.apply(outcome);
    assert_eq!(strs(&d.fold(0)), ["a1", "A2", "a3"]);
    assert_eq!(strs(&d.fold(1)), ["b1", "b3"]);
    assert_eq!(d.proj.visible_len(), d.lines.len());
}

// ── several sources ────────────────────────────────────────────────────────

#[test]
fn each_span_folds_into_its_own_source() {
    let decor = Decor {
        same: (" ".into(), String::new()),
        added: ("+".into(), "added".into()),
        removed: ("-".into(), "removed".into()),
        new_line: NewLine::DiffMarker,
        gutter: false,
    };
    let spans = vec![
        Span {
            source: 0,
            target: (0, 1),
            group: Some(0),
        },
        Span {
            source: 1,
            target: (4, 5),
            group: Some(1),
        },
    ];
    let mut proj = Projection::new(vec!["a.txt".into(), "b.txt".into()], spans, decor);
    proj.push(LineOrigin::Live { added: true }, "added", Some(0));
    proj.push(LineOrigin::Live { added: true }, "added", Some(1));
    let lines: Vec<String> = ["+from a", "+from b"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let edits = proj.resolve(&lines);
    assert_eq!(
        edits,
        vec![
            SourceEdit {
                source: "a.txt".into(),
                start: 0,
                end: 1,
                lines: vec!["from a".into()],
                expected: None,
            },
            SourceEdit {
                source: "b.txt".into(),
                start: 4,
                end: 5,
                lines: vec!["from b".into()],
                expected: None,
            },
        ]
    );
}

// ── dirty tracking: only the spans that actually changed ───────────────────

#[test]
fn a_freshly_captured_projection_resolves_to_no_edits_at_all() {
    let mut d = two_files();
    d.capture();
    // Nothing has been typed, so there is nothing to write — and in particular
    // no instruction to overwrite either file's hunk with this view's idea of it.
    assert_eq!(d.proj.resolve(&d.lines), vec![]);
}

#[test]
fn only_the_edited_span_is_emitted_and_it_carries_what_it_expects() {
    let mut d = two_files();
    d.capture();
    d.splice(11, 1, &["+BEE"]); // retext bb's addition

    let edits = d.proj.resolve(&d.lines);
    assert_eq!(edits.len(), 1, "aa.txt was untouched: {edits:?}");
    let edit = &edits[0];
    assert_eq!(edit.source, "bb.txt");
    assert_eq!((edit.start, edit.end), (0, 3));
    assert_eq!(strs(&edit.lines), ["b1", "BEE", "b3"]);
    // The expectation is the file as it was when the view loaded — not the new
    // content, and not the base side.
    assert_eq!(
        edit.expected.as_deref().map(strs),
        Some(vec!["b1", "B2", "b3"])
    );
}

#[test]
fn undoing_the_edit_returns_the_span_to_clean() {
    let mut d = two_files();
    d.capture();
    d.splice(11, 1, &["+BEE"]);
    assert_eq!(d.proj.resolve(&d.lines).len(), 1);

    // Undo, the way the buffer does it: the text goes back, then the table.
    d.lines[11] = "+B2".into();
    d.proj.sync_to(0);
    assert_eq!(d.proj.resolve(&d.lines), vec![]);
}

#[test]
fn a_structural_revert_counts_as_an_edit_to_its_span_only() {
    let mut d = two_files();
    d.capture();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 9, count: 1 }, 1);
    d.apply(outcome); // revert bb's hunk

    let edits = d.proj.resolve(&d.lines);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].source, "bb.txt");
    assert_eq!(strs(&edits[0].lines), ["b1", "b3"]);
}

#[test]
fn an_edit_that_lands_back_on_the_original_text_is_not_an_edit() {
    let mut d = doc();
    d.capture();
    d.splice(5, 1, &["+nope"]);
    assert_eq!(d.proj.resolve(&d.lines).len(), 1);
    // Typing it back by hand (no undo) leaves the file exactly as it was, so
    // there is nothing to write — the comparison is on content, not on history.
    d.splice(5, 1, &["+TWO"]);
    assert_eq!(d.proj.resolve(&d.lines), vec![]);
}

// ── Gutter markers ─────────────────────────────────────────────────────────

/// The same fixture as [`doc`], but with the diff markers in the **gutter**:
/// the buffer holds the file's own text, and `+`/`-`/space are display only.
///
/// ```text
///  0  unified: main → working tree     locked title      (no marker)
///  1  @@@ file: a.txt                  group header      (no marker)
///  2  @@ -1,3 +1,3 @@                  span header       (no marker)
///  3  one                              context           gutter " "
///  4  two                              deletion          gutter "-"
///  5  TWO                              addition          gutter "+"
///  6  three                            context           gutter " "
/// ```
fn gutter_doc() -> Doc {
    let mut d = doc();
    d.proj.decor.gutter = true;
    // The same lines, with the marker taken off the four content rows.
    d.lines = [
        "unified: main → working tree",
        "@@@ file: a.txt",
        "@@ -1,3 +1,3 @@",
        "one",
        "two",
        "TWO",
        "three",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    d
}

/// The baseline: an untouched gutter projection folds to exactly what the
/// prefixed one does. Same change, same write-back — only the buffer differs.
#[test]
fn a_gutter_projection_folds_the_same_as_a_prefixed_one() {
    assert_eq!(gutter_doc().folded(), ["one", "TWO", "three"]);
}

/// The bug this mode exists for. Joining two lines in a prefixed diff drags the
/// second line's `+` into the middle of the first — `J` over `+one`/`+two`
/// yields `+one +two`, and the fold writes a literal `+` into the file. With
/// the markers in the gutter there is no marker to drag: the join is a join.
#[test]
fn joining_lines_in_a_gutter_projection_never_drags_a_marker_in() {
    // Prefixed: vim's `J` over rows 5..6 produces "+TWO three" — and the fold
    // strips only the leading marker, leaving the *inner* one in the file.
    let mut prefixed = doc();
    prefixed.splice(5, 2, &["+TWO three"]);
    assert_eq!(
        prefixed.folded(),
        ["one", "TWO three"],
        "sanity: with the marker stripped the join still reads oddly at the seam"
    );

    // Gutter: the same join over the same two lines is just the two texts.
    let mut d = gutter_doc();
    d.splice(5, 2, &["TWO three"]);
    assert_eq!(d.folded(), ["one", "TWO three"]);
}

/// A line typed into a gutter projection is taken literally — including one
/// that starts with `+` or `-`, which in a prefixed diff would be read as a
/// diff verb and either eaten or silently turned into a deletion. Code that
/// legitimately starts with a sign is ordinary code here.
#[test]
fn a_typed_line_in_a_gutter_projection_is_literal() {
    let mut d = gutter_doc();
    d.splice(6, 0, &["-x + y", "+1"]);
    assert_eq!(d.folded(), ["one", "TWO", "-x + y", "+1", "three"]);

    // The prefixed projection is what this contrasts with: `-x + y` there means
    // "delete", so it contributes nothing at all.
    let mut prefixed = doc();
    prefixed.splice(6, 0, &["-x + y", "+1"]);
    assert_eq!(prefixed.folded(), ["one", "TWO", "1", "three"]);
}

/// The gutter track is parallel to the visible rows, carries a glyph only for
/// base-relative content, and follows its line through edits rather than
/// drifting off it.
#[test]
fn line_markers_track_the_visible_rows() {
    let mut d = gutter_doc();
    assert_eq!(d.proj.line_markers(), ["", "", "", " ", "-", "+", " "]);

    // Insert two lines above the hunk's content: the markers shift with them,
    // and the typed lines get no glyph of their own until they are folded.
    d.splice(3, 0, &["x", "y"]);
    assert_eq!(
        d.proj.line_markers(),
        ["", "", "", "", "", " ", "-", "+", " "]
    );

    // A prefixed projection publishes no gutter track at all — its markers are
    // already in the text, and a gutter would show them twice.
    assert!(doc().proj.line_markers().is_empty());
}

/// Reverting a hunk revives the deletion as context. In gutter mode the revived
/// line comes back as its own base text — no marker is prepended, because none
/// was ever there — and the gutter follows the changed origin on its own.
#[test]
fn reverting_a_gutter_hunk_revives_the_base_text_undecorated() {
    let mut d = gutter_doc();
    let outcome = d.proj.intent(Intent::DeleteLines { start: 2, count: 1 }, 1);
    d.apply(outcome);
    // Row 2 is the hunk header, which a revert keeps; the content follows it.
    assert_eq!(d.lines[3..], ["one", "two", "three"]);
    assert_eq!(d.proj.line_markers()[3..], [" ", " ", " "]);
    assert_eq!(d.folded(), ["one", "two", "three"], "the hunk is reverted");
}

/// Opening a line inside a gutter hunk starts it empty: there is no marker to
/// copy, so autoindent has nothing extra to inherit.
#[test]
fn a_gutter_projection_offers_no_decoration_for_a_new_line() {
    assert_eq!(gutter_doc().proj.new_line_decor(5), None);
    // The prefixed one still hands back the addition marker, as it must.
    assert_eq!(doc().proj.new_line_decor(5), Some("+"));
}

/// Nothing in a gutter line is decoration, so nothing has to be looked past to
/// find the line's real content.
#[test]
fn a_gutter_line_wears_no_decoration() {
    let d = gutter_doc();
    for row in 0..7 {
        assert_eq!(d.proj.worn_decor(row), "", "row {row} wears no marker");
    }
    assert_eq!(doc().proj.worn_decor(5), "+");
}
