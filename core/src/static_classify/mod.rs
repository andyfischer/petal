//! The static-value classifier: which top-level declarations can only ever
//! produce one value for a given source text.
//!
//! This is stage 0 of `docs/proposals/static-values.md` (work order:
//! `docs/tasks/static-value-optimization.md`, section S0). It changes nothing
//! about how a program runs. It answers, for every top-level declaration:
//!
//! - its **class** — static data, a function (static-pure or not), or dynamic;
//! - for a dynamic one, the **reason chain** down to the term that makes it so;
//! - whether **dynamic code reads it**, which is what decides whether a later
//!   stage gives it a slot;
//! - its **dependency set** — every declaration its value can depend on, by
//!   module-qualified name, closed transitively — which is what a reload
//!   compares to decide whether a kept value is still good.
//!
//! # The rules
//!
//! The pass is a dataflow analysis over the term graph with a three-point
//! lattice per term (the proposal's section 1):
//!
//! - **data** — a value that is a function of the source text alone and holds
//!   no closure;
//! - **function** — a known function value (a set of possible targets);
//! - **dynamic** — anything else, including anything the pass cannot resolve.
//!
//! A user function is **static-pure** when every term of its body is static
//! under the assumption that its parameters are data. That assumption is
//! checked at every call the pass calls static: the arguments must be data.
//! Callees resolve through the two ways a function is actually bound — a
//! direct closure capture, and the write-once cell a hoisted forward reference
//! goes through — and recursion is solved optimistically (a fixpoint that
//! starts from "pure").
//!
//! The structural rules:
//!
//! - a loop, branch or match is static only as a whole region: its inputs and
//!   every term of every child block;
//! - a top-level name that is bound more than once (`xs[i] = v`,
//!   `xs = append(xs, v)`, a rebind inside a top-level `if` or loop) is
//!   dynamic at every definition, because such a write is lowered to an
//!   in-place update of the literal's own storage;
//! - a value that would hold a function is dynamic, because a kept value may
//!   not contain a closure;
//! - a method call is static only if every method of that name is, and every
//!   native that name can fall back to;
//! - a field read whose name is a constant-data key of some enum is static
//!   only if that data is.
//!
//! Natives are static only when registered `CONST`
//! ([`crate::native_fn::NativeConstness`]). A `native fn` whose host is not
//! present (as under `petal check` and the language server) is not registered
//! and therefore dynamic, so those tools may report less as static than a
//! host does and never more.
//!
//! **The classification never reads a literal's value.** It reads the shape of
//! the graph and the names the compiler wrote into it (builtin, method, field
//! and enum names), so a literal-only edit cannot change any answer here.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use smallvec::SmallVec;

use crate::constant_table::ConstantId;
use crate::native_fn::{NativeConstness, NativeFnId, NativeFnTable};
use crate::program::{
    BlockId, FunctionDef, FunctionId, Program, Term, TermId, TermOp, base_fn_name, split_enum_tag,
};
use crate::program_analysis::CellIndex;
use crate::source_map::FileId;

/// The builtins the compiler emits for enum declarations. Spelled here rather
/// than imported so this module depends on nothing but their names.
const ENUM_DATA: &str = "__enum_data";
const ENUM_FROM: &str = "__enum_from";
const ENUM_ALL: &str = "__enum_all";

// ---------------------------------------------------------------------------
// The lattice
// ---------------------------------------------------------------------------

/// One thing a function-valued term may call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Target {
    User(u32),
    Native(u32),
}

type Targets = SmallVec<[Target; 2]>;

/// A term's class. `Top` is the optimistic starting point ("not known to be
/// anything yet"); it is the identity of a join and never survives the pass.
#[derive(Debug, Clone, PartialEq)]
enum Class {
    Top,
    Data,
    Function(Targets),
    Dynamic(Why),
}

impl Class {
    fn is_dynamic(&self) -> bool {
        matches!(self, Class::Dynamic(_))
    }
}

/// Why a term is dynamic. Small and `Copy`: the text is built from the term
/// when a report asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// An operand, or a term inside the region, is dynamic.
    Input(TermId),
    /// Calls a user function that is not static-pure.
    Calls(FunctionId),
    /// Hands a function that is not static-pure to a higher-order builtin.
    Callback(FunctionId),
    /// A method call that may dispatch to a method that is not static-pure.
    Method(FunctionId),
    /// Calls a registered native that is not `CONST`.
    Native(NativeFnId),
    /// Calls a native by a name nothing registered.
    UnregisteredNative(ConstantId),
    /// Reads enum constant data whose record (the term) is dynamic.
    EnumData(TermId),
    /// Reads enum constant data the pass could not find the declaration of.
    UnknownEnumData,
    /// Declares, reads or writes `state`.
    State,
    /// Is, reads or writes a `var` cell that is not a local of the function.
    Var,
    /// The name is bound again at the given term.
    Reassigned(TermId),
    /// A hoisted function read before its declaration has run.
    UsedBeforeDeclared,
    /// Calls a value that is data, not a function.
    NotCallable,
    /// No method or constant native of that name is known.
    UnknownMethod,
    /// Would build a value that holds a function.
    HoldsFunction,
    /// Uses a function where a value is computed from it.
    FunctionOperand,
    /// Passes a function as an argument to something that is not a
    /// higher-order constant builtin.
    FunctionArgument,
    /// A function on one path and data on another.
    MixedValue,
    /// A callback that itself returns a function.
    CallbackReturnsFunction,
    /// A binding the pass cannot resolve.
    UnknownBinding,
    /// A term the compiler emitted for a compile error.
    CompileError,
}

fn join(a: Class, b: Class) -> Class {
    match (a, b) {
        (Class::Top, x) | (x, Class::Top) => x,
        (Class::Dynamic(w), _) | (_, Class::Dynamic(w)) => Class::Dynamic(w),
        (Class::Data, Class::Data) => Class::Data,
        (Class::Function(mut x), Class::Function(y)) => {
            for t in y {
                if let Err(at) = x.binary_search(&t) {
                    x.insert(at, t);
                }
            }
            Class::Function(x)
        }
        (Class::Data, Class::Function(_)) | (Class::Function(_), Class::Data) => {
            Class::Dynamic(Why::MixedValue)
        }
    }
}

// ---------------------------------------------------------------------------
// The public report
// ---------------------------------------------------------------------------

/// The class of one term, for a caller that walks the graph itself (the
/// lowering of a later stage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermClass {
    /// A function of the source text alone, holding no closure.
    Data,
    /// A known function value.
    Function,
    Dynamic,
}

/// The class of a top-level declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeclClass {
    /// Static data: its initializer can only ever produce one value for this
    /// source text.
    Static,
    /// A function value. [`StaticDecl::static_pure`] says whether a call to
    /// it with static arguments is static.
    Function,
    Dynamic,
}

/// What kind of declaration it is. The same vocabulary as
/// [`crate::source_diff::ConstructKind`], so a reload can pair the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DeclKind {
    #[serde(rename = "let")]
    Let,
    #[serde(rename = "config let")]
    ConfigLet,
    #[serde(rename = "var")]
    Var,
    #[serde(rename = "state")]
    State,
    #[serde(rename = "fn")]
    Function,
    #[serde(rename = "class")]
    Class,
    #[serde(rename = "enum")]
    Enum,
}

impl DeclKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DeclKind::Let => "let",
            DeclKind::ConfigLet => "config let",
            DeclKind::Var => "var",
            DeclKind::State => "state",
            DeclKind::Function => "fn",
            DeclKind::Class => "class",
            DeclKind::Enum => "enum",
        }
    }
}

/// One link of a reason chain: `subject` `text` at `file`:`line`.
///
/// ```text
/// LAB_W calls build_label_w at app.ptl:784
/// build_label_w calls text_width, which is not declared constant at app.ptl:382
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReasonStep {
    /// The declaration or function this link is about.
    pub subject: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl ReasonStep {
    /// `subject text at file:line`, the location left off when there is none.
    pub fn render(&self) -> String {
        let mut s = format!("{} {}", self.subject, self.text);
        match (&self.file, self.line) {
            (Some(f), Some(l)) => s.push_str(&format!(" at {f}:{l}")),
            (None, Some(l)) => s.push_str(&format!(" at line {l}")),
            _ => {}
        }
        s
    }
}

/// How a reader of a static declaration uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderKind {
    /// A dynamic term reads it as an operand (or a static term inside a
    /// dynamic declaration or region does, which runs just as often).
    Dynamic,
    /// A closure captures it. Closures are built on every run.
    Capture,
    /// A top-level statement that belongs to no declaration reads it (an
    /// `assert`, say). Such a statement runs on every run whatever its class.
    Statement,
    /// Another static declaration's initializer reads it.
    Static,
}

/// One reader of a static declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reader {
    pub kind: ReaderKind,
    /// The declaration or function doing the reading, when it has a name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// What the classifier concluded about one top-level declaration.
#[derive(Debug, Clone, Serialize)]
pub struct StaticDecl {
    /// The declared name, unqualified (`TILES`, `Point.len2`).
    pub name: String,
    /// The module-qualified name: `name` for the entry file, `module::name`
    /// for a module. This is the identity a dependency set is written in.
    pub qualified: String,
    /// The module's canonical name; `None` for the entry file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub kind: DeclKind,
    /// The file's display name, when the program has a file table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip)]
    pub file_id: FileId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub class: DeclClass,
    /// For a function: whether a call to it with static arguments is static.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_pure: Option<bool>,
    /// For a plain `state`: whether its initializer is static (what a
    /// `state`-as-constant lint asks). `None` for every other kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initializer_static: Option<bool>,
    /// Why it is dynamic, or why a function is not static-pure. Empty
    /// otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reason: Vec<ReasonStep>,
    /// For static data: whether code that runs on every run reads it — a
    /// dynamic operand, a closure capture, or a free-standing top-level
    /// statement. `None` for functions and dynamic declarations. A read *by
    /// name* from a host or a tool is not visible to the compiler and is not
    /// counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_by_dynamic: Option<bool>,
    /// For static data: who reads it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<Reader>,
    /// For static data: where its value is handed to something that may
    /// update a container in place (`ys = append(xs, v)`, an indexed write
    /// through an alias). The value is still static, and so is what is built
    /// from it, but the declaration itself must not be kept: the update may
    /// land on its own storage. `None` when nothing does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutated_by: Option<ReasonStep>,
    /// The declarations this one refers to directly, by qualified name.
    pub direct_depends_on: BTreeSet<String>,
    /// Every declaration this one's value can depend on, by qualified name:
    /// `direct_depends_on` closed transitively.
    pub depends_on: BTreeSet<String>,
    /// The term that carries the declaration's value.
    #[serde(skip)]
    pub term: TermId,
}

