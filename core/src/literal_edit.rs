//! Style-preserving edits of literal values in source — the engine under the
//! path goals of [`crate::goal_based_editing`].
//!
//! A config-style file holds its settings as literals: numbers, strings,
//! colors, and records and lists of them. This module changes such a literal to
//! a new [`StaticValue`] while keeping everything the author typed that the
//! change does not force out. The design is in `docs/source-preservation.md`;
//! in short there are two mechanisms, and nothing else:
//!
//! 1. **Minimal splices.** The old literal and the new value are walked
//!    together. Wherever they already agree, *no text is touched* — so every
//!    unchanged sub-value, and every comment, blank line and alignment column
//!    around it, comes back byte for byte. A difference is narrowed to the
//!    smallest construct that carries it: one scalar token, one inserted
//!    element, one removed element.
//! 2. **Captured style for new text.** Text that has to be synthesized takes
//!    its formatting from what is already there. A changed scalar keeps the
//!    spelling habits of the token it replaces (a padded `3.50`, an uppercase
//!    or short color). A new element or field is laid out like its siblings —
//!    the [`Style`] captured from the neighbouring literal: one-line or
//!    multi-line, the padding inside the brackets, the separator, the trailing
//!    comma, the indent step. An empty container supplies what it can (whether
//!    it is open across lines, its padding) and the file's dominant style fills
//!    in the rest.
//!
//! Every edit is one of three primitives on a path — **set** a value,
//! **insert** an element, **remove** an element — and setting a composite
//! value is planned as a short script of those same primitives, applied one at
//! a time. So a list that gains and loses elements is edited in place, and the
//! comments inside it survive.
//!
//! A value with no original to learn from renders through
//! [`StaticValue::to_source`], exactly as before.

use std::cell::OnceCell;
use std::ops::Range;

use crate::ast::{AssignTarget, Expr, ExprKind, RecordField, Stmt, StmtKind};
use crate::cst::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::goal_based_editing::Placement;
use crate::lexer::Token;
use crate::rewrite::{
    PathSeg, Splice, apply_splices, edge_significant_token, find_node, parse_ast,
    parse_binding_path, significant_range,
};
use crate::static_value::{StaticBinding, StaticValue, bindings_of, color_literal, eval};

/// What went wrong with a path edit, coarsely — enough for a host to pick a
/// status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditErrorKind {
    /// The source (before or after the edit) does not parse.
    Parse,
    /// The path text is not a path, or the edit makes no sense at that path
    /// (inserting into a number, an index past the end, a bad record key).
    Invalid,
    /// The path names nothing in the source.
    NotFound,
}

/// Why a path edit could not be applied. The source is never partly edited: an
/// error means it is exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    pub kind: EditErrorKind,
    pub message: String,
}

impl EditError {
    fn new(kind: EditErrorKind, message: impl Into<String>) -> EditError {
        EditError {
            kind,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> EditError {
        EditError::new(EditErrorKind::Invalid, message)
    }
    fn not_found(message: impl Into<String>) -> EditError {
        EditError::new(EditErrorKind::NotFound, message)
    }
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EditError {}

// ── The three primitives ─────────────────────────────────────────────────

/// Make the value at `path` (`POST.effects[2].amount`, or a bare binding name)
/// read as `value`.
///
/// Where the old and new values agree nothing is written; a value that already
/// holds returns the source byte-identical. A record field that does not exist
/// yet is added at the end of its record. Anything else the path fails to name
/// is [`EditErrorKind::NotFound`].
pub fn set_path(source: &str, path: &str, value: &StaticValue) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    set_at(source, path, &name, &segs, value)
}

/// [`set_path`] for a whole top-level binding, by name.
pub fn set_binding(source: &str, name: &str, value: &StaticValue) -> Result<String, EditError> {
    set_at(source, name, name, &[], value)
}

fn set_at(
    source: &str,
    path: &str,
    name: &str,
    segs: &[PathSeg],
    value: &StaticValue,
) -> Result<String, EditError> {
    let doc = Doc::parse(source)?;
    let (stmt, root) = doc.binding(name)?;
    let target = match resolve(root, segs) {
        Ok(expr) => expr,
        // The parent record exists and only the field is missing: add it.
        Err(depth)
            if depth + 1 == segs.len()
                && matches!(segs[depth], PathSeg::Field(_))
                && resolve(root, &segs[..depth])
                    .is_ok_and(|parent| matches!(shape(parent), Shape::Record(_))) =>
        {
            return insert_path(source, path, value, &Placement::End);
        }
        Err(_) => return Err(nothing_at(path)),
    };
    let bindings = bindings_of(&doc.stmts[..stmt], &doc.chars);
    let mut ops = Vec::new();
    doc.plan(&bindings, target, &mut segs.to_vec(), value, &mut ops);
    apply(source, name, &ops)
}

/// Insert `value` into the container `path` points into, so that afterwards
/// `path` names the new value.
///
/// - `effects[1]` inserts into the list `effects` before its current element 1
///   (an index equal to the length appends). A call's arguments are a list too.
/// - `POST.bloom` adds the field `bloom` to the record `POST`, at the end or
///   where `placement` says ([`Placement::After`] / [`Placement::Before`] name
///   a sibling field). If the field is already there its value is set instead —
///   the goal is that the field reads as `value`.
///
/// The new text is laid out like its siblings (see the module docs).
pub fn insert_path(
    source: &str,
    path: &str,
    value: &StaticValue,
    placement: &Placement,
) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    let Some((last, parent_segs)) = segs.split_last() else {
        return Err(EditError::invalid(format!(
            "`{path}` names a whole binding; insert needs a field or an index inside one"
        )));
    };
    let doc = Doc::parse(source)?;
    let (_, root) = doc.binding(&name)?;
    let parent = resolve(root, parent_segs).map_err(|_| nothing_at(path))?;
    let (index, key) = match (last, shape(parent)) {
        (PathSeg::Index(i), Shape::List(items) | Shape::Call(items)) => {
            if *i > items.len() {
                return Err(EditError::invalid(format!(
                    "`{path}`: index {i} is past the end ({} elements)",
                    items.len()
                )));
            }
            (*i, None)
        }
        (PathSeg::Field(key), Shape::Record(fields)) => {
            if field_index(fields, key).is_some() {
                return set_path(source, path, value);
            }
            let anchor = |anchor: &str, after: usize| {
                field_index(fields, anchor).map_or(fields.len(), |i| i + after)
            };
            let index = match placement {
                Placement::End => fields.len(),
                Placement::After(a) => anchor(a, 1),
                Placement::Before(a) => anchor(a, 0),
            };
            (index, Some(key.clone()))
        }
        _ => {
            return Err(EditError::invalid(format!(
                "`{path}`: cannot insert there — an index needs a list (or a call's arguments) and a field needs a record"
            )));
        }
    };
    let op = Op::Insert {
        segs: parent_segs.to_vec(),
        index,
        key,
        what: What::Value(value.clone()),
    };
    apply(source, &name, &[op])
}

/// Append `value` to the list (or call arguments) at `path`.
pub fn append_path(source: &str, path: &str, value: &StaticValue) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    let doc = Doc::parse(source)?;
    let (_, root) = doc.binding(&name)?;
    let target = resolve(root, &segs).map_err(|_| nothing_at(path))?;
    let (Shape::List(items) | Shape::Call(items)) = shape(target) else {
        return Err(EditError::invalid(format!(
            "`{path}` is not a list, so nothing can be appended to it"
        )));
    };
    let op = Op::Insert {
        segs,
        index: items.len(),
        key: None,
        what: What::Value(value.clone()),
    };
    apply(source, &name, &[op])
}

