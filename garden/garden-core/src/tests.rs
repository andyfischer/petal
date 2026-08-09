//! Unit tests for the garden-core text model.

use super::*;

fn buf(text: &str) -> Buffer {
    Buffer::from_str(text)
}

// --- construction & line accessors ---

#[test]
fn empty_buffer_has_one_empty_line() {
    let b = Buffer::new();
    assert_eq!(b.line_count(), 1);
    assert_eq!(b.line(0), "");
    assert_eq!(b.line_len(0), 0);
    assert!(!b.is_dirty());
    assert!(b.path().is_none());
}

#[test]
fn line_strips_trailing_newline() {
    let b = buf("alpha\nbeta\ngamma");
    assert_eq!(b.line_count(), 3);
    assert_eq!(b.line(0), "alpha");
    assert_eq!(b.line(1), "beta");
    assert_eq!(b.line(2), "gamma"); // last line, no trailing newline
    assert_eq!(b.line_len(0), 5);
    assert_eq!(b.line_len(2), 5);
}

#[test]
fn trailing_newline_yields_empty_last_line() {
    let b = buf("one\n");
    assert_eq!(b.line_count(), 2);
    assert_eq!(b.line(0), "one");
    assert_eq!(b.line(1), "");
    assert_eq!(b.line_len(1), 0);
}

#[test]
fn line_handles_crlf() {
    let b = buf("a\r\nb");
    assert_eq!(b.line(0), "a");
    assert_eq!(b.line_len(0), 1);
    assert_eq!(b.line(1), "b");
}

#[test]
fn line_len_counts_chars_not_bytes() {
    let b = buf("héllo\nwörld");
    assert_eq!(b.line_len(0), 5);
    assert_eq!(b.line_len(1), 5);
}

// --- clamp ---

#[test]
fn clamp_in_range_is_identity() {
    let b = buf("abc\ndefg");
    assert_eq!(b.clamp(Point::new(1, 2)), Point::new(1, 2));
}

#[test]
fn clamp_limits_line_and_col() {
    let b = buf("abc\nde");
    assert_eq!(b.clamp(Point::new(99, 0)), Point::new(1, 0));
    assert_eq!(b.clamp(Point::new(0, 99)), Point::new(0, 3));
    assert_eq!(b.clamp(Point::new(99, 99)), Point::new(1, 2));
}

#[test]
fn clamp_on_empty_buffer() {
    let b = Buffer::new();
    assert_eq!(b.clamp(Point::new(5, 5)), Point::new(0, 0));
}

#[test]
fn clamp_col_allows_end_of_line_before_newline() {
    let b = buf("ab\ncd");
    // col == line_len is valid (cursor after last char, before '\n')
    assert_eq!(b.clamp(Point::new(0, 2)), Point::new(0, 2));
}

// --- insert ---

#[test]
fn insert_within_line() {
    let mut b = buf("hello world");
    let p = b.insert(Point::new(0, 5), ",");
    assert_eq!(b.to_string(), "hello, world");
    assert_eq!(p, Point::new(0, 6));
    assert!(b.is_dirty());
}

#[test]
fn insert_newline_splits_line() {
    let mut b = buf("ab");
    let p = b.insert(Point::new(0, 1), "\n");
    assert_eq!(b.to_string(), "a\nb");
    assert_eq!(p, Point::new(1, 0));
    assert_eq!(b.line_count(), 2);
}

#[test]
fn insert_multiline_returns_position_after_text() {
    let mut b = buf("startend");
    let p = b.insert(Point::new(0, 5), "one\ntwo\nthree");
    assert_eq!(b.to_string(), "startone\ntwo\nthreeend");
    assert_eq!(p, Point::new(2, 5));
}

#[test]
fn insert_at_clamped_out_of_range_point() {
    let mut b = buf("ab");
    let p = b.insert(Point::new(9, 9), "!");
    assert_eq!(b.to_string(), "ab!");
    assert_eq!(p, Point::new(0, 3));
}

