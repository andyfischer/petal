//! Host → script values: the `pb_builder` API.
//!
//! A builder records a plain Rust tree ([`HostValue`]) rather than allocating
//! on the Petal heap directly. Materializing happens only at the moment the
//! value is handed over (bound, passed as call arguments, or returned from a
//! native), so a built value never sits on the heap unrooted where a GC could
//! collect it, and one builder can be reused across VMs.

use std::collections::HashMap;
use std::ffi::c_char;

use indexmap::IndexMap;
use petal::heap::Heap;
use petal::symbol::SymbolId;
use petal::value::Value;

use crate::ffi::{BResult, BridgeError, guard};

/// A host-built value, independent of any heap.
#[derive(Clone, Debug, PartialEq)]
pub enum HostValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Vec2(f64, f64),
    Vec3(f64, f64, f64),
    Str(String),
    Symbol(String),
    List(Vec<HostValue>),
    Map(Vec<(String, HostValue)>),
    Enum(String, Vec<HostValue>),
}

impl HostValue {
    /// Collect every symbol name in the tree (for pre-interning).
    fn symbols<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            HostValue::Symbol(s) => out.push(s),
            HostValue::List(items) | HostValue::Enum(_, items) => {
                items.iter().for_each(|i| i.symbols(out))
            }
            HostValue::Map(fields) => fields.iter().for_each(|(_, v)| v.symbols(out)),
            _ => {}
        }
    }

    /// Allocate this tree on `heap`. `syms` must hold every symbol in it
    /// (see [`materialize`]).
    fn to_value(&self, heap: &mut Heap, syms: &HashMap<&str, SymbolId>) -> Value {
        match self {
            HostValue::Nil => Value::Nil,
            HostValue::Bool(b) => Value::Bool(*b),
            HostValue::Int(i) => Value::Int(*i),
            HostValue::Float(f) => Value::Float(*f),
            HostValue::Vec2(x, y) => Value::Vec2(*x, *y),
            HostValue::Vec3(x, y, z) => heap.vec3_value(*x, *y, *z),
            HostValue::Str(s) => Value::String(heap.alloc_string(s.clone())),
            HostValue::Symbol(s) => syms
                .get(s.as_str())
                .map(|id| Value::Symbol(*id))
                .unwrap_or(Value::Nil),
            HostValue::List(items) => {
                let vals: Vec<Value> = items.iter().map(|i| i.to_value(heap, syms)).collect();
                Value::List(heap.alloc_list(vals))
            }
            HostValue::Map(fields) => {
                let mut map = IndexMap::with_capacity(fields.len());
                for (k, v) in fields {
                    let v = v.to_value(heap, syms);
                    map.insert(k.clone(), v);
                }
                Value::Map(heap.alloc_map(map))
            }
            HostValue::Enum(tag, items) => {
                let vals: Vec<Value> = items.iter().map(|i| i.to_value(heap, syms)).collect();
                let data = heap.alloc_list(vals);
                let tag = heap.alloc_string(tag.clone());
                Value::EnumVariant { tag, data }
            }
        }
    }
}

/// Symbol ids for every symbol in `values`, interned through `intern`.
///
/// Materializing is two-phase because the symbol table and the heap are
/// separate borrows on both the `Env` and a `PetalCxt`: intern first, then
/// allocate with [`materialize`].
pub fn intern_symbols(
    values: &[HostValue],
    mut intern: impl FnMut(&str) -> SymbolId,
) -> HashMap<&str, SymbolId> {
    let mut names = Vec::new();
    values.iter().for_each(|v| v.symbols(&mut names));
    names.into_iter().map(|n| (n, intern(n))).collect()
}

/// Allocate `values` on `heap` (symbols from [`intern_symbols`]).
pub fn materialize(
    values: &[HostValue],
    heap: &mut Heap,
    syms: &HashMap<&str, SymbolId>,
) -> Vec<Value> {
    values.iter().map(|v| v.to_value(heap, syms)).collect()
}

/// An open container on the builder stack.
enum Open {
    List(Vec<HostValue>),
    Map(Vec<(String, HostValue)>, Option<String>),
    Enum(String, Vec<HostValue>),
}

