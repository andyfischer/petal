//! Folding ranges: the blocks of a file an editor can collapse.
//!
//! A block is closed by an `end` or by its indentation alone
//! (docs/syntax/indentation.md), so pairing keywords with `end`s finds only
//! some of them. The ranges here come from the parse instead: one per block
//! construct that runs over several lines, whichever way it closes.
//!
//! A fold starts on the line of the keyword that opens the block and covers
//! its body. A closing `end` on a line of its own, and the `else` that
//! starts the next branch, stay visible: an `if` chain is one fold per
//! branch, and an `end`-ful block folds to the line above its `end`, so a
//! collapsed `fn f() …` / `end` reads as it does expanded. A block with no
//! `end` folds to its last line.

use std::rc::Rc;

use crate::cst::{GreenNode, SyntaxElement, SyntaxKind, SyntaxNode};
use crate::lexer::Token;
use crate::lsp::lsp_types::FoldingRange;

/// The folding ranges of a parsed file, in source order of their start.
pub fn folding_ranges(tree: &Rc<GreenNode>, source: &str) -> Vec<FoldingRange> {
    let mut line_starts = vec![0u32];
    for (i, c) in source.chars().enumerate() {
        if c == '\n' {
            line_starts.push(i as u32 + 1);
        }
    }
    let mut folder = Folder {
        line_starts,
        out: Vec::new(),
    };
    folder.walk(&SyntaxNode::new_root(Rc::clone(tree)));
    folder.out.sort_by_key(|r| (r.start_line, r.end_line));
    folder.out.dedup();
    folder.out
}

struct Folder {
    /// Char offset at which each line starts.
    line_starts: Vec<u32>,
    out: Vec<FoldingRange>,
}

/// The first and the last significant token of a node, by char offset: where
/// it starts, and where its last token starts.
fn extent(node: &SyntaxNode) -> Option<(u32, u32)> {
    let first = crate::rewrite::edge_significant_token(node, false)?;
    let last = last_code_token(node)?;
    Some((first.offset(), last))
}

/// The offset of the last token under `node` that is not a line break.
fn last_code_token(node: &SyntaxNode) -> Option<u32> {
    node.children().into_iter().rev().find_map(|el| match el {
        SyntaxElement::Node(n) => last_code_token(&n),
        SyntaxElement::Token(t) => match t.token() {
            Some(Token::Newline | Token::Eof) | None => None,
            Some(_) => Some(t.offset()),
        },
    })
}

impl Folder {
    fn line_of(&self, offset: u32) -> u32 {
        self.line_starts.partition_point(|&s| s <= offset) as u32 - 1
    }

    fn fold(&mut self, start_line: u32, end_line: u32) {
        if end_line > start_line {
            self.out.push(FoldingRange { start_line, end_line });
        }
    }

    /// The line of the `end` that closes `node`, when it has one of its own
    /// (a direct child) and nothing else of the node shares that line.
    fn own_end_line(&self, node: &SyntaxNode) -> Option<u32> {
        let children = node.children();
        let at = children.iter().rposition(|el| {
            matches!(el, SyntaxElement::Token(t) if matches!(t.token(), Some(Token::End)))
        })?;
        let SyntaxElement::Token(end) = &children[at] else { return None };
        let line = self.line_of(end.offset());
        // What comes before the `end` in the node must end on an earlier line.
        let before = children[..at].iter().rev().find_map(|el| match el {
            SyntaxElement::Node(n) => last_code_token(n),
            SyntaxElement::Token(t) => match t.token() {
                Some(Token::Newline | Token::Eof) | None => None,
                Some(_) => Some(t.offset()),
            },
        })?;
        (self.line_of(before) < line).then_some(line)
    }

    fn walk(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::IfExpr => self.fold_if(node),
            SyntaxKind::FnDecl
            | SyntaxKind::TestDecl
            | SyntaxKind::EnumDecl
            | SyntaxKind::ClassDecl
            | SyntaxKind::ForStmt
            | SyntaxKind::ForExpr
            | SyntaxKind::WhileStmt
            | SyntaxKind::MatchExpr
            | SyntaxKind::MatchArm
            | SyntaxKind::LambdaExpr => {
                if let Some((first, last)) = extent(node) {
                    let start = self.line_of(first);
                    let end = match self.own_end_line(node) {
                        Some(end) => end - 1,
                        None => self.line_of(last),
                    };
                    self.fold(start, end);
                }
            }
            // A literal written over several lines folds between its
            // brackets, the closing one left showing.
            SyntaxKind::ListExpr | SyntaxKind::RecordExpr => {
                if let Some((first, last)) = extent(node) {
                    let (start, end) = (self.line_of(first), self.line_of(last));
                    self.fold(start, end.saturating_sub(1));
                }
            }
            _ => {}
        }
        for el in node.children() {
            if let SyntaxElement::Node(child) = el {
                self.walk(&child);
            }
        }
    }

    /// An `if` chain: one fold per branch, each from the line of its keyword
    /// to the line above the next branch's (or above the chain's `end`).
    fn fold_if(&mut self, node: &SyntaxNode) {
        let Some((first, last)) = extent(node) else { return };
        // The lines the branches start on, and the node that holds the
        // chain's `end` (the last branch, or the `if` itself).
        let mut starts = vec![self.line_of(first)];
        let mut tail = node.clone();
        loop {
            let next = tail.children().into_iter().find_map(|el| match el {
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::ElseBranch => Some(n),
                _ => None,
            });
            let Some(branch) = next else { break };
            if let Some((at, _)) = extent(&branch) {
                starts.push(self.line_of(at));
            }
            tail = branch;
        }
        let close = match self.own_end_line(&tail) {
            Some(end) => end,
            None => self.line_of(last) + 1,
        };
        starts.push(close);
        for pair in starts.windows(2) {
            self.fold(pair[0], pair[1].saturating_sub(1));
        }
    }
}
