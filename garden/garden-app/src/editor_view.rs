//! One editor pane: a buffer plus view state (cursor, scroll), input handling
//! at the text-editing level, and scene building for the renderer.

use std::cell::{Cell, RefCell};
use std::path::Path;

use garden_core::projection::{Intent, Outcome, Projection, RowOp};
use garden_core::{Buffer, Point, Selection};
use garden_render::{Color, Primitive, Rect, TextStyle, FONT_SIZE};

use crate::search;
use crate::syntax::{self, Span};
use crate::theme;
use crate::vim::VimState;

/// Lazily-refreshed syntax-highlight state for a pane. Recomputed only when the
/// buffer revision changes (see [`EditorView::build_scene`]); interior-mutable
/// so the `&self` scene builder can refresh it.
struct HighlightState {
    highlighter: syntax::Highlighter,
    /// The buffer's language, resolved from its path; `None` = no highlighting.
    lang: Option<syntax::Language>,
    /// The buffer revision `lines` was computed at; `None` = never computed.
    rev: Option<u64>,
    /// One span list per buffer line.
    lines: Vec<Vec<Span>>,
}

impl HighlightState {
    fn new() -> HighlightState {
        HighlightState {
            highlighter: syntax::Highlighter::new(),
            lang: None,
            rev: None,
            lines: Vec::new(),
        }
    }
}

/// Inner padding between a pane's border and its content, logical pixels.
const PAD: f32 = 6.0;
/// Width of the cursor bar, logical pixels.
const CURSOR_W: f32 = 2.0;
/// Thickness of a scrollbar (track and thumb), logical pixels. Wide enough to
/// see at a glance and to hit with the pointer without aiming.
const SCROLLBAR_W: f32 = 10.0;

/// A scrollbar's placed geometry: the full track and the draggable thumb.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ScrollbarGeom {
    pub track: Rect,
    pub thumb: Rect,
}

/// Which scrollbar a press landed on, plus the grab offset between the pointer
/// and the thumb's leading edge so dragging keeps that grip point.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ScrollbarHit {
    pub axis: ScrollAxis,
    pub grab: f32,
}

/// A scrollbar axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
}
/// Tab stop width in display columns: a tab advances to the next multiple of
/// this, not a fixed run of cells.
pub(crate) const TAB_WIDTH: usize = 4;

// --- tab-aware display columns ----------------------------------------------
//
// The model (`Point.col`, selections, vim motions) counts *chars*; on-screen
// geometry counts *display columns*, where a tab spans up to `TAB_WIDTH`
// cells. These pure functions translate at the view boundary — everything
// that turns a column into pixels (or pixels into a column) goes through them.

/// Display-cell advance of `ch` when it starts at display column `col`: a tab
/// advances to the next `TAB_WIDTH` stop, everything else one cell.
fn char_advance(ch: char, col: usize) -> usize {
    if ch == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else {
        1
    }
}

/// The display column where `line`'s char `char_col` starts, expanding tabs
/// per tab stop. Columns past the end of the line keep counting one cell per
/// char, so callers needn't pre-clamp.
pub(crate) fn display_col(line: &str, char_col: usize) -> usize {
    let mut col = 0;
    let mut chars = 0;
    for ch in line.chars().take(char_col) {
        col += char_advance(ch, col);
        chars += 1;
    }
    col.saturating_add(char_col - chars)
}

/// Display width of the whole line (tabs expanded per tab stop).
pub(crate) fn display_width(line: &str) -> usize {
    line.chars().fold(0, |col, ch| col + char_advance(ch, col))
}

/// The char column nearest to the fractional display column `x`: a point in
/// the first half of a character's display cells maps to that character, the
/// second half to the position after it — so clicking the tail of a tab lands
/// after the tab, matching mainstream editors. Past the end of the line this
/// clamps to the line's char count.
pub(crate) fn char_col_for_display(line: &str, x: f32) -> usize {
    let mut col = 0usize;
    let mut chars = 0usize;
    for ch in line.chars() {
        let advance = char_advance(ch, col);
        if x < col as f32 + advance as f32 / 2.0 {
            return chars;
        }
        col += advance;
        chars += 1;
    }
    chars
}

/// The char-column at which each visual (soft-wrapped) row of `line` begins
/// when wrapped to `width` display columns. The first entry is always 0; a line
/// that fits within `width` returns `vec![0]`. Wrapping prefers to break after a
/// run of whitespace (word wrap); a single word wider than `width` is broken
/// hard at the column boundary. Tabs count by their display advance, and each
/// visual row restarts at display column 0 (so its tab stops are row-relative,
/// matching how a wrapped row is drawn from the text origin). A `width` of 0 is
/// treated as 1.
pub(crate) fn wrap_rows(line: &str, width: usize) -> Vec<usize> {
    let mut starts = vec![0];
    wrap_scan(line, width, |brk| starts.push(brk));
    starts
}

/// How many visual rows `line` occupies at `width`, without allocating the
/// per-row start list — the count-only path behind the wrap geometry (total
/// rows, scroll clamping). Always ≥ 1.
pub(crate) fn wrap_row_count(line: &str, width: usize) -> usize {
    let mut n = 1usize;
    wrap_scan(line, width, |_| n += 1);
    n
}

/// The one greedy word-wrap pass, shared by [`wrap_rows`] and
/// [`wrap_row_count`]: calls `on_break(char_idx)` at the start char of each
/// visual row after the first. Breaks after a whitespace run (word wrap), hard-
/// breaking an over-long word; tabs count by their display advance and each row
/// restarts at display column 0 (row-relative tab stops).
fn wrap_scan(line: &str, width: usize, mut on_break: impl FnMut(usize)) {
    let width = width.max(1);
    let chars: Vec<char> = line.chars().collect();
    let mut row_start = 0usize; // char index of the current row's first char
    let mut col = 0usize; // display column within the current row
    let mut last_break: Option<usize> = None; // char index to break before (after whitespace)
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        let adv = char_advance(ch, col);
        // Wrap before placing a char that would overflow the row — but never on
        // the row's first char, so an over-wide char/tab still makes progress.
        if col + adv > width && i > row_start {
            let brk = match last_break {
                Some(b) if b > row_start => b,
                _ => i, // no usable word boundary: hard break here
            };
            on_break(brk);
            row_start = brk;
            last_break = None;
            // The chars carried onto the new row (brk..i) restart at column 0.
            col = 0;
            for &c in &chars[brk..i] {
                col += char_advance(c, col);
            }
            continue; // re-evaluate this char on the fresh row
        }
        col += adv;
        i += 1;
        if ch == ' ' || ch == '\t' {
            last_break = Some(i); // a break may fall just after the whitespace
        }
    }
}

/// Where to place the cursor's line when repositioning the viewport around it
/// (vim's `zt`/`zz`/`zb`). See [`EditorView::scroll_cursor_to`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollAlign {
    Top,
    Center,
    Bottom,
}

/// Where a view is scrolled to, with **sub-cell precision** — the position a
/// pixel-smooth wheel or trackpad gesture lands on between two rows.
///
/// Vertically the position is an *anchor visual row* (`top`, a buffer line,
/// plus `sub`, a wrapped sub-row within it) and `frac`, how far the viewport
/// has moved into that row, in rows: `0.0..1.0`. The anchor is deliberately not
/// an absolute pixel offset. Rows above the viewport can re-wrap, the font size
/// can change, and lines can be inserted or deleted — an absolute offset would
/// slide the content under the user on all three, while an anchored one keeps
/// the same text under the same pixel. It also keeps every scroll operation
/// O(rows moved) rather than O(buffer): nothing has to measure the lines above.
///
/// Horizontally there is no wrapping to anchor to, so `left` is simply a
/// fractional display-column offset. It is meaningful only with `wrap` off;
/// wrapping pins it to `0.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    /// First (partially) visible buffer line.
    pub top: usize,
    /// Wrapped sub-row of `top` at the top of the viewport. Always 0 when
    /// `wrap` is off, where every buffer line is exactly one visual row.
    pub sub: usize,
    /// Rows scrolled past the `(top, sub)` anchor row, in `0.0..1.0`. This is
    /// the whole of "smooth" scrolling: `0.0` is the row-quantized position
    /// every other field describes, and anything in between shifts the drawn
    /// rows up by `frac * cell_height` pixels.
    pub frac: f32,
    /// First visible display column (fractional). Only meaningful with `wrap`
    /// off.
    pub left: f32,
}

impl Scroll {
    /// The anchor visual row, ignoring the fractional part. Scroll arithmetic
    /// that steps whole rows works in this space.
    fn vpos(&self) -> (usize, usize) {
        (self.top, self.sub)
    }

    /// Move to visual row `(line, sub)` exactly, dropping any fractional
    /// offset — what a jump (rather than a scroll) does.
    fn set_vpos(&mut self, (line, sub): (usize, usize)) {
        self.top = line;
        self.sub = sub;
        self.frac = 0.0;
    }

    /// Whether this position is past `max` (an anchor row, fraction 0) — the
    /// bottom clamp. Compared anchor-first, then fraction, since the anchor is
    /// the coarse coordinate.
    fn past(&self, max: (usize, usize)) -> bool {
        (self.vpos(), self.frac > 0.0) > (max, false)
    }
}

pub struct EditorView {
    pub buffer: Buffer,
    pub cursor: Point,
    /// Selection anchor; the selection spans `anchor..cursor`. `None` means
    /// no selection (a bare caret).
    pub anchor: Option<Point>,
    /// Where the view is scrolled to, on both axes, with sub-cell precision.
    pub scroll: Scroll,
    /// Soft-wrap long lines to the pane width instead of scrolling horizontally.
    /// On by default for editor panes; externally-supplied content turns it off
    /// (see [`set_external_content`](Self::set_external_content)) so a GPP
    /// process pane's client-computed rows stay 1:1 with buffer lines. A panel's
    /// embedded region — where the host, not the client, owns the row math — can
    /// opt back in with `text_view_wrap(id, true)`.
    pub wrap: bool,
    /// Last text-area width in display columns seen during layout, so wrap-aware
    /// operations that lack a viewport argument (vim's `zt`/`zz`/`zb`) can reuse
    /// it. Refreshed by [`build_scene`](Self::build_scene) and
    /// [`ensure_cursor_visible`](Self::ensure_cursor_visible).
    cols_hint: Cell<usize>,
    /// Column the cursor "wants" during a run of vertical moves: set on the
    /// first vertical move and restored when a long-enough line is reached,
    /// so passing through short lines doesn't lose the column. `usize::MAX`
    /// means stick to end-of-line (vim's `$`). Cleared by any other movement
    /// or edit.
    pub desired_col: Option<usize>,
    /// Display column (on-screen x, tabs expanded, measured from the visual
    /// row's left edge) the cursor "wants" during a run of display-line moves
    /// (`gj`/`gk`). The wrapped-row analogue of [`desired_col`](Self::desired_col):
    /// soft-wrapped rows begin at different char columns, so display x — not the
    /// char col — is the quantity a run of `gj`/`gk` preserves through short
    /// rows. Set by `gj`/`gk`; cleared by [`clamp_cursor_normal`] like
    /// `desired_col`.
    pub display_desired_col: Option<usize>,
    /// Vi/vim mode and pending-command state for this pane.
    pub vim: VimState,
    /// Selection granularity of the active mouse drag, set by the click
    /// count of the press that started it (see
    /// [`begin_drag_with_clicks`](Self::begin_drag_with_clicks)).
    pub drag: DragMode,
    /// Cached syntax highlighting, refreshed lazily in `build_scene` when the
    /// buffer revision changes.
    highlight: RefCell<HighlightState>,
    /// A display title supplied from outside the buffer — a GPP process pane
    /// reports one in its `initialize` response (the client name) and its
    /// `render` messages (e.g. the browsed directory). When set it overrides
    /// the buffer-path title, which a process-backed pane doesn't have.
    external_title: Option<String>,
    /// Per-line semantic style spans supplied from outside the buffer (a panel
    /// `text_view`'s line styling). When non-empty they replace syntax
    /// highlighting for the lines they cover; a line with no entry renders
    /// plain. Cleared by every content replacement.
    external_styles: Vec<Vec<StyleSpan>>,
    /// Per-line background spans supplied from outside the buffer. Each span
    /// tints a char-column range of its line, drawn under the text. A line with
    /// no entry renders no background. Cleared by every content replacement,
    /// like [`Self::external_styles`].
    external_backgrounds: Vec<Vec<BgSpan>>,
    /// Per-pane config: draw the line-number gutter. Defaults to `false`; set
    /// from the layout script's `editor(path, { line_numbers: true })`.
    pub show_line_numbers: bool,
    /// A 0-based line to flag as the source of a compile/runtime error, drawn
    /// with a reddish full-width background. Set by the Petal-IDE sync from the
    /// paired panel's error (see `App::sync_editor_panels`); `None` = no error.
    pub error_line: Option<usize>,
    /// The source range of the shape the pointer is currently over on a paired
    /// Petal-IDE canvas — Petal's **direct-manipulation** highlight — as
    /// `(start, end)` 0-based line/column points. Drawn as a tinted band behind
    /// the text.
    ///
    /// Distinct from a selection: it is *not* the user's, so it must not move
    /// the cursor, survive a click, or be something the keyboard can extend. It
    /// is a view of where the mouse is on the canvas, reconciled every frame by
    /// `App::sync_editor_panels` and cleared the moment the pointer leaves a
    /// shape.
    pub trace_highlight: Option<(Point, Point)>,
    /// The [`Projection`] this view is an editable *view* of, when its content
    /// was assembled out of other documents rather than read from one file (a
    /// unified diff, say). Every mutation is reported to it (see
    /// [`edit`](Self::edit)) so the origin of each line stays known, which is
    /// what lets the edits be folded back into the sources exactly. `None` for
    /// an ordinary file-backed view, where every mutation short-circuits.
    pub projection: Option<Projection>,
    /// Why the last edit was refused, if it was — set when an attached
    /// [`Projection`] rejects a structural edit (deleting a line that belongs to
    /// the view rather than to the change). Drained by the key routing into the
    /// status bar; `None` the rest of the time.
    pub edit_refusal: Option<String>,
}

/// How a mouse drag extends the selection. The multi-click modes remember the
/// initially selected range (the pivot) so dragging across it in either
/// direction always keeps that range selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DragMode {
    /// Plain click: the selection head follows the mouse character-wise.
    #[default]
    Caret,
    /// Double-click: the selection grows by whole words.
    Word { pivot: (Point, Point) },
    /// Triple-click (or more): the selection grows by whole lines.
    Line { pivot: (Point, Point) },
}

pub enum Move {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    /// `Ctrl+U`: cursor up half a viewport (vim's half-page scroll).
    HalfPageUp,
    /// `Ctrl+D`: cursor down half a viewport (vim's half-page scroll).
    HalfPageDown,
}

impl EditorView {
    pub fn open(file: Option<&str>) -> EditorView {
        let buffer = match file {
            Some(path) => Buffer::open(Path::new(path)).unwrap_or_else(|err| {
                Buffer::from_str(&format!("// could not open {path}: {err}\n"))
            }),
            None => Buffer::new(),
        };
        EditorView::from_buffer(buffer)
    }

    /// An editor view over `buffer` with default view state (cursor at the
    /// origin, no selection, unscrolled, Normal mode).
    pub(crate) fn from_buffer(buffer: Buffer) -> EditorView {
        EditorView {
            buffer,
            cursor: Point::default(),
            anchor: None,
            scroll: Scroll::default(),
            wrap: true,
            cols_hint: Cell::new(80),
            desired_col: None,
            display_desired_col: None,
            vim: VimState::default(),
            drag: DragMode::default(),
            highlight: RefCell::new(HighlightState::new()),
            external_title: None,
            external_styles: Vec::new(),
            external_backgrounds: Vec::new(),
            show_line_numbers: false,
            error_line: None,
            trace_highlight: None,
            projection: None,
            edit_refusal: None,
        }
    }

    /// The current selection, or `None` when there is no anchor or it
    /// coincides with the cursor.
    pub fn selection(&self) -> Option<Selection> {
        let sel = Selection::new(self.anchor?, self.cursor);
        (!sel.is_empty()).then_some(sel)
    }

    /// The selection to highlight on screen. In linewise Visual mode (`V`)
    /// the charwise `anchor..cursor` is widened to cover whole lines —
    /// including the trailing newline — so the highlight matches VISUAL LINE.
    fn render_selection(&self) -> Option<Selection> {
        let sel = self.selection()?;
        if self.vim.mode != crate::vim::Mode::VisualLine {
            return Some(sel);
        }
        let (a, b) = sel.ordered();
        let last = self.buffer.line_count().saturating_sub(1);
        let start = Point::new(a.line, 0);
        let end = if b.line < last {
            Point::new(b.line + 1, 0) // include the newline after the last line
        } else {
            Point::new(b.line, self.buffer.line_len(b.line))
        };
        Some(Selection::new(start, end))
    }

    /// The selected text, or an empty string with no selection.
    pub fn selected_text(&self) -> String {
        match self.selection() {
            Some(sel) => {
                let (start, end) = sel.ordered();
                self.buffer.text_range(start, end)
            }
            None => String::new(),
        }
    }

    pub fn select_all(&mut self) {
        let last = self.buffer.line_count() - 1;
        self.anchor = Some(Point::new(0, 0));
        self.cursor = Point::new(last, self.buffer.line_len(last));
    }

