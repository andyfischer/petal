//! Arguments that arrive as JSON (`pb_vm_call_json`): how they are laid out
//! as a call, and how a function's declared parameter types settle what JSON
//! leaves open.
//!
//! JSON has one number type and its writers disagree about it: JavaScript
//! prints the float `3.0` as `3`, so a `rate: float` parameter fed by a JS
//! client receives an int whenever the value happens to be whole, and
//! `rate / 2` then divides ints. Nor does JSON say anything about what a
//! caller *should* have sent. The function's own declaration does
//! (`Env::function_params`, the data behind `fn_info`), so the bridge reads
//! it and, per argument:
//!
//! | Declared | JSON | Becomes |
//! |----------|------|---------|
//! | `float` | a whole number (`3`) | the float `3.0` |
//! | `int` | a float with no fraction (`3.0`, `1e3`) | the int |
//! | `int` | any other float (`3.5`) | an error |
//! | `int`, `float`, `num` | not a number | an error |
//! | `string` / `str`, `bool`, `list`, `record` | another kind | an error |
//! | anything else, or no annotation | anything | passed as it is |
//!
//! `null` is always passed (as `nil`), whatever the declaration: it is how a
//! caller says "nothing". Only the argument itself is looked at, never what
//! is inside a list or a record, because an annotation says nothing about
//! that.

use petal::env::FunctionParam;
use serde_json::Value as Json;

/// A call's arguments as JSON: positional ones first, then named ones.
#[derive(Debug, Default, PartialEq)]
pub struct JsonArgs {
    pub values: Vec<Json>,
    /// Parallel to `values` (`None` = positional); empty when none is named.
    pub names: Vec<Option<String>>,
}

/// Read the `args_json` of a call: a JSON array is positional arguments, a
/// JSON object is arguments by parameter name, and `None`, an empty string or
/// `null` is no arguments.
pub fn parse_args(text: Option<&str>) -> Result<JsonArgs, String> {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Ok(JsonArgs::default());
    };
    let json: Json =
        serde_json::from_str(text).map_err(|e| format!("the arguments are not valid JSON ({e})"))?;
    match json {
        Json::Null => Ok(JsonArgs::default()),
        Json::Array(values) => Ok(JsonArgs {
            values,
            names: Vec::new(),
        }),
        Json::Object(fields) => {
            let (names, values) = fields.into_iter().map(|(k, v)| (Some(k), v)).unzip();
            Ok(JsonArgs { values, names })
        }
        other => Err(format!(
            "the arguments must be a JSON array (by position) or object (by parameter name), \
             got {}",
            kind(&other)
        )),
    }
}

/// What a JSON value is, in the words an error uses.
fn kind(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "a bool",
        Json::Number(n) if n.is_f64() => "a float",
        Json::Number(_) => "an int",
        Json::String(_) => "a string",
        Json::Array(_) => "a list",
        Json::Object(_) => "a record",
    }
}

/// Settle each argument against the parameter it binds to (see the module
/// docs). `params` is the function's declaration; an argument that binds to
/// no parameter (too many, an unknown name) is left for the call to report.
pub fn coerce(function: &str, params: &[FunctionParam], args: &mut JsonArgs) -> Result<(), String> {
    for (i, value) in args.values.iter_mut().enumerate() {
        let param = match args.names.get(i).and_then(Option::as_deref) {
            Some(name) => params.iter().find(|p| p.name == name),
            None => params.get(i),
        };
        let Some(param) = param else { continue };
        let Some(ty) = param.ty.as_deref() else { continue };
        if value.is_null() {
            continue;
        }
        let mismatch = |value: &Json| {
            let shown = match value {
                Json::Array(_) | Json::Object(_) => String::new(),
                other => format!(" ({other})"),
            };
            format!(
                "{function}(): parameter '{}' is declared {ty}, got {}{shown}",
                param.name,
                kind(value)
            )
        };
        match ty {
            "float" => match value.as_f64() {
                Some(f) if !value.is_f64() => {
                    // Every i64 has a finite f64, so this cannot fail.
                    *value = serde_json::Number::from_f64(f).map(Json::Number).unwrap_or(Json::Null);
                }
                Some(_) => {}
                None => return Err(mismatch(value)),
            },
            "int" => match value {
                Json::Number(n) if n.is_i64() => {}
                Json::Number(n) => match n.as_f64() {
                    // 2^63 as an f64: the first value past the largest i64.
                    Some(f) if f.fract() == 0.0 && f >= -9.223372036854775808e18 && f < 9.223372036854775808e18 => {
                        *value = Json::from(f as i64);
                    }
                    _ => return Err(mismatch(value)),
                },
                _ => return Err(mismatch(value)),
            },
            "num" if !value.is_number() => return Err(mismatch(value)),
            "string" | "str" if !value.is_string() => return Err(mismatch(value)),
            "bool" if !value.is_boolean() => return Err(mismatch(value)),
            "list" if !value.is_array() => return Err(mismatch(value)),
            "record" if !value.is_object() => return Err(mismatch(value)),
            _ => {}
        }
    }
    Ok(())
}

