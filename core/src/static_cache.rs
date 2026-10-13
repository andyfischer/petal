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
//! - **Enum constant data.** `enum Kind  park(code: 2) end` publishes one
//!   record per variant into the stack's enum table, by a call the
//!   declaration makes on every run. Where the data is static that call is a
//!   candidate like any other, and the record it publishes gets a slot
//!   whoever reads it: a frame that skips the run leaves the published
//!   record where the first run put it. The enum table and this one are
//!   emptied together, so a kept slot always has its record published.
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
//! and the memo records.
//!
//! # Following the diff
//!
//! [`Env::reload_program`](crate::env::Env::reload_program) and
//! [`Env::set_config_value`](crate::env::Env::set_config_value) put back,
//! after that clearing, the values the edit cannot have changed
//! (`env/static_follow.rs` decides which; [`StaticSlots::install`] takes the
//! answer). A slot is one of three things afterwards:
//!
//! - **kept**: filled with the value it held. Its declaration is not inside
//!   anything the edit touched, the set of declarations it depends on
//!   ([`StaticSlotInfo::deps`], from the classifier) has the same names in
//!   the old and the new program, and none of them is inside anything the
//!   edit touched either.
//! - **rebuilt**: empty, with the old value remembered beside it. The run
//!   executes on the next frame, and a `StaticStore` that finds the new
//!   value [`identical`] to the remembered one keeps the old heap value, so
//!   whatever holds the old one keeps holding the current one.
//! - **deferred**: filled with the value it held although the edit changed
//!   it, because its last build was slow and the edit is one tick of a drag
//!   ([`StaticDebounce`]). It is rebuilt when the drag pauses.
//!
//! A run executes whenever one slot it feeds is empty, so one rebuilt slot
//! re-executes its whole run; the other slots of the run keep the values
//! they were given. An enum's data is always rebuilt: the stack's enum
//! table is emptied by every reload and only the run can publish into it.
//!
//! The cache is off under the `baseline` and `explain` policies, for
//! run-once `petal run`, whenever observations or the per-term trace are
//! preserved (a skipped run records nothing for them), and under the static
//! audit.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::backend::bytecode::isa::{BytecodeProgram, Inst, Label, Reg};
use crate::native_fn::{NativeConstness, NativeFnTable};
use crate::heap::Heap;
use crate::program::{BlockId, Program, StateKey, TermId, TermOp};
use crate::source_map::FileId;
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
    /// Declared with `const`: the author asked for it to be constant, so a
    /// value that cannot be kept is worth saying louder.
    pub is_const: bool,
    /// What a reload matches this slot by across two lowerings: the
    /// module-qualified name of the declaration whose value it is. `None`
    /// for a value with no declaration of its own (the `[1, 2, 3]` written
    /// inside a call) and for a name two slots of one lowering share; such a
    /// slot is rebuilt by every reload that recompiles.
    pub key: Option<String>,
    /// Every declaration the value depends on, the slot's own included, as
    /// sorted indices into [`FollowInfo::decls`]. `None` when that is not
    /// known, which a reload takes to mean "everything".
    pub deps: Option<Box<[u32]>>,
    /// The terms whose instructions write the slot's register: where the
    /// value is written in the source, for a reload asking whether an edit
    /// was inside it.
    pub origins: Vec<TermId>,
    /// The slot holds the record an enum variant's constant data publishes.
    pub enum_data: bool,
}

impl StaticSlotInfo {
    /// The module-qualified name of the declaration whose value the slot
    /// holds, when it is one a reload can match by name.
    pub fn declaration(&self) -> Option<&str> {
        self.key.as_deref().filter(|k| !k.starts_with('\u{0}'))
    }
}

/// One declaration of a program, as a reload needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowDecl {
    /// Module-qualified name (`TILES`, `lib::BASE`).
    pub name: String,
    pub file: FileId,
    /// The terms that bind it: where it is written.
    pub defs: Vec<TermId>,
    /// For a top-level `state`: its key, and what its initializer depends
    /// on (indices into [`FollowInfo::decls`], sorted, closed transitively).
    pub state: Option<(StateKey, Box<[u32]>)>,
}

