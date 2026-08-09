//! Editable projections — edit a *view*, and have the edits fold back into the
//! sources the view was built from.
//!
//! A projection is a document assembled out of other documents: a unified diff
//! mixing a file's current lines with the base lines it dropped, a multi-file
//! grep result, a review thread woven through a patch. The point of this module
//! is that such a document can be *edited with the ordinary editor* — real vim,
//! real undo — and still be written back exactly, without anyone having to guess
//! afterwards what the user meant.
//!
//! # Why provenance, not alignment
//!
//! The obvious way to fold an edited projection back is to diff the edited text
//! against what was projected and read the differences as intent. That is what
//! `garden-diff` used to do (an LCS aligner plus a set of heuristics for pairing
//! deletions with insertions), and it is fundamentally a guess: by the time the
//! text is compared, the information about *what edit happened* has been thrown
//! away.
//!
//! This module keeps that information instead. Every projected line carries its
//! [`LineOrigin`] — where its content came from and what it means — and the
//! table of origins is transformed in lockstep with the buffer as it is edited
//! ([`Projection::splice`]). Saving is then a fold over the final table
//! ([`Projection::resolve`]), not an alignment. Nothing is inferred.
//!
//! The payoff is that the projection needs no per-command support: every buffer
//! mutation funnels through the same splice, so `dd`, `3dd`, `cc`, `V}d`, `p`,
//! `J`, `x`, `.`-repeat and insert-mode typing all fold back correctly with no
//! work of their own.
//!
//! # Two tiers
//!
//! **Tier 1 — the transform** (above) handles anything expressible as "these
//! projected lines changed". It is automatic and total.
//!
//! **Tier 2 — [`Intent`]s** handle edits that mean something the transform
//! cannot express, because they are requests about the *structure* rather than
//! the text: deleting a hunk header to revert that hunk, deleting a file header
//! to drop that file's changes. Before a line-wise edit lands as text, the
//! caller offers it to [`Projection::intent`]; the projection either claims it
//! (returning the text edit to perform instead, with its table already patched),
//! refuses it, or passes — in which case tier 1 handles it as usual. This is the
//! extension point: a projection that does not care about structure simply never
//! claims anything and still works.

/// One line of a projection: where its content came from, and what editing it
/// means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineOrigin {
    /// Content the source currently holds. `added` marks it as part of the
    /// change the projection is showing (an addition relative to the base)
    /// rather than untouched context — the distinction a revert acts on.
    Live { added: bool },
    /// Content the base held and the source no longer does — a deleted line,
    /// shown so it can be read and revived. It contributes nothing while it is
    /// visible; **deleting** the projected line is what puts `text` back.
    Ghost { text: String },
    /// Content typed fresh into the projection, belonging to whichever span it
    /// was typed inside.
    New,
    /// Structure rather than content: markers, titles, threaded comments.
    /// Contributes nothing to any source no matter how it is edited, so a user
    /// who retypes a marker can no longer corrupt a write-back.
    Chrome { role: ChromeRole, locked: bool },
}

/// What a chrome line stands for — which decides what *deleting* it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeRole {
    /// Inert decoration: a title, a threaded comment, a separator.
    Plain,
    /// Heads the span it belongs to. Deleting it asks to revert that span.
    SpanHeader,
    /// Heads a group of spans. Deleting it asks to revert all of them.
    GroupHeader,
}

/// A contiguous stretch of a source that the projection covers and may rewrite:
/// folding this span's lines produces the replacement for `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Index into [`Projection::sources`].
    pub source: u32,
    /// The 0-based `[start, end)` line range of the source this span replaces.
    pub target: (usize, usize),
    /// Optional grouping key, so several spans can be reverted as one (a file's
    /// hunks share a group).
    pub group: Option<u32>,
}

