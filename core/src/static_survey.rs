//! A measurement, not a feature: how much work a program does building
//! values inside function bodies that could only ever come out the same.
//!
//! The static-value cache keeps such values (`crate::static_cache`, the
//! `in_fn` module). It was written because of what this module measured
//! (`docs/tasks/static-value-optimization.md`, "Later: constants inside
//! function bodies"), and this is still how to ask what a program has to
//! gain, or has left to gain with the cache on: [`survey`] finds the
//! expressions, and a [`SiteCounter`] on the profiler counts what they cost
//! while the program runs. Nothing here changes how a program runs.
//!
//! # What counts
//!
//! What is *invariant* is [`FnPlan`]'s answer, the one the cache acts on:
//! a term the classifier calls static that reads nothing of the call. The
//! classifier alone is not enough: it classifies a function body *assuming
//! its parameters are data*, so `[a, 1]` is static there and is a
//! different list on every call.
//!
//! A **site** is a maximal invariant expression: one that no other
//! invariant term of the same function reads, and that is not under an
//! invariant loop, branch or match (which is a site whole). Only sites
//! with something to save are counted (an allocation, a call or a loop
//! somewhere under them; a bare number costs what a slot load would), and
//! a site whose value reaches the container operand of a write is left
//! out, as [`StaticPlan`](crate::static_cache::StaticPlan) leaves out such
//! a top-level value: `let out = []` in front of a loop that appends is
//! the commonest invariant allocation there is, and it cannot be kept.
//!
//! This is an **upper bound** on what the cache saves: it does not ask
//! whether the lowering can cut the site out (a jump in or out, a register
//! something else writes), whether the value would be rejected (a host
//! handle), or whether a caller writes to what the function returns. Run
//! with the cache on, what it still counts is what the cache did not get.

use std::collections::HashSet;

use crate::program::{Program, Term, TermId, TermOp};
use crate::static_cache::{FnPlan, StaticPlan};
use crate::static_classify::{StaticAnalysis, TermClass};

/// One maximal invariant expression inside a function.
#[derive(Debug, Clone)]
pub struct Site {
    /// The term whose value a slot would hold.
    pub head: TermId,
    /// The function it is written in (an index into `Program::functions`).
    pub function: u32,
    /// How many terms it is made of, the head included.
    pub terms: u32,
    /// How many of them allocate (a list, a record, an element, a variant
    /// with a payload, a string built by `++`).
    pub allocations: u32,
    /// How many of them call something or loop.
    pub calls: u32,
}

/// What [`survey`] found.
#[derive(Debug, Clone, Default)]
pub struct Survey {
    pub sites: Vec<Site>,
    /// Term id -> the site it belongs to, plus one; 0 for none.
    pub member: Vec<u32>,
    /// Invariant maximal expressions left out because their value reaches a
    /// write.
    pub mutated: u32,
    /// Left out because nothing under them allocates, calls or loops.
    pub weightless: u32,
}

fn is_phantom(term: &Term) -> bool {
    matches!(term.op, TermOp::Copy) && term.inputs.is_empty() && term.name.is_some()
}

