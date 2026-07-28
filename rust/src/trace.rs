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

use crate::heap::{CellId, Heap};
use crate::program::{Program, TermId, TermOp};
use crate::program_analysis::CellFrontier;
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
    dropped: u64,
}

impl TraceBuffer {
    pub fn new() -> Self {
        Self {
            enabled: false,
            capacity: DEFAULT_CAPACITY,
            events: VecDeque::new(),
            next_sequence: 0,
            dropped: 0,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.events.len() > self.capacity {
            self.events.pop_front();
            self.dropped += 1;
        }
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.next_sequence = 0;
        self.dropped = 0;
    }

    /// How many events the ring buffer has discarded. Derivable from the
    /// oldest surviving sequence, but kept explicit so the difference between
    /// "the write scrolled out of the window" and "nothing ever wrote it" is
    /// a field lookup rather than an inference.
    pub fn dropped(&self) -> u64 {
        self.dropped
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
            self.dropped += 1;
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

    /// Newest event for `term_id` that happened strictly before `seq`.
    ///
    /// Needed once `explain` re-roots across a cell boundary: after hopping to
    /// the write that supplied the value, every term on the continued chain
    /// must be valued as of *that write's* execution, not as of the last loop
    /// iteration.
    pub fn last_for_term_before(&self, term_id: TermId, seq: u64) -> Option<&TraceEvent> {
        self.events
            .iter()
            .rev()
            .find(|e| e.term_id == term_id && e.sequence < seq)
    }

    /// Newest write to `cell` strictly before `seq` — the `CellWrite` (or the
    /// `CellNew` that gave it its initial contents) that a read at `seq`
    /// actually saw.
    ///
    /// Matching is on the `CellId`, not on the declaring term: one declaration
    /// term mints a fresh cell per execution (`state(key) var` per key, and
    /// any `var` inside a function or loop body per entry — §8), so a
    /// static-only answer cannot tell two of them apart.
    ///
    /// `CellWrite` records the cell at `inputs[0]`; `CellNew` records the
    /// *initializer* there and the cell as its `result` (`isa.rs:512`), so the
    /// two need different tests.
    pub fn last_cell_write_before(
        &self,
        program: &Program,
        cell: CellId,
        seq: u64,
    ) -> Option<&TraceEvent> {
        self.events.iter().rev().find(|e| {
            if e.sequence >= seq {
                return false;
            }
            match program.get_term(e.term_id).op {
                TermOp::CellWrite => e.inputs.first() == Some(&Value::Cell(cell)),
                TermOp::CellNew => e.result == Value::Cell(cell),
                _ => false,
            }
        })
    }

    /// Every recorded write to `cell` through one of `write_terms`, oldest
    /// first. Backs the per-write listing in `explain`.
    pub fn var_writes(
        &self,
        program: &Program,
        heap: &Heap,
        write_terms: &[TermId],
        cell: CellId,
    ) -> Vec<WriteRecord> {
        self.events
            .iter()
            .filter(|e| {
                write_terms.contains(&e.term_id) && e.inputs.first() == Some(&Value::Cell(cell))
            })
            .map(|e| {
                let (line, column) = span_of(program, e.term_id);
                WriteRecord {
                    term_id: e.term_id,
                    line,
                    column,
                    seq: e.sequence,
                    value: value::value_to_display_string(&e.result, heap),
                }
            })
            .collect()
    }

    /// Walk backward through provenance from `target` and return each
    /// ancestor's most recent recorded value alongside its source location.
    /// Answers "why does this term have this value?"
    ///
    /// The walk stops at every cell (`Provenance::frontier`), and *re-roots*
    /// across the boundary when the trace can name the write that supplied the
    /// value. So with the trace on the chain is complete and correct across a
    /// `var`; the escape hatch's cost is only paid when the trace is off.
    pub fn explain(
        &self,
        program: &Program,
        heap: &Heap,
        target: TermId,
        max_depth: usize,
    ) -> ExplainResult {
        let index = program.cell_index();
        let mut out: Vec<ExplainEntry> = Vec::new();
        let mut seen: std::collections::HashSet<TermId> = std::collections::HashSet::new();
        let mut complete = true;
        let mut truncated = false;

        // "As of" bound for the whole walk: nothing that ran after the value
        // we are explaining can have contributed to it.
        let root_bound = self
            .last_for_term(target)
            .map(|e| e.sequence + 1)
            .unwrap_or(u64::MAX);

        // (root term, as-of sequence) pairs still to expand. A hop across a
        // resolved boundary pushes the write's value operand with the write's
        // own sequence.
        let mut roots: VecDeque<(TermId, u64)> = VecDeque::new();
        roots.push_back((target, root_bound));

        while let Some((root, bound)) = roots.pop_front() {
            let prov = program.trace_provenance_with(&index, root);
            let mut chain: Vec<TermId> = Vec::with_capacity(prov.ancestors.len() + 1);
            chain.push(root);
            chain.extend(prov.ancestors.iter().copied());

            for tid in chain {
                if !seen.insert(tid) {
                    continue;
                }
                if out.len() >= max_depth {
                    // An unannounced depth cut reads exactly like a completed
                    // chain — the same lie the frontier exists to prevent.
                    truncated = true;
                    complete = false;
                    return ExplainResult {
                        entries: out,
                        truncated,
                        complete,
                    };
                }
                let ev = self.event_for(tid, bound);
                let mut entry = ExplainEntry::from(program, heap, tid, ev);

                for f in prov.frontier.iter().filter(|f| f.read_term == tid) {
                    let boundary = self.resolve_boundary(program, heap, f, bound);
                    if let Resolution::Resolved { seq, .. } = boundary.resolution {
                        let write_value = program.get_term(boundary.write_term.unwrap()).inputs[1];
                        roots.push_back((write_value, seq));
                    } else if let Resolution::Initial { decl_term, .. } = boundary.resolution {
                        // "Never written, still holds its initializer" — the
                        // chain continues into the `CellNew`'s init expression.
                        if let Some(&init) = program.get_term(decl_term).inputs.first() {
                            roots.push_back((init, bound));
                        }
                    } else {
                        complete = false;
                    }
                    entry.boundary = Some(boundary);
                }
                out.push(entry);
            }
        }

        ExplainResult {
            entries: out,
            truncated,
            complete,
        }
    }

    fn event_for(&self, term_id: TermId, bound: u64) -> Option<&TraceEvent> {
        if bound == u64::MAX {
            self.last_for_term(term_id)
        } else {
            self.last_for_term_before(term_id, bound)
        }
    }

    /// Resolve one static [`CellFrontier`] against the trace.
    fn resolve_boundary(
        &self,
        program: &Program,
        heap: &Heap,
        f: &CellFrontier,
        bound: u64,
    ) -> CellBoundary {
        let may_writes = f
            .writes
            .iter()
            .map(|&t| {
                let (line, column) = span_of(program, t);
                Site {
                    term_id: t,
                    line,
                    column,
                }
            })
            .collect();
        let mut boundary = CellBoundary {
            var: f.var_name.clone(),
            decl_term: f.cell_decl,
            captured: f.captured,
            resolution: Resolution::NotTraced,
            write_term: None,
            may_writes,
            host_writable: f.host_writable,
            writes: Vec::new(),
        };
        if !self.enabled && self.is_empty() {
            return boundary;
        }

        // Which cell? A direct read records it at `inputs[0]`; for a capture
        // there is no read here at all, so fall back to the declaration's own
        // recorded result.
        let (cell, at_seq) = if f.captured {
            match f.cell_decl.and_then(|d| self.event_for(d, bound)) {
                Some(e) => match e.result {
                    Value::Cell(c) => (c, bound),
                    _ => return boundary,
                },
                None => {
                    boundary.resolution = self.missing();
                    return boundary;
                }
            }
        } else {
            match self.event_for(f.read_term, bound) {
                Some(e) => match e.inputs.first() {
                    Some(&Value::Cell(c)) => (c, e.sequence),
                    _ => return boundary,
                },
                None => {
                    boundary.resolution = self.missing();
                    return boundary;
                }
            }
        };

        boundary.writes = self.var_writes(program, heap, &f.writes, cell);

        match self.last_cell_write_before(program, cell, at_seq) {
            Some(e) if matches!(program.get_term(e.term_id).op, TermOp::CellNew) => {
                boundary.resolution = Resolution::Initial {
                    decl_term: e.term_id,
                    value: e
                        .inputs
                        .first()
                        .map(|v| value::value_to_display_string(v, heap))
                        .unwrap_or_else(|| "?".to_string()),
                };
            }
            Some(e) => {
                let (line, column) = span_of(program, e.term_id);
                boundary.write_term = Some(e.term_id);
                boundary.resolution = Resolution::Resolved {
                    line,
                    column,
                    seq: e.sequence,
                    value: value::value_to_display_string(&e.result, heap),
                };
            }
            None => {
                // The cell exists (we saw its id) but no write survives in the
                // window. That is eviction, not "never written".
                boundary.resolution = self.missing();
            }
        }
        boundary
    }

    /// Why an event we expected to find is absent. Once the ring buffer has
    /// discarded anything, "never ran" is no longer a claim the trace can
    /// make — and the two render identically unless they are kept apart.
    fn missing(&self) -> Resolution {
        if self.dropped > 0 {
            Resolution::Evicted
        } else {
            Resolution::NotExecuted
        }
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

/// A source location for a `set` site.
#[derive(Debug, Clone)]
pub struct Site {
    pub term_id: TermId,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// One recorded write to a cell.
#[derive(Debug, Clone)]
pub struct WriteRecord {
    pub term_id: TermId,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub seq: u64,
    pub value: String,
}

/// What the trace could say about which write supplied a cell read's value.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// A `set` wrote it, and we know which one.
    Resolved {
        line: Option<u32>,
        column: Option<u32>,
        seq: u64,
        value: String,
    },
    /// Never written — the cell still holds the value its declaration gave it.
    Initial { decl_term: TermId, value: String },
    /// The write happened, but scrolled out of the ring buffer.
    Evicted,
    /// No trace was recorded, so only the static write set is available.
    NotTraced,
    /// The read itself never ran.
    NotExecuted,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::Resolved { .. } => "resolved",
            Resolution::Initial { .. } => "initial",
            Resolution::Evicted => "evicted",
            Resolution::NotTraced => "not_traced",
            Resolution::NotExecuted => "not_executed",
        }
    }
}

/// A [`CellFrontier`] resolved against the trace. The static half stays in
/// `program_analysis`; this adds only what running the program can tell you.
#[derive(Debug, Clone)]
pub struct CellBoundary {
    pub var: Option<String>,
    pub decl_term: Option<TermId>,
    pub captured: bool,
    pub resolution: Resolution,
    /// The `CellWrite` named by a `Resolved` resolution.
    pub write_term: Option<TermId>,
    /// Every write site that *could* have supplied the value (§6d: complete).
    pub may_writes: Vec<Site>,
    /// A `state var`'s slot is writable by the host too, so `may_writes` is
    /// not the whole story for it.
    pub host_writable: bool,
    /// Every recorded write to this exact cell, in order.
    pub writes: Vec<WriteRecord>,
}

impl CellBoundary {
    /// The one-line summary rendered under the chain entry.
    pub fn summary(&self) -> String {
        let what = match (&self.var, self.captured) {
            (Some(v), false) => format!("read of var '{}'", v),
            (Some(v), true) => format!("closure captures var '{}'", v),
            (None, false) => "read of an unresolved cell".to_string(),
            (None, true) => "closure captures an unresolved cell".to_string(),
        };
        let tail = match &self.resolution {
            Resolution::Resolved {
                line: Some(l),
                column: Some(c),
                ..
            } => format!(
                "written by '{}' at line {}, column {}; chain continues from there",
                self.var
                    .as_ref()
                    .map(|v| format!("set {}", v))
                    .unwrap_or_else(|| "set".into()),
                l,
                c
            ),
            Resolution::Resolved { .. } => "written by a `set` with no source location".to_string(),
            Resolution::Initial { value, .. } => format!(
                "no `set` had run yet; still holds its initial value {}",
                value
            ),
            Resolution::Evicted => format!(
                "writer unknown (older than the trace window); {}",
                self.sites_phrase()
            ),
            Resolution::NotTraced => format!("not traced; {}", self.sites_phrase()),
            Resolution::NotExecuted => format!("never executed; {}", self.sites_phrase()),
        };
        let host = if self.host_writable {
            " (also writable by the host through set_state)"
        } else {
            ""
        };
        format!("{} — {}{}", what, tail, host)
    }

    fn sites_phrase(&self) -> String {
        if self.may_writes.is_empty() {
            return "no write sites".to_string();
        }
        let lines: Vec<String> = self
            .may_writes
            .iter()
            .map(|s| match s.line {
                Some(l) => format!("line {}", l),
                None => format!("t{}", s.term_id.0),
            })
            .collect();
        format!(
            "{} possible write site{}: {}",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" },
            lines.join(", ")
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        let last_write = match &self.resolution {
            Resolution::Resolved {
                line,
                column,
                seq,
                value,
            } => serde_json::json!({
                "term_id": self.write_term.map(|t| t.0),
                "line": line,
                "column": column,
                "seq": seq,
                "value": value,
            }),
            Resolution::Initial { decl_term, value } => serde_json::json!({
                "term_id": decl_term.0,
                "line": serde_json::Value::Null,
                "column": serde_json::Value::Null,
                "seq": serde_json::Value::Null,
                "value": value,
            }),
            _ => serde_json::Value::Null,
        };
        serde_json::json!({
            "var": self.var,
            "decl_term": self.decl_term.map(|t| t.0),
            "captured": self.captured,
            "resolution": self.resolution.as_str(),
            "last_write": last_write,
            "may_writes": self.may_writes.iter().map(|s| serde_json::json!({
                "term_id": s.term_id.0, "line": s.line, "column": s.column,
            })).collect::<Vec<_>>(),
            "host_writable": self.host_writable,
            "writes": self.writes.iter().map(|w| serde_json::json!({
                "term_id": w.term_id.0, "line": w.line, "column": w.column,
                "seq": w.seq, "value": w.value,
            })).collect::<Vec<_>>(),
            "summary": self.summary(),
        })
    }
}

/// The chain `explain` produced, plus whether it is the whole story.
/// `complete` is false when a cell boundary could not be resolved; `truncated`
/// when `max_depth` cut it short. Both had to become fields rather than
/// conventions — a chain that just ends reads as a chain that finished.
#[derive(Debug, Clone)]
pub struct ExplainResult {
    pub entries: Vec<ExplainEntry>,
    pub truncated: bool,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct ExplainEntry {
    pub term_id: TermId,
    pub name: Option<String>,
    pub op: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub value: Option<String>,
    /// Set when the backward walk stopped here because it reached a cell.
    pub boundary: Option<CellBoundary>,
}

impl ExplainEntry {
    fn from(program: &Program, heap: &Heap, term_id: TermId, event: Option<&TraceEvent>) -> Self {
        let term = program.get_term(term_id);
        let (line, column) = span_of(program, term_id);
        Self {
            term_id,
            name: term.name.clone(),
            op: format!("{:?}", term.op),
            line,
            column,
            value: event.map(|e| value::value_to_display_string(&e.result, heap)),
            boundary: None,
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
            "boundary": self.boundary.as_ref().map(|b| b.to_json()),
        })
    }
}

/// `(line, column)` for a term, or `(None, None)` for a phantom span.
fn span_of(program: &Program, term_id: TermId) -> (Option<u32>, Option<u32>) {
    match program.source_map.get(term_id) {
        Some(s) if s.start.line > 0 => (Some(s.start.line), Some(s.start.column)),
        _ => (None, None),
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

/// Cell-boundary resolution (§6e). These run real programs: the facts under
/// test are which `CellId` a read saw and which write preceded it, and a
/// synthetic event stream would be asserting the fixture rather than the code.
#[cfg(test)]
mod cell_boundary_tests {
    use super::*;
    use crate::env::Env;
    use crate::program::ProgramId;

    fn run(source: &str, capacity: Option<usize>) -> (Env, ProgramId) {
        let mut env = Env::new();
        env.trace_mut().enable();
        if let Some(c) = capacity {
            env.trace_mut().set_capacity(c);
        }
        let pid = env.load_program(source).expect("compiles");
        let sid = env.create_stack(pid).expect("stack");
        env.run(sid).expect("runs");
        (env, pid)
    }

    fn cell_of(env: &Env, term: TermId) -> CellId {
        match env.trace().last_for_term(term).expect("ran").inputs[0] {
            Value::Cell(c) => c,
            v => panic!("expected a cell operand, got {:?}", v),
        }
    }

    fn terms_with(program: &Program, want: fn(&TermOp) -> bool) -> Vec<TermId> {
        program
            .terms
            .iter()
            .filter(|t| want(&t.op))
            .map(|t| t.id)
            .collect()
    }

    /// A `CellNew` records the *initializer* at `inputs[0]` and the cell as its
    /// `result` (`isa.rs:512`), so a scan keyed on `inputs[0]` would never see
    /// it — and "never written" would render as "never executed".
    #[test]
    fn last_cell_write_before_picks_the_write_not_the_new() {
        let (env, pid) = run("var x = 0\nset x = 1\nlet y = x * 2\n", None);
        let p = env.get_program(pid).expect("program");
        let write = terms_with(p, |o| matches!(o, TermOp::CellWrite))[0];
        let read = *terms_with(p, |o| matches!(o, TermOp::CellRead))
            .last()
            .unwrap();
        let cell = cell_of(&env, read);
        let read_seq = env.trace().last_for_term(read).unwrap().sequence;

        let found = env
            .trace()
            .last_cell_write_before(p, cell, read_seq)
            .expect("a write precedes the read");
        assert_eq!(found.term_id, write);
    }

    #[test]
    fn last_cell_write_before_finds_the_new_when_never_written() {
        let (env, pid) = run("var x = 7\nlet y = x * 2\n", None);
        let p = env.get_program(pid).expect("program");
        let new = terms_with(p, |o| matches!(o, TermOp::CellNew))[0];
        let read = terms_with(p, |o| matches!(o, TermOp::CellRead))[0];
        let cell = cell_of(&env, read);
        let read_seq = env.trace().last_for_term(read).unwrap().sequence;

        let found = env
            .trace()
            .last_cell_write_before(p, cell, read_seq)
            .expect("the CellNew is visible");
        assert_eq!(found.term_id, new);

        // And it surfaces as `Initial`, not `NotExecuted` — "never written,
        // still holds its initializer" is a different fact.
        let result = env
            .trace()
            .explain(p, env.heap(), p.find_term("y").unwrap(), 16);
        let b = result
            .entries
            .iter()
            .find_map(|e| e.boundary.as_ref())
            .expect("a boundary");
        assert!(matches!(b.resolution, Resolution::Initial { .. }));
        assert!(result.complete);
    }

    /// One declaration term, two cells. A static-only answer cannot tell the
    /// two keys apart; the recorded `CellId` can. The same conflation applies
    /// to any `var` declared inside a function or loop body (§8).
    #[test]
    fn last_cell_write_before_is_per_cell() {
        let (env, pid) = run(
            "fn touch(k, n)\n  state(k) var h = 0\n  set h = n\n  h\nend\n\
             print(touch(1, 10))\nprint(touch(2, 20))\nprint(touch(1, 11))\n",
            None,
        );
        let p = env.get_program(pid).expect("program");
        let write = terms_with(p, |o| matches!(o, TermOp::CellWrite))[0];

        // The two keys mint two cells from the one `set` site.
        let mut seen: Vec<(CellId, u64)> = Vec::new();
        for e in env.trace().all_for_term(write) {
            if let Value::Cell(c) = e.inputs[0] {
                seen.push((c, e.sequence));
            }
        }
        assert_eq!(seen.len(), 3);
        let cell_a = seen[0].0;
        let cell_b = seen[1].0;
        assert_ne!(cell_a, cell_b);
        assert_eq!(seen[2].0, cell_a, "key 1 reuses its cell");

        // The last write to key 2's cell is the *middle* event, not the last.
        let last_b = env
            .trace()
            .last_cell_write_before(p, cell_b, u64::MAX)
            .unwrap();
        assert_eq!(last_b.sequence, seen[1].1);
        let last_a = env
            .trace()
            .last_cell_write_before(p, cell_a, u64::MAX)
            .unwrap();
        assert_eq!(last_a.sequence, seen[2].1);
    }

    #[test]
    fn evicted_write_reports_evicted_not_never_written() {
        let (env, pid) = run("var x = 0\nset x = 1\nlet y = x * 2\n", Some(2));
        let p = env.get_program(pid).expect("program");
        assert!(env.trace().dropped() > 0);

        let read = *terms_with(p, |o| matches!(o, TermOp::CellRead))
            .last()
            .unwrap();
        // The read itself has scrolled out too, so ask about the cell directly.
        let write = terms_with(p, |o| matches!(o, TermOp::CellWrite))[0];
        assert!(env.trace().last_for_term(write).is_none());
        assert!(env.trace().last_for_term(read).is_none());

        let result = env
            .trace()
            .explain(p, env.heap(), p.find_term("y").unwrap(), 16);
        let b = result
            .entries
            .iter()
            .find_map(|e| e.boundary.as_ref())
            .expect("a boundary");
        assert!(
            matches!(b.resolution, Resolution::Evicted),
            "got {:?}",
            b.resolution
        );
        assert!(!result.complete);
        // Whatever the trace could not see, the static write set survives.
        assert_eq!(b.may_writes.len(), 1);
    }

    #[test]
    fn explain_truncation_is_announced() {
        let (env, pid) = run("let a = 1 + 2\nlet b = a * 3\nlet y = b - 4\n", None);
        let p = env.get_program(pid).expect("program");
        let result = env
            .trace()
            .explain(p, env.heap(), p.find_term("y").unwrap(), 2);
        assert!(result.truncated);
        assert!(!result.complete);
        assert_eq!(result.entries.len(), 2);
    }
}
