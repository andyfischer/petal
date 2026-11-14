//! Step 3 of the ladder: an output built by a shared site, and the scope an
//! edit to it takes.
//!
//! When the site (or literal) behind an output built several outputs, an
//! edit can take one of three scopes ([`Scope`]):
//!
//! - **Edit the shared site**: every instance changes.
//! - **Edit the data**: the value comes from one entry of a data list that a
//!   `map` or a `for` loop reads per item (`cards = rows.map(card)` with the
//!   title read from `rows[i].title`); edit that entry, so only this output
//!   changes. [`data_entry`] finds it by tracing the field back through the
//!   call chain and the loop.
//! - **Write an override** at the use site, in the host's own vocabulary
//!   (petal-design's instance overrides). The crate does not know any such
//!   vocabulary: a host supplies an [`OverrideHook`].
//!
//! [`resolve`](crate::resolve) offers the scopes that verify as
//! [`Choice`](crate::Choice)s, or picks one under a [`ScopePolicy`].

use petal::program::{Program, TermId, TermOp};
use petal::provenance::{self, TraceEnd};
use petal::source_map::{FileId, SourceSpan};

use crate::intent::Intent;
use crate::output_map::OutputNode;
use crate::run::DocumentRun;

/// The scope of an edit to an output whose site built others too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Edit the shared site or literal: every output it built changes.
    EditShared,
    /// Edit the data entry the output was built from.
    EditData,
    /// Write an override for this output at its use site (an
    /// [`OverrideHook`]).
    Override,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::EditShared => "edit-shared",
            Scope::EditData => "edit-data",
            Scope::Override => "override",
        }
    }
}

/// How [`resolve`](crate::resolve) goes on when an intent needs a scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ScopePolicy {
    /// Return every scope that verifies as a choice, for the user to pick.
    #[default]
    Ask,
    /// Take the first of these scopes that verifies and changes no other
    /// output (an [`EditShared`](Scope::EditShared) in the list is taken even
    /// though it changes the others: that is what it means).
    Prefer(Vec<Scope>),
}

/// Where an override for one output may be written: the call in the
/// document's own file that (through any helpers) built it.
#[derive(Debug, Clone, PartialEq)]
pub struct UseSite {
    /// The call term.
    pub call: TermId,
    pub span: SourceSpan,
    /// The name it calls, as written (`card`, `Button`).
    pub callee: String,
    /// Its text.
    pub text: String,
}

/// What an [`OverrideHook`] is asked to do.
pub struct OverrideRequest<'a> {
    pub run: &'a DocumentRun,
    pub node: &'a OutputNode,
    pub intent: &'a Intent,
    /// The innermost call in the document's file on the output's call path,
    /// when there is one.
    pub use_site: Option<UseSite>,
}

/// An override a hook wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct OverrideEdit {
    /// The whole new source.
    pub source: String,
    /// What it changed, in the old source.
    pub span: SourceSpan,
    /// For the user: "override `w` on `card(2)` (line 5)".
    pub description: String,
}

/// A host's way of writing a per-instance override (petal-design's
/// `overrides` on an instance, a named argument on a component call). The
/// resolver verifies whatever it writes, like any other edit.
pub trait OverrideHook {
    /// The source with an override that makes `req.intent` hold for
    /// `req.node` alone; `None` when this host has no override for it, an
    /// error to say why it cannot write one.
    fn write_override(&self, req: &OverrideRequest<'_>) -> Option<Result<OverrideEdit, String>>;
}

/// The innermost call on `node`'s call path that is written in the
/// document's own file.
pub fn use_site(run: &DocumentRun, node: &OutputNode) -> Option<UseSite> {
    let site = node.site.as_ref()?;
    let program = run.program();
    site.chain.iter().zip(&site.chain_spans).find_map(|(call, span)| {
        let span = (*span)?;
        if span.file != FileId(0) {
            return None;
        }
        let t = program.terms.get(call.0 as usize)?;
        if !matches!(t.op, TermOp::Call | TermOp::MethodCall { .. }) {
            return None;
        }
        Some(UseSite {
            call: *call,
            span,
            callee: callee_text(program, *call).unwrap_or_default(),
            text: provenance::span_text(program, &span)?,
        })
    })
}

fn callee_text(program: &Program, call: TermId) -> Option<String> {
    let t = program.terms.get(call.0 as usize)?;
    match &t.op {
        TermOp::MethodCall { name, .. } | TermOp::BuiltinCall(name) => {
            program.get_string_constant(*name).map(str::to_string)
        }
        _ => {
            let callee = *t.inputs.first()?;
            let text = provenance::term_text(program, callee)?;
            Some(text.rsplit('.').next().unwrap_or(&text).to_string())
        }
    }
}

/// The data entry an output's field was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct DataEntry {
    /// The list (or record) literal holding the data.
    pub literal: TermId,
    pub span: SourceSpan,
    /// The path to the value inside it (`[2].title`).
    pub path: String,
    /// The loop or `map` that read the list per item.
    pub reader: TermId,
}

/// The builtins that call their callback once per item of their first
/// argument (or receiver), in order, with the item as its first parameter.
const PER_ITEM: &[&str] = &["map", "each", "for_each"];

