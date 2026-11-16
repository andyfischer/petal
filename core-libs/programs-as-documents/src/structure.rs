//! Structural edits stated as outcomes: "this list of outputs should read in
//! this order", "move this element into that group", insert, remove,
//! duplicate.
//!
//! A structural intent names outputs by their [`OutputKey`] and places by a
//! [`Parent`] (a list in the result, or an element's children) and an index
//! among the parent's elements. [`resolve_structure`] traces each element of
//! the parent back to the source entry that builds it ([`sibling_entries`]):
//!
//! - **An entry of a list or element literal** in the document (an element
//!   written there, a helper call such as `hero(1)`, any expression that is
//!   an item, an `if` that may give `nil`). The edit reorders, removes or
//!   inserts that literal's entries, keeping each kept entry's text as
//!   written ([`crate::entries`]). One entry that builds several outputs
//!   moves as a group.
//! - **An item of a data list** that a `map` (or `each`) callback or a `for`
//!   loop turns into outputs, in order. The edit reorders, removes, inserts
//!   or duplicates the *data* entries instead.
//! - **Locked**: built by code that is no entry of a literal in the document
//!   (a library's list in another file, a native). It keeps its place, and
//!   an edit that would move it is refused with the reason. So for a
//!   concatenation (`concat(LIBRARY, mine)`) only the segment from the
//!   document's own literal is edited.
//!
//! Every edit is verified by running the edited source: the parent must
//! read as asked (with new elements matched by tag, or exactly when their
//! value is known) and nothing else in the result may change.

use std::collections::HashSet;

use petal::program::{Program, TermId, TermOp};
use petal::provenance::{self, TraceEnd};
use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;

use crate::entries::{Container, ContainerKind, Slot, container_at, containers, entry_containing, entry_text, first_line, rewrite_entries};
use crate::intent::{Resolution, ResolvedEdit, Step, same};
use crate::output_map::{OutputKey, OutputNode, PathSeg, path_string};
use crate::run::{DocumentRun, Runner};
use crate::scope::iteration;
use crate::splice::byte_range;

/// A place in the result whose elements are in order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Parent {
    /// The list at this index path of the result (`[PathSeg::Index(0)]` for
    /// the first result value).
    List(Vec<PathSeg>),
    /// The children of this output element.
    Children(OutputKey),
}

/// A position among a parent's elements (text and other values among them
/// are not counted).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub parent: Parent,
    /// Where the element ends up: 0 is first, the number of elements is
    /// last.
    pub index: usize,
}

/// The source of a new entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Template {
    /// A value, written with [`StaticValue::to_source`]: an element for a
    /// literal's entries, the data entry for a data list.
    Value(StaticValue),
    /// Source text (a copied sibling, the clipboard), its later lines
    /// indented relative to its first.
    Source(String),
}

/// A structural edit, stated against the output.
#[derive(Debug, Clone, PartialEq)]
pub enum StructureIntent {
    /// The parent's elements read in this order (every one of them, once).
    Reorder { parent: Parent, order: Vec<OutputKey> },
    /// Output `target` ends up at `to`, in its own parent or another.
    Move { target: OutputKey, to: Place },
    /// A new element at `at`.
    Insert { at: Place, template: Template },
    /// Output `target` is gone.
    Remove { target: OutputKey },
    /// A copy of `target` right after it.
    Duplicate { target: OutputKey },
}

/// How structural edits are made.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructureOptions {
    /// Give each element the edit creates (insert, duplicate, paste) an
    /// `id=` of its own, so it stays addressable by
    /// [`OutputKey::Id`] across later hand edits. A duplicate whose
    /// original has an id always gets a fresh one when its text can take
    /// it.
    pub write_ids: bool,
}

/// Where one element of a parent comes from in the source.
#[derive(Debug, Clone, PartialEq)]
pub enum EntrySource {
    /// Entry `entry` of a list or element literal in the document. `group`
    /// is how many of the parent's elements that entry builds (more than
    /// one: they move together).
    Entry { container: Container, entry: usize, group: usize },
    /// Item `entry` of a data list a `map`/`each` callback or a `for` loop
    /// (the `reader`) turns into outputs.
    Data { list: Container, entry: usize, reader: TermId },
    /// Not editable here; `reason` says why.
    Locked { reason: String },
}

impl EntrySource {
    fn container(&self) -> Option<&Container> {
        match self {
            EntrySource::Entry { container, .. } => Some(container),
            EntrySource::Data { list, .. } => Some(list),
            EntrySource::Locked { .. } => None,
        }
    }

    fn entry(&self) -> Option<usize> {
        match self {
            EntrySource::Entry { entry, .. } | EntrySource::Data { entry, .. } => Some(*entry),
            EntrySource::Locked { .. } => None,
        }
    }

    pub fn is_movable(&self) -> bool {
        !matches!(self, EntrySource::Locked { .. })
    }
}

/// One element of a parent, and where it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct SiblingEntry {
    pub key: OutputKey,
    pub tag: String,
    /// Its index among the parent's elements.
    pub index: usize,
    pub source: EntrySource,
}

/// The parent output `key` is in, and its index there.
pub fn parent_of(run: &DocumentRun, key: &OutputKey) -> Option<(Parent, usize)> {
    let node = run.node(key)?;
    let path = &node.path;
    let n = path.len();
    // The run of list indexes the element is under: nested lists are
    // flattened, as a host that draws them does.
    let mut j = n;
    while j > 0 && matches!(path[j - 1], PathSeg::Index(_)) {
        j -= 1;
    }
    let j = j.max(1);
    let parent = match path.get(j - 1) {
        Some(PathSeg::Child(_)) => Parent::Children(run.map.at_path(&path[..j - 1])?.key.clone()),
        _ if j < n => Parent::List(path[..j].to_vec()),
        _ => return None,
    };
    let index = siblings(run, &parent).ok()?.iter().position(|n| n.key == *key)?;
    Some((parent, index))
}

/// The elements of `parent`, in order.
pub fn siblings<'a>(run: &'a DocumentRun, parent: &Parent) -> Result<Vec<&'a OutputNode>, String> {
    Ok(ParentInfo::new(run, parent)?.nodes)
}

/// Where each element of `parent` comes from: which can be moved, and why
/// the others cannot.
pub fn sibling_entries(run: &DocumentRun, parent: &Parent) -> Result<Vec<SiblingEntry>, String> {
    let info = ParentInfo::new(run, parent)?;
    let all = containers(&run.source)?;
    let sources = entry_sources(run, &all, &info);
    Ok(info
        .nodes
        .iter()
        .zip(sources)
        .enumerate()
        .map(|(index, (n, source))| SiblingEntry { key: n.key.clone(), tag: n.tag.clone(), index, source })
        .collect())
}

/// A parent, resolved against a run.
struct ParentInfo<'a> {
    /// The index path of the list, or of the parent element.
    path: Vec<PathSeg>,
    kind: ContainerKind,
    /// Its elements, in order.
    nodes: Vec<&'a OutputNode>,
    /// The parent element's own literal, when it has one in the document.
    literal: Option<SourceSpan>,
    describe: String,
}

