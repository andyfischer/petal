//! Style-preserving edits of literal values in source — the engine under the
//! path goals of [`crate::goal_based_editing`].
//!
//! A config-style file holds its settings as literals: numbers, strings,
//! colors, and records and lists of them. This module changes such a literal to
//! a new [`StaticValue`] while keeping everything the author typed that the
//! change does not force out. The design is in `docs/features/source-preservation.md`;
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
//! An element (`<text y={400}>Hi</text>`) is a container too, in two ways.
//! A `.name` step enters its attribute `name`, which is set, added and removed
//! like a record field, and keeps the way it is written (`"text"`, `{value}`,
//! or a bare name for `true`) when the new value allows. A child step
//! (`/image[1]`, `/*[0]`; see [`crate::edit_path`]) enters a child element,
//! which is set, inserted, removed and moved like a list element: a new child
//! goes on its own line at its siblings' indentation when they each have one,
//! and the comments and blank lines between the tags stay. Setting a whole
//! element whose children are all elements edits them child by child the same
//! way; children with text among them are rewritten whole, and only if they
//! differ.
//!
//! [`move_path`] is remove plus insert, carrying the moved element's text.
//!
//! A value with no original to learn from renders through
//! [`StaticValue::to_source`], exactly as before.

use std::cell::OnceCell;
use std::ops::Range;

use crate::ast::{AssignTarget, Expr, ExprKind, JsxChild, Literal, RecordField, Stmt, StmtKind};
use crate::edit_path::{
    element_children, insert_position, parse_edit_path, path_text, select_child, tag_of,
};
use crate::cst::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::goal_based_editing::Placement;
use crate::lexer::Token;
use crate::rewrite::{
    PathSeg, Splice, apply_splices, edge_significant_token, find_node, parse_ast,
    significant_range,
};
use crate::static_value::{
    StaticBinding, StaticValue, bindings_of, color_literal, setting_value_expr,
    element_child_lines, eval,
};

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

// ── The primitives ───────────────────────────────────────────────────────

/// Make the value at `path` (`POST.effects[2].amount`,
/// `layers[2]/group[0]/image[1].x`, or a bare binding name) read as `value`.
///
/// Where the old and new values agree nothing is written; a value that already
/// holds returns the source byte-identical. A record field or an attribute that
/// does not exist yet is added at the end of its record or open tag. Anything
/// else the path fails to name is [`EditErrorKind::NotFound`], with a message
/// naming the step that failed and what is there. A child element step
/// (`/image[1]`) can only be set to an element.
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
    let (segs, target) = match locate(root, path, name, segs) {
        Ok(found) => found,
        // The parent record or element exists and only the field is missing:
        // add it.
        Err(fail) if fail.missing_field && fail.depth + 1 == segs.len() => {
            return insert_path(source, path, value, &Placement::End);
        }
        Err(fail) => return Err(fail.error),
    };
    if matches!(segs.last(), Some(PathSeg::ChildAt(_)))
        && !matches!(value, StaticValue::Element { .. })
    {
        return Err(EditError::invalid(format!(
            "`{path}` names a child element, so it can only be set to an element"
        )));
    }
    let bindings = bindings_of(&doc.stmts[..stmt], &doc.chars);
    let mut ops = Vec::new();
    // An attribute: how it is written decides whether its value can change
    // in place.
    if let Some((PathSeg::Field(key), parent_segs)) = segs.split_last()
        && let Ok(parent) = resolve(root, parent_segs)
        && let Shape::Element(props) = shape(parent)
        && let Some(tag) = doc.tag(parent)
        && let Some(index) = attr_index(props, key)
    {
        if eval(target, &bindings).as_ref() != Ok(value) {
            let attr = (key.as_str(), target, tag.attrs[index].form, index);
            doc.plan_attr(&bindings, &mut parent_segs.to_vec(), attr, value, &mut ops);
        }
        return apply(source, name, &ops);
    }
    doc.plan(&bindings, target, &mut segs.clone(), value, &mut ops);
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
///   the goal is that the field reads as `value`. An element's attribute is
///   added the same way.
/// - `layers[2]/image[1]` inserts a child element into the `<group>` at
///   `layers[2]`, before its current second `<image>` child (an index equal to
///   the number of `<image>` children puts it just after the last one).
///   `layers[2]/*[0]` makes it the first child element. Only an element can be
///   inserted as a child.
///
/// The new text is laid out like its siblings (see the module docs).
pub fn insert_path(
    source: &str,
    path: &str,
    value: &StaticValue,
    placement: &Placement,
) -> Result<String, EditError> {
    insert_what(source, path, What::Value(value.clone()), placement)
}

fn insert_what(
    source: &str,
    path: &str,
    what: What,
    placement: &Placement,
) -> Result<String, EditError> {
    match plan_insert(source, path, what, placement)? {
        Planned::Done(text) => Ok(text),
        Planned::Op(name, op) => apply(source, &name, &[op]),
    }
}

/// What an insert comes to: an op on a binding, or (a field that is already
/// there being set) the finished source.
enum Planned {
    Done(String),
    Op(String, Op),
}

fn plan_insert(
    source: &str,
    path: &str,
    what: What,
    placement: &Placement,
) -> Result<Planned, EditError> {
    let (name, segs) = parse_path(path)?;
    let Some((last, parent_path)) = segs.split_last() else {
        return Err(EditError::invalid(format!(
            "`{path}` names a whole binding; insert needs a field, an index or a child step inside one"
        )));
    };
    let doc = Doc::parse(source)?;
    let (_, root) = doc.binding(&name)?;
    let (parent_segs, parent) = locate(root, path, &name, parent_path).map_err(|f| f.error)?;
    let at = path_text(&name, parent_path);
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
                return match what {
                    What::Value(value) => set_path(source, path, &value).map(Planned::Done),
                    What::Text { .. } => Err(EditError::invalid(format!(
                        "`{path}` is already there; a field cannot be moved onto another"
                    ))),
                };
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
        (PathSeg::Field(key), Shape::Element(props)) => {
            if attr_index(props, key).is_some() {
                return match what {
                    What::Value(value) => set_path(source, path, &value).map(Planned::Done),
                    What::Text { .. } => Err(EditError::invalid(format!(
                        "`{path}` is already there; an attribute cannot be moved onto another"
                    ))),
                };
            }
            let anchor = |anchor: &str, after: usize| {
                attr_index(props, anchor).map_or(props.len(), |i| i + after)
            };
            let index = match placement {
                Placement::End => props.len(),
                Placement::After(a) => anchor(a, 1),
                Placement::Before(a) => anchor(a, 0),
            };
            (index, Some(key.clone()))
        }
        (PathSeg::Child(step), Shape::Element(_)) => {
            check_child(path, &what)?;
            let index = insert_position(&element_children(parent), step).map_err(|why| {
                EditError::invalid(format!("`{path}`: {} {why}", describe_at(parent, &at)))
            })?;
            let op = Op::InsertChild {
                segs: parent_segs,
                index,
                what,
            };
            return Ok(Planned::Op(name, op));
        }
        (PathSeg::Index(i), Shape::Element(_)) => {
            return Err(EditError::invalid(format!(
                "`{path}`: {} has no list to index; its child elements are `{at}/*[{i}]`",
                describe_at(parent, &at)
            )));
        }
        _ => {
            return Err(EditError::invalid(format!(
                "`{path}`: cannot insert there — {} (an index needs a list or a call's arguments, a field needs a record or an element, and a child step needs an element)",
                describe_at(parent, &at)
            )));
        }
    };
    if key.is_some() && matches!(what, What::Text { .. }) {
        return Err(EditError::invalid(format!(
            "`{path}`: only a list element or a child element can be moved"
        )));
    }
    let op = Op::Insert {
        segs: parent_segs,
        index,
        key,
        what,
    };
    Ok(Planned::Op(name, op))
}

