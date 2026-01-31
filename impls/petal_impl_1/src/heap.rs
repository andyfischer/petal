//! Garbage-collected heap for runtime-allocated values

use std::collections::HashMap;

use crate::value::Value;

/// String ID - reference into the heap
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// List ID - reference into the heap
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

/// Map ID - reference into the heap
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);

/// Heap-allocated string
#[derive(Debug, Clone)]
struct HeapString {
    data: String,
    gc_mark: bool,
}

/// Heap-allocated list
#[derive(Debug, Clone)]
struct HeapList {
    elements: Vec<Value>,
    gc_mark: bool,
}

/// Heap-allocated map
#[derive(Debug, Clone)]
struct HeapMap {
    entries: HashMap<String, Value>,
    gc_mark: bool,
}

/// Garbage-collected heap
#[derive(Debug, Clone)]
pub struct Heap {
    strings: Vec<HeapString>,
    lists: Vec<HeapList>,
    maps: Vec<HeapMap>,
    /// Interned strings for deduplication
    string_intern: HashMap<String, StringId>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            lists: Vec::new(),
            maps: Vec::new(),
            string_intern: HashMap::new(),
        }
    }

    /// Allocate a new string (with interning)
    pub fn alloc_string(&mut self, s: String) -> StringId {
        // Check if already interned
        if let Some(&id) = self.string_intern.get(&s) {
            return id;
        }

        let id = StringId(self.strings.len() as u32);
        self.strings.push(HeapString {
            data: s.clone(),
            gc_mark: false,
        });
        self.string_intern.insert(s, id);
        id
    }

    /// Allocate a new list
    pub fn alloc_list(&mut self, elements: Vec<Value>) -> ListId {
        let id = ListId(self.lists.len() as u32);
        self.lists.push(HeapList {
            elements,
            gc_mark: false,
        });
        id
    }

    /// Allocate a new map
    pub fn alloc_map(&mut self, entries: HashMap<String, Value>) -> MapId {
        let id = MapId(self.maps.len() as u32);
        self.maps.push(HeapMap {
            entries,
            gc_mark: false,
        });
        id
    }

    /// Get a string by ID
    pub fn get_string(&self, id: StringId) -> Option<&str> {
        self.strings.get(id.0 as usize).map(|s| s.data.as_str())
    }

    /// Get a list by ID
    pub fn get_list(&self, id: ListId) -> Option<&[Value]> {
        self.lists.get(id.0 as usize).map(|l| l.elements.as_slice())
    }

    /// Get a mutable list by ID
    pub fn get_list_mut(&mut self, id: ListId) -> Option<&mut Vec<Value>> {
        self.lists.get_mut(id.0 as usize).map(|l| &mut l.elements)
    }

    /// Get a map by ID
    pub fn get_map(&self, id: MapId) -> Option<&HashMap<String, Value>> {
        self.maps.get(id.0 as usize).map(|m| &m.entries)
    }

    /// Get a mutable map by ID
    pub fn get_map_mut(&mut self, id: MapId) -> Option<&mut HashMap<String, Value>> {
        self.maps.get_mut(id.0 as usize).map(|m| &mut m.entries)
    }

    /// Mark phase of garbage collection
    fn mark(&mut self, roots: &[Value]) {
        // Clear all marks
        for s in &mut self.strings {
            s.gc_mark = false;
        }
        for l in &mut self.lists {
            l.gc_mark = false;
        }
        for m in &mut self.maps {
            m.gc_mark = false;
        }

        // Mark from roots
        for root in roots {
            self.mark_value(root);
        }
    }

    fn mark_value(&mut self, value: &Value) {
        match value {
            Value::String(id) => {
                if let Some(s) = self.strings.get_mut(id.0 as usize) {
                    if !s.gc_mark {
                        s.gc_mark = true;
                    }
                }
            }
            Value::List(id) => {
                if let Some(l) = self.lists.get_mut(id.0 as usize) {
                    if !l.gc_mark {
                        l.gc_mark = true;
                        let elements = l.elements.clone();
                        for elem in &elements {
                            self.mark_value(elem);
                        }
                    }
                }
            }
            Value::Map(id) => {
                if let Some(m) = self.maps.get_mut(id.0 as usize) {
                    if !m.gc_mark {
                        m.gc_mark = true;
                        let values: Vec<_> = m.entries.values().cloned().collect();
                        for val in &values {
                            self.mark_value(val);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Run garbage collection (mark and sweep)
    pub fn collect(&mut self, roots: &[Value]) {
        self.mark(roots);
        // For now, we don't actually free memory - just a simple mark phase
        // A full implementation would compact and update IDs
    }
}
