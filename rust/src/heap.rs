//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! See docs/Architecture.md for the surrounding runtime design.
//!
//! Heap objects are **immutable by construction**: there are no in-place
//! mutators for collection payloads. "Mutations" (`list_append`, `list_set`,
//! `list_drop_last`, `map_set`, `map_remove`, `f64_array_set`,
//! `f64_array_swap`) allocate and return a *new* id, leaving the input
//! untouched (value semantics). This is what makes sharing heap objects
//! between executions safe — see the "Speculative execution" section of
//! docs/program-modification.md.
//!
//! **One exception: [`CellId`]**, the box behind a `var` binding, which
//! [`Heap::cell_write`] overwrites in place. It is confined by construction —
//! no expression evaluates to a `Value::Cell`, so a cell id never enters a
//! collection payload — and `fork` deep-copies the cell slab like every other,
//! so speculative execution stays isolated.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::stats::{AllocKind, AllocStats, DupKind, DupStats};
use crate::value::Value;

/// Bytes copied when a `Vec<Value>`/map of `n` `Value`s is cloned. The `Value`
/// enum is `Copy`, so cloning the backing store copies `n * size_of::<Value>()`
/// bytes (string/list/map payloads referenced by id are shared, not copied).
fn value_slice_bytes(len: usize) -> u64 {
    (len * std::mem::size_of::<Value>()) as u64
}

/// Bytes copied when a map's entry table is cloned: each key `String`'s content
/// plus one `Copy` `Value` per entry.
fn map_entries_bytes(entries: &IndexMap<String, Value>) -> u64 {
    let keys: u64 = entries.keys().map(|k| k.len() as u64).sum();
    keys + value_slice_bytes(entries.len())
}

/// Opaque handle to a heap-allocated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// Opaque handle to a heap-allocated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

/// Opaque handle to a heap-allocated flat f64 array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct F64ArrayId(pub u32);

/// Opaque handle to a heap-allocated map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);

/// Opaque handle to a heap-allocated element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(pub u32);

/// Opaque handle to a heap-allocated **cell** — the one-value mutable box
/// behind a `var` binding.
///
/// Cells are the sole exception to this module's immutable-by-construction
/// rule: [`cell_write`](Heap::cell_write) overwrites the slot in place and
/// keeps the id, which is the whole point (every holder of the id, including a
/// closure that captured it, observes the write). What keeps that sound is the
/// *containment invariant*: no expression evaluates to a `Value::Cell`, so a
/// cell id never reaches a collection payload, a host, or user code. Reads
/// dereference; only closure capture shares one. See
/// docs/lowering-confusion-20260726.md §6d.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellId(pub u32);

/// Payload of a single heap element: three `Copy` ids referencing the element's
/// tag string, props map, and children list. Stored as the `T` of an element
/// slab; the `gc_mark`/`alive` bits live in the enclosing [`Slot`].
#[derive(Clone, Copy)]
struct ElementPayload {
    tag: StringId,
    props: MapId,
    children: ListId,
}

/// One slab slot: a payload plus its GC bits. `gc_mark` is the mark-and-sweep
/// reachability flag (cleared each sweep); `alive` is false for a reclaimed slot
/// sitting on the free list.
#[derive(Clone)]
struct Slot<T> {
    data: T,
    gc_mark: bool,
    alive: bool,
}

