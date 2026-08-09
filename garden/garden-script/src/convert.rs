//! Conversion of a Petal record tree (built by `editor`/`row`/`column`, or
//! hand-written record literals with the same shape) into a [`LayoutNode`] —
//! plus the inverse, [`layout_to_static_value`], which expresses a [`LayoutNode`] as a
//! goal-based-editing call tree for persisting layout changes back to source.
//!
//! Runs eagerly inside `layout`, while the values are guaranteed live on
//! the heap. Structural problems (unknown `kind`, non-list children, empty
//! children) are hard errors; a malformed `ratios` list degrades to `None`
//! with a warning pushed onto `warnings`.

use petal::goal_based_editing::StaticValue;
use petal::heap::Heap;
use petal::value::Value;

use crate::{LayoutNode, Theme};

/// Convert a layout record `value` into a [`LayoutNode`], recursing through
/// `children`. Recoverable issues are appended to `warnings`.
pub(crate) fn convert_layout(
    value: Value,
    heap: &Heap,
    warnings: &mut Vec<String>,
) -> Result<LayoutNode, String> {
    let map_id = match value {
        Value::Map(id) => id,
        other => {
            return Err(format!(
                "expected a layout record (from editor()/row()/column()), got {}",
                other.type_name()
            ))
        }
    };
    let fields = heap.get_map(map_id);

    let kind = match fields.get("kind") {
        Some(Value::String(id)) => heap.get_string(*id),
        Some(other) => {
            return Err(format!(
                "layout record has a non-string 'kind' field ({})",
                other.type_name()
            ))
        }
        None => return Err("layout record is missing a 'kind' field".to_string()),
    };

    match kind {
        "editor" => convert_editor(
            fields.get("file").copied(),
            fields.get("line_numbers").copied(),
            fields.get("wrap").copied(),
            heap,
        ),
        "process" => convert_process(
            fields.get("command").copied(),
            fields.get("args").copied(),
            heap,
        ),
        "panel" => convert_panel(
            fields.get("script").copied(),
            fields.get("screens").copied(),
            heap,
        ),
        "row" | "column" => {
            let is_row = kind == "row";
            let children = convert_children(kind, fields.get("children").copied(), heap, warnings)?;
            let ratios = convert_ratios(
                kind,
                fields.get("ratios").copied(),
                children.len(),
                heap,
                warnings,
            );
            Ok(if is_row {
                LayoutNode::Row { children, ratios }
            } else {
                LayoutNode::Column { children, ratios }
            })
        }
        other => Err(format!("unknown layout kind '{other}'")),
    }
}

/// Convert a `color_theme` record value (a `Value::Map` of `field: "#hex"`
/// pairs) into a [`Theme`]. A non-record value is a hard error (the script
/// passed the wrong type); an individual entry that is not a valid hex color
/// string degrades to a warning pushed onto `warnings` and is skipped, so the
/// key keeps the application's built-in default — mirroring `convert_ratios`'s
/// degrade-with-warning handling.
///
/// Runs host-side after the run drains the `color_theme` output buffer, while
/// the values are still live on the heap.
pub(crate) fn convert_theme(
    value: Value,
    heap: &Heap,
    warnings: &mut Vec<String>,
) -> Result<Theme, String> {
    let map_id = match value {
        Value::Map(id) => id,
        other => {
            return Err(format!(
                "color_theme expects a record of color values, got {}",
                other.type_name()
            ))
        }
    };

    let mut theme = Theme::default();
    for (key, val) in heap.get_map(map_id).iter() {
        let raw = match val {
            Value::String(id) => heap.get_string(*id),
            other => {
                warnings.push(format!(
                    "color_theme '{key}' must be a hex color string, got {}; ignoring",
                    other.type_name()
                ));
                continue;
            }
        };
        match parse_hex_color(raw) {
            Ok(rgba) => theme.insert(key.clone(), rgba),
            Err(e) => warnings.push(format!("color_theme '{key}': {e}; ignoring")),
        }
    }
    Ok(theme)
}

