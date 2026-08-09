//! A generic LSP client: one language-server child process on stdio.
//!
//! Shaped after [`crate::process_pane::ProcessPane`] — piped stdin/stdout, a
//! synchronous initialize handshake, a background reader thread forwarding
//! parsed messages over an mpsc channel, and a Drop that shuts the child down.
//! The differences are all protocol: LSP frames with `Content-Length` headers
//! (see [`super::framing`]), correlates replies by request id, and interleaves
//! server-initiated notifications with those replies.
//!
//! Nothing here is Petal-specific; the language is entirely a
//! [`super::registry::LanguageServer`] descriptor.
//!
//! Wire failures are non-fatal by design: a language server that dies, hangs,
//! or answers garbage must never take the editor with it, so writes to a dead
//! child are dropped and malformed messages are skipped.

use std::io::{BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use serde_json::{json, Value};

use super::framing;

/// A message from the server: either a reply to one of our requests, or a
/// server-initiated notification (`textDocument/publishDiagnostics`, …).
#[derive(Debug, Clone)]
pub enum ServerMessage {
    Response {
        id: i64,
        result: Option<Value>,
        /// Surfaced to the completion session in G7 Phase 3; a server that
        /// errors a completion request should dismiss the popup, not hang it.
        #[allow(dead_code)]
        error: Option<Value>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// The subset of `ServerCapabilities` Garden acts on. Everything else in the
/// initialize result is ignored rather than modeled.
///
/// Parsed and asserted on today; read by the completion path in G7 Phases 3-4.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct ServerCapabilities {
    /// Whether the server advertised `completionProvider`.
    pub completion: bool,
    /// Characters that should implicitly fire a completion request.
    pub trigger_chars: Vec<String>,
    /// `textDocumentSync.change`: 1 = full text, 2 = incremental. Garden only
    /// sends full text today; a server asking for incremental still gets full
    /// documents, which is valid but chattier than it wants.
    pub change_sync: i64,
}

impl ServerCapabilities {
    fn parse(result: &Value) -> Self {
        let caps = &result["capabilities"];
        let completion_provider = &caps["completionProvider"];
        let trigger_chars = completion_provider["triggerCharacters"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // textDocumentSync is either a number or an object with a `change`.
        let sync = &caps["textDocumentSync"];
        let change_sync = sync
            .as_i64()
            .or_else(|| sync["change"].as_i64())
            .unwrap_or(1);

        Self {
            completion: completion_provider.is_object(),
            trigger_chars,
            change_sync,
        }
    }
}

/// One language server child process.
pub struct LspClient {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    rx: Receiver<ServerMessage>,
    next_id: i64,
    /// Asserted on by tests today; read by the trigger-char path in G7 Phase 4.
    #[allow(dead_code)]
    capabilities: ServerCapabilities,
    /// Set once a write fails: the child is gone and further traffic is futile.
    dead: bool,
}

impl LspClient {
    /// Spawn `command` with `args` and complete the `initialize` /
    /// `initialized` handshake before returning.
    ///
    /// The handshake is synchronous — like [`ProcessPane::spawn`] — so a server
    /// that is missing or immediately broken fails here, at the call site that
    /// can report it, rather than as silence later. `root_uri` scopes the
    /// server to the project.
    ///
    /// [`ProcessPane::spawn`]: crate::process_pane::ProcessPane::spawn
    pub fn spawn(command: &str, args: &[String], root_uri: Option<&str>) -> std::io::Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited, like GPP clients: a server's stderr is its log, and
            // capturing it would need another thread to drain or it deadlocks
            // once the pipe fills.
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut stdin = BufWriter::new(child.stdin.take().expect("child stdin was piped"));
        let stdout = child.stdout.take().expect("child stdout was piped");
        let mut reader = BufReader::new(stdout);

        let init_id = 1;
        write_framed(
            &mut stdin,
            &request(
                init_id,
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {
                        "textDocument": {
                            "synchronization": { "didSave": false },
                            "completion": {
                                "completionItem": { "snippetSupport": false }
                            }
                        }
                    },
                }),
            ),
        )?;

        // Read until the reply to *our* id. A server may legitimately emit
        // notifications (`window/logMessage`) before answering, so anything
        // else is skipped rather than treated as a protocol violation.
        let capabilities = loop {
            let Some(body) = framing::read_message(&mut reader)? else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "language server exited before responding to initialize",
                ));
            };
            let Ok(msg) = serde_json::from_str::<Value>(&body) else {
                continue;
            };
            if msg["id"].as_i64() != Some(init_id) {
                continue;
            }
            if !msg["error"].is_null() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("language server rejected initialize: {}", msg["error"]),
                ));
            }
            break ServerCapabilities::parse(&msg["result"]);
        };

        write_framed(&mut stdin, &notification("initialized", json!({})))?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(Some(body)) = framing::read_message(&mut reader) {
                if let Some(msg) = parse_server_message(&body) {
                    if tx.send(msg).is_err() {
                        break; // the LspClient was dropped
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            rx,
            next_id: init_id + 1,
            capabilities,
            dead: false,
        })
    }

    #[allow(dead_code)] // trigger chars drive implicit completion (G7 Phase 4)
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// Whether a write has failed, meaning the child is gone. The owner uses
    /// this to drop and (optionally) respawn the client.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Collect everything the reader thread has parsed, without blocking.
    pub fn try_drain(&self) -> Vec<ServerMessage> {
        self.rx.try_iter().collect()
    }

    pub fn notify_did_open(&mut self, uri: &str, language_id: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        );
    }

    /// Full-text sync: the whole document replaces the server's copy. Garden
    /// has no incremental-change plumbing, and full sync is what the Petal
    /// server advertises anyway.
    pub fn notify_did_change(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ],
            }),
        );
    }

    pub fn notify_did_close(&mut self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
    }

    /// Request completions at a position. Returns the request id, which the
    /// caller correlates against a later [`ServerMessage::Response`] — the
    /// answer arrives on a subsequent poll tick, not from this call.
    pub fn request_completion(&mut self, uri: &str, line: u32, character: u32) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let req = request(
            id,
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line, "character": character },
            }),
        );
        self.send(&req);
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = notification(method, params);
        self.send(&msg);
    }

    /// Write a message, marking the client dead if the pipe is gone. Failures
    /// are logged, never propagated: no editor action should fail because a
    /// language server did.
    fn send(&mut self, msg: &Value) {
        if self.dead {
            return;
        }
        if let Err(e) = write_framed(&mut self.stdin, msg) {
            eprintln!("garden: language server write failed ({e}); dropping the connection");
            self.dead = true;
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Best-effort graceful shutdown, then EOF, then the hammer — the same
        // escalation ProcessPane uses. `shutdown`'s reply is not awaited; we
        // are on our way out either way.
        if !self.dead {
            let _ = write_framed(&mut self.stdin, &request(0, "shutdown", Value::Null));
            let _ = write_framed(&mut self.stdin, &notification("exit", Value::Null));
            let _ = self.stdin.flush();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn write_framed<W: Write>(writer: &mut W, msg: &Value) -> std::io::Result<()> {
    writer.write_all(&framing::encode(&msg.to_string()))?;
    // Flush per message: the server is blocked reading, and a buffered request
    // that never reaches it looks exactly like a hung server.
    writer.flush()
}

/// Classify one raw message. Returns `None` for anything unparseable or
/// server-initiated *requests* (which Garden does not answer) — dropping those
/// is preferable to blocking the reader thread on them.
fn parse_server_message(body: &str) -> Option<ServerMessage> {
    let msg: Value = serde_json::from_str(body).ok()?;
    let has_method = msg.get("method").and_then(Value::as_str).is_some();

    match (msg.get("id").and_then(Value::as_i64), has_method) {
        // A reply to one of our requests.
        (Some(id), false) => Some(ServerMessage::Response {
            id,
            result: msg.get("result").cloned().filter(|v| !v.is_null()),
            error: msg.get("error").cloned().filter(|v| !v.is_null()),
        }),
        // No id + a method = a notification.
        (None, true) => Some(ServerMessage::Notification {
            method: msg["method"].as_str().unwrap().to_string(),
            params: msg.get("params").cloned().unwrap_or(Value::Null),
        }),
        // id + method = a server-initiated request; unsupported.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_response() {
        let msg = parse_server_message(r#"{"jsonrpc":"2.0","id":7,"result":{"items":[]}}"#);
        match msg {
            Some(ServerMessage::Response { id, result, error }) => {
                assert_eq!(id, 7);
                assert!(result.is_some());
                assert!(error.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_error_response() {
        let msg = parse_server_message(
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"nope"}}"#,
        );
        match msg {
            Some(ServerMessage::Response { result, error, .. }) => {
                assert!(result.is_none());
                assert_eq!(error.unwrap()["code"], -32601);
            }
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_notification() {
        let msg = parse_server_message(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a"}}"#,
        );
        match msg {
            Some(ServerMessage::Notification { method, params }) => {
                assert_eq!(method, "textDocument/publishDiagnostics");
                assert_eq!(params["uri"], "file:///a");
            }
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    #[test]
    fn drops_server_initiated_requests_and_garbage() {
        // id + method: a request we cannot answer.
        assert!(parse_server_message(r#"{"id":1,"method":"window/showMessageRequest"}"#).is_none());
        assert!(parse_server_message("not json").is_none());
    }

    #[test]
    fn reads_capabilities_from_an_initialize_result() {
        let caps = ServerCapabilities::parse(&json!({
            "capabilities": {
                "textDocumentSync": { "openClose": true, "change": 1 },
                "completionProvider": { "triggerCharacters": ["."] },
                "hoverProvider": true,
            }
        }));
        assert!(caps.completion);
        assert_eq!(caps.trigger_chars, vec!["."]);
        assert_eq!(caps.change_sync, 1);
    }

    /// End-to-end against a real server, when one is available.
    ///
    /// Skipped (not failed) when `petal` can't be resolved: Garden path-depends
    /// on the petal *crates*, not on an installed `petal` *binary*, so a
    /// checkout can legitimately lack it. Set `GARDEN_PETAL_LSP_BIN` or put
    /// `petal` on `$PATH` to exercise this.
    #[test]
    fn completes_against_a_live_petal_server() {
        use std::time::{Duration, Instant};

        let server = &super::super::registry::REGISTRY[0];
        let command = server.resolved_command();
        // A bare name that isn't on PATH fails at spawn; probe before asserting.
        if Command::new(&command)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping: no `{command}` binary (set GARDEN_PETAL_LSP_BIN)");
            return;
        }

        let mut client = LspClient::spawn(&command, &server.args(), None)
            .expect("the petal language server should start");
        assert!(client.capabilities().completion);
        assert_eq!(client.capabilities().trigger_chars, vec!["."]);

        client.notify_did_open(
            "file:///t.ptl",
            server.language_id,
            1,
            "fn calculate(x)\n  return x * 2\nend\nca\n",
        );
        let id = client.request_completion("file:///t.ptl", 3, 2);

        // The answer arrives on the reader thread; in the app this is a poll
        // tick, so poll here too rather than blocking on the channel.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut labels: Vec<String> = Vec::new();
        while Instant::now() < deadline && labels.is_empty() {
            for msg in client.try_drain() {
                if let ServerMessage::Response {
                    id: got, result, ..
                } = msg
                {
                    if got == id {
                        let items = result.expect("a completion result")["items"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        labels = items
                            .iter()
                            .filter_map(|i| i["label"].as_str().map(str::to_string))
                            .collect();
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(
            labels.iter().any(|l| l == "calculate"),
            "expected the user definition in {labels:?}"
        );
    }

    #[test]
    fn reads_capabilities_when_sync_is_a_bare_number() {
        // The spec allows textDocumentSync to be just the change kind.
        let caps = ServerCapabilities::parse(&json!({
            "capabilities": { "textDocumentSync": 2 }
        }));
        assert_eq!(caps.change_sync, 2);
        assert!(
            !caps.completion,
            "no completionProvider means no completion"
        );
        assert!(caps.trigger_chars.is_empty());
    }
}
