//! Concrete syntax tree (CST) — the lossless, tree-structured substrate for
//! source-preserving edits.
//!
//! This is **Step 3** of the source-preservation plan
//! (`docs/dev/source-preservation-plan.md`). Steps 1–2 gave the lexer *tiling*
//! spans, so the token stream — significant tokens plus the whitespace/comment
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

use std::rc::Rc;

use crate::lexer::{Lexer, Token};
use crate::trivia::{Trivia, TriviaKind};

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
}

/// A leaf of the green tree: either a significant token or a trivia run, each
/// with its verbatim source text.
#[derive(Debug, Clone, PartialEq)]
pub enum GreenToken {
    /// A significant lexer token and the exact source text it spans.
    Token { token: Token, text: String },
    /// A run of whitespace or a `//` comment.
    Trivia { kind: TriviaKind, text: String },
}

impl GreenToken {
    /// The exact source text of this leaf.
    pub fn text(&self) -> &str {
        match self {
            GreenToken::Token { text, .. } | GreenToken::Trivia { text, .. } => text,
        }
    }

    /// Length of this leaf in characters (spans are char-indexed).
    pub fn text_len(&self) -> u32 {
        self.text().chars().count() as u32
    }

    /// Whether this leaf is trivia (whitespace or comment) rather than a
    /// significant token.
    pub fn is_trivia(&self) -> bool {
        matches!(self, GreenToken::Trivia { .. })
    }
}

/// A child of a green node: a subtree or a leaf.
#[derive(Debug, Clone, PartialEq)]
pub enum GreenChild {
    Node(Rc<GreenNode>),
    Token(Rc<GreenToken>),
}

impl GreenChild {
    fn text_len(&self) -> u32 {
        match self {
            GreenChild::Node(n) => n.text_len(),
            GreenChild::Token(t) => t.text_len(),
        }
    }

    fn write_text(&self, out: &mut String) {
        match self {
            GreenChild::Node(n) => n.write_text(out),
            GreenChild::Token(t) => out.push_str(t.text()),
        }
    }
}

/// An immutable, position-independent interior node of the green tree.
#[derive(Debug, Clone, PartialEq)]
pub struct GreenNode {
    kind: SyntaxKind,
    children: Vec<GreenChild>,
    /// Total length of all descendants in characters, cached so offset math and
    /// `text_len` are O(1) rather than O(subtree).
    text_len: u32,
}

impl GreenNode {
    /// The node kind.
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// This node's direct children (nodes and leaves), in source order.
    pub fn children(&self) -> &[GreenChild] {
        &self.children
    }

    /// Total length of this subtree in characters.
    pub fn text_len(&self) -> u32 {
        self.text_len
    }

    /// Reconstruct the exact source text this subtree covers.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.text_len as usize);
        self.write_text(&mut out);
        out
    }

    fn write_text(&self, out: &mut String) {
        for child in &self.children {
            child.write_text(out);
        }
    }
}

/// Assembles a [`GreenNode`] from a stream of `start_node` / `token` /
/// `finish_node` events — the interface the parser will drive to emit a CST as
/// it recognizes constructs.
///
/// ```
/// use petal::cst::{GreenNodeBuilder, GreenToken, SyntaxKind};
/// use petal::lexer::Token;
/// let mut b = GreenNodeBuilder::new();
/// b.start_node(SyntaxKind::Root);
/// b.token(GreenToken::Token { token: Token::Int(1), text: "1".to_string() });
/// let root = b.finish();
/// assert_eq!(root.text(), "1");
/// ```
#[derive(Default)]
pub struct GreenNodeBuilder {
    /// Stack of in-progress nodes: `(kind, children-so-far)`.
    stack: Vec<(SyntaxKind, Vec<GreenChild>)>,
    /// The finished root, set when the outermost `finish_node` pops the stack.
    root: Option<Rc<GreenNode>>,
}

impl GreenNodeBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new interior node; subsequent tokens/nodes become its children
    /// until the matching [`GreenNodeBuilder::finish_node`].
    pub fn start_node(&mut self, kind: SyntaxKind) {
        self.stack.push((kind, Vec::new()));
    }

    /// Add a leaf to the node currently being built.
    ///
    /// Panics if called before any `start_node` — every token must live inside
    /// a node.
    pub fn token(&mut self, token: GreenToken) {
        let (_, children) = self
            .stack
            .last_mut()
            .expect("GreenNodeBuilder::token called with no open node");
        children.push(GreenChild::Token(Rc::new(token)));
    }

    /// Finish the current node, attaching it to its parent (or, for the
    /// outermost node, recording it as the root).
    ///
    /// Panics if there is no open node.
    pub fn finish_node(&mut self) {
        let (kind, children) = self
            .stack
            .pop()
            .expect("GreenNodeBuilder::finish_node called with no open node");
        let text_len = children.iter().map(GreenChild::text_len).sum();
        let node = Rc::new(GreenNode { kind, children, text_len });
        match self.stack.last_mut() {
            Some((_, parent)) => parent.push(GreenChild::Node(node)),
            None => self.root = Some(node),
        }
    }

    /// Consume the builder and return the finished root node.
    ///
    /// Panics if the tree is unbalanced (a `start_node` without its
    /// `finish_node`, or nothing built at all).
    pub fn finish(mut self) -> Rc<GreenNode> {
        assert!(
            self.stack.is_empty(),
            "GreenNodeBuilder::finish with {} unfinished node(s)",
            self.stack.len()
        );
        self.root.take().expect("GreenNodeBuilder::finish with no root node")
    }
}

