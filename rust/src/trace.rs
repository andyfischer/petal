//! Trace - Structured execution trace buffer.
//!
//! Records per-term evaluation events (term id, inputs, result) into a
//! bounded ring buffer. Off by default; when disabled, `push()` is a single
//! bool check on the hot path. When enabled, the buffer is queryable for
//! post-hoc debugging ("why does term X have value Y?") and serializable to
//! JSON for external tools.
//!
//! See docs/debugging-visibility.md for the end-user story.

use std::collections::VecDeque;

use smallvec::SmallVec;

use crate::heap::Heap;
use crate::program::{Program, TermId, TermOp};
use crate::value::{self, Value};

/// Default ring buffer capacity. ~100 bytes/event × 200k = ~20 MB worst case.
/// The buffer drops the oldest events once full so long-running programs
/// don't OOM.
pub const DEFAULT_CAPACITY: usize = 200_000;

#[derive(Debug, Clone)]
pub struct TraceEvent {
    pub sequence: u64,
    pub term_id: TermId,
    pub inputs: SmallVec<[Value; 4]>,
    pub result: Value,
}

pub struct TraceBuffer {
    pub enabled: bool,
    capacity: usize,
    events: VecDeque<TraceEvent>,
    next_sequence: u64,
}

impl TraceBuffer {
    pub fn new() -> Self {
        Self {
            enabled: false,
            capacity: DEFAULT_CAPACITY,
            events: VecDeque::new(),
            next_sequence: 0,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.next_sequence = 0;
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Record an event. Cheap when disabled — one bool check.
    #[inline]
    pub fn push(&mut self, term_id: TermId, inputs: &[Value], result: Value) {
        if !self.enabled {
            return;
        }
        if self.events.len() >= self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(TraceEvent {
            sequence: self.next_sequence,
            term_id,
            inputs: SmallVec::from_slice(inputs),
            result,
        });
        self.next_sequence += 1;
    }

    pub fn events(&self) -> impl Iterator<Item = &TraceEvent> {
        self.events.iter()
    }

    /// Most recent event for a given term id, or None if the term never ran.
    pub fn last_for_term(&self, term_id: TermId) -> Option<&TraceEvent> {
        self.events.iter().rev().find(|e| e.term_id == term_id)
    }

    /// All events for a given term, oldest first.
    pub fn all_for_term(&self, term_id: TermId) -> impl Iterator<Item = &TraceEvent> {
        self.events.iter().filter(move |e| e.term_id == term_id)
    }

    /// Return every recorded write to the variable bound at `target`.
    /// This handles the common case where a name like `total` is reassigned
    /// in a loop: the language compiles those reassignments into `Assign(target)`
    /// terms that write to the original term's register, and this method
    /// walks the trace in order collecting every such write (plus the
    /// initialization itself). Returns an empty vec if there were no
    /// reassignments — callers can fall back to `explain` in that case.
    pub fn history(
        &self,
        program: &Program,
        heap: &Heap,
        target: TermId,
    ) -> Vec<HistoryEntry> {
        // Collect all terms that write to `target`'s value: the target itself
        // (initial assignment) plus every `Assign(target)` term.
        let mut writers: Vec<TermId> = vec![target];
        for term in &program.terms {
            if let TermOp::Assign(t) = &term.op {
                if *t == target {
                    writers.push(term.id);
                }
            }
        }
        let assigns_count = writers.len() - 1;

        // Walk trace events in sequence order, collecting writes.
        let mut entries = Vec::new();
        for event in self.events.iter() {
            if !writers.contains(&event.term_id) {
                continue;
            }
            let term = program.get_term(event.term_id);
            let (value, kind) = match &term.op {
                TermOp::Assign(_) => (
                    event.inputs.first().copied().unwrap_or(Value::Nil),
                    HistoryKind::Reassign,
                ),
                _ => (event.result, HistoryKind::Initial),
            };
            // Prefer the term's own source span; fall back to its first input
            // (e.g. an Assign term inside a loop body has no span of its own,
            // but its RHS expression term carries the assignment's location).
            let mut span = program.source_map.get(event.term_id);
            if span.map(|s| s.start.line == 0).unwrap_or(true) {
                if let Some(&input_id) = term.inputs.first() {
                    if let Some(s) = program.source_map.get(input_id) {
                        if s.start.line > 0 {
                            span = Some(s);
                        }
                    }
                }
            }
            let (line, column) = match span {
                Some(s) if s.start.line > 0 => (Some(s.start.line), Some(s.start.column)),
                _ => (None, None),
            };
            entries.push(HistoryEntry {
                sequence: event.sequence,
                term_id: event.term_id,
                kind,
                value: value::value_to_display_string(&value, heap),
                line,
                column,
            });
        }

        // If there were no `Assign` writers, history is the same as just the
        // initial term — let callers decide whether that's worth showing.
        if assigns_count == 0 {
            return Vec::new();
        }
        entries
    }

    /// Walk backward through provenance from `target` and return each
    /// ancestor's most recent recorded value alongside its source location.
    /// Answers "why does this term have this value?"
    pub fn explain(
        &self,
        program: &Program,
        heap: &Heap,
        target: TermId,
        max_depth: usize,
    ) -> Vec<ExplainEntry> {
        let mut out = Vec::new();
        // Include the target itself first.
        if let Some(ev) = self.last_for_term(target) {
            out.push(ExplainEntry::from(program, heap, target, Some(ev)));
        } else {
            out.push(ExplainEntry::from(program, heap, target, None));
        }

        let (ancestors, _edges) = program.trace_provenance(target);
        for aid in ancestors {
            if out.len() >= max_depth {
                break;
            }
            let ev = self.last_for_term(aid);
            out.push(ExplainEntry::from(program, heap, aid, ev));
        }
        out
    }

    /// Serialize the buffer to a JSON value using program + heap for
    /// human-readable names and value strings.
    pub fn to_json(&self, program: &Program, heap: &Heap) -> serde_json::Value {
        let events: Vec<_> = self
            .events
            .iter()
            .map(|e| event_to_json(e, program, heap))
            .collect();
        serde_json::json!({
            "capacity": self.capacity,
            "count": events.len(),
            "events": events,
        })
    }
}

impl Default for TraceBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryKind {
    Initial,
    Reassign,
}

impl HistoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HistoryKind::Initial => "init",
            HistoryKind::Reassign => "set",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub sequence: u64,
    pub term_id: TermId,
    pub kind: HistoryKind,
    pub value: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl HistoryEntry {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "seq": self.sequence,
            "term_id": self.term_id.0,
            "kind": self.kind.as_str(),
            "value": self.value,
            "line": self.line,
            "column": self.column,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExplainEntry {
    pub term_id: TermId,
    pub name: Option<String>,
    pub op: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub value: Option<String>,
}

impl ExplainEntry {
    fn from(
        program: &Program,
        heap: &Heap,
        term_id: TermId,
        event: Option<&TraceEvent>,
    ) -> Self {
        let term = program.get_term(term_id);
        let span = program.source_map.get(term_id);
        let (line, column) = match span {
            Some(s) if s.start.line > 0 => (Some(s.start.line), Some(s.start.column)),
            _ => (None, None),
        };
        Self {
            term_id,
            name: term.name.clone(),
            op: format!("{:?}", term.op),
            line,
            column,
            value: event.map(|e| value::value_to_display_string(&e.result, heap)),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "term_id": self.term_id.0,
            "name": self.name,
            "op": self.op,
            "line": self.line,
            "column": self.column,
            "value": self.value,
        })
    }
}

fn event_to_json(e: &TraceEvent, program: &Program, heap: &Heap) -> serde_json::Value {
    let term = program.get_term(e.term_id);
    let span = program.source_map.get(e.term_id);
    let (line, column) = match span {
        Some(s) if s.start.line > 0 => (Some(s.start.line), Some(s.start.column)),
        _ => (None, None),
    };
    serde_json::json!({
        "seq": e.sequence,
        "term_id": e.term_id.0,
        "name": term.name,
        "op": format!("{:?}", term.op),
        "line": line,
        "column": column,
        "inputs": e.inputs.iter().map(|v| value::value_to_display_string(v, heap)).collect::<Vec<_>>(),
        "result": value::value_to_display_string(&e.result, heap),
    })
}
