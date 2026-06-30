//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! See docs/Architecture.md for the surrounding runtime design.
//!
//! Heap objects are **immutable by construction**: there are no in-place
//! mutators for collection payloads. "Mutations" (`list_append`, `list_set`,
//! `list_drop_last`, `map_set`, `map_remove`, `f64_array_set`,
//! `f64_array_swap`) allocate and return a *new* id, leaving the input
//! untouched (value semantics). This is what makes sharing heap objects
//! between executions safe — see docs/dev/speculative-execution-plan.md.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::stats::{DupKind, DupStats};
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

#[derive(Clone)]
struct HeapString {
    data: String,
    gc_mark: bool,
    alive: bool,
}

#[derive(Clone)]
struct HeapList {
    elements: Vec<Value>,
    gc_mark: bool,
    alive: bool,
}

#[derive(Clone)]
struct HeapF64Array {
    data: Vec<f64>,
    gc_mark: bool,
    alive: bool,
}

#[derive(Clone)]
struct HeapMap {
    entries: IndexMap<String, Value>,
    gc_mark: bool,
    alive: bool,
}

#[derive(Clone)]
struct HeapElement {
    tag: StringId,
    props: MapId,
    children: ListId,
    gc_mark: bool,
    alive: bool,
}

#[derive(Clone)]
pub struct Heap {
    strings: Vec<HeapString>,
    lists: Vec<HeapList>,
    f64_arrays: Vec<HeapF64Array>,
    maps: Vec<HeapMap>,
    elements: Vec<HeapElement>,
    /// Free slot indices for reuse
    free_strings: Vec<u32>,
    free_lists: Vec<u32>,
    free_f64_arrays: Vec<u32>,
    free_maps: Vec<u32>,
    free_elements: Vec<u32>,
    /// String intern table: content → existing StringId
    intern_table: HashMap<String, StringId>,
    /// Allocation counter — GC triggers after this many allocations
    alloc_count: u32,
    /// Value-duplication statistics. Records every copy-on-write and fork so we
    /// can track (and shrink) how much copying immutable values cost. Collected
    /// only in debug builds or with the `dup-stats` feature — see
    /// [`crate::stats`].
    stats: DupStats,
}

/// Number of allocations between GC cycles
const GC_THRESHOLD: u32 = 1024;

