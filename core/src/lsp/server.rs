//! The LSP server — dispatches JSON-RPC messages to handlers.

use serde_json::Value;

use crate::lsp::document::{
    self, Definition, DefinitionKind, DocumentStore, KEYWORDS, identifier_at_position,
    qualifier_at_position,
};
use crate::lsp::lsp_types::*;
use crate::lsp::members::Origin;
use crate::lsp::protocol::*;

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
                vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))]
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
            "textDocument/documentSymbol" => {
                if !self.initialized {
                    return vec![OutgoingMessage::Response(RpcResponse::err(
                        id,
                        SERVER_NOT_INITIALIZED,
                        "Server not initialized".into(),
                    ))];
                }
                self.handle_document_symbol(id, params)
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
                    trigger_characters: Some(vec![".".to_string(), "/".to_string()]),
                }),
                document_symbol_provider: Some(true),
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
                ))];
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

        let describe = |def: &Definition| {
            let kind_str = match def.kind {
                DefinitionKind::Variable | DefinitionKind::Loop => "variable",
                DefinitionKind::Function => "function",
                DefinitionKind::Enum => "enum",
                DefinitionKind::Parameter => "parameter",
            };
            let detail = def.detail.as_deref().unwrap_or("");
            if detail.is_empty() {
                format!("({kind_str}) {}", def.name)
            } else {
                format!("({kind_str}) {} {detail}", def.name)
            }
        };
        // `Kind.park`, `op.place`: a member of an enum or of a variant value.
        let member = doc.analysis.member_at(&doc.text, p.position);
        // (A name the enum does not have is looked up by name below, as
        // any other is.)
        if let Some(value) = member.and_then(|m| m.hover(&ident)) {
            let hover = Hover {
                contents: MarkupContent {
                    kind: "markdown".to_string(),
                    value: format!("```petal\n{value}\n```"),
                },
                range: None,
            };
            return vec![OutgoingMessage::Response(RpcResponse::ok(
                id,
                serde_json::to_value(hover).unwrap(),
            ))];
        }

        // `m.name`: the module's export, and nothing else by that name.
        let qualified = qualifier_at_position(&doc.text, p.position.line, p.position.character)
            .and_then(|q| doc.analysis.qualifier(&q));
        if let Some(module) = qualified {
            let value = match module.export(&ident) {
                Some(found) => format!("{}\n// from module {}", describe(&found.def), module.module),
                None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
            };
            let hover = Hover {
                contents: MarkupContent {
                    kind: "markdown".to_string(),
                    value: format!("```petal\n{value}\n```"),
                },
                range: None,
            };
            return vec![OutgoingMessage::Response(RpcResponse::ok(
                id,
                serde_json::to_value(hover).unwrap(),
            ))];
        }

        if let Some(def) = doc.analysis.definition_in_scope(&ident, p.position) {
            // A top-level declaration also says what the static-value
            // classifier makes of it.
            let value = match self.documents.static_note(&p.text_document.uri, def) {
                Some(note) => format!("{}\n// {note}", describe(def)),
                None => describe(def),
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

        // A name an import brought in: say which module it is from, since
        // nothing on the line does.
        if let Some(found) = doc.analysis.imported(&ident) {
            let from = doc
                .analysis
                .imports
                .iter()
                .find(|m| m.binds_bare(&ident) && m.export(&ident).is_some())
                .map(|m| m.module.as_str())
                .unwrap_or("");
            let hover = Hover {
                contents: MarkupContent {
                    kind: "markdown".to_string(),
                    value: format!("```petal\n{}\n// from module {from}\n```", describe(&found.def)),
                },
                range: None,
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
                ))];
            }
        };

        let doc = match self.documents.get(&p.text_document.uri) {
            Some(d) => d,
            None => return vec![OutgoingMessage::Response(RpcResponse::ok(id, Value::Null))],
        };

        let located = |uri: String, range: Range| {
            vec![OutgoingMessage::Response(RpcResponse::ok(
                id.clone(),
                serde_json::to_value(Location { uri, range }).unwrap(),
            ))]
        };
        let nothing = || vec![OutgoingMessage::Response(RpcResponse::ok(id.clone(), Value::Null))];

        // On an import's path: the file the path names.
        if let Some(path) =
            document::import_path_at_position(&doc.text, p.position.line, p.position.character)
        {
            return match document::import_target_uri(&path, &p.text_document.uri) {
                Some(uri) => located(uri, Range::default()),
                None => nothing(),
            };
        }

        let ident = match identifier_at_position(&doc.text, p.position.line, p.position.character) {
            Some(i) => i,
            None => return nothing(),
        };

        // `Kind.park`, `op.place`: where the enum declares that member.
        // (A name the enum does not have is looked up by name below.)
        let member = doc.analysis.member_at(&doc.text, p.position);
        if let Some(found) = member.and_then(|member| member.declaration(&ident)) {
            let range = document::span_to_range(&found.span);
            return match found.origin {
                Origin::Here => located(p.text_document.uri.clone(), range),
                Origin::File(uri) => located(uri.to_string(), range),
                Origin::Unknown => nothing(),
            };
        }

        // `m.name`: the declaration in the module `m` stands for.
        if let Some(module) =
            qualifier_at_position(&doc.text, p.position.line, p.position.character)
                .and_then(|q| doc.analysis.qualifier(&q))
        {
            return match module.export(&ident) {
                Some(found) => match &found.uri {
                    Some(uri) => located(uri.clone(), document::span_to_range(&found.def.span)),
                    None => nothing(),
                },
                None => nothing(),
            };
        }

        if let Some(def) = doc.analysis.definition_in_scope(&ident, p.position) {
            return located(
                p.text_document.uri.clone(),
                document::span_to_range(&def.span),
            );
        }

        // A qualifier itself (`hud` in `hud.draw`): the module's file.
        if let Some(uri) = doc.analysis.qualifier(&ident).and_then(|m| m.uri.clone()) {
            return located(uri, Range::default());
        }

        // A bare name an import brought in: its declaration, in its own file.
        if let Some(found) = doc.analysis.imported(&ident)
            && let Some(uri) = &found.uri
        {
            return located(uri.clone(), document::span_to_range(&found.def.span));
        }

        nothing()
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
                ))];
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
                ))];
            }
        };

        // The identifier under the cursor, or — the usual case while typing,
        // with the cursor just past its last character — the one ending there.
        let prefix = identifier_at_position(&doc.text, p.position.line, p.position.character)
            .or_else(|| {
                let before = p.position.character.checked_sub(1)?;
                identifier_at_position(&doc.text, p.position.line, before)
            })
            .unwrap_or_default();
        let item_for = |def: &Definition, detail: Option<String>| CompletionItem {
            label: def.name.clone(),
            kind: Some(match def.kind {
                DefinitionKind::Function => CompletionItemKind::Function,
                DefinitionKind::Enum => CompletionItemKind::Enum,
                _ => CompletionItemKind::Variable,
            }),
            detail,
        };
        let respond = |items: Vec<CompletionItem>| {
            vec![OutgoingMessage::Response(RpcResponse::ok(
                id.clone(),
                serde_json::to_value(CompletionList {
                    is_incomplete: false,
                    items,
                })
                .unwrap(),
            ))]
        };

        // On an import line: the modules that could be named there.
        if let Some(path) =
            document::import_path_prefix(&doc.text, p.position.line, p.position.character)
        {
            return respond(
                document::import_path_completions(&path, &p.text_document.uri)
                    .into_iter()
                    .map(|label| CompletionItem {
                        label,
                        kind: Some(CompletionItemKind::Variable),
                        detail: Some("module".to_string()),
                    })
                    .collect(),
            );
        }

        // After `Kind.`, or a variant value and a dot: the enum's members,
        // or the fields that value answers — and only those.
        if let Some(member) = doc.analysis.member_at(&doc.text, p.position) {
            let mut items = member.completions();
            items.retain(|item| item.label.starts_with(&prefix));
            return respond(items);
        }

        // After `m.`: what the module exports, and only that.
        if let Some(module) =
            qualifier_at_position(&doc.text, p.position.line, p.position.character)
                .and_then(|q| doc.analysis.qualifier(&q))
        {
            return respond(
                module
                    .exports
                    .iter()
                    .filter(|e| e.def.name.starts_with(&prefix))
                    .map(|e| item_for(&e.def, e.def.detail.clone()))
                    .collect(),
            );
        }

        let mut items: Vec<CompletionItem> = Vec::new();

        // Names the imports bind bare, and the qualifiers themselves. The
        // implicit prelude is left out of an empty-prefix list: it is every
        // `std` function, on every keystroke.
        let mut seen: std::collections::HashSet<&str> = doc
            .analysis
            .definitions
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        for module in doc.analysis.imports.iter().rev() {
            if module.implicit && prefix.is_empty() {
                continue;
            }
            if let Some(q) = &module.qualifier
                && !module.implicit
                && q.starts_with(&prefix)
                && seen.insert(q.as_str())
            {
                items.push(CompletionItem {
                    label: q.clone(),
                    kind: Some(CompletionItemKind::Variable),
                    detail: Some(format!("module {}", module.module)),
                });
            }
            for export in &module.exports {
                let name = export.def.name.as_str();
                if module.binds_bare(name) && name.starts_with(&prefix) && seen.insert(name) {
                    let detail = match &export.def.detail {
                        Some(d) => format!("{d} — {}", module.module),
                        None => module.module.clone(),
                    };
                    items.push(item_for(&export.def, Some(detail)));
                }
            }
        }

        for def in &doc.analysis.definitions {
            if !prefix.is_empty() && !def.name.starts_with(&prefix) {
                continue;
            }
            let kind = match def.kind {
                DefinitionKind::Variable => CompletionItemKind::Variable,
                DefinitionKind::Function => CompletionItemKind::Function,
                DefinitionKind::Enum => CompletionItemKind::Enum,
                DefinitionKind::Parameter | DefinitionKind::Loop => CompletionItemKind::Variable,
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

impl Server {
    // -----------------------------------------------------------------------
    // Document symbols
    // -----------------------------------------------------------------------

    fn handle_document_symbol(&self, id: Value, params: Value) -> Vec<OutgoingMessage> {
        let p: DocumentSymbolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(_) => {
                return vec![OutgoingMessage::Response(RpcResponse::err(
                    id,
                    INVALID_PARAMS,
                    "Invalid documentSymbol params".into(),
                ))];
            }
        };
        let symbols = self
            .documents
            .get(&p.text_document.uri)
            .map(|d| d.analysis.symbols.clone())
            .unwrap_or_default();
        vec![OutgoingMessage::Response(RpcResponse::ok(
            id,
            serde_json::to_value(symbols).unwrap(),
        ))]
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