/// Parse a Petal hex color string (`"#rgb"`, `"#rrggbb"`, or `"#rrggbbaa"`)
/// into normalized rgba (`0.0..=1.0`). Alpha defaults to opaque (`1.0`) for the
/// 3- and 6-digit forms. Returns a descriptive error on a malformed string.
pub(crate) fn parse_hex_color(s: &str) -> Result<[f32; 4], String> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| format!("color '{s}' must start with '#'"))?;

    // (component count, nibbles per component). 3/4-digit forms repeat each
    // nibble (#abc → #aabbcc), matching CSS shorthand.
    let (channels, has_alpha, short) = match hex.len() {
        3 => (3, false, true),
        6 => (3, false, false),
        8 => (4, true, false),
        _ => {
            return Err(format!(
                "color '{s}' must have 3, 6, or 8 hex digits after '#', got {}",
                hex.len()
            ))
        }
    };

    let mut rgba = [0.0f32, 0.0, 0.0, 1.0];
    for (i, channel) in rgba.iter_mut().enumerate().take(channels) {
        let byte = if short {
            let n = nibble(hex, i, s)?;
            n * 16 + n
        } else {
            let hi = nibble(hex, i * 2, s)?;
            let lo = nibble(hex, i * 2 + 1, s)?;
            hi * 16 + lo
        };
        *channel = byte as f32 / 255.0;
    }
    debug_assert!(has_alpha || rgba[3] == 1.0);
    Ok(rgba)
}

/// Parse a single hex nibble at byte offset `i` of `hex`.
fn nibble(hex: &str, i: usize, orig: &str) -> Result<u8, String> {
    let c = hex.as_bytes()[i] as char;
    c.to_digit(16)
        .map(|d| d as u8)
        .ok_or_else(|| format!("color '{orig}' has a non-hex digit '{c}'"))
}

fn convert_editor(
    file: Option<Value>,
    line_numbers: Option<Value>,
    wrap: Option<Value>,
    heap: &Heap,
) -> Result<LayoutNode, String> {
    let file = match file {
        None | Some(Value::Nil) => None,
        Some(Value::String(id)) => Some(heap.get_string(id).to_string()),
        Some(other) => {
            return Err(format!(
                "editor 'file' field must be a string or nil, got {}",
                other.type_name()
            ))
        }
    };
    let bool_field = |value: Option<Value>, key: &str, default: bool| match value {
        None | Some(Value::Nil) => Ok(default),
        Some(Value::Bool(b)) => Ok(b),
        Some(other) => Err(format!(
            "editor '{key}' field must be a bool, got {}",
            other.type_name()
        )),
    };
    let line_numbers = bool_field(line_numbers, "line_numbers", false)?;
    let wrap = bool_field(wrap, "wrap", true)?;
    Ok(LayoutNode::Editor {
        file,
        line_numbers,
        wrap,
    })
}

/// Convert a `process` record. `command` is a required string; `args` is an
/// optional list of strings (missing or nil → empty). A missing/non-string
/// `command`, a non-list `args`, or any non-string `args` entry is a hard
/// error — the same strictness as `children`, since a subprocess invocation
/// silently dropping its executable or an argument would be a confusing
/// failure rather than a recoverable layout glitch like `ratios`.
fn convert_process(
    command: Option<Value>,
    args: Option<Value>,
    heap: &Heap,
) -> Result<LayoutNode, String> {
    let command = match command {
        Some(Value::String(id)) => heap.get_string(id).to_string(),
        Some(other) => {
            return Err(format!(
                "process 'command' field must be a string, got {}",
                other.type_name()
            ))
        }
        None => return Err("process record is missing a 'command' field".to_string()),
    };

    let args = match args {
        None | Some(Value::Nil) => Vec::new(),
        Some(Value::List(id)) => {
            let elements = heap.get_list(id).to_vec();
            let mut parsed = Vec::with_capacity(elements.len());
            for element in elements {
                match element {
                    Value::String(sid) => parsed.push(heap.get_string(sid).to_string()),
                    other => {
                        return Err(format!(
                            "process 'args' entries must be strings, got {}",
                            other.type_name()
                        ))
                    }
                }
            }
            parsed
        }
        Some(other) => {
            return Err(format!(
                "process 'args' field must be a list or nil, got {}",
                other.type_name()
            ))
        }
    };

    Ok(LayoutNode::Process { command, args })
}

