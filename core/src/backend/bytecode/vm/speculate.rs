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
//! - **Saved lazily.** Like the slots and cells, the run's contained effects
//!   are logged by their writers ([`PetalCxt`](crate::native_fn::PetalCxt)'s
//!   `print`, `rng_next_f64`, `set_noise_seed`, `push_output` and the
//!   counters) the first time the call changes them ([`SpecSaves`]), not
//!   snapshotted on entry. A call that touches none of them saves nothing
//!   and puts back nothing, and one with no emits and no places returns the
//!   heap's shared empty list for both, so `speculate` of a pure function
//!   allocates only its result record (`speculate_value` not even that).
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
//! `speculate_value(f, ...args)` is the same call returning only the value,
//! for a caller that wants neither (`ui_measure`, `autodiff`'s probes).

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use super::*;
use crate::heap::CellId;
use crate::record::Shape;
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
    /// The run's contained effects the call changed, logged by their writers.
    pub(crate) saves: SpecSaves,
}

/// What a speculative call changed of the run's contained effects, each
/// logged by its writer the first time ([`PetalCxt`](crate::native_fn::PetalCxt)
/// holds it while a speculative call runs): the value or length before the
/// call, which `Vm::speculate_exit` puts back. Empty for a call that touched
/// none of them.
#[derive(Debug, Clone, Default)]
pub struct SpecSaves {
    /// The random stream before the call's first draw.
    rng: Option<u64>,
    /// The noise seed before the call first set it.
    noise: Option<u64>,
    /// The print buffer's length before the call's first `print`.
    output: Option<usize>,
    /// Each counter the call advanced or set, with its value before (`None`:
    /// it did not exist). One entry per counter.
    counters: Vec<(SymbolId, Option<u64>)>,
    /// Each channel the call emitted on, with its buffer's length and its
    /// origins' length before the first emit. One entry per channel.
    buffers: Vec<(SymbolId, usize, usize)>,
}

impl SpecSaves {
    /// The random stream is about to advance from `now`.
    #[inline]
    pub(crate) fn note_rng(&mut self, now: u64) {
        self.rng.get_or_insert(now);
    }

    /// The noise seed is about to change from `now`.
    #[inline]
    pub(crate) fn note_noise(&mut self, now: u64) {
        self.noise.get_or_insert(now);
    }

    /// A line is about to be printed onto a buffer `len` long.
    #[inline]
    pub(crate) fn note_output(&mut self, len: usize) {
        self.output.get_or_insert(len);
    }

    /// Counter `sym`, holding `old`, is about to change.
    pub(crate) fn note_counter(&mut self, sym: SymbolId, old: Option<u64>) {
        if !self.counters.iter().any(|(s, _)| *s == sym) {
            self.counters.push((sym, old));
        }
    }

    /// Whether channel `sym` has been emitted on already in this call.
    #[inline]
    pub(crate) fn has_buffer(&self, sym: SymbolId) -> bool {
        self.buffers.iter().any(|(s, _, _)| *s == sym)
    }

    /// Channel `sym` is about to get its first emit of the call, onto a
    /// buffer `len` long whose origins are `origins` long.
    pub(crate) fn note_buffer(&mut self, sym: SymbolId, len: usize, origins: usize) {
        self.buffers.push((sym, len, origins));
    }

    /// How many things were logged: 0 for a call that touched none of them.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rng.is_some() as usize
            + self.noise.is_some() as usize
            + self.output.is_some() as usize
            + self.counters.len()
            + self.buffers.len()
    }
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

/// The run's flags a speculative call sets for itself and puts back. The
/// run's contained effects are not here: they are logged as they change
/// ([`SpecSaves`]).
struct Saved {
    memo: bool,
    echo: bool,
    trace_emit: bool,
    observe: bool,
    callback: Option<crate::stack::BuiltinCallback>,
}

/// What a speculative call emitted: per channel, in channel-name order, the
/// values and their origins.
type Emits = Vec<(SymbolId, Vec<(Value, crate::execution_context::EmitSite)>)>;

/// What [`Vm::speculate_call`] did.
enum Speculated {
    /// The function was pending: no call, and this is the answer.
    Absorbed(Value),
    /// The call ran: its value, what it emitted and its places.
    Ran(Value, Emits, Vec<(Value, Value)>),
}