/// What a reload needs to know about a lowering to follow an edit through
/// it: every declaration with where it is written, and what each `state`
/// initializer reads. The slots' own dependency sets are on
/// [`StaticSlotInfo`]. In the classifier's report order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowInfo {
    pub decls: Vec<FollowDecl>,
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
    /// The program's declarations, for a reload (see "Following the diff").
    pub follow: Arc<FollowInfo>,
}

impl StaticLayout {
    /// Whether the lowering has anything to keep.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The runs that execute while `slot` is empty.
    pub fn feeders(&self, slot: u32) -> &[u32] {
        self.feeders.get(slot as usize).map(|v| &v[..]).unwrap_or(&[])
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
    /// Slots a reload filled again with the value they held.
    pub carried: u64,
    /// Rebuilt values that turned out identical to the one the slot held
    /// before the reload, which was kept instead.
    pub reused: u64,
}

/// How a slow rebuild is put off while a value is being dragged (see
/// [`Env::set_static_debounce`](crate::env::Env::set_static_debounce)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticDebounce {
    /// A slot whose last build took at least this long is not rebuilt on
    /// every tick of a drag.
    pub threshold: Duration,
    /// Two `set_config_value` calls closer together than this are one drag;
    /// a frame that starts this long after the last one rebuilds what the
    /// drag put off.
    pub quiet: Duration,
}

impl Default for StaticDebounce {
    fn default() -> Self {
        StaticDebounce {
            threshold: Duration::from_millis(4),
            quiet: Duration::from_millis(150),
        }
    }
}

