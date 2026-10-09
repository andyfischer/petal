//! [`StaticValue`] as JSON, both ways — the wire form a host in another
//! language uses to read a config file's values and to say what an edit should
//! write (the C bridge's `pb_source_*`).
//!
//! Floats are JSON numbers, strings JSON strings, lists arrays and `nil`
//! `null`. The shapes JSON cannot tell apart are single-key objects:
//!
//! ```text
//! {"int": 3}                         an integer (a bare number is a float)
//! {"rec": {"key": value, ...}}       a record, keys in order
//! {"call": "vec3", "args": [...]}    a call, unevaluated
//! {"color": "#ff2e88"}               a color literal
//! ```

use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};

use crate::static_value::StaticValue;

/// `value` as JSON. A float that is not finite has no JSON form and is written
/// `null`.
pub fn to_json(value: &StaticValue) -> String {
    let mut out = String::new();
    write_json(value, &mut out);
    out
}

fn quoted(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

fn write_list(items: &[StaticValue], out: &mut String) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_json(item, out);
    }
    out.push(']');
}

fn write_json(value: &StaticValue, out: &mut String) {
    match value {
        StaticValue::Nil => out.push_str("null"),
        StaticValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        StaticValue::Int(n) => out.push_str(&format!("{{\"int\":{n}}}")),
        StaticValue::Float(f) if f.is_finite() => out.push_str(&format!("{f:?}")),
        StaticValue::Float(_) => out.push_str("null"),
        StaticValue::Str(s) => out.push_str(&quoted(s)),
        StaticValue::Color { .. } => {
            out.push_str(&format!("{{\"color\":{}}}", quoted(&value.to_source())))
        }
        StaticValue::List(items) => write_list(items, out),
        StaticValue::Record(fields) => {
            out.push_str("{\"rec\":{");
            let mut seen: Vec<&str> = Vec::new();
            for (key, field) in fields {
                // A key written twice keeps its first position (JSON objects
                // cannot repeat one); the later value is what the program reads.
                if seen.contains(&key.as_str()) {
                    continue;
                }
                if !seen.is_empty() {
                    out.push(',');
                }
                seen.push(key);
                out.push_str(&quoted(key));
                out.push(':');
                let last = fields
                    .iter()
                    .rev()
                    .find(|(k, _)| k == key)
                    .map_or(field, |(_, v)| v);
                write_json(last, out);
            }
            out.push_str("}}");
        }
        StaticValue::Call { function, args } => {
            out.push_str(&format!("{{\"call\":{},\"args\":", quoted(function)));
            write_list(args, out);
            out.push('}');
        }
    }
}

/// Read a value from the JSON [`to_json`] writes. Record fields keep the order
/// they have in the text.
pub fn from_json(text: &str) -> Result<StaticValue, String> {
    serde_json::from_str::<Wire>(text)
        .map(|wire| wire.0)
        .map_err(|e| format!("not a static value in JSON: {e}"))
}

/// A [`StaticValue`] being deserialized. Driven by hand rather than through
/// `serde_json::Value`, whose objects do not keep their key order.
struct Wire(StaticValue);

/// The fields of a `{"rec": {...}}`, in document order.
struct Fields(Vec<(String, StaticValue)>);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of record fields")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Wire>()? {
                    fields.push((key, value.0));
                }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_map(FieldsVisitor)
    }
}

