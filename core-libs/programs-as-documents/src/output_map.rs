//! The output source map: for each element in a run's result, where in the
//! source it was built and where each of its fields came from.
//!
//! Built from petal's element-origin trace ([`Env::element_origin`]) and
//! [`provenance::trace_value`]. For each element in the result tree (through
//! lists, record fields, element props and children) an [`OutputNode`]
//! records:
//!
//! - its **construction site**: the element literal that built it, the call
//!   path that reached it (innermost call first), and its **fan-out**, how
//!   many elements of this result the same literal built;
//! - **per-field provenance**: for each prop, the literal its value flows
//!   from unchanged ([`FieldProvenance::Literal`]), or why it is computed;
//! - a stable [`OutputKey`] to find the same output in the next run.

use std::collections::HashMap;
use std::fmt;

use petal::env::Env;
use petal::heap::ElementId;
use petal::program::{FunctionId, Program, ProgramId, TermId, TermOp, base_fn_name};
use petal::provenance::{self, TraceEnd, TraceHop, ValueTrace};
use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;
use petal::value::Value;

/// One step from a value into one of its parts, for an output's index path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathSeg {
    /// An element of a list, or (first step) one of the run's result values.
    Index(usize),
    /// A field of a record.
    Field(String),
    /// A prop of an element whose value holds elements (a slot).
    Prop(String),
    /// A child of an element, counting every child value (text too).
    Child(usize),
}

/// An index path, written like `[1].items[2]/3`.
pub fn path_string(path: &[PathSeg]) -> String {
    let mut s = String::new();
    for seg in path {
        match seg {
            PathSeg::Index(i) => s.push_str(&format!("[{i}]")),
            PathSeg::Field(f) => s.push_str(&format!(".{f}")),
            PathSeg::Prop(p) => s.push_str(&format!("@{p}")),
            PathSeg::Child(i) => s.push_str(&format!("/{i}")),
        }
    }
    s
}

/// How an output is found again in the next run, in order of preference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputKey {
    /// The element's `id=` prop.
    Id(String),
    /// Its construction site and call path, each as a key that survives edits
    /// to values and attributes (see [`Site::key`]), and which of the
    /// outputs with that site and path it is, in result order (the elements
    /// a loop built at one site are told apart by this).
    Site {
        site: String,
        chain: Vec<String>,
        occurrence: usize,
    },
    /// The index path: for an element with no recorded origin (built by a
    /// native or copied with changes by a builtin).
    Path(Vec<PathSeg>),
}

impl fmt::Display for OutputKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputKey::Id(id) => write!(f, "#{id}"),
            OutputKey::Site { site, chain, occurrence } => {
                write!(f, "{site}")?;
                for c in chain {
                    write!(f, " < {c}")?;
                }
                if *occurrence > 0 {
                    write!(f, " ({occurrence})")?;
                }
                Ok(())
            }
            OutputKey::Path(p) => write!(f, "{}", path_string(p)),
        }
    }
}

/// Where an element was built.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    /// The element literal (`AllocElement` term).
    pub term: TermId,
    /// Its span: the whole `<tag …>…</tag>`.
    pub span: Option<SourceSpan>,
    /// The call site of each enclosing frame, innermost first.
    pub chain: Vec<TermId>,
    /// The span of each call in [`chain`](Self::chain).
    pub chain_spans: Vec<Option<SourceSpan>>,
    /// How many elements of this run's result the literal built.
    pub fan_out: usize,
    /// A key for the literal that survives edits to values and attributes:
    /// the enclosing function, the tag, and which literal of that tag in
    /// that function it is (`card/<rect>#0`, `/<text>#2` at top level).
    pub key: String,
    /// The same kind of key for each call in the chain (`/card()#1`).
    pub chain_keys: Vec<String>,
}

impl Site {
    /// The file the literal is written in.
    pub fn file(&self) -> Option<FileId> {
        self.span.map(|s| s.file)
    }
}

