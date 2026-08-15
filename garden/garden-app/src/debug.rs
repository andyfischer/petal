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
//!                    boundary — check it before asserting a command "failed".
//!                    The root `identity` block says which Garden this is (pid,
//!                    port, layout script, panel scripts) — check it when more
//!                    than one Garden is running. A panel's `values` map is
//!                    every binding its last good frame made, which for a real
//!                    app is hundreds of keys: narrow it with
//!                    `?values=a,b,c` (exact names, or a `.`-qualified key's
//!                    tail) and/or `?values_prefix=obs_`, or drop it entirely
//!                    with `?values=none`
//! POST /tick         {"n": 60, "dt": 0.016} — advance every panel by n frames
//!                    of exactly dt seconds, ignoring the sleep/wake window. The
//!                    way to drive an animation or a game without faking input
//! POST /panel/reset  restart every file-backed panel from source, discarding
//!                    Petal `state` — what to call after editing seeded data,
//!                    which hot reload deliberately preserves
//! GET  /version      what this build is: version, git commit + date, build
//!                    date, the named `features` a client can probe, and the
//!                    petal-ui prelude's level and export list. Answered
//!                    without touching the event loop. Ask this *before*
//!                    calling a newer endpoint or flag rather than reading a
//!                    404 as "unsupported" — see `docs/debug-server.md`
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
//!                    home, end, pageup, pagedown. Modifier names: cmd/super/
//!                    meta, ctrl/control, shift, alt/option
//!                    {"key": "w", "op": "down"} / {"op": "up"}  hold a key
//!                    across frames (default "tap" = press+release in one)
//! POST /text         {"text": "hello\nworld"}        insert into focused pane
//! POST /mouse        {"op": "click"|"down"|"move"|"up"|"drag"|"scroll",
//!                     "x": 10, "y": 20,
//!                     "to": {"x": 80, "y": 60},      drag destination
//!                     "lines": 3,                    scroll amount
//!                     "shift": true,                 extend selection
//!                     "clicks": 2,                   double-click (click_count)
//!                     "mods": ["cmd", "alt"]}        every modifier is
//!                                                    delivered, not just shift
//! POST /theme        {"scheme": "light"}              switch built-in scheme
//! GET  /menu         the catalog of native-menu actions POST /menu accepts
//! POST /menu         {"action": "Save"}               fire a native-menu item;
//!                    {"action": "OpenFile", "arg": "path"} / {"action":
//!                    "SetTheme", "arg": "dark"} for the items that take one
//! ```

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::vim::Key;

/// How long a connection waits for the event loop to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on frames one `POST /tick` may advance. The event loop is
/// blocked for the whole batch, and the connection gives up after
/// [`REPLY_TIMEOUT`]; a few seconds of panel time per call is plenty, and a
/// harness that wants more can call again.
const MAX_TICK_FRAMES: u64 = 600;

/// The port this process's debug server is listening on, for `/state`'s
/// identity block. `0` until [`spawn`] binds.
static SERVER_PORT: AtomicU16 = AtomicU16::new(0);

/// The port the debug server bound, or `None` if it never started. Reported in
/// `/state` so a session that is talking to the wrong Garden — easy with
/// several running, since `localhost` may resolve to a different process's
/// IPv6 socket on the same port number — can see it immediately.
pub fn server_port() -> Option<u16> {
    match SERVER_PORT.load(Ordering::Relaxed) {
        0 => None,
        port => Some(port),
    }
}

/// Which of a panel's observed values `GET /state` should report.
///
/// The unfiltered map is every binding the script's last good frame made —
/// every colour constant, every seeded list, and every intermediate that
/// re-derives it — which for a real app runs to hundreds of keys and makes the
/// response unreadable. `?values=a,b,c` and `?values_prefix=obs_` narrow it;
/// `?values=none` drops it. Both selectors may be given, and both accept a
/// comma-separated list; a key matches if *any* selector matches it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueFilter {
    /// Exact names. A key is matched either whole (`sel`) or by its
    /// function-qualified tail (`sel` matches `list_row.sel`), since the
    /// qualification is an artifact of where the binding sits, not something a
    /// caller wants to have to know.
    names: Vec<String>,
    /// Name prefixes, tested against the whole key and its qualified tail.
    prefixes: Vec<String>,
    /// `?values=none`: report an empty map.
    drop_all: bool,
}

