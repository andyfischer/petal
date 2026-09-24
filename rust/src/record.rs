//! Record storage: a shared, ordered key list (a [`Shape`]) plus one value
//! per key.
//!
//! A record used to be an `IndexMap<String, Value>`: a hash table, an entry
//! vector and one heap `String` per key, so every record literal cost several
//! allocations and every field read hashed a string. A [`RecordMap`] is
//! instead an `Arc<Shape>` and a `Vec<Value>`:
//!
//! - A record literal's shape is built once, when the bytecode is lowered, and
//!   every record the literal makes shares it (`Inst::AllocMap`), so building
//!   `{pos: p, rot: r}` allocates just the value vector.
//! - Copy-on-write updates (`r.x = 1`, `{...r, x: 1}` for a field `r` already
//!   has) clone the `Arc`, not the keys.
//! - A shape has a process-unique [`Shape::id`], which is what a `GetField`
//!   inline cache remembers (`shape id → slot`): a hit is a compare and an
//!   indexed load. A shape's key list never changes under an id that anyone
//!   else can see: a shared shape is copied (with a fresh id) before it is
//!   extended, and removing a key always takes a fresh id.
//! - Small shapes are searched linearly (a pointer compare first, then the
//!   bytes); a shape with more than [`INDEX_MIN`] keys also keeps a hash index,
//!   so big dictionary-style records stay O(1).
//!
//! The API mirrors the subset of `IndexMap` the runtime uses, so callers read
//! the same: insertion order is iteration order, `insert` on an existing key
//! keeps its position, `shift_remove` preserves the order of the rest.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use crate::fxhash::FxBuildHasher;
use crate::value::Value;

/// A record key: a shared, immutable string. Cloning one is a reference
/// count bump, so records that share a shape share their key text too.
#[derive(Clone)]
pub struct Key(Arc<str>);

