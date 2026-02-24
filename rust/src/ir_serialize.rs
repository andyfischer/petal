//! Serde helpers for IR serialization.
//!
//! HashMap<TermId, V> can't be directly serialized to JSON because JSON keys
//! must be strings. This module provides a helper that converts TermId keys
//! to their string representation.

use std::collections::HashMap;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::program::TermId;

pub fn serialize_termid_map<V, S>(map: &HashMap<TermId, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    V: Serialize,
    S: Serializer,
{
    let mut ser_map = serializer.serialize_map(Some(map.len()))?;
    for (k, v) in map {
        ser_map.serialize_entry(&k.0.to_string(), v)?;
    }
    ser_map.end()
}
