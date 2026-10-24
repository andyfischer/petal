//! Candidate parameter discovery (docs/dev/direct-manipulation-v2-plan.md
//! §2.2, M2): which numeric literals of a program could move a goal, and
//! which of those a drag may move.
//!
//! A **parameter** is a numeric literal written in the source (a `Constant`
//! term, or `Neg` over one) with a span. The candidates for a set of goals
//! come in stages, cheapest first:
//!
//! 1. [`discover_params`], a static superset: a backward walk from the goals'
//!    anchor terms ([`anchors_for`]) over value edges, made interprocedural
//!    and conservative. It enters a function through its results at a call
//!    and leaves it through its parameters to the arguments of the call it
//!    came from (along the goal's own call chain where it knows it, every
//!    caller otherwise), follows closures into callbacks (`map`, `filter`,
//!    …), uses the baseline's observed call edges for dynamic calls and
//!    method calls, and crosses `var` cells and `state` to their writes. It
//!    records, per literal, *how* it was reached (only through a hint bound,
//!    only through a comparison or an index, only through an RNG or other
//!    no-feedback native, only through a `state` initializer, through a
//!    `var`, …), and how far it is from the goal.
//! 2. The loaded filter: what the baseline never loaded cannot matter
//!    ([`SpecRun::loaded`](super::spec::SpecRun::loaded)).
//! 3. [`eligibility`], the hard filters of §2.5.1, applied before anything is
//!    perturbed. Library literals are never editable.
//! 4. [`screen_params`]: adaptive group testing with real runs, to keep only
//!    the literals that move a goal.
//!
//! Parameter identity is the literal's `Constant` [`TermId`], valid for one
//! program. Clients that outlive a reload select by [`ParamSelector`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::constant_table::ConstantValue;
use crate::dm_v2::hints::{Hint, HintKind, Hints};
use crate::dm_v2::observe::Observable;
use crate::dm_v2::spec::{OverrideValue, Overrides, SpecHost, SpecRun, SpecSession};
use crate::env::Env;
use crate::native_fn::{InputClasses, NativeFnTable};
use crate::program::{BlockId, FunctionId, Program, Term, TermId, TermOp, base_fn_name};
use crate::source_map::{ENTRY_FILE, FileId, SourceSpan};

// ── the parameter ───────────────────────────────────────────────────────────

/// Where a literal is written (§2.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeOrigin {
    /// The entry file.
    Entry,
    /// Another file of the entry's project (under the nearest `petal.toml`,
    /// else the entry's directory), outside the package cache and vendored
    /// copies.
    UserFile,
    /// A file on disk outside the project: a local path dependency.
    PathDep,
    /// The `std` prelude, a bundled package, the package cache, a vendored
    /// copy, or an in-memory module: never editable.
    Library,
}

/// A numeric literal that may move a goal (§6.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The literal's `Constant` term: what a run overrides.
    pub term: TermId,
    /// What an edit replaces: the literal, including its `-` when negated.
    pub span: SourceSpan,
    pub file: FileId,
    /// The value as written, sign included (`-5` for `-5`).
    pub value: f64,
    pub is_int: bool,
    /// Written with a unary minus: the `Constant` term holds `-value`.
    pub negated: bool,
    /// The binding the literal defines (`let gap = 10`), if it is one.
    pub binding: Option<String>,
    /// That binding is a `config let`.
    pub config: bool,
    pub hint: Option<Hint>,
    /// The function whose body the literal is written in.
    pub function: Option<FunctionId>,
    /// Some path to a goal crosses a `var` cell.
    pub through_var: bool,
    /// Some path to a goal crosses `state` (feedback from earlier frames).
    pub through_state: bool,
    /// Every path to a goal goes through a `state` initializer.
    pub state_init: bool,
    /// Every path to a goal goes through the arguments of an RNG or other
    /// no-feedback native, or through `rigid(...)`.
    pub through_rng: bool,
    /// Every path to a goal goes through a comparison, an index, a loop
    /// bound, a `match` subject or a loop key: the literal changes which code
    /// runs, not where a shape is.
    pub structural: bool,
    /// Every path to a goal is as a `tunable` bound (`min`/`max`/`step`).
    pub hint_bound: bool,
    /// Written as an argument of the goal's own user-frame call.
    pub in_goal_frame: bool,
    pub origin: CodeOrigin,
    /// Call, definition and cell edges on the shortest path to a goal.
    pub distance: u32,
}

impl Param {
    /// The override that makes this literal read `v` (as written, so a
    /// negated literal's constant is set to `-v`). An int literal takes an
    /// int when `v` is whole.
    pub fn override_for(&self, v: f64) -> OverrideValue {
        let loaded = if self.negated { -v } else { v };
        if self.is_int && loaded.fract() == 0.0 && loaded.abs() < 9.0e15 {
            OverrideValue::Int(loaded as i64)
        } else {
            OverrideValue::Num(loaded)
        }
    }

    /// The override that makes this literal read the int `n` (as written).
    pub fn override_int(&self, n: i64) -> OverrideValue {
        OverrideValue::Int(if self.negated { -n } else { n })
    }
}

/// The anchor of a goal: the term whose value the goal reads, and the call
/// chain it was reached by (the emit's chain, innermost first), which lets
/// discovery leave the anchor's function through the caller that actually
/// called it.
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub term: TermId,
    pub chain: Vec<TermId>,
}

// ── selectors and eligibility (§2.5.1, §2.7) ────────────────────────────────

/// A client's say over which parameters may move (§2.7).
#[derive(Debug, Clone, PartialEq)]
pub enum ParamSelector {
    /// Exactly this literal (this program only).
    Term(TermId),
    /// A literal whose span starts inside this span.
    Span(SourceSpan),
    /// A binding, by name (module-qualified or base name).
    Variable(String),
    /// Any literal on this 1-based line of the entry file.
    Line(u32),
    /// `config let` and `tunable` literals.
    ConfigOnly,
    /// Literals in this function's body.
    Function(String),
    /// Every literal in user code.
    AllUserCode,
    /// Let literals in local path dependencies be edited.
    AllowPathDeps,
    /// Screen library literals for the report; they are still never edited.
    ReportLibrary,
    IncludeStateInit,
    IncludeStructural,
    IncludeThroughRng,
}

impl ParamSelector {
    /// Whether this selector names a set of literals (as opposed to lifting a
    /// default exclusion).
    fn is_set(&self) -> bool {
        matches!(
            self,
            ParamSelector::Term(_)
                | ParamSelector::Span(_)
                | ParamSelector::Variable(_)
                | ParamSelector::Line(_)
                | ParamSelector::ConfigOnly
                | ParamSelector::Function(_)
                | ParamSelector::AllUserCode
        )
    }