impl Key {
    pub fn new(s: &str) -> Self {
        Key(Arc::from(s))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether two keys are the same allocation (a cheap sufficient test for
    /// equality).
    #[inline]
    pub fn ptr_eq(a: &Key, b: &Key) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl Deref for Key {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Key {
    #[inline]
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Key {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Hash for Key {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Same as `str`'s hash, so an index keyed by `Key` answers `&str`.
        self.as_str().hash(state)
    }
}

impl PartialEq for Key {
    #[inline]
    fn eq(&self, other: &Key) -> bool {
        Key::ptr_eq(self, other) || self.as_str() == other.as_str()
    }
}
impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Key) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialEq<str> for Key {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for Key {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<String> for Key {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}
impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Self {
        Key::new(s)
    }
}
impl From<String> for Key {
    fn from(s: String) -> Self {
        Key(Arc::from(s))
    }
}
impl From<&String> for Key {
    fn from(s: &String) -> Self {
        Key::new(s)
    }
}
impl From<Key> for String {
    fn from(k: Key) -> String {
        k.as_str().to_string()
    }
}

impl serde::Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Anything a record can be keyed by. Lets [`RecordMap::insert`] look the key
/// up as a `&str` first and only build a [`Key`] when the field is new.
pub trait IntoKey {
    fn key_str(&self) -> &str;
    fn into_key(self) -> Key;
}
impl IntoKey for Key {
    fn key_str(&self) -> &str {
        self.as_str()
    }
    fn into_key(self) -> Key {
        self
    }
}
impl IntoKey for &Key {
    fn key_str(&self) -> &str {
        self.as_str()
    }
    fn into_key(self) -> Key {
        self.clone()
    }
}
impl IntoKey for String {
    fn key_str(&self) -> &str {
        self.as_str()
    }
    fn into_key(self) -> Key {
        Key::from(self)
    }
}
impl IntoKey for &String {
    fn key_str(&self) -> &str {
        self.as_str()
    }
    fn into_key(self) -> Key {
        Key::new(self)
    }
}
impl IntoKey for &str {
    fn key_str(&self) -> &str {
        self
    }
    fn into_key(self) -> Key {
        Key::new(self)
    }
}

/// Keys at or below this count are found by a linear scan; a bigger shape
/// keeps a hash index as well.
pub const INDEX_MIN: usize = 12;

/// Ids handed to shapes. 0 is never used, so a zeroed inline cache is empty.
static NEXT_SHAPE_ID: AtomicU64 = AtomicU64::new(1);

fn fresh_shape_id() -> u64 {
    NEXT_SHAPE_ID.fetch_add(1, Ordering::Relaxed)
}

/// An ordered list of record keys, shared by every record with those keys in
/// that order.
pub struct Shape {
    id: u64,
    keys: Vec<Key>,
    /// Sum of the keys' byte lengths (the heap's payload accounting).
    key_bytes: u64,
    /// `key → position`, kept only when there are more than [`INDEX_MIN`]
    /// keys.
    index: Option<HashMap<Key, u32, FxBuildHasher>>,
}

impl Clone for Shape {
    /// A copy is a different shape: it gets a fresh id, because the copy is
    /// about to be changed while the original lives on.
    fn clone(&self) -> Self {
        Shape {
            id: fresh_shape_id(),
            keys: self.keys.clone(),
            key_bytes: self.key_bytes,
            index: self.index.clone(),
        }
    }
}

impl fmt::Debug for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Shape#{}{:?}", self.id, self.keys)
    }
}

impl Shape {
    fn empty() -> Self {
        Shape {
            id: fresh_shape_id(),
            keys: Vec::new(),
            key_bytes: 0,
            index: None,
        }
    }

    /// The shape of a record literal with these keys, or `None` when a key
    /// repeats (the literal then keeps only the first position, which the
    /// general insert path handles).
    pub fn from_keys<I: IntoIterator<Item = Key>>(keys: I) -> Option<Arc<Shape>> {
        let mut s = Shape::empty();
        for k in keys {
            if s.position(k.as_str()).is_some() {
                return None;
            }
            s.push(k);
        }
        Some(Arc::new(s))
    }

    /// The process-unique id an inline cache keys on.
    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The position of `key`, if present.
    #[inline]
    pub fn position(&self, key: &str) -> Option<usize> {
        if let Some(index) = &self.index {
            return index.get(key).map(|&i| i as usize);
        }
        let kp = key.as_ptr();
        let kb = key.as_bytes();
        self.keys
            .iter()
            .position(|k| k.as_ptr() == kp && k.len() == kb.len() || k.as_bytes() == kb)
    }

    /// Append a key known to be absent. Keeps the id: every existing key keeps
    /// its position, and the only record that can see this shape is the one
    /// being extended (callers hold it uniquely).
    fn push(&mut self, k: Key) {
        self.key_bytes += k.len() as u64;
        if let Some(index) = &mut self.index {
            index.insert(k.clone(), self.keys.len() as u32);
        } else if self.keys.len() == INDEX_MIN {
            let mut index: HashMap<Key, u32, FxBuildHasher> =
                HashMap::with_capacity_and_hasher(INDEX_MIN * 2, Default::default());
            for (i, k) in self.keys.iter().enumerate() {
                index.insert(k.clone(), i as u32);
            }
            index.insert(k.clone(), self.keys.len() as u32);
            self.index = Some(index);
        }
        self.keys.push(k);
    }

    /// Remove the key at `i`. Positions after it shift, so the shape takes a
    /// fresh id.
    fn remove_at(&mut self, i: usize) -> Key {
        let k = self.keys.remove(i);
        self.key_bytes -= k.len() as u64;
        self.id = fresh_shape_id();
        if self.index.is_some() {
            if self.keys.len() <= INDEX_MIN {
                self.index = None;
            } else {
                let index = self.index.as_mut().unwrap();
                index.clear();
                for (j, k) in self.keys.iter().enumerate() {
                    index.insert(k.clone(), j as u32);
                }
            }
        }
        k
    }
}

static EMPTY_SHAPE: LazyLock<Arc<Shape>> = LazyLock::new(|| Arc::new(Shape::empty()));

/// A record's fields: name → value, in insertion order. See the module docs.
#[derive(Clone)]
pub struct RecordMap {
    shape: Arc<Shape>,
    values: Vec<Value>,
}

impl Default for RecordMap {
    fn default() -> Self {
        RecordMap {
            shape: EMPTY_SHAPE.clone(),
            values: Vec::new(),
        }
    }
}

impl fmt::Debug for RecordMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl PartialEq for RecordMap {
    /// Same keys (in any order) with equal values, like `IndexMap`'s `==`.
    fn eq(&self, other: &RecordMap) -> bool {
        if self.len() != other.len() {
            return false;
        }
        if Arc::ptr_eq(&self.shape, &other.shape) {
            return self.values == other.values;
        }
        self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl RecordMap {
    /// An empty record with room for `n` values.
    pub fn with_capacity(n: usize) -> Self {
        RecordMap {
            shape: EMPTY_SHAPE.clone(),
            values: Vec::with_capacity(n),
        }
    }

    /// A record of `shape` holding `values` in the shape's key order.
    #[inline]
    pub fn from_shape(shape: Arc<Shape>, values: Vec<Value>) -> Self {
        debug_assert_eq!(shape.len(), values.len());
        RecordMap { shape, values }
    }

    #[inline]
    pub fn shape(&self) -> &Arc<Shape> {
        &self.shape
    }

    /// Shorthand for `shape().id()`.
    #[inline]
    pub fn shape_id(&self) -> u64 {
        self.shape.id
    }

    /// The values, in key order.
    #[inline]
    pub fn values_slice(&self) -> &[Value] {
        &self.values
    }

    /// Payload bytes for the heap's accounting: key text plus values.
    pub fn payload_bytes(&self) -> u64 {
        self.shape.key_bytes + (self.values.len() * std::mem::size_of::<Value>()) as u64
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.values.capacity()
    }

    #[inline]
    pub fn get_index_of(&self, key: &str) -> Option<usize> {
        self.shape.position(key)
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.shape.position(key).map(|i| &self.values[i])
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.shape.position(key).map(|i| &mut self.values[i])
    }

    pub fn get_full(&self, key: &str) -> Option<(usize, &Key, &Value)> {
        self.shape
            .position(key)
            .map(|i| (i, &self.shape.keys[i], &self.values[i]))
    }

    pub fn get_key_value(&self, key: &str) -> Option<(&Key, &Value)> {
        self.shape
            .position(key)
            .map(|i| (&self.shape.keys[i], &self.values[i]))
    }

    #[inline]
    pub fn contains_key(&self, key: &str) -> bool {
        self.shape.position(key).is_some()
    }

    pub fn get_index(&self, i: usize) -> Option<(&Key, &Value)> {
        Some((self.shape.keys.get(i)?, self.values.get(i)?))
    }

    pub fn first(&self) -> Option<(&Key, &Value)> {
        self.get_index(0)
    }

    pub fn last(&self) -> Option<(&Key, &Value)> {
        self.get_index(self.len().checked_sub(1)?)
    }

    /// Set `key` to `val`. An existing key keeps its position (and the record
    /// its shape); a new one is appended. Returns the old value, if any.
    pub fn insert<K: IntoKey>(&mut self, key: K, val: Value) -> Option<Value> {
        if let Some(i) = self.shape.position(key.key_str()) {
            return Some(std::mem::replace(&mut self.values[i], val));
        }
        Arc::make_mut(&mut self.shape).push(key.into_key());
        self.values.push(val);
        None
    }

    /// Like [`insert`](Self::insert), also returning the key's position.
    pub fn insert_full<K: IntoKey>(&mut self, key: K, val: Value) -> (usize, Option<Value>) {
        if let Some(i) = self.shape.position(key.key_str()) {
            return (i, Some(std::mem::replace(&mut self.values[i], val)));
        }
        Arc::make_mut(&mut self.shape).push(key.into_key());
        self.values.push(val);
        (self.values.len() - 1, None)
    }

    /// Remove `key`, keeping the order of the rest.
    pub fn shift_remove(&mut self, key: &str) -> Option<Value> {
        let i = self.shape.position(key)?;
        Arc::make_mut(&mut self.shape).remove_at(i);
        Some(self.values.remove(i))
    }

    pub fn shift_remove_entry(&mut self, key: &str) -> Option<(Key, Value)> {
        let i = self.shape.position(key)?;
        let k = Arc::make_mut(&mut self.shape).remove_at(i);
        Some((k, self.values.remove(i)))
    }

    pub fn clear(&mut self) {
        self.shape = EMPTY_SHAPE.clone();
        self.values.clear();
    }

    #[inline]
    pub fn iter(&self) -> Iter<'_> {
        self.shape.keys.iter().zip(self.values.iter())
    }

    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = (&Key, &mut Value)> + ExactSizeIterator {
        self.shape.keys.iter().zip(self.values.iter_mut())
    }

    #[inline]
    pub fn keys(&self) -> std::slice::Iter<'_, Key> {
        self.shape.keys.iter()
    }

    #[inline]
    pub fn values(&self) -> std::slice::Iter<'_, Value> {
        self.values.iter()
    }

    #[inline]
    pub fn values_mut(&mut self) -> std::slice::IterMut<'_, Value> {
        self.values.iter_mut()
    }

    /// Keep the entries `f` accepts, in order.
    pub fn retain<F: FnMut(&Key, &mut Value) -> bool>(&mut self, mut f: F) {
        let mut out = RecordMap::with_capacity(self.len());
        let keys = self.shape.keys.clone();
        for (k, mut v) in keys.into_iter().zip(std::mem::take(&mut self.values)) {
            if f(&k, &mut v) {
                out.insert(k, v);
            }
        }
        *self = out;
    }

    /// Sort entries by key.
    pub fn sort_keys(&mut self) {
        let mut pairs: Vec<(Key, Value)> = std::mem::take(self).into_iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        *self = pairs.into_iter().collect();
    }
}

pub type Iter<'a> = std::iter::Zip<std::slice::Iter<'a, Key>, std::slice::Iter<'a, Value>>;

impl<'a> IntoIterator for &'a RecordMap {
    type Item = (&'a Key, &'a Value);
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl IntoIterator for RecordMap {
    type Item = (Key, Value);
    type IntoIter = std::iter::Zip<std::vec::IntoIter<Key>, std::vec::IntoIter<Value>>;
    fn into_iter(self) -> Self::IntoIter {
        let keys = match Arc::try_unwrap(self.shape) {
            Ok(shape) => shape.keys,
            Err(shared) => shared.keys.clone(),
        };
        keys.into_iter().zip(self.values)
    }
}

impl<K: IntoKey> FromIterator<(K, Value)> for RecordMap {
    fn from_iter<I: IntoIterator<Item = (K, Value)>>(iter: I) -> Self {
        let mut m = RecordMap::default();
        m.extend(iter);
        m
    }
}

impl<K: IntoKey> Extend<(K, Value)> for RecordMap {
    fn extend<I: IntoIterator<Item = (K, Value)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

/// A `GetField` inline cache: the last `(shape id, slot)` a field read hit,
/// packed into one word so concurrent readers never see a torn pair. Zero is
/// empty (shape ids start at 1).
#[derive(Default)]
pub struct FieldCache(AtomicU64);

const SLOT_BITS: u32 = 20;
const SLOT_MASK: u64 = (1 << SLOT_BITS) - 1;

impl FieldCache {
    /// The cached slot of `key` in `map`, when the cache was filled from
    /// `map`'s shape.
    #[inline(always)]
    pub fn lookup(&self, map: &RecordMap) -> Option<Value> {
        let packed = self.0.load(Ordering::Relaxed);
        if packed >> SLOT_BITS == map.shape.id {
            // In range by construction; `get` keeps a bad pack harmless.
            return map.values.get((packed & SLOT_MASK) as usize).copied();
        }
        None
    }