#[cfg(test)]
thread_local! {
    /// How many entries the last speculative call on this thread logged
    /// (state slots, cells and [`SpecSaves`]), for the tests: a call that
    /// touches nothing must log nothing.
    pub(crate) static LAST_LOGGED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// The shape of a record with `keys`, in that order. The records a
/// speculative call returns share one shape per kind, built once, instead of
/// building it key by key for every record (and the field reads of every
/// result then hit the same inline cache).
fn shape_of(keys: &[&str]) -> Arc<Shape> {
    let mut map = crate::heap::RecordMap::default();
    for k in keys {
        map.insert(*k, Value::Nil);
    }
    Arc::clone(map.shape())
}

static RESULT_SHAPE: LazyLock<Arc<Shape>> = LazyLock::new(|| shape_of(&["value", "emits", "places"]));
static EMIT_SHAPE: LazyLock<Arc<Shape>> = LazyLock::new(|| shape_of(&["channel", "value", "at"]));
static AT_SHAPE: LazyLock<Arc<Shape>> = LazyLock::new(|| shape_of(&["file", "line", "column"]));
static PLACE_SHAPE: LazyLock<Arc<Shape>> = LazyLock::new(|| shape_of(&["key", "value"]));

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
        match self.speculate_call(args, origin, true)? {
            Speculated::Absorbed(v) => Ok(v),
            Speculated::Ran(value, emits, places) => Ok(self.speculate_result(value, emits, places)),
        }
    }

    /// `speculate_value(f, ...args)`: `speculate(f, ...args).value`, without
    /// building the record or collecting the emits (they are dropped from
    /// the buffers all the same).
    pub(super) fn builtin_speculate_value(
        &mut self,
        args: &[Value],
        origin: Option<TermId>,
    ) -> Result<Value, String> {
        match self.speculate_call(args, origin, false)? {
            Speculated::Absorbed(v) | Speculated::Ran(v, _, _) => Ok(v),
        }
    }

    /// Run `args[0](...args[1..])` speculatively. `collect`: take the emits
    /// out of the buffers with their origins, to return them; otherwise they
    /// are only cut off.
    fn speculate_call(
        &mut self,
        args: &[Value],
        origin: Option<TermId>,
        collect: bool,
    ) -> Result<Speculated, String> {
        let Some((&callee, call_args)) = args.split_first() else {
            return Err("speculate() expects a function to call, then its arguments".into());
        };
        // A pending function is absorbed, as a call of one would be.
        if matches!(callee, Value::Pending(_)) {
            self.note_absorption(callee, origin);
            return Ok(Speculated::Absorbed(callee));
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
        let (emits, places) = self.speculate_exit(saved, collect);
        Ok(Speculated::Ran(result?, emits, places))
    }

    /// Set the run's flags for a speculative call and start its record. The
    /// run's contained effects are saved as they change, not here.
    fn speculate_enter(&mut self) -> Saved {
        let saved = Saved {
            memo: self.memo,
            echo: self.echo,
            trace_emit: self.trace_emit,
            observe: self.observations.enabled,
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
        // The last call's record, emptied on its way out: no allocation.
        self.stack.speculation = Some(self.stack.speculation_spare.take().unwrap_or_default());
        saved
    }

    /// Put back everything the call changed, and take what it emitted out of
    /// the buffers: with `collect`, per channel, in channel-name order, the
    /// values and their origins (without, they are only cut off). Runs
    /// whether or not the call failed.
    fn speculate_exit(&mut self, saved: Saved, collect: bool) -> (Emits, Vec<(Value, Value)>) {
        let mut spec = self.stack.speculation.take().unwrap_or_default();
        self.heap.end_speculation();
        #[cfg(test)]
        LAST_LOGGED.with(|c| c.set(Some(spec.state_undo.len() + spec.cell_undo.len() + spec.saves.len())));
        for (key, old) in spec.state_undo.drain(..) {
            match old {
                Some(v) => {
                    self.stack.state.insert(key, v);
                }
                None => {
                    self.stack.state.remove(&key);
                }
            }
        }
        spec.state_seen.clear();
        for (cell, old) in spec.cell_undo.drain(..) {
            self.heap.cell_write(cell, old);
        }
        spec.cell_seen.clear();
        let saves = &mut spec.saves;
        // The emits: everything past each channel's length before the call
        // first emitted on it. A channel it did not emit on is untouched.
        let mut emits = Vec::new();
        for (sym, from, origins_from) in saves.buffers.drain(..) {
            let Some(buf) = self.output_buffers.get_mut(&sym) else {
                continue;
            };
            if buf.len() <= from {
                continue;
            }
            if !collect {
                buf.truncate(from);
                if let Some(o) = self.emit_origins.get_mut(&sym) {
                    o.truncate(origins_from);
                }
                continue;
            }
            let values = buf.split_off(from);
            let mut origins = match self.emit_origins.get_mut(&sym) {
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
            emits.push((sym, values.into_iter().zip(origins).collect::<Vec<_>>()));
        }
        // Channels in name order: the result must not depend on the order
        // the call first emitted on them.
        if emits.len() > 1 {
            let symbols = &*self.symbols;
            emits.sort_by(|a, b| symbols.name(a.0).cmp(&symbols.name(b.0)));
        }
        if let Some(len) = saves.output.take() {
            self.output.truncate(len);
        }
        if let Some(rng) = saves.rng.take() {
            *self.rng_state = rng;
        }
        if let Some(noise) = saves.noise.take() {
            *self.noise_seed = noise;
        }
        for (sym, old) in saves.counters.drain(..) {
            match old {
                Some(v) => {
                    self.counters.insert(sym, v);
                }
                None => {
                    self.counters.remove(&sym);
                }
            }
        }
        let places = std::mem::take(&mut spec.places);
        self.stack.speculation_spare = Some(spec);
        self.memo = saved.memo;
        self.echo = saved.echo;
        self.trace_emit = saved.trace_emit;
        self.observations.enabled = saved.observe;
        self.stack.callback = saved.callback;
        (emits, places)
    }

    /// `{value, emits, places}`: `emits` is `[{channel, value, at}]`, where
    /// `at` is `{file, line, column}` of the call that emitted (the first
    /// frame outside the prelude, as an author would point at it) or nil;
    /// `places` is `[{key, value}]`.
    fn speculate_result(&mut self, value: Value, emits: Emits, places: Vec<(Value, Value)>) -> Value {
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
                        self.record(&AT_SHAPE, vec![file, Value::Int(line as i64), Value::Int(column as i64)])
                    }
                    None => Value::Nil,
                };
                let rec = self.record(&EMIT_SHAPE, vec![channel, v, at]);
                emit_values.push(rec);
            }
        }
        let place_values: Vec<Value> = places
            .into_iter()
            .map(|(k, v)| self.record(&PLACE_SHAPE, vec![k, v]))
            .collect();
        // Nothing emitted or placed: the heap's one shared empty list, which
        // nothing can edit in place, instead of a new one.
        let emits = Value::List(self.list_or_shared_empty(emit_values));
        let places = Value::List(self.list_or_shared_empty(place_values));
        self.record(&RESULT_SHAPE, vec![value, emits, places])
    }

    fn list_or_shared_empty(&mut self, items: Vec<Value>) -> crate::heap::ListId {
        if items.is_empty() {
            self.heap.shared_empty_list()
        } else {
            self.heap.alloc_list(items)
        }
    }

    /// A record of `shape` holding `values`, in the shape's key order.
    fn record(&mut self, shape: &LazyLock<Arc<Shape>>, values: Vec<Value>) -> Value {
        let map = crate::heap::RecordMap::from_shape(Arc::clone(shape), values);
        Value::Map(self.heap.alloc_map(map))
    }
}

