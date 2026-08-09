//! LSP wire framing: `Content-Length: N\r\n\r\n<body>`.
//!
//! Deliberately *not* shared with GPP, which frames messages as
//! newline-delimited JSON ([`gpp::read_message`]). The two protocols look
//! similar at the `serde_json` layer and are incompatible at the byte layer;
//! reusing GPP's reader here would appear to work until a message contained a
//! newline.
//!
//! [`read_message`] reads one message at a time — headers by line, then exactly
//! `Content-Length` bytes — rather than accumulating into a string buffer and
//! slicing. `Content-Length` counts *bytes*, so a buffer-and-slice reader
//! desyncs the moment a body carries multi-byte UTF-8 and the length is read as
//! a character count. This mirrors `petal`'s own `rust/src/lsp/stdio.rs`.

use std::io::{self, BufRead};

/// Frame a JSON body for transmission.
pub fn encode(json: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len() + 32);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", json.len()).as_bytes());
    out.extend_from_slice(json.as_bytes());
    out
}

/// Read one framed message, returning its JSON body. `Ok(None)` at a clean EOF
/// (the server closed stdout, i.e. it exited).
///
/// Unrecognized headers are ignored — the spec allows `Content-Type` — but a
/// header block with no `Content-Length` is unframeable and reported as an
/// error rather than guessed at.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().ok();
            }
        }
    }

    let Some(len) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message header had no Content-Length",
        ));
    };

    let mut body = vec![0u8; len];
    io::Read::read_exact(reader, &mut body)?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_body() {
        let framed = encode(r#"{"jsonrpc":"2.0"}"#);
        assert_eq!(
            String::from_utf8_lossy(&framed),
            "Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}"
        );
        let mut cursor = io::Cursor::new(framed);
        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"jsonrpc":"2.0"}"#)
        );
    }

    #[test]
    fn reads_consecutive_messages_without_desync() {
        let mut stream = encode(r#"{"a":1}"#);
        stream.extend(encode(r#"{"b":2}"#));
        stream.extend(encode(r#"{"c":3}"#));
        let mut cursor = io::Cursor::new(stream);

        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"a":1}"#)
        );
        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"b":2}"#)
        );
        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"c":3}"#)
        );
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }

    #[test]
    fn a_multibyte_body_does_not_desync_the_next_message() {
        // The regression this reader exists to prevent: Content-Length is a
        // byte count, and 'é'/'—' make bytes and chars diverge.
        let mut stream = encode(r#"{"text":"héllo — ✓"}"#);
        stream.extend(encode(r#"{"next":true}"#));
        let mut cursor = io::Cursor::new(stream);

        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"text":"héllo — ✓"}"#)
        );
        assert_eq!(
            read_message(&mut cursor).unwrap().as_deref(),
            Some(r#"{"next":true}"#),
            "the message after a multi-byte body was mis-framed"
        );
    }

    #[test]
    fn extra_headers_are_ignored_and_casing_does_not_matter() {
        let body = r#"{"ok":1}"#;
        let raw = format!(
            "Content-Type: application/vscode-jsonrpc\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut cursor = io::Cursor::new(raw.into_bytes());
        assert_eq!(read_message(&mut cursor).unwrap().as_deref(), Some(body));
    }

    #[test]
    fn a_truncated_body_is_an_error_not_a_short_read() {
        let raw = "Content-Length: 99\r\n\r\n{\"trunc";
        let mut cursor = io::Cursor::new(raw.as_bytes().to_vec());
        assert!(read_message(&mut cursor).is_err());
    }

    #[test]
    fn a_header_block_without_content_length_is_an_error() {
        let raw = "Content-Type: application/vscode-jsonrpc\r\n\r\n{}";
        let mut cursor = io::Cursor::new(raw.as_bytes().to_vec());
        assert!(read_message(&mut cursor).is_err());
    }

    #[test]
    fn eof_before_any_header_is_a_clean_stop() {
        let mut cursor = io::Cursor::new(Vec::new());
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }
}
