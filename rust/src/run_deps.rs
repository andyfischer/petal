//! What the last run of a stack depended on, so a host can skip a frame whose
//! inputs have not changed.
//!
//! Every host re-runs its script once per frame. Most of those frames read
//! exactly what the previous frame read and would draw exactly what it drew.
//! [`RunDeps`] records, per run, the things a run's result can depend on:
//!
//! - the host bindings it read (`mouse_x`, `time`, `screen_width`, …), each
//!   with a content fingerprint of the value it saw;
//! - whether it consulted host data outside the binding table (a `host_data`
//!   or `query` native), which only the host can say has changed;
//! - whether it consumed randomness;
//! - whether it left `state` different from how it found it, in which case the
//!   *next* run may differ even with identical inputs (a counter, a motion
//!   that has not settled);
//! - the resource table's revision, so a resolved `Pending` re-runs the frame.
//!
//! [`crate::env::Env::run_needed`] compares that record against the present:
//! a run is needed if any read binding's fingerprint differs, or any of the
//! other flags says the previous run did not reach a fixed point. A host that
//! consults it before `env.run` gets exact, host-independent frame gating: a
//! form that never reads `time()` stops running while the pointer is still.
//!
//! Recording is cheap and always on. A binding read sets one flag in a vector
//! indexed by symbol id; fingerprints are computed once per completed run for
//! the handful of symbols read. Nothing here is a GC root: only fingerprints
//! (u64s) outlive the run.

use std::collections::HashMap;

use crate::closure_table::ClosureTable;
use crate::heap::Heap;
use crate::memo::content_equal;
use crate::symbol::SymbolId;
use crate::value::Value;

/// Why [`crate::env::Env::run_needed`] says a run is needed. Diagnostic: a
/// host overlay or a test can show which input moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunReason {
    /// No completed run has recorded its dependencies yet (a fresh stack, a
    /// run that yielded or was reset before completing).
    NoRecord,
    /// The host asked for a run ([`crate::env::Env::invalidate_run`]), or
    /// changed state from outside (`set_state`, `restore_state`, hot reload).
    Forced,
    /// A binding the last run read has a different value now.
    BindingChanged(SymbolId),
    /// The last run consulted host data outside the binding table
    /// (`host_data`, `query`, …) and the host has since reported it changed.
    HostDataChanged,
    /// The last run wrote a different value into some `state` slot, so the
    /// next run starts from different state.
    StateUnsettled,
    /// The last run drew from the random stream; another run would draw
    /// different numbers.
    RngConsumed,
    /// A pending resource was created, resolved or rejected since the last run.
    ResourcesChanged,
}

/// A snapshot of [`RunDeps::activity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    pub binding_reads: u32,
    pub host_reads: u32,
    pub resource_reads: u32,
    pub emits: u32,
    pub effects: u32,
}

/// The dependency record of one stack's most recent run. Lives on the
/// [`crate::stack::Stack`]; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct RunDeps {
    /// Per-symbol "read this run" flags, indexed by `SymbolId.0`. Grown on
    /// demand; cleared between runs by walking `read_list`, not the vector.
    read_flags: Vec<bool>,
    /// The symbols read this run, in first-read order.
    read_list: Vec<SymbolId>,
    /// The completed record: each binding read by the last run and the
    /// fingerprint of the value it read.
    reads: Vec<(SymbolId, u64)>,
    /// The last run called a native that reads host data outside the binding
    /// table. See [`note_host_read`](Self::note_host_read).
    host_read: bool,
    /// Host-data revision the last run saw; the host bumps it through
    /// [`crate::env::Env::note_host_data_changed`].
    host_data_revision: u64,
    /// Current host-data revision.
    host_data_now: u64,
    /// The RNG state when the run began, to detect draws.
    rng_at_start: u64,
    rng_consumed: bool,
    /// Set by state instructions during the run when a slot ends up holding a
    /// different value than it started with.
    state_unsettled: bool,
    /// Resource-table revision when the run completed.
    resources_revision: u64,
    /// Whether `reads` and the flags above describe a *completed* run.
    valid: bool,
    /// A host-side request for a run, cleared when the next run begins.
    forced: bool,
    /// Activity counters for the run in progress, read by the VM around each
    /// native call to classify it for memoized scopes (`crate::memo`): how
    /// many binding reads, host-data reads, resource-table reads, output
    /// pushes and irreproducible effects natives have reported so far.
    /// Wrapping counters compared before and after a call; never reset.
    binding_reads: u32,
    host_reads: u32,
    resource_reads: u32,
    emits: u32,
    effects: u32,
}

