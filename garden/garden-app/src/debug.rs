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
//!                    with `?values=none`. Any JSON endpoint also takes
//!                    `?select=panes.0.cursor,focus` to project the reply onto
//!                    dotted paths (see [`Select`]). On a state-changing
//!                    command (`POST /key?select=…` etc.) it projects the
//!                    settled post-command `/state` instead of the receipt
//!                    (see [`DebugCmd::changes_state`])
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
//! GET  /capture      one capture of the settled frame (see /screenshot for the
//!                    settle contract). ?format=png|json|text (default: the
//!                    frontend's native raster, PNG or the --term grid);
//!                    ?pane=<n> crops/rebases. /scene and /screenshot are its
//!                    aliases
//! GET  /scene        = /capture?format=json: the primitives of the current
//!                    frame (quads + text runs). ?find=text:Save
//!                    (exact) or ?find=text~:Sav (substring) keeps only the
//!                    matching text runs, each with its `rect` and `center`
//! GET  /frame        {"ok": true, "frame": n} — the global frame counter,
//!                    answered instantly (never blocks); optional ?min=N adds
//!                    "reached": frame >= N for easy client-side polling
//! GET  /buffer/<n>   full text of pane n's buffer (text/plain)
//! GET  /screenshot   = /capture: PNG of a complete, settled frame rendered offscreen:
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

/// One step of a `?select=` path, and the matching rule [`ValueFilter`] has
/// always used for observed-value names — the two share this one vocabulary.
///
/// - `*` matches every key (or every array element).
/// - `name*` matches a key that starts with `name`, whole or by its
///   `.`-qualified tail.
/// - `name` matches a key exactly, or by its `.`-qualified tail (`sel` matches
///   `list_row.sel`), since that qualification is an artifact of where a
///   binding sits, not something a caller should have to know. On an array it
///   is an index (`panes.0`).
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Any,
    Name(String),
    Prefix(String),
}

impl Segment {
    fn parse(text: &str) -> Result<Segment, String> {
        match text {
            "" => Err("empty path segment".to_string()),
            "*" => Ok(Segment::Any),
            _ => match text.strip_suffix('*') {
                Some(stem) if !stem.contains('*') => Ok(Segment::Prefix(stem.to_string())),
                None if !text.contains('*') => Ok(Segment::Name(text.to_string())),
                _ => Err(format!(
                    "{text:?}: `*` is only allowed at the end of a segment"
                )),
            },
        }
    }

    /// Whether an object key matches this segment.
    pub fn matches_key(&self, key: &str) -> bool {
        let tail = key.rsplit('.').next().unwrap_or(key);
        match self {
            Segment::Any => true,
            Segment::Name(n) => n == key || n == tail,
            Segment::Prefix(p) => key.starts_with(p.as_str()) || tail.starts_with(p.as_str()),
        }
    }

    /// Whether array element `index` matches this segment.
    fn matches_index(&self, index: usize) -> bool {
        match self {
            Segment::Any => true,
            Segment::Name(n) => n.parse::<usize>() == Ok(index),
            Segment::Prefix(_) => false,
        }
    }
}

/// A `?select=` field projection, accepted by every JSON endpoint: a
/// comma-separated list of dotted paths (`panes.0.cursor,focus`,
/// `panes.*.panel.values.sel`), each step a [`Segment`]. The reply keeps only
/// the selected fields *in their original positions* — objects keep the
/// selected keys, arrays keep their indices (unselected elements before a
/// selected one read as `null`) — so `reply.panes[0].cursor` reads the same
/// value projected as unprojected. A path that matches nothing is simply
/// absent. A top-level `ok` is always kept.
///
/// Observed-value keys may themselves contain dots (`list_row.sel`); a path
/// matches such a key either by its tail or by spelling it out
/// (`values.list_row.sel`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Select {
    paths: Vec<Vec<Segment>>,
}