/// Only an element can go among an element's children.
fn check_child(path: &str, what: &What) -> Result<(), EditError> {
    let ok = match what {
        What::Value(value) => matches!(value, StaticValue::Element { .. }),
        What::Text { text, .. } => text.starts_with('<'),
    };
    if ok {
        Ok(())
    } else {
        Err(EditError::invalid(format!(
            "`{path}`: only an element can be a child element"
        )))
    }
}

/// Append `value` to the list (or call arguments) at `path`, or, when `path`
/// names an element, make `value` (an element) its last child element.
pub fn append_path(source: &str, path: &str, value: &StaticValue) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    let doc = Doc::parse(source)?;
    let (_, root) = doc.binding(&name)?;
    let (segs, target) = locate(root, path, &name, &segs).map_err(|f| f.error)?;
    let op = match shape(target) {
        Shape::List(items) | Shape::Call(items) => Op::Insert {
            segs,
            index: items.len(),
            key: None,
            what: What::Value(value.clone()),
        },
        Shape::Element(_) => {
            let what = What::Value(value.clone());
            check_child(path, &what)?;
            Op::InsertChild {
                segs,
                index: element_children(target).len(),
                what,
            }
        }
        _ => {
            return Err(EditError::invalid(format!(
                "`{path}` is {}, not a list or an element, so nothing can be appended to it",
                describe(target)
            )));
        }
    };
    apply(source, &name, &[op])
}

/// Remove the list element (or call argument), record field, attribute or
/// child element `path` names.
///
/// A record field or attribute that is not there is already removed: the
/// source comes back unchanged. A list index past the end, or a child step
/// that names no child, is [`EditErrorKind::NotFound`]. An element on its own
/// line takes its line (and a comment trailing it on that line) with it;
/// comments on other lines stay.
pub fn remove_path(source: &str, path: &str) -> Result<String, EditError> {
    let (name, segs) = parse_path(path)?;
    let Some((last, parent_path)) = segs.split_last() else {
        return Err(EditError::invalid(format!(
            "`{path}` names a whole binding; remove needs a field, an index or a child step inside one"
        )));
    };
    let mut text = source.to_string();
    // A key written twice is removed twice, so the field is really gone.
    loop {
        let doc = Doc::parse(&text)?;
        let (_, root) = doc.binding(&name)?;
        let (parent_segs, parent) =
            locate(root, path, &name, parent_path).map_err(|f| f.error)?;
        let op = match (last, shape(parent)) {
            (PathSeg::Child(_), Shape::Element(_)) => {
                let (mut segs, _) = locate(root, path, &name, &segs).map_err(|f| f.error)?;
                let Some(PathSeg::ChildAt(index)) = segs.pop() else {
                    return Err(nothing_at(path));
                };
                let op = Op::RemoveChild { segs, index, lead: false };
                return apply(&text, &name, &[op]);
            }
            (PathSeg::Index(_), _) => {
                let (mut segs, _) = locate(root, path, &name, &segs).map_err(|f| f.error)?;
                let Some(PathSeg::Index(index)) = segs.pop() else {
                    return Err(nothing_at(path));
                };
                let op = Op::Remove { segs, index, lead: false };
                return apply(&text, &name, &[op]);
            }
            (PathSeg::Field(key), Shape::Record(fields)) => match field_index(fields, key) {
                Some(index) => Op::Remove {
                    segs: parent_segs,
                    index,
                    lead: false,
                },
                None => return Ok(text),
            },
            (PathSeg::Field(key), Shape::Element(props)) => match attr_index(props, key) {
                Some(index) => Op::Remove {
                    segs: parent_segs,
                    index,
                    lead: false,
                },
                None => return Ok(text),
            },
            _ => return Err(locate(root, path, &name, &segs).err().map_or_else(|| nothing_at(path), |f| f.error)),
        };
        text = apply(&text, &name, &[op])?;
    }
}

/// Move the list element or child element `from` names to `to`, keeping its
/// text — its comments and layout inside — as written.
///
/// `to` is an insert path ([`insert_path`]) read *after* `from` is taken out,
/// so it names where the value ends up: `layers[0]` → `layers[2]` makes the
/// first layer the third. It can be in another container, of either kind:
/// `layers[3]` → `layers[1]/*[0]` moves a top-level layer into a group as its
/// first child. A multi-line value is re-indented to its new depth. A `to`
/// naming the place `from` already is, however it is spelled, changes
/// nothing. The comment trailing the value on its line comes along (among an
/// element's children, on its own line above it).
pub fn move_path(source: &str, from: &str, to: &str) -> Result<String, EditError> {
    let (name, segs) = parse_path(from)?;
    parse_path(to)?;
    let doc = Doc::parse(source)?;
    let (_, root) = doc.binding(&name)?;
    let (mut segs, value) = locate(root, from, &name, &segs).map_err(|f| f.error)?;
    if from.trim() == to.trim() {
        return Ok(source.to_string());
    }
    let parent = segs
        .len()
        .checked_sub(1)
        .and_then(|n| resolve(root, &segs[..n]).ok());
    let (op, what) = match (segs.pop(), parent) {
        (Some(PathSeg::Index(index)), Some(parent))
            if matches!(shape(parent), Shape::List(_) | Shape::Call(_)) =>
        {
            let seq = doc.container(parent)?;
            let item = seq.items.get(index).ok_or_else(|| nothing_at(from))?;
            let what = What::Text {
                text: doc.text(item.start, item.end),
                tail: doc.tail(item),
                indent: doc.line_indent(item.start),
                lead: String::new(),
            };
            (Op::Remove { segs, index, lead: false }, what)
        }
        (Some(PathSeg::ChildAt(index)), Some(parent)) => {
            let start = value.span.start.offset as usize;
            let tail = doc
                .child_items(parent)
                .and_then(|items| items.get(index).map(|item| doc.child_tail(item)))
                .unwrap_or_default();
            let what = What::Text {
                text: doc.expr_text(value),
                tail,
                indent: doc.line_indent(start),
                lead: String::new(),
            };
            (Op::RemoveChild { segs, index, lead: false }, what)
        }
        _ => {
            return Err(EditError::invalid(format!(
                "`{from}`: only a list element (or call argument) or a child element can be moved"
            )));
        }
    };
    let removed = apply(source, &name, std::slice::from_ref(&op))?;
    match plan_insert(&removed, to, what, &Placement::End)? {
        Planned::Done(text) => Ok(text),
        Planned::Op(to_name, to_op) => {
            // Back where it was (`g/image[1]` → `g/*[1]`): nothing moves.
            let same = to_name == name
                && match (&op, &to_op) {
                    (
                        Op::Remove { segs, index, .. },
                        Op::Insert { segs: to_segs, index: to_index, key: None, .. },
                    )
                    | (
                        Op::RemoveChild { segs, index, .. },
                        Op::InsertChild { segs: to_segs, index: to_index, .. },
                    ) => segs == to_segs && index == to_index,
                    _ => false,
                };
            if same {
                return Ok(source.to_string());
            }
            apply(&removed, &to_name, &[to_op])
        }
    }
}

