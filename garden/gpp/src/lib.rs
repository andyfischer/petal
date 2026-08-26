//! gpp — the Garden Pane Protocol, version 2.
//!
//! GPP lets a child process (a **client app**) drive the content of a Garden
//! pane. The model is "web browser + web server": right after the handshake the
//! client pushes a **Petal UI script** ([`method::SET_SCRIPT`]) that the host
//! compiles and runs in its in-process panel runtime, handling all input and
//! rendering locally. The client then acts as the panel's *data server*,
//! answering [`method::QUERY`] / [`method::MUTATE`] / [`method::NAVIGATE`]
//! requests the host issues on the running script's behalf. Only data crosses
//! the pipe; the interaction loop never does.
//!
//! This crate is the **single wire definition**, depended on by both the host
//! (garden-app) and the client library (`petal-query`, which re-exports the
//! shared [`CachePolicy`]). It deliberately has no other dependencies than
//! serde/serde_json, so any client can link it alone. Protocol reference:
//! `garden/docs/gpp.md`; the client how-to is `garden/docs/writing-gpp-apps.md`.
//!
//! # Transport
//!
//! Newline-delimited JSON: exactly one compact [`Envelope`] per line, with no
//! embedded newlines, written to the child's stdin (host -> client) and read
//! from its stdout (client -> host). stderr is reserved for the client's own
//! logging. Each [`Envelope`] is JSON-RPC 2.0 shaped:
//!
//! - **request** — `id` + `method` + `params`
//! - **notification** — `method` + `params` (no `id`)
//! - **response** — `id` + `result` (success) *or* `id` + `error` (failure);
//!   never `method`, and responses correlate to their request **by `id` only**.
//!
//! Use [`write_message`] / [`read_message`] for framing.
//!
//! # Message flow
//!
//! 1. The host spawns the child and writes an `initialize` request (id 1,
//!    [`InitializeParams`] with `protocol: 2`). The client MUST reply with an
//!    `initialize` response ([`InitializeResult`], also carrying `protocol: 2`)
//!    before sending anything else, then SHOULD immediately push its UI script
//!    with a `setScript` notification. A protocol-major mismatch on either side
//!    is answered with an [`error_code::PROTOCOL_MISMATCH`] error and the
//!    session ends.
//! 2. Steady state — host -> client: `query` / `mutate` / `navigate` requests
//!    and `emit` notifications (the script's `emit(event, arg)` calls);
//!    client -> host: responses, plus `setScript` (hot reload), `invalidate`,
//!    and `emit` notifications (reserved events: [`event::OPEN_PATH`],
//!    [`event::STATUS`]).
//! 3. The session ends when the host sends `shutdown` or closes stdin (EOF).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

mod cache_control;

pub use cache_control::{CachePolicy, Freshness};

/// The protocol major version this crate speaks. Carried by both halves of the
/// `initialize` handshake; a peer reporting a different major is incompatible
/// and the session is refused with [`error_code::PROTOCOL_MISMATCH`].
pub const PROTOCOL_VERSION: u64 = 2;

/// JSON-RPC method names used on the wire.
pub mod method {
    /// Host -> client request (id 1): protocol version, pane id, size, launch
    /// args, cwd, host capabilities.
    pub const INITIALIZE: &str = "initialize";
    /// Host -> client notification: the client should exit. (It also exits on
    /// stdin EOF.)
    pub const SHUTDOWN: &str = "shutdown";
    /// Client -> host notification: (re)load the pane's Petal UI script. Sent
    /// once right after the `initialize` response; a later push hot-reloads
    /// (preserving the panel's `state`).
    pub const SET_SCRIPT: &str = "setScript";
    /// Host -> client request: the running script called `query(kind, arg)` and
    /// the host has no fresh cached value. The client answers with a
    /// [`QueryResult`] (or an [`error_code::APP`] error).
    pub const QUERY: &str = "query";
    /// Host -> client request: run the effectful `mutation(name, arg)` the
    /// script asked for and answer with a [`MutateResult`]. Uncached (unlike
    /// `query`) and response-carrying (unlike `emit`).
    pub const MUTATE: &str = "mutate";
    /// Host -> client request: the script navigated to a declared screen; serve
    /// that screen's UI source ([`NavigateResult`]). The host owns the history
    /// stack; the client owns the sources.
    pub const NAVIGATE: &str = "navigate";
    /// Notification, both directions. Host -> client: the script called
    /// `emit(event, arg)` — a fire-and-forget signal for the app to act on.
    /// Client -> host: a client-raised event for the host; see [`event`] for
    /// the reserved names. Unknown events are ignored by both sides.
    pub const EMIT: &str = "emit";
    /// Client -> host notification: drop the cached value for `(kind, arg)` so
    /// the script re-`query`s it (the client detected fresh data).
    pub const INVALIDATE: &str = "invalidate";
}

