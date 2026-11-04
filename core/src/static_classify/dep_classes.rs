//! Dependency classes: what each function of a program can depend on, by
//! input class, and what it does on every call.
//!
//! This is layer L5 of the retained optimization plan (Petal's P2 in
//! `docs/dev/reactive-rendering-plan.md`; the design and what is built are
//! in `docs/dev/retained-scene-output.md`). It runs after the static-value
//! classifier ([`super::Pass::solve`]) and reuses what that pass resolved:
//! which function or native every call can reach, the structure of blocks,
//! captures, phis and cells.
//!
//! # The classes
//!
//! A [`Deps`] is a bit set. The low bits are the native rows' input classes
//! ([`InputClasses`]: `POINTER`, `KEYBOARD`, `CLOCK`, `VIEWPORT`,
//! `HOST_DATA`, `RESOURCES`, `RNG`, `BINDINGS`, `PHYSICS`, `NET`); above them
//! are the program's own sources of change and the kinds of output:
//!
//! | Bit | Set by |
//! |---|---|
//! | `STATE` | a `state` read, initialization or write |
//! | `VAR` | a `var` of an enclosing function read or written |
//! | `MODULE_VAR` | a top-level plain `var` read or written (a new cell every run) |
//! | `ENUM` | a field read that may reach enum constant data that is not static |
//! | `UNKNOWN` | a call the pass cannot resolve (a parameter, a dynamic value, an unregistered native, a method a record field may answer) |
//! | `ARG` | (terms only) the value depends on a parameter of the call |
//! | `EFFECT` | something a replay cannot reproduce: an `effect` native, a draw from the random stream, a read no probe or revision validates |
//! | `COMMAND` | a `command` native (re-issued on replay) |
//! | `EMIT`, `DECLARE` | an `emit` or `declare` native |
//! | `TICKS` | a native whose answer changes every run (`NativeEffects::ticks`) |
//!
//! `CONST` is the empty set: a term or function whose class has no bit but
//! `EMIT` and `DECLARE` depends on the source text alone.
//!
//! # The per-term bit mask
//!
//! The same forward walk `trace_dependents` (`program_analysis.rs`) makes
//! from one term, made from every source at once and carried across calls:
//! a term's mask is its own bits (a native's row, a `state` access, an
//! outside `var`, an unresolved call), the masks of the terms its value is
//! built from (value inputs; for a `var` read, every write of the cell, the
//! `CellMay` edge; for a phi, every rebinding and the control term that
//! chose it; for a loop or pattern variable, what the loop iterates or the
//! match inspects; for a capture, the captured value at every place the
//! closure is made; a parameter is `ARG`), the mask of the control terms
//! the term runs under, and for a call the summary of every function it can
//! reach. A function's summary is the union of its body's masks without
//! `ARG`. The whole program is iterated to a fixed point; the lattice is
//! finite and every step only adds bits, so it terminates.
//!
//! # Every call
//!
//! The second half is a must-analysis. A function's **every-call** set is
//! what its body does on every path that returns normally: the terms of its
//! body block in order, up to the first control term that can return from
//! inside one of its branches (a loop body may not run, a branch may not be
//! taken, so neither counts). An effect is carried over from a function
//! called on every path (an effect makes every enclosing scope effectful, so
//! no cutoff can save the caller); a `TICKS` read and a `MODULE_VAR` access
//! count only directly, because a callee's failing record may still be
//! re-executed to the same output and keep its caller valid.
//!
//! # Uses
//!
//! - [`FnDeps::hint`] decides the compile-time memo decisions the lowering
//!   applies (`backend::bytecode::memo_hints`).
//! - [`DepClasses::static_args`] lists the calls whose arguments are all
//!   static, so a replay check need not compare them.
//! - [`DepClasses::findings`] is the `never-replays` lint: a function whose
//!   own record can never (or, under a revisioned class, rarely) replay, and
//!   why, down to the native.

use std::fmt::Write as _;

use super::{Class, HashSet, Pass, Target};
use crate::native_fn::{InputClasses, NativeConstness, NativeEffects, NativeFnId};
use crate::program::{BlockId, FunctionId, Program, Term, TermId, TermOp, base_fn_name};

/// A set of dependency classes and output kinds (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Deps(pub u32);

impl Deps {
    pub const CONST: Deps = Deps(0);
    pub const STATE: Deps = Deps(1 << 12);
    pub const VAR: Deps = Deps(1 << 13);
    pub const MODULE_VAR: Deps = Deps(1 << 14);
    pub const ENUM: Deps = Deps(1 << 15);
    pub const UNKNOWN: Deps = Deps(1 << 16);
    pub const ARG: Deps = Deps(1 << 17);
    pub const EFFECT: Deps = Deps(1 << 20);
    pub const COMMAND: Deps = Deps(1 << 21);
    pub const EMIT: Deps = Deps(1 << 22);
    pub const DECLARE: Deps = Deps(1 << 23);
    pub const TICKS: Deps = Deps(1 << 24);

    /// What a call or term may *do* rather than depend on.
    pub const OUTPUT: Deps = Deps(Self::EMIT.0 | Self::DECLARE.0);

