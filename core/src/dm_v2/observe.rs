//! What a speculative run observes, in a form that outlives the run.
//!
//! A run happens in a forked execution whose heap is dropped as soon as the
//! run ends, so nothing a [`SpecRun`](super::spec::SpecRun) holds may be a heap
//! id. Values are converted to [`ObsValue`] *when they are recorded* (an emit,
//! a watched term, a native's raw arguments): numbers stay exact, strings are
//! hashed, containers are copied to a bounded depth. Two runs are compared on
//! these, bit for bit.
//!
//! [`Observable`] names one number in a run (a raw argument of the native
//! that produced an emit, a field of an emitted value, or a watched term's
//! value before an emit), and [`SpecRun::read`](super::spec::SpecRun::read)
//! reads it. See docs/dev/direct-manipulation-v2-plan.md §2.1.6 and §6.1.

use std::hash::{Hash, Hasher};

use crate::heap::Heap;
use crate::program::TermId;
use crate::value::Value;

/// A value as a speculative run recorded it, independent of any heap.
///
/// Floats compare by bit pattern ([`PartialEq`] here is bit equality), so a
/// determinism check is exact and `NaN` equals itself.
#[derive(Debug, Clone)]
pub enum ObsValue {
    /// A float, or the primal of a dual number.
    Num(f64),
    Int(i64),
    Bool(bool),
    /// A string, by the hash of its contents.
    Str(u64),
    Nil,
    /// A list, a vec2/vec3 (as numbers) or an `f64_array`.
    List(Vec<ObsValue>),
    /// A record, fields in insertion order.
    Record(Vec<(String, ObsValue)>),
    /// An enum variant (a draw command is one): the tag's hash and the
    /// payload list.
    Variant { tag: u64, data: Vec<ObsValue> },
    /// Something with no stable value outside its heap (a closure, a handle,
    /// a pending resource), or a container cut off by the depth bound.
    Opaque,
}