/// Convert a `panel` record. `script` is a required string path. A
/// missing/non-string `script` is a hard error — like `process`'s `command`, a
/// panel silently dropping its script would be a confusing failure rather than a
/// recoverable layout glitch.
///
/// `screens` is the optional explicit navigation allowlist: a list of `.ptl`
/// screen names that *narrows* the implicit script-directory default when
/// present (see [`LayoutNode::Panel`]). Missing or nil → an empty vec (not
/// declared). Like `process`'s `args`, a non-list `screens` or any non-string
/// entry is a hard error rather than a silently-dropped glitch — a misdeclared
/// allowlist that silently reverted to "allow the whole directory" would defeat
/// the narrowing it was written to enforce.
fn convert_panel(
    script: Option<Value>,
    screens: Option<Value>,
    heap: &Heap,
) -> Result<LayoutNode, String> {
    let script = match script {
        Some(Value::String(id)) => heap.get_string(id).to_string(),
        Some(other) => {
            return Err(format!(
                "panel 'script' field must be a string, got {}",
                other.type_name()
            ))
        }
        None => return Err("panel record is missing a 'script' field".to_string()),
    };
    let screens = match screens {
        None | Some(Value::Nil) => Vec::new(),
        Some(Value::List(id)) => {
            let elements = heap.get_list(id).to_vec();
            let mut parsed = Vec::with_capacity(elements.len());
            for element in elements {
                match element {
                    Value::String(sid) => parsed.push(heap.get_string(sid).to_string()),
                    other => {
                        return Err(format!(
                            "panel 'screens' entries must be strings, got {}",
                            other.type_name()
                        ))
                    }
                }
            }
            parsed
        }
        Some(other) => {
            return Err(format!(
                "panel 'screens' field must be a list or nil, got {}",
                other.type_name()
            ))
        }
    };
    Ok(LayoutNode::Panel { script, screens })
}

fn convert_children(
    kind: &str,
    children: Option<Value>,
    heap: &Heap,
    warnings: &mut Vec<String>,
) -> Result<Vec<LayoutNode>, String> {
    let list_id = match children {
        Some(Value::List(id)) => id,
        Some(other) => {
            return Err(format!(
                "{kind} 'children' must be a list, got {}",
                other.type_name()
            ))
        }
        None => return Err(format!("{kind} record is missing a 'children' field")),
    };
    let elements = heap.get_list(list_id).to_vec();
    if elements.is_empty() {
        return Err(format!("{kind} has no children"));
    }
    elements
        .into_iter()
        .map(|child| convert_layout(child, heap, warnings))
        .collect()
}

/// The inverse of [`convert_layout`]: express `node` as a structured
/// [`StaticValue`] call tree (`editor(...)`, `process(...)`, `row(...)`, ...) for
/// goal-based source editing. Persisting a layout is then one goal — "there is
/// a top-level call `layout(<this tree>)`" — and the goal engine handles
/// rendering, escaping, and splicing it over the existing call.
pub(crate) fn layout_to_static_value(node: &LayoutNode) -> StaticValue {
    match node {
        LayoutNode::Editor {
            file,
            line_numbers,
            wrap,
        } => {
            // Only non-default config keys are emitted (line_numbers defaults
            // off, wrap defaults on) so the common pane stays a bare `editor()`.
            let mut config = Vec::new();
            if *line_numbers {
                config.push(("line_numbers", StaticValue::bool(true)));
            }
            if !*wrap {
                config.push(("wrap", StaticValue::bool(false)));
            }
            let mut args = Vec::new();
            // A config record needs a positional file argument before it, so a
            // file-less pane with config gets `editor(nil, { ... })`.
            match (file, config.is_empty()) {
                (Some(file), _) => args.push(StaticValue::str(file.clone())),
                (None, false) => args.push(StaticValue::nil()),
                (None, true) => {}
            }
            if !config.is_empty() {
                args.push(StaticValue::record(config));
            }
            StaticValue::call("editor", args)
        }
        LayoutNode::Process { command, args } => {
            let mut call_args = vec![StaticValue::str(command.clone())];
            if !args.is_empty() {
                call_args.push(StaticValue::list(
                    args.iter().map(|a| StaticValue::str(a.clone())),
                ));
            }
            StaticValue::call("process", call_args)
        }
        LayoutNode::Panel { script, screens } => {
            let mut args = vec![StaticValue::str(script.clone())];
            // An explicit allowlist round-trips as a config record
            // `panel(script, { screens: [...] })` (the `editor(path, { ... })`
            // convention); an empty vec (not declared) emits just `panel(script)`.
            if !screens.is_empty() {
                args.push(StaticValue::record(vec![(
                    "screens",
                    StaticValue::list(screens.iter().map(|s| StaticValue::str(s.clone()))),
                )]));
            }
            StaticValue::call("panel", args)
        }
        LayoutNode::Row { children, ratios } => container_to_static_value("row", children, ratios),
        LayoutNode::Column { children, ratios } => {
            container_to_static_value("column", children, ratios)
        }
    }
}

