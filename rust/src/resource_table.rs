//! Resource table — the home for pending/unresolved resources.
//!
//! A [`Value::Pending`](crate::value::Value::Pending) is a thin
//! [`PendingId`](crate::value::PendingId) index into this table; the resolution
//! state and provenance live here so [`Value`] stays `Copy`. The table lives on
//! the [`ExecutionContext`](crate::execution_context::ExecutionContext) (next to
//! the heap) so it survives `reset_stack` and is forked/cloned consistently with
//! the rest of that context. See docs/dev/pending-values-plan.md.

use std::collections::HashMap;

use crate::program::TermId;
use crate::value::{PendingId, Value};

/// The resolution state of one resource. Modelled on React Query / Elm
/// `RemoteData`: a resource starts `Loading`, then lands as `Ready` or `Errored`.
/// An `Errored` resource is still a pending-*kind* value at the language level —
/// only the (later-chunk) meta functions distinguish it.
#[derive(Clone, Debug)]
pub enum ResourceState {
    Loading,
    Errored(Value),
    Ready(Value),
}

/// One resource's table entry: its cache key, current state, origin call site
/// (for the visualization tooling — `None` when a native can't reach the origin
/// term), and how many ops absorbed it this frame.
#[derive(Clone, Debug)]
pub struct ResourceEntry {
    pub key: u64,
    pub state: ResourceState,
    pub origin: Option<TermId>,
    pub absorbed_count: u64,
}

/// Keyed table of resources. `entries` is index-addressed by [`PendingId`];
/// `by_key` dedups so two fetches of the same key share one entry (and thus one
/// `PendingId`). Cloned wholesale on context fork, exactly like the heap.
#[derive(Clone, Debug, Default)]
pub struct ResourceTable {
    entries: Vec<ResourceEntry>,
    by_key: HashMap<u64, PendingId>,
}

impl ResourceTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The [`PendingId`] currently mapped to `key`, if any.
    pub fn pending_for_key(&self, key: u64) -> Option<PendingId> {
        self.by_key.get(&key).copied()
    }

    /// Return the existing entry for `key`, or create a fresh `Loading` one.
    /// Dedup: the same key always yields the same `PendingId`.
    pub fn get_or_create_loading(&mut self, key: u64) -> PendingId {
        if let Some(id) = self.by_key.get(&key) {
            return *id;
        }
        let id = PendingId(self.entries.len() as u32);
        self.entries.push(ResourceEntry {
            key,
            state: ResourceState::Loading,
            // TODO(pending): thread the requesting instruction's origin TermId in.
            origin: None,
            absorbed_count: 0,
        });
        self.by_key.insert(key, id);
        id
    }

    /// Mark `key`'s entry `Ready(value)`, creating it if absent.
    pub fn resolve(&mut self, key: u64, value: Value) {
        let id = self.get_or_create_loading(key);
        self.entries[id.0 as usize].state = ResourceState::Ready(value);
    }

    /// Mark `key`'s entry `Errored(error)`, creating it if absent.
    pub fn reject(&mut self, key: u64, error: Value) {
        let id = self.get_or_create_loading(key);
        self.entries[id.0 as usize].state = ResourceState::Errored(error);
    }

    /// The entry a `PendingId` points at.
    pub fn entry(&self, id: PendingId) -> &ResourceEntry {
        &self.entries[id.0 as usize]
    }

    /// The resolved value for `id` if its entry is `Ready`; `None` while it is
    /// `Loading` or `Errored`.
    pub fn value_for(&self, id: PendingId) -> Option<Value> {
        match self.entries[id.0 as usize].state {
            ResourceState::Ready(v) => Some(v),
            _ => None,
        }
    }

    /// GC roots: the heap-backed payload `Value`s the table holds alive.
    /// `Ready`/`Errored` entries can carry heap ids (String/List/Map/Element),
    /// and the table outlives any single run's stack, so these must be marked
    /// or a mid-run collection would sweep a resolved value out from under a
    /// still-pending resource. `Loading` entries carry no payload.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for entry in &self.entries {
            match entry.state {
                ResourceState::Ready(v) | ResourceState::Errored(v) => mark(v),
                ResourceState::Loading => {}
            }
        }
    }
}
