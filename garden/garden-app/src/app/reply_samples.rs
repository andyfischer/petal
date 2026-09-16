//! Pins the debug server's reply shapes against checked-in JSON samples.
//!
//! `garden/tools/lib/debug-client.ts` hand-writes TypeScript types (`AppState`,
//! `PaneState`, `VersionReport`, `SceneFindReply`, `BatchReply`, ...) for replies
//! the Rust side builds with `json!` literals. Nothing tied the two together, so
//! a renamed or dropped field broke clients only at run time. This test drives
//! one of each reply through the same routing and `App::answer` path a real
//! connection uses, and compares its *shape* — every `path: type` it contains,
//! array elements merged — against `garden/tools/lib/debug-replies/<name>.json`.
//! The TS types are written from those samples.
//!
//! Values are not compared (pids, temp paths and build stamps differ per run);
//! only which fields exist and what JSON type each holds. A panel's `values`
//! map is open-ended, so only its presence is.
//!
//! When a reply shape changes on purpose:
//!
//! ```text
//! UPDATE_REPLY_SAMPLES=1 cargo test -p garden-app reply_samples
//! ```
//!
//! then update the matching types in `tools/lib/debug-client.ts` and review the
//! sample diff alongside them.

use super::debug_server::NoCapture;
use super::App;
use crate::clipboard::InMemoryClipboard;
use crate::debug::{self, Reply};
use garden_script::LayoutNode;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

fn samples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/lib/debug-replies")
}

/// An editor pane (with a file) beside a panel that draws text, keeps `state`
/// and binds a few values — enough for every optional block of `/state` to be
/// present.
fn sample_app() -> (App, Vec<tempfile::NamedTempFile>) {
    let mut text = tempfile::NamedTempFile::with_suffix(".txt").unwrap();
    write!(text, "hello\nworld\n").unwrap();
    let mut script = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
    write!(
        script,
        "state hits = 0\n\
         if key_pressed(\"space\") then hits = hits + 1 end\n\
         let seen = hits\n\
         let label = \"Save\"\n\
         draw_rect(0, 0, 10, 10, 1, 2, 3)\n\
         draw_text(label, 40, 60, 16, 1, 1, 1)\n"
    )
    .unwrap();
    let mut app = App::new(
        None,
        LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some(text.path().to_string_lossy().into_owned()),
                    line_numbers: false,
                    wrap: true,
                },
                LayoutNode::Panel {
                    script: script.path().to_string_lossy().into_owned(),
                    screens: Vec::new(),
                },
            ],
            ratios: None,
        },
        true,
        crate::app::Viewport {
            size: (800.0, 600.0),
            cell: (8.0, 16.0),
            scale: 1.0,
        },
        Box::new(InMemoryClipboard::default()),
    );
    app.settle_panels();
    (app, vec![text, script])
}

/// One request the way `handle_connection` runs it: `window=` then `select=`
/// peeled off, routed, answered (with the snapshot when a state-changing
/// command carries `select=`), then projected.
fn request(app: &mut App, method: &str, path: &str, body: &str) -> Value {
    let (path, _window) = debug::parse_target(path).expect("window=");
    let (path, select) = debug::parse_select(&path).expect("select=");
    let cmd = debug::route_for_test(method, &path, body.as_bytes())
        .unwrap_or_else(|(status, err)| panic!("{method} {path} routes: {status} {err}"));
    let snapshot = select.is_some() && cmd.changes_state();
    let reply = app
        .answer(cmd, snapshot, &mut NoCapture)
        .unwrap_or_else(|err| panic!("{method} {path} answers: {err}"));
    let Reply::Json(value) = reply else {
        panic!("{method} {path} must answer JSON");
    };
    match select {
        Some(select) => select.apply(&value),
        None => value,
    }
}

/// Every `path: type` line in a value. Array elements share the path `[]`, so
/// an array's shape is the union of its elements' shapes.
fn shape(value: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, path: &str, out: &mut BTreeSet<String>) {
        let kind = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(items) => {
                for item in items {
                    walk(item, &format!("{path}[]"), out);
                }
                "array"
            }
            // A panel's `values` map is whatever the script (and the prelude
            // and theme it loads) bound: open-ended, `Record<string, unknown>`
            // on the TS side, so only its presence is pinned.
            Value::Object(_) if path == "values" || path.ends_with(".values") => "object",
            Value::Object(map) => {
                for (key, item) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(item, &child, out);
                }
                "object"
            }
        };
        let at = if path.is_empty() { "(root)" } else { path };
        out.insert(format!("{at}: {kind}"));
    }
    let mut out = BTreeSet::new();
    walk(value, "", &mut out);
    out
}

