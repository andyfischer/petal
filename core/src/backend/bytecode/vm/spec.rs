//! The VM side of a speculative run (docs/dev/direct-manipulation-v2-plan.md
//! §2.1, R1): literal overrides, the native gate, random and host-data replay,
//! emit and watched-term capture, the branch fingerprint and observed call
//! edges.
//!
//! [`SpecHooks`] is built by [`crate::dm_v2::spec::SpecSession`] for one run
//! and handed to the VM that runs the session's fork; every other run has
//! none, and pays one `Option` test at the few sites below (a `LoadConst` in
//! the general executor, a frame push, a native call). With hooks present the
//! VM takes its general path for every instruction (`Vm::hooks`), so the
//! straight-line loop in `fast` never runs and needs no hook of its own.
//!
//! Nothing here holds a heap id past the run: emits, watched values and raw
//! arguments are converted to [`ObsValue`] when they are recorded, and the
//! replay tables hold raw random draws and [`Portable`] host answers.

use std::collections::{HashMap, HashSet};
use std::hash::Hasher;

use smallvec::SmallVec;

use crate::constant_table::ConstantId;
use crate::dm_v2::observe::{EmitKey, ObsBounds, ObsValue};
use crate::execution_context::EmitSite;
use crate::fxhash::FxHasher;
use crate::heap::Heap;
use crate::native_fn::NativeFnId;
use crate::program::{FunctionId, TermId};
use crate::stack::PathPart;
use crate::symbol::SymbolId;
use crate::value::Value;

/// How random draws behave in a speculative run (§2.1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RngMode {
    /// Each draw belongs to *where* it is made (call chain, loop path,
    /// occurrence): the baseline records the raw uniform draw, later runs get
    /// the same uniform at the same place and map it with the native's
    /// current arguments. A perturbation that adds a draw does not shift the
    /// draws after it.
    #[default]
    ReplayByCallSite,
    /// Draw from the forked stream, as a plain fork does.
    ForkStream,
}

/// Whether a run records the replay tables (the baselines) or answers from
/// them (every run after).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPhase {
    Record,
    Replay,
}

/// The key a random draw is recorded under: chain hash, loop-path hash,
/// occurrence of an RNG call under that chain and path, and the draw's index
/// within the call.
pub(crate) type RngKey = (u64, u64, u32, u32);

/// One buffered emit of a speculative run.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitRecord {
    pub channel: SymbolId,
    pub key: EmitKey,
    pub chain: EmitSite,
    /// The spec-local sequence number of the emit (shared with
    /// [`WatchRecord::seq`]), not the Env trace's.
    pub seq: u64,
    /// The native that pushed it.
    pub native: Option<NativeFnId>,
    /// That native's arguments as it was handed them, before any conversion
    /// (a draw native truncates its coordinates to ints; these do not).
    pub native_args: SmallVec<[ObsValue; 8]>,
    pub value: ObsValue,
}

/// One recorded value of a watched term.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchRecord {
    pub term: TermId,
    pub seq: u64,
    pub value: ObsValue,
}

/// A native call a speculative run did not make, because the native's effect
/// is not contained (`NativeEffects::is_escaping`). It answered `nil`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppressedCall {
    pub native: String,
    pub origin: Option<TermId>,
}

/// A host-data answer recorded by the baseline, in a form that can be
/// allocated into another run's heap.
#[derive(Debug, Clone, PartialEq)]
pub enum Portable {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Symbol(SymbolId),
    Vec2(f64, f64),
    List(Vec<Portable>),
    Record(Vec<(String, Portable)>),
}

impl Portable {
    /// `None` for a value that cannot be carried to another heap (a closure,
    /// a handle, a pending resource).
    pub fn from_value(v: Value, heap: &Heap) -> Option<Portable> {
        Some(match v {
            Value::Nil => Portable::Nil,
            Value::Bool(b) => Portable::Bool(b),
            Value::Int(i) => Portable::Int(i),
            Value::Float(f) => Portable::Float(f),
            Value::Vec2(x, y) => Portable::Vec2(x, y),
            Value::Symbol(s) => Portable::Symbol(s),
            Value::String(id) => Portable::Str(heap.get_string(id).to_string()),
            Value::List(id) => Portable::List(
                heap.get_list(id)
                    .iter()
                    .map(|&x| Portable::from_value(x, heap))
                    .collect::<Option<_>>()?,
            ),
            Value::Map(id) => Portable::Record(
                heap.get_map(id)
                    .iter()
                    .map(|(k, &x)| Some((k.as_str().to_string(), Portable::from_value(x, heap)?)))
                    .collect::<Option<_>>()?,
            ),
            _ => return None,
        })
    }

