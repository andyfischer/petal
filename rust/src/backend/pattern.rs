//! Shared runtime pattern matching, used by both execution backends.
//!
//! [`match_pattern`] tests a `Value` against a `Pattern`, accumulating variable
//! bindings. It is pure over `&mut Heap` (mutable only because rest-patterns
//! allocate the remainder list), so the graph engine's `Match` handler and the
//! bytecode VM's `MatchArm` op share it verbatim — the pattern-matching parity
//! lever. How the resulting bindings are written into registers differs between
//! the engines and stays in each.

use crate::ast::{Literal, Pattern};
use crate::heap::Heap;
use crate::value::Value;

/// Match `value` against `pattern`, pushing any captured `(name, value)`
/// bindings onto `bindings`. Returns whether the pattern matched. On a failed
/// match, `bindings` may contain partial captures and should be discarded.
pub fn match_pattern(
    pattern: &Pattern,
    value: Value,
    heap: &mut Heap,
    bindings: &mut Vec<(String, Value)>,
) -> bool {
    match pattern {
        Pattern::Wildcard => true,

        Pattern::Literal(lit) => match (lit, value) {
            (Literal::Nil, Value::Nil) => true,
            (Literal::Bool(a), Value::Bool(b)) => *a == b,
            (Literal::Int(a), Value::Int(b)) => *a == b,
            (Literal::Float(a), Value::Float(b)) => *a == b,
            (Literal::String(a), Value::String(sid)) => a == heap.get_string(sid),
            _ => false,
        },

        Pattern::Variable(name) => {
            // Pure variable binding — always matches and captures the value.
            // (Known enum variant names are resolved to Pattern::Variant by the
            // compiler.)
            bindings.push((name.clone(), value));
            true
        }

        Pattern::Variant { name, fields } => {
            let Value::EnumVariant { tag, data } = value else {
                return false;
            };
            if heap.get_string(tag) != name {
                return false;
            }
            let data_fields = heap.get_list(data);
            if data_fields.len() != fields.len() {
                return false;
            }
            let data_copy: Vec<Value> = data_fields.to_vec();
            fields
                .iter()
                .zip(data_copy)
                .all(|(pat, val)| match_pattern(pat, val, heap, bindings))
        }

        Pattern::List { elements, rest } => {
            let Value::List(list_id) = value else {
                return false;
            };
            let list_copy: Vec<Value> = heap.get_list(list_id).to_vec();
            match rest {
                Some(rest_name) => {
                    if list_copy.len() < elements.len() {
                        return false;
                    }
                    for (pat, val) in elements.iter().zip(list_copy.iter()) {
                        if !match_pattern(pat, *val, heap, bindings) {
                            return false;
                        }
                    }
                    let rest_vals: Vec<Value> = list_copy[elements.len()..].to_vec();
                    let rest_list = Value::List(heap.alloc_list(rest_vals));
                    bindings.push((rest_name.clone(), rest_list));
                    true
                }
                None => {
                    list_copy.len() == elements.len()
                        && elements
                            .iter()
                            .zip(list_copy)
                            .all(|(pat, val)| match_pattern(pat, val, heap, bindings))
                }
            }
        }

        Pattern::Record(fields) => {
            let Value::Map(map_id) = value else {
                return false;
            };
            // Copy relevant entries out before recursive matching.
            let entries: Vec<(String, Value)> = {
                let map = heap.get_map(map_id);
                fields
                    .iter()
                    .filter_map(|(key, _)| map.get(key).map(|&val| (key.clone(), val)))
                    .collect()
            };
            if entries.len() != fields.len() {
                return false; // Some fields missing.
            }
            fields
                .iter()
                .zip(entries)
                .all(|((_, pat), (_, val))| match_pattern(pat, val, heap, bindings))
        }
    }
}