/// One line's entry in the projection table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub origin: LineOrigin,
    /// Index into [`Projection::styles`] — the semantic style name the host
    /// paints this line with. Carried per entry (rather than as a positional
    /// list) so it follows its line through insertions and deletions instead of
    /// drifting.
    pub style: u16,
    /// The span this line's content belongs to; `None` for chrome that sits
    /// outside every span (a document title).
    pub span: Option<u32>,
    /// Whether this entry currently occupies a buffer line. A hidden entry is a
    /// *resolved* one: a deleted `Live` line (gone from the source), or — the
    /// interesting case — a deleted [`LineOrigin::Ghost`], whose text comes back.
    pub visible: bool,
}

/// How projected lines are decorated, so the fold can strip the decoration back
/// off and a revert can redecorate a revived line.
///
/// For a unified diff this is `("+", "-", " ")` with the matching style names.
/// A projection that decorates nothing leaves the prefixes empty, and every
/// prefix operation below becomes a no-op.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decor {
    /// Prefix on a line the source holds unchanged, and the style it wears.
    pub same: (String, String),
    /// Prefix on a line added relative to the base, and its style.
    pub added: (String, String),
    /// Prefix on a line the base held and the source dropped, and its style.
    pub removed: (String, String),
    /// How the text of a freshly typed line is read.
    pub new_line: NewLine,
    /// Whether the three markers live in a **gutter** beside the text rather
    /// than at the head of the buffer line.
    ///
    /// This is the difference between a diff you can *read* and a diff you can
    /// *edit*. With the markers in the text, every buffer operation is one
    /// character out of step with the content: `J` joins `+one` and `+two` into
    /// `+one +two`, `0` puts the cursor on a `+` instead of the indent, a
    /// column selection takes the marker with it, and search hits shift. With
    /// `gutter`, the buffer holds the file's own text and nothing else — the
    /// markers are display, drawn by the view from the origin table — so all of
    /// that behaves the way it does in a normal file.
    ///
    /// It turns off every text-level use of the prefixes: nothing is stripped
    /// on read (there is nothing there to strip), nothing is prepended on
    /// revert, and a typed line is taken literally, so a user who genuinely
    /// wants a leading `+` in their code gets one. The strings stay — they are
    /// the glyphs the gutter draws, via [`Decor::marker`].
    pub gutter: bool,
}

/// How a line the user typed from scratch contributes to its source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewLine {
    /// Taken literally, decoration and all. Right for a projection that does not
    /// decorate.
    #[default]
    Literal,
    /// Read as a diff line: a leading "same" or "added" prefix is stripped, and
    /// a line typed with the "removed" prefix contributes nothing (the user
    /// wrote a deletion). A line with no recognised prefix is taken literally —
    /// typing a bare line into a diff means to add it.
    DiffMarker,
}

impl Decor {
    /// The decoration `origin` expects at the head of its projected line —
    /// always empty in [`gutter`](Self::gutter) mode, where the line's text is
    /// the source's text verbatim.
    fn prefix(&self, origin: &LineOrigin) -> &str {
        if self.gutter {
            return "";
        }
        self.marker(origin)
    }

    /// The glyph that stands for `origin` — what a gutter draws beside the
    /// line, and (outside [`gutter`](Self::gutter) mode) what the line's text
    /// is prefixed with. Empty for anything that is not base-relative content:
    /// a typed line has no diff status yet, and chrome is not content at all.
    pub fn marker(&self, origin: &LineOrigin) -> &str {
        match origin {
            LineOrigin::Live { added: false } => &self.same.0,
            LineOrigin::Live { added: true } => &self.added.0,
            LineOrigin::Ghost { .. } => &self.removed.0,
            LineOrigin::New | LineOrigin::Chrome { .. } => "",
        }
    }

