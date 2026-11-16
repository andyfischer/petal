//! An element's text ([`Intent::SetText`]), and whether a field is already
//! written as an expression ([`Intent::SetFieldExpr`]).
//!
//! The text of an output element is its text children. [`set_text`] finds
//! where they are written, through the same ladder as a field:
//!
//! 1. **Literal text** between the element literal's tags
//!    (`<text>Hello</text>`): the text is replaced there.
//! 2. **Single site**: the literal has no children yet (`<text />`), so
//!    text is added between new tags.
//! 3. **A `{…}` hole** that is the only child: its value is traced back
//!    ([`petal::provenance::trace_value`], through parameters and names) to
//!    the string literal it flows from (`hint(cx, "Hello")` … `{s}`), which
//!    is edited; or to the data entry a `map`/`for` read it from. A literal
//!    shared by other outputs is offered as a choice, like a field's.
//!
//! `<br />` children are line breaks: the text reads `\n` for each, and a
//! `\n` in the new text writes one (through a hole, a `\n` in the string).
//! A hole of an element written in another file (a package's helper) is
//! followed the same way: its children are read from that file, and only
//! the string or data entry the hole reads has to be in the document.
//!
//! An element with other child elements, or with text and holes mixed, has
//! no single place its text is written, and is refused with the reason.

use petal::ast::{Expr, ExprKind, ExprVisitor, JsxChild, walk_expr};
use petal::provenance::{self, TraceEnd};
use petal::rewrite::parse_ast;
use petal::source_map::{FileId, SourceSpan};
use petal::static_expr::StaticExpr;
use petal::static_value::StaticValue;

use crate::intent::{
    Content, DataEdit, Intent, Resolution, ResolveOptions, ResolvedEdit, Step, describe_edit, edit_and_verify,
    data_edit, file_check, finish_verified,
};
use crate::output_map::{FieldProvenance, OutputNode};
use crate::run::{DocumentRun, Runner};
use crate::scope::data_entry_from;
use crate::splice::{LiteralEdit, edit_span};

/// The expression written at `span` in `source` (the outermost one, when
/// several share the span), parsed.
pub(crate) fn expr_at(source: &str, span: &SourceSpan) -> Option<Expr> {
    struct Find<'s> {
        span: &'s SourceSpan,
        found: Option<Expr>,
    }
    impl ExprVisitor for Find<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if self.found.is_some() {
                return;
            }
            if e.span.start.offset == self.span.start.offset && e.span.end.offset == self.span.end.offset {
                self.found = Some(e.clone());
                return;
            }
            walk_expr(self, e);
        }
    }
    let (_, stmts) = parse_ast(source).ok()?;
    let mut f = Find { span, found: None };
    for s in &stmts {
        f.visit_stmt(s);
    }
    f.found
}

/// Whether `field` of `node` is already written as `expr`: on the element
/// literal that built it, or as the literal its value flows from.
pub(crate) fn written_as(run: &DocumentRun, node: &OutputNode, field: &str, expr: &StaticExpr) -> bool {
    if field.contains('.') {
        return false;
    }
    let same = |span: &SourceSpan| {
        span.file == FileId(0)
            && expr_at(&run.source, span).is_some_and(|e| StaticExpr::from_ast(&e).is_ok_and(|w| w == *expr))
    };
    if let Some(FieldProvenance::Literal(lit)) = node.field(field).map(|f| &f.provenance)
        && same(&lit.span)
    {
        return true;
    }
    let Some(span) = node.site.as_ref().and_then(|s| s.span).filter(|s| s.file == FileId(0)) else {
        return false;
    };
    let Some(el) = expr_at(&run.source, &span) else { return false };
    let ExprKind::Element { props, .. } = &el.kind else { return false };
    props
        .iter()
        .rev()
        .find(|(k, _)| k == field)
        .is_some_and(|(_, v)| StaticExpr::from_ast(v).is_ok_and(|w| w == *expr))
}

