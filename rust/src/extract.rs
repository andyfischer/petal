//! Typed extraction helpers for pulling Rust data out of Petal `Value`s.
//!
//! Embedders that accept structured data from scripts (petal-sdl draw commands,
//! panel layouts, ...) otherwise hand-walk the heap: match `Value::Map`, call
//! `heap.get_map(id)`, match each field's `Value`, and recurse through lists —
//! re-deriving the same type checks and error strings every time. These methods
//! collapse that into one call per field with an error message that names the
//! field and the type actually found.
//!
//! All accessors are read-only (`&Heap`) and return `Result<_, String>` so an
//! embedder can `?`-propagate. String accessors borrow the heap and hand back
//! `&str`; call `.to_string()` when an owned value is needed.
//!
//! ```ignore
//! // Before:
//! let map_id = match value { Value::Map(id) => id, other => return Err(...) };
//! let kind = match heap.get_map(map_id).get("kind") {
//!     Some(Value::String(id)) => heap.get_string(*id),
//!     Some(other) => return Err(...),
//!     None => return Err(...),
//! };
//!
//! // After:
//! let kind = heap.field_str(value, "kind")?;
//! for child in heap.field_list(value, "children")? { /* recurse */ }
//! let file = heap.opt_field_str(value, "file")?; // Option<&str>, absent or nil -> None
//! ```

use crate::heap::{Heap, MapId};
use crate::value::Value;

impl Heap {
    // ── Scalar / container coercions ─────────────────────────────────────

    /// Require `value` to be a record, returning its `MapId`.
    pub fn as_record(&self, value: Value) -> Result<MapId, String> {
        match value {
            Value::Map(id) => Ok(id),
            other => Err(format!("expected a record, got {}", other.type_name())),
        }
    }

    /// Require `value` to be a string.
    pub fn as_string(&self, value: Value) -> Result<&str, String> {
        match value {
            Value::String(id) => Ok(self.get_string(id)),
            other => Err(format!("expected a string, got {}", other.type_name())),
        }
    }

    /// Require `value` to be a list, returning its elements.
    pub fn as_list(&self, value: Value) -> Result<&[Value], String> {
        match value {
            Value::List(id) => Ok(self.get_list(id)),
            other => Err(format!("expected a list, got {}", other.type_name())),
        }
    }

    /// Require `value` to be an integer. A whole `Float` is accepted and
    /// truncated, matching the leniency of `PetalCxt::get_int`.
    pub fn as_int(&self, value: Value) -> Result<i64, String> {
        match value {
            Value::Int(n) => Ok(n),
            Value::Float(f) => Ok(f as i64),
            other => Err(format!("expected an int, got {}", other.type_name())),
        }
    }

    /// Require `value` to be numeric, returning it as `f64`. Accepts `Int`,
    /// `Float`, and the primal part of a `Dual`.
    pub fn as_float(&self, value: Value) -> Result<f64, String> {
        value
            .as_f64()
            .ok_or_else(|| format!("expected a number, got {}", value.type_name()))
    }

    /// Require `value` to be a bool.
    pub fn as_bool(&self, value: Value) -> Result<bool, String> {
        match value {
            Value::Bool(b) => Ok(b),
            other => Err(format!("expected a bool, got {}", other.type_name())),
        }
    }

    // ── Required record fields ───────────────────────────────────────────

    /// Read a required field from a record `value`. Errors if `value` is not a
    /// record or the field is absent.
    pub fn field(&self, value: Value, name: &str) -> Result<Value, String> {
        let map_id = self.as_record(value)?;
        self.get_map(map_id)
            .get(name)
            .copied()
            .ok_or_else(|| format!("missing field '{}'", name))
    }

    /// Read a required string field. The error names the field and the type
    /// found, e.g. `field 'kind' must be a string, got int`.
    pub fn field_str(&self, value: Value, name: &str) -> Result<&str, String> {
        let v = self.field(value, name)?;
        self.as_string(v)
            .map_err(|_| field_type_err(name, "string", v))
    }

    /// Read a required integer field.
    pub fn field_int(&self, value: Value, name: &str) -> Result<i64, String> {
        let v = self.field(value, name)?;
        self.as_int(v).map_err(|_| field_type_err(name, "int", v))
    }

    /// Read a required numeric field as `f64`.
    pub fn field_float(&self, value: Value, name: &str) -> Result<f64, String> {
        let v = self.field(value, name)?;
        self.as_float(v)
            .map_err(|_| field_type_err(name, "number", v))
    }