    /// What a visible content line contributes to its source. `None` means it
    /// contributes nothing.
    ///
    /// A line still wearing the decoration its origin expects is read straight —
    /// the overwhelmingly common case. A line that is *not* is read as if the
    /// user had typed it, which is exactly what happened: retyping ` three` as
    /// `-three` in a diff means "delete this line", and stripping the expected
    /// space would instead have written a literal `-three` into the file.
    fn read(&self, origin: &LineOrigin, line: &str) -> Option<String> {
        let prefix = self.prefix(origin);
        if !prefix.is_empty() {
            if let Some(rest) = line.strip_prefix(prefix) {
                return Some(rest.to_string());
            }
        }
        self.typed(line)
    }

    /// What a freshly typed `line` contributes, under [`NewLine`]'s rules.
    /// `None` means it contributes nothing.
    fn typed(&self, line: &str) -> Option<String> {
        // In gutter mode there is no marker in the text to interpret, so a
        // typed line is its own content — including a leading `+` or `-`, which
        // is now just a character in the file rather than a diff verb.
        if self.gutter {
            return Some(line.to_string());
        }
        match self.new_line {
            NewLine::Literal => Some(line.to_string()),
            NewLine::DiffMarker => {
                for prefix in [&self.added.0, &self.same.0] {
                    if !prefix.is_empty() {
                        if let Some(rest) = line.strip_prefix(prefix.as_str()) {
                            return Some(rest.to_string());
                        }
                    }
                }
                if !self.removed.0.is_empty() && line.starts_with(&self.removed.0) {
                    return None;
                }
                Some(line.to_string())
            }
        }
    }
}

/// One resolved write-back: replace source lines `[start, end)` with `lines`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    /// The source's opaque name, as the projection's author supplied it (a file
    /// path, a URL — this crate never interprets it).
    pub source: String,
    pub start: usize,
    pub end: usize,
    pub lines: Vec<String>,
    /// What this span held when the projection was built — the lines the writer
    /// should still find at `[start, end)`. It is the baseline the span was
    /// judged dirty against ([`Projection::capture_baseline`]), so a writer can
    /// compare it with the source's *current* content and tell the user their
    /// edit landed on top of someone else's change. `None` when no baseline was
    /// captured, which means "no expectation recorded" — not "expect nothing".
    pub expected: Option<Vec<String>>,
}

/// A structural edit offered to the projection before it lands as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Whole projected lines `[start, start + count)` are about to be deleted.
    DeleteLines { start: usize, count: usize },
}

/// What the projection made of an [`Intent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not the projection's business — apply it as an ordinary buffer edit and
    /// let the tier-1 transform handle it.
    Pass,
    /// Refused, with a reason to show the user. The edit must not be applied.
    Refused(String),
    /// Claimed: perform *these* row operations instead, covering buffer rows
    /// `[start, start + ops.len())`. The table has already been patched to
    /// match, so the caller applies the text edits *without* calling
    /// [`Projection::splice`] for them.
    Claimed { start: usize, ops: Vec<RowOp> },
}

/// What a claimed [`Outcome`] does to one buffer row. Row-wise rather than a
/// replacement block because the projection holds no buffer: it can name the
/// lines it rewrites, but not the current text of the ones it merely keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowOp {
    /// Leave the row's text alone.
    Keep,
    /// Delete the row.
    Delete,
    /// Replace the row's text.
    Set(String),
}

/// One transformation of the entry table, kept so it can be undone and redone
/// in step with the buffer's own undo stack. Stores both sides of the touched
/// range rather than a snapshot of the whole table, so the cost is proportional
/// to the edit, not to the document.
#[derive(Clone, Debug)]
struct Change {
    /// The buffer's `undo_index` *after* the edit that caused this change.
    /// Coalesced and grouped buffer edits share one index, so undoing to `k`
    /// correctly reverts every change recorded above `k` — see [`Projection::sync_to`].
    undo_index: usize,
    at: usize,
    before: Vec<Entry>,
    after: Vec<Entry>,
}