/// Remove the list element (or call argument) or record field `path` names.
///
/// A record field that is not there is already removed: the source comes back
/// unchanged. A list index past the end is [`EditErrorKind::NotFound`]. An
/// element on its own line takes its line (and a comment trailing it on that
/// line) with it; comments on other lines stay.
pub fn remove_path(source: &str, path: &str) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    let Some((last, parent_segs)) = segs.split_last() else {
        return Err(EditError::invalid(format!(
            "`{path}` names a whole binding; remove needs a field or an index inside one"
        )));
    };
    let mut text = source.to_string();
    // A key written twice is removed twice, so the field is really gone.
    loop {
        let doc = Doc::parse(&text)?;
        let (_, root) = doc.binding(&name)?;
        let parent = resolve(root, parent_segs).map_err(|_| nothing_at(path))?;
        let index = match (last, shape(parent)) {
            (PathSeg::Index(i), Shape::List(items) | Shape::Call(items)) => {
                if *i >= items.len() {
                    return Err(nothing_at(path));
                }
                *i
            }
            (PathSeg::Field(key), Shape::Record(fields)) => match field_index(fields, key) {
                Some(i) => i,
                None => return Ok(text),
            },
            _ => return Err(nothing_at(path)),
        };
        let op = Op::Remove {
            segs: parent_segs.to_vec(),
            index,
        };
        text = apply(&text, &name, &[op])?;
        if matches!(last, PathSeg::Index(_)) {
            return Ok(text);
        }
    }
}

fn parse_path(path: &str) -> Result<(String, Vec<PathSeg>), EditError> {
    parse_binding_path(path).ok_or_else(|| {
        EditError::invalid(format!(
            "`{path}` is not a binding path (name, then .field or [index] steps)"
        ))
    })
}

fn nothing_at(path: &str) -> EditError {
    EditError::not_found(format!("`{path}` names no literal value in the source"))
}

// ── Reading the shape of a literal ───────────────────────────────────────

/// What a path step can enter.
enum Shape<'a> {
    Record(&'a [RecordField]),
    List(&'a [Expr]),
    /// The positional arguments of a call to a plain name: a constructor like
    /// `vec3(0.3, -1.0, 0.5)`.
    Call(&'a [Expr]),
    /// A scalar, a color literal (one token, whatever it lowers to), or an
    /// expression that is not a literal at all.
    Leaf,
}

fn shape(expr: &Expr) -> Shape<'_> {
    if color_literal(expr).is_some() {
        return Shape::Leaf;
    }
    match &expr.kind {
        ExprKind::Record(fields) => Shape::Record(fields),
        ExprKind::List(items) => Shape::List(items),
        ExprKind::Call {
            function,
            args,
            arg_names,
        } if matches!(function.kind, ExprKind::Ident(_))
            && arg_names.iter().all(Option::is_none) =>
        {
            Shape::Call(args)
        }
        _ => Shape::Leaf,
    }
}

/// Index of the field `key` — the last one, which is the one a read sees.
fn field_index(fields: &[RecordField], key: &str) -> Option<usize> {
    fields
        .iter()
        .rposition(|f| matches!(f, RecordField::Named(k, _) if k == key))
}

/// The value expression of element `index` of a container.
fn element(expr: &Expr, index: usize) -> Option<&Expr> {
    match shape(expr) {
        Shape::Record(fields) => match fields.get(index)? {
            RecordField::Named(_, value) => Some(value),
            RecordField::Spread(_) => None,
        },
        Shape::List(items) | Shape::Call(items) => items.get(index),
        Shape::Leaf => None,
    }
}

/// Follow `segs` from `root`. `Err(i)` is the index of the first step that
/// names nothing.
fn resolve<'a>(root: &'a Expr, segs: &[PathSeg]) -> Result<&'a Expr, usize> {
    let mut expr = root;
    for (depth, seg) in segs.iter().enumerate() {
        let index = match (seg, shape(expr)) {
            (PathSeg::Field(key), Shape::Record(fields)) => field_index(fields, key),
            (PathSeg::Index(i), Shape::List(_) | Shape::Call(_)) => Some(*i),
            _ => None,
        };
        expr = index.and_then(|i| element(expr, i)).ok_or(depth)?;
    }
    Ok(expr)
}

// ── The edit script ──────────────────────────────────────────────────────

