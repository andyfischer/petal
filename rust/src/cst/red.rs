//! The red tree: lightweight cursors that pair a green node with its absolute
//! char offset, computing child cursors on demand.

use std::rc::Rc;

use crate::lexer::Token;

use super::green::{GreenChild, GreenNode, GreenToken};
use super::SyntaxKind;

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