#[test]
fn insert_empty_is_noop() {
    let mut b = buf("ab");
    let p = b.insert(Point::new(0, 1), "");
    assert_eq!(b.to_string(), "ab");
    assert_eq!(p, Point::new(0, 1));
    assert!(!b.is_dirty());
    assert!(b.undo().is_none());
}

// --- delete ---

#[test]
fn delete_within_line() {
    let mut b = buf("hello world");
    let p = b.delete(Point::new(0, 5), Point::new(0, 11));
    assert_eq!(b.to_string(), "hello");
    assert_eq!(p, Point::new(0, 5));
    assert!(b.is_dirty());
}

#[test]
fn delete_across_line_boundary_joins_lines() {
    let mut b = buf("abc\ndef");
    let p = b.delete(Point::new(0, 2), Point::new(1, 1));
    assert_eq!(b.to_string(), "abef");
    assert_eq!(p, Point::new(0, 2));
    assert_eq!(b.line_count(), 1);
}

#[test]
fn delete_swapped_range() {
    let mut b = buf("abcdef");
    let p = b.delete(Point::new(0, 4), Point::new(0, 1));
    assert_eq!(b.to_string(), "aef");
    assert_eq!(p, Point::new(0, 1));
}

#[test]
fn delete_empty_range_is_noop() {
    let mut b = buf("abc");
    let p = b.delete(Point::new(0, 1), Point::new(0, 1));
    assert_eq!(b.to_string(), "abc");
    assert_eq!(p, Point::new(0, 1));
    assert!(!b.is_dirty());
    assert!(b.undo().is_none());
}

#[test]
fn delete_clamps_out_of_range_points() {
    let mut b = buf("abc\ndef");
    let p = b.delete(Point::new(1, 1), Point::new(99, 99));
    assert_eq!(b.to_string(), "abc\nd");
    assert_eq!(p, Point::new(1, 1));
}

// --- undo / redo ---

#[test]
fn undo_redo_insert_round_trip() {
    let mut b = buf("hello");
    b.insert(Point::new(0, 5), " world");
    assert_eq!(b.to_string(), "hello world");

    let p = b.undo().unwrap();
    assert_eq!(b.to_string(), "hello");
    assert_eq!(p, Point::new(0, 5));

    let p = b.redo().unwrap();
    assert_eq!(b.to_string(), "hello world");
    assert_eq!(p, Point::new(0, 11));
}

#[test]
fn undo_redo_delete_round_trip() {
    let mut b = buf("abc\ndef");
    b.delete(Point::new(0, 1), Point::new(1, 1));
    assert_eq!(b.to_string(), "aef");

    let p = b.undo().unwrap();
    assert_eq!(b.to_string(), "abc\ndef");
    assert_eq!(p, Point::new(1, 1)); // end of restored text

    let p = b.redo().unwrap();
    assert_eq!(b.to_string(), "aef");
    assert_eq!(p, Point::new(0, 1));
}

#[test]
fn undo_restores_the_pending_cursor() {
    // A join-like edit: replace two lines with one. Without a pending cursor,
    // undo lands at the end of the restored text (see the round-trip tests).
    let mut b = buf("abc\ndef");
    b.set_pending_cursor(Point::new(0, 2)); // where the caret sat before the edit
    b.replace(Point::new(0, 0), Point::new(1, 3), "abc def");
    assert_eq!(b.to_string(), "abc def");

    let p = b.undo().unwrap();
    assert_eq!(b.to_string(), "abc\ndef");
    assert_eq!(p, Point::new(0, 2)); // back where the edit started, not the end
}

#[test]
fn undo_run_restores_first_char_pending_cursor() {
    // A coalesced typing run keeps the pending cursor of its first character,
    // even though later characters update the pending cursor.
    let mut b = buf("");
    let mut p = Point::new(0, 0);
    for (i, ch) in ["a", "b", "c"].into_iter().enumerate() {
        b.set_pending_cursor(Point::new(0, i)); // caret before typing this char
        p = b.insert(p, ch);
    }
    assert_eq!(b.to_string(), "abc");

    let cursor = b.undo().unwrap();
    assert_eq!(b.to_string(), "");
    assert_eq!(cursor, Point::new(0, 0)); // start of the run, not (0, 2)
}

