//! Native fns registered into the Petal env: `editor`, `process`, `row`,
//! `column`, `panel`, `layout`, `color_theme`, `color_scheme`.
//!
//! `editor`/`process`/`row`/`column`/`panel` build ordinary Petal records
//! (`Value::Map`) so the script can pass them around like any other value.
//!
//! `layout`/`color_theme`/`color_scheme` are *observable* calls: rather than
//! stashing anything in a host-side global, each pushes its (raw) argument
//! value into a symbol-keyed **output buffer** on the [`Env`]. After the run
//! the host drains those buffers with `Env::take_output_buffer` and interprets
//! the values (see [`crate::ScriptHost::run_and_extract`]). This is Petal's
//! canonical mechanism for observing what a script called — no thread-locals,
//! and it works identically for forks. See `../docs/embedding-guide.md`.

use indexmap::IndexMap;
use petal::env::Env;
use petal::native_fn::{NativeClass, NativeResult, PetalCxt};
use petal::value::Value;

/// Output-buffer symbol names shared by the native fns (which `push_output`
/// into them) and the host (which drains them by the same name). Interning the
/// same name on both sides yields the same `SymbolId`.
pub(crate) const LAYOUT_SYM: &str = "garden.layout";
pub(crate) const THEME_SYM: &str = "garden.color_theme";
pub(crate) const SCHEME_SYM: &str = "garden.color_scheme";

/// Register all Garden native fns. Must run before `env.load_program`.
pub(crate) fn register_all(env: &mut Env) {
    env.register_native("editor", native_editor);
    env.register_native("process", native_process);
    env.register_native("panel", native_panel);
    env.register_native("row", native_row);
    env.register_native("column", native_column);
    // The three observable calls only emit into an output buffer, so classify
    // them `Effectful`: a `Pending` argument makes the call a no-op (emitting
    // nothing) instead of being absorbed as its result.
    let layout = env.register_native("layout", native_layout);
    let theme = env.register_native("color_theme", native_color_theme);
    let scheme = env.register_native("color_scheme", native_color_scheme);
    env.set_native_class(layout, NativeClass::Effectful);
    env.set_native_class(theme, NativeClass::Effectful);
    env.set_native_class(scheme, NativeClass::Effectful);
}