/// Find the invariant sites of every function of `program`. What is
/// invariant is [`FnPlan`]'s answer, the one the cache itself acts on.
pub fn survey(program: &Program, analysis: &StaticAnalysis) -> Survey {
    let n = program.terms.len();
    let plan = StaticPlan::of(program, analysis);
    let fns = FnPlan::of(program, analysis, &plan);
    let mutated = &plan.mutated;
    // Under an invariant loop, branch or match: built with it.
    let covered = |t: TermId| fns.region_of(t).is_some();

    // Heads: invariant, executed, read by no invariant term of a function,
    // and not inside an invariant region.
    let mut has_inv_user = vec![false; n];
    for term in &program.terms {
        if fns.function_of(term.id).is_none() || !(fns.invariant(term.id) || covered(term.id)) {
            continue;
        }
        for x in &term.inputs {
            has_inv_user[x.0 as usize] = true;
        }
    }
    let mut out = Survey { member: vec![0; n], ..Survey::default() };
    for term in &program.terms {
        let i = term.id.0 as usize;
        if !fns.invariant(term.id)
            || is_phantom(term)
            || has_inv_user[i]
            || covered(term.id)
            || analysis.term_class(term.id) != TermClass::Data
        {
            continue;
        }
        let Some(function) = fns.function_of(term.id) else { continue };
        // The expression under it.
        let mut members: Vec<TermId> = Vec::new();
        let mut seen: HashSet<TermId> = HashSet::new();
        let mut work = vec![term.id];
        while let Some(t) = work.pop() {
            let tt = program.get_term(t);
            if fns.function_of(t).is_none() || !fns.invariant(t) || is_phantom(tt) || !seen.insert(t) {
                continue;
            }
            members.push(t);
            work.extend(tt.inputs.iter().copied());
            // Everything under an invariant loop, branch or match is built
            // with it, and so is what that reads from outside.
            for &x in fns.under(t) {
                let xt = program.get_term(x);
                if !is_phantom(xt) && seen.insert(x) {
                    members.push(x);
                    work.extend(xt.inputs.iter().copied());
                }
            }
        }
        let (mut allocations, mut calls) = (0, 0);
        for &m in &members {
            let mt = program.get_term(m);
            match mt.op {
                TermOp::AllocList
                | TermOp::AllocMap { .. }
                | TermOp::AllocMapSpread { .. }
                | TermOp::AllocElement { .. }
                | TermOp::Concat => allocations += 1,
                TermOp::MakeEnumVariant(_) if !mt.inputs.is_empty() => allocations += 1,
                TermOp::Call
                | TermOp::MethodCall { .. }
                | TermOp::BuiltinCall(_)
                | TermOp::ForLoop
                | TermOp::NumericForLoop
                | TermOp::WhileLoop => calls += 1,
                _ => {}
            }
        }
        if allocations + calls == 0 {
            out.weightless += 1;
            continue;
        }
        if mutated.contains(&term.id) {
            out.mutated += 1;
            continue;
        }
        let index = out.sites.len() as u32 + 1;
        let mut count = 0;
        for &m in &members {
            // A term two sites are built from is charged to the first.
            if out.member[m.0 as usize] == 0 {
                out.member[m.0 as usize] = index;
                count += 1;
            }
        }
        out.sites.push(Site { head: term.id, function, terms: count, allocations, calls });
    }
    out
}

/// What the sites of a [`Survey`] cost while the program runs. Attached to
/// the profiler ([`VmProfile::sites`](crate::profile::VmProfile::sites)) and
/// fed one call per retired instruction, on the hooked dispatch path only.
#[derive(Debug, Clone, Default)]
pub struct SiteCounter {
    member: Vec<u32>,
    /// Per site: times entered, instructions retired under it (the
    /// functions it calls included), objects allocated under it.
    pub entered: Vec<u64>,
    pub instructions: Vec<u64>,
    pub allocations: Vec<u64>,
    /// The site an instruction that pushed a frame belongs to, and the
    /// depth it stood at: everything deeper is charged to it.
    inside: Option<(usize, u32)>,
    /// The site the previous instruction was charged to (0: none), and the
    /// heap's allocation count when it started.
    last: u32,
    last_allocs: u64,
}

impl SiteCounter {
    pub fn new(survey: &Survey) -> Self {
        let n = survey.sites.len();
        SiteCounter {
            member: survey.member.clone(),
            entered: vec![0; n],
            instructions: vec![0; n],
            allocations: vec![0; n],
            ..Self::default()
        }
    }

    /// One instruction is about to execute in the frame at `depth`, lowered
    /// from `origin`, with `allocs` objects allocated so far.
    #[inline]
    pub fn step(&mut self, origin: Option<TermId>, depth: usize, allocs: u64) {
        self.settle(allocs);
        let site = match self.inside {
            Some((d, s)) if depth > d => s,
            _ => {
                self.inside = None;
                let s = origin.and_then(|t| self.member.get(t.0 as usize).copied()).unwrap_or(0);
                if s != 0 {
                    self.inside = Some((depth, s));
                }
                s
            }
        };
        if site != 0 {
            let i = site as usize - 1;
            self.instructions[i] += 1;
            if self.last != site {
                self.entered[i] += 1;
            }
        }
        self.last = site;
    }

    /// Charge what was allocated since the last instruction started to the
    /// site it belonged to. Call once more after the last run.
    pub fn settle(&mut self, allocs: u64) {
        if self.last != 0 {
            self.allocations[self.last as usize - 1] += allocs.saturating_sub(self.last_allocs);
        }
        self.last_allocs = allocs;
    }

    /// A run ended: the next instruction starts no site in the middle.
    pub fn end_run(&mut self, allocs: u64) {
        self.settle(allocs);
        self.inside = None;
        self.last = 0;
    }
}
