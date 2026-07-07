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

use crate::ast::Stmt;
use crate::lexer::{Lexer, Token};
use crate::source_map::{FileId, SourceSpan, ENTRY_FILE};
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
    /// Build a node from `kind` and `children`, computing the cached
    /// `text_len`. This is the tree-editing entry point: an edit constructs
    /// replacement nodes with this and rebuilds the spine above them with
    /// [`GreenNode::replace_child`].
    pub fn with_children(kind: SyntaxKind, children: Vec<GreenChild>) -> Rc<GreenNode> {
        let text_len = children.iter().map(GreenChild::text_len).sum();
        Rc::new(GreenNode { kind, children, text_len })
    }

    /// A copy of this node with the child at `index` swapped for `child`.
    /// Green nodes are immutable and `Rc`-shared, so an edit rebuilds only the
    /// parent chain above the change — O(depth) — while every untouched
    /// subtree stays shared with the original tree.
    pub fn replace_child(&self, index: usize, child: GreenChild) -> Rc<GreenNode> {
        let mut children = self.children.clone();
        children[index] = child;
        Self::with_children(self.kind, children)
    }

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
/// b.finish_node();
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
        let node = GreenNode::with_children(kind, children);
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

    /// The significant lexer token this leaf carries, or `None` for trivia.
    pub fn token(&self) -> Option<&Token> {
        match self.green.as_ref() {
            GreenToken::Token { token, .. } => Some(token),
            GreenToken::Trivia { .. } => None,
        }
    }

    /// The underlying green leaf.
    pub fn green(&self) -> &Rc<GreenToken> {
        &self.green
    }
}

/// A parse event: the abstract shape of the tree, emitted by the parser as it
/// recognizes constructs and later materialized by [`build_tree`]. Keeping
/// events separate from tree construction buys two things the parser needs:
/// checkpoints that retroactively *wrap* already-emitted nodes (for
/// left-associative operators — see [`EventBuilder::wrap`]), and a single place
/// to decide how trivia attaches to the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Open an interior node of this kind. Children follow until the matching
    /// [`Event::Close`].
    Open(SyntaxKind),
    /// Close the most recently opened node.
    Close,
    /// Consume the next significant token from the token stream (trivia is
    /// attached automatically by [`build_tree`]).
    Token,
}

/// A saved position in an [`EventBuilder`]'s event list, used to wrap the nodes
/// emitted after it. Only valid for the builder that produced it, and only
/// until an earlier `wrap` shifts it (the standard recursive-descent usage —
/// each parse function wraps at its own checkpoint after its callees have
/// returned — never does this).
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint(usize);

/// Records the parser's [`Event`] stream. Interior nodes are opened with
/// [`EventBuilder::open`] and closed with [`EventBuilder::close`]; a token is
/// marked consumed with [`EventBuilder::token`]. For left-recursive constructs,
/// take a [`EventBuilder::checkpoint`] *before* parsing the left operand and
/// call [`EventBuilder::wrap`] *after* the right operand to enclose the whole
/// range in a node — nesting outward on each loop iteration, which yields
/// left-associative trees.
#[derive(Debug, Default, Clone)]
pub struct EventBuilder {
    events: Vec<Event>,
}

impl EventBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A checkpoint at the current position, for a later [`EventBuilder::wrap`].
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.events.len())
    }

    /// Open a node; subsequent events are its children until [`Self::close`].
    pub fn open(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Open(kind));
    }

    /// Close the most recently opened node.
    pub fn close(&mut self) {
        self.events.push(Event::Close);
    }

    /// Mark the next significant token as consumed.
    pub fn token(&mut self) {
        self.events.push(Event::Token);
    }

    /// Retroactively wrap everything emitted since `cp` in a node of `kind`.
    /// Repeated `wrap`s at the same checkpoint nest outward, so a `a + b + c`
    /// loop builds the left-associative `((a + b) + c)`.
    pub fn wrap(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.events.insert(cp.0, Event::Open(kind));
        self.events.push(Event::Close);
    }

    /// The recorded events.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Consume the builder, returning the recorded events.
    pub fn into_events(self) -> Vec<Event> {
        self.events
    }
}