/// New content: a value to render in the surrounding style, or source text
/// lifted from elsewhere in the same container (an element that only moved) —
/// the element's own text, and the comment trailing it on its line, if any.
#[derive(Debug, Clone)]
enum What {
    Value(StaticValue),
    Text(String, String),
}

/// One primitive edit. `segs` is a path below the binding; ops are applied in
/// order, each against the text the previous one produced.
#[derive(Debug, Clone)]
enum Op {
    /// Replace the expression at `segs` with `value`.
    Replace {
        segs: Vec<PathSeg>,
        value: StaticValue,
    },
    /// Insert into the container at `segs` so the new element has `index`.
    Insert {
        segs: Vec<PathSeg>,
        index: usize,
        key: Option<String>,
        what: What,
    },
    /// Remove element `index` of the container at `segs`.
    Remove { segs: Vec<PathSeg>, index: usize },
}

/// One step of lining an old sequence up with a new one.
enum Step {
    Keep(usize, usize),
    Insert(usize),
    Remove(usize),
}

/// Longest common subsequence of `0..n` and `0..m` under `same`.
fn lcs(n: usize, m: usize, same: &dyn Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if same(i, j) {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if same(i, j) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Line `olds` up with `news`: pair what the first test calls the same, then,
/// in the gaps between those pairs, what the next test does, and so on. What
/// no test pairs is inserted (first, so an insertion always has siblings to
/// copy its style from) and removed.
fn align(
    olds: Range<usize>,
    news: Range<usize>,
    tests: &[&dyn Fn(usize, usize) -> bool],
    out: &mut Vec<Step>,
) {
    let Some((test, rest)) = tests.split_first() else {
        out.extend(news.map(Step::Insert));
        out.extend(olds.map(Step::Remove));
        return;
    };
    let (o0, n0) = (olds.start, news.start);
    let pairs = lcs(olds.len(), news.len(), &|i, j| test(o0 + i, n0 + j));
    let (mut o, mut n) = (o0, n0);
    for (i, j) in pairs {
        let (i, j) = (o0 + i, n0 + j);
        align(o..i, n..j, rest, out);
        out.push(Step::Keep(i, j));
        (o, n) = (i + 1, j + 1);
    }
    align(o..olds.end, n..news.end, rest, out);
}

/// How much an old and a new element look like the same *thing* with different
/// contents — worth editing in place rather than replacing, so the comments
/// and spelling inside it survive. 2: they share much of their content (half
/// the fields of a record, the function of a call). 1: they have the same
/// form (a record's keys, a scalar's type). 0: nothing in common.
fn likeness(old: &StaticValue, new: &StaticValue) -> u8 {
    use StaticValue::*;
    match (old, new) {
        (Record(a), Record(b)) => {
            let shared = a.iter().filter(|field| b.contains(field)).count();
            let same_keys =
                a.len() == b.len() && a.iter().all(|(k, _)| b.iter().any(|(k2, _)| k == k2));
            if shared > 0 && shared * 2 >= a.len().min(b.len()) {
                2
            } else {
                same_keys as u8
            }
        }
        (Call { function: f, .. }, Call { function: g, .. }) => (f == g) as u8 * 2,
        (Int(_) | Float(_), Int(_) | Float(_)) => 1,
        _ => (std::mem::discriminant(old) == std::mem::discriminant(new)) as u8,
    }
}

// ── A parsed document ────────────────────────────────────────────────────

/// One element of a bracketed sequence, as char offsets into the source.
#[derive(Debug, Clone)]
struct Item {
    /// The element's significant range: `key: value` for a record field.
    start: usize,
    end: usize,
    /// Offset of the comma after it, if it has one.
    comma: Option<usize>,
    /// For a record field: the text between its key and its value (`: `),
    /// and where the value starts.
    colon: Option<String>,
    value_start: usize,
}

/// A record literal, a list literal or a call's argument list, located in the
/// source: where its brackets and elements are.
#[derive(Debug, Clone)]
struct Seq {
    kind: SyntaxKind,
    /// Just past the opening bracket, and the closing bracket's offset.
    open_end: usize,
    close: usize,
    items: Vec<Item>,
}

/// The formatting of one bracketed sequence — what is captured from an
/// existing literal and reapplied to new text.
#[derive(Debug, Clone)]
struct Style {
    /// Each element on its own line (else all on one).
    multiline: bool,
    /// One-line only: the padding inside the brackets (`{ a }` vs `{a}`).
    pad: String,
    /// One-line only: what separates two elements (`, `).
    sep: String,
    /// Between a record key and its value (`: `).
    colon: String,
    /// Whether the last element is followed by a comma.
    trailing_comma: bool,
    /// Multi-line only: how much deeper the elements are indented.
    step: String,
}

/// What the file as a whole prefers, for when a literal has nothing to copy.
#[derive(Debug, Clone)]
struct FileStyle {
    record_pad: String,
    list_pad: String,
    sep: String,
    step: String,
}

struct Doc {
    chars: Vec<char>,
    root: SyntaxNode,
    stmts: Vec<Stmt>,
    file_style: OnceCell<FileStyle>,
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r'
}

impl Doc {
    fn parse(source: &str) -> Result<Doc, EditError> {
        let (tree, stmts) = parse_ast(source).map_err(|e| {
            EditError::new(EditErrorKind::Parse, format!("source did not parse: {e}"))
        })?;
        Ok(Doc {
            chars: source.chars().collect(),
            root: SyntaxNode::new_root(tree),
            stmts,
            file_style: OnceCell::new(),
        })
    }

    /// The last top-level binding of `name`: its statement index and value.
    fn binding(&self, name: &str) -> Result<(usize, &Expr), EditError> {
        self.stmts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, stmt)| match &stmt.kind {
                StmtKind::Let {
                    name: bound, value, ..
                } if bound == name => Some((i, value)),
                StmtKind::Assign {
                    target: AssignTarget::Name(bound),
                    value,
                } if bound == name => Some((i, value)),
                _ => None,
            })
            .ok_or_else(|| EditError::not_found(format!("no top-level binding for `{name}`")))
    }

    fn text(&self, start: usize, end: usize) -> String {
        self.chars[start.min(end)..end].iter().collect()
    }

    fn expr_text(&self, expr: &Expr) -> String {
        self.text(
            expr.span.start.offset as usize,
            expr.span.end.offset as usize,
        )
    }

    fn line_start(&self, at: usize) -> usize {
        self.chars[..at]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1)
    }

    /// Just past the newline ending the line `at` is on.
    fn line_end_after(&self, at: usize) -> usize {
        self.chars[at..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.chars.len(), |i| at + i + 1)
    }

    /// The indentation of the line `at` is on.
    fn line_indent(&self, at: usize) -> String {
        self.chars[self.line_start(at)..]
            .iter()
            .take_while(|&&c| is_blank(c))
            .collect()
    }

    /// Whether `item` has its line(s) to itself: only indentation before it,
    /// and after it (and its comma) only a comment.
    fn own_line(&self, item: &Item) -> bool {
        let before = &self.chars[self.line_start(item.start)..item.start];
        let mut after = item.comma.map_or(item.end, |c| c + 1);
        while self.chars.get(after).is_some_and(|&c| is_blank(c)) {
            after += 1;
        }
        let rest = &self.chars[after..];
        before.iter().all(|&c| is_blank(c))
            && (rest.is_empty() || rest[0] == '\n' || rest.starts_with(&['/', '/']))
    }

    /// What trails `item` on its line when it has the line to itself: the
    /// spacing and comment after its comma.
    fn tail(&self, item: &Item) -> String {
        if !self.own_line(item) {
            return String::new();
        }
        let after = item.comma.map_or(item.end, |c| c + 1);
        let line: String = self.chars[after..]
            .iter()
            .take_while(|&&c| c != '\n')
            .collect();
        line.trim_end().to_string()
    }

    /// The colon spacing that lines a new field `key` up with its siblings,
    /// when they are lined up: every field on its own line has its value in
    /// the same column, and it took padding to get them there.
    fn aligned_colon(&self, seq: &Seq, near: &Item, key: &str) -> Option<String> {
        let rows: Vec<&Item> = seq
            .items
            .iter()
            .filter(|item| item.colon.is_some() && self.own_line(item))
            .collect();
        let column = |item: &Item| item.value_start - self.line_start(item.start);
        let target = column(near);
        let lined_up = rows.len() >= 2
            && rows.iter().all(|item| column(item) == target)
            && rows.iter().any(|item| item.colon != rows[0].colon);
        let used = near.start - self.line_start(near.start) + key.chars().count() + 1;
        (lined_up && target > used).then(|| format!(":{}", " ".repeat(target - used)))
    }

    // ── Locating sequences ───────────────────────────────────────────────

    /// The bracketed sequence `expr` is written as, if it is one.
    fn seq(&self, expr: &Expr) -> Option<Seq> {
        let count = match shape(expr) {
            Shape::Record(fields) => fields.len(),
            Shape::List(items) | Shape::Call(items) => items.len(),
            Shape::Leaf => return None,
        };
        let node = find_node(&self.root, (expr.span.start.offset, expr.span.end.offset))?;
        let seq = seq_of_node(&node, &self.chars)?;
        (seq.items.len() == count).then_some(seq)
    }

    /// The style `seq` is written in; `None` when it is empty and so shows
    /// none.
    fn style_of(&self, seq: &Seq) -> Option<Style> {
        let first = seq.items.first()?;
        let lead = self.text(seq.open_end, first.start);
        let multiline = lead.contains('\n');
        let sep = seq.items.windows(2).find_map(|pair| {
            let gap = self.text(pair[0].end, pair[1].start);
            let spacing = gap.strip_prefix(',')?;
            spacing.chars().all(is_blank).then_some(gap)
        });
        let step = multiline
            .then(|| {
                let outer = self.line_indent(seq.open_end - 1);
                let inner = self.line_indent(first.start);
                inner.strip_prefix(outer.as_str()).map(str::to_string)
            })
            .flatten()
            .filter(|step| !step.is_empty());
        Some(Style {
            multiline,
            pad: if !multiline && lead.chars().all(is_blank) {
                lead
            } else {
                String::new()
            },
            sep: sep.unwrap_or_else(|| self.file_style().sep.clone()),
            // The tightest spacing in use: wider ones are column padding.
            colon: seq
                .items
                .iter()
                .filter_map(|item| item.colon.clone())
                .min_by_key(String::len)
                .unwrap_or_else(|| ": ".into()),
            trailing_comma: seq.items.last().is_some_and(|item| item.comma.is_some()),
            step: step.unwrap_or_else(|| self.file_style().step.clone()),
        })
    }

    /// The style for a value with no literal to copy: the layout
    /// [`StaticValue::to_source`] uses, in the file's dominant spacing.
    fn default_style(&self, value: &StaticValue) -> Style {
        let file = self.file_style();
        let multiline = matches!(value, StaticValue::List(items) if items.iter().any(StaticValue::is_composite));
        Style {
            multiline,
            pad: match value {
                StaticValue::Record(_) => file.record_pad.clone(),
                StaticValue::List(_) => file.list_pad.clone(),
                _ => String::new(),
            },
            sep: file.sep.clone(),
            colon: ": ".into(),
            trailing_comma: multiline,
            step: file.step.clone(),
        }
    }

    fn file_style(&self) -> &FileStyle {
        self.file_style.get_or_init(|| {
            // Votes: [record padded, record tight, list padded, list tight],
            // and for `, ` against `,` between elements on one line.
            let mut votes = [0usize; 4];
            let mut seps = [0usize; 2];
            let mut step = None;
            let mut stack = vec![self.root.clone()];
            while let Some(node) = stack.pop() {
                if matches!(node.kind(), SyntaxKind::RecordExpr | SyntaxKind::ListExpr)
                    && let Some(seq) = seq_of_node(&node, &self.chars)
                    && let Some(style) = (!seq.items.is_empty())
                        .then(|| self.style_of_unfiled(&seq))
                        .flatten()
                {
                    for pair in seq.items.windows(2) {
                        match self.text(pair[0].end, pair[1].start).as_str() {
                            ", " => seps[0] += 1,
                            "," => seps[1] += 1,
                            _ => {}
                        }
                    }
                    if style.0 {
                        step = step.or(style.2);
                    } else {
                        let list = (node.kind() == SyntaxKind::ListExpr) as usize * 2;
                        votes[list + style.1.is_empty() as usize] += 1;
                    }
                }
                stack.extend(node.children().into_iter().rev().filter_map(|el| match el {
                    SyntaxElement::Node(n) => Some(n),
                    SyntaxElement::Token(_) => None,
                }));
            }
            // With no multi-line literal to measure, the shallowest indented
            // line in the file is one step.
            let shallowest = || {
                self.chars
                    .split(|&c| c == '\n')
                    .filter_map(|line| {
                        let indent: String = line.iter().take_while(|&&c| is_blank(c)).collect();
                        (!indent.is_empty() && indent.len() < line.len()).then_some(indent)
                    })
                    .min_by_key(String::len)
            };
            FileStyle {
                // A tie keeps what `to_source` writes: `{ a: 1 }` and `[1, 2]`.
                record_pad: if votes[1] > votes[0] { "" } else { " " }.into(),
                list_pad: if votes[2] > votes[3] { " " } else { "" }.into(),
                sep: if seps[1] > seps[0] { "," } else { ", " }.into(),
                step: step.or_else(shallowest).unwrap_or_else(|| "  ".into()),
            }
        })
    }

    /// `(multiline, pad, step)` of a non-empty sequence, without consulting
    /// the file style (which is being computed).
    fn style_of_unfiled(&self, seq: &Seq) -> Option<(bool, String, Option<String>)> {
        let first = seq.items.first()?;
        let lead = self.text(seq.open_end, first.start);
        if !lead.chars().all(|c| is_blank(c) || c == '\n') {
            return None;
        }
        let multiline = lead.contains('\n');
        let step = multiline
            .then(|| {
                let outer = self.line_indent(seq.open_end - 1);
                let inner = self.line_indent(first.start);
                inner.strip_prefix(outer.as_str()).map(str::to_string)
            })
            .flatten()
            .filter(|step| !step.is_empty());
        Some((multiline, lead, step))
    }

    // ── Rendering in a captured style ────────────────────────────────────

    /// Render `value` starting on a line indented by `indent`, formatted like
    /// `model` — a literal of the same kind standing where this value goes, or
    /// next to it — wherever the two have the same shape, and in the file's
    /// style elsewhere.
    fn render(&self, value: &StaticValue, model: Option<&Expr>, indent: &str) -> String {
        // The model's style, and its elements as models for this value's.
        let like = |wanted: fn(&Shape) -> bool| {
            let model = model.filter(|m| wanted(&shape(m)))?;
            Some((model, self.seq(model).and_then(|seq| self.style_of(&seq))))
        };
        let style = |found: Option<Option<Style>>| {
            found.flatten().unwrap_or_else(|| self.default_style(value))
        };
        match value {
            StaticValue::Record(fields) => {
                let like = like(|s| matches!(s, Shape::Record(_)));
                let style = style(like.as_ref().map(|(_, s)| s.clone()));
                let parts: Vec<_> = fields
                    .iter()
                    .map(|(key, field)| {
                        let model = like.as_ref().and_then(|(m, _)| {
                            let Shape::Record(model_fields) = shape(m) else {
                                return None;
                            };
                            element(m, field_index(model_fields, key)?)
                        });
                        (format!("{key}{}", style.colon), field, model)
                    })
                    .collect();
                self.render_seq('{', '}', &parts, &style, indent)
            }
            StaticValue::List(items) => {
                let like = like(|s| matches!(s, Shape::List(_)));
                let style = style(like.as_ref().map(|(_, s)| s.clone()));
                self.render_seq('[', ']', &self.item_parts(items, like), &style, indent)
            }
            StaticValue::Call { function, args } => {
                let like = like(|s| matches!(s, Shape::Call(_)));
                let style = style(like.as_ref().map(|(_, s)| s.clone()));
                let body = self.render_seq('(', ')', &self.item_parts(args, like), &style, indent);
                format!("{function}{body}")
            }
            StaticValue::Color { .. } => {
                let model = model
                    .filter(|m| color_literal(m).is_some())
                    .map(|m| self.expr_text(m));
                color_like(value, model.as_deref())
            }
            scalar => scalar.to_source(),
        }
    }

    /// The elements of a list (or call) paired with their models: the model's
    /// element at the same position, or its last one — siblings look alike.
    fn item_parts<'a>(
        &self,
        items: &'a [StaticValue],
        like: Option<(&'a Expr, Option<Style>)>,
    ) -> Vec<(String, &'a StaticValue, Option<&'a Expr>)> {
        let models = match like.map(|(m, _)| shape(m)) {
            Some(Shape::List(models) | Shape::Call(models)) => models,
            _ => &[],
        };
        items
            .iter()
            .enumerate()
            .map(|(i, item)| (String::new(), item, models.get(i).or(models.last())))
            .collect()
    }

    fn render_seq(
        &self,
        open: char,
        close: char,
        parts: &[(String, &StaticValue, Option<&Expr>)],
        style: &Style,
        indent: &str,
    ) -> String {
        if parts.is_empty() {
            return format!("{open}{close}");
        }
        let mut out = String::from(open);
        if style.multiline {
            let inner = format!("{indent}{}", style.step);
            for (i, (prefix, value, model)) in parts.iter().enumerate() {
                out.push('\n');
                out.push_str(&inner);
                out.push_str(prefix);
                out.push_str(&self.render(value, *model, &inner));
                if i + 1 < parts.len() || style.trailing_comma {
                    out.push(',');
                }
            }
            out.push('\n');
            out.push_str(indent);
        } else {
            out.push_str(&style.pad);
            for (i, (prefix, value, model)) in parts.iter().enumerate() {
                if i > 0 {
                    out.push_str(&style.sep);
                }
                out.push_str(prefix);
                out.push_str(&self.render(value, *model, indent));
            }
            if style.trailing_comma {
                out.push(',');
            }
            out.push_str(&style.pad);
        }
        out.push(close);
        out
    }

    /// The text that replaces `old` with `value`: `value` rendered like `old`,
    /// and a number keeping `old`'s spelling habits.
    fn replacement(&self, old: &Expr, value: &StaticValue) -> String {
        match value {
            StaticValue::Float(f) => float_like(*f, &self.expr_text(old)),
            _ => self.render(
                value,
                Some(old),
                &self.line_indent(old.span.start.offset as usize),
            ),
        }
    }

    // ── Planning ─────────────────────────────────────────────────────────

    /// Plan the ops that make `old` (at `segs`) read as `new`, touching only
    /// what differs.
    fn plan(
        &self,
        bindings: &[StaticBinding],
        old: &Expr,
        segs: &mut Vec<PathSeg>,
        new: &StaticValue,
        ops: &mut Vec<Op>,
    ) {
        if eval(old, bindings).as_ref() == Ok(new) {
            return;
        }
        let whole = |ops: &mut Vec<Op>| {
            ops.push(Op::Replace {
                segs: segs.clone(),
                value: new.clone(),
            })
        };
        // A container the tree cannot locate (it should not happen) is not
        // edited inside.
        if !matches!(shape(old), Shape::Leaf) && self.seq(old).is_none() {
            return whole(ops);
        }
        let mut steps = Vec::new();
        // The old elements' texts, for an element that only moved.
        let texts: Vec<(String, String)> = match self.seq(old) {
            Some(seq) => seq
                .items
                .iter()
                .map(|item| (self.text(item.start, item.end), self.tail(item)))
                .collect(),
            None => Vec::new(),
        };
        let old_values: Vec<Option<StaticValue>>;
        let new_values: Vec<&StaticValue>;
        let mut keys: Option<Vec<&str>> = None;
        match (shape(old), new) {
            (Shape::Record(old_fields), StaticValue::Record(new_fields))
                if unique_named(old_fields) && unique_keys(new_fields) =>
            {
                let old_keys: Vec<&str> = old_fields
                    .iter()
                    .map(|f| match f {
                        RecordField::Named(key, _) => key.as_str(),
                        RecordField::Spread(_) => "",
                    })
                    .collect();
                align(
                    0..old_fields.len(),
                    0..new_fields.len(),
                    &[&|i, j| old_keys[i] == new_fields[j].0],
                    &mut steps,
                );
                old_values = (0..old_fields.len())
                    .map(|i| element(old, i).and_then(|e| eval(e, bindings).ok()))
                    .collect();
                new_values = new_fields.iter().map(|(_, v)| v).collect();
                keys = Some(new_fields.iter().map(|(k, _)| k.as_str()).collect());
            }
            (Shape::List(old_items), StaticValue::List(new_items)) => {
                old_values = old_items.iter().map(|e| eval(e, bindings).ok()).collect();
                new_values = new_items.iter().collect();
            }
            (Shape::Call(old_args), StaticValue::Call { function, args })
                if matches!(&old.kind, ExprKind::Call { function: callee, .. }
                    if matches!(&callee.kind, ExprKind::Ident(name) if name == function)) =>
            {
                old_values = old_args.iter().map(|e| eval(e, bindings).ok()).collect();
                new_values = args.iter().collect();
            }
            _ => return whole(ops),
        }
        let like = |i: usize, j: usize| {
            old_values[i]
                .as_ref()
                .map_or(0, |old| likeness(old, new_values[j]))
        };
        if keys.is_none() {
            align(
                0..old_values.len(),
                0..new_values.len(),
                &[
                    &|i, j| old_values[i].as_ref() == Some(new_values[j]),
                    &|i, j| like(i, j) >= 2,
                    &|i, j| like(i, j) >= 1,
                ],
                &mut steps,
            );
        }
        // An element that leaves one place and arrives in another, unchanged,
        // keeps its text.
        let mut moved: Vec<usize> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Remove(i) => Some(*i),
                _ => None,
            })
            .collect();
        let mut pos = 0;
        for step in steps {
            match step {
                Step::Keep(i, j) => {
                    let Some(child) = element(old, i) else {
                        continue;
                    };
                    segs.push(match &keys {
                        Some(keys) => PathSeg::Field(keys[j].to_string()),
                        None => PathSeg::Index(pos),
                    });
                    self.plan(bindings, child, segs, new_values[j], ops);
                    segs.pop();
                    pos += 1;
                }
                Step::Insert(j) => {
                    let source = moved.iter().position(|&i| {
                        old_values[i].as_ref() == Some(new_values[j])
                            && keys.as_ref().is_none_or(|keys| {
                                matches!(shape(old), Shape::Record(fields)
                                    if matches!(&fields[i], RecordField::Named(k, _) if k == keys[j]))
                            })
                    });
                    let text = source
                        .map(|at| moved.swap_remove(at))
                        .and_then(|i| texts.get(i).cloned());
                    ops.push(Op::Insert {
                        segs: segs.clone(),
                        index: pos,
                        key: keys.as_ref().map(|keys| keys[j].to_string()),
                        what: match text {
                            Some((text, tail)) => What::Text(text, tail),
                            None => What::Value(new_values[j].clone()),
                        },
                    });
                    pos += 1;
                }
                Step::Remove(_) => ops.push(Op::Remove {
                    segs: segs.clone(),
                    index: pos,
                }),
            }
        }
    }

    // ── Splices for one op ───────────────────────────────────────────────

    fn container(&self, expr: &Expr) -> Result<Seq, EditError> {
        self.seq(expr).ok_or_else(|| {
            EditError::invalid("the value there is not written as a record, list or call literal")
        })
    }

    fn insert(
        &self,
        container: &Expr,
        index: usize,
        key: Option<&str>,
        what: &What,
    ) -> Result<Vec<Splice>, EditError> {
        let seq = self.container(container)?;
        let count = seq.items.len();
        if index > count {
            return Err(EditError::invalid("insert index is past the end"));
        }
        let style = self.style_of(&seq);
        let outer = self.line_indent(container.span.start.offset as usize);
        // The sibling the new element is laid out like: the one before it, or
        // the one after when it goes first.
        let sibling = if index > 0 { index - 1 } else { 0 };
        let model = element(container, sibling);
        let colon = match (key, seq.items.get(sibling)) {
            (Some(key), Some(near)) if self.own_line(near) => self.aligned_colon(&seq, near, key),
            _ => None,
        };
        let colon = colon
            .or_else(|| style.as_ref().map(|s| s.colon.clone()))
            .unwrap_or_else(|| ": ".into());
        let elem = |indent: &str| match what {
            What::Text(text, _) => text.clone(),
            What::Value(value) => {
                let body = self.render(value, model, indent);
                match key {
                    Some(key) => format!("{key}{colon}{body}"),
                    None => body,
                }
            }
        };
        let tail = match what {
            What::Text(_, tail) => tail.as_str(),
            What::Value(_) => "",
        };
        let at = |at: usize, text: String| Splice {
            start: at,
            end: at,
            text,
        };

        let Some(near) = seq.items.get(sibling) else {
            // An empty container: its own shape decides, then the file's.
            let gap = self.text(seq.open_end, seq.close);
            let inner = format!("{outer}{}", self.file_style().step);
            let composite = matches!(what, What::Value(v) if v.is_composite());
            return Ok(vec![if gap.contains('\n') {
                let line = self.line_end_after(seq.open_end);
                at(line, format!("{inner}{},{tail}\n", elem(&inner)))
            } else if seq.kind == SyntaxKind::ListExpr && composite && gap.trim().is_empty() {
                // What `to_source` does with a list of composites: one per line.
                Splice {
                    start: seq.open_end,
                    end: seq.close,
                    text: format!("\n{inner}{},\n{outer}", elem(&inner)),
                }
            } else {
                let file = self.file_style();
                let pad = match seq.kind {
                    _ if !gap.is_empty() && gap.chars().all(is_blank) => gap.as_str(),
                    SyntaxKind::RecordExpr => file.record_pad.as_str(),
                    SyntaxKind::ListExpr => file.list_pad.as_str(),
                    _ => "",
                };
                Splice {
                    start: seq.open_end,
                    end: seq.close,
                    text: format!("{pad}{}{pad}", elem(&outer)),
                }
            }]);
        };

        if self.own_line(near) {
            let indent = self.line_indent(near.start);
            if index == 0 {
                let line = self.line_end_after(seq.open_end);
                return Ok(vec![at(
                    line,
                    format!("{indent}{},{tail}\n", elem(&indent)),
                )]);
            }
            // Appending after an element with no comma: it gains one, and the
            // new last element goes without, as the old one did.
            let comma = if index < count || near.comma.is_some() {
                ","
            } else {
                ""
            };
            let line = self.line_end_after(near.comma.map_or(near.end, |c| c + 1));
            let mut splices = Vec::new();
            if near.comma.is_none() {
                splices.push(at(near.end, ",".into()));
            }
            let mut text = format!("{indent}{}{comma}{tail}\n", elem(&indent));
            if self.chars.get(line - 1) != Some(&'\n') {
                text.insert(0, '\n');
            }
            splices.push(at(line, text));
            return Ok(splices);
        }
        let sep = style.as_ref().map_or(", ", |s| s.sep.as_str());
        Ok(vec![if index > 0 {
            at(near.end, format!("{sep}{}", elem(&outer)))
        } else {
            at(near.start, format!("{}{sep}", elem(&outer)))
        }])
    }

    fn remove(&self, container: &Expr, index: usize) -> Result<Vec<Splice>, EditError> {
        let seq = self.container(container)?;
        let item = seq
            .items
            .get(index)
            .ok_or_else(|| EditError::not_found("no element at that index"))?;
        let prev = index.checked_sub(1).map(|i| &seq.items[i]);
        let next = seq.items.get(index + 1);
        let cut = |start: usize, end: usize| Splice {
            start,
            end,
            text: String::new(),
        };
        let same_line = |a: usize, b: usize| !self.chars[a..b].contains(&'\n');

        if self.own_line(item) {
            let mut cuts = vec![cut(
                self.line_start(item.start),
                self.line_end_after(item.comma.map_or(item.end, |c| c + 1)),
            )];
            // The list had no trailing comma; it still has none.
            if next.is_none()
                && item.comma.is_none()
                && let Some(comma) = prev.and_then(|p| p.comma)
            {
                cuts.insert(0, cut(comma, comma + 1));
            }
            return Ok(cuts);
        }
        if let Some(next) = next.filter(|next| same_line(item.end, next.start)) {
            return Ok(vec![cut(item.start, next.start)]);
        }
        if let Some(prev) = prev.filter(|prev| same_line(prev.end, item.start)) {
            return Ok(vec![cut(prev.end, item.end)]);
        }
        // Alone between its brackets on one line: leave them, and any padding
        // (`{ a: 1 }` becomes `{ }`), so an insert finds the style again.
        let mut end = item.comma.map_or(item.end, |c| c + 1);
        while self.chars.get(end).is_some_and(|&c| is_blank(c)) {
            end += 1;
        }
        Ok(vec![cut(item.start, end)])
    }
}