#[test]
fn undo_empty_stack_returns_none() {
    let mut b = buf("x");
    assert!(b.undo().is_none());
    assert!(b.redo().is_none());
}

#[test]
fn coalesces_consecutive_single_char_inserts() {
    let mut b = Buffer::new();
    let mut p = Point::new(0, 0);
    for ch in ["h", "e", "l", "l", "o"] {
        p = b.insert(p, ch);
    }
    assert_eq!(b.to_string(), "hello");

    // One undo removes the whole run.
    let cursor = b.undo().unwrap();
    assert_eq!(b.to_string(), "");
    assert_eq!(cursor, Point::new(0, 0));
    assert!(b.undo().is_none());

    // One redo restores it.
    let cursor = b.redo().unwrap();
    assert_eq!(b.to_string(), "hello");
    assert_eq!(cursor, Point::new(0, 5));
}

#[test]
fn newline_insert_does_not_coalesce() {
    let mut b = Buffer::new();
    let p = b.insert(Point::new(0, 0), "a");
    let p = b.insert(p, "\n");
    b.insert(p, "b");
    assert_eq!(b.to_string(), "a\nb");

    b.undo().unwrap();
    assert_eq!(b.to_string(), "a\n");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "a");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "");
}

#[test]
fn multichar_insert_does_not_coalesce() {
    let mut b = Buffer::new();
    let p = b.insert(Point::new(0, 0), "ab");
    b.insert(p, "c");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "ab");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "");
}

#[test]
fn non_adjacent_insert_starts_new_transaction() {
    let mut b = buf("xyz");
    b.insert(Point::new(0, 3), "a"); // at end
    b.insert(Point::new(0, 0), "b"); // jump to start
    assert_eq!(b.to_string(), "bxyza");

    b.undo().unwrap();
    assert_eq!(b.to_string(), "xyza");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "xyz");
}

#[test]
fn delete_breaks_insert_coalescing() {
    let mut b = Buffer::new();
    let p = b.insert(Point::new(0, 0), "a");
    let p = b.insert(p, "b");
    b.delete(Point::new(0, 1), p); // delete the 'b'
    b.insert(Point::new(0, 1), "c");
    assert_eq!(b.to_string(), "ac");

    b.undo().unwrap();
    assert_eq!(b.to_string(), "a"); // undo insert 'c'
    b.undo().unwrap();
    assert_eq!(b.to_string(), "ab"); // undo delete
    b.undo().unwrap();
    assert_eq!(b.to_string(), ""); // undo coalesced "ab"
}

#[test]
fn new_edit_clears_redo_stack() {
    let mut b = Buffer::new();
    let p = b.insert(Point::new(0, 0), "first");
    b.insert(p, "second");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "first");

    b.insert(Point::new(0, 5), "X");
    assert_eq!(b.to_string(), "firstX");
    assert!(b.redo().is_none());

    b.undo().unwrap();
    assert_eq!(b.to_string(), "first");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "");
    assert!(b.undo().is_none());
}

#[test]
fn insert_after_undo_does_not_coalesce_with_redone_run() {
    let mut b = Buffer::new();
    let p = b.insert(Point::new(0, 0), "a");
    b.insert(p, "b");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "");
    b.redo().unwrap();
    assert_eq!(b.to_string(), "ab");

    // Typing after an undo/redo cycle starts a fresh transaction.
    b.insert(Point::new(0, 2), "c");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "ab");
}

#[test]
fn interleaved_undo_redo_sequence() {
    let mut b = Buffer::new();
    b.insert(Point::new(0, 0), "one\n");
    b.insert(Point::new(1, 0), "two\n");
    b.delete(Point::new(0, 0), Point::new(1, 0));
    assert_eq!(b.to_string(), "two\n");

    b.undo().unwrap();
    assert_eq!(b.to_string(), "one\ntwo\n");
    b.undo().unwrap();
    assert_eq!(b.to_string(), "one\n");
    b.redo().unwrap();
    assert_eq!(b.to_string(), "one\ntwo\n");
    b.redo().unwrap();
    assert_eq!(b.to_string(), "two\n");
    assert!(b.redo().is_none());
}