    /// The input classes of a native row, as dependency bits.
    pub const fn inputs(classes: InputClasses) -> Deps {
        Deps(classes.0 as u32)
    }

    /// The input classes in this set.
    pub const fn input_classes(self) -> InputClasses {
        InputClasses((self.0 & 0xfff) as u16)
    }

    pub const fn union(self, o: Deps) -> Deps {
        Deps(self.0 | o.0)
    }

    pub const fn contains(self, o: Deps) -> bool {
        self.0 & o.0 == o.0
    }

    pub const fn intersects(self, o: Deps) -> bool {
        self.0 & o.0 != 0
    }

    pub const fn without(self, o: Deps) -> Deps {
        Deps(self.0 & !o.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether this is the class of a constant: nothing but output.
    pub const fn is_const(self) -> bool {
        self.without(Self::OUTPUT).is_empty()
    }

    /// The names of the bits, the input classes first; `INPUT` stands for
    /// `POINTER` and `KEYBOARD` together, and `CONST` for the empty set.
    pub fn names(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let ic = self.input_classes();
        if ic.contains(InputClasses::INPUT) {
            out.push("INPUT");
        }
        for (c, n) in InputClasses::NAMED {
            if ic.contains(c) && !(InputClasses::INPUT.contains(c) && ic.contains(InputClasses::INPUT)) {
                out.push(n);
            }
        }
        for (b, n) in [
            (Self::STATE, "STATE"),
            (Self::VAR, "VAR"),
            (Self::MODULE_VAR, "MODULE_VAR"),
            (Self::ENUM, "ENUM"),
            (Self::UNKNOWN, "UNKNOWN"),
            (Self::ARG, "ARG"),
            (Self::EFFECT, "EFFECT"),
            (Self::COMMAND, "COMMAND"),
            (Self::EMIT, "EMIT"),
            (Self::DECLARE, "DECLARE"),
            (Self::TICKS, "TICKS"),
        ] {
            if self.intersects(b) {
                out.push(n);
            }
        }
        if out.is_empty() {
            out.push("CONST");
        }
        out
    }
}

impl std::ops::BitOr for Deps {
    type Output = Deps;
    fn bitor(self, o: Deps) -> Deps {
        self.union(o)
    }
}

impl std::ops::BitOrAssign for Deps {
    fn bitor_assign(&mut self, o: Deps) {
        self.0 |= o.0;
    }
}

/// The most rounds either fixed point is given. Each round of the forward
/// walk carries a function's summary one call level up, so this bounds the
/// depth of call chain the walk follows exactly; past it the walk gives up
/// soundly (see [`Pass::dep_classes`]).
const MAX_ROUNDS: usize = 4096;

/// The dependency bits of a native row: its input classes, and what its
/// call does to a memoized scope.
pub fn native_deps(row: NativeEffects) -> Deps {
    let mut d = Deps::inputs(row.reads);
    if row.effect {
        d |= Deps::EFFECT;
    }
    // A draw advances the random stream, which makes the scope effectful.
    if row.reads.contains(InputClasses::RNG) {
        d |= Deps::EFFECT;
    }
    // A read of a class that is neither a probe nor validated by a flag or a
    // revision cannot be checked at validation: the scope is given up.
    let validated = InputClasses(
        InputClasses::HOST_DATA.0
            | InputClasses::RESOURCES.0
            | InputClasses::RNG.0
            | InputClasses::REVISIONED.0,
    );
    if !row.probe && !InputClasses(row.reads.0 & !validated.0).is_empty() {
        d |= Deps::EFFECT;
    }
    if row.command {
        d |= Deps::COMMAND;
    }
    if row.declares {
        d |= Deps::DECLARE;
    } else if row.emits {
        d |= Deps::EMIT;
    }
    if row.ticks {
        d |= Deps::TICKS;
    }
    d
}

/// Why a function has a bit: the first term found that gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Witness {
    pub bit: Deps,
    /// The term in the function's body: the call, the `state` access, the
    /// `var` read.
    pub term: TermId,
    /// What that term is, in words (`time()`, `state score`, `var log`,
    /// `draw_hud()`).
    pub what: String,
    /// When the bit came through a call of another function, that function.
    pub via: Option<FunctionId>,
    /// For a native: what its row says it does that gives the bit, in words
    /// ("an effect a replay cannot reproduce").
    pub note: Option<&'static str>,
}

/// One function's classes.
#[derive(Debug, Clone, PartialEq)]
pub struct FnDeps {
    pub id: FunctionId,
    /// The function's name as written, `<anonymous>` for a lambda.
    pub name: String,
    /// Declared in the entry file (not an imported module or the prelude).
    pub entry: bool,
    /// The line it is declared on (1-based), when the compiler kept a span.
    pub line: Option<u32>,
    /// Everything a call may depend on or do (the body's terms, the
    /// functions it calls, what it captures), without `ARG`.
    pub may: Deps,
    /// What every call that returns normally does: `EFFECT` (directly or
    /// through a function called on every path), `TICKS` and `MODULE_VAR`
    /// (directly), and the input classes it reads, the commands it issues and
    /// the output it pushes, directly, on every path.
    pub every: Deps,
    /// The function captures nothing, or its one closure is made once per
    /// run (top-level code, outside any loop) and captures static data or
    /// known functions there.
    pub captures_static: bool,
    /// The first witness of each bit of `may` and of `every`.
    pub why_may: Vec<Witness>,
    pub why_every: Vec<Witness>,
}

impl FnDeps {
    /// The compile-time memo decisions for calls of this function.
    pub fn hint(&self) -> crate::backend::bytecode::isa::MemoHint {
        crate::backend::bytecode::isa::MemoHint {
            effect_every_call: self.every.contains(Deps::EFFECT),
            ticks_every_call: self.every.contains(Deps::TICKS),
            constant: self.may.is_const() && self.captures_static,
        }
    }