fn parse_path(path: &str) -> Result<(String, Vec<PathSeg>), EditError> {
    parse_edit_path(path).map_err(EditError::invalid)
}

fn nothing_at(path: &str) -> EditError {
    EditError::not_found(format!("`{path}` names no literal value in the source"))
}

/// A path step that names nothing.
struct StepFail {
    /// Which step.
    depth: usize,
    /// The step is a field that the record or element before it lacks (so a
    /// set can add it).
    missing_field: bool,
    error: EditError,
}

/// What `expr` is, for an error message: "a list of 3 elements", "a `<group>`
/// element".
fn describe(expr: &Expr) -> String {
    match shape(expr) {
        Shape::Record(_) => "a record".into(),
        Shape::List(items) => format!("a list of {} elements", items.len()),
        Shape::Call(args) => match &expr.kind {
            ExprKind::Call { function, .. } => match &function.kind {
                ExprKind::Ident(f) => format!("a call to `{f}` with {} arguments", args.len()),
                _ => "a call".into(),
            },
            _ => "a call".into(),
        },
        Shape::Element(_) => format!("a `<{}>` element", tag_of(expr).unwrap_or("?")),
        Shape::Leaf if color_literal(expr).is_some() => "a color".into(),
        Shape::Leaf => match &expr.kind {
            ExprKind::Literal(Literal::Int(_) | Literal::Float(_)) => "a number".into(),
            ExprKind::Literal(Literal::String(_)) => "a string".into(),
            ExprKind::Literal(Literal::Bool(_)) => "a boolean".into(),
            ExprKind::Literal(Literal::Nil) => "nil".into(),
            ExprKind::Ident(name) => {
                format!("the name `{name}` (a path cannot see through a name)")
            }
            _ => "an expression that is not a literal".into(),
        },
    }
}

/// "the `<group>` element at `layers[2]`".
fn describe_at(expr: &Expr, at: &str) -> String {
    let what = describe(expr);
    let what = what.strip_prefix("a ").unwrap_or(&what);
    format!("the {what} at `{at}`")
}

/// Follow the steps of `path` (`name` then `segs`) from `root`, resolving each
/// child step to the position of the child it names
/// ([`PathSeg::ChildAt`]). Returns those steps and the expression they reach;
/// the error names the first step that names nothing and says what is there.
fn locate<'a>(
    root: &'a Expr,
    path: &str,
    name: &str,
    segs: &[PathSeg],
) -> Result<(Vec<PathSeg>, &'a Expr), StepFail> {
    let mut expr = root;
    let mut out = Vec::with_capacity(segs.len());
    for (depth, seg) in segs.iter().enumerate() {
        let at = path_text(name, &segs[..depth]);
        let fail = |missing_field: bool, why: String| StepFail {
            depth,
            missing_field,
            error: EditError::not_found(format!(
                "`{path}`: step `{seg}` names nothing — {why}"
            )),
        };
        let here = describe_at(expr, &at);
        let (next, resolved) = match (seg, shape(expr)) {
            (PathSeg::Field(key), Shape::Record(fields)) => match field_index(fields, key) {
                Some(i) => (element(expr, i), seg.clone()),
                None => return Err(fail(true, format!("{here} has no field `{key}`"))),
            },
            (PathSeg::Field(key), Shape::Element(props)) => match attr_index(props, key) {
                Some(i) => (element(expr, i), seg.clone()),
                None => return Err(fail(true, format!("{here} has no attribute `{key}`"))),
            },
            (PathSeg::Index(i), Shape::List(items) | Shape::Call(items)) => match items.get(*i) {
                Some(item) => (Some(item), seg.clone()),
                None => return Err(fail(false, format!("{here} has no element {i}"))),
            },
            (PathSeg::Index(i), Shape::Element(_)) => {
                return Err(fail(
                    false,
                    format!("{here} is not a list; its child elements are `{at}/*[{i}]`"),
                ));
            }
            (PathSeg::Child(step), Shape::Element(_)) => {
                let children = element_children(expr);
                match select_child(&children, step) {
                    Ok(i) => (Some(children[i]), PathSeg::ChildAt(i)),
                    Err(why) => return Err(fail(false, format!("{here} {why}"))),
                }
            }
            (PathSeg::ChildAt(i), Shape::Element(_)) => {
                (element_children(expr).get(*i).copied(), seg.clone())
            }
            (PathSeg::Field(_), _) => {
                return Err(fail(false, format!("`{at}` is {}, which has no fields", describe(expr))));
            }
            (PathSeg::Index(_), _) => {
                return Err(fail(false, format!("`{at}` is {}, not a list", describe(expr))));
            }
            (PathSeg::Child(_) | PathSeg::ChildAt(_), _) => {
                return Err(fail(false, format!("`{at}` is {}, not an element", describe(expr))));
            }
        };
        expr = next.ok_or_else(|| fail(false, format!("{here} has nothing there")))?;
        out.push(resolved);
    }
    Ok((out, expr))
}

// ── Reading the shape of a literal ───────────────────────────────────────

/// What a path step can enter.
enum Shape<'a> {
    Record(&'a [RecordField]),
    List(&'a [Expr]),
    /// The positional arguments of a call to a plain name: a constructor like
    /// `vec3(0.3, -1.0, 0.5)`.
    Call(&'a [Expr]),
    /// An element's attributes. A `.name` step enters an attribute, and a
    /// child step (`/tag[n]`, resolved to [`PathSeg::ChildAt`]) a child
    /// element.
    Element(&'a [(String, Expr)]),
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
        ExprKind::Element { props, .. } => Shape::Element(props),
        _ => Shape::Leaf,
    }
}

/// Index of the attribute `key` — the last one, which is the one a read sees.
fn attr_index(props: &[(String, Expr)], key: &str) -> Option<usize> {
    props.iter().rposition(|(k, _)| k == key)
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
        Shape::Element(props) => props.get(index).map(|(_, value)| value),
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
            (PathSeg::Field(key), Shape::Element(props)) => attr_index(props, key),
            (PathSeg::Index(i), Shape::List(_) | Shape::Call(_)) => Some(*i),
            (PathSeg::ChildAt(i), Shape::Element(_)) => {
                expr = element_children(expr).get(*i).copied().ok_or(depth)?;
                continue;
            }
            (PathSeg::Child(step), Shape::Element(_)) => {
                let children = element_children(expr);
                expr = children[select_child(&children, step).map_err(|_| depth)?];
                continue;
            }
            _ => None,
        };
        expr = index.and_then(|i| element(expr, i)).ok_or(depth)?;
    }
    Ok(expr)
}

// ── The edit script ──────────────────────────────────────────────────────

/// New content: a value to render in the surrounding style, or source text
/// lifted from elsewhere (an element that only moved) — the element's own
/// text, the comment trailing it on its line, if any, and the indentation of
/// the line it started on, so a multi-line text can be re-indented. `lead`
/// is the `//` comment lines directly above it, when they move with it (a
/// reorder through a whole-value set), each ending in a newline.
#[derive(Debug, Clone)]
enum What {
    Value(StaticValue),
    Text {
        text: String,
        tail: String,
        indent: String,
        lead: String,
    },
}