/// Reserved client -> host [`method::EMIT`] event names the host acts on.
/// Any other event is ignored by the host (reserved for future use).
pub mod event {
    /// `emit("open_path", { "path": … })` — replace this pane with a normal
    /// editor on `path`. Ends the session: the host shuts the client down.
    pub const OPEN_PATH: &str = "open_path";
    /// `emit("status", { "text": … })` — set the pane's status-bar text.
    pub const STATUS: &str = "status";
}

/// Error codes used in [`RpcError`]. The negative codes are the standard
/// JSON-RPC 2.0 ones; the small positive codes are GPP-specific.
pub mod error_code {
    /// An application-level failure: the handler for a `query` / `mutate` /
    /// `navigate` ran and failed (e.g. "not a git repo", "no such screen").
    /// The `message` is what the panel script surfaces via `error_of`.
    pub const APP: i64 = 1;
    /// The peer speaks an incompatible protocol major version (see
    /// [`crate::PROTOCOL_VERSION`]).
    pub const PROTOCOL_MISMATCH: i64 = 2;
    /// The request's method (or a mutation's name) has no handler.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The request's params did not decode.
    pub const INVALID_PARAMS: i64 = -32602;
}

/// A JSON-RPC error object, carried on a response [`Envelope`]'s `error` field.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    /// See [`error_code`].
    pub code: i64,
    /// Human-readable reason; for [`error_code::APP`] this is the message the
    /// panel script sees.
    pub message: String,
}

/// The on-the-wire JSON-RPC envelope.
///
/// Requests have `id` + `method` + `params`; notifications have `method` +
/// `params` (no `id`); responses have `id` + `result` **or** `id` + `error`
/// (no `method`). Absent fields are skipped during serialization so the JSON
/// matches the shape of each message kind.
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

    /// Build a success response envelope (`id` + `result`, no `method`).
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

    /// Build an error response envelope (`id` + `error`, no `method`).
    pub fn error_response(id: u64, code: i64, message: impl Into<String>) -> Envelope {
        Envelope {
            jsonrpc: jsonrpc_version(),
            id: Some(id),
            method: None,
            params: None,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    /// Is this a notification (or request) with the given method name?
    pub fn is_method(&self, m: &str) -> bool {
        self.method.as_deref() == Some(m)
    }

    /// Is this a response (success or error)? A response carries an `id` and no
    /// `method` — the only correlation to its request is that `id`.
    pub fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none()
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
    /// The protocol major version the host speaks (see [`PROTOCOL_VERSION`]).
    /// Absent on the wire decodes as `1` — a pre-versioning host — which a v2
    /// client refuses with [`error_code::PROTOCOL_MISMATCH`].
    #[serde(default = "protocol_v1")]
    pub protocol: u64,
    pub pane_id: u64,
    pub rows: u32,
    pub cols: u32,
    /// The launch arguments (a directory, a database path, flags) — how a
    /// client learns what to serve.
    pub args: Vec<String>,
    pub cwd: String,
    /// Freeform capability names the host supports beyond the core protocol
    /// (e.g. `"hotReload"`). Clients ignore names they don't know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

fn protocol_v1() -> u64 {
    1
}

impl InitializeParams {
    /// The first launch arg, or the pane `cwd` when none was given — the common
    /// "which directory/target do I operate on?" resolution a file/git app wants.
    pub fn repo_arg(&self) -> String {
        self.args
            .first()
            .cloned()
            .unwrap_or_else(|| self.cwd.clone())
    }
}

/// Result of the `initialize` request (client -> host).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// The protocol major version the client speaks. Absent decodes as `1` (a
    /// pre-versioning client), which a v2 host refuses cleanly (error card).
    #[serde(default = "protocol_v1")]
    pub protocol: u64,
    /// The pane's display name (titlebar/status until the drawer sets one).
    pub name: String,
    /// Freeform capability names the client supports. The host ignores names
    /// it doesn't know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Params for a `setScript` notification (client -> host): the Petal UI source
/// the host runs in its panel runtime. A later push with new source hot-reloads
/// the pane (the panel's `state` is preserved).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetScriptParams {
    pub source: String,
}

/// Params for a `query` request (host -> client): the script called
/// `query(kind, arg)` and the host had no cached value. `arg` is **any JSON
/// value** — a string, a record, a list — so composite keys need no string
/// encoding; the host caches per `(kind, arg)`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueryParams {
    pub kind: String,
    #[serde(default)]
    pub arg: serde_json::Value,
}

