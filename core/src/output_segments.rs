//! Segmented output: a replayed memo scope reaches the host as one segment
//! reference instead of the values it emitted.
//!
//! An output buffer (`push_output`, an emitter native) is a flat
//! `Vec<Value>` per symbol. Memoization ([`crate::memo`]) skips the calls
//! whose inputs did not change, but a replay still appends every value the
//! scope emitted last time, so a host that reads the buffer walks the whole
//! frame: a static world of 24,000 declarations costs the host 24,000 items a
//! frame even when the script itself ran in 70 µs.
//!
//! A host can switch a buffer to **segmented** mode
//! ([`crate::env::Env::set_output_segmented`]). In that mode:
//!
//! - when a memo scope is recorded, the values it emitted into the buffer
//!   (and the segments of its child scopes) are moved out of the buffer into a
//!   [`SegmentBody`], and the buffer keeps a single [`SegMark`] where they
//!   were;
//! - when a scope is replayed, the VM pushes one mark for its body and appends
//!   nothing;
//! - the host drains a [`SegmentedFrame`]
//!   ([`crate::env::Env::take_output_segments`]): this frame's values and
//!   segment references in order, the definition of every segment it has not
//!   been given before (children before the parents that contain them), and
//!   the serials of segments it may forget.
//!
//! The host keeps what it made of each segment (decoded draws, body slots)
//! by serial and splices it back where a reference appears, so a frame costs
//! the host O(scopes) plus the values emitted fresh, not O(values).
//!
//! # Identity
//!
//! A segment has two numbers. Its **serial** names one recording of one
//! scope: it is unique for the life of the process (across stacks, contexts
//! and reloads), so a definition never changes under a serial and a host
//! cache keyed by serial is never stale. Its **id** names the scope: a hash
//! of the call path that ran it (the same `[Call(site) | Index(i)]` chain
//! that keys `state`), stable across frames and runs of the same program. A
//! re-recorded scope keeps its id and gets a new serial, which is how a
//! retained renderer can reuse the old segment's buffer range for the new
//! contents. Ids are hashes and may in principle collide; content is always
//! looked up by serial.
//!
//! # Lifetime
//!
//! A body lives as long as a memo record, a parent body, or an undrained
//! buffer holds it. The VM remembers which serials it has defined to the host
//! (by weak reference); when one of them dies the next drain lists it in
//! [`SegmentedFrame::dropped`]. Until then the values in its definition stay
//! rooted for the collector, so a host may keep them (by id) across frames. A
//! hot reload clears the memo table, so every segment is dropped on the drain
//! after it.
//!
//! What is not segmented: a buffer is segmented only while emit tracing is
//! off ([`crate::execution_context::ExecutionContext::enable_emit_trace`]),
//! since a segment does not carry the origins of its values; scopes that do
//! not get a record (effectful, folded as too small, past the table's bounds)
//! leave their values in the buffer as fresh items, exactly as before.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crate::stack::PathPart;
use crate::symbol::SymbolId;
use crate::value::Value;

/// Serials are process-wide, so two stacks (or a stack before and after a
/// reload) can never hand the same host the same serial for different
/// contents.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

fn next_serial() -> u64 {
    NEXT_SERIAL.fetch_add(1, Ordering::Relaxed)
}

