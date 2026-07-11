//! Project the typed AST out of the lossless CST.
//!
//! [`project`] walks a structured green tree produced by
//! [`crate::cst::parse_cst`] and rebuilds the exact [`crate::ast`] values the
//! parser builds directly — same shapes, same [`SourceSpan`]s. A differential
//! test proves the two agree over the entire repo `.ptl` corpus. This
//! establishes that the CST carries everything the AST needs, which is what
//! lets a later step (3d) make `parse_cst` the sole parser and derive the AST
//! from the tree instead of building both in parallel.
//!
//! The walk mirrors `parse.rs` construct-for-construct, including its span
//! conventions:
//!
//! - Most constructs span "first token .. last consumed token"; in tree terms,
//!   the node's first .. last significant token leaf.
//! - Binary and postfix expressions start at their *left operand's expression
//!   span*, which excludes grouping parens the CST keeps (`(a + b).x` spans
//!   from `a`, not `(`).
//! - A pipe call `a |> f` spans from the `|>` token, matching the parser's
//!   rewrite of pipe syntax into a call.
//! - Compound assignment `x += e` desugars into `x = x + e` with the value
//!   expression spanning the whole statement.
//!
//! Line/column positions are 1-based and reset on `'\n'`, exactly like the
//! lexer's tracking, so they are recomputed here from a line index over the
//! tree's own text.

use crate::ast::*;
use crate::cst::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
use crate::lexer::Token;
use crate::parse::{expr_to_assign_target, parse_color_hex};
use crate::source_map::{FileId, SourcePosition, SourceSpan, ENTRY_FILE};

/// Project the statements of a whole-file `Root` node, with spans tagged as
/// the entry file (matching a tree built by [`crate::cst::parse_cst`]).
pub fn project(root: &SyntaxNode) -> Result<Vec<Stmt>, String> {
    project_in_file(root, ENTRY_FILE)
}

/// Like [`project`], but tags every span with `file` (for trees of imported
/// modules, whose positions stay file-local).
pub fn project_in_file(root: &SyntaxNode, file: FileId) -> Result<Vec<Stmt>, String> {
    let text = root.text();
    let mut line_starts = vec![0u32];
    for (i, ch) in text.chars().enumerate() {
        if ch == '\n' {
            line_starts.push(i as u32 + 1);
        }
    }
    let mut p = Projector { line_starts, file, next_state_id: 0 };
    child_nodes(root).iter().map(|n| p.stmt(n)).collect()
}

struct Projector {
    /// Char offset of each line start, for offset → line/column conversion.
    line_starts: Vec<u32>,
    file: FileId,
    /// Mirrors the parser's running `state` id counter. Ids are allocated in
    /// parse order — notably *after* a state's init expression is parsed, so a
    /// state nested inside the init (via a lambda body) gets the lower id.
    next_state_id: usize,
}

// ---- Red-tree access helpers ----

fn child_nodes(node: &SyntaxNode) -> Vec<SyntaxNode> {
    node.children()
        .into_iter()
        .filter_map(|el| match el {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(_) => None,
        })
        .collect()
}

/// Direct significant (non-trivia) token children, in source order. Includes
/// separators like `Newline`/`Comma`; callers match on token kind.
fn direct_tokens(node: &SyntaxNode) -> Vec<SyntaxToken> {
    node.children()
        .into_iter()
        .filter_map(|el| match el {
            SyntaxElement::Token(t) if !t.is_trivia() => Some(t),
            _ => None,
        })
        .collect()
}

/// First significant token leaf in the subtree (the construct's first token).
fn first_token_deep(node: &SyntaxNode) -> Option<SyntaxToken> {
    for el in node.children() {
        match el {
            SyntaxElement::Token(t) if !t.is_trivia() => return Some(t),
            SyntaxElement::Node(n) => {
                if let Some(t) = first_token_deep(&n) {
                    return Some(t);
                }
            }
            SyntaxElement::Token(_) => {}
        }
    }
    None
}

/// Last significant token leaf in the subtree (the construct's last consumed
/// token — every parser span ends here).
fn last_token_deep(node: &SyntaxNode) -> Option<SyntaxToken> {
    for el in node.children().into_iter().rev() {
        match el {
            SyntaxElement::Token(t) if !t.is_trivia() => return Some(t),
            SyntaxElement::Node(n) => {
                if let Some(t) = last_token_deep(&n) {
                    return Some(t);
                }
            }
            SyntaxElement::Token(_) => {}
        }
    }
    None
}

fn ident_value(t: &SyntaxToken) -> Option<String> {
    match t.token()? {
        Token::Ident(name) => Some(name.clone()),
        _ => None,
    }
}

impl Projector {
    // ---- Span reconstruction ----

    fn pos_at(&self, offset: u32) -> SourcePosition {
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        SourcePosition {
            line: (line_idx + 1) as u32,
            column: offset - self.line_starts[line_idx] + 1,
            offset,
        }
    }