/// Resolve an [`Intent::SetText`] (see the [module docs](self)).
pub(crate) fn set_text(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    text: &str,
    opts: &ResolveOptions,
) -> Resolution {
    let tag = &node.tag;
    if crate::intent::text_of(&run.observed_node(node)).as_deref() == Some(text) {
        return Resolution::AlreadyHolds;
    }
    let Some(site) = &node.site else {
        return Resolution::Unresolved { reason: format!("<{tag}> was not built by an element literal, so its text has no source"), at: None };
    };
    let Some(span) = site.span else {
        return Resolution::Unresolved { reason: format!("<{tag}>'s element literal has no source position"), at: None };
    };
    let line = span.start.line;
    // The element's children are read from the file its literal is in: a
    // package's `<text>{title}</text>` is edited at the string the hole
    // reads, which may well be in the document.
    let in_document = span.file == FileId(0);
    let file_text = if in_document { Some(run.source.as_str()) } else { provenance::file_source(run.program(), span.file).map(|(s, _)| s) };
    let Some(el) = file_text.and_then(|src| expr_at(src, &span)) else {
        if let Some(r) = file_check(run, span.file, &format!("<{tag}>"), opts) {
            return r;
        }
        return Resolution::Unresolved { reason: format!("<{tag}> on line {line} could not be found in the source"), at: Some(span) };
    };
    // A literal written in another file is refused (unless other files may
    // be edited) before any reason that names a line in it.
    let other_file = || if in_document { None } else { file_check(run, span.file, &format!("<{tag}>"), opts) };
    let ExprKind::Element { children, .. } = &el.kind else {
        if let Some(r) = other_file() {
            return r;
        }
        return Resolution::Unresolved { reason: format!("<{tag}> on line {line} is not an element literal"), at: Some(span) };
    };
    if children.iter().any(|c| matches!(c, JsxChild::Expr(e) if matches!(e.kind, ExprKind::Element { .. }) && !is_line_break(e))) {
        if let Some(r) = other_file() {
            return r;
        }
        return Resolution::Unresolved {
            reason: format!("<{tag}> on line {line} has child elements, so its text is not one run of text; edit it by hand"),
            at: Some(span),
        };
    }
    let holes = children.iter().filter(|c| matches!(c, JsxChild::Expr(e) if !is_line_break(e))).count();
    let value = StaticValue::Str(text.to_string());
    // Text that `{"…"}` string holes write among literal text and `<br />`s
    // (as Petal writes text with `{` or `<` in it) is literal text too.
    let all_text = children.iter().all(|c| match c {
        JsxChild::Text(_) => true,
        JsxChild::Expr(e) => is_line_break(e) || matches!(e.kind, ExprKind::Literal(petal::ast::Literal::String(_))),
    });
    if holes == 0 || (all_text && children.len() > 1) {
        // Literal text (and `<br />`s) between the tags, or none yet.
        if let Some(r) = file_check(run, span.file, &format!("<{tag}>"), opts) {
            return r;
        }
        let new_children = text_children(text);
        let step = if children.is_empty() { Step::SingleSite } else { Step::DirectLiteral };
        let what = format!("<{tag}> on line {line}");
        let edit = LiteralEdit::SetChildren { path: "", children: &new_children };
        // An element written on one line keeps its text on that line:
        // `<text>One<br />two</text>`.
        let one_line = provenance::span_text(run.program(), &span).is_some_and(|t| !t.contains('\n'));
        let apply = |e: LiteralEdit<'_>| match one_line.then(|| inline_lines(&run.source, &span, text)).flatten() {
            Some(s) => Ok(s),
            None => edit_span(&run.source, &span, e),
        };
        let (source, verified) = match edit_and_verify(runner, run, node, intent, edit, &apply, &what) {
            Ok(s) => s,
            Err(e) => return Resolution::Refused { reason: format!("could not edit <{tag}> on line {line}: {e}") },
        };
        let description = describe_edit(edit, &format!("<{tag}>"), &format!("(line {line})"));
        let edit = ResolvedEdit { source, step, span, description, also_changes: Vec::new(), base: None };
        return finish_verified(runner, run, node, intent, edit, verified, site.fan_out, &what, opts);
    }
    if holes > 1 || children.len() > 1 {
        if let Some(r) = other_file() {
            return r;
        }
        return Resolution::Unresolved {
            reason: format!("<{tag}> on line {line} mixes text and `{{…}}` holes, so its text is not written in one place; edit it by hand"),
            at: Some(span),
        };
    }
    // One `{…}` hole: the string literal it reads, or its data entry.
    let program = run.program();
    let Some(origin) = run.env.element_origin(node.element) else {
        return Resolution::Unresolved { reason: format!("<{tag}> has no recorded origin"), at: Some(span) };
    };
    let Some(&term) = provenance::element_child_terms(program, origin.term).first() else {
        return Resolution::Unresolved { reason: format!("<{tag}> on line {line} has no child to edit"), at: Some(span) };
    };
    let trace = provenance::trace_value(program, term, &origin.chain);
    let hole_text = provenance::term_text(program, term).unwrap_or_default();
    if let TraceEnd::Constant(StaticValue::Str(_)) = &trace.end_kind
        && let Some(lit_span) = program.source_map.get(trace.end).copied()
    {
        let lit_line = lit_span.start.line;
        if let Some(r) = file_check(run, lit_span.file, &format!("the text of <{tag}>"), opts) {
            return r;
        }
        let what = format!("the string on line {lit_line}");
        let edit = LiteralEdit::Set { path: "", value: &value };
        let apply = |e: LiteralEdit<'_>| edit_span(&run.source, &lit_span, e);
        let (source, verified) = match edit_and_verify(runner, run, node, intent, edit, &apply, &what) {
            Ok(s) => s,
            Err(e) => return Resolution::Refused { reason: format!("could not edit {what}: {e}") },
        };
        let description = format!("set the string on line {lit_line} (the text of <{tag}>) to {}", value.to_source());
        let edit = ResolvedEdit { source, step: Step::DirectLiteral, span: lit_span, description, also_changes: Vec::new(), base: None };
        return finish_verified(runner, run, node, intent, edit, verified, 1, &what, opts);
    }
    match text_data_edit(runner, run, node, intent, opts) {
        Some(DataEdit::Refused(r)) => r,
        Some(DataEdit::Edit(e)) if e.also_changes.is_empty() => Resolution::Edit(e),
        Some(DataEdit::Edit(e)) => Resolution::NeedsChoice {
            reason: format!("changing {} also changes {}", e.description, e.also_changes.join(", ")),
            options: vec![crate::intent::Choice {
                label: "Change the data entry".into(),
                scope: crate::scope::Scope::EditData,
                edit: Some(e),
            }],
        },
        None if !in_document => file_check(run, span.file, &format!("<{tag}>"), opts).unwrap_or_else(|| Resolution::Unresolved {
            reason: format!("the text of <{tag}> is computed by `{{{hole_text}}}` on line {line}; edit it by hand"),
            at: Some(span),
        }),
        None => Resolution::Unresolved {
            reason: format!("the text of <{tag}> is computed by `{{{hole_text}}}` on line {line}; edit it by hand"),
            at: Some(span),
        },
    }
}