    /// Look `key` up in `map` and remember where it was.
    #[inline]
    pub fn fill(&self, map: &RecordMap, key: &str) -> Option<Value> {
        let i = map.shape.position(key)?;
        let id = map.shape.id;
        if (i as u64) <= SLOT_MASK && id < (1u64 << (64 - SLOT_BITS)) {
            self.0.store(id << SLOT_BITS | i as u64, Ordering::Relaxed);
        }
        Some(map.values[i])
    }
}

impl Clone for FieldCache {
    /// A copied instruction starts cold.
    fn clone(&self) -> Self {
        FieldCache::default()
    }
}

impl fmt::Debug for FieldCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FieldCache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_keeps_order_and_position() {
        let mut m = RecordMap::default();
        m.insert("b", Value::Int(1));
        m.insert("a", Value::Int(2));
        m.insert("b", Value::Int(3));
        let keys: Vec<&str> = m.keys().map(|k| k.as_str()).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(m.get("b"), Some(&Value::Int(3)));
        assert_eq!(m.shift_remove("b"), Some(Value::Int(2 + 1)));
        assert_eq!(m.get("a"), Some(&Value::Int(2)));
        assert!(m.get("b").is_none());
    }

    #[test]
    fn clones_share_a_shape_until_one_grows() {
        let mut a = RecordMap::default();
        a.insert("x", Value::Int(1));
        let mut b = a.clone();
        b.insert("x", Value::Int(2));
        assert_eq!(a.shape_id(), b.shape_id());
        b.insert("y", Value::Int(3));
        assert_ne!(a.shape_id(), b.shape_id());
        assert_eq!(a.len(), 1);
        assert_eq!(a.get("x"), Some(&Value::Int(1)));
    }

