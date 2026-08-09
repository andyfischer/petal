//! gpp — the Garden Pane Protocol.
//!
//! GPP lets a child process drive the text content of a Garden pane. The host
//! (garden-app) reuses its existing editor view as a passive render surface;
//! the subprocess pushes full-screen content and the host forwards a subscribed
//! set of keystrokes back. This crate is the shared contract depended on by
//! both the host and every GPP client (the first being a directory browser).
//!
//! # Transport
//!
//! Newline-delimited JSON: exactly one compact [`Envelope`] per line, with no
//! embedded newlines, written to the child's stdin (host -> client) and read
//! from its stdout (client -> host). stderr is reserved for the client's own
//! logging. Each [`Envelope`] is JSON-RPC 2.0 shaped: requests carry an `id`,
//! a `method`, and `params`; notifications drop the `id`; responses carry an
//! `id` and a `result` (and no `method`). Use [`write_message`] and
//! [`read_message`] for framing.
//!
//! # Message flow
//!
//! 1. The host spawns the child and writes an `initialize` request (id 1,
//!    [`InitializeParams`]). The client MUST reply with an `initialize`
//!    response carrying [`InitializeResult`] before sending any notification.
//! 2. After responding the client SHOULD immediately send a `render`
//!    notification with its initial content.
//! 3. Thereafter:
//!    - Client -> host notifications: [`method::RENDER`] (replace full content),
//!      [`method::SET_KEYMAP`] (replace the forwarded-key set),
//!      [`method::OPEN_PATH`] (host swaps this pane for a normal editor on the
//!      path and shuts the subprocess down), and [`method::SET_STATUS`].
//!    - Host -> client notifications: [`method::KEY`] (a subscribed key pressed
//!      while focused), [`method::RESIZE`], and [`method::SHUTDOWN`] (the client
//!      exits). The client also exits on stdin EOF.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// JSON-RPC method names used on the wire.
pub mod method {
    /// Host -> client request: hand the client its pane id, size, args, and cwd.
    pub const INITIALIZE: &str = "initialize";
    /// Host -> client notification: a subscribed key was pressed while focused.
    pub const KEY: &str = "key";
    /// Host -> client notification: the pane was resized.
    pub const RESIZE: &str = "resize";
    /// Host -> client notification: a mouse click landed on a content row
    /// (sent only when the client opted in via `InitializeResult::mouse`).
    pub const MOUSE: &str = "mouse";
    /// Host -> client notification: the client should exit.
    pub const SHUTDOWN: &str = "shutdown";
    /// Client -> host notification: replace the full pane content.
    pub const RENDER: &str = "render";
    /// Client -> host notification: replace the set of forwarded keys.
    pub const SET_KEYMAP: &str = "setKeymap";
    /// Client -> host notification: replace this pane with an editor on a path.
    pub const OPEN_PATH: &str = "openPath";
    /// Client -> host notification: set the pane status text.
    pub const SET_STATUS: &str = "setStatus";

    // --- Panel-mode messages (the script-push protocol iteration; see
    // docs/gpp.md) ---

    /// Client -> host notification: (re)load this pane's Petal UI script. A
    /// later push hot-reloads (preserving the panel's `state`).
    pub const SET_SCRIPT: &str = "setScript";
    /// Host -> client request: the pushed script called `query(kind, arg)` and
    /// the host has no cached value; the client should fetch it and answer with
    /// a `queryResult` response.
    pub const QUERY: &str = "query";
    /// Client -> host response: the answer (value or error) to a `query` request.
    pub const QUERY_RESULT: &str = "queryResult";
    /// Client -> host notification: proactively drop a cached `(kind, arg)` so
    /// the script re-`query`s it (the client detected fresh data).
    pub const INVALIDATE: &str = "invalidate";
    /// Host -> client notification: the pushed script called `emit(event, arg)`
    /// — a fire-and-forget user-intent signal the client acts on.
    pub const EMIT: &str = "emit";
    /// Host -> client request: run an effectful `mutation(name, arg)` on the
    /// script's behalf and answer with a `mutateResult` response. Uncached (unlike
    /// `query`) and response-carrying (unlike `emit`). The host issues the built-in
    /// `navigate` mutation to fetch a screen's UI source when a subprocess panel
    /// navigates (the host owns the history stack; the client supplies the source).
    pub const MUTATE: &str = "mutate";
}

/// A JSON-RPC error object.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// The on-the-wire JSON-RPC envelope.
///
/// Requests have `id` + `method` + `params`; notifications have `method` +
/// `params` (no `id`); responses have `id` + `result` (no `method`). Absent
/// fields are skipped during serialization so the JSON matches the shape of
/// each message kind.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Envelope {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<RpcError>,
}

fn jsonrpc_version() -> String {
    "2.0".into()
}

impl Envelope {
    /// Build a request envelope (`id` + `method` + `params`).
    pub fn request(id: u64, method: &str, params: impl Serialize) -> Envelope {
        Envelope {
            jsonrpc: jsonrpc_version(),
            id: Some(id),
            method: Some(method.to_string()),
            params: Some(serde_json::to_value(params).expect("params serialize")),
            result: None,
            error: None,
        }
    }

    /// Build a notification envelope (`method` + `params`, no `id`).
    pub fn notification(method: &str, params: impl Serialize) -> Envelope {
        Envelope {
            jsonrpc: jsonrpc_version(),
            id: None,
            method: Some(method.to_string()),
            params: Some(serde_json::to_value(params).expect("params serialize")),
            result: None,
            error: None,
        }
    }

