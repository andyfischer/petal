//! The static-value cache: top-level values that depend on no dynamic data
//! are built once and kept on the stack between runs.
//!
//! A host re-runs the whole script every frame, so without this every
//! top-level table is rebuilt every frame. [`crate::static_classify`] says
//! which top-level terms are *static*: a function of the source text alone.
//! Under [`OptFlags::static_cache`](crate::backend::OptFlags) the root code is
//! lowered with two extra instructions around each run of static
//! instructions, and a stack keeps what those runs computed:
//!
//! ```text
//!       static_skip run 0 [r7 = s0, r9 = s1] -> L     ; every slot it feeds filled? load, jump
//!       ...the run, exactly as it lowers without the cache...
//!       static_store [s0 = r7, s1 = r9]
//!   L:  ...
//! ```
//!
//! **No instruction is moved or removed.** With the flag off the code is the
//! same code without those two instructions, which is the oracle the cache is
//! held to (design: `docs/proposals/static-values.md`, section 3; work order:
//! `docs/tasks/static-value-optimization.md`, S1).
//!
//! # What gets a run, and what gets a slot
//!
//! The pass works on the lowered root code, after every other pass, because
//! that is where it can see what it must not break:
//!
//! - A **candidate** instruction belongs to a top-level statement whose term
//!   is static *data* and is in the [`StaticPlan`] (not a function, not a
//!   value that reaches a write), and is of a kind that is safe to leave out:
//!   no `state`, no cell, no `return`, no call of a native that is not
//!   constant. A statement with one instruction that fails this is out as a
//!   whole. So is one that reads a register some dynamic instruction writes
//!   (the classifier says that cannot happen; lowering artifacts are checked
//!   here rather than trusted).
//! - A **run** is a maximal stretch of candidate instructions. Lowering can
//!   put one statement's instruction in the middle of another's (a constant
//!   materialized where a loop first uses it), so a statement may be several
//!   runs; a run that control flow enters or leaves anywhere but at its two
//!   ends is not a run, and the statement that caused it is out.
//! - A register a run writes gets a **slot** when something that executes on
//!   every run reads it: dynamic code, a closure capture, a static statement
//!   that is not skippable (a top-level `assert`), the program's result. A
//!   register only a *later skippable run* reads gets none: the run that
//!   wrote it is skipped only when that later run is too, which is what a
//!   run's **feeds** say (its own slots plus those of the runs that read it).
//!   So a table that only exists to build another one is built with it and
//!   then dropped.
//! - A run that feeds nothing always executes: an unused `let BAD = XS[7]`
//!   still raises at its line, on every run. So does a run too small to be
//!   worth a check (fewer than [`MIN_WEIGHT`] allocations).
//!
//! A run that raises never reaches its `StaticStore`, so nothing of it is
//! kept and it raises again on the next run, at the same place.
//!
//! # What keeps a kept value from changing
//!
//! Three layers, the first two static and the last one at run time:
//!
//! 1. **A value that is mutated is never kept.** [`StaticPlan`] leaves out
//!    every top-level term whose value reaches the container operand of a
//!    write or of a mutating builtin (`let by_sec = [0, 0, 0]` then
//!    `by_sec[i] = v`), and the pass refuses a slot to any register that
//!    reaches an in-place instruction.
//! 2. **A slot load is never a fresh root.** Escape analysis and the last-use
//!    pass are handed the plan and will not rewrite a mutation in place on a
//!    term a slot may fill (`escape::analyze_with_static_roots`,
//!    `lastuse::apply_with_static_roots`).
//! 3. **The immutable bit.** `StaticStore` marks the value and every
//!    container under it ([`Heap::mark_immutable`](crate::heap::Heap::mark_immutable));
//!    the in-place heap operations copy such an object instead of editing
//!    it. This is what holds when a kept table is handed to a function that
//!    writes its parameter, or an element of it is edited through an alias.
//!
//! A kept value holds no function, `var` cell, host handle or pending
//! result: one that does is not stored, its slots are rejected for the life
//! of the table, and [`StaticSlots::rejected`] says so.
//!
//! # Lifetime
//!
//! The table belongs to the [`Stack`](crate::stack::Stack), is a GC root, and
//! is stamped with the lowering it was filled under
//! ([`StaticLayout::id`], new for every lowering, so for every program and
//! every flag set) and the revision of the program's constant table (moved
//! by a literal patched in place). A stack that meets any other stamp starts
//! empty. `transfer_stack_state` clears it with the closures, the enum data
//! and the memo records, so every reload rebuilds every static value once.
//! Following the source diff instead is the next stage.
//!
//! The cache is off under the `baseline` and `explain` policies, for
//! run-once `petal run`, whenever observations or the per-term trace are
//! preserved (a skipped run records nothing for them), and under the static
//! audit.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::backend::bytecode::isa::{BytecodeProgram, Inst, Label, Reg};
use crate::native_fn::{NativeConstness, NativeFnTable};
use crate::program::{BlockId, Program, TermId, TermOp};
use crate::static_classify::{StaticAnalysis, TermClass};
use crate::value::Value;

