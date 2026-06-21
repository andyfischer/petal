//! Source rewriting — programmatic, formatting-preserving edits to Petal source.
//!
//! Petal's AST (see [`crate::ast`]) is *lossy*: it carries source spans but not
//! comments or the original whitespace, so re-emitting a whole parsed program
//! would discard the author's formatting. The tooling here takes the opposite
//! approach — **surgical source splicing**. It parses only to locate the source
//! span of the construct being edited, then replaces exactly those characters
//! in the original text, leaving every other byte (comments, blank lines,
//! unrelated statements) untouched.
//!
//! This is what an embedder needs when the program is *also* a live document
//! the user is editing: a tool can rewrite, say, a single `layout(...)` call and
//! write the result back without clobbering the rest of the file.
//!
//! Spans store **character** offsets (the lexer indexes source as `Vec<char>`),
//! so [`splice`] slices by character, not byte — safe for multi-byte source.

use crate::ast::{Expr, ExprKind, Stmt, StmtKind};
use crate::lexer::Lexer;
use crate::parse::Parser;
use crate::source_map::SourceSpan;

/// Parse `source` into its top-level statements, preserving each node's source
/// span. A thin wrapper over the lexer + parser for tools that want to inspect
/// or rewrite source rather than run it.
pub fn parse_ast(source: &str) -> Result<Vec<Stmt>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    parser.parse_program()
}

/// Find a top-level call `name(...)` written as its own statement, returning the
/// source span covering the whole call expression — from the function name
/// through the closing parenthesis. Returns the first such call in source order,
/// or `None` if there is none.
///
/// Only statement-position calls are considered (the shape of declarative
/// configuration, e.g. a bare `layout(...)` line); a call nested inside another
/// expression is intentionally ignored.
pub fn find_call(stmts: &[Stmt], name: &str) -> Option<SourceSpan> {
    stmts.iter().find_map(|stmt| match &stmt.kind {
        StmtKind::Expr(expr) => call_span_if_named(expr, name),
        _ => None,
    })
}

/// The span of `expr` if it is a call whose callee is the bare identifier
/// `name`, else `None`.
fn call_span_if_named(expr: &Expr, name: &str) -> Option<SourceSpan> {
    if let ExprKind::Call { function, .. } = &expr.kind {
        if let ExprKind::Ident(ident) = &function.kind {
            if ident == name {
                return Some(expr.span);
            }
        }
    }
    None
}

/// Replace the character range covered by `span` in `source` with
/// `replacement`, returning the new source. Everything outside the span is
/// copied verbatim, so unrelated code and comments survive unchanged.
///
/// Offsets are clamped to the source length, so a stale span can never panic
/// (it degrades to appending at the end).
pub fn splice(source: &str, span: SourceSpan, replacement: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let start = (span.start.offset as usize).min(chars.len());
    let end = (span.end.offset as usize).clamp(start, chars.len());
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.extend(chars[..start].iter());
    out.push_str(replacement);
    out.extend(chars[end..].iter());
    out
}

/// Replace the first top-level `name(...)` call in `source` with `replacement`.
/// If there is no such call, `replacement` is appended as a new top-level
/// statement on its own line. Returns the rewritten source.
///
/// `replacement` is the full call text to write (e.g. `"layout(editor())"`); it
/// is spliced in verbatim, so the caller controls its formatting.
pub fn replace_or_append_call(
    source: &str,
    name: &str,
    replacement: &str,
) -> Result<String, String> {
    let stmts = parse_ast(source)?;
    match find_call(&stmts, name) {
        Some(span) => Ok(splice(source, span, replacement)),
        None => {
            let trimmed = source.trim_end_matches('\n');
            if trimmed.is_empty() {
                Ok(format!("{replacement}\n"))
            } else {
                Ok(format!("{trimmed}\n\n{replacement}\n"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_top_level_call_span() {
        let src = "x = 1\nlayout(editor())\n";
        let stmts = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").expect("layout call found");
        // The span should cover exactly `layout(editor())`.
        let chars: Vec<char> = src.chars().collect();
        let text: String = chars[span.start.offset as usize..span.end.offset as usize]
            .iter()
            .collect();
        assert_eq!(text, "layout(editor())");
    }

    #[test]
    fn splice_preserves_surrounding_code_and_comments() {
        let src = "// a comment\nx = 1\nlayout(editor())\n// trailing\n";
        let stmts = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").unwrap();
        let out = splice(src, span, "layout(editor(\"new.rs\"))");
        assert_eq!(
            out,
            "// a comment\nx = 1\nlayout(editor(\"new.rs\"))\n// trailing\n"
        );
    }

    #[test]
    fn replace_or_append_replaces_existing() {
        let src = "layout(editor())\n";
        let out = replace_or_append_call(src, "layout", "layout(row([editor()]))").unwrap();
        assert_eq!(out, "layout(row([editor()]))\n");
    }

    #[test]
    fn replace_or_append_appends_when_missing() {
        let src = "set_theme({})\n";
        let out = replace_or_append_call(src, "layout", "layout(editor())").unwrap();
        assert_eq!(out, "set_theme({})\n\nlayout(editor())\n");
    }

    #[test]
    fn append_to_empty_source() {
        let out = replace_or_append_call("", "layout", "layout(editor())").unwrap();
        assert_eq!(out, "layout(editor())\n");
    }

    #[test]
    fn multiline_call_span_is_replaced_whole() {
        let src = "layout(\n    column([\n        editor(),\n    ])\n)\nx = 2\n";
        let stmts = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").unwrap();
        let out = splice(src, span, "layout(editor())");
        assert_eq!(out, "layout(editor())\nx = 2\n");
    }

    #[test]
    fn ignores_calls_with_other_names() {
        let src = "layout(editor())\n";
        let stmts = parse_ast(src).unwrap();
        assert!(find_call(&stmts, "set_layout").is_none());
    }

    #[test]
    fn multibyte_source_splices_by_char() {
        // A comment with multi-byte characters before the call: the char-offset
        // splice must not corrupt it.
        let src = "// café ☕ notes\nlayout(editor())\n";
        let stmts = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").unwrap();
        let out = splice(src, span, "layout(editor(\"x\"))");
        assert_eq!(out, "// café ☕ notes\nlayout(editor(\"x\"))\n");
    }
}