/// Where a field's value is written.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldProvenance {
    /// The value flows unchanged (through names, arguments, captures, field
    /// and index reads) from one literal.
    Literal(Box<LiteralSource>),
    /// The value is computed, or comes from somewhere the trace cannot see
    /// through; `reason` says which, for the user.
    Computed {
        /// Where the trace stopped, when it has a span.
        at: Option<SourceSpan>,
        reason: String,
    },
}

/// The literal a field's value flows from.
#[derive(Debug, Clone, PartialEq)]
pub struct LiteralSource {
    /// The literal: a constant, or a record, list or element literal.
    pub term: TermId,
    pub span: SourceSpan,
    /// The constant, when the literal is one.
    pub constant: Option<StaticValue>,
    /// When the value was read out of a record or list literal (`t.colors.ink`
    /// with `let t = {colors: {ink: …}}`), that outer literal and the path to
    /// the value inside it (`.colors.ink`).
    pub within: Option<WithinLiteral>,
    /// How many elements of this run's result have a field that flows from
    /// this same literal. More than one means editing it changes them all.
    pub shared: usize,
    /// The whole walk, for a host that wants the hops.
    pub trace: ValueTrace,
}

impl LiteralSource {
    pub fn file(&self) -> FileId {
        self.span.file
    }
}

/// A literal a value was read out of, and the path to it inside.
#[derive(Debug, Clone, PartialEq)]
pub struct WithinLiteral {
    pub literal: TermId,
    pub span: Option<SourceSpan>,
    pub path: String,
}

/// One prop of an output element.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub provenance: FieldProvenance,
}

/// One element of a run's result.
#[derive(Debug, Clone)]
pub struct OutputNode {
    /// Where it is in the result.
    pub path: Vec<PathSeg>,
    pub key: OutputKey,
    pub element: ElementId,
    pub tag: String,
    /// Where it was built; `None` for an element with no recorded origin.
    pub site: Option<Site>,
    /// Its props, in the element's order, each with its provenance.
    pub fields: Vec<Field>,
}

impl OutputNode {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// The output source map of one run: every element of the result, in
/// pre-order (a parent before its children).
#[derive(Debug, Clone, Default)]
pub struct OutputMap {
    pub nodes: Vec<OutputNode>,
}

impl OutputMap {
    /// Map the elements of `outputs`, the result of running `program` in
    /// `env` with element tracing on.
    pub fn build(env: &Env, program: ProgramId, outputs: &[Value]) -> OutputMap {
        let Some(prog) = env.get_program(program) else {
            return OutputMap::default();
        };
        let mut b = Builder {
            env,
            program: prog,
            keys: Keys::default(),
            nodes: Vec::new(),
        };
        for (i, v) in outputs.iter().enumerate() {
            b.walk(*v, vec![PathSeg::Index(i)], 0);
        }
        let mut nodes = b.nodes;
        finish(&mut nodes);
        OutputMap { nodes }
    }

    /// The node `key` names. A [`OutputKey::Path`] is looked up by path.
    pub fn find(&self, key: &OutputKey) -> Option<&OutputNode> {
        match key {
            OutputKey::Path(p) => self.nodes.iter().find(|n| &n.path == p),
            k => self.nodes.iter().find(|n| &n.key == k),
        }
    }

    /// The node at index path `path`.
    pub fn at_path(&self, path: &[PathSeg]) -> Option<&OutputNode> {
        self.nodes.iter().find(|n| n.path == path)
    }

    /// The nodes built by element literal `term`.
    pub fn built_at(&self, term: TermId) -> impl Iterator<Item = &OutputNode> {
        self.nodes.iter().filter(move |n| n.site.as_ref().is_some_and(|s| s.term == term))
    }
}

struct Builder<'a> {
    env: &'a Env,
    program: &'a Program,
    keys: Keys,
    nodes: Vec<OutputNode>,
}

const MAX_DEPTH: usize = 64;