    fn span_between(&self, start_offset: u32, end_offset: u32) -> SourceSpan {
        SourceSpan {
            start: self.pos_at(start_offset),
            end: self.pos_at(end_offset),
            file: self.file,
        }
    }

    fn token_span(&self, t: &SyntaxToken) -> SourceSpan {
        self.span_between(t.offset(), t.offset() + t.text_len())
    }

    /// The parser's `span_from(first token of the construct)`: first .. last
    /// significant token leaf of the node.
    fn node_span(&self, node: &SyntaxNode) -> Result<SourceSpan, String> {
        let first = first_token_deep(node)
            .ok_or_else(|| format!("CST {:?} node has no tokens", node.kind()))?;
        let last = last_token_deep(node).expect("node with a first token has a last");
        Ok(self.span_between(first.offset(), last.offset() + last.text_len()))
    }

    /// End position of the node's last significant token — where the parser's
    /// `prev_span().end` pointed when it built the construct.
    fn end_of(&self, node: &SyntaxNode) -> Result<SourcePosition, String> {
        let last = last_token_deep(node)
            .ok_or_else(|| format!("CST {:?} node has no tokens", node.kind()))?;
        Ok(self.pos_at(last.offset() + last.text_len()))
    }

    // ---- Statements ----

    fn stmt(&mut self, node: &SyntaxNode) -> Result<Stmt, String> {
        let span = self.node_span(node)?;
        let kind = match node.kind() {
            SyntaxKind::LetStmt => {
                let name = self.only_ident(node)?;
                let value = self.only_expr(node)?;
                StmtKind::Let { name, value }
            }
            SyntaxKind::AssignStmt => return self.assign_stmt(node, span),
            SyntaxKind::ExprStmt => StmtKind::Expr(self.only_expr(node)?),
            SyntaxKind::FnDecl => {
                let name = self.only_ident(node)?;
                let params = self.param_list(node)?;
                let body = self.block(node)?;
                StmtKind::FnDecl { name, params, body }
            }
            SyntaxKind::EnumDecl => self.enum_decl(node)?,
            SyntaxKind::ForStmt => {
                let var = self.only_ident(node)?;
                let nodes = child_nodes(node);
                let iter = self.expr(nodes.first().ok_or("for statement missing iterable")?)?;
                let body = self.block(node)?;
                StmtKind::For { var, iter, body }
            }
            SyntaxKind::WhileStmt => {
                let nodes = child_nodes(node);
                let condition =
                    self.expr(nodes.first().ok_or("while statement missing condition")?)?;
                let body = self.block(node)?;
                StmtKind::While { condition, body }
            }
            SyntaxKind::ReturnStmt => {
                let nodes = child_nodes(node);
                StmtKind::Return(match nodes.first() {
                    Some(value) => Some(self.expr(value)?),
                    None => None,
                })
            }
            SyntaxKind::BreakStmt => StmtKind::Break,
            SyntaxKind::ContinueStmt => StmtKind::Continue,
            SyntaxKind::StateStmt => {
                let name = self.only_ident(node)?;
                let nodes = child_nodes(node);
                // Two child nodes means an explicit key: `state(key) name = init`.
                let (key_node, init_node) = match nodes.len() {
                    1 => (None, &nodes[0]),
                    2 => (Some(&nodes[0]), &nodes[1]),
                    n => return Err(format!("state statement with {n} expression nodes")),
                };
                // Parse order: key, then init, then the id — so states nested
                // inside the init (via lambda bodies) take lower ids.
                let key = match key_node {
                    Some(k) => Some(self.expr(k)?),
                    None => None,
                };
                let init = self.expr(init_node)?;
                let id = self.next_state_id;
                self.next_state_id += 1;
                StmtKind::State { name, init, id, key }
            }
            SyntaxKind::ImportStmt => self.import_stmt(node)?,
            other => return Err(format!("expected a statement node, got {other:?}")),
        };
        Ok(Stmt { kind, span })
    }

    fn assign_stmt(&mut self, node: &SyntaxNode, span: SourceSpan) -> Result<Stmt, String> {
        let nodes = child_nodes(node);
        let [target_node, value_node] = nodes.as_slice() else {
            return Err(format!("assignment with {} expression nodes", nodes.len()));
        };
        let target_expr = self.expr(target_node)?;
        let rhs = self.expr(value_node)?;
        let compound_op = direct_tokens(node).iter().find_map(|t| match t.token() {
            Some(Token::PlusAssign) => Some(BinOp::Add),
            Some(Token::MinusAssign) => Some(BinOp::Sub),
            Some(Token::StarAssign) => Some(BinOp::Mul),
            Some(Token::SlashAssign) => Some(BinOp::Div),
            Some(Token::PercentAssign) => Some(BinOp::Mod),
            _ => None,
        });
        let (target, value) = match compound_op {
            // `target op= rhs` desugars to `target = target op rhs`, the
            // desugared value spanning the whole statement.
            Some(op) => (
                expr_to_assign_target(target_expr.clone())?,
                Expr {
                    kind: ExprKind::BinaryOp {
                        op,
                        left: Box::new(target_expr),
                        right: Box::new(rhs),
                    },
                    span,
                },
            ),
            None => (expr_to_assign_target(target_expr)?, rhs),
        };
        Ok(Stmt { kind: StmtKind::Assign { target, value }, span })
    }