/// An editable projection: the sources, the spans of them it covers, and the
/// per-line origin table that is kept in step with the buffer.
#[derive(Clone, Debug, Default)]
pub struct Projection {
    /// Opaque source names, indexed by [`Span::source`].
    pub sources: Vec<String>,
    pub spans: Vec<Span>,
    pub decor: Decor,
    /// Interned semantic style names, indexed by [`Entry::style`].
    styles: Vec<String>,
    entries: Vec<Entry>,
    history: Vec<Change>,
    redo: Vec<Change>,
    /// What each span folded to when the projection was built — one entry per
    /// span, or empty when [`capture_baseline`](Self::capture_baseline) was
    /// never called. This is what makes [`resolve`](Self::resolve) a report of
    /// *edits* rather than of the whole document: a span that still folds to its
    /// baseline is not emitted at all.
    baseline: Vec<Vec<String>>,
}

impl Projection {
    pub fn new(sources: Vec<String>, spans: Vec<Span>, decor: Decor) -> Projection {
        Projection {
            sources,
            spans,
            decor,
            styles: vec![String::new()],
            entries: Vec::new(),
            history: Vec::new(),
            redo: Vec::new(),
            baseline: Vec::new(),
        }
    }

    /// Append one projected line. Lines are appended in the order they appear in
    /// the projected text, so entry order *is* document order.
    pub fn push(&mut self, origin: LineOrigin, style: &str, span: Option<u32>) {
        let style = self.intern(style);
        self.entries.push(Entry {
            origin,
            style,
            span,
            visible: true,
        });
    }

    fn intern(&mut self, name: &str) -> u16 {
        if let Some(i) = self.styles.iter().position(|s| s == name) {
            return i as u16;
        }
        self.styles.push(name.to_string());
        (self.styles.len() - 1) as u16
    }

    fn style_name(&self, i: u16) -> &str {
        self.styles
            .get(i as usize)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The number of buffer lines the table currently accounts for. Should equal
    /// the buffer's line count; a mismatch means a mutation bypassed
    /// [`splice`](Self::splice).
    pub fn visible_len(&self) -> usize {
        self.entries.iter().filter(|e| e.visible).count()
    }

    /// The per-line style names for the visible lines, in buffer order — what
    /// the host paints. Derived from the table, so the bands stay on their lines
    /// as the document is edited instead of drifting off them.
    pub fn line_styles(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.visible)
            .map(|e| self.style_name(e.style).to_string())
            .collect()
    }

    /// The gutter glyph for each visible line, parallel to the buffer's rows —
    /// what a view draws beside the text in [`Decor::gutter`] mode. Empty
    /// outside that mode (the markers are in the text there, so a gutter would
    /// show them twice) and empty per-line for chrome and freshly typed lines.
    ///
    /// Like [`line_styles`](Self::line_styles) this is derived from the table,
    /// so a marker follows its line through insertions and deletions instead of
    /// drifting off it.
    pub fn line_markers(&self) -> Vec<String> {
        if !self.decor.gutter {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|e| e.visible)
            .map(|e| self.decor.marker(&e.origin).to_string())
            .collect()
    }

    /// The decoration a line freshly opened at `row` should start with: the
    /// "added" prefix, when `row` sits inside a span this projection can write
    /// back. `None` outside every span (a new line there has nowhere to go) and
    /// for an undecorated projection.
    ///
    /// This is what stops vim's autoindent from copying a `+`/`-`/space as if it
    /// were indentation: in a diff, a line you open is a line you are *adding*,
    /// so it wears the addition marker rather than its neighbour's.
    pub fn new_line_decor(&self, row: usize) -> Option<&str> {
        // Nothing to copy in gutter mode: the marker was never in the text, so
        // an opened line simply starts empty (and the gutter shows it as an
        // addition on its own).
        if self.decor.gutter || self.decor.added.0.is_empty() {
            return None;
        }
        let entry = self.entries.get(self.entry_at(row))?;
        entry.span?;
        Some(&self.decor.added.0)
    }