    fn witness(list: &[Witness], bit: Deps) -> Option<&Witness> {
        list.iter().find(|w| w.bit == bit)
    }
}

/// How sure a `never-replays` finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySeverity {
    /// The function's own record can never validate.
    Never,
    /// Only on runs where the host did not revise the class (a frame
    /// without a physics step); in a game that steps every frame, never.
    Rarely,
}

/// One `never-replays` finding: a function whose calls do not replay, and
/// why.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayFinding {
    pub function: FunctionId,
    pub name: String,
    pub severity: ReplaySeverity,
    /// The term in the function that is the reason (for its position).
    pub term: TermId,
    /// The reason, as a sentence fragment after the function's name.
    pub reason: String,
}

/// The dependency classes of a whole program.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DepClasses {
    /// Per function, in `Program::functions` order.
    pub fns: Vec<FnDeps>,
    /// Per term, by term id.
    terms: Vec<Deps>,
    /// The `Call` terms whose every argument is static.
    static_args: HashSet<TermId>,
}

impl DepClasses {
    /// The class of one term (see the module docs). `CONST` for an id the
    /// program does not have.
    pub fn term(&self, id: TermId) -> Deps {
        self.terms.get(id.0 as usize).copied().unwrap_or(Deps::CONST)
    }

    /// The classes of a function, by id.
    pub fn function(&self, id: FunctionId) -> Option<&FnDeps> {
        self.fns.iter().find(|f| f.id == id)
    }

    /// Whether every argument of the `Call` term `call` is static: the same
    /// value on every run at that call.
    pub fn static_args(&self, call: TermId) -> bool {
        self.static_args.contains(&call)
    }

    /// The `never-replays` findings: functions whose own record can never
    /// (or rarely) replay, with the reason down to the native or the
    /// access, in program order. A function called through a path the pass
    /// cannot resolve is not judged on what it cannot see.
    pub fn findings(&self, program: &Program, all_modules: bool) -> Vec<ReplayFinding> {
        let mut out = Vec::new();
        for f in &self.fns {
            if f.name.starts_with("__") || !(all_modules || f.entry) {
                continue;
            }
            let push = |sev, w: &Witness, reason: String, out: &mut Vec<ReplayFinding>| {
                out.push(ReplayFinding {
                    function: f.id,
                    name: f.name.clone(),
                    severity: sev,
                    term: w.term,
                    reason,
                })
            };
            if let Some(w) = FnDeps::witness(&f.why_every, Deps::EFFECT) {
                let reason = match w.via {
                    Some(g) => format!(
                        "calls {} on every call, which never replays: {}",
                        w.what,
                        self.chain(program, g, Deps::EFFECT, 0)
                    ),
                    None => format!(
                        "calls {} on every call (class EFFECT: {})",
                        w.what,
                        w.note.unwrap_or("an effect a replay cannot reproduce")
                    ),
                };
                push(ReplaySeverity::Never, w, reason, &mut out);
                continue;
            }
            if let Some(w) = FnDeps::witness(&f.why_every, Deps::TICKS) {
                let class = self.term_class_name(w.term);
                push(
                    ReplaySeverity::Never,
                    w,
                    format!(
                        "reads {} on every call (class {class}), whose answer changes every run; \
                         its calls are not recorded unless a caller is",
                        w.what
                    ),
                    &mut out,
                );
                continue;
            }
            if let Some(w) = FnDeps::witness(&f.why_every, Deps::MODULE_VAR) {
                push(
                    ReplaySeverity::Never,
                    w,
                    format!(
                        "uses {} on every call (class MODULE_VAR): a top-level var is a new cell every run, \
                         so the record never validates; a `state var` is one cell for the life of the program",
                        w.what
                    ),
                    &mut out,
                );
                continue;
            }
            for class in [InputClasses::PHYSICS, InputClasses::NET] {
                if let Some(w) = FnDeps::witness(&f.why_every, Deps::inputs(class)) {
                    let name = class.names().first().copied().unwrap_or("?");
                    push(
                        ReplaySeverity::Rarely,
                        w,
                        format!(
                            "reads {} on every call (class {name}): it replays only on runs where the host \
                             did not revise {name}",
                            w.what
                        ),
                        &mut out,
                    );
                    break;
                }
            }
        }
        out
    }

