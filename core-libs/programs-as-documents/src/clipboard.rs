//! Copy and paste of output elements as source.
//!
//! [`copy`] takes an output element's source text: the element literal or
//! the call that builds it (whichever is the smallest code in the document
//! that builds this element alone and needs nothing local to a function),
//! or, when no such code exists (a loop, a library), the literal of its
//! value. Free-variable analysis of that text finds the names it reads from
//! the document's top-level bindings ([`FreeRef`]): `x={m}`,
//! `fill={tok.color.ink}`, a helper `hero(1)`.
//!
//! [`paste`] inserts the text into a document (the same or another) at an
//! output position, with one option per reference ([`PasteRef`]):
//!
//! - **CopyAsSource**: bring the binding (and the bindings it uses) along,
//!   written before the statement the paste lands in, if the target has no
//!   different binding of that name.
//! - **CopyAsValue**: inline the static value read at each use
//!   (`tok.color.ink` → `:1a1a1a`).
//! - **Rebind**: use the target's binding of that (or another) name.
//!
//! The default: a name the target binds the same way is kept; a constant is
//! copied as its value; anything else is an error naming the reference, for
//! the host to ask about. The paste is a structural insert, so it is
//! verified by rerun like any other edit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use petal::ast::{Expr, ExprKind, ExprVisitor, Literal, Stmt, StmtKind, walk_expr, walk_stmt};
use petal::rewrite::parse_ast;
use petal::source_map::{FileId, SourceSpan};
use petal::static_value::{StaticValue, static_values};

use crate::entries::first_line;
use crate::intent::Resolution;
use crate::output_map::{OutputKey, OutputNode};

use crate::run::{DocumentRun, Runner};
use crate::splice::{byte_range, dedent, line_indent};
use crate::structure::{Place, StructureIntent, StructureOptions, Template, resolve_structure_with};

/// What a clip's text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSource {
    /// The element literal as written.
    Literal,
    /// The call (or other entry) that builds the element.
    Call,
    /// The literal of the element's value: no code in the document builds
    /// it alone.
    Value,
}

/// What a name a clip reads is bound to in the copied document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A top-level binding with a static value.
    Constant,
    /// A top-level function.
    Function,
    /// A top-level binding whose value is computed.
    Computed,
}

/// One use of a name in a clip's text.
#[derive(Debug, Clone, PartialEq)]
pub struct RefUse {
    /// The byte range in the clip's text of the whole read: the name and
    /// the field and constant index reads after it (`tok.color.ink`).
    pub range: Range<usize>,
    /// The path read after the name (`.color.ink`, `[2]`, or empty).
    pub path: String,
    /// The static value read there, when the binding has one.
    pub value: Option<StaticValue>,
}

/// A top-level name of the copied document that a clip reads.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeRef {
    pub name: String,
    pub kind: RefKind,
    pub uses: Vec<RefUse>,
}

/// A top-level binding of the copied document, for
/// [`PasteRef::CopyAsSource`].
#[derive(Debug, Clone, PartialEq)]
pub struct ClipBinding {
    pub name: String,
    /// The whole statement, as written (later lines relative to the first).
    pub text: String,
    pub kind: RefKind,
    /// The value, for a constant.
    pub value: Option<StaticValue>,
    /// The document's other top-level names it reads.
    pub deps: Vec<String>,
}

/// A copied output element.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// The source text (later lines relative to the first).
    pub text: String,
    pub source: ClipSource,
    pub tag: String,
    /// The element's value in the copied run.
    pub value: Option<StaticValue>,
    /// The document's top-level names the text reads.
    pub refs: Vec<FreeRef>,
    /// The document's top-level bindings the references reach, directly or
    /// through each other, in source order.
    pub bindings: Vec<ClipBinding>,
}

impl Clip {
    pub fn reference(&self, name: &str) -> Option<&FreeRef> {
        self.refs.iter().find(|r| r.name == name)
    }
}