/// What a reload puts in one slot of the table it leaves (see the module
/// docs, "Following the diff").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seed {
    /// Nothing: the slot is new, or held nothing.
    Empty,
    /// The value it held, which the edit cannot have changed.
    Kept(Value),
    /// Empty, but this is what it held: a rebuilt value identical to it is
    /// dropped in its favour.
    Rebuild(Value),
    /// The value it held, which the edit did change; rebuilt when the drag
    /// that changed it pauses.
    Deferred(Value),
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
    /// How many of `rejected` a host has been told about
    /// ([`take_unreported`](Self::take_unreported)).
    reported: usize,
    /// Per slot, the value it held before the reload that emptied it. Empty
    /// when no slot has one.
    prior: Vec<Option<Value>>,
    /// Slots that hold a value an edit changed, waiting for the drag to
    /// pause; and when the last tick of the drag was.
    deferred: Vec<u32>,
    /// Slots rebuilt while others were deferred: they may have been built
    /// from a deferred slot's old value, and are rebuilt again with it.
    redo: Vec<u32>,
    last_live_edit: Option<Instant>,
    /// Per run, how long its last execution took, in nanoseconds; 0 when it
    /// has not been timed. Kept across a reload that leaves the lowering in
    /// place.
    build_ns: Vec<u64>,
    building: Option<(u32, Instant)>,
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
        self.reported = 0;
        self.prior.clear();
        self.deferred.clear();
        self.redo.clear();
        self.building = None;
        self.build_ns.clear();
        self.build_ns.resize(layout.runs.len(), 0);
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
        self.reported = 0;
        self.prior.clear();
        self.deferred.clear();
        self.redo.clear();
        self.building = None;
        // `build_ns` stays: `install` asks for it when the same lowering is
        // filled again, and `reset` drops it for any other.
    }

    /// The lowering the table was last filled under, and the constant
    /// revision; `None` after a [`clear`](Self::clear).
    pub fn stamp(&self) -> Option<(u64, u64)> {
        self.stamp
    }

    /// Make the table the one for `layout` at `revision`, holding what
    /// `seeds` says slot by slot: what a reload that follows the diff leaves
    /// (see the module docs). `timings` are the per-run build times of the
    /// table this one replaces, kept when it was for the same lowering.
    pub(crate) fn install(
        &mut self,
        layout: &StaticLayout,
        revision: u64,
        seeds: &[Seed],
        timings: Option<Vec<u64>>,
    ) {
        let last_live_edit = self.last_live_edit;
        self.reset(layout, revision);
        self.last_live_edit = last_live_edit;
        if let Some(t) = timings.filter(|t| t.len() == layout.runs.len()) {
            self.build_ns = t;
        }
        for (i, seed) in seeds.iter().enumerate().take(self.slots.len()) {
            match *seed {
                Seed::Empty => {}
                Seed::Kept(v) | Seed::Deferred(v) => {
                    self.slots[i] = Slot::Filled(v);
                    self.stats.carried += 1;
                    for &run in layout.feeders(i as u32) {
                        if let Some(p) = self.pending.get_mut(run as usize) {
                            *p = p.saturating_sub(1);
                        }
                    }
                    if matches!(seed, Seed::Deferred(_)) {
                        self.deferred.push(i as u32);
                    }
                }
                Seed::Rebuild(v) => {
                    if self.prior.is_empty() {
                        self.prior.resize(self.slots.len(), None);
                    }
                    self.prior[i] = Some(v);
                }
            }
        }
    }

    /// The per-run build times, for the table that replaces this one.
    pub(crate) fn timings(&self) -> Vec<u64> {
        self.build_ns.clone()
    }

    /// How long the runs that execute while `slot` is empty took the last
    /// time they ran. A run with no slots of its own is not timed.
    pub fn build_time(&self, layout: &StaticLayout, slot: u32) -> Duration {
        let ns: u64 = layout
            .feeders(slot)
            .iter()
            .map(|r| self.build_ns.get(*r as usize).copied().unwrap_or(0))
            .sum();
        Duration::from_nanos(ns)
    }

    /// The value `slot` held before the reload that emptied it, if a rebuilt
    /// value should be compared with it.
    #[inline]
    pub(crate) fn prior(&self, slot: u32) -> Option<Value> {
        self.prior.get(slot as usize).copied().flatten()
    }

    /// Whether any slot is waiting for a value to compare with.
    #[inline]
    pub(crate) fn has_prior(&self) -> bool {
        !self.prior.is_empty()
    }

    /// The slots holding a value an edit changed, not rebuilt yet.
    pub fn deferred(&self) -> &[u32] {
        &self.deferred
    }

    /// The slots that were rebuilt while others were deferred (see
    /// [`set_redo`](Self::set_redo)).
    pub fn redo(&self) -> &[u32] {
        &self.redo
    }

    /// Name the slots that the edit which deferred some rebuilds also
    /// reached and that are being rebuilt now. One of them may read a
    /// deferred slot, and would then be built from its old value, so
    /// [`flush_deferred`](Self::flush_deferred) rebuilds them again.
    pub(crate) fn set_redo(&mut self, slots: Vec<u32>) {
        self.redo = if self.deferred.is_empty() { Vec::new() } else { slots };
    }

    /// A tick of a drag happened now. Returns whether it continues one: the
    /// tick before it was less than `quiet` ago.
    pub(crate) fn note_live_edit(&mut self, quiet: Duration) -> bool {
        let now = Instant::now();
        let continues = self.last_live_edit.is_some_and(|t| now.duration_since(t) < quiet);
        self.last_live_edit = Some(now);
        continues
    }

    /// Whether the drag that deferred some rebuilds has paused for `quiet`.
    pub(crate) fn drag_paused(&self, quiet: Duration) -> bool {
        !self.deferred.is_empty() && self.last_live_edit.is_none_or(|t| t.elapsed() >= quiet)
    }

    /// Empty every deferred slot, remembering its value for the comparison:
    /// the next run rebuilds them. Returns how many there were.
    pub(crate) fn flush_deferred(&mut self, layout: &StaticLayout) -> usize {
        let slots = std::mem::take(&mut self.deferred);
        let redo = std::mem::take(&mut self.redo);
        for &s in slots.iter().chain(&redo) {
            let Some(entry) = self.slots.get_mut(s as usize) else { continue };
            let Slot::Filled(v) = *entry else { continue };
            *entry = Slot::Empty;
            if self.prior.is_empty() {
                self.prior.resize(self.slots.len(), None);
            }
            self.prior[s as usize] = Some(v);
            for &run in layout.feeders(s) {
                if let Some(p) = self.pending.get_mut(run as usize) {
                    *p += 1;
                }
            }
        }
        slots.len()
    }

    /// A run is about to execute (its `StaticSkip` fell through).
    #[inline]
    pub(crate) fn begin_build(&mut self, run: u32) {
        self.building = Some((run, Instant::now()));
    }

    /// The run in progress reached its `StaticStore`.
    #[inline]
    pub(crate) fn end_build(&mut self) {
        if let Some((run, since)) = self.building.take()
            && let Some(ns) = self.build_ns.get_mut(run as usize)
        {
            *ns = since.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        }
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

    /// The rejections since the last call: each is handed out once per
    /// life of the table, so a host that prints them prints each once per
    /// load and once more after each reload that leaves it standing.
    pub fn take_unreported(&mut self) -> &[(u32, &'static str)] {
        let from = std::mem::replace(&mut self.reported, self.rejected.len());
        &self.rejected[from.min(self.rejected.len())..]
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
        if let Some(p) = self.prior.get_mut(slot as usize) {
            *p = None;
        }
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
        for v in self.prior.iter().flatten() {
            mark(*v);
        }
    }
}

/// Whether two values are the same value to everything a program can ask:
/// the same kinds, numbers bit for bit (so `1` is not `1.0` and `0.0` is
/// not `-0.0`), strings by content, lists in order, records with the same
/// class and the same keys in the same order, and the same answer to
/// `frozen`. Stricter than `==`, which is what lets a reload keep the old
/// heap value of a rebuilt constant without anything being able to tell.
/// Anything a kept value cannot hold (a function, a cell, a handle) is
/// never identical to anything.
pub fn identical(a: Value, b: Value, heap: &Heap) -> bool {
    let frozen = |x: Value, y: Value| heap.is_frozen(x) == heap.is_frozen(y);
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::String(x), Value::String(y)) => x == y || heap.get_string(x) == heap.get_string(y),
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            ax.to_bits() == bx.to_bits() && ay.to_bits() == by.to_bits()
        }
        (Value::Vec3(x), Value::Vec3(y)) => {
            x == y || {
                let (p, q) = (heap.get_vec3(x), heap.get_vec3(y));
                p.iter().zip(&q).all(|(m, n)| m.to_bits() == n.to_bits())
            }
        }
        (Value::Dual { value: av, derivative: ad }, Value::Dual { value: bv, derivative: bd }) => {
            av.to_bits() == bv.to_bits() && ad.to_bits() == bd.to_bits()
        }
        (Value::List(x), Value::List(y)) => {
            if x == y {
                return true;
            }
            let (xs, ys) = (heap.get_list(x), heap.get_list(y));
            frozen(a, b)
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(p, q)| identical(*p, *q, heap))
        }
        (Value::F64Array(x), Value::F64Array(y)) => {
            if x == y {
                return true;
            }
            let (xs, ys) = (heap.get_f64_array(x), heap.get_f64_array(y));
            frozen(a, b)
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(p, q)| p.to_bits() == q.to_bits())
        }
        (Value::Map(x), Value::Map(y)) => {
            if x == y {
                return true;
            }
            if !frozen(a, b) || heap.map_class_name(x) != heap.map_class_name(y) {
                return false;
            }
            let (xs, ys) = (heap.get_map(x), heap.get_map(y));
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|((k, p), (l, q))| k == l && identical(*p, *q, heap))
        }
        (Value::EnumVariant { tag: at, data: ad }, Value::EnumVariant { tag: bt, data: bd }) => {
            (at == bt || heap.get_string(at) == heap.get_string(bt))
                && identical(Value::List(ad), Value::List(bd), heap)
        }
        (Value::Element(x), Value::Element(y)) => {
            x == y
                || (heap.get_string(heap.get_element_tag(x)) == heap.get_string(heap.get_element_tag(y))
                    && identical(
                        Value::Map(heap.get_element_props(x)),
                        Value::Map(heap.get_element_props(y)),
                        heap,
                    )
                    && identical(
                        Value::List(heap.get_element_children(x)),
                        Value::List(heap.get_element_children(y)),
                        heap,
                    ))
        }
        _ => false,
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
            // a variant's record into the stack's enum table, which keeps it
            // for as long as the stack keeps its static values (both are
            // emptied by a reload, and a table rebuilt for another lowering
            // meets another layout id): a skipped publication leaves the
            // record of the run that was not skipped where it is. The record
            // gets a slot of its own (see `apply`), so the run is skipped
            // only while the kept record is the published one.
            natives.constness_of_name(name) != NativeConstness::Dynamic
                || name == crate::program::ENUM_ALL_BUILTIN
                || name == crate::program::ENUM_FROM_BUILTIN
                || name == crate::program::ENUM_DATA_BUILTIN
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
    apply_runs(bc, program, analysis, natives, plan);
    follow(&mut bc.statics, program, analysis);
}