impl PartialEq for ObsValue {
    fn eq(&self, other: &Self) -> bool {
        use ObsValue::*;
        match (self, other) {
            (Num(a), Num(b)) => a.to_bits() == b.to_bits(),
            (Int(a), Int(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (Nil, Nil) | (Opaque, Opaque) => true,
            (List(a), List(b)) => a == b,
            (Record(a), Record(b)) => a == b,
            (Variant { tag: t1, data: d1 }, Variant { tag: t2, data: d2 }) => t1 == t2 && d1 == d2,
            _ => false,
        }
    }
}

/// How deep and how wide a conversion goes.
#[derive(Debug, Clone, Copy)]
pub struct ObsBounds {
    pub depth: u32,
    pub max_len: usize,
}

impl ObsBounds {
    /// For a native's raw arguments (R8): depth 3, lists of at most 64.
    pub const ARGS: ObsBounds = ObsBounds { depth: 3, max_len: 64 };
    /// For an emitted value or a watched term: a little deeper and wider,
    /// since an emitted record is what an observable addresses.
    pub const VALUE: ObsBounds = ObsBounds { depth: 5, max_len: 256 };
}

/// The hash a string is recorded by: stable within and across processes.
pub fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl ObsValue {
    /// Convert `v`, read against `heap`, within `bounds`.
    pub fn from_value(v: Value, heap: &Heap, bounds: ObsBounds) -> ObsValue {
        Self::convert(v, heap, bounds.depth, bounds.max_len)
    }

    fn convert(v: Value, heap: &Heap, depth: u32, max_len: usize) -> ObsValue {
        let list = |items: &[Value]| -> ObsValue {
            if depth == 0 {
                return ObsValue::Opaque;
            }
            ObsValue::List(
                items
                    .iter()
                    .take(max_len)
                    .map(|&x| Self::convert(x, heap, depth - 1, max_len))
                    .collect(),
            )
        };
        match v {
            Value::Nil => ObsValue::Nil,
            Value::Bool(b) => ObsValue::Bool(b),
            Value::Int(i) => ObsValue::Int(i),
            Value::Float(f) => ObsValue::Num(f),
            Value::Dual { value, .. } => ObsValue::Num(value),
            Value::Vec2(x, y) => ObsValue::List(vec![ObsValue::Num(x), ObsValue::Num(y)]),
            Value::Vec3(id) => {
                ObsValue::List(heap.get_vec3(id).iter().map(|&c| ObsValue::Num(c)).collect())
            }
            Value::String(id) => ObsValue::Str(hash_str(heap.get_string(id))),
            Value::List(id) => list(heap.get_list(id)),
            Value::F64Array(id) => ObsValue::List(
                heap.get_f64_array(id).iter().take(max_len).map(|&f| ObsValue::Num(f)).collect(),
            ),
            Value::Map(id) => {
                if depth == 0 {
                    return ObsValue::Opaque;
                }
                ObsValue::Record(
                    heap.get_map(id)
                        .iter()
                        .take(max_len)
                        .map(|(k, &x)| (k.as_str().to_string(), Self::convert(x, heap, depth - 1, max_len)))
                        .collect(),
                )
            }
            Value::Dict(id) => {
                if depth == 0 {
                    return ObsValue::Opaque;
                }
                ObsValue::List(
                    heap.get_dict(id)
                        .iter()
                        .take(max_len)
                        .map(|(k, x)| {
                            ObsValue::List(vec![
                                Self::convert(k, heap, depth - 1, max_len),
                                Self::convert(x, heap, depth - 1, max_len),
                            ])
                        })
                        .collect(),
                )
            }
            Value::EnumVariant { tag, data } => {
                let tag = hash_str(heap.get_string(tag));
                if depth == 0 {
                    return ObsValue::Variant { tag, data: Vec::new() };
                }
                let data = heap
                    .get_list(data)
                    .iter()
                    .take(max_len)
                    .map(|&x| Self::convert(x, heap, depth - 1, max_len))
                    .collect();
                ObsValue::Variant { tag, data }
            }
            Value::Symbol(s) => ObsValue::Int(s.0 as i64),
            Value::Closure(_)
            | Value::OverloadSet(_)
            | Value::NativeFunction(_)
            | Value::Element(_)
            | Value::Cell(_)
            | Value::Handle(_)
            | Value::Pending(_) => ObsValue::Opaque,
        }
    }

    /// The number this value is, if it is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ObsValue::Num(f) => Some(*f),
            ObsValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Follow `path` into this value.
    pub fn at(&self, path: &[PathStep]) -> Option<&ObsValue> {
        let mut cur = self;
        for step in path {
            cur = match (cur, step) {
                (ObsValue::List(items), PathStep::Index(i)) => items.get(*i)?,
                (ObsValue::Variant { data, .. }, PathStep::Index(i)) => data.get(*i)?,
                (ObsValue::Record(fields), PathStep::Field(name)) => {
                    &fields.iter().find(|(k, _)| k == name)?.1
                }
                // A vec2 written as `{x, y}` in a path reads as the list.
                (ObsValue::List(items), PathStep::Field(name)) if items.len() <= 3 => {
                    let i = match name.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        _ => return None,
                    };
                    items.get(i)?
                }
                _ => return None,
            };
        }
        Some(cur)
    }
}

/// Which emit of a run: stable across runs whose control flow is the same,
/// so one shape out of a loop can be matched between runs (§2.1.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EmitKey {
    /// The hash of the emit's call chain (the native's call site, then the
    /// call site of every enclosing frame).
    pub chain_hash: u64,
    /// The hash of the loop-iteration parts of the emitting frame's state
    /// path, so shape 3 of loop B does not depend on loop A's trip count.
    pub path_hash: u64,
    /// How many emits with the same chain and path came before this one in
    /// the run.
    pub occurrence: u32,
}

/// One step into a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStep {
    Index(usize),
    Field(String),
}

/// One number in a run, by where it is (§6.1).
#[derive(Debug, Clone, PartialEq)]
pub enum Observable {
    /// Raw argument `arg` (0-based) of the native that produced emit `key`,
    /// then `path` into it. Raw, so the int truncation of a draw native
    /// (`get_int`) does not hide a small change (§1.7).
    EmitArg { key: EmitKey, arg: usize, path: Vec<PathStep> },
    /// A part of the emitted value itself.
    EmitValue { key: EmitKey, path: Vec<PathStep> },
    /// The last value of watched term `term` recorded before emit `at`.
    Watch { term: TermId, at: EmitKey, path: Vec<PathStep> },
}

/// What the live frame showed for some observables, for the baseline
/// comparison (§2.1.5): the speculative baseline must agree with it.
#[derive(Debug, Clone, Default)]
pub struct LiveObservation {
    pub goals: Vec<(Observable, f64)>,
}