    /// Build a response envelope (`id` + `result`, no `method`).
    pub fn response(id: u64, result: impl Serialize) -> Envelope {
        Envelope {
            jsonrpc: jsonrpc_version(),
            id: Some(id),
            method: None,
            params: None,
            result: Some(serde_json::to_value(result).expect("result serialize")),
            error: None,
        }
    }

    /// Is this a notification (or request) with the given method name?
    pub fn is_method(&self, m: &str) -> bool {
        self.method.as_deref() == Some(m)
    }

    /// Deserialize [`Self::params`] into a typed struct.
    pub fn params_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        let value = self.params.clone().unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value)
    }

    /// Deserialize [`Self::result`] into a typed struct.
    pub fn result_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        let value = self.result.clone().unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value)
    }
}

/// Params for the `initialize` request (host -> client).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub pane_id: u64,
    pub rows: u32,
    pub cols: u32,
    pub args: Vec<String>,
    pub cwd: String,
}

/// How much of the host pane's default behavior a client takes over.
///
/// GPP takeover is **layered and opt-in**: a client chooses how much of
/// Garden's built-in pane behavior it wants to replace. Heavier levels forward
/// more input to the client and leave less host default behavior in place.
///
/// Two things are reserved at *every* level and can never be taken over:
/// - the **host command bar** (`:` opens Garden's ex command line), and
/// - the **global host chords** — the Cmd/Ctrl editing shortcuts, the `Ctrl+W`
///   window-navigation prefix, and `Cmd`/`Ctrl`+`Q` quit.
///
/// So even an "almost-full" takeover still lets the user run `:w`, `:q`, `:E`,
/// and friends, and still move focus between panes. Adding a heavier layer is
/// the only thing a future protocol revision needs to do — `Keymap` stays the
/// default, so older clients keep their behavior.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Takeover {
    /// Lightest. The host owns the pane; it forwards only the keys the client
    /// lists in its `keymap`, and runs its own default (scrolling the passive
    /// view) for every other key. Most of Garden's pane behavior stays. This is
    /// the default, and what navigation-style clients (directory/git browsers)
    /// want.
    #[default]
    Keymap,
    /// Almost-full. The host forwards *every* key to the client except the
    /// reserved set (command bar + global host chords); the `keymap` is then
    /// irrelevant. The client drives all navigation, scrolling, and editing
    /// itself. For full-screen TUI-style clients.
    Keyboard,
}

/// Which rendering model a client drives its pane with.
///
/// This is the top-level fork of the protocol: a [`Lines`](ClientMode::Lines)
/// client pushes text ([`RenderParams`]) and the host forwards a subscribed key
/// set (the original GPP model — [`Takeover`], `keymap`, and `mouse` all apply
/// to it). A [`Panel`](ClientMode::Panel) client instead pushes a **Petal UI
/// script** ([`method::SET_SCRIPT`]) the host runs in its in-process panel
/// runtime, and drives it by answering [`method::QUERY`] requests over the pipe;
/// the host uses the panel input policy (every non-reserved key/mouse/wheel is
/// forwarded to the script), so `takeover`/`keymap` are ignored. See
/// `docs/dev/gpp-protocol-iteration-20260713.md`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ClientMode {
    /// The client pushes text lines (`render`). The default, so a client that
    /// omits `mode` keeps the original behavior.
    #[default]
    Lines,
    /// The client pushes a Petal UI script (`setScript`) and answers `query`
    /// requests. The host runs the script; input flows to it directly.
    Panel,
}

/// Result of the `initialize` request (client -> host): the pane's display name,
/// the rendering model it drives, the takeover level it wants (Lines mode), and
/// the initial set of keys it wants forwarded (Lines mode).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub name: String,
    /// Which rendering model the client drives. Absent on the wire means
    /// [`ClientMode::Lines`], so older clients are unaffected. When `Panel`, the
    /// `takeover`/`keymap`/`mouse` fields below are ignored and the client is
    /// expected to send a [`method::SET_SCRIPT`] right after this response.
    #[serde(default)]
    pub mode: ClientMode,
    /// How much host behavior to take over. Absent on the wire means
    /// [`Takeover::Keymap`] (the lightest level), so older clients are
    /// unaffected.
    #[serde(default)]
    pub takeover: Takeover,
    #[serde(default)]
    pub keymap: Vec<String>,
    /// Opt in to `mouse` notifications: clicks landing on this pane's content
    /// are forwarded (see [`MouseParams`]) instead of moving the passive view's
    /// cursor. Absent on the wire means `false`, so older clients keep today's
    /// host-side click behavior. Changeable later via [`SetKeymapParams::mouse`].
    #[serde(default)]
    pub mouse: bool,
}

/// The semantic style of one [`StyleSpan`], mapped to a theme color by the
/// host. A deliberately small palette that covers the browsers' needs;
/// unknown strings fail to decode, so the host drops such a `render`'s styles
/// rather than guessing.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum StyleKind {
    /// An added diff line (rendered green-ish).
    Added,
    /// A removed diff line (rendered red-ish).
    Removed,
    /// A diff hunk header `@@ … @@` (rendered cyan/blue-ish).
    Hunk,
    /// A heading (a commit header, a section title) — an accent color.
    Title,
    /// De-emphasized text (metadata, dates, authors) — comment gray.
    Dim,
    /// A review-comment accent (an author name, a thread marker) — a distinct
    /// accent so inline comments read apart from diff text.
    Comment,
}