fn unique_keys(fields: &[(String, StaticValue)]) -> bool {
    fields
        .iter()
        .enumerate()
        .all(|(i, (key, _))| fields[..i].iter().all(|(other, _)| other != key))
}

/// Every field is `key: value` and no key repeats — the records a field path
/// names unambiguously.
fn unique_named(fields: &[RecordField]) -> bool {
    fields.iter().enumerate().all(|(i, field)| match field {
        RecordField::Named(key, _) => fields[..i]
            .iter()
            .all(|other| !matches!(other, RecordField::Named(k, _) if k == key)),
        RecordField::Spread(_) => false,
    })
}

/// Locate the brackets, elements and commas of a record, list or call node.
fn seq_of_node(node: &SyntaxNode, chars: &[char]) -> Option<Seq> {
    let node = match node.kind() {
        SyntaxKind::RecordExpr | SyntaxKind::ListExpr | SyntaxKind::ArgList => node.clone(),
        SyntaxKind::CallExpr => node.children().into_iter().find_map(|el| match el {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::ArgList => Some(n),
            _ => None,
        })?,
        _ => return None,
    };
    let (mut open_end, mut close, mut items) = (None, None, Vec::<Item>::new());
    for el in node.children() {
        match el {
            SyntaxElement::Token(t) if t.is_trivia() => {}
            SyntaxElement::Token(t) => {
                let at = t.offset() as usize;
                match t.token() {
                    _ if open_end.is_none() => open_end = Some(at + t.text_len() as usize),
                    Some(Token::Comma) => items.last_mut()?.comma = Some(at),
                    Some(Token::RBrace | Token::RBracket | Token::RParen) => close = Some(at),
                    _ => {}
                }
            }
            SyntaxElement::Node(n) => {
                let (start, end) = significant_range(&n)?;
                // A record field is `key`, `:`, value: the text from the end of
                // the key to the start of the value is how its colon is spaced.
                let colon = (n.kind() == SyntaxKind::RecordField)
                    .then(|| {
                        let key = edge_significant_token(&n, false)?;
                        let value = n.children().into_iter().rev().find_map(|el| match el {
                            SyntaxElement::Node(v) => significant_range(&v),
                            SyntaxElement::Token(_) => None,
                        })?;
                        let text: String = chars
                            [(key.offset() + key.text_len()) as usize..value.0 as usize]
                            .iter()
                            .collect();
                        (text.trim() == ":" && text.chars().all(|c| c == ':' || c == ' '))
                            .then_some((key.text_len() as usize, text))
                    })
                    .flatten();
                items.push(Item {
                    start: start as usize,
                    end: end as usize,
                    comma: None,
                    value_start: start as usize
                        + colon.as_ref().map_or(0, |(key, c)| key + c.len()),
                    colon: colon.map(|(_, c)| c),
                });
            }
        }
    }
    Some(Seq {
        kind: node.kind(),
        open_end: open_end?,
        close: close?,
        items,
    })
}

