//! Memoized scopes: a user-function call whose inputs are what they were the
//! last time it ran is skipped, and its result, output and state effects are
//! replayed instead.
//!
//! The frame gate ([`crate::run_deps`]) skips a whole frame whose inputs did
//! not move. This is the same idea one level down. Every call of a user
//! function through a `Call` instruction is a **scope**, addressed by the call
//! path the frame runs on (the same `[Call(site) | Index(i)]` chain that keys
//! `state`, see docs/dev/state-call-paths.md). While a scope runs, the VM
//! records what it depended on and what it did:
//!
//! - the arguments and captured values it was entered with;
//! - every host binding it read, as a **probe**: the native that read it, its
//!   arguments and its result — re-evaluated at validation time, so a pointer
//!   that moved but did not change `hovered(r)`'s answer does not invalidate
//!   the scope (Salsa-style early cutoff on the probe);
//! - every `state` slot and `var` cell it read, with the value it saw, and
//!   every one it wrote, with the value it wrote;
//! - the child scopes it called, by path and record serial;
//! - the output it emitted (draw commands, events), as a segment per buffer;
//! - the observations it recorded, so a host reading `panel.values` sees a
//!   skipped scope's bindings exactly as a run would have left them.
//!
//! On the next entry with equal arguments and captures, the record is
//! validated in order — a read against the live value, a probe by re-running
//! the native, a child by validating *its* record recursively and, if that
//! fails, re-executing the child alone and comparing what it produced with
//! what it produced before (a child that comes back equal is a cutoff: the
//! parent stays valid). A valid scope is **replayed**: the cached output is
//! spliced back into the buffers, the writes are re-applied in order, the
//! state keys it touched are kept alive through the sweep, and the cached
//! result is returned without pushing a frame.
//!
//! What cannot be replayed is not recorded: a scope that printed, consumed
//! randomness, created a resource, called a handle method, produced a
//! `Pending`, or let a cell it created escape through its result or a write is
//! *effectful*, and so is every scope enclosing it. Trivial scopes — no
//! dependencies, no output, a handful of instructions — are not worth a
//! record and are folded into their parent.
//!
//! The VM-side half (recording hooks, validation, replay, re-execution) is
//! `backend::bytecode::vm::memo`; this module holds the data model, the table
//! and the value comparisons, which need no VM.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use smallvec::SmallVec;

use crate::closure_table::ClosureTable;
use crate::execution_context::EmitSite;
use crate::heap::{CellId, Heap};
use crate::native_fn::NativeFnId;
use crate::program::{ClosureId, FunctionId, TermId};
use crate::stack::{PathPart, RuntimeStateKey, StateTouches, TouchCapture};
use crate::symbol::SymbolId;
use crate::value::Value;

/// A scope's address: the call path of the frame that ran it. The same shape
/// as a `VmFrame::path`.
pub type ScopePath = SmallVec<[PathPart; 4]>;

/// The arguments or captures a scope was entered with. Inline for the arities
/// widgets have, so opening a scope allocates nothing.
pub type ScopeValues = SmallVec<[Value; 6]>;

/// The hasher for the memo's own maps: scope paths (a few words of already
/// well-mixed callsite hashes and loop indices) and closure-id pairs, looked
/// up a dozen times per scope. The default SipHash was the single largest
/// cost of a replay; this is the rustc `FxHasher` recurrence (multiply-rotate
/// per word), which the profile shows as noise. Never used for a content
/// fingerprint, where a collision would mean a stale replay.
#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.write_u64(n as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

/// A scope whose subtree retired fewer instructions than this, with nothing
/// recorded and nothing emitted, is folded into its parent rather than given
/// a record: validating it would cost about what running it does.
pub const MIN_SCOPE_INSTS: u64 = 32;

/// Records kept at most. Past this the table stops recording new scopes for
/// the run (existing ones still validate and replay), so a deep recursion or
/// a pathological call graph bounds its own memory.
pub const MAX_SLOTS: usize = 200_000;

/// Node budget for one argument comparison. A row record, a style record, a
/// short list of points compare in full; a large structure passed by a fresh
/// allocation every frame is deemed different, which costs a re-run.
pub const ARG_COMPARE_BUDGET: usize = 512;

/// Node budget for comparing a re-executed child's outputs with the cached
/// ones, per emitted value, on top of a fixed allowance.
pub const OUTPUT_COMPARE_PER_VALUE: usize = 24;

/// How many of a call site's records may be thrown away — evicted by a
/// sweep, or replaced after a failed validation — without any of them ever
/// being replayed, before the site stops being recorded at all. A recompute
/// that runs once per edit (a formula tokenizer behind a revision check)
/// records thousands of scopes that the next run evicts unused, and a scope
/// whose arguments differ every run re-records every run; both pay for
/// records that never come back as a replay.
pub const COLD_AFTER_UNHIT_EVICTIONS: u32 = 3;

