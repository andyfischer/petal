//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! See docs/tech_outline/topics/Heap.md

use std::collections::BTreeMap;

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

struct HeapString {
    data: String,
    _gc_mark: bool,
}

struct HeapList {
    elements: Vec<Value>,
    _gc_mark: bool,
}

struct HeapMap {
    entries: BTreeMap<String, Value>,
    _gc_mark: bool,
}

pub struct Heap {
    strings: Vec<HeapString>,
    lists: Vec<HeapList>,
    maps: Vec<HeapMap>,
}

impl Heap {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            lists: Vec::new(),
            maps: Vec::new(),
        }
    }

    // --- String allocation ---

    pub fn alloc_string(&mut self, s: String) -> StringId {
        let id = StringId(self.strings.len() as u32);
        self.strings.push(HeapString {
            data: s,
            _gc_mark: false,
        });
        id
    }

    pub fn get_string(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize].data
    }

    // --- List allocation ---

    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        let id = ListId(self.lists.len() as u32);
        self.lists.push(HeapList {
            elements,
            _gc_mark: false,
        });
        id
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
        let id = MapId(self.maps.len() as u32);
        self.maps.push(HeapMap {
            entries,
            _gc_mark: false,
        });
        id
    }

    pub fn get_map(&self, id: MapId) -> &BTreeMap<String, Value> {
        &self.maps[id.0 as usize].entries
    }

    pub fn get_map_mut(&mut self, id: MapId) -> &mut BTreeMap<String, Value> {
        &mut self.maps[id.0 as usize].entries
    }

    /// GC stub - no-op for now.
    pub fn collect(&mut self) {
        // Future: mark-and-sweep implementation
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}
