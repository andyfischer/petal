//! Heap - Garbage-collected storage for strings, lists, and maps.
//!
//! In this implementation, heap allocation is handled by Rust's Rc<RefCell<>>
//! mechanism. This module provides the types and interfaces described in the
//! Petal specification for future GC implementation.
//!
//! See docs/tech_outline/topics/Heap.md

/// Opaque handle to a heap-allocated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub u32);

/// Opaque handle to a heap-allocated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u32);

/// Opaque handle to a heap-allocated map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId(pub u32);