/// Counts over a report's declarations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StaticSummary {
    pub declarations: usize,
    /// Static data.
    #[serde(rename = "static")]
    pub static_data: usize,
    /// Static data that dynamic code reads.
    pub static_read_by_dynamic: usize,
    /// Static data whose value reaches an in-place update, so it is not kept.
    pub static_mutated: usize,
    pub functions: usize,
    pub functions_static_pure: usize,
    pub dynamic: usize,
}

/// The classifier's answer for a program: one entry per top-level declaration
/// of every module, in file then source order.
#[derive(Debug, Clone, Serialize)]
pub struct StaticReport {
    pub declarations: Vec<StaticDecl>,
}

impl StaticReport {
    /// The declaration with this module-qualified name.
    pub fn get(&self, qualified: &str) -> Option<&StaticDecl> {
        self.declarations.iter().find(|d| d.qualified == qualified)
    }

    /// The declarations of the entry file only.
    pub fn entry(&self) -> impl Iterator<Item = &StaticDecl> {
        self.declarations.iter().filter(|d| d.module.is_none())
    }

    /// Counts over the entry file's declarations, or over every module's.
    pub fn summary(&self, all_modules: bool) -> StaticSummary {
        let mut s = StaticSummary::default();
        for d in &self.declarations {
            if !all_modules && d.module.is_some() {
                continue;
            }
            s.declarations += 1;
            match d.class {
                DeclClass::Static => {
                    s.static_data += 1;
                    if d.read_by_dynamic == Some(true) {
                        s.static_read_by_dynamic += 1;
                    }
                    if d.mutated_by.is_some() {
                        s.static_mutated += 1;
                    }
                }
                DeclClass::Function => {
                    s.functions += 1;
                    if d.static_pure == Some(true) {
                        s.functions_static_pure += 1;
                    }
                }
                DeclClass::Dynamic => s.dynamic += 1,
            }
        }
        s
    }

    /// The report as text: one line per declaration, a dynamic one (or a
    /// function that is not static-pure) followed by its reason chain.
    pub fn render_text(&self, all_modules: bool) -> String {
        use std::fmt::Write;
        let decls: Vec<&StaticDecl> = self
            .declarations
            .iter()
            .filter(|d| all_modules || d.module.is_none())
            .collect();
        let width = decls
            .iter()
            .map(|d| d.kind.as_str().len() + 1 + display_name(d, all_modules).len())
            .max()
            .unwrap_or(0)
            .min(48);
        let mut out = String::new();
        for d in &decls {
            let class = match d.class {
                DeclClass::Static => "static",
                DeclClass::Function => "function",
                DeclClass::Dynamic => "dynamic",
            };
            let what = format!("{} {}", d.kind.as_str(), display_name(d, all_modules));
            let line = match d.line {
                Some(l) => format!("line {l}"),
                None => String::new(),
            };
            let note = match d.class {
                DeclClass::Static => match d.read_by_dynamic {
                    Some(true) => format!("read by dynamic code ({})", readers_text(d)),
                    _ if d.kind == DeclKind::Enum => "constant data".to_string(),
                    _ if d.readers.is_empty() => "not read".to_string(),
                    _ => format!("read only by static code ({})", readers_text(d)),
                },
                DeclClass::Function => match d.static_pure {
                    Some(true) => "static-pure".to_string(),
                    _ => "not static-pure".to_string(),
                },
                // A `state` or a `var` is dynamic by being one; a line under
                // it saying so is noise in a file that declares forty.
                DeclClass::Dynamic if self_evident(d) => match d.initializer_static {
                    Some(true) => "its initializer is static".to_string(),
                    _ => String::new(),
                },
                DeclClass::Dynamic => String::new(),
            };
            let row = format!("{class:<9} {what:<width$}  {line:<9} {note}");
            writeln!(out, "{}", row.trim_end()).unwrap();
            if let Some(m) = &d.mutated_by {
                writeln!(out, "            ! not kept: {}", m.render()).unwrap();
            }
            if self_evident(d) {
                continue;
            }
            for step in &d.reason {
                writeln!(out, "            = {}", step.render()).unwrap();
            }
        }
        let s = self.summary(all_modules);
        writeln!(
            out,
            "\n{} declaration{}: {} static ({} read by dynamic code), {} function{} ({} static-pure), {} dynamic",
            s.declarations,
            if s.declarations == 1 { "" } else { "s" },
            s.static_data,
            s.static_read_by_dynamic,
            s.functions,
            if s.functions == 1 { "" } else { "s" },
            s.functions_static_pure,
            s.dynamic
        )
        .unwrap();
        out
    }

    /// The report as JSON: `{summary, declarations}`.
    pub fn to_json(&self, all_modules: bool) -> serde_json::Value {
        let decls: Vec<&StaticDecl> = self
            .declarations
            .iter()
            .filter(|d| all_modules || d.module.is_none())
            .collect();
        serde_json::json!({
            "summary": self.summary(all_modules),
            "declarations": decls,
        })
    }
}

/// A `state` or `var` declaration whose whole reason is that it is one.
fn self_evident(d: &StaticDecl) -> bool {
    matches!(d.kind, DeclKind::State | DeclKind::Var)
        && d.class == DeclClass::Dynamic
        && d.reason.len() == 1
}

fn display_name(d: &StaticDecl, qualified: bool) -> &str {
    if qualified { &d.qualified } else { &d.name }
}

fn readers_text(d: &StaticDecl) -> String {
    // Dynamic readers first: they are what decides the answer.
    // When dynamic code reads it, those readers are the answer; the static
    // ones are listed only when they are all there is.
    let dynamic = d.read_by_dynamic == Some(true);
    let readers: Vec<&Reader> = d
        .readers
        .iter()
        .filter(|r| !dynamic || r.kind != ReaderKind::Static)
        .collect();
    let shown: Vec<String> = readers
        .iter()
        .take(3)
        .map(|r| {
            let who = match (&r.by, r.line) {
                (Some(by), _) => by.clone(),
                (None, Some(l)) => format!("line {l}"),
                (None, None) => "a statement".to_string(),
            };
            match r.kind {
                ReaderKind::Capture => format!("captured by {who}"),
                ReaderKind::Statement => format!("statement at {who}"),
                ReaderKind::Dynamic | ReaderKind::Static => who,
            }
        })
        .collect();
    let mut s = shown.join(", ");
    if readers.len() > 3 {
        s.push_str(&format!(", and {} more", readers.len() - 3));
    }
    s
}

/// The whole analysis: the report, plus the class of every term for a caller
/// that needs to walk the graph (a later stage's lowering).
pub struct StaticAnalysis {
    pub report: StaticReport,
    classes: Vec<TermClass>,
}

impl StaticAnalysis {
    /// The class of a term. A term inside a function body is classified under
    /// the assumption that the function's parameters are data; that holds at
    /// every call this analysis calls static, and nowhere else.
    pub fn term_class(&self, id: TermId) -> TermClass {
        self.classes
            .get(id.0 as usize)
            .copied()
            .unwrap_or(TermClass::Dynamic)
    }
}

/// Classify every top-level declaration of `program` against the natives of
/// `natives` (the table of the `Env` that compiled it).
pub fn classify(program: &Program, natives: &NativeFnTable) -> StaticReport {
    analyze(program, natives).report
}

/// [`classify`], keeping the per-term classes.
pub fn analyze(program: &Program, natives: &NativeFnTable) -> StaticAnalysis {
    let mut pass = Pass::new(program, natives);
    pass.solve();
    let report = pass.report();
    let classes = pass
        .class
        .iter()
        .map(|c| match c {
            Class::Data | Class::Top => TermClass::Data,
            Class::Function(_) => TermClass::Function,
            Class::Dynamic(_) => TermClass::Dynamic,
        })
        .collect();
    StaticAnalysis { report, classes }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// A top-level name of one file: every root-block term that binds it.
struct Group {
    file: FileId,
    name: String,
    /// The defining terms, in root-block order. Empty for an enum.
    defs: Vec<TermId>,
    kind: DeclKind,
    /// The term that carries the value: the first definition, or for a
    /// function the closure or overload set the name ends up bound to.
    canonical: TermId,
    /// For an enum: the records of its variants' constant data.
    enum_records: Vec<TermId>,
    /// For an enum: the `__enum_data` statements that publish them.
    enum_calls: Vec<TermId>,
}

struct Pass<'a> {
    p: &'a Program,
    natives: &'a NativeFnTable,
    cells: CellIndex,
    class: Vec<Class>,

    // --- structure, computed once ---
    fn_index: HashMap<FunctionId, usize>,
    /// Function body block -> index into `p.functions`.
    body_fn: HashMap<BlockId, usize>,
    /// The function whose body tree a block is in; `None` for the top level.
    owner: Vec<Option<usize>>,
    /// The control term a block hangs off (a child block's parent, or the
    /// match a guard block belongs to).
    parent: Vec<Option<TermId>>,
    block_phantoms: Vec<Vec<TermId>>,
    /// `MakeClosure` sites per function.
    sites: Vec<Vec<TermId>>,
    /// Phi -> (rebinding term, the control term whose child block holds it).
    phi_srcs: HashMap<TermId, Vec<(TermId, Option<TermId>)>>,
    /// Match term -> its guard blocks.
    guards: HashMap<TermId, Vec<BlockId>>,
    /// `MakeClosure` term -> the overload sets it is a member of.
    sets_of: HashMap<TermId, Vec<TermId>>,
    /// `Return` terms per function.
    returns: Vec<Vec<TermId>>,
    /// Hoisted function cell (its `CellNew`) -> the one `CellWrite` to it.
    fn_cells: HashMap<TermId, TermId>,
    /// Position of each listed root-block term.
    root_pos: HashMap<TermId, usize>,
    /// Definition of a reassigned name -> the first rebinding of it.
    reassigned: HashMap<TermId, TermId>,
    /// Method name -> the function terms a call of it may dispatch to.
    dispatch: HashMap<ConstantId, Vec<TermId>>,
    /// Method name -> the built-in class methods of that name (`Rect.inset`).
    class_natives: HashMap<String, Vec<NativeFnId>>,
    /// Enum data key -> the records that carry it.
    enum_keys: HashMap<String, Vec<TermId>>,
    /// Every data key any enum declares.
    declared_enum_keys: HashSet<String>,
    groups: Vec<Group>,
    /// Root-block defining term -> its group.
    group_of: HashMap<TermId, usize>,
    /// State key -> the group that declares it.
    state_groups: HashMap<u64, usize>,

    // --- summaries, recomputed each round ---
    /// The first dynamic term of a function's body, if any.
    fn_dyn: Vec<Option<TermId>>,
    /// The class of what a function returns.
    fn_ret: Vec<Class>,
}