/// Result of a successful `query` request (client -> host response).
///
/// - `value` present — the resolved data (a JSON tree the panel runtime maps
///   onto Petal values).
/// - `value` absent — **still loading**: the client acknowledges the request
///   but the data is not ready (a background thread is working). The host
///   keeps the script's spinner up without re-requesting until the client
///   pushes an `invalidate` for the key or it is otherwise re-queried.
///
/// A *failed* query is not a `QueryResult` at all: it is an error response
/// ([`RpcError`], code [`error_code::APP`]) whose message the script reads via
/// `error_of`.
///
/// `cache` tells the host how cacheable the answer is (see [`CachePolicy`]).
/// Absent means fresh forever — cached until an `invalidate`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// How cacheable this answer is; `None` = fresh forever (cache until
    /// `invalidate`). Carried through to the host's query cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CachePolicy>,
}

/// Params for a `mutate` request (host -> client): an effectful call the host
/// forwards on the script's behalf and awaits a response for. `arg` is any JSON
/// tree. A short list of names is answered by the **host itself** and never
/// reaches the client (`open_path`, `open_project`, `open_pr`,
/// `open_file_dialog` — see `docs/gpp.md`).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MutateParams {
    pub name: String,
    #[serde(default)]
    pub arg: serde_json::Value,
}

/// Result of a successful `mutate` request (client -> host response). Carries
/// no cache policy — a mutation is effectful and never cached. A string `value`
/// is surfaced verbatim as the pane's status line. A failed mutation is an
/// error response ([`error_code::APP`]).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MutateResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// Params for a `navigate` request (host -> client): the running script
/// navigated to `screen` and the host needs that screen's UI source. `arg` is
/// the optional subject the two-argument `navigate(screen, arg)` form carried
/// (the target screen reads it back with `nav_arg()`); `Null` for the
/// one-argument form. Back/forward re-issue the restored entry's `navigate`
/// with the entry's own `arg`, so a client handler with side effects re-primes
/// each revisit.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NavigateParams {
    pub screen: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub arg: serde_json::Value,
}

/// Result of a successful `navigate` request (client -> host response): the
/// target screen's Petal UI source. A refused navigation (undeclared screen) is
/// an error response ([`error_code::APP`]).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NavigateResult {
    pub screen: String,
    pub source: String,
}

/// Params for an `emit` notification (either direction; see [`method::EMIT`]).
/// `event` names the intent; `arg` is any JSON tree. Fire-and-forget — no
/// reply is expected or possible. Unknown events are skipped by the receiver.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EmitParams {
    pub event: String,
    #[serde(default)]
    pub arg: serde_json::Value,
}