impl<'a> ParentInfo<'a> {
    fn new(run: &'a DocumentRun, parent: &Parent) -> Result<ParentInfo<'a>, String> {
        let (path, kind, literal, describe) = match parent {
            Parent::List(path) => (path.clone(), ContainerKind::List, None, format!("the list at {}", path_string(path))),
            Parent::Children(key) => {
                let n = run.node(key).ok_or_else(|| format!("the document has no output {key}"))?;
                let literal = n.site.as_ref().and_then(|s| s.span).filter(|s| s.file == FileId(0));
                (n.path.clone(), ContainerKind::Children, literal, format!("<{}>'s children", n.tag))
            }
        };
        let nodes: Vec<&OutputNode> = run
            .map
            .nodes
            .iter()
            .filter(|n| slot_of(&path, kind, &n.path).is_some())
            .collect();
        if nodes.is_empty() && matches!(parent, Parent::List(_)) {
            let outputs: Vec<Option<StaticValue>> = run.observed_outputs().into_iter().map(Some).collect();
            if !matches!(value_at(&outputs, &path), Some(StaticValue::List(_))) {
                return Err(format!("the result has no list at {}", path_string(&path)));
            }
        }
        Ok(ParentInfo { path, kind, nodes, literal, describe })
    }
}

/// Where the element at `node_path` sits among the items of the parent at
/// `path` (the list's items, or the element's children), through nested
/// lists: the item indexes, outermost first. `None` when it is not one of
/// the parent's elements.
fn slot_of(path: &[PathSeg], kind: ContainerKind, node_path: &[PathSeg]) -> Option<Vec<usize>> {
    let rest = node_path.strip_prefix(path)?;
    let (first, more) = rest.split_first()?;
    let head = match (kind, first) {
        (ContainerKind::List, PathSeg::Index(i)) | (ContainerKind::Children, PathSeg::Child(i)) => *i,
        _ => return None,
    };
    let mut out = vec![head];
    for seg in more {
        match seg {
            PathSeg::Index(i) => out.push(*i),
            _ => return None,
        }
    }
    Some(out)
}

/// Where each of `info`'s elements comes from.
fn entry_sources(run: &DocumentRun, all: &[Container], info: &ParentInfo<'_>) -> Vec<EntrySource> {
    let entries: Vec<Result<(usize, usize), String>> =
        info.nodes.iter().map(|n| entry_of(run, all, n, info.literal.as_ref())).collect();
    entries
        .iter()
        .zip(&info.nodes)
        .map(|(e, n)| {
            let group = match e {
                Ok(hit) => entries.iter().filter(|x| x.as_ref().ok() == Some(hit)).count(),
                Err(_) => 0,
            };
            if let Ok((ci, ei)) = e
                && group == 1
            {
                return EntrySource::Entry { container: all[*ci].clone(), entry: *ei, group };
            }
            match data_of(run, all, n) {
                Ok((ci, entry, reader)) => EntrySource::Data { list: all[ci].clone(), entry, reader },
                Err(data_why) => match e {
                    // A `{…}` among an element's children that builds
                    // several of them is not one element's entry.
                    Ok((ci, ei)) if all[*ci].holes[*ei] => {
                        let span = all[*ci].entries[*ei];
                        let text = byte_range(&run.source, &span).map(|r| first_line(&run.source[r])).unwrap_or_default();
                        EntrySource::Locked {
                            reason: format!(
                                "<{}> comes from `{{{text}}}` (line {}), which builds {group} of {}'s elements; only a `{{…}}` that builds one element can be edited as an entry",
                                n.tag,
                                span.start.line,
                                all[*ci].describe()
                            ),
                        }
                    }
                    Ok((ci, ei)) => EntrySource::Entry { container: all[*ci].clone(), entry: *ei, group },
                    Err(why) => EntrySource::Locked {
                        reason: if data_why.is_empty() { why.clone() } else { format!("{why}, and {data_why}") },
                    },
                },
            }
        })
        .collect()
}

/// The entry of a literal in the document that builds `node`: the innermost
/// entry around its element literal or, failing that, around a call on its
/// call path.
fn entry_of(run: &DocumentRun, all: &[Container], node: &OutputNode, exclude: Option<&SourceSpan>) -> Result<(usize, usize), String> {
    let tag = &node.tag;
    let site = node
        .site
        .as_ref()
        .ok_or_else(|| format!("<{tag}> was built by a native or a builtin, not written in the document"))?;
    let spans: Vec<SourceSpan> = site.span.iter().chain(site.chain_spans.iter().flatten()).copied().collect();
    for s in spans.iter().filter(|s| s.file == FileId(0)) {
        if let Some(hit) = entry_containing(all, s, exclude) {
            return Ok(hit);
        }
    }
    if let Some(hit) = list_entry_by_flow(run, all, node) {
        return Ok(hit);
    }
    match spans.iter().find(|s| s.file == FileId(0)) {
        Some(s) => Err(format!(
            "<{tag}> is built by code on line {} that is not an entry of a list or of an element's children",
            s.start.line
        )),
        None => {
            let program = run.program();
            let file = site
                .span
                .and_then(|s| provenance::file_source(program, s.file))
                .map(|(_, name)| name.to_string())
                .unwrap_or_else(|| "another file".into());
            Err(format!("<{tag}> comes from `{file}`; only the document's own lists can be edited"))
        }
    }
}

/// The entry of a list literal in the document whose value is `node`'s
/// element, written elsewhere and reached through plain data flow: `clip`
/// in `[clip] ++ lines` where `let clip = <video … />`. Only when exactly
/// one entry of one list literal reads it (a name used in two lists is no
/// one entry's).
fn list_entry_by_flow(run: &DocumentRun, all: &[Container], node: &OutputNode) -> Option<(usize, usize)> {
    let program = run.program();
    let origin = run.env.element_origin(node.element)?;
    let mut hits = Vec::new();
    for (ti, t) in program.terms.iter().enumerate() {
        if !matches!(t.op, TermOp::AllocList) {
            continue;
        }
        let Some(span) = program.source_map.get(TermId(ti as u32)) else { continue };
        if span.file != FileId(0) {
            continue;
        }
        let Some(ci) = all.iter().position(|c| c.kind == ContainerKind::List && c.span.start.offset == span.start.offset) else {
            continue;
        };
        if all[ci].entries.len() != t.inputs.len() {
            continue;
        }
        for (k, input) in t.inputs.iter().enumerate() {
            if provenance::trace_value(program, *input, &[]).end == origin.term {
                hits.push((ci, k));
            }
        }
    }
    match hits.as_slice() {
        [hit] => Some(*hit),
        _ => None,
    }
}

/// The builtins that call their callback once per item of their first
/// argument, in order.
const PER_ITEM: &[&str] = &["map", "each", "for_each"];

/// The data list item `node` was built from: the list literal (a container
/// index), the item's index, and the `map` call or `for` loop that reads
/// the list. The innermost reader on the call path is taken.
fn data_of(run: &DocumentRun, all: &[Container], node: &OutputNode) -> Result<(usize, usize, TermId), String> {
    let program = run.program();
    let Some(origin) = run.env.element_origin(node.element) else {
        return Err(String::new());
    };
    let chain: Vec<TermId> = origin.chain.to_vec();
    let iteration = iteration(run, node).ok_or("its iteration is unknown")?;
    // Frame k holds the element term (k = 0) or the call chain[k - 1].
    for k in 0..=chain.len() {
        let term = if k == 0 { origin.term } else { chain[k - 1] };
        let found = if let Some(lp) = enclosing_for(program, term) {
            Some((lp, program.terms[lp.0 as usize].inputs.first().copied(), k))
        } else if let Some(&call) = chain.get(k)
            && per_item(program, call)
        {
            Some((call, program.terms[call.0 as usize].inputs.first().copied(), k + 1))
        } else {
            None
        };
        let Some((reader, list, ctx)) = found else { continue };
        let list = match program.terms[reader.0 as usize].op {
            // `for i in range(0, len(xs))`: the items of `xs`, in order.
            TermOp::NumericForLoop => counted_list(program, reader, &chain[ctx.min(chain.len())..])?,
            _ => list.ok_or("the loop reads no list")?,
        };
        let tr = provenance::trace_value(program, list, &chain[ctx.min(chain.len())..]);
        let what = provenance::term_text(program, list).map(|t| format!("`{}`", first_line(&t))).unwrap_or("its list".into());
        if !matches!(tr.end_kind, TraceEnd::Literal) {
            return Err(format!("the list it is made from, {what}, is computed, not a list written in the document"));
        }
        let span = program.source_map.get(tr.end).copied().ok_or("the data list has no source position")?;
        if span.file != FileId(0) {
            let file = provenance::file_source(program, span.file).map(|(_, n)| n.to_string()).unwrap_or_default();
            return Err(format!("its data list is in `{file}`; only the document's own lists can be edited"));
        }
        let ci = container_at(all, &span, ContainerKind::List).ok_or("the data list is not a list literal")?;
        if iteration >= all[ci].entries.len() {
            return Err(format!("{} has no entry {iteration}", all[ci].describe()));
        }
        return Ok((ci, iteration, reader));
    }
    Err(String::new())
}

/// The list a `for i in range(0, len(xs))` (or `range(len(xs))`) loop
/// counts the items of: `xs`. Its iterations are `xs`'s items in order, so
/// removing, reordering or duplicating an iteration is that edit on `xs`.
fn counted_list(program: &Program, lp: TermId, chain: &[TermId]) -> Result<TermId, String> {
    let t = &program.terms[lp.0 as usize];
    let (Some(&start), Some(&end)) = (t.inputs.first(), t.inputs.get(1)) else {
        return Err("the loop's range is unknown".into());
    };
    let text = |id: TermId| provenance::term_text(program, id).map(|t| format!("`{}`", first_line(&t))).unwrap_or("its end".into());
    if provenance::trace_value(program, start, chain).end_kind != TraceEnd::Constant(StaticValue::Int(0)) {
        return Err(format!("the loop's range starts at {}, not 0", text(start)));
    }
    let tr = provenance::trace_value(program, end, chain);
    let call = &program.terms[tr.end.0 as usize];
    let name = match &call.op {
        TermOp::BuiltinCall(n) | TermOp::MethodCall { name: n, .. } => program.get_string_constant(*n),
        _ => None,
    };
    match (name, call.inputs.first()) {
        (Some("len"), Some(&list)) => Ok(list),
        _ => Err(format!("the loop counts to {}, not to the length of a list written in the document", text(end))),
    }
}

fn per_item(program: &Program, call: TermId) -> bool {
    let Some(t) = program.terms.get(call.0 as usize) else { return false };
    match &t.op {
        TermOp::BuiltinCall(n) | TermOp::MethodCall { name: n, .. } => {
            program.get_string_constant(*n).is_some_and(|n| PER_ITEM.contains(&n))
        }
        _ => false,
    }
}

/// The `for` loop whose body `term` is in, within its own function.
fn enclosing_for(program: &Program, term: TermId) -> Option<TermId> {
    let mut block = program.terms.get(term.0 as usize)?.block_id;
    for _ in 0..64 {
        if program.functions.iter().any(|f| f.body_block == block) {
            return None;
        }
        let b = program.blocks.iter().find(|b| b.id == block)?;
        let parent = b.parent_term_id?;
        let pt = program.terms.get(parent.0 as usize)?;
        if matches!(pt.op, TermOp::ForLoop | TermOp::NumericForLoop) {
            return Some(parent);
        }
        block = pt.block_id;
    }
    None
}

// ── resolving ─────────────────────────────────────────────────────────────

/// What a new element is expected to be, for verification.
#[derive(Debug, Clone)]
enum Expect {
    Exact(StaticValue),
    Tag(String),
    Any,
}

/// A new entry: its text is chosen once its container is known.
#[derive(Debug, Clone)]
enum NewItem {
    Template(Template),
    /// An entry's text carried from elsewhere (a move between parents, a
    /// duplicate), and what it builds. `comment` is the comment that
    /// trailed it on its line in a list (`<x />, # note`), which goes with
    /// it as a comment line above it.
    Carry { text: String, expect: Expect, comment: Option<String>, data: bool },
}

impl NewItem {
    /// Whether this is an element (or other literal entry) rather than a
    /// data entry: next to a loop's elements it goes in the list the loop
    /// is an entry of, not in the loop's data.
    fn is_entry(&self) -> bool {
        match self {
            NewItem::Template(Template::Value(v)) => matches!(v, StaticValue::Element { .. }),
            NewItem::Template(Template::Source(t)) => element_tag_of(t).is_some(),
            NewItem::Carry { data, .. } => !data,
        }
    }
}

/// One element of a parent's desired sequence.
#[derive(Debug, Clone)]
enum Want {
    Old(usize),
    New(NewItem),
}

/// A parent and the sequence it should read as.
struct Desired<'a> {
    info: ParentInfo<'a>,
    sources: Vec<EntrySource>,
    /// Per element built from a data list: the entry of a list or element
    /// literal its loop (or `map`) is written as, when it is one
    /// (`[<a />, for t in tips do … end, <z />]`). That entry is one entry
    /// among the literal's others: a move across it moves past all the
    /// loop's elements.
    carriers: Vec<Option<(Container, usize)>>,
    want: Vec<Want>,
}

