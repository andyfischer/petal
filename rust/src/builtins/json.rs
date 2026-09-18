//! `json_stringify` / `json_parse`: the string codec for anything that has to
//! leave the process as text — chiefly the panel store, whose values are
//! strings, and whose docs have always told authors to "pair it with
//! `json_stringify`". Every embedder already exposed the conversion (the debug
//! server's `panel.values`, GPP, the MCP tools) through
//! [`crate::value::value_to_json`]; this is the same conversion handed to the
//! script.
//!
//! The mapping is the one `value_to_json` / `json_to_value` define: nil ↔
//! `null`, bools, ints and floats ↔ numbers, strings, lists ↔ arrays, records
//! ↔ objects. `json_parse` yields plain records, not class instances, so a
//! round trip flattens a class value to its fields — the same thing a `state`
//! dump shows. A value with no JSON form (a function, a symbol) stringifies as
//! its display text rather than failing, since a stringified debug dump is
//! more useful than an aborted frame.

use crate::native_fn::PetalCxt;
use crate::value::{json_to_value, value_to_json};

use super::require_args;

/// `json_stringify(value) -> string`: compact JSON, no whitespace. Records keep
/// their field order, so a stringified record is stable across frames and
/// diffable in a store.
///
/// `json_stringify(value, indent)` pretty-prints with `indent` spaces per
/// level (0 is the compact form) — for a file a person will read.
pub(super) fn native_json_stringify(state: &mut PetalCxt) -> Result<u32, String> {
    let indent = match state.arg_count() {
        1 => 0,
        2 => state.get_int(2)?,
        n => {
            return Err(format!(
                "json_stringify() expects 1 or 2 arguments (value, indent), got {n}"
            ));
        }
    };
    let value = state.get_value(1)?;
    let json = value_to_json(&value, state.heap());
    let text = if indent > 0 {
        let pad = " ".repeat(indent.clamp(0, 16) as usize);
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(pad.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        serde::Serialize::serialize(&json, &mut ser)
            .map_err(|e| format!("json_stringify() failed: {e}"))?;
        String::from_utf8(buf).map_err(|e| format!("json_stringify() failed: {e}"))?
    } else {
        serde_json::to_string(&json).map_err(|e| format!("json_stringify() failed: {e}"))?
    };
    state.push_string(text);
    Ok(1)
}

/// `json_parse(text) -> value | nil`: the value the JSON text denotes, or `nil`
/// when the text is not JSON — the same failable contract as `parse_int` /
/// `parse_float`, so a corrupt store entry is a `?? default`, not an aborted
/// frame. Note that the JSON text `null` also parses to `nil`; a caller that
/// must tell the two apart checks the text first.
pub(super) fn native_json_parse(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "json_parse")?;
    let text = state.get_string(1)?;
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => {
            let v = json_to_value(&json, state.heap_mut())?;
            state.push_value(v);
        }
        Err(_) => state.push_nil(),
    }
    Ok(1)
}