impl What {
    /// The comments that go with a moved text, as whole lines at `indent`:
    /// its leading comment lines, then the comment that trailed it (which
    /// cannot trail an element's tag: there it would be text).
    fn comment_lines(&self, indent: &str, with_tail: bool) -> String {
        let What::Text { tail, lead, .. } = self else {
            return String::new();
        };
        let mut out = String::new();
        for line in lead.lines() {
            out.push_str(indent);
            out.push_str(line.trim());
            out.push('\n');
        }
        if with_tail && !tail.trim().is_empty() {
            out.push_str(indent);
            out.push_str(tail.trim());
            out.push('\n');
        }
        out
    }

    /// The same comments as `/* … */` block comments, for a place with no
    /// line of its own (each followed by a space).
    fn inline_comments(&self, with_tail: bool, hole: bool) -> String {
        let What::Text { tail, lead, .. } = self else {
            return String::new();
        };
        let tail = if with_tail { tail.as_str() } else { "" };
        lead.lines()
            .chain(std::iter::once(tail))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let body = line.strip_prefix("//").unwrap_or(line).trim().replace("*/", "* /");
                if hole {
                    format!("{{/* {body} */}} ")
                } else {
                    format!("/* {body} */ ")
                }
            })
            .collect()
    }
}

/// `text`, which started on a line indented by `from`, re-indented for a line
/// indented by `to`: its later lines move by the difference.
fn reindent(text: &str, from: &str, to: &str) -> String {
    if from == to || !text.contains('\n') {
        return text.to_string();
    }
    let mut lines = text.split('\n');
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        match line.strip_prefix(from) {
            Some(rest) => {
                out.push_str(to);
                out.push_str(rest);
            }
            None => out.push_str(line),
        }
    }
    out
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
    /// Remove element `index` of the container at `segs` (with the comment
    /// lines directly above it when `lead`, for an element that moves).
    Remove {
        segs: Vec<PathSeg>,
        index: usize,
        lead: bool,
    },
    /// Rewrite attribute `index` of the element at `segs` whole, for a value
    /// its current spelling cannot hold (`font="Futura"` becoming a number,
    /// a bare `muted` becoming `false`).
    ReplaceAttr {
        segs: Vec<PathSeg>,
        index: usize,
        value: StaticValue,
    },
    /// Rewrite the children of the element at `segs`.
    ReplaceChildren {
        segs: Vec<PathSeg>,
        children: Vec<StaticValue>,
    },
    /// Insert a child element into the element at `segs` so it becomes child
    /// element `index` (counting every child element, `<br />` included).
    InsertChild {
        segs: Vec<PathSeg>,
        index: usize,
        what: What,
    },
    /// Remove child element `index` of the element at `segs` (with the
    /// comment lines directly above it when `lead`).
    RemoveChild {
        segs: Vec<PathSeg>,
        index: usize,
        lead: bool,
    },
}

/// One step of lining an old sequence up with a new one.
enum Step {
    Keep(usize, usize),
    Insert(usize),
    Remove(usize),
}

/// The elements that only move: each `Insert(j)` paired with a `Remove(i)`
/// of the same value (`same(i, j)`), first come first served, as
/// `(j, i)` pairs.
fn pair_moves(steps: &[Step], same: &dyn Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let mut removed: Vec<usize> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Remove(i) => Some(*i),
            _ => None,
        })
        .collect();
    steps
        .iter()
        .filter_map(|step| match step {
            Step::Insert(j) => {
                let at = removed.iter().position(|&i| same(i, *j))?;
                Some((*j, removed.swap_remove(at)))
            }
            _ => None,
        })
        .collect()
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
        (
            Element {
                tag: f, props: a, ..
            },
            Element {
                tag: g, props: b, ..
            },
        ) if f == g => {
            let shared = a.iter().filter(|attr| b.contains(attr)).count();
            if shared > 0 && shared * 2 >= a.len().min(b.len()) {
                2
            } else {
                1
            }
        }
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

/// How an attribute's value is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrForm {
    /// `name="text"`
    Quoted,
    /// `name={value}`
    Braced,
    /// A bare `name`, which is `true`.
    Bare,
}

/// One attribute of an element, as char offsets into the source.
#[derive(Debug, Clone)]
struct Attr {
    start: usize,
    end: usize,
    form: AttrForm,
}