impl Builder<'_> {
    fn walk(&mut self, v: Value, path: Vec<PathSeg>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let heap = self.env.heap();
        match v {
            Value::List(id) => {
                for (i, item) in heap.get_list(id).to_vec().into_iter().enumerate() {
                    self.walk(item, extend(&path, PathSeg::Index(i)), depth + 1);
                }
            }
            Value::Map(id) => {
                let fields: Vec<(String, Value)> = heap.get_map(id).iter().map(|(k, v)| (k.to_string(), *v)).collect();
                for (k, item) in fields {
                    self.walk(item, extend(&path, PathSeg::Field(k)), depth + 1);
                }
            }
            Value::Element(id) => {
                self.element(id, path.clone());
                let props: Vec<(String, Value)> = heap
                    .get_map(heap.get_element_props(id))
                    .iter()
                    .map(|(k, v)| (k.to_string(), *v))
                    .collect();
                let children = heap.get_list(heap.get_element_children(id)).to_vec();
                for (k, item) in props {
                    if matches!(item, Value::Element(_) | Value::List(_)) {
                        self.walk(item, extend(&path, PathSeg::Prop(k)), depth + 1);
                    }
                }
                for (i, item) in children.into_iter().enumerate() {
                    self.walk(item, extend(&path, PathSeg::Child(i)), depth + 1);
                }
            }
            _ => {}
        }
    }

    fn element(&mut self, id: ElementId, path: Vec<PathSeg>) {
        let heap = self.env.heap();
        let program = self.program;
        let tag = heap.get_string(heap.get_element_tag(id)).to_string();
        let props: Vec<(String, Value)> =
            heap.get_map(heap.get_element_props(id)).iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let explicit_id = props.iter().find(|(k, _)| k == "id").and_then(|(_, v)| match v {
            Value::String(s) => Some(heap.get_string(*s).to_string()),
            Value::Int(n) => Some(n.to_string()),
            _ => None,
        });
        let origin = self.env.element_origin(id).cloned();
        let site = origin.as_ref().map(|o| Site {
            term: o.term,
            span: program.source_map.get(o.term).copied(),
            chain: o.chain.to_vec(),
            chain_spans: o.chain.iter().map(|t| program.source_map.get(*t).copied()).collect(),
            fan_out: 0,
            key: self.keys.element_key(program, o.term),
            chain_keys: o.chain.iter().map(|t| self.keys.call_key(program, *t)).collect(),
        });
        let fields = props
            .iter()
            .map(|(name, _)| Field {
                name: name.clone(),
                provenance: match &origin {
                    Some(o) => field_provenance(program, o.term, &o.chain, name),
                    None => FieldProvenance::Computed {
                        at: None,
                        reason: format!("<{tag}> was not built by an element literal (a native or a builtin made it)"),
                    },
                },
            })
            .collect();
        let key = match (explicit_id, &site) {
            (Some(id), _) => OutputKey::Id(id),
            (None, Some(s)) => OutputKey::Site {
                site: s.key.clone(),
                chain: s.chain_keys.clone(),
                occurrence: 0,
            },
            (None, None) => OutputKey::Path(path.clone()),
        };
        self.nodes.push(OutputNode {
            path,
            key,
            element: id,
            tag,
            site,
            fields,
        });
    }
}

fn extend(path: &[PathSeg], seg: PathSeg) -> Vec<PathSeg> {
    let mut p = path.to_vec();
    p.push(seg);
    p
}