    /// Allocate this value into `heap`.
    pub fn materialize(&self, heap: &mut Heap) -> Value {
        match self {
            Portable::Nil => Value::Nil,
            Portable::Bool(b) => Value::Bool(*b),
            Portable::Int(i) => Value::Int(*i),
            Portable::Float(f) => Value::Float(*f),
            Portable::Vec2(x, y) => Value::Vec2(*x, *y),
            Portable::Symbol(s) => Value::Symbol(*s),
            Portable::Str(s) => Value::String(heap.alloc_string(s.clone())),
            Portable::List(items) => {
                let values = items.iter().map(|p| p.materialize(heap)).collect();
                Value::List(heap.alloc_list(values))
            }
            Portable::Record(fields) => {
                let mut map = crate::heap::RecordMap::default();
                for (k, p) in fields {
                    let v = p.materialize(heap);
                    map.insert(k.clone(), v);
                }
                Value::Map(heap.alloc_map(map))
            }
        }
    }
}

/// The native call in progress, as the hooks describe it to the natives'
/// own context (`PetalCxt`): where it is, for keying its draws and its emits,
/// and its raw arguments, for attaching to its emits.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallInfo {
    pub chain: SmallVec<[TermId; 4]>,
    pub chain_hash: u64,
    pub path_hash: u64,
    pub nid: Option<NativeFnId>,
    pub args: SmallVec<[ObsValue; 8]>,
    /// The RNG occurrence of this call under its chain and path, taken the
    /// first time it draws.
    pub rng_occ: Option<u32>,
    /// Draws made so far by this call.
    pub draws: u32,
}

/// The part of the hooks a native reaches through its `PetalCxt`: random
/// draws and emits.
#[derive(Debug, Default)]
pub struct SpecTap {
    pub(crate) rng_mode: RngMode,
    pub(crate) phase: Option<ReplayPhase>,
    pub(crate) session_seed: u64,
    /// The raw uniform draws by place. Owned by the session between runs.
    pub(crate) rng_table: HashMap<RngKey, u64>,
    pub(crate) rng_occ: HashMap<(u64, u64), u32>,
    pub(crate) new_random_keys: u32,
    pub(crate) call: Option<CallInfo>,
    pub(crate) emits: Vec<EmitRecord>,
    pub(crate) emit_occ: HashMap<(u64, u64), u32>,
    /// `None` captures every channel.
    pub(crate) channels: Option<Vec<SymbolId>>,
    pub(crate) seq: u64,
}

impl SpecTap {
    /// The next raw draw for the native in progress.
    pub(crate) fn next_u64(&mut self, state: &mut u64) -> u64 {
        if self.rng_mode == RngMode::ForkStream {
            return crate::builtins::rng_next_u64(state);
        }
        let Some(call) = self.call.as_mut() else {
            return crate::builtins::rng_next_u64(state);
        };
        let occ = match call.rng_occ {
            Some(o) => o,
            None => {
                let slot = self.rng_occ.entry((call.chain_hash, call.path_hash)).or_insert(0);
                let o = *slot;
                *slot += 1;
                call.rng_occ = Some(o);
                o
            }
        };
        let key = (call.chain_hash, call.path_hash, occ, call.draws);
        call.draws += 1;
        match self.phase {
            Some(ReplayPhase::Replay) => match self.rng_table.get(&key) {
                Some(&u) => u,
                None => {
                    // A place the baseline never drew at: a draw of its own,
                    // derived from the place and the session's seed, so it is
                    // the same in every run that reaches it.
                    self.new_random_keys += 1;
                    let mut h = FxHasher::default();
                    h.write_u64(key.0);
                    h.write_u64(key.1);
                    h.write_u32(key.2);
                    h.write_u32(key.3);
                    h.write_u64(self.session_seed);
                    let mut s = h.finish() | 1;
                    crate::builtins::rng_next_u64(&mut s)
                }
            },
            _ => {
                let u = crate::builtins::rng_next_u64(state);
                self.rng_table.insert(key, u);
                u
            }
        }
    }

    /// Record an emit `value` into `channel`, made by the native in progress.
    pub(crate) fn note_emit(&mut self, channel: SymbolId, value: Value, heap: &Heap) {
        if let Some(only) = &self.channels
            && !only.contains(&channel)
        {
            return;
        }
        let (chain, chain_hash, path_hash, nid, args) = match &self.call {
            Some(c) => (c.chain.clone(), c.chain_hash, c.path_hash, c.nid, c.args.clone()),
            None => Default::default(),
        };
        let slot = self.emit_occ.entry((chain_hash, path_hash)).or_insert(0);
        let occurrence = *slot;
        *slot += 1;
        let seq = self.seq;
        self.seq += 1;
        self.emits.push(EmitRecord {
            channel,
            key: EmitKey { chain_hash, path_hash, occurrence },
            chain: EmitSite { chain },
            seq,
            native: nid,
            native_args: args,
            value: ObsValue::from_value(value, heap, ObsBounds::VALUE),
        });
    }
}

