//! Minimal LSP type definitions — just enough for the features we implement.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Positions and ranges
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

// ---------------------------------------------------------------------------
// Document identifiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentItem {
    pub uri: String,
    #[serde(rename = "languageId")]
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

// ---------------------------------------------------------------------------
// Text document content changes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentContentChangeEvent {
    pub range: Option<Range>,
    pub text: String,
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// An LSP enumeration: the protocol carries its members as integers, so it
/// is written and read as the number, never as the Rust name.
macro_rules! protocol_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant = $value),+
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_u8(*self as u8)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                match u8::deserialize(d)? {
                    $($value => Ok($name::$variant),)+
                    other => Err(serde::de::Error::custom(format!(
                        concat!("unknown ", stringify!($name), " {}"),
                        other
                    ))),
                }
            }
        }
    };
}

protocol_enum!(DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Option<DiagnosticSeverity>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

// ---------------------------------------------------------------------------
// Publish diagnostics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitializeParams {
    #[serde(default)]
    pub capabilities: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(rename = "textDocumentSync", skip_serializing_if = "Option::is_none")]
    pub text_document_sync: Option<TextDocumentSyncOptions>,
    #[serde(rename = "hoverProvider", skip_serializing_if = "Option::is_none")]
    pub hover_provider: Option<bool>,
    #[serde(rename = "definitionProvider", skip_serializing_if = "Option::is_none")]
    pub definition_provider: Option<bool>,
    #[serde(rename = "completionProvider", skip_serializing_if = "Option::is_none")]
    pub completion_provider: Option<CompletionOptions>,
    #[serde(rename = "documentSymbolProvider", skip_serializing_if = "Option::is_none")]
    pub document_symbol_provider: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentSyncOptions {
    #[serde(rename = "openClose")]
    pub open_close: bool,
    pub change: u8,
}

pub const TEXT_DOCUMENT_SYNC_FULL: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionOptions {
    #[serde(rename = "triggerCharacters", skip_serializing_if = "Option::is_none")]
    pub trigger_characters: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Hover
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentPositionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hover {
    pub contents: MarkupContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}

// ---------------------------------------------------------------------------
// Go to definition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

protocol_enum!(CompletionItemKind {
    Text = 1,
    Function = 3,
    Field = 5,
    Variable = 6,
    Enum = 13,
    Keyword = 14,
    Snippet = 15,
    EnumMember = 20,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub kind: Option<CompletionItemKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionList {
    #[serde(rename = "isIncomplete")]
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
}

// ---------------------------------------------------------------------------
// Document symbols
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSymbolParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
}

protocol_enum!(SymbolKind {
    Class = 5,
    Field = 8,
    Enum = 10,
    Function = 12,
    Variable = 13,
    EnumMember = 22,
});

/// One declaration in a document's outline, with the declarations inside it
/// (an enum's variants, a class's fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole declaration.
    pub range: Range,
    /// The declared name within it.
    #[serde(rename = "selectionRange")]
    pub selection_range: Range,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocumentSymbol>,
}

// ---------------------------------------------------------------------------
// Did open / change / close
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidOpenTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidChangeTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: VersionedTextDocumentIdentifier,
    #[serde(rename = "contentChanges")]
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidCloseTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
}