/// Record what a reload needs to follow an edit through this lowering: the
/// program's declarations, and for each slot what it is matched by and what
/// it depends on (see the module docs, "Following the diff").
fn follow(layout: &mut StaticLayout, program: &Program, analysis: &StaticAnalysis) {
    let decls: Vec<FollowDecl> = analysis
        .report
        .declarations
        .iter()
        .enumerate()
        .map(|(i, d)| FollowDecl {
            name: d.qualified.clone(),
            file: d.file_id,
            defs: d.defs.clone(),
            state: (d.kind == crate::static_classify::DeclKind::State)
                .then(|| program.get_term(d.term).state_key)
                .flatten()
                .map(|key| {
                    let mut deps = analysis.root_dependencies(program, d.term).unwrap_or_default();
                    deps.retain(|x| *x != i as u32);
                    (key, deps.into())
                }),
        })
        .collect();
    let canonical: HashMap<TermId, &crate::static_classify::StaticDecl> =
        analysis.report.declarations.iter().map(|d| (d.term, d)).collect();
    let parents = crate::static_audit::block_parents(program);
    for slot in &mut layout.slots {
        // Everything any writer of the register depends on. One writer the
        // classifier knows nothing about makes the whole set unknown.
        let mut deps: Option<BTreeSet<u32>> = Some(BTreeSet::new());
        if slot.origins.is_empty() {
            deps = None;
        }
        for &t in &slot.origins {
            let of = crate::static_audit::root_statement(program, &parents, t)
                .and_then(|root| analysis.root_dependencies(program, root));
            match (of, deps.as_mut()) {
                (Some(of), Some(all)) => all.extend(of),
                _ => deps = None,
            }
        }
        slot.deps = deps.map(|d| d.into_iter().collect());
        slot.key = slot
            .term
            .and_then(|t| canonical.get(&t))
            .filter(|d| d.kind != crate::static_classify::DeclKind::Enum && !slot.enum_data)
            .map(|d| d.qualified.clone());
    }
    unnamed_keys(layout, program);
    // A key two slots share names neither.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for slot in &layout.slots {
        if let Some(k) = &slot.key {
            *seen.entry(k.clone()).or_default() += 1;
        }
    }
    for slot in &mut layout.slots {
        if slot.key.as_ref().is_some_and(|k| seen.get(k).copied().unwrap_or(0) > 1) {
            slot.key = None;
        }
    }
    layout.follow = Arc::new(FollowInfo { decls });
}