/// `editor()` / `editor(path)` / `editor(path, { line_numbers: true, wrap: false })`
/// → `{ kind: "editor", file: path|nil, line_numbers: bool, wrap: bool }`.
///
/// The optional second argument is a config record; recognized keys are
/// `line_numbers` (a bool, default `false`) and `wrap` (a bool, default `true`).
fn native_editor(cxt: &mut PetalCxt) -> NativeResult {
    let file = if cxt.arg_count() == 0 {
        Value::Nil
    } else {
        match cxt.get_value(1)? {
            v @ (Value::String(_) | Value::Nil) => v,
            other => {
                return Err(format!(
                    "editor() expects a string path, got {}",
                    other.type_name()
                ))
            }
        }
    };

    // Optional config record (arg 2). A missing or nil config means defaults.
    let config = (cxt.arg_count() >= 2)
        .then(|| cxt.get_value(2))
        .transpose()?;
    let (line_numbers, wrap) = match config {
        None | Some(Value::Nil) => (false, true),
        Some(Value::Map(id)) => {
            let map = cxt.heap().get_map(id);
            let bool_field = |key: &str, default: bool| -> Result<bool, String> {
                match map.get(key) {
                    None | Some(Value::Nil) => Ok(default),
                    Some(Value::Bool(b)) => Ok(*b),
                    Some(other) => Err(format!(
                        "editor() '{key}' must be a bool, got {}",
                        other.type_name()
                    )),
                }
            };
            (
                bool_field("line_numbers", false)?,
                bool_field("wrap", true)?,
            )
        }
        Some(other) => {
            return Err(format!(
                "editor() config must be a record, got {}",
                other.type_name()
            ))
        }
    };

    let mut fields = IndexMap::new();
    fields.insert("kind".to_string(), alloc_str(cxt, "editor"));
    fields.insert("file".to_string(), file);
    fields.insert("line_numbers".to_string(), Value::Bool(line_numbers));
    fields.insert("wrap".to_string(), Value::Bool(wrap));
    let id = cxt.heap_mut().alloc_map(fields);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// `process(command)` / `process(command, args)` →
/// `{ kind: "process", command: string, args: list|nil }`
///
/// `command` (arg 0) is a required string; `args` (arg 1) is an optional list
/// (its string-ness is enforced at conversion time, mirroring how `children`
/// element types are checked).
fn native_process(cxt: &mut PetalCxt) -> NativeResult {
    let command = match cxt.get_value(1)? {
        v @ Value::String(_) => v,
        other => {
            return Err(format!(
                "process() expects a command string, got {}",
                other.type_name()
            ))
        }
    };

    let args = if cxt.arg_count() >= 2 {
        match cxt.get_value(2)? {
            v @ (Value::List(_) | Value::Nil) => v,
            other => {
                return Err(format!(
                    "process() args must be a list, got {}",
                    other.type_name()
                ))
            }
        }
    } else {
        Value::Nil
    };

    let mut fields = IndexMap::new();
    fields.insert("kind".to_string(), alloc_str(cxt, "process"));
    fields.insert("command".to_string(), command);
    fields.insert("args".to_string(), args);
    let id = cxt.heap_mut().alloc_map(fields);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// `panel(script)` / `panel(script, { screens: ["a.ptl", "b.ptl"] })` →
/// `{ kind: "panel", script: string, screens: list|nil }`.
///
/// `script` (arg 0) is a required string path to the Petal sketch that draws the
/// pane each frame. The optional config record (arg 1) — same convention as
/// `editor(path, { line_numbers: true })` — recognizes one key, `screens`: an
/// explicit navigation allowlist that *narrows* the default script-directory
/// allowlist for the `navigate(...)` history API (see
/// [`LayoutNode::Panel`](crate::LayoutNode)). Its entries' string-ness is
/// enforced at conversion time, mirroring how `process` args are checked.
fn native_panel(cxt: &mut PetalCxt) -> NativeResult {
    let script = match cxt.get_value(1)? {
        v @ Value::String(_) => v,
        other => {
            return Err(format!(
                "panel() expects a script path string, got {}",
                other.type_name()
            ))
        }
    };

    // Optional config record (arg 2). A missing or nil config means no explicit
    // screens allowlist (the implicit script-directory default applies).
    let screens = match (cxt.arg_count() >= 2)
        .then(|| cxt.get_value(2))
        .transpose()?
    {
        None | Some(Value::Nil) => Value::Nil,
        Some(Value::Map(id)) => match cxt.heap().get_map(id).get("screens") {
            None | Some(Value::Nil) => Value::Nil,
            Some(v @ Value::List(_)) => *v,
            Some(other) => {
                return Err(format!(
                    "panel() 'screens' must be a list, got {}",
                    other.type_name()
                ))
            }
        },
        Some(other) => {
            return Err(format!(
                "panel() config must be a record, got {}",
                other.type_name()
            ))
        }
    };

    let mut fields = IndexMap::new();
    fields.insert("kind".to_string(), alloc_str(cxt, "panel"));
    fields.insert("script".to_string(), script);
    fields.insert("screens".to_string(), screens);
    let id = cxt.heap_mut().alloc_map(fields);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// `row(children)` / `row(children, ratios)` → `{ kind: "row", ... }`
fn native_row(cxt: &mut PetalCxt) -> NativeResult {
    build_container(cxt, "row")
}

/// `column(children)` / `column(children, ratios)` → `{ kind: "column", ... }`
fn native_column(cxt: &mut PetalCxt) -> NativeResult {
    build_container(cxt, "column")
}

/// Shared body of `row`/`column`: validates the children list and the
/// (optional) ratios list, then builds the record. A ratios list whose length
/// doesn't match the children is dropped with a warning rather than erroring.
fn build_container(cxt: &mut PetalCxt, kind: &str) -> NativeResult {
    let children = cxt.get_value(1)?;
    let child_count = match children {
        Value::List(id) => cxt.heap().list_len(id),
        other => {
            return Err(format!(
                "{kind}() expects a list of children, got {}",
                other.type_name()
            ))
        }
    };
    if child_count == 0 {
        return Err(format!("{kind}() needs at least one child"));
    }

    let ratios = if cxt.arg_count() >= 2 {
        match cxt.get_value(2)? {
            Value::Nil => Value::Nil,
            Value::List(id) => {
                let ratio_count = cxt.heap().list_len(id);
                if ratio_count == child_count {
                    Value::List(id)
                } else {
                    cxt.print(format!(
                        "[garden-script] warning: {kind}() got {ratio_count} ratios \
                         for {child_count} children; ignoring ratios"
                    ));
                    Value::Nil
                }
            }
            other => {
                return Err(format!(
                    "{kind}() ratios must be a list, got {}",
                    other.type_name()
                ))
            }
        }
    } else {
        Value::Nil
    };

    let mut fields = IndexMap::new();
    fields.insert("kind".to_string(), alloc_str(cxt, kind));
    fields.insert("children".to_string(), children);
    fields.insert("ratios".to_string(), ratios);
    let id = cxt.heap_mut().alloc_map(fields);
    cxt.push_value(Value::Map(id));
    Ok(1)
}

/// `layout(node)` → emits the record tree into the `garden.layout` output
/// buffer. The host drains it after the run and converts the last value to a
/// [`LayoutNode`](crate::LayoutNode) via [`crate::convert::convert_layout`];
/// structural problems (unknown kind, malformed children, ...) surface there.
/// This native validates nothing itself — it only records the call — so a
/// script that calls `layout(...)` many times leaves every value in the buffer
/// (the host takes the last, matching "last call wins").
fn native_layout(cxt: &mut PetalCxt) -> NativeResult {
    let value = cxt.get_value(1)?;
    let sym = cxt.intern_symbol(LAYOUT_SYM);
    cxt.push_output(sym, value);
    cxt.push_nil();
    Ok(1)
}

/// `color_theme(record)` → emits the raw record of `field: "#hexcolor"` pairs
/// into the `garden.color_theme` output buffer. The host parses the hex colors
/// into a [`Theme`](crate::Theme) after the run (see
/// [`crate::convert::convert_theme`]); a malformed color degrades to a warning
/// there rather than aborting the run.
fn native_color_theme(cxt: &mut PetalCxt) -> NativeResult {
    let value = cxt.get_value(1)?;
    let sym = cxt.intern_symbol(THEME_SYM);
    cxt.push_output(sym, value);
    cxt.push_nil();
    Ok(1)
}

/// `color_scheme(name)` → emits the base scheme name (e.g. `"dark"`, `"light"`)
/// into the `garden.color_scheme` output buffer. Unlike `color_theme` (which
/// overlays individual color keys), this selects a whole palette; the host
/// reads the last emitted string and maps it onto its `ThemeScheme` (an unknown
/// name is ignored there, keeping the built-in default). This is the call the
/// settings UI persists via goal-based editing so a menu choice survives a
/// restart.
fn native_color_scheme(cxt: &mut PetalCxt) -> NativeResult {
    let value = cxt.get_value(1)?;
    let sym = cxt.intern_symbol(SCHEME_SYM);
    cxt.push_output(sym, value);
    cxt.push_nil();
    Ok(1)
}

fn alloc_str(cxt: &mut PetalCxt, s: &str) -> Value {
    Value::String(cxt.heap_mut().alloc_string(s.to_string()))
}
