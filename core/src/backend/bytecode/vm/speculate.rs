//! `speculate(f, ...args)`: a hypothetical call (docs/features/speculate.md,
//! docs/proposals/declarative-dragging.md §P1).
//!
//! The call runs `f` on the live VM, against the live heap and state, and
//! then puts back everything it changed. It is a **write log**, not a fork:
//!
//! - **State.** The first time the call writes a slot (`StateWrite`, which
//!   is also what commits a `StateInit`), the slot's value before the call
//!   (or its absence) goes into [`Speculation::state_undo`]. Reads see the
//!   live slot until the call writes it, and the call's own value after, so
//!   a slot first reached inside the call is initialized there and read
//!   back from there. On return every logged slot is restored or removed.
//! - **Cells.** A `set` on a cell the call did not create logs the cell's
//!   contents the same way. A cell the call created needs no log: nothing
//!   outside the call can reach it once the call is over (unless the call
//!   returns it, and then its contents are the call's to keep).
//! - **The heap is shared.** Values are immutable, so the only things a call
//!   could change under the live program are slots, cells and the containers
//!   the optimizer edits in place. For the last, the heap stamps every
//!   object with the speculative call it was allocated in
//!   ([`Heap::begin_speculation`](crate::heap::Heap::begin_speculation)):
//!   while the call runs, an in-place edit of an older object copies it, so
//!   the call edits its own values in place and never the live program's. A
//!   `set xs[i] = v` through an older cell reads the cell's container as
//!   shared for the same reason.
//! - **Natives** follow the speculative-run rules of direct manipulation
//!   (docs/dev/native-effects.md): one whose effect escapes the run (or a
//!   host handle's method) is not called and answers `nil`, with a warning
//!   once per call site ([`Stack::runtime_warnings`]); a contained effect
//!   runs and is undone (the print buffer, the noise seed, the counters);
//!   what the call emits, on any channel, is taken out of the buffers and
//!   returned with its origin; random draws come from a copy of the stream,
//!   which is put back, so the live stream does not advance.
//! - **Memo.** No memo scope opens inside the call and none records what it
//!   does, and a scope around the call is marked effectful (it cannot be
//!   replayed, since nothing the call read was recorded). A record made
//!   outside could in principle be replayed inside, but records are keyed by
//!   call path and every path inside the call passes through the
//!   `speculate` call's own site, which no live record has; so replaying is
//!   simply off.
//! - **Nesting** is an error.
//!
//! `speculating()` and `speculate_place(key, value)` are the two natives a
//! library uses inside the call: the second appends `{key, value}` to the
//! call's `places`, which `speculate` returns with the value and the emits.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::heap::CellId;
use crate::stack::RuntimeStateKey;

/// The running speculative call's record, kept on the stack
/// ([`Stack::speculation`]): what to put back, and what the call recorded.
#[derive(Debug, Clone, Default)]
pub struct Speculation {
    /// Each state slot the call wrote, with what it held before (`None`: the
    /// slot did not exist). One entry per slot, the first write's.
    state_undo: Vec<(RuntimeStateKey, Option<Value>)>,
    state_seen: HashSet<RuntimeStateKey>,
    /// Each older cell the call wrote, with what it held before.
    cell_undo: Vec<(CellId, Value)>,
    cell_seen: HashSet<CellId>,
    /// `speculate_place(key, value)` calls, in order.
    places: Vec<(Value, Value)>,
}

impl Speculation {
    /// The values the record holds: the undo log's old values, which the
    /// live program gets back, and the places.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for (_, v) in &self.state_undo {
            if let Some(v) = v {
                mark(*v);
            }
        }
        for (_, v) in &self.cell_undo {
            mark(*v);
        }
        for (k, v) in &self.places {
            mark(*k);
            mark(*v);
        }
    }
}

/// What a speculative call saves of the run around it and puts back.
struct Saved {
    memo: bool,
    echo: bool,
    trace_emit: bool,
    observe: bool,
    rng: u64,
    noise: u64,
    counters: HashMap<SymbolId, u64>,
    output: usize,
    /// Each channel's buffer length, and its origins' length.
    buffers: Vec<(SymbolId, usize, usize)>,
    callback: Option<crate::stack::BuiltinCallback>,
}

/// The message for a `speculate` inside another.
pub(crate) const NESTED_MESSAGE: &str =
    "speculate() cannot call itself: a speculative call is already running";

impl<'a> Vm<'a> {
    /// Whether a speculative call is running.
    #[inline(always)]
    pub(super) fn speculating(&self) -> bool {
        self.stack.speculation.is_some()
    }

    /// A speculative call is about to write slot `key`: log what it holds
    /// now, the first time.
    #[cold]
    #[inline(never)]
    pub(super) fn speculate_log_state(&mut self, key: &RuntimeStateKey) {
        let old = self.stack.state.get(key).copied();
        if let Some(spec) = self.stack.speculation.as_deref_mut()
            && !spec.state_seen.contains(key)
        {
            spec.state_seen.insert(key.clone());
            spec.state_undo.push((key.clone(), old));
        }
    }