/// A scope with more dependency entries than this is not recorded:
/// validating it would cost about what running it does, and its record
/// would be most of the memory the run touched.
pub const MAX_SCOPE_DEPS: usize = 8192;

/// One thing a scope depended on or did, in execution order.
#[derive(Debug, Clone)]
pub enum Dep {
    /// A native read a host binding and answered `result` for `args`.
    /// Validated by calling it again: an unchanged answer keeps the scope
    /// valid even when the binding moved.
    Probe {
        native: NativeFnId,
        args: SmallVec<[Value; 4]>,
        /// Content fingerprint of any container arguments, so a container the
        /// scope later mutated in place cannot pass as the argument the native
        /// actually saw.
        args_fp: u64,
        result: Value,
    },
    /// A `state` slot was read (or initialized) and held `value` — `None`
    /// for a read of an absent slot.
    StateRead {
        key: RuntimeStateKey,
        value: Option<Value>,
    },
    /// A `state` slot was written.
    StateWrite { key: RuntimeStateKey, value: Value },
    /// A `var` cell created outside the scope was read and held `value`.
    CellRead { cell: CellId, value: Value },
    /// A `var` cell created outside the scope was written.
    CellWrite { cell: CellId, value: Value },
    /// A native consulted host data outside the binding table; valid while the
    /// host has not reported that data changed since the record was made.
    HostRead,
    /// A native consulted the resource table; valid while its revision is
    /// what it was when the record was made.
    ResourcesRead,
    /// A child scope ran (or was replayed) here. `serial` names the record
    /// the parent saw; a different serial in the table means the child was
    /// re-recorded by some other call and the parent cannot trust it.
    Child { path: ScopePath, serial: u64 },
    /// A named term was observed with `value` (only while observation is
    /// on). Kept in order with the children so a replay reports the same
    /// last value per term a run would: not a dependency, an effect.
    Observed { term: TermId, value: Value },
}

/// The values a scope appended to one output buffer.
#[derive(Debug, Clone)]
pub struct OutputSegment {
    pub sym: SymbolId,
    pub values: Vec<Value>,
    /// Emit attribution for each value, when the context was tracing emits.
    pub origins: Option<Vec<EmitSite>>,
}

/// The record of a scope's last execution.
#[derive(Debug, Clone)]
pub struct MemoSlot {
    /// Bumped every time the slot is (re)recorded.
    pub serial: u64,
    /// The run this slot was last recorded, validated or replayed in. Slots
    /// not visited by a run are evicted when it completes.
    pub visited: u64,
    pub fn_id: FunctionId,
    /// The callsite hash this scope was entered through, with `fn_id` the key
    /// of the cold-site table (see [`MemoTable::site_records`]).
    pub site: u64,
    /// Whether this record was ever replayed. A record evicted without a
    /// single hit is what makes its site cold.
    pub hit: bool,
    pub captures: ScopeValues,
    pub args: ScopeValues,
    pub result: Value,
    pub deps: Vec<Dep>,
    pub outputs: Vec<OutputSegment>,
    /// Every state key the scope's subtree touched.
    pub touches: StateTouches,
    /// Host-data revision when recorded, for a `HostRead` dep.
    pub host_revision: u64,
    /// Resource-table revision when recorded, for a `ResourcesRead` dep.
    pub resources_revision: u64,
    /// The scope's subtree wrote no `state` slot and no cell. Only a pure
    /// scope is re-executed during a parent's validation: a re-execution's
    /// writes would land before the parent's own re-run applied them again.
    pub pure: bool,
}

/// A scope that is executing right now.
#[derive(Debug)]
pub struct OpenScope {
    pub path: ScopePath,
    /// `vm_frames.len()` with the scope's frame on top.
    pub depth: usize,
    pub fn_id: FunctionId,
    /// The callsite the scope was entered through (see [`MemoSlot::site`]).
    pub site: u64,
    pub captures: ScopeValues,
    pub args: ScopeValues,
    pub deps: Vec<Dep>,
    /// Output buffer lengths at entry, so the scope's own segment is the tail.
    pub out_start: SmallVec<[(SymbolId, usize); 2]>,
    pub insts_at_entry: u64,
    pub rng_at_entry: u64,
    /// Something happened that a replay could not reproduce.
    pub effectful: bool,
    /// Cells created inside this scope. A read of one is not a dependency; a
    /// result or a write carrying one makes the scope effectful.
    pub local_cells: HashSet<CellId>,
    /// The touch capture bracketing the scope; taken when it closes.
    pub capture: Option<TouchCapture>,
    /// Re-execution during validation: on close, compare with `previous` and
    /// report the verdict instead of recording a `Child` dep in a parent.
    pub previous: Option<Box<PreviousRecord>>,
}

/// What a scope produced the last time, for the cutoff comparison after a
/// re-execution.
#[derive(Debug)]
pub struct PreviousRecord {
    pub result: Value,
    pub deps: Vec<Dep>,
    pub outputs: Vec<OutputSegment>,
    pub touches: StateTouches,
}

