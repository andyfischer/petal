//! The entry table behind a `map` value (`Map()`, `Value::Dict`).
//!
//! A record (`Value::Map`, `{a: 1}`) is keyed by field name. A `map` is keyed
//! by **any hashable value**, compared by content: `m[[1, 2]]` finds the entry
//! stored under another `[1, 2]`, and `m[2]` the one stored under `2.0`,
//! because the two keys are `==`. Equal keys must hash equally, so the hash is
//! [`value::hash_value`], the content hash `state(key)` already relies on
//! (see docs/dev/formal-verification.md, `num_key_matches_eq`).
//!
//! The hash reads through the heap, so the table cannot hash its keys itself.
//! It stores the entries in insertion order and an index from a key's content
//! hash to the positions holding a key with that hash; a lookup hashes the
//! probe, then compares it with [`value::values_equal`] against each candidate.
//!
//! Like every heap container a map is a value: the heap's `dict_*` operations
//! return a changed copy, and the in-place forms edit only a map the caller
//! has proven is not shared (see `Heap::dict_set_in_place`).

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::fxhash::FxBuildHasher;
use crate::heap::Heap;
use crate::value::{self, Value};

/// Insertion-ordered `key → value` entries plus a content-hash index.
#[derive(Clone, Default)]
pub struct DictEntries {
    entries: Vec<(Value, Value)>,
    /// Content hash of a key → the positions in `entries` whose key has it.
    index: HashMap<u64, SmallVec<[u32; 1]>, FxBuildHasher>,
}

impl DictEntries {
    pub fn with_capacity(n: usize) -> Self {
        DictEntries {
            entries: Vec::with_capacity(n),
            index: HashMap::with_capacity_and_hasher(n, FxBuildHasher::default()),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (Value, Value)> + '_ {
        self.entries.iter().copied()
    }

    pub fn keys(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().map(|(k, _)| *k)
    }

    pub fn values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().map(|(_, v)| *v)
    }

    /// The entry at `pos` (insertion order).
    pub fn entry_at(&self, pos: usize) -> (Value, Value) {
        self.entries[pos]
    }

    /// Where the key `key`, whose content hash is `hash`, is stored.
    pub fn find(&self, hash: u64, key: &Value, heap: &Heap) -> Option<usize> {
        self.index.get(&hash)?.iter().map(|&p| p as usize).find(|&p| {
            let k = &self.entries[p].0;
            k == key || value::values_equal(k, key, heap)
        })
    }

    /// Store `val` under `key`: at `pos` when [`find`](Self::find) located the
    /// key there (the key keeps its place in the order), else as a new last
    /// entry.
    pub fn put(&mut self, hash: u64, pos: Option<usize>, key: Value, val: Value) {
        match pos {
            Some(p) => self.entries[p].1 = val,
            None => {
                let p = self.entries.len() as u32;
                self.entries.push((key, val));
                self.index.entry(hash).or_default().push(p);
            }
        }
    }

    /// Remove the entry at `pos`, whose key hashes to `hash`, keeping the
    /// order of the rest. O(len), like a record's `shift_remove`.
    pub fn remove_at(&mut self, hash: u64, pos: usize) {
        self.entries.remove(pos);
        if let Some(bucket) = self.index.get_mut(&hash) {
            bucket.retain(|p| *p as usize != pos);
            if bucket.is_empty() {
                self.index.remove(&hash);
            }
        }
        for bucket in self.index.values_mut() {
            for p in bucket.iter_mut() {
                if *p as usize > pos {
                    *p -= 1;
                }
            }
        }
    }

    /// Rough bytes of backing store, for the collector's budget and the
    /// duplication stats.
    pub fn payload_bytes(&self) -> u64 {
        let entry = 2 * std::mem::size_of::<Value>();
        let slot = std::mem::size_of::<u64>() + std::mem::size_of::<SmallVec<[u32; 1]>>();
        (self.entries.len() * (entry + slot)) as u64
    }

    /// Every key and value, keys first per entry: what the map references.
    pub fn all_values(&self) -> impl Iterator<Item = Value> + '_ {
        self.entries.iter().flat_map(|(k, v)| [*k, *v])
    }
}

/// The content hash of `key` as a map key, or why it cannot be one.
///
/// A key is compared by `==`, so it must be something `==` compares by
/// content and that stays equal to itself: a function compares by identity
/// and cannot be rebuilt as an equal key, a pending value has no content yet,
/// and NaN is not `==` to itself, so an entry stored under it could never be
/// found again. Containers are checked to any depth.
pub fn key_hash(key: Value, heap: &Heap) -> Result<u64, String> {
    if let Some(what) = unhashable_part(key, heap) {
        return Err(format!("Cannot use {what} as a map key"));
    }
    Ok(value::hash_value(&key, heap))
}

/// The first part of `v` that cannot be hashed by content, named for an
/// error message.
fn unhashable_part(v: Value, heap: &Heap) -> Option<String> {
    match v {
        Value::Closure(_) | Value::OverloadSet(_) | Value::NativeFunction(_) => {
            Some("a function".into())
        }
        Value::Pending(_) => Some("a pending value".into()),
        Value::Cell(_) => Some("a `var` cell".into()),
        Value::Float(f) if f.is_nan() => Some("NaN".into()),
        Value::Dual { value, .. } if value.is_nan() => Some("NaN".into()),
        Value::Vec2(x, y) if x.is_nan() || y.is_nan() => Some("a vector holding NaN".into()),
        Value::Vec3(id) if heap.get_vec3(id).iter().any(|c| c.is_nan()) => {
            Some("a vector holding NaN".into())
        }
        Value::F64Array(id) if heap.get_f64_array(id).iter().any(|c| c.is_nan()) => {
            Some("an f64_array holding NaN".into())
        }
        Value::List(id) => heap.get_list(id).iter().find_map(|e| unhashable_part(*e, heap)),
        Value::Map(id) => heap.get_map(id).values().find_map(|e| unhashable_part(*e, heap)),
        Value::Dict(id) => heap.get_dict(id).all_values().find_map(|e| unhashable_part(e, heap)),
        Value::EnumVariant { data, .. } => unhashable_part(Value::List(data), heap),
        Value::Element(id) => unhashable_part(Value::Map(heap.get_element_props(id)), heap)
            .or_else(|| unhashable_part(Value::List(heap.get_element_children(id)), heap)),
        _ => None,
    }
}
