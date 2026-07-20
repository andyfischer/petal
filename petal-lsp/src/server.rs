//! The LSP server — dispatches JSON-RPC messages to handlers.

use serde_json::Value;

use crate::document::{
    self, identifier_at_position, DefinitionKind, DocumentStore, KEYWORDS,
};
use crate::lsp_types::*;
use crate::protocol::*;

pub struct Server {
    initialized: bool,
    documents: DocumentStore,
}

impl Server {
    pub fn new() -> Self {
        Self {
            initialized: false,
            documents: DocumentStore::new(),
        }
    }

    /// Process one JSON-RPC message and return any outgoing messages (responses
    /// and/or notifications). The caller is responsible for framing
    /// (Content-Length headers) and transport.
    pub fn handle_message(&mut self, json: &str) -> Vec<OutgoingMessage> {
        let msg: RpcMessage = match serde_json::from_str(json) {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };

        if msg.is_request() {
            self.handle_request(msg)
        } else if msg.is_notification() {
            self.handle_notification(msg)
        } else {
            Vec::new()
        }
    }

    // -----------------------------------------------------------------------
    // Request dispatch
    // -----------------------------------------------------------------------

    fn handle_request(&mut self, msg: RpcMessage) -> Vec<OutgoingMessage> {
        let id = msg.id.unwrap();
        let method = msg.method.as_deref().unwrap();
        let params = msg.params.unwrap_or(Value::Null);

        match method {
            "initialize" => self.handle_initialize(id, params),
            "shutdown" => {
                vec![OutgoingMessage::Response(RpcResponse::ok(
                    id,
                    Value::Null,
                ))]
            }
            "textDocument/hover" => {
                if !self.initialized {
                    return vec![OutgoingMessage::Response(RpcResponse::err(
                        id,
                        SERVER_NOT_INITIALIZED,
                        "Server not initialized".into(),
                    ))];
                }
                self.handle_hover(id, params)
            }
            "textDocument/definition" => {
                if !self.initialized {
                    return vec![OutgoingMessage::Response(RpcResponse::err(
                        id,
                        SERVER_NOT_INITIALIZED,
                        "Server not initialized".into(),
                    ))];
                }
                self.handle_definition(id, params)
            }
            "textDocument/completion" => {
                if !self.initialized {
                    return vec![OutgoingMessage::Response(RpcResponse::err(
                        id,
                        SERVER_NOT_INITIALIZED,
                        "Server not initialized".into(),
                    ))];
                }
                self.handle_completion(id, params)
            }
            _ => vec![OutgoingMessage::Response(RpcResponse::err(
                id,
                METHOD_NOT_FOUND,
                format!("Method not found: {method}"),
            ))],
        }
    }

    // -----------------------------------------------------------------------
    // Notification dispatch
    // -----------------------------------------------------------------------

