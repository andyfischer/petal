//! Live debug server: HTTP over localhost for agent-driven inspection and
//! input injection while the app runs (the petal-sdl `--agent` protocol idea,
//! adapted to a long-running interactive app).
//!
//! A background thread accepts connections and forwards each parsed request
//! to the frontend's event loop through a [`RequestSink`] (a winit event-loop
//! proxy for the windowed frontend, a plain mpsc sender for the headless and
//! terminal frontends); the loop handles it against the live [`crate::app::App`]
//! and replies over an mpsc channel. Opt in with `garden --debug-port <n>`
//! (0 picks a free port); there is no default port, and headless mode
//! requires one.
//!
//! Endpoints (all on `127.0.0.1`, JSON bodies):
//!
//! ```text
//! GET  /state        editor state: panes, cursors, selections, focus, errors;
//!                    each editor pane's `pending` is its mid-command vim state
//!                    (buffered count/operator/prefix) or null at a clean
//!                    boundary — check it before asserting a command "failed"
//! GET  /scene        the primitives of the current frame (quads + text runs),
//!                    panels settled first (see /screenshot)
//! GET  /frame        {"ok": true, "frame": n} — the global frame counter,
//!                    answered instantly (never blocks); optional ?min=N adds
//!                    "reached": frame >= N for easy client-side polling
//! GET  /buffer/<n>   full text of pane n's buffer (text/plain)
//! GET  /screenshot   PNG of a complete, settled frame rendered offscreen:
//!                    panel frames are run until their output is steady, so
//!                    the capture reflects all previously injected input; the
//!                    captured frame number is in the X-Garden-Frame header
//! POST /key          {"key": "s", "mods": ["cmd"]}   named keys: enter, tab,
//!                    space, backspace, delete, escape, left/right/up/down,
//!                    home, end, pageup, pagedown
//! POST /text         {"text": "hello\nworld"}        insert into focused pane
//! POST /mouse        {"op": "click"|"down"|"move"|"up"|"drag"|"scroll",
//!                     "x": 10, "y": 20,
//!                     "to": {"x": 80, "y": 60},      drag destination
//!                     "lines": 3,                    scroll amount
//!                     "shift": true,                 extend selection
//!                     "mods": ["cmd"]}               cmd-click a shape to
//!                                                    jump to its code
//! POST /theme        {"scheme": "light"}              switch built-in scheme
//! GET  /menu         the catalog of native-menu actions POST /menu accepts
//! POST /menu         {"action": "Save"}               fire a native-menu item;
//!                    {"action": "OpenFile", "arg": "path"} / {"action":
//!                    "SetTheme", "arg": "dark"} for the items that take one
//! ```

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::vim::Key;