/// Fill in what needs every node: fan-outs, literal sharing, occurrences.
fn finish(nodes: &mut [OutputNode]) {
    let mut fan: HashMap<TermId, usize> = HashMap::new();
    let mut shared: HashMap<TermId, usize> = HashMap::new();
    for n in nodes.iter() {
        if let Some(s) = &n.site {
            *fan.entry(s.term).or_default() += 1;
        }
        let mut seen = Vec::new();
        for f in &n.fields {
            if let FieldProvenance::Literal(l) = &f.provenance
                && !seen.contains(&l.term)
            {
                seen.push(l.term);
                *shared.entry(l.term).or_default() += 1;
            }
        }
    }
    // An `id=` names one output only when no other output has it: two
    // outputs with one id would share a key, and an edit aimed at the second
    // would find (and verify against) the first. They fall back to their
    // site keys.
    let mut ids: HashMap<String, usize> = HashMap::new();
    for n in nodes.iter() {
        if let OutputKey::Id(id) = &n.key {
            *ids.entry(id.clone()).or_default() += 1;
        }
    }
    // Occurrences count every output a site and call path built, with an id
    // or not, so giving one of them an id (or taking it away) does not
    // renumber the others.
    let mut occurrences: HashMap<(String, Vec<String>), usize> = HashMap::new();
    for n in nodes.iter_mut() {
        if let Some(s) = &mut n.site {
            s.fan_out = fan[&s.term];
        }
        for f in &mut n.fields {
            if let FieldProvenance::Literal(l) = &mut f.provenance {
                l.shared = shared[&l.term];
            }
        }
        let occurrence = n.site.as_ref().map(|s| {
            let k = occurrences.entry((s.key.clone(), s.chain_keys.clone())).or_default();
            *k += 1;
            *k - 1
        });
        let duplicate_id = matches!(&n.key, OutputKey::Id(id) if ids[id] > 1);
        match (&mut n.key, &n.site, occurrence) {
            (OutputKey::Site { occurrence: o, .. }, _, Some(k)) => *o = k,
            (OutputKey::Id(_), Some(s), Some(k)) if duplicate_id => {
                n.key = OutputKey::Site { site: s.key.clone(), chain: s.chain_keys.clone(), occurrence: k };
            }
            (OutputKey::Id(_), None, _) if duplicate_id => n.key = OutputKey::Path(n.path.clone()),
            _ => {}
        }
    }
}

/// The provenance of prop `name` of an element built by `element_term` with
/// call chain `chain`.
pub fn field_provenance(program: &Program, element_term: TermId, chain: &[TermId], name: &str) -> FieldProvenance {
    let Some(input) = provenance::element_prop_term(program, element_term, name) else {
        return FieldProvenance::Computed {
            at: program.source_map.get(element_term).copied(),
            reason: format!("`{name}` is not written on the element literal"),
        };
    };
    let trace = provenance::trace_value(program, input, chain);
    let end_span = program.source_map.get(trace.end).copied();
    match (&trace.end_kind, end_span) {
        (TraceEnd::Computed, _) => {
            let what = provenance::term_text(program, trace.end)
                .map(|t| format!("`{}`", one_line(&t)))
                .unwrap_or_else(|| "its value".to_string());
            let line = end_span.map(|s| format!(" (line {})", s.start.line)).unwrap_or_default();
            FieldProvenance::Computed {
                at: end_span,
                reason: format!("`{name}` is computed: {what}{line} is not a literal"),
            }
        }
        (_, None) => FieldProvenance::Computed {
            at: None,
            reason: format!("`{name}` comes from a literal with no source position"),
        },
        (_, Some(span)) => FieldProvenance::Literal(Box::new(LiteralSource {
            term: trace.end,
            span,
            constant: trace.constant().cloned(),
            within: within_literal(program, &trace),
            shared: 0,
            trace,
        })),
    }
}

/// The outermost literal of the trailing run of field and index reads that
/// led to the trace's end, and the path inside it.
fn within_literal(program: &Program, trace: &ValueTrace) -> Option<WithinLiteral> {
    let mut steps: Vec<&TraceHop> = Vec::new();
    for h in trace.hops.iter().rev() {
        match h {
            TraceHop::Field { .. } | TraceHop::Index { .. } => steps.push(h),
            TraceHop::Alias { .. } => {}
            _ => break,
        }
    }
    let outer = *steps.last()?;
    let mut path = String::new();
    for h in steps.iter().rev() {
        match h {
            TraceHop::Field { field, .. } => path.push_str(&format!(".{field}")),
            TraceHop::Index { index, .. } => path.push_str(&format!("[{index}]")),
            _ => {}
        }
    }
    let literal = match outer {
        TraceHop::Field { record, .. } => *record,
        TraceHop::Index { list, .. } => *list,
        _ => return None,
    };
    Some(WithinLiteral {
        literal,
        span: program.source_map.get(literal).copied(),
        path,
    })
}