impl RunDeps {
    /// Record that the run in progress read binding `sym`. One flag test on
    /// the hot path; the symbol is listed once no matter how often it is read.
    #[inline]
    pub fn note_binding_read(&mut self, sym: SymbolId) {
        self.binding_reads = self.binding_reads.wrapping_add(1);
        let i = sym.0 as usize;
        if i >= self.read_flags.len() {
            self.read_flags.resize(i + 1, false);
        }
        if !self.read_flags[i] {
            self.read_flags[i] = true;
            self.read_list.push(sym);
        }
    }

    /// Record that the run in progress consulted host data the binding table
    /// does not cover. Natives that answer from a host-owned source (`host_data`,
    /// a query cache, an editor buffer) call this so the gate knows the run
    /// depends on something only the host can see change.
    #[inline]
    pub fn note_host_read(&mut self) {
        self.host_read = true;
        self.host_reads = self.host_reads.wrapping_add(1);
    }

    /// Record that a native consulted the resource table (a `Pending` it may
    /// answer differently once the resource resolves).
    #[inline]
    pub fn note_resource_read(&mut self) {
        self.resource_reads = self.resource_reads.wrapping_add(1);
    }

    /// Record that a native pushed a value into an output buffer.
    #[inline]
    pub fn note_emit(&mut self) {
        self.emits = self.emits.wrapping_add(1);
    }

    /// Record that a native did something a replay could not reproduce:
    /// printed, advanced a counter, created a resource, reseeded noise.
    #[inline]
    pub fn note_effect(&mut self) {
        self.effects = self.effects.wrapping_add(1);
    }

    /// The activity counters as a snapshot. Two snapshots around a native
    /// call say what the call did.
    #[inline]
    pub fn activity(&self) -> Activity {
        Activity {
            binding_reads: self.binding_reads,
            host_reads: self.host_reads,
            resource_reads: self.resource_reads,
            emits: self.emits,
            effects: self.effects,
        }
    }

    /// The host-data revision now.
    pub fn host_data_now(&self) -> u64 {
        self.host_data_now
    }

    /// Record that a `state` slot changed value during the run in progress.
    #[inline]
    pub fn note_state_unsettled(&mut self) {
        self.state_unsettled = true;
    }

    /// Record a write that replaced `old` with `new` in a `state` slot, and
    /// mark the run unsettled if that changed the slot (see
    /// [`state_changed`]). `mutated` means an in-place producer already
    /// edited the slot's object, which counts as a change without comparing.
    pub fn note_state_write(
        &mut self,
        old: Option<Value>,
        new: Value,
        mutated: bool,
        heap: &Heap,
        closures: &ClosureTable,
    ) {
        if !self.state_unsettled && (mutated || state_changed(old, new, heap, closures)) {
            self.state_unsettled = true;
        }
    }

    /// Whether the run in progress (or, after completion, the last run) has
    /// changed some state slot's value.
    pub fn state_unsettled(&self) -> bool {
        self.state_unsettled
    }

    /// Whether the last run consulted host data outside the binding table.
    pub fn host_read(&self) -> bool {
        self.valid && self.host_read
    }

    /// Whether the last run consumed randomness.
    pub fn rng_consumed(&self) -> bool {
        self.valid && self.rng_consumed
    }

    /// The bindings the last run read, in first-read order.
    pub fn bindings_read(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.reads.iter().map(|(s, _)| *s)
    }

    /// Whether a completed run has recorded its dependencies.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Ask for a run regardless of what changed (state edited from outside,
    /// the program reloaded, host data replaced).
    pub fn force(&mut self) {
        self.forced = true;
    }

    /// The host's data behind `host_data`-style natives changed. Only matters
    /// if the last run read such data.
    pub fn note_host_data_changed(&mut self) {
        self.host_data_now = self.host_data_now.wrapping_add(1);
    }