#[cfg(test)]
mod tests {
    use super::LAST_LOGGED;
    use crate::env::Env;

    /// What the last speculative call of `code`'s run logged.
    fn logged(code: &str) -> usize {
        LAST_LOGGED.with(|c| c.set(None));
        let mut env = Env::new();
        let pid = env.load_program(code).expect("loads");
        let sid = env.create_stack(pid).expect("stack");
        env.run(sid).expect("runs");
        LAST_LOGGED.with(|c| c.get()).expect("a speculative call ran")
    }

    /// The fast path: a call that touches nothing `speculate` would have to
    /// put back saves nothing, through either entry.
    #[test]
    fn an_untouched_call_logs_nothing() {
        let pure = "fn f(x)\n  (x - 3.0) * (x - 3.0) + sin(x)\nend\n";
        assert_eq!(logged(&format!("{pure}speculate(f, 2.0).value\n")), 0);
        assert_eq!(logged(&format!("{pure}speculate_value(f, 2.0)\n")), 0);
        // Reading state, and building and editing its own values, is not a
        // write the live program could see.
        let reads = "state n = 4\nfn g()\n  var xs = []\n  for i in range(0, n) do\n    set xs = append(xs, i)\n  end\n  len(xs)\nend\nspeculate_value(g)\n";
        assert_eq!(logged(reads), 0);
    }

