# Heap

The garbage-collected heap for runtime-allocated values.

## Related Topics

- [[Execution]] - Execution allocates on the heap
- [[Value|Value]] - Values reference heap objects

## Overview

Strings, lists, and maps are allocated on a garbage-collected heap rather than being stored inline in [[Value|Value]]. Values hold opaque IDs (`StringId`, `ListId`, `MapId`) that reference heap objects.

## Heap IDs

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);
```

These IDs are indices into the heap's internal storage. They are only valid within the [[Env|Env]] that allocated them.

## Heap Structure

```rust
pub struct Heap {
    strings: Vec<HeapString>,
    lists: Vec<HeapList>,
    maps: Vec<HeapMap>,

    /// GC metadata
    gc_state: GcState,
}

pub struct HeapString {
    data: String,
    gc_mark: bool,
}

pub struct HeapList {
    elements: Vec<Value>,
    gc_mark: bool,
}

pub struct HeapMap {
    entries: HashMap<Value, Value>,
    gc_mark: bool,
}
```

## Garbage Collection

The heap uses a mark-and-sweep garbage collector:

1. **Mark phase**: Starting from roots (stack registers, state storage), traverse all reachable Values and mark their heap objects
2. **Sweep phase**: Free any unmarked objects, compacting IDs if desired

GC is triggered based on allocation pressure or explicitly via the API.

```rust
impl Heap {
    /// Allocate a new string
    pub fn alloc_string(&mut self, s: String) -> StringId;

    /// Allocate a new empty list
    pub fn alloc_list(&mut self) -> ListId;

    /// Allocate a new empty map
    pub fn alloc_map(&mut self) -> MapId;

    /// Run garbage collection
    pub fn collect(&mut self, roots: &[Value]);

    /// Get a string by ID
    pub fn get_string(&self, id: StringId) -> Option<&str>;

    /// Get a list by ID (mutable for append, etc.)
    pub fn get_list_mut(&mut self, id: ListId) -> Option<&mut Vec<Value>>;

    /// Get a map by ID
    pub fn get_map_mut(&mut self, id: MapId) -> Option<&mut HashMap<Value, Value>>;
}
```

## Ownership

The Heap is owned by the [[Env|Env]]. All stacks within an Env share the same heap, allowing values to be passed between stacks.

## String Interning

Small strings or frequently-used strings may be interned (deduplicated). This is an optional optimization - the implementor may choose:
- No interning (simplest)
- Intern all strings
- Intern strings below a size threshold
- Intern only string constants from the [[Program|Program]]'s constant table

## Implementation Notes

The exact GC strategy is left to the implementor. Options include:
- Simple mark-and-sweep (easiest to implement)
- Generational GC (better for short-lived allocations)
- Reference counting with cycle detection
- Arena-based allocation with bulk deallocation per stack

The key requirement is that heap IDs remain valid until the object is collected, and that collection only occurs at safe points (not mid-evaluation).

---

See also: [[Outline|Implementation Plan]]