    /// Start recording a run. Clears the in-progress scratch and marks the
    /// record invalid until [`finish_run`](Self::finish_run).
    pub fn begin_run(&mut self, rng_state: u64) {
        for sym in self.read_list.drain(..) {
            self.read_flags[sym.0 as usize] = false;
        }
        self.host_read = false;
        self.state_unsettled = false;
        self.rng_consumed = false;
        self.rng_at_start = rng_state;
        self.valid = false;
        self.forced = false;
    }

    /// Complete the record for a run that ran to completion or stopped on an
    /// error. `bindings` and `heap` are the context the run executed in;
    /// `rng_state` its RNG afterwards; `resources_revision` the resource
    /// table's revision now.
    pub fn finish_run(
        &mut self,
        bindings: &HashMap<SymbolId, Value>,
        heap: &Heap,
        rng_state: u64,
        resources_revision: u64,
    ) {
        self.reads.clear();
        for sym in &self.read_list {
            let fp = fingerprint_binding(bindings.get(sym), heap);
            self.reads.push((*sym, fp));
        }
        self.rng_consumed = rng_state != self.rng_at_start;
        self.resources_revision = resources_revision;
        self.host_data_revision = self.host_data_now;
        self.valid = true;
    }

    /// Whether a run is needed now, and why. `None` means the last run's
    /// inputs are all unchanged and it reached a fixed point, so running again
    /// would reproduce it.
    pub fn run_needed(
        &self,
        bindings: &HashMap<SymbolId, Value>,
        heap: &Heap,
        resources_revision: u64,
    ) -> Option<RunReason> {
        if !self.valid {
            return Some(RunReason::NoRecord);
        }
        if self.forced {
            return Some(RunReason::Forced);
        }
        if self.state_unsettled {
            return Some(RunReason::StateUnsettled);
        }
        if self.rng_consumed {
            return Some(RunReason::RngConsumed);
        }
        if resources_revision != self.resources_revision {
            return Some(RunReason::ResourcesChanged);
        }
        if self.host_read && self.host_data_now != self.host_data_revision {
            return Some(RunReason::HostDataChanged);
        }
        for (sym, fp) in &self.reads {
            if fingerprint_binding(bindings.get(sym), heap) != *fp {
                return Some(RunReason::BindingChanged(*sym));
            }
        }
        None
    }
}

/// Fingerprint budget: the number of heap nodes a binding may span before it
/// is treated as unfingerprintable. Host bindings are small (a key list, a
/// palette record); anything larger is deemed changed on every check rather
/// than risk two different values hashing alike after truncation.
const FINGERPRINT_BUDGET: usize = 4096;

/// A fingerprint that never matches a real one, for a value that could not be
/// fingerprinted: it makes every check report "changed", which is the safe
/// answer.
const UNFINGERPRINTABLE: u64 = u64::MAX;

/// Content fingerprint of a bound value, `None`/absent included. Two bindings
/// with equal content — a fresh list of the same key names, the same palette
/// record re-allocated this frame — fingerprint alike, so a host re-binding
/// its inputs every frame does not defeat the gate.
pub fn fingerprint_binding(value: Option<&Value>, heap: &Heap) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut budget = FINGERPRINT_BUDGET;
    match value {
        None => h.write_u8(0xFF),
        Some(v) => {
            if !hash_content(v, heap, &mut h, &mut budget) {
                return UNFINGERPRINTABLE;
            }
        }
    }
    let fp = h.finish();
    if fp == UNFINGERPRINTABLE { fp - 1 } else { fp }
}

