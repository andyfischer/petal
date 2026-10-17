use petal::lsp::lsp_types::*;
use petal::lsp::protocol::*;
use petal::lsp::server::Server;
use serde_json::{Value, json};

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
    assert!(!diags.is_empty(), "Invalid code should produce diagnostics");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::Error));
}

#[test]
fn test_did_change_updates_diagnostics() {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///test.ptl", "let x = 42\n");

    let msgs = change(&mut server, "file:///test.ptl", 2, "let = \n");
    let diags = get_diagnostics(&msgs);
    assert!(
        !diags.is_empty(),
        "After introducing an error, should have diagnostics"
    );

    let msgs = change(&mut server, "file:///test.ptl", 3, "let y = 10\n");
    let diags = get_diagnostics(&msgs);
    assert!(
        diags.is_empty(),
        "After fixing, diagnostics should be cleared"
    );
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
    open(
        &mut server,
        "file:///test.ptl",
        "let greeting = \"hello\"\n",
    );

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
    assert!(
        resp["result"].is_null(),
        "Hover on a literal should return null"
    );
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
    assert!(
        diags.is_empty(),
        "Empty document should have no diagnostics"
    );
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
    assert!(
        resp_a["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("a")
    );

    let resp_b = hover(&mut server, "file:///b.ptl", 0, 4);
    assert!(
        resp_b["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("b")
    );
}

#[test]
fn test_enum_definition() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "enum Color\n  Red,\n  Green,\n  Blue,\nend\n",
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
    use petal::lsp::document::identifier_at_position;

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
    use petal::lsp::document::analyze;

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
    use petal::lsp::document::analyze;

    let result = analyze("let = bad syntax");
    assert!(!result.diagnostics.is_empty());
    assert_eq!(
        result.diagnostics[0].severity,
        Some(DiagnosticSeverity::Error)
    );
}

#[test]
fn test_analyze_compile_error_has_real_position() {
    use petal::lsp::document::analyze;

    // `x = 2` on a `var` must use `set`. The diagnostic belongs on line 2,
    // column 1 (0-based 1, 0) — not at the top of the file.
    let result = analyze("var x = 1\nx = 2\n");
    assert_eq!(result.diagnostics.len(), 1);
    let d = &result.diagnostics[0];
    assert_eq!(d.severity, Some(DiagnosticSeverity::Error));
    assert_eq!(
        (d.range.start.line, d.range.start.character),
        (1, 0),
        "compiler diagnostic should be reported at its own span, got {:?}",
        d.range
    );
    assert!(
        !d.message.contains("[line "),
        "message should not carry a position suffix: {}",
        d.message
    );
}

#[test]
fn test_analyze_reports_every_compile_error() {
    use petal::lsp::document::analyze;

    let result = analyze("var x = 1\nvar y = 2\nx = 3\ny = 4\n");
    assert_eq!(
        result.diagnostics.len(),
        2,
        "two compile errors should yield two diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    let lines: Vec<u32> = result
        .diagnostics
        .iter()
        .map(|d| d.range.start.line)
        .collect();
    assert_eq!(lines, vec![2, 3]);
}

#[test]
fn test_analyze_keeps_definitions_after_compile_error() {
    use petal::lsp::document::analyze;

    let result = analyze("fn helper(a)\n  return a\nend\nvar x = 1\nx = 2\n");
    assert!(!result.diagnostics.is_empty());
    let names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.contains(&"helper"),
        "definitions should survive a semantic error, got {names:?}"
    );
    assert!(names.contains(&"x"));
}

#[test]
fn test_goto_definition_after_compile_error() {
    let mut server = Server::new();
    init(&mut server);
    open(
        &mut server,
        "file:///test.ptl",
        "fn helper(a)\n  return a\nend\nvar x = 1\nx = 2\nhelper(1)\n",
    );

    let resp = definition(&mut server, "file:///test.ptl", 5, 1);
    assert!(
        !resp["result"].is_null(),
        "go-to-definition should still work when the file has a compile error"
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

// ===========================================================================
// Imports: diagnostics, go-to-definition, hover and completion across files
// ===========================================================================

/// A scratch project on disk, removed on drop. `uri(rel)` is the `file://`
/// URI of a file in it.
struct Project(std::path::PathBuf);

impl Project {
    fn new(tag: &str, files: &[(&str, &str)]) -> Project {
        let root = std::env::temp_dir().join(format!("petal-lsp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project(std::fs::canonicalize(&root).unwrap())
    }

    fn uri(&self, rel: &str) -> String {
        petal::lsp::document::path_to_uri(&self.0.join(rel))
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const HUD: &str = "pub fn draw_score(n: int) -> string\n  \"score\"\nend\n\npub let margin = 4\n\nfn private_helper()\n  1\nend\n";

fn labels(resp: &Value) -> Vec<String> {
    resp["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_relative_import_resolves_from_the_documents_own_path() {
    let project = Project::new("diag", &[("hud.ptl", HUD)]);
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    let diags = get_diagnostics(&open(
        &mut server,
        &uri,
        "import ./hud\nprint(draw_score(1))\nprint(hud.margin)\n",
    ));
    assert!(diags.is_empty(), "{diags:?}");

    // The old spelling is an error that says what to write instead.
    let diags = get_diagnostics(&change(&mut server, &uri, 2, "import hud\nprint(1)\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.contains("`import ./hud`"), "{diags:?}");
}

#[test]
fn an_ambiguous_bare_name_is_a_diagnostic_at_the_use() {
    let project = Project::new(
        "ambiguous",
        &[
            ("hud.ptl", "pub fn draw()\n  1\nend\n"),
            ("level.ptl", "pub fn draw()\n  2\nend\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    let diags = get_diagnostics(&open(
        &mut server,
        &uri,
        "import ./hud\nimport ./level\nprint(hud.draw())\nprint(draw())\n",
    ));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.contains("`draw` is ambiguous"), "{diags:?}");
    assert_eq!(diags[0].range.start.line, 3);
    assert_eq!(diags[0].range.start.character, 6);
}

#[test]
fn goto_definition_follows_a_bare_and_a_qualified_imported_name() {
    let project = Project::new("goto", &[("hud.ptl", HUD)]);
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    open(
        &mut server,
        &uri,
        "import ./hud\nprint(draw_score(1))\nprint(hud.margin)\n",
    );
    // `draw_score`, used bare.
    let resp = definition(&mut server, &uri, 1, 8);
    assert_eq!(resp["result"]["uri"], json!(project.uri("hud.ptl")), "{resp}");
    assert_eq!(resp["result"]["range"]["start"]["line"], 0);
    // `margin` in `hud.margin`.
    let resp = definition(&mut server, &uri, 2, 11);
    assert_eq!(resp["result"]["uri"], json!(project.uri("hud.ptl")), "{resp}");
    assert_eq!(resp["result"]["range"]["start"]["line"], 4);
    // The qualifier `hud`, and the import path itself: the module's file.
    for (line, character) in [(2, 7), (0, 10)] {
        let resp = definition(&mut server, &uri, line, character);
        assert_eq!(resp["result"]["uri"], json!(project.uri("hud.ptl")), "{resp}");
        assert_eq!(resp["result"]["range"]["start"]["line"], 0);
    }
}

#[test]
fn goto_definition_follows_a_re_export_to_the_declaring_file() {
    let project = Project::new(
        "reexport",
        &[
            ("kit/petal.toml", "[package]\nname = \"kit\"\n"),
            ("kit/src/kit.ptl", "pub import ./menu\n"),
            ("kit/src/menu.ptl", "// the menu\npub fn open()\n  1\nend\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    // A file inside the package reaches it by name with no configuration.
    let uri = project.uri("kit/examples/demo.ptl");
    let diags = get_diagnostics(&open(&mut server, &uri, "import kit\nprint(open())\n"));
    assert!(diags.is_empty(), "{diags:?}");
    let resp = definition(&mut server, &uri, 1, 7);
    assert_eq!(
        resp["result"]["uri"],
        json!(project.uri("kit/src/menu.ptl")),
        "{resp}"
    );
    assert_eq!(resp["result"]["range"]["start"]["line"], 1);
}

#[test]
fn hover_on_an_imported_name_shows_its_signature_and_module() {
    let project = Project::new("hover", &[("hud.ptl", HUD)]);
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    open(&mut server, &uri, "import ./hud as h\nprint(draw_score(1))\nprint(h.margin)\n");
    let resp = hover(&mut server, &uri, 1, 8);
    let text = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(text.contains("draw_score fn(n: int) -> string"), "{text}");
    assert!(text.contains("from module hud"), "{text}");
    let resp = hover(&mut server, &uri, 2, 9);
    let text = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(text.contains("(variable) margin"), "{text}");
}

#[test]
fn completion_offers_bare_imported_names_and_a_modules_exports_after_its_qualifier() {
    let project = Project::new("complete", &[("hud.ptl", HUD), ("level.ptl", "pub let depth = 1\n")]);
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    open(
        &mut server,
        &uri,
        "import ./hud\nimport ./level: depth\nlet mine = 1\n\nprint(mine)\n",
    );
    // No prefix: locals, what the imports bind bare, the qualifier, keywords
    // — and nothing private, and not the selective import's qualifier.
    let all = labels(&completion(&mut server, &uri, 3, 0));
    for expected in ["mine", "draw_score", "margin", "depth", "hud", "let"] {
        assert!(all.contains(&expected.to_string()), "{expected} in {all:?}");
    }
    assert!(!all.contains(&"private_helper".to_string()), "{all:?}");
    assert!(!all.contains(&"level".to_string()), "{all:?}");

    // Mid-edit the file does not even parse; the imports are still known.
    change(
        &mut server,
        &uri,
        2,
        "import ./hud\nimport ./level: depth\ndra\nhud.\nhud.ma\n",
    );
    // A bare prefix: the plain import's export, with where it is from.
    let resp = completion(&mut server, &uri, 2, 3);
    let items = resp["result"]["items"].as_array().unwrap();
    let draw = items.iter().find(|i| i["label"] == "draw_score").expect("draw_score");
    assert!(draw["detail"].as_str().unwrap().contains("hud"), "{draw}");
    // After the qualifier: that module's exports and nothing else.
    let mut after_dot = labels(&completion(&mut server, &uri, 3, 4));
    after_dot.sort();
    assert_eq!(after_dot, ["draw_score", "margin"]);
    assert_eq!(labels(&completion(&mut server, &uri, 4, 6)), ["margin"]);
}

#[test]
fn completion_on_an_import_line_offers_sibling_files_and_packages() {
    let project = Project::new(
        "complete-path",
        &[("hud.ptl", HUD), ("ui/hint.ptl", "pub let h = 1\n"), ("notes.txt", "")],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    open(&mut server, &uri, "import ./\nimport ./ui/h\nimport co\nprint(1)\n");
    assert_eq!(labels(&completion(&mut server, &uri, 0, 9)), ["hud", "ui"]);
    assert_eq!(labels(&completion(&mut server, &uri, 1, 13)), ["hint"]);
    // No relative prefix: package names. `core` is bundled but always in
    // scope, so only its modules are offered, under its name.
    let packages = labels(&completion(&mut server, &uri, 2, 9));
    assert!(!packages.contains(&"hud".to_string()), "{packages:?}");
}

#[test]
fn an_unsaved_edit_to_an_open_module_is_what_its_importer_sees() {
    let project = Project::new("unsaved", &[("hud.ptl", HUD)]);
    let mut server = Server::new();
    init(&mut server);
    let hud = project.uri("hud.ptl");
    let main = project.uri("main.ptl");
    // The editor's copy of hud.ptl has a function the file on disk lacks.
    open(&mut server, &hud, &format!("{HUD}pub fn fresh()\n  1\nend\n"));
    let diags = get_diagnostics(&open(&mut server, &main, "import ./hud\nprint(fresh())\n"));
    assert!(diags.is_empty(), "{diags:?}");
    let resp = definition(&mut server, &main, 1, 7);
    assert_eq!(resp["result"]["uri"], json!(hud), "{resp}");
}

#[test]
fn a_file_uri_round_trips_through_a_path() {
    use petal::lsp::document::{path_to_uri, uri_to_path};
    let path = std::path::Path::new("/tmp/my game/level 1.ptl");
    let uri = path_to_uri(path);
    assert_eq!(uri, "file:///tmp/my%20game/level%201.ptl");
    assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    assert_eq!(uri_to_path("untitled:Untitled-1"), None);
}

// ===========================================================================
// Enums: members after `Enum.`, fields of a variant value, the outline
// ===========================================================================

const ENUMS: &str = r#"enum Use
  road, green
end
enum Kind
  parkway(shape: "route", use: Use.road),
  park(shape: "area", use: Use.green),
end
enum Op
  fight(place, won, tone: "bad"),
  gift(amount, tone: "good"),
end
fn describe(k: Kind, op: Op) -> Kind
  let here = Kind.park
  let hit = Op.fight("x", true)
  print(k.shape, op.place, here.name, hit.won)
  for each in Kind.all do
    print(each.use)
  end
  match op
    when Op.gift(amount) -> print(amount)
    when Op.fight(_, _) -> nil
  end
  Kind.from("park")
end
"#;

/// The position `offset` characters into the first occurrence of `needle`.
fn at(text: &str, needle: &str, offset: u32) -> (u32, u32) {
    position(text, needle, offset, false)
}

fn position(text: &str, needle: &str, offset: u32, last: bool) -> (u32, u32) {
    let mut lines = text.lines().enumerate().filter(|(_, l)| l.contains(needle));
    let found = if last { lines.last() } else { lines.next() };
    let (line, src) = found.unwrap_or_else(|| panic!("no `{needle}` in the text"));
    let col = if last { src.rfind(needle) } else { src.find(needle) }.unwrap();
    (line as u32, src[..col].chars().count() as u32 + offset)
}

/// `ENUMS` with `line` added as the last line of `describe`'s body — a line
/// being typed, which need not parse.
fn typing(line: &str) -> String {
    ENUMS.replace("  Kind.from(\"park\")\n", &format!("  Kind.from(\"park\")\n  {line}\n"))
}

/// The labels completion offers at the end of the line being typed.
fn complete_after(typed: &str) -> Vec<String> {
    let mut server = Server::new();
    init(&mut server);
    let text = typing(typed);
    open(&mut server, "file:///enums.ptl", &text);
    let (line, col) = position(&text, typed, typed.chars().count() as u32, true);
    labels(&completion(&mut server, "file:///enums.ptl", line, col))
}

fn hover_text(server: &mut Server, uri: &str, text: &str, needle: &str, offset: u32) -> String {
    let (line, col) = at(text, needle, offset);
    let resp = hover(server, uri, line, col);
    resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no hover on `{needle}`+{offset}: {resp}"))
        .to_string()
}

#[test]
fn completion_after_an_enum_offers_its_variants_all_and_from() {
    // Nothing after the dot yet: the document does not parse, and the enum's
    // declaration is still what the answer is made of.
    assert_eq!(complete_after("let a = Kind."), ["parkway", "park", "all", "from"]);
    assert_eq!(complete_after("let a = Kind.pa"), ["parkway", "park"]);
    assert_eq!(complete_after("print(Op."), ["fight", "gift", "all", "from"]);
    // In a pattern, and in a condition whose `then` is not written yet.
    assert_eq!(complete_after("if k == Kind.f"), ["from"]);
    // A name that is not an enum gets no enum members.
    assert!(!complete_after("let a = describe.").contains(&"all".to_string()));
}

#[test]
fn completion_after_a_variant_value_offers_its_fields_data_and_name() {
    // One known variant: its own data, and `name`.
    assert_eq!(complete_after("Kind.park."), ["shape", "use", "name"]);
    // A constructed payload variant, written out or held in a `let`.
    assert_eq!(complete_after("Op.fight(1, 2)."), ["place", "won", "tone", "name"]);
    assert_eq!(complete_after("hit."), ["place", "won", "tone", "name"]);
    assert_eq!(complete_after("here.s"), ["shape"]);
    // Some variant of an annotated enum: the data every variant has, `name`,
    // and each payload field with the variants that carry it.
    assert_eq!(complete_after("k."), ["shape", "use", "name"]);
    assert_eq!(complete_after("op."), ["tone", "name", "place", "won", "amount"]);
    // A loop over `Enum.all`, and `Enum.from(…)`, yield values of the enum.
    assert_eq!(complete_after("Kind.from(\"park\")."), ["shape", "use", "name"]);
    // An uncalled constructor and the `all` list are not variant values.
    assert!(complete_after("Op.fight.").is_empty());
    assert!(complete_after("Kind.all.").is_empty());

    let mut server = Server::new();
    init(&mut server);
    let text = ENUMS.replace("print(each.use)", "print(each.)");
    open(&mut server, "file:///loop.ptl", &text);
    let (line, col) = at(&text, "each.)", 5);
    assert_eq!(
        labels(&completion(&mut server, "file:///loop.ptl", line, col)),
        ["shape", "use", "name"]
    );
}

#[test]
fn completion_items_carry_protocol_kinds_as_numbers() {
    let mut server = Server::new();
    init(&mut server);
    let text = typing("Kind.");
    open(&mut server, "file:///kinds.ptl", &text);
    let (line, col) = position(&text, "Kind.", 5, true);
    let resp = completion(&mut server, "file:///kinds.ptl", line, col);
    let items = resp["result"]["items"].as_array().unwrap();
    let kind = |label: &str| items.iter().find(|i| i["label"] == label).unwrap()["kind"].clone();
    assert_eq!(kind("park"), json!(20), "EnumMember");
    assert_eq!(kind("all"), json!(5), "Field");
    assert_eq!(kind("from"), json!(3), "Function");
    let park = items.iter().find(|i| i["label"] == "park").unwrap();
    assert_eq!(park["detail"], "Kind.park");
}

#[test]
fn hover_describes_an_enum_its_variants_and_a_variant_values_fields() {
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///hover.ptl";
    open(&mut server, uri, ENUMS);
    let mut on = |needle: &str, offset: u32| hover_text(&mut server, uri, ENUMS, needle, offset);

    // The enum, where it is declared and where it is used as a type.
    assert!(on("enum Op", 5).contains("(enum) Op fight(place, won) | gift(amount)"));
    assert!(on("k: Kind", 3).contains("(enum) Kind parkway | park"));
    // A variant, with its constant data; a payload variant, with its fields.
    let park = on("Kind.park\n".trim_end(), 5);
    assert!(park.contains("(variant) Kind.park"), "{park}");
    assert!(park.contains("shape: \"area\"") && park.contains("use: Use.green"), "{park}");
    assert!(on("Op.fight(\"x\"", 3).contains("(variant) Op.fight(place, won)"));
    // In a pattern too.
    assert!(on("when Op.gift", 8).contains("(variant) Op.gift(amount)"));
    // `all` and `from`.
    let all = on("Kind.all", 5);
    assert!(all.contains("Kind.all") && all.contains("[Kind.parkway, Kind.park]"), "{all}");
    assert!(on("Kind.from", 5).contains("Kind.from(name)"));
    // Fields of a variant value: one known variant, then a value of the enum.
    assert!(on("here.name", 5).contains("Kind.park.name = \"park\""));
    assert!(on("hit.won", 4).contains("(payload field) won"));
    let shape = on("k.shape", 2);
    assert!(shape.contains("(constant data) Kind.shape"), "{shape}");
    assert!(shape.contains("parkway: \"route\"") && shape.contains("park: \"area\""), "{shape}");
    assert!(on("op.place", 3).contains("of Op.fight(place, won)"));
    assert!(on("each.use", 5).contains("(constant data) Kind.use"));
    // A parameter still hovers as itself.
    assert!(on("k.shape", 0).contains("(parameter) k : Kind"));
}

#[test]
fn a_name_the_enum_does_not_have_is_still_looked_up_by_name() {
    // `here.describe` is no field of a `Kind`; the editor still answers for
    // the `describe` the file declares rather than going quiet.
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///method.ptl";
    let text = "enum Kind\n  park(shape: \"area\"),\nend\nfn describe(k: Kind)\n  k.shape\nend\nlet here = Kind.park\nprint(here.describe())\n";
    open(&mut server, uri, text);
    assert!(hover_text(&mut server, uri, text, "here.describe", 6).contains("(function) describe fn(k: Kind)"));
    let (line, col) = at(text, "here.describe", 6);
    let resp = definition(&mut server, uri, line, col);
    assert_eq!(resp["result"]["range"]["start"]["line"], 3, "{resp}");
}

#[test]
fn goto_definition_on_an_enum_and_on_its_members() {
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///goto.ptl";
    open(&mut server, uri, ENUMS);
    let mut line_of = |needle: &str, offset: u32| {
        let (line, col) = at(ENUMS, needle, offset);
        let resp = definition(&mut server, uri, line, col);
        assert_eq!(resp["result"]["uri"], uri, "`{needle}`+{offset}: {resp}");
        resp["result"]["range"]["start"]["line"].as_u64().unwrap()
    };
    // The enum's name: its declaration.
    assert_eq!(line_of("k: Kind", 3), 3);
    assert_eq!(line_of("Kind.park\n".trim_end(), 1), 3);
    // `Enum.variant`: the variant's own line.
    assert_eq!(line_of("Kind.park\n".trim_end(), 5), 5);
    assert_eq!(line_of("Op.fight(\"x\"", 3), 8);
    assert_eq!(line_of("when Op.gift", 8), 9);
    // `all` / `from`: the enum.
    assert_eq!(line_of("Kind.all", 5), 3);
    // A field of a known variant: that variant. Of an enum's value: the
    // first variant that declares it.
    assert_eq!(line_of("here.name", 5), 5);
    assert_eq!(line_of("hit.won", 4), 8);
    assert_eq!(line_of("op.place", 3), 8);
    assert_eq!(line_of("k.shape", 2), 4);
}

#[test]
fn enum_members_resolve_through_an_import_and_a_module_qualifier() {
    let project = Project::new(
        "enum-import",
        &[("town.ptl", "// kinds of place\npub enum Kind\n  parkway(shape: \"route\"),\n  park(shape: \"area\"),\nend\n")],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    let text = "import ./town\nlet a = Kind.park\nlet b = town.Kind.parkway\nprint(a.shape, b.name)\n";
    assert!(get_diagnostics(&open(&mut server, &uri, text)).is_empty());

    // Go-to-definition lands on the variant, in the module's file.
    for (needle, offset, line) in [("Kind.park", 5, 3), ("town.Kind.parkway", 10, 2), ("a.shape", 2, 3)] {
        let (l, c) = at(text, needle, offset);
        let resp = definition(&mut server, &uri, l, c);
        assert_eq!(resp["result"]["uri"], json!(project.uri("town.ptl")), "{needle}: {resp}");
        assert_eq!(resp["result"]["range"]["start"]["line"], line, "{needle}: {resp}");
    }
    assert!(hover_text(&mut server, &uri, text, "town.Kind.parkway", 10).contains("shape: \"route\""));

    // Completion, mid-edit, bare and qualified.
    let typing = "import ./town\nlet a = Kind.\nlet b = town.Kind.\nlet c = town.Kind.park.\n";
    change(&mut server, &uri, 2, typing);
    assert_eq!(labels(&completion(&mut server, &uri, 1, 13)), ["parkway", "park", "all", "from"]);
    assert_eq!(labels(&completion(&mut server, &uri, 2, 18)), ["parkway", "park", "all", "from"]);
    assert_eq!(labels(&completion(&mut server, &uri, 3, 23)), ["shape", "name"]);
}

#[test]
fn document_symbols_outline_enums_classes_functions_and_bindings() {
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///outline.ptl";
    let text = format!("{ENUMS}class Point\n  x: int,\n  y: int,\nend\nlet origin = Point(0, 0)\nstate count = 0\n");
    open(&mut server, uri, &text);
    let msgs = server.handle_message(&request(
        4,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    ));
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    let symbols = resp["result"].as_array().expect("symbols");
    let summary: Vec<(String, u64)> = symbols
        .iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), s["kind"].as_u64().unwrap()))
        .collect();
    let expect = [("Use", 10), ("Kind", 10), ("Op", 10), ("describe", 12), ("Point", 5), ("origin", 13), ("count", 13)];
    assert_eq!(summary, expect.map(|(n, k)| (n.to_string(), k)));

    // An enum's variants are its children, each with its payload and data.
    let op = &symbols[2];
    let variants: Vec<(&str, u64, &str)> = op["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["kind"].as_u64().unwrap(), c["detail"].as_str().unwrap()))
        .collect();
    assert_eq!(
        variants,
        [("fight", 22, "(place, won, tone: \"bad\")"), ("gift", 22, "(amount, tone: \"good\")")]
    );
    assert!(symbols[0]["children"][0].get("detail").is_none(), "a bare variant has no detail");
    // The selection range is the name; the range is the whole declaration.
    assert_eq!(op["selectionRange"]["start"], json!({"line": 7, "character": 5}));
    assert_eq!(op["selectionRange"]["end"], json!({"line": 7, "character": 7}));
    assert_eq!(op["range"]["start"]["line"], 7);
    assert_eq!(op["range"]["end"]["line"], 10);
    assert_eq!(op["children"][1]["selectionRange"]["start"], json!({"line": 9, "character": 2}));
    // A class's fields likewise.
    let fields: Vec<&str> = symbols[4]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["x", "y"]);

    let caps = server.handle_message(&request(5, "initialize", json!({"capabilities": {}})));
    let caps: Value = serde_json::from_str(&caps[0].to_json()).unwrap();
    assert_eq!(caps["result"]["capabilities"]["documentSymbolProvider"], true);
}

#[test]
fn diagnostics_carry_their_severity_as_a_number() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(&mut server, "file:///sev.ptl", "enum Color\n  red,\nend\nprint(Color.rde)\n");
    let v: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    let diag = &v["params"]["diagnostics"][0];
    assert_eq!(diag["severity"], json!(1), "{v}");
    assert!(diag["message"].as_str().unwrap().contains("did you mean Color.red?"), "{v}");
}

// ---------------------------------------------------------------------------
// Hover: the static-value class of a top-level declaration
// ---------------------------------------------------------------------------

/// The hover text for the name at `(line, character)` of `text`.
fn static_hover(text: &str, line: u32, character: u32) -> String {
    let mut server = Server::new();
    init(&mut server);
    open(&mut server, "file:///static.ptl", text);
    let resp = hover(&mut server, "file:///static.ptl", line, character);
    resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no hover at {line}:{character}: {resp}"))
        .to_string()
}

const STATIC_DOC: &str = "\
let TILES = [1, 2, 3]
const LIMIT = len(TILES)
state frame = 0
frame = frame + 1
let shown = TILES[frame % LIMIT]
fn double(x)
  x * 2
end
fn tick()
  frame + 1
end
var v = 0
state SCRIPT = [1, 2]
const BAD = frame + 1
print(shown, double(2), tick(), get v, SCRIPT, BAD)
";

#[test]
fn hover_says_a_static_declaration_is_static() {
    let text = static_hover(STATIC_DOC, 0, 5);
    // One list, with a function declaration (`double`, hoisted) between it
    // and what reads it: kept all the same.
    assert_eq!(text, "```petal\n(variable) TILES\n// static, kept between runs\n```");
    let text = static_hover(STATIC_DOC, 1, 7);
    assert_eq!(text, "```petal\n(variable) LIMIT\n// static, kept between runs\n```");
}

/// The classification behind the hover is worked out by the first hover
/// that asks, not on every change of the document, and kept until the next
/// change.
#[test]
fn hover_classifies_lazily_and_once_per_change() {
    let uri = "file:///lazy.ptl";
    let text = |limit: u32| format!("let ROWS = [[1, 2], [3, {limit}]]\nstate n = 0\nn = n + 1\nprint(ROWS[n % 2])\n");
    let said = |resp: Value| resp["result"]["contents"]["value"].as_str().unwrap().to_string();
    let mut server = Server::new();
    init(&mut server);
    let before = petal::static_classify::analyses_run();
    open(&mut server, uri, &text(4));
    change(&mut server, uri, 2, &text(5));
    assert_eq!(petal::static_classify::analyses_run(), before, "a change classifies nothing");
    assert!(said(hover(&mut server, uri, 0, 5)).ends_with("// static, kept between runs\n```"));
    let n = said(hover(&mut server, uri, 1, 6));
    assert!(n.contains("// dynamic"), "{n}");
    assert_eq!(petal::static_classify::analyses_run(), before + 1, "two hovers, one classification");
    // An edit that makes the table dynamic is seen by the next hover.
    change(&mut server, uri, 3, &text(6).replace("[3, 6]", "[3, n0]").replace("let ROWS", "state n0 = 0\nlet ROWS"));
    assert_eq!(petal::static_classify::analyses_run(), before + 1);
    let now = said(hover(&mut server, uri, 1, 5));
    assert!(now.contains("// dynamic"), "{now}");
    assert_eq!(petal::static_classify::analyses_run(), before + 2);
}

/// A static declaration the cache gives a slot says so; one that is
/// written to, and one that is built with the only value that reads it,
/// do not.
#[test]
fn hover_says_whether_a_static_declaration_is_kept() {
    let doc = "\
let ROWS = [[1, 2], [3, 4]]
const NAMES = map(ROWS, fn(r) -> r[0])
let BASE = [[5], [6]]
let ONLY_FOR_SUM = [[7], [8]]
let SUM = [ONLY_FOR_SUM[0][0], [9]]
state frame = 0
frame = frame + 1
let SMALL = [1, 2, 3]
let grown = append(BASE, frame)
print(ROWS[frame % 2], NAMES, SMALL, grown, SUM)
";
    assert_eq!(static_hover(doc, 0, 5), "```petal\n(variable) ROWS\n// static, kept between runs\n```");
    assert!(static_hover(doc, 1, 7).ends_with("// static, kept between runs\n```"));
    assert!(static_hover(doc, 9, 7).ends_with("// static, kept between runs\n```"), "at a use");
    // A constant `let` is hoisted, so `SMALL` is built, and kept, with the
    // tables above it.
    assert!(static_hover(doc, 7, 5).ends_with("// static, kept between runs\n```"));
    assert!(static_hover(doc, 2, 5).contains("// static, not kept:"), "written to");
    // Read by `SUM` alone, which is built past a function declaration (the
    // one in `NAMES`): the two are stored apart, so `SUM` can be rebuilt
    // without it.
    assert!(static_hover(doc, 3, 5).ends_with("// static, kept between runs\n```"));
    assert!(static_hover(doc, 4, 5).ends_with("// static, kept between runs\n```"));
    let doc = "\
let ONLY_FOR_SUM = [[7], [8]]
let SUM = [ONLY_FOR_SUM[0][0], [9]]
state frame = 0
frame = frame + 1
print(frame, SUM)
";
    assert!(static_hover(doc, 0, 5).ends_with("// static\n```"), "built with what reads it");
    assert!(static_hover(doc, 1, 5).ends_with("// static, kept between runs\n```"));
}

#[test]
fn hover_on_a_use_shows_the_class_of_its_declaration() {
    // `TILES` where it is read on line 5.
    assert!(static_hover(STATIC_DOC, 4, 13).ends_with("// static, kept between runs\n```"));
}

#[test]
fn hover_says_why_a_declaration_is_dynamic() {
    let text = static_hover(STATIC_DOC, 4, 5);
    assert_eq!(
        text,
        "```petal\n(variable) shown\n// dynamic: shown reads frame at line 5\n```"
    );
    let text = static_hover(STATIC_DOC, 2, 7);
    assert!(text.contains("// dynamic: frame is state, which changes between runs"), "{text}");
    let text = static_hover(STATIC_DOC, 11, 4);
    assert!(text.contains("// dynamic: v is a var"), "{text}");
}

#[test]
fn hover_says_whether_a_function_is_static_pure() {
    let text = static_hover(STATIC_DOC, 5, 4);
    assert_eq!(text, "```petal\n(function) double fn(x)\n// function, static-pure\n```");
    let text = static_hover(STATIC_DOC, 8, 4);
    assert!(text.contains("// function, not static-pure: tick reads frame"), "{text}");
}

#[test]
fn hover_points_out_a_state_whose_initializer_is_static() {
    let text = static_hover(STATIC_DOC, 12, 7);
    assert!(text.contains("its initializer is static"), "{text}");
    // `frame` is assigned, but its initializer `0` is static all the same:
    // the note is about the initializer, the lint about the assignment.
    assert!(static_hover(STATIC_DOC, 2, 7).contains("its initializer is static"));
}

#[test]
fn hover_flags_a_const_that_is_not_constant() {
    let text = static_hover(STATIC_DOC, 13, 7);
    assert!(text.contains("// dynamic: BAD reads frame at line 14"), "{text}");
    assert!(text.contains("(declared const: an error)"), "{text}");
}

#[test]
fn hover_on_a_const_of_host_natives_says_it_cannot_be_verified_here() {
    let text = static_hover("native fn tile(n)\nconst T = [tile(1)]\nprint(T)\n", 1, 6);
    assert!(text.contains("// dynamic: T calls tile, a native that is not registered here"), "{text}");
    assert!(text.contains("(declared const: cannot be verified without its host)"), "{text}");
}

#[test]
fn hover_on_a_local_says_nothing_about_static_values() {
    let doc = "let x = [1, 2]\nfn f()\n  let x = 3\n  x\nend\nprint(x, f())\n";
    // The local `x` on line 3 is not the top-level declaration of that name.
    assert_eq!(static_hover(doc, 2, 6), "```petal\n(variable) x\n```");
    assert!(static_hover(doc, 0, 4).contains("// static"));
}

#[test]
fn a_const_is_a_constant_in_the_outline_and_its_error_is_a_diagnostic() {
    let mut server = Server::new();
    init(&mut server);
    let msgs = open(
        &mut server,
        "file:///c.ptl",
        "state n = 0\nconst A = n + 1\nconst B = [1]\nprint(A, B)\n",
    );
    let diags = get_diagnostics(&msgs);
    assert_eq!(diags.len(), 1, "{diags:?}");
    let message = &diags[0].message;
    assert!(message.starts_with("`A` is declared `const` but is not constant"), "{message}");
    assert!(matches!(diags[0].severity, Some(DiagnosticSeverity::Error)));
    assert_eq!(diags[0].range.start.line, 1);

    let analysis = petal::lsp::document::analyze("const B = [1]\nlet c = 2\n");
    let outline = serde_json::to_value(&analysis.symbols).unwrap();
    let rows: Vec<(&str, &str, u64)> = outline
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap(),
                s["detail"].as_str().unwrap(),
                s["kind"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("B", "const", 14), ("c", "let", 13)]);
}

#[test]
fn a_test_block_is_outlined_by_its_title_and_its_function_is_not_offered() {
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///tested.ptl";
    let text = "fn after_hit(lives) lives - 1 end\n\ntest \"a hit costs one life\" do\n  let left = after_hit(3)\n  expect_eq(left, 2)\nend\n";
    // The body is checked like any function's: nothing to report here.
    assert!(get_diagnostics(&open(&mut server, uri, text)).is_empty());

    let msgs = server.handle_message(&request(
        4,
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    ));
    let resp: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    let symbols = resp["result"].as_array().expect("symbols");
    let summary: Vec<(&str, Option<&str>)> = symbols
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["detail"].as_str()))
        .collect();
    assert_eq!(
        summary,
        [("after_hit", Some("fn(lives)")), ("a hit costs one life", Some("test"))]
    );
    // The selection is the title, inside the quotes.
    assert_eq!(symbols[1]["selectionRange"]["start"], json!({"line": 2, "character": 6}));
    assert_eq!(symbols[1]["range"]["end"]["line"], 5);

    // Completion inside the body offers the file's functions and the test's
    // locals, and never the block's own function.
    let typing = "fn after_hit(lives) lives - 1 end\n\ntest \"a hit costs one life\" do\n  let left = after_hit(3)\n  \nend\n";
    change(&mut server, uri, 2, typing);
    let resp = completion(&mut server, uri, 4, 2);
    let labels: Vec<&str> = resp["result"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["label"].as_str())
        .collect();
    assert!(labels.contains(&"test"), "{labels:?}");
    assert!(labels.contains(&"after_hit"), "{labels:?}");
    assert!(labels.contains(&"left"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.contains("__test")), "{labels:?}");
    // A type error inside a test body is a diagnostic like any other.
    let bad = "fn double(x: int) x * 2 end\ntest \"typed\" do\n  expect_eq(double(\"a\"), 2)\nend\n";
    let diags = get_diagnostics(&change(&mut server, uri, 3, bad));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].range.start.line, 2);
}

// ---------------------------------------------------------------------------
// Diagnostics found in an imported module
// ---------------------------------------------------------------------------

/// A module whose ninth line calls a function nothing defines: the compile
/// succeeds, with an error in the module's diagnostics.
const LIB_WITH_ERROR: &str =
    "// lib\n\n\n\n\n\n\npub fn f()\n  nope()\nend\n";

#[test]
fn a_diagnostic_in_an_imported_module_goes_on_the_import_line_with_its_place() {
    let project = Project::new(
        "module-diag",
        &[
            ("lib.ptl", LIB_WITH_ERROR),
            ("via.ptl", "pub import ./lib\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    let diags = get_diagnostics(&open(&mut server, &uri, "// main\nimport ./lib\nprint(f())\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    assert!(d.message.starts_with("lib.ptl: "), "{d:?}");
    assert!(d.message.contains("`nope`"), "{d:?}");
    // On the path of the import that brings the module in, not at the
    // module's own line (8) in this two-line file.
    assert_eq!(d.range.start, Position { line: 1, character: 7 });
    assert_eq!(d.range.end, Position { line: 1, character: 12 });
    let related = d.related_information.as_ref().expect("related information");
    assert_eq!(related[0].location.uri, project.uri("lib.ptl"));
    assert_eq!(related[0].location.range.start.line, 8);
    assert_eq!(related[0].location.range.start.character, 2);

    // Through another module, there is no line of this file to point at:
    // the top, still with the module's name and place.
    let diags = get_diagnostics(&change(&mut server, &uri, 2, "import ./via\nprint(f())\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].range, Range::default());
    assert!(diags[0].message.starts_with("lib.ptl: "), "{diags:?}");
    let related = diags[0].related_information.as_ref().expect("related");
    assert_eq!(related[0].location.uri, project.uri("lib.ptl"));
}

#[test]
fn a_failed_compile_in_an_imported_module_goes_on_the_import_line() {
    let project = Project::new(
        "module-fail",
        &[("lib.ptl", "pub fn f()\n  1 +\nend\n")],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("main.ptl");
    let diags = get_diagnostics(&open(&mut server, &uri, "// main\nimport ./lib\nprint(f())\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.starts_with("lib.ptl: "), "{diags:?}");
    assert_eq!(diags[0].range.start, Position { line: 1, character: 7 });
    let related = diags[0].related_information.as_ref().expect("related");
    assert_eq!(related[0].location.uri, project.uri("lib.ptl"));
    assert_eq!(related[0].location.range.start.line, 2);

    // An import that does not resolve is reported on its own line.
    let diags = get_diagnostics(&change(&mut server, &uri, 2, "// main\nimport ./nothere\nprint(1)\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.contains("cannot find module './nothere'"), "{diags:?}");
    assert_eq!(diags[0].range.start.line, 1);
}

// ---------------------------------------------------------------------------
// Position encodings
// ---------------------------------------------------------------------------

fn init_offering(server: &mut Server, encodings: Value) -> Value {
    let msgs = server.handle_message(&request(
        0,
        "initialize",
        json!({"capabilities": {"general": {"positionEncodings": encodings}}}),
    ));
    let v: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    v["result"]["capabilities"]["positionEncoding"].clone()
}

/// `😀` is one code point, two UTF-16 code units and four UTF-8 bytes, so
/// `name` after it starts at code point 11, UTF-16 unit 12, byte 14.
const EMOJI_LINE: &str = "let name = 1\nprint(\"😀\", name, nope)\n";

#[test]
fn columns_are_utf16_by_default_across_an_astral_character() {
    let mut server = Server::new();
    let msgs = server.handle_message(&request(0, "initialize", json!({"capabilities": {}})));
    let v: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    assert_eq!(v["result"]["capabilities"]["positionEncoding"], "utf-16");
    let uri = "file:///emoji.ptl";
    let diags = get_diagnostics(&open(&mut server, uri, EMOJI_LINE));
    assert_eq!(diags.len(), 1, "{diags:?}");
    // `nope` is code point 17, UTF-16 unit 18.
    assert_eq!(diags[0].range.start, Position { line: 1, character: 18 });
    // Where the client sees `name` is where the server finds it.
    let resp = hover(&mut server, uri, 1, 12);
    assert!(resp["result"]["contents"]["value"].as_str().unwrap().contains("name"), "{resp}");
    let resp = definition(&mut server, uri, 1, 12);
    assert_eq!(resp["result"]["range"]["start"]["line"], 0, "{resp}");
    // The emoji's second unit is still the emoji, not `"`.
    let resp = hover(&mut server, uri, 1, 8);
    assert!(resp["result"].is_null(), "{resp}");
}

#[test]
fn a_client_may_count_in_utf8_or_utf32() {
    let uri = "file:///emoji.ptl";
    // Without UTF-16 on offer, the server takes the first it knows.
    let mut server = Server::new();
    assert_eq!(init_offering(&mut server, json!(["utf-32", "utf-8"])), "utf-32");
    let diags = get_diagnostics(&open(&mut server, uri, EMOJI_LINE));
    assert_eq!(diags[0].range.start, Position { line: 1, character: 17 });
    let resp = hover(&mut server, uri, 1, 11);
    assert!(resp["result"]["contents"]["value"].as_str().unwrap().contains("name"), "{resp}");

    let mut server = Server::new();
    assert_eq!(init_offering(&mut server, json!(["utf-8"])), "utf-8");
    let diags = get_diagnostics(&open(&mut server, uri, EMOJI_LINE));
    assert_eq!(diags[0].range.start, Position { line: 1, character: 20 });
    let resp = hover(&mut server, uri, 1, 14);
    assert!(resp["result"]["contents"]["value"].as_str().unwrap().contains("name"), "{resp}");

    // Offered alongside others, UTF-16 wins.
    let mut server = Server::new();
    assert_eq!(init_offering(&mut server, json!(["utf-8", "utf-16"])), "utf-16");
}

#[test]
fn outline_ranges_are_in_the_clients_encoding() {
    let mut server = Server::new();
    init(&mut server);
    let uri = "file:///outline-emoji.ptl";
    open(&mut server, uri, "let face = \"😀\"\n");
    let msgs = server.handle_message(&request(
        4,
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    ));
    let v: Value = serde_json::from_str(&msgs[0].to_json()).unwrap();
    // The statement ends after the closing quote: code point 14, unit 15.
    assert_eq!(v["result"][0]["range"]["end"], json!({"line": 0, "character": 15}), "{v}");
}

// ---------------------------------------------------------------------------
// A project's [host]: what the host puts in scope
// ---------------------------------------------------------------------------

/// A project whose host binds the package `kit` as an implicit prelude and
/// joins two library files ahead of each script. (`kit` stands in for a
/// host interface such as `ui`, which is slow to compile in a debug build.
/// Like `ui`, it imports `color`, which a script may import too.)
const HOST_MANIFEST: &str = "[package]\nname = \"panels\"\n\n[dependencies]\nkit = { path = \"kit\" }\n\n[host]\nimports = [\"kit\"]\nprelude = [\n  \"lib/base.ptl\",\n  \"lib/more.ptl\",\n]\n";

const KIT: &[(&str, &str)] = &[
    ("kit/petal.toml", "[package]\nname = \"kit\"\n"),
    (
        "kit/src/kit.ptl",
        "import color\n\npub fn rect(x, y, w, h)\n  [x, y, w, h]\nend\n\npub fn draw_rect(r, c)\n  nil\nend\n",
    ),
];

#[test]
fn a_script_sees_what_its_projects_host_puts_in_scope() {
    let project = Project::new(
        "host",
        &[
            KIT[0],
            KIT[1],
            ("petal.toml", HOST_MANIFEST),
            ("lib/base.ptl", "import color\n\n// not pub: the host joins files\nfn helper(x)\n  x + 1\nend\n"),
            ("lib/more.ptl", "fn twice(x)\n  helper(helper(x))\nend\n"),
            ("screens/menu.ptl", "import noise\nlet LIMIT = twice(1)\ndraw_rect(rect(0, 0, 10, 10), #ffffff)\nprint(helper(LIMIT))\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let menu = project.uri("screens/menu.ptl");
    let text = std::fs::read_to_string(project.0.join("screens/menu.ptl")).unwrap();
    let diags = get_diagnostics(&open(&mut server, &menu, &text));
    assert!(diags.is_empty(), "{diags:?}");

    // A diagnostic of the script is on the script's own line, past the
    // prelude it was joined with.
    let broken = format!("{text}nope()\n");
    let diags = get_diagnostics(&change(&mut server, &menu, 2, &broken));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].range.start, Position { line: 4, character: 0 });
    assert!(diags[0].message.contains("`nope`"), "{diags:?}");
    change(&mut server, &menu, 3, &text);

    // A prelude name hovers with its file and goes to its declaration.
    let resp = hover(&mut server, &menu, 3, 7);
    let value = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("helper fn(x)"), "{value}");
    assert!(value.contains("lib/base.ptl"), "{value}");
    let resp = definition(&mut server, &menu, 3, 7);
    assert_eq!(resp["result"]["uri"], json!(project.uri("lib/base.ptl")), "{resp}");
    assert_eq!(resp["result"]["range"]["start"]["line"], 3);
    let resp = completion(&mut server, &menu, 3, 9);
    assert!(labels(&resp).contains(&"helper".to_string()), "{resp}");
    // The static note is the script's declaration's, on the script's line.
    let resp = hover(&mut server, &menu, 1, 4);
    let value = resp["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("(variable) LIMIT\n//"), "{value}");

    // A prelude file sees the files before it, and not those after.
    let more = project.uri("lib/more.ptl");
    let diags = get_diagnostics(&open(&mut server, &more, "fn twice(x)\n  helper(helper(x))\nend\n"));
    assert!(diags.is_empty(), "{diags:?}");
    let base = project.uri("lib/base.ptl");
    let diags = get_diagnostics(&open(&mut server, &base, "fn helper(x)\n  twice(x)\nend\n"));
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.contains("`twice`"), "{diags:?}");
}

#[test]
fn without_a_host_section_the_hosts_names_are_unknown() {
    let project = Project::new(
        "no-host",
        &[
            ("petal.toml", "[package]\nname = \"panels\"\n"),
            ("lib/base.ptl", "fn helper(x)\n  x + 1\nend\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("menu.ptl");
    let diags = get_diagnostics(&open(
        &mut server,
        &uri,
        "draw_rect(rect(0, 0, 10, 10), #ffffff)\nprint(helper(1))\n",
    ));
    assert_eq!(diags.len(), 3, "{diags:?}");
}

#[test]
fn a_prelude_file_that_cannot_be_read_or_compiled_is_reported() {
    let project = Project::new(
        "host-broken",
        &[
            (
                "petal.toml",
                "[package]\nname = \"p\"\n[host]\nprelude = [\"lib/gone.ptl\", \"lib/bad.ptl\"]\n",
            ),
            ("lib/bad.ptl", "fn f()\n  1 +\nend\n"),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("menu.ptl");
    let diags = get_diagnostics(&open(&mut server, &uri, "print(1)\n"));
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert!(diags[0].message.contains("cannot read lib/gone.ptl"), "{diags:?}");
    assert_eq!(diags[0].severity, Some(DiagnosticSeverity::Warning));
    assert!(diags[1].message.starts_with("lib/bad.ptl (host prelude): "), "{diags:?}");
    let related = diags[1].related_information.as_ref().expect("related");
    assert_eq!(related[0].location.uri, project.uri("lib/bad.ptl"));
    assert_eq!(related[0].location.range.start.line, 2);
}

#[test]
fn a_broken_script_still_completes_the_hosts_implicit_imports() {
    let project = Project::new(
        "host-broken-doc",
        &[
            KIT[0],
            KIT[1],
            (
                "petal.toml",
                "[package]\nname = \"p\"\n[dependencies]\nkit = { path = \"kit\" }\n[host]\nimports = [\"kit\"]\n",
            ),
        ],
    );
    let mut server = Server::new();
    init(&mut server);
    let uri = project.uri("menu.ptl");
    open(&mut server, &uri, "let r = rect(0, 0, 1, 1)\nlet x = (\ndraw_r");
    let resp = completion(&mut server, &uri, 2, 6);
    assert!(labels(&resp).contains(&"draw_rect".to_string()), "{resp}");
}