impl Select {
    /// Parse one `select=` value. An empty list, an empty segment (`a..b`), or
    /// a `*` anywhere but a segment's end is an error.
    pub fn parse(text: &str) -> Result<Select, String> {
        let mut paths = Vec::new();
        for path in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let segments = path
                .split('.')
                .map(Segment::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("bad select path {path:?}: {e}"))?;
            paths.push(segments);
        }
        if paths.is_empty() {
            return Err("select= needs at least one path".to_string());
        }
        Ok(Select { paths })
    }

    /// Project a reply onto the selected paths.
    pub fn apply(&self, value: &Value) -> Value {
        let paths: Vec<&[Segment]> = self.paths.iter().map(Vec::as_slice).collect();
        let mut out = project(value, &paths).unwrap_or_else(|| json!({}));
        if let (Some(ok), Some(obj)) = (value.get("ok"), out.as_object_mut()) {
            obj.entry("ok").or_insert_with(|| ok.clone());
        }
        out
    }
}

/// The part of `value` any of `paths` reaches, or `None` if none reaches
/// anything. An exhausted path takes the whole subtree.
fn project(value: &Value, paths: &[&[Segment]]) -> Option<Value> {
    if paths.iter().any(|p| p.is_empty()) {
        return Some(value.clone());
    }
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                let rests: Vec<&[Segment]> =
                    paths.iter().filter_map(|p| object_step(key, p)).collect();
                if rests.is_empty() {
                    continue;
                }
                if let Some(v) = project(child, &rests) {
                    out.insert(key.clone(), v);
                }
            }
            (!out.is_empty()).then_some(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for (i, child) in items.iter().enumerate() {
                let rests: Vec<&[Segment]> = paths
                    .iter()
                    .filter(|p| p[0].matches_index(i))
                    .map(|p| &p[1..])
                    .collect();
                if rests.is_empty() {
                    continue;
                }
                if let Some(v) = project(child, &rests) {
                    out.resize(i, Value::Null);
                    out.push(v);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        _ => None,
    }
}

/// Where a path goes after stepping into object key `key`, if it matches: the
/// first segment against the key (exact, tail, prefix, `*`), or several
/// segments spelling out a dotted key (`list_row.sel`).
fn object_step<'a>(key: &str, path: &'a [Segment]) -> Option<&'a [Segment]> {
    if path[0].matches_key(key) {
        return Some(&path[1..]);
    }
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() > 1 && parts.len() <= path.len() {
        let spelled = parts
            .iter()
            .zip(path)
            .all(|(part, seg)| matches!(seg, Segment::Name(n) if n == part));
        if spelled {
            return Some(&path[parts.len()..]);
        }
    }
    None
}

/// Which of a panel's observed values `GET /state` should report.
///
/// The unfiltered map is every binding the script's last good frame made —
/// every colour constant, every seeded list, and every intermediate that
/// re-derives it — which for a real app runs to hundreds of keys and makes the
/// response unreadable. `?values=a,b,c` and `?values_prefix=obs_` narrow it;
/// `?values=none` drops it. Both selectors may be given, and both accept a
/// comma-separated list; a key matches if *any* selector matches it.
///
/// These are aliases in the [`Select`] vocabulary: `values=sel` is the
/// [`Segment::Name`] rule and `values_prefix=obs_` the [`Segment::Prefix`]
/// rule, applied to the `values` map in place (the rest of `/state` is kept).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueFilter {
    /// Names and prefixes; a key survives if any matches it.
    selectors: Vec<Segment>,
    /// `?values=none`: report an empty map.
    drop_all: bool,
}

impl ValueFilter {
    /// No selector given: report everything, as `/state` always has.
    pub fn is_all(&self) -> bool {
        !self.drop_all && self.selectors.is_empty()
    }