impl Heap {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            lists: Vec::new(),
            f64_arrays: Vec::new(),
            maps: Vec::new(),
            elements: Vec::new(),
            free_strings: Vec::new(),
            free_lists: Vec::new(),
            free_f64_arrays: Vec::new(),
            free_maps: Vec::new(),
            free_elements: Vec::new(),
            intern_table: HashMap::new(),
            alloc_count: 0,
            stats: DupStats::new(),
        }
    }

    /// Value-duplication statistics accumulated by this heap's copy-on-write
    /// operations and forks. All zero in release builds unless the `dup-stats`
    /// feature is enabled — see [`crate::stats`].
    pub fn dup_stats(&self) -> &DupStats {
        &self.stats
    }

    /// Mutable access to the duplication stats, e.g. to [`DupStats::reset`] them
    /// between runs.
    pub fn dup_stats_mut(&mut self) -> &mut DupStats {
        &mut self.stats
    }

    /// Total bytes of live payload this heap holds — the rough cost of cloning
    /// it. Used to attribute a `Fork`'s byte count; also handy for diagnostics.
    fn payload_bytes(&self) -> u64 {
        let strings: u64 = self
            .strings
            .iter()
            .filter(|s| s.alive)
            .map(|s| s.data.len() as u64)
            .sum();
        let lists: u64 = self
            .lists
            .iter()
            .filter(|l| l.alive)
            .map(|l| value_slice_bytes(l.elements.len()))
            .sum();
        let f64s: u64 = self
            .f64_arrays
            .iter()
            .filter(|a| a.alive)
            .map(|a| (a.data.len() * std::mem::size_of::<f64>()) as u64)
            .sum();
        let maps: u64 = self
            .maps
            .iter()
            .filter(|m| m.alive)
            .map(|m| map_entries_bytes(&m.entries))
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
    /// docs/dev/speculative-execution-plan.md, Increment 4).
    pub fn fork(&self) -> Heap {
        let mut child = self.clone();
        // The fork copied this whole heap. Attribute that copy to the child
        // (the execution that now owns the duplicate) with fresh counters, so
        // each context measures the duplication done on its own behalf rather
        // than re-counting its parent's history.
        child.stats.reset();
        child.stats.record(DupKind::Fork, || self.payload_bytes());
        child
    }

    fn tick_alloc(&mut self) {
        self.alloc_count += 1;
    }

    // --- String allocation ---

    pub fn alloc_string(&mut self, s: String) -> StringId {
        // Check intern table for an existing live string with the same content
        if let Some(&existing_id) = self.intern_table.get(&s) {
            let slot = &self.strings[existing_id.0 as usize];
            if slot.alive {
                return existing_id;
            }
            // Stale entry — will be overwritten below
        }

        self.tick_alloc();
        let id = if let Some(idx) = self.free_strings.pop() {
            let slot = &mut self.strings[idx as usize];
            slot.data = s.clone();
            slot.gc_mark = false;
            slot.alive = true;
            StringId(idx)
        } else {
            let id = StringId(self.strings.len() as u32);
            self.strings.push(HeapString {
                data: s.clone(),
                gc_mark: false,
                alive: true,
            });
            id
        };

        self.intern_table.insert(s, id);
        id
    }

    pub fn get_string(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize].data
    }

    // --- List allocation ---

    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        self.tick_alloc();
        if let Some(idx) = self.free_lists.pop() {
            let slot = &mut self.lists[idx as usize];
            slot.elements = elements;
            slot.gc_mark = false;
            slot.alive = true;
            ListId(idx)
        } else {
            let id = ListId(self.lists.len() as u32);
            self.lists.push(HeapList {
                elements,
                gc_mark: false,
                alive: true,
            });
            id
        }
    }

    pub fn get_list(&self, id: ListId) -> &[Value] {
        &self.lists[id.0 as usize].elements
    }

    pub fn list_len(&self, id: ListId) -> usize {
        self.lists[id.0 as usize].elements.len()
    }

    // --- Immutable list operations (value semantics) ---
    //
    // These never mutate the input list; they allocate and return a new list.
    // Today they copy the backing `Vec`; once the backing becomes a persistent
    // structure the copy becomes a cheap structural-sharing operation and these
    // signatures stay the same.

    /// Return a new list equal to `id` with `val` appended. `id` is unchanged.
    pub fn list_append(&mut self, id: ListId, val: Value) -> ListId {
        let mut elements = self.lists[id.0 as usize].elements.clone();
        self.stats.record(DupKind::List, || value_slice_bytes(elements.len()));
        elements.push(val);
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with `elements[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds (eval already
    /// bounds-checks before calling).
    pub fn list_set(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        let mut elements = self.lists[id.0 as usize].elements.clone();
        self.stats.record(DupKind::List, || value_slice_bytes(elements.len()));
        elements[index] = val;
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with its last element removed. `id` is
    /// unchanged. On an empty list, returns a new empty list.
    pub fn list_drop_last(&mut self, id: ListId) -> ListId {
        let mut elements = self.lists[id.0 as usize].elements.clone();
        self.stats.record(DupKind::List, || value_slice_bytes(elements.len()));
        elements.pop();
        self.alloc_list(elements)
    }

    // --- F64 array allocation ---

    pub fn alloc_f64_array(&mut self, data: Vec<f64>) -> F64ArrayId {
        self.tick_alloc();
        if let Some(idx) = self.free_f64_arrays.pop() {
            let slot = &mut self.f64_arrays[idx as usize];
            slot.data = data;
            slot.gc_mark = false;
            slot.alive = true;
            F64ArrayId(idx)
        } else {
            let id = F64ArrayId(self.f64_arrays.len() as u32);
            self.f64_arrays.push(HeapF64Array {
                data,
                gc_mark: false,
                alive: true,
            });
            id
        }
    }

    pub fn get_f64_array(&self, id: F64ArrayId) -> &[f64] {
        &self.f64_arrays[id.0 as usize].data
    }

    pub fn f64_array_len(&self, id: F64ArrayId) -> usize {
        self.f64_arrays[id.0 as usize].data.len()
    }

    /// Return a new f64 array equal to `id` with `data[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds.
    pub fn f64_array_set(&mut self, id: F64ArrayId, index: usize, val: f64) -> F64ArrayId {
        let mut data = self.f64_arrays[id.0 as usize].data.clone();
        self.stats
            .record(DupKind::F64Array, || (data.len() * std::mem::size_of::<f64>()) as u64);
        data[index] = val;
        self.alloc_f64_array(data)
    }

    /// Return a new f64 array equal to `id` with elements `i` and `j` swapped.
    /// `id` is unchanged. The caller must ensure `i` and `j` are in bounds.
    pub fn f64_array_swap(&mut self, id: F64ArrayId, i: usize, j: usize) -> F64ArrayId {
        let mut data = self.f64_arrays[id.0 as usize].data.clone();
        self.stats
            .record(DupKind::F64Array, || (data.len() * std::mem::size_of::<f64>()) as u64);
        data.swap(i, j);
        self.alloc_f64_array(data)
    }

    // --- Map allocation ---

    pub fn alloc_map(&mut self, entries: IndexMap<String, Value>) -> MapId {
        self.tick_alloc();
        if let Some(idx) = self.free_maps.pop() {
            let slot = &mut self.maps[idx as usize];
            slot.entries = entries;
            slot.gc_mark = false;
            slot.alive = true;
            MapId(idx)
        } else {
            let id = MapId(self.maps.len() as u32);
            self.maps.push(HeapMap {
                entries,
                gc_mark: false,
                alive: true,
            });
            id
        }
    }

    pub fn get_map(&self, id: MapId) -> &IndexMap<String, Value> {
        &self.maps[id.0 as usize].entries
    }

    /// Return a new map equal to `id` with `key` set to `val`. `id` is
    /// unchanged (value semantics).
    pub fn map_set(&mut self, id: MapId, key: String, val: Value) -> MapId {
        let mut entries = self.maps[id.0 as usize].entries.clone();
        self.stats.record(DupKind::Map, || map_entries_bytes(&entries));
        entries.insert(key, val);
        self.alloc_map(entries)
    }

    /// Return a new map equal to `id` with `key` removed. `id` is unchanged
    /// (value semantics). Insertion order of the remaining keys is preserved.
    /// Removing an absent key returns an equivalent new map.
    pub fn map_remove(&mut self, id: MapId, key: &str) -> MapId {
        let mut entries = self.maps[id.0 as usize].entries.clone();
        self.stats.record(DupKind::Map, || map_entries_bytes(&entries));
        entries.shift_remove(key);
        self.alloc_map(entries)
    }

    // --- Element allocation ---

    pub fn alloc_element(&mut self, tag: StringId, props: MapId, children: ListId) -> ElementId {
        self.tick_alloc();
        if let Some(idx) = self.free_elements.pop() {
            let slot = &mut self.elements[idx as usize];
            slot.tag = tag;
            slot.props = props;
            slot.children = children;
            slot.gc_mark = false;
            slot.alive = true;
            ElementId(idx)
        } else {
            let id = ElementId(self.elements.len() as u32);
            self.elements.push(HeapElement {
                tag,
                props,
                children,
                gc_mark: false,
                alive: true,
            });
            id
        }
    }

    pub fn get_element_tag(&self, id: ElementId) -> StringId {
        self.elements[id.0 as usize].tag
    }

    pub fn get_element_props(&self, id: ElementId) -> MapId {
        self.elements[id.0 as usize].props
    }

    pub fn get_element_children(&self, id: ElementId) -> ListId {
        self.elements[id.0 as usize].children
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
            // Non-heap values: nothing to mark
            Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_)
            | Value::Closure(_) | Value::OverloadSet(_) | Value::NativeFunction(_)
            | Value::Dual { .. } | Value::Vec2(_, _) | Value::Symbol(_) => {}
        }
    }

    fn mark_string(&mut self, id: StringId) {
        let slot = &mut self.strings[id.0 as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
        }
    }

    fn mark_list(&mut self, id: ListId) {
        let slot = &mut self.lists[id.0 as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
            // Copy elements to avoid borrow conflict
            let elements: Vec<Value> = slot.elements.clone();
            for val in elements {
                self.mark_value(val);
            }
        }
    }

    fn mark_f64_array(&mut self, id: F64ArrayId) {
        let slot = &mut self.f64_arrays[id.0 as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
            // f64s are primitives — nothing recursive to mark.
        }
    }

    fn mark_map(&mut self, id: MapId) {
        let slot = &mut self.maps[id.0 as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
            // Copy values to avoid borrow conflict
            let values: Vec<Value> = slot.entries.values().copied().collect();
            for val in values {
                self.mark_value(val);
            }
        }
    }

    fn mark_element(&mut self, id: ElementId) {
        let slot = &mut self.elements[id.0 as usize];
        if slot.alive && !slot.gc_mark {
            slot.gc_mark = true;
            let tag = slot.tag;
            let props = slot.props;
            let children = slot.children;
            self.mark_string(tag);
            self.mark_map(props);
            self.mark_list(children);
        }
    }

    /// Sweep phase: free all unmarked objects and reset marks.
    /// Call this after marking all roots.
    pub fn sweep(&mut self) {
        self.free_strings.clear();
        for (i, slot) in self.strings.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    self.intern_table.remove(&slot.data);
                    slot.data.clear();
                    self.free_strings.push(i as u32);
                }
            }
        }

        self.free_lists.clear();
        for (i, slot) in self.lists.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    slot.elements.clear();
                    self.free_lists.push(i as u32);
                }
            }
        }

        self.free_f64_arrays.clear();
        for (i, slot) in self.f64_arrays.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    slot.data.clear();
                    self.free_f64_arrays.push(i as u32);
                }
            }
        }

        self.free_maps.clear();
        for (i, slot) in self.maps.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    slot.entries.clear();
                    self.free_maps.push(i as u32);
                }
            }
        }

        self.free_elements.clear();
        for (i, slot) in self.elements.iter_mut().enumerate() {
            if slot.alive {
                if slot.gc_mark {
                    slot.gc_mark = false;
                } else {
                    slot.alive = false;
                    self.free_elements.push(i as u32);
                }
            }
        }

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
        assert_eq!(
            stats.get(DupKind::List).bytes,
            2 * value_slice_bytes(3),
        );
        assert_eq!(stats.total_count(), 2);
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