    /// Whether this selector names one place explicitly, which overrides the
    /// default exclusions (rigid, structural, …) for what it names.
    fn is_explicit(&self) -> bool {
        matches!(
            self,
            ParamSelector::Term(_)
                | ParamSelector::Span(_)
                | ParamSelector::Variable(_)
                | ParamSelector::Line(_)
                | ParamSelector::Function(_)
        )
    }

    /// Whether `p` is in the set this selector names.
    pub fn matches(&self, program: &Program, p: &Param) -> bool {
        match self {
            ParamSelector::Term(t) => p.term == *t,
            ParamSelector::Span(s) => {
                s.file == p.file
                    && s.start.offset <= p.span.start.offset
                    && p.span.start.offset < s.end.offset.max(s.start.offset + 1)
            }
            ParamSelector::Variable(name) => p.binding.as_deref().is_some_and(|b| {
                b == name || b.rsplit("::").next() == Some(name.as_str())
            }),
            ParamSelector::Line(l) => p.file == ENTRY_FILE && p.span.start.line == *l,
            ParamSelector::ConfigOnly => p.config || matches!(p.hint, Some(Hint::Tunable { .. })),
            ParamSelector::Function(name) => p.function.is_some_and(|f| {
                program
                    .functions
                    .get(f.0 as usize)
                    .and_then(|d| d.name.as_deref())
                    .is_some_and(|n| {
                        let n = base_fn_name(n);
                        n == name || n.rsplit("::").next() == Some(name.as_str())
                    })
            }),
            ParamSelector::AllUserCode => p.origin != CodeOrigin::Library,
            _ => false,
        }
    }
}

/// Which parameters may move: `allow: None` is the defaults of §2.5.1;
/// `allow: Some(..)` with any set selector is exact (only what it names,
/// minus what is never editable).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamFilter {
    pub allow: Option<Vec<ParamSelector>>,
    pub deny: Vec<ParamSelector>,
}

/// What the session knows about the checkpoint, for the state rule of
/// §2.1.2: a `state` initializer's literal moves the running program only
/// when the checkpoint does not already hold that state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EligibilityContext {
    /// The checkpoint holds `state` (a live frame). On a fresh stack (the
    /// CLI's) it holds none, and initializer literals are ordinary.
    pub checkpoint_has_state: bool,
}

/// Whether a parameter is measured and whether it may be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eligibility {
    /// Perturb it (screen and measure its column).
    pub screen: bool,
    /// Offer it as an edit; `Err` says why not.
    pub edit: Result<(), &'static str>,
}

/// The hard filters of §2.5.1 for `p` under `filter`.
pub fn eligibility(
    program: &Program,
    p: &Param,
    filter: &ParamFilter,
    ctx: EligibilityContext,
) -> Eligibility {
    let allow = filter.allow.as_deref().unwrap_or(&[]);
    let has = |s: &ParamSelector| allow.contains(s);
    let report_library = has(&ParamSelector::ReportLibrary);
    let no = |why: &'static str, screen: bool| Eligibility { screen, edit: Err(why) };

    // Never editable, whatever the filter says.
    if p.origin == CodeOrigin::Library {
        return no("library code (the prelude or a package) is never edited", report_library);
    }
    if p.hint_bound {
        return no("a `tunable` bound is never edited", false);
    }
    if filter.deny.iter().any(|s| s.matches(program, p)) {
        return no("denied by the request", false);
    }
    let sets: Vec<&ParamSelector> = allow.iter().filter(|s| s.is_set()).collect();
    let explicit = if sets.is_empty() {
        false
    } else {
        if !sets.iter().any(|s| s.matches(program, p)) {
            return no("not among the allowed parameters", false);
        }
        sets.iter().any(|s| s.is_explicit() && s.matches(program, p))
    };
    if explicit {
        return Eligibility { screen: true, edit: Ok(()) };
    }
    // Excluded by default.
    if p.origin == CodeOrigin::PathDep && !has(&ParamSelector::AllowPathDeps) {
        return no("in a path dependency (allow with AllowPathDeps)", false);
    }
    if p.hint == Some(Hint::Rigid) {
        return no("declared `rigid`", false);
    }
    if p.state_init && ctx.checkpoint_has_state && !has(&ParamSelector::IncludeStateInit) {
        return no("a `state` initializer, which a running program has already used", false);
    }
    if p.through_rng && !has(&ParamSelector::IncludeThroughRng) {
        return no("reaches the goal only through randomness, noise or `rigid`", false);
    }
    if p.structural && !has(&ParamSelector::IncludeStructural) {
        return no("structural: a comparison, index, loop bound or key", false);
    }
    Eligibility { screen: true, edit: Ok(()) }
}

// ── anchors ─────────────────────────────────────────────────────────────────

/// The anchor terms of `goals` in `run` (normally the baseline): for an
/// emit argument, the argument term of the native call that made the emit;
/// for an emitted value, every argument of that call; for a watched term, the
/// term. A goal whose emit the run does not have gives no anchor.
pub fn anchors_for(program: &Program, run: &SpecRun, goals: &[Observable]) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::new();
    let mut push = |a: Anchor| {
        if !out.contains(&a) {
            out.push(a);
        }
    };
    for goal in goals {
        match goal {
            Observable::EmitArg { key, arg, .. } => {
                let Some(emit) = run.emit(key) else { continue };
                let chain: Vec<TermId> = emit.chain.chain.iter().copied().collect();
                let Some(&call) = chain.first() else { continue };
                let Some(t) = program.terms.get(call.0 as usize) else { continue };
                let offset = t.op.arg_offset().unwrap_or(0);
                if let Some(&term) = t.inputs.get(offset + arg) {
                    push(Anchor { term, chain });
                }
            }
            Observable::EmitValue { key, .. } => {
                let Some(emit) = run.emit(key) else { continue };
                let chain: Vec<TermId> = emit.chain.chain.iter().copied().collect();
                let Some(&call) = chain.first() else { continue };
                let Some(t) = program.terms.get(call.0 as usize) else { continue };
                let offset = t.op.arg_offset().unwrap_or(0);
                for &term in t.inputs.iter().skip(offset) {
                    push(Anchor { term, chain: chain.clone() });
                }
            }
            Observable::Watch { term, .. } => push(Anchor { term: *term, chain: Vec::new() }),
        }
    }
    out
}

// ── the static walk ─────────────────────────────────────────────────────────