/// `f` spelled the way `model` spells its number: a literal padded with
/// trailing zeros (`3.50`, `0.020000`) keeps that many decimals when the new
/// value fits in them exactly. Otherwise the shortest text that reads back as
/// `f`, always with a decimal point.
fn float_like(f: f64, model: &str) -> String {
    let shortest = format!("{f:?}");
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let model = model.trim_start_matches('-').trim();
    let Some((whole, decimals)) = model.split_once('.') else {
        return shortest;
    };
    let Ok(old) = model.parse::<f64>() else {
        return shortest;
    };
    let natural = format!("{old:?}");
    let padded = natural
        .split_once('.')
        .is_some_and(|(_, d)| digits(d) && d.len() < decimals.len());
    if !digits(whole) || !digits(decimals) || !padded {
        return shortest;
    }
    let fixed = format!("{f:.*}", decimals.len());
    if fixed.parse::<f64>() == Ok(f) {
        fixed
    } else {
        shortest
    }
}

/// A color spelled the way `model` (another color literal) is: uppercase if it
/// is, and in the short `#rgb` form if it is and the value fits.
fn color_like(color: &StaticValue, model: Option<&str>) -> String {
    let long = color.to_source();
    let (Some(model), StaticValue::Color { r, g, b, a }) = (model, color) else {
        return long;
    };
    let model = model.trim_start_matches('#');
    let mut hex = long[1..].to_string();
    let fits = [Some(*r), Some(*g), Some(*b), *a]
        .iter()
        .flatten()
        .all(|c| c >> 4 == c & 15);
    if model.len() <= 4 && fits {
        hex = hex.chars().step_by(2).collect();
    }
    let upper = model.chars().any(|c| c.is_ascii_uppercase())
        && !model.chars().any(|c| c.is_ascii_lowercase());
    if upper {
        hex = hex.to_ascii_uppercase();
    }
    format!("#{hex}")
}