/// A `row`/`column` call: the children list, then the optional ratios list.
fn container_to_static_value(
    kind: &str,
    children: &[LayoutNode],
    ratios: &Option<Vec<f32>>,
) -> StaticValue {
    let mut args = vec![StaticValue::list(
        children.iter().map(layout_to_static_value),
    )];
    if let Some(ratios) = ratios {
        args.push(StaticValue::list(ratios.iter().copied()));
    }
    StaticValue::call(kind, args)
}

/// Parse the optional `ratios` field. Any problem (wrong type, length
/// mismatch, non-numeric entry) degrades to `None` with a warning, so a
/// sloppy ratios list never takes the layout down.
fn convert_ratios(
    kind: &str,
    ratios: Option<Value>,
    child_count: usize,
    heap: &Heap,
    warnings: &mut Vec<String>,
) -> Option<Vec<f32>> {
    let list_id = match ratios {
        None | Some(Value::Nil) => return None,
        Some(Value::List(id)) => id,
        Some(other) => {
            warnings.push(format!(
                "{kind} 'ratios' must be a list, got {}; ignoring ratios",
                other.type_name()
            ));
            return None;
        }
    };
    let elements = heap.get_list(list_id);
    if elements.len() != child_count {
        warnings.push(format!(
            "{kind} got {} ratios for {child_count} children; ignoring ratios",
            elements.len()
        ));
        return None;
    }
    let mut parsed = Vec::with_capacity(elements.len());
    for element in elements {
        match element.as_f64() {
            Some(f) => parsed.push(f as f32),
            None => {
                warnings.push(format!(
                    "{kind} ratio entries must be numbers, got {}; ignoring ratios",
                    element.type_name()
                ));
                return None;
            }
        }
    }
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::{convert_layout, parse_hex_color};
    use crate::LayoutNode;
    use indexmap::IndexMap;
    use petal::heap::Heap;
    use petal::value::Value;

    fn approx(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    /// Allocate a string value on `heap`.
    fn str_val(heap: &mut Heap, s: &str) -> Value {
        Value::String(heap.alloc_string(s.to_string()))
    }

    /// Build a `process` record `{ kind: "process", command, args }` on `heap`,
    /// omitting any field whose value is `None`.
    fn process_record(heap: &mut Heap, command: Option<Value>, args: Option<Value>) -> Value {
        let kind = str_val(heap, "process");
        let mut fields = IndexMap::new();
        fields.insert("kind".to_string(), kind);
        if let Some(command) = command {
            fields.insert("command".to_string(), command);
        }
        if let Some(args) = args {
            fields.insert("args".to_string(), args);
        }
        Value::Map(heap.alloc_map(fields))
    }

    #[test]
    fn converts_process_with_args() {
        let mut heap = Heap::new();
        let command = str_val(&mut heap, "ls");
        let arg0 = str_val(&mut heap, "-l");
        let arg1 = str_val(&mut heap, "/tmp");
        let args = Value::List(heap.alloc_list(vec![arg0, arg1]));
        let record = process_record(&mut heap, Some(command), Some(args));

        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Process {
                command: "ls".to_string(),
                args: vec!["-l".to_string(), "/tmp".to_string()],
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn converts_process_without_args() {
        let mut heap = Heap::new();
        let command = str_val(&mut heap, "garden-dir");
        let record = process_record(&mut heap, Some(command), None);

        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Process {
                command: "garden-dir".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn process_nil_args_is_empty() {
        let mut heap = Heap::new();
        let command = str_val(&mut heap, "garden-dir");
        let record = process_record(&mut heap, Some(command), Some(Value::Nil));

        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Process {
                command: "garden-dir".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn process_missing_command_is_error() {
        let mut heap = Heap::new();
        let record = process_record(&mut heap, None, None);
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(err.contains("missing a 'command' field"), "got: {err}");
    }

    #[test]
    fn process_non_string_command_is_error() {
        let mut heap = Heap::new();
        let record = process_record(&mut heap, Some(Value::Int(7)), None);
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(
            err.contains("'command' field must be a string"),
            "got: {err}"
        );
    }

    #[test]
    fn process_non_string_arg_is_error() {
        let mut heap = Heap::new();
        let command = str_val(&mut heap, "ls");
        let bad = Value::List(heap.alloc_list(vec![Value::Int(1)]));
        let record = process_record(&mut heap, Some(command), Some(bad));
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(err.contains("'args' entries must be strings"), "got: {err}");
    }

    #[test]
    fn process_non_list_args_is_error() {
        let mut heap = Heap::new();
        let command = str_val(&mut heap, "ls");
        let bad = str_val(&mut heap, "not-a-list");
        let record = process_record(&mut heap, Some(command), Some(bad));
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(
            err.contains("'args' field must be a list or nil"),
            "got: {err}"
        );
    }

    #[test]
    fn converts_panel() {
        let mut heap = Heap::new();
        let kind = str_val(&mut heap, "panel");
        let script = str_val(&mut heap, "sketch.ptl");
        let mut fields = IndexMap::new();
        fields.insert("kind".to_string(), kind);
        fields.insert("script".to_string(), script);
        let record = Value::Map(heap.alloc_map(fields));

        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Panel {
                script: "sketch.ptl".to_string(),
                screens: Vec::new(),
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn panel_missing_script_is_error() {
        let mut heap = Heap::new();
        let kind = str_val(&mut heap, "panel");
        let mut fields = IndexMap::new();
        fields.insert("kind".to_string(), kind);
        let record = Value::Map(heap.alloc_map(fields));
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(err.contains("missing a 'script' field"), "got: {err}");
    }

    /// Build a `panel` record `{ kind: "panel", script, screens }`, omitting
    /// `screens` when `None`.
    fn panel_record(heap: &mut Heap, script: &str, screens: Option<Value>) -> Value {
        let kind = str_val(heap, "panel");
        let script = str_val(heap, script);
        let mut fields = IndexMap::new();
        fields.insert("kind".to_string(), kind);
        fields.insert("script".to_string(), script);
        if let Some(screens) = screens {
            fields.insert("screens".to_string(), screens);
        }
        Value::Map(heap.alloc_map(fields))
    }

    #[test]
    fn converts_panel_with_screens_allowlist() {
        let mut heap = Heap::new();
        let a = str_val(&mut heap, "a.ptl");
        let b = str_val(&mut heap, "b.ptl");
        let screens = Value::List(heap.alloc_list(vec![a, b]));
        let record = panel_record(&mut heap, "sketch.ptl", Some(screens));

        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Panel {
                script: "sketch.ptl".to_string(),
                screens: vec!["a.ptl".to_string(), "b.ptl".to_string()],
            }
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn panel_nil_screens_is_empty() {
        let mut heap = Heap::new();
        let record = panel_record(&mut heap, "sketch.ptl", Some(Value::Nil));
        let mut warnings = Vec::new();
        let node = convert_layout(record, &heap, &mut warnings).unwrap();
        assert_eq!(
            node,
            LayoutNode::Panel {
                script: "sketch.ptl".to_string(),
                screens: Vec::new(),
            }
        );
    }

    #[test]
    fn panel_non_string_screen_is_error() {
        let mut heap = Heap::new();
        let bad = Value::List(heap.alloc_list(vec![Value::Int(3)]));
        let record = panel_record(&mut heap, "sketch.ptl", Some(bad));
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(
            err.contains("'screens' entries must be strings"),
            "got: {err}"
        );
    }

    #[test]
    fn panel_non_list_screens_is_error() {
        let mut heap = Heap::new();
        let bad = str_val(&mut heap, "not-a-list");
        let record = panel_record(&mut heap, "sketch.ptl", Some(bad));
        let mut warnings = Vec::new();
        let err = convert_layout(record, &heap, &mut warnings).unwrap_err();
        assert!(
            err.contains("'screens' field must be a list or nil"),
            "got: {err}"
        );
    }

    #[test]
    fn parses_six_digit() {
        let c = parse_hex_color("#102030").unwrap();
        assert!(approx(
            c,
            [
                0x10 as f32 / 255.0,
                0x20 as f32 / 255.0,
                0x30 as f32 / 255.0,
                1.0
            ]
        ));
    }

    #[test]
    fn parses_three_digit_shorthand() {
        // #abc expands to #aabbcc.
        let c = parse_hex_color("#abc").unwrap();
        assert!(approx(
            c,
            [
                0xaa as f32 / 255.0,
                0xbb as f32 / 255.0,
                0xcc as f32 / 255.0,
                1.0
            ]
        ));
    }

    #[test]
    fn parses_eight_digit_alpha() {
        let c = parse_hex_color("#01020304").unwrap();
        assert!(approx(
            c,
            [
                0x01 as f32 / 255.0,
                0x02 as f32 / 255.0,
                0x03 as f32 / 255.0,
                0x04 as f32 / 255.0,
            ]
        ));
    }

    #[test]
    fn defaults_alpha_opaque() {
        assert_eq!(parse_hex_color("#ffffff").unwrap()[3], 1.0);
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_hex_color("102030").is_err()); // no '#'
        assert!(parse_hex_color("#12345").is_err()); // wrong length
        assert!(parse_hex_color("#gg0000").is_err()); // non-hex digit
    }
}