    /// The input-class name a ticking witness reads.
    fn term_class_name(&self, term: TermId) -> String {
        let names = Deps(self.term(term).0 & 0xfff).names();
        names.join(" | ")
    }

    /// "`g` calls `h` on every call, which …" down to the native.
    fn chain(&self, program: &Program, g: FunctionId, bit: Deps, depth: usize) -> String {
        let Some(f) = self.function(g) else {
            return String::new();
        };
        let Some(w) = FnDeps::witness(&f.why_every, bit) else {
            return String::new();
        };
        match w.via {
            Some(h) if depth < 8 => format!(
                "{} calls {} on every call; {}",
                f.name,
                w.what,
                self.chain(program, h, bit, depth + 1)
            ),
            _ => format!(
                "{} calls {} on every call ({})",
                f.name,
                w.what,
                w.note.unwrap_or("an effect")
            ),
        }
    }

    /// A text report: one line per function with its classes, then the
    /// findings. For `petal check --deps` and tests.
    pub fn render(&self, program: &Program, all_modules: bool) -> String {
        let mut s = String::new();
        for f in &self.fns {
            if f.name.starts_with("__") || !(all_modules || f.entry) {
                continue;
            }
            let _ = writeln!(
                s,
                "{}: may {}; every call {}{}",
                f.name,
                f.may.names().join(" | "),
                f.every.names().join(" | "),
                if f.hint().constant { " (constant)" } else { "" }
            );
        }
        for x in self.findings(program, all_modules) {
            let _ = writeln!(s, "{} {}: {}", match x.severity {
                ReplaySeverity::Never => "never replays:",
                ReplaySeverity::Rarely => "rarely replays:",
            }, x.name, x.reason);
        }
        s
    }
}