impl<'de> Deserialize<'de> for Wire {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct WireVisitor;
        impl<'de> Visitor<'de> for WireVisitor {
            type Value = Wire;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a number, string, boolean, null, array, or one of {\"int\"}, {\"rec\"}, {\"call\", \"args\"}, {\"color\"}",
                )
            }
            fn visit_unit<E: Error>(self) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Nil))
            }
            fn visit_bool<E: Error>(self, b: bool) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Bool(b)))
            }
            fn visit_i64<E: Error>(self, n: i64) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Float(n as f64)))
            }
            fn visit_u64<E: Error>(self, n: u64) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Float(n as f64)))
            }
            fn visit_f64<E: Error>(self, f: f64) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Float(f)))
            }
            fn visit_str<E: Error>(self, s: &str) -> Result<Wire, E> {
                Ok(Wire(StaticValue::Str(s.to_string())))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Wire, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Wire>()? {
                    items.push(item.0);
                }
                Ok(Wire(StaticValue::List(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Wire, A::Error> {
                let mut value = None;
                let (mut function, mut args) = (None::<String>, None::<Vec<Wire>>);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "int" => {
                            let n = map.next_value::<serde_json::Number>()?;
                            let int = n.as_i64().or_else(|| {
                                n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)
                            });
                            value = Some(StaticValue::Int(int.ok_or_else(|| {
                                A::Error::custom(format!("{{\"int\": {n}}} is not an integer"))
                            })?));
                        }
                        "rec" => value = Some(StaticValue::Record(map.next_value::<Fields>()?.0)),
                        "color" => {
                            let hex = map.next_value::<String>()?;
                            value = Some(StaticValue::color_hex(&hex).ok_or_else(|| {
                                A::Error::custom(format!(
                                    "`{hex}` is not a color (#rgb, #rgba, #rrggbb or #rrggbbaa)"
                                ))
                            })?);
                        }
                        "call" => function = Some(map.next_value()?),
                        "args" => args = Some(map.next_value()?),
                        other => {
                            return Err(A::Error::custom(format!(
                                "unknown value tag `{other}` (a record is {{\"rec\": {{...}}}})"
                            )));
                        }
                    }
                }
                let call = function.map(|function| StaticValue::Call {
                    function,
                    args: args.unwrap_or_default().into_iter().map(|a| a.0).collect(),
                });
                value.or(call).map(Wire).ok_or_else(|| {
                    A::Error::custom("an object needs one of `int`, `rec`, `call`, `color`")
                })
            }
        }
        deserializer.deserialize_any(WireVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_through_json_keeping_field_order() {
        let value = StaticValue::record(vec![
            ("zeta", StaticValue::int(-3)),
            ("alpha", StaticValue::float(2.0)),
            ("tint", StaticValue::color(0xff, 0x2e, 0x88)),
            (
                "list",
                StaticValue::list([
                    StaticValue::str("a\"b"),
                    StaticValue::nil(),
                    StaticValue::bool(true),
                    StaticValue::call("vec3", [0.5, -1.0, 0.0]),
                    StaticValue::color_alpha(1, 2, 3, 4),
                ]),
            ),
        ]);
        let json = to_json(&value);
        assert_eq!(
            json,
            r##"{"rec":{"zeta":{"int":-3},"alpha":2.0,"tint":{"color":"#ff2e88"},"list":["a\"b",null,true,{"call":"vec3","args":[0.5,-1.0,0.0]},{"color":"#01020304"}]}}"##
        );
        assert_eq!(from_json(&json), Ok(value));
    }

    #[test]
    fn reads_what_a_host_would_write() {
        // A bare number is a float; an integer has to say so.
        assert_eq!(from_json("2"), Ok(StaticValue::float(2.0)));
        assert_eq!(from_json(r#"{"int": 2}"#), Ok(StaticValue::int(2)));
        assert_eq!(from_json(r#"{"int": 2.0}"#), Ok(StaticValue::int(2)));
        assert_eq!(
            from_json(r##"{"color": "#F80"}"##),
            Ok(StaticValue::color(0xff, 0x88, 0))
        );
        assert_eq!(
            from_json(r#"{"args": [1], "call": "f"}"#),
            Ok(StaticValue::call("f", [1.0]))
        );
        assert_eq!(
            from_json(r#"{"call": "f"}"#),
            Ok(StaticValue::call("f", Vec::<StaticValue>::new()))
        );
        for bad in [
            r#"{"a": 1}"#,
            r#"{"int": 2.5}"#,
            r#"{"color": "red"}"#,
            "{}",
            "[1,",
        ] {
            assert!(from_json(bad).is_err(), "{bad} should be refused");
        }
    }
}
