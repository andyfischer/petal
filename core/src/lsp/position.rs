//! Position encodings: the unit an LSP `Position.character` counts in.
//!
//! Inside the server every column is a count of Unicode code points (`char`s),
//! which is what the lexer's spans count. The client may count in another
//! unit, agreed in `initialize` ([`PositionEncoding::negotiate`]): UTF-16 code
//! units (the protocol's default, and what a client that says nothing means),
//! UTF-8 bytes, or UTF-32 code units, which are code points. The server
//! converts at its edge — each incoming position once, when a request is
//! read ([`PositionEncoding::to_internal`]), and each outgoing range once,
//! when a reply is written ([`PositionEncoding::range_out`]) — so nothing in
//! between has to know.
//!
//! The units differ only on a line with a character outside the Basic
//! Multilingual Plane (most emoji) for UTF-16, or with any non-ASCII
//! character for UTF-8; an ASCII line is the same in all three.

use crate::lsp::lsp_types::{Position, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    /// UTF-16 code units: the LSP default.
    #[default]
    Utf16,
    /// UTF-8 bytes.
    Utf8,
    /// UTF-32 code units: code points, the server's own unit.
    Utf32,
}

impl PositionEncoding {
    /// The name the protocol gives the encoding (`positionEncoding`).
    pub fn name(self) -> &'static str {
        match self {
            PositionEncoding::Utf16 => "utf-16",
            PositionEncoding::Utf8 => "utf-8",
            PositionEncoding::Utf32 => "utf-32",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "utf-16" => Some(PositionEncoding::Utf16),
            "utf-8" => Some(PositionEncoding::Utf8),
            "utf-32" => Some(PositionEncoding::Utf32),
            _ => None,
        }
    }

    /// The encoding to use with a client whose `initialize` capabilities are
    /// `capabilities`: UTF-16 whenever the client offers it or offers nothing
    /// (the protocol says a server must support it, and every client does),
    /// else the first of the client's `general.positionEncodings` the server
    /// knows. A client that lists only encodings the server does not know
    /// gets UTF-16, which is the protocol's fallback.
    pub fn negotiate(capabilities: &serde_json::Value) -> Self {
        let offered: Vec<&str> = capabilities
            .pointer("/general/positionEncodings")
            .and_then(|v| v.as_array())
            .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        if offered.is_empty() || offered.contains(&"utf-16") {
            return PositionEncoding::Utf16;
        }
        offered
            .iter()
            .find_map(|name| Self::from_name(name))
            .unwrap_or(PositionEncoding::Utf16)
    }

    /// The width of `c` in this encoding's units.
    fn width(self, c: char) -> u32 {
        match self {
            PositionEncoding::Utf16 => c.len_utf16() as u32,
            PositionEncoding::Utf8 => c.len_utf8() as u32,
            PositionEncoding::Utf32 => 1,
        }
    }

    /// The code-point column `units` of this encoding reaches on `line`. A
    /// position inside a character (half of a surrogate pair, a byte in the
    /// middle of a UTF-8 sequence) is that character's; one past the end of
    /// the line is the end of the line plus the excess, as the client sent it.
    pub fn column_in(self, line: &str, units: u32) -> u32 {
        if self == PositionEncoding::Utf32 || line.is_ascii() {
            return units;
        }
        let mut used = 0u32;
        let mut column = 0u32;
        for c in line.chars() {
            if used >= units {
                return column;
            }
            let w = self.width(c);
            if used + w > units {
                // Inside `c`.
                return column;
            }
            used += w;
            column += 1;
        }
        column + (units - used)
    }

    /// The column, in this encoding's units, of code-point column `column` on
    /// `line`.
    pub fn column_out(self, line: &str, column: u32) -> u32 {
        if self == PositionEncoding::Utf32 || line.is_ascii() {
            return column;
        }
        let mut units = 0u32;
        let mut seen = 0u32;
        for c in line.chars() {
            if seen >= column {
                return units;
            }
            units += self.width(c);
            seen += 1;
        }
        units + (column - seen)
    }

    /// A position the client sent, as the code-point position the server
    /// works in, on `text`.
    pub fn to_internal(self, text: &str, position: Position) -> Position {
        if self == PositionEncoding::Utf32 {
            return position;
        }
        let line = line_of(text, position.line);
        Position {
            line: position.line,
            character: self.column_in(line, position.character),
        }
    }

    /// A code-point position on `text`, as the client counts it.
    pub fn position_out(self, text: &str, position: Position) -> Position {
        if self == PositionEncoding::Utf32 {
            return position;
        }
        let line = line_of(text, position.line);
        Position {
            line: position.line,
            character: self.column_out(line, position.character),
        }
    }

    /// A code-point range on `text`, as the client counts it.
    pub fn range_out(self, text: &str, range: Range) -> Range {
        Range {
            start: self.position_out(text, range.start),
            end: self.position_out(text, range.end),
        }
    }
}

/// Line `n` (0-based) of `text`, without its line ending; empty past the end.
fn line_of(text: &str, n: u32) -> &str {
    text.split('\n')
        .nth(n as usize)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::PositionEncoding::{self, *};
    use serde_json::json;

    #[test]
    fn negotiation_prefers_utf16_and_otherwise_takes_what_the_client_offers() {
        let offer = |list: serde_json::Value| {
            PositionEncoding::negotiate(&json!({ "general": { "positionEncodings": list } }))
        };
        assert_eq!(PositionEncoding::negotiate(&json!({})), Utf16);
        assert_eq!(PositionEncoding::negotiate(&serde_json::Value::Null), Utf16);
        assert_eq!(offer(json!(["utf-8", "utf-16"])), Utf16);
        assert_eq!(offer(json!(["utf-32", "utf-8"])), Utf32);
        assert_eq!(offer(json!(["utf-8"])), Utf8);
        assert_eq!(offer(json!(["latin-1"])), Utf16);
    }

    #[test]
    fn an_astral_character_is_two_utf16_units_and_four_bytes() {
        // `😀` is one code point, two UTF-16 units, four UTF-8 bytes.
        let line = "let s = \"😀\" + name";
        let name_cp = line.chars().position(|c| c == 'n').unwrap() as u32;
        assert_eq!(name_cp, 14);
        assert_eq!(Utf16.column_out(line, name_cp), 15);
        assert_eq!(Utf8.column_out(line, name_cp), 17);
        assert_eq!(Utf32.column_out(line, name_cp), 14);
        for enc in [Utf16, Utf8, Utf32] {
            assert_eq!(enc.column_in(line, enc.column_out(line, name_cp)), name_cp);
        }
        // Before the emoji nothing changes; inside it is the emoji's column.
        assert_eq!(Utf16.column_out(line, 9), 9);
        assert_eq!(Utf16.column_in(line, 10), 9);
        assert_eq!(Utf8.column_in(line, 11), 9);
        // A BMP non-ASCII character is one UTF-16 unit and two bytes.
        assert_eq!(Utf16.column_out("é x", 2), 2);
        assert_eq!(Utf8.column_out("é x", 2), 3);
        // Past the end of the line, the excess carries over.
        assert_eq!(Utf16.column_out("😀", 3), 4);
        assert_eq!(Utf16.column_in("😀", 4), 3);
    }
}