/// Counters for `--memo-stats` and the tests. Cumulative over the table's
/// life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoStats {
    /// Scopes replayed instead of run.
    pub hits: u64,
    /// Scopes that had a record but failed validation (arguments, a read, a
    /// probe or a child differed) and ran.
    pub misses: u64,
    /// Scopes recorded (first run, or run after a miss).
    pub records: u64,
    /// Scopes folded into their parent as not worth a record.
    pub inlined: u64,
    /// Scopes that could not be recorded because they did something a replay
    /// cannot reproduce.
    pub effectful: u64,
    /// Children re-executed alone during a parent's validation.
    pub reexecs: u64,
    /// Re-executions that produced what they produced before, so the parent
    /// stayed valid.
    pub cutoffs: u64,
    /// Records evicted for not being visited by a run.
    pub evicted: u64,
    /// Scopes not recorded because their call site went cold — its records
    /// kept being evicted without ever being replayed.
    pub cold: u64,
}

/// What a call site's records have been worth, for [`MemoTable::site_records`].
#[derive(Debug, Default, Clone)]
struct ColdSite {
    /// Records from this site thrown away without a replay, evicted or
    /// replaced, since its last hit.
    unhit_evictions: u32,
    /// The run a cold site was last allowed one record, so a site that
    /// becomes productive again can be noticed.
    probed_run: u64,
}

/// One stack's memo table: the records, the scopes open right now, and the
/// per-run bookkeeping.
#[derive(Debug, Clone, Default)]
pub struct MemoTable {
    slots: FastMap<ScopePath, MemoSlot>,
    pub open: Vec<OpenScope>,
    run: u64,
    serial: u64,
    pub stats: MemoStats,
    /// Set while a probe is re-evaluated during validation, so the native's
    /// reads are not recorded into whatever scope happens to be open.
    pub suppress: bool,
    /// A re-execution failed: nothing more is recorded or replayed this run.
    pub poisoned: bool,
    /// The verdict of the most recent re-execution: `Some(true)` if it
    /// produced something different from its previous record.
    pub last_reexec_changed: Option<bool>,
    /// Per-callsite record of what memoizing there has been worth.
    cold: FastMap<(FunctionId, u64), ColdSite>,
    /// Structural-equality answers for pairs of closures compared this run.
    /// Top-level functions are re-created every run and capture one another,
    /// so the same pairs come up for every scope that takes a callback.
    eq_cache: FastMap<(ClosureId, ClosureId), bool>,
}

impl Clone for OpenScope {
    /// A stack is cloned when it is forked, which never happens mid-run, so
    /// there is nothing open to clone; a capture cannot be duplicated anyway.
    fn clone(&self) -> Self {
        panic!("an open memo scope cannot be cloned")
    }
}

impl MemoTable {
    /// Start a run: nothing is open, and the run stamp advances so eviction
    /// can tell what this run visited.
    pub fn begin_run(&mut self) {
        self.open.clear();
        self.run = self.run.wrapping_add(1);
        self.suppress = false;
        self.poisoned = false;
        self.last_reexec_changed = None;
        self.eq_cache.clear();
    }

    /// The current run stamp.
    pub fn run(&self) -> u64 {
        self.run
    }

    /// Drop every record a completed run did not visit — the same rule the
    /// state sweep applies: a branch not taken this frame loses its records
    /// and re-records when it is taken again.
    pub fn sweep(&mut self) -> usize {
        let run = self.run;
        let before = self.slots.len();
        let mut unhit: Vec<(FunctionId, u64)> = Vec::new();
        self.slots.retain(|_, s| {
            let keep = s.visited == run;
            if !keep && !s.hit {
                unhit.push((s.fn_id, s.site));
            }
            keep
        });
        // A site whose records die unused is charged once per record: a
        // recompute that makes thousands of them goes cold on its first run,
        // while a widget that misses one frame in three does not.
        for key in unhit {
            self.cold.entry(key).or_default().unhit_evictions += 1;
        }
        let evicted = before - self.slots.len();
        self.stats.evicted += evicted as u64;
        evicted
    }

    /// Whether a call of `fn_id` through callsite `site` should be opened as
    /// a recording scope. False for a **cold** site: one whose records have
    /// been evicted unreplayed often enough that recording them is a cost
    /// with no return. Such a call still runs normally — its reads land in
    /// the enclosing scope, exactly as a scope folded into its parent does.
    ///
    /// A cold site is let through once per run, so a site that becomes
    /// productive (the recompute that now happens every frame, a branch that
    /// started being taken) records again and clears its coldness on the
    /// first replay.
    pub fn site_records(&mut self, fn_id: FunctionId, site: u64) -> bool {
        let run = self.run;
        match self.cold.get_mut(&(fn_id, site)) {
            None => true,
            Some(c) if c.unhit_evictions < COLD_AFTER_UNHIT_EVICTIONS => true,
            Some(c) if c.probed_run != run => {
                c.probed_run = run;
                true
            }
            Some(_) => false,
        }
    }