impl Desired<'_> {
    /// The container element `i` is an entry of in the source, for the
    /// order of entries: a loop's element counts as its carrier entry.
    /// `None` for one that is not an entry of a literal (locked, or a data
    /// item whose loop is not written as an entry).
    fn unit_container(&self, i: usize) -> Option<&Container> {
        match (&self.carriers[i], &self.sources[i]) {
            (Some((c, _)), _) => Some(c),
            (None, EntrySource::Entry { container, .. }) => Some(container),
            _ => None,
        }
    }
}

/// The entry of a literal in the document that the loop (or `map`) reading
/// a data list is written as, for each element built from a data list.
fn carriers_of(run: &DocumentRun, all: &[Container], sources: &[EntrySource]) -> Vec<Option<(Container, usize)>> {
    let program = run.program();
    sources
        .iter()
        .map(|s| {
            let EntrySource::Data { list, reader, .. } = s else { return None };
            let span = program.source_map.get(*reader).copied().filter(|s| s.file == FileId(0))?;
            let (ci, ei) = entry_containing(all, &span, None)?;
            // The data list's own entry (a loop inside a data item) is not
            // one.
            let e = all[ci].entries[ei];
            let inside_list = list.span.start.offset <= e.start.offset && e.end.offset <= list.span.end.offset;
            (!inside_list && all[ci].span != list.span).then(|| (all[ci].clone(), ei))
        })
        .collect()
}

/// Resolve a structural intent against `run`, the latest run of its source.
/// The edit comes back verified ([`Resolution::Edit`], with
/// [`Step::Entries`] or [`Step::DataList`]), or refused with the reason.
pub fn resolve_structure(runner: &Runner, run: &DocumentRun, intent: &StructureIntent, opts: &StructureOptions) -> Resolution {
    match plan(run, intent, opts) {
        Ok(p) => finish(runner, run, p, &[]),
        Err(reason) => Resolution::Refused { reason },
    }
    .stamped(&run.source)
}

/// [`resolve_structure`] with top-level statements to add before the
/// statement the edit lands in (bindings a paste brings along).
pub(crate) fn resolve_structure_with(
    runner: &Runner,
    run: &DocumentRun,
    intent: &StructureIntent,
    opts: &StructureOptions,
    preamble: &[String],
) -> Resolution {
    match plan(run, intent, opts) {
        Ok(p) => finish(runner, run, p, preamble),
        Err(reason) => Resolution::Refused { reason },
    }
    .stamped(&run.source)
}

struct Plan<'a> {
    parents: Vec<Desired<'a>>,
    opts: StructureOptions,
    /// Ids already in use, and the ones this edit writes.
    ids: HashSet<String>,
    what: String,
}