/// How long a connection waits for the event loop to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// One parsed debug command, handled on the event-loop thread.
pub enum DebugCmd {
    State,
    Scene,
    Screenshot,
    /// The global frame counter, answered instantly (the client polls it —
    /// blocking here would tie up the event-loop thread that must keep
    /// ticking to advance the very frame being waited on). `min` is echoed
    /// back as a `reached` boolean for convenience.
    Frame {
        min: Option<u64>,
    },
    BufferText {
        pane: usize,
    },
    Key {
        key: String,
        mods: Vec<String>,
    },
    Text {
        text: String,
    },
    /// An ex command as typed, without the leading `:` — `{"command": "Diff main"}`.
    Command {
        command: String,
    },
    Theme {
        scheme: String,
    },
    /// List the native-menu actions `Menu` accepts (`GET /menu`).
    MenuList,
    /// List the open OS windows and which one is focused (`GET /windows`).
    /// Answered by the frontend (only it knows the window registry), like
    /// `Screenshot`.
    Windows,
    /// Fire a native menu-bar item by name — the one input the menu bar
    /// produces that keystroke injection can't reach (muda accelerators and
    /// clicks). `arg` is the path for the Open items / the theme for `SetTheme`.
    Menu {
        action: String,
        arg: Option<String>,
    },
    Mouse {
        op: String,
        x: f32,
        y: f32,
        to: Option<(f32, f32)>,
        /// Vertical wheel amount for `scroll`, in lines (positive = down).
        /// Fractional: `0.5` is half a line, the sub-cell motion a trackpad
        /// produces, so smooth scrolling is drivable from a test.
        lines: f32,
        /// Horizontal wheel amount for `scroll`, in display columns (positive =
        /// right). Fractional, like `lines`.
        cols: f32,
        /// Modifiers held during the press. `shift` extends the selection;
        /// `cmd`/`ctrl` on a traced canvas is the jump-to-code gesture. Taken
        /// from `"mods": ["cmd"]`, with `"shift": true` still accepted as the
        /// shorthand it always was.
        mods: crate::app::Mods,
        /// Multi-click count for `click`/`down`/`drag` (default 1; 2 =
        /// double-click word selection, 3 = triple-click line selection).
        clicks: u32,
        /// Which button `click`/`down`/`up` press, in the `petal-ui` numbering:
        /// 0 = left (the default, and every op's behavior before this existed),
        /// 1 = right — the context gesture panel scripts open menus from. Any
        /// other value is treated as left, since there is nothing else the host
        /// routes. `drag`/`scroll` ignore it: neither has a right-button form.
        button: u8,
    },
}

/// A successful reply body.
pub enum Reply {
    Json(Value),
    /// A screenshot: the PNG bytes plus the global frame number of the
    /// captured scene, sent as an `X-Garden-Frame` response header.
    Png {
        png: Vec<u8>,
        frame: u64,
    },
    Text(String),
}

/// What travels from a server connection thread to the frontend's loop.
pub struct DebugRequest {
    pub cmd: DebugCmd,
    /// Target window by 1-based session ordinal (`?window=<n>`), or `None` for
    /// the focused window. Single-window frontends ignore anything but `1`.
    pub window: Option<u64>,
    pub reply: mpsc::Sender<Result<Reply, String>>,
}

/// How the server hands requests to whatever event loop owns the [`crate::app::App`].
/// Each frontend supplies its own: the windowed frontend wraps a winit
/// `EventLoopProxy`, the headless and terminal frontends a plain mpsc sender.
pub trait RequestSink: Clone + Send + 'static {
    /// Deliver one request to the app loop. Returns false once the loop has
    /// exited (the connection then reports the app as gone).
    fn send(&self, request: DebugRequest) -> bool;
}

impl RequestSink for mpsc::Sender<DebugRequest> {
    fn send(&self, request: DebugRequest) -> bool {
        mpsc::Sender::send(self, request).is_ok()
    }
}

/// Bind the listener and spawn the accept thread. Returns the bound port
/// (useful with port 0).
pub fn spawn<S: RequestSink>(port: u16, sink: S) -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let sink = sink.clone();
            thread::spawn(move || {
                let _ = handle_connection(stream, sink);
            });
        }
    });
    Ok(port)
}

fn handle_connection<S: RequestSink>(stream: TcpStream, sink: S) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return respond_json(
            &mut writer,
            400,
            &json!({"ok": false, "error": "bad request line"}),
        );
    };
    let (method, path) = (method.to_string(), path.to_string());

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;

    // Peel an optional `?window=<n>` selector off the path before routing, so
    // every endpoint can target a specific window; the rest of the path routes
    // exactly as it did single-window.
    let (route_path, window) = match parse_target(&path) {
        Ok(target) => target,
        Err((status, msg)) => {
            return respond_json(&mut writer, status, &json!({"ok": false, "error": msg}));
        }
    };
    let cmd = match route(&method, route_path, &body) {
        Ok(cmd) => cmd,
        Err((status, msg)) => {
            return respond_json(&mut writer, status, &json!({"ok": false, "error": msg}));
        }
    };

    let (tx, rx) = mpsc::channel();
    if !sink.send(DebugRequest {
        cmd,
        window,
        reply: tx,
    }) {
        return respond_json(
            &mut writer,
            500,
            &json!({"ok": false, "error": "event loop has exited"}),
        );
    }
    match rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(Reply::Json(value))) => respond_json(&mut writer, 200, &value),
        Ok(Ok(Reply::Png { png, frame })) => respond_with(
            &mut writer,
            200,
            "image/png",
            &[("X-Garden-Frame", frame.to_string())],
            &png,
        ),
        Ok(Ok(Reply::Text(text))) => respond(
            &mut writer,
            200,
            "text/plain; charset=utf-8",
            text.as_bytes(),
        ),
        Ok(Err(msg)) => respond_json(&mut writer, 400, &json!({"ok": false, "error": msg})),
        Err(_) => respond_json(
            &mut writer,
            504,
            &json!({"ok": false, "error": "timed out waiting for the event loop"}),
        ),
    }
}

