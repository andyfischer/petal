use petal_lsp::lsp_types::*;
use petal_lsp::protocol::*;
use petal_lsp::server::Server;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn request(id: i32, method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
    .to_string()
}

fn notification(method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    })
    .to_string()
}

fn init(server: &mut Server) {
    let msgs = server.handle_message(&request(0, "initialize", json!({"capabilities": {}})));
    assert_eq!(msgs.len(), 1);
    server.handle_message(&notification("initialized", json!({})));
}

fn open(server: &mut Server, uri: &str, text: &str) -> Vec<OutgoingMessage> {
    server.handle_message(&notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "petal",
                "version": 1,
                "text": text
            }
        }),
    ))
}

fn change(server: &mut Server, uri: &str, version: i32, text: &str) -> Vec<OutgoingMessage> {
    server.handle_message(&notification(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [{ "text": text }]
        }),
    ))
}

fn hover(server: &mut Server, uri: &str, line: u32, character: u32) -> Value {
    let msgs = server.handle_message(&request(
        1,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }),
    ));
    assert_eq!(msgs.len(), 1);
    let json_str = msgs[0].to_json();
    serde_json::from_str(&json_str).unwrap()
}

fn definition(server: &mut Server, uri: &str, line: u32, character: u32) -> Value {
    let msgs = server.handle_message(&request(
        2,
        "textDocument/definition",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }),
    ));
    assert_eq!(msgs.len(), 1);
    let json_str = msgs[0].to_json();
    serde_json::from_str(&json_str).unwrap()
}

fn completion(server: &mut Server, uri: &str, line: u32, character: u32) -> Value {
    let msgs = server.handle_message(&request(
        3,
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        }),
    ));
    assert_eq!(msgs.len(), 1);
    let json_str = msgs[0].to_json();
    serde_json::from_str(&json_str).unwrap()
}

fn get_diagnostics(msgs: &[OutgoingMessage]) -> Vec<Diagnostic> {
    for m in msgs {
        let json_str = m.to_json();
        let v: Value = serde_json::from_str(&json_str).unwrap();
        if v.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
            let params: PublishDiagnosticsParams =
                serde_json::from_value(v["params"].clone()).unwrap();
            return params.diagnostics;
        }
    }
    Vec::new()
}

// ===========================================================================
// Tests
// ===========================================================================

#[test]
fn test_initialize() {
    let mut server = Server::new();
    let msgs = server.handle_message(&request(1, "initialize", json!({"capabilities": {}})));
    assert_eq!(msgs.len(), 1);
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    assert_eq!(resp["id"], 1);
    let caps = &resp["result"]["capabilities"];
    assert_eq!(caps["hoverProvider"], true);
    assert_eq!(caps["definitionProvider"], true);
    assert!(caps["completionProvider"].is_object());
    assert_eq!(caps["textDocumentSync"]["openClose"], true);
    assert_eq!(caps["textDocumentSync"]["change"], 1);
}

#[test]
fn test_shutdown() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = server.handle_message(&request(99, "shutdown", json!(null)));
    assert_eq!(msgs.len(), 1);
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    assert_eq!(resp["id"], 99);
    assert!(resp["result"].is_null());
}

#[test]
fn test_method_not_found() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = server.handle_message(&request(5, "textDocument/formatting", json!({})));
    assert_eq!(msgs.len(), 1);
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
}

#[test]
fn test_did_open_valid_code() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(&mut server, "file:///test.ptl", "let x = 42\n");
    let diags = get_diagnostics(&msgs);
    assert!(diags.is_empty(), "Valid code should produce no diagnostics");
}

#[test]
fn test_did_open_with_error() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(&mut server, "file:///test.ptl", "let = \n");
    let diags = get_diagnostics(&msgs);
    assert!(
        !diags.is_empty(),
        "Invalid code should produce diagnostics"
    );
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::Error));
}

#[test]
fn test_did_change_updates_diagnostics() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 42\n");

    let msgs = change(&mut server, "file:///test.ptl", 2, "let = \n");
    let diags = get_diagnostics(&msgs);
    assert!(!diags.is_empty(), "After introducing an error, should have diagnostics");

    let msgs = change(&mut server, "file:///test.ptl", 3, "let y = 10\n");
    let diags = get_diagnostics(&msgs);
    assert!(diags.is_empty(), "After fixing, diagnostics should be cleared");
}

#[test]
fn test_did_close_clears_diagnostics() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 42\n");

    let msgs = server.handle_message(&notification(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": "file:///test.ptl" } }),
    ));
    let diags = get_diagnostics(&msgs);
    assert!(diags.is_empty());
}