    /// Whether an observed key survives the filter.
    pub fn matches(&self, key: &str) -> bool {
        if self.is_all() {
            return true;
        }
        !self.drop_all && self.selectors.iter().any(|s| s.matches_key(key))
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
                "values" => filter.selectors.extend(items().map(Segment::Name)),
                "values_prefix" => filter.selectors.extend(items().map(Segment::Prefix)),
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
///
/// A command that changes state ([`DebugCmd::changes_state`]) replies with a
/// small receipt by default — for the input endpoints that is
/// [`INPUT_ACK_SELECT`], a projection of the `/state` snapshot. Given
/// `?select=`, it instead replies with that projection of the whole snapshot
/// taken after the command ran (and after panels settled), with its own
/// receipt fields laid over the snapshot's top level: one round trip for
/// "press a key, then read what it did".
pub enum DebugCmd {
    /// Editor + panel state. `values` narrows each panel's observed-value map
    /// (see [`ValueFilter`]); the default reports all of it. `output` says how
    /// the accumulated `print(...)` lines are read (see [`OutputRead`]).
    State {
        values: ValueFilter,
        output: OutputRead,
    },
    /// One capture of the settled frame (`GET /capture`, and its aliases
    /// `/scene` = `format=json`, `/screenshot` = the frontend's native raster).
    /// The pane check, the settle, and the frame stamp are shared by every
    /// format; only the serialization differs.
    Capture {
        /// `?pane=<n>`: crop a raster to that pane's rect, or restrict a JSON
        /// dump to it and rebase every coordinate onto the pane's origin, so
        /// the two line up without the client doing the arithmetic.
        pane: Option<usize>,
        /// `?format=png|json|text`. `None` is the frontend's native raster:
        /// PNG, or the character grid under `--term`.
        format: Option<CaptureFormat>,
        /// `?find=text:Save` (JSON only): reply with only the matching
        /// primitives, each with its measured `rect` and `center` — a locator,
        /// so a test can click a label instead of hard-coding where it was
        /// last drawn.
        find: Option<SceneFind>,
    },
    /// Advance every panel by `n` frames of `dt` seconds each, ignoring the
    /// sleep/wake window — deterministic panel time for animation and game
    /// tests, which otherwise have to inject a no-op key per frame.
    Tick {
        n: u32,
        dt: f64,
        /// Whether these frames also advance the script clock `time()` reads,
        /// by exactly `dt` each (the default). See [`crate::app::App::advance_panels`].
        advance_clock: bool,
    },
    /// Reseed every panel's `random()` stream, so a script that generates
    /// placeholder content draws the same content on two runs.
    Seed {
        seed: u64,
    },
    /// Restart every file-backed panel from source, discarding Petal `state`.
    PanelReset,
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
    /// The core answers it from the frontend's listing (only the frontend
    /// knows the window registry) via [`crate::app::Capture::windows`].
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

/// The input endpoints' default acknowledgment, as a `?select=` over the
/// `/state` snapshot: `POST /key` with no `select=` replies exactly what
/// `POST /key?select=focus,cursor,selection` would (bar the settle), where the
/// top-level `cursor` / `selection` are the focused pane's.
// Read only by the test pinning `input_ack` to it; it names the contract.
#[cfg_attr(not(test), allow(dead_code))]
pub const INPUT_ACK_SELECT: &str = "focus,cursor,selection";

impl DebugCmd {
    /// Whether this command acts rather than reads — the commands whose
    /// `?select=` projects the post-command `/state` snapshot. Reads (`/state`,
    /// `/capture`, `/windows`, `/menu` listing, `/frame`, `/buffer`) project
    /// their own reply instead.
    pub fn changes_state(&self) -> bool {
        match self {
            DebugCmd::Key { .. }
            | DebugCmd::Text { .. }
            | DebugCmd::Command { .. }
            | DebugCmd::Menu { .. }
            | DebugCmd::Mouse { .. }
            | DebugCmd::Theme { .. }
            | DebugCmd::Tick { .. }
            | DebugCmd::Seed { .. }
            | DebugCmd::PanelReset => true,
            DebugCmd::State { .. }
            | DebugCmd::Capture { .. }
            | DebugCmd::Frame { .. }
            | DebugCmd::BufferText { .. }
            | DebugCmd::MenuList
            | DebugCmd::Windows => false,
        }
    }
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
    /// Reply with the post-command `/state` snapshot (with the command's
    /// receipt laid over it) rather than the receipt alone. Set for a
    /// state-changing command that carries `?select=`; the connection then
    /// projects the snapshot. See [`DebugCmd::changes_state`].
    pub snapshot: bool,
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
    // `?select=` is peeled off the same way: it projects whatever JSON the
    // endpoint replies with, so no route has to know about it.
    let target = parse_target(&path)
        .and_then(|(p, window)| parse_select(&p).map(|(p, select)| (p, window, select)));
    let (route_path, window, select) = match target {
        Ok(target) => target,
        Err((status, msg)) => {
            return respond_json(&mut writer, status, &json!({"ok": false, "error": msg}));
        }
    };
    let project = |value: Value| match &select {
        Some(select) => select.apply(&value),
        None => value,
    };
    // `/version` is a pure constant — answer it here rather than through the
    // event loop, so a client can still ask what this build is while the app is
    // busy (or wedged), which is exactly when it wants to know.
    if is_static_endpoint(&method, &route_path) {
        return respond_json(&mut writer, 200, &project(crate::version::report_json()));
    }

    let cmd = match route(&method, &route_path, &body) {
        Ok(cmd) => cmd,
        Err((status, msg)) => {
            return respond_json(&mut writer, status, &json!({"ok": false, "error": msg}));
        }
    };

    let (tx, rx) = mpsc::channel();
    let snapshot = select.is_some() && cmd.changes_state();
    if !sink.send(DebugRequest {
        cmd,
        window,
        snapshot,
        reply: tx,
    }) {
        return respond_json(
            &mut writer,
            500,
            &json!({"ok": false, "error": "event loop has exited"}),
        );
    }
    match rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(Reply::Json(value))) => respond_json(&mut writer, 200, &project(value)),
        Ok(Ok(_)) if select.is_some() => respond_json(
            &mut writer,
            400,
            &json!({"ok": false, "error": "select= applies only to JSON replies"}),
        ),
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

/// How `GET /state` should read the accumulated script `print(...)` lines.
///
/// The read used to be a *drain*, which quietly made `/state` single-reader:
/// two pollers each saw part of a panel's output and neither saw all of it, so
/// an observer could not run alongside a driver. Draining is still the default
/// (a harness that polls in a loop wants only what is new), but it is now one
/// of three explicit modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputRead {
    /// `?output=new` (the default): everything not yet handed to a draining
    /// read, and it moves the cursor.
    #[default]
    Drain,
    /// `?output=all`: the whole retained buffer, leaving the cursor alone.
    All,
    /// `?output=<n>`: everything from absolute line `n` on, leaving the cursor
    /// alone — how a second reader resumes from where it got to. The cursor to
    /// pass next time comes back as `script.output_next`.
    From(u64),
    /// Everything a draining read would return, *without* moving the cursor —
    /// how a command's `?select=` snapshot reports output without stealing it
    /// from the next `/state` poll. Not reachable from `?output=`.
    Peek,
}

impl OutputRead {
    fn from_query(query: &[(String, String)]) -> Result<OutputRead, String> {
        let Some((_, v)) = query.iter().find(|(k, _)| k == "output") else {
            return Ok(OutputRead::Drain);
        };
        match v.as_str() {
            "new" | "drain" | "" => Ok(OutputRead::Drain),
            "all" => Ok(OutputRead::All),
            n => n
                .parse::<u64>()
                .map(OutputRead::From)
                .map_err(|_| format!("{n:?} is not \"new\", \"all\", or a line cursor")),
        }
    }
}

/// A `/scene?find=` locator. Two forms, both over text runs:
///
/// - `text:<s>` — runs whose text, trimmed of surrounding whitespace, is
///   exactly `<s>`;
/// - `text~:<s>` — runs whose text contains `<s>`.
///
/// Only text is locatable today; the `kind:` prefix leaves room for more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneFind {
    Text { needle: String, exact: bool },
}

impl SceneFind {
    fn from_query(query: &[(String, String)]) -> Result<Option<SceneFind>, String> {
        let Some((_, v)) = query.iter().find(|(k, _)| k == "find") else {
            return Ok(None);
        };
        let (kind, needle) = v
            .split_once(':')
            .ok_or_else(|| format!("{v:?} is not <kind>:<value> (e.g. text:Save)"))?;
        let exact = match kind {
            "text" => true,
            "text~" => false,
            other => {
                return Err(format!(
                    "unknown locator kind {other:?} (expected \"text\" or \"text~\")"
                ))
            }
        };
        if needle.trim().is_empty() {
            return Err("empty text to find".to_string());
        }
        Ok(Some(SceneFind::Text {
            needle: needle.to_string(),
            exact,
        }))
    }

