//! The parse driver: orchestrates the lexer, the CST-recording parser, tree
//! construction, and AST projection into a single `parse_source` entry point.

use std::rc::Rc;

use crate::ast::Stmt;
use crate::error::{LoadError, Phase};
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
    parse_source_phased(source, file).map_err(|e| e.to_string())
}

/// [`parse_source`] with a typed error: the lexer's failures are tagged
/// [`Phase::Lex`] and the parser's / projector's [`Phase::Parse`], so the CLI
/// no longer has to sniff the message text to tell them apart. Positions are
/// recovered from the legacy `" [line N, column M]"` suffix — see
/// [`crate::error`] for why that is still the case here.
pub fn parse_source_phased(
    source: &str,
    file: FileId,
) -> Result<(Rc<GreenNode>, Vec<Stmt>), LoadError> {
    parse_with(Lexer::new_in_file(source, file), source, file)
}

/// [`parse_source`] the way source read before `else if` was one token: each
/// `else if` is an `else` block holding an `if` that takes its own `end`
/// ([`Lexer::unmerged_else_if`]). For the `else-if-nesting` migration of
/// `petal migrate-old-code` only; nothing compiles this parse.
pub(crate) fn parse_source_unmerged_else_if(source: &str) -> Result<Vec<Stmt>, String> {
    parse_with(Lexer::new_in_file(source, ENTRY_FILE).unmerged_else_if(), source, ENTRY_FILE)
        .map(|(_, stmts)| stmts)
        .map_err(|e| e.to_string())
}

fn parse_with(
    mut lexer: Lexer,
    source: &str,
    file: FileId,
) -> Result<(Rc<GreenNode>, Vec<Stmt>), LoadError> {
    let phased = |phase: Phase| {
        move |e: String| {
            let mut err = LoadError::legacy(phase, e);
            if let Some(span) = err.items[0].span.as_mut() {
                span.file = file;
            }
            err
        }
    };
    lexer.tokenize().map_err(phased(Phase::Lex))?;
    let mut parser = crate::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
    let direct = parser.parse_program().map_err(phased(Phase::Parse))?;
    let green = build_tree(
        parser.cst_events(),
        &lexer.tokens,
        &lexer.token_spans,
        &lexer.token_leading_trivia,
        source,
    );
    let projected = crate::cst_project::project_in_file(&SyntaxNode::new_root(green.clone()), file)
        .map_err(phased(Phase::Parse))?;
    // The corpus tests prove projection ≡ direct parse; this catches drift on
    // inputs the corpus lacks.
    #[cfg(debug_assertions)]
    check_projection(source, file, &direct, &projected);
    #[cfg(not(debug_assertions))]
    let _ = direct;
    Ok((green, projected))
}

/// The debug-build check that the projected AST is the parser's direct AST.
///
/// The AST has no `PartialEq`, so the two are compared through their `Debug`
/// output — which costs far more than the parse itself. Every `Env` re-parses
/// the same stdlib modules and tests load the same programs again and again,
/// so a given text (in a given file) is checked once per process, and the two
/// renderings are streamed into hashes rather than built as strings. Only a
/// mismatch pays for the pretty-printed assert that shows the difference.
#[cfg(debug_assertions)]
fn check_projection(source: &str, file: FileId, direct: &[Stmt], projected: &[Stmt]) {
    use std::collections::HashSet;
    use std::fmt::Write as _;
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::sync::{LazyLock, Mutex};

    struct HashWriter(DefaultHasher);
    impl std::fmt::Write for HashWriter {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.write(s.as_bytes());
            Ok(())
        }
    }
    let debug_hash = |stmts: &[Stmt]| {
        let mut w = HashWriter(DefaultHasher::new());
        write!(w, "{stmts:?}").expect("hashing never fails");
        w.0.finish()
    };

    static CHECKED: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Default::default);
    let mut key = DefaultHasher::new();
    (source, file).hash(&mut key);
    let key = key.finish();
    if CHECKED.lock().is_ok_and(|seen| seen.contains(&key)) {
        return;
    }
    if debug_hash(direct) != debug_hash(projected) {
        assert_eq!(
            format!("{direct:#?}"),
            format!("{projected:#?}"),
            "CST projection diverged from the parser's direct AST"
        );
    }
    if let Ok(mut seen) = CHECKED.lock() {
        seen.insert(key);
    }
}