fn plan<'a>(run: &'a DocumentRun, intent: &StructureIntent, opts: &StructureOptions) -> Result<Plan<'a>, String> {
    let all = containers(&run.source)?;
    let desired = |parent: &Parent| -> Result<Desired<'a>, String> {
        let info = ParentInfo::new(run, parent)?;
        let sources = entry_sources(run, &all, &info);
        let want = (0..info.nodes.len()).map(Want::Old).collect();
        let carriers = carriers_of(run, &all, &sources);
        Ok(Desired { info, sources, carriers, want })
    };
    let locate = |key: &OutputKey| -> Result<(Parent, usize), String> {
        parent_of(run, key).ok_or_else(|| match run.node(key) {
            None => format!("the document has no output {key}"),
            Some(n) => format!("<{}> is not in a list or among an element's children", n.tag),
        })
    };
    let ids: HashSet<String> = run
        .map
        .nodes
        .iter()
        .filter_map(|n| match &n.key {
            OutputKey::Id(id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    let mut parents = Vec::new();
    let what;
    match intent {
        StructureIntent::Reorder { parent, order } => {
            let mut d = desired(parent)?;
            if order.len() != d.info.nodes.len() {
                return Err(format!(
                    "{} has {} elements, but the order names {}",
                    d.info.describe,
                    d.info.nodes.len(),
                    order.len()
                ));
            }
            let mut want = Vec::new();
            for k in order {
                let i = d.info.nodes.iter().position(|n| n.key == *k).ok_or_else(|| format!("{} has no output {k}", d.info.describe))?;
                if want.iter().any(|w| matches!(w, Want::Old(j) if *j == i)) {
                    return Err(format!("the order names {k} twice"));
                }
                want.push(Want::Old(i));
            }
            d.want = want;
            what = format!("reorder {}", d.info.describe);
            parents.push(d);
        }
        StructureIntent::Move { target, to } => {
            let (from, i) = locate(target)?;
            let mut src = desired(&from)?;
            let same_parent = resolve_parent_path(run, &from) == resolve_parent_path(run, &to.parent);
            if same_parent {
                src.want.remove(i);
                if to.index > src.want.len() {
                    return Err(format!("{} has no position {}", src.info.describe, to.index));
                }
                // Next to elements of another list (`xs ++ ys`): it moves
                // into that list.
                let neighbour = src.want[..to.index]
                    .iter()
                    .rev()
                    .chain(src.want[to.index..].iter())
                    .find_map(|w| match w {
                        Want::Old(j) => src.unit_container(*j).map(|c| c.span),
                        Want::New(_) => None,
                    });
                let carry = match &src.sources[i] {
                    EntrySource::Entry { container, entry, group: 1 } if neighbour.is_some_and(|n| n != container.span) => {
                        let text = entry_text(&run.source, &container.entries[*entry]).ok_or("the entry's position is outside the source")?;
                        let node = src.info.nodes[i];
                        let expect = run.node_value(node).map_or(Expect::Tag(node.tag.clone()), Expect::Exact);
                        let comment = carried_comment(&run.source, container, *entry);
                        Some(Want::New(NewItem::Carry { text, expect, comment, data: false }))
                    }
                    _ => None,
                };
                src.want.insert(to.index, carry.unwrap_or(Want::Old(i)));
                what = format!("move <{}> to position {} of {}", src.info.nodes[i].tag, to.index, src.info.describe);
                parents.push(src);
            } else {
                let node = src.info.nodes[i];
                if let Some(dest_path) = resolve_parent_path(run, &to.parent)
                    && dest_path.starts_with(&node.path)
                {
                    return Err(format!("<{}> cannot be moved inside itself", node.tag));
                }
                let (text, comment) = match &src.sources[i] {
                    EntrySource::Entry { container, entry, group: 1 } => (
                        entry_text(&run.source, &container.entries[*entry]).ok_or("the entry's position is outside the source")?,
                        carried_comment(&run.source, container, *entry),
                    ),
                    EntrySource::Entry { container, entry, .. } => {
                        return Err(group_reason(run, node, container, *entry));
                    }
                    EntrySource::Data { list, entry, .. } => {
                        return Err(format!(
                            "<{}> is built from entry {entry} of the data list on line {}; it can only be moved within its own list",
                            node.tag, list.span.start.line
                        ));
                    }
                    EntrySource::Locked { reason } => return Err(reason.clone()),
                };
                let expect = run.node_value(node).map_or(Expect::Tag(node.tag.clone()), Expect::Exact);
                src.want.remove(i);
                let mut dest = desired(&to.parent)?;
                if to.index > dest.want.len() {
                    return Err(format!("{} has no position {}", dest.info.describe, to.index));
                }
                dest.want.insert(to.index, Want::New(NewItem::Carry { text, expect, comment, data: false }));
                what = format!("move <{}> from {} to {}", node.tag, src.info.describe, dest.info.describe);
                parents.push(src);
                parents.push(dest);
            }
        }
        StructureIntent::Insert { at, template } => {
            let mut d = desired(&at.parent)?;
            if at.index > d.want.len() {
                return Err(format!("{} has no position {}", d.info.describe, at.index));
            }
            d.want.insert(at.index, Want::New(NewItem::Template(template.clone())));
            what = format!("insert into {}", d.info.describe);
            parents.push(d);
        }
        StructureIntent::Remove { target } => {
            let (parent, i) = locate(target)?;
            let mut d = desired(&parent)?;
            let node = d.info.nodes[i];
            match &d.sources[i] {
                EntrySource::Locked { reason } => return Err(reason.clone()),
                EntrySource::Entry { container, entry, group } if *group > 1 => {
                    return Err(group_reason(run, node, container, *entry));
                }
                _ => {}
            }
            d.want.remove(i);
            what = format!("remove <{}>", node.tag);
            parents.push(d);
        }
        StructureIntent::Duplicate { target } => {
            let (parent, i) = locate(target)?;
            let mut d = desired(&parent)?;
            let node = d.info.nodes[i];
            let (container, entry) = match &d.sources[i] {
                EntrySource::Locked { reason } => return Err(reason.clone()),
                EntrySource::Entry { container, entry, group } if *group > 1 => {
                    return Err(group_reason(run, node, container, *entry));
                }
                s => (s.container().expect("movable").clone(), s.entry().expect("movable")),
            };
            let text = entry_text(&run.source, &container.entries[entry]).ok_or("the entry's position is outside the source")?;
            let value = run.node_value(node);
            let explicit = matches!(node.key, OutputKey::Id(_));
            let data = matches!(d.sources[i], EntrySource::Data { .. });
            let mut text = text;
            let mut expect = value.clone().map_or(Expect::Tag(node.tag.clone()), Expect::Exact);
            if explicit || opts.write_ids {
                let fresh = fresh_id(&ids, node);
                match set_id_in_text(&text, &fresh) {
                    Ok(t) => {
                        text = t;
                        expect = match (&value, data) {
                            (Some(v), false) => Expect::Exact(with_id(v.clone(), &fresh)),
                            _ => Expect::Tag(node.tag.clone()),
                        };
                    }
                    Err(_) if explicit => {
                        let OutputKey::Id(id) = &node.key else { unreachable!() };
                        return Err(format!(
                            "the copy of <{}> would have the same id `{id}`, and `{}` (line {}) has no `id` to change",
                            node.tag,
                            first_line(&text),
                            container.entries[entry].start.line
                        ));
                    }
                    Err(_) => {}
                }
            }
            if data && !matches!(expect, Expect::Exact(_)) {
                expect = Expect::Tag(node.tag.clone());
            }
            d.want.insert(i + 1, Want::New(NewItem::Carry { text, expect, comment: None, data }));
            what = format!("duplicate <{}>", node.tag);
            parents.push(d);
        }
    }
    Ok(Plan { parents, opts: opts.clone(), ids, what })
}

/// The comment trailing entry `entry` of a list on its line, which a
/// remove takes with the line, so a move into another container carries it.
fn carried_comment(source: &str, container: &Container, entry: usize) -> Option<String> {
    if container.kind != ContainerKind::List {
        return None;
    }
    crate::entries::trailing_comment(source, &container.entries[entry])
}

fn group_reason(run: &DocumentRun, node: &OutputNode, container: &Container, entry: usize) -> String {
    let span = container.entries[entry];
    let text = byte_range(&run.source, &span).map(|r| first_line(&run.source[r])).unwrap_or_default();
    let n = parent_of(run, &node.key)
        .and_then(|(p, _)| sibling_entries(run, &p).ok())
        .map(|es| {
            es.iter()
                .filter(|e| matches!(&e.source, EntrySource::Entry { container: c, entry: x, .. } if c.span == container.span && *x == entry))
                .count()
        })
        .unwrap_or(0);
    format!(
        "<{}> comes from `{text}` (line {}), which builds {n} of these elements; they move together",
        node.tag, span.start.line
    )
}

/// The index path a parent's elements are under.
fn resolve_parent_path(run: &DocumentRun, parent: &Parent) -> Option<Vec<PathSeg>> {
    match parent {
        Parent::List(p) => Some(p.clone()),
        Parent::Children(k) => run.node(k).map(|n| n.path.clone()),
    }
}

/// An id no output has: the original's with a counter (`logo-2`), or the
/// tag's (`card-1`).
fn fresh_id(taken: &HashSet<String>, node: &OutputNode) -> String {
    let base = match &node.key {
        OutputKey::Id(id) => id.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('-').to_string(),
        _ => node.tag.clone(),
    };
    let base = if base.is_empty() { node.tag.clone() } else { base };
    (1..).map(|n| format!("{base}-{n}")).find(|c| !taken.contains(c)).expect("some id is free")
}

fn with_id(v: StaticValue, id: &str) -> StaticValue {
    match v {
        StaticValue::Element { tag, mut props, children } => {
            match props.iter_mut().find(|(k, _)| k == "id") {
                Some(p) => p.1 = StaticValue::Str(id.into()),
                None => props.push(("id".into(), StaticValue::Str(id.into()))),
            }
            StaticValue::Element { tag, props, children }
        }
        other => other,
    }
}

/// `text` (an element or record literal) with its `id` set to `id`.
pub(crate) fn set_id_in_text(text: &str, id: &str) -> Result<String, String> {
    let t = text.trim_start();
    if !(t.starts_with('<') || t.starts_with('{')) {
        return Err("not an element or record literal".into());
    }
    let head = "let __programs_as_documents_copy = [\n";
    let tail = ",\n]\n";
    let scratch = format!("{head}{text}{tail}");
    let edited = petal::literal_edit::set_path(&scratch, "__programs_as_documents_copy[0].id", &StaticValue::Str(id.into()))
        .map_err(|e| e.message)?;
    edited
        .strip_prefix(head)
        .and_then(|t| t.strip_suffix(tail))
        .map(str::to_string)
        .ok_or_else(|| "the id edit changed more than the literal".into())
}

/// The text and expected value of a new item placed in `container`.
fn new_text(item: &NewItem, container: &Container, data: bool, opts: &StructureOptions, ids: &mut HashSet<String>) -> Result<(Slot, Expect), String> {
    match item {
        NewItem::Carry { text, expect, comment, .. } => {
            let text = match comment {
                // Among children a `#` line would be text: a comment hole.
                Some(c) if container.kind == ContainerKind::Children => format!("{{/* {} */}}\n{text}", c.replace("*/", "* /")),
                Some(c) => format!("# {c}\n{text}"),
                None => text.clone(),
            };
            Ok((Slot::New(text), expect.clone()))
        }
        NewItem::Template(Template::Value(v)) => {
            let is_el = matches!(v, StaticValue::Element { .. });
            if data && is_el {
                return Err(format!(
                    "the elements here are built from the data list on line {}; insert a data entry, not an element",
                    container.span.start.line
                ));
            }
            // A helper call goes among an element's children as a `{…}`.
            if container.kind == ContainerKind::Children && !is_el && !matches!(v, StaticValue::Call { .. }) {
                return Err(format!("only an element, or a call that builds one, can be a child of {}", container.describe()));
            }
            let mut v = v.clone();
            if is_el && opts.write_ids && !has_id(&v) {
                let tag = match &v {
                    StaticValue::Element { tag, .. } => tag.clone(),
                    _ => unreachable!(),
                };
                let id = (1..).map(|n| format!("{tag}-{n}")).find(|c| !ids.contains(c)).expect("free id");
                ids.insert(id.clone());
                v = with_id(v, &id);
            }
            let expect = if is_el { Expect::Exact(v.clone()) } else { Expect::Any };
            Ok((Slot::Value(v), expect))
        }
        NewItem::Template(Template::Source(text)) => {
            let tag = element_tag_of(text);
            if data && tag.is_some() {
                return Err(format!(
                    "the elements here are built from the data list on line {}; insert a data entry, not an element",
                    container.span.start.line
                ));
            }
            let mut text = text.clone();
            if let Some(tag) = &tag
                && opts.write_ids
                && !text_has_id(&text)
            {
                let id = (1..).map(|n| format!("{tag}-{n}")).find(|c| !ids.contains(c)).expect("free id");
                ids.insert(id.clone());
                text = set_id_in_text(&text, &id)?;
            }
            Ok((Slot::New(text), tag.map_or(Expect::Any, Expect::Tag)))
        }
    }
}

fn has_id(v: &StaticValue) -> bool {
    matches!(v, StaticValue::Element { props, .. } if props.iter().any(|(k, _)| k == "id"))
}

/// The `id` written as a string or integer literal on an element literal's
/// text.
pub(crate) fn literal_id(text: &str) -> Option<String> {
    let (_, stmts) = petal::rewrite::parse_ast(text).ok()?;
    let petal::ast::StmtKind::Expr(e) = &stmts.first()?.kind else { return None };
    let petal::ast::ExprKind::Element { props, .. } = &e.kind else { return None };
    let (_, v) = props.iter().find(|(k, _)| k == "id")?;
    match &v.kind {
        petal::ast::ExprKind::Literal(petal::ast::Literal::String(s)) => Some(s.clone()),
        petal::ast::ExprKind::Literal(petal::ast::Literal::Int(i)) => Some(i.to_string()),
        _ => None,
    }
}

fn text_has_id(text: &str) -> bool {
    petal::rewrite::parse_ast(text)
        .ok()
        .and_then(|(_, stmts)| match stmts.first().map(|s| &s.kind) {
            Some(petal::ast::StmtKind::Expr(e)) => match &e.kind {
                petal::ast::ExprKind::Element { props, .. } => Some(props.iter().any(|(k, _)| k == "id")),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or(false)
}

/// The tag of an element literal's text.
fn element_tag_of(text: &str) -> Option<String> {
    let t = text.trim_start().strip_prefix('<')?;
    let tag: String = t.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
    (!tag.is_empty()).then_some(tag)
}

/// A container's new entries, and which parents' wants it serves.
struct ContainerEdit {
    container: Container,
    data: bool,
    /// Per old entry index that is a unit of an edited parent: kept (true)
    /// or not.
    units: Vec<usize>,
    /// The desired unit sequence: kept entries and new texts.
    seq: Vec<Slot>,
}

fn finish(runner: &Runner, run: &DocumentRun, mut plan: Plan<'_>, preamble: &[String]) -> Resolution {
    match build(run, &mut plan) {
        Ok((source, step, span, expected)) => {
            let source = match add_preamble(&source, &run.source, &span, preamble) {
                Ok(s) => s,
                Err(reason) => return Resolution::Refused { reason },
            };
            if source == run.source {
                if expected == run.compared_outputs().into_iter().map(Some).collect::<Vec<_>>() {
                    return Resolution::AlreadyHolds;
                }
                return Resolution::Refused {
                    reason: format!(
                        "the elements are written in separate lists ({}), and an element cannot move between them by reordering; move it instead",
                        plan.parents
                            .iter()
                            .flat_map(|d| d.sources.iter().filter_map(|s| s.container().map(Container::describe)))
                            .fold(Vec::<String>::new(), |mut v, x| {
                                if !v.contains(&x) {
                                    v.push(x);
                                }
                                v
                            })
                            .join(", ")
                    ),
                };
            }
            let after = match runner.run(&source) {
                Ok(a) => a,
                Err(e) => return Resolution::Refused { reason: format!("the edited document does not run: {e}") },
            };
            let got: Vec<Option<StaticValue>> = after.compared_outputs().into_iter().map(Some).collect();
            if expected.len() != got.len() {
                return Resolution::Refused { reason: "the edited document has a different number of results".into() };
            }
            for (i, (e, g)) in expected.iter().zip(&got).enumerate() {
                let (Some(e), Some(g)) = (e, g) else { continue };
                if let Some(diff) = first_diff(e, g, &mut vec![PathSeg::Index(i)]) {
                    return Resolution::Refused {
                        reason: format!("the edit does not do what was asked when run ({diff}); nothing was changed"),
                    };
                }
            }
            Resolution::Edit(ResolvedEdit { source, step, span, description: plan.what.clone(), also_changes: Vec::new(), base: None })
        }
        Err(reason) => Resolution::Refused { reason },
    }
}

/// Make the edit: the new source, the step, the first edited container, and
/// the result the rerun must give (new elements as wildcards).
#[allow(clippy::type_complexity)]
fn build(run: &DocumentRun, plan: &mut Plan<'_>) -> Result<(String, Step, SourceSpan, Vec<Option<StaticValue>>), String> {
    let mut edits: Vec<ContainerEdit> = Vec::new();
    let mut expected: Vec<Option<StaticValue>> = run.compared_outputs().into_iter().map(Some).collect();
    // Deeper parents first, so a shallower parent's path is still valid.
    plan.parents.sort_by_key(|d| std::cmp::Reverse(d.info.path.len()));
    let mut ids = plan.ids.clone();
    for d in &plan.parents {
        check_locked(d)?;
        check_groups(run, d)?;
        check_carriers(run, d)?;
        // Each parent's wanted sequence, per container.
        let mut new_values: Vec<Result<usize, StaticValue>> = Vec::new();
        // Per wanted element: a new one written beside a loop's entry, in
        // the list the loop is an entry of.
        let mut beside: Vec<bool> = Vec::new();
        for (k, w) in d.want.iter().enumerate() {
            match w {
                Want::Old(i) => {
                    let s = &d.sources[*i];
                    new_values.push(Ok(*i));
                    beside.push(false);
                    let (Some(c), Some(e)) = (s.container(), s.entry()) else { continue };
                    let data = matches!(s, EntrySource::Data { .. });
                    let ce = edit_for(&mut edits, c, data);
                    if !ce.units.contains(&e) {
                        ce.units.push(e);
                    }
                    if !ce.seq.iter().any(|x| matches!(x, Slot::Keep(j) if *j == e)) {
                        ce.seq.push(Slot::Keep(e));
                    }
                    // The loop's own entry takes its place among its
                    // literal's entries where its first element is.
                    if let Some((cc, ce_i)) = &d.carriers[*i] {
                        let ce = edit_for(&mut edits, cc, false);
                        if !ce.units.contains(ce_i) {
                            ce.units.push(*ce_i);
                        }
                        if !ce.seq.iter().any(|x| matches!(x, Slot::Keep(j) if j == ce_i)) {
                            ce.seq.push(Slot::Keep(*ce_i));
                        }
                    }
                }
                Want::New(item) => {
                    let (c, data) = anchor_container(run, d, k, item.is_entry())?;
                    let (slot, mut expect) = new_text(item, &c, data, &plan.opts, &mut ids)?;
                    // Among an element's children, what is new must be an
                    // element (a `{…}` that builds one).
                    if c.kind == ContainerKind::Children && matches!(expect, Expect::Any) {
                        expect = Expect::Tag("*".into());
                    }
                    edit_for(&mut edits, &c, data).seq.push(slot);
                    beside.push(!data && d.carriers.iter().flatten().any(|(cc, _)| cc.span == c.span));
                    new_values.push(Err(match expect {
                        Expect::Exact(v) => v,
                        Expect::Tag(t) => wildcard(Some(&t)),
                        Expect::Any => wildcard(None),
                    }));
                }
            }
        }
        // Units of this parent that are no longer wanted are removed.
        for (s, carrier) in d.sources.iter().zip(&d.carriers) {
            if let Some((cc, ce_i)) = carrier {
                let ce = edit_for(&mut edits, cc, false);
                if !ce.units.contains(ce_i) {
                    ce.units.push(*ce_i);
                }
            }
            let (Some(c), Some(e)) = (s.container(), s.entry()) else { continue };
            let data = matches!(s, EntrySource::Data { .. });
            let ce = edit_for(&mut edits, c, data);
            if !ce.units.contains(&e) {
                ce.units.push(e);
            }
        }
        replace_siblings(&mut expected, d, new_values, &beside)?;
    }
    // Apply, the latest container in the source first.
    edits.sort_by_key(|e| std::cmp::Reverse(e.container.span.start.offset));
    let mut source = run.source.clone();
    let mut changed: Vec<&ContainerEdit> = Vec::new();
    for ce in &edits {
        let target = fill(ce);
        let next = rewrite_entries(&source, ce.container.span.start.offset, ce.container.kind, &target)
            .map_err(|e| format!("could not edit {}: {e}", ce.container.describe()))?;
        if next != source {
            changed.push(ce);
        }
        source = next;
    }
    let first = changed.last().copied().or(edits.last()).ok_or("nothing to edit")?;
    let step = if changed.iter().any(|c| c.data) { Step::DataList } else { Step::Entries };
    if step == Step::DataList {
        plan.what = format!("{} (in the data list on line {})", plan.what, first.container.span.start.line);
    } else {
        plan.what = format!("{} (in {})", plan.what, first.container.describe());
    }
    Ok((source, step, first.container.span, expected))
}

fn edit_for<'e>(edits: &'e mut Vec<ContainerEdit>, c: &Container, data: bool) -> &'e mut ContainerEdit {
    let i = match edits.iter().position(|e| e.container.span == c.span && e.container.kind == c.kind) {
        Some(i) => i,
        None => {
            edits.push(ContainerEdit { container: c.clone(), data, units: Vec::new(), seq: Vec::new() });
            edits.len() - 1
        }
    };
    &mut edits[i]
}

/// The container a new element at position `k` of `d`'s wanted sequence
/// goes in: the nearest earlier element's, else the nearest later one's,
/// else (an element's children with none that can be edited) the parent
/// element's own literal.
fn anchor_container(run: &DocumentRun, d: &Desired<'_>, k: usize, entry: bool) -> Result<(Container, bool), String> {
    let pick = |w: &Want| match w {
        Want::Old(i) => d.sources[*i].container().map(|c| (c.clone(), matches!(d.sources[*i], EntrySource::Data { .. }))),
        Want::New(_) => None,
    };
    // An element next to a loop's elements goes in the list the loop is an
    // entry of, beside it, not in the loop's data; and next to a loop that
    // is no entry (`[clip] ++ lines ++ [count]`), in the nearest literal.
    let pick_entry = |w: &Want| match w {
        Want::Old(i) => d.unit_container(*i).map(|c| (c.clone(), false)),
        Want::New(_) => None,
    };
    if entry
        && let Some(hit) = d.want[..k].iter().rev().find_map(pick_entry).or_else(|| d.want[k + 1..].iter().find_map(pick_entry))
    {
        return Ok(hit);
    }
    if let Some(hit) = d.want[..k].iter().rev().find_map(pick).or_else(|| d.want[k + 1..].iter().find_map(pick)) {
        return Ok(hit);
    }
    if let Some(span) = d.info.literal {
        let all = containers(&run.source)?;
        if let Some(c) = all.iter().find(|c| c.kind == ContainerKind::Children && c.span.start.offset == span.start.offset) {
            return Ok((c.clone(), false));
        }
    }
    let why = d.sources.iter().find_map(|s| match s {
        EntrySource::Locked { reason } => Some(format!(": {reason}")),
        _ => None,
    });
    Err(format!("nowhere in the document to write a new element of {}{}", d.info.describe, why.unwrap_or_default()))
}

/// A locked element keeps its place: the elements before it stay the same.
fn check_locked(d: &Desired<'_>) -> Result<(), String> {
    let olds: Vec<usize> = d
        .want
        .iter()
        .filter_map(|w| match w {
            Want::Old(i) => Some(*i),
            Want::New(_) => None,
        })
        .collect();
    let removed: Vec<usize> = (0..d.info.nodes.len()).filter(|i| !olds.contains(i)).collect();
    let before_old: Vec<usize> = (0..d.info.nodes.len()).filter(|i| !removed.contains(i)).collect();
    for (i, s) in d.sources.iter().enumerate() {
        let EntrySource::Locked { reason } = s else { continue };
        if removed.contains(&i) {
            return Err(reason.clone());
        }
        let a: HashSet<usize> = before_old.iter().take_while(|j| **j != i).copied().collect();
        let b: HashSet<usize> = olds.iter().take_while(|j| **j != i).copied().collect();
        if a != b {
            return Err(format!("<{}> cannot move or be moved past: {reason}", d.info.nodes[i].tag));
        }
    }
    Ok(())
}

/// The elements a loop written as an entry of a literal builds stay
/// together: an element can go before or after them all (the loop entry
/// moves as one entry among the literal's others), not between them, and
/// one of them cannot leave them.
fn check_carriers(run: &DocumentRun, d: &Desired<'_>) -> Result<(), String> {
    let olds: Vec<usize> = d
        .want
        .iter()
        .filter_map(|w| match w {
            Want::Old(i) => Some(*i),
            Want::New(_) => None,
        })
        .collect();
    for carrier in &d.carriers {
        let Some((cc, ce)) = carrier else { continue };
        let member = |j: usize| matches!(&d.carriers[j], Some((c, e)) if c.span == cc.span && e == ce);
        let pos: Vec<usize> = olds.iter().enumerate().filter(|(_, j)| member(**j)).map(|(p, _)| p).collect();
        let (Some(first), Some(last)) = (pos.first(), pos.last()) else { continue };
        if last - first + 1 == pos.len() {
            continue;
        }
        let span = cc.entries[*ce];
        let text = byte_range(&run.source, &span).map(|r| first_line(&run.source[r])).unwrap_or_default();
        return Err(format!(
            "the elements `{text}` (line {}) builds stay together, as it is one entry of {}: an element can go before or after them all, not between them, and one of them cannot leave them",
            span.start.line,
            cc.describe()
        ));
    }
    Ok(())
}

/// The elements one entry builds stay together and in order.
fn check_groups(run: &DocumentRun, d: &Desired<'_>) -> Result<(), String> {
    for (i, s) in d.sources.iter().enumerate() {
        let EntrySource::Entry { container, entry, group } = s else { continue };
        if *group < 2 {
            continue;
        }
        let members: Vec<usize> = (0..d.sources.len())
            .filter(|j| matches!(&d.sources[*j], EntrySource::Entry { container: c, entry: e, .. } if c.span == container.span && e == entry))
            .collect();
        let pos: Vec<usize> =
            members.iter().filter_map(|m| d.want.iter().position(|w| matches!(w, Want::Old(x) if x == m))).collect();
        let contiguous = pos.len() == members.len() && pos.windows(2).all(|w| w[1] == w[0] + 1);
        if !contiguous {
            return Err(group_reason(run, d.info.nodes[i], container, *entry));
        }
    }
    Ok(())
}

/// A container's full new entry sequence: the old entries that are not
/// units of an edited parent stay where they are; the unit slots, in order,
/// take the wanted kept entries; removed units go; new entries go next to
/// their wanted neighbour.
fn fill(ce: &ContainerEdit) -> Vec<Slot> {
    let n = ce.container.entries.len();
    let kept: Vec<usize> = ce
        .seq
        .iter()
        .filter_map(|s| match s {
            Slot::Keep(e) => Some(*e),
            _ => None,
        })
        .collect();
    let mut units: Vec<usize> = ce.units.clone();
    units.sort();
    let mut out: Vec<Slot> = Vec::new();
    let mut next_kept = kept.iter();
    for e in 0..n {
        if !units.contains(&e) {
            out.push(Slot::Keep(e));
        } else if kept.contains(&e) {
            out.push(Slot::Keep(*next_kept.next().expect("as many kept units as kept slots")));
        }
    }
    // New entries: after the kept entry before them in the wanted sequence,
    // or before the one after, or at the end.
    for (k, s) in ce.seq.iter().enumerate() {
        if !s.is_new() {
            continue;
        }
        let prev = ce.seq[..k].iter().rev().find_map(|s| match s {
            Slot::Keep(e) => Some(*e),
            _ => None,
        });
        let pos = match prev {
            Some(p) => out.iter().position(|x| *x == Slot::Keep(p)).map(|i| i + 1),
            None => {
                let next = ce.seq[k + 1..].iter().find_map(|s| match s {
                    Slot::Keep(e) => Some(*e),
                    _ => None,
                });
                match next {
                    Some(nx) => out.iter().position(|x| *x == Slot::Keep(nx)),
                    None => None,
                }
            }
        };
        // Several new entries in a row keep their order.
        let pos = pos.map(|p| {
            let mut p = p;
            while out.get(p).is_some_and(Slot::is_new) {
                p += 1;
            }
            p
        });
        let at = pos.unwrap_or(out.len());
        out.insert(at, s.clone());
    }
    out
}


// ── the expected result ───────────────────────────────────────────────────

const WILD: &str = "\u{1}programs-as-documents:any\u{1}";

fn wildcard(tag: Option<&str>) -> StaticValue {
    StaticValue::Str(format!("{WILD}{}", tag.unwrap_or("")))
}

fn value_at<'v>(outputs: &'v [Option<StaticValue>], path: &[PathSeg]) -> Option<&'v StaticValue> {
    let (first, rest) = path.split_first()?;
    let PathSeg::Index(i) = first else { return None };
    let mut cur = outputs.get(*i)?.as_ref()?;
    for seg in rest {
        cur = match (seg, cur) {
            (PathSeg::Index(i), StaticValue::List(xs)) => xs.get(*i)?,
            (PathSeg::Field(f), StaticValue::Record(fs)) => &fs.iter().rev().find(|(k, _)| k == f)?.1,
            (PathSeg::Prop(f), StaticValue::Element { props, .. }) => &props.iter().rev().find(|(k, _)| k == f)?.1,
            (PathSeg::Child(i), StaticValue::Element { children, .. }) => children.get(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn value_at_mut<'v>(outputs: &'v mut [Option<StaticValue>], path: &[PathSeg]) -> Option<&'v mut StaticValue> {
    let (first, rest) = path.split_first()?;
    let PathSeg::Index(i) = first else { return None };
    let mut cur = outputs.get_mut(*i)?.as_mut()?;
    for seg in rest {
        cur = match (seg, cur) {
            (PathSeg::Index(i), StaticValue::List(xs)) => xs.get_mut(*i)?,
            (PathSeg::Field(f), StaticValue::Record(fs)) => &mut fs.iter_mut().rev().find(|(k, _)| k == f)?.1,
            (PathSeg::Prop(f), StaticValue::Element { props, .. }) => &mut props.iter_mut().rev().find(|(k, _)| k == f)?.1,
            (PathSeg::Child(i), StaticValue::Element { children, .. }) => children.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Put `values` in place of `d`'s elements in the expected result: the
/// element slots (through nested lists) take them in order (`Ok(i)`: the
/// value element `i` has now, after deeper parents were edited), anything else
/// (text, `nil`) stays; extra values go after the last slot, and slots
/// left over are dropped.
fn replace_siblings(
    expected: &mut [Option<StaticValue>],
    d: &Desired<'_>,
    values: Vec<Result<usize, StaticValue>>,
    beside: &[bool],
) -> Result<(), String> {
    let slots: Vec<Vec<usize>> = d.info.nodes.iter().filter_map(|n| slot_of(&d.info.path, d.info.kind, &n.path)).collect();
    let current: Vec<StaticValue> = {
        let mut out = Vec::new();
        for n in &d.info.nodes {
            let v = value_at(expected, &n.path).cloned().unwrap_or_else(|| wildcard(Some(&n.tag)));
            out.push(v);
        }
        out
    };
    // Each value goes in the nested list (segment) its element was in, or,
    // for a new one, its neighbour's.
    let seg_of = |i: usize| slots.get(i).map(|p| p[..p.len() - 1].to_vec()).unwrap_or_default();
    let mut segs: Vec<(Vec<usize>, std::collections::VecDeque<StaticValue>)> = Vec::new();
    let mut value_segs: Vec<Vec<usize>> = Vec::new();
    for (k, v) in values.iter().enumerate() {
        let seg = match v {
            Ok(i) => seg_of(*i),
            Err(_) => values[..k]
                .iter()
                .rev()
                .chain(values[k + 1..].iter())
                .find_map(|w| {
                    let i = *w.as_ref().ok()?;
                    let seg = seg_of(i);
                    // Beside a loop's elements, in the list the loop is an
                    // entry of: the segment around the loop's.
                    Some(if beside.get(k) == Some(&true) && d.carriers[i].is_some() && !seg.is_empty() {
                        seg[..seg.len() - 1].to_vec()
                    } else {
                        seg
                    })
                })
                .unwrap_or_default(),
        };
        value_segs.push(seg.clone());
        let value = match v {
            Ok(i) => current[*i].clone(),
            Err(v) => v.clone(),
        };
        match segs.iter_mut().find(|(s, _)| *s == seg) {
            Some((_, q)) => q.push_back(value),
            None => segs.push((seg, [value].into())),
        }
    }
    let items: &mut Vec<StaticValue> = match value_at_mut(expected, &d.info.path) {
        Some(StaticValue::List(xs)) => xs,
        Some(StaticValue::Element { children, .. }) => children,
        _ => return Err(format!("{} is not a list in the result", d.info.describe)),
    };
    // An element's children with text between them (`<a /> // note`): the
    // text after an element is written after its entry, so it goes where
    // that element goes.
    let flat = slots.iter().all(|s| s.len() == 1);
    let texty = items.iter().enumerate().any(|(i, v)| {
        !slots.contains(&vec![i]) && !matches!(v, StaticValue::Str(t) if t.trim().is_empty())
    });
    if d.info.kind == ContainerKind::Children && flat && texty && !slots.is_empty() {
        let starts: Vec<usize> = slots.iter().map(|s| s[0]).collect();
        let old = std::mem::take(items);
        let first = starts.iter().copied().min().unwrap_or(0);
        let mut out: Vec<StaticValue> = old[..first].to_vec();
        let trailing = |i: usize| -> Vec<StaticValue> {
            let at = starts[i];
            old[at + 1..]
                .iter()
                .enumerate()
                .take_while(|(k, _)| !starts.contains(&(at + 1 + k)))
                .map(|(_, v)| v.clone())
                .collect()
        };
        for v in &values {
            match v {
                Ok(i) => {
                    out.push(current[*i].clone());
                    out.extend(trailing(*i));
                }
                Err(v) => out.push(v.clone()),
            }
        }
        *items = out;
        return Ok(());
    }
    if d.info.kind == ContainerKind::List && reorder_blocks(items, d, &slots, &values, &value_segs, &mut segs) {
        return Ok(());
    }
    refill(items, &[], &slots, &mut segs);
    if let Some((_, q)) = segs.iter_mut().find(|(s, _)| s.is_empty()) {
        items.extend(q.drain(..));
    }
    Ok(())
}

/// A list whose items are elements and the nested lists loops written as
/// its entries give (`[<a />, for t in tips do … end, <z />]`): each loop's
/// list moves as one item, where its first element is wanted. `false`
/// (nothing done) for any other shape.
fn reorder_blocks(
    items: &mut Vec<StaticValue>,
    d: &Desired<'_>,
    slots: &[Vec<usize>],
    values: &[Result<usize, StaticValue>],
    value_segs: &[Vec<usize>],
    segs: &mut [(Vec<usize>, std::collections::VecDeque<StaticValue>)],
) -> bool {
    use std::collections::{HashMap, VecDeque};
    let blocks: HashSet<usize> = d
        .carriers
        .iter()
        .zip(slots)
        .filter(|(c, s)| c.is_some() && s.len() == 2)
        .map(|(_, s)| s[0])
        .collect();
    if blocks.is_empty()
        || slots.iter().any(|s| s.len() > 2 || (s.len() == 2 && !blocks.contains(&s[0])))
        || value_segs.iter().any(|s| s.len() > 1 || (s.len() == 1 && !blocks.contains(&s[0])))
        || !(0..items.len()).all(|p| blocks.contains(&p) || slots.contains(&vec![p]))
    {
        return false;
    }
    let old = std::mem::take(items);
    let mut nested: HashMap<usize, StaticValue> = HashMap::new();
    for p in &blocks {
        let mut it = old[*p].clone();
        if let StaticValue::List(xs) = &mut it {
            refill(xs, &[*p], slots, segs);
            if xs.is_empty()
                && let Some((_, q)) = segs.iter_mut().find(|(s, _)| *s == [*p])
            {
                xs.extend(q.drain(..));
            }
        }
        nested.insert(*p, it);
    }
    let mut top: VecDeque<StaticValue> = segs.iter_mut().find(|(s, _)| s.is_empty()).map(|(_, q)| std::mem::take(q)).unwrap_or_default();
    // A loop none of whose elements is wanted (all removed) keeps its place
    // after the element that was before it.
    let referenced: HashSet<usize> = value_segs.iter().filter_map(|s| s.first().copied()).collect();
    let after = |out: &mut Vec<StaticValue>, nested: &mut HashMap<usize, StaticValue>, p: usize| {
        let mut q = p + 1;
        while blocks.contains(&q) && !referenced.contains(&q) {
            if let Some(v) = nested.remove(&q) {
                out.push(v);
            }
            q += 1;
        }
    };
    let slot_index = |i: usize| slots.get(i).map(|s| s[0]);
    let mut out = Vec::new();
    let mut lead = 0;
    while blocks.contains(&lead) && !referenced.contains(&lead) {
        if let Some(v) = nested.remove(&lead) {
            out.push(v);
        }
        lead += 1;
    }
    for (v, seg) in values.iter().zip(value_segs) {
        match seg.first() {
            None => {
                if let Some(x) = top.pop_front() {
                    out.push(x);
                }
                if let Ok(i) = v
                    && let Some(p) = slot_index(*i)
                {
                    after(&mut out, &mut nested, p);
                }
            }
            Some(p) => {
                if let Some(x) = nested.remove(p) {
                    out.push(x);
                    after(&mut out, &mut nested, *p);
                }
            }
        }
    }
    out.extend(top);
    let mut rest: Vec<usize> = nested.keys().copied().collect();
    rest.sort();
    out.extend(rest.into_iter().filter_map(|p| nested.remove(&p)));
    *items = out;
    true
}

fn refill(
    items: &mut Vec<StaticValue>,
    prefix: &[usize],
    slots: &[Vec<usize>],
    segs: &mut [(Vec<usize>, std::collections::VecDeque<StaticValue>)],
) {
    let last = slots.iter().filter(|s| s[..s.len() - 1] == *prefix).max().cloned();
    let old = std::mem::take(items);
    for (i, mut item) in old.into_iter().enumerate() {
        let mut path = prefix.to_vec();
        path.push(i);
        if slots.contains(&path) {
            if let Some((_, q)) = segs.iter_mut().find(|(s, _)| s == prefix) {
                if let Some(v) = q.pop_front() {
                    items.push(v);
                }
                if Some(&path) == last.as_ref() {
                    items.extend(q.drain(..));
                }
            }
        } else if slots.iter().any(|s| s.starts_with(&path)) {
            if let StaticValue::List(xs) = &mut item {
                refill(xs, &path, slots, segs);
            }
            items.push(item);
        } else {
            items.push(item);
        }
    }
}

/// Where `got` differs from `want` (wildcards match an element of their
/// tag), described for the user.
fn first_diff(want: &StaticValue, got: &StaticValue, path: &mut Vec<PathSeg>) -> Option<String> {
    use StaticValue as S;
    if let S::Str(w) = want
        && let Some(tag) = w.strip_prefix(WILD)
    {
        return match got {
            S::Element { tag: t, .. } if tag.is_empty() || tag == "*" || t == tag => None,
            _ if tag.is_empty() => None,
            _ if tag == "*" => Some(format!("at {} a new element was expected, got {}", path_string(path), short(got))),
            _ => Some(format!("at {} a new <{tag}> was expected, got {}", path_string(path), short(got))),
        };
    }
    let here = |path: &[PathSeg]| if path.len() <= 1 { "in the result".to_string() } else { format!("at {}", path_string(path)) };
    match (want, got) {
        (S::List(xs), S::List(ys)) | (S::Element { children: xs, .. }, S::Element { children: ys, .. })
            if matches!(want, S::List(_)) == matches!(got, S::List(_)) =>
        {
            if let (S::Element { tag: t1, props: p1, .. }, S::Element { tag: t2, props: p2, .. }) = (want, got)
                && (t1 != t2 || !same(&S::Record(p1.clone()), &S::Record(p2.clone())))
            {
                return Some(format!("{} expected {}, got {}", here(path), short(want), short(got)));
            }
            // Whitespace between tags is layout, not content.
            let blank = |v: &&StaticValue| matches!(v, S::Str(t) if t.trim().is_empty());
            let (xs, ys): (Vec<StaticValue>, Vec<StaticValue>) = if matches!(want, S::Element { .. }) {
                (xs.iter().filter(|v| !blank(v)).cloned().collect(), ys.iter().filter(|v| !blank(v)).cloned().collect())
            } else {
                (xs.clone(), ys.clone())
            };
            let (xs, ys) = (&xs, &ys);
            if xs.len() != ys.len() {
                return Some(format!(
                    "{} expected [{}], got [{}]",
                    here(path),
                    xs.iter().map(short).collect::<Vec<_>>().join(", "),
                    ys.iter().map(short).collect::<Vec<_>>().join(", ")
                ));
            }
            let child = matches!(want, S::Element { .. });
            for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
                path.push(if child { PathSeg::Child(i) } else { PathSeg::Index(i) });
                let d = first_diff(x, y, path);
                path.pop();
                if d.is_some() {
                    // Name the order when the elements are all there.
                    if xs.len() > 1 && !xs.iter().zip(ys).all(|(x, y)| first_diff(x, y, &mut Vec::new()).is_none()) {
                        let order = |v: &[StaticValue]| v.iter().map(short).collect::<Vec<_>>().join(", ");
                        if ys.iter().all(|y| xs.iter().any(|x| first_diff(x, y, &mut Vec::new()).is_none())) {
                            return Some(format!("{} expected [{}], got [{}]", here(path), order(xs), order(ys)));
                        }
                    }
                    return d;
                }
            }
            None
        }
        (S::Record(xs), S::Record(ys)) => {
            for (k, x) in xs {
                let Some((_, y)) = ys.iter().rev().find(|(j, _)| j == k) else {
                    return Some(format!("{} `{k}` is missing", here(path)));
                };
                path.push(PathSeg::Field(k.clone()));
                let d = first_diff(x, y, path);
                path.pop();
                if d.is_some() {
                    return d;
                }
            }
            (xs.len() != ys.len()).then(|| format!("{} expected {}, got {}", here(path), short(want), short(got)))
        }
        _ if same(want, got) => None,
        _ => Some(format!("{} expected {}, got {}", here(path), short(want), short(got))),
    }
}

/// A short label for a value: `<card #a>`, `<text>`, `3`.
fn short(v: &StaticValue) -> String {
    match v {
        StaticValue::Element { tag, props, .. } => match props.iter().find(|(k, _)| k == "id") {
            Some((_, StaticValue::Str(id))) => format!("<{tag} #{id}>"),
            _ => match props.first() {
                Some((k, v)) if matches!(v, StaticValue::Str(_) | StaticValue::Int(_) | StaticValue::Float(_)) => {
                    format!("<{tag} {k}={}>", v.to_source())
                }
                _ => format!("<{tag}>"),
            },
        },
        StaticValue::Str(s) if s.starts_with(WILD) => format!("<{}> (new)", &s[WILD.len()..]),
        other => {
            let s = other.to_source();
            if s.chars().count() > 30 { format!("{}…", s.chars().take(30).collect::<String>()) } else { s }
        }
    }
}

/// Add `preamble` statements on their own lines before the top-level
/// statement of `old` that holds `span`, in `source` (its edited version:
/// the statement starts at the same place).
fn add_preamble(source: &str, old: &str, span: &SourceSpan, preamble: &[String]) -> Result<String, String> {
    if preamble.is_empty() {
        return Ok(source.to_string());
    }
    let (_, stmts) = petal::rewrite::parse_ast(old)?;
    let stmt = stmts
        .iter()
        .find(|s| s.span.start.offset <= span.start.offset && span.start.offset <= s.span.end.offset)
        .ok_or("the edit is not inside a top-level statement")?;
    let at = crate::splice::byte_at(source, stmt.span.start.offset).ok_or("bad position")?;
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let mut block = preamble.join("\n");
    block.push_str("\n\n");
    Ok(format!("{}{block}{}", &source[..line_start], &source[line_start..]))
}