    #[test]
    fn big_records_use_an_index() {
        let mut m = RecordMap::default();
        for i in 0..100 {
            m.insert(format!("k{i}"), Value::Int(i));
        }
        for i in 0..100 {
            assert_eq!(m.get(&format!("k{i}")), Some(&Value::Int(i)));
        }
        m.shift_remove("k3");
        assert_eq!(m.get("k4"), Some(&Value::Int(4)));
        assert_eq!(m.get_index_of("k4"), Some(3));
        for i in 5..95 {
            m.shift_remove(&format!("k{i}"));
        }
        assert_eq!(m.len(), 9);
        assert_eq!(m.get("k97"), Some(&Value::Int(97)));
    }

    #[test]
    fn field_cache_hits_only_its_shape() {
        let shape = Shape::from_keys([Key::new("a"), Key::new("b")]).unwrap();
        let r1 = RecordMap::from_shape(shape.clone(), vec![Value::Int(1), Value::Int(2)]);
        let r2 = RecordMap::from_shape(shape, vec![Value::Int(3), Value::Int(4)]);
        let cache = FieldCache::default();
        assert_eq!(cache.lookup(&r1), None);
        assert_eq!(cache.fill(&r1, "b"), Some(Value::Int(2)));
        assert_eq!(cache.lookup(&r2), Some(Value::Int(4)));
        let mut r3 = r2.clone();
        r3.shift_remove("a");
        assert_eq!(cache.lookup(&r3), None);
        assert!(Shape::from_keys([Key::new("a"), Key::new("a")]).is_none());
    }

    #[test]
    fn equality_ignores_order() {
        let a: RecordMap = [("x", Value::Int(1)), ("y", Value::Int(2))].into_iter().collect();
        let b: RecordMap = [("y", Value::Int(2)), ("x", Value::Int(1))].into_iter().collect();
        assert_eq!(a, b);
    }
}