    fn enum_decl(&mut self, node: &SyntaxNode) -> Result<StmtKind, String> {
        let mut name = None;
        let mut variants: Vec<EnumVariant> = Vec::new();
        for el in node.children() {
            match el {
                SyntaxElement::Token(t) => {
                    if let Some(ident) = ident_value(&t) {
                        // First identifier is the enum's name; the rest start variants.
                        if name.is_none() {
                            name = Some(ident);
                        } else {
                            variants.push(EnumVariant { name: ident, fields: Vec::new() });
                        }
                    }
                }
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::ParamList => {
                    let fields = param_names(&n);
                    variants
                        .last_mut()
                        .ok_or("enum field list before any variant")?
                        .fields = fields;
                }
                SyntaxElement::Node(n) => {
                    return Err(format!("unexpected {:?} node in enum declaration", n.kind()))
                }
            }
        }
        Ok(StmtKind::EnumDecl {
            name: name.ok_or("enum declaration missing name")?,
            variants,
        })
    }

    fn import_stmt(&mut self, node: &SyntaxNode) -> Result<StmtKind, String> {
        let tokens = direct_tokens(node);
        let mut idents = tokens.iter().filter_map(|t| ident_value(t).map(|v| (t, v)));
        let module = idents.next().ok_or("import missing module name")?.1;

        let mut alias = None;
        let mut names = None;
        let has_colon = tokens.iter().any(|t| matches!(t.token(), Some(Token::Colon)));
        if has_colon {
            names = Some(idents.map(|(_, v)| v).collect());
        } else if let Some((_, kw)) = idents.next() {
            debug_assert_eq!(kw, "as", "only `as` can follow the module name");
            alias = Some(idents.next().ok_or("import `as` missing alias")?.1);
        }
        Ok(StmtKind::Import(ImportDecl { module, alias, names }))
    }

