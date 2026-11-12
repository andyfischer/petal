//! `pb_source`: reading and editing Petal source text over the C ABI, without
//! running it.
//!
//! This is the bridge's window onto Petal as a **configuration format** (see
//! `docs/usage/config-files.md` and `docs/features/program-modification.md`): a host that
//! shows a script's settings — an editor panel of sliders, say — reads them
//! with [`pb_source_bindings_json`], and writes them back by path
//! (`POST.effects[2].amount`): [`pb_source_set_value`] (and
//! [`pb_source_set_setting`], by a setting's name), [`pb_source_insert`],
//! [`pb_source_append`] and [`pb_source_remove`] take values (as JSON) and
//! edit the literal in place through `petal::literal_edit` — only what differs
//! is rewritten, and new text is formatted like what is already there — while
//! [`pb_source_set`] splices in source text the host wrote itself. Either way
//! every other character of the file, comments and layout included, stays as
//! the author wrote it. The edited text is then the host's to save; a running
//! program picks it up by hot reload.
//!
//! A source is independent of any VM: it is text plus a parse.

use std::ffi::{CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use petal::goal_based_editing::Placement;
use petal::literal_edit::{self, EditError, EditErrorKind};
use petal::rewrite::{find_binding_path, parse_ast, parse_edit_path, splice, splice_node};
use petal::static_json;
use petal::static_value::{StaticValue, static_bindings};

use crate::ffi::{Status, arg_str, cstring_lossy, guard, panic_message};

/// The state behind a `pb_source*`.
pub struct PbSource {
    text: String,
    /// Buffers behind the pointers handed out.
    text_c: CString,
    json: CString,
    expr: CString,
    /// Message of the last failed call, if the most recent call failed.
    error: Option<CString>,
}

impl PbSource {
    fn new(text: &str) -> PbSource {
        PbSource {
            text: text.to_string(),
            text_c: cstring_lossy(text),
            json: CString::default(),
            expr: CString::default(),
            error: None,
        }
    }

    fn fail(&mut self, code: Status, msg: String) -> Status {
        self.error = Some(cstring_lossy(&msg));
        code
    }

    /// The char range of the expression `path` names, or why there is none.
    fn locate(&self, path: &str) -> Result<petal::source_map::SourceSpan, (Status, String)> {
        let (name, segs) = parse_edit_path(path).map_err(|e| (Status::InvalidArg, e))?;
        let (_, stmts) = parse_ast(&self.text)
            .map_err(|e| (Status::Compile, format!("source did not parse: {e}")))?;
        find_binding_path(&stmts, &name, &segs).ok_or_else(|| {
            (
                Status::NotFound,
                if segs.is_empty() {
                    format!("no top-level binding for `{name}`")
                } else {
                    format!("`{path}` names no literal value in the source")
                },
            )
        })
    }

    fn set(&mut self, path: &str, value: &str) -> Status {
        let span = match self.locate(path) {
            Ok(span) => span,
            Err((code, msg)) => return self.fail(code, msg),
        };
        // The replacement must be one expression: a tree splice checks that by
        // construction; the string fallback (for a span no single node covers)
        // only runs for text that parses as a lone expression statement.
        let value = value.trim();
        let is_expression = parse_ast(value).is_ok_and(|(_, stmts)| {
            matches!(&stmts[..], [stmt] if matches!(stmt.kind, petal::ast::StmtKind::Expr(_)))
        });
        if !is_expression {
            return self.fail(
                Status::InvalidArg,
                format!("`{value}` is not a single Petal expression"),
            );
        }
        let chars: Vec<char> = self.text.chars().collect();
        let old: String = chars[span.start.offset as usize..span.end.offset as usize]
            .iter()
            .collect();
        // A goal that already holds writes nothing.
        if old == value {
            self.error = None;
            return Status::Ok;
        }
        let Ok((tree, _)) = parse_ast(&self.text) else {
            return self.fail(Status::Compile, "source did not parse".into());
        };
        let edited = match splice_node(&tree, span, value) {
            Some(tree) => tree.text(),
            None => splice(&self.text, span, value),
        };
        // Never hand back text that stopped parsing.
        if let Err(e) = parse_ast(&edited) {
            return self.fail(
                Status::InvalidArg,
                format!("setting `{path}` to `{value}` would break the source: {e}"),
            );
        }
        self.text = edited;
        self.text_c = cstring_lossy(&self.text);
        self.error = None;
        Status::Ok
    }

    /// Run a value edit on the text; keep its result, or its error and the
    /// text as it was.
    fn edit(
        &mut self,
        value_json: Option<&str>,
        f: impl FnOnce(&str, &StaticValue) -> Result<String, EditError>,
    ) -> Status {
        let value = match value_json.map(static_json::from_json) {
            Some(Ok(value)) => value,
            Some(Err(e)) => return self.fail(Status::InvalidArg, e),
            None => StaticValue::Nil,
        };
        match f(&self.text, &value) {
            Ok(edited) => {
                if edited != self.text {
                    self.text = edited;
                    self.text_c = cstring_lossy(&self.text);
                }
                self.error = None;
                Status::Ok
            }
            Err(e) => {
                let code = match e.kind {
                    EditErrorKind::Parse => Status::Compile,
                    EditErrorKind::Invalid => Status::InvalidArg,
                    EditErrorKind::NotFound => Status::NotFound,
                };
                self.fail(code, e.message)
            }
        }
    }

    fn set_value(&mut self, path: &str, value_json: &str) -> Status {
        self.edit(Some(value_json), |text, value| {
            literal_edit::set_path(text, path, value)
        })
    }

    /// Set the value of the setting `name`: only `v` of `let NAME =
    /// tunable(v, …)` changes; bounds, options and comment stay as written.
    fn set_setting(&mut self, name: &str, value_json: &str) -> Status {
        let bindings = match static_bindings(&self.text) {
            Ok(b) => b,
            Err(e) => return self.fail(Status::Compile, format!("source did not parse: {e}")),
        };
        match bindings.iter().rev().find(|b| b.name == name) {
            Some(b) if b.is_config => self.set_value(name, value_json),
            Some(_) => self.fail(
                Status::InvalidArg,
                format!("`{name}` is not a setting (write it `let {name} = tunable(…)`)"),
            ),
            None => self.fail(Status::NotFound, format!("no top-level binding for `{name}`")),
        }
    }

    fn insert(&mut self, path: &str, value_json: &str, before: Option<&str>) -> Status {
        let placement = before.map_or(Placement::End, |key| Placement::Before(key.to_string()));
        self.edit(Some(value_json), |text, value| {
            literal_edit::insert_path(text, path, value, &placement)
        })
    }

    fn append(&mut self, path: &str, value_json: &str) -> Status {
        self.edit(Some(value_json), |text, value| {
            literal_edit::append_path(text, path, value)
        })
    }

    fn remove(&mut self, path: &str) -> Status {
        self.edit(None, |text, _| literal_edit::remove_path(text, path))
    }

    fn move_to(&mut self, from: &str, to: &str) -> Status {
        self.edit(None, |text, _| literal_edit::move_path(text, from, to))
    }
}

/// Every top-level binding of `source` as a JSON array, in source order.
pub fn bindings_json(source: &str) -> Result<String, String> {
    let bindings = static_bindings(source).map_err(|e| e.to_string())?;
    let mut out = String::from("[");
    for (i, b) in bindings.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let string = |s: &str| serde_json::Value::from(s).to_string();
        out.push_str(&format!(
            "{{\"name\":{},\"config\":{},\"exported\":{},\"line\":{}",
            string(&b.name),
            b.is_config,
            b.exported,
            b.line
        ));
        if let Some(text) = &b.text {
            out.push_str(&format!(",\"text\":{}", string(text)));
        }
        if let Some(comment) = &b.comment {
            out.push_str(&format!(",\"comment\":{}", string(comment)));
        }
        match &b.value {
            Ok(value) => {
                out.push_str(",\"value\":");
                out.push_str(&static_json::to_json(value));
            }
            Err(reason) => out.push_str(&format!(",\"reason\":{}", string(reason))),
        }
        // A setting (`let X = tunable(v, min, max, step, options)`, or
        // `config let`): what was written, each key omitted when it is not.
        // `"config"` and `"range"` are its deprecated mirrors
        // (docs/tasks/lifecycle-annotation-functions-refactor.md).
        let bounds: Vec<String> = b
            .range
            .iter()
            .flat_map(|r| [("min", r.min), ("max", r.max), ("step", r.step)])
            .filter_map(|(k, v)| Some(format!("\"{k}\":{}", serde_json::Value::from(v?))))
            .collect();
        if b.is_config {
            let mut meta = bounds.clone();
            if let Some(options) = &b.options {
                let items: Vec<String> = options.iter().map(static_json::to_json).collect();
                meta.push(format!("\"options\":[{}]", items.join(",")));
            }
            out.push_str(&format!(",\"tunable\":{{{}}}", meta.join(",")));
        }
        if b.range.is_some() {
            out.push_str(&format!(",\"range\":{{{}}}", bounds.join(",")));
        }
        out.push('}');
    }
    out.push(']');
    Ok(out)
}

