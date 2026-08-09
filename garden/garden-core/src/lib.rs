//! garden-core — the text model for the Garden IDE.
//!
//! Pure Rust, no graphics. A [`Buffer`] wraps a [`ropey::Rope`] and adds
//! line/column addressing ([`Point`]), edits, file open/save, and a
//! transaction-based undo/redo stack. See `docs/architecture.md` for the
//! crate contract.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;

pub mod projection;

/// Line/column position. `col` is a char offset within the line (not bytes).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

impl Point {
    /// Convenience constructor.
    pub fn new(line: usize, col: usize) -> Point {
        Point { line, col }
    }
}

/// A contiguous text selection: the `anchor` is where the selection started
/// (mouse press, or cursor position when extension began) and the `head` is
/// the moving end (the cursor). The two may be in either order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
}

impl Selection {
    pub fn new(anchor: Point, head: Point) -> Selection {
        Selection { anchor, head }
    }

    /// True when the selection covers no characters.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in document order.
    pub fn ordered(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The selected column range on `line`, for rendering one line's
    /// highlight: `(start_col, end_col, includes_newline)` where
    /// `start_col..end_col` are the selected chars within the line (callers
    /// pass `line_len` to bound interior/start lines) and `includes_newline`
    /// is true when the selection continues onto the next line. Returns
    /// `None` for lines outside the selection and for empty selections.
    pub fn cols_on_line(&self, line: usize, line_len: usize) -> Option<(usize, usize, bool)> {
        if self.is_empty() {
            return None;
        }
        let (start, end) = self.ordered();
        if line < start.line || line > end.line {
            return None;
        }
        let start_col = if line == start.line { start.col } else { 0 };
        let (end_col, newline) = if line == end.line {
            (end.col, false)
        } else {
            (line_len, true)
        };
        Some((start_col.min(line_len), end_col.min(line_len), newline))
    }
}

/// One primitive, invertible edit at an absolute char index in the rope.
#[derive(Clone, Debug)]
enum Edit {
    /// `text` was inserted at `char_idx`.
    Insert { char_idx: usize, text: String },
    /// `text` was deleted starting at `char_idx`.
    Delete { char_idx: usize, text: String },
}

/// A group of edits that undo/redo as one unit.
#[derive(Clone, Debug)]
struct Transaction {
    edits: Vec<Edit>,
    /// True while this transaction is a run of single-char insertions that
    /// the next adjacent single-char insertion may extend.
    coalescible: bool,
    /// The cursor position just before this transaction was applied, captured
    /// from [`Buffer::pending_cursor`] when the transaction was pushed. Undo
    /// restores it so the caret returns to where the edit started (e.g. `J`
    /// then `u`); `None` when no pending cursor was set, falling back to the
    /// position computed from the inverse edits.
    cursor_before: Option<Point>,
}

impl Transaction {
    /// Char index just past this transaction's text, if it is an open
    /// single-insert run (used to test adjacency of the next insertion).
    fn coalesce_end(&self) -> Option<usize> {
        if !self.coalescible {
            return None;
        }
        match self.edits.as_slice() {
            [Edit::Insert { char_idx, text }] => Some(char_idx + text.chars().count()),
            _ => None,
        }
    }
}

/// A cheap fingerprint of a file's on-disk state, compared to detect external
/// edits without re-reading the whole file. Modification time *and* length,
/// since either alone can miss a change (a same-length rewrite can keep a
/// coarse mtime; some edits change content without bumping mtime resolution).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiskStamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl DiskStamp {
    /// Stat `path` for its current stamp; `Err` if the file cannot be read.
    fn of(path: &Path) -> io::Result<DiskStamp> {
        let meta = fs::metadata(path)?;
        Ok(DiskStamp {
            mtime: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

/// An editable text buffer: rope storage, undo/redo stack, optional file
/// path, and a derived dirty flag (see [`Buffer::is_dirty`]).
pub struct Buffer {
    rope: Rope,
    /// Undo stack. Entries at `..undo_index` are applied; entries at
    /// `undo_index..` are redoable.
    transactions: Vec<Transaction>,
    undo_index: usize,
    path: Option<PathBuf>,
    /// The `undo_index` at which the buffer last matched its saved (or
    /// initial) content, or `None` once that revision has been discarded by
    /// editing after an undo. Dirtiness is `undo_index != saved_undo_index`.
    saved_undo_index: Option<usize>,
    /// The on-disk stamp as of the last open/save/reload — what we consider
    /// "in sync". `disk_changed` compares the file's current stamp to this.
    disk: Option<DiskStamp>,
    /// Monotonically increasing counter bumped on every content mutation
    /// (insert/delete/replace/undo/redo/reload). Unlike `undo_index` it never
    /// repeats a value for different content, so it is a reliable cache key
    /// for derived data such as syntax highlighting.
    revision: u64,
    /// The caret position to stamp onto the next pushed transaction as its
    /// pre-edit cursor (see [`Transaction::cursor_before`]). Callers set this
    /// to the cursor as it stood *before* the edit; `None` leaves undo to fall
    /// back to a position computed from the edits.
    pending_cursor: Option<Point>,
    /// While `true`, every edit folds into one undo transaction instead of
    /// pushing its own (see [`Buffer::begin_undo_group`]). `group_open` tracks
    /// whether that transaction has been created yet.
    grouping: bool,
    group_open: bool,
}

impl Buffer {
    /// An empty buffer with no associated file.
    pub fn new() -> Buffer {
        Buffer::from_str("")
    }

    /// A buffer initialized with `text`, no associated file.
    #[allow(clippy::should_implement_trait)] // name fixed by the architecture doc
    pub fn from_str(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            transactions: Vec::new(),
            undo_index: 0,
            path: None,
            saved_undo_index: Some(0),
            disk: None,
            revision: 0,
            pending_cursor: None,
            grouping: false,
            group_open: false,
        }
    }

    /// Record the caret position as it stands *before* the next edit, so the
    /// transaction that edit creates restores it on undo. Editors call this
    /// once per keypress with their current cursor; a coalesced typing run
    /// keeps the position of its first character. No-op for edits that never
    /// reach a fresh transaction (coalesced or empty).
    pub fn set_pending_cursor(&mut self, cursor: Point) {
        self.pending_cursor = Some(cursor);
    }

    /// Read `path` (UTF-8, lossy) into a new buffer that remembers the path.
    pub fn open(path: &Path) -> io::Result<Buffer> {
        let bytes = fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        let mut buf = Buffer::from_str(&text);
        buf.path = Some(path.to_path_buf());
        buf.disk = DiskStamp::of(path).ok();
        Ok(buf)
    }

    /// Write the buffer to `path`, adopting it as the buffer's file (the "save
    /// as" primitive). A pathless or save-protected buffer becomes owned by
    /// `path`, so subsequent [`save`](Self::save)s write there.
    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        self.path = Some(path.to_path_buf());
        self.save()
    }

    /// Write the buffer back to its file and record this revision as the
    /// saved one, so [`Buffer::is_dirty`] reports clean.
    ///
    /// Errors with [`io::ErrorKind::InvalidInput`] if the buffer has no path.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "buffer has no file path")
        })?;
        fs::write(path, self.rope.to_string())?;
        // The saved revision must stay an undo-stack boundary, or chars
        // coalesced in afterward would leave `undo_index` unchanged and
        // falsely report clean.
        self.end_undo_run();
        self.saved_undo_index = Some(self.undo_index);
        // Our own write is the new in-sync state, not an external change.
        self.disk = self.path.as_deref().and_then(|p| DiskStamp::of(p).ok());
        Ok(())
    }

    /// The current on-disk stamp if the file has changed since it was last
    /// opened, saved, or reloaded (an *external* edit), else `None`. Also
    /// `None` for a pathless buffer or when the file cannot be stat'd
    /// (e.g. deleted) — those are not actionable reloads. The returned stamp
    /// lets callers dedupe repeated notifications about the same disk version.
    pub fn disk_changed(&self) -> Option<DiskStamp> {
        let path = self.path.as_deref()?;
        let current = DiskStamp::of(path).ok()?;
        (Some(current) != self.disk).then_some(current)
    }

    /// Re-read the file from disk, replacing the buffer's content and
    /// discarding undo history, so the buffer becomes clean and in sync with
    /// the file (like a fresh open that keeps the same path). Used to refresh
    /// a clean buffer after an external edit. The caller is responsible for
    /// re-clamping any cursor/scroll positions into the new content.
    ///
    /// Errors with [`io::ErrorKind::InvalidInput`] if the buffer has no path.
    pub fn reload(&mut self) -> io::Result<()> {
        let path = self.path.clone().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "buffer has no file path")
        })?;
        let bytes = fs::read(&path)?;
        let text = String::from_utf8_lossy(&bytes);
        self.rope = Rope::from_str(&text);
        self.transactions.clear();
        self.undo_index = 0;
        self.saved_undo_index = Some(0);
        self.disk = DiskStamp::of(&path).ok();
        self.revision += 1;
        Ok(())
    }

    /// A monotonically increasing edit counter (see the `revision` field). Use
    /// it as a cache key for data derived from the buffer content; it changes
    /// on every mutation and never repeats a value for differing content.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The buffer's position in its undo stack. Stable across edits that fold
    /// into an open group or coalesce into a run, and moved by exactly one on
    /// each undo/redo — which is what lets a
    /// [`Projection`](crate::projection::Projection) key its own history to the
    /// buffer's and stay in step through both.
    pub fn undo_index(&self) -> usize {
        self.undo_index
    }

    /// Open an undo group: every edit until the next [`end_undo_run`] folds
    /// into a single transaction, so a whole vim insert session (typing,
    /// backspaces, and in-insert newlines) or a change command (its delete plus
    /// the typed replacement) undoes in one step. Closes any open coalescing
    /// run first so the group can't merge backward into a prior edit.
    pub fn begin_undo_group(&mut self) {
        self.end_undo_run();
        self.grouping = true;
        self.group_open = false;
    }

    /// Close any open coalescing run or undo group: the next edit starts a new
    /// undo transaction even if it lands exactly where the run stopped. Called
    /// on save (the saved revision must be an undo boundary) and when leaving
    /// vim Insert mode (one insert session = one undo step).
    pub fn end_undo_run(&mut self) {
        self.grouping = false;
        self.group_open = false;
        if self.undo_index == self.transactions.len() {
            if let Some(last) = self.transactions.last_mut() {
                last.coalescible = false;
            }
        }
    }

    /// The file this buffer was opened from, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// True if the buffer's content differs from its saved (or initial)
    /// revision. Derived from undo-stack position, so undoing or redoing
    /// back onto the saved revision reports clean again; if that revision
    /// was discarded by editing after an undo, the buffer stays dirty until
    /// the next save.
    pub fn is_dirty(&self) -> bool {
        self.saved_undo_index != Some(self.undo_index)
    }

    /// Number of lines. An empty buffer has one (empty) line; a trailing
    /// newline yields a final empty line.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// The buffer's full content as a single string, including line breaks (a
    /// trailing newline is preserved). The counterpart to [`Buffer::from_str`].
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The text of line `idx`, without its trailing line break.
    pub fn line(&self, idx: usize) -> String {
        let mut s = self.rope.line(idx).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        if s.ends_with('\r') {
            s.pop();
        }
        s
    }

    /// Length of line `idx` in chars, excluding its trailing line break.
    pub fn line_len(&self, idx: usize) -> usize {
        let line = self.rope.line(idx);
        let mut len = line.len_chars();
        let mut chars = line.chars_at(len);
        while let Some(c) = chars.prev() {
            if c == '\n' || c == '\r' {
                len -= 1;
            } else {
                break;
            }
        }
        len
    }

    /// The text in `[start, end)` (both clamped, swapped if reversed),
    /// including line breaks.
    pub fn text_range(&self, start: Point, end: Point) -> String {
        let mut a = self.point_to_char(self.clamp(start));
        let mut b = self.point_to_char(self.clamp(end));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        self.rope.slice(a..b).to_string()
    }

    /// Clamp `p` to a valid position: line into range, col to the line length.
    pub fn clamp(&self, p: Point) -> Point {
        let line = p.line.min(self.line_count() - 1);
        let col = p.col.min(self.line_len(line));
        Point { line, col }
    }

    /// Insert `text` at `p` (clamped); returns the position just after the
    /// inserted text.
    pub fn insert(&mut self, p: Point, text: &str) -> Point {
        let p = self.clamp(p);
        if text.is_empty() {
            return p;
        }
        let char_idx = self.point_to_char(p);
        self.rope.insert(char_idx, text);
        self.record_insert(char_idx, text);
        self.revision += 1;
        self.char_to_point(char_idx + text.chars().count())
    }

    /// Delete the range `[start, end)` (both clamped, swapped if reversed);
    /// returns the start of the deleted range.
    pub fn delete(&mut self, start: Point, end: Point) -> Point {
        self.replace(start, end, "")
    }

    /// Delete `[start, end)` (both clamped, swapped if reversed) and insert
    /// `text` at its start, as a **single undo transaction** — one undo
    /// restores both the deleted range and removes the inserted text (typing
    /// over a selection). Returns the position just after the inserted text.
    pub fn replace(&mut self, start: Point, end: Point, text: &str) -> Point {
        let mut a = self.point_to_char(self.clamp(start));
        let mut b = self.point_to_char(self.clamp(end));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let mut edits = Vec::new();
        if a < b {
            let deleted = self.rope.slice(a..b).to_string();
            self.rope.remove(a..b);
            edits.push(Edit::Delete {
                char_idx: a,
                text: deleted,
            });
        }
        if !text.is_empty() {
            self.rope.insert(a, text);
            edits.push(Edit::Insert {
                char_idx: a,
                text: text.to_string(),
            });
        }
        if edits.is_empty() {
            return self.char_to_point(a);
        }
        self.record_transaction(edits, false);
        self.revision += 1;
        self.char_to_point(a + text.chars().count())
    }

    /// Undo one transaction; returns a cursor position to restore, or `None`
    /// if there is nothing to undo.
    pub fn undo(&mut self) -> Option<Point> {
        if self.undo_index == 0 {
            return None;
        }
        self.undo_index -= 1;
        // The undone transaction may be redone later; never let a subsequent
        // insertion coalesce into it.
        self.transactions[self.undo_index].coalescible = false;
        let txn = self.transactions[self.undo_index].clone();
        let mut cursor = Point::default();
        for edit in txn.edits.iter().rev() {
            cursor = self.apply_inverse(edit);
        }
        self.revision += 1;
        // Prefer the pre-edit caret captured when the transaction was pushed,
        // so undo lands where the edit started rather than at the end of the
        // text it restored.
        Some(txn.cursor_before.unwrap_or(cursor))
    }

    /// Redo one undone transaction; returns a cursor position to restore, or
    /// `None` if there is nothing to redo. The caret lands at the *end* of the
    /// re-applied text (the standard editor feel); vim's `<C-R>` wants the
    /// start of the change instead — see [`Buffer::redo_vim`].
    pub fn redo(&mut self) -> Option<Point> {
        if self.undo_index == self.transactions.len() {
            return None;
        }
        let txn = self.transactions[self.undo_index].clone();
        self.undo_index += 1;
        let mut cursor = Point::default();
        for edit in &txn.edits {
            cursor = self.apply_forward(edit);
        }
        self.revision += 1;
        Some(cursor)
    }

    /// Like [`Buffer::redo`], but returns the caret at the **start** of the
    /// redone change (its first modified position) rather than the end of the
    /// re-applied text. This is vim's `<C-R>` rule: after redoing an insert the
    /// caret sits on the first re-inserted character, not past the last one. The
    /// returned point is clamped to the buffer; callers pull it onto a real
    /// character for Normal mode.
    pub fn redo_vim(&mut self) -> Option<Point> {
        if self.undo_index == self.transactions.len() {
            return None;
        }
        let start = match self.transactions[self.undo_index].edits.first() {
            Some(Edit::Insert { char_idx, .. } | Edit::Delete { char_idx, .. }) => *char_idx,
            None => 0,
        };
        self.redo();
        Some(self.char_to_point(start.min(self.rope.len_chars())))
    }

    /// Absolute char index of a (valid) point.
    fn point_to_char(&self, p: Point) -> usize {
        self.rope.line_to_char(p.line) + p.col
    }

    /// Point for an absolute char index.
    fn char_to_point(&self, char_idx: usize) -> Point {
        let line = self.rope.char_to_line(char_idx);
        let col = char_idx - self.rope.line_to_char(line);
        Point { line, col }
    }

    /// Record an insertion in the undo stack, coalescing a single-char,
    /// non-newline insertion into an adjacent preceding run.
    fn record_insert(&mut self, char_idx: usize, text: &str) {
        let single_plain_char =
            text.chars().count() == 1 && !text.contains('\n') && !text.contains('\r');
        // Outside a group, a single-char insertion extends an adjacent run so a
        // typing burst is one undo step. Inside a group every edit folds in
        // anyway (see record_transaction), so skip the run-merge fast path.
        if !self.grouping && single_plain_char && self.undo_index == self.transactions.len() {
            if let Some(last) = self.transactions.last_mut() {
                if last.coalesce_end() == Some(char_idx) {
                    if let [Edit::Insert { text: run, .. }] = last.edits.as_mut_slice() {
                        run.push_str(text);
                        return;
                    }
                }
            }
        }
        self.record_transaction(
            vec![Edit::Insert {
                char_idx,
                text: text.to_string(),
            }],
            single_plain_char,
        );
    }

    /// Record a completed edit. While an undo group is open (see
    /// [`Buffer::begin_undo_group`]) every call folds its edits into the one
    /// group transaction; otherwise each call pushes its own.
    fn record_transaction(&mut self, edits: Vec<Edit>, coalescible: bool) {
        if self.grouping {
            if self.group_open && self.undo_index == self.transactions.len() {
                if let Some(group) = self.transactions.last_mut() {
                    group.edits.extend(edits);
                    return;
                }
            }
            self.push_transaction(Transaction {
                edits,
                coalescible: false,
                cursor_before: None,
            });
            self.group_open = true;
            return;
        }
        self.push_transaction(Transaction {
            edits,
            coalescible,
            cursor_before: None,
        });
    }

    /// Push a new transaction, discarding any redoable entries and closing
    /// the previous coalescing run.
    fn push_transaction(&mut self, mut txn: Transaction) {
        // Stamp the pre-edit cursor so undo can restore it. Coalesced single-
        // char inserts skip this path, so a typing run keeps the position of
        // its first character.
        txn.cursor_before = self.pending_cursor;
        // If the saved revision lives in the redo entries being discarded,
        // it is gone for good: no undo position can be clean until the next
        // save.
        if self
            .saved_undo_index
            .is_some_and(|saved| saved > self.undo_index)
        {
            self.saved_undo_index = None;
        }
        self.transactions.truncate(self.undo_index);
        if let Some(last) = self.transactions.last_mut() {
            last.coalescible = false;
        }
        self.transactions.push(txn);
        self.undo_index = self.transactions.len();
    }

    /// Apply the inverse of `edit`; returns the cursor position to restore.
    fn apply_inverse(&mut self, edit: &Edit) -> Point {
        match edit {
            Edit::Insert { char_idx, text } => {
                self.rope.remove(*char_idx..char_idx + text.chars().count());
                self.char_to_point(*char_idx)
            }
            Edit::Delete { char_idx, text } => {
                self.rope.insert(*char_idx, text);
                self.char_to_point(char_idx + text.chars().count())
            }
        }
    }

    /// Re-apply `edit`; returns the cursor position to restore.
    fn apply_forward(&mut self, edit: &Edit) -> Point {
        match edit {
            Edit::Insert { char_idx, text } => {
                self.rope.insert(*char_idx, text);
                self.char_to_point(char_idx + text.chars().count())
            }
            Edit::Delete { char_idx, text } => {
                self.rope.remove(*char_idx..char_idx + text.chars().count());
                self.char_to_point(*char_idx)
            }
        }
    }
}

impl Default for Buffer {
    fn default() -> Buffer {
        Buffer::new()
    }
}

impl std::fmt::Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rope)
    }
}

#[cfg(test)]
mod tests;