/// The sample as written to disk: long lists of strings (feature names,
/// prelude exports) cut to three entries, and temp-file paths written as
/// `<tmp>/…`, which keeps the file readable and stable across updates without
/// changing its shape — checked, and skipped if it would.
fn trimmed(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            let strings = items.iter().all(Value::is_string);
            let kept = if strings {
                &items[..items.len().min(3)]
            } else {
                &items[..]
            };
            Value::Array(kept.iter().map(trimmed).collect())
        }
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), trimmed(v))).collect())
        }
        Value::String(text) => {
            let tmp = std::env::temp_dir()
                .to_string_lossy()
                .trim_end_matches('/')
                .to_string();
            let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned());
            let prefixes = [
                (format!("/private{tmp}"), "<tmp>"),
                (tmp, "<tmp>"),
                (cwd.unwrap_or_default(), "<cwd>"),
            ];
            for (prefix, stand_in) in prefixes {
                if let Some(rest) = text
                    .strip_prefix(prefix.as_str())
                    .filter(|_| prefix.len() > 1)
                {
                    return Value::String(format!("{stand_in}{rest}"));
                }
            }
            value.clone()
        }
        other => other.clone(),
    }
}

/// Compare a reply against its sample, or rewrite the sample under
/// `UPDATE_REPLY_SAMPLES=1`. Returns a description of the drift, if any.
fn check(name: &str, reply: &Value) -> Option<String> {
    let file = samples_dir().join(format!("{name}.json"));
    if std::env::var_os("UPDATE_REPLY_SAMPLES").is_some() {
        let short = trimmed(reply);
        let written = if shape(&short) == shape(reply) {
            short
        } else {
            reply.clone()
        };
        std::fs::create_dir_all(samples_dir()).unwrap();
        std::fs::write(
            &file,
            serde_json::to_string_pretty(&written).unwrap() + "\n",
        )
        .unwrap();
        return None;
    }
    let sample: Value = match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(err) => return Some(format!("{name}: {} is not JSON: {err}", file.display())),
        },
        Err(err) => return Some(format!("{name}: no sample at {} ({err})", file.display())),
    };
    let (want, got) = (shape(&sample), shape(reply));
    if want == got {
        return None;
    }
    let mut report = format!("{name} ({}):", file.display());
    for line in want.difference(&got) {
        report.push_str(&format!("\n  - {line}   (in the sample, not the reply)"));
    }
    for line in got.difference(&want) {
        report.push_str(&format!("\n  + {line}   (in the reply, not the sample)"));
    }
    Some(report)
}

#[test]
fn debug_replies_match_their_samples() {
    let (mut app, _files) = sample_app();
    let mut replies: Vec<(&str, Value)> = Vec::new();
    let mut add = |name, value| replies.push((name, value));

    // Reads.
    add("version", crate::version::report_json());
    add("state", request(&mut app, "GET", "/state", ""));
    add(
        "state-select",
        request(
            &mut app,
            "GET",
            "/state?select=focus,panes.0.cursor,panes.*.panel.frame_stats",
            "",
        ),
    );
    add("frame", request(&mut app, "GET", "/frame?min=1", ""));
    add("windows", request(&mut app, "GET", "/windows", ""));
    add("menu-list", request(&mut app, "GET", "/menu", ""));

    // Captures (JSON form; PNG and text are not JSON).
    add("scene", request(&mut app, "GET", "/scene?pane=1", ""));
    add(
        "scene-find",
        request(&mut app, "GET", "/scene?find=text:Save&pane=1", ""),
    );

    // Commands: the default receipts, and (`key-select`) a command carrying
    // select=, which projects the settled snapshot.
    // `v` then `l` leaves a visual selection, so the acks carry its shape.
    add("key", request(&mut app, "POST", "/key", r#"{"key":"v"}"#));
    add(
        "key-select",
        request(
            &mut app,
            "POST",
            "/key?select=focus,cursor,selection,panes.1.panel.values.seen",
            r#"{"key":"l"}"#,
        ),
    );
    request(&mut app, "POST", "/key", r#"{"key":"escape"}"#);
    add(
        "text",
        request(&mut app, "POST", "/text", r#"{"text":"x"}"#),
    );
    add(
        "mouse",
        request(
            &mut app,
            "POST",
            "/mouse",
            r#"{"op":"click","x":20,"y":50}"#,
        ),
    );
    add("tick", request(&mut app, "POST", "/tick", r#"{"n":2}"#));
    add("seed", request(&mut app, "POST", "/seed", r#"{"seed":7}"#));
    add(
        "panel-reset",
        request(&mut app, "POST", "/panel/reset", "{}"),
    );
    add(
        "theme",
        request(&mut app, "POST", "/theme", r#"{"scheme":"light"}"#),
    );

    // Batches, succeeding and failing.
    add(
        "batch",
        request(
            &mut app,
            "POST",
            "/batch",
            r#"[{"path":"/key","key":"space"},
                {"path":"/key?select=panes.1.panel.values.seen","key":"space"},
                {"path":"/buffer/0","method":"GET"},
                {"path":"/tick","n":1}]"#,
        ),
    );
    add(
        "batch-failed",
        request(
            &mut app,
            "POST",
            "/batch",
            r#"[{"path":"/tick","n":1},{"path":"/theme","scheme":"no-such-scheme"}]"#,
        ),
    );

    let drift: Vec<String> = replies
        .iter()
        .filter_map(|(name, value)| check(name, value))
        .collect();
    assert!(
        drift.is_empty(),
        "debug reply shapes drifted from tools/lib/debug-replies. If intended, rerun with \
         UPDATE_REPLY_SAMPLES=1 and update the types in tools/lib/debug-client.ts.\n{}",
        drift.join("\n")
    );
}