impl<'a> Pass<'a> {
    fn new(p: &'a Program, natives: &'a NativeFnTable) -> Self {
        let nblocks = p.blocks.len();
        let nfns = p.functions.len();
        let mut pass = Pass {
            p,
            natives,
            cells: p.cell_index(),
            class: vec![Class::Top; p.terms.len()],
            fn_index: p.functions.iter().enumerate().map(|(i, f)| (f.id, i)).collect(),
            body_fn: p
                .functions
                .iter()
                .enumerate()
                .map(|(i, f)| (f.body_block, i))
                .collect(),
            owner: vec![None; nblocks],
            parent: vec![None; nblocks],
            block_phantoms: vec![Vec::new(); nblocks],
            sites: vec![Vec::new(); nfns],
            phi_srcs: HashMap::new(),
            guards: HashMap::new(),
            sets_of: HashMap::new(),
            returns: vec![Vec::new(); nfns],
            fn_cells: HashMap::new(),
            root_pos: HashMap::new(),
            reassigned: HashMap::new(),
            dispatch: p.dispatch_targets(),
            class_natives: HashMap::new(),
            enum_keys: HashMap::new(),
            declared_enum_keys: HashSet::new(),
            groups: Vec::new(),
            group_of: HashMap::new(),
            state_groups: HashMap::new(),
            fn_dyn: vec![None; nfns],
            fn_ret: vec![Class::Top; nfns],
        };
        pass.index_structure();
        pass.index_groups();
        pass
    }