fn one_line(s: &str) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s }
}

/// Edit-stable keys for element literals and calls: the enclosing function's
/// name and the ordinal of the term among its kind in that function.
#[derive(Default)]
struct Keys {
    /// Per enclosing function: its element literals and calls, in term order.
    by_fn: Option<HashMap<Option<FunctionId>, Vec<TermId>>>,
}

impl Keys {
    fn terms_in(&mut self, program: &Program, f: Option<FunctionId>) -> &[TermId] {
        let by_fn = self.by_fn.get_or_insert_with(|| {
            let mut m: HashMap<Option<FunctionId>, Vec<TermId>> = HashMap::new();
            for t in &program.terms {
                if matches!(
                    t.op,
                    TermOp::AllocElement { .. } | TermOp::Call | TermOp::MethodCall { .. } | TermOp::BuiltinCall(_)
                ) {
                    m.entry(provenance::enclosing_function(program, t.id)).or_default().push(t.id);
                }
            }
            m
        });
        by_fn.get(&f).map(Vec::as_slice).unwrap_or(&[])
    }

    fn fn_name(program: &Program, f: Option<FunctionId>) -> String {
        match f {
            None => String::new(),
            Some(f) => match program.functions.iter().find(|d| d.id == f).and_then(|d| d.name.as_deref()) {
                Some(n) => base_fn_name(n.rsplit("::").next().unwrap_or(n)).to_string(),
                None => format!("fn#{}", f.0),
            },
        }
    }

    fn element_key(&mut self, program: &Program, term: TermId) -> String {
        let f = provenance::enclosing_function(program, term);
        let tag = element_tag(program, term).unwrap_or_default();
        let ordinal = self
            .terms_in(program, f)
            .iter()
            .filter(|t| element_tag(program, **t).as_deref() == Some(tag.as_str()))
            .position(|t| *t == term)
            .unwrap_or(0);
        format!("{}/<{tag}>#{ordinal}", Self::fn_name(program, f))
    }

    fn call_key(&mut self, program: &Program, term: TermId) -> String {
        let f = provenance::enclosing_function(program, term);
        let callee = callee_name(program, term).unwrap_or_default();
        let ordinal = self
            .terms_in(program, f)
            .iter()
            .filter(|t| element_tag(program, **t).is_none() && callee_name(program, **t).as_deref() == Some(&callee))
            .position(|t| *t == term)
            .unwrap_or(0);
        format!("{}/{callee}()#{ordinal}", Self::fn_name(program, f))
    }
}

fn element_tag(program: &Program, term: TermId) -> Option<String> {
    match &program.terms.get(term.0 as usize)?.op {
        TermOp::AllocElement { tag, .. } => program.get_string_constant(*tag).map(str::to_string),
        _ => None,
    }
}

/// The name a call calls, as written.
fn callee_name(program: &Program, term: TermId) -> Option<String> {
    let t = program.terms.get(term.0 as usize)?;
    let raw = match &t.op {
        TermOp::BuiltinCall(name) | TermOp::MethodCall { name, .. } => program.get_string_constant(*name)?.to_string(),
        TermOp::Call => {
            let mut cur = *t.inputs.first()?;
            let mut found = None;
            for _ in 0..16 {
                let c = program.terms.get(cur.0 as usize)?;
                if let Some(n) = &c.name {
                    found = Some(n.clone());
                    break;
                }
                match (&c.op, c.inputs.first()) {
                    (TermOp::Copy, Some(i)) => cur = *i,
                    _ => break,
                }
            }
            found.unwrap_or_else(|| "call".to_string())
        }
        _ => return None,
    };
    Some(base_fn_name(raw.rsplit("::").next().unwrap_or(&raw)).to_string())
}
