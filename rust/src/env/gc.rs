//! Garbage collection: the mark-and-sweep cycle over one execution context's
//! heap *and* its [`ClosureTable`](crate::closure_table::ClosureTable).
//!
//! The two stores are marked jointly. A closure lives outside the heap but can
//! be referenced from inside it (a record field, a list element), and its
//! captures point back into the heap, so neither can be traced alone: marking a
//! `Value::Closure` only records the id in the heap's *gray set*, and the loop
//! below alternates — drain the gray set, mark those table entries, feed their
//! captures back through the heap — until a round turns up nothing new. Only
//! then is either store swept.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors. `collect_garbage` is `pub(super)` (rather than private) only so
//! the run loops in `env::run` and the `env::tests` submodule can reach it
//! across the module split — it is not part of the public API.

use super::*;

impl Env {
    /// Run a mark-and-sweep garbage collection cycle.
    /// Marks all values reachable from roots (stack registers, state, closures,
    /// loop state), then sweeps unmarked heap objects.
    pub(super) fn collect_garbage(&mut self, ck: ContextKey) {
        // Timed only while profiling is on; `Instant::now` is a couple of ns
        // against a collection's microseconds, so it is not worth gating twice.
        let started = self.profile.enabled.then(std::time::Instant::now);
        // Disjoint borrows: stacks (shared) + the one context (mut). Mark all
        // roots into THAT context's heap, then sweep it.
        let ctx = self.contexts.get_mut(&ck).expect("context exists");
        let heap = &mut ctx.heap;

        // 1. Stack frame registers and state — only stacks bound to this
        //    context. `Stack::gc_roots` enumerates each stack's live values.
        for stack in self.stacks.values() {
            if stack.context != ck {
                continue;
            }
            stack.gc_roots(|val| heap.mark_value(val));
        }

        // 2. Closures are *not* roots. Every one reachable from a real root
        //    gets marked by the fixpoint below, and the rest are garbage — this
        //    is the whole point of the table: a host that re-runs a program per
        //    frame creates a closure per `fn` declaration per frame, and
        //    rooting them all pinned both the closures and everything they had
        //    captured for the life of the process.

        // 3. Print output buffer holds Rust Strings, not heap values — nothing
        //    to mark. The per-symbol output buffers, however, hold heap-backed
        //    Values (e.g. draw-command enum variants with string tags + list
        //    args), so they are GC roots: a frame can trip a collection mid-run
        //    while commands are still buffered.
        for buffer in ctx.output_buffers.values() {
            for val in buffer {
                heap.mark_value(*val);
            }
        }

        // 4. Host→script bindings hold heap-backed Values (e.g. a bound list of
        //    pressed keys), so they are GC roots too. Counters are plain u64s.
        for val in ctx.bindings.values() {
            heap.mark_value(*val);
        }

        // 5. The resource table persists resolved values (Ready/Errored) across
        //    runs, independent of any stack — so a heap-backed resolved value
        //    would otherwise be swept while a pending resource still references
        //    it. Mark those payloads as roots.
        ctx.resources.gc_roots(|val| heap.mark_value(val));

        // 6. Observed values (`crate::observe`) outlive the instruction that
        //    produced them — the whole point is that a host can read them
        //    afterwards — so an unrooted buffer would hand back ids into swept,
        //    possibly recycled slots. Only when the buffer's stamped context is
        //    the one being collected: its ids index *that* heap, and marking
        //    them into a fork's heap would resurrect unrelated objects there.
        if self.observations.context() == Some(ck) {
            for (_, val) in self.observations.iter() {
                heap.mark_value(val);
            }
        }

        // Joint fixpoint: follow every closure/overload set marking ran into,
        // marking their captures back into the heap (which may turn up more of
        // them), until a round comes back empty. `mark_closure`/`mark_set`
        // return false for an entry already marked, so a cycle terminates.
        let closures = &mut ctx.closures;
        loop {
            let (gray_closures, gray_sets) = heap.take_gray();
            if gray_closures.is_empty() && gray_sets.is_empty() {
                break;
            }
            for set_id in gray_sets {
                if closures.mark_set(set_id) {
                    for entry in closures.set(set_id).to_vec() {
                        // Re-grayed rather than marked here, so the closure and
                        // its captures go through the one path below.
                        heap.mark_value(crate::value::Value::Closure(entry.closure_id));
                    }
                }
            }
            for cid in gray_closures {
                if closures.mark_closure(cid) {
                    for val in closures.closure(cid).captures.clone() {
                        heap.mark_value(val);
                    }
                }
            }
        }

        // Sweep phase — both stores, now that the joint mark has settled.
        closures.sweep();
        heap.sweep();
        if let Some(t) = started {
            self.profile.record_gc(t.elapsed());
        }
    }
}