#[test]
fn undo_multiline_insert() {
    let mut b = buf("AB");
    b.insert(Point::new(0, 1), "1\n2\n3");
    assert_eq!(b.to_string(), "A1\n2\n3B");
    let p = b.undo().unwrap();
    assert_eq!(b.to_string(), "AB");
    assert_eq!(p, Point::new(0, 1));
}

// --- replace ---

#[test]
fn replace_swaps_text_and_returns_end_of_insert() {
    let mut b = buf("hello world");
    let p = b.replace(Point::new(0, 0), Point::new(0, 5), "goodbye");
    assert_eq!(b.to_string(), "goodbye world");
    assert_eq!(p, Point::new(0, 7));
}

#[test]
fn replace_undoes_as_one_transaction() {
    let mut b = buf("abc\ndef");
    b.replace(Point::new(0, 2), Point::new(1, 1), "XY");
    assert_eq!(b.to_string(), "abXYef");

    let p = b.undo().unwrap();
    assert_eq!(b.to_string(), "abc\ndef");
    assert_eq!(p, Point::new(1, 1));
    assert!(b.undo().is_none());

    b.redo().unwrap();
    assert_eq!(b.to_string(), "abXYef");
}

#[test]
fn replace_with_empty_text_is_a_delete() {
    let mut b = buf("abcdef");
    let p = b.replace(Point::new(0, 4), Point::new(0, 1), ""); // reversed range
    assert_eq!(b.to_string(), "aef");
    assert_eq!(p, Point::new(0, 1));
    b.undo().unwrap();
    assert_eq!(b.to_string(), "abcdef");
}

#[test]
fn replace_empty_range_and_text_is_noop() {
    let mut b = buf("abc");
    let p = b.replace(Point::new(0, 1), Point::new(0, 1), "");
    assert_eq!(b.to_string(), "abc");
    assert_eq!(p, Point::new(0, 1));
    assert!(!b.is_dirty());
    assert!(b.undo().is_none());
}

// --- text_range ---

#[test]
fn text_range_within_line() {
    let b = buf("hello world");
    assert_eq!(b.text_range(Point::new(0, 6), Point::new(0, 11)), "world");
}

#[test]
fn text_range_across_lines_includes_newlines() {
    let b = buf("abc\ndef\nghi");
    assert_eq!(
        b.text_range(Point::new(0, 2), Point::new(2, 1)),
        "c\ndef\ng"
    );
}

#[test]
fn text_range_swapped_and_clamped() {
    let b = buf("abc\nde");
    assert_eq!(b.text_range(Point::new(99, 99), Point::new(0, 1)), "bc\nde");
    assert_eq!(b.text_range(Point::new(0, 1), Point::new(0, 1)), "");
}

// --- Selection ---

#[test]
fn selection_ordered_swaps_reversed_ends() {
    let s = Selection::new(Point::new(2, 1), Point::new(0, 4));
    assert_eq!(s.ordered(), (Point::new(0, 4), Point::new(2, 1)));
    assert!(!s.is_empty());

    let fwd = Selection::new(Point::new(0, 1), Point::new(0, 5));
    assert_eq!(fwd.ordered(), (Point::new(0, 1), Point::new(0, 5)));
}

#[test]
fn selection_empty_has_no_line_cols() {
    let s = Selection::new(Point::new(1, 3), Point::new(1, 3));
    assert!(s.is_empty());
    assert_eq!(s.cols_on_line(1, 10), None);
}

#[test]
fn selection_cols_single_line() {
    let s = Selection::new(Point::new(0, 2), Point::new(0, 5));
    assert_eq!(s.cols_on_line(0, 10), Some((2, 5, false)));
    assert_eq!(s.cols_on_line(1, 10), None);
}