    /// The decoration the line at `row` currently wears — what a caller has to
    /// look past to find the line's real content (its indent, say).
    pub fn worn_decor(&self, row: usize) -> &str {
        match self.entries.get(self.entry_at(row)) {
            Some(e) => self.decor.prefix(&e.origin),
            None => "",
        }
    }

    /// The entry index of buffer row `row` (or the end of the table).
    fn entry_at(&self, row: usize) -> usize {
        let mut seen = 0;
        for (i, e) in self.entries.iter().enumerate() {
            if e.visible {
                if seen == row {
                    return i;
                }
                seen += 1;
            }
        }
        self.entries.len()
    }

    /// The buffer row an entry occupies (its index among the visible entries).
    fn row_of(&self, entry: usize) -> usize {
        self.entries[..entry.min(self.entries.len())]
            .iter()
            .filter(|e| e.visible)
            .count()
    }

    // ── tier 1: the transform ──────────────────────────────────────────────

    /// Reconcile the table with a buffer edit that replaced buffer lines
    /// `[start, start + removed)` with `inserted` fresh lines. `undo_index` is
    /// the buffer's undo position after the edit (see [`Change::undo_index`]).
    ///
    /// Removed and inserted lines are paired positionally, shortest run first:
    /// a paired line was **retexted** (its entry survives, and the fold reads
    /// its new text out of the buffer), a surplus removed line is **hidden**
    /// (resolved — a `Live` line leaves its source, a `Ghost` comes back), and a
    /// surplus inserted line becomes a [`LineOrigin::New`] entry inheriting its
    /// neighbour's span. Because the caller knows the exact extent of the edit,
    /// this pairing is a fact rather than the guess an after-the-fact alignment
    /// would have to make.
    pub fn splice(&mut self, start: usize, removed: usize, inserted: usize, undo_index: usize) {
        if removed == 0 && inserted == 0 {
            return;
        }
        let first = self.entry_at(start);
        // The entry range spanning `removed` visible rows. Hidden entries caught
        // inside it are already resolved and are left exactly as they are.
        let mut end = first;
        let mut seen = 0;
        while end < self.entries.len() && seen < removed {
            if self.entries[end].visible {
                seen += 1;
            }
            end += 1;
        }
        let visible: Vec<usize> = (first..end).filter(|i| self.entries[*i].visible).collect();
        let before = self.entries[first..end].to_vec();

        let same_style = self.same_style();
        let paired = visible.len().min(inserted);
        for &i in &visible[..paired] {
            // A retexted ghost is the one origin that changes meaning: typing
            // over a deleted line means you want that text in the file, so the
            // line stops being a ghost and becomes freshly authored content.
            if matches!(self.entries[i].origin, LineOrigin::Ghost { .. }) {
                self.entries[i].origin = LineOrigin::New;
                self.entries[i].style = same_style;
            }
        }
        for &i in &visible[paired..] {
            self.entries[i].visible = false;
        }

        let extra = inserted.saturating_sub(visible.len());
        if extra > 0 {
            let at = if paired > 0 {
                visible[paired - 1] + 1
            } else {
                first
            };
            let (span, style) = self.inherit(at);
            let fresh = Entry {
                origin: LineOrigin::New,
                style,
                span,
                visible: true,
            };
            self.entries
                .splice(at..at, std::iter::repeat(fresh).take(extra));
        }

        let after_len = (end - first) + extra;
        let after = self.entries[first..first + after_len].to_vec();
        self.record(Change {
            undo_index,
            at: first,
            before,
            after,
        });
    }

