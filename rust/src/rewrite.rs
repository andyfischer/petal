//! Source rewriting — programmatic, formatting-preserving edits to Petal source.
//!
//! Petal's AST (see [`crate::ast`]) is *lossy*: it carries source spans but not
//! comments or the original whitespace, so re-emitting a whole parsed program
//! would discard the author's formatting. The tooling here edits the
//! **lossless green tree** instead ([`crate::cst`]): it locates the tree node
//! covering the construct
//! being edited, splices in a subtree parsed from the replacement snippet —
//! keeping the old node's leading/trailing trivia (comments, indentation)
//! around it — and re-emits the tree's text. Everything outside the replaced
//! node (comments, blank lines, unrelated statements) is untouched because it
//! is still the same shared green subtrees.
//!
//! This is what an embedder needs when the program is *also* a live document
//! the user is editing: a tool can rewrite, say, a single `layout(...)` call and
//! write the result back without clobbering the rest of the file.
//!
//! [`splice`] remains as the span-based *string* fallback for replacements
//! that don't parse as a single expression. Spans store **character** offsets
//! (the lexer indexes source as `Vec<char>`), so it slices by character, not
//! byte — safe for multi-byte source.

use std::rc::Rc;

use crate::ast::{Expr, ExprKind, Stmt, StmtKind};
use crate::cst::{
    parse_cst, parse_source, GreenChild, GreenNode, SyntaxElement, SyntaxKind, SyntaxNode,
    SyntaxToken,
};
use crate::source_map::{SourceSpan, ENTRY_FILE};

