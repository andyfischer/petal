//! Proof that stripping comments from the embedded Petal sources is safe.
//!
//! A wasm build embeds the bundled packages and the std prelude with their
//! whole-line comments blanked (`core/build_support/strip_comments.rs`,
//! applied by `core/build.rs`). That rule works on lines, not tokens, so this
//! test lexes every embedded file before and after stripping and requires the
//! same tokens at the same line and column — a `//` line inside a multi-line
//! string, say, would change a token and fail here rather than in a browser.

#[path = "../build_support/strip_comments.rs"]
mod strip_comments;

use crate::lexer::Lexer;

/// Every embedded `.ptl`: `(label, source)`.
fn embedded_sources() -> Vec<(String, &'static str)> {
    let mut out = vec![
        (
            "prelude/std.ptl".to_string(),
            include_str!("../prelude/std.ptl"),
        ),
        (
            "packages/web/src/web.ptl".to_string(),
            include_str!("../packages/web/src/web.ptl"),
        ),
    ];
    for package in crate::bundled::BUNDLED_PACKAGES {
        for (path, source) in package.sources() {
            out.push((format!("{}/{path}", package.name), source));
        }
    }
    out
}

/// Tokens with their start and end line/column. Byte offsets move when
/// comments go, so they are left out, and so is a newline token's start
/// column: it starts where its line's text ends, which is where a blanked
/// comment used to end.
fn lex(label: &str, source: &str) -> Vec<(crate::lexer::Token, [u32; 4])> {
    let mut lexer = Lexer::new(source);
    lexer
        .tokenize()
        .unwrap_or_else(|e| panic!("{label} does not lex: {e}"));
    lexer
        .tokens
        .into_iter()
        .zip(lexer.token_spans)
        .map(|(t, s)| {
            let start_column = if t == crate::lexer::Token::Newline {
                0
            } else {
                s.start.column
            };
            (t, [s.start.line, start_column, s.end.line, s.end.column])
        })
        .collect()
}

#[test]
fn stripped_embedded_sources_lex_to_the_same_tokens_at_the_same_positions() {
    let sources = embedded_sources();
    assert!(
        sources.len() > 10,
        "found only {} embedded sources",
        sources.len()
    );
    for (label, source) in sources {
        let stripped = strip_comments::strip_comments(&source);
        assert_eq!(
            source.split('\n').count(),
            stripped.split('\n').count(),
            "{label}: stripping changed the line count"
        );
        assert_eq!(
            lex(&label, source),
            lex(&label, &stripped),
            "{label}: stripping comments changed its tokens or their positions"
        );
    }
}

#[test]
fn strip_blanks_comment_lines_and_keeps_everything_else() {
    let src = "// header\nlet x = 1 // trailing\n    // indented\n\nfn f() end   \n";
    assert_eq!(
        strip_comments::strip_comments(src),
        "\nlet x = 1 // trailing\n\n\nfn f() end\n"
    );
}
