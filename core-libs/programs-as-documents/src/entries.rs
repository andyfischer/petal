//! The lists and child lists written in a document's source, and rewriting
//! their entries in place.
//!
//! A structural edit (reorder, insert, remove, move) lands on a
//! [`Container`]: a list literal, whose entries are its items (an element, a
//! helper call, any expression), or an element literal, whose entries are its
//! child elements. [`rewrite_entries`] gives a container a new sequence of
//! entries, each kept from the old ones or new text, through Petal's
//! style-preserving path edits ([`petal::literal_edit`]) on the container
//! lifted out of the file: a kept entry keeps its text as written (its
//! comments and layout, a helper call stays a call), a removed one takes its
//! line with it, and a new one is laid out like its siblings.

use petal::ast::{Expr, ExprKind, ExprVisitor, JsxChild, Stmt, walk_expr};
use petal::goal_based_editing::Placement;
use petal::literal_edit::{insert_path, move_path, remove_path};
use petal::rewrite::parse_ast;
use petal::source_map::SourceSpan;
use petal::static_value::StaticValue;

use crate::splice::{byte_range, dedent, lifted, line_indent, reindent};

/// What a container's entries are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// The items of a list literal.
    List,
    /// The child elements of an element literal, and its `{…}` holes that
    /// are not text (`{badge("x", 1)}`, `{rows.map(card)}`); not text,
    /// `{"strings"}` or `<br />`.
    Children,
}

/// A list or element literal written in the source, and its entries.
#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub kind: ContainerKind,
    /// The whole literal (character offsets, like every petal span).
    pub span: SourceSpan,
    /// Its entries, in order.
    pub entries: Vec<SourceSpan>,
    /// The element's tag, for [`ContainerKind::Children`].
    pub tag: Option<String>,
    /// Per entry: whether it is a `{…}` hole among an element's children
    /// (its span is then the expression inside the braces). Always false in
    /// a list.
    pub holes: Vec<bool>,
}

impl Container {
    /// "the list on line 3", "the `<group>` on line 5".
    pub fn describe(&self) -> String {
        match &self.tag {
            Some(t) => format!("the <{t}> on line {}", self.span.start.line),
            None => format!("the list on line {}", self.span.start.line),
        }
    }
}

/// Every list and element literal of `source`, in source order (an outer
/// literal before the ones inside it).
pub fn containers(source: &str) -> Result<Vec<Container>, String> {
    let (_, stmts) = parse_ast(source)?;
    Ok(containers_of(&stmts))
}

pub(crate) fn containers_of(stmts: &[Stmt]) -> Vec<Container> {
    struct Collect(Vec<Container>);
    impl ExprVisitor for Collect {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::List(items) => self.0.push(Container {
                    kind: ContainerKind::List,
                    span: e.span,
                    entries: items.iter().map(|i| i.span).collect(),
                    tag: None,
                    holes: vec![false; items.len()],
                }),
                ExprKind::Element { tag, children, .. } => {
                    let (entries, holes) = children
                        .iter()
                        .filter_map(|c| match c {
                            JsxChild::Expr(c) => match &c.kind {
                                ExprKind::Element { tag, .. } if tag == "br" => None,
                                ExprKind::Element { .. } => Some((c.span, false)),
                                // Text written as a hole.
                                ExprKind::Literal(_) | ExprKind::StringInterp { .. } => None,
                                _ => Some((c.span, true)),
                            },
                            JsxChild::Text(_) => None,
                        })
                        .unzip();
                    self.0.push(Container {
                        kind: ContainerKind::Children,
                        span: e.span,
                        entries,
                        tag: Some(tag.clone()),
                        holes,
                    })
                }
                _ => {}
            }
            walk_expr(self, e);
        }
    }
    let mut c = Collect(Vec::new());
    for s in stmts {
        c.visit_stmt(s);
    }
    c.0
}

fn contains(outer: &SourceSpan, inner: &SourceSpan) -> bool {
    outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
}

/// The innermost entry (container index, entry index) whose text contains
/// `span`, skipping entries that contain `exclude` (an entry around the
/// parent itself is not an entry *of* the parent).
pub fn entry_containing(
    containers: &[Container],
    span: &SourceSpan,
    exclude: Option<&SourceSpan>,
) -> Option<(usize, usize)> {
    let mut best: Option<((usize, usize), u32)> = None;
    for (ci, c) in containers.iter().enumerate() {
        if !contains(&c.span, span) {
            continue;
        }
        for (ei, e) in c.entries.iter().enumerate() {
            if !contains(e, span) || exclude.is_some_and(|x| contains(e, x)) {
                continue;
            }
            let len = e.end.offset - e.start.offset;
            if best.is_none_or(|(_, l)| len < l) {
                best = Some(((ci, ei), len));
            }
        }
    }
    best.map(|(hit, _)| hit)
}

