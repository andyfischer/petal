//! Where the blocks of a parsed file end.
//!
//! A block may be closed by an `end` or by its indentation alone
//! (docs/syntax/indentation.md). The token stream shows the first kind; only
//! the parse knows the second. [`Blocks`] is what the indentation pass needs
//! from the parse: for each construct with no `end` of its own, the last
//! token it spans.
//!
//! Tokens are named by *ordinal*: their index among the tokens that are not
//! line breaks. The passes move whitespace and never tokens, so an ordinal
//! taken from the source as written still names the same token in the text a
//! pass produced, which an offset would not.

use std::collections::HashMap;
use std::rc::Rc;

use crate::cst::{GreenChild, GreenNode, GreenToken, SyntaxKind};
use crate::lexer::Token;

/// The constructs of a file that no `end` closes.
#[derive(Default, Debug, Clone)]
pub struct Blocks {
    /// Ordinal of the token that opens the construct (`if`, `for`, `while`,
    /// `match`, `enum`, `class`, `fn`, or the `do` of a `test` or an arm) →
    /// ordinal of the last token the construct spans.
    closed_by_layout: HashMap<usize, usize>,
}

impl Blocks {
    /// Every construct is taken to have its `end`: the reading of source
    /// that could not be parsed.
    pub fn all_closed_by_end() -> Self {
        Self::default()
    }

    /// The blocks of a parsed file, from its green tree.
    pub fn of(tree: &Rc<GreenNode>) -> Self {
        let mut blocks = Blocks::default();
        let mut next = 0usize;
        blocks.walk(tree, &mut next);
        blocks
    }

    /// The last token of the construct opened at `opener`, when no `end`
    /// closes it.
    pub fn closed_by_layout(&self, opener: usize) -> Option<usize> {
        self.closed_by_layout.get(&opener).copied()
    }

    /// Is any construct of the file closed by its indentation alone?
    pub fn any_closed_by_layout(&self) -> bool {
        !self.closed_by_layout.is_empty()
    }

    /// Visit `node`, `next` being the ordinal of the next token. Returns
    /// whether an `end` closes `node` (for the `else` branches of an `if`,
    /// whose last one holds the chain's `end`).
    fn walk(&mut self, node: &GreenNode, next: &mut usize) -> bool {
        let kind = node.kind();
        let mut opener = None;
        let mut has_end = false;
        for child in node.children() {
            match child {
                GreenChild::Token(t) => {
                    let GreenToken::Token { token, .. } = &**t else { continue };
                    if matches!(token, Token::Newline) {
                        continue;
                    }
                    if matches!(token, Token::End) {
                        has_end = true;
                    }
                    if opener.is_none() && opens(kind, token) {
                        opener = Some(*next);
                    }
                    *next += 1;
                }
                GreenChild::Node(n) => {
                    let ended = self.walk(n, next);
                    if n.kind() == SyntaxKind::ElseBranch {
                        has_end |= ended;
                    }
                }
            }
        }
        if let Some(opener) = opener
            && !has_end
            && *next > 0
        {
            self.closed_by_layout.insert(opener, *next - 1);
        }
        has_end
    }
}

/// Is `token`, a direct child of a `kind` node, the one the indentation pass
/// opens that construct at?
fn opens(kind: SyntaxKind, token: &Token) -> bool {
    match kind {
        SyntaxKind::IfExpr => matches!(token, Token::If),
        SyntaxKind::ForStmt | SyntaxKind::ForExpr => matches!(token, Token::For),
        SyntaxKind::WhileStmt => matches!(token, Token::While),
        SyntaxKind::MatchExpr => matches!(token, Token::Match),
        SyntaxKind::EnumDecl => matches!(token, Token::Enum),
        SyntaxKind::ClassDecl => {
            matches!(token, Token::Ident(w) if w == crate::parse::CLASS_KEYWORD)
        }
        SyntaxKind::FnDecl | SyntaxKind::LambdaExpr => matches!(token, Token::Fn),
        SyntaxKind::TestDecl | SyntaxKind::MatchArm => matches!(token, Token::Do),
        _ => false,
    }
}

/// Per token of a lexed file: its ordinal (see the module docs). A line
/// break takes the ordinal of the token after it.
pub(super) fn ordinals(tokens: &[Token]) -> Vec<usize> {
    let mut next = 0usize;
    tokens
        .iter()
        .map(|t| {
            let ord = next;
            if !matches!(t, Token::Newline) {
                next += 1;
            }
            ord
        })
        .collect()
}
