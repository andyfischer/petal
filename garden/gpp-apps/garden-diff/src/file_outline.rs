//! Grouping the diff's flat file list into the hierarchical outline the drawer's
//! stat view renders.
//!
//! A real diff's paths repeat their directories dozens of times
//! (`components/context_hub_transformers/src/…` on every row), which crowds out
//! the part that differs. This module factors those shared prefixes out: files
//! sharing a directory get one **dir** row naming the prefix, followed by
//! **file** rows carrying only the remainder, indented under it. Prefix chains
//! collapse into a single row (`a/b/c/` rather than three nested ones), and a
//! directory holding just one file stays inline — a header for one row buys
//! nothing.
//!
//! All of the shaping happens here so the drawer only walks a flat list of
//! pre-laid-out rows (see [`Row`]) and draws each at its `depth`.

use crate::diff_core::FileSummary;

/// Whether a [`Row`] names a shared directory prefix or one changed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Dir,
    File,
}

/// One line of the outline: a directory prefix, or a file under one.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    /// Indent level — 0 for a top-level row, +1 under each dir row.
    pub depth: usize,
    /// What to print: the shared prefix (trailing `/`) for a dir row, or the
    /// file's path with its enclosing dir rows' prefix stripped off.
    pub label: String,
    /// Added / removed line counts — the file's own, or the sum beneath a dir.
    pub added: u32,
    pub removed: u32,
    /// Set on a file row that git reported as binary; never on a dir row.
    pub binary: bool,
    /// Index into the `files` list this row came from; `None` for a dir row.
    pub file: Option<usize>,
    /// Where each projection must scroll to bring this row into view — the
    /// file's own marker lines, or (for a dir row) its first file's, so clicking
    /// a heading lands at the top of that subtree.
    pub line_before: usize,
    pub line_after: usize,
    pub line_unified: usize,
}

/// One file, pre-split into path segments, plus where it sits in `files`.
struct Entry {
    segments: Vec<String>,
    index: usize,
}

/// Group `files` into the outline, in path order.
pub fn build(files: &[FileSummary]) -> Vec<Row> {
    let mut entries: Vec<Entry> = files
        .iter()
        .enumerate()
        .map(|(index, f)| Entry {
            segments: f.path.split('/').map(str::to_string).collect(),
            index,
        })
        .collect();
    // Sorting by segments (not by the raw string) keeps a directory's contents
    // contiguous even when a sibling file sorts between them textually.
    entries.sort_by(|a, b| a.segments.cmp(&b.segments));
    let refs: Vec<&Entry> = entries.iter().collect();
    let mut out = Vec::new();
    group(&refs, 0, 0, files, &mut out);
    out
}

/// Emit the rows for `entries`, all of which share their first `from` segments
/// (already printed by an enclosing dir row, if any), at indent `depth`.
fn group(entries: &[&Entry], from: usize, depth: usize, files: &[FileSummary], out: &mut Vec<Row>) {
    // Entries are sorted, so each directory's members are already contiguous:
    // walk the run of entries sharing `segments[from]` and handle it as a unit.
    let mut i = 0;
    while i < entries.len() {
        let key = &entries[i].segments[from];
        let mut j = i + 1;
        while j < entries.len() && &entries[j].segments[from] == key {
            j += 1;
        }
        let run = &entries[i..j];
        // A run of one whose segment is the whole path is a plain file row; a
        // longer path alone in its directory also stays inline (no header for a
        // single child), printing the rest of its path on the one row.
        if run.len() == 1 {
            let e = run[0];
            out.push(file_row(e, from, depth, files));
        } else {
            let end = shared_prefix_end(run, from);
            out.push(dir_row(run, from, end, depth, files));
            group(run, end, depth + 1, files, out);
        }
        i = j;
    }
}

/// How far the shared directory prefix of `run` extends past `from`: segment
/// `from` is shared by construction, and each further segment joins it while
/// every entry still has a directory there and agrees on its name. Returns the
/// index one past the last shared segment.
fn shared_prefix_end(run: &[&Entry], from: usize) -> usize {
    let mut end = from + 1;
    loop {
        // `end + 1 < len` — the entry must have something *below* this segment
        // for it to be a directory rather than the file name itself.
        let extends = run
            .iter()
            .all(|e| end + 1 < e.segments.len() && e.segments[end] == run[0].segments[end]);
        if !extends {
            return end;
        }
        end += 1;
    }
}

/// The dir row for `run`, labelled with segments `from..end` and carrying the
/// summed counts of every file beneath it.
fn dir_row(run: &[&Entry], from: usize, end: usize, depth: usize, files: &[FileSummary]) -> Row {
    let added = run.iter().map(|e| files[e.index].added).sum();
    let removed = run.iter().map(|e| files[e.index].removed).sum();
    // `run` is in path order, so its first entry is the subtree's first file —
    // the row a click on this heading should land on.
    let first = &files[run[0].index];
    Row {
        kind: RowKind::Dir,
        depth,
        label: format!("{}/", run[0].segments[from..end].join("/")),
        added,
        removed,
        binary: false,
        file: None,
        line_before: first.line_before,
        line_after: first.line_after,
        line_unified: first.line_unified,
    }
}