/// `source` with the element literal at `span` given the text `text`, which
/// has line breaks, written on the literal's line: the lines as bare text
/// with `<br />` between them. `None` when a line can't be bare text (it
/// would be a quoted hole) or `text` has no line break.
fn inline_lines(source: &str, span: &SourceSpan, text: &str) -> Option<String> {
    const MARK: char = '\u{E000}';
    if !text.contains('\n') || text.contains(MARK) || source.contains(MARK) {
        return None;
    }
    let joined = text.replace('\n', &MARK.to_string());
    let children = [StaticValue::Str(joined.clone())];
    let edited = edit_span(source, span, LiteralEdit::SetChildren { path: "", children: &children }).ok()?;
    let start = crate::splice::byte_range(source, span)?.start;
    let at = start + edited[start..].find(&joined)?;
    let bare = edited[..at].ends_with('>') && edited[at + joined.len()..].starts_with('<');
    bare.then(|| edited.replace(MARK, "<br />"))
}

/// Whether `e` is a line break, `<br />`: a `br` element with no attributes
/// or children.
fn is_line_break(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Element { tag, props, children } if tag == "br" && props.is_empty() && children.is_empty())
}

/// An element's children that read as `text`: a run of text per line, with
/// `<br />` between lines (an empty line has no run).
pub(crate) fn text_children(text: &str) -> Vec<StaticValue> {
    let mut out = Vec::new();
    for (i, l) in text.split('\n').enumerate() {
        if i > 0 {
            out.push(StaticValue::Element { tag: "br".into(), props: Vec::new(), children: Vec::new() });
        }
        if !l.is_empty() {
            out.push(StaticValue::Str(l.to_string()));
        }
    }
    out
}

/// The data-scope edit of an [`Intent::SetText`]: the entry of the data
/// list the element's one `{…}` child reads its text from.
pub(crate) fn text_data_edit(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    opts: &ResolveOptions,
) -> Option<DataEdit> {
    let Intent::SetText { text, .. } = intent else { return None };
    let program = run.program();
    let origin = run.env.element_origin(node.element)?;
    let terms = provenance::element_child_terms(program, origin.term);
    let [term] = terms else { return None };
    let entry = data_entry_from(run, node, *term, "the text").ok()?;
    let value = StaticValue::Str(text.clone());
    data_edit(runner, run, node, intent, &entry, "", Content::Value(&value), opts).ok()
}