/// One styled run within a rendered line: char columns `[start, end)` (end
/// exclusive) drawn in the theme color the host maps [`Self::style`] to.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StyleSpan {
    pub start: usize,
    pub end: usize,
    pub style: StyleKind,
}

/// The semantic kind of a [`BgSpan`] — a background tint the host paints behind
/// a run of a rendered line, mapped to a theme color. Backgrounds are the
/// primitive that lets a rich client (e.g. the PR browser) paint diff rows and
/// inline comment blocks as tinted bands, which foreground [`StyleSpan`]s alone
/// cannot. Unknown strings fail to decode, so the host drops a `render`'s
/// backgrounds rather than guessing.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum BgKind {
    /// An added diff row — a translucent green band.
    Added,
    /// A removed diff row — a translucent red band.
    Removed,
    /// An inline review-comment block — a distinct translucent band.
    Comment,
    /// A selected / active row — the host's selection tint.
    Selected,
    /// A section header band (a file header, a column heading).
    Header,
}

/// One background run within a rendered line: char columns `[start, end)` (end
/// exclusive) filled with the theme color the host maps [`Self::kind`] to,
/// drawn behind the line's text. Column-scoped (not whole-line) so a client
/// can tint just one region of a composed multi-column layout — e.g. tint the
/// right-hand diff/comment area while leaving a left-hand file list untinted.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BgSpan {
    pub start: usize,
    pub end: usize,
    pub kind: BgKind,
}

/// Params for a `render` notification (client -> host): replaces the full pane
/// content. `cursor_line` selects/highlights a 0-based row.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RenderParams {
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Optional per-line style spans, indexed like [`Self::lines`]. `None` (or
    /// a line with no entry / an empty span list) keeps the plain default
    /// rendering, so clients that never send styles behave exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub styles: Option<Vec<Vec<StyleSpan>>>,
    /// Optional per-line background spans, indexed like [`Self::lines`]. Each
    /// [`BgSpan`] paints a tinted band behind a char-column range of its line,
    /// drawn under the text (and under selection/caret). `None` (or a line with
    /// no entry) leaves that line's background plain, so clients that never send
    /// backgrounds behave exactly as before. This is what makes a rich diff /
    /// review UI possible over GPP: added/removed rows and inline comment blocks
    /// become tinted regions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounds: Option<Vec<Vec<BgSpan>>>,
}

/// Params for a `setKeymap` notification (client -> host): replace the set of
/// keys the host forwards to this client, and optionally change the takeover
/// level at runtime (e.g. a client that switches between a navigation view and
/// a full-input view).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetKeymapParams {
    pub keys: Vec<String>,
    /// A new takeover level, or `None` to leave it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takeover: Option<Takeover>,
    /// A new mouse-forwarding opt-in (see [`InitializeResult::mouse`]), or
    /// `None` to leave it unchanged — mirroring how `takeover` works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<bool>,
}

/// Params for an `openPath` notification (client -> host).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenPathParams {
    pub path: String,
}

/// Params for a `setStatus` notification (client -> host).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetStatusParams {
    pub text: String,
}

/// Params for a `setScript` notification (client -> host, [`ClientMode::Panel`]):
/// the Petal UI source the host runs in its panel runtime. A later push with new
/// source hot-reloads the pane (the panel's `state` is preserved).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetScriptParams {
    pub source: String,
}

/// Params for a `query` request (host -> client, [`ClientMode::Panel`]): the
/// script called `query(kind, arg)` and the host had no cached value. The client
/// fetches the resource and answers with a [`QueryResult`] response carrying the
/// same request `id`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueryParams {
    pub kind: String,
    pub arg: String,
}

/// Result of a `query` request (client -> host response). `kind`/`arg` echo the
/// request so the host can key its cache without tracking request ids. Exactly
/// one of `value` (the resolved data, a `HostData`-shaped JSON tree) or `error`
/// (a failure message) should be set; both absent is treated as still-loading
/// (the host keeps waiting / the script keeps its spinner).
///
/// `cache_control` tells the host how cacheable the answer is (see
/// [`petal_query::CachePolicy`]): how long it stays fresh, whether to serve it
/// stale while revalidating, or never to cache it. Absent on the wire means the
/// historical default — [`CachePolicy::forever`](petal_query::CachePolicy::forever),
/// cached until an `invalidate` — so a client that never sends it is unchanged.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub kind: String,
    pub arg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How cacheable this answer is; `None` = fresh forever (cache until
    /// `invalidate`). Carried through to the host's query cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<petal_query::CachePolicy>,
}

/// Params for an `invalidate` notification (client -> host, [`ClientMode::Panel`]):
/// drop the cached value for `(kind, arg)` so the next frame's `query` re-requests
/// it. The client-driven counterpart of the script's own `invalidate(...)` — how a
/// client pushes fresh data (e.g. a file-watch fired).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InvalidateParams {
    pub kind: String,
    pub arg: String,
}