    /// The statements of the node's `Block` child (fn/for/while bodies, etc.).
    fn block(&mut self, parent: &SyntaxNode) -> Result<Vec<Stmt>, String> {
        let block = child_nodes(parent)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::Block)
            .ok_or_else(|| format!("{:?} missing its Block child", parent.kind()))?;
        self.block_stmts(&block)
    }

    fn block_stmts(&mut self, block: &SyntaxNode) -> Result<Vec<Stmt>, String> {
        child_nodes(block).iter().map(|n| self.stmt(n)).collect()
    }

    fn param_list(&self, parent: &SyntaxNode) -> Result<Vec<String>, String> {
        let params = child_nodes(parent)
            .into_iter()
            .find(|n| n.kind() == SyntaxKind::ParamList)
            .ok_or_else(|| format!("{:?} missing its ParamList child", parent.kind()))?;
        Ok(param_names(&params))
    }

    /// The first direct identifier token — the declared name of a let / fn /
    /// state / for construct (identifiers inside sub-expressions are nested in
    /// their own nodes, never direct children).
    fn only_ident(&self, node: &SyntaxNode) -> Result<String, String> {
        direct_tokens(node)
            .iter()
            .find_map(ident_value)
            .ok_or_else(|| format!("{:?} missing an identifier token", node.kind()))
    }

    /// The node's single expression child.
    fn only_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let nodes = child_nodes(node);
        let [expr] = nodes.as_slice() else {
            return Err(format!(
                "{:?} expected exactly one expression node, got {}",
                node.kind(),
                nodes.len()
            ));
        };
        self.expr(expr)
    }

    // ---- Expressions ----

    fn expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        match node.kind() {
            // The AST drops grouping parens; the projected expression is the
            // inner one, spans and all.
            SyntaxKind::ParenExpr => self.only_expr(node),
            SyntaxKind::LiteralExpr => self.literal_expr(node),
            SyntaxKind::IdentExpr => Ok(Expr {
                kind: ExprKind::Ident(self.only_ident(node)?),
                span: self.node_span(node)?,
            }),
            SyntaxKind::AtVarExpr => Ok(Expr {
                kind: ExprKind::AtVar(self.only_ident(node)?),
                span: self.node_span(node)?,
            }),
            SyntaxKind::UnaryExpr => {
                let op = direct_tokens(node)
                    .iter()
                    .find_map(|t| match t.token() {
                        Some(Token::Minus | Token::MinusPrefix) => Some(UnaryOp::Neg),
                        Some(Token::Bang) => Some(UnaryOp::Not),
                        _ => None,
                    })
                    .ok_or("unary expression missing its operator")?;
                let operand = self.only_expr(node)?;
                Ok(Expr {
                    kind: ExprKind::UnaryOp { op, operand: Box::new(operand) },
                    span: self.node_span(node)?,
                })
            }
            SyntaxKind::BinaryExpr => self.binary_expr(node),
            SyntaxKind::CallExpr => self.call_expr(node),
            SyntaxKind::FieldAccessExpr => {
                let object = self.field_access_object(node)?;
                let field = self.only_ident(node)?;
                let span = SourceSpan {
                    start: object.span.start,
                    end: self.end_of(node)?,
                    file: object.span.file,
                };
                Ok(Expr {
                    kind: ExprKind::FieldAccess { object: Box::new(object), field },
                    span,
                })
            }
            SyntaxKind::IndexAccessExpr => {
                let nodes = child_nodes(node);
                let [object_node, index_node] = nodes.as_slice() else {
                    return Err(format!("index access with {} expression nodes", nodes.len()));
                };
                let object = self.expr(object_node)?;
                let index = self.expr(index_node)?;
                let span = SourceSpan {
                    start: object.span.start,
                    end: self.end_of(node)?,
                    file: object.span.file,
                };
                Ok(Expr {
                    kind: ExprKind::IndexAccess {
                        object: Box::new(object),
                        index: Box::new(index),
                    },
                    span,
                })
            }
            SyntaxKind::ListExpr => {
                let elements = child_nodes(node)
                    .iter()
                    .map(|n| self.expr(n))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr { kind: ExprKind::List(elements), span: self.node_span(node)? })
            }
            SyntaxKind::RecordExpr => {
                let fields = child_nodes(node)
                    .iter()
                    .map(|n| self.record_field(n))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr { kind: ExprKind::Record(fields), span: self.node_span(node)? })
            }
            SyntaxKind::IfExpr => self.if_expr(node),
            SyntaxKind::MatchExpr => {
                let nodes = child_nodes(node);
                let subject = self.expr(nodes.first().ok_or("match missing its subject")?)?;
                let arms = nodes
                    .iter()
                    .filter(|n| n.kind() == SyntaxKind::MatchArm)
                    .map(|n| self.match_arm(n))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr {
                    kind: ExprKind::Match { subject: Box::new(subject), arms },
                    span: self.node_span(node)?,
                })
            }
            SyntaxKind::LambdaExpr => self.lambda_expr(node),
            SyntaxKind::StringInterpExpr => {
                let (parts, exprs) = self.interp_parts(node)?;
                Ok(Expr {
                    kind: ExprKind::StringInterp { parts, exprs },
                    span: self.node_span(node)?,
                })
            }
            SyntaxKind::ElementExpr => self.element_expr(node),
            other => Err(format!("expected an expression node, got {other:?}")),
        }
    }

    fn literal_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let span = self.node_span(node)?;
        let tokens = direct_tokens(node);
        let first = tokens.first().and_then(|t| t.token().cloned());
        let kind = match first {
            Some(Token::Int(n)) => ExprKind::Literal(Literal::Int(n)),
            Some(Token::Float(f)) => ExprKind::Literal(Literal::Float(f)),
            Some(Token::String(s)) => ExprKind::Literal(Literal::String(s)),
            Some(Token::True) => ExprKind::Literal(Literal::Bool(true)),
            Some(Token::False) => ExprKind::Literal(Literal::Bool(false)),
            Some(Token::Nil) => ExprKind::Literal(Literal::Nil),
            // A color literal parses into a record of channel values, every
            // span being the color token itself.
            Some(Token::Color(hex)) => ExprKind::Record(
                parse_color_hex(&hex)
                    .into_iter()
                    .map(|(name, value)| {
                        RecordField::Named(name.to_string(), Expr {
                            kind: ExprKind::Literal(Literal::Int(value)),
                            span,
                        })
                    })
                    .collect(),
            ),
            // An interpolated string with no holes collapses to a plain string
            // literal: its single part, or "" when even that is absent.
            Some(Token::InterpStart) => {
                let value = tokens
                    .iter()
                    .find_map(|t| match t.token() {
                        Some(Token::String(s)) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                ExprKind::Literal(Literal::String(value))
            }
            other => return Err(format!("unexpected literal token {other:?}")),
        };
        Ok(Expr { kind, span })
    }

    fn binary_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let nodes = child_nodes(node);
        let [left_node, right_node] = nodes.as_slice() else {
            return Err(format!("binary expression with {} operand nodes", nodes.len()));
        };
        let op = direct_tokens(node)
            .iter()
            .find_map(|t| bin_op(t.token()?))
            .ok_or("binary expression missing its operator")?;
        let left = self.expr(left_node)?;
        let right = self.expr(right_node)?;
        Ok(Expr {
            span: SourceSpan {
                start: left.span.start,
                end: right.span.end,
                file: left.span.file,
            },
            kind: ExprKind::BinaryOp { op, left: Box::new(left), right: Box::new(right) },
        })
    }

    /// A `CallExpr` is either pipe syntax (`a |> f`, marked by a direct `|>`
    /// token) or a postfix call (`f(args)`, marked by an `ArgList` child).
    fn call_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let nodes = child_nodes(node);
        let pipe_tok = direct_tokens(node)
            .into_iter()
            .find(|t| matches!(t.token(), Some(Token::Pipe)));

        if let Some(pipe) = pipe_tok {
            // `left |> rhs` — the parser rewrites into a call whose span runs
            // from the `|>` token to the end of the right-hand side.
            let [left_node, rhs_node] = nodes.as_slice() else {
                return Err(format!("pipe expression with {} operand nodes", nodes.len()));
            };
            let left = self.expr(left_node)?;
            let rhs = self.expr(rhs_node)?;
            let span = SourceSpan {
                start: self.pos_at(pipe.offset()),
                end: self.end_of(node)?,
                file: self.file,
            };
            let kind = match rhs.kind {
                ExprKind::Call { function, mut args } => {
                    args.insert(0, left);
                    ExprKind::Call { function, args }
                }
                _ => ExprKind::Call { function: Box::new(rhs), args: vec![left] },
            };
            return Ok(Expr { kind, span });
        }

        let [callee_node, arg_list] = nodes.as_slice() else {
            return Err(format!("call expression with {} nodes", nodes.len()));
        };
        if arg_list.kind() != SyntaxKind::ArgList {
            return Err(format!("call expression missing ArgList, got {:?}", arg_list.kind()));
        }
        let function = self.expr(callee_node)?;
        let args = child_nodes(arg_list)
            .iter()
            .map(|n| self.expr(n))
            .collect::<Result<Vec<_>, _>>()?;
        let span = SourceSpan {
            start: function.span.start,
            end: self.end_of(node)?,
            file: function.span.file,
        };
        Ok(Expr { kind: ExprKind::Call { function: Box::new(function), args }, span })
    }

    fn field_access_object(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let nodes = child_nodes(node);
        let [object] = nodes.as_slice() else {
            return Err(format!("field access with {} expression nodes", nodes.len()));
        };
        self.expr(object)
    }

    fn record_field(&mut self, node: &SyntaxNode) -> Result<RecordField, String> {
        if node.kind() != SyntaxKind::RecordField {
            return Err(format!("expected RecordField, got {:?}", node.kind()));
        }
        let is_spread = direct_tokens(node)
            .iter()
            .any(|t| matches!(t.token(), Some(Token::DotDotDot)));
        if is_spread {
            Ok(RecordField::Spread(self.only_expr(node)?))
        } else {
            let key = self.only_ident(node)?;
            Ok(RecordField::Named(key, self.only_expr(node)?))
        }
    }

    fn if_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let nodes = child_nodes(node);
        let condition = self.expr(nodes.first().ok_or("if missing its condition")?)?;
        let then_body = self.block(node)?;
        let else_body = match nodes.into_iter().find(|n| n.kind() == SyntaxKind::ElseBranch) {
            Some(branch) => Some(self.else_branch(&branch)?),
            None => None,
        };
        Ok(Expr {
            kind: ExprKind::If {
                condition: Box::new(condition),
                then_body,
                else_body,
            },
            span: self.node_span(node)?,
        })
    }

    /// An `ElseBranch` node is either `else <block> end` or an `elsif` chain,
    /// which the AST represents as a nested if-expression spanning from the
    /// `elsif` keyword through the chain's closing `end`.
    fn else_branch(&mut self, node: &SyntaxNode) -> Result<ElseBranch, String> {
        let is_elsif = matches!(
            first_token_deep(node).and_then(|t| t.token().cloned()),
            Some(Token::Elsif)
        );
        if !is_elsif {
            return Ok(ElseBranch::Block(self.block(node)?));
        }
        let nodes = child_nodes(node);
        let condition = self.expr(nodes.first().ok_or("elsif missing its condition")?)?;
        let then_body = self.block(node)?;
        let else_body = match nodes.into_iter().find(|n| n.kind() == SyntaxKind::ElseBranch) {
            Some(branch) => Some(self.else_branch(&branch)?),
            None => None,
        };
        Ok(ElseBranch::ElseIf(Box::new(Expr {
            kind: ExprKind::If {
                condition: Box::new(condition),
                then_body,
                else_body,
            },
            span: self.node_span(node)?,
        })))
    }

    fn match_arm(&mut self, node: &SyntaxNode) -> Result<MatchArm, String> {
        let nodes = child_nodes(node);
        let (pattern_node, rest) = nodes.split_first().ok_or("match arm missing its pattern")?;
        let pattern = self.pattern(pattern_node)?;

        // A direct `if` token marks a guard, whose expression node directly
        // follows the pattern; the body node is always last.
        let has_guard = direct_tokens(node)
            .iter()
            .any(|t| matches!(t.token(), Some(Token::If)));
        let (guard_node, body_node) = match (has_guard, rest) {
            (true, [guard, body]) => (Some(guard), body),
            (false, [body]) => (None, body),
            _ => return Err(format!("match arm with {} nodes after the pattern", rest.len())),
        };
        let guard = match guard_node {
            Some(g) => Some(self.expr(g)?),
            None => None,
        };

        let body = if body_node.kind() == SyntaxKind::Block {
            // `when pat do … end` — a block expression spanning `do` .. `end`.
            let do_tok = direct_tokens(node)
                .into_iter()
                .find(|t| matches!(t.token(), Some(Token::Do)))
                .ok_or("match arm block body missing `do`")?;
            Expr {
                kind: ExprKind::Block(self.block_stmts(body_node)?),
                span: SourceSpan {
                    start: self.pos_at(do_tok.offset()),
                    end: self.end_of(node)?,
                    file: self.file,
                },
            }
        } else {
            self.expr(body_node)?
        };
        Ok(MatchArm { pattern, guard, body })
    }

    fn pattern(&mut self, node: &SyntaxNode) -> Result<Pattern, String> {
        if node.kind() != SyntaxKind::Pattern {
            return Err(format!("expected Pattern node, got {:?}", node.kind()));
        }
        let tokens = direct_tokens(node);
        let first = tokens.first().and_then(|t| t.token().cloned());
        match first {
            Some(Token::Ident(name)) if name == "_" => Ok(Pattern::Wildcard),
            Some(Token::Ident(name)) => {
                let has_parens = tokens.iter().any(|t| matches!(t.token(), Some(Token::LParen)));
                if has_parens {
                    let fields = child_nodes(node)
                        .iter()
                        .map(|n| self.pattern(n))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Pattern::Variant { name, fields })
                } else {
                    Ok(Pattern::Variable(name))
                }
            }
            Some(Token::Int(n)) => Ok(Pattern::Literal(Literal::Int(n))),
            Some(Token::Float(f)) => Ok(Pattern::Literal(Literal::Float(f))),
            Some(Token::String(s)) => Ok(Pattern::Literal(Literal::String(s))),
            Some(Token::True) => Ok(Pattern::Literal(Literal::Bool(true))),
            Some(Token::False) => Ok(Pattern::Literal(Literal::Bool(false))),
            Some(Token::Nil) => Ok(Pattern::Literal(Literal::Nil)),
            Some(Token::Minus | Token::MinusPrefix) => {
                match tokens.get(1).and_then(|t| t.token().cloned()) {
                    Some(Token::Int(n)) => Ok(Pattern::Literal(Literal::Int(-n))),
                    Some(Token::Float(f)) => Ok(Pattern::Literal(Literal::Float(-f))),
                    other => Err(format!("expected number after '-' in pattern, got {other:?}")),
                }
            }
            Some(Token::LBracket) => {
                let elements = child_nodes(node)
                    .iter()
                    .map(|n| self.pattern(n))
                    .collect::<Result<Vec<_>, _>>()?;
                // `...rest` — the rest name is the direct identifier token
                // following `...` (element identifiers live in Pattern nodes).
                let mut rest = None;
                let mut after_dots = false;
                for t in &tokens {
                    if matches!(t.token(), Some(Token::DotDotDot)) {
                        after_dots = true;
                    } else if after_dots {
                        if let Some(name) = ident_value(t) {
                            rest = Some(name);
                            after_dots = false;
                        }
                    }
                }
                Ok(Pattern::List { elements, rest })
            }
            Some(Token::LBrace) => {
                // Direct identifier tokens are the keys, in order, pairing
                // with the Pattern child nodes.
                let keys: Vec<String> = tokens.iter().filter_map(ident_value).collect();
                let values = child_nodes(node);
                if keys.len() != values.len() {
                    return Err(format!(
                        "record pattern with {} keys but {} value patterns",
                        keys.len(),
                        values.len()
                    ));
                }
                let fields = keys
                    .into_iter()
                    .zip(values.iter())
                    .map(|(k, v)| Ok((k, self.pattern(v)?)))
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(Pattern::Record(fields))
            }
            other => Err(format!("unexpected pattern start {other:?}")),
        }
    }

    fn lambda_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let span = self.node_span(node)?;
        let params = self.param_list(node)?;
        let has_arrow = direct_tokens(node)
            .iter()
            .any(|t| matches!(t.token(), Some(Token::Arrow)));
        let body = if has_arrow {
            // `fn(x) -> expr` — a single expression statement whose span the
            // parser stretches over the whole lambda.
            let expr_node = child_nodes(node)
                .into_iter()
                .find(|n| n.kind() != SyntaxKind::ParamList)
                .ok_or("arrow lambda missing its body expression")?;
            let expr = self.expr(&expr_node)?;
            vec![Stmt { kind: StmtKind::Expr(expr), span }]
        } else {
            self.block(node)?
        };
        Ok(Expr { kind: ExprKind::Lambda { params, body }, span })
    }

    /// Rebuild interpolation parts/exprs: parts are the direct string tokens
    /// in order, and when a hole abuts the closing quote the parser appends an
    /// empty final part (the lexer emits no string token there). The invariant
    /// `parts.len() == exprs.len() + 1` always holds.
    fn interp_parts(&mut self, node: &SyntaxNode) -> Result<(Vec<String>, Vec<Expr>), String> {
        let mut parts: Vec<String> = direct_tokens(node)
            .iter()
            .filter_map(|t| match t.token() {
                Some(Token::String(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let exprs = child_nodes(node)
            .iter()
            .map(|n| self.expr(n))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() == exprs.len() {
            parts.push(String::new());
        }
        if parts.len() != exprs.len() + 1 {
            return Err(format!(
                "interpolation with {} parts and {} exprs",
                parts.len(),
                exprs.len()
            ));
        }
        Ok((parts, exprs))
    }

    fn element_expr(&mut self, node: &SyntaxNode) -> Result<Expr, String> {
        let span = self.node_span(node)?;
        let mut tag = None;
        let mut props = Vec::new();
        let mut children = Vec::new();
        for el in node.children() {
            match el {
                SyntaxElement::Token(t) => match t.token() {
                    // The first tag name is the element's tag; the second (the
                    // closing tag) is validated by the parser and dropped.
                    Some(Token::JsxTagName(name)) if tag.is_none() => tag = Some(name.clone()),
                    Some(Token::JsxText(text)) => children.push(JsxChild::Text(text.clone())),
                    _ => {}
                },
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::JsxAttr => {
                    props.push(self.jsx_attr(&n)?);
                }
                // Child content: a nested element or a `{expr}` hole.
                SyntaxElement::Node(n) => children.push(JsxChild::Expr(self.expr(&n)?)),
            }
        }
        Ok(Expr {
            kind: ExprKind::Element {
                tag: tag.ok_or("JSX element missing its tag name")?,
                props,
                children,
            },
            span,
        })
    }

    fn jsx_attr(&mut self, node: &SyntaxNode) -> Result<(String, Expr), String> {
        let name = self.only_ident(node)?;
        let value = match child_nodes(node).first() {
            // `name={expr}` — the braced expression.
            Some(expr_node) => self.expr(expr_node)?,
            // `name="value"` — a string literal spanning its token.
            None => {
                let string_tok = direct_tokens(node)
                    .into_iter()
                    .find(|t| matches!(t.token(), Some(Token::String(_))))
                    .ok_or("JSX attribute missing its value")?;
                let Some(Token::String(s)) = string_tok.token().cloned() else { unreachable!() };
                Expr {
                    kind: ExprKind::Literal(Literal::String(s)),
                    span: self.token_span(&string_tok),
                }
            }
        };
        Ok((name, value))
    }
}

fn bin_op(tok: &Token) -> Option<BinOp> {
    Some(match tok {
        Token::Plus => BinOp::Add,
        Token::Minus | Token::MinusPrefix => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::Percent => BinOp::Mod,
        Token::PlusPlus => BinOp::Concat,
        Token::Eq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::Le => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::Ge => BinOp::Ge,
        Token::And => BinOp::And,
        Token::Or => BinOp::Or,
        Token::DoubleQuestion => BinOp::Coalesce,
        _ => return None,
    })
}

fn param_names(param_list: &SyntaxNode) -> Vec<String> {
    direct_tokens(param_list).iter().filter_map(ident_value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::parse_cst;
    use crate::lexer::Lexer;
    use crate::parse::Parser;

    /// Parse `src` the ordinary way (the authoritative AST).
    fn direct_ast(src: &str) -> Result<Vec<Stmt>, String> {
        let mut lexer = Lexer::new(src);
        lexer.tokenize()?;
        let mut parser = Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
        parser.parse_program()
    }

    /// Parse `src` through the CST and project the AST back out.
    fn projected_ast(src: &str) -> Result<Vec<Stmt>, String> {
        let green = parse_cst(src)?;
        project(&SyntaxNode::new_root(green))
    }

    /// The 3c invariant: the projection reproduces the parser's AST exactly —
    /// kinds, values, and spans. Compared via Debug formatting, which covers
    /// every field including SourceSpan line/column/offset.
    fn assert_projects(src: &str) {
        let direct = direct_ast(src).expect("direct parse");
        let projected = projected_ast(src).expect("projected parse");
        assert_eq!(
            format!("{direct:#?}"),
            format!("{projected:#?}"),
            "projected AST differs from parser AST for {src:?}"
        );
    }

    #[test]
    fn projects_core_statements() {
        assert_projects("let x = 1\n");
        assert_projects("x = 2\n");
        assert_projects("x += 1\ny *= 2 + 3\n");
        assert_projects("a.b = 1\na[0] = 2\na.b.c += 3\n");
        assert_projects("fn add(a, b)\n  a + b\nend\n");
        assert_projects("fn none()\nend\n");
        assert_projects("for i in [1, 2] do\n  print(i)\nend\n");
        assert_projects("while x < 10 do\n  x += 1\nend\n");
        assert_projects("fn f()\n  return\nend\nfn g()\n  return 1\nend\n");
        assert_projects("for i in xs do\n  if i then\n    break\n  else\n    continue\n  end\nend\n");
        assert_projects("state count = 0\nstate(key) slot = init()\n");
        assert_projects("enum Shape\n  Circle(r)\n  Point\n  Rect(w, h)\nend\n");
    }

    #[test]
    fn projects_import_forms() {
        assert_projects("import ui\n");
        assert_projects("import ui as u\n");
        assert_projects("import ui: button, clicked\n");
    }

    #[test]
    fn projects_expressions() {
        assert_projects("1 + 2 * 3 - 4 / 5 % 6\n");
        assert_projects("a ++ b ++ \"s\"\n");
        assert_projects("a == b != c\na < b <= c > d >= e\n");
        assert_projects("a && b || !c\n");
        assert_projects("-x - -y\n");
        assert_projects("(a + b) * c\n");
        assert_projects("f(a, b)(c)\n");
        assert_projects("f()\n");
        assert_projects("obj.field.nested[i + 1](arg)\n");
        assert_projects("[1, 2, 3]\n[1 -2 3]\n[]\n");
        assert_projects("{ a: 1, b: f(2), ...rest }\n");
        assert_projects("nil\ntrue\nfalse\n1.5\n\"plain\"\n");
        assert_projects("f(@x, y)\n");
        assert_projects("let c = #ff8800\nlet d = #f80\nlet e = #f808\nlet g = #ff880080\n");
    }

    #[test]
    fn projects_pipes() {
        assert_projects("xs |> sum\n");
        assert_projects("xs |> map(fn(x) -> x * 2) |> sum\n");
        assert_projects("a |> f |> g |> h\n");
        assert_projects("xs |>\n  sum\n");
    }

    #[test]
    fn projects_if_and_match() {
        assert_projects("if x then\n  1\nend\n");
        assert_projects("if x then\n  1\nelse\n  2\nend\n");
        assert_projects("if a then\n  1\nelsif b then\n  2\nelsif c then\n  3\nelse\n  4\nend\n");
        assert_projects("if a then\n  1\nelsif b then\n  2\nend\n");
        assert_projects("let x = if c then\n  1\nelse\n  2\nend\n");
        assert_projects("match v\nwhen Some(x) -> x\nwhen _ -> 0\nend\n");
        assert_projects("match v\nwhen x if x > 1 -> x\nwhen _ do\n  print(1)\n  2\nend\nend\n");
        assert_projects(
            "match v\nwhen [a, b, ...rest] -> a\nwhen { x: 1, y: p } -> p\nwhen -1 -> 0\nwhen -1.5 -> 0\nwhen \"s\" -> 1\nwhen true -> 2\nwhen nil -> 3\nend\n",
        );
    }

    #[test]
    fn projects_lambdas() {
        assert_projects("let f = fn(x) -> x * 2\n");
        assert_projects("let f = fn(x, y)\n  let z = x + y\n  z\nend\n");
        assert_projects("let f = fn() -> 1\n");
    }

    #[test]
    fn projects_string_interpolation() {
        assert_projects("print(\"hello, {name}!\")\n");
        assert_projects("print(\"{a}{b}{c}\")\n");
        assert_projects("print(\"sum = {1 + 2}\")\n");
        assert_projects("print(\"{x}\")\n");
        assert_projects("let s = \"\"\"\n  raw {braces} kept\n\"\"\"\n");
        assert_projects("let empty = \"\"\n");
    }

    #[test]
    fn projects_jsx() {
        assert_projects("let e = <div class=\"x\">hello {name} world</div>\n");
        assert_projects("let e = <br/>\n");
        assert_projects("let e = <div a=\"1\" b={x + 1}><span>inner</span><hr/></div>\n");
    }

    #[test]
    fn state_ids_allocate_in_parse_order() {
        // A state nested inside another state's init (via a lambda body) is
        // parsed — and must be numbered — before the outer state's own id.
        assert_projects("state outer = fn()\n  state inner = 1\n  inner\nend\nstate last = 2\n");
        let stmts = projected_ast(
            "state outer = fn()\n  state inner = 1\n  inner\nend\nstate last = 2\n",
        )
        .unwrap();
        let StmtKind::State { id: outer_id, ref init, .. } = stmts[0].kind else {
            panic!("expected state stmt");
        };
        let StmtKind::State { id: last_id, .. } = stmts[1].kind else {
            panic!("expected state stmt");
        };
        let ExprKind::Lambda { ref body, .. } = init.kind else { panic!("expected lambda") };
        let StmtKind::State { id: inner_id, .. } = body[0].kind else {
            panic!("expected nested state stmt");
        };
        assert_eq!((inner_id, outer_id, last_id), (0, 1, 2));
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

    /// The definitive 3c proof: for every repo program that parses, the AST
    /// projected from the CST is identical — including spans — to the AST the
    /// parser builds directly.
    #[test]
    fn projected_ast_matches_parser_over_repo_corpus() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);

        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else { continue };
            let Ok(direct) = direct_ast(&src) else { continue };
            let projected = projected_ast(&src)
                .unwrap_or_else(|e| panic!("projection failed for {}: {e}", path.display()));
            assert_eq!(
                format!("{direct:#?}"),
                format!("{projected:#?}"),
                "projected AST differs from parser AST for {}",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }
}