/// Everything a speculative run's VM consults. See the module docs.
#[derive(Debug, Default)]
pub struct SpecHooks {
    /// The override for each constant, by `ConstantId`; empty past the last
    /// overridden one.
    pub(crate) overrides: Vec<Option<Value>>,
    /// The session's parameter index of each slotted constant, by
    /// `ConstantId` (`u32::MAX` for none), for the loaded set.
    pub(crate) param_of: Vec<u32>,
    /// Which parameters a `LoadConst` read this run.
    pub(crate) loaded: Vec<bool>,
    pub(crate) tap: SpecTap,
    /// Capture each emitting native's raw arguments.
    pub(crate) capture_args: bool,
    /// Host-data answers by (native, hash of arguments); `None` when the
    /// answer could not be carried over, and is asked again.
    pub(crate) host_phase: Option<ReplayPhase>,
    pub(crate) host_table: HashMap<(u32, u64), Option<Portable>>,
    pub(crate) new_host_keys: u32,
    /// The baseline's answers of `rigid(x)` by place (chain hash, loop-path
    /// hash, occurrence), replayed by every later run so that no feedback
    /// passes through the call (§2.1.3, §2.6). `None`: the answer could not
    /// be carried over, and the call goes through.
    pub(crate) rigid_table: HashMap<(u64, u64, u32), Option<Portable>>,
    pub(crate) rigid_occ: HashMap<(u64, u64), u32>,
    /// Handle classes whose methods a speculative run may call.
    pub(crate) safe_handles: HashSet<u16>,
    /// Set when the run met a handle method it may not call.
    pub(crate) unsafe_hit: Option<String>,
    pub(crate) suppressed: Vec<SuppressedCall>,
    pub(crate) watch: HashSet<TermId>,
    pub(crate) watched: Vec<WatchRecord>,
    pub(crate) fingerprint: u64,
    pub(crate) record_call_edges: bool,
    pub(crate) call_edges: Vec<(TermId, FunctionId)>,
    pub(crate) call_edge_set: HashSet<(TermId, FunctionId)>,
}

impl SpecHooks {
    /// What a `LoadConst` of `k` loads instead of the table's value, if
    /// anything; notes the parameter as loaded either way.
    #[inline]
    pub(crate) fn load_const(&mut self, k: ConstantId) -> Option<Value> {
        let i = k.0 as usize;
        if let Some(&p) = self.param_of.get(i)
            && p != u32::MAX
            && let Some(slot) = self.loaded.get_mut(p as usize)
        {
            *slot = true;
        }
        self.overrides.get(i).copied().flatten()
    }

    fn fold(&mut self, words: &[u64]) {
        let mut h = FxHasher::default();
        h.write_u64(self.fingerprint);
        for &w in words {
            h.write_u64(w);
        }
        self.fingerprint = h.finish();
    }

    /// A conditional instruction at `ip` of `func` went on to `next` (R7):
    /// branches, loop steps (and so trip counts), match arms, and a `state`
    /// declaration's hit or miss (§2.1.2).
    pub(crate) fn note_branch(&mut self, func: Option<FunctionId>, ip: usize, next: usize) {
        let f = func.map_or(u64::MAX, |f| f.0 as u64);
        self.fold(&[f, ip as u64, next as u64]);
    }

    /// A user function was called from `site` (R13).
    pub(crate) fn note_call(&mut self, site: TermId, func: FunctionId) {
        if self.record_call_edges && self.call_edge_set.insert((site, func)) {
            self.call_edges.push((site, func));
        }
    }

    /// Record `value` for a watched `term` (R5).
    pub(crate) fn note_watch(&mut self, term: TermId, value: Value, heap: &Heap) {
        if self.watch.contains(&term) {
            let seq = self.tap.seq;
            self.tap.seq += 1;
            self.watched.push(WatchRecord {
                term,
                seq,
                value: ObsValue::from_value(value, heap, ObsBounds::VALUE),
            });
        }
    }
}

/// The hash of a call chain.
pub(crate) fn chain_hash(chain: &[TermId]) -> u64 {
    let mut h = FxHasher::default();
    h.write_u64(0x6368_6169_6e00_0000);
    for t in chain {
        h.write_u32(t.0);
    }
    h.finish()
}

/// The hash of the loop-iteration parts of a state path (`Index` and `Key`).
pub(crate) fn loop_path_hash(path: &[PathPart]) -> u64 {
    let mut h = FxHasher::default();
    h.write_u64(0x6c6f_6f70_0000_0000);
    for part in path {
        match part {
            PathPart::Index(i) => {
                h.write_u8(1);
                h.write_u64(*i as u64);
            }
            PathPart::Key(k) => {
                h.write_u8(2);
                h.write_u64(*k);
            }
            PathPart::Call(c) => {
                // A loop inside a callee and the same loop inside another
                // call are different places; the call part separates them.
                h.write_u8(3);
                h.write_u64(*c);
            }
            _ => {}
        }
    }
    h.finish()
}

/// The speculative run in progress on an `Env`: the hooks apply to this
/// stack's runs only.
#[derive(Debug)]
pub(crate) struct ActiveSpec {
    pub stack: crate::stack::StackKey,
    pub hooks: Box<SpecHooks>,
}