    /// Note that the record at `path` was replayed: the site is productive,
    /// so it is no longer cold.
    pub fn note_hit(&mut self, path: &ScopePath) {
        if let Some(s) = self.slots.get_mut(path) {
            s.hit = true;
            let key = (s.fn_id, s.site);
            self.cold.remove(&key);
        }
    }

    /// Forget everything: the program changed under the records.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.open.clear();
        self.eq_cache.clear();
        self.cold.clear();
    }

    pub fn get(&self, path: &ScopePath) -> Option<&MemoSlot> {
        self.slots.get(path)
    }

    pub fn get_mut(&mut self, path: &ScopePath) -> Option<&mut MemoSlot> {
        self.slots.get_mut(path)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The next record serial.
    pub fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// Whether the table has room for another record.
    pub fn has_room(&self) -> bool {
        self.slots.len() < MAX_SLOTS
    }

    /// Store a record, replacing any at the same path. A record replaced
    /// without ever having been replayed charges its site the same way an
    /// eviction does: a scope that misses validation every run is paying for
    /// records it never gets anything back from.
    pub fn insert(&mut self, path: ScopePath, slot: MemoSlot) {
        let key = (slot.fn_id, slot.site);
        if let Some(old) = self.slots.insert(path, slot)
            && !old.hit
        {
            self.cold.entry(key).or_default().unhit_evictions += 1;
        }
    }

    /// Remove and return a record.
    pub fn take(&mut self, path: &ScopePath) -> Option<MemoSlot> {
        self.slots.remove(path)
    }

    /// Mark a slot visited this run.
    pub fn visit(&mut self, path: &ScopePath) {
        let run = self.run;
        if let Some(s) = self.slots.get_mut(path) {
            s.visited = run;
        }
    }

    /// Whether recording hooks should do anything right now: some scope is
    /// open, and nothing is suppressing records.
    #[inline]
    pub fn recording(&self) -> bool {
        !self.open.is_empty() && !self.suppress && !self.poisoned
    }

    /// The innermost open scope.
    #[inline]
    pub fn innermost(&mut self) -> Option<&mut OpenScope> {
        self.open.last_mut()
    }

    /// Mark the innermost open scope effectful.
    pub fn note_effect(&mut self) {
        if let Some(s) = self.open.last_mut() {
            s.effectful = true;
        }
    }

    /// Every heap value the table holds, for the collector. Records pin the
    /// values they compare against and replay; open scopes pin their
    /// arguments and what they have recorded so far.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for s in self.slots.values() {
            s.captures.iter().chain(&s.args).for_each(|v| mark(*v));
            mark(s.result);
            mark_deps(&s.deps, &mut mark);
            mark_outputs(&s.outputs, &mut mark);
        }
        for s in &self.open {
            s.captures.iter().chain(&s.args).for_each(|v| mark(*v));
            mark_deps(&s.deps, &mut mark);
            if let Some(p) = &s.previous {
                mark(p.result);
                mark_deps(&p.deps, &mut mark);
                mark_outputs(&p.outputs, &mut mark);
            }
        }
    }

    /// Structural equality of two values for the memo's purposes, with this
    /// run's closure-pair cache. See [`ValueEq`].
    pub fn values_equal(
        &mut self,
        a: &Value,
        b: &Value,
        heap: &Heap,
        closures: &ClosureTable,
        budget: usize,
    ) -> bool {
        ValueEq::new(heap, closures, &mut self.eq_cache, budget).eq(a, b)
    }
}

/// [`ValueEq`] for a one-off comparison, outside a memo table and its
/// closure-pair cache (the frame gate's "did this `state` write change the
/// slot").
pub fn content_equal(
    a: &Value,
    b: &Value,
    heap: &Heap,
    closures: &ClosureTable,
    budget: usize,
) -> bool {
    let mut cache = FastMap::default();
    ValueEq::new(heap, closures, &mut cache, budget).eq(a, b)
}

/// Mark every heap value a record's dependencies hold (see
/// [`MemoTable::gc_roots`]).
fn mark_deps(deps: &[Dep], mark: &mut impl FnMut(Value)) {
    for d in deps {
        match d {
            Dep::Probe { args, result, .. } => {
                args.iter().for_each(|v| mark(*v));
                mark(*result);
            }
            Dep::StateRead { value, .. } => value.iter().for_each(|v| mark(*v)),
            Dep::StateWrite { value, .. }
            | Dep::CellRead { value, .. }
            | Dep::CellWrite { value, .. }
            | Dep::Observed { value, .. } => mark(*value),
            Dep::HostRead | Dep::ResourcesRead | Dep::Child { .. } => {}
        }
    }
}

fn mark_outputs(outputs: &[OutputSegment], mark: &mut impl FnMut(Value)) {
    outputs
        .iter()
        .flat_map(|seg| &seg.values)
        .for_each(|v| mark(*v));
}