    /// Does a text run's text match?
    pub fn matches_text(&self, text: &str) -> bool {
        match self {
            SceneFind::Text {
                needle,
                exact: true,
            } => text.trim() == needle.trim(),
            SceneFind::Text {
                needle,
                exact: false,
            } => text.contains(needle.as_str()),
        }
    }
}

/// What `GET /capture?format=` serializes the settled frame as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureFormat {
    /// PNG bytes at physical-pixel size (`X-Garden-Frame` header).
    Png,
    /// The scene's primitives as JSON — what `/scene` has always answered.
    Json,
    /// A character grid (`text/plain`) — the terminal frontend's raster.
    Text,
}

impl CaptureFormat {
    /// The `?format=` spelling.
    pub fn name(self) -> &'static str {
        match self {
            CaptureFormat::Png => "png",
            CaptureFormat::Json => "json",
            CaptureFormat::Text => "text",
        }
    }

    fn parse(name: &str) -> Option<CaptureFormat> {
        match name {
            "png" => Some(CaptureFormat::Png),
            "json" => Some(CaptureFormat::Json),
            "text" => Some(CaptureFormat::Text),
            _ => None,
        }
    }
}

/// `GET /capture` and its aliases. `alias_format` is the format the alias
/// implies (`/scene` is JSON); an explicit `?format=` must agree with it.
fn route_capture(
    query: &[(String, String)],
    path: &str,
    alias_format: Option<CaptureFormat>,
) -> Result<DebugCmd, (u16, String)> {
    let explicit = query
        .iter()
        .find(|(k, _)| k == "format")
        .map(|(_, v)| {
            CaptureFormat::parse(v).ok_or_else(|| {
                (
                    400,
                    format!("bad format={v:?} in {path} (expected png, json, or text)"),
                )
            })
        })
        .transpose()?;
    let find =
        SceneFind::from_query(query).map_err(|err| (400, format!("bad find= in {path}: {err}")))?;
    let format = match (alias_format, explicit) {
        (Some(a), Some(e)) if a != e => {
            return Err((
                400,
                format!(
                    "{path} is always format={}; use /capture for format={}",
                    a.name(),
                    e.name()
                ),
            ))
        }
        (a, e) => e.or(a),
    };
    // A locator narrows primitives, so it only means something for JSON; asking
    // for it alone implies JSON rather than being silently ignored by a raster.
    let format = match (&find, format) {
        (Some(_), None) => Some(CaptureFormat::Json),
        (Some(_), Some(f)) if f != CaptureFormat::Json => {
            return Err((400, format!("find= needs format=json in {path}")));
        }
        (_, f) => f,
    };
    Ok(DebugCmd::Capture {
        pane: pane_selector(query, path)?,
        format,
        find,
    })
}

