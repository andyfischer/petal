//! Language-server lifecycle and document sync, driven off the poll tick.
//!
//! [`LspManager`] owns one [`LspClient`] per *language* (not per file, and not
//! per pane): servers are long-lived and shared, so opening a second `.ptl`
//! pane reuses the running server. Clients are spawned lazily, the first time
//! an eligible file is actually open — a session that never opens a `.ptl`
//! file never starts a Petal server.
//!
//! [`App::poll_lsp`] reconciles, each tick, the set of open editor documents
//! against what the servers have been told:
//!
//! - a newly-visible eligible file → `didOpen`
//! - a file whose `Buffer::revision` moved → `didChange` (full text)
//! - a file no longer in any pane → `didClose`
//!
//! Revision-gating is what keeps this cheap: `poll_files` runs at
//! `RELOAD_POLL` cadence, but an untouched buffer costs one integer compare,
//! not a `text()` allocation.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::lsp::registry::{self, LanguageServer};
use crate::lsp::{client::ServerMessage, path_to_uri, LspClient};

/// What a server has been told about one open document.
#[derive(Debug, Clone)]
pub struct OpenDoc {
    pub uri: String,
    /// Language id, which is also the key of the client that owns this doc.
    pub language_id: &'static str,
    /// LSP document version, incremented on every didChange we send.
    pub version: i64,
    /// The `Buffer::revision` the server's copy reflects.
    pub revision: u64,
}

/// Per-language clients plus the documents they've been told about.
#[derive(Default)]
pub struct LspManager {
    clients: HashMap<&'static str, LspClient>,
    docs: HashMap<PathBuf, OpenDoc>,
    /// Languages whose server failed to start. Kept so a missing binary is
    /// reported once and then stops being retried every tick.
    failed: HashMap<&'static str, String>,
    /// Latest diagnostics per document URI, as published by the server.
    diagnostics: HashMap<String, Vec<serde_json::Value>>,
    /// Completion responses that have arrived and not yet been consumed, keyed
    /// by request id. Phase 3 drains these into the completion session; until
    /// then they are surfaced in `/state` so the transport is observable.
    completions: HashMap<i64, serde_json::Value>,
}

impl LspManager {
    /// The client for `server`, spawning it on first use. `None` once a spawn
    /// has failed — the error is recorded and not retried.
    fn client_for(
        &mut self,
        server: &'static LanguageServer,
        root: &PathBuf,
    ) -> Option<&mut LspClient> {
        let key = server.language_id;
        if self.failed.contains_key(key) {
            return None;
        }
        if !self.clients.contains_key(key) {
            let command = server.resolved_command();
            match LspClient::spawn(&command, &server.args(), Some(&path_to_uri(root))) {
                Ok(client) => {
                    self.clients.insert(key, client);
                }
                Err(e) => {
                    // The likely cause is simply that the server isn't
                    // installed, so name the binary and how to point at it
                    // rather than surfacing a bare NotFound.
                    let msg = format!(
                        "{key}: could not start `{command}` ({e}). \
                         Install it or set {} to its path.",
                        server.env_override
                    );
                    eprintln!("garden: {msg}");
                    self.failed.insert(key, msg);
                    return None;
                }
            }
        }
        self.clients.get_mut(key)
    }

    /// Diagnostics last published for `uri`.
    pub fn diagnostics_for(&self, uri: &str) -> &[serde_json::Value] {
        self.diagnostics.get(uri).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Languages whose server could not be started, with the reason.
    pub fn failures(&self) -> &HashMap<&'static str, String> {
        &self.failed
    }

    /// Take a completion response that has arrived, if any.
    #[allow(dead_code)] // consumed by the completion session (G7 Phase 3)
    pub fn take_completion(&mut self, id: i64) -> Option<serde_json::Value> {
        self.completions.remove(&id)
    }

    /// Whether a language server is running for `path`.
    #[allow(dead_code)] // completion integration entry point
    pub fn has_client_for(&self, path: &std::path::Path) -> bool {
        registry::for_path(path)
            .map(|s| self.clients.contains_key(s.language_id))
            .unwrap_or(false)
    }

    /// Request completions for an open document. `None` when the file has no
    /// server, its server isn't running, or the document was never opened —
    /// all ordinary states, not errors.
    #[allow(dead_code)] // completion integration entry point
    pub fn request_completion(
        &mut self,
        path: &std::path::Path,
        line: u32,
        character: u32,
    ) -> Option<i64> {
        let doc = self.docs.get(path)?;
        let client = self.clients.get_mut(doc.language_id)?;
        Some(client.request_completion(&doc.uri.clone(), line, character))
    }