/// The id of the scope at `path`: a hash of the call path, stable across
/// frames and runs of the same program.
pub fn scope_id(path: &[PathPart]) -> u64 {
    // `DefaultHasher::new` is SipHash with fixed keys: deterministic across
    // runs and processes, unlike a `RandomState`.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

/// A segment reference at a position of a buffer (or of a body's values):
/// the segment's contents come before the value at index `at`.
#[derive(Debug, Clone)]
pub struct SegMark {
    pub at: usize,
    pub body: Arc<SegmentBody>,
}

/// What one recording of one scope emitted into one buffer: its own values,
/// with its child scopes' segments at their positions among them.
#[derive(Debug)]
pub struct SegmentBody {
    /// The scope (hash of its call path; see the module docs).
    pub id: u64,
    /// This recording (process-unique).
    pub serial: u64,
    /// The values the scope emitted itself, outside any child segment.
    pub values: Vec<Value>,
    /// Child segments, by position in `values`, in order.
    pub marks: Vec<SegMark>,
}

impl SegmentBody {
    pub fn new(id: u64, values: Vec<Value>, marks: Vec<SegMark>) -> Self {
        SegmentBody {
            id,
            serial: next_serial(),
            values,
            marks,
        }
    }

    /// Every value of this segment with its child segments expanded in
    /// place: what the flat buffer would have held.
    pub fn flatten_into(&self, out: &mut Vec<Value>) {
        flatten(&self.values, &self.marks, out);
    }

    /// Number of values with children expanded.
    pub fn flat_len(&self) -> usize {
        self.values.len() + self.marks.iter().map(|m| m.body.flat_len()).sum::<usize>()
    }

    /// Mark every heap value this body and its children hold, each body
    /// once (`seen` by serial): a child is shared by its parent's body and
    /// its own record.
    pub fn mark_values(&self, seen: &mut HashSet<u64>, mark: &mut impl FnMut(Value)) {
        if !seen.insert(self.serial) {
            return;
        }
        self.values.iter().for_each(|v| mark(*v));
        for m in &self.marks {
            m.body.mark_values(seen, mark);
        }
    }
}

/// `values` with `marks` expanded in place.
fn flatten(values: &[Value], marks: &[SegMark], out: &mut Vec<Value>) {
    let mut i = 0;
    for m in marks {
        out.extend_from_slice(&values[i..m.at]);
        m.body.flatten_into(out);
        i = m.at;
    }
    out.extend_from_slice(&values[i..]);
}

/// One segmented buffer's live state in an execution context.
#[derive(Debug, Default)]
pub struct SegChannel {
    /// This frame's segment references, by position in the buffer's values.
    /// Positions never decrease.
    pub marks: Vec<SegMark>,
    /// The serials the host has been given a definition of, still alive or
    /// not yet reported dropped.
    delivered: HashMap<u64, Weak<SegmentBody>>,
}

impl SegChannel {
    /// Forget what the host has been given: the next drain defines every
    /// segment it references again.
    pub fn forget_delivered(&mut self) {
        self.delivered.clear();
    }

    /// Number of segments the host holds a definition of.
    pub fn delivered_len(&self) -> usize {
        self.delivered.len()
    }

    /// Build this frame's drain from the buffer's values and this channel's
    /// marks.
    pub fn take(&mut self, values: Vec<Value>) -> SegmentedFrame {
        let marks = std::mem::take(&mut self.marks);
        let mut frame = SegmentedFrame::default();
        frame.items = items_of(&values, &marks);
        for m in &marks {
            self.define(&m.body, &mut frame.defined);
        }
        drop(marks);
        self.delivered.retain(|serial, body| {
            let alive = body.strong_count() > 0;
            if !alive {
                frame.dropped.push(*serial);
            }
            alive
        });
        frame.dropped.sort_unstable();
        frame
    }

    /// Add `body`'s definition to `out` unless the host has it, after its
    /// children's.
    fn define(&mut self, body: &Arc<SegmentBody>, out: &mut Vec<SegmentDef>) {
        if self.delivered.contains_key(&body.serial) {
            return;
        }
        for m in &body.marks {
            self.define(&m.body, out);
        }
        self.delivered.insert(body.serial, Arc::downgrade(body));
        out.push(SegmentDef {
            id: body.id,
            serial: body.serial,
            items: items_of(&body.values, &body.marks),
        });
    }
}

fn items_of(values: &[Value], marks: &[SegMark]) -> Vec<SegItem> {
    let mut items = Vec::with_capacity(values.len() + marks.len());
    let mut i = 0;
    for m in marks {
        items.extend(values[i..m.at].iter().map(|v| SegItem::Value(*v)));
        items.push(SegItem::Segment(m.body.serial));
        i = m.at;
    }
    items.extend(values[i..].iter().map(|v| SegItem::Value(*v)));
    items
}

/// One entry of a segmented buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegItem {
    /// A value emitted fresh: by code outside any recorded scope, or by a
    /// scope recorded this frame (inside a [`SegmentDef`]).
    Value(Value),
    /// Everything segment `serial` holds, in its place.
    Segment(u64),
}