/// Copy output `key` of `run` as source.
pub fn copy(run: &DocumentRun, key: &OutputKey) -> Result<Clip, String> {
    let node = run.node(key).ok_or_else(|| format!("the document has no output {key}"))?;
    let value = run.node_value(node);
    let (_, stmts) = parse_ast(&run.source)?;
    let doc = DocBindings::new(&run.source, &stmts);
    for (span, source) in candidates(run, node) {
        let Some(r) = byte_range(&run.source, &span) else { continue };
        let text = dedent(&run.source[r.clone()], &line_indent(&run.source, r.start));
        let Ok(uses) = free_uses(&text) else { continue };
        let locals = locals_around(&stmts, &span);
        if uses.iter().any(|u| locals.contains(&u.0)) {
            continue;
        }
        return Ok(clip_of(text, source, node.tag.clone(), value, &uses, &doc));
    }
    let v = value.clone().ok_or_else(|| format!("<{}> has no literal form", node.tag))?;
    Ok(Clip { text: v.to_source(), source: ClipSource::Value, tag: node.tag.clone(), value, refs: vec![], bindings: vec![] })
}

/// [`copy`] for text the host already has (a selection), read against the
/// document `source` it came from.
pub fn copy_text(source: &str, text: &str) -> Result<Clip, String> {
    let (_, stmts) = parse_ast(source)?;
    let doc = DocBindings::new(source, &stmts);
    let uses = free_uses(text)?;
    let tag = text.trim_start().strip_prefix('<').map(|t| t.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect()).unwrap_or_default();
    let mut clip = clip_of(text.to_string(), ClipSource::Literal, tag, None, &uses, &doc);
    if !text.trim_start().starts_with('<') {
        clip.source = ClipSource::Call;
    }
    Ok(clip)
}

/// The code that may be copied for `node`, smallest first: its element
/// literal when it built only this output, then each call on its path in
/// the document that led to only this output.
fn candidates(run: &DocumentRun, node: &OutputNode) -> Vec<(SourceSpan, ClipSource)> {
    let Some(site) = &node.site else { return vec![] };
    let mut out = Vec::new();
    if let Some(s) = site.span
        && s.file == FileId(0)
        && site.fan_out == 1
    {
        out.push((s, ClipSource::Literal));
    }
    for (call, span) in site.chain.iter().zip(&site.chain_spans) {
        let Some(s) = span else { continue };
        if s.file != FileId(0) {
            continue;
        }
        // Only this output (and what is inside it) comes through the call.
        let others = run.map.nodes.iter().any(|n| {
            !n.path.starts_with(&node.path) && n.site.as_ref().is_some_and(|s| s.chain.contains(call))
        });
        if !others {
            out.push((*s, ClipSource::Call));
        }
    }
    out
}

fn clip_of(
    text: String,
    source: ClipSource,
    tag: String,
    value: Option<StaticValue>,
    uses: &[(String, Range<usize>, String)],
    doc: &DocBindings,
) -> Clip {
    let mut refs: Vec<FreeRef> = Vec::new();
    for (name, range, path) in uses {
        let Some(b) = doc.get(name) else { continue };
        let v = b.value.as_ref().and_then(|v| read_path(v, path));
        let use_ = RefUse { range: range.clone(), path: path.clone(), value: v };
        match refs.iter_mut().find(|r| &r.name == name) {
            Some(r) => r.uses.push(use_),
            None => refs.push(FreeRef { name: name.clone(), kind: b.kind, uses: vec![use_] }),
        }
    }
    // The bindings the references reach.
    let mut reach: HashSet<String> = HashSet::new();
    let mut todo: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
    while let Some(n) = todo.pop() {
        if !reach.insert(n.clone()) {
            continue;
        }
        if let Some(b) = doc.get(&n) {
            todo.extend(b.deps.iter().cloned());
        }
    }
    let bindings = doc.order.iter().filter(|b| reach.contains(&b.name)).cloned().collect();
    Clip { text, source, tag, value, refs, bindings }
}

/// A document's top-level bindings, in source order.
struct DocBindings {
    order: Vec<ClipBinding>,
}