    /// Drain every client's reader channel, filing responses and notifications.
    fn drain_clients(&mut self) {
        for client in self.clients.values_mut() {
            for msg in client.try_drain() {
                match msg {
                    ServerMessage::Notification { method, params } => {
                        if method == "textDocument/publishDiagnostics" {
                            if let Some(uri) = params["uri"].as_str() {
                                let diags = params["diagnostics"]
                                    .as_array()
                                    .cloned()
                                    .unwrap_or_default();
                                self.diagnostics.insert(uri.to_string(), diags);
                            }
                        }
                    }
                    ServerMessage::Response { id, result, .. } => {
                        if let Some(result) = result {
                            self.completions.insert(id, result);
                        }
                    }
                }
            }
        }

        // A server that died takes its documents with it: dropping the doc
        // entries means a respawn re-opens them from scratch rather than
        // resuming a conversation the new process never had.
        let dead: Vec<&'static str> = self
            .clients
            .iter()
            .filter(|(_, c)| c.is_dead())
            .map(|(k, _)| *k)
            .collect();
        for key in dead {
            self.clients.remove(key);
            self.docs.retain(|_, d| d.language_id != key);
        }
    }
}

impl super::App {
    /// Reconcile open editor documents with the language servers. Called once
    /// per poll tick beside `poll_processes`.
    pub fn poll_lsp(&mut self) {
        // Snapshot the eligible open files first: (path, revision). Reading
        // text is deferred until a revision actually differs.
        let mut visible: Vec<(PathBuf, u64)> = Vec::new();
        for pane in &self.panes {
            if pane.is_panel() || pane.is_process() {
                continue;
            }
            let Some(path) = pane.view.buffer.path() else {
                continue;
            };
            if registry::for_path(path).is_none() {
                continue;
            }
            let path = path.to_path_buf();
            if !visible.iter().any(|(p, _)| *p == path) {
                visible.push((path, pane.view.buffer.revision()));
            }
        }

        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

        // didClose for documents that are no longer shown anywhere.
        let live: HashSet<&PathBuf> = visible.iter().map(|(p, _)| p).collect();
        let closed: Vec<PathBuf> = self
            .lsp
            .docs
            .keys()
            .filter(|p| !live.contains(p))
            .cloned()
            .collect();
        for path in closed {
            if let Some(doc) = self.lsp.docs.remove(&path) {
                if let Some(client) = self.lsp.clients.get_mut(doc.language_id) {
                    client.notify_did_close(&doc.uri);
                }
                self.lsp.diagnostics.remove(&doc.uri);
            }
        }

        // didOpen / didChange.
        for (path, revision) in visible {
            let Some(server) = registry::for_path(&path) else {
                continue;
            };
            match self.lsp.docs.get(&path) {
                Some(doc) if doc.revision == revision => continue,
                Some(doc) => {
                    let (uri, language_id, version) =
                        (doc.uri.clone(), doc.language_id, doc.version + 1);
                    let Some(text) = self.buffer_text(&path) else {
                        continue;
                    };
                    if let Some(client) = self.lsp.clients.get_mut(language_id) {
                        client.notify_did_change(&uri, version, &text);
                    }
                    if let Some(doc) = self.lsp.docs.get_mut(&path) {
                        doc.version = version;
                        doc.revision = revision;
                    }
                }
                None => {
                    let Some(text) = self.buffer_text(&path) else {
                        continue;
                    };
                    let uri = path_to_uri(&path);
                    if self.lsp.client_for(server, &root).is_none() {
                        continue; // server unavailable; already reported
                    }
                    let client = self
                        .lsp
                        .clients
                        .get_mut(server.language_id)
                        .expect("client_for just inserted it");
                    client.notify_did_open(&uri, server.language_id, 1, &text);
                    self.lsp.docs.insert(
                        path,
                        OpenDoc {
                            uri,
                            language_id: server.language_id,
                            version: 1,
                            revision,
                        },
                    );
                }
            }
        }

        self.lsp.drain_clients();
    }

    /// LSP state for `GET /state`: which documents each server has, at which
    /// version, plus any diagnostics and spawn failures.
    ///
    /// This is how Phase 2 is verified without a window — a headless run can
    /// assert the server was told about a buffer, and that editing it bumped
    /// the version. Mirrors how `file_finder` exposes `match_paths`.
    pub(in crate::app) fn lsp_state_json(&self) -> serde_json::Value {
        let mut docs: Vec<serde_json::Value> = self
            .lsp
            .docs
            .iter()
            .map(|(path, doc)| {
                serde_json::json!({
                    "path": path.display().to_string(),
                    "uri": doc.uri,
                    "language_id": doc.language_id,
                    "version": doc.version,
                    "revision": doc.revision,
                    "diagnostics": self.lsp.diagnostics_for(&doc.uri).len(),
                })
            })
            .collect();
        // Stable order: a map iteration order would make headless assertions flaky.
        docs.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

        let mut servers: Vec<&str> = self.lsp.clients.keys().copied().collect();
        servers.sort_unstable();

        serde_json::json!({
            "servers": servers,
            "documents": docs,
            "failures": self.lsp.failures().iter().map(|(k, v)| serde_json::json!({
                "language_id": k,
                "error": v,
            })).collect::<Vec<_>>(),
            "pending_completions": self.lsp.completions.len(),
        })
    }

    /// Current text of the editor pane showing `path`.
    fn buffer_text(&self, path: &std::path::Path) -> Option<String> {
        self.panes
            .iter()
            .filter(|p| !p.is_panel() && !p.is_process())
            .find(|p| p.view.buffer.path() == Some(path))
            .map(|p| p.view.buffer.text())
    }
}