/// Params for an `emit` notification (host -> client, [`ClientMode::Panel`]): the
/// script called `emit(event, arg)`, a fire-and-forget user-intent signal for the
/// client to act on (the richer sibling of the dedicated `openPath`). No reply is
/// expected or possible — there is no id to answer. `event` names the intent;
/// `arg` is whatever JSON tree the script passed (string, int, record, list, …).
/// Clients that don't care about an event (or about `emit` at all) simply skip
/// the notification.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EmitParams {
    pub event: String,
    pub arg: serde_json::Value,
}

/// Params for a `mutate` request (host -> client, [`ClientMode::Panel`]): an
/// effectful call the host forwards on the script's behalf and awaits a
/// `mutateResult` for. `arg` is any JSON tree (like `emit`, unlike a `query`'s
/// `&str`). The built-in `navigate` mutation carries `{ "screen": name }`. Wire
/// shape matches `petal_query::wire::MutateParams` byte-for-byte.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MutateParams {
    pub name: String,
    pub arg: serde_json::Value,
}

/// The `mutateResult` response (client -> host, [`ClientMode::Panel`]): the
/// mutation's result. Echoes `name`; exactly one of `value`/`error` is set.
/// Carries **no** `cacheControl` — a mutation is effectful and never cached (the
/// key difference from [`QueryResult`]). For the built-in `navigate` mutation the
/// `value` is `{ "screen": name, "source": <ptl source> }`. Wire shape matches
/// `petal_query::wire::MutateResult`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MutateResult {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Params for a `key` notification (host -> client).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KeyParams {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub cmd: bool,
}

/// Params for a `resize` notification (host -> client).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResizeParams {
    pub rows: u32,
    pub cols: u32,
}

/// What kind of press a `mouse` notification reports.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum MouseKind {
    /// A single click. A double-click's first press arrives as this.
    Click,
    /// The second (or later) press of a multi-click.
    Double,
}

/// Params for a `mouse` notification (host -> client), sent only when the
/// client opted in (see [`InitializeResult::mouse`]). `line` is the 0-based
/// **content** row — scroll-adjusted, i.e. an index into the lines of the last
/// `render` — and `col` the char column on that line (clamped to its length).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MouseParams {
    pub line: usize,
    pub col: usize,
    pub kind: MouseKind,
}

/// Canonical key-name encoding shared by host and client.
///
/// Printable single chars encode as themselves (`"j"`, `"/"`, `"G"`, `" "`);
/// named keys use the exact strings in [`Key::to_name`]. Letters are
/// case-sensitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Key {
    /// Encode this key as its canonical wire name.
    pub fn to_name(self) -> String {
        match self {
            Key::Char(c) => c.to_string(),
            Key::Enter => "Enter".to_string(),
            Key::Tab => "Tab".to_string(),
            Key::Backspace => "Backspace".to_string(),
            Key::Delete => "Delete".to_string(),
            Key::Escape => "Escape".to_string(),
            Key::Up => "Up".to_string(),
            Key::Down => "Down".to_string(),
            Key::Left => "Left".to_string(),
            Key::Right => "Right".to_string(),
            Key::Home => "Home".to_string(),
            Key::End => "End".to_string(),
            Key::PageUp => "PageUp".to_string(),
            Key::PageDown => "PageDown".to_string(),
        }
    }

    /// Decode a wire name back into a [`Key`]. A single-char string maps to
    /// [`Key::Char`]; the named-key strings map to their variants; anything
    /// else returns `None`.
    pub fn from_name(s: &str) -> Option<Key> {
        match s {
            "Enter" => Some(Key::Enter),
            "Tab" => Some(Key::Tab),
            "Backspace" => Some(Key::Backspace),
            "Delete" => Some(Key::Delete),
            "Escape" => Some(Key::Escape),
            "Up" => Some(Key::Up),
            "Down" => Some(Key::Down),
            "Left" => Some(Key::Left),
            "Right" => Some(Key::Right),
            "Home" => Some(Key::Home),
            "End" => Some(Key::End),
            "PageUp" => Some(Key::PageUp),
            "PageDown" => Some(Key::PageDown),
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(Key::Char(c)),
                    _ => None,
                }
            }
        }
    }
}