impl DocBindings {
    fn new(source: &str, stmts: &[Stmt]) -> DocBindings {
        let values: BTreeMap<String, StaticValue> = static_values(source).unwrap_or_default();
        let mut order: Vec<ClipBinding> = Vec::new();
        let names: HashSet<String> = stmts.iter().filter_map(top_name).collect();
        for s in stmts {
            let Some(name) = top_name(s) else { continue };
            let Some(r) = byte_range(source, &s.span) else { continue };
            let text = dedent(&source[r.clone()], &line_indent(source, r.start));
            let kind = match &s.kind {
                StmtKind::FnDecl { .. } => RefKind::Function,
                StmtKind::Let { value, .. } if matches!(value.kind, ExprKind::Lambda { .. }) => RefKind::Function,
                _ if values.contains_key(&name) => RefKind::Constant,
                _ => RefKind::Computed,
            };
            let deps = free_uses(&text)
                .map(|u| {
                    let mut d: Vec<String> =
                        u.into_iter().map(|(n, _, _)| n).filter(|n| *n != name && names.contains(n)).collect();
                    d.sort();
                    d.dedup();
                    d
                })
                .unwrap_or_default();
            let value = if kind == RefKind::Constant { values.get(&name).cloned() } else { None };
            // A later binding of the same name is the one the program uses.
            order.retain(|b| b.name != name);
            order.push(ClipBinding { name, text, kind, value, deps });
        }
        DocBindings { order }
    }

    fn get(&self, name: &str) -> Option<&ClipBinding> {
        self.order.iter().find(|b| b.name == name)
    }
}

fn top_name(s: &Stmt) -> Option<String> {
    match &s.kind {
        StmtKind::Let { name, .. } => Some(name.clone()),
        StmtKind::FnDecl { name, class: None, .. } => Some(name.clone()),
        _ => None,
    }
}

/// The static value at `path` (`.a.b[2]`) inside `v`.
fn read_path(v: &StaticValue, path: &str) -> Option<StaticValue> {
    let mut cur = v.clone();
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let n = r.find(['.', '[']).unwrap_or(r.len());
            let (f, after) = r.split_at(n);
            cur = match cur {
                StaticValue::Record(fs) => fs.into_iter().rev().find(|(k, _)| k == f)?.1,
                StaticValue::Color { r, g, b, a } => match f {
                    "r" => StaticValue::Int(r as i64),
                    "g" => StaticValue::Int(g as i64),
                    "b" => StaticValue::Int(b as i64),
                    "a" => StaticValue::Int(a? as i64),
                    _ => return None,
                },
                _ => return None,
            };
            rest = after;
        } else {
            let r = rest.strip_prefix('[')?;
            let n = r.find(']')?;
            let i: usize = r[..n].parse().ok()?;
            cur = match cur {
                StaticValue::List(xs) => xs.into_iter().nth(i)?,
                _ => return None,
            };
            rest = &r[n + 1..];
        }
    }
    Some(cur)
}

