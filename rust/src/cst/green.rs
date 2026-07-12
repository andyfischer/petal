//! The green tree: immutable, position-independent nodes and leaves, plus the
//! [`GreenNodeBuilder`] that assembles one from a stream of parser events.

use std::rc::Rc;

use crate::lexer::Token;
use crate::trivia::TriviaKind;

use super::SyntaxKind;

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
    pub(super) fn text_len(&self) -> u32 {
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
        Rc::new(GreenNode {
            kind,
            children,
            text_len,
        })
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
        self.root
            .take()
            .expect("GreenNodeBuilder::finish with no root node")
    }
}