/// The container whose literal starts where `span` does (the literal a
/// trace ended at), or else the smallest list containing it.
pub fn container_at(containers: &[Container], span: &SourceSpan, kind: ContainerKind) -> Option<usize> {
    containers
        .iter()
        .position(|c| c.kind == kind && c.span.start.offset == span.start.offset)
        .or_else(|| {
            containers
                .iter()
                .enumerate()
                .filter(|(_, c)| c.kind == kind && contains(&c.span, span))
                .min_by_key(|(_, c)| c.span.end.offset - c.span.start.offset)
                .map(|(i, _)| i)
        })
}

/// The text of `span` in `source`, with its later lines made relative to
/// its first ([`dedent`]), ready to be placed at another indentation.
pub fn entry_text(source: &str, span: &SourceSpan) -> Option<String> {
    let r = byte_range(source, span)?;
    Some(dedent(&source[r.clone()], &line_indent(source, r.start)))
}

/// One entry of a container's new sequence.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    /// The old entry at this index, its text as written.
    Keep(usize),
    /// New text (lines after the first relative to the first: see
    /// [`dedent`]).
    New(String),
    /// A new value, laid out like its siblings: a multi-line element
    /// after one written an attribute per line is written the same way (as
    /// Petal's own `insert_path` lays it out). Falls back to the value's
    /// one-line source where that cannot be done.
    Value(StaticValue),
}

impl Slot {
    /// Whether this is a new entry (text or value).
    pub fn is_new(&self) -> bool {
        !matches!(self, Slot::Keep(_))
    }
}

const PLACEHOLDER: &str = "__programs_as_documents_new_entry";
const PLACEHOLDER_TAG: &str = "programs-as-documents-new-entry";

/// Give the container of `kind` whose literal starts at character offset
/// `start` the entries `target`. Each old entry not kept is removed, the
/// kept ones are moved into the new order, and the new ones inserted, all
/// as path edits on the lifted literal, so the rest of the file is
/// untouched.
pub fn rewrite_entries(source: &str, start: u32, kind: ContainerKind, target: &[Slot]) -> Result<String, String> {
    let all = containers(source)?;
    let c = all
        .iter()
        .find(|c| c.kind == kind && c.span.start.offset == start)
        .ok_or("the list to edit is no longer where it was")?;
    let n = c.entries.len();
    let kept: Vec<usize> = target
        .iter()
        .filter_map(|s| match s {
            Slot::Keep(i) => Some(*i),
            Slot::New(_) | Slot::Value(_) => None,
        })
        .collect();
    if let Some(bad) = kept.iter().find(|i| **i >= n) {
        return Err(format!("{} has no entry {bad}", c.describe()));
    }
    let unchanged = target.len() == n && kept.len() == n && kept.iter().enumerate().all(|(i, k)| i == *k);
    if unchanged {
        return Ok(source.to_string());
    }
    if c.holes.iter().any(|h| *h) {
        // The path edits address child elements: each hole stands in as a
        // placeholder element while they run, and is put back as written.
        let (masked, holes) = mask_holes(source, c)?;
        let out = rewrite_entries(&masked, start, kind, target)?;
        return Ok(holes.iter().fold(out, |out, (ph, text)| out.replacen(ph.as_str(), text, 1)));
    }
    let span = c.span;
    lifted(source, &span, |scratch, root| {
        let at = |i: usize| match kind {
            ContainerKind::List => format!("{root}[{i}]"),
            ContainerKind::Children => format!("{root}/*[{i}]"),
        };
        let mut s = scratch.to_string();
        for i in (0..n).rev() {
            if !kept.contains(&i) {
                s = remove_path(&s, &at(i)).map_err(|e| e.message)?;
            }
        }
        // Move only the entries off the longest run already in order, each
        // to just after the entry before it, so the fewest texts move (a
        // comment above an entry that stays put stays put).
        let mut cur: Vec<usize> = (0..n).filter(|i| kept.contains(i)).collect();
        let stay = longest_increasing(&kept);
        for (p, want) in kept.iter().enumerate() {
            if stay.contains(want) {
                continue;
            }
            let q = cur.iter().position(|x| x == want).expect("kept entries are in cur");
            let v = cur.remove(q);
            let to = if p == 0 { 0 } else { cur.iter().position(|x| *x == kept[p - 1]).expect("placed") + 1 };
            cur.insert(to, v);
            s = move_path(&s, &at(q), &at(to)).map_err(|e| e.message)?;
        }
        debug_assert_eq!(cur, kept);
        for (p, slot) in target.iter().enumerate() {
            match slot {
                Slot::New(text) => s = insert_text(&s, kind, &at(p), text)?,
                Slot::Value(v) => s = insert_value(&s, kind, &at(p), v)?,
                Slot::Keep(_) => {}
            }
        }
        Ok(s)
    })
}

const HOLE_TAG: &str = "programs-as-documents-hole";