/// The names `text` reads that it does not bind itself, each with the byte
/// range of the whole read and the field/index path after the name.
fn free_uses(text: &str) -> Result<Vec<(String, Range<usize>, String)>, String> {
    let (_, stmts) = parse_ast(text)?;
    #[derive(Default)]
    struct V {
        binders: HashSet<String>,
        uses: Vec<(String, SourceSpan, String)>,
    }
    fn chain(e: &Expr) -> Option<(String, String)> {
        match &e.kind {
            ExprKind::Ident(n) => Some((n.clone(), String::new())),
            ExprKind::FieldAccess { object, field } => chain(object).map(|(n, p)| (n, format!("{p}.{field}"))),
            ExprKind::IndexAccess { object, index } => match &index.kind {
                ExprKind::Literal(Literal::Int(i)) => chain(object).map(|(n, p)| (n, format!("{p}[{i}]"))),
                _ => None,
            },
            _ => None,
        }
    }
    impl ExprVisitor for V {
        fn visit_expr(&mut self, e: &Expr) {
            if let Some((name, path)) = chain(e) {
                self.uses.push((name, e.span, path));
                return;
            }
            match &e.kind {
                // A method call: the receiver is read, not `recv.method`.
                ExprKind::Call { function, args, .. } if matches!(function.kind, ExprKind::FieldAccess { .. }) => {
                    if let ExprKind::FieldAccess { object, .. } = &function.kind {
                        self.visit_expr(object);
                    }
                    for a in args {
                        self.visit_expr(a);
                    }
                    return;
                }
                ExprKind::Lambda { params, .. } => {
                    self.binders.extend(params.iter().map(|p| p.name.clone()));
                }
                ExprKind::For { var, .. } => {
                    self.binders.insert(var.clone());
                }
                _ => {}
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::Let { name, .. } => {
                    self.binders.insert(name.clone());
                }
                StmtKind::FnDecl { name, params, .. } => {
                    self.binders.insert(name.clone());
                    self.binders.extend(params.iter().map(|p| p.name.clone()));
                }
                StmtKind::For { var, .. } => {
                    self.binders.insert(var.clone());
                }
                _ => {}
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V::default();
    for s in &stmts {
        v.visit_stmt(s);
    }
    let mut out = Vec::new();
    for (name, span, path) in v.uses {
        if v.binders.contains(&name) {
            continue;
        }
        let r = byte_range(text, &span).ok_or("a position outside the text")?;
        out.push((name, r, path));
    }
    Ok(out)
}

/// The names bound by the functions, lambdas and loops around `span` in
/// the document: text inside them may read their parameters and locals.
fn locals_around(stmts: &[Stmt], span: &SourceSpan) -> HashSet<String> {
    struct V<'s> {
        span: &'s SourceSpan,
        names: HashSet<String>,
    }
    fn around(outer: &SourceSpan, inner: &SourceSpan) -> bool {
        outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
    }
    fn bound_in(stmts: &[Stmt], names: &mut HashSet<String>) {
        struct L<'n>(&'n mut HashSet<String>);
        impl ExprVisitor for L<'_> {
            fn visit_stmt(&mut self, s: &Stmt) {
                if let StmtKind::Let { name, .. } = &s.kind {
                    self.0.insert(name.clone());
                }
                walk_stmt(self, s);
            }
        }
        let mut l = L(names);
        for s in stmts {
            l.visit_stmt(s);
        }
    }
    impl ExprVisitor for V<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if around(&e.span, self.span) {
                match &e.kind {
                    ExprKind::Lambda { params, body } => {
                        self.names.extend(params.iter().map(|p| p.name.clone()));
                        bound_in(body, &mut self.names);
                    }
                    ExprKind::For { var, .. } => {
                        self.names.insert(var.clone());
                    }
                    _ => {}
                }
            }
            walk_expr(self, e);
        }
        fn visit_stmt(&mut self, s: &Stmt) {
            if around(&s.span, self.span) {
                match &s.kind {
                    StmtKind::FnDecl { params, body, .. } => {
                        self.names.extend(params.iter().map(|p| p.name.clone()));
                        bound_in(body, &mut self.names);
                    }
                    StmtKind::For { var, .. } => {
                        self.names.insert(var.clone());
                    }
                    _ => {}
                }
            }
            walk_stmt(self, s);
        }
    }
    let mut v = V { span, names: HashSet::new() };
    for s in stmts {
        v.visit_stmt(s);
    }
    v.names
}

// ── paste ─────────────────────────────────────────────────────────────────

/// How one reference of a clip is pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteRef {
    /// Bring the binding (and the bindings it reads) along.
    CopyAsSource,
    /// Inline the static value read at each use.
    CopyAsValue,
    /// Read the target's binding of this name instead.
    Rebind(String),
}

/// How [`paste`] treats a clip's references and new elements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PasteOptions {
    /// Per reference name; a name not listed takes the default (see the
    /// module docs).
    pub refs: HashMap<String, PasteRef>,
    pub structure: StructureOptions,
}

/// A clip made ready for one target document.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedPaste {
    /// The text to insert.
    pub text: String,
    /// Top-level statements to add before the statement the paste lands in.
    pub preamble: Vec<String>,
    /// What was done with each reference.
    pub choices: Vec<(String, PasteRef)>,
}

