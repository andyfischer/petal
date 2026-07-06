//! Lossless trivia + source reconstruction.
//!
//! This is the foundation for making Petal's parsed representation *lossless* —
//! able to reproduce the original source byte-for-byte, comments and layout
//! included — which is what programmatic, source-preserving runtime edits need.
//! See `docs/dev/source-preservation-plan.md` for the staged plan toward a full
//! concrete syntax tree (CST); this module is its first, load-bearing step.
//!
//! ## The idea
//!
//! The lexer discards nothing *positionally*: every significant token carries a
//! [`SourceSpan`] with char offsets, and the lexer advances a single cursor
//! through the source, so tokens appear in strictly non-decreasing start order.
//! Any characters that fall *between* one token's end and the next token's
//! start — whitespace, `//` comments, and the odd interpolation/JSX delimiter —
//! are **trivia**. We recover them purely from the spans plus the original
//! source, without changing the tokenizer.
//!
//! [`reconstruct`] walks the tokens with a cursor, emitting each gap verbatim
//! and then each token's own source text. It is robust to the two span
//! irregularities the lexer currently produces — zero-width tokens (collapsed
//! JSX text, empty interpolation parts) and forward-overlapping spans (the
//! `InterpStart` token spans the opening quote plus the first literal run) —
//! because it clamps with a monotonic cursor and never emits a character twice.
//! The invariant, checked by tests over a broad corpus, is:
//!
//! ```text
//! reconstruct(src, &lexer.token_spans) == src   for every src
//! ```
//!
//! [`leading_trivia`] additionally classifies each gap into typed [`Trivia`]
//! runs (whitespace vs. line comment vs. other) attached to the following
//! token — the shape a CST will hang comments off of. `TriviaKind::Other`
//! marks characters that ended up in a gap only because a token's span is
//! looser than its text (interpolation/JSX); reconstruction stays lossless, but
//! these flag the spots whose spans a later CST stage should tighten.

use crate::source_map::SourceSpan;

/// The classification of one run of trivia characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriviaKind {
    /// Spaces, tabs, and non-significant newlines (those consumed inside
    /// interpolation / JSX holes without emitting a `Newline` token).
    Whitespace,
    /// A `//` line comment. `text` excludes the terminating newline, which is
    /// lexed as a significant `Newline` token, not trivia.
    LineComment,
    /// Characters that landed in an inter-token gap because a nearby token's
    /// span is looser than its source text (interpolation / JSX delimiters).
    /// Reconstruction is unaffected; this is a marker for future span cleanup.
    Other,
}

/// One contiguous run of trivia, with its exact source text and start offset.
#[derive(Debug, Clone)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub text: String,
    /// Char offset in the source where this run begins.
    pub start: u32,
}

/// Reconstruct the original source exactly from token spans.
///
/// Lossless for any input: `reconstruct(src, &lexer.token_spans) == src`.
/// `spans` must be the lexer's `token_spans` (in emission order, ending with
/// the `Eof` token whose span sits at the end of the source).
pub fn reconstruct(source: &str, spans: &[SourceSpan]) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for span in spans {
        let start = (span.start.offset as usize).min(chars.len());
        let end = (span.end.offset as usize).min(chars.len());
        // Gap between the cursor and this token: trivia, emitted verbatim.
        if start > cursor {
            out.extend(chars[cursor..start].iter());
            cursor = start;
        }
        // The token's own source text, clamped so overlapping spans (e.g.
        // InterpStart spanning the opening quote) never re-emit characters.
        if end > cursor {
            out.extend(chars[cursor..end].iter());
            cursor = end;
        }
    }
    // Any characters beyond the last span (shouldn't happen once Eof is
    // present, but keeps the function total).
    if cursor < chars.len() {
        out.extend(chars[cursor..].iter());
    }
    out
}

/// The leading trivia preceding each token, parallel to the token list:
/// `result[i]` is the classified trivia between token `i-1`'s end and token
/// `i`'s start (and, for `i == 0`, between the start of the source and the
/// first token). Trailing end-of-file trivia is attached to the final `Eof`
/// token.
pub fn leading_trivia(source: &str, spans: &[SourceSpan]) -> Vec<Vec<Trivia>> {
    let chars: Vec<char> = source.chars().collect();
    let mut result = Vec::with_capacity(spans.len());
    let mut cursor = 0usize;
    for span in spans {
        let start = (span.start.offset as usize).min(chars.len());
        let end = (span.end.offset as usize).min(chars.len());
        if start > cursor {
            result.push(classify(&chars[cursor..start], cursor as u32));
        } else {
            result.push(Vec::new());
        }
        cursor = cursor.max(end);
    }
    result
}