    fn block(&self, b: BlockId) -> &'a crate::program::Block {
        &self.p.blocks[b.0 as usize]
    }

    fn func(&self, i: usize) -> &'a FunctionDef {
        &self.p.functions[i]
    }

    fn cls(&self, t: TermId) -> &Class {
        &self.class[t.0 as usize]
    }

    fn is_phantom(term: &Term) -> bool {
        matches!(term.op, TermOp::Copy) && term.inputs.is_empty() && term.name.is_some()
    }

    fn string(&self, c: ConstantId) -> &'a str {
        self.p.get_string_constant(c).unwrap_or("")
    }

    /// Every block a control term runs: its child blocks and, for a match,
    /// its arms' guard and body blocks.
    fn blocks_of(&self, term: &Term) -> SmallVec<[BlockId; 4]> {
        // Nearly every term runs no block at all.
        if term.child_blocks.is_empty() && !matches!(term.op, TermOp::Match) {
            return SmallVec::new();
        }
        let mut out: SmallVec<[BlockId; 4]> = term.child_blocks.iter().copied().collect();
        if let Some(gs) = self.guards.get(&term.id) {
            for g in gs {
                if !out.contains(g) {
                    out.push(*g);
                }
            }
        }
        if let Some(arms) = self.p.match_arms.get(&term.id) {
            for arm in arms {
                if !out.contains(&arm.body_block) {
                    out.push(arm.body_block);
                }
            }
        }
        out
    }

    fn index_structure(&mut self) {
        let p = self.p;
        for (term_id, arms) in &p.match_arms {
            let gs: Vec<BlockId> = arms.iter().filter_map(|a| a.guard_block).collect();
            if !gs.is_empty() {
                self.guards.insert(*term_id, gs);
            }
        }
        for block in &p.blocks {
            self.parent[block.id.0 as usize] = block.parent_term_id;
        }
        for term in &p.terms {
            for b in self.blocks_of(term) {
                if let Some(slot) = self.parent.get_mut(b.0 as usize) {
                    slot.get_or_insert(term.id);
                }
            }
            if Self::is_phantom(term) {
                if let Some(v) = self.block_phantoms.get_mut(term.block_id.0 as usize) {
                    v.push(term.id);
                }
            }
            if let TermOp::MakeClosure(fid) = term.op
                && let Some(&i) = self.fn_index.get(&fid)
            {
                self.sites[i].push(term.id);
            }
            if matches!(term.op, TermOp::MakeOverloadSet) {
                for &member in &term.inputs {
                    self.sets_of.entry(member).or_default().push(term.id);
                }
            }
        }
        for block in &p.blocks {
            let control = self.parent[block.id.0 as usize];
            for po in &block.phi_outs {
                self.phi_srcs
                    .entry(po.dest_term)
                    .or_default()
                    .push((po.src_term, control));
            }
        }

        // Which function each block belongs to: walk every body's block tree.
        for fi in 0..p.functions.len() {
            let mut stack = vec![p.functions[fi].body_block];
            while let Some(b) = stack.pop() {
                let Some(slot) = self.owner.get_mut(b.0 as usize) else {
                    continue;
                };
                if slot.is_some() {
                    continue;
                }
                *slot = Some(fi);
                for &t in &self.block(b).terms {
                    let term = p.get_term(t);
                    if matches!(term.op, TermOp::Return) {
                        self.returns[fi].push(t);
                    }
                    stack.extend(self.blocks_of(term));
                }
            }
        }

        let root = self.block(p.root_block);
        for (i, &t) in root.terms.iter().enumerate() {
            self.root_pos.insert(t, i);
        }

        // Hoisted function cells: an unnamed root-block `CellNew` with exactly
        // one write, and that write in the root block. (A user's `var` is
        // named.) Whether the written value is a function is checked where
        // the cell is read.
        for term in &p.terms {
            if !matches!(term.op, TermOp::CellNew)
                || term.block_id != p.root_block
                || term.name.is_some()
                || !self.cells.is_decl(term.id)
            {
                continue;
            }
            let writes = self.cells.writes_of(term.id);
            if let [w] = writes
                && p.get_term(*w).block_id == p.root_block
            {
                self.fn_cells.insert(term.id, *w);
            }
        }

        for i in 0..self.natives.count() {
            let id = NativeFnId(i as u32);
            if let Some((_, method)) = self.natives.get_name(id).rsplit_once('.') {
                self.class_natives.entry(method.to_string()).or_default().push(id);
            }
        }

        for def in p.enums.values() {
            for v in &def.variants {
                self.declared_enum_keys.extend(v.data.iter().cloned());
            }
        }
        for term in &p.terms {
            if !self.is_builtin(term, ENUM_DATA) {
                continue;
            }
            let Some(&record) = term.inputs.get(1) else { continue };
            if let TermOp::AllocMap { fields, .. } = &p.get_term(record).op {
                for f in fields {
                    self.enum_keys
                        .entry(self.string(*f).to_string())
                        .or_default()
                        .push(record);
                }
            }
        }
    }

    fn is_builtin(&self, term: &Term, name: &str) -> bool {
        matches!(term.op, TermOp::BuiltinCall(c) if self.string(c) == name)
    }

    /// The module qualifier and the bare name a root-block term's name holds.
    fn bare_name(name: &str) -> &str {
        let local = name.rsplit("::").next().unwrap_or(name);
        base_fn_name(local)
    }

    /// Whether `term` defines the function `name`: a closure of a function
    /// declared under that name, an overload set, or the write that fills a
    /// hoisted function's cell.
    fn is_fn_def(&self, term: &Term, name: &str) -> bool {
        match term.op {
            TermOp::MakeClosure(fid) => self
                .fn_index
                .get(&fid)
                .and_then(|&i| self.func(i).name.as_deref())
                .is_some_and(|n| Self::bare_name(n) == name),
            TermOp::MakeOverloadSet => true,
            TermOp::CellWrite => term
                .inputs
                .first()
                .and_then(|c| self.cells.decl_of(*c))
                .is_some_and(|d| self.fn_cells.contains_key(&d)),
            _ => false,
        }
    }

    /// Whether `term` is the constructor a `class name` declaration compiles
    /// to: a function whose whole body allocates the record tagged `name`.
    fn is_class_constructor(&self, term: TermId, name: &str) -> bool {
        let TermOp::MakeClosure(fid) = self.p.get_term(term).op else {
            return false;
        };
        let Some(&fi) = self.fn_index.get(&fid) else {
            return false;
        };
        match &self.block(self.func(fi).body_block).terms[..] {
            [only] => matches!(
                &self.p.get_term(*only).op,
                TermOp::AllocMap { class: Some(c), .. } if self.string(*c) == name
            ),
            _ => false,
        }
    }

    fn index_groups(&mut self) {
        let p = self.p;
        let root = self.block(p.root_block);
        let mut by_key: HashMap<(FileId, String), usize> = HashMap::new();
        // A term's file is its position's. One with no position anywhere
        // near (a merged overload set) belongs to the file being compiled
        // around it: the modules are emitted one after another.
        let mut cur_file = crate::source_map::ENTRY_FILE;
        for &tid in &root.terms {
            let term = p.get_term(tid);
            let own = match term.op {
                // A set's operands are closures, possibly another module's.
                TermOp::MakeOverloadSet => self.own_span(tid),
                _ => self.span(tid),
            };
            if let Some(span) = own {
                cur_file = span.file;
            }
            let Some(name) = term.name.as_deref() else { continue };
            let bare = Self::bare_name(name).to_string();
            let gi = *by_key.entry((cur_file, bare.clone())).or_insert_with(|| {
                self.groups.push(Group {
                    file: cur_file,
                    name: bare,
                    defs: Vec::new(),
                    kind: DeclKind::Let,
                    canonical: tid,
                    enum_records: Vec::new(),
                    enum_calls: Vec::new(),
                });
                self.groups.len() - 1
            });
            self.groups[gi].defs.push(tid);
            self.group_of.insert(tid, gi);
        }

        for gi in 0..self.groups.len() {
            let name = self.groups[gi].name.clone();
            let defs = self.groups[gi].defs.clone();
            let ops: Vec<&Term> = defs.iter().map(|d| p.get_term(*d)).collect();
            let all_fn = ops.iter().all(|t| self.is_fn_def(t, &name));
            let kind = if all_fn {
                // The value the name ends up bound to: the last closure or
                // overload set (a hoisted function's cell write names it).
                let canonical = ops
                    .iter()
                    .rev()
                    .find(|t| matches!(t.op, TermOp::MakeClosure(_) | TermOp::MakeOverloadSet))
                    .map(|t| t.id)
                    .or_else(|| ops.last().and_then(|t| t.inputs.get(1).copied()))
                    .unwrap_or(defs[0]);
                self.groups[gi].canonical = canonical;
                if self.is_class_constructor(canonical, &name) {
                    DeclKind::Class
                } else {
                    DeclKind::Function
                }
            } else if ops.iter().any(|t| matches!(t.op, TermOp::StateInit)) {
                if let Some(t) = ops.iter().find(|t| matches!(t.op, TermOp::StateInit)) {
                    self.groups[gi].canonical = t.id;
                }
                DeclKind::State
            } else if ops.iter().any(|t| matches!(t.op, TermOp::CellNew)) {
                DeclKind::Var
            } else if ops.iter().any(|t| t.is_config) {
                DeclKind::ConfigLet
            } else {
                DeclKind::Let
            };
            self.groups[gi].kind = kind;
            if matches!(kind, DeclKind::Let | DeclKind::ConfigLet) && defs.len() > 1 {
                for &d in &defs {
                    self.reassigned.insert(d, defs[1]);
                }
            }
            if kind == DeclKind::State {
                for t in &ops {
                    if let Some(k) = t.state_key {
                        self.state_groups.insert(k.0, gi);
                    }
                }
            }
        }

        // Enums: declared by a root-block binding phantom that computes
        // nothing; their constant data is published by `__enum_data`.
        let mut enum_group: HashMap<&str, usize> = HashMap::new();
        for term in &p.terms {
            if term.block_id != p.root_block || !Self::is_phantom(term) {
                continue;
            }
            let Some(name) = term.name.as_deref() else { continue };
            if !p.enums.contains_key(name) {
                continue;
            }
            let Some(span) = p.source_map.get(term.id).filter(|s| s.start.line > 0) else {
                continue;
            };
            let gi = self.groups.len();
            self.groups.push(Group {
                file: span.file,
                name: name.to_string(),
                defs: Vec::new(),
                kind: DeclKind::Enum,
                canonical: term.id,
                enum_records: Vec::new(),
                enum_calls: Vec::new(),
            });
            enum_group.insert(name, gi);
        }
        for term in &p.terms {
            if !self.is_builtin(term, ENUM_DATA) {
                continue;
            }
            let (Some(&tag), Some(&record)) = (term.inputs.first(), term.inputs.get(1)) else {
                continue;
            };
            let TermOp::Constant(tag_c) = p.get_term(tag).op else { continue };
            // The tag is a name the compiler wrote, not a literal of the
            // program's.
            let Some(enum_name) = split_enum_tag(self.string(tag_c)).0 else { continue };
            if let Some(&gi) = enum_group.get(enum_name) {
                self.groups[gi].enum_records.push(record);
                self.groups[gi].enum_calls.push(term.id);
            }
        }
    }

    // -----------------------------------------------------------------------
    // The fixpoint
    // -----------------------------------------------------------------------

    fn solve(&mut self) {
        // Each round can only lower a class, so this terminates; the cap is a
        // backstop that leaves anything unsettled dynamic-free but reported.
        for _ in 0..10_000 {
            let mut changed = false;
            for fi in 0..self.p.functions.len() {
                changed |= self.eval_block(self.func(fi).body_block);
                changed |= self.summarize(fi);
            }
            changed |= self.eval_block(self.p.root_block);
            if !changed {
                break;
            }
        }
        // What is still `Top` sits on a cycle nothing definite feeds — a
        // function that only ever calls itself. It never produces a value.
        for c in &mut self.class {
            if *c == Class::Top {
                *c = Class::Data;
            }
        }
        for c in &mut self.fn_ret {
            if *c == Class::Top {
                *c = Class::Data;
            }
        }
    }

    fn eval_block(&mut self, b: BlockId) -> bool {
        let mut changed = false;
        let phantoms = std::mem::take(&mut self.block_phantoms[b.0 as usize]);
        for &t in &phantoms {
            let c = self.phantom_class(t);
            changed |= self.set(t, c);
        }
        self.block_phantoms[b.0 as usize] = phantoms;
        let block = self.block(b);
        for &t in &block.terms {
            let term = self.p.get_term(t);
            for child in self.blocks_of(term) {
                changed |= self.eval_block(child);
            }
            let c = self.transfer(term);
            changed |= self.set(t, c);
        }
        changed
    }

    /// Lower a term's class to `new`. A class only ever moves down: `Top`,
    /// then data or a function set (which may grow), then dynamic, which is
    /// final and keeps its first reason.
    fn set(&mut self, t: TermId, new: Class) -> bool {
        let old = &self.class[t.0 as usize];
        let next = match (old, new) {
            (Class::Dynamic(_), _) | (_, Class::Top) => return false,
            (Class::Top, n) => n,
            (_, Class::Dynamic(w)) => Class::Dynamic(w),
            (o, n) => join(o.clone(), n),
        };
        if next == *old {
            return false;
        }
        self.class[t.0 as usize] = next;
        true
    }

    fn summarize(&mut self, fi: usize) -> bool {
        let body = self.block(self.func(fi).body_block);
        let dynamic = body.terms.iter().copied().find(|t| self.cls(*t).is_dynamic());
        let mut ret = self.block_result(body.id);
        for &r in &self.returns[fi] {
            let value = match self.p.get_term(r).inputs.first() {
                Some(v) => self.cls(*v).clone(),
                None => Class::Data,
            };
            ret = join(ret, value);
        }
        let changed = dynamic != self.fn_dyn[fi] || ret != self.fn_ret[fi];
        self.fn_dyn[fi] = dynamic;
        self.fn_ret[fi] = ret;
        changed
    }

    /// The class of a block's value: its last term's, or data (nil) when it
    /// is empty. A block that ends by leaving (`return`, `break`, `continue`)
    /// yields no value.
    fn block_result(&self, b: BlockId) -> Class {
        match self.block(b).terms.last() {
            None => Class::Data,
            Some(&t) => match self.p.get_term(t).op {
                TermOp::Return | TermOp::Break | TermOp::Continue => Class::Top,
                _ => self.cls(t).clone(),
            },
        }
    }

    /// The first dynamic term in any block `term` runs.
    fn region_dynamic(&self, term: &Term) -> Option<TermId> {
        for b in self.blocks_of(term) {
            if let Some(&t) = self.block(b).terms.iter().find(|t| self.cls(**t).is_dynamic()) {
                return Some(t);
            }
        }
        None
    }

    /// Data when every input is data; dynamic when one is dynamic, or is a
    /// function (`on_function` says why that is not allowed here).
    fn all_data(&self, inputs: &[TermId], on_function: Why) -> Class {
        let mut top = false;
        let mut function = false;
        for &i in inputs {
            match self.cls(i) {
                Class::Dynamic(_) => return Class::Dynamic(Why::Input(i)),
                Class::Function(_) => function = true,
                Class::Top => top = true,
                Class::Data => {}
            }
        }
        if function {
            Class::Dynamic(on_function)
        } else if top {
            Class::Top
        } else {
            Class::Data
        }
    }

    fn phantom_class(&self, tid: TermId) -> Class {
        let term = self.p.get_term(tid);
        let b = term.block_id;
        if b == self.p.root_block {
            // The native table: one phantom per registered native, in
            // registration order, ahead of everything else.
            let id = NativeFnId(tid.0);
            if (tid.0 as usize) < self.natives.count()
                && term.name.as_deref() == Some(self.natives.get_name(id))
            {
                return Class::Function(smallvec::smallvec![Target::Native(id.0)]);
            }
            return Class::Dynamic(Why::UnknownBinding);
        }
        if let Some(&fi) = self.body_fn.get(&b) {
            let f = self.func(fi);
            if f.self_ref_register == Some(term.register) {
                return Class::Function(smallvec::smallvec![Target::User(fi as u32)]);
            }
            if let Some(slot) = f.capture_registers.iter().position(|r| *r == term.register) {
                if self.cells.decl_of(tid).is_some() {
                    return Class::Dynamic(Why::Var);
                }
                let mut c = Class::Top;
                for &site in &self.sites[fi] {
                    let Some(&outer) = self.p.get_term(site).inputs.get(slot) else {
                        return Class::Dynamic(Why::UnknownBinding);
                    };
                    if self.cells.is_decl(outer) || self.cells.decl_of(outer).is_some() {
                        return Class::Dynamic(Why::Var);
                    }
                    // One arity of an overloaded function calling another
                    // (`fn f(a) f(a, 0) end`): the capture of its own name is
                    // nil when the closure is made and is patched to the
                    // overload set when the set is (`make_overload_set`).
                    if self.is_overload_self_capture(f, slot, outer)
                        && let Some(sets) = self.sets_of.get(&site)
                    {
                        for &set in sets {
                            c = match self.cls(set) {
                                Class::Dynamic(_) => return Class::Dynamic(Why::Input(set)),
                                other => join(c, other.clone()),
                            };
                        }
                        continue;
                    }
                    c = match self.cls(outer) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(outer)),
                        other => join(c, other.clone()),
                    };
                }
                return c;
            }
            if (term.register.0 as usize) < f.bound_arg_count() {
                // A parameter, or the was-it-passed flag of an optional one.
                // Assumed data; every static call checks its arguments are.
                return Class::Data;
            }
            return Class::Dynamic(Why::UnknownBinding);
        }
        // A loop variable or a pattern variable: as static as what the loop
        // iterates or the match inspects.
        match self.parent[b.0 as usize] {
            Some(pt) => {
                let parent = self.p.get_term(pt);
                if parent.inputs.is_empty() {
                    Class::Dynamic(Why::UnknownBinding)
                } else {
                    self.all_data(&parent.inputs, Why::FunctionOperand)
                }
            }
            None => Class::Dynamic(Why::UnknownBinding),
        }
    }

    /// Whether capture `slot` of `f` is the one `make_overload_set` patches:
    /// a capture of the function's own name whose value, where the closure is
    /// made, is a root-block placeholder that nothing ever writes.
    fn is_overload_self_capture(&self, f: &FunctionDef, slot: usize, outer: TermId) -> bool {
        let Some(own) = f.name.as_deref().map(base_fn_name) else {
            return false;
        };
        let o = self.p.get_term(outer);
        f.capture_names.get(slot).map(String::as_str) == Some(own)
            && o.block_id == self.p.root_block
            && Self::is_phantom(o)
            && !matches!(self.cls(outer), Class::Function(_))
    }

    fn transfer(&self, term: &Term) -> Class {
        if let Some(&at) = self.reassigned.get(&term.id) {
            return Class::Dynamic(Why::Reassigned(at));
        }
        let inputs = &term.inputs[..];
        match &term.op {
            TermOp::Constant(_) => Class::Data,
            TermOp::Error(_) => Class::Dynamic(Why::CompileError),

            TermOp::Add
            | TermOp::Sub
            | TermOp::Mul
            | TermOp::Div
            | TermOp::Mod
            | TermOp::Neg
            | TermOp::Eq
            | TermOp::Ne
            | TermOp::Lt
            | TermOp::Le
            | TermOp::Gt
            | TermOp::Ge
            | TermOp::Not
            | TermOp::Concat
            | TermOp::GetIndex
            | TermOp::GetIndexOpt
            | TermOp::SetField(_)
            | TermOp::SetIndex => self.all_data(inputs, Why::FunctionOperand),

            TermOp::AllocList
            | TermOp::AllocMap { .. }
            | TermOp::AllocMapSpread { .. }
            | TermOp::AllocElement { .. }
            | TermOp::MakeEnumVariant(_) => self.all_data(inputs, Why::HoldsFunction),

            TermOp::GetField(c) | TermOp::GetFieldOpt(c) => {
                let obj = self.all_data(inputs, Why::FunctionOperand);
                if obj.is_dynamic() {
                    return obj;
                }
                match self.enum_key(self.string(*c)) {
                    Class::Data => obj,
                    other => other,
                }
            }

            TermOp::Copy => match inputs.first() {
                Some(&i) => match self.cls(i) {
                    Class::Dynamic(_) => Class::Dynamic(Why::Input(i)),
                    other => other.clone(),
                },
                // A binding phantom; `eval_block` classifies those.
                None => self.cls(term.id).clone(),
            },

            TermOp::Phi => {
                let mut c = Class::Top;
                if let Some(&i) = inputs.first() {
                    c = match self.cls(i) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(i)),
                        other => other.clone(),
                    };
                }
                for &(src, control) in self.phi_srcs.get(&term.id).map(|v| &v[..]).unwrap_or(&[]) {
                    // Which rebinding supplied the value is decided by the
                    // control flow, so that has to be static too.
                    if let Some(control) = control
                        && self.cls(control).is_dynamic()
                    {
                        return Class::Dynamic(Why::Input(control));
                    }
                    c = match self.cls(src) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(src)),
                        other => join(c, other.clone()),
                    };
                }
                c
            }

            TermOp::Branch | TermOp::Match | TermOp::And | TermOp::Or | TermOp::Coalesce => {
                let cond = self.all_data(inputs, Why::FunctionOperand);
                if cond.is_dynamic() {
                    return cond;
                }
                if let Some(t) = self.region_dynamic(term) {
                    return Class::Dynamic(Why::Input(t));
                }
                let mut value = match term.op {
                    // The left operand is one of the values these yield.
                    TermOp::And | TermOp::Or | TermOp::Coalesce => Class::Data,
                    _ => Class::Top,
                };
                let guards = self.guards.get(&term.id);
                for b in self.blocks_of(term) {
                    if guards.is_some_and(|g| g.contains(&b)) {
                        continue;
                    }
                    value = join(value, self.block_result(b));
                }
                match (cond, value) {
                    (_, Class::Dynamic(w)) => Class::Dynamic(w),
                    (Class::Top, _) => Class::Top,
                    (_, v) => v,
                }
            }

            TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop => {
                let c = self.all_data(inputs, Why::FunctionOperand);
                if c.is_dynamic() {
                    return c;
                }
                if let Some(t) = self.region_dynamic(term) {
                    return Class::Dynamic(Why::Input(t));
                }
                if term.collect
                    && let Some(&body) = term.child_blocks.last()
                    && matches!(self.block_result(body), Class::Function(_))
                {
                    return Class::Dynamic(Why::HoldsFunction);
                }
                c
            }

            TermOp::Break | TermOp::Continue => Class::Data,
            TermOp::Return => match inputs.first().map(|i| (i, self.cls(*i))) {
                Some((&i, Class::Dynamic(_))) => Class::Dynamic(Why::Input(i)),
                Some((_, Class::Top)) => Class::Top,
                _ => Class::Data,
            },

            TermOp::MakeClosure(fid) => match self.fn_index.get(fid) {
                Some(&fi) => Class::Function(smallvec::smallvec![Target::User(fi as u32)]),
                None => Class::Dynamic(Why::UnknownBinding),
            },
            TermOp::MakeOverloadSet => {
                let mut c = Class::Top;
                for &i in inputs {
                    c = match self.cls(i) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(i)),
                        Class::Data => return Class::Dynamic(Why::NotCallable),
                        other => join(c, other.clone()),
                    };
                }
                c
            }

            TermOp::Call => {
                let Some((&callee, args)) = inputs.split_first() else {
                    return Class::Dynamic(Why::UnknownBinding);
                };
                if let Some(&a) = args.iter().find(|a| self.cls(**a).is_dynamic()) {
                    return Class::Dynamic(Why::Input(a));
                }
                match self.cls(callee) {
                    Class::Dynamic(_) => Class::Dynamic(Why::Input(callee)),
                    Class::Data => Class::Dynamic(Why::NotCallable),
                    Class::Top => Class::Top,
                    Class::Function(targets) => self.call(targets, args, Why::Calls),
                }
            }

            TermOp::MethodCall { name, .. } => self.method_call(*name, inputs),

            TermOp::BuiltinCall(name_c) => {
                let name = self.string(*name_c);
                if name == ENUM_DATA {
                    // Publishes a variant's constant data: as static as the
                    // record it publishes.
                    return match inputs.get(1) {
                        Some(&r) => match self.cls(r) {
                            Class::Dynamic(_) => Class::Dynamic(Why::Input(r)),
                            Class::Function(_) => Class::Dynamic(Why::HoldsFunction),
                            other => other.clone(),
                        },
                        None => Class::Dynamic(Why::UnknownBinding),
                    };
                }
                if name == crate::classes::DECLARE_METHOD_BUILTIN {
                    // Registers `fn Class.method`: the same registration on
                    // every run.
                    return Class::Data;
                }
                if name == ENUM_FROM || name == ENUM_ALL {
                    // Answered from the program's own enum declarations.
                    return self.all_data(inputs, Why::FunctionArgument);
                }
                if let Some(&a) = inputs.iter().find(|a| self.cls(**a).is_dynamic()) {
                    return Class::Dynamic(Why::Input(a));
                }
                match self.natives.lookup_name(name) {
                    Some(id) => self.native_call(id, inputs),
                    None => Class::Dynamic(Why::UnregisteredNative(*name_c)),
                }
            }

            TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite => {
                Class::Dynamic(Why::State)
            }

            TermOp::CellNew => {
                if self.fn_cells.contains_key(&term.id) {
                    return Class::Data;
                }
                if self.local_cell(term.id, term.block_id) {
                    return match inputs.first().map(|i| (i, self.cls(*i))) {
                        Some((&i, Class::Dynamic(_))) => Class::Dynamic(Why::Input(i)),
                        Some((_, Class::Top)) => Class::Top,
                        _ => Class::Data,
                    };
                }
                Class::Dynamic(Why::Var)
            }
            TermOp::CellRead => {
                let Some(&operand) = inputs.first() else {
                    return Class::Dynamic(Why::Var);
                };
                let Some(decl) = self.cells.decl_of(operand) else {
                    return Class::Dynamic(Why::Var);
                };
                if let Some(&write) = self.fn_cells.get(&decl) {
                    // A hoisted function, read through its cell. At the top
                    // level the read has to come after the declaration; a
                    // read inside a function runs whenever it is called, and
                    // one that runs too early calls nil and raises.
                    if let Some(at) = self.root_ancestor(term)
                        && self.root_pos.get(&at) < self.root_pos.get(&write)
                    {
                        return Class::Dynamic(Why::UsedBeforeDeclared);
                    }
                    let Some(&value) = self.p.get_term(write).inputs.get(1) else {
                        return Class::Dynamic(Why::Var);
                    };
                    return match self.cls(value) {
                        Class::Function(t) => Class::Function(t.clone()),
                        Class::Top => Class::Top,
                        Class::Dynamic(_) => Class::Dynamic(Why::Input(value)),
                        Class::Data => Class::Dynamic(Why::Var),
                    };
                }
                if operand != decl || !self.local_cell(decl, term.block_id) {
                    return Class::Dynamic(Why::Var);
                }
                // A local `var` of this function: whatever was ever put in it.
                let mut c = match self.p.get_term(decl).inputs.first() {
                    Some(&i) => match self.cls(i) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(i)),
                        other => other.clone(),
                    },
                    None => Class::Data,
                };
                for &w in self.cells.writes_of(decl) {
                    let Some(&v) = self.p.get_term(w).inputs.get(1) else { continue };
                    c = match self.cls(v) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::Input(v)),
                        other => join(c, other.clone()),
                    };
                }
                match c {
                    Class::Function(_) => Class::Dynamic(Why::FunctionOperand),
                    other => other,
                }
            }
            TermOp::CellWrite => {
                let (Some(&operand), Some(&value)) = (inputs.first(), inputs.get(1)) else {
                    return Class::Dynamic(Why::Var);
                };
                let Some(decl) = self.cells.decl_of(operand) else {
                    return Class::Dynamic(Why::Var);
                };
                if self.fn_cells.get(&decl) == Some(&term.id) {
                    return match self.cls(value) {
                        Class::Function(_) => Class::Data,
                        Class::Top => Class::Top,
                        _ => Class::Dynamic(Why::Var),
                    };
                }
                if operand != decl || !self.local_cell(decl, term.block_id) {
                    return Class::Dynamic(Why::Var);
                }
                match self.cls(value) {
                    Class::Dynamic(_) => Class::Dynamic(Why::Input(value)),
                    Class::Top => Class::Top,
                    _ => Class::Data,
                }
            }
        }
    }

    /// Is `decl` a `var` of the function whose body `from` is in? A cell the
    /// top level declares is never local: nothing bounds who writes it.
    fn local_cell(&self, decl: TermId, from: BlockId) -> bool {
        let d = self.p.get_term(decl);
        if !matches!(d.op, TermOp::CellNew) {
            return false;
        }
        let owner = self.owner.get(d.block_id.0 as usize).copied().flatten();
        owner.is_some() && owner == self.owner.get(from.0 as usize).copied().flatten()
    }

    /// The root-block term a top-level term sits in (itself, when it is in
    /// the root block). `None` for a term inside a function.
    fn root_ancestor(&self, term: &Term) -> Option<TermId> {
        let mut t = term.id;
        let mut b = term.block_id;
        for _ in 0..10_000 {
            if b == self.p.root_block {
                return Some(t);
            }
            t = self.parent.get(b.0 as usize).copied().flatten()?;
            b = self.p.get_term(t).block_id;
        }
        None
    }

    /// Whether a field or method of this name can read enum constant data,
    /// and if so whether that data is static: `Data` when it cannot or the
    /// data is, `Top` while unsettled, otherwise dynamic.
    fn enum_key(&self, key: &str) -> Class {
        match self.enum_keys.get(key) {
            Some(records) => {
                let mut top = false;
                for &r in records {
                    match self.cls(r) {
                        Class::Dynamic(_) => return Class::Dynamic(Why::EnumData(r)),
                        Class::Function(_) => return Class::Dynamic(Why::EnumData(r)),
                        Class::Top => top = true,
                        Class::Data => {}
                    }
                }
                if top { Class::Top } else { Class::Data }
            }
            None if self.declared_enum_keys.contains(key) => {
                Class::Dynamic(Why::UnknownEnumData)
            }
            None => Class::Data,
        }
    }

    /// A call that may reach any of `targets` with `args` (none of which is
    /// dynamic — the caller checked). `impure` builds the reason for a user
    /// function that is not static-pure.
    fn call(&self, targets: &Targets, args: &[TermId], impure: fn(FunctionId) -> Why) -> Class {
        let function_arg = args.iter().any(|a| matches!(self.cls(*a), Class::Function(_)));
        let mut top = args.iter().any(|a| *self.cls(*a) == Class::Top);
        let mut ret = Class::Top;
        for t in targets {
            match *t {
                Target::User(fi) => {
                    let fi = fi as usize;
                    if function_arg {
                        // A parameter is assumed to be data. A function
                        // passed in could be called, and nothing here says
                        // what it would do.
                        return Class::Dynamic(Why::FunctionArgument);
                    }
                    if self.fn_dyn[fi].is_some() {
                        return Class::Dynamic(impure(self.func(fi).id));
                    }
                    ret = join(ret, self.fn_ret[fi].clone());
                }
                Target::Native(id) => match self.native_call(NativeFnId(id), args) {
                    Class::Dynamic(w) => return Class::Dynamic(w),
                    Class::Top => top = true,
                    other => ret = join(ret, other),
                },
            }
        }
        match ret {
            Class::Dynamic(w) => Class::Dynamic(w),
            _ if top => Class::Top,
            other => other,
        }
    }

    /// A call of a registered native with `args`.
    fn native_call(&self, id: NativeFnId, args: &[TermId]) -> Class {
        let callbacks = match self.natives.constness(id) {
            NativeConstness::Dynamic => return Class::Dynamic(Why::Native(id)),
            NativeConstness::Const => false,
            NativeConstness::ConstIfCallbacksAre => true,
        };
        let mut top = false;
        for &a in args {
            match self.cls(a) {
                Class::Dynamic(_) => return Class::Dynamic(Why::Input(a)),
                Class::Top => top = true,
                Class::Data => {}
                Class::Function(_) if !callbacks => {
                    return Class::Dynamic(Why::FunctionArgument);
                }
                Class::Function(targets) => {
                    for t in targets {
                        match *t {
                            Target::User(fi) => {
                                let fi = fi as usize;
                                if self.fn_dyn[fi].is_some() {
                                    return Class::Dynamic(Why::Callback(self.func(fi).id));
                                }
                                match &self.fn_ret[fi] {
                                    Class::Function(_) => {
                                        return Class::Dynamic(Why::CallbackReturnsFunction);
                                    }
                                    Class::Dynamic(w) => return Class::Dynamic(*w),
                                    Class::Top => top = true,
                                    Class::Data => {}
                                }
                            }
                            Target::Native(n) => {
                                if self.natives.constness(NativeFnId(n)) != NativeConstness::Const {
                                    return Class::Dynamic(Why::Native(NativeFnId(n)));
                                }
                            }
                        }
                    }
                }
            }
        }
        if top { Class::Top } else { Class::Data }
    }

    /// `recv.name(args)`. Dispatch is by name at run time, so the call is
    /// static only if everything that name can reach is: every user method
    /// of that name, every built-in class method of that name, and the
    /// global native of that name the call can fall back to. (The fourth
    /// thing a method call can reach — a function held in a record field — is
    /// excluded by the receiver being data, which holds no function.)
    fn method_call(&self, name_c: ConstantId, inputs: &[TermId]) -> Class {
        let Some((&recv, args)) = inputs.split_first() else {
            return Class::Dynamic(Why::UnknownBinding);
        };
        if let Some(&a) = inputs.iter().find(|a| self.cls(**a).is_dynamic()) {
            return Class::Dynamic(Why::Input(a));
        }
        if matches!(self.cls(recv), Class::Function(_)) {
            return Class::Dynamic(Why::FunctionOperand);
        }
        let name = self.string(name_c);
        let mut top = inputs.iter().any(|a| *self.cls(*a) == Class::Top);
        // An enum variant's constant data is looked up by the method's name
        // first, like a record field.
        match self.enum_key(name) {
            Class::Dynamic(w) => return Class::Dynamic(w),
            Class::Top => top = true,
            _ => {}
        }
        let mut any = false;
        let mut ret = Class::Top;
        for &func in self.dispatch.get(&name_c).map(|v| &v[..]).unwrap_or(&[]) {
            any = true;
            match self.cls(func) {
                Class::Function(targets) => match self.call(targets, args, Why::Method) {
                    Class::Dynamic(w) => return Class::Dynamic(w),
                    Class::Top => top = true,
                    other => ret = join(ret, other),
                },
                Class::Top => top = true,
                _ => return Class::Dynamic(Why::Input(func)),
            }
        }
        let class_methods = self.class_natives.get(name).map(|v| &v[..]).unwrap_or(&[]);
        for &id in class_methods.iter().chain(self.natives.lookup_name(name).iter()) {
            any = true;
            match self.native_call(id, inputs) {
                Class::Dynamic(w) => return Class::Dynamic(w),
                Class::Top => top = true,
                other => ret = join(ret, other),
            }
        }
        if !any {
            return Class::Dynamic(Why::UnknownMethod);
        }
        match ret {
            Class::Dynamic(w) => Class::Dynamic(w),
            _ if top => Class::Top,
            other => other,
        }
    }

    // -----------------------------------------------------------------------
    // Reason chains
    // -----------------------------------------------------------------------

    fn fn_display(&self, fid: FunctionId) -> String {
        let Some(&fi) = self.fn_index.get(&fid) else {
            return "a function".to_string();
        };
        match self.func(fi).name.as_deref() {
            Some(n) => base_fn_name(n).to_string(),
            None => {
                let line = self.sites[fi].first().and_then(|s| self.loc(*s)).map(|l| l.1);
                match line {
                    Some(l) => format!("the function at line {l}"),
                    None => "an anonymous function".to_string(),
                }
            }
        }
    }

    fn own_span(&self, t: TermId) -> Option<&'a crate::source_map::SourceSpan> {
        self.p.source_map.get(t).filter(|s| s.start.line > 0)
    }

    /// Where a term is written. The compiler gives some terms no position of
    /// their own (a `state` declaration, the copy an assignment rebinds a
    /// name with); those answer with the nearest term that has one: the
    /// first of a child block, else the closest operand.
    fn span(&self, t: TermId) -> Option<&'a crate::source_map::SourceSpan> {
        if let Some(s) = self.own_span(t) {
            return Some(s);
        }
        let term = self.p.get_term(t);
        for b in self.blocks_of(term) {
            if let Some(s) = self.block(b).terms.iter().find_map(|c| self.own_span(*c)) {
                return Some(s);
            }
        }
        let mut level: Vec<TermId> = term.inputs.to_vec();
        for _ in 0..4 {
            if let Some(s) = level.iter().find_map(|i| self.own_span(*i)) {
                return Some(s);
            }
            level = level
                .iter()
                .flat_map(|i| self.p.get_term(*i).inputs.iter().copied())
                .collect();
            if level.is_empty() {
                break;
            }
        }
        None
    }

    /// `(file display name, line, column)` of where a term is written.
    fn loc(&self, t: TermId) -> Option<(Option<String>, u32, u32)> {
        let span = self.span(t)?;
        let file = self.p.source_map.file(span.file).map(|f| f.name.clone());
        Some((file, span.start.line, span.start.column))
    }

    fn step(&self, subject: &str, text: String, at: TermId) -> ReasonStep {
        let loc = self.loc(at);
        ReasonStep {
            subject: subject.to_string(),
            text,
            file: loc.as_ref().and_then(|l| l.0.clone()),
            line: loc.as_ref().map(|l| l.1),
            column: loc.as_ref().map(|l| l.2),
        }
    }

    fn native_text(&self, name: &str, registered: bool) -> String {
        if registered {
            format!("calls {name}, which is not declared constant")
        } else {
            format!("calls {name}, a native that is not registered here")
        }
    }

    /// The chain from a dynamic term down to its root cause.
    fn explain(&self, subject: &str, start: TermId) -> Vec<ReasonStep> {
        let mut steps = Vec::new();
        let mut subject = subject.to_string();
        let mut cur = start;
        let mut seen: HashSet<TermId> = HashSet::new();
        // The nearest term on the way down that has a source position, for a
        // link whose own term has none.
        let mut at = start;
        loop {
            if !seen.insert(cur) || steps.len() >= 12 {
                break;
            }
            if self.loc(cur).is_some() {
                at = cur;
            }
            let term = self.p.get_term(cur);
            let Class::Dynamic(why) = *self.cls(cur) else {
                break;
            };
            let enter = |pass: &Self, fid: FunctionId| -> Option<TermId> {
                pass.fn_index.get(&fid).and_then(|&fi| pass.fn_dyn[fi])
            };
            match why {
                Why::Input(i) => {
                    // Crossing into another declaration is a link worth
                    // showing; an operand of the same expression is not.
                    if let Some(&gi) = self.group_of.get(&i)
                        && self.groups[gi].name != subject
                    {
                        let name = self.groups[gi].name.clone();
                        steps.push(self.step(&subject, format!("reads {name}"), at));
                        subject = name;
                        at = i;
                    }
                    cur = i;
                }
                Why::Calls(fid) | Why::Callback(fid) | Why::Method(fid) => {
                    let name = self.fn_display(fid);
                    let text = match why {
                        Why::Calls(_) => format!("calls {name}"),
                        Why::Callback(_) => format!("passes {name} to a higher-order builtin"),
                        _ => {
                            let method = match term.op {
                                TermOp::MethodCall { name, .. } => self.string(name),
                                _ => "",
                            };
                            format!("calls method .{method}, which may be {name}")
                        }
                    };
                    steps.push(self.step(&subject, text, at));
                    subject = name;
                    match enter(self, fid) {
                        Some(next) => {
                            cur = next;
                            at = next;
                        }
                        None => break,
                    }
                }
                Why::EnumData(record) => {
                    let key = match term.op {
                        TermOp::GetField(c) | TermOp::GetFieldOpt(c) => self.string(c),
                        TermOp::MethodCall { name, .. } => self.string(name),
                        _ => "",
                    };
                    steps.push(self.step(
                        &subject,
                        format!("reads .{key}, which may be enum constant data that is not static"),
                        at,
                    ));
                    subject = "that enum data".to_string();
                    cur = record;
                    at = record;
                }
                terminal => {
                    let text = match terminal {
                        Why::Native(id) => self.native_text(self.natives.get_name(id), true),
                        Why::UnregisteredNative(c) => self.native_text(self.string(c), false),
                        Why::UnknownEnumData => {
                            "reads enum constant data the classifier cannot find".to_string()
                        }
                        Why::State => match term.op {
                            TermOp::StateInit => "is state, which changes between runs".to_string(),
                            TermOp::StateWrite => "writes state".to_string(),
                            _ => "reads state".to_string(),
                        },
                        Why::Var => {
                            let name = term
                                .inputs
                                .first()
                                .and_then(|c| self.cells.decl_of(*c))
                                .and_then(|d| self.cells.var_name(d))
                                .map(|n| format!(" {n}"))
                                .unwrap_or_default();
                            match term.op {
                                TermOp::CellNew => "is a var, which can be written at any time".to_string(),
                                TermOp::CellWrite => format!("writes the var{name}"),
                                _ => format!("reads the var{name}"),
                            }
                        }
                        Why::Reassigned(other) => {
                            at = other;
                            "is assigned again (a value that is mutated is never kept)".to_string()
                        }
                        Why::UsedBeforeDeclared => {
                            "uses a function before its declaration has run".to_string()
                        }
                        Why::NotCallable => {
                            // A call of a parameter is the usual way here.
                            let mut callee = term.inputs.first().copied();
                            while let Some(c) = callee {
                                let ct = self.p.get_term(c);
                                if matches!(ct.op, TermOp::Copy) && !ct.inputs.is_empty() {
                                    callee = ct.inputs.first().copied();
                                } else {
                                    break;
                                }
                            }
                            let param = callee
                                .map(|c| self.p.get_term(c))
                                .filter(|ct| Self::is_phantom(ct))
                                .and_then(|ct| ct.name.clone());
                            match param {
                                Some(name) => format!(
                                    "calls {name}, a function it was passed (a parameter is taken to be data)"
                                ),
                                None => "calls a value that is not a function".to_string(),
                            }
                        }
                        Why::UnknownMethod => {
                            let method = match term.op {
                                TermOp::MethodCall { name, .. } => self.string(name),
                                _ => "",
                            };
                            format!(
                                "calls method .{method}, which no method or constant native here defines"
                            )
                        }
                        Why::HoldsFunction => {
                            "holds a function (a kept value cannot contain a closure)".to_string()
                        }
                        Why::FunctionOperand => "computes with a function value".to_string(),
                        Why::FunctionArgument => {
                            "passes a function as an argument (only map, filter, reduce, sort and sort_by take one in a static value)".to_string()
                        }
                        Why::MixedValue => {
                            "is a function on one path and data on another".to_string()
                        }
                        Why::CallbackReturnsFunction => {
                            "passes a callback that returns a function".to_string()
                        }
                        Why::UnknownBinding => {
                            "reads a binding the classifier cannot resolve".to_string()
                        }
                        Why::CompileError => "has a compile error".to_string(),
                        Why::Input(_)
                        | Why::Calls(_)
                        | Why::Callback(_)
                        | Why::Method(_)
                        | Why::EnumData(_) => unreachable!(),
                    };
                    steps.push(self.step(&subject, text, at));
                    break;
                }
            }
        }
        steps
    }

    // -----------------------------------------------------------------------
    // The report
    // -----------------------------------------------------------------------

    /// Module canonical name per file id; the entry file has none.
    fn module_names(&self) -> HashMap<u16, String> {
        self.p
            .imports
            .files
            .iter()
            .map(|(name, id)| (*id, name.clone()))
            .collect()
    }

    fn qualified(&self, modules: &HashMap<u16, String>, gi: usize) -> String {
        let g = &self.groups[gi];
        match modules.get(&g.file.0) {
            Some(m) if g.file != crate::source_map::ENTRY_FILE => format!("{m}::{}", g.name),
            _ => g.name.clone(),
        }
    }

    /// The declarations a group's terms refer to directly.
    fn direct_deps(&self, gi: usize, enum_groups: &HashMap<&str, usize>) -> BTreeSet<usize> {
        let p = self.p;
        let g = &self.groups[gi];
        let mut deps: BTreeSet<usize> = BTreeSet::new();
        let mut seen: HashSet<TermId> = HashSet::new();
        let mut seen_fns: HashSet<usize> = HashSet::new();
        let mut stack: Vec<TermId> = g.defs.clone();
        stack.extend(&g.enum_calls);
        // A reference to `t` from inside this group: another declaration is a
        // dependency and is not walked into; anything else is part of this
        // one's own expression.
        let refer = |t: TermId, deps: &mut BTreeSet<usize>, stack: &mut Vec<TermId>| {
            match self.group_of.get(&t) {
                Some(&other) if other != gi => {
                    deps.insert(other);
                }
                _ => stack.push(t),
            }
        };
        let enum_dep = |name: &str, deps: &mut BTreeSet<usize>| {
            if let Some(&e) = enum_groups.get(name)
                && e != gi
            {
                deps.insert(e);
            }
        };
        while let Some(t) = stack.pop() {
            if !seen.insert(t) {
                continue;
            }
            let term = p.get_term(t);
            for &i in &term.inputs {
                refer(i, &mut deps, &mut stack);
            }
            for b in self.blocks_of(term) {
                stack.extend(&self.block(b).terms);
            }
            match &term.op {
                TermOp::MakeClosure(fid) => {
                    if let Some(&fi) = self.fn_index.get(fid)
                        && seen_fns.insert(fi)
                    {
                        // The body's block tree; its child blocks are reached
                        // through the control terms that own them.
                        stack.extend(&self.block(self.func(fi).body_block).terms);
                    }
                }
                TermOp::MethodCall { name, .. } => {
                    for &f in self.dispatch.get(name).map(|v| &v[..]).unwrap_or(&[]) {
                        refer(f, &mut deps, &mut stack);
                    }
                    self.enum_key_deps(self.string(*name), enum_groups, gi, &mut deps);
                }
                TermOp::GetField(c) | TermOp::GetFieldOpt(c) => {
                    self.enum_key_deps(self.string(*c), enum_groups, gi, &mut deps);
                }
                TermOp::MakeEnumVariant(tag) => {
                    if let Some(name) = split_enum_tag(self.string(*tag)).0 {
                        enum_dep(name, &mut deps);
                    }
                }
                TermOp::BuiltinCall(c) if matches!(self.string(*c), ENUM_FROM | ENUM_ALL) => {
                    // The enum's name, written by the compiler.
                    if let Some(&first) = term.inputs.first()
                        && let TermOp::Constant(nc) = p.get_term(first).op
                    {
                        enum_dep(self.string(nc), &mut deps);
                    }
                }
                TermOp::CellRead | TermOp::CellWrite => {
                    if let Some(decl) = term.inputs.first().and_then(|c| self.cells.decl_of(*c)) {
                        match self.fn_cells.get(&decl) {
                            // A hoisted function: the dependency is on what
                            // its declaration writes.
                            Some(&write) => {
                                if let Some(&value) = p.get_term(write).inputs.get(1) {
                                    refer(value, &mut deps, &mut stack);
                                }
                            }
                            None => {
                                refer(decl, &mut deps, &mut stack);
                                if let Some(k) = p.get_term(decl).state_key
                                    && let Some(&sg) = self.state_groups.get(&k.0)
                                    && sg != gi
                                {
                                    deps.insert(sg);
                                }
                            }
                        }
                    }
                }
                TermOp::StateRead | TermOp::StateWrite => {
                    if let Some(k) = term.state_key
                        && let Some(&sg) = self.state_groups.get(&k.0)
                        && sg != gi
                    {
                        deps.insert(sg);
                    }
                }
                TermOp::Phi => {
                    for &(src, _) in self.phi_srcs.get(&t).map(|v| &v[..]).unwrap_or(&[]) {
                        refer(src, &mut deps, &mut stack);
                    }
                }
                _ => {}
            }
        }
        deps
    }

    fn enum_key_deps(
        &self,
        key: &str,
        enum_groups: &HashMap<&str, usize>,
        gi: usize,
        deps: &mut BTreeSet<usize>,
    ) {
        if !self.declared_enum_keys.contains(key) {
            return;
        }
        for (name, def) in &self.p.enums {
            if def.variants.iter().any(|v| v.data.iter().any(|k| k == key))
                && let Some(&e) = enum_groups.get(name.as_str())
                && e != gi
            {
                deps.insert(e);
            }
        }
    }

    fn group_class(&self, gi: usize) -> (DeclClass, Option<bool>, Vec<ReasonStep>) {
        let g = &self.groups[gi];
        if g.kind == DeclKind::Enum {
            let bad = g.enum_records.iter().copied().find(|r| self.cls(*r).is_dynamic());
            return match bad {
                Some(r) => (DeclClass::Dynamic, None, self.explain(&g.name, r)),
                None => (DeclClass::Static, None, Vec::new()),
            };
        }
        match self.cls(g.canonical) {
            Class::Data | Class::Top => (DeclClass::Static, None, Vec::new()),
            Class::Dynamic(_) => (DeclClass::Dynamic, None, self.explain(&g.name, g.canonical)),
            Class::Function(targets) => {
                for t in targets {
                    match *t {
                        Target::User(fi) => {
                            let fi = fi as usize;
                            if let Some(first) = self.fn_dyn[fi] {
                                let subject = self.fn_display(self.func(fi).id);
                                return (
                                    DeclClass::Function,
                                    Some(false),
                                    self.explain(&subject, first),
                                );
                            }
                        }
                        Target::Native(id) => {
                            let id = NativeFnId(id);
                            if self.natives.constness(id) == NativeConstness::Dynamic {
                                let name = self.natives.get_name(id);
                                let step = self.step(
                                    &g.name,
                                    format!("is {name}, which is not declared constant"),
                                    g.canonical,
                                );
                                return (DeclClass::Function, Some(false), vec![step]);
                            }
                        }
                    }
                }
                (DeclClass::Function, Some(true), Vec::new())
            }
        }
    }

    /// Who reads each static-data declaration, and the first place (if any)
    /// its value is handed to an operation that may update it in place.
    fn readers(&self, classes: &[DeclClass]) -> Vec<(Vec<Reader>, Option<ReasonStep>)> {
        let p = self.p;
        let mut users: HashMap<TermId, Vec<TermId>> = HashMap::new();
        for term in &p.terms {
            for &i in &term.inputs {
                users.entry(i).or_default().push(term.id);
            }
        }
        // Which declaration's initializer each unnamed root-block term is
        // part of: walk back from every definition through unnamed terms.
        let mut root_owner: HashMap<TermId, usize> = HashMap::new();
        for (gi, g) in self.groups.iter().enumerate() {
            let mut stack: Vec<TermId> = g.defs.iter().chain(&g.enum_calls).copied().collect();
            for &c in &g.enum_calls {
                root_owner.insert(c, gi);
            }
            while let Some(t) = stack.pop() {
                for &i in &p.get_term(t).inputs {
                    let input = p.get_term(i);
                    if input.block_id == p.root_block
                        && self.root_pos.contains_key(&i)
                        && !self.group_of.contains_key(&i)
                        && !root_owner.contains_key(&i)
                    {
                        root_owner.insert(i, gi);
                        stack.push(i);
                    }
                }
            }
        }

        let mut out: Vec<(Vec<Reader>, Option<ReasonStep>)> =
            vec![(Vec::new(), None); self.groups.len()];
        for (gi, g) in self.groups.iter().enumerate() {
            if classes[gi] != DeclClass::Static || g.kind == DeclKind::Enum {
                continue;
            }
            let mut readers: Vec<Reader> = Vec::new();
            let mut push = |r: Reader| {
                if !readers.contains(&r) {
                    readers.push(r);
                }
            };
            let mut seen: HashSet<TermId> = HashSet::new();
            let mut stack: Vec<TermId> = g.defs.clone();
            // Everything that carries the same value under another name: a
            // copy (named or not) or a phi it seeds. The container operand
            // of a write or of a mutating builtin on any of them is a place
            // lowering may update the value in place.
            let mut mutation: Option<(usize, ReasonStep)> = None;
            let mut aliases: Vec<TermId> = g.defs.clone();
            let mut alias_seen: HashSet<TermId> = g.defs.iter().copied().collect();
            while let Some(t) = aliases.pop() {
                for &u in users.get(&t).map(|v| &v[..]).unwrap_or(&[]) {
                    let user = p.get_term(u);
                    if user.inputs.first() == Some(&t)
                        && let Some(what) = self.mutation_name(user)
                    {
                        let at = self
                            .root_ancestor(user)
                            .and_then(|r| self.root_pos.get(&r).copied())
                            .unwrap_or(usize::MAX);
                        if mutation.as_ref().is_none_or(|m| at < m.0) {
                            let text = format!("is passed to {what}, which may update it in place");
                            mutation = Some((at, self.step(&g.name, text, u)));
                        }
                    }
                    if matches!(user.op, TermOp::Copy | TermOp::Phi) && alias_seen.insert(u) {
                        aliases.push(u);
                    }
                }
            }
            while let Some(t) = stack.pop() {
                for &u in users.get(&t).map(|v| &v[..]).unwrap_or(&[]) {
                    let user = p.get_term(u);
                    if !seen.insert(u) {
                        continue;
                    }
                    let line = self.loc(u).map(|l| l.1);
                    // An unnamed copy is the reference itself; what reads the
                    // declaration is whatever reads the copy.
                    if matches!(user.op, TermOp::Copy) && !self.group_of.contains_key(&u) {
                        stack.push(u);
                        continue;
                    }
                    if let TermOp::MakeClosure(fid) = user.op {
                        push(Reader {
                            kind: ReaderKind::Capture,
                            by: Some(self.fn_display(fid)),
                            line,
                        });
                        continue;
                    }
                    let Some(root) = self.root_ancestor(user) else {
                        push(Reader { kind: ReaderKind::Dynamic, by: None, line });
                        continue;
                    };
                    let owner = self
                        .group_of
                        .get(&root)
                        .or_else(|| root_owner.get(&root))
                        .copied();
                    let dynamic_here =
                        self.cls(u).is_dynamic() || self.cls(root).is_dynamic();
                    match owner {
                        Some(o) if o == gi => {}
                        Some(o) if classes[o] == DeclClass::Static && !dynamic_here => {
                            push(Reader {
                                kind: ReaderKind::Static,
                                by: Some(self.groups[o].name.clone()),
                                line,
                            });
                        }
                        Some(o) => push(Reader {
                            kind: ReaderKind::Dynamic,
                            by: Some(self.groups[o].name.clone()),
                            line,
                        }),
                        None if dynamic_here => {
                            push(Reader { kind: ReaderKind::Dynamic, by: None, line });
                        }
                        None => push(Reader { kind: ReaderKind::Statement, by: None, line }),
                    }
                }
            }
            out[gi] = (readers, mutation.map(|m| m.1));
        }
        out
    }

    /// What a term does to its first operand, when that is something lowering
    /// may turn into an in-place update: an indexed or field write, or one of
    /// the mutating builtins (called bare or as a method).
    fn mutation_name(&self, term: &Term) -> Option<String> {
        let builtin = |c: ConstantId| {
            let name = self.string(c);
            crate::builtins::is_mutating_builtin(name).then(|| name.to_string())
        };
        match term.op {
            TermOp::SetIndex => Some("an indexed write".to_string()),
            TermOp::SetField(_) => Some("a field write".to_string()),
            TermOp::BuiltinCall(c) => builtin(c),
            TermOp::MethodCall { name, .. } => builtin(name),
            _ => None,
        }
    }

    fn report(&self) -> StaticReport {
        let p = self.p;
        let modules = self.module_names();
        let enum_groups: HashMap<&str, usize> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind == DeclKind::Enum)
            .map(|(i, g)| (g.name.as_str(), i))
            .collect();

        let n = self.groups.len();
        let classified: Vec<(DeclClass, Option<bool>, Vec<ReasonStep>)> =
            (0..n).map(|gi| self.group_class(gi)).collect();
        let classes: Vec<DeclClass> = classified.iter().map(|c| c.0).collect();
        let readers = self.readers(&classes);

        let direct: Vec<BTreeSet<usize>> =
            (0..n).map(|gi| self.direct_deps(gi, &enum_groups)).collect();
        // Closed transitively, each declaration from its own direct set.
        let closed: Vec<BTreeSet<usize>> = (0..n)
            .map(|gi| {
                let mut all: BTreeSet<usize> = BTreeSet::new();
                let mut stack: Vec<usize> = direct[gi].iter().copied().collect();
                while let Some(d) = stack.pop() {
                    if d != gi && all.insert(d) {
                        stack.extend(direct[d].iter().copied());
                    }
                }
                all
            })
            .collect();
        let names: Vec<String> = (0..n).map(|gi| self.qualified(&modules, gi)).collect();

        let mut decls: Vec<StaticDecl> = Vec::with_capacity(n);
        for (gi, (class, static_pure, reason)) in classified.into_iter().enumerate() {
            let g = &self.groups[gi];
            let at = match g.kind {
                DeclKind::Enum => g.canonical,
                _ => g.defs.first().copied().unwrap_or(g.canonical),
            };
            let loc = self.loc(at).or_else(|| self.loc(g.canonical));
            let module = (g.file != crate::source_map::ENTRY_FILE)
                .then(|| modules.get(&g.file.0).cloned())
                .flatten();
            let initializer_static = (g.kind == DeclKind::State).then(|| {
                let init = p.get_term(g.canonical);
                // A `state var` keeps a cell; only a plain `state` whose
                // initializer is static data could have been a constant.
                let block_static = |b: &BlockId| {
                    let terms = &self.block(*b).terms;
                    terms.iter().all(|t| {
                        !self.cls(*t).is_dynamic() && !matches!(p.get_term(*t).op, TermOp::CellNew)
                    }) && terms.last().is_none_or(|t| matches!(self.cls(*t), Class::Data))
                };
                init.inputs.iter().all(|i| matches!(self.cls(*i), Class::Data))
                    && init.child_blocks.iter().all(block_static)
            });
            let is_static = class == DeclClass::Static && g.kind != DeclKind::Enum;
            decls.push(StaticDecl {
                name: g.name.clone(),
                qualified: names[gi].clone(),
                module,
                kind: g.kind,
                file: p.source_map.file(g.file).map(|f| f.name.clone()),
                file_id: g.file,
                line: loc.as_ref().map(|l| l.1),
                column: loc.as_ref().map(|l| l.2),
                class,
                static_pure,
                initializer_static,
                reason,
                read_by_dynamic: is_static
                    .then(|| readers[gi].0.iter().any(|r| r.kind != ReaderKind::Static)),
                readers: readers[gi].0.clone(),
                mutated_by: readers[gi].1.clone(),
                direct_depends_on: direct[gi].iter().map(|d| names[*d].clone()).collect(),
                depends_on: closed[gi].iter().map(|d| names[*d].clone()).collect(),
                term: g.canonical,
            });
        }
        decls.sort_by_key(|d| (d.file_id.0, d.line.unwrap_or(0), d.column.unwrap_or(0)));
        StaticReport { declarations: decls }
    }
}

#[cfg(test)]
mod tests;