/// A segment the host has not been given before.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentDef {
    /// The scope (see the module docs, "Identity").
    pub id: u64,
    /// This recording; what [`SegItem::Segment`] refers to.
    pub serial: u64,
    pub items: Vec<SegItem>,
}

/// One drain of a segmented buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentedFrame {
    /// The buffer, in order.
    pub items: Vec<SegItem>,
    /// Definitions of the segments referenced (here or inside another
    /// definition) that the host has not been given, children first.
    pub defined: Vec<SegmentDef>,
    /// Serials no record or buffer refers to any more: the host may forget
    /// them. Sorted.
    pub dropped: Vec<u64>,
}

/// A host-side cache of segment definitions, for a host that wants the flat
/// buffer back (tests, a host that has not been taught segments yet) and as
/// the reference for what a segmented host must do. Feed it every drain in
/// order.
#[derive(Debug, Default, Clone)]
pub struct SegmentCache {
    defs: HashMap<u64, Vec<SegItem>>,
}

impl SegmentCache {
    /// Take in a drain's definitions and drops, and return its buffer with
    /// every segment expanded: exactly what an unsegmented buffer would have
    /// held. Panics on a reference to a segment never defined, which is a
    /// broken drain.
    pub fn apply(&mut self, frame: &SegmentedFrame) -> Vec<Value> {
        for d in &frame.defined {
            self.defs.insert(d.serial, d.items.clone());
        }
        let mut out = Vec::new();
        self.expand(&frame.items, &mut out);
        for s in &frame.dropped {
            self.defs.remove(s);
        }
        out
    }

    fn expand(&self, items: &[SegItem], out: &mut Vec<Value>) {
        for it in items {
            match it {
                SegItem::Value(v) => out.push(*v),
                SegItem::Segment(s) => {
                    let inner = self
                        .defs
                        .get(s)
                        .unwrap_or_else(|| panic!("segment {s} referenced before it was defined"));
                    self.expand(inner, out);
                }
            }
        }
    }

    /// Number of segments held.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

/// The segmented buffers of one execution context, by symbol. A symbol is
/// segmented while it has an entry.
pub type SegChannels = HashMap<SymbolId, SegChannel>;

/// Flatten a buffer's values with a channel's marks (for a host that turns
/// segmentation off with marks still undrained).
pub fn flatten_buffer(values: &[Value], marks: &[SegMark]) -> Vec<Value> {
    let mut out = Vec::with_capacity(values.len());
    flatten(values, marks, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(values: &[i64], marks: Vec<SegMark>) -> Arc<SegmentBody> {
        Arc::new(SegmentBody::new(
            7,
            values.iter().map(|i| Value::Int(*i)).collect(),
            marks,
        ))
    }

    #[test]
    fn a_drain_defines_once_and_drops_what_died() {
        let child = body(&[2, 3], vec![]);
        let parent = body(
            &[1, 4],
            vec![SegMark {
                at: 1,
                body: child.clone(),
            }],
        );
        let mut ch = SegChannel::default();
        ch.marks.push(SegMark {
            at: 1,
            body: parent.clone(),
        });
        let frame = ch.take(vec![Value::Int(0), Value::Int(5)]);
        assert_eq!(frame.defined.len(), 2);
        assert_eq!(frame.defined[0].serial, child.serial, "children first");
        let mut cache = SegmentCache::default();
        let flat = cache.apply(&frame);
        assert_eq!(flat, (0..6).map(Value::Int).collect::<Vec<_>>());

        // Replayed: the same body again, nothing to define.
        ch.marks.push(SegMark {
            at: 0,
            body: parent.clone(),
        });
        let frame = ch.take(vec![]);
        assert!(frame.defined.is_empty());
        assert_eq!(cache.apply(&frame).len(), 4);

        drop(parent);
        drop(child);
        let frame = ch.take(vec![]);
        assert_eq!(frame.dropped.len(), 2);
        cache.apply(&frame);
        assert!(cache.is_empty());
    }
}
