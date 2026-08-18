//! ClosureTable — the collected home for runtime closures and overload sets.
//!
//! A `Value::Closure`/`Value::OverloadSet` is an index into this table, the way
//! a `Value::List` is an index into the [`Heap`](crate::heap::Heap). It lives
//! beside the heap rather than inside it because the VM borrows the two
//! disjointly (a call reads a closure's captures while the heap is borrowed
//! mutably), but it is collected *with* the heap: marking spans both stores via
//! the heap's gray set, and [`Env::collect_garbage`](crate::env::Env) sweeps
//! them together.
//!
//! Before this table existed the two were plain `Vec`s that only ever grew, and
//! nothing reclaimed them: a host that re-runs a program per frame (a Garden
//! panel, a game loop) allocated one closure per `fn` declaration per frame
//! forever — and because every capture was treated as a GC root, the dead
//! closures pinned everything they had captured, so the heap could not shrink
//! either. That is the leak this fixes; see docs/dev/Architecture.md (Heap & GC).

use crate::backend::RuntimeClosure;
use crate::heap::Slab;
use crate::program::{ClosureId, OverloadEntry, OverloadSetId};

/// Rough payload cost of one closure, for the collector's budget: the record
/// itself plus its capture vector. Approximate on purpose — it only has to
/// scale with the work a collection will do (see
/// [`Heap::charge_external_alloc`](crate::heap::Heap::charge_external_alloc)).
fn closure_bytes(c: &RuntimeClosure) -> u64 {
    (std::mem::size_of::<RuntimeClosure>()
        + c.captures.len() * std::mem::size_of::<crate::value::Value>()) as u64
}

/// One execution context's closures and overload sets.
#[derive(Clone)]
pub struct ClosureTable {
    closures: Slab<RuntimeClosure>,
    sets: Slab<Vec<OverloadEntry>>,
}

impl ClosureTable {
    pub fn new() -> ClosureTable {
        ClosureTable {
            closures: Slab::new(),
            sets: Slab::new(),
        }
    }

    /// Store a closure, reusing a reclaimed slot when there is one.
    pub fn alloc_closure(&mut self, closure: RuntimeClosure) -> ClosureId {
        ClosureId(self.closures.alloc(closure))
    }

    /// The payload cost the caller should charge the heap's collector budget
    /// for `closure` — call before handing it to [`alloc_closure`].
    pub fn alloc_cost(closure: &RuntimeClosure) -> u64 {
        closure_bytes(closure)
    }

    pub fn closure(&self, id: ClosureId) -> &RuntimeClosure {
        self.closures.get(id.0)
    }

    pub fn closure_mut(&mut self, id: ClosureId) -> &mut RuntimeClosure {
        self.closures.get_mut(id.0)
    }

    /// Store an overload set, reusing a reclaimed slot when there is one.
    pub fn alloc_set(&mut self, entries: Vec<OverloadEntry>) -> OverloadSetId {
        OverloadSetId(self.sets.alloc(entries))
    }

    pub fn set(&self, id: OverloadSetId) -> &[OverloadEntry] {
        self.sets.get(id.0)
    }

    /// Mark one closure reachable. Returns true iff it was newly marked — the
    /// collector then traces its captures.
    pub fn mark_closure(&mut self, id: ClosureId) -> bool {
        self.closures.mark(id.0)
    }

    /// Mark one overload set reachable. Returns true iff it was newly marked —
    /// the collector then traces the closures it names.
    pub fn mark_set(&mut self, id: OverloadSetId) -> bool {
        self.sets.mark(id.0)
    }

    /// Reclaim every entry this cycle did not mark. Call only after the joint
    /// mark with the heap has reached its fixpoint (see the module docs): an
    /// entry that is still gray has not been traced yet, and sweeping it would
    /// free a live closure.
    pub fn sweep(&mut self) {
        self.closures.sweep_with(|c| c.captures = Vec::new());
        self.sets.sweep_with(|s| *s = Vec::new());
    }

    /// Drop everything. Used by `transfer_state`, which re-runs the program
    /// from scratch and rebuilds every closure it needs.
    pub fn clear(&mut self) {
        self.closures = Slab::new();
        self.sets = Slab::new();
    }

    /// Live closures — a diagnostic (`Debug for Env`, tests asserting that a
    /// per-frame host is not leaking).
    pub fn closure_count(&self) -> usize {
        self.closures.live_count()
    }

    /// Live overload sets.
    pub fn set_count(&self) -> usize {
        self.sets.live_count()
    }

    /// Slots this table walks in a collection, live or free — a diagnostic for
    /// how much slack the free list is holding.
    pub fn slot_count(&self) -> usize {
        self.closures.slot_count() + self.sets.slot_count()
    }
}

impl Default for ClosureTable {
    fn default() -> Self {
        ClosureTable::new()
    }
}