/// The `?pane=<n>` selector of `/capture` (and `/screenshot`, `/scene`).
fn pane_selector(query: &[(String, String)], path: &str) -> Result<Option<usize>, (u16, String)> {
    query
        .iter()
        .find(|(k, _)| k == "pane")
        .map(|(_, v)| v.parse::<usize>())
        .transpose()
        .map_err(|_| (400, format!("bad pane= in {path}")))
}

fn route(method: &str, path: &str, body: &[u8]) -> Result<DebugCmd, (u16, String)> {
    let parse_body = || -> Result<Value, (u16, String)> {
        serde_json::from_slice(body).map_err(|e| (400, format!("invalid JSON body: {e}")))
    };
    let (bare, query) = split_query(path);
    match (method, bare) {
        ("GET", "/state") => Ok(DebugCmd::State {
            values: ValueFilter::from_query(&query),
            output: OutputRead::from_query(&query)
                .map_err(|err| (400, format!("bad output= in {path}: {err}")))?,
        }),
        ("GET", "/capture") => route_capture(&query, path, None),
        ("GET", "/scene") => route_capture(&query, path, Some(CaptureFormat::Json)),
        ("GET", "/screenshot") => route_capture(&query, path, None),
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
            // Ticked frames advance the script clock by `dt` each by default:
            // the whole point of naming `dt` is that sixty frames of 0.016 are
            // 0.96 seconds of script time, however long the batch took to run.
            // `{"advance_clock": false}` keeps the wall clock for a caller that
            // wants ticks to be extra frames of real time.
            let advance_clock = v["advance_clock"].as_bool().unwrap_or(true);
            Ok(DebugCmd::Tick {
                n: n as u32,
                dt,
                advance_clock,
            })
        }
        ("POST", "/seed") => {
            let v = parse_body()?;
            let seed = v["seed"]
                .as_u64()
                .ok_or((400, "missing integer \"seed\"".to_string()))?;
            Ok(DebugCmd::Seed { seed })
        }
        ("POST", "/panel/reset") => Ok(DebugCmd::PanelReset),
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
            route_key(&key)?;
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
/// path with that parameter removed plus the target window's 1-based ordinal —
/// `None` means the focused window (the single-window default). The parameter
/// may sit anywhere in the query (`/frame?window=2&min=5`); the other
/// parameters are handed on untouched. The ordinal is 1-based, so `0`,
/// negatives, non-numbers, and a repeated `window=` are rejected 400.
pub(crate) fn parse_target(path: &str) -> Result<(String, Option<u64>), (u16, String)> {
    let (rest, values) = take_param(path, "window");
    let window = match values.as_slice() {
        [] => None,
        [value] => Some(
            value
                .parse::<u64>()
                .ok()
                .filter(|&n| n >= 1)
                .ok_or((400, format!("bad window ordinal in {path:?}: {value:?}")))?,
        ),
        _ => return Err((400, format!("more than one window= in {path:?}"))),
    };
    Ok((rest, window))
}

/// Split an optional `?select=` projection off a debug path (see [`Select`]).
/// Like `window=`, it applies to every endpoint, so it is taken off before
/// routing; repeated `select=` parameters combine.
pub(crate) fn parse_select(path: &str) -> Result<(String, Option<Select>), (u16, String)> {
    let (rest, values) = take_param(path, "select");
    if values.is_empty() {
        return Ok((rest, None));
    }
    Select::parse(&values.join(","))
        .map(|select| (rest, Some(select)))
        .map_err(|err| (400, format!("bad select= in {path:?}: {err}")))
}

/// Remove every `name=` parameter from a path's query, wherever it sits,
/// returning the remaining path (other parameters kept verbatim, still
/// encoded) and the removed values, decoded.
fn take_param(path: &str, name: &str) -> (String, Vec<String>) {
    let Some((bare, query)) = path.split_once('?') else {
        return (path.to_string(), Vec::new());
    };
    let mut kept = Vec::new();
    let mut taken = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(k) == name {
            taken.push(percent_decode(v));
        } else {
            kept.push(pair);
        }
    }
    if kept.is_empty() {
        (bare.to_string(), taken)
    } else {
        (format!("{bare}?{}", kept.join("&")), taken)
    }
}