/// Rewrite `clip`'s text for a document whose source is `target`, by
/// `opts`, or say which reference needs a choice and why.
pub fn prepare_paste(target: &str, clip: &Clip, opts: &PasteOptions) -> Result<PreparedPaste, String> {
    let (_, stmts) = parse_ast(target)?;
    let have = DocBindings::new(target, &stmts);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut preamble: Vec<String> = Vec::new();
    let mut choices = Vec::new();
    for r in &clip.refs {
        let theirs = have.get(&r.name);
        let ours = clip.bindings.iter().find(|b| b.name == r.name);
        let same_binding = match (theirs, ours) {
            (Some(t), Some(o)) => t.text == o.text || (t.value.is_some() && t.value == o.value),
            _ => false,
        };
        let constant = r.kind == RefKind::Constant && r.uses.iter().all(|u| u.value.is_some());
        let choice = match opts.refs.get(&r.name) {
            Some(c) => c.clone(),
            None if same_binding => PasteRef::Rebind(r.name.clone()),
            None if constant => PasteRef::CopyAsValue,
            None => {
                let what = match r.kind {
                    RefKind::Function => "a function",
                    RefKind::Computed => "computed",
                    RefKind::Constant => "read at a path with no static value",
                };
                let line = ours.map(|b| format!(" (`{}`)", first_line(&b.text))).unwrap_or_default();
                return Err(match theirs {
                    Some(_) => format!(
                        "`{}` is {what} in the copied document{line}, and the target binds `{}` differently: paste it with CopyAsSource under another name, or Rebind it",
                        r.name, r.name
                    ),
                    None => format!(
                        "`{}` is {what} in the copied document{line}: paste it with CopyAsSource (bring the binding along) or Rebind it to a name in the target",
                        r.name
                    ),
                });
            }
        };
        match &choice {
            PasteRef::CopyAsValue => {
                for u in &r.uses {
                    let v = u.value.as_ref().ok_or_else(|| {
                        format!("`{}{}` has no static value, so it cannot be copied as a value", r.name, u.path)
                    })?;
                    edits.push((u.range.clone(), v.to_source()));
                }
            }
            PasteRef::Rebind(to) => {
                if have.get(to).is_none() {
                    return Err(format!("the target has no top-level `{to}` to rebind `{}` to", r.name));
                }
                if to != &r.name {
                    for u in &r.uses {
                        edits.push((u.range.start..u.range.start + r.name.len(), to.clone()));
                    }
                }
            }
            PasteRef::CopyAsSource => {
                for b in closure(clip, &r.name) {
                    match have.get(&b.name) {
                        Some(t) if t.text == b.text => {}
                        Some(_) => {
                            return Err(format!(
                                "the target already has a different `{}`; Rebind `{}` or copy it as a value",
                                b.name, r.name
                            ));
                        }
                        None => {
                            if !preamble.contains(&b.text) {
                                preamble.push(b.text.clone());
                            }
                        }
                    }
                }
            }
        }
        choices.push((r.name.clone(), choice));
    }
    // Keep the brought bindings in the copied document's order.
    preamble.sort_by_key(|t| clip.bindings.iter().position(|b| &b.text == t));
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut text = clip.text.clone();
    for (r, with) in edits {
        text.replace_range(r, &with);
    }
    Ok(PreparedPaste { text, preamble, choices })
}

/// The bindings `name` needs, itself included.
fn closure<'c>(clip: &'c Clip, name: &str) -> Vec<&'c ClipBinding> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut todo = vec![name.to_string()];
    while let Some(n) = todo.pop() {
        if let Some(b) = clip.bindings.iter().find(|b| b.name == n)
            && seen.insert(&b.name)
        {
            todo.extend(b.deps.iter().cloned());
        }
    }
    clip.bindings.iter().filter(|b| seen.contains(b.name.as_str())).collect()
}

/// Paste `clip` into the document `run` is the latest run of, at `at`.
/// The result is a structural insert's: verified by rerun.
pub fn paste(runner: &Runner, run: &DocumentRun, clip: &Clip, at: Place, opts: &PasteOptions) -> Resolution {
    let prepared = match prepare_paste(&run.source, clip, opts) {
        Ok(p) => p,
        Err(reason) => return Resolution::Refused { reason },
    };
    // An id the target already has gets a fresh one, so the pasted element
    // is addressable on its own.
    let mut text = prepared.text;
    if let Some(id) = crate::structure::literal_id(&text) {
        let taken: HashSet<String> = run
            .map
            .nodes
            .iter()
            .filter_map(|n| match &n.key {
                OutputKey::Id(i) => Some(i.clone()),
                _ => None,
            })
            .collect();
        if taken.contains(&id) {
            let base = id.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('-');
            let base = if base.is_empty() { clip.tag.as_str() } else { base };
            let fresh = (1..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).expect("a free id");
            if let Ok(t) = crate::structure::set_id_in_text(&text, &fresh) {
                text = t;
            }
        }
    }
    let intent = StructureIntent::Insert { at, template: Template::Source(text) };
    let res = resolve_structure_with(runner, run, &intent, &opts.structure, &prepared.preamble);
    match res {
        Resolution::Edit(mut e) => {
            e.description = format!("paste <{}> ({})", clip.tag, e.description);
            Resolution::Edit(e)
        }
        other => other,
    }
}