    fn handle_notification(&mut self, msg: RpcMessage) -> Vec<OutgoingMessage> {
        let method = msg.method.as_deref().unwrap();
        let params = msg.params.unwrap_or(Value::Null);

        match method {
            "initialized" => Vec::new(),
            "exit" => Vec::new(),
            "textDocument/didOpen" => self.handle_did_open(params),
            "textDocument/didChange" => self.handle_did_change(params),
            "textDocument/didClose" => self.handle_did_close(params),
            _ => Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Initialize
    // -----------------------------------------------------------------------

    fn handle_initialize(&mut self, id: Value, _params: Value) -> Vec<OutgoingMessage> {
        self.initialized = true;
        let result = InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncOptions {
                    open_close: true,
                    change: TEXT_DOCUMENT_SYNC_FULL,
                }),
                hover_provider: Some(true),
                definition_provider: Some(true),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                }),
            },
        };
        vec![OutgoingMessage::Response(RpcResponse::ok(
            id,
            serde_json::to_value(result).unwrap(),
        ))]
    }

    // -----------------------------------------------------------------------
    // Document sync
    // -----------------------------------------------------------------------

    fn handle_did_open(&mut self, params: Value) -> Vec<OutgoingMessage> {
        let p: DidOpenTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        self.documents.open(
            p.text_document.uri.clone(),
            p.text_document.version,
            p.text_document.text,
        );
        self.publish_diagnostics(&p.text_document.uri)
    }

    fn handle_did_change(&mut self, params: Value) -> Vec<OutgoingMessage> {
        let p: DidChangeTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        if let Some(change) = p.content_changes.into_iter().last() {
            self.documents
                .change(&p.text_document.uri, p.text_document.version, change.text);
        }
        self.publish_diagnostics(&p.text_document.uri)
    }

    fn handle_did_close(&mut self, params: Value) -> Vec<OutgoingMessage> {
        let p: DidCloseTextDocumentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let uri = p.text_document.uri;
        self.documents.close(&uri);
        vec![OutgoingMessage::Notification(RpcNotification::new(
            "textDocument/publishDiagnostics",
            serde_json::to_value(PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
            })
            .unwrap(),
        ))]
    }

    fn publish_diagnostics(&self, uri: &str) -> Vec<OutgoingMessage> {
        let diagnostics = self
            .documents
            .get(uri)
            .map(|d| d.analysis.diagnostics.clone())
            .unwrap_or_default();
        vec![OutgoingMessage::Notification(RpcNotification::new(
            "textDocument/publishDiagnostics",
            serde_json::to_value(PublishDiagnosticsParams {
                uri: uri.to_string(),
                diagnostics,
            })
            .unwrap(),
        ))]
    }

    // -----------------------------------------------------------------------
    // Hover
    // -----------------------------------------------------------------------

    fn handle_hover(&self, id: Value, params: Value) -> Vec<OutgoingMessage> {
        let p: TextDocumentPositionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => {
                return vec![OutgoingMessage::Response(RpcResponse::err(
                    id,
                    INVALID_PARAMS,
                    "Invalid hover params".into(),
                ))]
            }
        };

        let doc = match self.documents.get(&p.text_document.uri) {
            Some(d) => d,
            None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
        };

        let ident = match identifier_at_position(&doc.text, p.position.line, p.position.character) {
            Some(i) => i,
            None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
        };

        if let Some(def) = doc
            .analysis
            .definitions
            .iter()
            .find(|d| d.name == ident)
        {
            let kind_str = match def.kind {
                DefinitionKind::Variable => "variable",
                DefinitionKind::Function => "function",
                DefinitionKind::Enum => "enum",
                DefinitionKind::Parameter => "parameter",
            };
            let detail = def.detail.as_deref().unwrap_or("");
            let value = if detail.is_empty() {
                format!("({kind_str}) {}", def.name)
            } else {
                format!("({kind_str}) {} {detail}", def.name)
            };
            let hover = Hover {
                contents: MarkupContent {
                    kind: "markdown".to_string(),
                    value: format!("```petal\n{value}\n```"),
                },
                range: Some(document::span_to_range(&def.span)),
            };
            return vec![OutgoingMessage::Response(RpcResponse::ok(
                id,
                serde_json::to_value(hover).unwrap(),
            ))];
        }

        if KEYWORDS.contains(&ident.as_str()) {
            let hover = Hover {
                contents: MarkupContent {
                    kind: "markdown".to_string(),
                    value: format!("(keyword) `{ident}`"),
                },
                range: None,
            };
            return vec![OutgoingMessage::Response(RpcResponse::ok(
                id,
                serde_json::to_value(hover).unwrap(),
            ))];
        }

        vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))]
    }

    // -----------------------------------------------------------------------
    // Go to definition
    // -----------------------------------------------------------------------

    fn handle_definition(&self, id: Value, params: Value) -> Vec<OutgoingMessage> {
        let p: TextDocumentPositionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => {
                return vec![OutgoingMessage::Response(RpcResponse::err(
                    id,
                    INVALID_PARAMS,
                    "Invalid definition params".into(),
                ))]
            }
        };

        let doc = match self.documents.get(&p.text_document.uri) {
            Some(d) => d,
            None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
        };

        let ident = match identifier_at_position(&doc.text, p.position.line, p.position.character) {
            Some(i) => i,
            None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
        };

        if let Some(def) = doc
            .analysis
            .definitions
            .iter()
            .find(|d| d.name == ident)
        {
            let loc = Location {
                uri: p.text_document.uri.clone(),
                range: document::span_to_range(&def.span),
            };
            return vec![OutgoingMessage::Response(RpcResponse::ok(
                id,
                serde_json::to_value(loc).unwrap(),
            ))];
        }

        vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))]
    }

    // -----------------------------------------------------------------------
    // Completion
    // -----------------------------------------------------------------------

    fn handle_completion(&self, id: Value, params: Value) -> Vec<OutgoingMessage> {
        let p: CompletionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => {
                return vec![OutgoingMessage::Response(RpcResponse::err(
                    id,
                    INVALID_PARAMS,
                    "Invalid completion params".into(),
                ))]
            }
        };

        let doc = match self.documents.get(&p.text_document.uri) {
            Some(d) => d,
            None => {
                return vec![OutgoingMessage::Response(RpcResponse::ok(
                    id,
                    serde_json::to_value(CompletionList {
                        is_incomplete: false,
                        items: Vec::new(),
                    })
                    .unwrap(),
                ))]
            }
        };

        let prefix =
            identifier_at_position(&doc.text, p.position.line, p.position.character)
                .unwrap_or_default();

        let mut items: Vec<CompletionItem> = Vec::new();

        for def in &doc.analysis.definitions {
            if !prefix.is_empty() && !def.name.starts_with(&prefix) {
                continue;
            }
            let kind = match def.kind {
                DefinitionKind::Variable => CompletionItemKind::Variable,
                DefinitionKind::Function => CompletionItemKind::Function,
                DefinitionKind::Enum => CompletionItemKind::Variable,
                DefinitionKind::Parameter => CompletionItemKind::Variable,
            };
            items.push(CompletionItem {
                label: def.name.clone(),
                kind: Some(kind),
                detail: def.detail.clone(),
            });
        }

        for &kw in KEYWORDS {
            if !prefix.is_empty() && !kw.starts_with(&prefix) {
                continue;
            }
            items.push(CompletionItem {
                label: kw.to_string(),
                kind: Some(CompletionItemKind::Keyword),
                detail: None,
            });
        }

        vec![OutgoingMessage::Response(RpcResponse::ok(
            id,
            serde_json::to_value(CompletionList {
                is_incomplete: false,
                items,
            })
            .unwrap(),
        ))]
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
