//! Intents: edits stated against a document's *output*, and the resolution
//! ladder that turns one into a source edit.
//!
//! An [`Intent`] names an output element by its [`OutputKey`] and says what
//! should hold of it ("`#logo` has `x = 300`"). [`resolve`] finds where in
//! the program that can be made true, trying the most direct strategy first:
//!
//! 1. **Direct literal.** The field's value flows unchanged from one literal
//!    (`x={300}`, `let m = 96` … `x={m}`, a helper called with `x: 300`).
//!    Edit that literal.
//! 2. **Single site.** The field is not written, and the element literal that
//!    built the output built only it (fan-out 1). Add the attribute there,
//!    wherever that literal is.
//!
//! Anything else comes back as a [`Resolution`] that says why: a literal or
//! site shared by several outputs is [`Resolution::NeedsChoice`] (with the
//! edit that would change all of them, for the host to offer); a computed
//! value is [`Resolution::Unresolved`]; a literal in another file, or an
//! edit that does not do what was asked, is [`Resolution::Refused`].
//!
//! Every edit is verified by running the edited source: the intent must hold
//! in the new output, and no other output may change (an edit that changes
//! others too is reported as a choice, not made).

use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;

use crate::output_map::{FieldProvenance, LiteralSource, OutputKey, OutputNode, PathSeg, path_string};
use crate::run::{DocumentRun, Runner};
use crate::splice::{LiteralEdit, edit_span};

/// What the user wants to be true of the output.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Output `target` has `field` (dotted for a field inside a record prop:
    /// `style.fill`) equal to `value`.
    SetField { target: OutputKey, field: String, value: StaticValue },
    /// Output `target` has no `field`.
    RemoveField { target: OutputKey, field: String },
}

impl Intent {
    pub fn target(&self) -> &OutputKey {
        match self {
            Intent::SetField { target, .. } | Intent::RemoveField { target, .. } => target,
        }
    }

    pub fn field(&self) -> &str {
        match self {
            Intent::SetField { field, .. } | Intent::RemoveField { field, .. } => field,
        }
    }
}

/// Which rung of the ladder an edit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Step 1: the literal the field flows from.
    DirectLiteral,
    /// Step 2: the element literal that built the output, in place.
    SingleSite,
}

/// A verified source edit.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEdit {
    /// The whole new source.
    pub source: String,
    pub step: Step,
    /// The literal that was edited, in the old source.
    pub span: SourceSpan,
    /// What the edit does, for the user ("set `96` on line 1 to `120`").
    pub description: String,
}

/// One way to go on when an intent needs the user's say.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub label: String,
    /// The edit that makes the intent hold this way, verified to make it
    /// hold (but it changes other outputs too). `None` for a choice the host
    /// carries out some other way.
    pub edit: Option<ResolvedEdit>,
}

/// The outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Apply this edit (it is verified).
    Edit(ResolvedEdit),
    /// The intent already holds; nothing to do.
    AlreadyHolds,
    /// The intent can be satisfied, but only by changing other outputs too,
    /// or in a way the user should pick.
    NeedsChoice { reason: String, options: Vec<Choice> },
    /// No strategy of steps 1 and 2 applies (the value is computed, the
    /// element has no recorded origin).
    Unresolved { reason: String, at: Option<SourceSpan> },
    /// The edit would land outside what may be written, or did not do what
    /// was asked when run.
    Refused { reason: String },
}

/// Limits on where [`resolve`] may write.
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Allow edits outside the document's own file (the entry file). Off by
    /// default: such an edit is refused with a reason naming the file. The
    /// returned source is always the entry file's, so a host that turns this
    /// on must write other files itself; today this only changes the
    /// refusal into [`Resolution::Unresolved`].
    pub allow_other_files: bool,
}