/// Structural equality as the memo needs it. Unlike the language's `==` it
/// compares records by content and closures by function and captures, so a
/// callback re-created this frame from the same code with the same captured
/// values counts as the same callback. Cells compare by identity: two boxes
/// are the same input only if a write through one is visible through the
/// other. It gives up (answers "different", the safe answer) past its node
/// budget.
pub struct ValueEq<'a> {
    heap: &'a Heap,
    closures: &'a ClosureTable,
    cache: &'a mut FastMap<(ClosureId, ClosureId), bool>,
    budget: usize,
    /// Closure pairs on the current comparison path; a pair met again is
    /// assumed equal, which is what makes mutually-capturing functions
    /// comparable.
    visiting: Vec<(ClosureId, ClosureId)>,
}

impl<'a> ValueEq<'a> {
    fn new(
        heap: &'a Heap,
        closures: &'a ClosureTable,
        cache: &'a mut FastMap<(ClosureId, ClosureId), bool>,
        budget: usize,
    ) -> Self {
        ValueEq {
            heap,
            closures,
            cache,
            budget,
            visiting: Vec::new(),
        }
    }

    pub fn eq(&mut self, a: &Value, b: &Value) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let heap = self.heap;
        match (a, b) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            (Value::String(a), Value::String(b)) => {
                a == b || heap.get_string(*a) == heap.get_string(*b)
            }
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
                ax.to_bits() == bx.to_bits() && ay.to_bits() == by.to_bits()
            }
            (
                Value::Dual {
                    value: av,
                    derivative: ad,
                },
                Value::Dual {
                    value: bv,
                    derivative: bd,
                },
            ) => av.to_bits() == bv.to_bits() && ad.to_bits() == bd.to_bits(),
            (Value::List(a), Value::List(b)) => {
                if a == b {
                    return true;
                }
                let (xs, ys) = (heap.get_list(*a), heap.get_list(*b));
                xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| self.eq(x, y))
            }
            (Value::Map(a), Value::Map(b)) => {
                if a == b {
                    return true;
                }
                if heap.map_class_name(*a) != heap.map_class_name(*b) {
                    return false;
                }
                let (xs, ys) = (heap.get_map(*a), heap.get_map(*b));
                xs.len() == ys.len()
                    && xs.iter().all(|(k, x)| match ys.get(k) {
                        Some(y) => self.eq(x, y),
                        None => false,
                    })
            }
            (
                Value::EnumVariant { tag: at, data: ad },
                Value::EnumVariant { tag: bt, data: bd },
            ) => {
                (at == bt || heap.get_string(*at) == heap.get_string(*bt))
                    && self.eq(&Value::List(*ad), &Value::List(*bd))
            }
            (Value::F64Array(a), Value::F64Array(b)) => {
                a == b || heap.get_f64_array(*a) == heap.get_f64_array(*b)
            }
            (Value::Element(a), Value::Element(b)) => {
                a == b
                    || (heap.get_string(heap.get_element_tag(*a))
                        == heap.get_string(heap.get_element_tag(*b))
                        && self.eq(
                            &Value::Map(heap.get_element_props(*a)),
                            &Value::Map(heap.get_element_props(*b)),
                        )
                        && self.eq(
                            &Value::List(heap.get_element_children(*a)),
                            &Value::List(heap.get_element_children(*b)),
                        ))
            }
            (Value::Cell(a), Value::Cell(b)) => a == b,
            (Value::Closure(a), Value::Closure(b)) => self.closures_eq(*a, *b),
            (Value::OverloadSet(a), Value::OverloadSet(b)) => {
                if a == b {
                    return true;
                }
                if !self.closures.is_set_live(*a) || !self.closures.is_set_live(*b) {
                    return false;
                }
                let (xs, ys) = (self.closures.set(*a), self.closures.set(*b));
                xs.len() == ys.len()
                    && xs.iter().zip(ys).all(|(x, y)| {
                        x.arity == y.arity && self.closures_eq(x.closure_id, y.closure_id)
                    })
            }
            (Value::NativeFunction(a), Value::NativeFunction(b)) => a == b,
            (Value::Handle(a), Value::Handle(b)) => a == b,
            // A pending resource is a promise of a value that may since have
            // arrived; never equal, so nothing is replayed over it.
            (Value::Pending(_), Value::Pending(_)) => false,
            _ => false,
        }
    }

    fn closures_eq(&mut self, a: ClosureId, b: ClosureId) -> bool {
        if a == b {
            return true;
        }
        if let Some(&known) = self.cache.get(&(a, b)) {
            return known;
        }
        if self.visiting.contains(&(a, b)) {
            return true;
        }
        if !self.closures.is_closure_live(a) || !self.closures.is_closure_live(b) {
            return false;
        }
        let (ca, cb) = (self.closures.closure(a), self.closures.closure(b));
        if ca.function_id != cb.function_id || ca.captures.len() != cb.captures.len() {
            self.cache.insert((a, b), false);
            return false;
        }
        self.visiting.push((a, b));
        let caps_a = ca.captures.clone();
        let caps_b = cb.captures.clone();
        let equal = caps_a.iter().zip(&caps_b).all(|(x, y)| self.eq(x, y));
        self.visiting.pop();
        // Only a definite answer is cached: a comparison that ran out of
        // budget says nothing about the pair.
        if equal || self.budget > 0 {
            self.cache.insert((a, b), equal);
        }
        equal
    }
}

