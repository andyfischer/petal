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
//!
//! ## Ids are generational
//!
//! Every id ([`ListId`], [`StringId`], ..., and the closure table's
//! `ClosureId`/`OverloadSetId`) is a `(slot index, generation)` pair naming one
//! *allocation*. Reusing a reclaimed slot bumps its generation, so an id that
//! outlives its object never compares equal to the slot's new occupant, and
//! [`Heap::is_live`] can tell that it is stale.
//!
//! The rule for anything that keeps an id past the instruction that produced
//! it: **an id held across a collection must either be a GC root (strong; list
//! it in `Env::collect_garbage`) or be checked with [`Heap::is_live`] before
//! every dereference (weak).** The execution trace buffer is the model weak
//! holder. Dereferencing a stale id is checked only in debug builds.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::program::{ClosureId, OverloadSetId};
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

/// The raw `(slot index, generation)` pair a [`Slab`] hands out. Wrapped by
/// every typed id ([`ListId`], [`ClosureId`], ...) through
/// [`generational_id!`]; only a slab builds one, so no code outside the stores
/// can conjure an id from bare numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawId {
    index: u32,
    generation: u32,
}

/// Define a typed generational id: a `(index, generation)` pair naming *one
/// allocation*, not one slot. A slot that is reclaimed and reused gets a new
/// generation, so a stale id never compares equal to (or passes
/// [`Slab::is_live`] for) whatever lives in the slot now. See the module docs.
///
/// `Debug` prints `ListId(7#2)` (index `#` generation).
macro_rules! generational_id {
    ($(#[$meta:meta])* pub struct $name:ident;) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name($crate::heap::RawId);

        impl $name {
            /// The slot index, for display and debugging only. Two ids with
            /// the same index are not the same object unless their
            /// generations match too.
            pub fn index(self) -> u32 {
                self.0.index()
            }

            /// The slot generation this id was minted with.
            pub fn generation(self) -> u32 {
                self.0.generation()
            }

            pub(crate) fn from_raw(raw: $crate::heap::RawId) -> Self {
                Self(raw)
            }

            pub(crate) fn raw(self) -> $crate::heap::RawId {
                self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({}#{})", stringify!($name), self.index(), self.generation())
            }
        }
    };
}
pub(crate) use generational_id;

impl RawId {
    pub(crate) fn index(self) -> u32 {
        self.index
    }

    pub(crate) fn generation(self) -> u32 {
        self.generation
    }
}

generational_id! {
    /// Opaque handle to a heap-allocated string.
    pub struct StringId;
}

generational_id! {
    /// Opaque handle to a heap-allocated list.
    pub struct ListId;
}

generational_id! {
    /// Opaque handle to a heap-allocated flat f64 array.
    pub struct F64ArrayId;
}

generational_id! {
    /// Opaque handle to a heap-allocated map.
    pub struct MapId;
}

generational_id! {
    /// Opaque handle to a heap-allocated element.
    pub struct ElementId;
}

generational_id! {
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
    /// docs/var.md (Containment).
    pub struct CellId;
}

/// Payload of a single heap map: its entry table plus an optional **class
/// tag**.
///
/// The tag is what makes a record an *instance*: `Rect(0, 0, 10, 4)` allocates
/// an ordinary entry table and stamps it with the interned name `"Rect"`, so
/// the value keeps behaving exactly like `{x: 0, y: 0, w: 10, h: 4}` for every
/// record operation while method dispatch and `type()` can still tell which
/// class it came from. See `crate::classes`.
///
/// The tag rides along through the copy-on-write updates that keep an
/// instance's identity (`map_set`, `map_remove` — a field write on a `Rect` is
/// still a `Rect`) and is *not* carried by anything that builds a fresh record
/// from parts (a `{...r, extra: 1}` spread allocates an untagged record,
/// because the result is no longer that class's shape).
#[derive(Clone)]
struct MapObj {
    entries: IndexMap<String, Value>,
    /// The interned class name, or `None` for a plain record. Marked by the
    /// collector so the name outlives every instance that carries it.
    class: Option<StringId>,
}

/// Payload of a single heap element: three `Copy` ids referencing the element's
/// tag string, props map, and children list. Stored as the `T` of an element
/// slab; the `gc_mark`/`alive` bits live in the enclosing [`Slot`].
#[derive(Clone, Copy)]
struct ElementPayload {
    tag: StringId,
    props: MapId,
    children: ListId,
}

/// One slab slot: a payload plus its GC bits and generation. `gc_mark` is the
/// mark-and-sweep reachability flag (cleared each sweep); `alive` is false for a
/// reclaimed slot sitting on the free list.
#[derive(Clone)]
pub(crate) struct Slot<T> {
    data: T,
    /// Generation of the object currently (or most recently) in this slot. An
    /// id is live iff `alive` and its generation equals this.
    generation: u32,
    /// Highest generation ever issued for this slot. Usually equal to
    /// `generation`; it runs ahead when [`Slab::inherit_generations`] merges in
    /// another timeline's history, so the next reuse still issues a generation
    /// neither timeline has handed out.
    high_water: u32,
    gc_mark: bool,
    alive: bool,
}

/// A generic slot store with an index free list. Backs each of the heap's object
/// kinds, and the closure table's.
///
/// Ids are generational ([`RawId`]): a reclaimed slot is reused, but reuse bumps
/// its generation, so an id minted before the reuse no longer matches. That
/// makes stale ids detectable ([`is_live`](Self::is_live)) and never equal to a
/// live one. It does *not* make dereferencing them safe for free: in release
/// builds [`get`](Self::get) does not check. An id held across a collection
/// must either be a GC root (strong) or be checked with `is_live` before every
/// dereference (weak).
#[derive(Clone)]
pub(crate) struct Slab<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Slab<T> {
    pub(crate) fn new() -> Self {
        Slab {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Allocate `data` into a reused free slot or a fresh one; return its id.
    ///
    /// A reused slot's generation advances past its high-water mark. A slot
    /// whose high-water mark has reached `u32::MAX` is *retired*: it is kept
    /// off the free list for good rather than wrapped, so the no-aliasing
    /// guarantee is unconditional.
    #[inline]
    pub(crate) fn alloc(&mut self, data: T) -> RawId {
        if let Some(idx) = self.free.pop() {
            let slot = &mut self.slots[idx as usize];
            // Exhausted slots never reach the free list (`sweep_with` and
            // `inherit_generations` keep them off it), so this cannot wrap.
            debug_assert!(slot.high_water < u32::MAX, "retired slot on the free list");
            let generation = slot.high_water + 1;
            slot.data = data;
            slot.generation = generation;
            slot.high_water = generation;
            slot.gc_mark = false;
            slot.alive = true;
            return RawId {
                index: idx,
                generation,
            };
        }
        let idx = self.slots.len() as u32;
        self.slots.push(Slot {
            data,
            generation: 0,
            high_water: 0,
            gc_mark: false,
            alive: true,
        });
        RawId {
            index: idx,
            generation: 0,
        }
    }

    /// How many slots exist, live or free — what a collection has to walk.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// How many slots are live (allocated and not yet reclaimed). Counted
    /// directly rather than as `slots - free`: a slot is free the moment it is
    /// reclaimed, whether or not it is currently on the list.
    pub(crate) fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.alive).count()
    }

    /// Whether `id` still names the object in its slot: the slot is allocated
    /// and has not been reclaimed-and-reused since `id` was minted.
    #[inline]
    pub(crate) fn is_live(&self, id: RawId) -> bool {
        self.slots
            .get(id.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == id.generation)
    }

    /// The payload `id` names, or `None` if it has been collected. For holders
    /// of weak ids.
    pub(crate) fn try_get(&self, id: RawId) -> Option<&T> {
        let slot = self.slots.get(id.index as usize)?;
        (slot.alive && slot.generation == id.generation).then_some(&slot.data)
    }

    /// The payload `id` names. `id` must be live — a strong (rooted) id, or a
    /// weak one already checked with [`is_live`](Self::is_live). Checked in
    /// debug builds only; the release hot path is a bounds check.
    #[inline]
    pub(crate) fn get(&self, id: RawId) -> &T {
        let slot = &self.slots[id.index as usize];
        debug_assert!(
            slot.alive && slot.generation == id.generation,
            "stale heap id {id:?} (slot alive: {}, generation {})",
            slot.alive,
            slot.generation
        );
        &slot.data
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: RawId) -> &mut T {
        let slot = &mut self.slots[id.index as usize];
        debug_assert!(
            slot.alive && slot.generation == id.generation,
            "stale heap id {id:?} (slot alive: {}, generation {})",
            slot.alive,
            slot.generation
        );
        &mut slot.data
    }

    /// Mark `id` live. Returns true iff it was newly marked (live and not
    /// already marked) — the caller then recurses into the payload's children.
    /// A stale id marks nothing: it must not resurrect the slot's new occupant.
    pub(crate) fn mark(&mut self, id: RawId) -> bool {
        let slot = &mut self.slots[id.index as usize];
        if slot.alive && slot.generation == id.generation && !slot.gc_mark {
            slot.gc_mark = true;
            true
        } else {
            false
        }
    }

    /// Sweep: reclaim every unmarked-live slot (flip alive off, run `on_reclaim`
    /// on its id and payload to release backing memory / side-table entries,
    /// push to the free list); clear the mark on every surviving slot. Rebuilds
    /// `free` from *every* dead slot, not just this cycle's — a slot reclaimed
    /// by an earlier sweep and not yet reused is still free, and dropping it
    /// from the list would orphan it for the rest of the run (the slot vector
    /// would then grow monotonically no matter how much was collected). Retired
    /// slots (see [`alloc`](Self::alloc)) stay off the list.
    ///
    /// Generations do not change here: bumping happens at reuse, and a
    /// swept-but-unreused slot already fails `is_live` through `alive`.
    ///
    /// `on_reclaim` must *release* the payload's heap allocation, not merely
    /// empty it: `Vec::clear()` keeps the buffer, so a swept 160 KB array would
    /// sit on the free list still holding its 160 KB. That memory can never be
    /// reused for anything either, because [`alloc`](Self::alloc) overwrites
    /// `slot.data` wholesale (dropping whatever buffer was there). Assign a
    /// fresh empty value (`*v = Vec::new()`) instead.
    pub(crate) fn sweep_with(&mut self, mut on_reclaim: impl FnMut(RawId, &mut T)) {
        self.free.clear();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                    continue;
                }
                slot.alive = false;
                let id = RawId {
                    index: i as u32,
                    generation: slot.generation,
                };
                on_reclaim(id, &mut slot.data);
            }
            // Dead now or from an earlier sweep and never reused: free, unless
            // its generations are exhausted.
            if slot.high_water != u32::MAX {
                self.free.push(i as u32);
            }
        }
    }

    /// Reclaim every slot, keeping the generation history — the difference
    /// from replacing the slab with [`Slab::new`], which would let a fresh
    /// allocation reissue an id some holder still has.
    pub(crate) fn clear_with(&mut self, on_reclaim: impl FnMut(RawId, &mut T)) {
        for slot in &mut self.slots {
            slot.gc_mark = false;
        }
        self.sweep_with(on_reclaim);
    }

    /// Raise every slot's high-water mark to at least `other`'s, extending with
    /// dead slots (payload from `filler`) where `other` has more. Live objects
    /// keep their current generations, but no future reuse will issue a
    /// generation either slab has already handed out.
    ///
    /// Used when this slab *replaces* `other` under the same owner (restoring a
    /// snapshot over a live execution): ids a host kept from `other` must not
    /// come back to life when the restored slab reuses their slots.
    pub(crate) fn inherit_generations(&mut self, other: &Slab<T>, mut filler: impl FnMut() -> T) {
        let mut extra = Vec::new();
        for (i, theirs) in other.slots.iter().enumerate() {
            match self.slots.get_mut(i) {
                Some(slot) => slot.high_water = slot.high_water.max(theirs.high_water),
                None => {
                    self.slots.push(Slot {
                        data: filler(),
                        generation: theirs.high_water,
                        high_water: theirs.high_water,
                        gc_mark: false,
                        alive: false,
                    });
                    if theirs.high_water != u32::MAX {
                        extra.push(i as u32);
                    }
                }
            }
        }
        // Put the new dead slots at the bottom of the free list, so the
        // restored slab keeps reusing slots in the order the snapshot would.
        if !extra.is_empty() {
            extra.reverse();
            extra.extend(self.free.iter().copied());
            self.free = extra;
        }
        self.free.retain(|&i| self.slots[i as usize].high_water != u32::MAX);
    }
}