/// Materialize a structured green tree from a parser's [`Event`] stream and the
/// lexer output. Every `Event::Token` consumes the next significant token (and
/// first emits that token's leading trivia as leaves); `Open`/`Close` frame
/// interior nodes. The whole thing is wrapped in a [`SyntaxKind::Root`], and any
/// tokens the parser did not consume (the trailing `Eof` and its leading
/// trivia) are flushed as final `Root` children — so the tree always covers the
/// entire source and round-trips exactly.
///
/// `tokens`, `spans`, and `leading_trivia` are the parallel arrays the lexer
/// produces (see [`crate::lexer::Lexer`]); `source` is the original text.
pub fn build_tree(
    events: &[Event],
    tokens: &[Token],
    spans: &[SourceSpan],
    leading_trivia: &[Vec<Trivia>],
    source: &str,
) -> Rc<GreenNode> {
    let chars: Vec<char> = source.chars().collect();
    let mut builder = GreenNodeBuilder::new();
    builder.start_node(SyntaxKind::Root);

    // A monotonic cursor over the source, mirroring `trivia::reconstruct`: each
    // token contributes its leading-trivia gap plus its own clamped text. Step-2
    // tiling spans mean this covers every character exactly once.
    let mut cursor = 0usize;
    let mut k = 0usize;
    for event in events {
        match event {
            Event::Open(kind) => builder.start_node(*kind),
            Event::Close => builder.finish_node(),
            Event::Token => {
                emit_token(&mut builder, &mut k, &mut cursor, &chars, tokens, spans, leading_trivia);
            }
        }
    }
    // Flush tokens the parser never consumed (trailing Eof + its trivia) into
    // the root, so nothing is lost even though no event references them.
    while k < tokens.len() {
        emit_token(&mut builder, &mut k, &mut cursor, &chars, tokens, spans, leading_trivia);
    }
    // Defensive: any characters past the final token (only reachable if the
    // stream somehow lacks an Eof whose trivia covers the tail).
    if cursor < chars.len() {
        let text: String = chars[cursor..].iter().collect();
        builder.token(GreenToken::Trivia { kind: TriviaKind::Whitespace, text });
    }

    builder.finish_node(); // Root
    builder.finish()
}

/// Emit token `*k`'s leading trivia and its own clamped source text, advancing
/// `*k` and the monotonic `*cursor`.
#[allow(clippy::too_many_arguments)]
fn emit_token(
    builder: &mut GreenNodeBuilder,
    k: &mut usize,
    cursor: &mut usize,
    chars: &[char],
    tokens: &[Token],
    spans: &[SourceSpan],
    leading_trivia: &[Vec<Trivia>],
) {
    for tr in &leading_trivia[*k] {
        builder.token(GreenToken::Trivia { kind: tr.kind.clone(), text: tr.text.clone() });
    }
    let span = spans[*k];
    let len = chars.len();
    let start = (span.start.offset as usize).min(len);
    let end = (span.end.offset as usize).min(len);
    *cursor = (*cursor).max(start);
    let text: String = if end > *cursor {
        let t = chars[*cursor..end].iter().collect();
        *cursor = end;
        t
    } else {
        // Zero-width token (Eof, empty interpolation parts): still emit a leaf so
        // the tree has one token leaf per lexer token.
        String::new()
    };
    builder.token(GreenToken::Token { token: tokens[*k].clone(), text });
    *k += 1;
}

/// Build a flat, lossless green tree from `source`: a single [`SyntaxKind::Root`]
/// node whose children are every trivia run and significant token, in source
/// order. This is [`build_tree`] with an empty event stream — no grammar
/// structure yet, that arrives when the parser drives [`EventBuilder`] — and it
/// round-trips exactly: `build_lossless(src).text() == src`.
///
/// Returns the lexer's error if `source` does not tokenize.
pub fn build_lossless(source: &str) -> Result<Rc<GreenNode>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    Ok(build_tree(
        &[],
        &lexer.tokens,
        &lexer.token_spans,
        &lexer.token_leading_trivia,
        source,
    ))
}

/// Parse `source` into a *structured* lossless green tree: the parser runs as
/// normal (the AST it builds is discarded) while recording a CST [`Event`]
/// stream, which [`build_tree`] materializes with trivia interleaved. The
/// result has grammar nodes ([`SyntaxKind::LetStmt`], [`SyntaxKind::CallExpr`],
/// …) and still round-trips exactly: `parse_cst(src)?.text() == src`.
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
    let mut parser =
        crate::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
    let direct = parser.parse_program()?;
    let green = build_tree(
        parser.cst_events(),
        &lexer.tokens,
        &lexer.token_spans,
        &lexer.token_leading_trivia,
        source,
    );
    let projected = crate::cst_project::project_in_file(&SyntaxNode::new_root(green.clone()), file)?;
    // The corpus tests prove projection ≡ direct parse; this catches drift on
    // inputs the corpus lacks.
    debug_assert_eq!(
        format!("{direct:#?}"),
        format!("{projected:#?}"),
        "CST projection diverged from the parser's direct AST"
    );
    Ok((green, projected))
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
        assert_eq!(tree.text(), src, "unconsumed tokens must be flushed losslessly");
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
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            let Ok(root) = build_lossless(&src) else { continue };
            assert_eq!(root.text(), src, "CST round-trip mismatch for {}", path.display());
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
            let Ok(src) = std::fs::read_to_string(path) else { continue };
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
        assert_eq!(root.text(), src, "structured round-trip mismatch for {src:?}");
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
        assert!(find_node(&fn_decl, SyntaxKind::Block).is_some(), "fn body Block");
        assert!(find_node(&fn_decl, SyntaxKind::ReturnStmt).is_some(), "ReturnStmt");
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
