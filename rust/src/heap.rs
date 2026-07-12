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
    /// String intern table: content → existing StringId
    intern_table: HashMap<String, StringId>,
    /// Allocation counter — GC triggers after this many allocations
    alloc_count: u32,
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

/// Number of allocations between GC cycles
const GC_THRESHOLD: u32 = 1024;

impl Heap {
    pub fn new() -> Self {
        Self {
            strings: Slab::new(),
            lists: Slab::new(),
            f64_arrays: Slab::new(),
            maps: Slab::new(),
            elements: Slab::new(),
            intern_table: HashMap::new(),
            alloc_count: 0,
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

    /// Total bytes of live payload this heap holds — the rough cost of cloning
    /// it. Used to attribute a `Fork`'s byte count; also handy for diagnostics.
    fn payload_bytes(&self) -> u64 {
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

    /// Returns true if the allocation counter has exceeded the GC threshold.
    pub fn should_collect(&self) -> bool {
        self.alloc_count >= GC_THRESHOLD
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
        child
            .dup_stats
            .record(DupKind::Fork, || self.payload_bytes());
        child
    }

    fn tick_alloc(&mut self, kind: AllocKind) {
        self.alloc_count += 1;
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

        self.tick_alloc(AllocKind::String);
        let id = StringId(self.strings.alloc(s.clone()));
        self.intern_table.insert(s, id);
        id
    }

    pub fn get_string(&self, id: StringId) -> &str {
        self.strings.get(id.0)
    }

    // --- List allocation ---

    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        self.tick_alloc(AllocKind::List);
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
        self.tick_alloc(AllocKind::F64Array);
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
        self.tick_alloc(AllocKind::Map);
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
        self.tick_alloc(AllocKind::Element);
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
        strings.sweep_with(|s| {
            intern_table.remove(s.as_str());
            s.clear();
        });

        self.lists.sweep_with(|v| v.clear());
        self.f64_arrays.sweep_with(|v| v.clear());
        self.maps.sweep_with(|v| v.clear());
        self.elements.sweep_with(|_| {});

        self.alloc_count = 0;
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