#[derive(Clone)]
pub struct Heap {
    strings: Slab<String>,
    lists: Slab<Vec<Value>>,
    f64_arrays: Slab<Vec<f64>>,
    maps: Slab<MapObj>,
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
    /// Closures and overload sets seen while marking, for the collector to
    /// follow — the heap's half of a mark that spans two stores.
    ///
    /// A `Value::Closure`/`Value::OverloadSet` is an id into the owning
    /// context's [`ClosureTable`](crate::closure_table::ClosureTable), which
    /// the heap cannot reach, so marking one records it here instead. The
    /// collector ([`Env::collect_garbage`](crate::env::Env)) drains this
    /// "gray set", marks those entries in the table, and feeds their captures
    /// back through [`mark_value`](Self::mark_value) until nothing new turns
    /// up — the two stores reach a joint fixpoint before either is swept.
    gray_closures: Vec<ClosureId>,
    gray_overload_sets: Vec<OverloadSetId>,
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
            gray_closures: Vec::new(),
            gray_overload_sets: Vec::new(),
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
            .map(|m| value_slice_bytes(m.data.entries.capacity()))
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
            .map(|m| map_entries_bytes(&m.data.entries))
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

    /// Take the closures and overload sets marking has run into so far, leaving
    /// the gray set empty. The collector calls this in a loop: each batch it
    /// marks may mark more values, which may turn up more closures, until a
    /// round comes back empty. See [`gray_closures`](Self::gray_closures).
    pub fn take_gray(&mut self) -> (Vec<ClosureId>, Vec<OverloadSetId>) {
        (
            std::mem::take(&mut self.gray_closures),
            std::mem::take(&mut self.gray_overload_sets),
        )
    }

