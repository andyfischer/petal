//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! See docs/tech_outline/topics/Heap.md

use std::collections::{BTreeMap, HashMap};

use crate::value::Value;

/// Opaque handle to a heap-allocated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// Opaque handle to a heap-allocated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

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

struct HeapMap {
    entries: BTreeMap<String, Value>,
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
    maps: Vec<HeapMap>,
    elements: Vec<HeapElement>,
    /// Free slot indices for reuse
    free_strings: Vec<u32>,
    free_lists: Vec<u32>,
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
            maps: Vec::new(),
            elements: Vec::new(),
            free_strings: Vec::new(),
            free_lists: Vec::new(),
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

    // --- Map allocation ---

    pub fn alloc_map(&mut self, entries: BTreeMap<String, Value>) -> MapId {
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

    pub fn get_map(&self, id: MapId) -> &BTreeMap<String, Value> {
        &self.maps[id.0 as usize].entries
    }

    pub fn get_map_mut(&mut self, id: MapId) -> &mut BTreeMap<String, Value> {
        &mut self.maps[id.0 as usize].entries
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
            Value::Map(id) => self.mark_map(id),
            Value::Element(id) => self.mark_element(id),
            Value::EnumVariant { tag, data } => {
                self.mark_string(tag);
                self.mark_list(data);
            }
            // Non-heap values: nothing to mark
            Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_)
            | Value::Closure(_) | Value::NativeFunction(_) => {}
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