/// Whether `v` holds (directly, in a container, or through a closure's
/// captures) a cell in `locals`. A scope whose result or a write carries one of
/// its own cells has let mutable state escape, and cannot be replayed: the
/// cached cell would carry writes made after the scope returned.
pub fn holds_local_cell(
    v: &Value,
    heap: &Heap,
    closures: &ClosureTable,
    locals: &HashSet<CellId>,
) -> bool {
    let mut budget = LOCAL_CELL_SCAN_BUDGET;
    !locals.is_empty() && scan_for_local_cell(v, heap, closures, locals, &mut budget)
}

/// Node budget for one [`holds_local_cell`] scan; past it the scan assumes
/// the worst.
const LOCAL_CELL_SCAN_BUDGET: usize = 256;

fn scan_for_local_cell(
    v: &Value,
    heap: &Heap,
    closures: &ClosureTable,
    locals: &HashSet<CellId>,
    budget: &mut usize,
) -> bool {
    if *budget == 0 {
        // Out of budget: assume the worst.
        return true;
    }
    *budget -= 1;
    match v {
        Value::Cell(id) => locals.contains(id),
        Value::List(id) => heap
            .get_list(*id)
            .iter()
            .any(|e| scan_for_local_cell(e, heap, closures, locals, budget)),
        Value::Map(id) => heap
            .get_map(*id)
            .values()
            .any(|e| scan_for_local_cell(e, heap, closures, locals, budget)),
        Value::EnumVariant { data, .. } => {
            scan_for_local_cell(&Value::List(*data), heap, closures, locals, budget)
        }
        Value::Element(id) => {
            scan_for_local_cell(
                &Value::Map(heap.get_element_props(*id)),
                heap,
                closures,
                locals,
                budget,
            ) || scan_for_local_cell(
                &Value::List(heap.get_element_children(*id)),
                heap,
                closures,
                locals,
                budget,
            )
        }
        Value::Closure(id) => {
            closures.is_closure_live(*id)
                && closures
                    .closure(*id)
                    .captures
                    .iter()
                    .any(|c| scan_for_local_cell(c, heap, closures, locals, budget))
        }
        Value::OverloadSet(id) => {
            closures.is_set_live(*id)
                && closures.set(*id).iter().any(|e| {
                    scan_for_local_cell(
                        &Value::Closure(e.closure_id),
                        heap,
                        closures,
                        locals,
                        budget,
                    )
                })
        }
        _ => false,
    }
}

/// Whether a value is a heap container a scope could mutate in place after
/// handing it to a native — the arguments a probe fingerprints.
pub fn is_container(v: &Value) -> bool {
    matches!(
        v,
        Value::List(_) | Value::Map(_) | Value::F64Array(_) | Value::Element(_)
    )
}