/// A run lighter than this is not worth a check per run: rebuilding one
/// small list costs less than testing for it. Counted in allocations, with a
/// call or a loop taken to be at least this much.
pub const MIN_WEIGHT: u32 = 2;

/// A run that would feed more slots than this keeps what later runs read of
/// it in slots of its own instead, which bounds the layout's size.
const MAX_FEEDS: usize = 64;

// ---------------------------------------------------------------------------
// The layout: what one lowering's static instructions refer to
// ---------------------------------------------------------------------------

/// What one slot holds, for a report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticSlotInfo {
    /// The declaration whose value it is (`TILES`, `lib::BASE`); `None` for
    /// an unnamed value a later statement reads.
    pub name: Option<String>,
    /// `file:line` of the term that produces it.
    pub at: Option<String>,
    /// That term.
    pub term: Option<TermId>,
}

/// One skippable run of static instructions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticRun {
    /// Every slot that must be filled for the run to be skipped: the ones it
    /// stores, and those of each later run that reads one of its registers.
    pub feeds: Vec<u32>,
    /// The slots its own `StaticStore` fills.
    pub stores: Vec<u32>,
    /// How many root instructions a skip does not execute. The calls among
    /// them run more: this is a floor on what a skip saves.
    pub instructions: usize,
    /// How many of those allocate a list, record or element directly.
    pub allocations: usize,
}

/// The static-value side of one lowering (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct StaticLayout {
    /// Unique to this lowering; 0 when the program was lowered without the
    /// cache. What a stack's table is stamped with.
    pub id: u64,
    pub slots: Vec<StaticSlotInfo>,
    pub runs: Vec<StaticRun>,
    /// Slot -> the runs that feed it (the inverse of [`StaticRun::feeds`]).
    feeders: Vec<Vec<u32>>,
    /// Runs of candidate instructions the pass found, skippable or not.
    pub candidate_runs: usize,
    /// Root instructions before the pass added its own.
    pub root_instructions: usize,
}

impl StaticLayout {
    /// Whether the lowering has anything to keep.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

static NEXT_LAYOUT_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// The table: what one stack holds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Slot {
    Empty,
    Filled(Value),
    /// The value could not be kept; the run executes on every run.
    Rejected,
}

/// Counters of one stack's table, cumulative since the stack was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StaticStats {
    /// Runs jumped over.
    pub skips: u64,
    /// Instructions those skips did not execute.
    pub skipped_instructions: u64,
    /// Slots filled.
    pub stored: u64,
    /// Slots whose value could not be kept.
    pub rejected: u64,
    /// Times the table was emptied: a reload, a re-lowering, a patched
    /// literal.
    pub clears: u64,
}

/// The static values one stack holds: a dense vector of slots indexed by the
/// lowering's [`StaticLayout`], and per run the number of slots it feeds
/// that are not filled yet (a `StaticSkip` is a test of that against zero).
#[derive(Debug, Clone, Default)]
pub struct StaticSlots {
    /// (layout id, constant-table revision) the contents belong to.
    stamp: Option<(u64, u64)>,
    slots: Vec<Slot>,
    pending: Vec<u32>,
    rejected: Vec<(u32, &'static str)>,
    pub stats: StaticStats,
}

impl StaticSlots {
    /// Make the table the one for `layout` at constant revision `revision`:
    /// a no-op when it already is, an empty table otherwise. Called on every
    /// dispatch into the VM, so the common case is two comparisons.
    #[inline]
    pub(crate) fn sync(&mut self, layout: &StaticLayout, revision: u64) {
        if self.stamp == Some((layout.id, revision)) {
            return;
        }
        self.reset(layout, revision);
    }