/// Keys for the slots that hold a value with no declaration of its own
/// (the `[1, 2, 3]` written inside a call, the `len(TILES)` hoisted out of
/// one): what a reload matches them by across two lowerings.
///
/// Such a value is written inside exactly one top-level statement. The key
/// is that statement's text, the shape the statement compiled to (the
/// operation, position and name of each of its top-level terms), and where
/// in that sequence the value's term is. Two programs that give the same
/// key compiled the same text to the same terms, so the term is the same
/// expression; whether what it refers to is unchanged is the dependency
/// set's question, as for a declaration. A statement written twice gives
/// its values the same keys, and a key two slots share is dropped.
fn unnamed_keys(layout: &mut StaticLayout, program: &Program) {
    use std::hash::{Hash, Hasher};
    if layout.slots.iter().all(|s| s.key.is_some() || s.enum_data) {
        return;
    }
    let Some(root) = program.block_terms.get(&program.root_block) else { return };
    // Every positioned term of the root block, in block order.
    struct At {
        term: TermId,
        file: u16,
        start: u32,
        end: u32,
    }
    let placed: Vec<At> = root
        .iter()
        .filter_map(|t| {
            let span = program.source_map.get(*t).filter(|s| s.start.line > 0)?;
            Some(At {
                term: *t,
                file: span.file.0,
                start: span.start.offset,
                end: span.end.offset.max(span.start.offset),
            })
        })
        .collect();
    // The statements: the spans no other span contains.
    let mut order: Vec<usize> = (0..placed.len()).collect();
    order.sort_by_key(|&i| (placed[i].file, placed[i].start, std::cmp::Reverse(placed[i].end)));
    let mut statements: Vec<(u16, u32, u32)> = Vec::new();
    for &i in &order {
        let a = &placed[i];
        match statements.last() {
            Some(&(file, _, end)) if file == a.file && a.end <= end => {}
            _ => statements.push((a.file, a.start, a.end)),
        }
    }
    let statement_of = |file: u16, start: u32| -> Option<usize> {
        let at = statements.partition_point(|s| (s.0, s.1) <= (file, start));
        let i = at.checked_sub(1)?;
        let (f, s, e) = statements[i];
        (f == file && s <= start && start < e.max(s + 1)).then_some(i)
    };
    // The terms of each statement that holds an unnamed slot.
    let position: HashMap<TermId, usize> =
        placed.iter().enumerate().map(|(i, a)| (a.term, i)).collect();
    let mut wanted: HashMap<usize, Vec<usize>> = HashMap::new();
    let slot_at = |slot: &StaticSlotInfo| -> Option<(usize, usize)> {
        if slot.key.is_some() || slot.enum_data {
            return None;
        }
        let [t] = slot.origins[..] else { return None };
        let i = *position.get(&t)?;
        Some((statement_of(placed[i].file, placed[i].start)?, i))
    };
    for slot in &layout.slots {
        if let Some((st, _)) = slot_at(slot) {
            wanted.entry(st).or_default();
        }
    }
    if wanted.is_empty() {
        return;
    }
    for (i, a) in placed.iter().enumerate() {
        if let Some(st) = statement_of(a.file, a.start)
            && let Some(terms) = wanted.get_mut(&st)
        {
            terms.push(i);
        }
    }
    let mut texts: HashMap<u16, Vec<char>> = HashMap::new();
    let mut prefix: HashMap<usize, String> = HashMap::new();
    for (&st, terms) in &wanted {
        let (file, start, end) = statements[st];
        let chars = texts.entry(file).or_insert_with(|| {
            match program.source_map.files.get(file as usize) {
                Some(f) => f.source.chars().collect(),
                None => program.source.chars().collect(),
            }
        });
        let Some(text) = chars.get(start as usize..end as usize) else { continue };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        terms.len().hash(&mut h);
        for &i in terms {
            let term = program.get_term(placed[i].term);
            std::mem::discriminant(&term.op).hash(&mut h);
            (placed[i].start - start, placed[i].end - start).hash(&mut h);
            term.name.hash(&mut h);
            term.inputs.len().hash(&mut h);
        }
        let name = program
            .source_map
            .files
            .get(file as usize)
            .map(|f| f.name.as_str())
            .unwrap_or("");
        prefix.insert(st, format!("\u{0}{name}\u{0}{:016x}", h.finish()));
    }
    let keys: Vec<Option<String>> = layout
        .slots
        .iter()
        .map(|slot| {
            let (st, i) = slot_at(slot)?;
            let at = wanted.get(&st)?.iter().position(|x| *x == i)?;
            Some(format!("{}\u{0}{at}", prefix.get(&st)?))
        })
        .collect();
    for (slot, key) in layout.slots.iter_mut().zip(keys) {
        if key.is_some() {
            slot.key = key;
        }
    }
}