    /// Charge the collector budget for an object allocated *outside* the heap
    /// but collected with it — a closure or an overload set in the context's
    /// [`ClosureTable`](crate::closure_table::ClosureTable). Without this a
    /// program whose only churn is closures (every frame of a panel script
    /// re-runs its `fn` declarations) would never reach
    /// [`should_collect`](Self::should_collect) and never reclaim them.
    pub fn charge_external_alloc(&mut self, payload_bytes: u64) {
        self.alloc_charge += payload_bytes + SLOT_TRACE_COST;
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

    /// Make this heap safe to install in place of `previous` under the same
    /// execution context: every slab's generation history becomes at least
    /// `previous`'s (see [`Slab::inherit_generations`]).
    ///
    /// `restore_execution` swaps a snapshot's heap in for a live one but keeps
    /// the context key, so ids a host read from the live heap stay in its
    /// hands. Without this, the snapshot could reuse one of their slots at a
    /// generation the live heap already issued, and the old id would alias the
    /// new object.
    pub fn inherit_generations(&mut self, previous: &Heap) {
        // Filler payloads for slots that exist only in `previous`. They are
        // dead, never read, and overwritten on reuse.
        let placeholder_id = RawId {
            index: 0,
            generation: 0,
        };
        self.strings.inherit_generations(&previous.strings, String::new);
        self.lists.inherit_generations(&previous.lists, Vec::new);
        self.f64_arrays
            .inherit_generations(&previous.f64_arrays, Vec::new);
        self.maps.inherit_generations(&previous.maps, || MapObj {
            entries: IndexMap::new(),
            class: None,
        });
        self.elements
            .inherit_generations(&previous.elements, || ElementPayload {
                tag: StringId::from_raw(placeholder_id),
                props: MapId::from_raw(placeholder_id),
                children: ListId::from_raw(placeholder_id),
            });
        self.cells.inherit_generations(&previous.cells, || Value::Nil);
    }

    /// Whether every heap object `v` references directly is still live — the
    /// check a *weak* holder (one that is not a GC root) makes before
    /// dereferencing. True for non-heap values. An `EnumVariant` checks both
    /// its tag and its payload list.
    ///
    /// Closures and overload sets live in the context's
    /// [`ClosureTable`](crate::closure_table::ClosureTable), which this heap
    /// cannot see, so they read as live here; ask
    /// [`ExecutionContext::is_live`](crate::execution_context::ExecutionContext::is_live)
    /// for an answer that covers them too.
    ///
    /// Shallow on purpose: the collector marks everything a live object
    /// references, so a live list's elements are live too.
    pub fn is_live(&self, v: Value) -> bool {
        match v {
            Value::String(id) => self.strings.is_live(id.raw()),
            Value::List(id) => self.lists.is_live(id.raw()),
            Value::F64Array(id) => self.f64_arrays.is_live(id.raw()),
            Value::Map(id) => self.maps.is_live(id.raw()),
            Value::Element(id) => self.elements.is_live(id.raw()),
            Value::Cell(id) => self.cells.is_live(id.raw()),
            Value::EnumVariant { tag, data } => {
                self.strings.is_live(tag.raw()) && self.lists.is_live(data.raw())
            }
            Value::Closure(_)
            | Value::OverloadSet(_)
            | Value::Nil
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::NativeFunction(_)
            | Value::Dual { .. }
            | Value::Vec2(_, _)
            | Value::Symbol(_)
            | Value::Handle(_)
            | Value::Pending(_) => true,
        }
    }

    /// [`get_string`](Self::get_string) for a weak id: `None` once collected.
    pub fn try_get_string(&self, id: StringId) -> Option<&str> {
        self.strings.try_get(id.raw()).map(String::as_str)
    }

    /// [`get_list`](Self::get_list) for a weak id: `None` once collected.
    pub fn try_get_list(&self, id: ListId) -> Option<&[Value]> {
        self.lists.try_get(id.raw()).map(Vec::as_slice)
    }

    /// [`get_map`](Self::get_map) for a weak id: `None` once collected.
    pub fn try_get_map(&self, id: MapId) -> Option<&IndexMap<String, Value>> {
        self.maps.try_get(id.raw()).map(|m| &m.entries)
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

    /// The id already interned for this content, if it is still live. A stale
    /// entry (the string was collected) reads as a miss and is overwritten by
    /// the next [`insert_interned`](Self::insert_interned).
    fn interned(&self, s: &str) -> Option<StringId> {
        let id = *self.intern_table.get(s)?;
        self.strings.is_live(id.raw()).then_some(id)
    }

    /// Allocate `s` and index it in the intern table. The caller must have
    /// missed [`interned`](Self::interned) first — this always allocates.
    fn insert_interned(&mut self, s: String) -> StringId {
        self.tick_alloc(AllocKind::String, s.len() as u64);
        let id = StringId::from_raw(self.strings.alloc(s.clone()));
        self.intern_table.insert(s, id);
        id
    }

    /// Intern an owned string. Prefer [`intern_str`](Self::intern_str) when the
    /// content is borrowed: this signature forces the allocation before the
    /// intern table can say whether it was needed.
    pub fn alloc_string(&mut self, s: String) -> StringId {
        match self.interned(&s) {
            Some(id) => id,
            None => self.insert_interned(s),
        }
    }

    /// Intern a borrowed string, allocating only on a miss.
    ///
    /// Every `LoadConst` of a string literal, and every builtin that returns
    /// text it already has in hand, goes through here. On a hot loop over
    /// string literals, [`alloc_string`](Self::alloc_string) is a malloc and a
    /// free per instruction for a value the heap already holds; taking `&str`
    /// moves the allocation to the miss path.
    pub fn intern_str(&mut self, s: &str) -> StringId {
        match self.interned(s) {
            Some(id) => id,
            None => self.insert_interned(s.to_string()),
        }
    }

    /// Intern the byte range `start..end` of an existing heap string without
    /// materializing it first. `slice()` over text is the motivating caller:
    /// a scanner walking a string one character at a time asks for substrings
    /// that are, almost without exception, already interned — so the common
    /// case here is a hash lookup with no allocation at all.
    ///
    /// The caller must pass char-boundary offsets (as `slice()` does); an
    /// interior byte offset panics exactly as `&str` indexing would.
    pub fn intern_substring(&mut self, id: StringId, start: usize, end: usize) -> StringId {
        let sub = &self.strings.get(id.raw())[start..end];
        match self.interned(sub) {
            Some(existing) => existing,
            // Miss: take ownership, which ends the borrow of `strings`.
            None => {
                let owned = sub.to_string();
                self.insert_interned(owned)
            }
        }
    }

    pub fn get_string(&self, id: StringId) -> &str {
        self.strings.get(id.raw())
    }

    // --- List allocation ---

    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        self.tick_alloc(AllocKind::List, value_slice_bytes(elements.len()));
        ListId::from_raw(self.lists.alloc(elements))
    }

    pub fn get_list(&self, id: ListId) -> &[Value] {
        self.lists.get(id.raw())
    }

    pub fn list_len(&self, id: ListId) -> usize {
        self.lists.get(id.raw()).len()
    }

    // --- Immutable list operations (value semantics) ---
    //
    // These never mutate the input list; they allocate and return a new list.
    // Today they copy the backing `Vec`; once the backing becomes a persistent
    // structure the copy becomes a cheap structural-sharing operation and these
    // signatures stay the same.

    /// Return a new list equal to `id` with `val` appended. `id` is unchanged.
    pub fn list_append(&mut self, id: ListId, val: Value) -> ListId {
        let mut elements = self.lists.get(id.raw()).clone();
        self.dup_stats
            .record(DupKind::List, || value_slice_bytes(elements.len()));
        elements.push(val);
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with `elements[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds (eval already
    /// bounds-checks before calling).
    pub fn list_set(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        let mut elements = self.lists.get(id.raw()).clone();
        self.dup_stats
            .record(DupKind::List, || value_slice_bytes(elements.len()));
        elements[index] = val;
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with its last element removed. `id` is
    /// unchanged. On an empty list, returns a new empty list.
    pub fn list_drop_last(&mut self, id: ListId) -> ListId {
        let mut elements = self.lists.get(id.raw()).clone();
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
            self.lists.is_live(id.raw()),
            "in-place append on a dead list"
        );
        self.lists.get_mut(id.raw()).push(val);
        id
    }

    /// In-place [`list_set`](Self::list_set): overwrite `elements[index]` and
    /// return `id`. The caller must ensure `index` is in bounds.
    pub fn list_set_in_place(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        debug_assert!(
            self.lists.is_live(id.raw()),
            "in-place set on a dead list"
        );
        self.lists.get_mut(id.raw())[index] = val;
        id
    }

    /// In-place [`list_drop_last`](Self::list_drop_last): pop `id`'s last
    /// element and return `id`. A no-op on an empty list.
    pub fn list_drop_last_in_place(&mut self, id: ListId) -> ListId {
        debug_assert!(
            self.lists.is_live(id.raw()),
            "in-place drop_last on a dead list"
        );
        self.lists.get_mut(id.raw()).pop();
        id
    }

    // --- F64 array allocation ---

    pub fn alloc_f64_array(&mut self, data: Vec<f64>) -> F64ArrayId {
        self.tick_alloc(
            AllocKind::F64Array,
            (data.len() * std::mem::size_of::<f64>()) as u64,
        );
        F64ArrayId::from_raw(self.f64_arrays.alloc(data))
    }

    pub fn get_f64_array(&self, id: F64ArrayId) -> &[f64] {
        self.f64_arrays.get(id.raw())
    }

    pub fn f64_array_len(&self, id: F64ArrayId) -> usize {
        self.f64_arrays.get(id.raw()).len()
    }

    /// Return a new f64 array equal to `id` with `data[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds.
    pub fn f64_array_set(&mut self, id: F64ArrayId, index: usize, val: f64) -> F64ArrayId {
        let mut data = self.f64_arrays.get(id.raw()).clone();
        self.dup_stats.record(DupKind::F64Array, || {
            (data.len() * std::mem::size_of::<f64>()) as u64
        });
        data[index] = val;
        self.alloc_f64_array(data)
    }

    /// Return a new f64 array equal to `id` with elements `i` and `j` swapped.
    /// `id` is unchanged. The caller must ensure `i` and `j` are in bounds.
    pub fn f64_array_swap(&mut self, id: F64ArrayId, i: usize, j: usize) -> F64ArrayId {
        let mut data = self.f64_arrays.get(id.raw()).clone();
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
            self.f64_arrays.is_live(id.raw()),
            "in-place set on a dead f64 array"
        );
        self.f64_arrays.get_mut(id.raw())[index] = val;
        id
    }

    /// In-place [`f64_array_swap`](Self::f64_array_swap): swap elements `i` and
    /// `j` and return `id`. Caller must ensure both are in bounds.
    pub fn f64_array_swap_in_place(&mut self, id: F64ArrayId, i: usize, j: usize) -> F64ArrayId {
        debug_assert!(
            self.f64_arrays.is_live(id.raw()),
            "in-place swap on a dead f64 array"
        );
        self.f64_arrays.get_mut(id.raw()).swap(i, j);
        id
    }

    // --- Map allocation ---

    pub fn alloc_map(&mut self, entries: IndexMap<String, Value>) -> MapId {
        self.alloc_map_tagged(entries, None)
    }

    /// Allocate a record tagged as an instance of the class interned at
    /// `class`. The entry table is an ordinary record; only the tag differs.
    /// See [`MapObj`].
    pub fn alloc_class_instance(
        &mut self,
        entries: IndexMap<String, Value>,
        class: StringId,
    ) -> MapId {
        self.alloc_map_tagged(entries, Some(class))
    }

    fn alloc_map_tagged(
        &mut self,
        entries: IndexMap<String, Value>,
        class: Option<StringId>,
    ) -> MapId {
        self.tick_alloc(AllocKind::Map, map_entries_bytes(&entries));
        MapId::from_raw(self.maps.alloc(MapObj { entries, class }))
    }

    pub fn get_map(&self, id: MapId) -> &IndexMap<String, Value> {
        &self.maps.get(id.raw()).entries
    }

    /// The interned class name tagging `id`, or `None` for a plain record.
    pub fn map_class(&self, id: MapId) -> Option<StringId> {
        self.maps.get(id.raw()).class
    }

    /// The class name tagging `id` as a string, or `None` for a plain record.
    /// The borrow is of the heap's own string storage — no allocation.
    pub fn map_class_name(&self, id: MapId) -> Option<&str> {
        self.map_class(id).map(|s| self.get_string(s))
    }

    /// Return a new map equal to `id` with `key` set to `val`. `id` is
    /// unchanged (value semantics).
    pub fn map_set(&mut self, id: MapId, key: String, val: Value) -> MapId {
        let class = self.maps.get(id.raw()).class;
        let mut entries = self.maps.get(id.raw()).entries.clone();
        self.dup_stats
            .record(DupKind::Map, || map_entries_bytes(&entries));
        entries.insert(key, val);
        self.alloc_map_tagged(entries, class)
    }

    /// Return a new map equal to `id` with `key` removed. `id` is unchanged
    /// (value semantics). Insertion order of the remaining keys is preserved.
    /// Removing an absent key returns an equivalent new map.
    pub fn map_remove(&mut self, id: MapId, key: &str) -> MapId {
        let class = self.maps.get(id.raw()).class;
        let mut entries = self.maps.get(id.raw()).entries.clone();
        self.dup_stats
            .record(DupKind::Map, || map_entries_bytes(&entries));
        entries.shift_remove(key);
        self.alloc_map_tagged(entries, class)
    }

    /// In-place [`map_set`](Self::map_set): insert/overwrite `key` in `id`'s
    /// entry table and return `id`. See the in-place list methods for the
    /// soundness contract.
    pub fn map_set_in_place(&mut self, id: MapId, key: String, val: Value) -> MapId {
        debug_assert!(
            self.maps.is_live(id.raw()),
            "in-place set on a dead map"
        );
        self.maps.get_mut(id.raw()).entries.insert(key, val);
        id
    }

    /// In-place [`map_remove`](Self::map_remove): shift-remove `key` from `id`
    /// (preserving order of the rest) and return `id`. A no-op for an absent key.
    pub fn map_remove_in_place(&mut self, id: MapId, key: &str) -> MapId {
        debug_assert!(
            self.maps.is_live(id.raw()),
            "in-place remove on a dead map"
        );
        self.maps.get_mut(id.raw()).entries.shift_remove(key);
        id
    }

    // --- Element allocation ---

    pub fn alloc_element(&mut self, tag: StringId, props: MapId, children: ListId) -> ElementId {
        // Three `Copy` ids: no backing store of its own beyond the slot.
        self.tick_alloc(AllocKind::Element, 0);
        ElementId::from_raw(self.elements.alloc(ElementPayload {
            tag,
            props,
            children,
        }))
    }

    pub fn get_element_tag(&self, id: ElementId) -> StringId {
        self.elements.get(id.raw()).tag
    }

    pub fn get_element_props(&self, id: ElementId) -> MapId {
        self.elements.get(id.raw()).props
    }

    pub fn get_element_children(&self, id: ElementId) -> ListId {
        self.elements.get(id.raw()).children
    }

    // --- Cell allocation (`var` bindings) ---

    /// Allocate a cell holding `init`. The returned id is the *identity* a
    /// `var` binding carries: capturing it in a closure shares the box, and
    /// [`cell_write`](Self::cell_write) is visible through every copy of the id.
    pub fn alloc_cell(&mut self, init: Value) -> CellId {
        // One `Copy` Value: no backing store of its own beyond the slot.
        self.tick_alloc(AllocKind::Cell, 0);
        CellId::from_raw(self.cells.alloc(init))
    }

    /// Read a cell's current contents.
    pub fn cell_read(&self, id: CellId) -> Value {
        *self.cells.get(id.raw())
    }

    /// Overwrite a cell's contents in place, keeping its id. The one mutating
    /// operation in this module — see [`CellId`] for why it is sound.
    pub fn cell_write(&mut self, id: CellId, val: Value) {
        debug_assert!(
            self.cells.is_live(id.raw()),
            "write to a collected cell"
        );
        *self.cells.get_mut(id.raw()) = val;
    }

    // -----------------------------------------------------------------------
    // Garbage collection: mark-and-sweep
    // -----------------------------------------------------------------------

    /// Mark a single value as reachable, recursively marking any heap objects it references.
    ///
    /// The scalar check is split out and inlined so marking a list of numbers
    /// does not pay a call (and the id-carrying arms' stack frame) per element.
    #[inline]
    pub fn mark_value(&mut self, val: Value) {
        if !matches!(
            val,
            Value::Nil
                | Value::Bool(_)
                | Value::Int(_)
                | Value::Float(_)
                | Value::NativeFunction(_)
                | Value::Dual { .. }
                | Value::Vec2(_, _)
                | Value::Symbol(_)
                | Value::Handle(_)
                | Value::Pending(_)
        ) {
            self.mark_referenced(val);
        }
    }

    fn mark_referenced(&mut self, val: Value) {
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
            // Ids into the context's ClosureTable, which this heap cannot
            // reach: record them for the collector to follow (see
            // `gray_closures`) instead of marking them here.
            Value::Closure(id) => self.gray_closures.push(id),
            Value::OverloadSet(id) => self.gray_overload_sets.push(id),
            // Non-heap values need no marking. `Pending` is an id into the resource
            // table; its Ready/Errored payloads are rooted separately.
            Value::Nil
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
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
        self.strings.mark(id.raw());
    }

    fn mark_list(&mut self, id: ListId) {
        if self.lists.mark(id.raw()) {
            // Move the elements out to release the slab borrow during the
            // recursion, and put them back after. Nothing reads this payload
            // meanwhile: the slot is already marked, so reaching it again (a
            // cycle) returns before looking inside. Cheaper than cloning,
            // which copied every list on every collection.
            let elements = std::mem::take(self.lists.get_mut(id.raw()));
            for &val in &elements {
                self.mark_value(val);
            }
            *self.lists.get_mut(id.raw()) = elements;
        }
    }

    fn mark_f64_array(&mut self, id: F64ArrayId) {
        // Leaf: f64s are primitives — nothing recursive to mark.
        self.f64_arrays.mark(id.raw());
    }

    fn mark_map(&mut self, id: MapId) {
        if self.maps.mark(id.raw()) {
            // Copy values to avoid borrow conflict
            let values: Vec<Value> = self.maps.get(id.raw()).entries.values().copied().collect();
            for val in values {
                self.mark_value(val);
            }
            // The class tag names a heap string. Marking it here is what keeps
            // the name alive for exactly as long as some instance carries it.
            if let Some(class) = self.maps.get(id.raw()).class {
                self.mark_string(class);
            }
        }
    }

    fn mark_element(&mut self, id: ElementId) {
        if self.elements.mark(id.raw()) {
            let e = *self.elements.get(id.raw());
            self.mark_string(e.tag);
            self.mark_map(e.props);
            self.mark_list(e.children);
        }
    }

    fn mark_cell(&mut self, id: CellId) {
        if self.cells.mark(id.raw()) {
            // A cell's contents are an ordinary value and may themselves be
            // heap-backed (a `var` holding a list). The `mark` guard makes the
            // recursion terminate even if a cell ever reached itself.
            let contents = *self.cells.get(id.raw());
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
        strings.sweep_with(|id, s| {
            // Remove only this id's entry. The table maps content to the one
            // live id with that content, so it should always be this one; the
            // check keeps a reclaim from ever evicting a different, live id.
            if intern_table.get(s.as_str()) == Some(&StringId::from_raw(id)) {
                intern_table.remove(s.as_str());
            }
            *s = String::new();
        });

        self.lists.sweep_with(|_, v| *v = Vec::new());
        self.f64_arrays.sweep_with(|_, v| *v = Vec::new());
        self.maps.sweep_with(|_, v| {
            v.entries = IndexMap::new();
            v.class = None;
        });
        self.elements.sweep_with(|_, _| {});
        self.cells.sweep_with(|_, v| *v = Value::Nil);

        // Size the next collection's budget against what this collection would
        // cost to repeat (see `should_collect`). Computed here, once per cycle,
        // where an O(slots) walk is already being paid — never per allocation.
        // The collector drains the gray set before sweeping; anything left is
        // a leftover from an interrupted mark and must not survive into the
        // next cycle, where those ids would resurrect unrelated entries.
        self.gray_closures.clear();
        self.gray_overload_sets.clear();
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

    /// A reclaimed slot is reused, but the reuse mints a new id: the stale id
    /// is neither equal to the new one nor live. (Before generational ids the
    /// two compared equal and the stale id dereferenced to the new list.)
    #[test]
    fn a_reused_slot_does_not_alias_a_stale_id() {
        let mut heap = Heap::new();
        let old = heap.alloc_list(vec![Value::Int(1)]);
        heap.sweep();
        let new = heap.alloc_list(vec![Value::Int(2)]);

        assert_eq!(new.index(), old.index(), "the slot is reused");
        assert_ne!(new, old);
        assert_ne!(Value::List(new), Value::List(old));
        assert!(!heap.is_live(Value::List(old)));
        assert!(heap.is_live(Value::List(new)));
        assert_eq!(heap.try_get_list(old), None);
        assert_eq!(heap.try_get_list(new), Some(&[Value::Int(2)][..]));
    }

    #[test]
    fn a_swept_but_unreused_id_is_not_live() {
        let mut heap = Heap::new();
        let s = heap.alloc_string("gone".to_string());
        let kept = heap.alloc_string("kept".to_string());
        heap.mark_value(Value::String(kept));
        heap.sweep();

        assert!(!heap.is_live(Value::String(s)));
        assert_eq!(heap.try_get_string(s), None);
        assert_eq!(heap.try_get_string(kept), Some("kept"));
        // An enum variant is live only if both of its ids are.
        let data = heap.alloc_list(vec![]);
        assert!(!heap.is_live(Value::EnumVariant { tag: s, data }));
        assert!(heap.is_live(Value::EnumVariant { tag: kept, data }));
    }

    /// Marking a stale id must not resurrect the slot's new occupant.
    #[test]
    fn marking_a_stale_id_marks_nothing() {
        let mut heap = Heap::new();
        let old = heap.alloc_list(vec![]);
        heap.sweep();
        let new = heap.alloc_list(vec![]);
        assert_eq!(new.index(), old.index());

        heap.mark_value(Value::List(old));
        heap.sweep();
        assert!(!heap.is_live(Value::List(new)));
    }

    /// Re-interning content after its string was collected hands out a fresh
    /// id, and the intern table never returns the stale one.
    #[test]
    fn interning_after_collection_mints_a_fresh_id() {
        let mut heap = Heap::new();
        let old = heap.intern_str("hello");
        heap.sweep();
        let new = heap.intern_str("hello");
        assert_ne!(new, old);
        assert_eq!(heap.intern_str("hello"), new);
        assert_eq!(heap.get_string(new), "hello");
    }

    /// A slot whose generations are exhausted is retired, not wrapped: it is
    /// never handed out again.
    #[test]
    fn an_exhausted_slot_is_retired() {
        let mut slab: Slab<u8> = Slab::new();
        let a = slab.alloc(1);
        slab.slots[a.index() as usize].high_water = u32::MAX;
        slab.slots[a.index() as usize].generation = u32::MAX;
        slab.sweep_with(|_, _| {});

        let b = slab.alloc(2);
        assert_ne!(b.index(), a.index());
        assert_eq!(slab.slot_count(), 2);
        slab.sweep_with(|_, _| {});
        assert_ne!(slab.alloc(3).index(), a.index());
    }

    /// `inherit_generations` raises high-water marks and extends with dead
    /// slots, so a replacement slab never reissues an id the replaced one had.
    #[test]
    fn inherit_generations_never_reissues_a_replaced_slab_s_ids() {
        let mut live: Slab<u8> = Slab::new();
        let snapshot = live.clone();
        // The live slab moves on: two slots, the first reused once.
        let first = live.alloc(1);
        live.sweep_with(|_, _| {});
        let reused = live.alloc(2);
        let second = live.alloc(3);
        assert_eq!(reused.index(), first.index());
        let held = [first, reused, second];

        let mut restored = snapshot.clone();
        restored.inherit_generations(&live, || 0);
        assert_eq!(restored.slot_count(), 2);
        for _ in 0..4 {
            let id = restored.alloc(9);
            assert!(!held.contains(&id), "reissued {id:?}");
        }
    }

    #[test]
    fn list_append_does_not_mutate_the_input() {
        let mut heap = Heap::new();
        let original = heap.alloc_list(vec![Value::Int(1), Value::Int(2)]);

        let grown = heap.list_append(original, Value::Int(3));

        // A new, distinct list is returned with the extra element…
        assert_ne!(original, grown);
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
        assert_ne!(original, updated);
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

    /// A class instance is an ordinary entry table plus a tag: the entries are
    /// indistinguishable from a record's, and the tag is what dispatch and
    /// `type()` read. See [`MapObj`].
    #[test]
    fn a_class_instance_is_a_record_with_a_tag() {
        let mut heap = Heap::new();
        let tag = heap.alloc_string("Rect".to_string());
        let mut entries = IndexMap::new();
        entries.insert("x".to_string(), Value::Int(1));
        let instance = heap.alloc_class_instance(entries.clone(), tag);
        let plain = heap.alloc_map(entries);

        assert_eq!(heap.get_map(instance), heap.get_map(plain));
        assert_eq!(heap.map_class_name(instance), Some("Rect"));
        assert_eq!(heap.map_class_name(plain), None);
    }

    /// A copy-on-write field update keeps the instance an instance: `r.x = 5`
    /// on a `Rect` must not silently demote it to a plain record.
    #[test]
    fn map_set_and_remove_carry_the_class_tag() {
        let mut heap = Heap::new();
        let tag = heap.alloc_string("Rect".to_string());
        let mut entries = IndexMap::new();
        entries.insert("x".to_string(), Value::Int(1));
        entries.insert("y".to_string(), Value::Int(2));
        let r = heap.alloc_class_instance(entries, tag);

        let moved = heap.map_set(r, "x".to_string(), Value::Int(5));
        assert_ne!(moved, r, "value semantics: a new map");
        assert_eq!(heap.map_class_name(moved), Some("Rect"));
        assert_eq!(heap.get_map(moved).get("x"), Some(&Value::Int(5)));

        let shrunk = heap.map_remove(r, "y");
        assert_eq!(heap.map_class_name(shrunk), Some("Rect"));

        // The in-place forms keep the id, so they keep the tag by construction.
        let same = heap.map_set_in_place(r, "x".to_string(), Value::Int(9));
        assert_eq!(same, r);
        assert_eq!(heap.map_class_name(r), Some("Rect"));
    }

    /// The tag names a heap string, so the collector has to trace it — an
    /// instance that survives a sweep must not be left pointing at a reclaimed
    /// (and possibly reused) string slot.
    #[test]
    fn the_class_tag_survives_a_collection() {
        let mut heap = Heap::new();
        let tag = heap.alloc_string("Rect".to_string());
        let mut entries = IndexMap::new();
        entries.insert("x".to_string(), Value::Int(1));
        let r = heap.alloc_class_instance(entries, tag);
        // Garbage the collector should reclaim, so the sweep really runs.
        heap.alloc_string("unreferenced".to_string());

        heap.mark_value(Value::Map(r));
        heap.sweep();

        assert_eq!(heap.map_class_name(r), Some("Rect"));
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
        assert_ne!(original, updated);
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
        assert_ne!(original, updated);
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
        assert_ne!(original, shorter);
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
        assert_ne!(original, swapped);
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
        assert_ne!(original, removed);
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

        assert!(!heap.cells.is_live(cell.raw()));
        assert!(!heap.lists.is_live(list.raw()));
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
