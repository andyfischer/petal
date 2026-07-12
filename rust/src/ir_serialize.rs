//! Serde helpers for IR serialization.
//!
//! HashMap<TermId, V> can't be directly serialized to JSON because JSON keys
//! must be strings. This module provides a helper that converts TermId keys
//! to their string representation.

use std::collections::HashMap;

use serde::de::Error as DeError;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::program::TermId;

pub fn serialize_termid_map<V, S>(
    map: &HashMap<TermId, V>,
    serializer: S,
) -> Result<S::Ok, S::Error>
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

/// Inverse of `serialize_termid_map`: read a JSON object whose keys are
/// stringified TermIds back into a `HashMap<TermId, V>`.
pub fn deserialize_termid_map<'de, V, D>(deserializer: D) -> Result<HashMap<TermId, V>, D::Error>
where
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    let string_map: HashMap<String, V> = HashMap::deserialize(deserializer)?;
    let mut out = HashMap::with_capacity(string_map.len());
    for (k, v) in string_map {
        let id = k
            .parse::<u32>()
            .map_err(|_| DeError::custom(format!("invalid TermId key: {:?}", k)))?;
        out.insert(TermId(id), v);
    }
    Ok(out)
}