/// The state behind a `pb_builder*`.
#[derive(Default)]
pub struct Builder {
    roots: Vec<HostValue>,
    stack: Vec<Open>,
    error: Option<String>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.roots.clear();
        self.stack.clear();
        self.error = None;
    }

    fn fail(&mut self, msg: &str) {
        if self.error.is_none() {
            self.error = Some(msg.to_string());
        }
    }

    /// Append a finished value to the innermost open container (or the roots).
    pub fn push(&mut self, v: HostValue) {
        match self.stack.last_mut() {
            None => self.roots.push(v),
            Some(Open::List(items)) | Some(Open::Enum(_, items)) => items.push(v),
            Some(Open::Map(fields, key)) => match key.take() {
                Some(k) => fields.push((k, v)),
                None => self.fail("map field value without a preceding pb_builder_key"),
            },
        }
    }

    pub fn key(&mut self, k: &str) {
        match self.stack.last_mut() {
            Some(Open::Map(_, key)) => {
                if key.is_some() {
                    self.fail("pb_builder_key called twice without a value");
                } else {
                    *key = Some(k.to_string());
                }
            }
            _ => self.fail("pb_builder_key outside a map"),
        }
    }

    fn begin(&mut self, open: Open) {
        self.stack.push(open);
    }

    fn end(&mut self, what: &str) {
        let v = match self.stack.pop() {
            Some(Open::List(items)) if what == "list" => HostValue::List(items),
            Some(Open::Map(fields, None)) if what == "map" => HostValue::Map(fields),
            Some(Open::Map(_, Some(_))) if what == "map" => {
                self.fail("map ended with a key that has no value");
                return;
            }
            Some(Open::Enum(tag, items)) if what == "enum" => HostValue::Enum(tag, items),
            Some(other) => {
                self.stack.push(other);
                self.fail(&format!(
                    "pb_builder_end_{what} does not match the open container"
                ));
                return;
            }
            None => {
                self.fail(&format!("pb_builder_end_{what} with nothing open"));
                return;
            }
        };
        self.push(v);
    }

    /// The finished roots, or the first misuse error.
    pub fn roots(&self) -> BResult<&[HostValue]> {
        if let Some(e) = &self.error {
            return Err(BridgeError::invalid(format!("builder: {e}")));
        }
        if !self.stack.is_empty() {
            return Err(BridgeError::invalid("builder: unclosed container"));
        }
        Ok(&self.roots)
    }

    /// Exactly one root (for bindings).
    pub fn single(&self) -> BResult<&HostValue> {
        match self.roots()? {
            [one] => Ok(one),
            r => Err(BridgeError::invalid(format!(
                "builder: expected exactly one value, found {}",
                r.len()
            ))),
        }
    }

    /// Zero roots = nil, one root = that value (for native results).
    pub fn result(&self) -> BResult<HostValue> {
        match self.roots()? {
            [] => Ok(HostValue::Nil),
            [one] => Ok(one.clone()),
            r => Err(BridgeError::invalid(format!(
                "native result: expected at most one value, found {}",
                r.len()
            ))),
        }
    }
}

// ── C API ─────────────────────────────────────────────────────────────────

/// Opaque `pb_builder`.
pub type PbBuilder = Builder;

fn with<F: FnOnce(&mut Builder)>(b: *mut PbBuilder, f: F) {
    if b.is_null() {
        return;
    }
    // SAFETY: a non-null pb_builder* came from pb_builder_new (or a pb_call).
    let b = unsafe { &mut *b };
    guard((), || f(b));
}

unsafe fn text(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_new() -> *mut PbBuilder {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(Builder::new()))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_free(b: *mut PbBuilder) {
    if !b.is_null() {
        guard((), || drop(unsafe { Box::from_raw(b) }));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_clear(b: *mut PbBuilder) {
    with(b, |b| b.clear());
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_root_count(b: *const PbBuilder) -> usize {
    if b.is_null() {
        return 0;
    }
    unsafe { &*b }.roots.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_nil(b: *mut PbBuilder) {
    with(b, |b| b.push(HostValue::Nil));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_bool(b: *mut PbBuilder, v: bool) {
    with(b, |b| b.push(HostValue::Bool(v)));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_int(b: *mut PbBuilder, v: i64) {
    with(b, |b| b.push(HostValue::Int(v)));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_float(b: *mut PbBuilder, v: f64) {
    with(b, |b| b.push(HostValue::Float(v)));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_vec2(b: *mut PbBuilder, x: f64, y: f64) {
    with(b, |b| b.push(HostValue::Vec2(x, y)));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_vec3(b: *mut PbBuilder, x: f64, y: f64, z: f64) {
    with(b, |b| b.push(HostValue::Vec3(x, y, z)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_string(b: *mut PbBuilder, utf8: *const c_char, len: usize) {
    with(b, |b| {
        let s = if utf8.is_null() {
            String::new()
        } else {
            // SAFETY: caller passes `len` readable bytes.
            let bytes = unsafe { std::slice::from_raw_parts(utf8 as *const u8, len) };
            String::from_utf8_lossy(bytes).into_owned()
        };
        b.push(HostValue::Str(s));
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_symbol(b: *mut PbBuilder, name: *const c_char) {
    with(b, |b| match unsafe { text(name) } {
        Some(s) => b.push(HostValue::Symbol(s)),
        None => b.fail("pb_builder_symbol: NULL name"),
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_floats(b: *mut PbBuilder, values: *const f64, n: usize) {
    with(b, |b| {
        let items = if values.is_null() || n == 0 {
            Vec::new()
        } else {
            // SAFETY: caller passes `n` readable doubles.
            unsafe { std::slice::from_raw_parts(values, n) }
                .iter()
                .map(|f| HostValue::Float(*f))
                .collect()
        };
        b.push(HostValue::List(items));
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_begin_list(b: *mut PbBuilder) {
    with(b, |b| b.begin(Open::List(Vec::new())));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_end_list(b: *mut PbBuilder) {
    with(b, |b| b.end("list"));
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_begin_map(b: *mut PbBuilder) {
    with(b, |b| b.begin(Open::Map(Vec::new(), None)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_key(b: *mut PbBuilder, key: *const c_char) {
    with(b, |b| match unsafe { text(key) } {
        Some(k) => b.key(&k),
        None => b.fail("pb_builder_key: NULL key"),
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_end_map(b: *mut PbBuilder) {
    with(b, |b| b.end("map"));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_builder_begin_enum(b: *mut PbBuilder, tag: *const c_char) {
    with(b, |b| match unsafe { text(tag) } {
        Some(t) => b.begin(Open::Enum(t, Vec::new())),
        None => {
            b.fail("pb_builder_begin_enum: NULL tag");
            // Keep begin/end balanced so the error is the one reported.
            b.begin(Open::Enum(String::new(), Vec::new()));
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_builder_end_enum(b: *mut PbBuilder) {
    with(b, |b| b.end("enum"));
}
