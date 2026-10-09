//! Observation — the last value bound to every named IR term.
//!
//! Where [`crate::trace`] records *history* (every retired instruction, in a
//! bounded ring buffer, so `explain` can walk backwards), this records only the
//! *present*: one slot per term, overwritten on every write. That makes it
//! cheap enough to leave on for a whole session and gives an embedder the thing
//! it actually wants — "what is `grid` right now?" — without the program having
//! to cooperate by pushing values through a side channel.
//!
//! Off by default; when disabled, [`Observations::record`] is a single bool
//! check on the hot path, exactly as [`crate::trace::TraceBuffer::push`] is.
//!
//! # Values belong to one execution context
//! The stored `Value`s are heap ids, and a fork ([`crate::env::Env::fork_execution`])
//! runs against a *different* heap — the same id means a different object there.
//! So the buffer stamps itself with the [`ContextKey`] its contents came from
//! ([`Observations::enter_context`]) and clears whenever execution moves to
//! another context. Readers and the garbage collector both check the stamp
//! before touching a value; the alternative — silently decoding ids against the
//! wrong heap — produces plausible-looking nonsense rather than an error.

use std::collections::HashMap;

use crate::execution_context::ContextKey;
use crate::program::TermId;
use crate::value::Value;

/// Last-write-wins values for named IR terms, scoped to one execution context.
///
/// Only *named*, non-phantom terms are recorded; the VM applies that filter at
/// the recording sites (see `Vm::is_observable`) so the buffer never holds
/// entries no reader could name.
#[derive(Default)]
pub struct Observations {
    /// Recording gate. Off by default; the VM checks this before doing any of
    /// the per-instruction work observation needs.
    pub enabled: bool,
    /// The execution context `values` were recorded in — i.e. the heap their
    /// ids index. `None` when the buffer is empty.
    context: Option<ContextKey>,
    /// One slot per observed term, overwritten on every write. A term inside a
    /// loop or a function therefore reports its most recent binding, not its
    /// history: history is the trace buffer's job.
    values: HashMap<TermId, Value>,
}

impl Observations {
    pub fn new() -> Self {
        Self {
            enabled: false,
            context: None,
            values: HashMap::new(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Drop every recorded value and the context stamp with it.
    pub fn clear(&mut self) {
        self.values.clear();
        self.context = None;
    }

    /// The context the recorded values belong to, or `None` when empty. The GC
    /// marks this buffer only for the matching context, and readers refuse to
    /// decode against any other heap.
    pub fn context(&self) -> Option<ContextKey> {
        self.context
    }

    /// Point the buffer at `ck`, discarding anything recorded in a different
    /// context. Called wherever execution is about to enter a context (both VM
    /// construction sites in `env::run`), so a fork's values can never be read
    /// or marked against its parent's heap, or vice versa.
    pub fn enter_context(&mut self, ck: ContextKey) {
        if self.context != Some(ck) {
            self.values.clear();
            self.context = Some(ck);
        }
    }

    /// Clear the buffer and stamp it with `ck`. Called at the start of a run —
    /// and only there, never on resuming a yielded one — so each run reports
    /// its own bindings rather than accumulating across frames.
    pub fn start_run(&mut self, ck: ContextKey) {
        self.values.clear();
        self.context = Some(ck);
    }

    /// Record `value` as `term_id`'s current binding. Cheap when disabled — one
    /// bool check.
    #[inline]
    pub fn record(&mut self, term_id: TermId, value: Value) {
        if !self.enabled {
            return;
        }
        self.values.insert(term_id, value);
    }

    /// The current value of one term, if it has been bound since the last clear.
    pub fn get(&self, term_id: TermId) -> Option<Value> {
        self.values.get(&term_id).copied()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Every recorded `(term, value)` pair, in unspecified order. Callers that
    /// need determinism (the JSON reader does, because several terms can share
    /// one qualified name) must sort.
    pub fn iter(&self) -> impl Iterator<Item = (TermId, Value)> + '_ {
        self.values.iter().map(|(k, v)| (*k, *v))
    }
}