/// Apply `ops` in order to the binding `name` of `source`. Each op is resolved
/// against the text the one before it left, so its indices mean what the plan
/// meant. The result is only returned if it still parses.
fn apply(source: &str, name: &str, ops: &[Op]) -> Result<String, EditError> {
    if ops.is_empty() {
        return Ok(source.to_string());
    }
    let mut text = source.to_string();
    let mut next = 0;
    while next < ops.len() {
        let doc = Doc::parse(&text)?;
        let (_, root) = doc.binding(name)?;
        let at = |segs: &[PathSeg]| {
            resolve(root, segs).map_err(|_| EditError::not_found("the edit lost its place"))
        };
        let mut splices = Vec::new();
        match &ops[next] {
            // Replacements do not move one another's paths: do a run at once.
            Op::Replace { .. } => {
                while let Some(Op::Replace { segs, value }) = ops.get(next) {
                    let old = at(segs)?;
                    splices.push(Splice {
                        start: old.span.start.offset as usize,
                        end: old.span.end.offset as usize,
                        text: doc.replacement(old, value),
                    });
                    next += 1;
                }
            }
            Op::Insert {
                segs,
                index,
                key,
                what,
            } => {
                splices = doc.insert(at(segs)?, *index, key.as_deref(), what)?;
                next += 1;
            }
            Op::Remove { segs, index } => {
                splices = doc.remove(at(segs)?, *index)?;
                next += 1;
            }
        }
        splices.sort_by_key(|s| s.start);
        text = apply_splices(&doc.chars, &splices);
    }
    match parse_ast(&text) {
        Ok(_) => Ok(text),
        Err(e) => Err(EditError::invalid(format!(
            "the edit would leave source that does not parse: {e}"
        ))),
    }
}