/// Resolve `intent` against `run`, the latest run of `source` with `runner`.
pub fn resolve(runner: &Runner, run: &DocumentRun, intent: &Intent, opts: &ResolveOptions) -> Resolution {
    let Some(node) = run.map.find(intent.target()) else {
        return Resolution::Unresolved {
            reason: format!("the document has no output {}", intent.target()),
            at: None,
        };
    };
    let (head, rest) = split_field(intent.field());
    if let Some(r) = already_holds(run, node, intent) {
        return r;
    }
    let tag = &node.tag;
    let Some(site) = &node.site else {
        return Resolution::Unresolved {
            reason: format!("<{tag}> at {} was not built by an element literal, so it has no source to edit", path_string(&node.path)),
            at: None,
        };
    };
    let field = node.field(head);
    match (intent, field.map(|f| &f.provenance)) {
        // Step 1: the literal the value flows from.
        (Intent::SetField { value, .. }, Some(FieldProvenance::Literal(lit))) => {
            if lit.constant.is_some() && !rest.is_empty() {
                return Resolution::Refused {
                    reason: format!(
                        "`{head}` is {} (line {}), which has no field `{}`",
                        lit.constant.as_ref().map(StaticValue::to_source).unwrap_or_default(),
                        lit.span.start.line,
                        rest.trim_start_matches('.')
                    ),
                };
            }
            direct_literal(runner, run, node, intent, lit, LiteralEdit::Set { path: rest, value }, opts)
        }
        (Intent::RemoveField { .. }, Some(FieldProvenance::Literal(lit))) if !rest.is_empty() => {
            if lit.constant.is_some() {
                return Resolution::AlreadyHolds;
            }
            direct_literal(runner, run, node, intent, lit, LiteralEdit::Remove { path: rest }, opts)
        }
        (Intent::SetField { .. }, Some(FieldProvenance::Computed { at, reason })) => Resolution::Unresolved {
            reason: format!("{reason}; edit it by hand"),
            at: *at,
        },
        (Intent::RemoveField { .. }, Some(FieldProvenance::Computed { at, reason })) if !rest.is_empty() => {
            Resolution::Unresolved {
                reason: format!("{reason}; edit it by hand"),
                at: *at,
            }
        }
        // Step 2: on the element literal itself. Setting a field it does not
        // write, or removing an attribute.
        (Intent::SetField { value, .. }, None) => {
            let value = nest(rest, value.clone());
            let path = format!(".{head}");
            single_site(runner, run, node, intent, LiteralEdit::Set { path: &path, value: &value }, opts)
        }
        (Intent::RemoveField { .. }, _) => {
            let path = format!(".{head}");
            let _ = site;
            single_site(runner, run, node, intent, LiteralEdit::Remove { path: &path }, opts)
        }
    }
}

/// `style.fill` → (`style`, `.fill`).
fn split_field(field: &str) -> (&str, &str) {
    match field.find('.') {
        Some(i) => (&field[..i], &field[i..]),
        None => (field, ""),
    }
}

/// `value` under the record path `rest` (`.a.b`): `{a: {b: value}}`.
fn nest(rest: &str, value: StaticValue) -> StaticValue {
    rest.split('.')
        .filter(|s| !s.is_empty())
        .rev()
        .fold(value, |v, k| StaticValue::Record(vec![(k.to_string(), v)]))
}

fn already_holds(run: &DocumentRun, node: &OutputNode, intent: &Intent) -> Option<Resolution> {
    let el = run.node_value(node)?;
    let current = field_of(&el, intent.field());
    let holds = match intent {
        Intent::SetField { value, .. } => current.is_some_and(|c| same(c, value)),
        Intent::RemoveField { .. } => current.is_none(),
    };
    holds.then_some(Resolution::AlreadyHolds)
}

fn file_check(run: &DocumentRun, file: FileId, what: &str, opts: &ResolveOptions) -> Option<Resolution> {
    if file == FileId(0) {
        return None;
    }
    let name = petal::provenance::file_source(run.program(), file)
        .map(|(_, n)| n.to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("file {}", file.0));
    let reason = format!("{what} is written in {name}, not in this document; edit it there by hand");
    Some(if opts.allow_other_files { Resolution::Unresolved { reason, at: None } } else { Resolution::Refused { reason } })
}

fn literal_text(run: &DocumentRun, span: &SourceSpan) -> String {
    let t = petal::provenance::span_text(run.program(), span).unwrap_or_default();
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 40 { format!("{}…", t.chars().take(40).collect::<String>()) } else { t }
}