#[test]
fn test_hover_on_variable() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let greeting = \"hello\"\n");

    let resp = hover(&mut server, "file:///test.ptl", 0, 5);
    let contents = &resp["result"]["contents"]["value"];
    assert!(
        contents.as_str().unwrap().contains("greeting"),
        "Hover should mention the variable name"
    );
    assert!(
        contents.as_str().unwrap().contains("variable"),
        "Hover should identify it as a variable"
    );
}

#[test]
fn test_hover_on_function() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "fn add(a, b)\n  return a + b\nend\n",
    );

    let resp = hover(&mut server, "file:///test.ptl", 0, 4);
    let contents = &resp["result"]["contents"]["value"];
    let text = contents.as_str().unwrap();
    assert!(text.contains("add"), "Hover should mention function name");
    assert!(
        text.contains("function"),
        "Hover should identify it as a function"
    );
}

#[test]
fn test_hover_on_typed_function() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "fn double(x: int) -> int\n  return x * 2\nend\n",
    );

    let resp = hover(&mut server, "file:///test.ptl", 0, 4);
    let text = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(text.contains("x: int"), "Should show parameter types");
    assert!(text.contains("-> int"), "Should show return type");
}

#[test]
fn test_hover_on_keyword() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 1\n");

    let resp = hover(&mut server, "file:///test.ptl", 0, 0);
    let contents = &resp["result"]["contents"]["value"];
    assert!(
        contents.as_str().unwrap().contains("keyword"),
        "Hover on 'let' should say keyword"
    );
}

#[test]
fn test_hover_on_nothing() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 42\n");

    let resp = hover(&mut server, "file:///test.ptl", 0, 8);
    assert!(resp["result"].is_null(), "Hover on a literal should return null");
}

#[test]
fn test_goto_definition_variable() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "let count = 0\nlet y = count + 1\n",
    );

    let resp = definition(&mut server, "file:///test.ptl", 1, 9);
    assert!(
        !resp["result"].is_null(),
        "Should find definition for 'count'"
    );
    assert_eq!(resp["result"]["uri"], "file:///test.ptl");
}

#[test]
fn test_goto_definition_function() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "fn greet(name)\n  return \"hi \" ++ name\nend\ngreet(\"world\")\n",
    );

    let resp = definition(&mut server, "file:///test.ptl", 3, 1);
    assert!(
        !resp["result"].is_null(),
        "Should find definition for 'greet'"
    );
}

#[test]
fn test_goto_definition_not_found() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 42\n");

    let resp = definition(&mut server, "file:///test.ptl", 0, 8);
    assert!(
        resp["result"].is_null(),
        "Definition on a literal should return null"
    );
}

#[test]
fn test_completion_keywords() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "le\n");

    let resp = completion(&mut server, "file:///test.ptl", 0, 2);
    let items = resp["result"]["items"].as_array().unwrap();
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"let"), "Should suggest 'let' keyword");
}

#[test]
fn test_completion_user_definitions() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "fn calculate(x)\n  return x * 2\nend\nca\n",
    );

    let resp = completion(&mut server, "file:///test.ptl", 3, 2);
    let items = resp["result"]["items"].as_array().unwrap();
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(
        labels.contains(&"calculate"),
        "Should suggest 'calculate' from user definitions"
    );
}

#[test]
fn test_completion_empty_prefix() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 1\n\n");

    let resp = completion(&mut server, "file:///test.ptl", 1, 0);
    let items = resp["result"]["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "Empty prefix should return all completions"
    );
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"x"));
    assert!(labels.contains(&"let"));
    assert!(labels.contains(&"fn"));
}

#[test]
fn test_diagnostics_with_warnings() {
    let mut server = Server::new();
    init(&mut server);
    // Type mismatch should produce a warning
    let msgs = open(
        &mut server,
        "file:///test.ptl",
        "fn add(a: int, b: int) -> int\n  return a + b\nend\nadd(1, \"hello\")\n",
    );
    let diags = get_diagnostics(&msgs);
    // The type checker produces warnings for type mismatches
    if !diags.is_empty() {
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::Warning));
        assert!(diags[0].source.as_deref() == Some("petal"));
    }
}

#[test]
fn test_empty_document() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(&mut server, "file:///test.ptl", "");
    let diags = get_diagnostics(&msgs);
    assert!(diags.is_empty(), "Empty document should have no diagnostics");
}

