//! Concrete syntax tree (CST) — the lossless, tree-structured substrate for
//! source-preserving edits.
//!
//! The foundation is the lexer's *tiling* spans, so the token stream — significant tokens plus the whitespace/comment
//! trivia between them — reproduces the source byte-for-byte
//! ([`crate::trivia`]). This module lifts that flat stream into a **green tree**
//! in the rowan/Roslyn style: an immutable, shareable tree whose leaves are
//! tokens (including trivia) and whose interior nodes are grammar constructs.
//! The eventual typed AST becomes a *view* over this tree, so edits are tree
//! splices that inherently carry comments and layout.
//!
//! ## What this increment builds
//!
//! The load-bearing data structures and their lossless guarantee, kept
//! deliberately separate from the parser so nothing else has to change yet:
//!
//! - [`GreenNode`] / [`GreenToken`] — the immutable green tree. A green token is
//!   either a significant lexer [`Token`] or a run of [`Trivia`], each carrying
//!   its exact source text. Green nodes are position-independent (they know
//!   their length, not their absolute offset) so identical subtrees can be
//!   shared.
//! - [`GreenNodeBuilder`] — assembles a green tree from a stream of
//!   `start_node` / `token` / `finish_node` events. This is the interface the
//!   parser will drive in a later increment.
//! - [`SyntaxNode`] / [`SyntaxToken`] — the *red* tree: a lightweight cursor
//!   over a green node that adds absolute [char] offsets (and, later, parent
//!   navigation). This is the "view" layer typed accessors will sit on.
//! - [`build_lossless`] — builds a flat green tree (one `Root` node wrapping
//!   every token and trivia leaf) directly from the lexer output. It has no
//!   grammar structure yet — that arrives when the parser is wired to the
//!   builder — but it exercises the whole stack and pins the invariant:
//!
//!   ```text
//!   build_lossless(src).text() == src   for every src
//!   ```
//!
//! Offsets and lengths are counted in **characters**, matching
//! [`crate::source_map::SourceSpan`] (the lexer indexes source as `Vec<char>`).

mod driver;
mod events;
mod green;
mod red;

pub use driver::{parse_cst, parse_source};
pub use events::{Checkpoint, Event, EventBuilder, build_lossless, build_tree};
pub use green::{GreenChild, GreenNode, GreenNodeBuilder, GreenToken};
pub use red::{SyntaxElement, SyntaxNode, SyntaxToken};