impl ValueFilter {
    /// No selector given: report everything, as `/state` always has.
    pub fn is_all(&self) -> bool {
        !self.drop_all && self.names.is_empty() && self.prefixes.is_empty()
    }

    /// Whether an observed key survives the filter.
    pub fn matches(&self, key: &str) -> bool {
        if self.is_all() {
            return true;
        }
        if self.drop_all {
            return false;
        }
        // `fn list_row`'s `let sel` is observed as `list_row.sel`; match on the
        // bare name too so a caller need not know the enclosing function.
        let tail = key.rsplit('.').next().unwrap_or(key);
        self.names.iter().any(|n| n == key || n == tail)
            || self
                .prefixes
                .iter()
                .any(|p| key.starts_with(p.as_str()) || tail.starts_with(p.as_str()))
    }

    /// Build a filter from `(key, value)` query pairs — the parsing under test
    /// for consumers outside this module.
    #[cfg(test)]
    pub(crate) fn from_query_for_test(pairs: &[(&str, &str)]) -> ValueFilter {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ValueFilter::from_query(&owned)
    }

    /// Build from the `values=` / `values_prefix=` query parameters.
    fn from_query(params: &[(String, String)]) -> ValueFilter {
        let mut filter = ValueFilter::default();
        for (key, value) in params {
            let items = || {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            match key.as_str() {
                "values" if value == "none" => filter.drop_all = true,
                "values" if value == "all" => {}
                "values" => filter.names.extend(items()),
                "values_prefix" => filter.prefixes.extend(items()),
                _ => {}
            }
        }
        filter
    }
}

/// The press/release phase a `POST /key` delivers.
///
/// A key used to be undeliverable as *held*: `/key` fed a down and an up in the
/// same frame, so `key_down(k)` was never true from a later `GET /state` and no
/// hold-to-do-X interaction could be driven headless (games in the testbed all
/// invented tap-impulse workarounds instead). `down`/`up` fix that, matching the
/// shape `/mouse` has always had.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyOp {
    /// Press and release in one frame — the default, and what every existing
    /// `{"key": "j"}` body means.
    #[default]
    Tap,
    /// Press and *hold*: the key stays in `keys_down` until an `up` for it.
    Down,
    /// Release a key held by a previous `down`.
    Up,
}

/// One parsed debug command, handled on the event-loop thread.
pub enum DebugCmd {
    /// Editor + panel state. `values` narrows each panel's observed-value map
    /// (see [`ValueFilter`]); the default reports all of it.
    State {
        values: ValueFilter,
    },
    Scene,
    /// Advance every panel by `n` frames of `dt` seconds each, ignoring the
    /// sleep/wake window — deterministic panel time for animation and game
    /// tests, which otherwise have to inject a no-op key per frame.
    Tick {
        n: u32,
        dt: f64,
    },
    /// Restart every file-backed panel from source, discarding Petal `state`.
    PanelReset,
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
        /// Press/release phase — `{"op": "down"}` / `{"op": "up"}` for a **held**
        /// key, the keyboard counterpart of `/mouse`'s down/up. The default,
        /// [`KeyOp::Tap`], is the historical behavior: one press and its release
        /// in the same frame.
        op: KeyOp,
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

/// Bind the listener(s) and spawn the accept thread(s). Returns the bound port
/// (useful with port 0).
///
/// Binds **both** loopback families on the same port: `127.0.0.1` (the
/// authoritative bind, and what the port number is chosen on) and, when it can
/// be had, `[::1]`. Binding only IPv4 is what let `curl localhost:$PORT` reach a
/// *different* Garden — `localhost` resolves to `::1` first on macOS, so with
/// two sessions running the v6 socket of the same port number could belong to
/// someone else's process. Taking both makes the port unambiguous; if the v6
/// side is already held by another process, that is exactly the dangerous case,
/// so say so loudly rather than leaving it to be discovered by debugging the
/// wrong app.
pub fn spawn<S: RequestSink>(port: u16, sink: S) -> io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    SERVER_PORT.store(port, Ordering::Relaxed);
    match TcpListener::bind(("::1", port)) {
        Ok(v6) => accept_loop(v6, sink.clone()),
        Err(err) => eprintln!(
            "garden: debug server could not also bind [::1]:{port} ({err}); \
             use http://127.0.0.1:{port} explicitly — `localhost:{port}` may \
             resolve to a different process's IPv6 socket"
        ),
    }
    accept_loop(listener, sink);
    Ok(port)
}