fn route(method: &str, path: &str, body: &[u8]) -> Result<DebugCmd, (u16, String)> {
    let parse_body = || -> Result<Value, (u16, String)> {
        serde_json::from_slice(body).map_err(|e| (400, format!("invalid JSON body: {e}")))
    };
    match (method, path) {
        ("GET", "/state") => Ok(DebugCmd::State),
        ("GET", "/scene") => Ok(DebugCmd::Scene),
        ("GET", "/screenshot") => Ok(DebugCmd::Screenshot),
        ("GET", p) if p == "/frame" || p.starts_with("/frame?") => {
            // Optional ?min=N: never blocks, just echoed back as `reached` so a
            // client poll loop is a one-liner. See the DebugCmd::Frame docs.
            let min = p
                .split_once('?')
                .map(|(_, q)| q)
                .and_then(|q| {
                    q.split('&')
                        .find_map(|pair| pair.strip_prefix("min="))
                        .map(|v| v.parse::<u64>())
                })
                .transpose()
                .map_err(|_| (400, format!("bad min= in {p}")))?;
            Ok(DebugCmd::Frame { min })
        }
        ("GET", p) if p.starts_with("/buffer/") => {
            let idx = p["/buffer/".len()..]
                .parse()
                .map_err(|_| (400, format!("bad pane index in {p}")))?;
            Ok(DebugCmd::BufferText { pane: idx })
        }
        ("POST", "/key") => {
            let v = parse_body()?;
            let key = str_field(&v, "key").ok_or((400, "missing \"key\"".to_string()))?;
            let mods = v["mods"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Ok(DebugCmd::Key { key, mods })
        }
        ("POST", "/text") => {
            let v = parse_body()?;
            let text = str_field(&v, "text").ok_or((400, "missing \"text\"".to_string()))?;
            Ok(DebugCmd::Text { text })
        }
        ("POST", "/command") => {
            let v = parse_body()?;
            let command =
                str_field(&v, "command").ok_or((400, "missing \"command\"".to_string()))?;
            Ok(DebugCmd::Command { command })
        }
        ("POST", "/theme") => {
            let v = parse_body()?;
            let scheme = str_field(&v, "scheme").ok_or((400, "missing \"scheme\"".to_string()))?;
            Ok(DebugCmd::Theme { scheme })
        }
        ("GET", "/windows") => Ok(DebugCmd::Windows),
        ("GET", "/menu") => Ok(DebugCmd::MenuList),
        ("POST", "/menu") => {
            let v = parse_body()?;
            let action = str_field(&v, "action").ok_or((400, "missing \"action\"".to_string()))?;
            Ok(DebugCmd::Menu {
                action,
                arg: str_field(&v, "arg"),
            })
        }
        ("POST", "/mouse") => {
            let v = parse_body()?;
            let op = str_field(&v, "op").ok_or((400, "missing \"op\"".to_string()))?;
            let to = v
                .get("to")
                .and_then(|t| Some((t["x"].as_f64()? as f32, t["y"].as_f64()? as f32)));
            Ok(DebugCmd::Mouse {
                op,
                x: v["x"].as_f64().unwrap_or(0.0) as f32,
                y: v["y"].as_f64().unwrap_or(0.0) as f32,
                to,
                lines: v["lines"].as_f64().unwrap_or(0.0) as f32,
                cols: v["cols"].as_f64().unwrap_or(0.0) as f32,
                mods: mouse_mods(&v),
                clicks: v["clicks"].as_u64().unwrap_or(1) as u32,
                button: v["button"].as_u64().unwrap_or(0) as u8,
            })
        }
        _ => Err((404, format!("no endpoint {method} {path}"))),
    }
}

/// Modifiers for a `/mouse` body: the `"mods": ["cmd", "shift", …]` array plus
/// the older `"shift": true` shorthand, which stays valid — every existing
/// harness uses it, and it means exactly `mods: ["shift"]`.
fn mouse_mods(v: &serde_json::Value) -> crate::app::Mods {
    let mut mods = crate::app::Mods {
        shift: v["shift"].as_bool().unwrap_or(false),
        ..Default::default()
    };
    for name in v["mods"].as_array().into_iter().flatten() {
        match name.as_str().unwrap_or_default() {
            "cmd" | "super" | "meta" => mods.cmd = true,
            "ctrl" => mods.ctrl = true,
            "shift" => mods.shift = true,
            _ => {}
        }
    }
    mods
}

/// Split an optional `?window=<n>` selector off a debug path, returning the
/// stripped path (a borrowed subslice) plus the target window's 1-based
/// ordinal — `None` means the focused window (the single-window default). The
/// selector must be the sole query parameter or the last one
/// (`/frame?min=5&window=2`), so what remains is always a clean prefix. The
/// ordinal is 1-based, so `0`, negatives, and non-numbers are rejected 400.
pub(crate) fn parse_target(path: &str) -> Result<(&str, Option<u64>), (u16, String)> {
    // Prefer `?window=` (sole parameter), then `&window=` (trailing one);
    // either way the cut point lets us hand back a subslice of `path`.
    for sep in ['?', '&'] {
        let needle = format!("{sep}window=");
        let Some(cut) = path.rfind(&needle) else {
            continue;
        };
        let value = &path[cut + needle.len()..];
        // Only the final parameter is strippable to a contiguous prefix.
        if value.contains('&') {
            continue;
        }
        let ordinal = value
            .parse::<u64>()
            .ok()
            .filter(|&n| n >= 1)
            .ok_or((400, format!("bad window ordinal in {path:?}: {value:?}")))?;
        return Ok((&path[..cut], Some(ordinal)));
    }
    Ok((path, None))
}

fn str_field(v: &Value, name: &str) -> Option<String> {
    v[name].as_str().map(str::to_string)
}

fn respond_json(stream: &mut TcpStream, status: u16, value: &Value) -> io::Result<()> {
    respond(
        stream,
        status,
        "application/json",
        value.to_string().as_bytes(),
    )
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    respond_with(stream, status, content_type, &[], body)
}

/// Like [`respond`], with extra response headers (name, value) — e.g. the
/// screenshot endpoint's `X-Garden-Frame`.
fn respond_with(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// Map a debug key name to the toolkit-independent [`Key`] the app core
/// consumes. Single characters map to `Key::Char`; everything else must be a
/// named key.
pub fn parse_key(name: &str) -> Option<Key> {
    let named = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Char(' '),
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "escape" | "esc" => Key::Escape,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        _ => {
            if name.chars().count() == 1 {
                return Some(Key::Char(name.chars().next()?));
            }
            return None;
        }
    };
    Some(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse_clicks(body: &str) -> u32 {
        match route("POST", "/mouse", body.as_bytes()) {
            Ok(DebugCmd::Mouse { clicks, .. }) => clicks,
            other => panic!("expected a mouse command, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn mouse_route_parses_clicks() {
        assert_eq!(mouse_clicks(r#"{"op":"click","x":1,"y":2,"clicks":2}"#), 2);
        assert_eq!(mouse_clicks(r#"{"op":"down","x":1,"y":2,"clicks":3}"#), 3);
    }

    #[test]
    fn mouse_route_defaults_clicks_to_one() {
        assert_eq!(mouse_clicks(r#"{"op":"click","x":1,"y":2}"#), 1);
    }

    // ---- MWI Phase 6: window addressing ----------------------------------
    //
    // Contract under test (not yet implemented):
    // - `parse_target(path)` splits an optional `?window=<n>` query off any
    //   endpoint path, returning the bare path plus the target window's
    //   per-session ordinal (1-based). It runs in `handle_connection` BEFORE
    //   `route()`, and rejects malformed ordinals with the same
    //   `(status, message)` error shape `route()` uses for unknown paths.
    // - `DebugRequest` carries the ordinal as `window: Option<u64>`;
    //   None = focused window (today's behavior).
    // - `GET /windows` routes to a new `DebugCmd::Windows` (the frontend
    //   builds the {ok, windows: [...]} reply, so only routing is pinned).

    #[test]
    fn windows_endpoint_routes() {
        match route("GET", "/windows", b"") {
            Ok(DebugCmd::Windows) => {}
            Ok(_) => panic!("GET /windows routed to the wrong command"),
            Err((status, msg)) => panic!("GET /windows rejected: {status} {msg}"),
        }
    }

    #[test]
    fn window_query_param_is_parsed() {
        let (path, window) = parse_target("/state?window=2").expect("valid window param");
        assert_eq!(path, "/state");
        assert_eq!(window, Some(2));
    }

    #[test]
    fn ex_commands_route_to_command() {
        match route("POST", "/command", br#"{"command":"Diff main"}"#) {
            Ok(DebugCmd::Command { command }) => assert_eq!(command, "Diff main"),
            _ => panic!("POST /command must route to DebugCmd::Command"),
        }
        match route("POST", "/command", b"{}") {
            Err((status, _)) => assert_eq!(status, 400, "a body without \"command\" is rejected"),
            Ok(_) => panic!("a /command body without \"command\" must be rejected"),
        }
    }

    #[test]
    fn no_window_param_targets_focused() {
        let (path, window) = parse_target("/state").expect("plain path");
        assert_eq!(path, "/state");
        assert_eq!(window, None, "no ?window= must mean the focused window");
    }

    #[test]
    fn window_param_composes_with_existing_routes() {
        let (path, window) = parse_target("/buffer/3?window=1").expect("valid window param");
        assert_eq!(window, Some(1));
        match route("GET", path, b"") {
            Ok(DebugCmd::BufferText { pane }) => assert_eq!(pane, 3),
            _ => panic!("stripped path {path:?} no longer routes to BufferText"),
        }
    }

    #[test]
    fn malformed_window_param_is_rejected() {
        for bad in ["/state?window=abc", "/state?window=0", "/state?window=-1"] {
            let (status, _msg) = parse_target(bad)
                .err()
                .unwrap_or_else(|| panic!("{bad} must be rejected"));
            assert!(
                (400..500).contains(&status),
                "{bad} must reject with a 4xx, got {status}"
            );
        }
    }

    #[test]
    fn post_endpoints_accept_window_param() {
        let (path, window) = parse_target("/key?window=2").expect("valid window param");
        assert_eq!(window, Some(2));
        match route("POST", path, br#"{"key":"s","mods":["cmd"]}"#) {
            Ok(DebugCmd::Key { key, mods }) => {
                assert_eq!(key, "s");
                assert_eq!(mods, vec!["cmd".to_string()]);
            }
            _ => panic!("stripped path {path:?} no longer routes to Key"),
        }
    }
}

/// Encode tightly packed RGBA8 pixels as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("PNG header");
        writer.write_image_data(rgba).expect("PNG encode");
    }
    out
}