/// Parse `source` into its lossless green tree plus its top-level statements,
/// preserving each node's source span. A thin wrapper over
/// [`crate::cst::parse_source`] for tools that want to inspect or rewrite
/// source rather than run it; the tree carries the comments and layout the AST
/// drops, which is what tree-splice edits operate on.
pub fn parse_ast(source: &str) -> Result<(Rc<GreenNode>, Vec<Stmt>), String> {
    parse_source(source, ENTRY_FILE)
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
    if let ExprKind::Call { function, .. } = &expr.kind
        && let ExprKind::Ident(ident) = &function.kind
        && ident == name
    {
        return Some(expr.span);
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

/// Tree splice: replace the node of `tree` whose significant tokens cover
/// exactly `span` with `replacement`, parsed as a single expression (or
/// statement). The old node's leading/trailing trivia leaves — indentation and
/// comments at its edges — are kept around the new subtree, and everything
/// outside the node is shared with the original tree, so all other formatting
/// survives by construction.
///
/// Returns the edited tree (read the new source off it with
/// [`crate::cst::GreenNode::text`]), or `None` when the splice can't be
/// resolved to tree nodes: no node's significant range matches `span`, or
/// `replacement` doesn't parse as a single statement whose node reproduces it
/// verbatim. Callers should then fall back to the string-level [`splice`].
pub fn splice_node(
    tree: &Rc<GreenNode>,
    span: SourceSpan,
    replacement: &str,
) -> Option<Rc<GreenNode>> {
    let root = SyntaxNode::new_root(Rc::clone(tree));
    let path = find_node_path(&root, (span.start.offset, span.end.offset))?;
    let repl = replacement_node(replacement)?;
    Some(splice_at(tree, &path, repl))
}

/// Path of child indices from `node` to the deepest descendant node whose
/// significant range (first .. last non-trivia token) is exactly `range` —
/// the tree node an AST span identifies, since AST spans exclude trivia.
fn find_node_path(node: &SyntaxNode, range: (u32, u32)) -> Option<Vec<usize>> {
    for (i, el) in node.children().into_iter().enumerate() {
        let SyntaxElement::Node(child) = el else { continue };
        // The node's full extent (trivia included) must cover the range; a
        // node's extent is always a superset of its significant range.
        if child.offset() > range.0 || child.offset() + child.text_len() < range.1 {
            continue;
        }
        if let Some(mut path) = find_node_path(&child, range) {
            path.insert(0, i);
            return Some(path);
        }
        if significant_range(&child) == Some(range) {
            return Some(vec![i]);
        }
    }
    None
}

/// Char range `[start, end)` of `node`'s significant tokens, or `None` for an
/// all-trivia node.
fn significant_range(node: &SyntaxNode) -> Option<(u32, u32)> {
    let first = edge_significant_token(node, false)?;
    let last = edge_significant_token(node, true)?;
    Some((first.offset(), last.offset() + last.text_len()))
}

/// First (`from_end: false`) or last (`from_end: true`) non-trivia token leaf
/// in `node`'s subtree.
fn edge_significant_token(node: &SyntaxNode, from_end: bool) -> Option<SyntaxToken> {
    let mut children = node.children();
    if from_end {
        children.reverse();
    }
    children.into_iter().find_map(|el| match el {
        SyntaxElement::Node(n) => edge_significant_token(&n, from_end),
        SyntaxElement::Token(t) if !t.is_trivia() => Some(t),
        SyntaxElement::Token(_) => None,
    })
}

/// Parse `replacement` and return its sole statement's node — unwrapping an
/// `ExprStmt` to the expression inside, so a call replaces a call. Returns
/// `None` unless the node's text reproduces `replacement` exactly (a trailing
/// newline or comment falls outside the node and would be dropped, and a
/// multi-statement replacement has no single node), so the caller's verbatim
/// contract can never be silently violated.
fn replacement_node(replacement: &str) -> Option<Rc<GreenNode>> {
    let root = SyntaxNode::new_root(parse_cst(replacement).ok()?);
    let mut stmts = root.children().into_iter().filter_map(|el| match el {
        SyntaxElement::Node(n) => Some(n),
        SyntaxElement::Token(_) => None,
    });
    let stmt = stmts.next()?;
    if stmts.next().is_some() {
        return None;
    }
    let node = if stmt.kind() == SyntaxKind::ExprStmt {
        match &stmt.children()[..] {
            [SyntaxElement::Node(expr)] => expr.clone(),
            _ => stmt,
        }
    } else {
        stmt
    };
    (node.text() == replacement).then(|| Rc::clone(node.green()))
}

/// Rebuild the spine along `path`, replacing the node at its end with `repl`
/// plus the replaced node's own leading/trailing trivia leaves — the comments
/// and indentation sitting inside the old node's edges survive the edit.
fn splice_at(node: &Rc<GreenNode>, path: &[usize], repl: Rc<GreenNode>) -> Rc<GreenNode> {
    let (&i, rest) = path.split_first().expect("splice path is never empty");
    let GreenChild::Node(child) = &node.children()[i] else {
        unreachable!("splice path indexes a token leaf");
    };
    if !rest.is_empty() {
        return node.replace_child(i, GreenChild::Node(splice_at(child, rest, repl)));
    }
    // Old node -> [its leading trivia..., repl, its trailing trivia...],
    // spliced into the parent as siblings so the result stays structurally a
    // 1:1 node replacement (trivia leaves are transparent to projection).
    let mut spliced = Vec::new();
    collect_edge_trivia(child, false, &mut spliced);
    spliced.push(GreenChild::Node(repl));
    let at = spliced.len();
    collect_edge_trivia(child, true, &mut spliced);
    spliced[at..].reverse();
    let mut children = node.children().to_vec();
    children.splice(i..=i, spliced);
    GreenNode::with_children(node.kind(), children)
}

/// Collect the trivia leaves at one edge of `node`'s subtree: everything
/// before its first significant token (`from_end: false`) or after its last
/// (`from_end: true`), pushed in visit order — reversed source order for the
/// trailing edge, so that caller reverses its slice.
fn collect_edge_trivia(node: &GreenNode, from_end: bool, out: &mut Vec<GreenChild>) -> bool {
    let mut children: Vec<&GreenChild> = node.children().iter().collect();
    if from_end {
        children.reverse();
    }
    for child in children {
        match child {
            GreenChild::Token(t) if t.is_trivia() => out.push(child.clone()),
            GreenChild::Token(_) => return true,
            GreenChild::Node(n) => {
                if collect_edge_trivia(n, from_end, out) {
                    return true;
                }
            }
        }
    }
    false
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
    let (tree, stmts) = parse_ast(source)?;
    match find_call(&stmts, name) {
        Some(span) => Ok(match splice_node(&tree, span, replacement) {
            Some(edited) => edited.text(),
            // Replacement isn't a single parseable expression: splice it in
            // verbatim at the string level.
            None => splice(source, span, replacement),
        }),
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
        let (_, stmts) = parse_ast(src).unwrap();
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
        let (_, stmts) = parse_ast(src).unwrap();
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
        let (_, stmts) = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").unwrap();
        let out = splice(src, span, "layout(editor())");
        assert_eq!(out, "layout(editor())\nx = 2\n");
    }

    #[test]
    fn ignores_calls_with_other_names() {
        let src = "layout(editor())\n";
        let (_, stmts) = parse_ast(src).unwrap();
        assert!(find_call(&stmts, "set_layout").is_none());
    }

    /// Tree-splice `src`'s first top-level `name(...)` call, asserting the
    /// tree path was actually taken (no silent string fallback).
    fn tree_replaced(src: &str, name: &str, replacement: &str) -> String {
        let (tree, stmts) = parse_ast(src).unwrap();
        let span = find_call(&stmts, name).expect("call found");
        let edited = splice_node(&tree, span, replacement).expect("tree splice resolved");
        edited.text()
    }

    #[test]
    fn tree_splice_keeps_inline_comment_after_replaced_call() {
        // The comment-preservation win of 3d-ii: the comment trailing the
        // replaced call is a trivia leaf outside the call node and survives.
        let out = tree_replaced(
            "layout(editor()) // chosen by user\nx = 1\n",
            "layout",
            "layout(row([editor()]))",
        );
        assert_eq!(out, "layout(row([editor()])) // chosen by user\nx = 1\n");
    }

    #[test]
    fn tree_splice_keeps_leading_trivia_inside_replaced_node() {
        // The indentation before `layout` is a trivia leaf *inside* the call's
        // node (leading trivia of its first token); the splice must keep it
        // around the new subtree rather than dropping it with the old node.
        let out = tree_replaced(
            "x = 1\n    layout(editor()) // note\n",
            "layout",
            "layout(editor(\"a.rs\"))",
        );
        assert_eq!(out, "x = 1\n    layout(editor(\"a.rs\")) // note\n");
    }

    #[test]
    fn tree_splice_keeps_comments_around_multiline_call() {
        let src = "// before\nlayout(\n    column([\n        editor(),\n    ])\n)\n// after\nx = 2\n";
        let out = tree_replaced(src, "layout", "layout(editor())");
        assert_eq!(out, "// before\nlayout(editor())\n// after\nx = 2\n");
    }

    #[test]
    fn replace_or_append_falls_back_to_string_splice_for_unparseable_replacement() {
        // Not a parseable expression — spliced in verbatim at the string level.
        let out = replace_or_append_call("layout(editor())\n", "layout", "<<broken").unwrap();
        assert_eq!(out, "<<broken\n");
    }

    #[test]
    fn edited_tree_reparses_and_can_be_edited_again() {
        // The edit's output is real source: parse it again and splice again.
        let once = tree_replaced("layout(a()) // c\n", "layout", "layout(b())");
        let twice = tree_replaced(&once, "layout", "layout(c())");
        assert_eq!(twice, "layout(c()) // c\n");
    }

    #[test]
    fn multibyte_source_splices_by_char() {
        // A comment with multi-byte characters before the call: the char-offset
        // splice must not corrupt it.
        let src = "// café ☕ notes\nlayout(editor())\n";
        let (_, stmts) = parse_ast(src).unwrap();
        let span = find_call(&stmts, "layout").unwrap();
        let out = splice(src, span, "layout(editor(\"x\"))");
        assert_eq!(out, "// café ☕ notes\nlayout(editor(\"x\"))\n");
    }
}