    /// The span and style a line inserted at entry position `at` should adopt:
    /// its preceding neighbour's, falling back to the following one's (an
    /// insertion at the very top). Chrome is never inherited from — a line typed
    /// under a hunk header belongs to the hunk, not to the header.
    ///
    /// A header *ends* the block above it, so the backward search does not walk
    /// through one: a line typed inside a file's heading block looks **forward**,
    /// to the file the heading introduces, rather than backward into the
    /// previous file's last line. Plain chrome — a woven review comment, a
    /// separator — sits inside a block and is merely skipped.
    fn inherit(&self, at: usize) -> (Option<u32>, u16) {
        let content = |e: &Entry| !matches!(e.origin, LineOrigin::Chrome { .. });
        let heads = |e: &Entry| {
            matches!(
                e.origin,
                LineOrigin::Chrome {
                    role: ChromeRole::SpanHeader | ChromeRole::GroupHeader,
                    ..
                }
            )
        };
        let mut before = None;
        let mut headed = false;
        for e in self.entries[..at].iter().rev() {
            if content(e) {
                before = Some(e);
                break;
            }
            headed |= heads(e);
        }
        let after = self.entries[at..].iter().find(|e| content(e));
        // With a header in between, `before` belongs to another block; it stays
        // only as the last resort when nothing follows.
        let pick = if headed {
            after.or(before)
        } else {
            before.or(after)
        };
        match pick {
            Some(e) => (e.span, e.style),
            None => (self.entries.get(at).and_then(|e| e.span), 0),
        }
    }

    fn record(&mut self, change: Change) {
        self.redo.clear();
        self.history.push(change);
    }

    /// Bring the table back in step with the buffer's undo position: undo every
    /// recorded change above `undo_index`, redo every one at or below it that
    /// was previously undone. Called after the buffer undoes or redoes.
    ///
    /// Keying on the buffer's own undo index (rather than counting steps) is
    /// what keeps the two in step through coalescing and undo groups, where one
    /// buffer transaction absorbs many edits.
    pub fn sync_to(&mut self, undo_index: usize) {
        while self
            .history
            .last()
            .is_some_and(|c| c.undo_index > undo_index)
        {
            let c = self.history.pop().expect("checked");
            self.entries
                .splice(c.at..c.at + c.after.len(), c.before.iter().cloned());
            self.redo.push(c);
        }
        while self.redo.last().is_some_and(|c| c.undo_index <= undo_index) {
            let c = self.redo.pop().expect("checked");
            self.entries
                .splice(c.at..c.at + c.before.len(), c.after.iter().cloned());
            self.history.push(c);
        }
    }

    // ── the fold ───────────────────────────────────────────────────────────

    /// Record what every span currently folds to, as the state the projection
    /// was built *from* — call it once, immediately after pushing the lines,
    /// with the same text the projection was seeded with.
    ///
    /// Two things hang off this baseline. It is what makes
    /// [`resolve`](Self::resolve) report only the spans the user actually
    /// changed (with no baseline, every span looks like an edit, and saving a
    /// freshly opened view would blindly rewrite every hunk it shows). And it
    /// travels out on each [`SourceEdit::expected`], so the writer can tell
    /// whether the source still holds what was projected.
    ///
    /// Capturing again later would silently adopt the user's edits as the new
    /// "unedited" state — so a host that rebuilds a projection must capture on
    /// the rebuild, and must *not* re-capture a projection it is keeping.
    pub fn capture_baseline(&mut self, lines: &[String]) {
        self.baseline = self.fold(lines);
    }

