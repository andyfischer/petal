//! JSON-RPC 2.0 message layer for LSP.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Incoming messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RpcMessage {
    pub id: Option<Value>,
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

impl RpcMessage {
    pub fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }
}

// ---------------------------------------------------------------------------
// Outgoing messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError { code, message }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    pub params: Value,
}

impl RpcNotification {
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        }
    }
}

// ---------------------------------------------------------------------------
// Serialized outgoing — either a response or a notification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum OutgoingMessage {
    Response(RpcResponse),
    Notification(RpcNotification),
}

impl OutgoingMessage {
    pub fn to_json(&self) -> String {
        match self {
            OutgoingMessage::Response(r) => serde_json::to_string(r).unwrap(),
            OutgoingMessage::Notification(n) => serde_json::to_string(n).unwrap(),
        }
    }
}

// ---------------------------------------------------------------------------
// LSP content-length framing
// ---------------------------------------------------------------------------

pub fn encode_lsp_message(json: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", json.len(), json)
}

pub fn decode_lsp_messages(buf: &str) -> Vec<(String, usize)> {
    let mut results = Vec::new();
    let mut pos = 0;
    let bytes = buf.as_bytes();
    while pos < bytes.len() {
        let header_end = match find_header_end(bytes, pos) {
            Some(e) => e,
            None => break,
        };
        let header = &buf[pos..header_end];
        let content_length = match parse_content_length(header) {
            Some(len) => len,
            None => break,
        };
        let body_start = header_end + 4; // skip \r\n\r\n
        let body_end = body_start + content_length;
        if body_end > buf.len() {
            break;
        }
        results.push((buf[body_start..body_end].to_string(), body_end));
        pos = body_end;
    }
    results
}

fn find_header_end(bytes: &[u8], start: usize) -> Option<usize> {
    for i in start..bytes.len().saturating_sub(3) {
        if &bytes[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

fn parse_content_length(header: &str) -> Option<usize> {
    for line in header.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            return val.trim().parse().ok();
        }
    }
    None
}

// ---------------------------------------------------------------------------
// LSP error codes
// ---------------------------------------------------------------------------

pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const SERVER_NOT_INITIALIZED: i32 = -32002;
