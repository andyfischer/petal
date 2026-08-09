//! Plain-text buffer search: the pure match-finding core behind `/`, `?`,
//! `n`/`N`, `*`, and the viewport match highlights.
//!
//! Patterns are case-sensitive single-line substrings (no regex). All columns
//! are **char offsets** (matching [`Point::col`]), never byte offsets, so
//! non-ASCII text searches correctly.

use std::borrow::Cow;
use std::ops::Range;

use garden_core::{Buffer, Point};

/// Smartcase (vim's `ignorecase` + `smartcase`): an all-lowercase pattern
/// matches case-insensitively, while any uppercase ASCII letter makes the
/// whole pattern case-sensitive. Folding is ASCII-only, so multi-byte chars
/// (and the char offsets that index them) are preserved exactly.
fn case_insensitive(pattern: &str) -> bool {
    !pattern.bytes().any(|b| b.is_ascii_uppercase())
}

/// `s` lowercased for case-insensitive comparison, or borrowed unchanged.
/// `to_ascii_lowercase` is length- and position-preserving, so columns
/// computed on the folded string are valid columns in the original.
fn fold(s: &str, insensitive: bool) -> Cow<'_, str> {
    if insensitive {
        Cow::Owned(s.to_ascii_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// A word character for `whole_word` boundaries: the same class vim's word
/// motions use (see `vim::char_class`) — alphanumeric or `_`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether the match of `len` chars starting at char-col `col` of `line` sits
/// on word boundaries: the neighboring chars (where present) are not word
/// characters. This is vim's `\<pat\>` for the `*`/`#` searches.
fn on_word_boundary(line: &str, col: usize, len: usize) -> bool {
    let before = col.checked_sub(1).and_then(|i| line.chars().nth(i));
    let after = line.chars().nth(col + len);
    !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
}

/// Char-offset start columns of every match of `pattern` in `line`
/// (overlapping matches included). Empty patterns match nothing. Matching is
/// smartcase (see [`case_insensitive`]); with `whole_word` only matches on
/// word boundaries count (vim's `\<pat\>`, used by `*`/`#`).
pub fn matches_on_line(line: &str, pattern: &str, whole_word: bool) -> Vec<usize> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let ci = case_insensitive(pattern);
    let folded = fold(line, ci);
    let pattern_folded = fold(pattern, ci);
    let pat = pattern_folded.as_bytes();
    let len = pattern.chars().count();
    folded
        .char_indices()
        .enumerate()
        .filter(|&(_, (byte, _))| folded.as_bytes()[byte..].starts_with(pat))
        .map(|(col, _)| col)
        // Boundary chars are checked on the original line; ASCII folding is
        // position-preserving, so the columns line up.
        .filter(|&col| !whole_word || on_word_boundary(line, col, len))
        .collect()
}

/// The first match strictly after (`forward`) or strictly before (backward)
/// `from`, wrapping around the buffer. Returns `None` only when the pattern
/// occurs nowhere (a lone match at `from` itself is found again by wrapping).
pub fn find_next(
    buffer: &Buffer,
    from: Point,
    pattern: &str,
    forward: bool,
    whole_word: bool,
) -> Option<Point> {
    if pattern.is_empty() {
        return None;
    }
    let last = buffer.line_count().saturating_sub(1);
    let from = buffer.clamp(from);

    // Visit the cursor's line twice: once for the unwrapped side of the
    // cursor, then every other line in order, then the wrapped side.
    let cols = |line: usize| matches_on_line(&buffer.line(line), pattern, whole_word);
    if forward {
        if let Some(&col) = cols(from.line).iter().find(|&&c| c > from.col) {
            return Some(Point::new(from.line, col));
        }
        for line in (from.line + 1..=last).chain(0..from.line) {
            if let Some(&col) = cols(line).first() {
                return Some(Point::new(line, col));
            }
        }
        cols(from.line)
            .first()
            .map(|&col| Point::new(from.line, col))
    } else {
        if let Some(&col) = cols(from.line).iter().rev().find(|&&c| c < from.col) {
            return Some(Point::new(from.line, col));
        }
        for line in (0..from.line).rev().chain((from.line + 1..=last).rev()) {
            if let Some(&col) = cols(line).last() {
                return Some(Point::new(line, col));
            }
        }
        cols(from.line)
            .last()
            .map(|&col| Point::new(from.line, col))
    }
}

/// All matches within the line range (for viewport highlighting), as
/// `(start, char_len)` pairs in document order.
pub fn matches_in_lines(
    buffer: &Buffer,
    lines: Range<usize>,
    pattern: &str,
    whole_word: bool,
) -> Vec<(Point, usize)> {
    let len = pattern.chars().count();
    if len == 0 {
        return Vec::new();
    }
    let end = lines.end.min(buffer.line_count());
    (lines.start..end)
        .flat_map(|line| {
            matches_on_line(&buffer.line(line), pattern, whole_word)
                .into_iter()
                .map(move |col| (Point::new(line, col), len))
        })
        .collect()
}

/// Replace occurrences of `pattern` with `replacement` in `line`, returning
/// the new line and the number of replacements made. With `global`, every
/// non-overlapping occurrence (left to right) is replaced; otherwise only the
/// first. With `ignore_case` the match is case-insensitive (the `:s` `i` flag;
/// ASCII-only folding, like search) — the default is case-sensitive, unlike
/// search's smartcase. An empty pattern matches nothing. The scan resumes past
/// each replacement, so a replacement that contains the pattern is not
/// re-matched. Plain text only — no regex, no escape sequences in
/// pattern/replacement.
pub fn substitute_line(
    line: &str,
    pattern: &str,
    replacement: &str,
    global: bool,
    ignore_case: bool,
) -> (String, usize) {
    if pattern.is_empty() {
        return (line.to_string(), 0);
    }
    // Match against folded copies; copy output chars from the original.
    // ASCII folding preserves lengths, so byte offsets agree between the two.
    let folded_line = fold(line, ignore_case);
    let folded_pattern = fold(pattern, ignore_case);
    let pat = folded_pattern.as_bytes();
    let bytes = folded_line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut count = 0;
    let mut i = 0; // byte offset, always on a char boundary
    while i < bytes.len() {
        if (global || count == 0) && bytes[i..].starts_with(pat) {
            out.push_str(replacement);
            i += pat.len(); // pattern is whole chars, so this stays on a boundary
            count += 1;
        } else {
            let ch = line[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_on_line_finds_all_starts() {
        assert_eq!(matches_on_line("foo bar foo", "foo", false), vec![0, 8]);
    }

    #[test]
    fn matches_on_line_finds_overlapping_matches() {
        assert_eq!(matches_on_line("aaa", "aa", false), vec![0, 1]);
    }

    #[test]
    fn matches_on_line_empty_pattern_matches_nothing() {
        assert_eq!(matches_on_line("abc", "", false), Vec::<usize>::new());
    }

    #[test]
    fn smartcase_lowercase_pattern_matches_any_case() {
        // An all-lowercase pattern is case-insensitive (vim smartcase).
        assert_eq!(matches_on_line("Foo foo FOO", "foo", false), vec![0, 4, 8]);
    }

    #[test]
    fn smartcase_pattern_with_uppercase_is_case_sensitive() {
        assert_eq!(matches_on_line("Foo foo FOO", "Foo", false), vec![0]);
    }

    #[test]
    fn smartcase_folding_preserves_non_ascii_columns() {
        // ASCII-only folding leaves multi-byte chars (and their columns) intact.
        assert_eq!(matches_on_line("héLLo HÉllo", "héllo", false), vec![0]);
    }

    #[test]
    fn smartcase_applies_to_find_next() {
        let buf = Buffer::from_str("ALPHA\nbravo\nAlpha");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "alpha", true, false),
            Some(Point::new(2, 0))
        );
    }

    #[test]
    fn smartcase_applies_to_viewport_matches() {
        let buf = Buffer::from_str("Foo foo");
        assert_eq!(
            matches_in_lines(&buf, 0..1, "foo", false),
            vec![(Point::new(0, 0), 3), (Point::new(0, 4), 3)]
        );
    }

    #[test]
    fn matches_on_line_uses_char_columns_not_bytes() {
        // 'é' and 'ö' are multi-byte; columns must still be char offsets.
        assert_eq!(matches_on_line("héllo wörld", "wörld", false), vec![6]);
        assert_eq!(matches_on_line("ééxéé", "éé", false), vec![0, 3]);
    }

    #[test]
    fn whole_word_rejects_embedded_matches() {
        // "foo" inside "foobar"/"xfoo" is not a whole word; the lone one is.
        assert_eq!(matches_on_line("foobar foo xfoo", "foo", true), vec![7]);
        // Without the flag all three match.
        assert_eq!(
            matches_on_line("foobar foo xfoo", "foo", false),
            vec![0, 7, 12]
        );
    }

    #[test]
    fn whole_word_boundaries_are_vim_word_chars() {
        // `_` and digits are word chars (no boundary); punctuation is one.
        assert_eq!(matches_on_line("_foo foo1 (foo)", "foo", true), vec![11]);
    }

    #[test]
    fn whole_word_at_line_edges_matches() {
        assert_eq!(matches_on_line("foo bar foo", "foo", true), vec![0, 8]);
    }

    #[test]
    fn whole_word_applies_to_find_next_and_viewport() {
        let buf = Buffer::from_str("foobar\nfoo");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "foo", true, true),
            Some(Point::new(1, 0))
        );
        assert_eq!(
            matches_in_lines(&buf, 0..2, "foo", true),
            vec![(Point::new(1, 0), 3)]
        );
    }

    #[test]
    fn whole_word_smartcase_folding_keeps_boundaries() {
        // Case-insensitive match; the boundary check still sees the original.
        assert_eq!(matches_on_line("Foo Foox", "foo", true), vec![0]);
    }

    #[test]
    fn find_next_forward_is_strictly_after_cursor() {
        let buf = Buffer::from_str("foo bar foo");
        // Sitting on the first match: the next one is at col 8.
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "foo", true, false),
            Some(Point::new(0, 8))
        );
    }

    #[test]
    fn find_next_forward_crosses_lines() {
        let buf = Buffer::from_str("alpha\nbravo\ncharlie");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "bravo", true, false),
            Some(Point::new(1, 0))
        );
    }

    #[test]
    fn find_next_forward_wraps_past_eof() {
        let buf = Buffer::from_str("target\nmiddle\nend");
        assert_eq!(
            find_next(&buf, Point::new(2, 0), "target", true, false),
            Some(Point::new(0, 0))
        );
    }

    #[test]
    fn find_next_forward_wrap_can_land_on_cursor() {
        // The only match is at the cursor itself: wrapping finds it again.
        let buf = Buffer::from_str("only\nplain");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "only", true, false),
            Some(Point::new(0, 0))
        );
    }

    #[test]
    fn find_next_backward_is_strictly_before_cursor() {
        let buf = Buffer::from_str("foo bar foo");
        assert_eq!(
            find_next(&buf, Point::new(0, 8), "foo", false, false),
            Some(Point::new(0, 0))
        );
    }

    #[test]
    fn find_next_backward_takes_the_last_match_of_a_line() {
        let buf = Buffer::from_str("foo foo foo\nx");
        assert_eq!(
            find_next(&buf, Point::new(1, 0), "foo", false, false),
            Some(Point::new(0, 8))
        );
    }

    #[test]
    fn find_next_backward_wraps_past_start() {
        let buf = Buffer::from_str("start\nmiddle\ntarget");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "target", false, false),
            Some(Point::new(2, 0))
        );
    }

    #[test]
    fn find_next_not_found_returns_none() {
        let buf = Buffer::from_str("alpha\nbravo");
        assert_eq!(find_next(&buf, Point::new(0, 0), "zzz", true, false), None);
        assert_eq!(find_next(&buf, Point::new(0, 0), "zzz", false, false), None);
        assert_eq!(find_next(&buf, Point::new(0, 0), "", true, false), None);
    }

    #[test]
    fn find_next_handles_non_ascii_lines() {
        let buf = Buffer::from_str("naïve test\nüber naïve");
        assert_eq!(
            find_next(&buf, Point::new(0, 0), "naïve", true, false),
            Some(Point::new(1, 5))
        );
        assert_eq!(
            find_next(&buf, Point::new(1, 5), "naïve", false, false),
            Some(Point::new(0, 0))
        );
    }

    #[test]
    fn matches_in_lines_enumerates_range_with_char_lens() {
        let buf = Buffer::from_str("foo\nbar foo\nfoo");
        assert_eq!(
            matches_in_lines(&buf, 0..2, "foo", false),
            vec![(Point::new(0, 0), 3), (Point::new(1, 4), 3)]
        );
    }

    #[test]
    fn matches_in_lines_clamps_to_buffer_end() {
        let buf = Buffer::from_str("foo");
        assert_eq!(
            matches_in_lines(&buf, 0..50, "foo", false),
            vec![(Point::new(0, 0), 3)]
        );
    }

    #[test]
    fn matches_in_lines_reports_char_len_for_non_ascii_pattern() {
        let buf = Buffer::from_str("xééx");
        assert_eq!(
            matches_in_lines(&buf, 0..1, "éé", false),
            vec![(Point::new(0, 1), 2)]
        );
    }

    #[test]
    fn substitute_line_replaces_only_the_first_when_not_global() {
        assert_eq!(
            substitute_line("foo foo foo", "foo", "bar", false, false),
            ("bar foo foo".to_string(), 1)
        );
    }

    #[test]
    fn substitute_line_replaces_all_when_global() {
        assert_eq!(
            substitute_line("foo foo foo", "foo", "bar", true, false),
            ("bar bar bar".to_string(), 3)
        );
    }

    #[test]
    fn substitute_line_counts_zero_when_absent() {
        assert_eq!(
            substitute_line("hello", "zzz", "x", true, false),
            ("hello".to_string(), 0)
        );
    }

    #[test]
    fn substitute_line_empty_replacement_deletes() {
        assert_eq!(
            substitute_line("a-b-c", "-", "", true, false),
            ("abc".to_string(), 2)
        );
    }

    #[test]
    fn substitute_line_empty_pattern_is_a_noop() {
        assert_eq!(
            substitute_line("abc", "", "x", true, false),
            ("abc".to_string(), 0)
        );
    }

    #[test]
    fn substitute_line_is_non_overlapping() {
        // "aa" in "aaaa" replaces at 0 and 2, not 0,1,2,3.
        assert_eq!(
            substitute_line("aaaa", "aa", "b", true, false),
            ("bb".to_string(), 2)
        );
    }

    #[test]
    fn substitute_line_handles_non_ascii() {
        assert_eq!(
            substitute_line("naïve naïve", "naïve", "wis€", true, false),
            ("wis€ wis€".to_string(), 2)
        );
    }

    #[test]
    fn substitute_line_replacement_can_contain_the_pattern() {
        // The scan jumps past each replacement, so it does not re-match inside it.
        assert_eq!(
            substitute_line("cat", "cat", "cats", true, false),
            ("cats".to_string(), 1)
        );
    }

    #[test]
    fn substitute_line_is_case_sensitive_by_default() {
        assert_eq!(
            substitute_line("Foo foo", "foo", "bar", true, false),
            ("Foo bar".to_string(), 1)
        );
    }

    #[test]
    fn substitute_line_ignore_case_matches_any_case() {
        assert_eq!(
            substitute_line("Foo FOO foo", "foo", "bar", true, true),
            ("bar bar bar".to_string(), 3)
        );
        // Non-matched chars keep their original case.
        assert_eq!(
            substitute_line("XFooX", "foo", "y", true, true),
            ("XyX".to_string(), 1)
        );
    }
}