    /// A speculative call is about to write (or edit) cell `id`: log what it
    /// holds now, the first time, unless the call created the cell. Returns
    /// whether the cell is older than the call.
    #[cold]
    #[inline(never)]
    pub(super) fn speculate_log_cell(&mut self, id: CellId) -> bool {
        if self.heap.cell_born_in_speculation(id) {
            return false;
        }
        let old = self.heap.cell_read(id);
        if let Some(spec) = self.stack.speculation.as_deref_mut()
            && spec.cell_seen.insert(id)
        {
            spec.cell_undo.push((id, old));
        }
        true
    }

    /// A native (or a handle method, `name`) a speculative call does not
    /// make: its effect escapes the run. It answers `nil`, and the first time
    /// at each call site a warning is kept for the host.
    #[cold]
    #[inline(never)]
    pub(super) fn speculate_decline(&mut self, name: &str, origin: Option<TermId>) -> Value {
        let key = origin.unwrap_or(TermId(u32::MAX));
        if self.stack.speculate_warned.insert(key) {
            let at = origin
                .and_then(|t| self.location_of(t))
                .map(|(file, line, col)| format!("\n  --> {file}:{line}:{col}"))
                .unwrap_or_default();
            self.stack.runtime_warnings.push(format!(
                "speculate(): `{name}` was not called (its effect escapes the run){at}"
            ));
        }
        Value::Nil
    }

    /// A term's position as `(file, line, column)`, when it has one.
    fn location_of(&self, term: TermId) -> Option<(String, u32, u32)> {
        let span = self.program.source_map.get(term)?;
        if span.start.line == 0 {
            return None;
        }
        let file = self
            .program
            .source_map
            .file_name_for_span(span)
            .map(str::to_string)
            .unwrap_or_else(|| self.program.entry_name.clone());
        Some((file, span.start.line, span.start.column))
    }

    /// `speculating()`.
    pub(super) fn builtin_speculating(&mut self, args: &[Value]) -> Result<Value, String> {
        if !args.is_empty() {
            return Err(format!("speculating() expects no arguments, got {}", args.len()));
        }
        Ok(Value::Bool(self.speculating()))
    }

    /// `speculate_place(key, value)`: record the pair into the running
    /// call's `places`, and return `value`. The key is checked the way a
    /// `for … key` key is, inside a call or not, so a key that would be
    /// refused there is refused here before it gets as far as a drag.
    pub(super) fn builtin_speculate_place(&mut self, args: &[Value]) -> Result<Value, String> {
        let [key, value] = args else {
            return Err(format!(
                "speculate_place() expects 2 arguments (key, value), got {}",
                args.len()
            ));
        };
        crate::dict::key_hash(*key, self.heap).map_err(|e| e.replace("as a map key", "as a place key"))?;
        if let Some(spec) = self.stack.speculation.as_deref_mut() {
            spec.places.push((*key, *value));
        }
        Ok(*value)
    }

    /// `speculate(f, ...args)`. See the module docs.
    pub(super) fn builtin_speculate(&mut self, args: &[Value], origin: Option<TermId>) -> Result<Value, String> {
        let Some((&callee, call_args)) = args.split_first() else {
            return Err("speculate() expects a function to call, then its arguments".into());
        };
        // A pending function is absorbed, as a call of one would be.
        if matches!(callee, Value::Pending(_)) {
            self.note_absorption(callee, origin);
            return Ok(callee);
        }
        if self.speculating() {
            let detail = crate::backend::errors::ErrorDetail {
                at: None,
                related: None,
                help: Some("compute the candidate scenes first, then compare them outside".into()),
            };
            self.error_already_annotated = true;
            return Err(self.annotate_with(NESTED_MESSAGE.into(), origin, &detail));
        }
        // Nothing the call reads is recorded, so a scope around it cannot be
        // replayed: it would hand back a result computed from reads it never
        // checked.
        self.memo_note_effect();
        let saved = self.speculate_enter();
        let site = self.site_of(origin);
        let result = self.call_closure_sync(callee, call_args, site);
        let (emits, places) = self.speculate_exit(saved);
        let value = result?;
        Ok(self.speculate_result(value, emits, places))
    }

    /// Save the run around a speculative call and start its record.
    fn speculate_enter(&mut self) -> Saved {
        let buffers = self
            .output_buffers
            .iter()
            .map(|(sym, buf)| {
                let origins = self.emit_origins.get(sym).map_or(0, Vec::len);
                (*sym, buf.len(), origins)
            })
            .collect();
        let saved = Saved {
            memo: self.memo,
            echo: self.echo,
            trace_emit: self.trace_emit,
            observe: self.observations.enabled,
            rng: *self.rng_state,
            noise: *self.noise_seed,
            counters: self.counters.clone(),
            output: self.output.len(),
            buffers,
            // A builtin's callback forbids `state` because the callback has no
            // place to keep it from one run to the next; nothing the call
            // keeps outlives it, so its own `state` is allowed again.
            callback: self.stack.callback.take(),
        };
        self.memo = false;
        self.echo = false;
        // Emits record where they came from, to be handed back with it.
        self.trace_emit = true;
        // What a hypothetical call computes is not what the program's names
        // held this run.
        self.observations.enabled = false;
        self.heap.begin_speculation();
        self.stack.speculation = Some(Box::default());
        saved
    }