/// Hash `v` by content. Returns false when the value is not fingerprintable
/// (a closure, a handle, or a structure past the budget).
fn hash_content(
    v: &Value,
    heap: &Heap,
    h: &mut impl std::hash::Hasher,
    budget: &mut usize,
) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    match v {
        Value::Nil => h.write_u8(0),
        Value::Bool(b) => {
            h.write_u8(1);
            h.write_u8(*b as u8);
        }
        Value::Int(n) => {
            h.write_u8(2);
            h.write_i64(*n);
        }
        Value::Float(f) => {
            h.write_u8(3);
            h.write_u64(f.to_bits());
        }
        Value::String(id) => {
            h.write_u8(4);
            h.write(heap.get_string(*id).as_bytes());
            h.write_u8(0);
        }
        Value::List(id) => {
            h.write_u8(5);
            let elems = heap.get_list(*id);
            h.write_usize(elems.len());
            for e in elems {
                if !hash_content(e, heap, h, budget) {
                    return false;
                }
            }
        }
        Value::Map(id) => {
            h.write_u8(6);
            let map = heap.get_map(*id);
            h.write_usize(map.len());
            for (k, e) in map {
                h.write(k.as_bytes());
                h.write_u8(0);
                if !hash_content(e, heap, h, budget) {
                    return false;
                }
            }
        }
        Value::EnumVariant { tag, data } => {
            h.write_u8(7);
            h.write(heap.get_string(*tag).as_bytes());
            h.write_u8(0);
            return hash_content(&Value::List(*data), heap, h, budget);
        }
        Value::F64Array(id) => {
            h.write_u8(8);
            let data = heap.get_f64_array(*id);
            h.write_usize(data.len());
            for f in data {
                h.write_u64(f.to_bits());
            }
        }
        Value::Vec2(x, y) => {
            h.write_u8(9);
            h.write_u64(x.to_bits());
            h.write_u64(y.to_bits());
        }
        Value::Dual { value, derivative } => {
            h.write_u8(10);
            h.write_u64(value.to_bits());
            h.write_u64(derivative.to_bits());
        }
        Value::Symbol(s) => {
            h.write_u8(11);
            h.write_u32(s.0);
        }
        Value::Element(id) => {
            h.write_u8(12);
            h.write(heap.get_string(heap.get_element_tag(*id)).as_bytes());
            h.write_u8(0);
            if !hash_content(&Value::Map(heap.get_element_props(*id)), heap, h, budget) {
                return false;
            }
            return hash_content(
                &Value::List(heap.get_element_children(*id)),
                heap,
                h,
                budget,
            );
        }
        Value::Handle(hv) => {
            h.write_u8(13);
            h.write_u32(hv.class.0.into());
            h.write_usize(hv.slot as usize);
            h.write_u64(hv.serial as u64);
        }
        Value::NativeFunction(id) => {
            h.write_u8(14);
            h.write_u32(id.0);
        }
        // A cell's identity is what a script reads through; its contents are
        // runtime state, not host input, so hash the contents.
        Value::Cell(id) => {
            h.write_u8(15);
            return hash_content(&heap.cell_read(*id), heap, h, budget);
        }
        // Closures, overload sets and pendings have no content identity a host
        // could re-create equal; treat them as always changed.
        Value::Closure(_) | Value::OverloadSet(_) | Value::Pending(_) => return false,
    }
    true
}

/// Node budget for one state-write comparison. Small records and short lists
/// compare in full; a large collection is deemed changed.
const STATE_COMPARE_BUDGET: usize = 256;

