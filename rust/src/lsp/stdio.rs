//! The standard LSP stdio transport: Content-Length-framed JSON-RPC over
//! stdin/stdout. This is the loop `petal lsp` runs.
//!
//! [`Server`](crate::lsp::Server) itself is transport-agnostic; everything
//! here is framing and I/O. Messages are read one at a time — headers by line,
//! then exactly `Content-Length` bytes of body — rather than accumulating into
//! a string buffer, so a body split across reads (or across a multi-byte UTF-8
//! character) can never be mis-sliced.

use std::io::{self, BufRead, Write};

use crate::lsp::Server;
use crate::lsp::protocol::encode_lsp_message;

/// Run the server on stdin/stdout until EOF or an `exit` notification.
pub fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    serve_on(&mut reader, &mut writer, &mut Server::new())
}

/// [`serve`] against arbitrary streams, so the loop is testable without
/// spawning a process.
pub fn serve_on<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    server: &mut Server,
) -> io::Result<()> {
    while let Some(body) = read_message(reader)? {
        // The server treats `exit` as a no-op notification; per the LSP spec
        // it is the transport's job to stop on it.
        let is_exit = method_of(&body).as_deref() == Some("exit");

        for out in server.handle_message(&body) {
            writer.write_all(encode_lsp_message(&out.to_json()).as_bytes())?;
        }
        // Flush per message, not per batch: a client blocked on a response
        // will never send the next request that would flush it for us.
        writer.flush()?;

        if is_exit {
            break;
        }
    }
    Ok(())
}

/// Read one framed message, returning its JSON body. `Ok(None)` at a clean EOF.
///
/// Unrecognized headers are ignored (the spec allows `Content-Type`); a header
/// block with no `Content-Length` is a protocol error.
fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // EOF. Mid-header is a truncated message, but there is nothing
            // useful to say to a client that has already gone away.
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok();
        }
    }

    let len = match content_length {
        Some(len) => len,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP message header had no Content-Length",
            ));
        }
    };

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The `method` field of a raw message, if it has one.
fn method_of(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("method")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(v: serde_json::Value) -> String {
        encode_lsp_message(&v.to_string())
    }

    /// Split the decoded stdout back into JSON bodies.
    fn bodies(out: &[u8]) -> Vec<serde_json::Value> {
        let text = String::from_utf8(out.to_vec()).unwrap();
        crate::lsp::protocol::decode_lsp_messages(&text)
            .into_iter()
            .map(|(body, _)| serde_json::from_str(&body).unwrap())
            .collect()
    }

    #[test]
    fn initialize_and_complete_over_the_framed_transport() {
        let input = [
            frame(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}),
            ),
            frame(json!({"jsonrpc":"2.0","method":"initialized","params":{}})),
            frame(
                json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///t.ptl","languageId":"petal","version":1,
                               "text":"let greeting = 1\ngree\n"}}}),
            ),
            frame(
                json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{
                "textDocument":{"uri":"file:///t.ptl"},
                "position":{"line":1,"character":4}}}),
            ),
        ]
        .concat();

        let mut out = Vec::new();
        serve_on(
            &mut io::Cursor::new(input.into_bytes()),
            &mut out,
            &mut Server::new(),
        )
        .unwrap();

        let msgs = bodies(&out);
        let init = msgs
            .iter()
            .find(|m| m["id"] == 1)
            .expect("initialize reply");
        assert!(init["result"]["capabilities"]["completionProvider"].is_object());

        let completion = msgs
            .iter()
            .find(|m| m["id"] == 2)
            .expect("completion reply");
        let items = completion["result"]["items"].as_array().unwrap();
        assert!(
            items.iter().any(|i| i["label"] == "greeting"),
            "expected the prefix match in {items:?}"
        );
    }

    #[test]
    fn exit_stops_the_loop_with_input_still_pending() {
        let input = [
            frame(json!({"jsonrpc":"2.0","method":"exit"})),
            frame(json!({"jsonrpc":"2.0","id":9,"method":"initialize","params":{}})),
        ]
        .concat();

        let mut out = Vec::new();
        serve_on(
            &mut io::Cursor::new(input.into_bytes()),
            &mut out,
            &mut Server::new(),
        )
        .unwrap();

        // The post-exit initialize must never have been answered.
        assert!(bodies(&out).is_empty(), "expected no replies, got {out:?}");
    }

    #[test]
    fn a_body_carrying_multibyte_utf8_survives_framing() {
        // Content-Length counts bytes, not characters — the classic off-by-N.
        let text = "x = 'héllo — ✓'";
        let input = [
            frame(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})),
            frame(json!({"jsonrpc":"2.0","method":"initialized","params":{}})),
            frame(
                json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///m.ptl","languageId":"petal","version":1,
                               "text":text}}}),
            ),
            frame(
                json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{
                "textDocument":{"uri":"file:///m.ptl"},
                "position":{"line":0,"character":1}}}),
            ),
        ]
        .concat();

        let mut out = Vec::new();
        serve_on(
            &mut io::Cursor::new(input.into_bytes()),
            &mut out,
            &mut Server::new(),
        )
        .unwrap();

        assert!(
            bodies(&out).iter().any(|m| m["id"] == 2),
            "the message after a multi-byte body was never read"
        );
    }

    #[test]
    fn eof_mid_stream_ends_cleanly() {
        let mut input = frame(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        input.push_str("Content-Length: 99\r\n\r\n{\"trunc");

        let mut out = Vec::new();
        let result = serve_on(
            &mut io::Cursor::new(input.into_bytes()),
            &mut out,
            &mut Server::new(),
        );
        assert!(
            result.is_err(),
            "a truncated body should surface as an error"
        );
    }
}