    /// Each kind of thing a call changes is logged once, however often it
    /// changes it.
    #[test]
    fn each_touched_thing_is_logged_once() {
        // A print, twice: the print buffer.
        assert_eq!(logged("fn f()\n  print(1)\n  print(2)\nend\nspeculate_value(f)\n"), 1);
        // Two draws: the random stream.
        assert_eq!(logged("fn f()\n  random() + random()\nend\nspeculate_value(f)\n"), 1);
        // A slot written twice, and two emits on one channel.
        let code = "import io\nfn f()\n  state n = 0\n  n += 1\n  n += 1\n  push_output(symbol(\"a\"), 1)\n  push_output(symbol(\"a\"), 2)\nend\nspeculate(f)\n";
        assert_eq!(logged(code), 2);
    }

    fn native_tick(cx: &mut crate::native_fn::PetalCxt) -> crate::native_fn::NativeResult {
        let sym = cx.intern_symbol("ticks");
        let v = cx.next_counter(sym);
        cx.push_int(v as i64);
        Ok(1)
    }

    fn run_display(env: &mut Env, code: &str) -> String {
        let pid = env.load_program(code).expect("loads");
        let sid = env.create_stack(pid).expect("stack");
        let v = env.run(sid).expect("runs");
        crate::value::value_to_display_string(&v, env.heap())
    }

    /// A counter is a contained effect, logged on first write: the call sees
    /// its own advances, the live program none of them, and a counter the
    /// call created is gone after it.
    #[test]
    fn counters_are_put_back() {
        let mut env = Env::new();
        env.register_native("tick", native_tick, crate::native_fn::NativeEffects::EFFECT.contained());
        let code = "native fn tick()
fn twice()
  [tick(), tick()]
end
let fresh = speculate_value(twice)
let a = tick()
let inside = speculate_value(twice)
let b = tick()
[fresh, a, inside, b]";
        assert_eq!(run_display(&mut env, code), "[[0, 1], 0, [1, 2], 1]");
    }

    /// `speculate_value` is `speculate(…).value`: the same value, and the
    /// emits and prints are dropped from the live buffers all the same.
    #[test]
    fn speculate_value_is_the_value_and_drops_the_emits() {
        let mut env = Env::new();
        let code = "import io
fn draw(n)
  for i in range(0, n) do push_output(symbol(\"cmds\"), i) end
  print(\"inside\")
  n * 2
end
push_output(symbol(\"cmds\"), \"before\")
let v = speculate_value(draw, 3)
push_output(symbol(\"cmds\"), \"after\")
[v, speculate(draw, 3).value]";
        assert_eq!(run_display(&mut env, code), "[6, 6]");
        assert!(env.take_output().is_empty());
        let cmds = env.intern_symbol("cmds");
        let live: Vec<String> = env
            .take_output_buffer(cmds)
            .iter()
            .map(|v| crate::value::value_to_display_string(v, env.heap()))
            .collect();
        assert_eq!(live, ["before", "after"]);
    }

    /// The empty `emits` and `places` are one shared list, and editing what
    /// one result handed out does not reach the next.
    #[test]
    fn the_shared_empty_list_is_not_edited_in_place() {
        let mut env = Env::new();
        let code = "fn f() 1 end
var e = speculate(f).emits
set e = append(e, 1)
set e = append(e, 2)
let p = speculate(f).places
[e, p, speculate(f).emits, len(p)]";
        assert_eq!(run_display(&mut env, code), "[[1, 2], [], [], 0]");
    }
}