    /// Fold the projection back into its sources: one [`SourceEdit`] per span
    /// **whose content changed** since [`capture_baseline`](Self::capture_baseline),
    /// in span order. `lines` is the buffer's current text, line by line.
    ///
    /// Skipping the unchanged spans is not an optimisation: an edit is an
    /// instruction to overwrite a range of a file, so emitting one for a hunk
    /// nobody touched asks the writer to force this view's idea of that hunk
    /// onto whatever the file holds now. It also makes the edit count a
    /// truthful "unsaved changes" signal — zero on a freshly built projection,
    /// back to zero when an edit is undone.
    ///
    /// With no baseline captured, every span is emitted (with no expectation) —
    /// the raw fold.
    ///
    /// Each span's replacement is its entries' contributions, in table order:
    ///
    /// | entry | contributes |
    /// |---|---|
    /// | visible `Live` | its current buffer text, undecorated ([`Decor::read`]) |
    /// | hidden `Live` | nothing — the line was deleted from the source |
    /// | visible `Ghost` | nothing — the deletion still stands |
    /// | hidden `Ghost` | its base text — the deletion was reverted |
    /// | visible `New` | its buffer text, read the same way |
    /// | hidden `New` | nothing |
    /// | `Chrome` | nothing, however it was edited |
    pub fn resolve(&self, lines: &[String]) -> Vec<SourceEdit> {
        self.fold(lines)
            .into_iter()
            .enumerate()
            .filter_map(|(i, folded)| {
                let span = self.spans.get(i)?;
                let expected = self.baseline.get(i);
                if expected.is_some_and(|base| *base == folded) {
                    return None;
                }
                Some(SourceEdit {
                    source: self.sources.get(span.source as usize)?.clone(),
                    start: span.target.0,
                    end: span.target.1,
                    lines: folded,
                    expected: expected.cloned(),
                })
            })
            .collect()
    }

    /// The raw fold: what each span's lines currently resolve to, in span order,
    /// whether or not anything changed. [`resolve`](Self::resolve) is this plus
    /// the dirty filter.
    fn fold(&self, lines: &[String]) -> Vec<Vec<String>> {
        let mut content: Vec<Vec<String>> = vec![Vec::new(); self.spans.len()];
        let mut row = 0;
        for entry in &self.entries {
            let text = if entry.visible {
                let t = lines.get(row).cloned().unwrap_or_default();
                row += 1;
                t
            } else {
                String::new()
            };
            let Some(span) = entry.span else { continue };
            let Some(bucket) = content.get_mut(span as usize) else {
                continue;
            };
            let line = match (&entry.origin, entry.visible) {
                (LineOrigin::Live { .. } | LineOrigin::New, true) => {
                    self.decor.read(&entry.origin, &text)
                }
                (LineOrigin::Ghost { text }, false) => Some(text.clone()),
                _ => None,
            };
            if let Some(line) = line {
                bucket.push(line);
            }
        }
        content
    }

    // ── tier 2: intents ────────────────────────────────────────────────────

    /// Offer a structural edit to the projection before it lands as text. See
    /// [`Outcome`] for what the three answers oblige the caller to do.
    ///
    /// A deletion's *first* line decides what it means: deleting a span header
    /// reverts its span, deleting a group header reverts every span in the
    /// group. Anything else passes, so an ordinary delete stays an ordinary
    /// text edit — which the tier-1 transform already handles correctly.
    ///
    /// The rest of the range is only checked for structure it would destroy,
    /// and the check is the same whatever the range's length: a deletion is
    /// refused if it covers a locked chrome line anywhere, or a span/group
    /// header anywhere but as its own first line. The bytes such a delete
    /// writes back are perfectly correct — but the *view* it leaves behind
    /// lies, showing one file's hunks under another file's heading, so it is
    /// refused rather than silently applied.
    pub fn intent(&mut self, intent: Intent, undo_index: usize) -> Outcome {
        let Intent::DeleteLines { start, count } = intent;
        if count == 0 {
            return Outcome::Pass;
        }
        let first = self.entry_at(start);
        // A locked chrome line is not part of the change the projection shows —
        // a document title, say. Refusing is friendlier than letting it be
        // deleted from a view it does not belong to; and a header buried inside
        // a range is structure the deletion never asked to revert.
        let refusal = || Outcome::Refused("this line is part of the view, not the change".into());
        let deleted = self.entries[first..]
            .iter()
            .filter(|e| e.visible)
            .take(count);
        for (n, entry) in deleted.enumerate() {
            let LineOrigin::Chrome { role, locked } = &entry.origin else {
                continue;
            };
            if *locked {
                return refusal();
            }
            if n > 0 && matches!(role, ChromeRole::SpanHeader | ChromeRole::GroupHeader) {
                return refusal();
            }
        }
        let Some(entry) = self.entries.get(first) else {
            return Outcome::Pass;
        };
        let (role, span) = match &entry.origin {
            LineOrigin::Chrome { role, .. } => (*role, entry.span),
            _ => return Outcome::Pass,
        };
        let Some(span) = span else {
            return Outcome::Pass;
        };
        let targets: Vec<u32> = match role {
            ChromeRole::Plain => return Outcome::Pass,
            ChromeRole::SpanHeader => vec![span],
            ChromeRole::GroupHeader => match self.spans.get(span as usize).and_then(|s| s.group) {
                Some(g) => (0..self.spans.len() as u32)
                    .filter(|i| self.spans[*i as usize].group == Some(g))
                    .collect(),
                None => vec![span],
            },
        };
        self.revert(&targets, undo_index)
    }