/// A generic slot store with an index free list. Backs each of the heap's object
/// kinds. Ids are bare indices (no generation counter): a reclaimed slot is
/// reused and hands back the same index value.
#[derive(Clone)]
struct Slab<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Slab<T> {
    fn new() -> Self {
        Slab {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Allocate `data` into a reused free slot or a fresh one; return its index.
    fn alloc(&mut self, data: T) -> u32 {
        if let Some(idx) = self.free.pop() {
            let slot = &mut self.slots[idx as usize];
            slot.data = data;
            slot.gc_mark = false;
            slot.alive = true;
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Slot {
                data,
                gc_mark: false,
                alive: true,
            });
            idx
        }
    }

    fn get(&self, idx: u32) -> &T {
        &self.slots[idx as usize].data
    }

    fn get_mut(&mut self, idx: u32) -> &mut T {
        &mut self.slots[idx as usize].data
    }

    /// Mark slot `idx` live. Returns true iff it was newly marked (alive and not
    /// already marked) — the caller then recurses into the payload's children.
    fn mark(&mut self, idx: u32) -> bool {
        let slot = &mut self.slots[idx as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
            true
        } else {
            false
        }
    }

    /// Sweep: reclaim every unmarked-live slot (flip alive off, run `on_reclaim`
    /// on its payload to release backing memory / side-table entries, push to the
    /// free list); clear the mark on every surviving slot. Rebuilds `free`.
    ///
    /// `on_reclaim` must *release* the payload's heap allocation, not merely
    /// empty it: `Vec::clear()` keeps the buffer, so a swept 160 KB array would
    /// sit on the free list still holding its 160 KB. That memory can never be
    /// reused for anything either, because [`alloc`](Self::alloc) overwrites
    /// `slot.data` wholesale (dropping whatever buffer was there). Assign a
    /// fresh empty value (`*v = Vec::new()`) instead.
    fn sweep_with(&mut self, mut on_reclaim: impl FnMut(&mut T)) {
        self.free.clear();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    on_reclaim(&mut slot.data);
                    self.free.push(i as u32);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct Heap {
    strings: Slab<String>,
    lists: Slab<Vec<Value>>,
    f64_arrays: Slab<Vec<f64>>,
    maps: Slab<IndexMap<String, Value>>,
    elements: Slab<ElementPayload>,
    /// One-value mutable boxes behind `var` bindings. See [`CellId`].
    cells: Slab<Value>,
    /// String intern table: content → existing StringId
    intern_table: HashMap<String, StringId>,
    /// Estimated collector work owed by everything allocated since the last
    /// collection, in the "bytes" currency of [`Heap::collection_cost`]: each
    /// allocation charges its payload size plus [`SLOT_TRACE_COST`]. Tracked
    /// incrementally (an allocation must stay O(1)) and compared against
    /// [`gc_budget`](Self::gc_budget) by [`should_collect`](Self::should_collect).
    alloc_charge: u64,
    /// How much may be charged to [`alloc_charge`](Self::alloc_charge) before
    /// the next collection. Recomputed at the end of every sweep from the live
    /// set; see [`should_collect`](Self::should_collect).
    gc_budget: u64,
    /// Mark-and-sweep cycles run so far — see [`collections`](Self::collections).
    collections: u64,
    /// Value-duplication statistics. Records every copy-on-write and fork so we
    /// can track (and shrink) how much copying immutable values cost. Collected
    /// only in debug builds or with the `dup-stats` feature — see
    /// [`crate::stats`].
    dup_stats: DupStats,
    /// Allocation statistics: how many new heap objects were created, per kind.
    /// Cumulative over the run (never decremented by GC), so it surfaces
    /// temporary-object churn. Same gate as `dup_stats`.
    alloc_stats: AllocStats,
}

/// What one slot costs a collection, expressed in the same "bytes" currency as
/// payload sizes so the two can be added into a single work estimate. Every
/// collection walks *every* slot of every slab (mark clears + sweep), live or
/// free, so slot count is a real cost driver independent of payload size —
/// without this term a heap of a million empty lists would look free to trace
/// and get collected constantly. The exact value is a rough weighting, not a
/// measurement: it says "visiting a slot costs about as much as copying 64
/// bytes".
const SLOT_TRACE_COST: u64 = 64;

/// Floor on the work budget between collections. Below this the heap is small
/// enough that collecting is pointless: a megabyte of floating garbage is
/// cheaper to tolerate than the collections that would reclaim it. (The previous
/// count-based rule collected every 1024 allocations no matter how tiny they
/// were; for small-object programs this floor is the replacement, and it lets
/// them run considerably further between traces.)
const GC_MIN_BUDGET_BYTES: u64 = 1024 * 1024;

/// How far the heap may grow, as a multiple of what tracing it costs, before
/// the next collection. This is what keeps collection cost *proportional to
/// live data*: a cycle costs O(live set), and we run one only after allocating
/// `GC_HEAP_GROWTH` times that much, so the collector's amortized cost per
/// allocated byte is a constant no matter how big the heap gets. Raising it
/// trades peak memory for throughput.
const GC_HEAP_GROWTH: u64 = 2;

impl Heap {
    pub fn new() -> Self {
        Self {
            strings: Slab::new(),
            lists: Slab::new(),
            f64_arrays: Slab::new(),
            maps: Slab::new(),
            elements: Slab::new(),
            cells: Slab::new(),
            intern_table: HashMap::new(),
            alloc_charge: 0,
            gc_budget: GC_MIN_BUDGET_BYTES,
            collections: 0,
            dup_stats: DupStats::new(),
            alloc_stats: AllocStats::new(),
        }
    }

    /// Value-duplication statistics accumulated by this heap's copy-on-write
    /// operations and forks. All zero in release builds unless the `dup-stats`
    /// feature is enabled — see [`crate::stats`].
    pub fn dup_stats(&self) -> &DupStats {
        &self.dup_stats
    }

    /// Mutable access to the duplication stats, e.g. to [`DupStats::reset`] them
    /// between runs.
    pub fn dup_stats_mut(&mut self) -> &mut DupStats {
        &mut self.dup_stats
    }

    /// Allocation statistics: how many new heap objects this heap created, per
    /// kind. Cumulative over the run; same gate as [`dup_stats`](Self::dup_stats).
    pub fn alloc_stats(&self) -> &AllocStats {
        &self.alloc_stats
    }

    /// Mutable access to the allocation stats, e.g. to reset them between runs.
    pub fn alloc_stats_mut(&mut self) -> &mut AllocStats {
        &mut self.alloc_stats
    }

    /// How many mark-and-sweep cycles this heap has run. A diagnostic: paired
    /// with [`live_bytes`](Self::live_bytes) it says whether collection is
    /// keeping up with allocation, and whether the *number* of collections is
    /// tracking bytes allocated (what we want) or object count (what the old
    /// count-based trigger did).
    pub fn collections(&self) -> u64 {
        self.collections
    }

    /// Bytes of backing store this heap is *holding onto*, live or not: the
    /// capacity of every slot's payload, including slots sitting on the free
    /// list. Compare against [`live_bytes`](Self::live_bytes) to see how much
    /// memory reclaimed-but-not-yet-reused slots are squatting on — the two
    /// should stay close after a sweep. Approximate for maps (it prices the
    /// entry table, not each key's own `String` buffer) — this is a diagnostic,
    /// not an allocator accounting.
    ///
    /// O(slots) and only used by diagnostics/tests, never on an allocation path.
    pub fn reserved_bytes(&self) -> u64 {
        let strings: u64 = self
            .strings
            .slots
            .iter()
            .map(|s| s.data.capacity() as u64)
            .sum();
        let lists: u64 = self
            .lists
            .slots
            .iter()
            .map(|l| value_slice_bytes(l.data.capacity()))
            .sum();
        let f64s: u64 = self
            .f64_arrays
            .slots
            .iter()
            .map(|a| (a.data.capacity() * std::mem::size_of::<f64>()) as u64)
            .sum();
        let maps: u64 = self
            .maps
            .slots
            .iter()
            .map(|m| value_slice_bytes(m.data.capacity()))
            .sum();
        strings + lists + f64s + maps
    }

    /// Total bytes of live payload this heap holds — the rough cost of cloning
    /// it. Used to attribute a `Fork`'s byte count and to size the GC budget
    /// (see [`should_collect`](Self::should_collect)); also handy for
    /// diagnostics. O(slots): never call it on an allocation path.
    pub fn live_bytes(&self) -> u64 {
        let strings: u64 = self
            .strings
            .slots
            .iter()
            .filter(|s| s.alive)
            .map(|s| s.data.len() as u64)
            .sum();
        let lists: u64 = self
            .lists
            .slots
            .iter()
            .filter(|l| l.alive)
            .map(|l| value_slice_bytes(l.data.len()))
            .sum();
        let f64s: u64 = self
            .f64_arrays
            .slots
            .iter()
            .filter(|a| a.alive)
            .map(|a| (a.data.len() * std::mem::size_of::<f64>()) as u64)
            .sum();
        let maps: u64 = self
            .maps
            .slots
            .iter()
            .filter(|m| m.alive)
            .map(|m| map_entries_bytes(&m.data))
            .sum();
        strings + lists + f64s + maps
    }

    /// Estimated cost of running one collection right now: the live payload
    /// this heap would have to trace, plus [`SLOT_TRACE_COST`] for every slot
    /// the mark/sweep walks. O(slots) — only called at the end of a sweep,
    /// which is already paying that walk.
    fn collection_cost(&self) -> u64 {
        let slots = (self.strings.slots.len()
            + self.lists.slots.len()
            + self.f64_arrays.slots.len()
            + self.maps.slots.len()
            + self.elements.slots.len()
            + self.cells.slots.len()) as u64;
        self.live_bytes() + SLOT_TRACE_COST * slots
    }

    /// Returns true when enough has been allocated since the last collection to
    /// justify another one.
    ///
    /// "Enough" is measured in *work owed*, not objects created. A collection is
    /// a full mark-and-sweep over every slab, so it costs O(live set + slots);
    /// counting allocations instead (the old `alloc_count >= 1024` rule) meant
    /// 1024 allocations of a 160 KB array — 160 MB of garbage — triggered the
    /// same single trace as 1024 tiny strings, while a program churning large
    /// arrays re-traced the whole heap every few frames and got steadily slower
    /// as the slab high-water mark grew.
    ///
    /// So each allocation charges its own size (plus a fixed per-slot term) to
    /// `alloc_charge`, and we collect once that reaches `gc_budget` — which the
    /// previous sweep set to `GC_HEAP_GROWTH ×` the cost of tracing the live
    /// set. Collection cost then stays proportional to live data: a program
    /// allocating steadily pays a constant amortized price per byte, however
    /// long it runs.
    ///
    /// Both sides are maintained incrementally, so this is O(1) — it is polled
    /// after every VM instruction.
    pub fn should_collect(&self) -> bool {
        self.alloc_charge >= self.gc_budget
    }

    /// Create an isolated clone of this heap for a forked execution. Because
    /// heap objects are immutable by construction (no in-place mutators), the
    /// fork shares no mutable state with its parent: each side allocates and
    /// GCs independently, while any id that existed at fork time refers to an
    /// equal object in both heaps. This is what makes two side-by-side
    /// executions safe — the variant can "mutate" freely (allocating new ids)
    /// without disturbing the original. Today this deep-copies the slot
    /// vectors; a later optimization can wrap payloads in `Rc` so the fork is
    /// O(live slots) pointer clones rather than a full copy (see
    /// docs/dev/bytecode-future-ideas.md, "Structural sharing").
    pub fn fork(&self) -> Heap {
        let mut child = self.clone();
        // The fork copied this whole heap. Attribute that copy to the child
        // (the execution that now owns the duplicate) with fresh counters, so
        // each context measures the work done on its own behalf rather than
        // re-counting its parent's history. Allocation counts reset too: the
        // child's objects already exist, so it starts counting new allocations
        // from the fork point.
        child.dup_stats.reset();
        child.alloc_stats.reset();
        child.collections = 0;
        // The GC counters themselves (`alloc_charge`, `gc_budget`) are *not*
        // reset: they describe the state of the heap the child just inherited,
        // not work done on anyone's behalf. The child owns that heap now — it
        // holds the same live set and is the same distance from its next
        // collection as the parent was.
        child.dup_stats.record(DupKind::Fork, || self.live_bytes());
        child
    }

    /// Account for one new heap object: charge the collector budget for the
    /// work this object will cost to trace and reclaim (`payload_bytes` of
    /// backing store plus one slot visit), and record it in the stats. The
    /// charge is what makes the GC trigger size-aware — see
    /// [`should_collect`](Self::should_collect).
    fn tick_alloc(&mut self, kind: AllocKind, payload_bytes: u64) {
        self.alloc_charge += payload_bytes + SLOT_TRACE_COST;
        self.alloc_stats.record(kind);
    }

    // --- String allocation ---

    pub fn alloc_string(&mut self, s: String) -> StringId {
        // Check intern table for an existing live string with the same content
        if let Some(&existing_id) = self.intern_table.get(&s) {
            let slot = &self.strings.slots[existing_id.0 as usize];
            if slot.alive {
                return existing_id;
            }
            // Stale entry — will be overwritten below
        }

        self.tick_alloc(AllocKind::String, s.len() as u64);
        let id = StringId(self.strings.alloc(s.clone()));
        self.intern_table.insert(s, id);
        id
    }

    pub fn get_string(&self, id: StringId) -> &str {
        self.strings.get(id.0)
    }

    // --- List allocation ---

    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        self.tick_alloc(AllocKind::List, value_slice_bytes(elements.len()));
        ListId(self.lists.alloc(elements))
    }

    pub fn get_list(&self, id: ListId) -> &[Value] {
        self.lists.get(id.0)
    }

    pub fn list_len(&self, id: ListId) -> usize {
        self.lists.get(id.0).len()
    }

    // --- Immutable list operations (value semantics) ---
    //
    // These never mutate the input list; they allocate and return a new list.
    // Today they copy the backing `Vec`; once the backing becomes a persistent
    // structure the copy becomes a cheap structural-sharing operation and these
    // signatures stay the same.

    /// Return a new list equal to `id` with `val` appended. `id` is unchanged.
    pub fn list_append(&mut self, id: ListId, val: Value) -> ListId {
        let mut elements = self.lists.get(id.0).clone();
        self.dup_stats
            .record(DupKind::List, || value_slice_bytes(elements.len()));
        elements.push(val);
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with `elements[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds (eval already
    /// bounds-checks before calling).
    pub fn list_set(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        let mut elements = self.lists.get(id.0).clone();
        self.dup_stats
            .record(DupKind::List, || value_slice_bytes(elements.len()));
        elements[index] = val;
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with its last element removed. `id` is
    /// unchanged. On an empty list, returns a new empty list.
    pub fn list_drop_last(&mut self, id: ListId) -> ListId {
        let mut elements = self.lists.get(id.0).clone();
        self.dup_stats
            .record(DupKind::List, || value_slice_bytes(elements.len()));
        elements.pop();
        self.alloc_list(elements)
    }

    // --- In-place list operations (M4; escape-analysis-gated) ---
    //
    // These MUTATE the backing store of `id` and return the SAME id, breaking
    // the immutable-by-construction contract the COW methods uphold. They are
    // sound *only* when the caller has statically proven `id` is uniquely owned
    // and non-escaping — see `backend/bytecode/escape.rs` and the
    // `OptFlags::in_place_mutation` gate. Because no backing `Vec` is cloned,
    // they record no `DupKind` copy: the whole point of M4 is that the byte
    // counters fall. `id` must be a live heap root at the call (the analysis
    // guarantees it stays in a register), which the `debug_assert!` pins.

    /// In-place [`list_append`](Self::list_append): push `val` onto `id`'s
    /// backing store and return `id` unchanged. Amortized O(1), no copy.
    pub fn list_append_in_place(&mut self, id: ListId, val: Value) -> ListId {
        debug_assert!(
            self.lists.slots[id.0 as usize].alive,
            "in-place append on a dead list"
        );
        self.lists.get_mut(id.0).push(val);
        id
    }

    /// In-place [`list_set`](Self::list_set): overwrite `elements[index]` and
    /// return `id`. The caller must ensure `index` is in bounds.
    pub fn list_set_in_place(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        debug_assert!(
            self.lists.slots[id.0 as usize].alive,
            "in-place set on a dead list"
        );
        self.lists.get_mut(id.0)[index] = val;
        id
    }

    /// In-place [`list_drop_last`](Self::list_drop_last): pop `id`'s last
    /// element and return `id`. A no-op on an empty list.
    pub fn list_drop_last_in_place(&mut self, id: ListId) -> ListId {
        debug_assert!(
            self.lists.slots[id.0 as usize].alive,
            "in-place drop_last on a dead list"
        );
        self.lists.get_mut(id.0).pop();
        id
    }

    // --- F64 array allocation ---

    pub fn alloc_f64_array(&mut self, data: Vec<f64>) -> F64ArrayId {
        self.tick_alloc(
            AllocKind::F64Array,
            (data.len() * std::mem::size_of::<f64>()) as u64,
        );
        F64ArrayId(self.f64_arrays.alloc(data))
    }

    pub fn get_f64_array(&self, id: F64ArrayId) -> &[f64] {
        self.f64_arrays.get(id.0)
    }

    pub fn f64_array_len(&self, id: F64ArrayId) -> usize {
        self.f64_arrays.get(id.0).len()
    }

    /// Return a new f64 array equal to `id` with `data[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds.
    pub fn f64_array_set(&mut self, id: F64ArrayId, index: usize, val: f64) -> F64ArrayId {
        let mut data = self.f64_arrays.get(id.0).clone();
        self.dup_stats.record(DupKind::F64Array, || {
            (data.len() * std::mem::size_of::<f64>()) as u64
        });
        data[index] = val;
        self.alloc_f64_array(data)
    }

    /// Return a new f64 array equal to `id` with elements `i` and `j` swapped.
    /// `id` is unchanged. The caller must ensure `i` and `j` are in bounds.
    pub fn f64_array_swap(&mut self, id: F64ArrayId, i: usize, j: usize) -> F64ArrayId {
        let mut data = self.f64_arrays.get(id.0).clone();
        self.dup_stats.record(DupKind::F64Array, || {
            (data.len() * std::mem::size_of::<f64>()) as u64
        });
        data.swap(i, j);
        self.alloc_f64_array(data)
    }

    /// In-place [`f64_array_set`](Self::f64_array_set): overwrite `data[index]`
    /// and return `id`. Caller must ensure `index` is in bounds. See the
    /// in-place list methods for the soundness contract.
    pub fn f64_array_set_in_place(&mut self, id: F64ArrayId, index: usize, val: f64) -> F64ArrayId {
        debug_assert!(
            self.f64_arrays.slots[id.0 as usize].alive,
            "in-place set on a dead f64 array"
        );
        self.f64_arrays.get_mut(id.0)[index] = val;
        id
    }

    /// In-place [`f64_array_swap`](Self::f64_array_swap): swap elements `i` and
    /// `j` and return `id`. Caller must ensure both are in bounds.
    pub fn f64_array_swap_in_place(&mut self, id: F64ArrayId, i: usize, j: usize) -> F64ArrayId {
        debug_assert!(
            self.f64_arrays.slots[id.0 as usize].alive,
            "in-place swap on a dead f64 array"
        );
        self.f64_arrays.get_mut(id.0).swap(i, j);
        id
    }

    // --- Map allocation ---

    pub fn alloc_map(&mut self, entries: IndexMap<String, Value>) -> MapId {
        self.tick_alloc(AllocKind::Map, map_entries_bytes(&entries));
        MapId(self.maps.alloc(entries))
    }

    pub fn get_map(&self, id: MapId) -> &IndexMap<String, Value> {
        self.maps.get(id.0)
    }

    /// Return a new map equal to `id` with `key` set to `val`. `id` is
    /// unchanged (value semantics).
    pub fn map_set(&mut self, id: MapId, key: String, val: Value) -> MapId {
        let mut entries = self.maps.get(id.0).clone();
        self.dup_stats
            .record(DupKind::Map, || map_entries_bytes(&entries));
        entries.insert(key, val);
        self.alloc_map(entries)
    }

    /// Return a new map equal to `id` with `key` removed. `id` is unchanged
    /// (value semantics). Insertion order of the remaining keys is preserved.
    /// Removing an absent key returns an equivalent new map.
    pub fn map_remove(&mut self, id: MapId, key: &str) -> MapId {
        let mut entries = self.maps.get(id.0).clone();
        self.dup_stats
            .record(DupKind::Map, || map_entries_bytes(&entries));
        entries.shift_remove(key);
        self.alloc_map(entries)
    }

    /// In-place [`map_set`](Self::map_set): insert/overwrite `key` in `id`'s
    /// entry table and return `id`. See the in-place list methods for the
    /// soundness contract.
    pub fn map_set_in_place(&mut self, id: MapId, key: String, val: Value) -> MapId {
        debug_assert!(
            self.maps.slots[id.0 as usize].alive,
            "in-place set on a dead map"
        );
        self.maps.get_mut(id.0).insert(key, val);
        id
    }

    /// In-place [`map_remove`](Self::map_remove): shift-remove `key` from `id`
    /// (preserving order of the rest) and return `id`. A no-op for an absent key.
    pub fn map_remove_in_place(&mut self, id: MapId, key: &str) -> MapId {
        debug_assert!(
            self.maps.slots[id.0 as usize].alive,
            "in-place remove on a dead map"
        );
        self.maps.get_mut(id.0).shift_remove(key);
        id
    }

    // --- Element allocation ---

    pub fn alloc_element(&mut self, tag: StringId, props: MapId, children: ListId) -> ElementId {
        // Three `Copy` ids: no backing store of its own beyond the slot.
        self.tick_alloc(AllocKind::Element, 0);
        ElementId(self.elements.alloc(ElementPayload {
            tag,
            props,
            children,
        }))
    }

    pub fn get_element_tag(&self, id: ElementId) -> StringId {
        self.elements.get(id.0).tag
    }

    pub fn get_element_props(&self, id: ElementId) -> MapId {
        self.elements.get(id.0).props
    }

    pub fn get_element_children(&self, id: ElementId) -> ListId {
        self.elements.get(id.0).children
    }

    // --- Cell allocation (`var` bindings) ---

    /// Allocate a cell holding `init`. The returned id is the *identity* a
    /// `var` binding carries: capturing it in a closure shares the box, and
    /// [`cell_write`](Self::cell_write) is visible through every copy of the id.
    pub fn alloc_cell(&mut self, init: Value) -> CellId {
        // One `Copy` Value: no backing store of its own beyond the slot.
        self.tick_alloc(AllocKind::Cell, 0);
        CellId(self.cells.alloc(init))
    }

    /// Read a cell's current contents.
    pub fn cell_read(&self, id: CellId) -> Value {
        *self.cells.get(id.0)
    }

    /// Overwrite a cell's contents in place, keeping its id. The one mutating
    /// operation in this module — see [`CellId`] for why it is sound.
    pub fn cell_write(&mut self, id: CellId, val: Value) {
        debug_assert!(
            self.cells.slots[id.0 as usize].alive,
            "write to a collected cell"
        );
        *self.cells.get_mut(id.0) = val;
    }

    // -----------------------------------------------------------------------
    // Garbage collection: mark-and-sweep
    // -----------------------------------------------------------------------

    /// Mark a single value as reachable, recursively marking any heap objects it references.
    pub fn mark_value(&mut self, val: Value) {
        match val {
            Value::String(id) => self.mark_string(id),
            Value::List(id) => self.mark_list(id),
            Value::F64Array(id) => self.mark_f64_array(id),
            Value::Map(id) => self.mark_map(id),
            Value::Element(id) => self.mark_element(id),
            Value::Cell(id) => self.mark_cell(id),
            Value::EnumVariant { tag, data } => {
                self.mark_string(tag);
                self.mark_list(data);
            }
            // Non-heap values: nothing to mark. `Pending` is a thin id into the
            // resource table (not the heap); the table's own Ready/Errored
            // payloads are rooted separately.
            // TODO(pending): root resource-table payload Values in GC.
            Value::Nil
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Closure(_)
            | Value::OverloadSet(_)
            | Value::NativeFunction(_)
            | Value::Dual { .. }
            | Value::Vec2(_, _)
            | Value::Symbol(_)
            | Value::Handle(_)
            | Value::Pending(_) => {}
        }
    }

    fn mark_string(&mut self, id: StringId) {
        // Leaf: no children to recurse into.
        self.strings.mark(id.0);
    }

    fn mark_list(&mut self, id: ListId) {
        if self.lists.mark(id.0) {
            // Copy elements to avoid borrow conflict
            let elements: Vec<Value> = self.lists.get(id.0).clone();
            for val in elements {
                self.mark_value(val);
            }
        }
    }

    fn mark_f64_array(&mut self, id: F64ArrayId) {
        // Leaf: f64s are primitives — nothing recursive to mark.
        self.f64_arrays.mark(id.0);
    }

    fn mark_map(&mut self, id: MapId) {
        if self.maps.mark(id.0) {
            // Copy values to avoid borrow conflict
            let values: Vec<Value> = self.maps.get(id.0).values().copied().collect();
            for val in values {
                self.mark_value(val);
            }
        }
    }

    fn mark_element(&mut self, id: ElementId) {
        if self.elements.mark(id.0) {
            let e = *self.elements.get(id.0);
            self.mark_string(e.tag);
            self.mark_map(e.props);
            self.mark_list(e.children);
        }
    }

    fn mark_cell(&mut self, id: CellId) {
        if self.cells.mark(id.0) {
            // A cell's contents are an ordinary value and may themselves be
            // heap-backed (a `var` holding a list). The `mark` guard makes the
            // recursion terminate even if a cell ever reached itself.
            let contents = *self.cells.get(id.0);
            self.mark_value(contents);
        }
    }

    /// Sweep phase: free all unmarked objects and reset marks.
    /// Call this after marking all roots.
    pub fn sweep(&mut self) {
        // Reclaiming a string must also drop its interned entry. Destructure to
        // borrow `strings` and `intern_table` disjointly (the closure needs the
        // table while `sweep_with` holds `strings` mutably).
        let Self {
            strings,
            intern_table,
            ..
        } = self;
        //
        // Each reclaim *replaces* the payload rather than clearing it: an
        // emptied `Vec`/`String`/`IndexMap` keeps its buffer, which would leave
        // a swept 160 KB array squatting on 160 KB while it sits on the free
        // list — and `Slab::alloc` drops that buffer unread when it reuses the
        // slot, so nothing is gained by keeping it. See `Slab::sweep_with`.
        strings.sweep_with(|s| {
            intern_table.remove(s.as_str());
            *s = String::new();
        });

        self.lists.sweep_with(|v| *v = Vec::new());
        self.f64_arrays.sweep_with(|v| *v = Vec::new());
        self.maps.sweep_with(|v| *v = IndexMap::new());
        self.elements.sweep_with(|_| {});
        self.cells.sweep_with(|v| *v = Value::Nil);

        // Size the next collection's budget against what this collection would
        // cost to repeat (see `should_collect`). Computed here, once per cycle,
        // where an O(slots) walk is already being paid — never per allocation.
        self.alloc_charge = 0;
        self.gc_budget = GC_MIN_BUDGET_BYTES.max(GC_HEAP_GROWTH * self.collection_cost());
        self.collections += 1;
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_append_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_list(vec![Value::Int(1), Value::Int(2)]);

        let grown = heap.list_append(original, Value::Int(3));

        // A new, distinct list is returned with the extra element…
        assert_ne!(original.0, grown.0);
        assert_eq!(
            heap.get_list(grown),
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        // …and the original list is untouched (value semantics).
        assert_eq!(heap.get_list(original), &[Value::Int(1), Value::Int(2)]);
    }

    #[test]
    fn list_append_to_empty_list() {
        let mut heap = Heap::new();
        let empty = heap.alloc_list(vec![]);
        let one = heap.list_append(empty, Value::Int(42));
        assert_eq!(heap.get_list(empty), &[] as &[Value]);
        assert_eq!(heap.get_list(one), &[Value::Int(42)]);
    }

    #[test]
    fn list_set_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);

        let updated = heap.list_set(original, 0, Value::Int(99));

        // A new, distinct list is returned with the element replaced…
        assert_ne!(original.0, updated.0);
        assert_eq!(
            heap.get_list(updated),
            &[Value::Int(99), Value::Int(2), Value::Int(3)]
        );
        // …and the original list is untouched (value semantics).
        assert_eq!(
            heap.get_list(original),
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        );
    }

    #[test]
    fn map_set_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let mut entries = IndexMap::new();
        entries.insert("a".to_string(), Value::Int(1));
        entries.insert("b".to_string(), Value::Int(2));
        let original = heap.alloc_map(entries);

        let updated = heap.map_set(original, "a".to_string(), Value::Int(99));

        // A new, distinct map is returned with the key updated…
        assert_ne!(original.0, updated.0);
        assert_eq!(heap.get_map(updated).get("a"), Some(&Value::Int(99)));
        assert_eq!(heap.get_map(updated).get("b"), Some(&Value::Int(2)));
        // …and the original map is untouched (value semantics).
        assert_eq!(heap.get_map(original).get("a"), Some(&Value::Int(1)));
        assert_eq!(heap.get_map(original).get("b"), Some(&Value::Int(2)));
    }

    #[test]
    fn map_set_can_add_a_new_key() {
        let mut heap = Heap::new();
        let mut entries = IndexMap::new();
        entries.insert("a".to_string(), Value::Int(1));
        let original = heap.alloc_map(entries);

        let updated = heap.map_set(original, "b".to_string(), Value::Int(2));

        assert_eq!(heap.get_map(updated).get("b"), Some(&Value::Int(2)));
        // Original is unchanged: the new key is not present.
        assert_eq!(heap.get_map(original).get("b"), None);
    }

    #[test]
    fn f64_array_set_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_f64_array(vec![1.0, 2.0, 3.0]);

        let updated = heap.f64_array_set(original, 1, 9.5);

        // A new, distinct array is returned with the element replaced…
        assert_ne!(original.0, updated.0);
        assert_eq!(heap.get_f64_array(updated), &[1.0, 9.5, 3.0]);
        // …and the original array is untouched (value semantics).
        assert_eq!(heap.get_f64_array(original), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn list_drop_last_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);

        let shorter = heap.list_drop_last(original);

        // A new, distinct list is returned without the last element…
        assert_ne!(original.0, shorter.0);
        assert_eq!(heap.get_list(shorter), &[Value::Int(1), Value::Int(2)]);
        // …and the original list is untouched (value semantics).
        assert_eq!(
            heap.get_list(original),
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        );
    }

    #[test]
    fn list_drop_last_on_empty_list() {
        let mut heap = Heap::new();
        let empty = heap.alloc_list(vec![]);
        let still_empty = heap.list_drop_last(empty);
        assert_eq!(heap.get_list(still_empty), &[] as &[Value]);
    }

    #[test]
    fn f64_array_swap_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_f64_array(vec![1.0, 2.0, 3.0]);

        let swapped = heap.f64_array_swap(original, 0, 2);

        // A new, distinct array is returned with the two elements swapped…
        assert_ne!(original.0, swapped.0);
        assert_eq!(heap.get_f64_array(swapped), &[3.0, 2.0, 1.0]);
        // …and the original array is untouched (value semantics).
        assert_eq!(heap.get_f64_array(original), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn map_remove_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let mut entries = IndexMap::new();
        entries.insert("a".to_string(), Value::Int(1));
        entries.insert("b".to_string(), Value::Int(2));
        let original = heap.alloc_map(entries);

        let removed = heap.map_remove(original, "a");

        // A new, distinct map is returned without the key…
        assert_ne!(original.0, removed.0);
        assert_eq!(heap.get_map(removed).get("a"), None);
        assert_eq!(heap.get_map(removed).get("b"), Some(&Value::Int(2)));
        // …and the original map is untouched (value semantics).
        assert_eq!(heap.get_map(original).get("a"), Some(&Value::Int(1)));
        assert_eq!(heap.get_map(original).get("b"), Some(&Value::Int(2)));
    }

    #[test]
    fn a_cell_write_is_visible_through_every_copy_of_its_id() {
        // The one mutable object in the heap: a second holder of the id — which
        // is what a closure capture is — observes the write.
        let mut heap = Heap::new();
        let cell = heap.alloc_cell(Value::Int(1));
        let captured = cell;

        heap.cell_write(cell, Value::Int(2));

        assert_eq!(heap.cell_read(captured), Value::Int(2));
    }

    #[test]
    fn gc_traces_through_a_cell_into_its_contents() {
        // A `var` holding a list is the list's only root. Marking the cell has
        // to reach the payload, or the list is swept out from under it.
        let mut heap = Heap::new();
        let list = heap.alloc_list(vec![Value::Int(7)]);
        let cell = heap.alloc_cell(Value::List(list));

        heap.mark_value(Value::Cell(cell));
        heap.sweep();

        assert_eq!(heap.get_list(list), &[Value::Int(7)]);
    }

    #[test]
    fn an_unreachable_cell_is_reclaimed_with_its_contents() {
        let mut heap = Heap::new();
        let list = heap.alloc_list(vec![Value::Int(7)]);
        let cell = heap.alloc_cell(Value::List(list));

        // Nothing marked: both the cell and the list it holds are garbage.
        heap.sweep();

        assert!(!heap.cells.slots[cell.0 as usize].alive);
        assert!(!heap.lists.slots[list.0 as usize].alive);
    }

    #[test]
    fn a_fork_writes_its_own_copy_of_a_cell() {
        // Speculative execution must not be able to reach back and mutate the
        // parent's `var`s — `Heap::fork` deep-copies the cell slab, so it can't.
        let mut parent = Heap::new();
        let cell = parent.alloc_cell(Value::Int(1));

        let mut child = parent.fork();
        child.cell_write(cell, Value::Int(99));

        assert_eq!(child.cell_read(cell), Value::Int(99));
        assert_eq!(parent.cell_read(cell), Value::Int(1));
    }

    #[test]
    fn fork_yields_an_isolated_heap_sharing_pre_fork_objects() {
        let mut parent = Heap::new();
        let shared = parent.alloc_list(vec![Value::Int(1), Value::Int(2)]);

        let mut child = parent.fork();

        // A pre-fork object is visible and equal in both heaps.
        assert_eq!(
            child.get_list(shared),
            &[Value::Int(1), Value::Int(2)],
            "fork should preserve pre-fork objects under their original ids"
        );

        // An immutable "mutation" in the child allocates a new id; the parent's
        // pre-fork object is untouched.
        let grown = child.list_append(shared, Value::Int(3));
        assert_eq!(
            child.get_list(grown),
            &[Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        assert_eq!(
            parent.get_list(shared),
            &[Value::Int(1), Value::Int(2)],
            "child mutation leaked into the parent heap"
        );

        // Fresh allocations on each side are independent and land in their own
        // heap only: the parent never sees the child's new object.
        let child_only = child.alloc_list(vec![Value::Int(9)]);
        let parent_only = parent.alloc_list(vec![Value::Int(8)]);
        assert_eq!(child.get_list(child_only), &[Value::Int(9)]);
        assert_eq!(parent.get_list(parent_only), &[Value::Int(8)]);
    }

    #[test]
    fn map_remove_absent_key_is_a_noop_copy() {
        let mut heap = Heap::new();
        let mut entries = IndexMap::new();
        entries.insert("a".to_string(), Value::Int(1));
        let original = heap.alloc_map(entries);

        let removed = heap.map_remove(original, "missing");

        assert_eq!(heap.get_map(removed).get("a"), Some(&Value::Int(1)));
        assert_eq!(heap.get_map(removed).len(), 1);
    }

    // The dup-stats assertions below only hold when collection is compiled in
    // (debug builds, which `cargo test` is, or the `dup-stats` feature).
    #[test]
    fn dup_stats_count_cow_operations() {
        if !crate::stats::DUP_STATS_ENABLED {
            return;
        }
        use crate::stats::DupKind;
        let mut heap = Heap::new();
        let list = heap.alloc_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);

        let _ = heap.list_append(list, Value::Int(4));
        let _ = heap.list_set(list, 0, Value::Int(9));

        let stats = heap.dup_stats();
        assert_eq!(stats.get(DupKind::List).count, 2);
        // Each clone copied the 3-element backing store.
        assert_eq!(stats.get(DupKind::List).bytes, 2 * value_slice_bytes(3),);
        assert_eq!(stats.total_count(), 2);
    }

    #[test]
    fn alloc_stats_count_new_objects_per_kind() {
        if !crate::stats::DUP_STATS_ENABLED {
            return;
        }
        use crate::stats::AllocKind;
        let mut heap = Heap::new();

        let list = heap.alloc_list(vec![Value::Int(1)]);
        let _ = heap.alloc_list(vec![Value::Int(2)]);
        let _ = heap.alloc_f64_array(vec![0.0; 3]);
        // A copy-on-write also allocates a fresh list.
        let _ = heap.list_append(list, Value::Int(9));

        let allocs = heap.alloc_stats();
        assert_eq!(allocs.get(AllocKind::List), 3); // two literals + the append's result
        assert_eq!(allocs.get(AllocKind::F64Array), 1);
        assert_eq!(allocs.get(AllocKind::Map), 0);
        assert_eq!(allocs.total(), 4);
    }

    #[test]
    fn interned_string_reuse_is_not_a_new_allocation() {
        if !crate::stats::DUP_STATS_ENABLED {
            return;
        }
        use crate::stats::AllocKind;
        let mut heap = Heap::new();
        let _ = heap.alloc_string("hello".to_string());
        let _ = heap.alloc_string("hello".to_string()); // interned — reuses the slot

        assert_eq!(heap.alloc_stats().get(AllocKind::String), 1);
    }

    #[test]
    fn fork_records_one_duplication_on_the_child() {
        if !crate::stats::DUP_STATS_ENABLED {
            return;
        }
        use crate::stats::DupKind;
        let mut parent = Heap::new();
        // Give the parent some COW history; the fork must not inherit it.
        let list = parent.alloc_list(vec![Value::Int(1), Value::Int(2)]);
        let _ = parent.list_append(list, Value::Int(3));
        assert_eq!(parent.dup_stats().get(DupKind::List).count, 1);

        let child = parent.fork();

        // The child starts fresh and records exactly the fork that birthed it.
        assert_eq!(child.dup_stats().get(DupKind::List).count, 0);
        assert_eq!(child.dup_stats().get(DupKind::Fork).count, 1);
        assert_eq!(child.dup_stats().total_count(), 1);
        // The parent's own counters are untouched by the fork.
        assert_eq!(parent.dup_stats().get(DupKind::Fork).count, 0);
        assert_eq!(parent.dup_stats().get(DupKind::List).count, 1);
    }
}