/// Trace field `prop` of `node` back through a `map` callback or a `for`
/// loop to the entry of the data list it was read from, or say why it
/// cannot be.
pub fn data_entry(run: &DocumentRun, node: &OutputNode, prop: &str) -> Result<DataEntry, String> {
    let program = run.program();
    let tag = &node.tag;
    let origin = run.env.element_origin(node.element).ok_or_else(|| format!("<{tag}> has no recorded origin"))?;
    let start = provenance::element_prop_term(program, origin.term, prop)
        .ok_or_else(|| format!("`{prop}` is not written on <{tag}>'s literal"))?;
    data_entry_from(run, node, start, &format!("`{prop}`"))
}

/// [`data_entry`] for the value of term `start` of the element literal that
/// built `node` (an attribute's or a child's input term); `what` names it in
/// the reasons ("`title`", "the text").
pub fn data_entry_from(run: &DocumentRun, node: &OutputNode, start: TermId, what: &str) -> Result<DataEntry, String> {
    let program = run.program();
    let tag = &node.tag;
    let prop = what;
    let origin = run.env.element_origin(node.element).ok_or_else(|| format!("<{tag}> has no recorded origin"))?;
    let chain: Vec<TermId> = origin.chain.to_vec();
    let mut cur = start;
    let iteration = iteration(run, node).ok_or("its iteration is unknown")?;
    let mut pos = 0usize;
    let mut path = String::new();
    let mut reader: Option<TermId> = None;
    for _ in 0..32 {
        let tr = provenance::trace_value(program, cur, &chain[pos.min(chain.len())..]);
        let end_chain = tr.end_chain.as_ref().ok_or("the value leaves every frame on its call path")?;
        pos = chain.len() - end_chain.len();
        if matches!(tr.end_kind, TraceEnd::Literal | TraceEnd::Constant(_)) {
            let Some(reader) = reader else {
                return Err(format!("{prop} is not read from a data list"));
            };
            if !matches!(tr.end_kind, TraceEnd::Literal) {
                return Err("the data is not a list or record literal".into());
            }
            let span = program.source_map.get(tr.end).copied().ok_or("the data literal has no source position")?;
            return Ok(DataEntry { literal: tr.end, span, path, reader });
        }
        let t = program.terms.get(tr.end.0 as usize).ok_or("bad term")?;
        match &t.op {
            TermOp::GetField(f) => {
                let f = program.get_string_constant(*f).ok_or("bad field")?;
                path = format!(".{f}{path}");
                cur = *t.inputs.first().ok_or("bad field read")?;
            }
            TermOp::GetIndex => {
                let ix = *t.inputs.get(1).ok_or("bad index read")?;
                let Some(petal::static_value::StaticValue::Int(i)) = provenance::constant_at(program, ix) else {
                    return Err(format!("{prop} is read at an index that is computed"));
                };
                path = format!("[{i}]{path}");
                cur = *t.inputs.first().ok_or("bad index read")?;
            }
            TermOp::Copy if t.inputs.is_empty() => {
                let (list, next, at) = per_item_source(program, tr.end, &chain, pos)
                    .ok_or_else(|| format!("{prop} is computed and not read from a data list"))?;
                if reader.is_some() {
                    return Err("the value is read through more than one loop".into());
                }
                reader = Some(at);
                path = format!("[{iteration}]{path}");
                cur = list;
                pos = next;
            }
            _ => {
                return Err(format!("{prop} is computed and not read from a data list"));
            }
        }
    }
    Err("the trace is too long".into())
}

/// When `phantom` is the item a per-item reader binds (a `map` callback's
/// first parameter, called by `chain[pos]`, or a `for` loop's variable):
/// the list term it reads, the chain position the list is in, and the
/// reader.
fn per_item_source(program: &Program, phantom: TermId, chain: &[TermId], pos: usize) -> Option<(TermId, usize, TermId)> {
    let t = program.terms.get(phantom.0 as usize)?;
    if let Some(f) = program.functions.iter().find(|f| f.body_block == t.block_id) {
        let name = t.name.as_deref()?;
        if f.params.iter().position(|p| p == name) != Some(0) {
            return None;
        }
        let &call = chain.get(pos)?;
        let c = program.terms.get(call.0 as usize)?;
        let callee = match &c.op {
            TermOp::BuiltinCall(n) | TermOp::MethodCall { name: n, .. } => program.get_string_constant(*n)?,
            _ => return None,
        };
        if !PER_ITEM.contains(&callee) {
            return None;
        }
        return Some((*c.inputs.first()?, pos + 1, call));
    }
    let block = program.blocks.iter().find(|b| b.id == t.block_id)?;
    let parent = block.parent_term_id?;
    let pt = program.terms.get(parent.0 as usize)?;
    if !matches!(pt.op, TermOp::ForLoop) {
        return None;
    }
    // `for i, x in xs`: only the item is an entry of `xs`.
    let bound: Vec<&str> = program
        .terms
        .iter()
        .filter(|o| o.block_id == t.block_id && matches!(o.op, TermOp::Copy) && o.inputs.is_empty())
        .filter(|o| !block.terms.contains(&o.id))
        .filter_map(|o| o.name.as_deref())
        .collect();
    if bound.len() > 1 && bound.last() != t.name.as_deref().as_ref() {
        return None;
    }
    Some((*pt.inputs.first()?, pos, parent))
}

/// Which iteration of its site and call path built `node`: how many
/// elements the same literal built before it under the same chain.
pub fn iteration(run: &DocumentRun, node: &OutputNode) -> Option<usize> {
    let o = run.env.element_origin(node.element)?;
    Some(
        run.env
            .element_origins()
            .filter(|(_, x)| x.term == o.term && x.chain == o.chain && x.occurrence < o.occurrence)
            .count(),
    )
}