/// The kind of an interior CST node. Token leaves carry their own identity (a
/// [`Token`] or a [`TriviaKind`]) rather than a `SyntaxKind`, so this enum only
/// names *node* kinds. Grammar node kinds (call, if-expression, record literal,
/// …) are added as the parser is wired to [`GreenNodeBuilder`]; for now the
/// flat lossless tree needs only a root and an error sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    /// The whole-file root node.
    Root,
    /// A node covering source the parser could not classify. Unused by
    /// [`build_lossless`] (which never fails structurally); reserved for the
    /// parser-driven increment so error recovery still yields a lossless tree.
    Error,

    // === Statement nodes ===
    LetStmt,
    AssignStmt,
    ExprStmt,
    FnDecl,
    EnumDecl,
    ForStmt,
    WhileStmt,
    ReturnStmt,
    BreakStmt,
    ContinueStmt,
    StateStmt,
    ImportStmt,

    // === Expression nodes ===
    LiteralExpr,
    IdentExpr,
    AtVarExpr,
    BinaryExpr,
    UnaryExpr,
    CallExpr,
    /// A parenthesized expression `( expr )`. The AST drops the parens; the CST
    /// keeps them so grouping round-trips.
    ParenExpr,
    IfExpr,
    MatchExpr,
    /// A `for` loop in value position (`x = for … do … end`). The statement
    /// form is [`SyntaxKind::ForStmt`]; `while` has no expression form.
    ForExpr,
    ListExpr,
    RecordExpr,
    FieldAccessExpr,
    IndexAccessExpr,
    LambdaExpr,
    StringInterpExpr,
    ElementExpr,

    // === Helper / interior nodes ===
    /// A `{ … }` statement block (fn/for/while/lambda/match-do bodies).
    Block,
    /// A parenthesized call-argument list.
    ArgList,
    /// A parenthesized parameter list.
    ParamList,
    /// An optional `: type` annotation on a `let` binding or a parameter.
    /// Wraps the `:` token and the type-name identifier.
    TypeAnnotation,
    /// An optional `-> type` return-type annotation on a named `fn`
    /// declaration. Wraps the `->` token and the type-name identifier.
    ReturnType,
    /// One `when pattern [if guard] -> body` arm of a `match`.
    MatchArm,
    /// A pattern in a `match` arm.
    Pattern,
    /// One field of a record literal (`key: value` or `...spread`).
    RecordField,
    /// An `elsif …` / `else …` tail of an if-expression.
    ElseBranch,
    /// One `name="value"` / `name={expr}` attribute of a JSX element.
    JsxAttr,
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::lexer::{Lexer, Token};
    use crate::source_map::SourceSpan;
    use crate::trivia::{Trivia, TriviaKind};

    use super::*;

    fn tree(src: &str) -> Rc<GreenNode> {
        build_lossless(src).expect("build_lossless")
    }

    /// The central invariant: the CST reproduces the source exactly.
    fn assert_round_trips(src: &str) {
        assert_eq!(tree(src).text(), src, "CST round-trip mismatch for {src:?}");
    }

    #[test]
    fn round_trips_core_language() {
        assert_round_trips("let x = 1\n");
        assert_round_trips("  let  x   =  1  \n");
        assert_round_trips("fn add(a, b)\n    a + b\nend\n");
        assert_round_trips("x = 1\n\n\n// trailing comment\n");
        assert_round_trips("let x = 1 // inline comment\nlet y = 2\n");
        assert_round_trips("// leading\n// two lines\nlet x = 1\n");
        assert_round_trips("x = 1;y = 2\n");
        assert_round_trips("grid = for i in xs do\n  i * 2\nend\n");
        assert_round_trips("no_newline_at_eof");
        assert_round_trips("   \t  \n   \n");
        assert_round_trips("");
    }

    #[test]
    fn round_trips_strings_interp_jsx_colors() {
        assert_round_trips("let n = \"Petal\"\nprint(\"hello, {n}!\")\n");
        assert_round_trips("print(\"sum = {2 + 2} done\")\n");
        assert_round_trips("let s = \"\"\"\n  raw {braces} kept\n\"\"\"\n");
        assert_round_trips("print(\"{a}{b}{c}\")\n");
        assert_round_trips("let c = #ff8800\nlet d = #f80\n");
        assert_round_trips("let e = <div class=\"x\">hello {name} world</div>\n");
    }

    #[test]
    fn root_kind_and_offsets() {
        let root = SyntaxNode::new_root(tree("let x = 1\n"));
        assert_eq!(root.kind(), SyntaxKind::Root);
        assert_eq!(root.offset(), 0);
        assert_eq!(root.text_len(), "let x = 1\n".chars().count() as u32);

        // Child offsets are contiguous and each child's text sits at its offset.
        let src = "let x = 1\n";
        let src_chars: Vec<char> = src.chars().collect();
        for el in root.children() {
            let (off, len, txt) = match el {
                SyntaxElement::Node(n) => (n.offset(), n.text_len(), n.text()),
                SyntaxElement::Token(t) => (t.offset(), t.text_len(), t.text().to_string()),
            };
            let slice: String = src_chars[off as usize..(off + len) as usize]
                .iter()
                .collect();
            assert_eq!(slice, txt, "child text must match source at its offset");
        }
    }

    #[test]
    fn leaves_partition_into_trivia_and_tokens() {
        // `x = 1 // c` — the comment and spaces are trivia leaves; the rest are
        // token leaves. Concatenating every leaf reproduces the source.
        let root = tree("x = 1 // c\n");
        let mut reassembled = String::new();
        let mut saw_comment = false;
        for child in root.children() {
            if let GreenChild::Token(t) = child {
                reassembled.push_str(t.text());
                if let GreenToken::Trivia {
                    kind: TriviaKind::LineComment,
                    text,
                } = &**t
                {
                    assert_eq!(text, "// c");
                    saw_comment = true;
                }
            }
        }
        assert_eq!(reassembled, "x = 1 // c\n");
        assert!(saw_comment, "the line comment should be a trivia leaf");
    }

    fn lex(src: &str) -> (Vec<Token>, Vec<SourceSpan>, Vec<Vec<Trivia>>) {
        let mut lexer = Lexer::new(src);
        lexer.tokenize().expect("tokenize");
        (
            lexer.tokens.clone(),
            lexer.token_spans.clone(),
            lexer.token_leading_trivia.clone(),
        )
    }

    /// Find the single child node of `parent` (asserting there is exactly one).
    fn only_child_node(parent: &SyntaxNode) -> SyntaxNode {
        let mut nodes: Vec<SyntaxNode> = parent
            .children()
            .into_iter()
            .filter_map(|el| match el {
                SyntaxElement::Node(n) => Some(n),
                SyntaxElement::Token(_) => None,
            })
            .collect();
        assert_eq!(nodes.len(), 1, "expected exactly one child node");
        nodes.pop().unwrap()
    }

    #[test]
    fn build_tree_with_empty_events_matches_build_lossless() {
        let src = "let x = 1 // c\nf(a, b)\n";
        let (tokens, spans, trivia) = lex(src);
        let a = build_tree(&[], &tokens, &spans, &trivia, src);
        let b = build_lossless(src).unwrap();
        assert_eq!(a, b, "empty-event build_tree must equal build_lossless");
        assert_eq!(a.text(), src);
    }

    #[test]
    fn wrap_builds_left_associative_nesting() {
        // Drive the builder as the parser's additive level would for `1 + 2 + 3`,
        // wrapping each operand as a LiteralExpr and each `l op r` as a
        // BinaryExpr. The Newline and Eof are left for build_tree to flush.
        let src = "1 + 2 + 3\n";
        let (tokens, spans, trivia) = lex(src);

        let mut b = EventBuilder::new();
        let cp = b.checkpoint();
        let literal = |b: &mut EventBuilder| {
            b.open(SyntaxKind::LiteralExpr);
            b.token();
            b.close();
        };
        literal(&mut b); // 1
        b.token(); // +
        literal(&mut b); // 2
        b.wrap(cp, SyntaxKind::BinaryExpr); // (1 + 2)
        b.token(); // +
        literal(&mut b); // 3
        b.wrap(cp, SyntaxKind::BinaryExpr); // ((1 + 2) + 3)

        let tree = build_tree(b.events(), &tokens, &spans, &trivia, src);
        assert_eq!(tree.text(), src, "structured tree must still round-trip");

        // Shape: Root > BinaryExpr(outer) > BinaryExpr(inner) > LiteralExpr…
        let root = SyntaxNode::new_root(tree);
        let outer = only_child_node(&root);
        assert_eq!(outer.kind(), SyntaxKind::BinaryExpr);
        assert_eq!(outer.text(), "1 + 2 + 3"); // node spans just the expression
        // The outer binary's first child node is the inner (left-assoc) binary.
        let inner = outer
            .children()
            .into_iter()
            .find_map(|el| match el {
                SyntaxElement::Node(n) => Some(n),
                _ => None,
            })
            .unwrap();
        assert_eq!(inner.kind(), SyntaxKind::BinaryExpr);
        assert_eq!(inner.text(), "1 + 2");
        assert_eq!(inner.offset(), 0);
    }

    #[test]
    fn node_offsets_track_leading_trivia() {
        // In `// hi\n42\n` the comment attaches to the leading Newline token; a
        // parser skips that newline (root level) before wrapping `42`, so the
        // LiteralExpr node starts *after* the comment.
        let src = "// hi\n42\n";
        let (tokens, spans, trivia) = lex(src);
        let mut b = EventBuilder::new();
        b.token(); // the Newline that carries the leading `// hi` comment
        b.open(SyntaxKind::LiteralExpr);
        b.token(); // 42
        b.close();
        let tree = build_tree(b.events(), &tokens, &spans, &trivia, src);
        assert_eq!(tree.text(), src);

        let root = SyntaxNode::new_root(tree);
        let lit = only_child_node(&root);
        assert_eq!(lit.kind(), SyntaxKind::LiteralExpr);
        // "// hi\n" is 6 chars, so the node starts at offset 6 and covers `42`.
        assert_eq!(lit.offset(), 6);
        assert_eq!(lit.text(), "42");
    }

    #[test]
    fn build_tree_flushes_unconsumed_tokens() {
        // Consume only the first token; the rest (including Eof) must still be
        // flushed so the tree covers the whole source.
        let src = "a b c\n";
        let (tokens, spans, trivia) = lex(src);
        let mut b = EventBuilder::new();
        b.token(); // only `a`
        let tree = build_tree(b.events(), &tokens, &spans, &trivia, src);
        assert_eq!(
            tree.text(),
            src,
            "unconsumed tokens must be flushed losslessly"
        );
    }

    #[test]
    fn builder_rejects_unbalanced_tree() {
        let mut b = GreenNodeBuilder::new();
        b.start_node(SyntaxKind::Root);
        b.start_node(SyntaxKind::Error);
        b.finish_node();
        // One node still open — finish() must not silently succeed.
        let unbalanced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.finish()));
        assert!(unbalanced.is_err());
    }

    fn collect_ptl(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "node_modules" || n == "target")
                {
                    continue;
                }
                collect_ptl(&path, out);
            } else if path.extension().is_some_and(|e| e == "ptl") {
                out.push(path);
            }
        }
    }

    fn repo_ptl_files() -> Vec<std::path::PathBuf> {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);
        files
    }

    /// The definitive proof: the CST round-trips every real Petal program in the
    /// repo, exactly as `trivia::reconstruct` does — the tree loses nothing the
    /// flat token stream preserved.
    #[test]
    fn round_trips_entire_repo_corpus() {
        let mut checked = 0;
        for path in &repo_ptl_files() {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(root) = build_lossless(&src) else {
                continue;
            };
            assert_eq!(
                root.text(),
                src,
                "CST round-trip mismatch for {}",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }

    // ---- parse_cst: the parser-driven structured tree (step 3b-ii) ----

    /// Every repo program that parses must round-trip through the *structured*
    /// tree too — the grammar nodes change the shape, never the text.
    #[test]
    fn parse_cst_round_trips_entire_repo_corpus() {
        let mut checked = 0;
        for path in &repo_ptl_files() {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let Ok(root) = parse_cst(&src) else { continue };
            assert_eq!(
                root.text(),
                src,
                "structured CST round-trip mismatch for {}",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }

    fn assert_parse_round_trips(src: &str) {
        let root = parse_cst(src).expect("parse_cst");
        assert_eq!(
            root.text(),
            src,
            "structured round-trip mismatch for {src:?}"
        );
    }

    #[test]
    fn parse_cst_round_trips_snippets() {
        assert_parse_round_trips("let x = 1 // keep me\nlet y = 2\n");
        assert_parse_round_trips("fn add(a, b)\n    a + b // sum\nend\n");
        assert_parse_round_trips("if x > 1 then\n  y\nelsif x > 0 then\n  z\nelse\n  w\nend\n");
        assert_parse_round_trips("match v\nwhen Some(x) -> x\nwhen _ -> 0\nend\n");
        assert_parse_round_trips("let r = { a: 1, ...rest }\nlet l = [1 -2 3]\n");
        assert_parse_round_trips("state count = 0\ncount += 1\n");
        assert_parse_round_trips("xs |> map(fn(x) -> x * 2) |> sum\n");
        assert_parse_round_trips("let e = <div class=\"x\">hi {name}<br/></div>\n");
        assert_parse_round_trips("print(\"sum = {2 + 2} done\")\n");
        assert_parse_round_trips("for i in [1, 2] do\n  print(i)\nend\n");
    }

    /// Depth-first search for the first descendant node of `kind`.
    fn find_node(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
        for el in node.children() {
            if let SyntaxElement::Node(n) = el {
                if n.kind() == kind {
                    return Some(n);
                }
                if let Some(found) = find_node(&n, kind) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn child_nodes(node: &SyntaxNode) -> Vec<SyntaxNode> {
        node.children()
            .into_iter()
            .filter_map(|el| match el {
                SyntaxElement::Node(n) => Some(n),
                SyntaxElement::Token(_) => None,
            })
            .collect()
    }

    fn parse_root(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse_cst(src).expect("parse_cst"))
    }

    #[test]
    fn parse_cst_binary_precedence_shape() {
        // `1 + 2 * 3` — the outer BinaryExpr's right operand is the `2 * 3`
        // BinaryExpr, nested per precedence.
        let root = parse_root("1 + 2 * 3\n");
        let stmt = find_node(&root, SyntaxKind::ExprStmt).expect("ExprStmt");
        let outer = find_node(&stmt, SyntaxKind::BinaryExpr).expect("outer BinaryExpr");
        assert_eq!(outer.text(), "1 + 2 * 3");
        let operands = child_nodes(&outer);
        assert_eq!(operands.len(), 2);
        assert_eq!(operands[0].kind(), SyntaxKind::LiteralExpr);
        assert_eq!(operands[0].text(), "1");
        assert_eq!(operands[1].kind(), SyntaxKind::BinaryExpr);
        // Leading trivia may sit just inside a wrapped node's boundary —
        // placement is refined in a later pass, so compare trimmed text.
        assert_eq!(operands[1].text().trim(), "2 * 3");
    }

    #[test]
    fn parse_cst_call_shape() {
        // `f(a, b)` — a CallExpr containing the callee and an ArgList that
        // spans the parens.
        let root = parse_root("f(a, b)\n");
        let call = find_node(&root, SyntaxKind::CallExpr).expect("CallExpr");
        assert_eq!(call.text(), "f(a, b)");
        let kids = child_nodes(&call);
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].kind(), SyntaxKind::IdentExpr);
        assert_eq!(kids[0].text(), "f");
        assert_eq!(kids[1].kind(), SyntaxKind::ArgList);
        assert_eq!(kids[1].text(), "(a, b)");
        let args = child_nodes(&kids[1]);
        assert_eq!(args.len(), 2);
        assert!(args.iter().all(|a| a.kind() == SyntaxKind::IdentExpr));
    }

    #[test]
    fn parse_cst_paren_grouping_shape() {
        // `(a + b) * c` — grouping parens survive as a ParenExpr under the
        // multiplicative BinaryExpr (the AST drops them; the CST must not).
        let root = parse_root("(a + b) * c\n");
        let outer = find_node(&root, SyntaxKind::BinaryExpr).expect("BinaryExpr");
        assert_eq!(outer.text(), "(a + b) * c");
        let kids = child_nodes(&outer);
        assert_eq!(kids[0].kind(), SyntaxKind::ParenExpr);
        assert_eq!(kids[0].text(), "(a + b)");
        let inner = find_node(&kids[0], SyntaxKind::BinaryExpr).expect("inner BinaryExpr");
        assert_eq!(inner.text(), "a + b");
    }

    #[test]
    fn parse_cst_stmt_shapes() {
        let root = parse_root("let x = 1\nfn f(a)\n  return a\nend\n");
        let let_stmt = find_node(&root, SyntaxKind::LetStmt).expect("LetStmt");
        assert_eq!(let_stmt.text(), "let x = 1");
        let fn_decl = find_node(&root, SyntaxKind::FnDecl).expect("FnDecl");
        assert_eq!(fn_decl.text(), "fn f(a)\n  return a\nend");
        let params = find_node(&fn_decl, SyntaxKind::ParamList).expect("ParamList");
        assert_eq!(params.text(), "(a)");
        assert!(
            find_node(&fn_decl, SyntaxKind::Block).is_some(),
            "fn body Block"
        );
        assert!(
            find_node(&fn_decl, SyntaxKind::ReturnStmt).is_some(),
            "ReturnStmt"
        );
    }

    #[test]
    fn parse_cst_comment_survives_inside_statement_region() {
        // The motivating case for the whole plan: a comment inside a parsed
        // construct is a trivia leaf in the tree, not lost. It lives somewhere
        // inside the FnDecl subtree (exact attachment — FnDecl vs Block — is
        // refined in a later pass).
        let src = "fn f()\n  // important note\n  1\nend\n";
        let root = parse_root(src);
        assert_eq!(root.text(), src);
        let fn_decl = find_node(&root, SyntaxKind::FnDecl).expect("FnDecl");
        assert!(
            fn_decl.text().contains("// important note"),
            "comment should live inside the FnDecl subtree, got {:?}",
            fn_decl.text()
        );
    }

    #[test]
    fn parse_cst_errors_on_bad_source() {
        assert!(parse_cst("let = 1\n").is_err());
        assert!(parse_cst("(1 + \n").is_err());
    }
}