/// Params for an `invalidate` notification (client -> host): drop the cached
/// value for `(kind, arg)` so the next frame's `query` re-requests it. The
/// client-driven counterpart of the script's own `invalidate(...)` — how a
/// client pushes fresh data (e.g. a file-watch fired). `arg` must equal the
/// queried arg (same JSON value) for the keys to match.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InvalidateParams {
    pub kind: String,
    #[serde(default)]
    pub arg: serde_json::Value,
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
    use serde_json::json;
    use std::io::Cursor;

    fn reparse(env: &Envelope) -> serde_json::Value {
        let s = serde_json::to_string(env).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    // ── Handshake ───────────────────────────────────────────────────────────

    #[test]
    fn initialize_request_round_trips_with_protocol_and_capabilities() {
        let env = Envelope::request(
            1,
            method::INITIALIZE,
            InitializeParams {
                protocol: PROTOCOL_VERSION,
                pane_id: 7,
                rows: 24,
                cols: 80,
                args: vec!["dir".to_string()],
                cwd: "/tmp".to_string(),
                capabilities: vec!["hotReload".to_string()],
            },
        );
        let v = reparse(&env);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["params"]["protocol"], 2);
        assert_eq!(v["params"]["paneId"], 7);
        assert_eq!(v["params"]["capabilities"][0], "hotReload");
        assert!(v.get("result").is_none());
        assert!(v.get("error").is_none());

        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.is_method(method::INITIALIZE));
        assert!(!back.is_response());
        let params: InitializeParams = back.params_as().unwrap();
        assert_eq!(params.protocol, 2);
        assert_eq!(params.repo_arg(), "dir");
    }

    #[test]
    fn initialize_result_round_trips_and_omits_empty_capabilities() {
        let env = Envelope::response(
            1,
            InitializeResult {
                protocol: PROTOCOL_VERSION,
                name: "files".to_string(),
                capabilities: Vec::new(),
            },
        );
        let v = reparse(&env);
        assert_eq!(v["id"], 1);
        assert!(v.get("method").is_none());
        assert_eq!(v["result"]["protocol"], 2);
        assert_eq!(v["result"]["name"], "files");
        assert!(v["result"].get("capabilities").is_none());

        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.is_response());
        let result: InitializeResult = back.result_as().unwrap();
        assert_eq!(result.protocol, 2);
        assert_eq!(result.name, "files");
        assert!(result.capabilities.is_empty());
    }

    #[test]
    fn a_pre_versioning_peer_decodes_as_protocol_1() {
        // No `protocol` field on either half of the handshake means a v1 peer,
        // which a v2 implementation must detect and refuse.
        let params: InitializeParams = serde_json::from_value(json!({
            "paneId": 0, "rows": 10, "cols": 40, "args": [], "cwd": "."
        }))
        .unwrap();
        assert_eq!(params.protocol, 1);
        let result: InitializeResult = serde_json::from_value(json!({ "name": "old" })).unwrap();
        assert_eq!(result.protocol, 1);
    }

    #[test]
    fn protocol_mismatch_error_shape() {
        let env = Envelope::error_response(
            1,
            error_code::PROTOCOL_MISMATCH,
            "client speaks gpp 2, host sent 1",
        );
        let v = reparse(&env);
        assert_eq!(v["id"], 1);
        assert!(v.get("result").is_none());
        assert!(v.get("method").is_none());
        assert_eq!(v["error"]["code"], error_code::PROTOCOL_MISMATCH);
        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.is_response());
        assert_eq!(back.error.unwrap().code, error_code::PROTOCOL_MISMATCH);
    }

    // ── Query ───────────────────────────────────────────────────────────────

    #[test]
    fn query_arg_carries_any_json_value() {
        for arg in [
            json!("abc123"),
            json!(null),
            json!(42),
            json!({ "table": "users", "page": 3 }),
            json!(["a", 1]),
        ] {
            let req = Envelope::request(
                5,
                method::QUERY,
                QueryParams {
                    kind: "commit".into(),
                    arg: arg.clone(),
                },
            );
            let v = reparse(&req);
            assert_eq!(v["id"], 5);
            assert_eq!(v["method"], "query");
            assert_eq!(v["params"]["arg"], arg);
            let back: QueryParams = req.params_as().unwrap();
            assert_eq!(back.arg, arg);
        }
    }

    #[test]
    fn query_response_correlates_by_id_only() {
        // The v2 result echoes nothing: no kind, no arg — the id is the
        // correlation, full stop.
        let resp = Envelope::response(
            5,
            QueryResult {
                value: Some(json!({ "body": "diff…", "files": [] })),
                cache: None,
            },
        );
        let v = reparse(&resp);
        assert_eq!(v["id"], 5);
        assert!(v.get("method").is_none());
        assert!(v["result"].get("kind").is_none());
        assert!(v["result"].get("arg").is_none());
        assert!(v["result"].get("cache").is_none()); // omitted when None
        let back: QueryResult = resp.result_as().unwrap();
        assert_eq!(back.value.unwrap()["body"], "diff…");
    }

    #[test]
    fn query_result_carries_cache_policy() {
        use std::time::Duration;
        let resp = Envelope::response(
            7,
            QueryResult {
                value: Some(json!({ "commits": [] })),
                cache: Some(
                    CachePolicy::max_age(Duration::from_secs(3))
                        .stale_while_revalidate(Duration::from_secs(60)),
                ),
            },
        );
        let v = reparse(&resp);
        assert_eq!(v["result"]["cache"]["maxAgeMs"], 3000);
        assert_eq!(v["result"]["cache"]["staleWhileRevalidateMs"], 60000);
        let back: QueryResult = resp.result_as().unwrap();
        assert_eq!(back.cache.unwrap().max_age_ms, Some(3000));
    }

    #[test]
    fn query_failure_is_an_error_response_not_a_result_field() {
        let resp = Envelope::error_response(9, error_code::APP, "not a git repo");
        let v = reparse(&resp);
        assert!(v.get("result").is_none());
        assert_eq!(v["error"]["code"], 1);
        assert_eq!(v["error"]["message"], "not a git repo");
        let back: Envelope = serde_json::from_value(v).unwrap();
        assert!(back.is_response());
        assert_eq!(back.error.unwrap().message, "not a git repo");
    }

    #[test]
    fn still_loading_is_an_empty_result() {
        let resp = Envelope::response(3, QueryResult::default());
        let v = reparse(&resp);
        assert_eq!(v["result"], json!({}));
        let back: QueryResult = resp.result_as().unwrap();
        assert!(back.value.is_none());
        assert!(back.cache.is_none());
    }

    // ── Mutate / navigate ───────────────────────────────────────────────────

    #[test]
    fn mutate_request_and_result_round_trip() {
        let req = Envelope::request(
            9,
            method::MUTATE,
            MutateParams {
                name: "apply".to_string(),
                arg: json!({ "edits": [] }),
            },
        );
        let v = reparse(&req);
        assert_eq!(v["method"], "mutate");
        assert_eq!(v["params"]["name"], "apply");
        let p: MutateParams = req.params_as().unwrap();
        assert_eq!(p.name, "apply");

        // The success response: id + result { value }, no name echo, no cache.
        let resp = Envelope::response(9, MutateResult {
            value: Some(json!("wrote 2 files")),
        });
        let rv = reparse(&resp);
        assert!(rv.get("method").is_none());
        assert!(rv["result"].get("name").is_none());
        assert!(rv["result"].get("cache").is_none());
        assert_eq!(rv["result"]["value"], "wrote 2 files");
    }

    #[test]
    fn navigate_is_a_first_class_request() {
        // One-argument form: no `arg` on the wire.
        let req = Envelope::request(
            4,
            method::NAVIGATE,
            NavigateParams {
                screen: "detail.ptl".into(),
                arg: serde_json::Value::Null,
            },
        );
        let v = reparse(&req);
        assert_eq!(v["method"], "navigate");
        assert_eq!(v["params"]["screen"], "detail.ptl");
        assert!(v["params"].get("arg").is_none());

        // Two-argument form carries the subject.
        let req = Envelope::request(
            5,
            method::NAVIGATE,
            NavigateParams {
                screen: "detail.ptl".into(),
                arg: json!({ "id": 7 }),
            },
        );
        let p: NavigateParams = req.params_as().unwrap();
        assert_eq!(p.arg["id"], 7);

        let resp = Envelope::response(
            5,
            NavigateResult {
                screen: "detail.ptl".into(),
                source: "SRC".into(),
            },
        );
        let back: NavigateResult = resp.result_as().unwrap();
        assert_eq!(back.screen, "detail.ptl");
        assert_eq!(back.source, "SRC");

        // A refused navigation is an ordinary error response.
        let refused = Envelope::error_response(6, error_code::APP, "no such screen 'x.ptl'");
        assert!(refused.error.unwrap().message.contains("no such screen"));
    }

    // ── Notifications ───────────────────────────────────────────────────────

    #[test]
    fn set_script_round_trips() {
        let env = Envelope::notification(
            method::SET_SCRIPT,
            SetScriptParams {
                source: "draw_text(\"hi\", {x: 0, y: 0}, 14, #ffffff)".into(),
            },
        );
        let v = reparse(&env);
        assert!(v.get("id").is_none());
        assert_eq!(v["method"], "setScript");
        let back: SetScriptParams = env.params_as().unwrap();
        assert!(back.source.contains("draw_text"));
    }

    #[test]
    fn emit_carries_any_json_arg_and_no_id() {
        for arg in [
            json!(null),
            json!(42),
            json!({ "pos": 240, "axis": "x" }),
            json!([1, "two", { "three": 3 }]),
        ] {
            let env = Envelope::notification(
                method::EMIT,
                EmitParams {
                    event: "divider".into(),
                    arg: arg.clone(),
                },
            );
            let v = reparse(&env);
            assert!(v.get("id").is_none()); // fire-and-forget
            assert_eq!(v["method"], "emit");
            assert_eq!(v["params"]["arg"], arg);
            let back: EmitParams = env.params_as().unwrap();
            assert_eq!(back.event, "divider");
            assert_eq!(back.arg, arg);
        }
    }

    #[test]
    fn reserved_client_emit_events_have_documented_shapes() {
        let open = Envelope::notification(
            method::EMIT,
            EmitParams {
                event: event::OPEN_PATH.into(),
                arg: json!({ "path": "/tmp/x.rs" }),
            },
        );
        let p: EmitParams = open.params_as().unwrap();
        assert_eq!(p.event, "open_path");
        assert_eq!(p.arg["path"], "/tmp/x.rs");

        let status = Envelope::notification(
            method::EMIT,
            EmitParams {
                event: event::STATUS.into(),
                arg: json!({ "text": "3 files" }),
            },
        );
        let p: EmitParams = status.params_as().unwrap();
        assert_eq!(p.event, "status");
        assert_eq!(p.arg["text"], "3 files");
    }

    #[test]
    fn invalidate_arg_is_json_and_matches_query_keying() {
        let env = Envelope::notification(
            method::INVALIDATE,
            InvalidateParams {
                kind: "table".into(),
                arg: json!({ "name": "users", "page": 0 }),
            },
        );
        let v = reparse(&env);
        assert!(v.get("id").is_none());
        let back: InvalidateParams = env.params_as().unwrap();
        assert_eq!(back.kind, "table");
        assert_eq!(back.arg["name"], "users");
    }

    // ── Framing ─────────────────────────────────────────────────────────────

    #[test]
    fn write_read_round_trip() {
        let messages = vec![
            Envelope::request(
                1,
                method::INITIALIZE,
                InitializeParams {
                    protocol: PROTOCOL_VERSION,
                    pane_id: 1,
                    rows: 5,
                    cols: 5,
                    args: vec![],
                    cwd: "/".to_string(),
                    capabilities: vec![],
                },
            ),
            Envelope::notification(
                method::EMIT,
                EmitParams {
                    event: event::STATUS.into(),
                    arg: json!({ "text": "ok" }),
                },
            ),
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
        let emit: EmitParams = second.params_as().unwrap();
        assert_eq!(emit.arg["text"], "ok");

        // EOF yields Ok(None).
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_message_eof_on_empty() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn a_malformed_line_is_invalid_data() {
        let mut cursor = Cursor::new(b"not json\n".to_vec());
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// The end-to-end client-loop shape: a stream of notifications and requests
    /// is consumed by dispatching on `method`, ignoring what it doesn't know.
    #[test]
    fn a_client_loop_dispatches_on_method_and_skips_unknowns() {
        let mut buf: Vec<u8> = Vec::new();
        write_message(
            &mut buf,
            &Envelope::notification(
                method::EMIT,
                EmitParams {
                    event: "divider".into(),
                    arg: json!({ "pos": 240 }),
                },
            ),
        )
        .unwrap();
        write_message(
            &mut buf,
            &Envelope::notification("futureThing", json!({})),
        )
        .unwrap();
        write_message(
            &mut buf,
            &Envelope::notification(method::SHUTDOWN, json!({})),
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
            // Unknown methods fall through — forward compatibility.
        }
        assert_eq!(seen, vec![("divider".to_string(), json!({ "pos": 240 }))]);
    }
}