/// Write one [`Envelope`] as a newline-terminated JSON line and flush.
pub fn write_message<W: std::io::Write>(w: &mut W, env: &Envelope) -> std::io::Result<()> {
    serde_json::to_writer(&mut *w, env)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// Read one newline-delimited [`Envelope`]. Returns `Ok(None)` at EOF. A line
/// that fails to parse is reported as [`std::io::ErrorKind::InvalidData`].
pub fn read_message<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Envelope>> {
    let mut line = String::new();
    let n = r.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let env = serde_json::from_str(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn reparse(env: &Envelope) -> serde_json::Value {
        let s = serde_json::to_string(env).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn request_round_trip() {
        let env = Envelope::request(
            1,
            method::INITIALIZE,
            InitializeParams {
                pane_id: 7,
                rows: 24,
                cols: 80,
                args: vec!["dir".to_string()],
                cwd: "/tmp".to_string(),
            },
        );
        let v = reparse(&env);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "initialize");
        assert!(v.get("result").is_none());
        assert!(v.get("error").is_none());

        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.is_method(method::INITIALIZE));
        let params: InitializeParams = back.params_as().unwrap();
        assert_eq!(params.pane_id, 7);
        assert_eq!(params.cols, 80);
    }

    #[test]
    fn notification_has_no_id() {
        let env = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec!["a".to_string(), "b".to_string()],
                cursor_line: Some(1),
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert!(v.get("id").is_none());
        assert_eq!(v["method"], "render");
        assert!(v.get("result").is_none());

        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.id.is_none());
        let params: RenderParams = back.params_as().unwrap();
        assert_eq!(params.cursor_line, Some(1));
    }

    #[test]
    fn response_has_no_method() {
        let env = Envelope::response(
            1,
            InitializeResult {
                name: "files".to_string(),
                mode: ClientMode::Lines,
                takeover: Takeover::Keymap,
                keymap: vec!["j".to_string(), "k".to_string()],
                mouse: false,
            },
        );
        let v = reparse(&env);
        assert_eq!(v["id"], 1);
        assert!(v.get("method").is_none());
        assert!(v.get("result").is_some());

        let back: Envelope = serde_json::from_value(v).unwrap();
        assert_eq!(back.method, None);
        let result: InitializeResult = back.result_as().unwrap();
        assert_eq!(result.name, "files");
        assert_eq!(result.keymap.len(), 2);
    }

    #[test]
    fn mutate_request_and_result_round_trip() {
        // A `mutate` request is a host->client request (id + method + params).
        let req = Envelope::request(
            9,
            method::MUTATE,
            MutateParams {
                name: "navigate".to_string(),
                arg: serde_json::json!({ "screen": "b.ptl" }),
            },
        );
        let v = reparse(&req);
        assert_eq!(v["method"], "mutate");
        assert_eq!(v["params"]["name"], "navigate");
        assert_eq!(v["params"]["arg"]["screen"], "b.ptl");
        let back: Envelope = serde_json::from_value(v).unwrap();
        let p: MutateParams = back.params_as().unwrap();
        assert_eq!(p.name, "navigate");

        // Its response is a `mutateResult` (id + result, no method); it carries
        // no cacheControl (mutations are never cached).
        let resp = Envelope::response(
            9,
            MutateResult {
                name: "navigate".to_string(),
                value: Some(serde_json::json!({ "screen": "b.ptl", "source": "SRC" })),
                error: None,
            },
        );
        let rv = reparse(&resp);
        assert!(rv.get("method").is_none());
        assert!(rv["result"].get("cacheControl").is_none());
        assert_eq!(rv["result"]["value"]["source"], "SRC");
        let rback: Envelope = serde_json::from_value(rv).unwrap();
        let r: MutateResult = rback.result_as().unwrap();
        assert_eq!(r.value.unwrap()["screen"], "b.ptl");
    }

    #[test]
    fn camel_case_field_mapping() {
        let env = Envelope::request(
            2,
            method::INITIALIZE,
            InitializeParams {
                pane_id: 3,
                rows: 10,
                cols: 40,
                args: vec![],
                cwd: ".".to_string(),
            },
        );
        let v = reparse(&env);
        assert_eq!(v["params"]["paneId"], 3);
        assert!(v["params"].get("pane_id").is_none());

        let render = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec![],
                cursor_line: Some(5),
                ..Default::default()
            },
        );
        let rv = reparse(&render);
        assert_eq!(rv["params"]["cursorLine"], 5);
        assert!(rv["params"].get("cursor_line").is_none());
    }

    #[test]
    fn takeover_defaults_to_keymap_when_absent() {
        // An initialize response from an older client carries no `takeover`
        // field; it must decode to the lightest level.
        let v: serde_json::Value = serde_json::json!({ "name": "files" });
        let result: InitializeResult = serde_json::from_value(v).unwrap();
        assert_eq!(result.takeover, Takeover::Keymap);
        assert!(result.keymap.is_empty());
    }

    #[test]
    fn takeover_encodes_kebab_case() {
        assert_eq!(serde_json::to_value(Takeover::Keymap).unwrap(), "keymap");
        assert_eq!(
            serde_json::to_value(Takeover::Keyboard).unwrap(),
            "keyboard"
        );
        let back: Takeover = serde_json::from_value(serde_json::json!("keyboard")).unwrap();
        assert_eq!(back, Takeover::Keyboard);
    }

    #[test]
    fn initialize_result_round_trips_takeover() {
        let env = Envelope::response(
            1,
            InitializeResult {
                name: "tui".to_string(),
                mode: ClientMode::Lines,
                takeover: Takeover::Keyboard,
                keymap: vec![],
                mouse: false,
            },
        );
        let v = reparse(&env);
        assert_eq!(v["result"]["takeover"], "keyboard");
        let result: InitializeResult = env.result_as().unwrap();
        assert_eq!(result.takeover, Takeover::Keyboard);
    }

    #[test]
    fn set_keymap_takeover_is_optional() {
        // Omitted on the wire when None, and decodes back to None.
        let env = Envelope::notification(
            method::SET_KEYMAP,
            SetKeymapParams {
                keys: vec!["j".into()],
                takeover: None,
                mouse: None,
            },
        );
        let v = reparse(&env);
        assert!(v["params"].get("takeover").is_none());
        let params: SetKeymapParams = env.params_as().unwrap();
        assert_eq!(params.takeover, None);

        let env = Envelope::notification(
            method::SET_KEYMAP,
            SetKeymapParams {
                keys: vec![],
                takeover: Some(Takeover::Keyboard),
                mouse: None,
            },
        );
        let params: SetKeymapParams = env.params_as().unwrap();
        assert_eq!(params.takeover, Some(Takeover::Keyboard));
    }

    #[test]
    fn set_keymap_mouse_is_optional() {
        // Omitted on the wire when None (an old client's message decodes to
        // None too), and round-trips when set — mirroring `takeover`.
        let env = Envelope::notification(
            method::SET_KEYMAP,
            SetKeymapParams {
                keys: vec![],
                takeover: None,
                mouse: None,
            },
        );
        let v = reparse(&env);
        assert!(v["params"].get("mouse").is_none());
        let params: SetKeymapParams = env.params_as().unwrap();
        assert_eq!(params.mouse, None);

        let env = Envelope::notification(
            method::SET_KEYMAP,
            SetKeymapParams {
                keys: vec![],
                takeover: None,
                mouse: Some(true),
            },
        );
        let v = reparse(&env);
        assert_eq!(v["params"]["mouse"], true);
        let params: SetKeymapParams = env.params_as().unwrap();
        assert_eq!(params.mouse, Some(true));
    }

    #[test]
    fn initialize_result_mouse_defaults_to_false_when_absent() {
        // An initialize response from an older client carries no `mouse`
        // field; it must decode to "not opted in".
        let v: serde_json::Value = serde_json::json!({ "name": "files" });
        let result: InitializeResult = serde_json::from_value(v).unwrap();
        assert!(!result.mouse);

        let env = Envelope::response(
            1,
            InitializeResult {
                name: "files".into(),
                mouse: true,
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert_eq!(v["result"]["mouse"], true);
        let back: InitializeResult = env.result_as().unwrap();
        assert!(back.mouse);
    }

    #[test]
    fn mouse_params_round_trip_with_kind_strings() {
        let env = Envelope::notification(
            method::MOUSE,
            MouseParams {
                line: 4,
                col: 7,
                kind: MouseKind::Double,
            },
        );
        let v = reparse(&env);
        assert_eq!(v["method"], "mouse");
        assert_eq!(v["params"]["line"], 4);
        assert_eq!(v["params"]["col"], 7);
        assert_eq!(v["params"]["kind"], "double");
        let back: MouseParams = env.params_as().unwrap();
        assert_eq!(
            back,
            MouseParams {
                line: 4,
                col: 7,
                kind: MouseKind::Double
            }
        );

        assert_eq!(serde_json::to_value(MouseKind::Click).unwrap(), "click");
        assert_eq!(serde_json::to_value(MouseKind::Double).unwrap(), "double");
    }

    #[test]
    fn render_styles_are_optional_and_round_trip() {
        // Absent styles serialize to nothing and decode to None — an old
        // client's render is byte-identical to before.
        let env = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec!["+new".into()],
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert!(v["params"].get("styles").is_none());
        let params: RenderParams = env.params_as().unwrap();
        assert!(params.styles.is_none());

        // Styles round-trip: per-line span lists in camelCase, kebab-case kinds.
        let styles = vec![
            vec![StyleSpan {
                start: 0,
                end: 4,
                style: StyleKind::Added,
            }],
            vec![], // a line with no styling keeps an empty entry
            vec![
                StyleSpan {
                    start: 0,
                    end: 2,
                    style: StyleKind::Hunk,
                },
                StyleSpan {
                    start: 3,
                    end: 9,
                    style: StyleKind::Dim,
                },
            ],
        ];
        let env = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec!["+new".into(), "ctx".into(), "@@ -1 +1 @@".into()],
                styles: Some(styles.clone()),
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert_eq!(v["params"]["styles"][0][0]["start"], 0);
        assert_eq!(v["params"]["styles"][0][0]["end"], 4);
        assert_eq!(v["params"]["styles"][0][0]["style"], "added");
        assert_eq!(v["params"]["styles"][2][1]["style"], "dim");
        let back: RenderParams = env.params_as().unwrap();
        assert_eq!(back.styles, Some(styles));
    }

    #[test]
    fn render_backgrounds_are_optional_and_round_trip() {
        // Absent backgrounds serialize to nothing and decode to None — an old
        // client's render is byte-identical to before.
        let env = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec!["+new".into()],
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert!(v["params"].get("backgrounds").is_none());
        let params: RenderParams = env.params_as().unwrap();
        assert!(params.backgrounds.is_none());

        // Backgrounds round-trip: per-line span lists in camelCase, kebab kinds.
        let backgrounds = vec![
            vec![BgSpan {
                start: 0,
                end: 40,
                kind: BgKind::Added,
            }],
            vec![],
            vec![
                BgSpan {
                    start: 34,
                    end: 80,
                    kind: BgKind::Comment,
                },
                BgSpan {
                    start: 0,
                    end: 33,
                    kind: BgKind::Selected,
                },
            ],
        ];
        let env = Envelope::notification(
            method::RENDER,
            RenderParams {
                lines: vec!["a".into(), "b".into(), "c".into()],
                backgrounds: Some(backgrounds.clone()),
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert_eq!(v["params"]["backgrounds"][0][0]["start"], 0);
        assert_eq!(v["params"]["backgrounds"][0][0]["kind"], "added");
        assert_eq!(v["params"]["backgrounds"][2][0]["kind"], "comment");
        assert_eq!(v["params"]["backgrounds"][2][1]["kind"], "selected");
        let back: RenderParams = env.params_as().unwrap();
        assert_eq!(back.backgrounds, Some(backgrounds));
    }

    #[test]
    fn bg_kinds_encode_as_lowercase_strings() {
        let pairs = [
            (BgKind::Added, "added"),
            (BgKind::Removed, "removed"),
            (BgKind::Comment, "comment"),
            (BgKind::Selected, "selected"),
            (BgKind::Header, "header"),
        ];
        for (kind, name) in pairs {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
            let back: BgKind = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(back, kind);
        }
        assert!(serde_json::from_value::<BgKind>(serde_json::json!("plaid")).is_err());
    }

    #[test]
    fn style_kinds_encode_as_lowercase_strings() {
        let pairs = [
            (StyleKind::Added, "added"),
            (StyleKind::Removed, "removed"),
            (StyleKind::Hunk, "hunk"),
            (StyleKind::Title, "title"),
            (StyleKind::Dim, "dim"),
            (StyleKind::Comment, "comment"),
        ];
        for (kind, name) in pairs {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
            let back: StyleKind = serde_json::from_value(serde_json::json!(name)).unwrap();
            assert_eq!(back, kind);
        }
        // An unknown style string fails to decode (the host drops the styles).
        assert!(serde_json::from_value::<StyleKind>(serde_json::json!("sparkly")).is_err());
    }

    #[test]
    fn key_name_round_trip() {
        let named = [
            Key::Enter,
            Key::Tab,
            Key::Backspace,
            Key::Delete,
            Key::Escape,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ];
        for key in named {
            let name = key.to_name();
            assert_eq!(Key::from_name(&name), Some(key));
        }

        for c in ['j', '/', 'G', ' ', 'k'] {
            let key = Key::Char(c);
            assert_eq!(key.to_name(), c.to_string());
            assert_eq!(Key::from_name(&c.to_string()), Some(key));
        }

        assert_eq!(Key::from_name(""), None);
        assert_eq!(Key::from_name("Nope"), None);
    }

    #[test]
    fn key_names_are_exact() {
        assert_eq!(Key::Enter.to_name(), "Enter");
        assert_eq!(Key::PageDown.to_name(), "PageDown");
        assert_eq!(Key::Char(' ').to_name(), " ");
    }

    #[test]
    fn write_read_round_trip() {
        let messages = vec![
            Envelope::request(
                1,
                method::INITIALIZE,
                InitializeParams {
                    pane_id: 1,
                    rows: 5,
                    cols: 5,
                    args: vec![],
                    cwd: "/".to_string(),
                },
            ),
            Envelope::notification(method::SET_STATUS, SetStatusParams { text: "ok".into() }),
        ];

        let mut buf: Vec<u8> = Vec::new();
        for env in &messages {
            write_message(&mut buf, env).unwrap();
        }
        // One line per message, no embedded newlines within a line.
        assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), 2);

        let mut cursor = Cursor::new(buf);
        let first = read_message(&mut cursor).unwrap().unwrap();
        assert!(first.is_method(method::INITIALIZE));
        let second = read_message(&mut cursor).unwrap().unwrap();
        let status: SetStatusParams = second.params_as().unwrap();
        assert_eq!(status.text, "ok");

        // EOF yields Ok(None).
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_message_eof_on_empty() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    // --- Panel-mode (script-push) protocol ---

    #[test]
    fn client_mode_defaults_to_lines_when_absent() {
        // An older client's initialize response has no `mode` field.
        let v: serde_json::Value = serde_json::json!({ "name": "files" });
        let result: InitializeResult = serde_json::from_value(v).unwrap();
        assert_eq!(result.mode, ClientMode::Lines);
    }

    #[test]
    fn client_mode_encodes_kebab_case_and_round_trips() {
        assert_eq!(serde_json::to_value(ClientMode::Lines).unwrap(), "lines");
        assert_eq!(serde_json::to_value(ClientMode::Panel).unwrap(), "panel");

        let env = Envelope::response(
            1,
            InitializeResult {
                name: "git".into(),
                mode: ClientMode::Panel,
                ..Default::default()
            },
        );
        let v = reparse(&env);
        assert_eq!(v["result"]["mode"], "panel");
        let back: InitializeResult = env.result_as().unwrap();
        assert_eq!(back.mode, ClientMode::Panel);
    }

    #[test]
    fn set_script_round_trips() {
        let env = Envelope::notification(
            method::SET_SCRIPT,
            SetScriptParams {
                source: "draw_text(\"hi\", 0, 0, 14, 255, 255, 255)".into(),
            },
        );
        let v = reparse(&env);
        assert!(v.get("id").is_none());
        assert_eq!(v["method"], "setScript");
        let back: SetScriptParams = env.params_as().unwrap();
        assert!(back.source.contains("draw_text"));
    }

    #[test]
    fn query_request_and_result_round_trip() {
        // Host -> client request carries an id.
        let req = Envelope::request(
            5,
            method::QUERY,
            QueryParams {
                kind: "commit".into(),
                arg: "abc123".into(),
            },
        );
        let v = reparse(&req);
        assert_eq!(v["id"], 5);
        assert_eq!(v["method"], "query");
        assert_eq!(v["params"]["kind"], "commit");
        assert!(req.is_method(method::QUERY));

        // Client -> host response echoes kind/arg and carries a value.
        let resp = Envelope::response(
            5,
            QueryResult {
                kind: "commit".into(),
                arg: "abc123".into(),
                value: Some(serde_json::json!({ "body": "diff…", "files": [] })),
                error: None,
                cache_control: None,
            },
        );
        let rv = reparse(&resp);
        assert_eq!(rv["id"], 5);
        assert!(rv.get("method").is_none());
        assert!(rv["result"].get("error").is_none()); // omitted when None
        assert!(rv["result"].get("cacheControl").is_none()); // omitted when None
        let back: QueryResult = resp.result_as().unwrap();
        assert_eq!(back.kind, "commit");
        assert_eq!(back.arg, "abc123");
        assert_eq!(back.value.unwrap()["body"], "diff…");
    }

    #[test]
    fn query_result_carries_cache_control_from_petal_query() {
        use std::time::Duration;
        let resp = Envelope::response(
            7,
            QueryResult {
                kind: "log".into(),
                arg: "".into(),
                value: Some(serde_json::json!({ "commits": [] })),
                error: None,
                cache_control: Some(
                    petal_query::CachePolicy::max_age(Duration::from_secs(3))
                        .stale_while_revalidate(Duration::from_secs(60)),
                ),
            },
        );
        let rv = reparse(&resp);
        assert_eq!(rv["result"]["cacheControl"]["maxAgeMs"], 3000);
        assert_eq!(
            rv["result"]["cacheControl"]["staleWhileRevalidateMs"],
            60000
        );
        // Decodes back into the shared petal-query type.
        let back: QueryResult = resp.result_as().unwrap();
        assert_eq!(back.cache_control.unwrap().max_age_ms, Some(3000));
    }

    #[test]
    fn query_result_error_path() {
        let resp = Envelope::response(
            9,
            QueryResult {
                kind: "log".into(),
                arg: "".into(),
                value: None,
                error: Some("not a git repo".into()),
                cache_control: None,
            },
        );
        let rv = reparse(&resp);
        assert!(rv["result"].get("value").is_none()); // omitted when None
        assert_eq!(rv["result"]["error"], "not a git repo");
        let back: QueryResult = resp.result_as().unwrap();
        assert!(back.value.is_none());
        assert_eq!(back.error.as_deref(), Some("not a git repo"));
    }

    #[test]
    fn invalidate_and_emit_round_trip() {
        let env = Envelope::notification(
            method::INVALIDATE,
            InvalidateParams {
                kind: "log".into(),
                arg: "".into(),
            },
        );
        let back: InvalidateParams = env.params_as().unwrap();
        assert_eq!(back.kind, "log");

        // A string arg — the common scalar case.
        let env = Envelope::notification(
            method::EMIT,
            EmitParams {
                event: "openPath".into(),
                arg: serde_json::json!("/tmp/x.rs"),
            },
        );
        let v = reparse(&env);
        assert!(v.get("id").is_none()); // fire-and-forget: a notification
        assert_eq!(v["method"], "emit");
        assert_eq!(v["params"]["event"], "openPath");
        let back: EmitParams = env.params_as().unwrap();
        assert_eq!(back.arg, serde_json::json!("/tmp/x.rs"));
    }

    #[test]
    fn emit_arg_carries_any_json_value() {
        // The arg is a full JSON tree, not just a string: ints, records, lists
        // (whatever the script passed to `emit(event, arg)`).
        for arg in [
            serde_json::json!(null),
            serde_json::json!(42),
            serde_json::json!({ "pos": 240, "axis": "x" }),
            serde_json::json!([1, "two", { "three": 3 }]),
        ] {
            let env = Envelope::notification(
                method::EMIT,
                EmitParams {
                    event: "divider".into(),
                    arg: arg.clone(),
                },
            );
            let v = reparse(&env);
            assert_eq!(v["method"], "emit");
            assert_eq!(v["params"]["arg"], arg);
            let back: EmitParams = env.params_as().unwrap();
            assert_eq!(back.event, "divider");
            assert_eq!(back.arg, arg);
        }
    }

    #[test]
    fn emit_notification_reaches_a_client_loop() {
        // The client-side shape: emit notifications arrive on stdin interleaved
        // with the requests a panel-mode app already reads. Frame two messages,
        // consume them the way the apps' `run` loops do (dispatch on `method`),
        // and check the emit surfaces as its (event, arg) pair while a loop that
        // ignores the method just skips it.
        let mut buf: Vec<u8> = Vec::new();
        write_message(
            &mut buf,
            &Envelope::notification(
                method::EMIT,
                EmitParams {
                    event: "divider".into(),
                    arg: serde_json::json!({ "pos": 240 }),
                },
            ),
        )
        .unwrap();
        write_message(
            &mut buf,
            &Envelope::notification(method::SHUTDOWN, serde_json::json!({})),
        )
        .unwrap();

        let mut cursor = Cursor::new(buf);
        let mut seen: Vec<(String, serde_json::Value)> = Vec::new();
        while let Some(env) = read_message(&mut cursor).unwrap() {
            if env.is_method(method::EMIT) {
                let p: EmitParams = env.params_as().unwrap();
                seen.push((p.event, p.arg));
            } else if env.is_method(method::SHUTDOWN) {
                break;
            }
            // Unknown methods fall through — an app that ignores `emit` keeps
            // working unchanged.
        }
        assert_eq!(
            seen,
            vec![("divider".to_string(), serde_json::json!({ "pos": 240 }))]
        );
    }
}