    /// Read a required bool field.
    pub fn field_bool(&self, value: Value, name: &str) -> Result<bool, String> {
        let v = self.field(value, name)?;
        self.as_bool(v).map_err(|_| field_type_err(name, "bool", v))
    }

    /// Read a required list field, returning its elements.
    pub fn field_list(&self, value: Value, name: &str) -> Result<&[Value], String> {
        let v = self.field(value, name)?;
        self.as_list(v).map_err(|_| field_type_err(name, "list", v))
    }

    /// Read a required record field, returning its `MapId`.
    pub fn field_record(&self, value: Value, name: &str) -> Result<MapId, String> {
        let v = self.field(value, name)?;
        self.as_record(v)
            .map_err(|_| field_type_err(name, "record", v))
    }

    // ── Optional record fields ───────────────────────────────────────────

    /// Read an optional field. A missing field or an explicit `nil` both yield
    /// `None`. Errors only if `value` is not a record.
    pub fn opt_field(&self, value: Value, name: &str) -> Result<Option<Value>, String> {
        let map_id = self.as_record(value)?;
        Ok(match self.get_map(map_id).get(name).copied() {
            None | Some(Value::Nil) => None,
            Some(v) => Some(v),
        })
    }

    /// Read an optional string field (absent or `nil` -> `None`).
    pub fn opt_field_str(&self, value: Value, name: &str) -> Result<Option<&str>, String> {
        match self.opt_field(value, name)? {
            None => Ok(None),
            Some(v) => self
                .as_string(v)
                .map(Some)
                .map_err(|_| field_type_err(name, "string", v)),
        }
    }
}

/// Build the "field 'x' must be a T, got U" error shared by the typed field
/// accessors.
fn field_type_err(name: &str, expected: &str, found: Value) -> String {
    format!(
        "field '{}' must be a {}, got {}",
        name,
        expected,
        found.type_name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    /// Build a record `{ kind: "row", count: 3, children: [<map>] }` for tests.
    fn sample(heap: &mut Heap) -> Value {
        let inner = heap.alloc_map(IndexMap::new());
        let children = heap.alloc_list(vec![Value::Map(inner)]);
        let kind = Value::String(heap.alloc_string("row".to_string()));
        let mut fields = IndexMap::new();
        fields.insert("kind".to_string(), kind);
        fields.insert("count".to_string(), Value::Int(3));
        fields.insert("children".to_string(), Value::List(children));
        Value::Map(heap.alloc_map(fields))
    }

    #[test]
    fn reads_typed_fields() {
        let mut heap = Heap::new();
        let v = sample(&mut heap);
        assert_eq!(heap.field_str(v, "kind").unwrap(), "row");
        assert_eq!(heap.field_int(v, "count").unwrap(), 3);
        assert_eq!(heap.field_list(v, "children").unwrap().len(), 1);
    }

    #[test]
    fn missing_field_names_the_field() {
        let mut heap = Heap::new();
        let v = sample(&mut heap);
        let err = heap.field_str(v, "nope").unwrap_err();
        assert_eq!(err, "missing field 'nope'");
    }

    #[test]
    fn wrong_field_type_reports_expected_and_actual() {
        let mut heap = Heap::new();
        let v = sample(&mut heap);
        let err = heap.field_str(v, "count").unwrap_err();
        assert_eq!(err, "field 'count' must be a string, got int");
    }

    #[test]
    fn non_record_value_is_rejected() {
        let heap = Heap::new();
        let err = heap.field_str(Value::Int(1), "kind").unwrap_err();
        assert_eq!(err, "expected a record, got int");
    }

    #[test]
    fn optional_field_absent_or_nil_is_none() {
        let mut heap = Heap::new();
        let v = sample(&mut heap);
        assert_eq!(heap.opt_field_str(v, "missing").unwrap(), None);
        assert_eq!(heap.opt_field_str(v, "kind").unwrap(), Some("row"));
    }

    #[test]
    fn optional_field_present_wrong_type_errors() {
        let mut heap = Heap::new();
        let v = sample(&mut heap);
        let err = heap.opt_field_str(v, "count").unwrap_err();
        assert_eq!(err, "field 'count' must be a string, got int");
    }

    #[test]
    fn as_int_truncates_whole_floats() {
        let heap = Heap::new();
        assert_eq!(heap.as_int(Value::Float(4.0)).unwrap(), 4);
    }
}