/// An element located in the source: its open tag's attributes, how the open
/// tag ends, and where the children and the close tag are.
#[derive(Debug, Clone)]
struct Tag {
    name: String,
    /// Just past the tag name in the open tag.
    name_end: usize,
    attrs: Vec<Attr>,
    /// The `>` that ends the open tag, or the `/>` of a self-closing element.
    open_end: Range<usize>,
    self_closing: bool,
    /// Where `</tag>` starts; `None` for a self-closing element.
    close: Option<usize>,
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
    /// The last top-level binding of `name`: its statement index and where
    /// its value is written. In `let X = tunable(v, …)` that is `v`
    /// ([`setting_value_expr`]): the path `X` names the setting, not the hint.
    fn binding(&self, name: &str) -> Result<(usize, &Expr), EditError> {
        self.stmts
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, stmt)| match &stmt.kind {
                StmtKind::Let {
                    name: bound,
                    value,
                    ..
                } if bound == name => Some((i, setting_value_expr(value))),
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

    /// Whether only indentation comes before `at` on its line.
    fn starts_line(&self, at: usize) -> bool {
        self.chars[self.line_start(at)..at]
            .iter()
            .all(|&c| is_blank(c))
    }

    // ── Elements ─────────────────────────────────────────────────────────

    /// The element `expr` is written as, located in the tree.
    fn tag(&self, expr: &Expr) -> Option<Tag> {
        let ExprKind::Element { props, .. } = &expr.kind else {
            return None;
        };
        let node = find_node(&self.root, (expr.span.start.offset, expr.span.end.offset))?;
        if node.kind() != SyntaxKind::ElementExpr {
            return None;
        }
        let (mut name, mut name_end, mut open_end, mut close) = (None, None, None, None);
        let mut self_closing = false;
        let mut attrs = Vec::new();
        for el in node.children() {
            match el {
                SyntaxElement::Token(t) if t.is_trivia() => {}
                SyntaxElement::Token(t) => {
                    let at = t.offset() as usize;
                    let end = at + t.text_len() as usize;
                    match t.token() {
                        Some(Token::JsxTagName(tag)) if name.is_none() => {
                            name = Some(tag.clone());
                            name_end = Some(end);
                        }
                        Some(Token::Gt) if open_end.is_none() => open_end = Some(at..end),
                        Some(Token::JsxSelfClose) if open_end.is_none() => {
                            open_end = Some(at..end);
                            self_closing = true;
                        }
                        Some(Token::JsxCloseStart) => close = Some(at),
                        _ => {}
                    }
                }
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::JsxAttr => {
                    let (start, end) = significant_range(&n)?;
                    let braced = n
                        .children()
                        .into_iter()
                        .any(|el| matches!(el, SyntaxElement::Node(_)));
                    let quoted = n.children().into_iter().any(|el| {
                        matches!(el, SyntaxElement::Token(t) if matches!(t.token(), Some(Token::String(_))))
                    });
                    attrs.push(Attr {
                        start: start as usize,
                        end: end as usize,
                        form: if braced {
                            AttrForm::Braced
                        } else if quoted {
                            AttrForm::Quoted
                        } else {
                            AttrForm::Bare
                        },
                    });
                }
                SyntaxElement::Node(_) => {}
            }
        }
        (attrs.len() == props.len()).then_some(())?;
        Some(Tag {
            name: name?,
            name_end: name_end?,
            attrs,
            open_end: open_end?,
            self_closing,
            close: if self_closing { None } else { Some(close?) },
        })
    }

    /// One attribute, `key="text"`, a bare `key` for `true`, or `key={value}`
    /// with the value rendered like `model`.
    fn render_attr(
        &self,
        key: &str,
        value: &StaticValue,
        model: Option<&Expr>,
        indent: &str,
    ) -> String {
        match value {
            StaticValue::Str(_) | StaticValue::Bool(true) => {
                crate::static_value::render_attr(key, value, 0)
            }
            other => format!("{key}={{{}}}", self.render(other, model, indent)),
        }
    }

    /// The step attributes are indented by when `tag` puts each on its own
    /// line, or `None` when they share the tag's line.
    fn attr_step(&self, tag: &Tag, start: usize) -> Option<String> {
        let first = tag.attrs.first()?;
        if !self.starts_line(first.start) {
            return None;
        }
        let outer = self.line_indent(start);
        let inner = self.line_indent(first.start);
        Some(
            inner
                .strip_prefix(outer.as_str())
                .filter(|step| !step.is_empty())
                .map_or_else(|| self.file_style().step.clone(), str::to_string),
        )
    }

    /// An element's children in source, for an element whose tags start on a
    /// line indented by `indent`: the text between `>` and `</tag>`.
    fn children_text(&self, children: &[StaticValue], indent: &str) -> String {
        let lines = element_child_lines(children, 0);
        let inner = format!("{indent}{}", self.file_style().step);
        match lines.as_slice() {
            [only] if !only.starts_with('<') && !only.contains('\n') => only.clone(),
            _ => {
                let mut out = String::from("\n");
                for line in &lines {
                    out.push_str(&inner);
                    out.push_str(&line.replace('\n', &format!("\n{inner}")));
                    out.push('\n');
                }
                out.push_str(indent);
                out
            }
        }
    }

    /// Render an element starting on a line indented by `indent`, laid out
    /// like `model` (another element) when there is one: its attributes on
    /// the tag's line or one per line, as the model has them.
    fn render_element(
        &self,
        tag: &str,
        props: &[(String, StaticValue)],
        children: &[StaticValue],
        model: Option<&Expr>,
        indent: &str,
    ) -> String {
        let model_tag = model.and_then(|m| Some((m, self.tag(m)?)));
        let step = model_tag
            .as_ref()
            .and_then(|(m, t)| self.attr_step(t, m.span.start.offset as usize));
        let attr_model = |key: &str| {
            let (m, _) = model_tag.as_ref()?;
            let Shape::Element(model_props) = shape(m) else {
                return None;
            };
            element(m, attr_index(model_props, key)?)
        };
        let mut out = format!("<{tag}");
        match &step {
            Some(step) if !props.is_empty() => {
                let inner = format!("{indent}{step}");
                for (key, value) in props {
                    out.push('\n');
                    out.push_str(&inner);
                    out.push_str(&self.render_attr(key, value, attr_model(key), &inner));
                }
                out.push('\n');
                out.push_str(indent);
                if children.is_empty() {
                    out.push_str("/>");
                    return out;
                }
            }
            _ => {
                for (key, value) in props {
                    out.push(' ');
                    out.push_str(&self.render_attr(key, value, attr_model(key), indent));
                }
                if children.is_empty() {
                    out.push_str(" />");
                    return out;
                }
            }
        }
        out.push('>');
        out.push_str(&self.children_text(children, indent));
        out.push_str(&format!("</{tag}>"));
        out
    }

    /// Splices adding attribute `key` as attribute `index` of `container`,
    /// laid out like the attribute before it (or after, when it goes first).
    fn insert_attr(
        &self,
        container: &Expr,
        index: usize,
        key: &str,
        what: &What,
    ) -> Result<Vec<Splice>, EditError> {
        let tag = self.tag(container).ok_or_else(|| {
            EditError::invalid("the element there could not be located in the source")
        })?;
        if index > tag.attrs.len() {
            return Err(EditError::invalid("insert index is past the end"));
        }
        let near = tag.attrs.get(index.saturating_sub(1));
        let indent = match near {
            Some(attr) if self.starts_line(attr.start) => Some(self.line_indent(attr.start)),
            _ => None,
        };
        let text = match what {
            What::Text { text, .. } => text.clone(),
            What::Value(value) => {
                let at = indent
                    .clone()
                    .unwrap_or_else(|| self.line_indent(container.span.start.offset as usize));
                self.render_attr(key, value, None, &at)
            }
        };
        let gap = match &indent {
            Some(indent) => format!("\n{indent}"),
            None => " ".to_string(),
        };
        let at = |at: usize, text: String| Splice {
            start: at,
            end: at,
            text,
        };
        Ok(vec![match (index, near) {
            (0, Some(first)) => at(first.start, format!("{text}{gap}")),
            (_, Some(prev)) => at(prev.end, format!("{gap}{text}")),
            (_, None) => at(tag.name_end, format!(" {text}")),
        }])
    }

    /// Splices removing attribute `index` of `container`, with the space (or
    /// the line break) before it.
    fn remove_attr(&self, container: &Expr, index: usize) -> Result<Vec<Splice>, EditError> {
        let tag = self.tag(container).ok_or_else(|| {
            EditError::invalid("the element there could not be located in the source")
        })?;
        let attr = tag
            .attrs
            .get(index)
            .ok_or_else(|| EditError::not_found("no attribute at that index"))?;
        let mut start = attr.start;
        while start > 0 && matches!(self.chars[start - 1], ' ' | '\t' | '\r' | '\n') {
            start -= 1;
        }
        Ok(vec![Splice {
            start,
            end: attr.end,
            text: String::new(),
        }])
    }

    fn replace_attr(
        &self,
        container: &Expr,
        index: usize,
        value: &StaticValue,
    ) -> Result<Vec<Splice>, EditError> {
        let tag = self.tag(container).ok_or_else(|| {
            EditError::invalid("the element there could not be located in the source")
        })?;
        let (Some(attr), ExprKind::Element { props, .. }) = (tag.attrs.get(index), &container.kind)
        else {
            return Err(EditError::not_found("no attribute at that index"));
        };
        let text = self.render_attr(&props[index].0, value, None, &self.line_indent(attr.start));
        Ok(vec![Splice {
            start: attr.start,
            end: attr.end,
            text,
        }])
    }

    /// Splices making the children of `container` read as `children`. A
    /// self-closing element gains a close tag, and one left empty closes
    /// itself.
    fn replace_children(
        &self,
        container: &Expr,
        children: &[StaticValue],
    ) -> Result<Vec<Splice>, EditError> {
        let tag = self.tag(container).ok_or_else(|| {
            EditError::invalid("the element there could not be located in the source")
        })?;
        let indent = self.line_indent(container.span.start.offset as usize);
        let end = container.span.end.offset as usize;
        // The open tag's own spacing before its `>` or `/>`: kept when the
        // delimiter has a line to itself (attributes one per line).
        let own_line = self.starts_line(tag.open_end.start);
        let mut before = tag.open_end.start;
        if !own_line {
            while before > tag.name_end && is_blank(self.chars[before - 1]) {
                before -= 1;
            }
        }
        if children.is_empty() {
            if tag.self_closing {
                return Ok(Vec::new());
            }
            let space = if own_line { "" } else { " " };
            return Ok(vec![Splice {
                start: before,
                end,
                text: format!("{space}/>"),
            }]);
        }
        let body = self.children_text(children, &indent);
        Ok(vec![match tag.close {
            Some(close) => Splice {
                start: tag.open_end.end,
                end: close,
                text: body,
            },
            None => Splice {
                start: before,
                end,
                text: format!(">{body}</{}>", tag.name),
            },
        }])
    }

    /// Plan making attribute `key` (now `old`, written in `form`, at `index`)
    /// of the element at `segs` read as `value`. `{...}` holds anything and a
    /// quoted string holds a string, so those are edited inside; everything
    /// else rewrites the attribute.
    fn plan_attr(
        &self,
        bindings: &[StaticBinding],
        segs: &mut Vec<PathSeg>,
        (key, old, form, index): (&str, &Expr, AttrForm, usize),
        value: &StaticValue,
        ops: &mut Vec<Op>,
    ) {
        let in_place = match form {
            AttrForm::Braced => !matches!(value, StaticValue::Bool(true)),
            AttrForm::Quoted => matches!(value, StaticValue::Str(_)),
            AttrForm::Bare => false,
        };
        if in_place {
            segs.push(PathSeg::Field(key.to_string()));
            self.plan(bindings, old, segs, value, ops);
            segs.pop();
        } else {
            ops.push(Op::ReplaceAttr {
                segs: segs.clone(),
                index,
                value: value.clone(),
            });
        }
    }

    /// [`Doc::plan`] for an element that stays the same tag: attributes are
    /// lined up by name and planned like record fields, and the children are
    /// rewritten when they differ.
    fn plan_element(
        &self,
        bindings: &[StaticBinding],
        old: &Expr,
        segs: &mut Vec<PathSeg>,
        new: &StaticValue,
        ops: &mut Vec<Op>,
    ) {
        let (
            ExprKind::Element {
                props: old_props,
                children: old_children,
                ..
            },
            StaticValue::Element {
                props: new_props,
                children: new_children,
                ..
            },
        ) = (&old.kind, new)
        else {
            return;
        };
        let Some(tag) = self.tag(old) else {
            return ops.push(Op::Replace {
                segs: segs.clone(),
                value: new.clone(),
            });
        };
        let mut steps = Vec::new();
        align(
            0..old_props.len(),
            0..new_props.len(),
            &[&|i, j| old_props[i].0 == new_props[j].0],
            &mut steps,
        );
        let mut pos = 0;
        for step in steps {
            match step {
                Step::Keep(i, j) => {
                    let (key, value) = &new_props[j];
                    let old_value = &old_props[i].1;
                    if eval(old_value, bindings).as_ref() != Ok(value) {
                        let attr = (key.as_str(), old_value, tag.attrs[i].form, pos);
                        self.plan_attr(bindings, segs, attr, value, ops);
                    }
                    pos += 1;
                }
                Step::Insert(j) => {
                    ops.push(Op::Insert {
                        segs: segs.clone(),
                        index: pos,
                        key: Some(new_props[j].0.clone()),
                        what: What::Value(new_props[j].1.clone()),
                    });
                    pos += 1;
                }
                Step::Remove(_) => ops.push(Op::Remove {
                    segs: segs.clone(),
                    index: pos,
                    lead: false,
                }),
            }
        }
        let old_values: Result<Vec<StaticValue>, String> = old_children
            .iter()
            .map(|child| match child {
                JsxChild::Text(text) => Ok(StaticValue::Str(text.clone())),
                JsxChild::Expr(expr) => eval(expr, bindings),
            })
            .collect();
        if old_values.as_ref() == Ok(new_children) {
            return;
        }
        // Children that are all elements, before and after, are edited one
        // child at a time, like a list's elements, so the comments and
        // layout between the tags stay. (No children before counts: the new
        // ones are inserted below any comments already inside.)
        let kids = element_children(old);
        let all_elements = kids.len() == old_children.len()
            && !new_children.is_empty()
            && new_children
                .iter()
                .all(|child| matches!(child, StaticValue::Element { .. }));
        if all_elements && let Some(items) = self.child_items(old) {
            return self.plan_children(bindings, &kids, &items, segs, new_children, ops);
        }
        // Text among them that stays as it is, with each element child still
        // an element in the same place: the elements are edited in place.
        // Otherwise the text changed, and the children are rewritten whole.
        if let Ok(old_values) = &old_values
            && old_values.len() == new_children.len()
            && old_children.iter().zip(old_values).zip(new_children).all(
                |((child, old_value), new_value)| match child {
                    JsxChild::Expr(e) if matches!(e.kind, ExprKind::Element { .. }) => {
                        matches!(new_value, StaticValue::Element { .. })
                    }
                    _ => old_value == new_value,
                },
            )
        {
            let pairs = old_children.iter().zip(new_children).filter_map(|(child, new)| {
                match child {
                    JsxChild::Expr(e) if matches!(e.kind, ExprKind::Element { .. }) => {
                        Some((e, new))
                    }
                    _ => None,
                }
            });
            for (at, (kid, new)) in pairs.enumerate() {
                segs.push(PathSeg::ChildAt(at));
                self.plan(bindings, kid, segs, new, ops);
                segs.pop();
            }
            return;
        }
        ops.push(Op::ReplaceChildren {
            segs: segs.clone(),
            children: new_children.to_vec(),
        });
    }

    /// Plan the ops that make the child elements `kids` (of the element at
    /// `segs`, located at `items`) read as `new`: lined up like a list's
    /// elements, edited in place where they look alike, and inserted, removed
    /// or moved (keeping their text) where not.
    fn plan_children(
        &self,
        bindings: &[StaticBinding],
        kids: &[&Expr],
        items: &[Item],
        segs: &mut Vec<PathSeg>,
        new: &[StaticValue],
        ops: &mut Vec<Op>,
    ) {
        let old_values: Vec<Option<StaticValue>> =
            kids.iter().map(|e| eval(e, bindings).ok()).collect();
        let like = |i: usize, j: usize| {
            old_values[i]
                .as_ref()
                .map_or(0, |old| likeness(old, &new[j]))
        };
        let mut steps = Vec::new();
        align(
            0..kids.len(),
            0..new.len(),
            &[
                &|i, j| old_values[i].as_ref() == Some(&new[j]),
                &|i, j| like(i, j) >= 2,
                &|i, j| like(i, j) >= 1,
            ],
            &mut steps,
        );
        // A child that only moved keeps its text, and the comment lines
        // directly above it go with it.
        let moves = pair_moves(&steps, &|i, j| old_values[i].as_ref() == Some(&new[j]));
        let mut pos = 0;
        for step in steps {
            match step {
                Step::Keep(i, j) => {
                    segs.push(PathSeg::ChildAt(pos));
                    self.plan(bindings, kids[i], segs, &new[j], ops);
                    segs.pop();
                    pos += 1;
                }
                Step::Insert(j) => {
                    let what = match moves.iter().find(|&&(to, _)| to == j) {
                        Some(&(_, i)) => {
                            let item = &items[i];
                            let line = self.line_start(item.start);
                            let lead = if self.child_own_line(item) {
                                self.text(self.lead_start(item.start), line)
                            } else {
                                String::new()
                            };
                            What::Text {
                                text: self.expr_text(kids[i]),
                                tail: self.child_tail(item),
                                indent: self.line_indent(item.start),
                                lead,
                            }
                        }
                        None => What::Value(new[j].clone()),
                    };
                    ops.push(Op::InsertChild {
                        segs: segs.clone(),
                        index: pos,
                        what,
                    });
                    pos += 1;
                }
                Step::Remove(i) => ops.push(Op::RemoveChild {
                    segs: segs.clone(),
                    index: pos,
                    lead: moves.iter().any(|&(_, from)| from == i),
                }),
            }
        }
    }

    /// The child elements of `container`, located in the source: one
    /// [`Item`] each, spanning the element (and the braces of a `{<tag />}`
    /// hole around it). `None` when they cannot be located.
    fn child_items(&self, container: &Expr) -> Option<Vec<Item>> {
        let len = self.chars.len();
        element_children(container)
            .iter()
            .map(|child| {
                let (mut start, mut end) = (
                    child.span.start.offset as usize,
                    child.span.end.offset as usize,
                );
                if start >= end || end > len || self.chars[start] != '<' {
                    return None;
                }
                // Out through `(…)` and blanks to the hole's braces, if the
                // element is the whole hole.
                let (mut s, mut e) = (start, end);
                loop {
                    while s > 0 && self.chars[s - 1].is_whitespace() {
                        s -= 1;
                    }
                    while e < len && self.chars[e].is_whitespace() {
                        e += 1;
                    }
                    match (s.checked_sub(1).map(|i| self.chars[i]), self.chars.get(e)) {
                        (Some('('), Some(')')) => (s, e) = (s - 1, e + 1),
                        (Some('{'), Some('}')) => {
                            (start, end) = (s - 1, e + 1);
                            break;
                        }
                        _ => break,
                    }
                }
                Some(Item {
                    start,
                    end,
                    comma: None,
                    colon: None,
                    value_start: start,
                })
            })
            .collect()
    }

    /// Whether a child element has its line(s) to itself: nothing before it
    /// on its line, and after it nothing, or a `//` note. (Such a note is
    /// text, not a comment, but it is written as one about this element: it
    /// goes where the element goes.)
    fn child_own_line(&self, item: &Item) -> bool {
        self.starts_line(item.start) && {
            let rest = self.text(item.end, self.line_end_after(item.end));
            let rest = rest.trim();
            rest.is_empty() || rest.starts_with("//")
        }
    }

    /// The `//` note after a child element on its line, if it has the line
    /// otherwise to itself.
    fn child_tail(&self, item: &Item) -> String {
        if !self.child_own_line(item) {
            return String::new();
        }
        self.text(item.end, self.line_end_after(item.end))
            .trim_end()
            .to_string()
    }

    /// Where the `//` comment lines directly above the line `at` is on
    /// start (no blank line between): the start of that line when there are
    /// none.
    fn lead_start(&self, at: usize) -> usize {
        let mut start = self.line_start(at);
        while start > 0 {
            let above = self.line_start(start - 1);
            if !self.text(above, start).trim_start().starts_with("//") {
                break;
            }
            start = above;
        }
        start
    }

    /// Splices inserting a child element into `container` so it becomes
    /// child element `index`, laid out like its siblings: on its own line at
    /// their indentation when the sibling it goes next to has a line to
    /// itself, else next to it with the spacing between the siblings.
    fn insert_child(
        &self,
        container: &Expr,
        index: usize,
        what: &What,
    ) -> Result<Vec<Splice>, EditError> {
        let located = || EditError::invalid("the element there could not be located in the source");
        let tag = self.tag(container).ok_or_else(located)?;
        let items = self.child_items(container).ok_or_else(located)?;
        let kids = element_children(container);
        if index > items.len() {
            return Err(EditError::invalid("insert index is past the end"));
        }
        let elem = |indent: &str, model: Option<&Expr>| match what {
            What::Text { text, indent: from, .. } => reindent(text, from, indent),
            What::Value(value) => self.render(value, model, indent),
        };
        // The element on its own line(s) at `indent`, its comments above it.
        let block = |indent: &str, model: Option<&Expr>| {
            format!("{}{indent}{}", what.comment_lines(indent, true), elem(indent, model))
        };
        // The element sharing a line, its comments as `{/* … */}` holes.
        let inline = |indent: &str, model: Option<&Expr>| {
            format!("{}{}", what.inline_comments(true, true), elem(indent, model))
        };
        let at = |at: usize, text: String| Splice {
            start: at,
            end: at,
            text,
        };
        let outer = self.line_indent(container.span.start.offset as usize);

        let Some(near) = items.get(index.saturating_sub(1)) else {
            // No child elements yet: the first goes on a line of its own,
            // one step in, as `to_source` writes children.
            let inner = format!("{outer}{}", self.file_style().step);
            let body = block(&inner, None);
            let Some(close) = tag.close else {
                let own_line = self.starts_line(tag.open_end.start);
                let mut before = tag.open_end.start;
                if !own_line {
                    while before > tag.name_end && is_blank(self.chars[before - 1]) {
                        before -= 1;
                    }
                }
                return Ok(vec![Splice {
                    start: before,
                    end: container.span.end.offset as usize,
                    text: format!(">\n{body}\n{outer}</{}>", tag.name),
                }]);
            };
            let inside = self.text(tag.open_end.end, close);
            return Ok(vec![if inside.trim().is_empty() {
                Splice {
                    start: tag.open_end.end,
                    end: close,
                    text: format!("\n{body}\n{outer}"),
                }
            } else if self.starts_line(close) {
                // Comments (or text) already inside: keep them, and add the
                // child below them.
                at(self.line_start(close), format!("{body}\n"))
            } else {
                at(close, inline(&inner, None))
            }]);
        };
        let model = kids.get(index.saturating_sub(1)).copied();
        let indent = self.line_indent(near.start);
        if self.child_own_line(near) {
            let text = format!("{}\n", block(&indent, model));
            if index == 0 {
                // Directly below the open tag (as a list's first element goes
                // directly below its `[`), but after any text there: the new
                // element goes between that text and the old first child.
                let below_open = self.line_end_after(tag.open_end.end);
                let line = self.line_start(near.start);
                return Ok(vec![at(self.after_text(below_open, line), text)]);
            }
            return Ok(vec![at(self.line_end_after(near.end), text)]);
        }
        let sep = items
            .windows(2)
            .map(|pair| self.text(pair[0].end, pair[1].start))
            .find(|gap| gap.chars().all(is_blank))
            .unwrap_or_default();
        Ok(vec![if index > 0 {
            at(near.end, format!("{sep}{}", inline(&indent, model)))
        } else {
            at(near.start, format!("{}{sep}", inline(&indent, model)))
        }])
    }

    /// The start of the first line in `from..to` (whole lines) after the
    /// last one holding text, skipping comment lines and blank ones: `from`
    /// when no line holds text. `to` when `from` is past it.
    fn after_text(&self, from: usize, to: usize) -> usize {
        if from >= to {
            return to;
        }
        let mut at = from;
        let mut pos = from;
        let mut in_block = false;
        while pos < to {
            let end = self.line_end_after(pos).min(to);
            let line = self.text(pos, end);
            let line = line.trim();
            if in_block {
                in_block = !line.contains("*/");
            } else if line.starts_with("/*") {
                in_block = !line.contains("*/");
            } else if !line.is_empty()
                && !line.starts_with("//")
                && !(line.starts_with("{/*") && line.ends_with("*/}"))
            {
                at = end;
            }
            pos = end;
        }
        at
    }

    /// Splices removing child element `index` of `container`: its whole
    /// line(s) when it has them to itself, else the element and the spacing
    /// between it and a sibling on the same line. Comments on other lines
    /// stay.
    fn remove_child(
        &self,
        container: &Expr,
        index: usize,
        lead: bool,
    ) -> Result<Vec<Splice>, EditError> {
        let items = self.child_items(container).ok_or_else(|| {
            EditError::invalid("the element there could not be located in the source")
        })?;
        let item = items
            .get(index)
            .ok_or_else(|| EditError::not_found("no child element at that index"))?;
        let cut = |start: usize, end: usize| Splice {
            start,
            end,
            text: String::new(),
        };
        if self.child_own_line(item) {
            let start = if lead {
                self.lead_start(item.start)
            } else {
                self.line_start(item.start)
            };
            return Ok(vec![cut(start, self.line_end_after(item.end))]);
        }
        let blank_between = |a: usize, b: usize| self.chars[a..b].iter().all(|&c| is_blank(c));
        if let Some(next) = items.get(index + 1).filter(|n| blank_between(item.end, n.start)) {
            return Ok(vec![cut(item.start, next.start)]);
        }
        if let Some(prev) = index
            .checked_sub(1)
            .map(|i| &items[i])
            .filter(|p| blank_between(p.end, item.start))
        {
            return Ok(vec![cut(prev.end, item.end)]);
        }
        Ok(vec![cut(item.start, item.end)])
    }

    // ── Locating sequences ───────────────────────────────────────────────

    /// The bracketed sequence `expr` is written as, if it is one.
    fn seq(&self, expr: &Expr) -> Option<Seq> {
        let count = match shape(expr) {
            Shape::Record(fields) => fields.len(),
            Shape::List(items) | Shape::Call(items) => items.len(),
            Shape::Element(..) | Shape::Leaf => return None,
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
                // A tie keeps what `to_source` writes: `{a: 1}` and `[1, 2]`.
                record_pad: if votes[0] > votes[1] { " " } else { "" }.into(),
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
            StaticValue::Element {
                tag,
                props,
                children,
            } => {
                let model = model.filter(|m| matches!(shape(m), Shape::Element(..)));
                self.render_element(tag, props, children, model, indent)
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
        if let (ExprKind::Element { tag, .. }, StaticValue::Element { tag: new_tag, .. }) =
            (&old.kind, new)
            && tag == new_tag
            && unique_attrs(shape(old))
        {
            return self.plan_element(bindings, old, segs, new, ops);
        }
        if matches!(shape(old), Shape::Element(..)) {
            return whole(ops);
        }
        // A container the tree cannot locate (it should not happen) is not
        // edited inside.
        if !matches!(shape(old), Shape::Leaf) && self.seq(old).is_none() {
            return whole(ops);
        }
        let mut steps = Vec::new();
        // The old elements' texts, for an element that only moved.
        let texts: Vec<What> = match self.seq(old) {
            Some(seq) => seq
                .items
                .iter()
                .map(|item| What::Text {
                    text: self.text(item.start, item.end),
                    tail: self.tail(item),
                    indent: self.line_indent(item.start),
                    lead: if self.own_line(item) {
                        self.text(self.lead_start(item.start), self.line_start(item.start))
                    } else {
                        String::new()
                    },
                })
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
        // keeps its text, and the comment lines directly above it go with it.
        let moves = pair_moves(&steps, &|i, j| {
            old_values[i].as_ref() == Some(new_values[j])
                && keys.as_ref().is_none_or(|keys| {
                    matches!(shape(old), Shape::Record(fields)
                        if matches!(&fields[i], RecordField::Named(k, _) if k == keys[j]))
                })
        });
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
                    let text = moves
                        .iter()
                        .find(|&&(to, _)| to == j)
                        .and_then(|&(_, i)| texts.get(i).cloned());
                    ops.push(Op::Insert {
                        segs: segs.clone(),
                        index: pos,
                        key: keys.as_ref().map(|keys| keys[j].to_string()),
                        what: text.unwrap_or_else(|| What::Value(new_values[j].clone())),
                    });
                    pos += 1;
                }
                Step::Remove(i) => ops.push(Op::Remove {
                    segs: segs.clone(),
                    index: pos,
                    lead: moves.iter().any(|&(_, from)| from == i),
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
        if let (Shape::Element(..), Some(key)) = (shape(container), key) {
            return self.insert_attr(container, index, key, what);
        }
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
            What::Text { text, indent: from, .. } => reindent(text, from, indent),
            What::Value(value) => {
                let body = self.render(value, model, indent);
                match key {
                    Some(key) => format!("{key}{colon}{body}"),
                    None => body,
                }
            }
        };
        let tail = match what {
            What::Text { tail, .. } => tail.as_str(),
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
                let lead = what.comment_lines(&inner, false);
                at(line, format!("{lead}{inner}{},{tail}\n", elem(&inner)))
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
            let lead = what.comment_lines(&indent, false);
            if index == 0 {
                let line = self.line_end_after(seq.open_end);
                return Ok(vec![at(
                    line,
                    format!("{lead}{indent}{},{tail}\n", elem(&indent)),
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
            let mut text = format!("{lead}{indent}{}{comma}{tail}\n", elem(&indent));
            if self.chars.get(line - 1) != Some(&'\n') {
                text.insert(0, '\n');
            }
            splices.push(at(line, text));
            return Ok(splices);
        }
        let sep = style.as_ref().map_or(", ", |s| s.sep.as_str());
        let lead = what.inline_comments(false, false);
        Ok(vec![if index > 0 {
            at(near.end, format!("{sep}{lead}{}", elem(&outer)))
        } else {
            at(near.start, format!("{lead}{}{sep}", elem(&outer)))
        }])
    }

    fn remove(
        &self,
        container: &Expr,
        index: usize,
        lead: bool,
    ) -> Result<Vec<Splice>, EditError> {
        if matches!(shape(container), Shape::Element(..)) {
            return self.remove_attr(container, index);
        }
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
            let start = if lead {
                self.lead_start(item.start)
            } else {
                self.line_start(item.start)
            };
            let mut cuts = vec![cut(
                start,
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

/// No attribute of the element is written twice.
fn unique_attrs(shape: Shape) -> bool {
    let Shape::Element(props) = shape else {
        return false;
    };
    props
        .iter()
        .enumerate()
        .all(|(i, (key, _))| props[..i].iter().all(|(other, _)| other != key))
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
pub(crate) fn float_like(f: f64, model: &str) -> String {
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
    // A file whose lines all end in CRLF gets CRLF in what is added too.
    let crlf = source.contains("\r\n") && !source.replace("\r\n", "").contains('\n');
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
            Op::Remove { segs, index, lead } => {
                splices = doc.remove(at(segs)?, *index, *lead)?;
                next += 1;
            }
            Op::ReplaceAttr { segs, index, value } => {
                splices = doc.replace_attr(at(segs)?, *index, value)?;
                next += 1;
            }
            Op::ReplaceChildren { segs, children } => {
                splices = doc.replace_children(at(segs)?, children)?;
                next += 1;
            }
            Op::InsertChild { segs, index, what } => {
                splices = doc.insert_child(at(segs)?, *index, what)?;
                next += 1;
            }
            Op::RemoveChild { segs, index, lead } => {
                splices = doc.remove_child(at(segs)?, *index, *lead)?;
                next += 1;
            }
        }
        if crlf {
            for splice in &mut splices {
                if splice.text.contains('\n') {
                    splice.text = splice.text.replace("\r\n", "\n").replace('\n', "\r\n");
                }
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
