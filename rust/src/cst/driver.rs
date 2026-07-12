//! The parse driver: orchestrates the lexer, the CST-recording parser, tree
//! construction, and AST projection into a single `parse_source` entry point.

use std::rc::Rc;

use crate::ast::Stmt;
use crate::lexer::Lexer;
use crate::source_map::{ENTRY_FILE, FileId};

use super::events::build_tree;
use super::green::GreenNode;
use super::red::SyntaxNode;

/// Parse `source` into a *structured* lossless green tree: the parser runs as
/// normal (the AST it builds is discarded) while recording a CST [`super::Event`]
/// stream, which [`build_tree`] materializes with trivia interleaved. The
/// result has grammar nodes ([`super::SyntaxKind::LetStmt`],
/// [`super::SyntaxKind::CallExpr`], …) and still round-trips exactly:
/// `parse_cst(src)?.text() == src`.
///
/// Returns the lexer's or parser's error if `source` does not parse; the tree
/// is only built on success (an error leaves the event stream unbalanced).
pub fn parse_cst(source: &str) -> Result<Rc<GreenNode>, String> {
    parse_source(source, ENTRY_FILE).map(|(green, _)| green)
}

/// Parse once: lex, parse with CST recording, build the green tree, and
/// project the typed AST from it ([`crate::cst_project`]). The tree is the
/// authoritative parse artifact; the AST the parser builds directly is used
/// only for a debug-build differential check against the projection.
///
/// Spans in the returned statements are tagged with `file` (pass
/// [`ENTRY_FILE`] for top-level source, the module's [`FileId`] for imports).
///
/// Returns the lexer's or parser's error if `source` does not parse; the tree
/// is only built on success (an error leaves the event stream unbalanced).
pub fn parse_source(source: &str, file: FileId) -> Result<(Rc<GreenNode>, Vec<Stmt>), String> {
    let mut lexer = Lexer::new_in_file(source, file);
    lexer.tokenize()?;
    let mut parser = crate::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
    let direct = parser.parse_program()?;
    let green = build_tree(
        parser.cst_events(),
        &lexer.tokens,
        &lexer.token_spans,
        &lexer.token_leading_trivia,
        source,
    );
    let projected =
        crate::cst_project::project_in_file(&SyntaxNode::new_root(green.clone()), file)?;
    // The corpus tests prove projection ≡ direct parse; this catches drift on
    // inputs the corpus lacks.
    debug_assert_eq!(
        format!("{direct:#?}"),
        format!("{projected:#?}"),
        "CST projection diverged from the parser's direct AST"
    );
    Ok((green, projected))
}