/// Whether a `state` slot (or a `state var` cell) that held `old` — `None` for
/// a slot being created — is changed by now holding `new`. Compares by content
/// ([`crate::memo::content_equal`]) and gives up past a small node budget:
/// "changed" is always the safe answer here, it only costs one more run.
pub fn state_changed(old: Option<Value>, new: Value, heap: &Heap, closures: &ClosureTable) -> bool {
    match old {
        None => true,
        Some(old) => !content_equal(&old, &new, heap, closures, STATE_COMPARE_BUDGET),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(entries: &[(u32, Value)]) -> HashMap<SymbolId, Value> {
        entries.iter().map(|(s, v)| (SymbolId(*s), *v)).collect()
    }

    #[test]
    fn fresh_record_always_needs_a_run() {
        let heap = Heap::new();
        let deps = RunDeps::default();
        assert_eq!(
            deps.run_needed(&HashMap::new(), &heap, 0),
            Some(RunReason::NoRecord)
        );
    }

    #[test]
    fn unchanged_read_bindings_need_no_run() {
        let mut heap = Heap::new();
        let b = bindings(&[(1, Value::Int(5)), (2, Value::Int(9))]);
        let mut deps = RunDeps::default();
        deps.begin_run(7);
        deps.note_binding_read(SymbolId(1));
        deps.finish_run(&b, &heap, 7, 0);
        assert_eq!(deps.run_needed(&b, &heap, 0), None);

        // An unread binding may change freely.
        let b2 = bindings(&[(1, Value::Int(5)), (2, Value::Int(10))]);
        assert_eq!(deps.run_needed(&b2, &heap, 0), None);

        // A read one may not.
        let b3 = bindings(&[(1, Value::Int(6)), (2, Value::Int(9))]);
        assert_eq!(
            deps.run_needed(&b3, &heap, 0),
            Some(RunReason::BindingChanged(SymbolId(1)))
        );
    }

    #[test]
    fn equal_content_in_a_fresh_allocation_fingerprints_alike() {
        let mut heap = Heap::new();
        let s1 = heap.alloc_string("a".into());
        let l1 = heap.alloc_list(vec![Value::String(s1), Value::Int(1)]);
        let b1 = bindings(&[(1, Value::List(l1))]);
        let mut deps = RunDeps::default();
        deps.begin_run(0);
        deps.note_binding_read(SymbolId(1));
        deps.finish_run(&b1, &heap, 0, 0);

        let s2 = heap.alloc_string("a".into());
        let l2 = heap.alloc_list(vec![Value::String(s2), Value::Int(1)]);
        let b2 = bindings(&[(1, Value::List(l2))]);
        assert_eq!(deps.run_needed(&b2, &heap, 0), None);

        let l3 = heap.alloc_list(vec![Value::String(s2), Value::Int(2)]);
        let b3 = bindings(&[(1, Value::List(l3))]);
        assert!(deps.run_needed(&b3, &heap, 0).is_some());
    }

    #[test]
    fn a_read_of_an_absent_binding_notices_it_appearing() {
        let mut heap = Heap::new();
        let mut deps = RunDeps::default();
        deps.begin_run(0);
        deps.note_binding_read(SymbolId(3));
        deps.finish_run(&HashMap::new(), &heap, 0, 0);
        assert_eq!(deps.run_needed(&HashMap::new(), &heap, 0), None);
        let b = bindings(&[(3, Value::Nil)]);
        assert_eq!(
            deps.run_needed(&b, &heap, 0),
            Some(RunReason::BindingChanged(SymbolId(3)))
        );
    }

    #[test]
    fn the_other_flags_each_force_a_run() {
        let heap = Heap::new();
        let b = HashMap::new();

        let mut deps = RunDeps::default();
        deps.begin_run(1);
        deps.finish_run(&b, &heap, 2, 0);
        assert_eq!(deps.run_needed(&b, &heap, 0), Some(RunReason::RngConsumed));

        let mut deps = RunDeps::default();
        deps.begin_run(1);
        deps.note_state_unsettled();
        deps.finish_run(&b, &heap, 1, 0);
        assert_eq!(
            deps.run_needed(&b, &heap, 0),
            Some(RunReason::StateUnsettled)
        );

        let mut deps = RunDeps::default();
        deps.begin_run(1);
        deps.finish_run(&b, &heap, 1, 4);
        assert_eq!(
            deps.run_needed(&b, &heap, 5),
            Some(RunReason::ResourcesChanged)
        );

        let mut deps = RunDeps::default();
        deps.begin_run(1);
        deps.finish_run(&b, &heap, 1, 0);
        deps.force();
        assert_eq!(deps.run_needed(&b, &heap, 0), Some(RunReason::Forced));
        deps.begin_run(1);
        deps.finish_run(&b, &heap, 1, 0);
        assert_eq!(deps.run_needed(&b, &heap, 0), None);
    }

    #[test]
    fn host_data_changes_matter_only_to_a_run_that_read_them() {
        let heap = Heap::new();
        let b = HashMap::new();
        let mut deps = RunDeps::default();
        deps.begin_run(0);
        deps.finish_run(&b, &heap, 0, 0);
        deps.note_host_data_changed();
        assert_eq!(
            deps.run_needed(&b, &heap, 0),
            None,
            "did not read host data"
        );

        deps.begin_run(0);
        deps.note_host_read();
        deps.finish_run(&b, &heap, 0, 0);
        assert_eq!(deps.run_needed(&b, &heap, 0), None);
        deps.note_host_data_changed();
        assert_eq!(
            deps.run_needed(&b, &heap, 0),
            Some(RunReason::HostDataChanged)
        );
    }
}