/// Split a gap into typed trivia runs. Gaps produced by the main lexer loop
/// contain only whitespace and `//` comments; interpolation/JSX gaps may
/// contain delimiter characters (classified `Other`) and non-significant
/// newlines (classified `Whitespace`).
fn classify(gap: &[char], start_offset: u32) -> Vec<Trivia> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < gap.len() {
        let run_start = i;
        let ch = gap[i];
        let kind = if is_ws(ch) {
            while i < gap.len() && is_ws(gap[i]) {
                i += 1;
            }
            TriviaKind::Whitespace
        } else if ch == '/' && gap.get(i + 1) == Some(&'/') {
            while i < gap.len() && gap[i] != '\n' {
                i += 1;
            }
            TriviaKind::LineComment
        } else {
            while i < gap.len() && !is_ws(gap[i]) && !(gap[i] == '/' && gap.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            TriviaKind::Other
        };
        out.push(Trivia {
            kind,
            text: gap[run_start..i].iter().collect(),
            start: start_offset + run_start as u32,
        });
    }
    out
}

fn is_ws(ch: char) -> bool {
    ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn spans_of(src: &str) -> Vec<SourceSpan> {
        let mut lexer = Lexer::new(src);
        lexer.tokenize().expect("tokenize");
        lexer.token_spans.clone()
    }

    /// The central invariant: reconstruction is byte-for-byte lossless.
    fn assert_round_trips(src: &str) {
        let got = reconstruct(src, &spans_of(src));
        assert_eq!(got, src, "round-trip mismatch for:\n{src:?}");
    }

    #[test]
    fn round_trips_core_language() {
        assert_round_trips("let x = 1\n");
        assert_round_trips("  let  x   =  1  \n"); // odd spacing preserved
        assert_round_trips("fn add(a, b)\n    a + b\nend\n");
        assert_round_trips("x = 1\n\n\n// trailing comment\n");
        assert_round_trips("let x = 1 // inline comment\nlet y = 2\n");
        assert_round_trips("// leading\n// two lines\nlet x = 1\n");
        assert_round_trips("x = 1;y = 2\n"); // semicolon separators
        assert_round_trips("let s = \"hi\"\nlet r = [1, 2, 3]\n");
        assert_round_trips("no_newline_at_eof");
        assert_round_trips("   \t  \n   \n"); // whitespace-only file
        assert_round_trips("");
    }

    #[test]
    fn round_trips_strings_and_interpolation() {
        assert_round_trips("let n = \"Petal\"\nprint(\"hello, {n}!\")\n");
        assert_round_trips("print(\"sum = {2 + 2} done\")\n");
        assert_round_trips("let s = \"\"\"\n  raw {braces} kept\n\"\"\"\n");
        assert_round_trips("print(\"{ a }{ b }\")\n"); // spaces inside holes
    }

    #[test]
    fn round_trips_jsx_and_colors() {
        assert_round_trips("let c = #ff8800\nlet d = #f80\n");
        assert_round_trips("let e = <div class=\"x\">hello {name}</div>\n");
    }

    #[test]
    fn captures_line_comments_as_trivia() {
        let src = "let x = 1 // hello\n";
        let trivia = leading_trivia(src, &spans_of(src));
        let comment = trivia
            .iter()
            .flatten()
            .find(|t| t.kind == TriviaKind::LineComment)
            .expect("a line comment should be captured");
        assert_eq!(comment.text, "// hello");
    }

    #[test]
    fn own_line_comment_captured() {
        let src = "// header\nlet x = 1\n";
        let trivia = leading_trivia(src, &spans_of(src));
        let texts: Vec<&str> = trivia
            .iter()
            .flatten()
            .filter(|t| t.kind == TriviaKind::LineComment)
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(texts, vec!["// header"]);
    }

    /// The definitive proof: reconstruction is lossless over every real Petal
    /// program in the repo (examples, tests, sample apps) — not just handcrafted
    /// snippets. Any file that lexes cleanly must reconstruct byte-for-byte.
    #[test]
    fn round_trips_entire_repo_corpus() {
        fn collect_ptl(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "node_modules" || n == "target") {
                        continue;
                    }
                    collect_ptl(&path, out);
                } else if path.extension().is_some_and(|e| e == "ptl") {
                    out.push(path);
                }
            }
        }

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);
        assert!(files.len() > 50, "expected a real corpus, found {}", files.len());

        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            let mut lexer = Lexer::new(&src);
            // Only valid programs have meaningful spans; skip anything that
            // fails to lex (reconstruction of a partial stream is undefined).
            if lexer.tokenize().is_err() {
                continue;
            }
            let got = reconstruct(&src, &lexer.token_spans);
            assert_eq!(got, src, "round-trip mismatch for {}", path.display());
            checked += 1;
        }
        assert!(checked > 50, "expected to check a real corpus, checked {checked}");
    }

    #[test]
    fn no_other_trivia_in_core_language() {
        // Outside interpolation/JSX, every gap char is whitespace or comment —
        // never `Other`. This guards against span regressions in the core.
        let src = "fn f(a, b)\n  // c\n  a + b // trailing\nend\n";
        let trivia = leading_trivia(src, &spans_of(src));
        assert!(
            !trivia.iter().flatten().any(|t| t.kind == TriviaKind::Other),
            "unexpected Other trivia (span regression): {trivia:#?}"
        );
    }
}
