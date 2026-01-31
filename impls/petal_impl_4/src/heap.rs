//! Garbage-collected heap for Petal runtime values

use std::collections::HashMap;
use crate::Value;

/// Reference to a string on the heap
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// Reference to a list on the heap
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

/// Reference to a map on the heap
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);

/// The heap stores strings, lists, and maps
pub struct Heap {
    strings: Vec<Option<String>>,
    lists: Vec<Option<Vec<Value>>>,
    maps: Vec<Option<HashMap<Value, Value>>>,
    free_strings: Vec<u32>,
    free_lists: Vec<u32>,
    free_maps: Vec<u32>,
}

impl Heap {
    /// Create a new empty heap
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            lists: Vec::new(),
            maps: Vec::new(),
            free_strings: Vec::new(),
            free_lists: Vec::new(),
            free_maps: Vec::new(),
        }
    }

    /// Allocate a string on the heap
    pub fn alloc_string(&mut self, s: &str) -> StringId {
        if let Some(id) = self.free_strings.pop() {
            self.strings[id as usize] = Some(s.to_string());
            StringId(id)
        } else {
            let id = self.strings.len() as u32;
            self.strings.push(Some(s.to_string()));
            StringId(id)
        }
    }

    /// Get a reference to a string
    pub fn get_string(&self, id: StringId) -> Option<&str> {
        self.strings.get(id.0 as usize)
            .and_then(|opt| opt.as_ref().map(|s| s.as_str()))
    }

    /// Allocate a new empty list
    pub fn alloc_list(&mut self) -> ListId {
        if let Some(id) = self.free_lists.pop() {
            self.lists[id as usize] = Some(Vec::new());
            ListId(id)
        } else {
            let id = self.lists.len() as u32;
            self.lists.push(Some(Vec::new()));
            ListId(id)
        }
    }

    /// Get a reference to a list
    pub fn get_list(&self, id: ListId) -> Option<&Vec<Value>> {
        self.lists.get(id.0 as usize)
            .and_then(|opt| opt.as_ref())
    }

    /// Get a mutable reference to a list
    pub fn get_list_mut(&mut self, id: ListId) -> Option<&mut Vec<Value>> {
        self.lists.get_mut(id.0 as usize)
            .and_then(|opt| opt.as_mut())
    }

    /// Push a value to a list
    pub fn push_to_list(&mut self, id: ListId, value: Value) {
        if let Some(list) = self.get_list_mut(id) {
            list.push(value);
        }
    }

    /// Pop a value from a list
    pub fn pop_from_list(&mut self, id: ListId) -> Option<Value> {
        if let Some(list) = self.get_list_mut(id) {
            list.pop()
        } else {
            None
        }
    }

    /// Allocate a new empty map
    pub fn alloc_map(&mut self) -> MapId {
        if let Some(id) = self.free_maps.pop() {
            self.maps[id as usize] = Some(HashMap::new());
            MapId(id)
        } else {
            let id = self.maps.len() as u32;
            self.maps.push(Some(HashMap::new()));
            MapId(id)
        }
    }

    /// Get a reference to a map
    pub fn get_map(&self, id: MapId) -> Option<&HashMap<Value, Value>> {
        self.maps.get(id.0 as usize)
            .and_then(|opt| opt.as_ref())
    }

    /// Get a mutable reference to a map
    pub fn get_map_mut(&mut self, id: MapId) -> Option<&mut HashMap<Value, Value>> {
        self.maps.get_mut(id.0 as usize)
            .and_then(|opt| opt.as_mut())
    }

    /// Set a value in a map
    pub fn set_in_map(&mut self, id: MapId, key: Value, value: Value) {
        if let Some(map) = self.get_map_mut(id) {
            map.insert(key, value);
        }
    }

    /// Delete a value from a map
    pub fn delete_from_map(&mut self, id: MapId, key: &Value) -> Option<Value> {
        if let Some(map) = self.get_map_mut(id) {
            map.remove(key)
        } else {
            None
        }
    }

    /// Check if a map contains a key
    pub fn map_contains_key(&self, id: MapId, key: &Value) -> bool {
        if let Some(map) = self.get_map(id) {
            map.contains_key(key)
        } else {
            false
        }
    }

    /// Garbage collection - mark and sweep
    /// This is a simplified version that just removes unreachable objects
    pub fn collect(&mut self, roots: &[Value]) {
        // Mark phase - simplified, just trace through lists and maps
        // In a real implementation, we'd use a more sophisticated algorithm

        // For now, we don't actually collect - this is a simple implementation
        // A full implementation would:
        // 1. Mark all objects reachable from roots
        // 2. Sweep unmarked objects
        // 3. Update references

        let _ = roots; // silence unused warning for now
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}