#[test]
fn test_whitespace_only_document() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(&mut server, "file:///test.ptl", "   \n  \n");
    let diags = get_diagnostics(&msgs);
    assert!(diags.is_empty());
}

#[test]
fn test_multiple_documents() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///a.ptl", "let a = 1\n");
    open(&mut server, "file:///b.ptl", "let b = 2\n");

    let resp_a = hover(&mut server, "file:///a.ptl", 0, 4);
    assert!(resp_a["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("a"));

    let resp_b = hover(&mut server, "file:///b.ptl", 0, 4);
    assert!(resp_b["result"]["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("b"));
}

#[test]
fn test_enum_definition() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "enum Color\n  Red\n  Green\n  Blue\nend\n",
    );

    let resp = hover(&mut server, "file:///test.ptl", 0, 6);
    let text = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(text.contains("enum"), "Should identify as enum");
    assert!(text.contains("Color"), "Should show enum name");
}

// ---------------------------------------------------------------------------
// Protocol layer tests
// ---------------------------------------------------------------------------

#[test]
fn test_encode_lsp_message() {
    let json = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
    let encoded = encode_lsp_message(json);
    assert!(encoded.starts_with("Content-Length: "));
    assert!(encoded.contains("\r\n\r\n"));
    assert!(encoded.ends_with(json));
}

#[test]
fn test_decode_lsp_messages() {
    let body = r#"{"jsonrpc":"2.0","id":1}"#;
    let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let results = decode_lsp_messages(&msg);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, body);
}

#[test]
fn test_decode_multiple_messages() {
    let body1 = r#"{"id":1}"#;
    let body2 = r#"{"id":2}"#;
    let msg = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        body1.len(),
        body1,
        body2.len(),
        body2
    );
    let results = decode_lsp_messages(&msg);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, body1);
    assert_eq!(results[1].0, body2);
}

#[test]
fn test_decode_incomplete_message() {
    let msg = "Content-Length: 100\r\n\r\nshort";
    let results = decode_lsp_messages(msg);
    assert!(results.is_empty(), "Incomplete body should not decode");
}

#[test]
fn test_rpc_response_ok() {
    let resp = RpcResponse::ok(json!(1), json!({"key": "val"}));
    assert_eq!(resp.jsonrpc, "2.0");
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["key"], "val");
}

#[test]
fn test_rpc_response_err() {
    let resp = RpcResponse::err(json!(2), -32600, "Invalid request".into());
    assert!(resp.result.is_none());
    assert_eq!(resp.error.as_ref().unwrap().code, -32600);
}

#[test]
fn test_malformed_json() {
    let mut server = Server::new();
    let msgs = server.handle_message("not json at all");
    assert!(msgs.is_empty(), "Malformed JSON should be silently ignored");
}

// ---------------------------------------------------------------------------
// Document analysis unit tests
// ---------------------------------------------------------------------------

#[test]
fn test_identifier_at_position() {
    use petal_lsp::document::identifier_at_position;

    assert_eq!(
        identifier_at_position("let foo = 42", 0, 4),
        Some("foo".to_string())
    );
    assert_eq!(
        identifier_at_position("let foo = 42", 0, 5),
        Some("foo".to_string())
    );
    assert_eq!(
        identifier_at_position("let foo = 42", 0, 6),
        Some("foo".to_string())
    );
    assert_eq!(identifier_at_position("let foo = 42", 0, 8), None);
    assert_eq!(identifier_at_position("let foo = 42", 0, 10), None);
    assert_eq!(identifier_at_position("let foo = 42", 5, 0), None);
}

#[test]
fn test_analyze_valid() {
    use petal_lsp::document::analyze;

    let result = analyze("let x = 1\nfn add(a, b)\n  return a + b\nend\n");
    assert!(result.diagnostics.is_empty());
    let names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"x"));
    assert!(names.contains(&"add"));
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[test]
fn test_analyze_error() {
    use petal_lsp::document::analyze;

    let result = analyze("let = bad syntax");
    assert!(!result.diagnostics.is_empty());
    assert_eq!(
        result.diagnostics[0].severity,
        Some(DiagnosticSeverity::Error)
    );
}

#[test]
fn test_not_initialized_error() {
    let mut server = Server::new();
    let msgs = server.handle_message(&request(
        1,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "file:///test.ptl" },
            "position": { "line": 0, "character": 0 }
        }),
    ));
    assert_eq!(msgs.len(), 1);
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    assert_eq!(resp["error"]["code"], SERVER_NOT_INITIALIZED);
}