/// Run `f` on a source with NULL and panic protection.
fn with_source<T>(s: *mut PbSource, fallback: T, f: impl FnOnce(&mut PbSource) -> T) -> T {
    if s.is_null() {
        return fallback;
    }
    // SAFETY: a non-null pb_source* came from pb_source_new.
    let s = unsafe { &mut *s };
    match catch_unwind(AssertUnwindSafe(|| f(&mut *s))) {
        Ok(v) => v,
        Err(payload) => {
            s.error = Some(cstring_lossy(&format!(
                "petal-bridge internal panic: {}",
                panic_message(&*payload)
            )));
            fallback
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_new(text: *const c_char) -> *mut PbSource {
    guard(std::ptr::null_mut(), || {
        let text = unsafe { arg_str(text, "text") }.unwrap_or("");
        Box::into_raw(Box::new(PbSource::new(text)))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_free(s: *mut PbSource) {
    if !s.is_null() {
        guard((), || drop(unsafe { Box::from_raw(s) }));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_source_text(s: *const PbSource) -> *const c_char {
    if s.is_null() {
        return std::ptr::null();
    }
    // SAFETY: a non-null pb_source* came from pb_source_new.
    unsafe { &*s }.text_c.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_source_bindings_json(s: *mut PbSource) -> *const c_char {
    with_source(s, std::ptr::null(), |s| match bindings_json(&s.text) {
        Ok(json) => {
            s.json = cstring_lossy(&json);
            s.error = None;
            s.json.as_ptr()
        }
        Err(e) => {
            s.fail(Status::Compile, e);
            std::ptr::null()
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_expr(s: *mut PbSource, path: *const c_char) -> *const c_char {
    with_source(s, std::ptr::null(), |s| {
        let path = match unsafe { arg_str(path, "path") } {
            Ok(p) => p,
            Err(e) => {
                s.fail(e.code, e.message);
                return std::ptr::null();
            }
        };
        match s.locate(path) {
            Ok(span) => {
                let text: String = s
                    .text
                    .chars()
                    .skip(span.start.offset as usize)
                    .take((span.end.offset - span.start.offset) as usize)
                    .collect();
                s.expr = cstring_lossy(&text);
                s.error = None;
                s.expr.as_ptr()
            }
            Err((code, msg)) => {
                s.fail(code, msg);
                std::ptr::null()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_set(
    s: *mut PbSource,
    path: *const c_char,
    value: *const c_char,
) -> Status {
    let st = with_source(s, Status::Panic, |s| {
        let path = match unsafe { arg_str(path, "path") } {
            Ok(p) => p,
            Err(e) => return s.fail(e.code, e.message),
        };
        let value = match unsafe { arg_str(value, "value") } {
            Ok(v) => v,
            Err(e) => return s.fail(e.code, e.message),
        };
        s.set(path, value)
    });
    if s.is_null() { Status::InvalidArg } else { st }
}

/// The shared shape of the value-edit entry points: decode the string
/// arguments (`optional` ones may be NULL), then run `f`.
unsafe fn edit_call<const N: usize>(
    s: *mut PbSource,
    args: [(*const c_char, &'static str, bool); N],
    f: impl FnOnce(&mut PbSource, [Option<&str>; N]) -> Status,
) -> Status {
    let st = with_source(s, Status::Panic, |s| {
        let mut decoded = [None; N];
        for (slot, (ptr, name, optional)) in decoded.iter_mut().zip(args) {
            if ptr.is_null() && optional {
                continue;
            }
            match unsafe { arg_str(ptr, name) } {
                Ok(text) => *slot = Some(text),
                Err(e) => return s.fail(e.code, e.message),
            }
        }
        f(s, decoded)
    });
    if s.is_null() { Status::InvalidArg } else { st }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_set_value(
    s: *mut PbSource,
    path: *const c_char,
    value_json: *const c_char,
) -> Status {
    unsafe {
        edit_call(
            s,
            [(path, "path", false), (value_json, "value_json", false)],
            |s, [path, value]| s.set_value(path.unwrap_or(""), value.unwrap_or("")),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_set_setting(
    s: *mut PbSource,
    name: *const c_char,
    value_json: *const c_char,
) -> Status {
    unsafe {
        edit_call(
            s,
            [(name, "name", false), (value_json, "value_json", false)],
            |s, [name, value]| s.set_setting(name.unwrap_or(""), value.unwrap_or("")),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_insert(
    s: *mut PbSource,
    path: *const c_char,
    value_json: *const c_char,
    before: *const c_char,
) -> Status {
    unsafe {
        edit_call(
            s,
            [
                (path, "path", false),
                (value_json, "value_json", false),
                (before, "before", true),
            ],
            |s, [path, value, before]| s.insert(path.unwrap_or(""), value.unwrap_or(""), before),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_append(
    s: *mut PbSource,
    path: *const c_char,
    value_json: *const c_char,
) -> Status {
    unsafe {
        edit_call(
            s,
            [(path, "path", false), (value_json, "value_json", false)],
            |s, [path, value]| s.append(path.unwrap_or(""), value.unwrap_or("")),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_remove(s: *mut PbSource, path: *const c_char) -> Status {
    unsafe {
        edit_call(s, [(path, "path", false)], |s, [path]| {
            s.remove(path.unwrap_or(""))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_source_move(
    s: *mut PbSource,
    from: *const c_char,
    to: *const c_char,
) -> Status {
    unsafe {
        edit_call(
            s,
            [(from, "from", false), (to, "to", false)],
            |s, [from, to]| s.move_to(from.unwrap_or(""), to.unwrap_or("")),
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_source_error(s: *const PbSource) -> *const c_char {
    if s.is_null() {
        return std::ptr::null();
    }
    // SAFETY: a non-null pb_source* came from pb_source_new.
    unsafe { &*s }
        .error
        .as_ref()
        .map_or(std::ptr::null(), |e| e.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// The night look.\nexport config let POST = {\n  exposure: 1.45,   // brighter than day\n  tint: #ff2e88,\n  effects: [{effect: \"grain\", amount: 0.3}, {effect: \"crt\"}],\n}\nlet DROPS = 450\nlet DIR = vec3(0.35, -1.0, 0.55)\nfn f() 1 end\n";

    #[test]
    fn bindings_json_describes_every_binding() {
        let json: serde_json::Value = serde_json::from_str(&bindings_json(SRC).unwrap()).unwrap();
        let post = &json[0];
        assert_eq!(post["name"], "POST");
        assert_eq!(post["config"], true);
        assert_eq!(post["line"], 2);
        assert_eq!(post["comment"], "The night look.");
        assert_eq!(post["value"]["rec"]["exposure"], 1.45);
        // A color literal reads as a color, not as the record it lowers to.
        assert_eq!(post["value"]["rec"]["tint"]["color"], "#ff2e88");
        assert_eq!(post["value"]["rec"]["effects"][0]["rec"]["effect"], "grain");
        assert_eq!(json[1]["value"]["int"], 450);
        assert_eq!(json[1]["config"], false);
        assert_eq!(json[2]["value"]["call"], "vec3");
        assert_eq!(json[2]["value"]["args"][1], -1.0);
        // A function is a binding with a reason instead of a value.
        assert_eq!(json[3]["name"], "f");
        assert!(json[3]["reason"].is_string());
    }

    #[test]
    fn floats_and_ints_stay_distinct_in_json() {
        let json = bindings_json("let a = 2\nlet b = 2.0\n").unwrap();
        assert!(json.contains("\"value\":{\"int\":2}"), "{json}");
        assert!(json.contains("\"value\":2.0"), "{json}");
    }

    #[test]
    fn a_tunable_config_reports_its_value_and_range() {
        let src = "config let A = tunable(4.0, 0, 10)\nconfig let B = tunable(2, step: 1)\n\
                   config let C = tunable(0.5)\nlet D = tunable(1.0, 0, 2)\n";
        let json: serde_json::Value = serde_json::from_str(&bindings_json(src).unwrap()).unwrap();
        assert_eq!(json[0]["value"], 4.0);
        assert_eq!(json[0]["text"], "4.0");
        assert_eq!(json[0]["range"], serde_json::json!({"min": 0.0, "max": 10.0}));
        assert_eq!(json[1]["value"]["int"], 2);
        assert_eq!(json[1]["range"], serde_json::json!({"step": 1.0}));
        // No bound written, no range. Without `config` it is the same
        // setting (docs/tasks/lifecycle-annotation-functions-refactor.md).
        assert_eq!(json[2]["value"], 0.5);
        assert!(json[2].get("range").is_none());
        assert_eq!(json[3]["config"], true);
        assert_eq!(json[3]["value"], 1.0);
        assert_eq!(json[3]["range"], serde_json::json!({"min": 0.0, "max": 2.0}));

        let mut s = PbSource::new(src);
        assert_eq!(s.set_value("A", "-1.5"), Status::Ok);
        assert_eq!(s.set("C", "0.75"), Status::Ok);
        assert!(s.text.starts_with("config let A = tunable(-1.5, 0, 10)\n"), "{}", s.text);
        assert!(s.text.contains("config let C = tunable(0.75)\n"), "{}", s.text);
    }

    #[test]
    fn an_annotated_setting_reports_what_the_call_form_does() {
        // docs/proposals/annotation-syntax.md: `@tunable(…) let X = v` is
        // `let X = tunable(v, …)`, and bounds may be named.
        let src = "// top speed\n@tunable(min: 3.0, max: 12.0) pub let RUN = 7.0\n\
                   @tunable(0, 10, step: 2) let N = 4\n\
                   @tunable(options: [\"title\", \"play\"])\nlet START = \"title\"\n\
                   let CALL = tunable(7.0, min: 3.0, max: 12.0)\n";
        let json: serde_json::Value = serde_json::from_str(&bindings_json(src).unwrap()).unwrap();
        let by = |n: &str| json.as_array().unwrap().iter().find(|b| b["name"] == n).unwrap().clone();
        let run = by("RUN");
        assert_eq!(run["value"], 7.0);
        assert_eq!(run["text"], "7.0");
        assert_eq!(run["config"], true);
        assert_eq!(run["exported"], true);
        assert_eq!(run["comment"], "top speed");
        assert_eq!(run["tunable"], serde_json::json!({"min": 3.0, "max": 12.0}));
        assert_eq!(run["tunable"], by("CALL")["tunable"]);
        assert_eq!(by("N")["tunable"], serde_json::json!({"min": 0.0, "max": 10.0, "step": 2.0}));
        assert_eq!(by("START")["tunable"], serde_json::json!({"options": ["title", "play"]}));

        let mut s = PbSource::new(src);
        assert_eq!(s.set_value("RUN", "8.5"), Status::Ok);
        assert_eq!(s.set_value("START", "\"play\""), Status::Ok);
        assert!(s.text.contains("@tunable(min: 3.0, max: 12.0) pub let RUN = 8.5\n"), "{}", s.text);
        assert!(s.text.contains("let START = \"play\"\n"), "{}", s.text);
    }

    #[test]
    fn a_setting_has_a_tunable_section_with_the_old_keys_mirrored() {
        let src = "let START = tunable(\"title\", options: [\"title\", \"select\", \"play\"])\n\
                   let SPEED = tunable(4.0, 0, 10)  // px per frame\n\
                   let DEBUG = tunable(false)\n\
                   pub let LOOK = tunable({bloom: 0.3, tint: #ff2e88})\n\
                   config let OLD = 3\n\
                   let PLAIN = 2\n\
                   var V = tunable(1.0)\n";
        let json: serde_json::Value = serde_json::from_str(&bindings_json(src).unwrap()).unwrap();
        let by = |n: &str| json.as_array().unwrap().iter().find(|b| b["name"] == n).unwrap().clone();
        let start = by("START");
        assert_eq!(start["value"], "title");
        assert_eq!(start["tunable"], serde_json::json!({"options": ["title", "select", "play"]}));
        assert_eq!(start["config"], true);
        assert!(start.get("range").is_none());
        let speed = by("SPEED");
        assert_eq!(speed["tunable"], serde_json::json!({"min": 0.0, "max": 10.0}));
        assert_eq!(speed["range"], speed["tunable"]);
        assert_eq!(speed["comment"], serde_json::Value::Null);
        assert_eq!(by("DEBUG")["value"], false);
        assert_eq!(by("DEBUG")["tunable"], serde_json::json!({}));
        let look = by("LOOK");
        assert_eq!(look["exported"], true);
        assert_eq!(look["value"]["rec"]["bloom"], 0.3);
        assert_eq!(look["tunable"], serde_json::json!({}));
        // `config let` is a setting with nothing written: an empty section.
        assert_eq!(by("OLD")["tunable"], serde_json::json!({}));
        for n in ["PLAIN", "V"] {
            assert!(by(n).get("tunable").is_none(), "{n}");
            assert_eq!(by(n)["config"], false, "{n}");
        }
    }

    #[test]
    fn set_setting_edits_only_the_value_of_a_setting() {
        let src = "let START = tunable(\"title\", options: [\"title\", \"play\"])  // first screen\n\
                   let SPEED = tunable(4.0, 0, 10)\n\
                   let PLAIN = 2\n";
        let mut s = PbSource::new(src);
        assert_eq!(s.set_setting("START", "\"play\""), Status::Ok);
        assert_eq!(s.set_setting("SPEED", "12.5"), Status::Ok, "not clamped");
        assert_eq!(
            s.text,
            "let START = tunable(\"play\", options: [\"title\", \"play\"])  // first screen\n\
             let SPEED = tunable(12.5, 0, 10)\n\
             let PLAIN = 2\n"
        );
        let before = s.text.clone();
        assert_eq!(s.set_setting("PLAIN", "{\"int\": 3}"), Status::InvalidArg);
        assert!(s.error.is_some());
        assert_eq!(s.set_setting("NOPE", "1.0"), Status::NotFound);
        // A path is not a name.
        assert_eq!(s.set_setting("SPEED.x", "1.0"), Status::NotFound);
        assert_eq!(s.text, before, "a failed call changes nothing");
    }

    #[test]
    fn set_changes_one_value_and_keeps_the_rest() {
        let mut s = PbSource::new(SRC);
        assert_eq!(s.set("POST.exposure", "1.2"), Status::Ok);
        assert_eq!(s.set("POST.effects[0].amount", "0.55"), Status::Ok);
        assert_eq!(s.set("POST.tint", "#29d9ff"), Status::Ok);
        assert_eq!(s.set("DIR[2]", "0.6"), Status::Ok);
        let expected = SRC
            .replace("1.45", "1.2")
            .replace("0.3}", "0.55}")
            .replace("#ff2e88", "#29d9ff")
            .replace("0.55)", "0.6)");
        assert_eq!(s.text, expected);
    }

    #[test]
    fn set_replaces_a_whole_binding() {
        let mut s = PbSource::new(SRC);
        assert_eq!(s.set("DROPS", "[1, 2]"), Status::Ok);
        assert!(s.text.contains("let DROPS = [1, 2]\n"));
        // A record literal is an expression too (not a block).
        assert_eq!(
            s.set("POST.effects[1]", "{effect: \"crt\", curve: 0.2}"),
            Status::Ok
        );
        assert!(
            s.text
                .contains("{effect: \"grain\", amount: 0.3}, {effect: \"crt\", curve: 0.2}],")
        );
        // A multi-line value keeps the file parsing.
        assert_eq!(
            s.set("POST.effects", "[\n    {effect: \"crt\"},\n  ]"),
            Status::Ok
        );
        assert!(bindings_json(&s.text).is_ok());
    }

    #[test]
    fn set_of_the_same_text_is_a_no_op() {
        let mut s = PbSource::new(SRC);
        assert_eq!(s.set("POST.exposure", "1.45"), Status::Ok);
        assert_eq!(s.text, SRC);
    }

    #[test]
    fn set_refuses_what_is_missing_or_malformed() {
        let mut s = PbSource::new(SRC);
        assert_eq!(s.set("POST.bloom", "1.0"), Status::NotFound);
        assert_eq!(s.set("NOPE", "1"), Status::NotFound);
        assert_eq!(s.set("POST..x", "1"), Status::InvalidArg);
        assert_eq!(s.set("POST.exposure", "1 +"), Status::InvalidArg);
        assert_eq!(s.set("POST.exposure", "1\nlet x = 2"), Status::InvalidArg);
        assert_eq!(s.text, SRC, "a refused edit changes nothing");
        assert!(s.error.is_some());
    }

    #[test]
    fn value_edits_change_only_what_differs() {
        let mut s = PbSource::new(SRC);
        assert_eq!(s.set_value("POST.exposure", "1.2"), Status::Ok);
        assert_eq!(
            s.set_value("POST.tint", r##"{"color":"#29d9ff"}"##),
            Status::Ok
        );
        assert_eq!(s.set_value("DROPS", r#"{"int":500}"#), Status::Ok);
        // A whole list written back with one element changed and one added:
        // the unchanged element is not touched, the new one copies its style.
        assert_eq!(
            s.set_value(
                "POST.effects",
                r#"[{"rec":{"effect":"grain","amount":0.3}},{"rec":{"effect":"crt","curve":0.2}},{"rec":{"effect":"halftone","amount":0.7}}]"#
            ),
            Status::Ok
        );
        assert_eq!(s.remove("POST.effects[0]"), Status::Ok);
        assert_eq!(s.append("DIR", "1.0"), Status::Ok);
        assert_eq!(s.insert("POST.bloom", "0.4", Some("tint")), Status::Ok);
        assert_eq!(
            s.insert("POST.effects[0]", r#"{"rec":{"effect":"a"}}"#, None),
            Status::Ok
        );
        let expected = SRC
            .replace("1.45", "1.2")
            .replace("  tint: #ff2e88,", "  bloom: 0.4,\n  tint: #29d9ff,")
            .replace("450", "500")
            .replace(
                "[{effect: \"grain\", amount: 0.3}, {effect: \"crt\"}]",
                "[{effect: \"a\"}, {effect: \"crt\", curve: 0.2}, {effect: \"halftone\", amount: 0.7}]",
            )
            .replace("0.55)", "0.55, 1.0)");
        assert_eq!(s.text, expected);
        // Writing what is already there changes nothing.
        let before = s.text.clone();
        assert_eq!(s.set_value("POST.exposure", "1.2"), Status::Ok);
        assert_eq!(s.remove("POST.absent"), Status::Ok);
        assert_eq!(s.text, before);
    }

    #[test]
    fn value_edits_refuse_what_cannot_be_done() {
        let mut s = PbSource::new(SRC);
        assert_eq!(
            s.set_value("POST.effects[5].amount", "1.0"),
            Status::NotFound
        );
        assert_eq!(s.set_value("NOPE", "1.0"), Status::NotFound);
        assert_eq!(
            s.set_value("POST.exposure", "{\"oops\": 1}"),
            Status::InvalidArg
        );
        assert_eq!(s.set_value("POST.exposure", "1 +"), Status::InvalidArg);
        assert_eq!(s.insert("POST.effects[9]", "1.0", None), Status::InvalidArg);
        assert_eq!(s.append("POST.exposure", "1.0"), Status::InvalidArg);
        assert_eq!(s.remove("POST.effects[2]"), Status::NotFound);
        assert_eq!(s.remove("POST"), Status::InvalidArg);
        assert_eq!(s.text, SRC, "a refused edit changes nothing");
        assert!(s.error.is_some());
        let mut broken = PbSource::new("let x = (\n");
        assert_eq!(broken.set_value("x", "1.0"), Status::Compile);
    }
}