/// Fingerprint of a probe's container arguments (0 when there are none).
pub fn container_args_fingerprint(args: &[Value], heap: &Heap) -> u64 {
    let mut fp = 0u64;
    for a in args {
        if is_container(a) {
            fp = fp.rotate_left(7) ^ crate::run_deps::fingerprint_binding(Some(a), heap);
        }
    }
    fp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RuntimeClosure;

    #[test]
    fn records_compare_by_content_and_closures_by_structure() {
        let mut heap = Heap::new();
        let mut closures = ClosureTable::new();
        let mut table = MemoTable::default();

        let mut m1 = indexmap::IndexMap::new();
        m1.insert("x".to_string(), Value::Int(1));
        let a = Value::Map(heap.alloc_map(m1.clone()));
        let b = Value::Map(heap.alloc_map(m1.clone()));
        assert!(table.values_equal(&a, &b, &heap, &closures, ARG_COMPARE_BUDGET));

        let c1 = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(3),
            captures: vec![a],
        });
        let c2 = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(3),
            captures: vec![b],
        });
        let c3 = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(3),
            captures: vec![Value::Int(9)],
        });
        let (v1, v2, v3) = (Value::Closure(c1), Value::Closure(c2), Value::Closure(c3));
        assert!(table.values_equal(&v1, &v2, &heap, &closures, ARG_COMPARE_BUDGET));
        assert!(!table.values_equal(&v1, &v3, &heap, &closures, ARG_COMPARE_BUDGET));
        // Cached answers survive.
        assert!(table.values_equal(&v1, &v2, &heap, &closures, ARG_COMPARE_BUDGET));

        // A cell is an identity, not a value.
        let cell_a = Value::Cell(heap.alloc_cell(Value::Int(1)));
        let cell_b = Value::Cell(heap.alloc_cell(Value::Int(1)));
        assert!(!table.values_equal(&cell_a, &cell_b, &heap, &closures, ARG_COMPARE_BUDGET));
        assert!(table.values_equal(&cell_a, &cell_a, &heap, &closures, ARG_COMPARE_BUDGET));
    }

    #[test]
    fn comparison_gives_up_past_the_budget() {
        let mut heap = Heap::new();
        let closures = ClosureTable::new();
        let big: Vec<Value> = (0..300).map(Value::Int).collect();
        let a = Value::List(heap.alloc_list(big.clone()));
        let b = Value::List(heap.alloc_list(big));
        assert!(!content_equal(&a, &b, &heap, &closures, 256));
        // Same id short-circuits regardless of size.
        assert!(content_equal(&a, &a, &heap, &closures, 256));
    }

    #[test]
    fn mutually_capturing_closures_compare_without_looping() {
        let heap = Heap::new();
        let mut closures = ClosureTable::new();
        let mut table = MemoTable::default();
        let a = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(1),
            captures: vec![],
        });
        let b = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(1),
            captures: vec![],
        });
        closures.closure_mut(a).captures = vec![Value::Closure(b)];
        closures.closure_mut(b).captures = vec![Value::Closure(a)];
        assert!(table.values_equal(
            &Value::Closure(a),
            &Value::Closure(b),
            &heap,
            &closures,
            ARG_COMPARE_BUDGET
        ));
    }

    #[test]
    fn a_local_cell_is_found_through_a_closure_in_a_record() {
        let mut heap = Heap::new();
        let mut closures = ClosureTable::new();
        let cell = heap.alloc_cell(Value::Int(0));
        let c = closures.alloc_closure(RuntimeClosure {
            function_id: FunctionId(1),
            captures: vec![Value::Cell(cell)],
        });
        let mut m = indexmap::IndexMap::new();
        m.insert("on_click".to_string(), Value::Closure(c));
        let rec = Value::Map(heap.alloc_map(m));
        let mut locals = HashSet::new();
        assert!(!holds_local_cell(&rec, &heap, &closures, &locals));
        locals.insert(cell);
        assert!(holds_local_cell(&rec, &heap, &closures, &locals));
    }

    #[test]
    fn sweep_keeps_only_slots_visited_this_run() {
        let mut table = MemoTable::default();
        let slot = |visited| MemoSlot {
            serial: 1,
            visited,
            fn_id: FunctionId(0),
            site: 0,
            hit: false,
            captures: ScopeValues::new(),
            args: ScopeValues::new(),
            result: Value::Nil,
            deps: vec![],
            outputs: vec![],
            touches: StateTouches::default(),
            host_revision: 0,
            resources_revision: 0,
            pure: true,
        };
        table.begin_run();
        let run = table.run();
        let p1: ScopePath = SmallVec::from_slice(&[PathPart::Call(1)]);
        let p2: ScopePath = SmallVec::from_slice(&[PathPart::Call(2)]);
        table.insert(p1.clone(), slot(run));
        table.insert(p2.clone(), slot(run.wrapping_sub(1)));
        assert_eq!(table.sweep(), 1);
        assert!(table.get(&p1).is_some());
        assert!(table.get(&p2).is_none());
        assert_eq!(table.stats.evicted, 1);
    }
}

/// End-to-end checks through `Env::run`, the way a console program or a host
/// frame loop exercises the memo.
#[cfg(test)]
mod env_tests {
    use crate::env::Env;
    use crate::value::Value;

    fn env(src: &str) -> (Env, crate::stack::StackKey) {
        let mut env = Env::new();
        env.set_echo(false);
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        (env, sid)
    }

    /// A function big enough to be worth a record (see `MIN_SCOPE_INSTS`).
    const SQUARE: &str = "fn square(x)
  let acc = 0
  for k in range(0, 20) do acc = acc + x end
  acc * x / 20 + 1
end
";

    #[test]
    fn a_pure_call_is_replayed_on_the_second_run() {
        let src = format!(
            "{SQUARE}let total = 0
for i in range(0, 10) do total = total + square(i) end
total"
        );
        let (mut env, sid) = env(&src);
        let first = env.run(sid).unwrap();
        assert_eq!(first, Value::Int(295));
        let recorded = env.memo_stats(sid).unwrap().records;
        env.reset_stack(sid).unwrap();
        let second = env.run(sid).unwrap();
        assert_eq!(second, first);
        let stats = env.memo_stats(sid).unwrap();
        assert!(recorded > 0, "the calls were recorded");
        assert_eq!(stats.hits, recorded, "and every one replayed");
    }