/// Split a debug path into its bare route and its decoded query parameters.
/// Runs after [`parse_target`] and [`parse_select`] have taken the `window=`
/// and `select=` parameters off, so what remains is the endpoint's own. Percent escapes are decoded (`%2C`
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
/// consumes. Named keys come from [`crate::vim::NAMED_KEYS`] (case-insensitive,
/// plus the `enter` and `esc` aliases); any other single character maps to
/// `Key::Char`.
pub fn parse_key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "enter" => "return",
        "esc" => "escape",
        other => other,
    };
    if let Some((_, key)) = crate::vim::NAMED_KEYS.iter().find(|(n, _)| *n == canonical) {
        return Some(*key);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(Key::Char(c)),
        _ => None,
    }
}

/// Parse a `/key` name at the route boundary: an unknown name is a 400 carrying
/// petal-ui's canonical vocabulary, not a press that silently drives nothing.
/// A canonical name Garden has no [`Key`] for (`f1`, `insert`, …) says so
/// rather than claiming the name is misspelled.
fn route_key(name: &str) -> Result<Key, (u16, String)> {
    parse_key(name).ok_or_else(|| {
        let msg = if petal_ui::input::is_canonical_key(name) {
            format!(
                "`{name}` is a canonical key name Garden cannot deliver; named keys it accepts: {}, or any single character",
                crate::vim::NAMED_KEYS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            )
        } else {
            petal_ui::input::non_canonical_key_error(name)
        };
        (400, msg)
    })
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
            Ok(DebugCmd::State { values, .. }) => values,
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
            Ok(DebugCmd::Tick { n, dt, .. }) => {
                assert_eq!(n, 60);
                assert!((dt - 0.016).abs() < 1e-9);
            }
            _ => panic!("POST /tick must route to Tick"),
        }
        // An empty body is one frame at 60fps — the common "step once" call.
        match route("POST", "/tick", b"") {
            Ok(DebugCmd::Tick { n, dt, .. }) => {
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

    /// `window=` used to have to be the sole or last parameter, because it was
    /// string-scanned off before the query was parsed. It may now sit anywhere.
    #[test]
    fn window_param_may_sit_anywhere_in_the_query() {
        let (path, window) = parse_target("/frame?window=2&min=5").expect("window first");
        assert_eq!((path.as_str(), window), ("/frame?min=5", Some(2)));
        match route("GET", &path, b"") {
            Ok(DebugCmd::Frame { min }) => assert_eq!(min, Some(5)),
            _ => panic!("{path} must still route to Frame"),
        }
        let (path, window) =
            parse_target("/state?values=a&window=3&output=all").expect("window mid-query");
        assert_eq!(
            (path.as_str(), window),
            ("/state?values=a&output=all", Some(3))
        );
        // The old trailing form still works, and a repeat is ambiguous.
        let (path, window) = parse_target("/frame?min=5&window=2").expect("window last");
        assert_eq!((path.as_str(), window), ("/frame?min=5", Some(2)));
        assert!(parse_target("/state?window=1&window=2").is_err());
    }

    /// `?select=` projects any JSON reply onto dotted paths, keeping each field
    /// where it was so the projected reply reads like the full one.
    #[test]
    fn select_projects_json_by_path() {
        let state = json!({
            "ok": true,
            "focus": 1,
            "cell": {"width": 8, "height": 17},
            "panes": [
                {"index": 0, "cursor": {"line": 2, "col": 4}, "panel": null},
                {"index": 1, "cursor": {"line": 9, "col": 0},
                 "panel": {"values": {"list_row.sel": 3, "obs_a": 1, "obs_b": 2, "palette": [1]}}},
            ],
        });
        let sel = |text: &str| Select::parse(text).expect(text).apply(&state);

        assert_eq!(
            sel("panes.0.cursor,focus"),
            json!({"ok": true, "focus": 1, "panes": [{"cursor": {"line": 2, "col": 4}}]})
        );
        // Array indices are preserved: element 1 stays at index 1.
        let second = sel("panes.1.cursor.line");
        assert_eq!(second["panes"][1]["cursor"]["line"], 9);
        assert_eq!(second["panes"][0], Value::Null);
        // Wildcard over elements; tail-matching and spelled-out dotted keys.
        let values = sel("panes.*.panel.values.sel");
        assert_eq!(
            values,
            json!({"ok": true, "panes": [null, {"panel": {"values": {"list_row.sel": 3}}}]})
        );
        assert_eq!(sel("panes.1.panel.values.list_row.sel"), values);
        // Prefix match, the `values_prefix=` rule.
        assert_eq!(
            sel("panes.1.panel.values.obs_*")["panes"][1]["panel"]["values"],
            json!({"obs_a": 1, "obs_b": 2})
        );
        // Unknown paths are absent, not errors; a whole subtree comes through.
        assert_eq!(
            sel("nope,cell"),
            json!({"ok": true, "cell": {"width": 8, "height": 17}})
        );

        for bad in ["", "a..b", "pa*nes", "*x"] {
            assert!(Select::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn select_param_is_taken_off_before_routing() {
        let (path, select) =
            parse_select("/state?select=focus%2Ccell.height&output=all").expect("valid select");
        assert_eq!(path, "/state?output=all");
        assert_eq!(select, Some(Select::parse("focus,cell.height").unwrap()));
        let (path, select) = parse_select("/state").expect("no select");
        assert_eq!((path.as_str(), select), ("/state", None));
        assert!(parse_select("/state?select=a..b").is_err());
        assert!(parse_select("/state?select=").is_err());
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
        match route("GET", &path, b"") {
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
        match route("POST", &path, br#"{"key":"s","mods":["cmd"]}"#) {
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

    /// `/key` names are checked where the request is parsed: canonical names,
    /// their aliases and single characters route; anything else is a 400 that
    /// lists the vocabulary instead of a press that does nothing.
    #[test]
    fn key_route_rejects_unknown_names() {
        for ok in [
            "pagedown", "PageDown", "enter", "return", "esc", "space", "K", "-",
        ] {
            let body = format!(r#"{{"key":"{ok}"}}"#);
            assert!(
                route("POST", "/key", body.as_bytes()).is_ok(),
                "{ok} should route"
            );
        }
        match route("POST", "/key", br#"{"key":"ArrowLeft"}"#) {
            Err((400, msg)) => {
                assert!(msg.contains("not a canonical key name"), "{msg}");
                assert!(msg.contains("pagedown"), "lists the vocabulary: {msg}");
            }
            other => panic!("ArrowLeft should be a 400, got ok={}", other.is_ok()),
        }
        match route("POST", "/key", br#"{"key":"f1"}"#) {
            Err((400, msg)) => assert!(msg.contains("cannot deliver"), "{msg}"),
            other => panic!("f1 should be a 400, got ok={}", other.is_ok()),
        }
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

/// Crop a captured frame to `rect` (logical pixels at `scale`), returning
/// `(width, height, rgba)` for [`encode_png`].
///
/// `GET /screenshot?pane=<n>` is this plus a pane rect. Every harness that
/// wanted one pane's pixels reimplemented the crop, and an off-by-one origin
/// there is silent: the image looks plausible and every measurement taken from
/// it is shifted. The clamp to the capture's bounds means a pane rect that
/// runs past the window (a stale viewport) yields a smaller image rather than
/// an error or a panic.
pub fn crop_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    rect: garden_render::Rect,
    scale: f64,
) -> (u32, u32, Vec<u8>) {
    let px = |v: f32| (v as f64 * scale).round().max(0.0) as u32;
    let x0 = px(rect.x).min(width);
    let y0 = px(rect.y).min(height);
    let w = px(rect.w).min(width - x0);
    let h = px(rect.h).min(height - y0);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y0..y0 + h {
        let start = ((row * width + x0) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (w * 4) as usize]);
    }
    (w, h, out)
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