/// `source` with each `{…}` hole entry of `c` written as a placeholder
/// element, and each placeholder's text paired with the hole's own text
/// (braces included).
fn mask_holes(source: &str, c: &Container) -> Result<(String, Vec<(String, String)>), String> {
    let mut out = source.to_string();
    let mut holes = Vec::new();
    for (i, span) in c.entries.iter().enumerate().rev() {
        if !c.holes[i] {
            continue;
        }
        let r = byte_range(source, span).ok_or("a hole's position is outside the source")?;
        let open = source[..r.start].trim_end();
        let close = source[r.end..].trim_start();
        if !open.ends_with('{') || !close.starts_with('}') {
            return Err(format!("the `{{…}}` on line {} could not be located", span.start.line));
        }
        let (a, b) = (open.len() - 1, source.len() - close.len() + 1);
        let ph = format!("<{HOLE_TAG}-{i} />");
        if source.contains(&ph) {
            return Err("the document already uses the placeholder tag".into());
        }
        holes.push((ph.clone(), source[a..b].to_string()));
        out.replace_range(a..b, &ph);
    }
    Ok((out, holes))
}

/// The values of a longest strictly increasing subsequence of `xs`.
fn longest_increasing(xs: &[usize]) -> Vec<usize> {
    let n = xs.len();
    let mut len = vec![1usize; n];
    let mut prev = vec![usize::MAX; n];
    for i in 0..n {
        for j in 0..i {
            if xs[j] < xs[i] && len[j] + 1 > len[i] {
                len[i] = len[j] + 1;
                prev[i] = j;
            }
        }
    }
    let Some(mut i) = (0..n).max_by_key(|i| (len[*i], std::cmp::Reverse(*i))) else { return vec![] };
    let mut out = vec![xs[i]];
    while prev[i] != usize::MAX {
        i = prev[i];
        out.push(xs[i]);
    }
    out.reverse();
    out
}

/// Insert `value` at insert path `path` of `source`, laid out like its
/// siblings by Petal's own path edit; or, where that cannot write it (a
/// helper call among children, a value it does not take), its one-line
/// source as [`insert_text`] places it.
fn insert_value(source: &str, kind: ContainerKind, path: &str, value: &StaticValue) -> Result<String, String> {
    let child_ok = kind == ContainerKind::List || matches!(value, StaticValue::Element { .. });
    if child_ok && let Ok(out) = insert_path(source, path, value, &Placement::End) {
        return Ok(out);
    }
    insert_text(source, kind, path, &value.to_source())
}

/// Insert `text` at insert path `path` of `source`: a placeholder value is
/// inserted (so it is laid out like its siblings), then its text replaced
/// by `text` at the placeholder's indentation.
fn insert_text(source: &str, kind: ContainerKind, path: &str, text: &str) -> Result<String, String> {
    let hole;
    let (ph, ph_text, text) = match kind {
        ContainerKind::List => (StaticValue::Str(PLACEHOLDER.into()), format!("\"{PLACEHOLDER}\""), text),
        ContainerKind::Children => {
            // An expression (a helper call that builds an element) goes
            // among the children as a hole, after the comment lines that
            // lead it.
            let body_at = text
                .split_inclusive('\n')
                .take_while(|l| l.trim().starts_with("{/*") && l.trim().ends_with("*/}"))
                .map(str::len)
                .sum::<usize>();
            let (lead, body) = text.split_at(body_at);
            let text = if body.trim_start().starts_with('<') {
                text
            } else {
                hole = format!("{lead}{{{}}}", body.trim());
                hole.as_str()
            };
            (
                StaticValue::Element { tag: PLACEHOLDER_TAG.into(), props: vec![], children: vec![] },
                format!("<{PLACEHOLDER_TAG} />"),
                text,
            )
        }
    };
    let with = insert_path(source, path, &ph, &Placement::End).map_err(|e| e.message)?;
    let at = with.find(&ph_text).ok_or("the new entry's place was lost")?;
    let indent = line_indent(&with, at);
    Ok(format!("{}{}{}", &with[..at], reindent(text, &indent), &with[at + ph_text.len()..]))
}

/// The comment after the entry at `span` on the same line (past its comma):
/// its text without the `#` (or `//`). `None` when the line goes on with
/// anything else.
pub fn trailing_comment(source: &str, span: &SourceSpan) -> Option<String> {
    let r = byte_range(source, span)?;
    let rest = &source[r.end..];
    let line = rest.split('\n').next()?;
    let line = line.trim_start();
    let line = line.strip_prefix(',').unwrap_or(line).trim();
    let body = line.strip_prefix('#').or_else(|| line.strip_prefix("//"))?;
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_string())
}

pub(crate) fn first_line(text: &str) -> String {
    let l = text.lines().next().unwrap_or("").trim();
    if text.lines().count() > 1 { format!("{l} …") } else { l.to_string() }
}
