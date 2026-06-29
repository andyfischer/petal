//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::value::Value;

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

struct HeapString {
    data: String,
    gc_mark: bool,
    alive: bool,
}

struct HeapList {
    elements: Vec<Value>,
    gc_mark: bool,
    alive: bool,
}

struct HeapF64Array {
    data: Vec<f64>,
    gc_mark: bool,
    alive: bool,
}

struct HeapMap {
    entries: IndexMap<String, Value>,
    gc_mark: bool,
    alive: bool,
}

struct HeapElement {
    tag: StringId,
    props: MapId,
    children: ListId,
    gc_mark: bool,
    alive: bool,
}

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
        }
    }

    /// Returns true if the allocation counter has exceeded the GC threshold.
    pub fn should_collect(&self) -> bool {
        self.alloc_count >= GC_THRESHOLD
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

    pub fn get_list_mut(&mut self, id: ListId) -> &mut Vec<Value> {
        &mut self.lists[id.0 as usize].elements
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
        elements.push(val);
        self.alloc_list(elements)
    }

    /// Return a new list equal to `id` with `elements[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds (eval already
    /// bounds-checks before calling).
    pub fn list_set(&mut self, id: ListId, index: usize, val: Value) -> ListId {
        let mut elements = self.lists[id.0 as usize].elements.clone();
        elements[index] = val;
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

    pub fn get_f64_array_mut(&mut self, id: F64ArrayId) -> &mut Vec<f64> {
        &mut self.f64_arrays[id.0 as usize].data
    }

    pub fn f64_array_len(&self, id: F64ArrayId) -> usize {
        self.f64_arrays[id.0 as usize].data.len()
    }

    /// Return a new f64 array equal to `id` with `data[index] = val`. `id` is
    /// unchanged. The caller must ensure `index` is in bounds.
    pub fn f64_array_set(&mut self, id: F64ArrayId, index: usize, val: f64) -> F64ArrayId {
        let mut data = self.f64_arrays[id.0 as usize].data.clone();
        data[index] = val;
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

    pub fn get_map_mut(&mut self, id: MapId) -> &mut IndexMap<String, Value> {
        &mut self.maps[id.0 as usize].entries
    }

    /// Return a new map equal to `id` with `key` set to `val`. `id` is
    /// unchanged (value semantics).
    pub fn map_set(&mut self, id: MapId, key: String, val: Value) -> MapId {
        let mut entries = self.maps[id.0 as usize].entries.clone();
        entries.insert(key, val);
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
}