fn apply_runs(
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

    // The record each `__enum_data` call publishes, and the variant it is
    // the data of. A run that publishes one keeps the record in a slot,
    // whoever else reads it: that is what marks it immutable, what refuses
    // a record that cannot be kept, and what makes the run something a
    // frame can skip at all (nothing reads the call's own result).
    let mut published: HashMap<usize, Reg> = HashMap::new();
    let mut published_name: HashMap<Reg, String> = HashMap::new();
    for (ip, inst) in bc.root.code.iter().enumerate() {
        let Inst::BuiltinCall { name, args, .. } = inst else { continue };
        if program.get_string_constant(*name) != Some(crate::program::ENUM_DATA_BUILTIN) {
            continue;
        }
        let Some(&record) = args.get(1) else { continue };
        published.insert(ip, record);
        let tag = origin(ip)
            .and_then(|t| program.get_term(t).inputs.first().copied())
            .and_then(|t| match program.get_term(t).op {
                TermOp::Constant(c) => program.get_string_constant(c),
                _ => None,
            });
        if let Some(tag) = tag {
            published_name.insert(record, format!("enum data of {tag}"));
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
        let records: HashSet<Reg> = (start..end).filter_map(|ip| published.get(&ip).copied()).collect();
        // A record this run publishes but does not build cannot be kept by
        // it; the publication then runs every time.
        let publishes_its_own = records.iter().all(|r| written.binary_search(r).is_ok());
        for &r in &written {
            let mut kept = bc.root.result_reg == Some(r) || records.contains(&r);
            let mut read_outside = kept;
            let mut read_by: Vec<usize> = Vec::new();
            for &ip in readers.get(&r).map(|v| &v[..]).unwrap_or(&[]) {
                if (start..end).contains(&ip) {
                    continue;
                }
                read_outside = true;
                match run_of[ip] {
                    Some(j) if j > k && runs[j].skippable => read_by.push(j),
                    _ => kept = true,
                }
            }
            if kept {
                // A register with a slot is loaded when this run is
                // skipped, so a later run that reads it needs nothing more
                // of this one: it can be rebuilt alone.
                own.push(r);
            } else {
                deps.extend(read_by);
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
        run.skippable = storable && publishes_its_own && feeds > 0 && run.weight >= MIN_WEIGHT;
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
    let mut writer_terms: HashMap<Reg, Vec<TermId>> = HashMap::new();
    // A register some instruction with no term behind it writes: nothing
    // says what that value depends on.
    let mut unattributed: HashSet<Reg> = HashSet::new();
    for ip in 0..n {
        match origin(ip) {
            Some(t) => {
                for r in &writes[ip] {
                    writer_term.insert(*r, t);
                    let all = writer_terms.entry(*r).or_default();
                    if !all.contains(&t) {
                        all.push(t);
                    }
                }
            }
            None => unattributed.extend(writes[ip].iter().copied()),
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
                name: published_name.get(r).cloned().or_else(|| {
                    term.and_then(|t| {
                        decl_names
                            .get(&t)
                            .map(|s| s.to_string())
                            .or_else(|| program.get_term(t).name.clone())
                    })
                }),
                at: term.and_then(|t| crate::static_audit::position(program, t)),
                term,
                is_const: term.is_some_and(|t| program.get_term(t).is_const),
                key: None,
                deps: None,
                origins: if unattributed.contains(r) {
                    Vec::new()
                } else {
                    writer_terms.get(r).cloned().unwrap_or_default()
                },
                enum_data: published_name.contains_key(r),
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