fn direct_literal(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    lit: &LiteralSource,
    edit: LiteralEdit<'_>,
    opts: &ResolveOptions,
) -> Resolution {
    let text = literal_text(run, &lit.span);
    let field = intent.field();
    if let Some(r) = file_check(run, lit.file(), &format!("`{field}`'s value `{text}`"), opts) {
        return r;
    }
    let source = match edit_span(&run.source, &lit.span, edit) {
        Ok(s) => s,
        Err(e) => return Resolution::Refused { reason: format!("could not edit `{text}` on line {}: {e}", lit.span.start.line) },
    };
    let description = match edit {
        LiteralEdit::Set { path, value } => {
            format!("set `{text}`{path} on line {} to `{}`", lit.span.start.line, value.to_source())
        }
        LiteralEdit::Remove { path } => format!("remove `{}` from `{text}` on line {}", path.trim_start_matches('.'), lit.span.start.line),
    };
    let edit = ResolvedEdit { source, step: Step::DirectLiteral, span: lit.span, description };
    finish(runner, run, node, intent, edit, lit.shared, &format!("`{text}` on line {}", lit.span.start.line))
}

fn single_site(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    edit: LiteralEdit<'_>,
    opts: &ResolveOptions,
) -> Resolution {
    let site = node.site.as_ref().expect("checked by resolve");
    let tag = &node.tag;
    let Some(span) = site.span else {
        return Resolution::Unresolved { reason: format!("<{tag}>'s element literal has no source position"), at: None };
    };
    if let Some(r) = file_check(run, span.file, &format!("<{tag}>"), opts) {
        return r;
    }
    let source = match edit_span(&run.source, &span, edit) {
        Ok(s) => s,
        Err(e) => return Resolution::Refused { reason: format!("could not edit <{tag}> on line {}: {e}", span.start.line) },
    };
    let description = match edit {
        LiteralEdit::Set { path, value } => {
            format!("set `{}` on <{tag}> (line {}) to `{}`", path.trim_start_matches('.'), span.start.line, value.to_source())
        }
        LiteralEdit::Remove { path } => {
            format!("remove `{}` from <{tag}> (line {})", path.trim_start_matches('.'), span.start.line)
        }
    };
    let edit = ResolvedEdit { source, step: Step::SingleSite, span, description };
    finish(runner, run, node, intent, edit, site.fan_out, &format!("<{tag}> on line {}", span.start.line))
}

/// Verify `edit`, and turn it into a resolution: the edit, a choice when it
/// (statically or by rerun) changes other outputs, or a refusal.
fn finish(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    intent: &Intent,
    edit: ResolvedEdit,
    shared: usize,
    what: &str,
) -> Resolution {
    match verify(runner, run, node, intent, &edit.source) {
        Verified::Fails(reason) => Resolution::Refused { reason },
        Verified::Holds { others } if shared > 1 || !others.is_empty() => {
            let reason = if shared > 1 {
                format!("{what} makes {shared} outputs, so changing it changes all of them")
            } else {
                format!("changing {what} also changes {}", others.join(", "))
            };
            let label = if shared > 1 { format!("Change all {shared}") } else { "Change them all".to_string() };
            Resolution::NeedsChoice { reason, options: vec![Choice { label, edit: Some(edit) }] }
        }
        Verified::Holds { .. } => Resolution::Edit(edit),
    }
}

/// The outcome of re-running an edited source.
#[derive(Debug, Clone, PartialEq)]
pub enum Verified {
    /// The intent holds in the new output. `others` lists the other outputs
    /// that changed too (empty when none did).
    Holds { others: Vec<String> },
    /// It does not hold, or the edited source does not run; why.
    Fails(String),
}

/// Run `new_source` and check that `intent` holds of the output `node` of
/// `before` names, and which other outputs changed.
pub fn verify(runner: &Runner, before: &DocumentRun, node: &OutputNode, intent: &Intent, new_source: &str) -> Verified {
    let after = match runner.run(new_source) {
        Ok(a) => a,
        Err(e) => return Verified::Fails(format!("the edited document does not run: {e}")),
    };
    // Setting `id` renames the output.
    let key = match intent {
        Intent::SetField { field, value, .. } if field == "id" => match value {
            StaticValue::Str(s) => OutputKey::Id(s.clone()),
            StaticValue::Int(n) => OutputKey::Id(n.to_string()),
            _ => OutputKey::Path(node.path.clone()),
        },
        Intent::RemoveField { field, .. } if field == "id" => OutputKey::Path(node.path.clone()),
        _ => node.key.clone(),
    };
    let Some(new_node) = after.map.find(&key) else {
        return Verified::Fails(format!("the edited document no longer has the output {key}"));
    };
    let Some(el) = after.node_value(new_node) else {
        return Verified::Fails("the edited output has no literal form".into());
    };
    let got = field_of(&el, intent.field());
    let ok = match intent {
        Intent::SetField { value, .. } => got.is_some_and(|g| same(g, value)),
        Intent::RemoveField { .. } => got.is_none(),
    };
    if !ok {
        let got = got.map(StaticValue::to_source).unwrap_or_else(|| "no value".into());
        let want = match intent {
            Intent::SetField { value, .. } => value.to_source(),
            Intent::RemoveField { .. } => "no value".into(),
        };
        return Verified::Fails(format!(
            "after the edit <{}>'s `{}` is {got}, not {want}; nothing was changed",
            node.tag,
            intent.field()
        ));
    }
    Verified::Holds { others: changed_others(before, &after, node, intent) }
}