    #[test]
    fn a_site_whose_records_are_never_replayed_goes_cold() {
        // Every call gets an argument it has never been called with, so no
        // record can ever be replayed and each run throws the last run's
        // away. After COLD_AFTER_UNHIT_EVICTIONS such rounds the site stops
        // being recorded (one probe per run aside) instead of paying for 20
        // records a run forever.
        let src = format!(
            "{SQUARE}\nstate n = 0\nn = n + 1\nlet t = 0\nfor i in range(0, 20) do t = t + square(n * 100 + i) end\nt"
        );
        let (mut env, sid) = env(&src);
        let mut per_run = Vec::new();
        for _ in 0..8 {
            let before = env.memo_stats(sid).unwrap().records;
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            per_run.push(env.memo_stats(sid).unwrap().records - before);
        }
        let stats = env.memo_stats(sid).unwrap();
        assert_eq!(
            stats.hits, 0,
            "a fresh argument every call can never replay"
        );
        assert!(stats.cold > 0, "the site went cold: {stats:?}");
        assert_eq!(
            per_run[0], 20,
            "it records everything at first: {per_run:?}"
        );
        assert!(
            per_run[7] <= 1,
            "and only probes once a run when cold: {per_run:?}"
        );
    }

    #[test]
    fn a_cold_site_records_again_once_it_starts_replaying() {
        // The same shape, but the arguments settle after the site has gone
        // cold. The once-per-run probe notices, and replays resume.
        let src = format!(
            "{SQUARE}\nstate n = 0\nif n < 4 then n = n + 1 end\nlet t = 0\nfor i in range(0, 20) do t = t + square(n * 100 + i) end\nt"
        );
        let (mut env, sid) = env(&src);
        for _ in 0..40 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
        }
        let stats = env.memo_stats(sid).unwrap();
        assert!(stats.cold > 0, "it went cold while `n` moved: {stats:?}");
        assert!(
            stats.hits > 0,
            "and replays again once it settled: {stats:?}"
        );
    }

    #[test]
    fn a_call_whose_result_is_mutated_in_place_is_not_replayed() {
        // `build`'s result roots an in-place web in the caller, so the call
        // is kept out of memoized scopes: a record would hold the array the
        // loop then rewrites, and the next run would replay it already bumped.
        let (mut env, sid) = env("fn build(n)
  let s = 0
  for k in range(0, 40) do s = s + k end
  f64_array(n)
end
let a = build(4)
for i in range(0, 4) do a[i] = a[i] + 1.0 end
a[0]");
        for _ in 0..3 {
            env.reset_stack(sid).unwrap();
            assert_eq!(env.run(sid).unwrap(), Value::Float(1.0));
        }
        assert_eq!(env.memo_stats(sid).unwrap().hits, 0);
    }

    #[test]
    fn a_counter_in_a_called_function_keeps_counting() {
        let (mut env, sid) = env("fn tick()
  state n = 0
  n = n + 1
  n
end
tick() + tick()");
        assert_eq!(env.run(sid).unwrap(), Value::Int(2));
        env.reset_stack(sid).unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(4));
        env.reset_stack(sid).unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(6));
        assert_eq!(
            env.memo_stats(sid).unwrap().hits,
            0,
            "a scope that changes its state never validates"
        );
    }

    #[test]
    fn a_printing_call_prints_on_every_run() {
        let (mut env, sid) = env("fn hello(i) print(\"hi {i}\") end
for i in range(0, 3) do hello(i) end");
        for _ in 0..3 {
            env.reset_stack(sid).unwrap();
            env.run(sid).unwrap();
            assert_eq!(env.take_output().len(), 3);
        }
        assert_eq!(env.memo_stats(sid).unwrap().hits, 0);
    }

    #[test]
    fn records_do_not_survive_a_program_transfer() {
        let src = format!("{SQUARE}square(1) + square(2)");
        let (mut env, sid) = env(&src);
        assert_eq!(env.run(sid).unwrap(), Value::Int(7));
        assert!(env.memo_slots(sid) > 0);
        let pid = env.stack(sid).unwrap().program_id;
        let program = env.compile_program(pid, &src).unwrap();
        env.transfer_state(sid, program).unwrap();
        assert_eq!(env.memo_slots(sid), 0);
        env.reset_stack(sid).unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(7));
    }

    #[test]
    fn unvisited_records_are_evicted() {
        let src = format!(
            "state var which = 0
{SQUARE}fn cube(x)
  let acc = 0
  for k in range(0, 20) do acc = acc + x * x end
  acc * x / 20
end
let r = if get which == 0 then square(3) else cube(3) end
set which = 1 - get which
r"
        );
        let (mut env, sid) = env(&src);
        assert_eq!(env.run(sid).unwrap(), Value::Int(10));
        assert_eq!(env.memo_slots(sid), 1);
        env.reset_stack(sid).unwrap();
        assert_eq!(env.run(sid).unwrap(), Value::Int(27));
        assert_eq!(
            env.memo_slots(sid),
            1,
            "square's record went with its branch"
        );
        assert_eq!(env.memo_stats(sid).unwrap().evicted, 1);
    }
}