/// A cursor into a green tree that pairs a node with its absolute char offset —
/// the *red* layer. Cheap to clone (an `Rc` bump plus a `u32`); child cursors
/// are computed on demand, so no red tree is materialized up front.
#[derive(Debug, Clone)]
pub struct SyntaxNode {
    green: Rc<GreenNode>,
    offset: u32,
}

/// A red-layer element: a child cursor is either a subtree or a token, both
/// carrying an absolute offset.
#[derive(Debug, Clone)]
pub enum SyntaxElement {
    Node(SyntaxNode),
    Token(SyntaxToken),
}

/// A cursor to a token leaf with its absolute char offset.
#[derive(Debug, Clone)]
pub struct SyntaxToken {
    green: Rc<GreenToken>,
    offset: u32,
}

impl SyntaxNode {
    /// A cursor to `green` as a root (offset 0).
    pub fn new_root(green: Rc<GreenNode>) -> Self {
        Self { green, offset: 0 }
    }

    pub fn kind(&self) -> SyntaxKind {
        self.green.kind()
    }

    /// Absolute char offset of this node's start in the source.
    pub fn offset(&self) -> u32 {
        self.offset
    }

    /// Length of this node in characters.
    pub fn text_len(&self) -> u32 {
        self.green.text_len()
    }

    /// The exact source text this node covers.
    pub fn text(&self) -> String {
        self.green.text()
    }

    /// The underlying green node.
    pub fn green(&self) -> &Rc<GreenNode> {
        &self.green
    }

    /// Direct children as red cursors, each with its absolute offset computed
    /// from the running length of its preceding siblings.
    pub fn children(&self) -> Vec<SyntaxElement> {
        let mut out = Vec::with_capacity(self.green.children().len());
        let mut off = self.offset;
        for child in self.green.children() {
            match child {
                GreenChild::Node(n) => {
                    out.push(SyntaxElement::Node(SyntaxNode { green: Rc::clone(n), offset: off }));
                }
                GreenChild::Token(t) => {
                    out.push(SyntaxElement::Token(SyntaxToken { green: Rc::clone(t), offset: off }));
                }
            }
            off += child.text_len();
        }
        out
    }
}

impl SyntaxToken {
    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn text(&self) -> &str {
        self.green.text()
    }

    pub fn text_len(&self) -> u32 {
        self.green.text_len()
    }

    pub fn is_trivia(&self) -> bool {
        self.green.is_trivia()
    }

    /// The underlying green leaf.
    pub fn green(&self) -> &Rc<GreenToken> {
        &self.green
    }
}

/// Build a flat, lossless green tree from `source`: a single [`SyntaxKind::Root`]
/// node whose children are every trivia run and significant token, in source
/// order. The tree has no grammar structure yet — that arrives when the parser
/// drives [`GreenNodeBuilder`] — but it round-trips exactly:
/// `build_lossless(src).text() == src`.
///
/// Returns the lexer's error if `source` does not tokenize.
pub fn build_lossless(source: &str) -> Result<Rc<GreenNode>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;

    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut builder = GreenNodeBuilder::new();
    builder.start_node(SyntaxKind::Root);

    // Walk the tokens with a monotonic cursor, mirroring `trivia::reconstruct`:
    // each token's leading trivia (the gap before it) becomes trivia leaves, and
    // the token's own clamped text becomes a token leaf. Because Step-2 spans
    // tile the source, this covers every character exactly once.
    let mut cursor = 0usize;
    for (i, span) in lexer.token_spans.iter().enumerate() {
        for tr in &lexer.token_leading_trivia[i] {
            emit_trivia(&mut builder, tr);
        }
        let start = (span.start.offset as usize).min(len);
        let end = (span.end.offset as usize).min(len);
        cursor = cursor.max(start);
        let text: String = if end > cursor {
            let t = chars[cursor..end].iter().collect();
            cursor = end;
            t
        } else {
            // Zero-width token (Eof, empty interpolation parts): still emit a
            // leaf so the tree has one token per lexer token.
            String::new()
        };
        builder.token(GreenToken::Token { token: lexer.tokens[i].clone(), text });
    }
    // Any characters past the final token (only if Eof is somehow absent).
    if cursor < len {
        let text: String = chars[cursor..].iter().collect();
        builder.token(GreenToken::Trivia { kind: TriviaKind::Whitespace, text });
    }

    builder.finish_node();
    Ok(builder.finish())
}

fn emit_trivia(builder: &mut GreenNodeBuilder, tr: &Trivia) {
    builder.token(GreenToken::Trivia { kind: tr.kind.clone(), text: tr.text.clone() });
}

#[cfg(test)]
mod tests {
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
            let slice: String = src_chars[off as usize..(off + len) as usize].iter().collect();
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
                if let GreenToken::Trivia { kind: TriviaKind::LineComment, text } = &**t {
                    assert_eq!(text, "// c");
                    saw_comment = true;
                }
            }
        }
        assert_eq!(reassembled, "x = 1 // c\n");
        assert!(saw_comment, "the line comment should be a trivia leaf");
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

    /// The definitive proof: the CST round-trips every real Petal program in the
    /// repo, exactly as `trivia::reconstruct` does — the tree loses nothing the
    /// flat token stream preserved.
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

        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            let Ok(root) = build_lossless(&src) else { continue };
            assert_eq!(root.text(), src, "CST round-trip mismatch for {}", path.display());
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }
}