    /// Put back everything the call changed, and take what it emitted out of
    /// the buffers: per channel, in channel-name order, the values and their
    /// origins. Runs whether or not the call failed.
    #[allow(clippy::type_complexity)]
    fn speculate_exit(
        &mut self,
        saved: Saved,
    ) -> (Vec<(SymbolId, Vec<(Value, crate::execution_context::EmitSite)>)>, Vec<(Value, Value)>) {
        let spec = self.stack.speculation.take().map(|b| *b).unwrap_or_default();
        self.heap.end_speculation();
        for (key, old) in spec.state_undo {
            match old {
                Some(v) => {
                    self.stack.state.insert(key, v);
                }
                None => {
                    self.stack.state.remove(&key);
                }
            }
        }
        for (cell, old) in spec.cell_undo {
            self.heap.cell_write(cell, old);
        }
        // The emits: everything past each buffer's length at entry.
        let start: HashMap<SymbolId, (usize, usize)> =
            saved.buffers.iter().map(|&(s, b, o)| (s, (b, o))).collect();
        let mut emits = Vec::new();
        for (sym, buf) in self.output_buffers.iter_mut() {
            let (from, origins_from) = start.get(sym).copied().unwrap_or((0, 0));
            if buf.len() <= from {
                continue;
            }
            let values = buf.split_off(from);
            let mut origins = match self.emit_origins.get_mut(sym) {
                Some(o) if o.len() > from => {
                    let tail = o.split_off(from);
                    o.truncate(origins_from);
                    tail
                }
                Some(o) => {
                    o.truncate(origins_from);
                    Vec::new()
                }
                None => Vec::new(),
            };
            origins.resize_with(values.len(), Default::default);
            emits.push((*sym, values.into_iter().zip(origins).collect::<Vec<_>>()));
        }
        // Channels in name order: the buffers are a hash map, and the result
        // must not depend on its iteration order.
        let symbols = &*self.symbols;
        emits.sort_by(|a, b| symbols.name(a.0).cmp(&symbols.name(b.0)));
        self.output.truncate(saved.output);
        *self.rng_state = saved.rng;
        *self.noise_seed = saved.noise;
        *self.counters = saved.counters;
        self.memo = saved.memo;
        self.echo = saved.echo;
        self.trace_emit = saved.trace_emit;
        self.observations.enabled = saved.observe;
        self.stack.callback = saved.callback;
        (emits, spec.places)
    }

    /// `{value, emits, places}`: `emits` is `[{channel, value, at}]`, where
    /// `at` is `{file, line, column}` of the call that emitted (the first
    /// frame outside the prelude, as an author would point at it) or nil;
    /// `places` is `[{key, value}]`.
    fn speculate_result(
        &mut self,
        value: Value,
        emits: Vec<(SymbolId, Vec<(Value, crate::execution_context::EmitSite)>)>,
        places: Vec<(Value, Value)>,
    ) -> Value {
        let mut emit_values = Vec::new();
        for (sym, items) in emits {
            let channel = self.symbols.name(sym).unwrap_or("").to_string();
            let channel = Value::String(self.heap.alloc_string(channel));
            for (v, site) in items {
                let at = site
                    .chain
                    .iter()
                    .copied()
                    .find(|t| !self.in_prelude(*t) && self.location_of(*t).is_some())
                    .or_else(|| site.leaf())
                    .and_then(|t| self.location_of(t));
                let at = match at {
                    Some((file, line, column)) => {
                        let file = Value::String(self.heap.alloc_string(file));
                        self.record(&[
                            ("file", file),
                            ("line", Value::Int(line as i64)),
                            ("column", Value::Int(column as i64)),
                        ])
                    }
                    None => Value::Nil,
                };
                let rec = self.record(&[("channel", channel), ("value", v), ("at", at)]);
                emit_values.push(rec);
            }
        }
        let place_values: Vec<Value> = places
            .into_iter()
            .map(|(k, v)| self.record(&[("key", k), ("value", v)]))
            .collect();
        let emits = Value::List(self.heap.alloc_list(emit_values));
        let places = Value::List(self.heap.alloc_list(place_values));
        self.record(&[("value", value), ("emits", emits), ("places", places)])
    }

    fn record(&mut self, fields: &[(&str, Value)]) -> Value {
        let mut map = crate::heap::RecordMap::default();
        for (k, v) in fields {
            map.insert((*k).to_string(), *v);
        }
        Value::Map(self.heap.alloc_map(map))
    }
}