/// How the walk reached a term, as bits: the "every path" flags of [`Param`]
/// are the bits every visit had, the "some path" ones the bits any had.
mod via {
    pub const VAR: u8 = 1;
    pub const STATE: u8 = 2;
    pub const STATE_INIT: u8 = 4;
    pub const BARRIER: u8 = 8;
    pub const STRUCT: u8 = 16;
    pub const HINT_BOUND: u8 = 32;
}

/// Where in the call structure the walk is, for leaving a function by its
/// parameters (§2.2): along the goal's own call chain, back through the call
/// it entered by, or through every caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Ctx {
    /// In the frame of `chain[level]`'s function.
    Chain(u16),
    /// Entered the current function through this call.
    Entered(TermId),
    /// Anywhere.
    Any,
}

/// A binding phantom (a term the compiler binds but never executes): a
/// parameter, a capture, a self-reference, a loop or match variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phantom {
    Param(FunctionId, usize),
    Capture(FunctionId, usize),
    SelfRef(FunctionId),
    Bound(TermId),
}

/// The walk stops after this many visits; the answer is then marked
/// truncated (still a usable, if partial, candidate list).
const MAX_VISITS: usize = 2_000_000;

/// Everything the walk asks of a program, computed once.
struct Index<'p> {
    program: &'p Program,
    natives: &'p NativeFnTable,
    hints: &'p Hints,
    block_fn: HashMap<BlockId, Option<FunctionId>>,
    block_last: HashMap<BlockId, TermId>,
    phantoms: HashMap<TermId, Phantom>,
    fn_results: HashMap<FunctionId, Vec<TermId>>,
    closures_of: HashMap<FunctionId, Vec<TermId>>,
    /// The functions each `Call` reaches statically.
    static_callees: HashMap<TermId, Vec<FunctionId>>,
    /// The functions each call term reached in the baseline (R13).
    observed: HashMap<TermId, Vec<FunctionId>>,
    /// Every call term that may call each function (static, observed, and
    /// the calls a closure of it is handed to).
    callers: HashMap<FunctionId, Vec<TermId>>,
    /// The `CellWrite`s of each cell declaration.
    cell_writes: HashMap<TermId, Vec<TermId>>,
    state_terms: HashMap<u64, Vec<TermId>>,
    phi_srcs: HashMap<TermId, Vec<TermId>>,
    neg_of: HashMap<TermId, TermId>,
}

