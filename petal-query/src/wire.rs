//! The stdio wire a panel-mode provider speaks — a self-contained implementation
//! of the query subset of the Garden Pane Protocol (GPP).
//!
//! A `petal-query` [`App`](crate::App) is a subprocess that a host launches and
//! drives over newline-delimited JSON-RPC 2.0 on stdio. This module defines the
//! handful of message shapes that exchange needs, so an app depends only on
//! `petal-query` (+ `serde_json`) — it does not link Garden's `gpp` crate.
//!
//! **The `gpp` crate is the canonical GPP definition;** these structs are the
//! wire-compatible client half of its query/panel subset. The JSON is
//! byte-for-byte identical (same method names, same `camelCase` fields, same
//! `kebab-case` enums), which is the contract — a host built on `gpp` and a
//! provider built on `petal-query` interoperate because they agree on the JSON,
//! not because they share types. The shared *new* piece, [`CachePolicy`], lives
//! in `petal-query` and `gpp` re-uses it, so the `cacheControl` field cannot
//! drift.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::cache_control::CachePolicy;

/// JSON-RPC method names on the wire (the query/panel subset of GPP).
pub mod method {
    /// Host → client request (id 1): pane id, size, launch args, cwd.
    pub const INITIALIZE: &str = "initialize";
    /// Client → host notification: (re)load the pane's Petal UI script.
    pub const SET_SCRIPT: &str = "setScript";
    /// Host → client request: the script called `query(kind, arg)` and the host
    /// has no fresh cached value; fetch it.
    pub const QUERY: &str = "query";
    /// Host → client notification: the script called `emit(event, arg)`.
    pub const EMIT: &str = "emit";
    /// Client → host notification: proactively drop a cached `(kind, arg)`.
    pub const INVALIDATE: &str = "invalidate";
    /// Client → host notification: replace this pane with an editor on a path.
    pub const OPEN_PATH: &str = "openPath";
    /// Client → host notification: set the pane status text.
    pub const SET_STATUS: &str = "setStatus";
    /// Host → client notification: the pane was resized.
    pub const RESIZE: &str = "resize";
    /// Host → client notification: the client should exit.
    pub const SHUTDOWN: &str = "shutdown";
}

/// A JSON-RPC error object.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// The on-the-wire JSON-RPC envelope (see the `gpp` crate for the canonical
/// definition). Absent fields are skipped so the JSON matches each message kind.
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
    /// Build a notification (`method` + `params`, no `id`).
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

    /// Build a response (`id` + `result`, no `method`).
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

    /// Is this a request/notification with the given method name?
    pub fn is_method(&self, m: &str) -> bool {
        self.method.as_deref() == Some(m)
    }

    /// Deserialize [`Self::params`] into a typed struct.
    pub fn params_as<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.params.clone().unwrap_or(serde_json::Value::Null))
    }
}

/// Params for the `initialize` request (host → client): the pane id, size, the
/// launch `args`, and the pane's `cwd` — how a provider learns what to serve.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub pane_id: u64,
    pub rows: u32,
    pub cols: u32,
    pub args: Vec<String>,
    pub cwd: String,
}

impl InitializeParams {
    /// The first launch arg, or the pane `cwd` when none was given — the common
    /// "which directory/target do I operate on?" resolution a file/git app wants.
    pub fn repo_arg(&self) -> String {
        self.args.first().cloned().unwrap_or_else(|| self.cwd.clone())
    }
}

/// The `initialize` response (client → host). A `petal-query` app always reports
/// panel mode; the Lines-mode fields (`takeover`/`keymap`/`mouse`) are omitted
/// and the host decodes their defaults.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub name: String,
    /// Always `"panel"` for a `petal-query` app.
    pub mode: String,
}

/// Params for a `setScript` notification (client → host): the Petal UI source.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetScriptParams {
    pub source: String,
}

/// Params for a `query` request (host → client): the script's `query(kind, arg)`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct QueryParams {
    pub kind: String,
    pub arg: String,
}

/// The `queryResult` response (client → host). Echoes `kind`/`arg` so the host
/// keys its cache without tracking ids; exactly one of `value`/`error` is set
/// (both absent means "still loading"). `cache_control` tells the host how long
/// to trust the answer — omitted (`None`) is [`CachePolicy::forever`].
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub kind: String,
    pub arg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How cacheable this answer is. Omitted for the default (fresh forever).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CachePolicy>,
}

/// Params for an `emit` notification (host → client): the script's
/// `emit(event, arg)`, a fire-and-forget user-intent signal. `arg` is any JSON.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EmitParams {
    pub event: String,
    pub arg: serde_json::Value,
}

/// Params for an `invalidate` notification (client → host): drop `(kind, arg)`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InvalidateParams {
    pub kind: String,
    pub arg: String,
}

/// Params for an `openPath` notification (client → host).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenPathParams {
    pub path: String,
}

/// Params for a `setStatus` notification (client → host).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetStatusParams {
    pub text: String,
}

/// Write one [`Envelope`] as a newline-terminated JSON line and flush.
pub fn write_message<W: std::io::Write>(w: &mut W, env: &Envelope) -> std::io::Result<()> {
    serde_json::to_writer(&mut *w, env)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// Read one newline-delimited [`Envelope`]. `Ok(None)` at EOF; a malformed line
/// is [`std::io::ErrorKind::InvalidData`].
pub fn read_message<R: std::io::BufRead>(r: &mut R) -> std::io::Result<Option<Envelope>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
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

    #[test]
    fn query_result_carries_cache_control() {
        let r = QueryResult {
            kind: "log".into(),
            arg: "".into(),
            value: Some(json!({ "commits": [] })),
            error: None,
            cache_control: Some(CachePolicy::max_age(std::time::Duration::from_secs(3))),
        };
        let v = serde_json::to_value(Envelope::response(1, r)).unwrap();
        assert_eq!(v["result"]["cacheControl"]["maxAgeMs"], 3000);
        assert!(v["result"].get("error").is_none());
    }

    #[test]
    fn query_result_omits_cache_control_when_none() {
        let r = QueryResult {
            kind: "x".into(),
            arg: "".into(),
            value: Some(json!(1)),
            ..Default::default()
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("cacheControl").is_none());
    }

    #[test]
    fn initialize_result_reports_panel_mode() {
        let v = serde_json::to_value(InitializeResult {
            name: "git-log".into(),
            mode: "panel".into(),
        })
        .unwrap();
        assert_eq!(v["mode"], "panel");
    }
}