impl<'a> Pass<'a> {
    /// Compute the dependency classes, after [`solve`](Self::solve).
    pub(super) fn dep_classes(&self) -> DepClasses {
        let p = self.p;
        let nfns = p.functions.len();
        let mut terms = vec![Deps::CONST; p.terms.len()];
        let mut may = vec![Deps::CONST; nfns];

        // The fixed point of the forward walk. Every round only adds bits
        // and a function's summary reaches its callers one round later, so a
        // call chain needs a round per level: run until nothing changes. The
        // bound is only a guard; a walk stopped short of its fixed point
        // would miss bits (a read at the bottom of a deep chain, and the top
        // taken for a constant), so then every term and function is given
        // `UNKNOWN`, which no hint trusts.
        let mut converged = false;
        for _round in 0..MAX_ROUNDS {
            let mut changed = false;
            for term in &p.terms {
                let m = self.term_deps(term, &terms, &may);
                let slot = &mut terms[term.id.0 as usize];
                if *slot != m.union(*slot) {
                    *slot = m.union(*slot);
                    changed = true;
                }
            }
            let mut next = vec![Deps::CONST; nfns];
            for term in &p.terms {
                if let Some(fi) = self.owner.get(term.block_id.0 as usize).copied().flatten() {
                    next[fi] |= terms[term.id.0 as usize].without(Deps::ARG);
                }
            }
            if next != may {
                may = next;
                changed = true;
            }
            if !changed {
                converged = true;
                break;
            }
        }
        if !converged {
            terms.iter_mut().for_each(|d| *d |= Deps::UNKNOWN);
            may.iter_mut().for_each(|d| *d |= Deps::UNKNOWN);
        }

        // Every call: a least fixed point over the call graph. Stopped
        // short it is a subset of the fixed point, which only costs hints.
        let mut every = vec![Deps::CONST; nfns];
        let mut why_every: Vec<Vec<Witness>> = vec![Vec::new(); nfns];
        for _round in 0..MAX_ROUNDS {
            let mut changed = false;
            for fi in 0..nfns {
                let (e, why) = self.every_call(fi, &every);
                if e != every[fi] {
                    every[fi] = e;
                    why_every[fi] = why;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut fns = Vec::with_capacity(nfns);
        for fi in 0..nfns {
            let f = self.func(fi);
            fns.push(FnDeps {
                id: f.id,
                name: base_fn_name(f.name.as_deref().unwrap_or("<anonymous>")).to_string(),
                entry: f.span.is_some_and(|sp| sp.file == crate::source_map::ENTRY_FILE),
                line: f.span.map(|sp| sp.start.line as u32),
                may: may[fi],
                every: every[fi],
                captures_static: self.captures_static(fi),
                why_may: self.why_may(fi, &may),
                why_every: std::mem::take(&mut why_every[fi]),
            });
        }

        let mut static_args = HashSet::default();
        for term in &p.terms {
            if matches!(term.op, TermOp::Call)
                && term.inputs.len() > 1
                && term.arg_names.iter().all(|n| n.is_none())
                && term.inputs[1..].iter().all(|&a| self.static_arg(a, 0))
            {
                static_args.insert(term.id);
            }
        }
        DepClasses {
            fns,
            terms,
            static_args,
        }
    }

    /// One term's mask from the current masks and function summaries.
    fn term_deps(&self, term: &Term, terms: &[Deps], may: &[Deps]) -> Deps {
        let get = |t: TermId| terms.get(t.0 as usize).copied().unwrap_or(Deps::CONST);
        let mut d = self.own_deps(term, may);
        // Value inputs (a `var` read and a capture of a cell are identity
        // edges: what flows is the cell's contents, below).
        for (pos, &i) in term.inputs.iter().enumerate() {
            let identity = match term.op {
                TermOp::CellRead | TermOp::CellWrite => pos == 0,
                TermOp::MakeClosure(_) => self.cells.is_decl(i) || self.cells.decl_of(i).is_some(),
                _ => false,
            };
            if !identity {
                d |= get(i);
            }
        }
        match term.op {
            TermOp::CellRead => {
                if let Some(decl) = term.inputs.first().and_then(|o| self.cells.decl_of(*o)) {
                    // Every write may reach every read (`CellMay`).
                    if let Some(&init) = self.p.get_term(decl).inputs.first() {
                        d |= get(init);
                    }
                    for &w in self.cells.writes_of(decl) {
                        if let Some(&v) = self.p.get_term(w).inputs.get(1) {
                            d |= get(v);
                        }
                    }
                }
            }
            TermOp::Phi => {
                for &(src, control) in self.phi_srcs.get(&term.id).map(|v| &v[..]).unwrap_or(&[]) {
                    d |= get(src);
                    if let Some(c) = control {
                        d |= self.control_deps(c, terms);
                    }
                }
            }
            _ => {}
        }
        if Self::is_phantom(term) {
            d |= self.phantom_deps(term, terms);
        }
        // Control dependence: a term inside a branch or loop is as dynamic
        // as what decides whether it runs.
        if let Some(control) = self.parent.get(term.block_id.0 as usize).copied().flatten() {
            d |= self.control_deps(control, terms);
        }
        d
    }

    /// What decides whether the blocks of `control` run, and how often: its
    /// inputs, and the controls above it.
    fn control_deps(&self, control: TermId, terms: &[Deps]) -> Deps {
        let mut d = Deps::CONST;
        let mut at = Some(control);
        for _ in 0..256 {
            let Some(c) = at else { break };
            let t = self.p.get_term(c);
            for &i in &t.inputs {
                d |= terms.get(i.0 as usize).copied().unwrap_or(Deps::CONST);
            }
            // A while loop's condition is a block of its own.
            if matches!(t.op, TermOp::WhileLoop)
                && let Some(&cond) = t.child_blocks.first()
                && let Some(&last) = self.block(cond).terms.last()
            {
                d |= terms.get(last.0 as usize).copied().unwrap_or(Deps::CONST);
            }
            at = self.parent.get(t.block_id.0 as usize).copied().flatten();
        }
        d.without(Deps::OUTPUT)
    }

    /// A binding phantom: a parameter, a capture, the self-reference, a loop
    /// or pattern variable, a registered native.
    fn phantom_deps(&self, term: &Term, terms: &[Deps]) -> Deps {
        let get = |t: TermId| terms.get(t.0 as usize).copied().unwrap_or(Deps::CONST);
        let b = term.block_id;
        if b == self.p.root_block {
            return Deps::CONST;
        }
        if let Some(&fi) = self.body_fn.get(&b) {
            let f = self.func(fi);
            if f.self_ref_register == Some(term.register) {
                return Deps::CONST;
            }
            if let Some(slot) = f.capture_registers.iter().position(|r| *r == term.register) {
                let mut d = Deps::CONST;
                for &site in &self.sites[fi] {
                    let Some(&outer) = self.p.get_term(site).inputs.get(slot) else {
                        return Deps::UNKNOWN;
                    };
                    d |= self.cell_class(outer).unwrap_or_else(|| get(outer));
                }
                return d;
            }
            if (term.register.0 as usize) < f.bound_arg_count() {
                return Deps::ARG;
            }
            return Deps::UNKNOWN;
        }
        // A loop or pattern variable: what the loop iterates or the match
        // inspects (already in the control dependence), and the per-call
        // part of it.
        Deps::CONST
    }

    /// For a term that is (or names) a `var` cell: the class of using it.
    fn cell_class(&self, t: TermId) -> Option<Deps> {
        let decl = if self.cells.is_decl(t) { Some(t) } else { self.cells.decl_of(t) }?;
        if self.fn_cells.contains_key(&decl) {
            return Some(Deps::CONST);
        }
        Some(self.var_class(decl))
    }

    /// The class of reading or writing the cell declared at `decl` from
    /// outside the function that declared it.
    fn var_class(&self, decl: TermId) -> Deps {
        let d = self.p.get_term(decl);
        let top = self.owner.get(d.block_id.0 as usize).copied().flatten().is_none();
        if self.cells.host_writable(decl) {
            // A `state var`: one cell, its slot read through `state`.
            Deps::STATE
        } else if top {
            Deps::MODULE_VAR
        } else {
            Deps::VAR
        }
    }

    /// The bits a term adds by itself.
    fn own_deps(&self, term: &Term, may: &[Deps]) -> Deps {
        match &term.op {
            TermOp::BuiltinCall(name_c) => {
                let name = self.string(*name_c);
                if name == super::ENUM_DATA
                    || name == super::ENUM_FROM
                    || name == super::ENUM_ALL
                    || name == crate::classes::DECLARE_METHOD_BUILTIN
                {
                    return Deps::CONST;
                }
                let native = match self.native_of.get(name_c) {
                    Some(found) => *found,
                    None => self.natives.lookup_name(name),
                };
                match native {
                    Some(id) => self.native_call_deps(id, &term.inputs, may),
                    None => Deps::UNKNOWN,
                }
            }
            TermOp::Call => {
                let Some(&callee) = term.inputs.first() else {
                    return Deps::UNKNOWN;
                };
                match self.cls(callee) {
                    Class::Function(targets) => {
                        let mut d = Deps::CONST;
                        for t in targets {
                            d |= match *t {
                                Target::User(fi) => may[fi as usize],
                                Target::Native(id) => {
                                    self.native_call_deps(NativeFnId(id), &term.inputs[1..], may)
                                }
                            };
                        }
                        d
                    }
                    _ => Deps::UNKNOWN,
                }
            }
            TermOp::MethodCall { name, .. } => {
                let mut d = Deps::CONST;
                let method = self.string(*name);
                // A receiver that is not static data may be a record whose
                // field of that name holds a closure, which is called.
                if !matches!(term.inputs.first().map(|r| self.cls(*r)), Some(Class::Data)) {
                    d |= Deps::UNKNOWN;
                }
                if self.enum_key(method).is_dynamic() {
                    d |= Deps::ENUM;
                }
                let mut any = false;
                for &func in self.dispatch.get(name).map(|v| &v[..]).unwrap_or(&[]) {
                    any = true;
                    match self.cls(func) {
                        Class::Function(targets) => {
                            for t in targets {
                                if let Target::User(fi) = *t {
                                    d |= may[fi as usize];
                                }
                            }
                        }
                        _ => d |= Deps::UNKNOWN,
                    }
                }
                let class_methods = self.class_natives.get(method).map(|v| &v[..]).unwrap_or(&[]);
                for &id in class_methods.iter().chain(self.natives.lookup_name(method).iter()) {
                    any = true;
                    d |= self.native_call_deps(id, &term.inputs, may);
                }
                if !any {
                    d |= Deps::UNKNOWN;
                }
                d
            }
            TermOp::GetField(c) | TermOp::GetFieldOpt(c) => {
                if self.enum_key(self.string(*c)).is_dynamic() {
                    Deps::ENUM
                } else {
                    Deps::CONST
                }
            }
            TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite => Deps::STATE,
            TermOp::CellRead | TermOp::CellWrite => {
                let Some(decl) = term.inputs.first().and_then(|o| self.cells.decl_of(*o)) else {
                    return Deps::UNKNOWN;
                };
                if self.fn_cells.contains_key(&decl) || self.local_cell(decl, term.block_id) {
                    Deps::CONST
                } else {
                    self.var_class(decl)
                }
            }
            TermOp::Error(_) => Deps::UNKNOWN,
            _ => Deps::CONST,
        }
    }

    /// A call of a registered native: its row, and for a higher-order
    /// builtin the functions it is handed.
    fn native_call_deps(&self, id: NativeFnId, args: &[TermId], may: &[Deps]) -> Deps {
        let mut d = native_deps(self.natives.effects(id));
        let takes_callbacks = matches!(
            self.natives.constness(id),
            NativeConstness::ConstIfCallbacksAre
        ) || Some(id) == self.natives.intrinsic_for_each;
        if takes_callbacks {
            for &a in args {
                match self.cls(a) {
                    Class::Function(targets) => {
                        for t in targets {
                            d |= match *t {
                                Target::User(fi) => may[fi as usize],
                                Target::Native(n) => native_deps(self.natives.effects(NativeFnId(n))),
                            };
                        }
                    }
                    Class::Data => {}
                    _ => d |= Deps::UNKNOWN,
                }
            }
        }
        d
    }

    /// The first witness of each bit of a function's `may` set.
    fn why_may(&self, fi: usize, may: &[Deps]) -> Vec<Witness> {
        let mut out: Vec<Witness> = Vec::new();
        let mut seen = Deps::CONST;
        let mut stack: Vec<BlockId> = vec![self.func(fi).body_block];
        while let Some(b) = stack.pop() {
            for &t in &self.block(b).terms {
                let term = self.p.get_term(t);
                let own = self.own_deps(term, may);
                let fresh = own.without(seen).without(Deps::ARG);
                if !fresh.is_empty() {
                    for bit in bits(fresh) {
                        out.push(Witness {
                            bit,
                            term: t,
                            what: self.describe(term),
                            via: self.single_user_target(term),
                            note: None,
                        });
                    }
                    seen |= fresh;
                }
                stack.extend(self.blocks_of(term));
            }
        }
        out
    }

    /// The every-call set of function `fi`, given the current sets of the
    /// functions it calls, with a witness per bit.
    fn every_call(&self, fi: usize, every: &[Deps]) -> (Deps, Vec<Witness>) {
        let mut d = Deps::CONST;
        let mut why = Vec::new();
        let body = self.func(fi).body_block;
        let note = |bit: Deps,
                    term: &Term,
                    via: Option<FunctionId>,
                    row: Option<NativeEffects>,
                    d: &mut Deps,
                    why: &mut Vec<Witness>| {
            if !d.contains(bit) {
                *d |= bit;
                why.push(Witness {
                    bit,
                    term: term.id,
                    what: self.describe(term),
                    via,
                    note: row.map(|r| effect_note(r, bit)),
                });
            }
        };
        for &t in &self.block(body).terms {
            let term = self.p.get_term(t);
            match &term.op {
                TermOp::BuiltinCall(_) | TermOp::Call | TermOp::MethodCall { .. } => {
                    let targets = self.call_targets(term);
                    if let Some(targets) = targets.filter(|t| !t.is_empty()) {
                        // What every target the call may reach does on every
                        // call: an effect is carried through a user function;
                        // a native's ticks and input classes count directly.
                        let mut all = Deps(u32::MAX);
                        for t in &targets {
                            let one = match *t {
                                Target::User(g) => Deps(every[g as usize].0 & Deps::EFFECT.0),
                                Target::Native(n) => {
                                    let nd = native_deps(self.natives.effects(NativeFnId(n)));
                                    Deps(
                                        nd.0 & (Deps::EFFECT.0
                                            | Deps::TICKS.0
                                            | Deps::COMMAND.0
                                            | Deps::OUTPUT.0
                                            | 0xfff),
                                    )
                                }
                            };
                            all = Deps(all.0 & one.0);
                        }
                        let via = match targets.as_slice() {
                            [Target::User(g)] => Some(self.func(*g as usize).id),
                            _ => None,
                        };
                        let row = match targets.as_slice() {
                            [Target::Native(n)] => Some(self.natives.effects(NativeFnId(*n))),
                            _ => None,
                        };
                        for bit in bits(all) {
                            note(bit, term, via, row, &mut d, &mut why);
                        }
                    }
                }
                TermOp::CellRead | TermOp::CellWrite => {
                    if let Some(decl) = term.inputs.first().and_then(|o| self.cells.decl_of(*o))
                        && !self.fn_cells.contains_key(&decl)
                        && !self.local_cell(decl, term.block_id)
                        && self.var_class(decl) == Deps::MODULE_VAR
                    {
                        note(Deps::MODULE_VAR, term, None, None, &mut d, &mut why);
                    }
                }
                TermOp::Return => break,
                _ => {}
            }
            // Past a control term that can return from inside, the rest of
            // the body is not on every path.
            if self.blocks_of(term).iter().any(|&b| self.block_returns(b)) {
                break;
            }
        }
        (d, why)
    }

    /// What a call term may reach, when every possibility is known.
    fn call_targets(&self, term: &Term) -> Option<Vec<Target>> {
        match &term.op {
            TermOp::BuiltinCall(name_c) => {
                let native = match self.native_of.get(name_c) {
                    Some(found) => *found,
                    None => self.natives.lookup_name(self.string(*name_c)),
                }?;
                Some(vec![Target::Native(native.0)])
            }
            TermOp::Call => match self.cls(*term.inputs.first()?) {
                Class::Function(targets) => Some(targets.to_vec()),
                _ => None,
            },
            // Dispatch is by the receiver's class at run time.
            _ => None,
        }
    }

    /// The one user function a call reaches, if it reaches exactly one.
    fn single_user_target(&self, term: &Term) -> Option<FunctionId> {
        match self.call_targets(term)?.as_slice() {
            [Target::User(g)] => Some(self.func(*g as usize).id),
            _ => None,
        }
    }

    /// Whether block `b` (or one under it) holds a `return`.
    fn block_returns(&self, b: BlockId) -> bool {
        let mut stack = vec![b];
        let mut n = 0;
        while let Some(b) = stack.pop() {
            n += 1;
            if n > 100_000 {
                return true;
            }
            for &t in &self.block(b).terms {
                let term = self.p.get_term(t);
                if matches!(term.op, TermOp::Return) {
                    return true;
                }
                stack.extend(self.blocks_of(term));
            }
        }
        false
    }

    /// Whether `site`, a `MakeClosure`, runs at most once per run: in
    /// top-level code and not inside a loop. A closure made there captures
    /// the same static values on every run; one made in a loop is a
    /// different closure on each iteration, and which of them a later call
    /// reaches may change from run to run.
    fn once_per_run(&self, site: TermId) -> bool {
        let st = self.p.get_term(site);
        if self.owner.get(st.block_id.0 as usize).copied().flatten().is_some() {
            return false;
        }
        let mut at = self.parent.get(st.block_id.0 as usize).copied().flatten();
        for _ in 0..4096 {
            let Some(c) = at else { return true };
            let t = self.p.get_term(c);
            if matches!(t.op, TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop) {
                return false;
            }
            at = self.parent.get(t.block_id.0 as usize).copied().flatten();
        }
        false
    }

    /// Whether the function's one closure is made once per run
    /// ([`once_per_run`](Self::once_per_run)) and what it captures there is
    /// static data or a known function: then every closure of it a run can
    /// call captures the same values as on every other run.
    fn captures_static(&self, fi: usize) -> bool {
        let f = self.func(fi);
        if f.capture_registers.is_empty() {
            return true;
        }
        self.sites[fi].len() == 1
            && self.sites[fi].iter().all(|&site| {
                let st = self.p.get_term(site);
                self.once_per_run(site)
                    && st.inputs.iter().all(|&i| {
                        if let Some(decl) = if self.cells.is_decl(i) { Some(i) } else { self.cells.decl_of(i) } {
                            return self.fn_cells.contains_key(&decl);
                        }
                        matches!(self.cls(i), Class::Data | Class::Function(_))
                    })
            })
    }

    /// Whether `t`, an argument of a call, is the same value on every run at
    /// that call: static data in top-level code, or in a function a literal
    /// built of literals and of static captures.
    fn static_arg(&self, t: TermId, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        let term = self.p.get_term(t);
        let in_fn = self.owner.get(term.block_id.0 as usize).copied().flatten();
        let Some(fi) = in_fn else {
            return matches!(self.cls(t), Class::Data);
        };
        if !matches!(self.cls(t), Class::Data) {
            return false;
        }
        match &term.op {
            TermOp::Constant(_) => true,
            TermOp::AllocList
            | TermOp::AllocMap { .. }
            | TermOp::AllocMapSpread { .. }
            | TermOp::MakeEnumVariant(_)
            | TermOp::Neg
            | TermOp::Add
            | TermOp::Sub
            | TermOp::Mul
            | TermOp::Div
            | TermOp::Concat => term.inputs.iter().all(|&i| self.static_arg(i, depth + 1)),
            TermOp::Copy if !term.inputs.is_empty() => {
                self.static_arg(term.inputs[0], depth + 1)
            }
            TermOp::Copy if Self::is_phantom(term) && term.block_id == self.func(fi).body_block => {
                // A capture of top-level static data, by the function's one
                // closure, made once per run.
                let f = self.func(fi);
                let Some(slot) = f.capture_registers.iter().position(|r| *r == term.register) else {
                    return false;
                };
                self.sites[fi].len() == 1
                    && self.sites[fi].iter().all(|&site| {
                        let st = self.p.get_term(site);
                        self.once_per_run(site)
                            && st.inputs.get(slot).is_some_and(|&o| {
                                !self.cells.is_decl(o)
                                    && self.cells.decl_of(o).is_none()
                                    && matches!(self.cls(o), Class::Data)
                            })
                    })
            }
            _ => false,
        }
    }

    /// A term in words, for a witness: the call with its callee's name, the
    /// `state` or `var` it touches.
    fn describe(&self, term: &Term) -> String {
        match &term.op {
            TermOp::BuiltinCall(c) => format!("`{}`", self.string(*c)),
            TermOp::MethodCall { name, .. } => format!("`.{}`", self.string(*name)),
            TermOp::Call => {
                let mut callee = term.inputs.first().copied();
                let mut name = None;
                for _ in 0..16 {
                    let Some(c) = callee else { break };
                    let ct = self.p.get_term(c);
                    if let Some(n) = &ct.name {
                        name = Some(n.clone());
                        break;
                    }
                    match ct.op {
                        TermOp::MakeClosure(fid) => {
                            name = self
                                .fn_index
                                .get(&fid)
                                .and_then(|&i| self.func(i).name.clone());
                            break;
                        }
                        TermOp::Copy | TermOp::CellRead => callee = ct.inputs.first().copied(),
                        _ => break,
                    }
                }
                match self.single_user_target(term) {
                    Some(g) => format!(
                        "`{}`",
                        base_fn_name(
                            self.fn_index
                                .get(&g)
                                .and_then(|&i| self.func(i).name.as_deref())
                                .unwrap_or("<anonymous>")
                        )
                    ),
                    None => format!("`{}`", name.as_deref().map(base_fn_name).unwrap_or("a function value")),
                }
            }
            TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite => match &term.name {
                Some(n) => format!("state `{n}`"),
                None => "a state slot".to_string(),
            },
            TermOp::CellRead | TermOp::CellWrite => {
                let name = term
                    .inputs
                    .first()
                    .and_then(|o| self.cells.decl_of(*o))
                    .and_then(|d| self.cells.var_name(d));
                match name {
                    Some(n) => format!("var `{n}`"),
                    None => "a var".to_string(),
                }
            }
            TermOp::GetField(c) | TermOp::GetFieldOpt(c) => format!("field `.{}`", self.string(*c)),
            _ => "an expression".to_string(),
        }
    }
}

/// Why a native row gives `bit`, in words.
fn effect_note(row: NativeEffects, bit: Deps) -> &'static str {
    if bit != Deps::EFFECT {
        return "";
    }
    if row.effect {
        "an effect a replay cannot reproduce"
    } else if row.reads.contains(InputClasses::RNG) {
        "a draw from the random stream"
    } else {
        "a read that is neither a probe nor revisioned, which nothing can validate"
    }
}

/// Each single bit of a set, low to high.
fn bits(d: Deps) -> impl Iterator<Item = Deps> {
    (0..32).map(|i| Deps(1 << i)).filter(move |b| d.contains(*b))
}