/// `pb_vm_function_json`: a function's parameters as a host reads them.
pub fn params_json(function: &str, params: &[FunctionParam]) -> Json {
    let params: Vec<Json> = params
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "type": p.ty,
                "has_default": p.has_default,
                "default_source": p.default_source,
            })
        })
        .collect();
    serde_json::json!({ "name": function, "params": params })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn param(name: &str, ty: Option<&str>) -> FunctionParam {
        FunctionParam {
            name: name.to_string(),
            ty: ty.map(str::to_string),
            has_default: false,
            default_source: None,
        }
    }

    #[test]
    fn arguments_are_an_array_an_object_or_nothing() {
        assert_eq!(parse_args(None).unwrap(), JsonArgs::default());
        assert_eq!(parse_args(Some("  ")).unwrap(), JsonArgs::default());
        assert_eq!(parse_args(Some("null")).unwrap(), JsonArgs::default());
        let a = parse_args(Some("[1, \"x\"]")).unwrap();
        assert_eq!((a.values, a.names), (vec![json!(1), json!("x")], vec![]));
        let a = parse_args(Some("{\"b\": 2, \"a\": [1]}")).unwrap();
        assert_eq!(a.names, [Some("a".to_string()), Some("b".to_string())]);
        assert_eq!(a.values, [json!([1]), json!(2)]);
        assert!(parse_args(Some("3")).unwrap_err().contains("got an int"));
        assert!(parse_args(Some("[1,")).unwrap_err().contains("not valid JSON"));
    }

    #[test]
    fn numbers_follow_the_declared_type() {
        let params = [
            param("rate", Some("float")),
            param("cents", Some("int")),
            param("n", Some("num")),
            param("free", None),
            param("kind", Some("Kind")),
        ];
        let mut args = parse_args(Some("[3, 4.0, 5, 6, 7]")).unwrap();
        coerce("toll", &params, &mut args).unwrap();
        assert!(args.values[0].is_f64() && args.values[0] == json!(3.0));
        assert!(args.values[1].is_i64() && args.values[1] == json!(4));
        // `num`, no annotation and a type the bridge does not know: as sent.
        assert!(args.values[2].is_i64() && args.values[3].is_i64() && args.values[4].is_i64());

        // By name, in any order.
        let mut args = parse_args(Some("{\"cents\": 1e3, \"rate\": 2}")).unwrap();
        coerce("toll", &params, &mut args).unwrap();
        assert_eq!(args.values, [json!(1000), json!(2.0)]);
        assert!(args.values[0].is_i64() && args.values[1].is_f64());
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_named() {
        let params = [
            param("id", Some("string")),
            param("cents", Some("int")),
            param("on", Some("bool")),
            param("xs", Some("list")),
            param("r", Some("record")),
            param("rate", Some("float")),
        ];
        let err = |text: &str| {
            let mut args = parse_args(Some(text)).unwrap();
            coerce("toll", &params, &mut args).unwrap_err()
        };
        assert_eq!(
            err("{\"cents\": \"abc\"}"),
            "toll(): parameter 'cents' is declared int, got a string (\"abc\")"
        );
        assert_eq!(err("[\"a\", 3.5]"), "toll(): parameter 'cents' is declared int, got a float (3.5)");
        assert_eq!(err("[5]"), "toll(): parameter 'id' is declared string, got an int (5)");
        assert!(err("{\"on\": 1}").contains("'on' is declared bool"));
        assert!(err("{\"xs\": {}}").ends_with("'xs' is declared list, got a record"));
        assert!(err("{\"r\": []}").contains("'r' is declared record, got a list"));
        assert!(err("{\"rate\": \"fast\"}").contains("'rate' is declared float"));
        // null is nil whatever the declaration, and what binds to no
        // parameter is the call's to report.
        let mut args = parse_args(Some("{\"id\": null, \"nope\": 1}")).unwrap();
        coerce("toll", &params, &mut args).unwrap();
        let mut args = parse_args(Some("[\"a\", 1, true, [], {}, 1, 99]")).unwrap();
        coerce("toll", &params, &mut args).unwrap();
    }
}