/// The file row for `e`, labelled with whatever of its path the enclosing dir
/// rows have not already printed.
fn file_row(e: &Entry, from: usize, depth: usize, files: &[FileSummary]) -> Row {
    let f = &files[e.index];
    Row {
        kind: RowKind::File,
        depth,
        label: e.segments[from..].join("/"),
        added: f.added,
        removed: f.removed,
        binary: f.binary,
        file: Some(e.index),
        line_before: f.line_before,
        line_after: f.line_after,
        line_unified: f.line_unified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(path: &str, added: u32, removed: u32) -> FileSummary {
        FileSummary {
            path: path.to_string(),
            added,
            removed,
            binary: false,
            ..FileSummary::default()
        }
    }

    /// Rendered shape of an outline: `depth`-indented labels, dirs marked `/`
    /// by their own label, so a test reads like the drawer looks.
    fn render(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
            .collect()
    }

    /// Files in the same directory print the prefix once; a chain of
    /// single-child directories collapses into that one prefix row.
    #[test]
    fn shared_prefixes_collapse_into_one_dir_row() {
        let files = vec![
            summary("components/hub/src/assa/client.py", 10, 2),
            summary("components/hub/src/assa/config.py", 1, 0),
        ];
        assert_eq!(
            render(&build(&files)),
            vec!["components/hub/src/assa/", "  client.py", "  config.py",]
        );
    }

    /// The shared prefix stops where the paths diverge, and each branch below it
    /// groups on its own.
    #[test]
    fn divergent_branches_nest_under_the_common_prefix() {
        let files = vec![
            summary("components/hub/src/a.py", 1, 1),
            summary("components/hub/tests/x.py", 2, 0),
            summary("components/hub/tests/y.py", 3, 0),
        ];
        assert_eq!(
            render(&build(&files)),
            vec![
                "components/hub/",
                "  src/a.py",
                "  tests/",
                "    x.py",
                "    y.py",
            ]
        );
    }

    /// A directory with a single file gets no header — the file keeps its whole
    /// remaining path on one row. Root-level files stay flat.
    #[test]
    fn lone_files_stay_inline() {
        let files = vec![
            summary(".gitignore", 4, 0),
            summary("docs/deep/nested/note.md", 1, 0),
        ];
        assert_eq!(
            render(&build(&files)),
            vec![".gitignore", "docs/deep/nested/note.md"]
        );
    }

    /// A dir row sums the counts beneath it; file rows keep their own and point
    /// back at their entry in the flat list.
    #[test]
    fn dir_rows_carry_the_subtree_totals() {
        let files = vec![summary("src/a.rs", 10, 3), summary("src/b.rs", 5, 4)];
        let rows = build(&files);
        assert_eq!(rows[0].kind, RowKind::Dir);
        assert_eq!((rows[0].added, rows[0].removed), (15, 7));
        assert_eq!(rows[0].file, None);
        assert_eq!(rows[1].file, Some(0));
        assert_eq!((rows[1].added, rows[1].removed), (10, 3));
    }

    /// Every row carries the anchor a click on it must scroll to: a file row its
    /// own file's marker lines, a dir row its *first* file's. The rows are
    /// re-ordered relative to the flat file list (they sort by segment), so this
    /// pins that each row kept the numbers of the file it names rather than the
    /// ones sitting at its position in the list.
    #[test]
    fn rows_carry_the_anchor_of_the_file_they_name() {
        let anchored = |path: &str, n: usize| FileSummary {
            path: path.to_string(),
            line_before: n,
            line_after: n,
            line_unified: n * 10,
            ..FileSummary::default()
        };
        // Deliberately out of segment order: `src.rs` sorts between the two
        // `src/` files textually, and the build re-orders them.
        let files = vec![
            anchored("src/b.rs", 3),
            anchored("src.rs", 1),
            anchored("src/a.rs", 2),
        ];
        let rows = build(&files);
        assert_eq!(render(&rows), vec!["src/", "  a.rs", "  b.rs", "src.rs"]);
        let anchors: Vec<(usize, usize, usize)> = rows
            .iter()
            .map(|r| (r.line_before, r.line_after, r.line_unified))
            .collect();
        assert_eq!(
            anchors,
            vec![
                (2, 2, 20), // the dir row → its first file, `src/a.rs`
                (2, 2, 20),
                (3, 3, 30),
                (1, 1, 10),
            ]
        );
    }

    /// Sorting is by path segment, so a directory's files stay contiguous even
    /// when a sibling file would sort between them as a raw string
    /// (`src.rs` falls between `src/a.rs` and `src/b.rs` textually).
    #[test]
    fn segment_order_keeps_directories_contiguous() {
        let files = vec![
            summary("src/b.rs", 1, 0),
            summary("src.rs", 1, 0),
            summary("src/a.rs", 1, 0),
        ];
        assert_eq!(
            render(&build(&files)),
            vec!["src/", "  a.rs", "  b.rs", "src.rs"]
        );
    }
}