    /// Revert every line of `targets` to its base state: additions go away,
    /// deletions come back as context, and anything typed inside is dropped.
    /// Patches the table and returns the matching row operations — revived
    /// deletions have to be redecorated from "removed" to "same", which is a
    /// text edit only the caller can make.
    fn revert(&mut self, targets: &[u32], undo_index: usize) -> Outcome {
        let touched: Vec<usize> = (0..self.entries.len())
            .filter(|i| {
                let e = &self.entries[*i];
                e.visible && e.span.is_some_and(|s| targets.contains(&s))
            })
            .collect();
        let (Some(&lo), Some(&hi)) = (touched.first(), touched.last()) else {
            return Outcome::Pass;
        };
        let start = self.row_of(lo);
        let before = self.entries[lo..=hi].to_vec();
        let same_style = self.same_style();
        // What a revived deletion is redecorated *to*. Empty in gutter mode:
        // the marker is not in the text, so the revived line needs no rewrite
        // beyond its own base text (the gutter follows the changed origin).
        let same_prefix = if self.decor.gutter {
            String::new()
        } else {
            self.decor.same.0.clone()
        };

        let mut ops = Vec::new();
        for i in lo..=hi {
            let entry = &mut self.entries[i];
            if !entry.visible {
                continue;
            }
            // A line of some *other* span caught between two targeted ones still
            // occupies a row, so it needs an op — an untouched one.
            if !entry.span.is_some_and(|s| targets.contains(&s)) {
                ops.push(RowOp::Keep);
                continue;
            }
            match &entry.origin {
                // Chrome inside the reverted range — the header itself, threaded
                // comments — is not content, and stays exactly as it is.
                LineOrigin::Chrome { .. } | LineOrigin::Live { added: false } => {
                    ops.push(RowOp::Keep)
                }
                LineOrigin::Live { added: true } | LineOrigin::New => {
                    entry.visible = false;
                    ops.push(RowOp::Delete);
                }
                LineOrigin::Ghost { text } => {
                    // The deletion is undone: the base line is content again,
                    // redecorated from "removed" to "same".
                    let restored = format!("{same_prefix}{text}");
                    entry.origin = LineOrigin::Live { added: false };
                    entry.style = same_style;
                    ops.push(RowOp::Set(restored));
                }
            }
        }
        let after = self.entries[lo..=hi].to_vec();
        self.record(Change {
            undo_index,
            at: lo,
            before,
            after,
        });
        Outcome::Claimed { start, ops }
    }

    /// The interned style of an unchanged (context) line — what a revived
    /// deletion and a line typed over one both become.
    fn same_style(&mut self) -> u16 {
        let name = self.decor.same.1.clone();
        self.intern(&name)
    }
}

#[cfg(test)]
mod tests;