/// The outputs other than `node` whose value differs between the runs,
/// described for the user, or a general note when the result changed in a
/// way no element accounts for.
fn changed_others(before: &DocumentRun, after: &DocumentRun, node: &OutputNode, intent: &Intent) -> Vec<String> {
    let mut out = Vec::new();
    // The target's other fields.
    if let (Some(mut want), Some(m)) = (shallow(before, node), after.map.find(&node.key)) {
        apply_intent(&mut want, intent);
        if let (StaticValue::Element { props: p1, .. }, Some(StaticValue::Element { props: p2, .. })) =
            (&want, shallow(after, m))
        {
            for (k, v) in p1 {
                let other = p2.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v);
                if !other.is_some_and(|o| same(o, v)) {
                    out.push(format!("`{k}` of the same <{}>", node.tag));
                }
            }
        }
    }
    for n in &before.map.nodes {
        if n.key == node.key || n.path == node.path {
            continue;
        }
        let Some(m) = after.map.find(&n.key) else {
            out.push(format!("<{}> {}", n.tag, n.key));
            continue;
        };
        if let (Some(a), Some(b)) = (shallow(before, n), shallow(after, m))
            && !same(&a, &b)
        {
            out.push(format!("<{}> {}", n.tag, n.key));
        }
    }
    if out.is_empty() {
        // Anything outside the elements (a project record, text).
        let (Some(mut want), Some(got)) = (
            before.static_outputs().into_iter().collect::<Option<Vec<_>>>(),
            after.static_outputs().into_iter().collect::<Option<Vec<_>>>(),
        ) else {
            return out;
        };
        if let Some(el) = node_at_mut(&mut want, &node.path) {
            apply_intent(el, intent);
        }
        if want.len() != got.len() || want.iter().zip(&got).any(|(a, b)| !same(a, b)) {
            out.push("other values in the result".into());
        }
    }
    out
}

/// An element node's own value: its tag, props and text, without its child
/// elements (they are nodes of their own).
fn shallow(run: &DocumentRun, n: &OutputNode) -> Option<StaticValue> {
    match run.node_value(n)? {
        StaticValue::Element { tag, props, children } => Some(StaticValue::Element {
            tag,
            props: props.into_iter().filter(|(_, v)| !holds_element(v)).collect(),
            children: children.into_iter().filter(|c| !holds_element(c)).collect(),
        }),
        _ => None,
    }
}

fn holds_element(v: &StaticValue) -> bool {
    match v {
        StaticValue::Element { .. } => true,
        StaticValue::List(xs) => xs.iter().any(holds_element),
        StaticValue::Record(fs) => fs.iter().any(|(_, v)| holds_element(v)),
        _ => false,
    }
}