    /// Display name for the status bar. A process pane's externally-supplied
    /// title (set via [`set_external_title`](Self::set_external_title)) wins
    /// over the buffer path; such panes aren't editable, so no dirty marker.
    pub fn title(&self) -> String {
        if let Some(title) = &self.external_title {
            return title.clone();
        }
        let name = self
            .buffer
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "[untitled]".to_string());
        if self.buffer.is_dirty() {
            format!("{name} •")
        } else {
            name
        }
    }

    // --- editing -----------------------------------------------------------

    /// Delete the selected range as one undo transaction, leaving the cursor
    /// at its start. Returns false (and does nothing) when there is no
    /// selection. Also used by Cmd+X after the selection is copied.
    pub(crate) fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let (start, end) = sel.ordered();
        self.cursor = self.erase(start, end);
        self.anchor = None;
        true
    }

    // --- the mutation choke point ------------------------------------------
    //
    // Every edit this view makes — vim's operators, insert-mode typing, paste,
    // the mouse — goes through `edit` (or its two shorthands). That is what
    // makes an attached `Projection` total: it sees each mutation as a line
    // splice and keeps every line's origin known, so no vim command needs
    // projection support of its own.

    /// Replace `[start, end)` with `text`, reporting the edit to the projection.
    /// Returns the position just past the inserted text.
    pub fn edit(&mut self, start: Point, end: Point, text: &str) -> Point {
        let (a, b) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let (a, b) = (self.buffer.clamp(a), self.buffer.clamp(b));
        // A pure insertion keeps `Buffer::insert`, whose single-char fast path is
        // what coalesces a typing burst into one undo step; `replace` always
        // pushes its own transaction.
        let after = if a == b {
            self.buffer.insert(a, text)
        } else {
            self.buffer.replace(a, b, text)
        };
        if self.projection.is_some() {
            let (row, removed, inserted) = line_splice(a, b, text, after);
            let undo_index = self.buffer.undo_index();
            if let Some(p) = &mut self.projection {
                p.splice(row, removed, inserted, undo_index);
            }
            self.refresh_projection_styles();
        }
        after
    }

    /// Delete `[start, end)`, reporting the edit to the projection.
    pub fn erase(&mut self, start: Point, end: Point) -> Point {
        self.edit(start, end, "")
    }

    /// Insert `text` at `p`, reporting the edit to the projection.
    pub fn insert_at(&mut self, p: Point, text: &str) -> Point {
        self.edit(p, p, text)
    }

    /// What a freshly opened line starts with: `line`'s own indent (vim's
    /// `autoindent`, truncated to `max_cols` so a break inside the indent never
    /// deepens it) — decorated as an addition when this view is a projection.
    ///
    /// In a diff, a line you open is a line you are *adding*, and the neighbour's
    /// leading `+`/`-`/space is a marker, not indentation: copying it verbatim
    /// would give the new line someone else's meaning (opening under a `-` line
    /// would produce another deletion). So the marker is stripped before the
    /// indent is read, and the projection's own "added" decoration is put back.
    fn open_seed(&self, line: usize, max_cols: usize) -> String {
        let text = self.buffer.line(line);
        let Some(proj) = self.projection.as_ref() else {
            return leading_indent(&text, max_cols).to_string();
        };
        let body = text.strip_prefix(proj.worn_decor(line)).unwrap_or(&text);
        let indent = leading_indent(body, max_cols);
        match proj.new_line_decor(line) {
            Some(decor) => format!("{decor}{indent}"),
            None => indent.to_string(),
        }
    }

    /// Repaint the per-line styles from the projection's origin table. Called
    /// after every mutation, so a band stays on the line it belongs to instead
    /// of drifting when a line is inserted above it.
    fn refresh_projection_styles(&mut self) {
        let Some(names) = self.projection.as_ref().map(|p| p.line_styles()) else {
            return;
        };
        self.set_external_line_styles(&names);
    }

    /// Offer a whole-line deletion to the projection as a structural
    /// [`Intent`] before it lands as text (see the module docs' "tier 2").
    /// `Ok(true)` means the projection claimed it and the edit is already done;
    /// `Ok(false)` means carry on with the ordinary text deletion. `Err` carries
    /// a reason to show the user — the edit must not happen.
    pub fn line_delete_intent(&mut self, start: usize, count: usize) -> Result<bool, String> {
        let undo_index = self.buffer.undo_index() + 1;
        let outcome = match &mut self.projection {
            Some(p) => p.intent(Intent::DeleteLines { start, count }, undo_index),
            None => return Ok(false),
        };
        let (row, ops) = match outcome {
            Outcome::Pass => return Ok(false),
            Outcome::Refused(why) => return Err(why),
            Outcome::Claimed { start, ops } => (start, ops),
        };
        // Bottom-up, so the rows still to be touched keep their indices. The
        // projection has already patched its table to match, so these edits go
        // straight to the buffer rather than back through `edit`.
        for (i, op) in ops.iter().enumerate().rev() {
            let line = row + i;
            match op {
                RowOp::Keep => {}
                RowOp::Delete => {
                    let (from, to) = if line + 1 < self.buffer.line_count() {
                        (Point::new(line, 0), Point::new(line + 1, 0))
                    } else if line > 0 {
                        (
                            Point::new(line - 1, self.buffer.line_len(line - 1)),
                            Point::new(line, self.buffer.line_len(line)),
                        )
                    } else {
                        (Point::new(0, 0), Point::new(0, self.buffer.line_len(0)))
                    };
                    self.buffer.replace(from, to, "");
                }
                RowOp::Set(text) => {
                    let end = Point::new(line, self.buffer.line_len(line));
                    self.buffer.replace(Point::new(line, 0), end, text);
                }
            }
        }
        self.cursor = self.buffer.clamp(Point::new(row, 0));
        self.anchor = None;
        self.refresh_projection_styles();
        Ok(true)
    }

    /// Insert a line break with auto-indent: the new line starts with the
    /// current line's leading whitespace (spaces/tabs verbatim), truncated
    /// to the cursor column so a break inside the indent never deepens it.
    /// One `insert` call, so the break and its indent are one undo
    /// transaction. Used by the Enter key (and only by it) — paste and other
    /// programmatic inserts go through [`insert`](Self::insert) untouched.
    pub fn insert_newline(&mut self) {
        // Typing over a selection replaces it, so the break lands at the
        // selection start; take the indent from that line.
        let at = match self.selection() {
            Some(sel) => sel.ordered().0,
            None => self.cursor,
        };
        let text = format!("\n{}", self.open_seed(at.line, at.col));
        self.insert(&text);
    }

    pub fn insert(&mut self, text: &str) {
        self.desired_col = None;
        self.display_desired_col = None;
        // Typing over a selection replaces it as one undo transaction.
        if let Some(sel) = self.selection() {
            let (start, end) = sel.ordered();
            self.cursor = self.edit(start, end, text);
            self.anchor = None;
        } else {
            self.cursor = self.insert_at(self.cursor, text);
        }
    }

    pub fn backspace(&mut self) {
        self.desired_col = None;
        self.display_desired_col = None;
        if self.delete_selection() {
            return;
        }
        if let Some(start) = self.point_before_cursor() {
            self.cursor = self.erase(start, self.cursor);
        }
    }

    pub fn delete_forward(&mut self) {
        self.desired_col = None;
        self.display_desired_col = None;
        if self.delete_selection() {
            return;
        }
        if let Some(end) = self.point_after_cursor() {
            self.cursor = self.erase(self.cursor, end);
        }
    }

    /// The position one step left of `p` (wrapping to the end of the previous
    /// line), or `None` at the start of the buffer.
    pub fn step_backward(&self, p: Point) -> Option<Point> {
        let Point { line, col } = p;
        if col > 0 {
            Some(Point { line, col: col - 1 })
        } else if line > 0 {
            Some(Point {
                line: line - 1,
                col: self.buffer.line_len(line - 1),
            })
        } else {
            None
        }
    }

    /// The position one step right of `p` (wrapping to the start of the next
    /// line), or `None` at the end of the buffer.
    pub fn step_forward(&self, p: Point) -> Option<Point> {
        let Point { line, col } = p;
        if col < self.buffer.line_len(line) {
            Some(Point { line, col: col + 1 })
        } else if line + 1 < self.buffer.line_count() {
            Some(Point {
                line: line + 1,
                col: 0,
            })
        } else {
            None
        }
    }

    /// Like [`step_backward`](Self::step_backward) but never crosses to the
    /// previous line — used when leaving Insert mode.
    pub fn step_backward_in_line(&self, p: Point) -> Point {
        Point {
            line: p.line,
            col: p.col.saturating_sub(1),
        }
    }

    fn point_before_cursor(&self) -> Option<Point> {
        self.step_backward(self.cursor)
    }

    fn point_after_cursor(&self) -> Option<Point> {
        self.step_forward(self.cursor)
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.buffer.undo() {
            self.cursor = self.buffer.clamp(p);
            self.anchor = None;
        }
        self.sync_projection();
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.buffer.redo() {
            self.cursor = self.buffer.clamp(p);
            self.anchor = None;
        }
        self.sync_projection();
    }

    /// Walk the projection's own history back to the buffer's undo position.
    /// Undoing has to restore the *origins*, not only the text: a `-` line
    /// brought back by undoing its deletion must be a deletion again, so that
    /// deleting it a second time reverts the deletion rather than dropping an
    /// addition.
    fn sync_projection(&mut self) {
        let undo_index = self.buffer.undo_index();
        let Some(p) = &mut self.projection else {
            return;
        };
        p.sync_to(undo_index);
        self.refresh_projection_styles();
    }

    /// Redo with vim's `<C-R>` caret rule: land on the start of the redone
    /// change rather than the end of the re-applied text.
    pub fn redo_vim(&mut self) {
        if let Some(p) = self.buffer.redo_vim() {
            self.cursor = self.buffer.clamp(p);
            self.anchor = None;
        }
        self.sync_projection();
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.buffer.save()
    }

    /// Write the buffer to `path`, adopting it as the pane's file ("save as").
    pub fn save_as(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.buffer.save_as(path)
    }

    /// Re-read the file from disk (replacing content, discarding undo), then
    /// pull the cursor/selection/scroll back into the refreshed buffer. Used
    /// to refresh a clean pane after an external edit (see `App::poll_files`).
    pub fn reload(&mut self) -> std::io::Result<()> {
        self.buffer.reload()?;
        self.cursor = self.buffer.clamp(self.cursor);
        self.anchor = None;
        self.desired_col = None;
        self.display_desired_col = None;
        self.scroll.top = self
            .scroll
            .top
            .min(self.buffer.line_count().saturating_sub(1));
        self.scroll.left = 0.0;
        Ok(())
    }

    /// Replace the view's content with externally-supplied text (a GPP
    /// subprocess render). Editing is driven by the host, not vim, so this
    /// resets the buffer and places the cursor at the start of `cursor_line`.
    /// The caller invokes [`ensure_cursor_visible`](Self::ensure_cursor_visible)
    /// afterwards.
    pub fn set_external_content(&mut self, text: &str, cursor_line: Option<usize>) {
        // A process pane is a passive render surface: the client computes cursor
        // and mouse rows against 1-line-per-row content, so never soft-wrap it.
        self.wrap = false;
        self.buffer = Buffer::from_str(text);
        // A projection describes the content it was built for; new content means
        // a new table (the caller attaches one if this view still has origins).
        self.projection = None;
        let last = self.buffer.line_count().saturating_sub(1);
        let line = cursor_line.unwrap_or(0).min(last);
        self.cursor = Point { line, col: 0 };
        self.anchor = None;
        self.desired_col = None;
        self.display_desired_col = None;
        // The old content's styles/backgrounds no longer apply; a `render`
        // without them (e.g. from an older client) falls back to plain rendering.
        self.external_styles.clear();
        self.external_backgrounds.clear();
        // Keep the existing scroll offset, but never strand it past the end of
        // a now-shorter buffer.
        self.scroll.top = self.scroll.top.min(last);
    }

    /// Apply per-line *semantic* styling to externally-supplied content: each
    /// `names[i]` tags line `i` with one of the palette names — `added`,
    /// `removed`, `hunk`, `title`, `dim`, `comment` — and the whole line is
    /// drawn in that foreground color, with a translucent background band for
    /// the diff kinds (`added`/`removed`/`hunk`). Any other value (including
    /// `""`) leaves the line plain. It spares the caller from computing
    /// char-column spans by spanning each whole line using the buffer's own
    /// line lengths. Call after
    /// [`set_external_content`](Self::set_external_content), which clears styling.
    /// It backs a panel `text_view`'s line-styling side channel.
    pub fn set_external_line_styles(&mut self, names: &[String]) {
        let n = names.len().min(self.buffer.line_count());
        let mut styles: Vec<Vec<StyleSpan>> = Vec::with_capacity(n);
        let mut bgs: Vec<Vec<BgSpan>> = Vec::with_capacity(n);
        for (i, name) in names.iter().take(n).enumerate() {
            let len = self.buffer.line_len(i);
            let (fg, bg): (Option<StyleKind>, Option<BgKind>) = match name.as_str() {
                "added" => (Some(StyleKind::Added), Some(BgKind::Added)),
                "removed" => (Some(StyleKind::Removed), Some(BgKind::Removed)),
                "hunk" => (Some(StyleKind::Hunk), Some(BgKind::Header)),
                "title" => (Some(StyleKind::Title), None),
                "dim" => (Some(StyleKind::Dim), None),
                "comment" => (Some(StyleKind::Comment), None),
                _ => (None, None),
            };
            styles.push(match fg {
                Some(style) if len > 0 => vec![StyleSpan {
                    start: 0,
                    end: len,
                    style,
                }],
                _ => Vec::new(),
            });
            bgs.push(match bg {
                Some(kind) if len > 0 => vec![BgSpan {
                    start: 0,
                    end: len,
                    kind,
                }],
                _ => Vec::new(),
            });
        }
        self.external_styles = styles;
        self.external_backgrounds = bgs;
    }

    /// Set (or clear) the externally-supplied display title used by
    /// [`title`](Self::title) / [`display_name`](Self::display_name) — a GPP
    /// process pane reports one at initialize and in each `render`.
    pub fn set_external_title(&mut self, title: Option<String>) {
        self.external_title = title;
    }

    /// Short file name for status messages ("foo.rs"), or "[untitled]". A
    /// process pane's external title (the browsed directory, a git view) stands
    /// in for the missing file name.
    pub fn display_name(&self) -> String {
        if let Some(title) = &self.external_title {
            return title.clone();
        }
        self.buffer
            .path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[untitled]".to_string())
    }

    // --- line-oriented edits (used by vim) ---------------------------------

    /// Delete `count` whole lines starting at `start`, including their line
    /// breaks, and place the cursor at the start of the line that takes their
    /// place. Used by `dd`.
    pub fn delete_lines(&mut self, start: usize, count: usize) {
        let line_count = self.buffer.line_count();
        if start >= line_count {
            return;
        }
        // Offer it to the projection first: deleting a hunk header is a request
        // to revert the hunk, not to remove a line of text. Every line-wise
        // delete in the editor lands here — `dd`, `3dd`, `d}`, `Vd` — so they
        // all get the structural reading from this one hook.
        match self.line_delete_intent(start, count) {
            Ok(true) => return,
            Ok(false) => {}
            Err(why) => {
                self.edit_refusal = Some(why);
                return;
            }
        }
        let end = (start + count).min(line_count);
        let (from, to) = if end < line_count {
            // Take the newline that follows the block so the lines below slide
            // up.
            (Point::new(start, 0), Point::new(end, 0))
        } else if start > 0 {
            // Deleting through the last line: also drop the preceding newline
            // so no empty line is left behind.
            (
                Point::new(start - 1, self.buffer.line_len(start - 1)),
                Point::new(end - 1, self.buffer.line_len(end - 1)),
            )
        } else {
            // The whole buffer.
            (
                Point::new(0, 0),
                Point::new(end - 1, self.buffer.line_len(end - 1)),
            )
        };
        self.erase(from, to);
        self.cursor = self.buffer.clamp(Point::new(start, 0));
        self.anchor = None;
    }

    /// Open a line below the cursor, carrying the current line's full indent
    /// (vim's `autoindent`), and move past the indent. Used by `o`.
    pub fn open_below(&mut self) {
        let line = self.cursor.line;
        let indent = self.open_seed(line, usize::MAX);
        let end = Point::new(line, self.buffer.line_len(line));
        self.cursor = self.insert_at(end, &format!("\n{indent}"));
        self.anchor = None;
    }

    /// Open a line above the cursor, carrying the current line's full indent
    /// (vim's `autoindent`), and move past the indent. Used by `O`.
    pub fn open_above(&mut self) {
        let line = self.cursor.line;
        let indent = self.open_seed(line, usize::MAX);
        self.insert_at(Point::new(line, 0), &format!("{indent}\n"));
        self.cursor = Point::new(line, indent.chars().count());
        self.anchor = None;
    }

    /// Pull the caret back onto a real character, as Normal mode requires
    /// (a line's caret may sit at most on its last character, not past it).
    /// Every normal-mode edit lands here, so it also drops the desired-column
    /// memory; vertical motions re-establish it afterwards.
    pub fn clamp_cursor_normal(&mut self) {
        let line = self
            .cursor
            .line
            .min(self.buffer.line_count().saturating_sub(1));
        let max_col = self.buffer.line_len(line).saturating_sub(1);
        self.cursor = Point::new(line, self.cursor.col.min(max_col));
        self.desired_col = None;
        self.display_desired_col = None;
    }

    // --- movement & scrolling ----------------------------------------------

    pub fn move_cursor(&mut self, m: Move, visible_lines: usize, extend: bool) {
        // Vertical moves keep (or establish) the desired column; everything
        // else snaps the memory to wherever the cursor actually lands.
        let desired = match m {
            Move::Up
            | Move::Down
            | Move::PageUp
            | Move::PageDown
            | Move::HalfPageUp
            | Move::HalfPageDown => Some(self.desired_col.unwrap_or(self.cursor.col)),
            _ => None,
        };
        self.desired_col = desired;
        // The display-space sticky column (wrap-aware arrows) only survives a
        // run of vertical moves; anything else invalidates it, like desired_col.
        if desired.is_none() {
            self.display_desired_col = None;
        }
        if extend {
            // Start a selection at the current cursor if none is active.
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else if let Some(sel) = self.selection() {
            // Left/Right collapse an active selection to its edge (the
            // standard editor behavior); other moves just drop it.
            let (start, end) = sel.ordered();
            self.anchor = None;
            match m {
                Move::Left => {
                    self.cursor = start;
                    return;
                }
                Move::Right => {
                    self.cursor = end;
                    return;
                }
                _ => {}
            }
        } else {
            self.anchor = None;
        }
        let Point { line, col } = self.cursor;
        let col = desired.unwrap_or(col);
        self.cursor = match m {
            Move::Left => self.point_before_cursor().unwrap_or(self.cursor),
            Move::Right => self.point_after_cursor().unwrap_or(self.cursor),
            Move::Up => self.buffer.clamp(Point {
                line: line.saturating_sub(1),
                col,
            }),
            Move::Down => self.buffer.clamp(Point {
                line: line + 1,
                col,
            }),
            Move::LineStart => Point { line, col: 0 },
            Move::LineEnd => Point {
                line,
                col: self.buffer.line_len(line),
            },
            Move::PageUp => self.buffer.clamp(Point {
                line: line.saturating_sub(visible_lines),
                col,
            }),
            Move::PageDown => self.buffer.clamp(Point {
                line: line + visible_lines,
                col,
            }),
            // Half-page moves mirror PageUp/PageDown at half the height (at
            // least one line, so a one-row viewport still moves).
            Move::HalfPageUp => self.buffer.clamp(Point {
                line: line.saturating_sub((visible_lines / 2).max(1)),
                col,
            }),
            Move::HalfPageDown => self.buffer.clamp(Point {
                line: line + (visible_lines / 2).max(1),
                col,
            }),
        };
    }

    // --- visual (wrapped) rows ---------------------------------------------
    //
    // With soft wrap on, one buffer line occupies several *visual rows* on
    // screen. Scrolling and cursor-visibility work in that visual-row space,
    // addressed as a `(line, sub)` pair (`sub` = 0-based wrapped row within the
    // buffer line). When wrap is off every line is a single row, so `sub` is
    // always 0 and these reduce to plain buffer-line arithmetic.

    /// The char-column starts of `line`'s visual rows at the current width.
    /// A single-element `[0]` when wrapping is off or the line fits.
    fn seg_starts(&self, line: &str, visible_cols: usize) -> Vec<usize> {
        if self.wrap {
            wrap_rows(line, visible_cols)
        } else {
            vec![0]
        }
    }

    /// How many visual rows buffer line `idx` occupies at the current width.
    /// Uses the allocation-free count path — it runs per line across the whole
    /// buffer for the scrollbar and scroll clamping.
    fn line_rows(&self, idx: usize, visible_cols: usize) -> usize {
        if self.wrap {
            wrap_row_count(&self.buffer.line(idx), visible_cols)
        } else {
            1
        }
    }

    /// The cursor's visual position `(line, sub)`: which wrapped row of its
    /// buffer line the cursor column falls on.
    fn cursor_vpos(&self, visible_cols: usize) -> (usize, usize) {
        let starts = self.seg_starts(&self.buffer.line(self.cursor.line), visible_cols);
        let sub = starts
            .iter()
            .rposition(|&s| s <= self.cursor.col)
            .unwrap_or(0);
        (self.cursor.line, sub)
    }

    /// Step a visual position forward by `n` rows, clamped to the buffer's last
    /// visual row.
    fn vpos_add(&self, mut line: usize, mut sub: usize, n: usize, cols: usize) -> (usize, usize) {
        let count = self.buffer.line_count();
        for _ in 0..n {
            if sub + 1 < self.line_rows(line, cols) {
                sub += 1;
            } else if line + 1 < count {
                line += 1;
                sub = 0;
            } else {
                break;
            }
        }
        (line, sub)
    }

    /// Step a visual position backward by `n` rows, clamped to `(0, 0)`.
    fn vpos_sub(&self, line: usize, sub: usize, n: usize, cols: usize) -> (usize, usize) {
        self.vpos_sub_sat(line, sub, n, cols).0
    }

    /// [`vpos_sub`](Self::vpos_sub) also reporting whether it ran into the top
    /// of the buffer before taking all `n` steps. A smooth scroll needs to
    /// know: on a clamped step the fractional remainder is no longer a position
    /// *within* the anchor row but a leftover that would push the view back
    /// down, so the caller drops it.
    fn vpos_sub_sat(
        &self,
        mut line: usize,
        mut sub: usize,
        n: usize,
        cols: usize,
    ) -> ((usize, usize), bool) {
        for _ in 0..n {
            if sub > 0 {
                sub -= 1;
            } else if line > 0 {
                line -= 1;
                sub = self.line_rows(line, cols) - 1;
            } else {
                return ((line, sub), true);
            }
        }
        ((line, sub), false)
    }

    /// The buffer's last visual row.
    fn end_vpos(&self, cols: usize) -> (usize, usize) {
        let last = self.buffer.line_count().saturating_sub(1);
        (last, self.line_rows(last, cols).saturating_sub(1))
    }

    /// The cursor's **display** column within its current visual (wrapped) row:
    /// the on-screen x-offset (tabs expanded) measured from that row's left
    /// edge, so every visual row's columns count from 0. This is the sticky
    /// quantity for `gj`/`gk` (see [`display_line_target`](Self::display_line_target)).
    pub(crate) fn cursor_display_col(&self, visible_cols: usize) -> usize {
        let cols = visible_cols.max(1);
        let line = self.buffer.line(self.cursor.line);
        let (_, sub) = self.cursor_vpos(cols);
        let starts = self.seg_starts(&line, cols);
        let s = starts.get(sub).copied().unwrap_or(0);
        // The row's text (tab stops restart at column 0, matching how the row is
        // drawn); the cursor sits at char offset `cursor.col - s` within it.
        let seg: String = line.chars().skip(s).collect();
        display_col(&seg, self.cursor.col.saturating_sub(s))
    }

    /// The char-column range `[start, end)` of the cursor's current visual
    /// (soft-wrapped) row. With wrap off, or a line that fits, this is the whole
    /// logical line (`0..line_len`) — so the wrap-aware Home/End motions built on
    /// it reduce to plain line-start / line-end.
    pub(crate) fn cursor_visual_row_range(&self, visible_cols: usize) -> (usize, usize) {
        let cols = visible_cols.max(1);
        let line = self.buffer.line(self.cursor.line);
        let starts = self.seg_starts(&line, cols);
        let (_, sub) = self.cursor_vpos(cols);
        let s = starts.get(sub).copied().unwrap_or(0);
        let e = starts
            .get(sub + 1)
            .copied()
            .unwrap_or_else(|| line.chars().count());
        (s, e)
    }

    /// Move the cursor one visual (soft-wrapped) row up or down, keeping a
    /// sticky display column — the wrap-aware core shared by the arrow keys in
    /// both Normal and Insert mode. `extend` grows a selection from the current
    /// anchor; `clamp_normal` pulls the caret back onto a real character
    /// (Normal/Visual) — Insert passes `false` so the caret may rest at
    /// end-of-line. With wrap off this reduces to a whole-line move.
    pub(crate) fn move_display_row(&mut self, up: bool, visible_cols: usize, extend: bool) {
        let cols = visible_cols.max(1);
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        let desired = self
            .display_desired_col
            .unwrap_or_else(|| self.cursor_display_col(cols));
        self.cursor = self.display_line_target(up, 1, cols, desired);
        self.display_desired_col = Some(desired);
        self.desired_col = None;
    }

    /// Target cursor position for a display-line motion (`gj`/`gk`): step the
    /// cursor's visual position by `count` wrapped rows (up when `up`, else
    /// down), then land at `desired_display_col` within the destination visual
    /// row — clamped to that row's char range. On a non-final wrapped row the
    /// landing column stops one short of the row's exclusive end, so the cursor
    /// stays on that row rather than spilling onto the next.
    ///
    /// With wrap off, or a line that fits in one row, `seg_starts` collapses to
    /// `[0]`: every buffer line is a single visual row, `vpos_add`/`vpos_sub`
    /// step whole lines, and this reduces exactly to `j`/`k` at the desired
    /// display column.
    pub(crate) fn display_line_target(
        &self,
        up: bool,
        count: usize,
        visible_cols: usize,
        desired_display_col: usize,
    ) -> Point {
        let cols = visible_cols.max(1);
        let (line, sub) = self.cursor_vpos(cols);
        let (tline, tsub) = if up {
            self.vpos_sub(line, sub, count, cols)
        } else {
            self.vpos_add(line, sub, count, cols)
        };
        let text = self.buffer.line(tline);
        let starts = self.seg_starts(&text, cols);
        let s = starts.get(tsub).copied().unwrap_or(0);
        let e = starts
            .get(tsub + 1)
            .copied()
            .unwrap_or_else(|| text.chars().count());
        // The destination row's text, restarting tab stops at display column 0.
        let seg: String = text.chars().skip(s).take(e.saturating_sub(s)).collect();
        let within = char_col_for_display(&seg, desired_display_col as f32);
        // On a continued (non-final) wrapped row the char at `e` begins the next
        // row, so cap the landing column at `e - 1` to stay on this row; the
        // final row may sit at end-of-line (Normal mode clamps it back).
        let is_last_seg = tsub + 1 >= starts.len();
        let max_col = if is_last_seg { e } else { e.saturating_sub(1) };
        let col = (s + within).min(max_col);
        self.buffer.clamp(Point::new(tline, col))
    }

    /// The furthest the view may scroll: the top position that pins the last
    /// visual row to the bottom of the viewport, so no blank rows show below.
    fn max_scroll(&self, visible_lines: usize, cols: usize) -> (usize, usize) {
        let (line, sub) = self.end_vpos(cols);
        self.vpos_sub(line, sub, visible_lines.max(1) - 1, cols)
    }

    /// Whether the content overflows a viewport of `visible_lines` rows (at
    /// `visible_cols` wrap width) — i.e. the view has any vertical scroll
    /// range at all. This is a property of the content, independent of the
    /// current scroll offset.
    pub fn can_scroll_v(&self, visible_lines: usize, visible_cols: usize) -> bool {
        self.max_scroll(visible_lines.max(1), visible_cols.max(1)) > (0, 0)
    }

    /// Whether the content overflows a viewport of `visible_cols` columns
    /// horizontally. Only possible with wrap off (wrapping pins the horizontal
    /// offset to 0).
    pub fn can_scroll_h(&self, visible_cols: usize) -> bool {
        !self.wrap && self.max_line_width() > visible_cols.max(1)
    }

    /// Pull the scroll position back to `max_scroll` if it ran past it. The
    /// last row is pinned flush to the bottom of the viewport, so the clamped
    /// position carries no fraction.
    fn clamp_scroll(&mut self, visible_lines: usize, cols: usize) {
        let max = self.max_scroll(visible_lines, cols);
        if self.scroll.past(max) {
            self.scroll.set_vpos(max);
        }
    }

    /// Total visual rows in the buffer at the current width (buffer lines when
    /// wrap is off). Drives the vertical scrollbar.
    fn total_rows(&self, cols: usize) -> usize {
        if !self.wrap {
            return self.buffer.line_count();
        }
        (0..self.buffer.line_count())
            .map(|i| self.line_rows(i, cols))
            .sum()
    }

    /// The visual-row offset of the top of the viewport (buffer line when wrap
    /// is off), fractional part included. Drives the vertical scrollbar thumb
    /// position, which therefore tracks a smooth scroll continuously instead of
    /// jumping a row at a time.
    fn scroll_offset_rows(&self, cols: usize) -> f32 {
        let whole = if self.wrap {
            (0..self.scroll.top)
                .map(|i| self.line_rows(i, cols))
                .sum::<usize>()
                + self.scroll.sub
        } else {
            self.scroll.top
        };
        whole as f32 + self.scroll.frac
    }

    /// Scroll so the given absolute visual-row `offset` — fractional, so a
    /// scrollbar drag moves by the pixel and not by the row — is at the top.
    /// Clamped to the valid range.
    fn set_scroll_rows(&mut self, offset: f32, visible_lines: usize, cols: usize) {
        let offset = offset.max(0.0);
        self.scroll
            .set_vpos(self.vpos_add(0, 0, offset.floor() as usize, cols));
        self.scroll.frac = offset.fract();
        self.clamp_scroll(visible_lines, cols);
    }

    // --- scrolling ---------------------------------------------------------

    /// Scroll vertically by `rows` visual rows (negative = up), **fractional**:
    /// `0.25` moves the view a quarter of a line, which is what a trackpad's
    /// pixel deltas amount to. `visible_cols` is the wrap width; it is ignored
    /// when wrapping is off.
    ///
    /// The fractional part is folded into the existing one and the whole rows
    /// that fall out of the sum are walked in visual-row space, so repeated
    /// sub-row scrolls accumulate exactly rather than each rounding to nothing.
    pub fn scroll_by(&mut self, rows: f32, visible_lines: usize, visible_cols: usize) {
        let cols = visible_cols.max(1);
        let target = self.scroll.frac + rows;
        // `floor` (not truncation) so a negative target borrows a whole row and
        // leaves a positive fraction: -0.25 rows is "one row up, three quarters
        // of the way down it".
        let whole = target.floor();
        self.scroll.frac = target - whole;
        if whole >= 0.0 {
            let vpos = self.vpos_add(self.scroll.top, self.scroll.sub, whole as usize, cols);
            (self.scroll.top, self.scroll.sub) = vpos;
        } else {
            let (vpos, hit_top) =
                self.vpos_sub_sat(self.scroll.top, self.scroll.sub, -whole as usize, cols);
            (self.scroll.top, self.scroll.sub) = vpos;
            if hit_top {
                // The scroll ran out of buffer: park at the very first row
                // rather than leaving a fraction that reads as scrolled-down.
                self.scroll.frac = 0.0;
            }
        }
        self.clamp_scroll(visible_lines, cols);
    }

    /// Scroll so buffer `line` (0-based) is the first visible one, and put the
    /// cursor there — programmatic navigation from outside the view (a panel's
    /// `text_view_scroll_to`, e.g. a file list jumping a diff to one file).
    /// Clamped to the buffer, so a stale line number lands on the last line
    /// rather than scrolling into nothing.
    ///
    /// Deliberately **not** clamped to the last screenful, unlike every other
    /// scroll here. This is an anchor, not a scroll: a file list that jumps a
    /// diff to a file has to put that file's header on the top row, and the
    /// ordinary clamp made that impossible for every file within a screenful of
    /// the end — clicking one of the last few files left the view sitting on
    /// whatever the last screenful happened to start with (a line belonging to
    /// some earlier file), which reads as "the jump went to the wrong place".
    /// Trailing blank rows below the last line are the price, and they are the
    /// same price every editor pays for jump-to-symbol.
    pub fn scroll_to_line(&mut self, line: usize, visible_cols: usize) {
        let cols = visible_cols.max(1);
        let line = line.min(self.buffer.line_count().saturating_sub(1));
        self.cursor = Point { line, col: 0 };
        self.anchor = None;
        self.scroll.set_vpos((line, 0));
        self.cols_hint.set(cols);
    }

    /// Reposition the viewport so the cursor's visual row sits at the top
    /// (`zt`), center (`zz`), or bottom (`zb`) of the visible area, vim-style.
    /// The cursor itself does not move. Clamped so the view never scrolls past
    /// the last line, matching the rest of garden's scrolling.
    pub fn scroll_cursor_to(&mut self, align: ScrollAlign, visible_lines: usize) {
        let visible_lines = visible_lines.max(1);
        let cols = self.cols_hint.get().max(1);
        let (cl, cs) = self.cursor_vpos(cols);
        self.scroll.set_vpos(match align {
            ScrollAlign::Top => (cl, cs),
            ScrollAlign::Center => self.vpos_sub(cl, cs, visible_lines / 2, cols),
            ScrollAlign::Bottom => self.vpos_sub(cl, cs, visible_lines - 1, cols),
        });
        self.clamp_scroll(visible_lines, cols);
    }

    /// Roll the viewport `n` visual rows down (`down`) or up, leaving the
    /// cursor where it is — vim's `Ctrl+E` / `Ctrl+Y`. The cursor moves only
    /// when the roll would carry it off screen, where it is dragged along the
    /// leading edge, keeping its column. That drag is not cosmetic: the caller
    /// runs `ensure_cursor_visible` after every key, so a cursor left outside
    /// the rolled viewport would scroll it straight back and the roll would
    /// read as a no-op.
    pub fn scroll_view_lines(
        &mut self,
        down: bool,
        n: usize,
        visible_lines: usize,
        visible_cols: usize,
    ) {
        let vis = visible_lines.max(1);
        let cols = visible_cols.max(1);
        let rows = n as f32;
        self.scroll_by(if down { rows } else { -rows }, vis, cols);
        let top = self.scroll.vpos();
        let bottom = self.vpos_add(top.0, top.1, vis - 1, cols);
        let cur = self.cursor_vpos(cols);
        let edge = if cur < top {
            Some(top)
        } else if cur > bottom {
            Some(bottom)
        } else {
            None
        };
        if let Some(v) = edge {
            self.cursor = self.vpos_point_at_col(v, self.cursor.col, cols);
            self.anchor_or_clamp();
        }
    }

    /// Normal mode never rests past a line's last character; Visual mode is
    /// mid-selection and must keep its anchor. Shared by the cursor drags that
    /// place the caret directly rather than through [`move_cursor`](Self::move_cursor).
    fn anchor_or_clamp(&mut self) {
        self.desired_col = None;
        self.display_desired_col = None;
        if self.vim.mode == crate::vim::Mode::Normal {
            self.clamp_cursor_normal();
        }
    }

    /// The buffer point on visual row `(line, sub)` nearest display column
    /// `col` — where a cursor dragged onto that row lands. With wrap off every
    /// line is one row and this is just the clamped column.
    fn vpos_point_at_col(&self, (line, sub): (usize, usize), col: usize, cols: usize) -> Point {
        let text = self.buffer.line(line);
        let starts = self.seg_starts(&text, cols);
        let start = starts.get(sub).copied().unwrap_or(0);
        // A continuation row ends one column before the next row begins;
        // landing *on* that column would put the caret on the next row and
        // leave it off screen again.
        let end = match starts.get(sub + 1) {
            Some(&next) => next.saturating_sub(1),
            None => text.chars().count(),
        };
        self.buffer.clamp(Point::new(line, col.clamp(start, end)))
    }

    /// Scroll horizontally by `cols` display columns, fractional like
    /// [`scroll_by`](Self::scroll_by). A no-op while wrapping is on, which pins
    /// the horizontal offset to 0.
    pub fn scroll_h_by(&mut self, cols: f32) {
        if self.wrap {
            return;
        }
        let max = self.max_line_width().saturating_sub(1) as f32;
        self.scroll.left = (self.scroll.left + cols).clamp(0.0, max);
    }

    /// Scroll just enough that the cursor is on screen on both axes.
    pub fn ensure_cursor_visible(&mut self, visible_lines: usize, visible_cols: usize) {
        let visible_lines = visible_lines.max(1);
        let cols = visible_cols.max(1);
        self.cols_hint.set(cols);

        // Vertical: keep the cursor's visual row within the viewport.
        let cursor = self.cursor_vpos(cols);
        let top = self.scroll.vpos();
        // Only a scroll this call actually performed is pulled back to the last
        // screenful (which is what keeps a shrinking buffer from stranding the
        // view below its own content). A viewport already showing its cursor is
        // left exactly where it is: `scroll_to_line` anchors past the end on
        // purpose (see there), and clamping a view that needed no scrolling
        // would undo that jump on the next keystroke.
        //
        // A fractional offset makes the anchor row and the last row *partly*
        // visible, so both count as off screen when the cursor lands on them:
        // the view snaps to a whole row rather than leaving the caret sliced by
        // the viewport edge. Every correction here lands on `frac == 0` for
        // that reason.
        let bottom = self.vpos_add(top.0, top.1, visible_lines - 1, cols);
        let clipped_edge = self.scroll.frac > 0.0;
        let mut moved = false;
        if cursor < top || (cursor == top && clipped_edge) {
            self.scroll.set_vpos(cursor);
            moved = true;
        } else if cursor > bottom || (cursor == bottom && clipped_edge) {
            self.scroll
                .set_vpos(self.vpos_sub(cursor.0, cursor.1, visible_lines - 1, cols));
            moved = true;
        }
        if moved {
            self.clamp_scroll(visible_lines, cols);
        }

        // Horizontal scrolling only applies when not wrapping (tabs expanded).
        if self.wrap {
            self.scroll.left = 0.0;
            return;
        }
        // Both edges are compared against the caret's *whole* cell, so a
        // fractional offset that slices the caret column scrolls it back into
        // view instead of leaving half a caret against the edge.
        let col = display_col(&self.buffer.line(self.cursor.line), self.cursor.col) as f32;
        if col < self.scroll.left {
            self.scroll.left = col;
        } else if col + 1.0 > self.scroll.left + cols as f32 {
            self.scroll.left = col + 1.0 - cols as f32;
        }
    }

    /// Turn soft-wrapping on or off, re-deriving the scroll offsets under the
    /// new mode so the cursor stays on screen. The two modes measure the
    /// viewport differently — wrapped views scroll by visual sub-row and never
    /// horizontally, unwrapped ones the reverse — so the offset that does not
    /// apply is dropped rather than left to mean something else on the way back.
    /// A no-op when the mode is already `on`, so it is safe to call every frame.
    pub fn set_wrap(&mut self, on: bool, visible_lines: usize, visible_cols: usize) {
        if self.wrap == on {
            return;
        }
        self.wrap = on;
        if on {
            self.scroll.left = 0.0;
        } else {
            // Sub-rows only exist while wrapping; the fractional offset within
            // the anchor row survives, since rows are rows in both modes.
            self.scroll.sub = 0;
        }
        self.ensure_cursor_visible(visible_lines, visible_cols);
    }

    // --- geometry ----------------------------------------------------------

    pub fn visible_lines(rect: Rect, cell_h: f32) -> usize {
        (((rect.h - 2.0 * PAD) / cell_h).floor() as usize).max(1)
    }

    /// Number of text columns that fit beside the gutter.
    pub fn visible_cols(&self, rect: Rect, cell_w: f32) -> usize {
        let text_w = rect.w - 2.0 * PAD - self.gutter_cols() as f32 * cell_w;
        ((text_w / cell_w).floor() as usize).max(1)
    }

    /// Width of the longest line, in display columns (tabs expanded per tab
    /// stop, matching the rendered text and scroll geometry).
    fn max_line_width(&self) -> usize {
        (0..self.buffer.line_count())
            .map(|i| display_width(&self.buffer.line(i)))
            .max()
            .unwrap_or(0)
    }

    /// Columns taken by the line-number gutter (0 when it is off).
    fn number_cols(&self) -> usize {
        if !self.show_line_numbers {
            return 0;
        }
        let digits = self.buffer.line_count().to_string().len();
        digits.max(3) + 2 // right-aligned digits plus one space of margin each side
    }

    /// Whether this view draws the **diff-marker** column: an attached
    /// projection whose decoration lives in the gutter rather than in the text
    /// (see [`garden_core::projection::Decor::gutter`]). The markers are drawn
    /// here precisely so they are *not* in the buffer, where every edit,
    /// selection and search would have to step around them.
    fn marker_gutter(&self) -> bool {
        // Deliberately a flag check and not a look at the marker track: this
        // sits under `visible_cols` / `text_origin` / `position_for_click`,
        // which run per frame and per click, and building the track allocates.
        self.projection.as_ref().is_some_and(|p| p.decor.gutter)
    }

    /// Columns taken by the diff-marker column: the glyph plus a space of
    /// margin, so the text does not start flush against a `+`.
    fn marker_cols(&self) -> usize {
        if self.marker_gutter() {
            2
        } else {
            0
        }
    }

    /// Total gutter width: line numbers, then diff markers, then the text.
    fn gutter_cols(&self) -> usize {
        self.number_cols() + self.marker_cols()
    }

    /// The color line `line_idx`'s text is painted in, for chrome that has to
    /// match it — the diff marker in the gutter. Falls back to the dim gutter
    /// color for a line with no style of its own.
    fn line_style_color(&self, line_idx: usize, theme: &theme::Theme) -> Color {
        self.external_styles
            .get(line_idx)
            .and_then(|spans| spans.first())
            .map(|s| style_color(s.style, theme))
            .unwrap_or(theme.text_dim)
    }

    /// Top-left of the first visible text cell.
    fn text_origin(&self, rect: Rect, cell_w: f32) -> (f32, f32) {
        (
            rect.x + PAD + self.gutter_cols() as f32 * cell_w,
            rect.y + PAD,
        )
    }

    /// Map a window-space click position to pane focus-local cursor placement.
    /// The horizontal position is a display column (tabs expanded), mapped to
    /// the nearest char boundary via [`char_col_for_display`].
    pub fn position_for_click(&self, rect: Rect, cell: (f32, f32), x: f32, y: f32) -> Point {
        let (cell_w, cell_h) = cell;
        let (ox, oy) = self.text_origin(rect, cell_w);
        let cols = self.visible_cols(rect, cell_w);
        // Which visual row was clicked, relative to the anchor row — the
        // fraction the view is scrolled by shifts the rows up under the
        // pointer, so it is added back before flooring.
        let vrow = ((y - oy) / cell_h + self.scroll.frac).floor().max(0.0) as usize;
        let (line, sub) = self.vpos_add(self.scroll.top, self.scroll.sub, vrow, cols);
        // The clicked visual row's char span within its buffer line.
        let full = self.buffer.line(line);
        let starts = self.seg_starts(&full, cols);
        let s = starts.get(sub).copied().unwrap_or(0);
        let e = starts
            .get(sub + 1)
            .copied()
            .unwrap_or_else(|| full.chars().count());
        let seg: String = full.chars().skip(s).take(e - s).collect();
        // Horizontal offset only applies when not wrapping.
        let scroll_left = if self.wrap { 0.0 } else { self.scroll.left };
        let dx = scroll_left + ((x - ox) / cell_w).max(0.0);
        let col = s + char_col_for_display(&seg, dx);
        self.buffer.clamp(Point { line, col })
    }

    // --- mouse selection ----------------------------------------------------

    /// The contiguous run of same-class characters around `p` (word chars,
    /// whitespace, or punctuation — vim's `w`-motion classification). A point
    /// at/past end-of-line targets the line's last character; an empty line
    /// yields an empty range.
    pub(crate) fn word_range_at(&self, p: Point) -> (Point, Point) {
        let p = self.buffer.clamp(p);
        let len = self.buffer.line_len(p.line);
        if len == 0 {
            return (p, p);
        }
        let col = p.col.min(len - 1);
        let class = |c: usize| crate::vim::char_class(self, Point::new(p.line, c));
        let cls = class(col);
        let mut start = col;
        while start > 0 && class(start - 1) == cls {
            start -= 1;
        }
        let mut end = col + 1;
        while end < len && class(end) == cls {
            end += 1;
        }
        (Point::new(p.line, start), Point::new(p.line, end))
    }

    /// The whole of `line` including its trailing newline (so replacing the
    /// selection swallows the line break); the last line ends at end-of-line.
    pub(crate) fn line_range_at(&self, line: usize) -> (Point, Point) {
        let line = line.min(self.buffer.line_count().saturating_sub(1));
        let end = if line + 1 < self.buffer.line_count() {
            Point::new(line + 1, 0)
        } else {
            Point::new(line, self.buffer.line_len(line))
        };
        (Point::new(line, 0), end)
    }

    /// Single-click convenience over
    /// [`begin_drag_with_clicks`](Self::begin_drag_with_clicks); real input
    /// always goes through the click-count form.
    #[cfg(test)]
    pub fn begin_drag(&mut self, p: Point, extend: bool) {
        self.begin_drag_with_clicks(p, extend, 1);
    }

    /// Mouse press at `p` with an explicit click count (counted at the
    /// frontend boundary, see [`crate::app::ClickCounter`]): 1 places the
    /// cursor and starts a (potential) character-wise selection, 2 selects
    /// the word under `p`, 3+ selects the whole line including its newline.
    /// With `extend` (shift-click), the existing caret/anchor becomes the
    /// fixed end instead, regardless of the count.
    pub fn begin_drag_with_clicks(&mut self, p: Point, extend: bool, clicks: u32) {
        self.desired_col = None;
        self.display_desired_col = None;
        if extend {
            self.drag = DragMode::Caret;
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
            self.cursor = p;
            return;
        }
        let pivot = match clicks {
            0 | 1 => {
                self.drag = DragMode::Caret;
                self.anchor = Some(p);
                self.cursor = p;
                return;
            }
            2 => {
                let pivot = self.word_range_at(p);
                self.drag = DragMode::Word { pivot };
                pivot
            }
            _ => {
                let pivot = self.line_range_at(p.line);
                self.drag = DragMode::Line { pivot };
                pivot
            }
        };
        self.anchor = Some(pivot.0);
        self.cursor = pivot.1;
    }

    /// Mouse drag: move the selection head, keeping the anchor. In a word or
    /// line drag the selection instead becomes the span from the pivot to the
    /// word/line under the mouse.
    pub fn drag_to(&mut self, p: Point) {
        match self.drag {
            DragMode::Caret => self.cursor = p,
            DragMode::Word { pivot } => self.extend_drag(self.word_range_at(p), pivot),
            DragMode::Line { pivot } => self.extend_drag(self.line_range_at(p.line), pivot),
        }
    }

    /// Select from the fixed pivot range out to `range`, putting the head on
    /// the moving end; inside the pivot the selection is the pivot itself.
    fn extend_drag(&mut self, range: (Point, Point), pivot: (Point, Point)) {
        if range.0 < pivot.0 {
            self.anchor = Some(pivot.1);
            self.cursor = range.0;
        } else if range.1 > pivot.1 {
            self.anchor = Some(pivot.0);
            self.cursor = range.1;
        } else {
            self.anchor = Some(pivot.0);
            self.cursor = pivot.1;
        }
    }

    /// Mouse release: collapse a zero-width selection back to a bare caret.
    pub fn end_drag(&mut self) {
        self.drag = DragMode::Caret;
        if self.selection().is_none() {
            self.anchor = None;
        }
    }

    // --- rendering ---------------------------------------------------------

    /// Append this pane's primitives: background, border, gutter, text lines,
    /// cursor. Tabs expand to real tab stops (see [`display_col`]); every
    /// column-to-pixel conversion here goes through that mapping so quads,
    /// carets, and glyphs stay aligned on lines containing tabs.
    pub fn build_scene(
        &self,
        rect: Rect,
        cell: (f32, f32),
        focused: bool,
        theme: &theme::Theme,
        prims: &mut Vec<Primitive>,
    ) {
        let (cell_w, cell_h) = cell;
        let (ox, oy) = self.text_origin(rect, cell_w);
        let visible = Self::visible_lines(rect, cell_h);

        prims.push(Primitive::Quad {
            rect,
            color: if focused {
                theme.pane_bg_focused
            } else {
                theme.pane_bg
            },
        });
        border(
            rect,
            if focused {
                theme.border_focused
            } else {
                theme.border
            },
            prims,
        );

        // The scrolled content is drawn shifted by a sub-cell offset on both
        // axes, which puts a partial row past the top edge and a partial row
        // past the bottom one. Everything below is therefore clipped to the
        // *content band* — the rows' own area, inside the padding — rather than
        // to the pane box: `sy` alone would spill glyphs into the padding and
        // over the pane border.
        let content_h = (rect.h - 2.0 * PAD).max(0.0);
        let clip = Rect {
            x: rect.x + 1.0,
            y: oy,
            w: (rect.w - 2.0).max(0.0),
            h: content_h,
        };
        // Text, selection and the caret scroll horizontally; the gutter stays
        // fixed. Clip the scrolling content to the area right of the gutter so
        // it never bleeds over the line numbers.
        let text_clip = Rect {
            x: ox,
            y: oy,
            w: (rect.x + rect.w - PAD - ox).max(0.0),
            h: content_h,
        };
        // Sub-cell scroll offsets in pixels: every row is drawn `sy` higher and
        // `sx` further left than its whole-cell position. This is where smooth
        // scrolling actually happens — the GPU rasterizes quads and glyphs at
        // fractional positions natively, so there is nothing to snap.
        // Horizontal scroll only applies when not wrapping.
        let sx = if self.wrap {
            0.0
        } else {
            self.scroll.left * cell_w
        };
        let sy = self.scroll.frac * cell_h;

        let number_w = self.number_cols() as f32;
        // The diff-marker glyphs, one per buffer line, when this view is a
        // gutter-mode projection. Built once per paint (it is derived from the
        // origin table, so it is always in step with the buffer) and empty for
        // every other view, where the loop below skips the column entirely.
        let markers = if self.marker_gutter() {
            self.projection
                .as_ref()
                .map(|p| p.line_markers())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let selection = self.render_selection();
        let visible_cols = self.visible_cols(rect, cell_w);
        self.cols_hint.set(visible_cols);
        // The active search pattern to highlight, if any (matched per line below).
        let search_pattern = self
            .vim
            .last_search
            .as_deref()
            .filter(|_| self.vim.search_hl);

        // Refresh cached syntax spans if the buffer changed (or this is the
        // first paint). Held in a RefCell so this `&self` builder can update it.
        let hl = self.highlight.borrow();
        let hl = if hl.lang.is_none() && hl.rev.is_none() {
            // First paint: resolve the language and force a compute below.
            drop(hl);
            let mut hl = self.highlight.borrow_mut();
            hl.lang = self.buffer.path().and_then(syntax::Language::from_path);
            drop(hl);
            self.highlight.borrow()
        } else {
            hl
        };
        let hl = if hl.lang.is_some() && hl.rev != Some(self.buffer.revision()) {
            drop(hl);
            let mut hl = self.highlight.borrow_mut();
            let lang = hl.lang.expect("lang is Some");
            let rev = self.buffer.revision();
            let text = self.buffer.to_string();
            hl.lines = hl.highlighter.highlight_lines(lang, &text);
            hl.rev = Some(rev);
            drop(hl);
            self.highlight.borrow()
        } else {
            hl
        };

        // Iterate the buffer's *visual* rows from the scroll position: one
        // buffer line may span several wrapped rows. `seg_i` walks the wrapped
        // sub-rows of the current line; `vrow` is the on-screen row.
        let line_count = self.buffer.line_count();
        let (cursor_line, cursor_sub) = self.cursor_vpos(visible_cols);
        let mut caret: Option<(f32, f32)> = None;
        // Rows are drawn until one would start below the content band. That is
        // one more than `visible` whenever the view sits between two rows —
        // the sliver `sy` opens up at the bottom has to be filled, or a smooth
        // scroll would show a blank strip there.
        let rows_to_draw = ((content_h + sy) / cell_h).ceil() as usize;
        let mut vrow = 0usize;
        let mut line_idx = self.scroll.top;
        let mut seg_i = self.scroll.sub;
        while line_idx < line_count && vrow < rows_to_draw {
            let line = self.buffer.line(line_idx);
            let line_len = self.buffer.line_len(line_idx);
            let starts = self.seg_starts(&line, visible_cols);
            // A stale scroll_sub (the line shortened) clamps to its last row.
            seg_i = seg_i.min(starts.len().saturating_sub(1));

            // Per-buffer-line data, computed once and sliced per visual row.
            let sel_on_line = selection.and_then(|sel| sel.cols_on_line(line_idx, line_len));
            // The direct-manipulation highlight is a range like a selection, so
            // it slices per line the same way — but it is drawn under the text
            // with the search-match band rather than as a selection, because it
            // is the *canvas's* idea of where you are, not the user's.
            let trace_on_line = self
                .trace_highlight
                .and_then(|(s, e)| Selection::new(s, e).cols_on_line(line_idx, line_len));
            let matches: Vec<(usize, usize)> = match search_pattern {
                Some(pat) => search::matches_in_lines(
                    &self.buffer,
                    line_idx..line_idx + 1,
                    pat,
                    self.vim.last_search_word,
                )
                .into_iter()
                .map(|(p, len)| (p.col, len))
                .collect(),
                None => Vec::new(),
            };
            // Externally-supplied (GPP) styles replace syntax highlighting when
            // present — a process pane has no language to highlight anyway.
            let runs: Vec<ColorRun> = if !self.external_styles.is_empty() {
                self.external_styles
                    .get(line_idx)
                    .map(|spans| {
                        spans
                            .iter()
                            .map(|s| (s.start, s.end, style_color(s.style, theme)))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                hl.lines
                    .get(line_idx)
                    .map(|spans| {
                        spans
                            .iter()
                            .map(|s| (s.start_col, s.end_col, s.kind.color(theme)))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            // Externally-supplied (GPP) background bands for this line, if any:
            // column ranges tinted under the text (diff rows, comment blocks).
            let bgs: Vec<ColorRun> = self
                .external_backgrounds
                .get(line_idx)
                .map(|spans| {
                    spans
                        .iter()
                        .map(|s| (s.start, s.end, bg_color(s.kind, theme)))
                        .collect()
                })
                .unwrap_or_default();

            while seg_i < starts.len() && vrow < rows_to_draw {
                let s = starts[seg_i];
                let e = starts.get(seg_i + 1).copied().unwrap_or(line_len);
                let is_last_seg = seg_i + 1 == starts.len();
                // The row's top edge, shifted up by the sub-cell scroll offset:
                // the first and last rows drawn straddle the content band's
                // edges, and every quad below is clipped to it.
                let y = oy + vrow as f32 * cell_h - sy;
                let seg: String = line.chars().skip(s).take(e - s).collect();
                // Half-open helper: x of char column `c` (buffer-relative) on
                // this visual row, in pixels, with horizontal scroll applied.
                let cell_x = |c: usize| ox + display_col(&seg, c - s) as f32 * cell_w - sx;
                // This row's slot in the text area, before clipping.
                let row_span = |x0: f32, x1: f32| Rect {
                    x: x0,
                    y,
                    w: x1 - x0,
                    h: cell_h,
                };

                // Externally-supplied background bands, clipped to this visual
                // row's char span. Drawn first so text, selection and the caret
                // composite over them (a comment/diff tint under everything).
                for (bstart, bend, color) in &bgs {
                    let a = (*bstart).max(s);
                    let b = (*bend).min(e);
                    if b > a {
                        push_clipped_quad(prims, row_span(cell_x(a), cell_x(b)), text_clip, *color);
                    }
                }

                // The cursor-line highlight covers every visual row of the line.
                if focused && line_idx == self.cursor.line {
                    push_clipped_quad(
                        prims,
                        row_span(clip.x, clip.x + clip.w),
                        clip,
                        theme.cursor_line,
                    );
                }

                // The error-line highlight: a reddish full-width band on the line
                // a compile/runtime error points at (derived from error_text so it
                // tracks the palette). Drawn regardless of focus so a broken buffer
                // is visible even in an unfocused pane.
                if self.error_line == Some(line_idx) {
                    push_clipped_quad(
                        prims,
                        row_span(clip.x, clip.x + clip.w),
                        clip,
                        Color {
                            a: 0.16,
                            ..theme.error_text
                        },
                    );
                }

                // The direct-manipulation highlight: the span of the `draw_*`
                // call whose shape the pointer is over on the paired canvas.
                // Drawn regardless of focus — the pointer is in the *other*
                // pane, so this pane is by definition not the focused one.
                if let Some((tstart, tend, _)) = trace_on_line {
                    let a = tstart.max(s);
                    let b = tend.min(e);
                    if b > a {
                        push_clipped_quad(
                            prims,
                            row_span(cell_x(a), cell_x(b)),
                            text_clip,
                            Color {
                                a: 0.30,
                                ..theme.search_match
                            },
                        );
                    }
                }

                // Search matches, clipped to this visual row's char span.
                for (mstart, mlen) in &matches {
                    let a = (*mstart).max(s);
                    let b = (*mstart + *mlen).min(e);
                    if b > a {
                        push_clipped_quad(
                            prims,
                            row_span(cell_x(a), cell_x(b)),
                            text_clip,
                            theme.search_match,
                        );
                    }
                }

                // Selection, clipped to this row; the newline half-cell tail
                // (a continued selection) shows only on the line's last row.
                if let Some((start_col, end_col, newline)) = sel_on_line {
                    let a = start_col.max(s);
                    let b = end_col.min(e);
                    let tail = newline && is_last_seg && end_col >= e;
                    if b > a || tail {
                        let cols = display_col(&seg, b - s) as f32
                            - display_col(&seg, a - s) as f32
                            + if tail { 0.5 } else { 0.0 };
                        let x0 = cell_x(a);
                        push_clipped_quad(
                            prims,
                            row_span(x0, x0 + cols * cell_w),
                            text_clip,
                            theme.selection,
                        );
                    }
                }

                // Gutter line number: only on the line's first visual row.
                if self.show_line_numbers && seg_i == 0 {
                    let number = format!("{:>width$}", line_idx + 1, width = number_w as usize - 2);
                    prims.push(Primitive::Text {
                        pos: (rect.x + PAD, y),
                        text: number,
                        color: theme.text_dim,
                        clip,
                        size: FONT_SIZE,
                        style: TextStyle::default(),
                    });
                }

                // Diff marker, in the column just left of the text and just
                // right of any line numbers. Like the number it belongs to the
                // *line*, so a wrapped line wears it once, on its first row.
                // Colored to match the line's own band — a red `-` beside a
                // deletion — so the column reads at a glance even where the
                // background tint is subtle.
                if seg_i == 0 {
                    if let Some(marker) = markers.get(line_idx).filter(|m| !m.is_empty()) {
                        prims.push(Primitive::Text {
                            pos: (rect.x + PAD + number_w * cell_w, y),
                            text: marker.clone(),
                            color: self.line_style_color(line_idx, theme),
                            clip,
                            size: FONT_SIZE,
                            style: TextStyle::default(),
                        });
                    }
                }

                // Color runs, sliced to this row's char span (relative to `s`).
                let seg_runs: Vec<ColorRun> = runs
                    .iter()
                    .filter_map(|(rs, re, c)| {
                        let a = (*rs).max(s);
                        let b = (*re).min(e);
                        // Lazy: the run may end before this row starts (b < s),
                        // so only subtract once `b > a` guarantees b > a >= s.
                        (b > a).then(|| (a - s, b - s, *c))
                    })
                    .collect();
                push_colored_runs(
                    &seg,
                    &seg_runs,
                    theme.text,
                    ox - sx,
                    y,
                    cell_w,
                    text_clip,
                    prims,
                );

                // Caret, if the cursor falls on this visual row.
                if focused && line_idx == cursor_line && seg_i == cursor_sub {
                    let cx = cell_x(self.cursor.col);
                    if cx >= text_clip.x - 0.5 && cx < text_clip.x + text_clip.w {
                        caret = Some((cx, y));
                    }
                }

                vrow += 1;
                seg_i += 1;
            }
            seg_i = 0;
            line_idx += 1;
        }
        drop(hl);

        if let Some((cx, cy)) = caret {
            // Block caret in Normal/Visual, thin bar in Insert.
            let (w, color) = if self.vim.mode.is_block_cursor() {
                (cell_w, theme.cursor_block)
            } else {
                (CURSOR_W, theme.cursor)
            };
            // Clipped like the rest: on a partly-scrolled row the caret is
            // sliced by the content band's edge rather than drawn over the
            // padding. (`ensure_cursor_visible` normally scrolls it whole again
            // — this covers the frames where the view moved and the cursor
            // didn't, such as a wheel scroll.)
            push_clipped_quad(
                prims,
                Rect {
                    x: cx,
                    y: cy,
                    w,
                    h: cell_h,
                },
                text_clip,
                color,
            );
        }

        self.scrollbars(rect, visible, visible_cols, theme, prims);
    }

    /// Draw vertical and horizontal scrollbars when content overflows the
    /// pane. Each is a dim track with a brighter thumb sized to the visible
    /// fraction and positioned by the scroll offset.
    fn scrollbars(
        &self,
        rect: Rect,
        visible_lines: usize,
        visible_cols: usize,
        theme: &theme::Theme,
        prims: &mut Vec<Primitive>,
    ) {
        let (vbar, hbar) = self.scrollbar_geom(rect, visible_lines, visible_cols);
        for bar in [vbar, hbar].into_iter().flatten() {
            prims.push(Primitive::Quad {
                rect: bar.track,
                color: theme.scrollbar_track,
            });
            prims.push(Primitive::Quad {
                rect: bar.thumb,
                color: theme.scrollbar_thumb,
            });
        }
    }

    /// The placed `(vertical, horizontal)` scrollbar geometry for this pane, each
    /// `Some` only when that axis overflows. Vertical is measured in visual
    /// (wrapped) rows; horizontal only appears when wrapping is off. Shared by
    /// [`scrollbars`](Self::scrollbars) (drawing) and the mouse layer
    /// (hit-testing + drag), so a thumb the user sees is exactly the one they hit.
    fn scrollbar_geom(
        &self,
        rect: Rect,
        visible_lines: usize,
        visible_cols: usize,
    ) -> (Option<ScrollbarGeom>, Option<ScrollbarGeom>) {
        let v_content = self.total_rows(visible_cols);
        let v_offset = self.scroll_offset_rows(visible_cols);
        let v_overflow = v_content > visible_lines;
        let (h_content, h_overflow) = if self.wrap {
            (0, false)
        } else {
            let c = self.max_line_width();
            (c, c > visible_cols)
        };

        let vbar = v_overflow.then(|| {
            // Leave room at the bottom for the horizontal bar if both show.
            let track_h = rect.h - 2.0 - if h_overflow { SCROLLBAR_W } else { 0.0 };
            let track = Rect {
                x: rect.x + rect.w - 1.0 - SCROLLBAR_W,
                y: rect.y + 1.0,
                w: SCROLLBAR_W,
                h: track_h,
            };
            let (y, h) = thumb(
                track.y,
                track.h,
                v_offset,
                visible_lines,
                v_content,
                SCROLLBAR_W,
            );
            ScrollbarGeom {
                track,
                thumb: Rect {
                    x: track.x,
                    y,
                    w: SCROLLBAR_W,
                    h,
                },
            }
        });
        let hbar = h_overflow.then(|| {
            let track_w = rect.w - 2.0 - if v_overflow { SCROLLBAR_W } else { 0.0 };
            let track = Rect {
                x: rect.x + 1.0,
                y: rect.y + rect.h - 1.0 - SCROLLBAR_W,
                w: track_w,
                h: SCROLLBAR_W,
            };
            let (x, w) = thumb(
                track.x,
                track.w,
                self.scroll.left,
                visible_cols,
                h_content,
                SCROLLBAR_W,
            );
            ScrollbarGeom {
                track,
                thumb: Rect {
                    x,
                    y: track.y,
                    w,
                    h: SCROLLBAR_W,
                },
            }
        });
        (vbar, hbar)
    }

    /// If `(x, y)` presses a scrollbar track, which axis and the grab offset
    /// from the pointer to the thumb's leading edge (so a drag holds that grip;
    /// a press outside the thumb centers it under the pointer). `None` when the
    /// press is not on a scrollbar — the caller then treats it as text input.
    pub fn scrollbar_hit(
        &self,
        rect: Rect,
        cell: (f32, f32),
        x: f32,
        y: f32,
    ) -> Option<ScrollbarHit> {
        let (cell_w, cell_h) = cell;
        let visible_lines = Self::visible_lines(rect, cell_h);
        let visible_cols = self.visible_cols(rect, cell_w);
        let (vbar, hbar) = self.scrollbar_geom(rect, visible_lines, visible_cols);
        if let Some(b) = vbar {
            if in_rect(b.track, x, y) {
                let grab = if y >= b.thumb.y && y < b.thumb.y + b.thumb.h {
                    y - b.thumb.y
                } else {
                    b.thumb.h / 2.0
                };
                return Some(ScrollbarHit {
                    axis: ScrollAxis::Vertical,
                    grab,
                });
            }
        }
        if let Some(b) = hbar {
            if in_rect(b.track, x, y) {
                let grab = if x >= b.thumb.x && x < b.thumb.x + b.thumb.w {
                    x - b.thumb.x
                } else {
                    b.thumb.w / 2.0
                };
                return Some(ScrollbarHit {
                    axis: ScrollAxis::Horizontal,
                    grab,
                });
            }
        }
        None
    }

    /// Scroll in response to a scrollbar drag: place the thumb's leading edge at
    /// the pointer minus its `grab` offset and derive the scroll position from
    /// the resulting track fraction. Because the thumb's position is linear in
    /// the scroll offset, this fraction inverts exactly to that offset.
    pub fn drag_scroll(
        &mut self,
        axis: ScrollAxis,
        grab: f32,
        rect: Rect,
        cell: (f32, f32),
        x: f32,
        y: f32,
    ) {
        let (cell_w, cell_h) = cell;
        let visible_lines = Self::visible_lines(rect, cell_h);
        let visible_cols = self.visible_cols(rect, cell_w);
        let (vbar, hbar) = self.scrollbar_geom(rect, visible_lines, visible_cols);
        match axis {
            ScrollAxis::Vertical => {
                let Some(b) = vbar else { return };
                let travel = (b.track.h - b.thumb.h).max(1.0);
                let frac = ((y - grab - b.track.y) / travel).clamp(0.0, 1.0);
                let max_off = self.total_rows(visible_cols).saturating_sub(visible_lines);
                // Not rounded to a row: the drag follows the pointer by the
                // pixel, like every other scroll now does.
                self.set_scroll_rows(frac * max_off as f32, visible_lines, visible_cols);
            }
            ScrollAxis::Horizontal => {
                let Some(b) = hbar else { return };
                let travel = (b.track.w - b.thumb.w).max(1.0);
                let frac = ((x - grab - b.track.x) / travel).clamp(0.0, 1.0);
                let max_off = self.max_line_width().saturating_sub(visible_cols);
                self.scroll.left = frac * max_off as f32;
            }
        }
    }
}

/// Push `rect` intersected with `clip`, dropping it when nothing is left.
///
/// Quads carry no clip of their own — the whole scene's quads are one instanced
/// draw call, so there is no per-quad scissor to set — and axis-aligned
/// rectangles clip exactly on the CPU anyway. Smooth scrolling is what makes
/// this load-bearing: the first and last rows drawn straddle the viewport's
/// edges, so their backgrounds, selections and carets have to be cut off at the
/// boundary. (Text is a different story: [`Primitive::Text`] carries a `clip`
/// that becomes a real GPU scissor, so glyphs are cut off by the rasterizer.)
fn push_clipped_quad(prims: &mut Vec<Primitive>, rect: Rect, clip: Rect, color: Color) {
    let x0 = rect.x.max(clip.x);
    let y0 = rect.y.max(clip.y);
    let x1 = (rect.x + rect.w).min(clip.x + clip.w);
    let y1 = (rect.y + rect.h).min(clip.y + clip.h);
    if x1 > x0 && y1 > y0 {
        prims.push(Primitive::Quad {
            rect: Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
            color,
        });
    }
}

/// Whether `(x, y)` lies within `r` (half-open on the far edges).
fn in_rect(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

/// The semantic style of one externally-styled span — the panel `text_view`
/// per-line styling vocabulary (see `text_view_line_styles`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StyleKind {
    Added,
    Removed,
    Hunk,
    Title,
    Dim,
    Comment,
}

/// One styled run within a line: char columns `[start, end)` (end exclusive).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct StyleSpan {
    start: usize,
    end: usize,
    style: StyleKind,
}

/// The kind of background band painted behind a run of a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BgKind {
    Added,
    Removed,
    Header,
}

/// One background run within a line: char columns `[start, end)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BgSpan {
    start: usize,
    end: usize,
    kind: BgKind,
}

/// A color-resolved run within one line, in char columns: `[start, end)`.
/// Syntax spans and external style spans both reduce to this before
/// drawing, so [`push_colored_runs`] has a single code path.
type ColorRun = (usize, usize, Color);

/// The themed color for one external style kind. Picked from the existing dark
/// palette: diff adds are string-green, removals the error red, hunk headers
/// function-blue, titles the type accent, and dim text the comment gray.
fn style_color(style: StyleKind, theme: &theme::Theme) -> Color {
    match style {
        StyleKind::Added => theme.syntax_string,
        StyleKind::Removed => theme.error_text,
        StyleKind::Hunk => theme.syntax_function,
        StyleKind::Title => theme.syntax_type,
        StyleKind::Dim => theme.text_dim,
        StyleKind::Comment => theme.syntax_function,
    }
}

/// The themed background color for one external background kind. Translucent
/// tints so they read on any theme background: diff rows pick up a faint
/// green/red, and a header band a light neutral wash.
fn bg_color(kind: BgKind, _theme: &theme::Theme) -> Color {
    match kind {
        BgKind::Added => theme::DIFF_ADDED_TINT,
        BgKind::Removed => theme::DIFF_REMOVED_TINT,
        BgKind::Header => theme::rgba(0x9a, 0xa4, 0xb2, 0.10),
    }
}

/// Render one line as colored text runs. Walks the raw line tracking a
/// display column (a `\t` advances to the next [`TAB_WIDTH`] stop and renders
/// as spaces — the same [`char_advance`] mapping the cursor, selection, and
/// click geometry use, so glyphs and quads line up), assigns each char the
/// color of the [`ColorRun`] covering its char column (else `default`), and
/// coalesces consecutive same-color chars into one [`Primitive::Text`] run. An
/// empty line emits nothing; a line with no spans emits a single
/// default-colored run, matching the pre-highlighting behavior exactly.
#[allow(clippy::too_many_arguments)]
fn push_colored_runs(
    line: &str,
    spans: &[ColorRun],
    default: Color,
    base_x: f32,
    y: f32,
    cell_w: f32,
    clip: Rect,
    prims: &mut Vec<Primitive>,
) {
    /// The color for char column `col`: the first covering span, else default.
    fn color_at(spans: &[ColorRun], col: usize, default: Color) -> Color {
        spans
            .iter()
            .find(|(start, end, _)| *start <= col && col < *end)
            .map(|(_, _, color)| *color)
            .unwrap_or(default)
    }

    let mut run_text = String::new();
    let mut run_color = default;
    let mut run_start_col = 0usize; // display column of the run's first char
    let mut dcol = 0usize; // display column of the next char to place

    let flush = |run_text: &mut String,
                 run_color: Color,
                 run_start_col: usize,
                 prims: &mut Vec<Primitive>| {
        if !run_text.is_empty() {
            prims.push(Primitive::Text {
                pos: (base_x + run_start_col as f32 * cell_w, y),
                text: std::mem::take(run_text),
                color: run_color,
                clip,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        }
    };

    for (char_col, ch) in line.chars().enumerate() {
        // Tabs render as spaces up to the next tab stop, all in the color at
        // the tab's char column.
        let advance = char_advance(ch, dcol);
        let rendered = (ch != '\t').then_some(ch);
        let color = color_at(spans, char_col, default);
        if run_text.is_empty() {
            run_color = color;
            run_start_col = dcol;
        } else if color != run_color {
            flush(&mut run_text, run_color, run_start_col, prims);
            run_color = color;
            run_start_col = dcol;
        }
        match rendered {
            Some(c) => run_text.push(c),
            None => {
                for _ in 0..advance {
                    run_text.push(' ');
                }
            }
        }
        dcol += advance;
    }
    flush(&mut run_text, run_color, run_start_col, prims);
}

/// The leading whitespace of `line` — spaces and tabs verbatim — truncated to
/// at most `max_cols` characters.
/// Read a buffer edit as a **line splice** — `(first row, rows removed, rows
/// inserted)` — which is the shape a [`Projection`] transforms its table by.
/// `after` is the position [`Buffer::replace`] returned.
///
/// An edit bounded by line starts and made of whole lines is a whole-line
/// splice, and is reported as one. That case matters: `dd` deletes
/// `[(l, 0), (l+1, 0))`, and reading it generically ("2 rows touched, 1 row
/// left") would pair the *wrong* surviving line — the projection would think
/// line `l` was retexted when in truth it was removed and line `l+1` slid up.
fn line_splice(start: Point, end: Point, text: &str, after: Point) -> (usize, usize, usize) {
    if start.col == 0 && end.col == 0 && (text.is_empty() || text.ends_with('\n')) {
        return (
            start.line,
            end.line - start.line,
            text.matches('\n').count(),
        );
    }
    (
        start.line,
        end.line - start.line + 1,
        after.line - start.line + 1,
    )
}

pub(crate) fn leading_indent(line: &str, max_cols: usize) -> &str {
    let mut end = 0;
    for (count, (idx, ch)) in line.char_indices().enumerate() {
        if count >= max_cols || (ch != ' ' && ch != '\t') {
            break;
        }
        end = idx + ch.len_utf8();
    }
    &line[..end]
}

/// Thumb `(start, length)` along a track: a viewport showing `visible` of
/// `content` units, scrolled by `offset`, maps to a proportional thumb at least
/// `min` long. `offset` is fractional so the thumb glides with a smooth scroll
/// instead of stepping a row at a time.
fn thumb(
    track_start: f32,
    track_len: f32,
    offset: f32,
    visible: usize,
    content: usize,
    min: f32,
) -> (f32, f32) {
    let content_f = (content.max(1)) as f32;
    let offset = offset.clamp(0.0, content.saturating_sub(1) as f32);
    let span = (offset + visible as f32).min(content_f);
    let start = track_start + track_len * (offset / content_f);
    let len = (track_len * ((span - offset) / content_f)).max(min);
    // `scroll_to_line` may anchor the view past the last screenful, and the
    // minimum thumb length rounds up: without this the thumb hangs off the end
    // of its own track.
    let start = start.min(track_start + (track_len - len).max(0.0));
    (start, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> EditorView {
        EditorView::from_buffer(Buffer::from_str(text))
    }

    #[test]
    fn drag_selects_text() {
        let mut v = view("hello world");
        v.begin_drag(Point::new(0, 2), false);
        assert!(v.selection().is_none()); // nothing selected until the head moves
        v.drag_to(Point::new(0, 7));
        assert_eq!(v.selected_text(), "llo w");
        v.end_drag();
        assert_eq!(v.selected_text(), "llo w"); // release keeps the selection
        assert_eq!(v.cursor, Point::new(0, 7));
    }

    #[test]
    fn zero_width_drag_collapses_to_caret() {
        let mut v = view("abc");
        v.begin_drag(Point::new(0, 1), false);
        v.end_drag();
        assert!(v.anchor.is_none());
    }

    #[test]
    fn click_clears_previous_selection() {
        let mut v = view("abc def");
        v.begin_drag(Point::new(0, 0), false);
        v.drag_to(Point::new(0, 3));
        v.end_drag();
        v.begin_drag(Point::new(0, 5), false);
        v.end_drag();
        assert!(v.selection().is_none());
        assert_eq!(v.cursor, Point::new(0, 5));
    }

    #[test]
    fn shift_click_extends_from_caret() {
        let mut v = view("abcdef");
        v.cursor = Point::new(0, 1);
        v.begin_drag(Point::new(0, 4), true);
        assert_eq!(v.selected_text(), "bcd");
    }

    #[test]
    fn reverse_drag_selects_backwards() {
        let mut v = view("abcdef");
        v.begin_drag(Point::new(0, 4), false);
        v.drag_to(Point::new(0, 1));
        assert_eq!(v.selected_text(), "bcd");
        assert_eq!(v.cursor, Point::new(0, 1));
    }

    #[test]
    fn multiline_drag_selects_across_lines() {
        let mut v = view("one\ntwo\nthree");
        v.begin_drag(Point::new(0, 2), false);
        v.drag_to(Point::new(2, 3));
        assert_eq!(v.selected_text(), "e\ntwo\nthr");
    }

    #[test]
    fn insert_replaces_selection() {
        let mut v = view("hello world");
        v.begin_drag(Point::new(0, 0), false);
        v.drag_to(Point::new(0, 5));
        v.insert("goodbye");
        assert_eq!(v.buffer.to_string(), "goodbye world");
        assert_eq!(v.cursor, Point::new(0, 7));
        assert!(v.selection().is_none());

        // One undo reverses the whole replacement.
        v.undo();
        assert_eq!(v.buffer.to_string(), "hello world");
    }

    #[test]
    fn backspace_and_delete_remove_only_the_selection() {
        let mut v = view("hello world");
        v.begin_drag(Point::new(0, 5), false);
        v.drag_to(Point::new(0, 11));
        v.backspace();
        assert_eq!(v.buffer.to_string(), "hello");
        assert_eq!(v.cursor, Point::new(0, 5));

        let mut v = view("hello world");
        v.begin_drag(Point::new(0, 5), false);
        v.drag_to(Point::new(0, 11));
        v.delete_forward();
        assert_eq!(v.buffer.to_string(), "hello");
    }

    #[test]
    fn shift_movement_extends_selection() {
        let mut v = view("abcdef");
        v.move_cursor(Move::Right, 10, true);
        v.move_cursor(Move::Right, 10, true);
        assert_eq!(v.selected_text(), "ab");
        v.move_cursor(Move::LineEnd, 10, true);
        assert_eq!(v.selected_text(), "abcdef");
    }

    #[test]
    fn plain_left_right_collapse_to_selection_edges() {
        let mut v = view("abcdef");
        v.begin_drag(Point::new(0, 1), false);
        v.drag_to(Point::new(0, 4));
        v.move_cursor(Move::Left, 10, false);
        assert!(v.selection().is_none());
        assert_eq!(v.cursor, Point::new(0, 1)); // collapse to start, not move

        let mut v = view("abcdef");
        v.begin_drag(Point::new(0, 1), false);
        v.drag_to(Point::new(0, 4));
        v.move_cursor(Move::Right, 10, false);
        assert_eq!(v.cursor, Point::new(0, 4));
    }

    #[test]
    fn plain_vertical_move_drops_selection() {
        let mut v = view("abc\ndef");
        v.begin_drag(Point::new(0, 0), false);
        v.drag_to(Point::new(0, 3));
        v.move_cursor(Move::Down, 10, false);
        assert!(v.selection().is_none());
        assert_eq!(v.cursor, Point::new(1, 3));
    }

    #[test]
    fn vertical_move_remembers_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(0, 9);
        v.move_cursor(Move::Down, 10, false);
        assert_eq!(v.cursor, Point::new(1, 2)); // clamped to the short line's end
        v.move_cursor(Move::Down, 10, false);
        assert_eq!(v.cursor, Point::new(2, 9)); // desired column restored
    }

    #[test]
    fn vertical_move_up_restores_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(2, 9);
        v.move_cursor(Move::Up, 10, false);
        assert_eq!(v.cursor, Point::new(1, 2));
        v.move_cursor(Move::Up, 10, false);
        assert_eq!(v.cursor, Point::new(0, 9));
    }

    #[test]
    fn horizontal_move_resets_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(0, 9);
        v.move_cursor(Move::Down, 10, false);
        v.move_cursor(Move::Left, 10, false);
        assert_eq!(v.cursor, Point::new(1, 1));
        v.move_cursor(Move::Down, 10, false);
        assert_eq!(v.cursor, Point::new(2, 1)); // memory was reset, not col 9
    }

    #[test]
    fn page_moves_keep_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(0, 9);
        v.move_cursor(Move::PageDown, 1, false);
        assert_eq!(v.cursor, Point::new(1, 2));
        v.move_cursor(Move::PageDown, 1, false);
        assert_eq!(v.cursor, Point::new(2, 9));
    }

    #[test]
    fn insert_resets_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(0, 9);
        v.move_cursor(Move::Down, 10, false);
        v.insert("x"); // "abx", cursor (1, 3)
        v.move_cursor(Move::Down, 10, false);
        assert_eq!(v.cursor, Point::new(2, 3));
    }

    #[test]
    fn click_resets_desired_column() {
        let mut v = view("long line here\nab\nlonger line!");
        v.cursor = Point::new(0, 9);
        v.move_cursor(Move::Down, 10, false);
        v.begin_drag(Point::new(1, 1), false);
        v.end_drag();
        v.move_cursor(Move::Down, 10, false);
        assert_eq!(v.cursor, Point::new(2, 1));
    }

    // --- multi-click selection: word/line ranges ---------------------------

    #[test]
    fn word_range_spans_word_chars() {
        let v = view("foo_bar1 baz");
        assert_eq!(
            v.word_range_at(Point::new(0, 3)),
            (Point::new(0, 0), Point::new(0, 8))
        );
    }

    #[test]
    fn word_range_on_non_ascii_word() {
        let v = view("héllo wörld");
        assert_eq!(
            v.word_range_at(Point::new(0, 8)),
            (Point::new(0, 6), Point::new(0, 11))
        );
    }

    #[test]
    fn word_range_on_punctuation_run() {
        let v = view("foo(((bar");
        assert_eq!(
            v.word_range_at(Point::new(0, 4)),
            (Point::new(0, 3), Point::new(0, 6))
        );
    }

    #[test]
    fn word_range_on_whitespace_run() {
        let v = view("a   b");
        assert_eq!(
            v.word_range_at(Point::new(0, 2)),
            (Point::new(0, 1), Point::new(0, 4))
        );
    }

    #[test]
    fn word_range_at_end_of_line_takes_the_trailing_word() {
        let v = view("hi there");
        assert_eq!(
            v.word_range_at(Point::new(0, 8)),
            (Point::new(0, 3), Point::new(0, 8))
        );
    }

    #[test]
    fn word_range_on_empty_line_is_empty() {
        let v = view("a\n\nb");
        assert_eq!(
            v.word_range_at(Point::new(1, 0)),
            (Point::new(1, 0), Point::new(1, 0))
        );
    }

    #[test]
    fn line_range_includes_trailing_newline() {
        let v = view("abc\ndef");
        assert_eq!(v.line_range_at(0), (Point::new(0, 0), Point::new(1, 0)));
    }

    #[test]
    fn line_range_on_last_line_stops_at_eol() {
        let v = view("abc\ndef");
        assert_eq!(v.line_range_at(1), (Point::new(1, 0), Point::new(1, 3)));
    }

    // --- multi-click selection: clicks and drags ----------------------------

    #[test]
    fn double_click_selects_word() {
        let mut v = view("hello world");
        v.begin_drag_with_clicks(Point::new(0, 7), false, 2);
        assert_eq!(v.selected_text(), "world");
        assert_eq!(v.anchor, Some(Point::new(0, 6)));
        assert_eq!(v.cursor, Point::new(0, 11));
        v.end_drag();
        assert_eq!(v.selected_text(), "world"); // release keeps the selection
    }

    #[test]
    fn double_click_on_whitespace_selects_the_run() {
        let mut v = view("a   b");
        v.begin_drag_with_clicks(Point::new(0, 2), false, 2);
        assert_eq!(v.selected_text(), "   ");
    }

    #[test]
    fn triple_click_selects_line_including_newline() {
        let mut v = view("one\ntwo\nthree");
        v.begin_drag_with_clicks(Point::new(1, 1), false, 3);
        assert_eq!(v.selected_text(), "two\n");
        assert_eq!(v.anchor, Some(Point::new(1, 0)));
        assert_eq!(v.cursor, Point::new(2, 0));
    }

    #[test]
    fn triple_click_on_last_line_selects_to_eol() {
        let mut v = view("one\ntwo");
        v.begin_drag_with_clicks(Point::new(1, 0), false, 3);
        assert_eq!(v.selected_text(), "two");
    }

    #[test]
    fn typing_over_triple_click_replaces_the_whole_line() {
        let mut v = view("one\ntwo\nthree");
        v.begin_drag_with_clicks(Point::new(1, 1), false, 3);
        v.insert("X");
        assert_eq!(v.buffer.to_string(), "one\nXthree");
    }

    #[test]
    fn word_drag_extends_forward_by_words() {
        let mut v = view("aaa bbb ccc ddd");
        v.begin_drag_with_clicks(Point::new(0, 5), false, 2); // on "bbb"
        v.drag_to(Point::new(0, 9)); // into "ccc"
        assert_eq!(v.selected_text(), "bbb ccc");
        assert_eq!(v.cursor, Point::new(0, 11));
    }

    #[test]
    fn word_drag_extends_backward_by_words() {
        let mut v = view("aaa bbb ccc");
        v.begin_drag_with_clicks(Point::new(0, 5), false, 2); // on "bbb"
        v.drag_to(Point::new(0, 1)); // back into "aaa"
        assert_eq!(v.selected_text(), "aaa bbb");
        assert_eq!(v.cursor, Point::new(0, 0)); // head at the selection start
    }

    #[test]
    fn word_drag_back_inside_pivot_keeps_the_word() {
        let mut v = view("aaa bbb ccc");
        v.begin_drag_with_clicks(Point::new(0, 5), false, 2);
        v.drag_to(Point::new(0, 9));
        v.drag_to(Point::new(0, 5)); // back onto the double-clicked word
        assert_eq!(v.selected_text(), "bbb");
    }

    #[test]
    fn word_drag_crosses_lines() {
        let mut v = view("aaa bbb\nccc ddd");
        v.begin_drag_with_clicks(Point::new(0, 5), false, 2); // on "bbb"
        v.drag_to(Point::new(1, 1)); // into "ccc"
        assert_eq!(v.selected_text(), "bbb\nccc");
    }

    #[test]
    fn line_drag_extends_by_lines() {
        let mut v = view("one\ntwo\nthree\nfour");
        v.begin_drag_with_clicks(Point::new(1, 1), false, 3);
        v.drag_to(Point::new(2, 2));
        assert_eq!(v.selected_text(), "two\nthree\n");
        v.drag_to(Point::new(0, 1));
        assert_eq!(v.selected_text(), "one\ntwo\n");
        assert_eq!(v.cursor, Point::new(0, 0));
    }

    #[test]
    fn single_click_after_multi_click_drags_by_character() {
        let mut v = view("aaa bbb");
        v.begin_drag_with_clicks(Point::new(0, 1), false, 2);
        v.end_drag();
        v.begin_drag_with_clicks(Point::new(0, 1), false, 1);
        v.drag_to(Point::new(0, 2));
        assert_eq!(v.selected_text(), "a");
    }

    #[test]
    fn shift_multi_click_extends_like_shift_click() {
        let mut v = view("abcdef");
        v.cursor = Point::new(0, 1);
        v.begin_drag_with_clicks(Point::new(0, 4), true, 2);
        assert_eq!(v.selected_text(), "bcd");
    }

    #[test]
    fn select_all_spans_buffer() {
        let mut v = view("abc\ndef");
        v.select_all();
        assert_eq!(v.selected_text(), "abc\ndef");
        assert_eq!(v.cursor, Point::new(1, 3));
    }

    #[test]
    fn undo_clears_selection() {
        let mut v = view("abc");
        v.insert("x");
        v.begin_drag(Point::new(0, 0), false);
        v.drag_to(Point::new(0, 2));
        v.undo();
        assert!(v.selection().is_none());
        assert_eq!(v.buffer.to_string(), "abc");
    }

    // --- auto-indent on Enter ----------------------------------------------

    #[test]
    fn leading_indent_takes_spaces_and_tabs_up_to_max_cols() {
        assert_eq!(leading_indent("    foo", usize::MAX), "    ");
        assert_eq!(leading_indent("    foo", 2), "  ");
        assert_eq!(leading_indent("\t\tfoo", 5), "\t\t");
        assert_eq!(leading_indent(" \t bar", usize::MAX), " \t ");
        assert_eq!(leading_indent("foo", 3), "");
        assert_eq!(leading_indent("", usize::MAX), "");
    }

    #[test]
    fn newline_copies_leading_indent() {
        let mut v = view("    foo");
        v.cursor = Point::new(0, 7);
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "    foo\n    ");
        assert_eq!(v.cursor, Point::new(1, 4)); // after the indent
    }

    #[test]
    fn newline_copies_tabs_verbatim() {
        let mut v = view("\t\tfoo");
        v.cursor = Point::new(0, 5);
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "\t\tfoo\n\t\t");
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn newline_in_the_middle_of_a_line_indents_the_moved_tail() {
        let mut v = view("  foo bar");
        v.cursor = Point::new(0, 6); // after "foo "
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "  foo \n  bar");
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn newline_on_unindented_line_adds_no_indent() {
        let mut v = view("foo");
        v.cursor = Point::new(0, 3);
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "foo\n");
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn pathless_buffer_titles_are_untitled() {
        let v = view("hello");
        assert_eq!(v.title(), "[untitled]");
        assert_eq!(v.display_name(), "[untitled]");
    }

    #[test]
    fn external_title_overrides_buffer_path() {
        // A GPP process pane reports its title from outside the buffer; it wins
        // over the pathless "[untitled]" and carries no dirty marker.
        let mut v = view("listing");
        v.set_external_title(Some("/tmp/demo".to_string()));
        assert_eq!(v.title(), "/tmp/demo");
        assert_eq!(v.display_name(), "/tmp/demo");
        // Clearing it falls back to the buffer-derived title again.
        v.set_external_title(None);
        assert_eq!(v.title(), "[untitled]");
    }

    #[test]
    fn newline_on_empty_line_adds_no_indent() {
        let mut v = view("");
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "\n");
        assert_eq!(v.cursor, Point::new(1, 0));
    }

    #[test]
    fn newline_on_all_whitespace_line_copies_it_all() {
        let mut v = view("   \t ");
        v.cursor = Point::new(0, 5);
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "   \t \n   \t ");
        assert_eq!(v.cursor, Point::new(1, 5));
    }

    #[test]
    fn newline_inside_the_indent_truncates_to_the_cursor_column() {
        let mut v = view("    foo");
        v.cursor = Point::new(0, 2); // inside the 4-space indent
        v.insert_newline();
        // The tail ("  foo") moves down; the new line gets only the indent
        // left of the cursor, not the line's full indent.
        assert_eq!(v.buffer.to_string(), "  \n    foo");
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn newline_over_a_selection_indents_from_the_selection_start_line() {
        let mut v = view("  hello world");
        v.anchor = Some(Point::new(0, 7));
        v.cursor = Point::new(0, 13);
        v.insert_newline();
        assert_eq!(v.buffer.to_string(), "  hello\n  ");
        assert_eq!(v.cursor, Point::new(1, 2));
    }

    #[test]
    fn newline_with_indent_undoes_in_one_step() {
        let mut v = view("    foo");
        v.cursor = Point::new(0, 7);
        v.insert_newline();
        v.undo();
        assert_eq!(v.buffer.to_string(), "    foo");
        assert_eq!(v.cursor, Point::new(0, 7));
    }

    #[test]
    fn newline_then_typing_undoes_in_two_steps() {
        let mut v = view("    foo");
        v.cursor = Point::new(0, 7);
        v.insert_newline();
        v.insert("x");
        v.insert("y"); // coalesces with "x"
        v.undo(); // drops the typed run
        assert_eq!(v.buffer.to_string(), "    foo\n    ");
        v.undo(); // drops the newline + indent together
        assert_eq!(v.buffer.to_string(), "    foo");
    }

    #[test]
    fn plain_multiline_insert_does_not_auto_indent() {
        // The paste path (Cmd+V and the debug /text endpoint) goes through
        // insert(), which must never add indentation to pasted lines.
        let mut v = view("    foo");
        v.cursor = Point::new(0, 7);
        v.insert("\nbar\nbaz");
        assert_eq!(v.buffer.to_string(), "    foo\nbar\nbaz");
    }

    #[test]
    fn scroll_keeps_last_line_at_bottom() {
        // 10 lines, 4 visible: the furthest we can scroll is line 6 at top,
        // putting line 10 (index 9) flush against the bottom. No void below.
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        v.scroll_by(100.0, 4, 80);
        assert_eq!(v.scroll.top, 6);
    }

    #[test]
    fn scroll_clamps_to_zero_when_content_fits() {
        let mut v = view("a\nb\nc");
        v.scroll_by(50.0, 10, 80); // far more visible rows than lines
        assert_eq!(v.scroll.top, 0);
    }

    // --- smooth (sub-row) scrolling ----------------------------------------

    /// A scroll smaller than a row moves the view without changing the anchor:
    /// this is the whole point of `Scroll::frac`. Trackpads deliver motion in
    /// exactly these amounts, and the old whole-row API rounded them away.
    #[test]
    fn a_sub_row_scroll_moves_the_view_without_changing_the_anchor_row() {
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        v.scroll_by(0.25, 4, 80);
        assert_eq!(v.scroll.vpos(), (0, 0));
        assert!((v.scroll.frac - 0.25).abs() < 1e-6);
    }

    /// Fractions accumulate exactly instead of each rounding to nothing, and
    /// carry into the anchor row once they add up to a whole one.
    #[test]
    fn sub_row_scrolls_accumulate_into_whole_rows() {
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        for _ in 0..5 {
            v.scroll_by(0.3, 4, 80);
        }
        // 5 * 0.3 = 1.5 rows: one whole row of anchor, half a row of offset.
        assert_eq!(v.scroll.vpos(), (1, 0));
        assert!((v.scroll.frac - 0.5).abs() < 1e-5);
    }

    /// Scrolling back up crosses row boundaries the same way — a negative
    /// delta borrows a whole row and leaves a positive fraction — and lands
    /// exactly where it started.
    #[test]
    fn a_sub_row_scroll_reverses_exactly() {
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        v.scroll_by(2.0, 4, 80);
        v.scroll_by(-0.25, 4, 80);
        assert_eq!(v.scroll.vpos(), (1, 0));
        assert!((v.scroll.frac - 0.75).abs() < 1e-6);
        v.scroll_by(0.25, 4, 80);
        assert_eq!(v.scroll.vpos(), (2, 0));
        assert_eq!(v.scroll.frac, 0.0);
    }

    /// Running into the top of the buffer drops the leftover fraction rather
    /// than leaving the view a fraction *below* the first row — which would
    /// read as the content bouncing back down at the end of a flick.
    #[test]
    fn scrolling_past_the_top_parks_flush_at_the_first_row() {
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        v.scroll_by(0.5, 4, 80);
        v.scroll_by(-3.5, 4, 80);
        assert_eq!(v.scroll.vpos(), (0, 0));
        assert_eq!(v.scroll.frac, 0.0);
    }

    /// The bottom clamp is flush too: the last row sits against the bottom
    /// edge with no fraction, so no partial row of void shows below it.
    #[test]
    fn scrolling_past_the_bottom_parks_flush_at_the_last_screenful() {
        let mut v = view("0\n1\n2\n3\n4\n5\n6\n7\n8\n9");
        v.scroll_by(100.5, 4, 80);
        assert_eq!(v.scroll.vpos(), (6, 0));
        assert_eq!(v.scroll.frac, 0.0);
    }

    /// A partly-scrolled view shifts its rows up by that fraction of a cell,
    /// and the row scrolled off the top is clipped at the content band's edge
    /// rather than drawn over the pane's padding and border.
    #[test]
    fn a_fractional_offset_shifts_rows_up_and_clips_the_partial_one() {
        let text = vec!["foo"; 20].join("\n");
        let mut v = view(&text);
        v.vim.last_search = Some("foo".to_string());
        v.vim.search_hl = true;
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 100.0,
        };
        let cell = (8.0, 18.0);
        v.scroll_by(2.5, EditorView::visible_lines(rect, cell.1), 80);

        let mut prims = Vec::new();
        v.build_scene(rect, cell, true, &theme::Theme::default(), &mut prims);
        let quads = search_match_quads(&prims);

        // Nothing escapes the content band (PAD..rect.h - PAD).
        let (top, bottom) = (PAD, rect.h - PAD);
        for q in &quads {
            assert!(
                q.y >= top - 1e-3 && q.y + q.h <= bottom + 1e-3,
                "row quad {q:?} escapes the content band {top}..{bottom}"
            );
        }
        // The first row is the one half-scrolled off the top: it starts at the
        // band's edge and is half a cell short.
        let first = quads.first().expect("rows are drawn");
        assert_eq!(first.y, top);
        assert!(
            (first.h - cell.1 / 2.0).abs() < 1e-3,
            "expected a half-height sliver at the top, got {}",
            first.h
        );
    }

    /// A click lands on the row that is under the pointer *after* the sub-row
    /// shift — hit-testing and drawing have to agree, or a partly-scrolled view
    /// puts the caret one line off.
    #[test]
    fn clicks_hit_the_row_the_fractional_offset_put_under_the_pointer() {
        let text = vec!["foo"; 20].join("\n");
        let mut v = view(&text);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 100.0,
        };
        let cell = (8.0, 18.0);
        v.scroll_by(2.5, EditorView::visible_lines(rect, cell.1), 80);
        // Rows are drawn from `PAD` at half a cell: line 2's remaining half
        // occupies PAD..PAD+9, then line 3 starts.
        assert_eq!(v.position_for_click(rect, cell, 0.0, PAD + 2.0).line, 2);
        assert_eq!(v.position_for_click(rect, cell, 0.0, PAD + 12.0).line, 3);
    }

    /// The scrollbar thumb tracks a sub-row scroll continuously; a row-quantized
    /// offset would make it jump while the text glided.
    #[test]
    fn the_scrollbar_thumb_follows_a_sub_row_scroll() {
        let text = vec!["foo"; 100].join("\n");
        let mut v = view(&text);
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 100.0,
        };
        let thumb_y = |v: &EditorView| {
            v.scrollbar_geom(rect, EditorView::visible_lines(rect, 18.0), 80)
                .0
                .expect("100 lines overflow a 4-row pane")
                .thumb
                .y
        };
        let before = thumb_y(&v);
        v.scroll_by(0.5, EditorView::visible_lines(rect, 18.0), 80);
        assert!(
            thumb_y(&v) > before,
            "thumb did not move for a half-row scroll"
        );
    }

    /// Horizontal scrolling is fractional too, and the caret's whole cell — not
    /// just its left edge — is what has to stay inside the viewport.
    #[test]
    fn horizontal_scroll_is_fractional_and_keeps_the_whole_caret_cell_visible() {
        let mut v = view("0123456789abcdef");
        v.wrap = false;
        v.scroll_h_by(0.5);
        assert_eq!(v.scroll.left, 0.5);
        // 5 columns visible from 0.5 means columns 0.5..5.5: the caret at
        // column 5 needs its whole cell, so the view follows to 5 + 1 - 5 = 1.
        v.cursor = Point::new(0, 5);
        v.ensure_cursor_visible(10, 5);
        assert_eq!(v.scroll.left, 1.0);
    }

    #[test]
    fn horizontal_scroll_follows_cursor_right() {
        let mut v = view("0123456789abcdef");
        v.wrap = false; // horizontal scroll only applies in non-wrap mode
        v.cursor = Point::new(0, 12);
        // Only 5 columns visible; cursor at col 12 must pull the view right so
        // the cursor stays on screen: scroll_left = 12 + 1 - 5 = 8.
        v.ensure_cursor_visible(10, 5);
        assert_eq!(v.scroll.left, 8.0);
    }

    #[test]
    fn horizontal_scroll_resets_when_cursor_returns_left() {
        let mut v = view("0123456789abcdef");
        v.wrap = false;
        v.scroll.left = 8.0;
        v.cursor = Point::new(0, 2);
        v.ensure_cursor_visible(10, 5);
        assert_eq!(v.scroll.left, 2.0);
    }

    #[test]
    fn horizontal_scroll_stays_when_cursor_in_view() {
        let mut v = view("0123456789abcdef");
        v.wrap = false;
        v.scroll.left = 4.0;
        v.cursor = Point::new(0, 6); // within [4, 4+5)
        v.ensure_cursor_visible(10, 5);
        assert_eq!(v.scroll.left, 4.0);
    }

    #[test]
    fn click_accounts_for_horizontal_scroll() {
        let mut v = view("0123456789abcdef");
        v.wrap = false;
        v.scroll.left = 8.0;
        // A click at the text origin lands on the first visible column, which
        // is column 8 given the scroll offset.
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let (ox, _) = v.text_origin(rect, 8.0);
        let p = v.position_for_click(rect, (8.0, 18.0), ox, 0.0);
        assert_eq!(p.col, 8);
    }

    // --- tab-aware display columns ------------------------------------------

    #[test]
    fn display_col_on_empty_line_counts_virtual_cells() {
        assert_eq!(display_col("", 0), 0);
        assert_eq!(display_col("", 3), 3); // past the end: one cell per char
    }

    #[test]
    fn display_col_expands_leading_tabs_to_tab_stops() {
        assert_eq!(display_col("\tx", 0), 0);
        assert_eq!(display_col("\tx", 1), 4); // tab advances to the next stop
        assert_eq!(display_col("\tx", 2), 5);
        assert_eq!(display_col("\t\tx", 2), 8);
    }

    #[test]
    fn display_col_advances_interior_tabs_to_the_next_stop() {
        // "ab" fills cells 0-1, so the tab spans only cells 2-3 — not 4 cells.
        assert_eq!(display_col("ab\tc", 3), 4);
        assert_eq!(display_col("ab\tc", 4), 5);
        // A tab already on a stop advances a full TAB_WIDTH.
        assert_eq!(display_col("abcd\te", 5), 8);
    }

    #[test]
    fn display_col_past_end_of_line_keeps_counting() {
        assert_eq!(display_col("\t", 3), 6); // 4 for the tab + 2 virtual cells
    }

    #[test]
    fn display_width_expands_tabs() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("ab\tc"), 5);
        assert_eq!(display_width("\t\t"), 8);
    }

    #[test]
    fn char_col_for_display_rounds_to_the_nearest_boundary() {
        // Plain chars: first half of the cell -> that char, second half -> next.
        assert_eq!(char_col_for_display("abc", 0.4), 0);
        assert_eq!(char_col_for_display("abc", 0.6), 1);
        // A tab spanning cells 0..4: its first half maps onto the tab, its
        // second half after it.
        assert_eq!(char_col_for_display("\tx", 1.9), 0);
        assert_eq!(char_col_for_display("\tx", 2.1), 1);
        // Past the end clamps to the line's char count.
        assert_eq!(char_col_for_display("abc", 99.0), 3);
        assert_eq!(char_col_for_display("", 2.0), 0);
    }

    #[test]
    fn display_col_round_trips_through_char_col_for_display() {
        let line = "a\tbc\t\td";
        for char_col in 0..=line.chars().count() {
            let d = display_col(line, char_col);
            assert_eq!(
                char_col_for_display(line, d as f32),
                char_col,
                "col {char_col}"
            );
        }
    }

    #[test]
    fn click_on_tabby_line_maps_display_to_char_cols() {
        let v = view("\tabc");
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let (ox, _) = v.text_origin(rect, 8.0);
        // The tab covers display cells 0..4; a click in its first half stays
        // on the tab, the second half lands after it, and cell 4 holds 'a'.
        let click = |x: f32| v.position_for_click(rect, (8.0, 18.0), ox + x * 8.0, 0.0);
        assert_eq!(click(1.0), Point::new(0, 0));
        assert_eq!(click(3.0), Point::new(0, 1));
        assert_eq!(click(4.2), Point::new(0, 1));
        assert_eq!(click(4.8), Point::new(0, 2));
    }

    #[test]
    fn horizontal_scroll_follows_the_display_column_on_tabby_lines() {
        let mut v = view("\t\tabcdefgh");
        v.wrap = false;
        v.cursor = Point::new(0, 4); // "b": display column 10
        v.ensure_cursor_visible(10, 5);
        assert_eq!(v.scroll.left, 6.0); // 10 + 1 - 5
    }

    #[test]
    fn h_scroll_range_uses_the_display_width() {
        let mut v = view("\tab"); // display width 6, not char count 3
        v.wrap = false;
        v.scroll_h_by(100.0);
        assert_eq!(v.scroll.left, 5.0); // clamped to width - 1
    }

    #[test]
    fn caret_sits_at_the_display_column_after_a_tab() {
        let mut v = view("\tx");
        v.cursor = Point::new(0, 1); // on 'x': display column 4
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), true, &theme, &mut prims);

        let (ox, _) = v.text_origin(rect, 8.0);
        let caret = prims
            .iter()
            .find_map(|p| match p {
                Primitive::Quad { rect, color } if *color == theme.cursor_block => Some(*rect),
                _ => None,
            })
            .expect("block caret quad");
        assert_eq!(caret.x, ox + 4.0 * 8.0);
    }

    #[test]
    fn selection_quads_expand_tabs_per_tab_stop() {
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let selection_quad = |v: &EditorView| -> Rect {
            let mut prims = Vec::new();
            v.build_scene(rect, (8.0, 18.0), true, &theme, &mut prims);
            prims
                .iter()
                .find_map(|p| match p {
                    Primitive::Quad { rect, color } if *color == theme.selection => Some(*rect),
                    _ => None,
                })
                .expect("selection quad")
        };
        let (ox, _) = view("").text_origin(rect, 8.0);

        // Selecting "ab" after a leading tab: the quad starts at display
        // column 4, two cells wide.
        let mut v = view("\tabc");
        v.begin_drag(Point::new(0, 1), false);
        v.drag_to(Point::new(0, 3));
        let quad = selection_quad(&v);
        assert_eq!(quad.x, ox + 4.0 * 8.0);
        assert_eq!(quad.w, 2.0 * 8.0);

        // Selecting the tab itself covers all four of its display cells.
        let mut v = view("\tabc");
        v.begin_drag(Point::new(0, 0), false);
        v.drag_to(Point::new(0, 1));
        let quad = selection_quad(&v);
        assert_eq!(quad.x, ox);
        assert_eq!(quad.w, 4.0 * 8.0);
    }

    #[test]
    fn search_highlight_quads_expand_tabs() {
        let mut v = view("\tfoo");
        v.vim.last_search = Some("foo".to_string());
        v.vim.search_hl = true;

        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(
            rect,
            (8.0, 18.0),
            true,
            &theme::Theme::default(),
            &mut prims,
        );

        let quads = search_match_quads(&prims);
        let (ox, _) = v.text_origin(rect, 8.0);
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].x, ox + 4.0 * 8.0); // match starts after the tab
        assert_eq!(quads[0].w, 3.0 * 8.0);
    }

    #[test]
    fn build_scene_emits_selection_quads() {
        let mut v = view("abc\ndef");
        v.begin_drag(Point::new(0, 1), false);
        v.drag_to(Point::new(1, 2));

        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(
            rect,
            (8.0, 18.0),
            true,
            &theme::Theme::default(),
            &mut prims,
        );

        let selection_quads: Vec<&Rect> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Quad { rect, color } if *color == theme::Theme::default().selection => {
                    Some(rect)
                }
                _ => None,
            })
            .collect();
        // One highlight on the anchor line (with newline tail), one on the
        // cursor line.
        assert_eq!(selection_quads.len(), 2);
        assert!(selection_quads[0].w > 2.0 * 8.0); // 2 chars + newline tail
        assert_eq!(selection_quads[1].w, 2.0 * 8.0);
    }

    fn search_match_quads(prims: &[Primitive]) -> Vec<Rect> {
        prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Quad { rect, color }
                    if *color == theme::Theme::default().search_match =>
                {
                    Some(*rect)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn build_scene_highlights_search_matches_in_view() {
        let mut v = view("foo bar\nbaz foo");
        v.vim.last_search = Some("foo".to_string());
        v.vim.search_hl = true;

        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(
            rect,
            (8.0, 18.0),
            true,
            &theme::Theme::default(),
            &mut prims,
        );

        let quads = search_match_quads(&prims);
        assert_eq!(quads.len(), 2); // one per match, on both lines
        assert_eq!(quads[0].w, 3.0 * 8.0); // 3-char pattern, cell width 8
        assert!(quads[1].x > quads[0].x); // second match starts at col 4
    }

    #[test]
    fn build_scene_skips_search_highlights_when_cleared() {
        let mut v = view("foo bar");
        v.vim.last_search = Some("foo".to_string());
        v.vim.search_hl = false; // `:noh`

        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(
            rect,
            (8.0, 18.0),
            true,
            &theme::Theme::default(),
            &mut prims,
        );
        assert!(search_match_quads(&prims).is_empty());
    }

    #[test]
    fn build_scene_only_highlights_visible_lines() {
        // 20 lines of "foo", but the pane fits only a few; quads must cover the
        // rows on screen, not the whole buffer.
        let text = vec!["foo"; 20].join("\n");
        let mut v = view(&text);
        v.vim.last_search = Some("foo".to_string());
        v.vim.search_hl = true;
        v.scroll.top = 5;

        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 100.0,
        };
        let mut prims = Vec::new();
        v.build_scene(
            rect,
            (8.0, 18.0),
            true,
            &theme::Theme::default(),
            &mut prims,
        );

        // The content band is 100 - 2*PAD = 88px at an 18px row: four whole
        // rows and a 16px sliver of a fifth. All five are drawn — the sliver is
        // real screen area, and leaving it blank is what a smooth scroll would
        // expose as a flickering strip along the bottom edge.
        let visible = EditorView::visible_lines(rect, 18.0);
        assert_eq!(visible, 4);
        assert_eq!(search_match_quads(&prims).len(), 5);
    }

    /// Text runs (the line content), excluding the gutter line numbers.
    fn text_runs(prims: &[Primitive], theme: &theme::Theme) -> Vec<(String, Color)> {
        prims
            .iter()
            .filter_map(|p| match p {
                // Skip the gutter numbers (dim, positioned at the far left).
                Primitive::Text { text, color, .. } if *color != theme.text_dim => {
                    Some((text.clone(), *color))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unhighlighted_file_renders_one_default_run_per_line() {
        // A pathless buffer has no language, so each non-empty line is a single
        // default-colored text run — identical to the pre-highlighting path.
        let v = view("hello world\nbye");
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);

        let runs = text_runs(&prims, &theme);
        assert_eq!(
            runs,
            vec![
                ("hello world".to_string(), theme.text),
                ("bye".to_string(), theme.text),
            ]
        );
    }

    #[test]
    fn tabs_expand_to_four_columns_in_runs() {
        // A leading tab renders as four spaces (to the next tab stop) before
        // the visible text, matching the display-column geometry.
        let v = view("\tx");
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);

        let runs = text_runs(&prims, &theme);
        assert_eq!(runs, vec![("    x".to_string(), theme.text)]);
    }

    #[test]
    fn interior_tabs_render_to_the_next_stop_not_a_fixed_width() {
        // "ab" fills cells 0-1; the tab pads only to cell 4, so the rendered
        // glyphs line up with display_col("ab\tc", 3) == 4.
        let v = view("ab\tc");
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);

        let runs = text_runs(&prims, &theme);
        assert_eq!(runs, vec![("ab  c".to_string(), theme.text)]);
    }

    #[test]
    fn external_styles_render_colored_runs() {
        // External style spans: whole-line added/removed colors, a styled
        // head with a plain tail, and an unstyled line rendering default.
        let mut v = view("+new line\n-old line\nplain");
        v.external_styles = vec![
            vec![StyleSpan {
                start: 0,
                end: 9,
                style: StyleKind::Added,
            }],
            vec![StyleSpan {
                start: 0,
                end: 4,
                style: StyleKind::Removed,
            }],
        ];

        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);

        let runs = text_runs(&prims, &theme);
        assert_eq!(
            runs,
            vec![
                ("+new line".to_string(), theme.syntax_string), // added → green
                ("-old".to_string(), theme.error_text),         // removed → red
                (" line".to_string(), theme.text),              // past the span
                ("plain".to_string(), theme.text),              // no entry → plain
            ]
        );
    }

    #[test]
    fn external_style_palette_maps_to_theme_colors() {
        let theme = theme::Theme::default();
        assert_eq!(style_color(StyleKind::Added, &theme), theme.syntax_string);
        assert_eq!(style_color(StyleKind::Removed, &theme), theme.error_text);
        assert_eq!(style_color(StyleKind::Hunk, &theme), theme.syntax_function);
        assert_eq!(style_color(StyleKind::Title, &theme), theme.syntax_type);
        // Dim shares text_dim with the gutter, so check the mapping directly.
        assert_eq!(style_color(StyleKind::Dim, &theme), theme.text_dim);
    }

    #[test]
    fn new_external_content_clears_stale_styles() {
        // A later content replacement without styles (a view that stopped
        // styling) must drop back to plain rendering.
        let mut v = view("");
        v.set_external_content("+styled", None);
        v.external_styles = vec![vec![StyleSpan {
            start: 0,
            end: 7,
            style: StyleKind::Added,
        }]];
        v.set_external_content("plain again", None);

        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);
        assert_eq!(
            text_runs(&prims, &theme),
            vec![("plain again".to_string(), theme.text)]
        );
    }

    #[test]
    fn rust_file_renders_multiple_colored_runs() {
        // A real .rs path triggers highlighting: the `fn` keyword run carries the
        // keyword color, distinct from the default text color.
        use std::io::Write;
        let dir =
            std::env::temp_dir().join(format!("garden-hl-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippet.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"fn main() {}\n").unwrap();
        drop(f);

        let v = EditorView::from_buffer(Buffer::open(&path).unwrap());
        let theme = theme::Theme::default();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), false, &theme, &mut prims);

        let runs = text_runs(&prims, &theme);
        // More than one run (the line is split by token color)...
        assert!(
            runs.len() > 1,
            "expected multiple colored runs, got {runs:?}"
        );
        // ...and the leading "fn" run is keyword-colored.
        assert_eq!(runs[0], ("fn".to_string(), theme.syntax_keyword));
    }

    #[test]
    fn build_scene_wraps_a_highlighted_line_without_underflow() {
        // A highlighted line that soft-wraps: the leading `fn` token ends at
        // col 2, well before a later visual row's start column, so clipping that
        // color run to the row would compute `b - s` with b < s. That tuple was
        // built eagerly inside `then_some`, underflowing `usize` (a debug panic)
        // even though the run is out of range. Regression guard — must not panic.
        use std::io::Write;
        let dir =
            std::env::temp_dir().join(format!("garden-hlwrap-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snippet.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"fn aaaaaaaaaaaaaaaaaaaaaaaa() {}\n").unwrap();
        drop(f);

        let mut v = EditorView::from_buffer(Buffer::open(&path).unwrap());
        v.wrap = true;
        v.show_line_numbers = true;
        let theme = theme::Theme::default();
        // ~6 display columns of text area, so the line wraps several times.
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 200.0,
        };
        let mut prims = Vec::new();
        v.build_scene(rect, (8.0, 18.0), true, &theme, &mut prims);
        assert!(prims.iter().any(|p| matches!(p, Primitive::Text { .. })));
    }

    // --- soft wrap ----------------------------------------------------------

    #[test]
    fn wrap_rows_keeps_a_fitting_line_on_one_row() {
        assert_eq!(wrap_rows("hello", 10), vec![0]);
        assert_eq!(wrap_rows("", 10), vec![0]);
        assert_eq!(wrap_rows("hello", 5), vec![0]); // exactly fills the width
    }

    #[test]
    fn wrap_rows_breaks_after_a_word_boundary() {
        // Width 5: "ab cd " carries the trailing space, then "ef".
        assert_eq!(wrap_rows("ab cd ef", 5), vec![0, 3]);
    }

    #[test]
    fn wrap_rows_hard_breaks_an_overlong_word() {
        assert_eq!(wrap_rows("aaaaaa", 3), vec![0, 3]);
    }

    #[test]
    fn wrap_rows_counts_tabs_by_display_width() {
        // A tab spans 4 cells, so "\tab" (6 cells) wraps after the tab at width 4.
        assert_eq!(wrap_rows("\tab", 4), vec![0, 1]);
    }

    /// A narrow pane whose text area is exactly 6 display columns wide, so
    /// `"aaaa bbbb cccc"` soft-wraps to the rows `[0, 5, 10]`.
    fn narrow(text: &str) -> (EditorView, Rect, (f32, f32)) {
        // visible_cols = floor((w - 2*PAD) / cell_w) = floor((60 - 12) / 8) = 6.
        (
            view(text),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 60.0,
                h: 200.0,
            },
            (8.0, 18.0),
        )
    }

    #[test]
    fn build_scene_wraps_a_long_line_onto_several_rows() {
        let (v, rect, cell) = narrow("aaaa bbbb cccc");
        let theme = theme::Theme::default();
        let mut prims = Vec::new();
        v.build_scene(rect, cell, false, &theme, &mut prims);

        // The one buffer line renders as text runs on three rows one cell apart.
        let mut ys: Vec<f32> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Text { pos, .. } => Some(pos.1),
                _ => None,
            })
            .collect();
        ys.dedup();
        assert_eq!(ys, vec![6.0, 6.0 + 18.0, 6.0 + 2.0 * 18.0]);
    }

    #[test]
    fn caret_renders_on_the_wrapped_continuation_row() {
        let (mut v, rect, cell) = narrow("aaaa bbbb cccc");
        v.cursor = Point::new(0, 12); // on the third wrapped row (starts [0,5,10])
        let theme = theme::Theme::default();
        let mut prims = Vec::new();
        v.build_scene(rect, cell, true, &theme, &mut prims);
        let caret = prims
            .iter()
            .find_map(|p| match p {
                Primitive::Quad { rect, color } if *color == theme.cursor_block => Some(*rect),
                _ => None,
            })
            .expect("caret quad");
        assert_eq!(caret.y, 6.0 + 2.0 * 18.0); // third visual row
        assert_eq!(caret.x, 6.0 + 2.0 * 8.0); // col 12 sits two cells into "cccc"
    }

    #[test]
    fn click_maps_onto_the_wrapped_row_and_column() {
        let (v, rect, cell) = narrow("aaaa bbbb cccc"); // rows start at [0, 5, 10]
        let (ox, oy) = (6.0, 6.0);
        // Second visual row, first column → char 5.
        assert_eq!(
            v.position_for_click(rect, cell, ox, oy + 18.0),
            Point::new(0, 5)
        );
        // Third visual row, two cells in → char 12.
        assert_eq!(
            v.position_for_click(rect, cell, ox + 2.0 * 8.0, oy + 2.0 * 18.0),
            Point::new(0, 12)
        );
    }

    #[test]
    fn scroll_by_advances_visual_rows_inside_a_wrapped_line() {
        // A single line that wraps to many rows; the wheel moves sub-rows.
        let mut v = view("aaaa bbbb cccc dddd eeee ffff gggg hhhh");
        v.scroll_by(2.0, 3, 6);
        assert_eq!(v.scroll.top, 0);
        assert_eq!(v.scroll.sub, 2);
        v.scroll_by(-1.0, 3, 6);
        assert_eq!(v.scroll.sub, 1);
    }

    #[test]
    fn ensure_cursor_visible_scrolls_within_a_wrapped_line() {
        let mut v = view("aaaa bbbb cccc dddd eeee ffff gggg hhhh");
        v.cursor = Point::new(0, 34); // deep into the wrapped line
        v.ensure_cursor_visible(3, 6);
        let (_, cs) = v.cursor_vpos(6);
        // The cursor's visual row is within the 3-row window at the new offset.
        assert!(v.scroll.sub > 0, "expected to scroll into the wrapped line");
        assert!(
            v.scroll.sub <= cs && cs < v.scroll.sub + 3,
            "cursor row {cs} off screen"
        );
    }

    #[test]
    fn wrapping_pins_horizontal_scroll_to_zero() {
        let (mut v, _rect, _cell) = narrow("aaaa bbbb cccc");
        v.scroll_h_by(100.0); // a no-op while wrapping
        assert_eq!(v.scroll.left, 0.0);
        v.cursor = Point::new(0, 12);
        v.ensure_cursor_visible(10, 6);
        assert_eq!(v.scroll.left, 0.0);
    }

    // --- scrollbars ---------------------------------------------------------

    fn many_lines(n: usize) -> String {
        (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n")
    }

    /// `scroll_to_line` puts the asked-for line at the top and the cursor on it,
    /// clamped only to the buffer: a line number past the end lands on the last
    /// line, and that line still goes to the top row.
    #[test]
    fn scroll_to_line_tops_the_view_and_clamps() {
        let mut v = view(&many_lines(50));
        v.wrap = false;
        v.scroll_to_line(20, 40);
        assert_eq!(v.scroll.top, 20);
        assert_eq!(v.cursor.line, 20);
        assert_eq!(v.anchor, None);

        // Past the end: cursor on the last line, and that line at the top.
        v.scroll_to_line(999, 40);
        assert_eq!(v.cursor.line, 49);
        assert_eq!(v.scroll.top, 49);
    }

    /// The anchor a file list jumps with has to reach the top *whatever* is
    /// below it: a target within the last screenful used to be pulled back to
    /// the last screenful's start — a line belonging to some earlier file, which
    /// is exactly what made clicking one of the last files in a diff look like
    /// it jumped to the wrong one. The trailing blank rows are the price.
    #[test]
    fn scroll_to_line_anchors_a_target_near_the_end_at_the_top() {
        let mut v = view(&many_lines(50));
        v.wrap = false;
        // 50 lines, 10 visible: line 45 is inside the last screenful.
        v.scroll_to_line(45, 40);
        assert_eq!(v.scroll.top, 45);

        // …and a keystroke afterwards does not yank it back: the cursor is on
        // the top row, so nothing needed scrolling.
        v.ensure_cursor_visible(10, 40);
        assert_eq!(v.scroll.top, 45);

        // Scrolling by hand still clamps normally — the anchor is not sticky.
        v.scroll_by(1.0, 10, 40);
        assert_eq!(v.scroll.top, 40);
    }

    /// The clamp `ensure_cursor_visible` gives up for an anchored view is still
    /// applied whenever it actually scrolls, so a buffer that shrank under a
    /// scrolled-down view never strands it below its own content.
    #[test]
    fn ensure_cursor_visible_still_recovers_a_shrunken_buffer() {
        let mut v = view(&many_lines(50));
        v.wrap = false;
        v.scroll_to_line(40, 40);
        v.set_external_content(&many_lines(5), None);
        v.ensure_cursor_visible(10, 40);
        assert_eq!(v.scroll.top, 0, "5 lines in a 10-row viewport start at 0");
    }

    #[test]
    fn vertical_scrollbar_shows_and_drags_to_the_bottom() {
        let mut v = view(&many_lines(50));
        v.wrap = false;
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        let cell = (8.0, 18.0);
        let visible_lines = EditorView::visible_lines(rect, cell.1);
        let cols = v.visible_cols(rect, cell.0);
        let (vbar, hbar) = v.scrollbar_geom(rect, visible_lines, cols);
        let vbar = vbar.expect("vertical scrollbar");
        assert!(hbar.is_none(), "short lines need no horizontal bar");

        // Grab the thumb, then drag far below the track: scroll pins the last
        // line to the bottom.
        let hit = v
            .scrollbar_hit(rect, cell, vbar.thumb.x + 1.0, vbar.thumb.y + 1.0)
            .expect("hit the thumb");
        assert_eq!(hit.axis, ScrollAxis::Vertical);
        v.drag_scroll(hit.axis, hit.grab, rect, cell, vbar.thumb.x, 10_000.0);
        assert_eq!(v.scroll.top, v.buffer.line_count() - visible_lines);
    }

    #[test]
    fn wrap_mode_has_no_horizontal_scrollbar() {
        let v = view("a single long line well beyond the pane width, wrapped not scrolled");
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 200.0,
        };
        let cols = v.visible_cols(rect, 8.0);
        let (_, hbar) = v.scrollbar_geom(rect, EditorView::visible_lines(rect, 18.0), cols);
        assert!(hbar.is_none());
    }

    #[test]
    fn clicking_the_text_area_is_not_a_scrollbar_hit() {
        let v = view(&many_lines(50));
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 100.0,
        };
        // The top-left text cell is nowhere near the right-edge scrollbar.
        assert!(v.scrollbar_hit(rect, (8.0, 18.0), 10.0, 10.0).is_none());
    }
}

/// Pane border as four 1px quads.
fn border(rect: Rect, color: Color, prims: &mut Vec<Primitive>) {
    let Rect { x, y, w, h } = rect;
    for r in [
        Rect { x, y, w, h: 1.0 },
        Rect {
            x,
            y: y + h - 1.0,
            w,
            h: 1.0,
        },
        Rect { x, y, w: 1.0, h },
        Rect {
            x: x + w - 1.0,
            y,
            w: 1.0,
            h,
        },
    ] {
        prims.push(Primitive::Quad { rect: r, color });
    }
}