#[test]
fn selection_cols_multi_line() {
    // Selection from (0,2) to (2,3): start line, interior line, end line.
    let s = Selection::new(Point::new(2, 3), Point::new(0, 2)); // reversed
    assert_eq!(s.cols_on_line(0, 5), Some((2, 5, true)));
    assert_eq!(s.cols_on_line(1, 4), Some((0, 4, true)));
    assert_eq!(s.cols_on_line(2, 8), Some((0, 3, false)));
    assert_eq!(s.cols_on_line(3, 8), None);
}

#[test]
fn selection_cols_clamp_to_line_len() {
    let s = Selection::new(Point::new(0, 2), Point::new(0, 99));
    assert_eq!(s.cols_on_line(0, 5), Some((2, 5, false)));
}

// --- open / save ---

#[test]
fn open_save_round_trip() {
    let dir = std::env::temp_dir().join(format!("garden-core-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.txt");
    fs::write(&path, "line one\nline two\n").unwrap();

    let mut b = Buffer::open(&path).unwrap();
    assert_eq!(b.path(), Some(path.as_path()));
    assert!(!b.is_dirty());
    assert_eq!(b.line(0), "line one");
    assert_eq!(b.line(1), "line two");

    b.insert(Point::new(1, 8), "!");
    assert!(b.is_dirty());

    b.save().unwrap();
    assert!(!b.is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "line one\nline two!\n");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn open_invalid_utf8_is_lossy() {
    let dir = std::env::temp_dir().join(format!("garden-core-test-utf8-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.txt");
    fs::write(&path, b"ok\xFF\xFEok").unwrap();

    let b = Buffer::open(&path).unwrap();
    assert_eq!(b.to_string(), "ok\u{FFFD}\u{FFFD}ok");

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_without_path_errors() {
    let mut b = buf("text");
    let err = b.save().unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn open_missing_file_errors() {
    let path = std::env::temp_dir().join("garden-core-test-does-not-exist.txt");
    assert!(Buffer::open(&path).is_err());
}

#[test]
fn undo_after_save_marks_dirty() {
    let dir = std::env::temp_dir().join(format!("garden-core-test-dirty-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dirty.txt");
    fs::write(&path, "abc").unwrap();

    let mut b = Buffer::open(&path).unwrap();
    b.insert(Point::new(0, 3), "d");
    b.save().unwrap();
    assert!(!b.is_dirty());
    b.undo().unwrap();
    assert!(b.is_dirty());

    fs::remove_dir_all(&dir).unwrap();
}

// --- precise dirty tracking (dirtiness derived from the saved revision) ---

/// Make a temp file containing `contents` and open it; returns the buffer
/// and the directory to remove when done.
fn open_temp(tag: &str, contents: &str) -> (Buffer, PathBuf) {
    let dir = std::env::temp_dir().join(format!("garden-core-test-{}-{}", tag, std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("file.txt");
    fs::write(&path, contents).unwrap();
    (Buffer::open(&path).unwrap(), dir)
}

#[test]
fn undo_redo_land_clean_exactly_at_save_point() {
    let (mut b, dir) = open_temp("save-point", "x");
    b.insert(Point::new(0, 1), "1\n"); // txn 1 (newline: not coalescible)
    b.insert(Point::new(1, 0), "2\n"); // txn 2
    b.save().unwrap();
    assert!(!b.is_dirty());

    b.insert(Point::new(2, 0), "3\n"); // txn 3, past the save point
    assert!(b.is_dirty());

    b.undo().unwrap(); // back at the saved revision
    assert!(!b.is_dirty());
    b.undo().unwrap(); // before the saved revision
    assert!(b.is_dirty());
    b.redo().unwrap(); // forward onto the saved revision
    assert!(!b.is_dirty());
    b.redo().unwrap(); // past it again
    assert!(b.is_dirty());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unsaved_buffer_undone_to_initial_content_is_clean() {
    let mut b = buf("seed");
    assert!(!b.is_dirty());
    b.insert(Point::new(0, 4), "!");
    assert!(b.is_dirty());
    b.undo().unwrap();
    assert!(!b.is_dirty());
    b.redo().unwrap();
    assert!(b.is_dirty());
}

#[test]
fn edit_after_undo_past_save_invalidates_saved_revision() {
    let (mut b, dir) = open_temp("truncate", "base");
    b.insert(Point::new(0, 4), "1");
    b.save().unwrap(); // saved revision is at undo index 1
    b.undo().unwrap(); // back to "base"
    assert!(b.is_dirty());

    // New edit truncates the redo history that held the saved revision.
    b.insert(Point::new(0, 4), "2");
    assert!(b.is_dirty());

    // The saved revision was thrown away: no undo position can be clean now.
    b.undo().unwrap();
    assert_eq!(b.to_string(), "base");
    assert!(b.is_dirty());
    assert!(b.undo().is_none());
    assert!(b.is_dirty());
    b.redo().unwrap();
    assert!(b.is_dirty());

    // Only the next save makes it clean again.
    b.save().unwrap();
    assert!(!b.is_dirty());

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_breaks_coalescing_so_chars_typed_after_save_are_dirty() {
    let (mut b, dir) = open_temp("coalesce-save", "");
    let p = b.insert(Point::new(0, 0), "a");
    let p = b.insert(p, "b"); // coalesces with 'a'
    b.save().unwrap();
    assert!(!b.is_dirty());

    // Without a transaction boundary at the save, this would extend the
    // "ab" run, leaving undo_index unchanged and falsely reporting clean.
    b.insert(p, "c");
    assert!(b.is_dirty());

    // Undo must land on the saved text, not wipe the whole run.
    b.undo().unwrap();
    assert_eq!(b.to_string(), "ab");
    assert!(!b.is_dirty());

    fs::remove_dir_all(&dir).unwrap();
}

// --- external change detection & reload --------------------------------------

#[test]
fn fresh_buffer_reports_no_disk_change() {
    let (b, dir) = open_temp("disk-fresh", "hello\n");
    assert!(b.disk_changed().is_none()); // just opened: in sync
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pathless_buffer_never_reports_disk_change() {
    let b = buf("scratch");
    assert!(b.disk_changed().is_none());
}

#[test]
fn external_write_is_detected() {
    let (b, dir) = open_temp("disk-detect", "hello\n");
    fs::write(dir.join("file.txt"), "goodbye world\n").unwrap(); // different length
    assert!(b.disk_changed().is_some());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reload_replaces_content_and_resets_undo() {
    let (mut b, dir) = open_temp("disk-reload", "one\ntwo\n");
    b.insert(Point::new(0, 0), "x"); // make it dirty with undo history
    fs::write(dir.join("file.txt"), "fresh\n").unwrap();

    b.reload().unwrap();
    assert_eq!(b.to_string(), "fresh\n");
    assert!(!b.is_dirty()); // reload lands on a clean, saved revision
    assert!(b.undo().is_none()); // undo history was cleared
    assert!(b.disk_changed().is_none()); // now in sync with disk again
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn save_resyncs_so_no_change_is_reported_afterward() {
    let (mut b, dir) = open_temp("disk-save", "seed\n");
    b.insert(Point::new(0, 4), "!");
    b.save().unwrap();
    assert!(b.disk_changed().is_none()); // our own save is not an external change
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reload_without_path_errors() {
    let mut b = buf("scratch");
    assert!(b.reload().is_err());
}

#[test]
fn revision_advances_on_every_mutation_and_never_repeats_across_branches() {
    let mut b = buf("ab");
    let r0 = b.revision();
    b.insert(Point::new(0, 2), "c"); // "abc"
    let r1 = b.revision();
    assert!(r1 > r0, "insert bumps revision");

    b.undo(); // back to "ab"
    let r2 = b.revision();
    assert!(r2 > r1, "undo bumps revision");

    // Editing after the undo produces different content than the earlier
    // same-undo_index state; the revision must not collide with it.
    b.insert(Point::new(0, 2), "d"); // "abd"
    let r3 = b.revision();
    assert!(r3 > r2 && r3 != r1, "post-undo edit gets a fresh revision");
}