/// Serve one bound listener on its own thread.
fn accept_loop<S: RequestSink>(listener: TcpListener, sink: S) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let sink = sink.clone();
            thread::spawn(move || {
                let _ = handle_connection(stream, sink);
            });
        }
    });
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
    // `/version` is a pure constant — answer it here rather than through the
    // event loop, so a client can still ask what this build is while the app is
    // busy (or wedged), which is exactly when it wants to know.
    if is_static_endpoint(&method, route_path) {
        return respond_json(&mut writer, 200, &crate::version::report_json());
    }

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

/// Route one request the way a connection would, for tests in other modules
/// (the app-side handlers are exercised through the real paths, query strings
/// and all, rather than by hand-building a [`DebugCmd`]).
#[cfg(test)]
pub(crate) fn route_for_test(
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<DebugCmd, (u16, String)> {
    route(method, path, body)
}

/// Endpoints answered straight from the connection thread, without a
/// [`DebugCmd`] round trip through the frontend's event loop.
fn is_static_endpoint(method: &str, path: &str) -> bool {
    let (bare, _) = split_query(path);
    (method, bare) == ("GET", "/version")
}

fn route(method: &str, path: &str, body: &[u8]) -> Result<DebugCmd, (u16, String)> {
    let parse_body = || -> Result<Value, (u16, String)> {
        serde_json::from_slice(body).map_err(|e| (400, format!("invalid JSON body: {e}")))
    };
    let (bare, query) = split_query(path);
    match (method, bare) {
        ("GET", "/state") => Ok(DebugCmd::State {
            values: ValueFilter::from_query(&query),
        }),
        ("GET", "/scene") => Ok(DebugCmd::Scene),
        ("POST", "/tick") => {
            let v = if body.is_empty() {
                Value::Null
            } else {
                parse_body()?
            };
            let n = v["n"].as_u64().unwrap_or(1);
            if n > MAX_TICK_FRAMES {
                return Err((
                    400,
                    format!("n={n} is more than the {MAX_TICK_FRAMES}-frame limit per /tick"),
                ));
            }
            // 60fps by default: the cadence an awake panel runs at anyway.
            let dt = v["dt"].as_f64().unwrap_or(1.0 / 60.0);
            if !dt.is_finite() || dt < 0.0 {
                return Err((400, format!("bad dt {dt}")));
            }
            Ok(DebugCmd::Tick { n: n as u32, dt })
        }
        ("POST", "/panel/reset") => Ok(DebugCmd::PanelReset),
        ("GET", "/screenshot") => Ok(DebugCmd::Screenshot),
        ("GET", "/frame") => {
            // Optional ?min=N: never blocks, just echoed back as `reached` so a
            // client poll loop is a one-liner. See the DebugCmd::Frame docs.
            let min = query
                .iter()
                .find(|(k, _)| k == "min")
                .map(|(_, v)| v.parse::<u64>())
                .transpose()
                .map_err(|_| (400, format!("bad min= in {path}")))?;
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
            let op = match str_field(&v, "op").as_deref() {
                None | Some("tap") | Some("press") | Some("click") => KeyOp::Tap,
                Some("down") => KeyOp::Down,
                Some("up") => KeyOp::Up,
                Some(other) => return Err((400, format!("unknown key op {other:?}"))),
            };
            Ok(DebugCmd::Key { key, mods, op })
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
        apply_mod_name(&mut mods, name.as_str().unwrap_or_default());
    }
    mods
}

/// The chord a `"mods": [...]` array names. Shared by `/key` and `/mouse` so the
/// two endpoints can never drift on which spellings they honor — `/mouse` used
/// to deliver only `shift`, which silently disabled every alt/cmd-modified
/// mouse behavior a panel script implemented.
pub fn mods_from_names<S: AsRef<str>>(names: &[S]) -> crate::app::Mods {
    let mut mods = crate::app::Mods::default();
    for name in names {
        apply_mod_name(&mut mods, name.as_ref());
    }
    mods
}

/// Set the bit one modifier spelling names. Unknown names are ignored.
fn apply_mod_name(mods: &mut crate::app::Mods, name: &str) {
    match name {
        "cmd" | "super" | "meta" | "command" => mods.cmd = true,
        "ctrl" | "control" => mods.ctrl = true,
        "shift" => mods.shift = true,
        "alt" | "option" | "opt" => mods.alt = true,
        _ => {}
    }
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

/// Split a debug path into its bare route and its decoded query parameters.
/// Runs after [`parse_target`] has taken any `window=` selector off, so what
/// remains is the endpoint's own parameters. Percent escapes are decoded (`%2C`
/// for a comma in a `values=` list) and `+` is a space, as in a form-encoded
/// query.
fn split_query(path: &str) -> (&str, Vec<(String, String)>) {
    let Some((bare, query)) = path.split_once('?') else {
        return (path, Vec::new());
    };
    let params = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect();
    (bare, params)
}

/// Decode `%XX` escapes and `+` in one query token, leaving anything malformed
/// as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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

    // ---- panel ergonomics: value filtering, frame stepping, state reset -----

    /// `/version` is answered from the connection thread, not routed to the
    /// event loop — and it is the only such endpoint.
    #[test]
    fn version_is_answered_without_the_event_loop() {
        assert!(is_static_endpoint("GET", "/version"));
        assert!(is_static_endpoint("GET", "/version?anything=1"));
        assert!(!is_static_endpoint("POST", "/version"));
        assert!(!is_static_endpoint("GET", "/state"));
        // …and it never reaches `route`, which would 404 it.
        assert!(route("GET", "/version", b"").is_err());
    }

    #[test]
    fn state_route_parses_the_value_filter() {
        let filter = |path: &str| match route("GET", path, b"") {
            Ok(DebugCmd::State { values }) => values,
            _ => panic!("{path} must route to State"),
        };
        assert!(filter("/state").is_all(), "no selector means everything");

        let named = filter("/state?values=sel,%20scroll");
        assert!(named.matches("sel") && named.matches("scroll"));
        assert!(!named.matches("palette"));

        let prefixed = filter("/state?values_prefix=obs_");
        assert!(prefixed.matches("obs_rows") && !prefixed.matches("rows"));

        // Both selectors compose, and either alone still narrows.
        let both = filter("/state?values=sel&values_prefix=obs_");
        assert!(both.matches("sel") && both.matches("obs_rows"));
        assert!(!both.matches("palette"));

        assert!(!filter("/state?values=none").matches("sel"));
    }

    #[test]
    fn tick_route_parses_count_and_dt() {
        match route("POST", "/tick", br#"{"n": 60, "dt": 0.016}"#) {
            Ok(DebugCmd::Tick { n, dt }) => {
                assert_eq!(n, 60);
                assert!((dt - 0.016).abs() < 1e-9);
            }
            _ => panic!("POST /tick must route to Tick"),
        }
        // An empty body is one frame at 60fps — the common "step once" call.
        match route("POST", "/tick", b"") {
            Ok(DebugCmd::Tick { n, dt }) => {
                assert_eq!(n, 1);
                assert!((dt - 1.0 / 60.0).abs() < 1e-9);
            }
            _ => panic!("POST /tick with no body must still route"),
        }
        let (status, _) = route("POST", "/tick", br#"{"n": 100000}"#)
            .err()
            .expect("an unbounded tick count must be rejected");
        assert_eq!(status, 400);
    }

    #[test]
    fn panel_reset_routes() {
        match route("POST", "/panel/reset", b"") {
            Ok(DebugCmd::PanelReset) => {}
            _ => panic!("POST /panel/reset must route to PanelReset"),
        }
    }

    /// The `?min=` poll helper still works now that queries are split off
    /// generically rather than by each route.
    #[test]
    fn frame_route_still_parses_min() {
        match route("GET", "/frame?min=5", b"") {
            Ok(DebugCmd::Frame { min }) => assert_eq!(min, Some(5)),
            _ => panic!("GET /frame?min=5 must route to Frame"),
        }
        assert!(route("GET", "/frame?min=abc", b"").is_err());
    }

    #[test]
    fn query_splitting_decodes_escapes() {
        let (path, params) = split_query("/state?values=a%2Cb&values_prefix=obs+x");
        assert_eq!(path, "/state");
        assert_eq!(params[0], ("values".to_string(), "a,b".to_string()));
        assert_eq!(
            params[1],
            ("values_prefix".to_string(), "obs x".to_string())
        );
    }

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
            Ok(DebugCmd::Key { key, mods, op }) => {
                assert_eq!(key, "s");
                assert_eq!(mods, vec!["cmd".to_string()]);
                assert_eq!(op, KeyOp::Tap, "a body with no \"op\" is a tap");
            }
            _ => panic!("stripped path {path:?} no longer routes to Key"),
        }
    }

    /// `/key` grew press/release forms so a driver can *hold* a key; an
    /// unknown op is a 400 rather than a silently-dropped press.
    #[test]
    fn key_route_parses_the_press_phase() {
        let op_of = |body: &str| match route("POST", "/key", body.as_bytes()) {
            Ok(DebugCmd::Key { op, .. }) => op,
            other => panic!("{body} did not route to Key ({})", other.is_ok()),
        };
        assert_eq!(op_of(r#"{"key":"w"}"#), KeyOp::Tap);
        assert_eq!(op_of(r#"{"key":"w","op":"tap"}"#), KeyOp::Tap);
        assert_eq!(op_of(r#"{"key":"w","op":"down"}"#), KeyOp::Down);
        assert_eq!(op_of(r#"{"key":"w","op":"up"}"#), KeyOp::Up);
        assert!(route("POST", "/key", br#"{"key":"w","op":"hold"}"#).is_err());
    }

    /// Both input endpoints understand the same modifier spellings — `/mouse`
    /// used to deliver only `shift`, so every alt/cmd-modified mouse behavior a
    /// panel implemented was untestable and shipped unverified.
    #[test]
    fn every_modifier_is_parsed_for_key_and_mouse() {
        let all = mods_from_names(&["cmd", "ctrl", "shift", "alt"]);
        assert_eq!(
            (all.cmd, all.ctrl, all.shift, all.alt),
            (true, true, true, true)
        );
        // Alternate spellings.
        let alt = mods_from_names(&["option"]);
        assert!(alt.alt);
        assert!(mods_from_names(&["control"]).ctrl);
        assert!(mods_from_names(&["super"]).cmd);
        assert!(mods_from_names(&["meta"]).cmd);
        // Unknown names are ignored, not an error.
        assert_eq!(mods_from_names(&["hyper"]), crate::app::Mods::default());

        let mouse = match route(
            "POST",
            "/mouse",
            br#"{"op":"down","x":1,"y":2,"mods":["alt","cmd"]}"#,
        ) {
            Ok(DebugCmd::Mouse { mods, .. }) => mods,
            _ => panic!("did not route to Mouse"),
        };
        assert!(mouse.alt, "/mouse must deliver alt, not just shift");
        assert!(mouse.cmd);
        // The legacy `"shift": true` shorthand still works.
        let legacy = match route(
            "POST",
            "/mouse",
            br#"{"op":"down","x":1,"y":2,"shift":true}"#,
        ) {
            Ok(DebugCmd::Mouse { mods, .. }) => mods,
            _ => panic!("did not route to Mouse"),
        };
        assert!(legacy.shift);
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