    #[cold]
    fn reset(&mut self, layout: &StaticLayout, revision: u64) {
        if self.slots.iter().any(|s| matches!(s, Slot::Filled(_))) {
            self.stats.clears += 1;
        }
        self.stamp = Some((layout.id, revision));
        self.slots.clear();
        self.slots.resize(layout.slots.len(), Slot::Empty);
        self.pending.clear();
        self.pending.extend(layout.runs.iter().map(|r| r.feeds.len() as u32));
        self.rejected.clear();
    }

    /// Drop every value. The next run rebuilds them.
    pub fn clear(&mut self) {
        if self.slots.iter().any(|s| matches!(s, Slot::Filled(_))) {
            self.stats.clears += 1;
        }
        self.stamp = None;
        self.slots.clear();
        self.pending.clear();
        self.rejected.clear();
    }

    /// How many slots the table has, and how many hold a value.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn filled(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s, Slot::Filled(_))).count()
    }

    /// The value in `slot`, if it holds one.
    pub fn get(&self, slot: u32) -> Option<Value> {
        match self.slots.get(slot as usize) {
            Some(Slot::Filled(v)) => Some(*v),
            _ => None,
        }
    }

    /// The slots whose value could not be kept, with what it held.
    pub fn rejected(&self) -> &[(u32, &'static str)] {
        &self.rejected
    }

    /// Whether `slot` was given up on (see [`rejected`](Self::rejected)).
    pub fn is_rejected(&self, slot: u32) -> bool {
        matches!(self.slots.get(slot as usize), Some(Slot::Rejected))
    }

    /// Whether every slot `run` feeds is filled.
    #[inline]
    pub(crate) fn ready(&self, run: u32) -> bool {
        self.pending.get(run as usize) == Some(&0)
    }

    /// Whether a `StaticStore` over `slots` has anything to do: none of them
    /// was rejected, and not all of them are filled already.
    pub(crate) fn wants(&self, slots: &[(u32, Reg)]) -> bool {
        let mut any_empty = false;
        for (s, _) in slots {
            match self.slots.get(*s as usize) {
                Some(Slot::Empty) => any_empty = true,
                Some(Slot::Filled(_)) => {}
                Some(Slot::Rejected) | None => return false,
            }
        }
        any_empty
    }

    /// Fill `slot`. A slot that already holds a value keeps it: the run was
    /// executed again only because something else it feeds is missing, and
    /// what it computed is the same value.
    pub(crate) fn fill(&mut self, layout: &StaticLayout, slot: u32, value: Value) {
        let Some(entry) = self.slots.get_mut(slot as usize) else { return };
        if !matches!(entry, Slot::Empty) {
            return;
        }
        *entry = Slot::Filled(value);
        self.stats.stored += 1;
        for &run in layout.feeders.get(slot as usize).map(|v| &v[..]).unwrap_or(&[]) {
            if let Some(p) = self.pending.get_mut(run as usize) {
                *p = p.saturating_sub(1);
            }
        }
    }

    /// Give up on every slot of one `StaticStore`: `slot` held `what`.
    pub(crate) fn reject(&mut self, slots: &[(u32, Reg)], slot: u32, what: &'static str) {
        for (s, _) in slots {
            if let Some(entry) = self.slots.get_mut(*s as usize)
                && matches!(entry, Slot::Empty)
            {
                *entry = Slot::Rejected;
                self.stats.rejected += 1;
            }
        }
        self.rejected.push((slot, what));
    }

    /// Every kept value, for the collector.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for s in &self.slots {
            if let Slot::Filled(v) = s {
                mark(*v);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The plan: which top-level terms may be kept at all
// ---------------------------------------------------------------------------

/// The top-level terms a slot may fill, decided on the term graph before
/// lowering so the in-place analyses can be told (see the module docs,
/// "What keeps a kept value from changing").
#[derive(Debug, Clone, Default)]
pub struct StaticPlan {
    /// Root-block terms that are static data and whose value reaches no
    /// write.
    pub roots: HashSet<TermId>,
}

impl StaticPlan {
    pub fn of(program: &Program, analysis: &StaticAnalysis) -> StaticPlan {
        let mutated = mutated_terms(program);
        // The classifier's own answer for declarations, which follows the
        // same aliases; either one is enough to leave a term out.
        let declared_mutated: HashSet<TermId> = analysis
            .report
            .declarations
            .iter()
            .filter(|d| d.mutated_by.is_some())
            .map(|d| d.term)
            .collect();
        let mut roots = HashSet::new();
        if let Some(terms) = program.block_terms.get(&program.root_block) {
            for &t in terms {
                if analysis.term_class(t) == TermClass::Data
                    && !mutated.contains(&t)
                    && !declared_mutated.contains(&t)
                {
                    roots.insert(t);
                }
            }
        }
        StaticPlan { roots }
    }

    /// The root registers of those terms. The root block is the first block
    /// of the root function's register file, so a root term's register is
    /// its flat register.
    pub fn root_regs(&self, program: &Program) -> HashSet<Reg> {
        self.roots
            .iter()
            .filter_map(|t| program.terms.get(t.0 as usize))
            .map(|t| t.register.0)
            .collect()
    }
}

/// Whether `term` writes into its first operand's container: an indexed or
/// field write, or a mutating builtin called bare or as a method. These are
/// the terms lowering may turn into an in-place update.
fn is_mutation(program: &Program, term: &crate::program::Term) -> bool {
    let builtin = |c| {
        program
            .get_string_constant(c)
            .is_some_and(crate::builtins::is_mutating_builtin)
    };
    match term.op {
        TermOp::SetIndex | TermOp::SetField(_) => true,
        TermOp::BuiltinCall(c) => builtin(c),
        TermOp::MethodCall { name, .. } => builtin(name),
        _ => false,
    }
}

/// Every term whose value may be the container some mutation writes into:
/// the container operands themselves, and back from each through what
/// carries a value unchanged (a copy, a phi and its carry-outs, the value of
/// a branch or match arm, the container of an earlier mutation).
fn mutated_terms(program: &Program) -> HashSet<TermId> {
    let mut last: HashMap<BlockId, TermId> = HashMap::new();
    let mut carried: HashMap<TermId, Vec<TermId>> = HashMap::new();
    for block in &program.blocks {
        let mut cur = block.entry;
        let mut steps = 0;
        while let Some(t) = cur {
            last.insert(block.id, t);
            cur = program.terms.get(t.0 as usize).and_then(|t| t.block_next);
            steps += 1;
            if steps > program.terms.len() {
                break;
            }
        }
        for po in &block.phi_outs {
            carried.entry(po.dest_term).or_default().push(po.src_term);
        }
    }
    let mut work: Vec<TermId> = program
        .terms
        .iter()
        .filter(|t| is_mutation(program, t))
        .filter_map(|t| t.inputs.first().copied())
        .collect();
    let mut seen: HashSet<TermId> = HashSet::new();
    while let Some(t) = work.pop() {
        if !seen.insert(t) {
            continue;
        }
        let Some(term) = program.terms.get(t.0 as usize) else { continue };
        let arm_results = |work: &mut Vec<TermId>| {
            for b in &term.child_blocks {
                work.extend(last.get(b).copied());
            }
            if let Some(arms) = program.match_arms.get(&t) {
                for arm in arms {
                    work.extend(last.get(&arm.body_block).copied());
                }
            }
        };
        match term.op {
            TermOp::Copy => work.extend(term.inputs.iter().copied()),
            TermOp::Phi => {
                work.extend(term.inputs.iter().copied());
                work.extend(carried.get(&t).into_iter().flatten().copied());
            }
            TermOp::Branch | TermOp::Match => arm_results(&mut work),
            TermOp::And | TermOp::Or | TermOp::Coalesce => {
                work.extend(term.inputs.first().copied());
                arm_results(&mut work);
            }
            _ if is_mutation(program, term) => work.extend(term.inputs.first().copied()),
            _ => {}
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Whether an instruction may sit in a run that is skipped. Everything a
/// static statement can lower to is here; what is not either cannot be
/// static (`state`, cells) or must run regardless (`return`).
fn skippable_inst(inst: &Inst, program: &Program, natives: &NativeFnTable) -> bool {
    match inst {
        Inst::BuiltinCall { name, .. } => {
            let Some(name) = program.get_string_constant(*name) else { return false };
            // The two enum reads answer from the program's own declarations,
            // which is how the classifier takes them. `__enum_data` publishes
            // into the stack and has to run.
            natives.constness_of_name(name) != NativeConstness::Dynamic
                || name == crate::program::ENUM_ALL_BUILTIN
                || name == crate::program::ENUM_FROM_BUILTIN
        }
        Inst::Return { .. }
        | Inst::CellNew { .. }
        | Inst::CellRead { .. }
        | Inst::CellWrite { .. }
        | Inst::StateInit { .. }
        | Inst::StateRead { .. }
        | Inst::StateWrite { .. }
        | Inst::StaticSkip { .. }
        | Inst::StaticStore { .. } => false,
        _ => true,
    }
}

/// What an instruction adds to its run's weight (see [`MIN_WEIGHT`]).
fn weight(inst: &Inst) -> u32 {
    match inst {
        Inst::AllocList { .. }
        | Inst::AllocMap { .. }
        | Inst::AllocMapSpread { .. }
        | Inst::AllocElement { .. }
        | Inst::Concat { .. }
        | Inst::SetField { .. }
        | Inst::SetIndex { .. } => 1,
        Inst::MakeEnumVariant { fields, .. } if !fields.is_empty() => 1,
        Inst::Call { .. }
        | Inst::MethodCall { .. }
        | Inst::BuiltinCall { .. }
        | Inst::ForEachInit { .. }
        | Inst::RangeInit { .. }
        | Inst::WhileInit { .. } => MIN_WEIGHT,
        _ => 0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Depends on dynamic data: what it writes is not static.
    Dynamic,
    /// Static, but executes on every run.
    Always,
    /// Static and may be skipped.
    Candidate,
}

struct Run {
    start: usize,
    end: usize,
    weight: u32,
    /// Registers it writes that something executing on every run reads.
    own: Vec<Reg>,
    /// Later skippable runs that read one of its registers.
    deps: BTreeSet<usize>,
    skippable: bool,
    /// Index into [`StaticLayout::runs`], when skippable.
    index: u32,
    /// First slot of `own`, when skippable.
    first_slot: u32,
}

/// Wrap the runs of static instructions in `bc`'s root code in
/// `StaticSkip` / `StaticStore` and record the layout in `bc.statics`. See
/// the module docs for the rules. Runs after every other pass: it reads
/// registers and labels as they finally are, and nothing after it moves
/// code.
pub fn apply(
    bc: &mut BytecodeProgram,
    program: &Program,
    analysis: &StaticAnalysis,
    natives: &NativeFnTable,
    plan: &StaticPlan,
) {
    let mut layout = StaticLayout {
        id: NEXT_LAYOUT_ID.fetch_add(1, Ordering::Relaxed),
        ..Default::default()
    };
    let n = bc.root.code.len();
    layout.root_instructions = n;
    if n == 0 || plan.roots.is_empty() {
        bc.statics = layout;
        return;
    }
    let class = |t: TermId| analysis.term_class(t);
    let origin = |ip: usize| bc.root.origins.get(ip).copied().flatten();

    // The top-level statement each root instruction belongs to. An
    // instruction with no origin of its own (a loop's bookkeeping) goes with
    // the one before it, and leading ones with the first statement.
    let parents = crate::static_audit::block_parents(program);
    let mut stmt: Vec<Option<TermId>> = Vec::with_capacity(n);
    for ip in 0..n {
        let own = origin(ip).and_then(|t| crate::static_audit::root_statement(program, &parents, t));
        let prev = stmt.last().copied().flatten();
        stmt.push(own.or(prev));
    }
    if let Some(first) = stmt.iter().copied().flatten().next() {
        for s in stmt.iter_mut().take_while(|s| s.is_none()) {
            *s = Some(first);
        }
    }

    let mut reads: Vec<Vec<Reg>> = Vec::with_capacity(n);
    let mut writes: Vec<Vec<Reg>> = Vec::with_capacity(n);
    for inst in &bc.root.code {
        reads.push(inst.read_regs().to_vec());
        let mut w = Vec::new();
        inst.for_each_write(&bc.match_binds, |r| w.push(r));
        writes.push(w);
    }

    // Statements that are out as a whole.
    let mut demoted: HashSet<TermId> = HashSet::new();
    for ip in 0..n {
        let Some(s) = stmt[ip] else { continue };
        if !plan.roots.contains(&s) {
            continue;
        }
        let own_dynamic = origin(ip).is_some_and(|t| class(t) == TermClass::Dynamic);
        if own_dynamic || !skippable_inst(&bc.root.code[ip], program, natives) {
            demoted.insert(s);
        }
    }

    let mut kind = vec![Kind::Dynamic; n];
    let mut tainted: HashSet<Reg> = HashSet::new();
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut run_of: Vec<Option<usize>> = vec![None; n];
    // Each round takes at least one statement out, so this ends.
    loop {
        for ip in 0..n {
            let stmt_dynamic = stmt[ip].is_none_or(|s| class(s) == TermClass::Dynamic);
            let own_dynamic = origin(ip).is_some_and(|t| class(t) == TermClass::Dynamic);
            kind[ip] = if stmt_dynamic || own_dynamic {
                Kind::Dynamic
            } else if stmt[ip].is_some_and(|s| plan.roots.contains(&s) && !demoted.contains(&s)) {
                Kind::Candidate
            } else {
                Kind::Always
            };
        }
        // A register any dynamic instruction writes is not static, whoever
        // else writes it.
        tainted.clear();
        for ip in 0..n {
            if kind[ip] == Kind::Dynamic {
                tainted.extend(writes[ip].iter().copied());
            }
        }
        let mut changed = false;
        for ip in 0..n {
            if kind[ip] == Kind::Candidate
                && reads[ip].iter().any(|r| tainted.contains(r))
                && let Some(s) = stmt[ip]
            {
                changed |= demoted.insert(s);
            }
        }
        if changed {
            continue;
        }

        bounds.clear();
        run_of.iter_mut().for_each(|r| *r = None);
        let mut ip = 0;
        while ip < n {
            if kind[ip] != Kind::Candidate {
                ip += 1;
                continue;
            }
            let start = ip;
            while ip < n && kind[ip] == Kind::Candidate {
                run_of[ip] = Some(bounds.len());
                ip += 1;
            }
            bounds.push((start, ip));
        }
        // Control enters a run at its first instruction and leaves it at its
        // end, or the run cannot be jumped over.
        for ip in 0..n {
            for to in bc.root.code[ip].targets() {
                let to = to as usize;
                if let Some(k) = run_of[ip] {
                    let (s, e) = bounds[k];
                    if (to < s || to > e)
                        && let Some(st) = stmt[ip]
                    {
                        changed |= demoted.insert(st);
                    }
                }
                if to < n
                    && let Some(j) = run_of[to]
                    && run_of[ip] != Some(j)
                    && to != bounds[j].0
                    && let Some(st) = stmt[to]
                {
                    changed |= demoted.insert(st);
                }
            }
        }
        if !changed {
            break;
        }
    }
    layout.candidate_runs = bounds.len();
    if bounds.is_empty() {
        bc.statics = layout;
        return;
    }

    // Registers that reach the container operand of an in-place instruction,
    // back through what carries a value unchanged. No slot may fill one.
    let mut in_place: HashSet<Reg> = HashSet::new();
    for inst in &bc.root.code {
        match inst {
            Inst::SetFieldInPlace { obj, .. } | Inst::SetIndexInPlace { obj, .. } => {
                in_place.insert(*obj);
            }
            Inst::BuiltinCall { args, in_place: true, .. } => {
                in_place.extend(args.first().copied());
            }
            _ => {}
        }
    }
    loop {
        let before = in_place.len();
        for inst in &bc.root.code {
            match inst {
                Inst::Move { dst, src } if in_place.contains(dst) => {
                    in_place.insert(*src);
                }
                Inst::MatchArm { subject, term, arm, .. } => {
                    let binds = bc.match_binds.get(&(*term, *arm));
                    if binds.is_some_and(|b| b.iter().any(|(_, r)| in_place.contains(r))) {
                        in_place.insert(*subject);
                    }
                }
                _ => {}
            }
        }
        if in_place.len() == before {
            break;
        }
    }

    let mut readers: HashMap<Reg, Vec<usize>> = HashMap::new();
    for (ip, rs) in reads.iter().enumerate() {
        for r in rs {
            readers.entry(*r).or_default().push(ip);
        }
    }

    // Later runs first: whether a reader is skippable decides whether the
    // register it reads needs a slot.
    let mut runs: Vec<Run> = bounds
        .iter()
        .map(|&(start, end)| Run {
            start,
            end,
            weight: bc.root.code[start..end].iter().map(weight).sum(),
            own: Vec::new(),
            deps: BTreeSet::new(),
            skippable: false,
            index: 0,
            first_slot: 0,
        })
        .collect();
    let mut feed_count: Vec<usize> = vec![0; runs.len()];
    for k in (0..runs.len()).rev() {
        let (start, end) = (runs[k].start, runs[k].end);
        let mut written: Vec<Reg> = (start..end).flat_map(|ip| writes[ip].iter().copied()).collect();
        written.sort_unstable();
        written.dedup();
        let mut own: Vec<Reg> = Vec::new();
        let mut all_read: Vec<Reg> = Vec::new();
        let mut deps: BTreeSet<usize> = BTreeSet::new();
        for &r in &written {
            let mut kept = bc.root.result_reg == Some(r);
            let mut read_outside = kept;
            for &ip in readers.get(&r).map(|v| &v[..]).unwrap_or(&[]) {
                if (start..end).contains(&ip) {
                    continue;
                }
                read_outside = true;
                match run_of[ip] {
                    Some(j) if j > k && runs[j].skippable => {
                        deps.insert(j);
                    }
                    _ => kept = true,
                }
            }
            if kept {
                own.push(r);
            }
            if read_outside {
                all_read.push(r);
            }
        }
        // Too many slots to feed: keep everything a later run reads instead.
        let mut feeds = own.len() + deps.iter().map(|&j| feed_count[j]).sum::<usize>();
        if feeds > MAX_FEEDS {
            own = all_read;
            deps.clear();
            feeds = own.len();
        }
        let storable = own.iter().all(|r| !tainted.contains(r) && !in_place.contains(r));
        let run = &mut runs[k];
        run.skippable = storable && feeds > 0 && run.weight >= MIN_WEIGHT;
        if run.skippable {
            feed_count[k] = feeds;
            run.own = own;
            run.deps = deps;
        }
    }

    // Number the runs and the slots in code order.
    let decl_names: HashMap<TermId, &str> = analysis
        .report
        .declarations
        .iter()
        .map(|d| (d.term, d.qualified.as_str()))
        .collect();
    let mut writer_term: HashMap<Reg, TermId> = HashMap::new();
    for ip in 0..n {
        if let Some(t) = origin(ip) {
            for r in &writes[ip] {
                writer_term.insert(*r, t);
            }
        }
    }
    for run in runs.iter_mut().filter(|r| r.skippable) {
        run.index = layout.runs.len() as u32;
        run.first_slot = layout.slots.len() as u32;
        for r in &run.own {
            // A root term's register is its flat register, so the term that
            // names the value is the root term with this register when there
            // is one, else whatever instruction wrote it.
            let term = plan
                .roots
                .iter()
                .copied()
                .find(|t| program.get_term(*t).register.0 == *r && decl_names.contains_key(t))
                .or_else(|| writer_term.get(r).copied());
            layout.slots.push(StaticSlotInfo {
                name: term.and_then(|t| {
                    decl_names
                        .get(&t)
                        .map(|s| s.to_string())
                        .or_else(|| program.get_term(t).name.clone())
                }),
                at: term.and_then(|t| crate::static_audit::position(program, t)),
                term,
            });
        }
        let stores: Vec<u32> = (0..run.own.len() as u32).map(|i| run.first_slot + i).collect();
        layout.runs.push(StaticRun {
            feeds: stores.clone(),
            stores,
            instructions: run.end - run.start,
            allocations: bc.root.code[run.start..run.end]
                .iter()
                .filter(|i| {
                    matches!(
                        i,
                        Inst::AllocList { .. }
                            | Inst::AllocMap { .. }
                            | Inst::AllocMapSpread { .. }
                            | Inst::AllocElement { .. }
                    )
                })
                .count(),
        });
    }
    for k in (0..runs.len()).rev() {
        if !runs[k].skippable {
            continue;
        }
        let mut feeds: BTreeSet<u32> = layout.runs[runs[k].index as usize].feeds.iter().copied().collect();
        for &j in &runs[k].deps {
            feeds.extend(layout.runs[runs[j].index as usize].feeds.iter().copied());
        }
        layout.runs[runs[k].index as usize].feeds = feeds.into_iter().collect();
    }
    layout.feeders = vec![Vec::new(); layout.slots.len()];
    for (i, run) in layout.runs.iter().enumerate() {
        for &s in &run.feeds {
            layout.feeders[s as usize].push(i as u32);
        }
    }
    if layout.runs.is_empty() {
        bc.statics = layout;
        return;
    }

    // Insert the two instructions per run and move every label.
    let skip_at: HashMap<usize, usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.skippable)
        .map(|(k, r)| (r.start, k))
        .collect();
    let store_at: HashMap<usize, usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.skippable && !r.own.is_empty())
        .map(|(k, r)| (r.end, k))
        .collect();
    let old_code = std::mem::take(&mut bc.root.code);
    let old_origins = std::mem::take(&mut bc.root.origins);
    let mut code: Vec<Inst> = Vec::with_capacity(n + 2 * layout.runs.len());
    let mut origins: Vec<Option<TermId>> = Vec::with_capacity(code.capacity());
    // Where each old instruction lands, and where the instruction put in
    // front of it does.
    let mut new_pos: Vec<usize> = vec![0; n + 1];
    let mut skip_pos: HashMap<usize, usize> = HashMap::new();
    let mut store_pos: HashMap<usize, usize> = HashMap::new();
    let slots_of = |run: &Run| -> Box<[(u32, Reg)]> {
        run.own
            .iter()
            .enumerate()
            .map(|(i, r)| (run.first_slot + i as u32, *r))
            .collect()
    };
    let mut old = old_code.into_iter().zip(old_origins.into_iter().chain(std::iter::repeat(None)));
    for ip in 0..=n {
        if let Some(&k) = store_at.get(&ip) {
            store_pos.insert(k, code.len());
            code.push(Inst::StaticStore { slots: slots_of(&runs[k]) });
            origins.push(None);
        }
        if let Some(&k) = skip_at.get(&ip) {
            skip_pos.insert(k, code.len());
            code.push(Inst::StaticSkip {
                after: 0,
                run: runs[k].index,
                slots: slots_of(&runs[k]),
            });
            origins.push(None);
        }
        new_pos[ip] = code.len();
        if let Some((inst, o)) = old.next() {
            code.push(inst);
            origins.push(o);
        }
    }
    let retarget = |from_old: Option<usize>, to: usize| -> Label {
        let from_run = from_old.and_then(|ip| run_of[ip]);
        // A jump out of a run to its end lands on the store.
        if let Some(k) = from_run
            && runs[k].end == to
            && let Some(&p) = store_pos.get(&k)
        {
            return p as Label;
        }
        // A jump to a run's first instruction from outside lands on the skip.
        if let Some(&k) = skip_at.get(&to)
            && from_run != Some(k)
            && let Some(&p) = skip_pos.get(&k)
        {
            return p as Label;
        }
        new_pos[to.min(n)] as Label
    };
    let mut old_ip = 0usize;
    for inst in code.iter_mut() {
        match inst {
            Inst::StaticSkip { after, run, .. } => {
                let k = runs.iter().position(|r| r.skippable && r.index == *run).unwrap_or(0);
                // Past the run and past its store; onto the next run's skip
                // if one starts right there.
                *after = match skip_pos.get(&skip_at.get(&runs[k].end).copied().unwrap_or(usize::MAX)) {
                    Some(&p) => p as Label,
                    None => new_pos[runs[k].end] as Label,
                };
            }
            Inst::StaticStore { .. } => {}
            other => {
                other.for_each_target_mut(|to| *to = retarget(Some(old_ip), *to as usize));
                old_ip += 1;
            }
        }
    }
    bc.root.code = code;
    bc.root.origins = origins;
    bc.statics = layout;
}

/// A summary of a layout and one line per slot, for `PETAL_OPT_STATS` and
/// tests.
pub fn describe(layout: &StaticLayout) -> String {
    let instructions: usize = layout.runs.iter().map(|r| r.instructions).sum();
    let allocations: usize = layout.runs.iter().map(|r| r.allocations).sum();
    let mut out = format!(
        "static cache: {} candidate runs, {} skippable, {} slots; a frame that skips them all \
         leaves out {} of {} root instructions ({} of them allocations) for {} checks\n",
        layout.candidate_runs,
        layout.runs.len(),
        layout.slots.len(),
        instructions,
        layout.root_instructions,
        allocations,
        layout.runs.len(),
    );
    for (i, s) in layout.slots.iter().enumerate() {
        out.push_str(&format!(
            "  s{i}: {}{}\n",
            s.name.as_deref().unwrap_or("(unnamed)"),
            s.at.as_ref().map(|a| format!(" at {a}")).unwrap_or_default()
        ));
    }
    out
}