fn node_at_mut<'a>(outputs: &'a mut [StaticValue], path: &[PathSeg]) -> Option<&'a mut StaticValue> {
    let (PathSeg::Index(first), rest) = path.split_first()? else { return None };
    let mut cur = outputs.get_mut(*first)?;
    for seg in rest {
        cur = match (seg, cur) {
            (PathSeg::Index(i), StaticValue::List(xs)) => xs.get_mut(*i)?,
            (PathSeg::Field(k), StaticValue::Record(fs)) => fs.iter_mut().rev().find(|(n, _)| n == k).map(|(_, v)| v)?,
            (PathSeg::Prop(k), StaticValue::Element { props, .. }) => {
                props.iter_mut().rev().find(|(n, _)| n == k).map(|(_, v)| v)?
            }
            (PathSeg::Child(i), StaticValue::Element { children, .. }) => children.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Make `intent` hold of the element value `el`.
fn apply_intent(el: &mut StaticValue, intent: &Intent) {
    let StaticValue::Element { props, .. } = el else { return };
    let parts: Vec<&str> = intent.field().split('.').collect();
    match intent {
        Intent::SetField { value, .. } => set_in(props, &parts, value.clone()),
        Intent::RemoveField { .. } => remove_in(props, &parts),
    }
}

fn set_in(fields: &mut Vec<(String, StaticValue)>, parts: &[&str], value: StaticValue) {
    let Some((k, rest)) = parts.split_first() else { return };
    let slot = fields.iter_mut().rev().find(|(n, _)| n == k);
    match (slot, rest.is_empty()) {
        (Some((_, v)), true) => *v = value,
        (Some((_, StaticValue::Record(inner))), false) => set_in(inner, rest, value),
        (Some((_, v)), false) => {
            let mut inner = Vec::new();
            set_in(&mut inner, rest, value);
            *v = StaticValue::Record(inner);
        }
        (None, true) => fields.push((k.to_string(), value)),
        (None, false) => {
            let mut inner = Vec::new();
            set_in(&mut inner, rest, value);
            fields.push((k.to_string(), StaticValue::Record(inner)));
        }
    }
}

fn remove_in(fields: &mut Vec<(String, StaticValue)>, parts: &[&str]) {
    let Some((k, rest)) = parts.split_first() else { return };
    if rest.is_empty() {
        fields.retain(|(n, _)| n != k);
    } else if let Some((_, StaticValue::Record(inner))) = fields.iter_mut().rev().find(|(n, _)| n == k) {
        remove_in(inner, rest);
    }
}

/// Field `field` (dotted) of element value `el`.
pub fn field_of<'a>(el: &'a StaticValue, field: &str) -> Option<&'a StaticValue> {
    let StaticValue::Element { props, .. } = el else { return None };
    let mut parts = field.split('.');
    let first = parts.next()?;
    let mut cur = props.iter().rev().find(|(k, _)| k == first).map(|(_, v)| v)?;
    for p in parts {
        let StaticValue::Record(fs) = cur else { return None };
        cur = fs.iter().rev().find(|(k, _)| k == p).map(|(_, v)| v)?;
    }
    Some(cur)
}

/// Whether two values are the same as far as an output is concerned: an
/// integer and a float of equal value are, a color and the `{r, g, b}`
/// record it is at run time are, and record fields may be in any order.
pub fn same(a: &StaticValue, b: &StaticValue) -> bool {
    use StaticValue as S;
    match (a, b) {
        (S::Int(x), S::Float(y)) | (S::Float(y), S::Int(x)) => (*x as f64) == *y,
        (S::Color { .. }, _) => same(&color_record(a), b),
        (_, S::Color { .. }) => same(a, &color_record(b)),
        (S::List(xs), S::List(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y)),
        (S::Record(xs), S::Record(ys)) => records_same(xs, ys),
        (S::Element { tag: t1, props: p1, children: c1 }, S::Element { tag: t2, props: p2, children: c2 }) => {
            t1 == t2 && records_same(p1, p2) && c1.len() == c2.len() && c1.iter().zip(c2).all(|(x, y)| same(x, y))
        }
        _ => a == b,
    }
}

fn records_same(xs: &[(String, StaticValue)], ys: &[(String, StaticValue)]) -> bool {
    let last = |fs: &[(String, StaticValue)], k: &str| fs.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let keys = |fs: &[(String, StaticValue)]| {
        let mut ks: Vec<String> = fs.iter().map(|(k, _)| k.clone()).collect();
        ks.sort();
        ks.dedup();
        ks
    };
    let (kx, ky) = (keys(xs), keys(ys));
    kx == ky && kx.iter().all(|k| matches!((last(xs, k), last(ys, k)), (Some(a), Some(b)) if same(&a, &b)))
}

fn color_record(c: &StaticValue) -> StaticValue {
    match c {
        StaticValue::Color { r, g, b, a } => {
            let mut fs = vec![
                ("r".to_string(), StaticValue::Int(*r as i64)),
                ("g".to_string(), StaticValue::Int(*g as i64)),
                ("b".to_string(), StaticValue::Int(*b as i64)),
            ];
            if let Some(a) = a {
                fs.push(("a".to_string(), StaticValue::Int(*a as i64)));
            }
            StaticValue::Record(fs)
        }
        other => other.clone(),
    }
}