impl<'p> Index<'p> {
    fn new(
        program: &'p Program,
        natives: &'p NativeFnTable,
        hints: &'p Hints,
        call_edges: &[(TermId, FunctionId)],
    ) -> Index<'p> {
        let mut ix = Index {
            program,
            natives,
            hints,
            block_fn: HashMap::new(),
            block_last: HashMap::new(),
            phantoms: HashMap::new(),
            fn_results: HashMap::new(),
            closures_of: HashMap::new(),
            static_callees: HashMap::new(),
            observed: HashMap::new(),
            callers: HashMap::new(),
            cell_writes: HashMap::new(),
            state_terms: HashMap::new(),
            phi_srcs: HashMap::new(),
            neg_of: HashMap::new(),
        };
        let body_of: HashMap<BlockId, FunctionId> =
            program.functions.iter().map(|f| (f.body_block, f.id)).collect();
        let by_id: HashMap<BlockId, &crate::program::Block> =
            program.blocks.iter().map(|b| (b.id, b)).collect();
        for block in &program.blocks {
            // The innermost function a block is in.
            let mut cur = Some(block.id);
            let mut f = None;
            for _ in 0..256 {
                let Some(b) = cur else { break };
                if let Some(&fid) = body_of.get(&b) {
                    f = Some(fid);
                    break;
                }
                cur = by_id
                    .get(&b)
                    .and_then(|bb| bb.parent_term_id)
                    .and_then(|t| program.terms.get(t.0 as usize))
                    .map(|t| t.block_id);
            }
            ix.block_fn.insert(block.id, f);
            if let Some(&last) = block.terms.last() {
                ix.block_last.insert(block.id, last);
            }
            for po in &block.phi_outs {
                ix.phi_srcs.entry(po.dest_term).or_default().push(po.src_term);
            }
        }
        // Binding phantoms: the terms of a block that its `terms` list does
        // not execute.
        let executed: HashSet<TermId> =
            program.blocks.iter().flat_map(|b| b.terms.iter().copied()).collect();
        for t in &program.terms {
            if !matches!(t.op, TermOp::Copy) || !t.inputs.is_empty() || executed.contains(&t.id) {
                continue;
            }
            let kind = if let Some(&fid) = body_of.get(&t.block_id) {
                let def = &program.functions[fid.0 as usize];
                let name = t.name.as_deref();
                if let Some(k) = name.and_then(|n| def.params.iter().position(|p| p == n)) {
                    Some(Phantom::Param(fid, k))
                } else if let Some(s) = def
                    .capture_registers
                    .iter()
                    .position(|r| *r == t.register)
                    .or_else(|| name.and_then(|n| def.capture_names.iter().position(|c| c == n)))
                {
                    Some(Phantom::Capture(fid, s))
                } else if def.self_ref_register == Some(t.register) {
                    Some(Phantom::SelfRef(fid))
                } else {
                    None
                }
            } else {
                by_id
                    .get(&t.block_id)
                    .and_then(|b| b.parent_term_id)
                    .filter(|p| {
                        program.terms.get(p.0 as usize).is_some_and(|pt| {
                            matches!(
                                pt.op,
                                TermOp::ForLoop | TermOp::NumericForLoop | TermOp::Match
                            )
                        })
                    })
                    .map(Phantom::Bound)
            };
            if let Some(k) = kind {
                ix.phantoms.insert(t.id, k);
            }
        }
        for t in &program.terms {
            match t.op {
                TermOp::MakeClosure(fid) => ix.closures_of.entry(fid).or_default().push(t.id),
                TermOp::Return => {
                    if let (Some(Some(f)), Some(&v)) = (ix.block_fn.get(&t.block_id), t.inputs.first()) {
                        ix.fn_results.entry(*f).or_default().push(v);
                    }
                }
                TermOp::Neg => {
                    if let Some(&inner) = t.inputs.first()
                        && matches!(program.terms.get(inner.0 as usize).map(|x| &x.op), Some(TermOp::Constant(_)))
                    {
                        ix.neg_of.insert(inner, t.id);
                    }
                }
                TermOp::StateInit | TermOp::StateWrite | TermOp::StateRead => {
                    if let Some(k) = t.state_key {
                        ix.state_terms.entry(k.0).or_default().push(t.id);
                    }
                }
                _ => {}
            }
        }
        for f in &program.functions {
            if let Some(&last) = ix.block_last.get(&f.body_block) {
                ix.fn_results.entry(f.id).or_default().push(last);
            }
        }
        for &(site, f) in call_edges {
            let e = ix.observed.entry(site).or_default();
            if !e.contains(&f) {
                e.push(f);
            }
        }
        // Static callees, callers, and the calls closures are handed to.
        for t in &program.terms {
            let is_call = matches!(t.op, TermOp::Call | TermOp::MethodCall { .. } | TermOp::BuiltinCall(_));
            if !is_call {
                continue;
            }
            let add_caller = |ix: &mut Index, f: FunctionId| {
                let e = ix.callers.entry(f).or_default();
                if !e.contains(&t.id) {
                    e.push(t.id);
                }
            };
            if matches!(t.op, TermOp::Call)
                && let Some(&callee) = t.inputs.first()
            {
                let fs = ix.closure_fns(callee);
                for &f in &fs {
                    add_caller(&mut ix, f);
                }
                if !fs.is_empty() {
                    ix.static_callees.insert(t.id, fs);
                }
            }
            let skip = usize::from(matches!(t.op, TermOp::Call));
            for &input in t.inputs.iter().skip(skip) {
                for f in ix.closure_fns(input) {
                    add_caller(&mut ix, f);
                }
            }
            if let Some(fs) = ix.observed.get(&t.id).cloned() {
                for f in fs {
                    add_caller(&mut ix, f);
                }
            }
        }
        // Cell writes, by the declaration they write.
        for t in &program.terms {
            if matches!(t.op, TermOp::CellWrite)
                && let Some(&cell) = t.inputs.first()
            {
                for decl in ix.cell_decls(cell, 0) {
                    ix.cell_writes.entry(decl).or_default().push(t.id);
                }
            }
        }
        ix
    }

    fn term(&self, id: TermId) -> Option<&'p Term> {
        self.program.terms.get(id.0 as usize)
    }

    /// The functions `id` is a closure (or overload set) of, through
    /// identity copies.
    fn closure_fns(&self, id: TermId) -> Vec<FunctionId> {
        let mut cur = id;
        for _ in 0..16 {
            let Some(t) = self.term(cur) else { break };
            match t.op {
                TermOp::MakeClosure(f) => return vec![f],
                TermOp::MakeOverloadSet => {
                    return t.inputs.iter().flat_map(|&i| self.closure_fns(i)).collect();
                }
                TermOp::Copy if !t.inputs.is_empty() => cur = t.inputs[0],
                TermOp::Copy => {
                    if let Some(Phantom::SelfRef(f)) = self.phantoms.get(&cur) {
                        return vec![*f];
                    }
                    break;
                }
                _ => break,
            }
        }
        Vec::new()
    }

    /// The cell declarations (`CellNew`, or a `state var`'s `StateInit`) the
    /// cell operand `id` may be, through copies and captures.
    fn cell_decls(&self, id: TermId, depth: u32) -> Vec<TermId> {
        if depth > 16 {
            return Vec::new();
        }
        let Some(t) = self.term(id) else { return Vec::new() };
        match t.op {
            TermOp::CellNew | TermOp::StateInit => vec![id],
            TermOp::Copy if !t.inputs.is_empty() => self.cell_decls(t.inputs[0], depth + 1),
            TermOp::Copy => match self.phantoms.get(&id) {
                Some(Phantom::Capture(f, s)) => self
                    .closures_of
                    .get(f)
                    .into_iter()
                    .flatten()
                    .filter_map(|&mc| self.term(mc).and_then(|m| m.inputs.get(*s)).copied())
                    .flat_map(|outer| self.cell_decls(outer, depth + 1))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn fn_of(&self, term: TermId) -> Option<FunctionId> {
        self.term(term).and_then(|t| self.block_fn.get(&t.block_id).copied().flatten())
    }

    /// The arguments of call `call` that feed parameter `k` of `f`.
    fn args_for(&self, call: TermId, f: FunctionId, k: usize) -> Vec<TermId> {
        let Some(t) = self.term(call) else { return Vec::new() };
        let direct = matches!(t.op, TermOp::Call)
            && (self.static_callees.get(&call).is_some_and(|fs| fs.contains(&f))
                || self.observed.get(&call).is_some_and(|fs| fs.contains(&f)));
        if direct && t.arg_names.is_empty() {
            return t.inputs.get(1 + k).copied().into_iter().collect();
        }
        // Named arguments, a method call, a callback handed to a builtin:
        // every argument that is not itself a function.
        let skip = usize::from(matches!(t.op, TermOp::Call));
        t.inputs
            .iter()
            .skip(skip)
            .copied()
            .filter(|&i| self.closure_fns(i).is_empty())
            .collect()
    }

    /// What a builtin call named by constant `name` is.
    fn builtin(&self, call: TermId, name: crate::constant_table::ConstantId) -> BuiltinKind {
        if let Some(k) = self.hints.call_kind(call) {
            return match k {
                HintKind::Tunable => BuiltinKind::Tunable,
                HintKind::Rigid => BuiltinKind::Rigid,
            };
        }
        let ConstantValue::String(name) = self.program.constants.get(name) else {
            return BuiltinKind::Plain;
        };
        self.builtin_by_name(name)
    }

    fn builtin_by_name(&self, name: &str) -> BuiltinKind {
        if name == "range" {
            return BuiltinKind::Range;
        }
        match self.natives.lookup_name(name) {
            Some(id) => {
                let fx = self.natives.effects(id);
                if fx.no_feedback || fx.reads.contains(InputClasses::RNG) {
                    BuiltinKind::Barrier
                } else {
                    BuiltinKind::Plain
                }
            }
            None => BuiltinKind::Plain,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltinKind {
    Plain,
    Tunable,
    Rigid,
    Barrier,
    Range,
}

/// What the walk learned about one literal.
#[derive(Debug, Clone, Copy)]
struct Reach {
    all: u8,
    any: u8,
    distance: u32,
}

/// The result of [`discover_params`].
#[derive(Debug, Clone, Default)]
pub struct Discovered {
    /// Every numeric literal the goals statically depend on, in proximity
    /// order (nearest first), eligible or not.
    pub params: Vec<Param>,
    /// The walk hit its visit bound; the list may be missing literals.
    pub truncated: bool,
}

/// The static superset of the literals that may move the anchors (§2.2,
/// stage 1), in proximity order. `call_edges` are the baseline's observed
/// call edges (R13, [`SpecRun::call_edges`]).
pub fn discover_params(
    program: &Program,
    natives: &NativeFnTable,
    hints: &Hints,
    anchors: &[Anchor],
    call_edges: &[(TermId, FunctionId)],
) -> Discovered {
    let ix = Index::new(program, natives, hints, call_edges);
    let mut reach: HashMap<TermId, Reach> = HashMap::new();
    let mut seen: HashSet<(TermId, Ctx, u8, u16)> = HashSet::new();
    let mut queue: VecDeque<(TermId, Ctx, u8, u32, u16)> = VecDeque::new();
    let mut visits = 0usize;
    let mut truncated = false;
    // Each anchor walks with its own chain.
    let chains: Vec<&[TermId]> = anchors.iter().map(|a| a.chain.as_slice()).collect();
    for (ai, a) in anchors.iter().enumerate() {
        let ctx = if a.chain.is_empty() { Ctx::Any } else { Ctx::Chain(0) };
        queue.push_back((a.term, ctx, 0, 0, ai as u16));
    }
    while let Some((id, ctx, mask, dist, ai)) = queue.pop_front() {
        if !seen.insert((id, ctx, mask, if matches!(ctx, Ctx::Chain(_)) { ai } else { 0 })) {
            continue;
        }
        visits += 1;
        if visits > MAX_VISITS {
            truncated = true;
            break;
        }
        let Some(t) = ix.term(id) else { continue };
        let chain = chains[ai as usize];
        let mut next: Vec<(TermId, Ctx, u8, i32)> = Vec::new();
        let mut go = |to: TermId, c: Ctx, bits: u8, cost: i32| next.push((to, c, mask | bits, cost));
        use via::*;
        match &t.op {
            TermOp::Constant(_) => {
                let r = reach.entry(id).or_insert(Reach { all: mask, any: mask, distance: dist });
                r.all &= mask;
                r.any |= mask;
                r.distance = r.distance.min(dist);
            }
            TermOp::Copy if t.inputs.is_empty() => match ix.phantoms.get(&id) {
                Some(Phantom::Param(f, k)) => {
                    let (f, k) = (*f, *k);
                    let along_chain = match ctx {
                        Ctx::Chain(l) => {
                            let l = l as usize;
                            (chain.get(l).and_then(|&c| ix.fn_of(c)) == Some(f))
                                .then(|| chain.get(l + 1).copied())
                                .flatten()
                                .map(|c| (c, Ctx::Chain((l + 1) as u16)))
                        }
                        Ctx::Entered(c) => Some((c, Ctx::Any)),
                        Ctx::Any => None,
                    };
                    // Back out through the call the walk came in by: the
                    // argument is as near as the call itself was, so the
                    // edge in cancels this one (`2` in `draw(place(2))` is
                    // in the goal's frame, `place`'s body one call away).
                    let back = if matches!(ctx, Ctx::Entered(_)) { -1 } else { 1 };
                    match along_chain {
                        Some((call, c)) => {
                            for a in ix.args_for(call, f, k) {
                                go(a, c, 0, back);
                            }
                        }
                        None => {
                            for &call in ix.callers.get(&f).into_iter().flatten() {
                                for a in ix.args_for(call, f, k) {
                                    go(a, Ctx::Any, 0, 1);
                                }
                            }
                        }
                    }
                }
                Some(Phantom::Capture(f, s)) => {
                    for &mc in ix.closures_of.get(f).into_iter().flatten() {
                        if let Some(&outer) = ix.term(mc).and_then(|m| m.inputs.get(*s)) {
                            go(outer, Ctx::Any, 0, 1);
                        }
                    }
                }
                Some(Phantom::SelfRef(_)) | None => {}
                Some(Phantom::Bound(p)) => {
                    if let Some(pt) = ix.term(*p) {
                        match pt.op {
                            TermOp::NumericForLoop => {
                                if let Some(&start) = pt.inputs.first() {
                                    go(start, ctx, 0, 0);
                                }
                                for &end in pt.inputs.iter().skip(1) {
                                    go(end, ctx, STRUCT, 0);
                                }
                            }
                            _ => {
                                if let Some(&src) = pt.inputs.first() {
                                    go(src, ctx, 0, 0);
                                }
                            }
                        }
                    }
                }
            },
            TermOp::MakeClosure(f) => {
                // A function value reached as data: whatever it returns.
                for &r in ix.fn_results.get(f).into_iter().flatten() {
                    go(r, Ctx::Any, 0, 1);
                }
            }
            TermOp::Call => {
                let statics = ix.static_callees.get(&id);
                let observed = ix.observed.get(&id);
                let mut known = false;
                for f in statics.into_iter().flatten().chain(observed.into_iter().flatten()) {
                    known = true;
                    for &r in ix.fn_results.get(f).into_iter().flatten() {
                        go(r, Ctx::Entered(id), 0, 1);
                    }
                }
                if !known {
                    // Nothing known about the callee: everything it was
                    // handed may matter, the callee expression included.
                    for &i in &t.inputs {
                        go(i, ctx, 0, 0);
                    }
                }
            }
            TermOp::MethodCall { name, .. } => {
                let mut user = false;
                for f in ix.observed.get(&id).into_iter().flatten() {
                    user = true;
                    for &r in ix.fn_results.get(f).into_iter().flatten() {
                        go(r, Ctx::Entered(id), 0, 1);
                    }
                }
                let kind = match program.constants.get(*name) {
                    ConstantValue::String(n) if !user => ix.builtin_by_name(n),
                    _ => BuiltinKind::Plain,
                };
                let bits = match kind {
                    BuiltinKind::Barrier => BARRIER,
                    BuiltinKind::Range => STRUCT,
                    _ => 0,
                };
                // A user method's receiver and arguments are reached through
                // its parameters; a builtin method's are its inputs.
                for &i in t.inputs.iter().filter(|_| !user) {
                    let fs = ix.closure_fns(i);
                    if fs.is_empty() {
                        go(i, ctx, bits, 0);
                    } else {
                        for f in fs {
                            for &r in ix.fn_results.get(&f).into_iter().flatten() {
                                go(r, Ctx::Entered(id), bits, 1);
                            }
                        }
                    }
                }
            }
            TermOp::BuiltinCall(name) => {
                let kind = ix.builtin(id, *name);
                for (j, &i) in t.inputs.iter().enumerate() {
                    let bits = match kind {
                        BuiltinKind::Tunable if j > 0 => HINT_BOUND,
                        BuiltinKind::Rigid | BuiltinKind::Barrier => BARRIER,
                        BuiltinKind::Range => STRUCT,
                        _ => 0,
                    };
                    let fs = ix.closure_fns(i);
                    if fs.is_empty() {
                        go(i, ctx, bits, 0);
                    } else {
                        // A callback (`map`, `filter`, …): its results, with
                        // its parameters fed by this call's other arguments.
                        for f in fs {
                            for &r in ix.fn_results.get(&f).into_iter().flatten() {
                                go(r, Ctx::Entered(id), bits, 1);
                            }
                        }
                    }
                }
            }
            TermOp::Eq | TermOp::Ne | TermOp::Lt | TermOp::Le | TermOp::Gt | TermOp::Ge | TermOp::LoopKey => {
                for &i in &t.inputs {
                    go(i, ctx, STRUCT, 0);
                }
            }
            TermOp::GetIndex | TermOp::GetIndexOpt | TermOp::SetIndex => {
                for (j, &i) in t.inputs.iter().enumerate() {
                    go(i, ctx, if j == 1 { STRUCT } else { 0 }, 0);
                }
            }
            TermOp::Branch | TermOp::Match => {
                for &i in &t.inputs {
                    go(i, ctx, STRUCT, 0);
                }
                for b in &t.child_blocks {
                    if let Some(&last) = ix.block_last.get(b) {
                        go(last, ctx, 0, 0);
                    }
                }
                if let Some(arms) = program.match_arms.get(&id) {
                    for arm in arms {
                        if let Some(&last) = ix.block_last.get(&arm.body_block) {
                            go(last, ctx, 0, 0);
                        }
                    }
                }
            }
            TermOp::And | TermOp::Or | TermOp::Coalesce => {
                for &i in &t.inputs {
                    go(i, ctx, 0, 0);
                }
                for b in &t.child_blocks {
                    if let Some(&last) = ix.block_last.get(b) {
                        go(last, ctx, 0, 0);
                    }
                }
            }
            TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop => {
                if t.collect {
                    if let Some(&last) = t.child_blocks.last().and_then(|b| ix.block_last.get(b)) {
                        go(last, ctx, 0, 0);
                    }
                    // The trip count decides how many items there are.
                    for &i in &t.inputs {
                        go(i, ctx, STRUCT, 0);
                    }
                    if matches!(t.op, TermOp::WhileLoop)
                        && let Some(&cond) = t.child_blocks.first().and_then(|b| ix.block_last.get(b))
                    {
                        go(cond, ctx, STRUCT, 0);
                    }
                }
            }
            TermOp::Phi => {
                for &i in &t.inputs {
                    go(i, ctx, 0, 0);
                }
                for &s in ix.phi_srcs.get(&id).into_iter().flatten() {
                    go(s, ctx, 0, 0);
                }
            }
            TermOp::CellRead => {
                for &i in &t.inputs {
                    go(i, ctx, VAR, 0);
                }
            }
            TermOp::CellNew => {
                for &i in &t.inputs {
                    go(i, ctx, 0, 0);
                }
                for &w in ix.cell_writes.get(&id).into_iter().flatten() {
                    if let Some(&v) = ix.term(w).and_then(|wt| wt.inputs.get(1)) {
                        go(v, ctx, VAR, 1);
                    }
                }
            }
            TermOp::CellWrite => {
                if let Some(&v) = t.inputs.get(1) {
                    go(v, ctx, 0, 0);
                }
            }
            TermOp::StateInit => {
                if let Some(&init) = t.child_blocks.first().and_then(|b| ix.block_last.get(b)) {
                    // A `state` read is the value earlier frames left, or
                    // (on a slot's first frame) the initializer.
                    go(init, ctx, STATE_INIT | STATE, 0);
                }
                for &w in ix.cell_writes.get(&id).into_iter().flatten() {
                    if let Some(&v) = ix.term(w).and_then(|wt| wt.inputs.get(1)) {
                        go(v, ctx, VAR | STATE, 1);
                    }
                }
                for &s in t.state_key.and_then(|k| ix.state_terms.get(&k.0)).into_iter().flatten() {
                    if s != id
                        && let Some(st) = ix.term(s)
                        && matches!(st.op, TermOp::StateWrite)
                        && let Some(&v) = st.inputs.first()
                    {
                        go(v, ctx, STATE, 1);
                    }
                }
            }
            TermOp::StateRead => {
                for &s in t.state_key.and_then(|k| ix.state_terms.get(&k.0)).into_iter().flatten() {
                    if let Some(st) = ix.term(s) {
                        match st.op {
                            TermOp::StateInit => go(s, ctx, STATE, 1),
                            TermOp::StateWrite => {
                                if let Some(&v) = st.inputs.first() {
                                    go(v, ctx, STATE, 1);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            TermOp::Break | TermOp::Continue | TermOp::Error(_) => {}
            _ => {
                // Arithmetic, records, lists, fields, `Return`, `Copy`, …:
                // every input is a value edge.
                for &i in &t.inputs {
                    go(i, ctx, 0, 0);
                }
            }
        }
        for (to, c, m, cost) in next {
            let item = (to, c, m, (dist as i64 + cost as i64).max(0) as u32, ai);
            if cost <= 0 {
                queue.push_front(item);
            } else {
                queue.push_back(item);
            }
        }
    }

    let origins = file_origins(program);
    let frames = goal_frame_spans(program, anchors);
    // Char-to-byte tables of the files that are not ASCII.
    let tables: Vec<Option<Vec<usize>>> = program
        .source_map
        .files
        .iter()
        .map(|f| {
            (!f.source.is_ascii()).then(|| {
                f.source.char_indices().map(|(b, _)| b).chain(std::iter::once(f.source.len())).collect()
            })
        })
        .collect();
    let mut params: Vec<Param> = reach
        .into_iter()
        .filter_map(|(term, r)| make_param(program, &ix, &origins, &tables, term, r, &frames))
        .collect();
    params.sort_by_key(|p| (p.distance, p.file.0, p.span.start.offset, p.term.0));
    Discovered { params, truncated }
}

/// The spans of the goals' own user-frame calls: the innermost frame of each
/// anchor's chain in the entry file. A literal written inside one (`2` in
/// `draw(place(2), …)`) is written in the goal's call.
fn goal_frame_spans(program: &Program, anchors: &[Anchor]) -> Vec<SourceSpan> {
    anchors
        .iter()
        .filter_map(|a| crate::provenance::pick_frame(program, &a.chain, ENTRY_FILE))
        .filter_map(|frame| program.source_map.get(frame).copied())
        .filter(|s| s.file == ENTRY_FILE)
        .collect()
}

/// The text of `[start, end)` of `source`, in chars (a span's offsets count
/// chars, not bytes). `bytes` is `source`'s char-to-byte table when it is
/// not ASCII.
fn span_text<'s>(source: &'s str, bytes: Option<&[usize]>, start: u32, end: u32) -> Option<&'s str> {
    let (start, end) = (start as usize, end as usize);
    match bytes {
        None => source.get(start..end),
        Some(b) => source.get(*b.get(start)?..*b.get(end)?),
    }
}

/// Build the parameter for literal `term`, if it is a numeric literal
/// written in the source as itself.
fn make_param(
    program: &Program,
    ix: &Index,
    origins: &[CodeOrigin],
    tables: &[Option<Vec<usize>>],
    term: TermId,
    r: Reach,
    goal_frames: &[SourceSpan],
) -> Option<Param> {
    let t = ix.term(term)?;
    let TermOp::Constant(k) = t.op else { return None };
    let (raw, is_int) = match program.constants.get(k) {
        ConstantValue::Int(n) => (*n as f64, true),
        ConstantValue::Float(bits) => (f64::from_bits(*bits), false),
        _ => return None,
    };
    let own = *program.source_map.get(term)?;
    // Written as itself: a constant the compiler made up (a loop's start, a
    // default) has no span, or a span that is not this number's text.
    let file = program.source_map.files.get(own.file.0 as usize);
    let source = file.map(|f| f.source.as_str()).unwrap_or(program.source.as_str());
    let table = tables.get(own.file.0 as usize).and_then(|t| t.as_deref());
    let text = span_text(source, table, own.start.offset, own.end.offset)?;
    let parsed = text.replace('_', "");
    let written = if is_int {
        parsed.parse::<i64>().ok() == Some(raw as i64)
    } else {
        parsed.parse::<f64>().ok() == Some(raw)
    };
    if !written {
        return None;
    }
    let neg = ix.neg_of.get(&term).copied();
    let (span, value) = match neg {
        Some(n) => (program.source_map.get(n).copied().unwrap_or(own), -raw),
        None => (own, raw),
    };
    // The binding the literal defines: its own name, its minus's, or that of
    // a hint call it is the value of (`let r = tunable(40)`).
    let binding_term = neg.unwrap_or(term);
    let mut named = ix.term(binding_term).filter(|bt| bt.name.is_some());
    if named.is_none() {
        named = ix
            .hints
            .calls
            .keys()
            .filter_map(|&call| ix.term(call))
            .find(|ct| ct.inputs.first() == Some(&binding_term) && ct.name.is_some());
    }
    let binding = named.and_then(|bt| bt.name.clone());
    let config = named.is_some_and(|bt| bt.is_config) || t.is_config;
    use via::*;
    Some(Param {
        term,
        span,
        file: own.file,
        value,
        is_int,
        negated: neg.is_some(),
        binding,
        config,
        hint: ix.hints.get(term).copied(),
        function: ix.block_fn.get(&t.block_id).copied().flatten(),
        through_var: r.any & VAR != 0,
        through_state: r.any & STATE != 0,
        state_init: r.all & STATE_INIT != 0,
        through_rng: r.all & BARRIER != 0,
        structural: r.all & STRUCT != 0,
        hint_bound: r.all & HINT_BOUND != 0 || ix.hints.bounds.contains(&term),
        in_goal_frame: goal_frames.iter().any(|f| {
            f.file == own.file && f.start.offset <= own.start.offset && own.end.offset <= f.end.offset
        }),
        origin: origins.get(own.file.0 as usize).copied().unwrap_or(CodeOrigin::Library),
        distance: r.distance,
    })
}

/// The origin of each file of `program` (§2.5.1).
pub fn file_origins(program: &Program) -> Vec<CodeOrigin> {
    let files = &program.source_map.files;
    if files.is_empty() {
        return vec![CodeOrigin::Entry];
    }
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let root: Option<PathBuf> = files[0].origin.as_deref().map(|entry| {
        let entry = abs(entry);
        let dir = entry.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut cur: Option<&Path> = Some(dir.as_path());
        while let Some(d) = cur {
            if d.join("petal.toml").is_file() {
                return d.to_path_buf();
            }
            cur = d.parent();
        }
        dir
    });
    let cache = abs(&crate::pkgman::cache_dir());
    files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            if i == 0 {
                return CodeOrigin::Entry;
            }
            let Some(origin) = f.origin.as_deref() else { return CodeOrigin::Library };
            if origin.to_string_lossy().starts_with('<') {
                return CodeOrigin::Library;
            }
            let path = abs(origin);
            if path.starts_with(&cache)
                || path
                    .components()
                    .any(|c| matches!(c.as_os_str().to_str(), Some("vendor" | "third_party")))
            {
                return CodeOrigin::Library;
            }
            match &root {
                Some(r) if path.starts_with(r) => CodeOrigin::UserFile,
                Some(_) => CodeOrigin::PathDep,
                None => CodeOrigin::UserFile,
            }
        })
        .collect()
}

// ── the run budget ──────────────────────────────────────────────────────────

/// The runs and the time one request may spend (§2.3.5).
#[derive(Debug, Clone)]
pub struct RunBudget {
    pub max_runs: usize,
    pub used: usize,
    /// `None` where there is no wall clock (wasm), or for no time limit.
    pub deadline: Option<Instant>,
}

impl RunBudget {
    /// `max_runs` runs within `wall` from now.
    pub fn new(max_runs: usize, wall: std::time::Duration) -> RunBudget {
        RunBudget {
            max_runs,
            used: 0,
            deadline: crate::env::spec_run::wall_now().map(|now| now + wall),
        }
    }

    /// No limit but the run count.
    pub fn runs(max_runs: usize) -> RunBudget {
        RunBudget { max_runs, used: 0, deadline: None }
    }

    /// Whether another run fits.
    pub fn has_room(&self) -> bool {
        self.used < self.max_runs
            && match (self.deadline, crate::env::spec_run::wall_now()) {
                (Some(d), Some(now)) => now < d,
                _ => true,
            }
    }

    /// Take one run, if it fits.
    pub fn take(&mut self) -> bool {
        if !self.has_room() {
            return false;
        }
        self.used += 1;
        true
    }
}

// ── screening ───────────────────────────────────────────────────────────────

/// The result of [`screen_params`].
#[derive(Debug, Clone, Default)]
pub struct ScreenResult {
    /// The parameters that moved a goal (indices into the param list), in
    /// the order they were screened.
    pub kept: Vec<usize>,
    /// Kept parameters whose own perturbation changed the control flow.
    pub regime: Vec<usize>,
    /// Parameters whose own perturbation made the run fail.
    pub errored: Vec<(usize, String)>,
    /// How many were screened, out of how many eligible (§2.2 stage 4).
    pub screened: usize,
    pub eligible: usize,
    pub runs: usize,
    pub budget_exhausted: bool,
}

/// How one probing run of a screening group came out.
enum Probe {
    Still,
    Moved,
    Regime,
    Failed(String),
}

/// The relative step of screening member `j` (§2.2 stage 5): distinct,
/// incommensurate, alternating in sign, so that the changes of a group do not
/// cancel.
fn screen_step(p: &Param, j: usize, rel: f64) -> f64 {
    let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
    sign * rel * p.value.abs().max(1.0) * (1.0 + 0.137 * j as f64)
}

/// The override of screening member `j` of a group.
fn screen_override(p: &Param, j: usize, rel: f64) -> OverrideValue {
    if p.is_int {
        let n = p.value as i64;
        p.override_int(if j % 2 == 0 { n + 1 } else { n - 1 })
    } else {
        p.override_for(p.value + screen_step(p, j, rel))
    }
}

/// Whether `run` moved any goal away from `base`.
fn probe(base: &SpecRun, run: &SpecRun, goals: &[Observable]) -> Probe {
    if let Some(e) = &run.error {
        return Probe::Failed(e.to_string());
    }
    let mut moved = false;
    for g in goals {
        match (base.read(g), run.read(g)) {
            (Some(a), Some(b)) => {
                if (a - b).abs() > 1e-12 * a.abs().max(1.0) || a.is_nan() != b.is_nan() {
                    moved = true;
                }
            }
            (Some(_), None) | (None, Some(_)) => moved = true,
            (None, None) => {}
        }
    }
    if run.fingerprint != base.fingerprint {
        return Probe::Regime;
    }
    if moved { Probe::Moved } else { Probe::Still }
}

/// One step of the screening search.
enum Screen {
    /// Run this group; when it moves nothing and `sibling` is given, the
    /// sibling (the other half of a group that moved) must be what moved.
    Test(Vec<usize>, Option<Vec<usize>>),
    /// A group known to move a goal without running it.
    Known(Vec<usize>),
}

/// Empirical screening by adaptive group testing (§2.2 stage 5): perturb a
/// group together; a group that moves no goal is dropped whole, one that
/// does is split in halves, and when the first half of a group that moved
/// moves nothing, the second half is known to without a run of its own.
/// Members are perturbed by distinct, incommensurate steps of alternating
/// sign (ints by `±1`, type-preserving), so their effects do not cancel.
/// `eligible` are indices into `params`, which must already be parameters of
/// `session`; at most `max_screen` of them, nearest first, are screened.
#[allow(clippy::too_many_arguments)]
pub fn screen_params(
    session: &mut SpecSession,
    env: &mut Env,
    host: &mut dyn SpecHost,
    params: &[Param],
    eligible: &[usize],
    max_screen: usize,
    goals: &[Observable],
    budget: &mut RunBudget,
) -> ScreenResult {
    let mut order: Vec<usize> = eligible.to_vec();
    order.sort_by_key(|&i| (params[i].distance, params[i].file.0, params[i].span.start.offset));
    let mut out = ScreenResult {
        eligible: order.len(),
        ..Default::default()
    };
    order.truncate(max_screen);
    out.screened = order.len();
    let base = session.baseline().clone();
    let mut work: Vec<Screen> = Vec::new();
    if !order.is_empty() {
        work.push(Screen::Test(order, None));
    }
    let split = |group: Vec<usize>, work: &mut Vec<Screen>| {
        let mid = group.len() / 2;
        work.push(Screen::Test(group[..mid].to_vec(), Some(group[mid..].to_vec())));
    };
    while let Some(step) = work.pop() {
        let (group, sibling) = match step {
            Screen::Known(group) => {
                if group.len() == 1 {
                    out.kept.push(group[0]);
                } else {
                    split(group, &mut work);
                }
                continue;
            }
            Screen::Test(group, sibling) => (group, sibling),
        };
        if !budget.take() {
            out.budget_exhausted = true;
            // What was not screened is kept: it may matter.
            out.kept.extend(group);
            out.kept.extend(sibling.into_iter().flatten());
            for w in work.drain(..) {
                match w {
                    Screen::Known(g) => out.kept.extend(g),
                    Screen::Test(g, s) => {
                        out.kept.extend(g);
                        out.kept.extend(s.into_iter().flatten());
                    }
                }
            }
            break;
        }
        let mut ov = Overrides::none();
        for (j, &i) in group.iter().enumerate() {
            ov = ov.with(params[i].term, screen_override(&params[i], j, 1e-3));
        }
        let run = session.run(env, host, &ov);
        out.runs += 1;
        let mut result = probe(&base, &run, goals);
        // Int members step by exactly ±1, so `a + b` cancels under the
        // alternating signs: a group with two ints that moved nothing is
        // probed once more with every int stepped +1 (§9 M3).
        if matches!(result, Probe::Still) && group.iter().filter(|&&i| params[i].is_int).count() >= 2 {
            if !budget.take() {
                out.budget_exhausted = true;
                out.kept.extend(group);
                out.kept.extend(sibling.into_iter().flatten());
                for w in work.drain(..) {
                    match w {
                        Screen::Known(g) => out.kept.extend(g),
                        Screen::Test(g, s) => {
                            out.kept.extend(g);
                            out.kept.extend(s.into_iter().flatten());
                        }
                    }
                }
                break;
            }
            let mut ov = Overrides::none();
            for (j, &i) in group.iter().enumerate() {
                let p = &params[i];
                let v = if p.is_int { p.override_int(p.value as i64 + 1) } else { screen_override(p, j, 1e-3) };
                ov = ov.with(p.term, v);
            }
            let run = session.run(env, host, &ov);
            out.runs += 1;
            result = probe(&base, &run, goals);
        }
        if matches!(result, Probe::Still) {
            if let Some(s) = sibling {
                work.push(Screen::Known(s));
            }
            continue;
        }
        if let Some(s) = sibling {
            work.push(Screen::Test(s, None));
        }
        if group.len() == 1 {
            match result {
                Probe::Failed(e) => out.errored.push((group[0], e)),
                Probe::Regime => {
                    out.kept.push(group[0]);
                    out.regime.push(group[0]);
                }
                _ => out.kept.push(group[0]),
            }
            continue;
        }
        split(group, &mut work);
    }
    out.kept.sort_by_key(|&i| (params[i].distance, params[i].file.0, params[i].span.start.offset));
    out
}
