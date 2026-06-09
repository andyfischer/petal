use crate::ast::*;
use crate::lexer::Token;
use crate::source_map::{SourceSpan, ZERO_SPAN};

pub struct Parser {
    tokens: Vec<Token>,
    token_spans: Vec<SourceSpan>,
    pos: usize,
    next_state_id: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>, token_spans: Vec<SourceSpan>) -> Self {
        Self {
            tokens,
            token_spans,
            pos: 0,
            next_state_id: 0,
        }
    }

    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.is_at_end() {
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            &Token::Eof
        }
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            let got = self.peek().clone();
            let msg = match (expected, &got) {
                (Token::Arrow, Token::Assign) => {
                    "Expected '->' but got '=' — use '->' for match arms, not '=>'".to_string()
                }
                (Token::RBrace, Token::Eof) => {
                    "Missing closing '}'".to_string()
                }
                (Token::RParen, Token::Eof) => {
                    "Missing closing ')'".to_string()
                }
                (Token::RBracket, Token::Eof) => {
                    "Missing closing ']'".to_string()
                }
                _ => {
                    format!("Expected {:?}, got {:?}", expected, got)
                }
            };
            Err(self.error_at_current(msg))
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    fn skip_separator(&mut self) {
        // Skip newlines and commas
        while matches!(self.peek(), Token::Newline | Token::Comma) {
            self.advance();
        }
    }

    /// Get the span of the token at position - 1 (the last consumed token).
    fn prev_span(&self) -> SourceSpan {
        if self.pos > 0 && self.pos - 1 < self.token_spans.len() {
            self.token_spans[self.pos - 1]
        } else {
            ZERO_SPAN
        }
    }

    /// Create a span from start_pos (token index) to the last consumed token.
    fn span_from(&self, start_pos: usize) -> SourceSpan {
        let start = if start_pos < self.token_spans.len() {
            self.token_spans[start_pos].start
        } else {
            ZERO_SPAN.start
        };
        let end = self.prev_span().end;
        SourceSpan { start, end }
    }

    /// Helper to create an Expr with a span from start_pos to the last consumed token.
    fn mk_expr(&self, kind: ExprKind, start_pos: usize) -> Expr {
        Expr { kind, span: self.span_from(start_pos) }
    }

    /// Helper to create a Stmt with a span from start_pos to the last consumed token.
    fn mk_stmt(&self, kind: StmtKind, start_pos: usize) -> Stmt {
        Stmt { kind, span: self.span_from(start_pos) }
    }

    // ---- Statement Parsing ----

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        let start = self.pos;
        match self.peek().clone() {
            Token::Let => self.parse_let(start),
            Token::Fn => self.parse_fn_decl(start),
            Token::For => self.parse_for(start),
            Token::While => self.parse_while(start),
            Token::Return => self.parse_return(start),
            Token::Break => {
                self.advance();
                Ok(self.mk_stmt(StmtKind::Break, start))
            }
            Token::Continue => {
                self.advance();
                Ok(self.mk_stmt(StmtKind::Continue, start))
            }
            Token::State => self.parse_state(start),
            Token::Enum => self.parse_enum_decl(start),
            _ => self.parse_expr_or_assign(start),
        }
    }

    fn parse_let(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'let'
        let name = self.expect_ident()?;
        self.expect(&Token::Assign)?;
        let value = self.parse_expr()?;
        Ok(self.mk_stmt(StmtKind::Let { name, value }, start))
    }

    fn parse_state(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'state'

        // Check for explicit key: state(expr) name = init
        let key = if matches!(self.peek(), Token::LParen) {
            self.advance(); // consume '('
            let key_expr = self.parse_expr()?;
            self.expect(&Token::RParen)?;
            Some(key_expr)
        } else {
            None
        };

        let name = self.expect_ident()?;
        self.expect(&Token::Assign)?;
        let init = self.parse_expr()?;
        let id = self.next_state_id;
        self.next_state_id += 1;
        Ok(self.mk_stmt(StmtKind::State { name, init, id, key }, start))
    }

    fn parse_fn_decl(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'fn'
        let name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        self.skip_newlines();
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
        Ok(self.mk_stmt(StmtKind::FnDecl { name, params, body }, start))
    }

    fn parse_enum_decl(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'enum'
        let name = self.expect_ident()?;
        self.skip_newlines();
        let mut variants = Vec::new();
        while !matches!(self.peek(), Token::End | Token::Eof) {
            let variant_name = self.expect_ident()?;
            let fields = if matches!(self.peek(), Token::LParen) {
                self.advance(); // consume '('
                let params = self.parse_param_list()?;
                self.expect(&Token::RParen)?;
                params
            } else {
                Vec::new()
            };
            variants.push(EnumVariant { name: variant_name, fields });
            self.skip_separator();
        }
        self.expect(&Token::End)?;
        Ok(self.mk_stmt(StmtKind::EnumDecl { name, variants }, start))
    }

    fn parse_for(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'for'
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
        Ok(self.mk_stmt(StmtKind::For { var, iter, body }, start))
    }

    fn parse_while(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'while'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
        Ok(self.mk_stmt(StmtKind::While { condition, body }, start))
    }

    fn parse_return(&mut self, start: usize) -> Result<Stmt, String> {
        self.advance(); // consume 'return'
        if matches!(self.peek(), Token::Newline | Token::End | Token::Else | Token::Elsif | Token::Eof) {
            Ok(self.mk_stmt(StmtKind::Return(None), start))
        } else {
            let expr = self.parse_expr()?;
            Ok(self.mk_stmt(StmtKind::Return(Some(expr)), start))
        }
    }

    fn parse_expr_or_assign(&mut self, start: usize) -> Result<Stmt, String> {
        let expr = self.parse_expr()?;

        if matches!(self.peek(), Token::Assign) {
            self.advance(); // consume '='
            let value = self.parse_expr()?;
            let target = expr_to_assign_target(expr)?;
            Ok(self.mk_stmt(StmtKind::Assign { target, value }, start))
        } else if let Some(op) = self.peek_compound_assign_op() {
            self.advance(); // consume the compound assignment token
            let rhs = self.parse_expr()?;
            // Desugar: target op= rhs  →  target = target op rhs
            let target = expr_to_assign_target(expr.clone())?;
            let value = Expr {
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(expr),
                    right: Box::new(rhs),
                },
                span: self.span_from(start),
            };
            Ok(self.mk_stmt(StmtKind::Assign { target, value }, start))
        } else {
            Ok(self.mk_stmt(StmtKind::Expr(expr), start))
        }
    }

    fn peek_compound_assign_op(&self) -> Option<BinOp> {
        match self.peek() {
            Token::PlusAssign => Some(BinOp::Add),
            Token::MinusAssign => Some(BinOp::Sub),
            Token::StarAssign => Some(BinOp::Mul),
            Token::SlashAssign => Some(BinOp::Div),
            Token::PercentAssign => Some(BinOp::Mod),
            _ => None,
        }
    }

    /// Parse statements until the next significant token is one of `stops`
    /// (or Eof). Does NOT consume the stop token.
    fn parse_block_until(&mut self, stops: &[Token]) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::Eof) && !stops.contains(self.peek()) {
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn parse_param_list(&mut self) -> Result<Vec<String>, String> {
        let mut params = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            let name = self.expect_ident()?;
            params.push(name);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
            }
        }
        Ok(params)
    }

    /// Get the span of the current token (the one at self.pos).
    fn current_span(&self) -> SourceSpan {
        if self.pos < self.token_spans.len() {
            self.token_spans[self.pos]
        } else {
            ZERO_SPAN
        }
    }

    /// Format an error message with the current token's source position.
    fn error_at_current(&self, msg: String) -> String {
        let span = self.current_span();
        if span.start.line > 0 {
            format!("{} [line {}, column {}]", msg, span.start.line, span.start.column)
        } else {
            msg
        }
    }

    /// Format an error at a specific token position.
    fn error_at(&self, pos: usize, msg: String) -> String {
        if pos < self.token_spans.len() {
            let span = self.token_spans[pos];
            if span.start.line > 0 {
                format!("{} [line {}, column {}]", msg, span.start.line, span.start.column)
            } else {
                msg
            }
        } else {
            msg
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        let pos = self.pos;
        match self.advance() {
            Token::Ident(name) => Ok(name),
            other => Err(self.error_at(pos, format!("Expected identifier, got {:?}", other))),
        }
    }

    // ---- Expression Parsing (Pratt parser) ----

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_pipe()
    }

    fn parse_pipe(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_or()?;
        while matches!(self.peek(), Token::Pipe) {
            let start = self.pos;
            self.advance();
            self.skip_newlines();
            let rhs = self.parse_or()?;
            left = match rhs.kind {
                ExprKind::Call { function, mut args } => {
                    args.insert(0, left);
                    self.mk_expr(ExprKind::Call { function, args }, start)
                }
                _ => {
                    self.mk_expr(ExprKind::Call {
                        function: Box::new(rhs),
                        args: vec![left],
                    }, start)
                }
            };
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_and()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while matches!(self.peek(), Token::And) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_equality()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        while matches!(self.peek(), Token::Eq | Token::Ne) {
            let op = match self.advance() {
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_comparison()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_concat()?;
        while matches!(self.peek(), Token::Lt | Token::Le | Token::Gt | Token::Ge) {
            let op = match self.advance() {
                Token::Lt => BinOp::Lt,
                Token::Le => BinOp::Le,
                Token::Gt => BinOp::Gt,
                Token::Ge => BinOp::Ge,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_concat()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        while matches!(self.peek(), Token::PlusPlus) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_additive()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::Concat,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        while matches!(self.peek(), Token::Plus | Token::Minus) {
            let op = match self.advance() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_multiplicative()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        while matches!(self.peek(), Token::Star | Token::Slash | Token::Percent) {
            let op = match self.advance() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_unary()?;
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        match self.peek().clone() {
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(self.mk_expr(ExprKind::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                }, start))
            }
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(self.mk_expr(ExprKind::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                }, start))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                        },
                        kind: ExprKind::FieldAccess {
                            object: Box::new(expr),
                            field,
                        },
                    };
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                        },
                        kind: ExprKind::IndexAccess {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                    };
                }
                Token::LParen => {
                    self.check_callable(&expr)?;
                    self.advance();
                    let args = self.parse_arg_list()?;
                    self.expect(&Token::RParen)?;
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                        },
                        kind: ExprKind::Call {
                            function: Box::new(expr),
                            args,
                        },
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn check_callable(&self, expr: &Expr) -> Result<(), String> {
        match &expr.kind {
            // Definitely callable: identifiers, field/index access, calls, lambdas, blocks
            ExprKind::Ident(_)
            | ExprKind::FieldAccess { .. }
            | ExprKind::IndexAccess { .. }
            | ExprKind::Call { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. } => Ok(()),

            // Not callable: literals, operators, collections, etc.
            ExprKind::Literal(_) => Err(self.error_at_current("Literal value cannot be called as a function".to_string())),
            ExprKind::BinaryOp { .. } => Err(self.error_at_current("Binary operation result cannot be called as a function".to_string())),
            ExprKind::UnaryOp { .. } => Err(self.error_at_current("Unary operation result cannot be called as a function".to_string())),
            ExprKind::List(_) => Err(self.error_at_current("List literal cannot be called as a function".to_string())),
            ExprKind::Record(_) => Err(self.error_at_current("Record literal cannot be called as a function".to_string())),
            ExprKind::StringInterp { .. } => Err(self.error_at_current("String interpolation cannot be called as a function".to_string())),
            ExprKind::Element { .. } => Err(self.error_at_current("Element cannot be called as a function".to_string())),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        match self.peek().clone() {
            Token::Int(n) => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Int(n)), start))
            }
            Token::Float(f) => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Float(f)), start))
            }
            Token::InterpStart => {
                self.parse_string_interp()
            }
            Token::String(s) => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::String(s)), start))
            }
            Token::True => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Bool(true)), start))
            }
            Token::False => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Bool(false)), start))
            }
            Token::Nil => {
                self.advance();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Nil), start))
            }
            Token::Ident(_) => {
                let name = self.expect_ident()?;
                Ok(self.mk_expr(ExprKind::Ident(name), start))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => self.parse_list_literal(),
            Token::LBrace => self.parse_record_literal(),
            Token::If => self.parse_if_expr(),
            Token::Match => self.parse_match_expr(),
            Token::Fn => self.parse_lambda(),
            Token::Color(hex) => {
                self.advance();
                let fields = parse_color_hex(&hex);
                let record_fields = fields
                    .into_iter()
                    .map(|(name, value)| {
                        RecordField::Named(name.to_string(), Expr {
                            kind: ExprKind::Literal(Literal::Int(value)),
                            span: self.span_from(start),
                        })
                    })
                    .collect();
                Ok(self.mk_expr(ExprKind::Record(record_fields), start))
            }
            Token::JsxOpenStart => self.parse_jsx_element(),
            other => Err(self.error_at_current(format!("Unexpected token: {:?}", other))),
        }
    }

    fn parse_list_literal(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume '['
        let mut elements = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            let elem = self.parse_expr()?;
            elements.push(elem);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
            }
        }
        self.expect(&Token::RBracket)?;
        Ok(self.mk_expr(ExprKind::List(elements), start))
    }

    fn parse_record_literal(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume '{'
        let mut fields = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            if matches!(self.peek(), Token::DotDotDot) {
                self.advance(); // consume '...'
                let expr = self.parse_expr()?;
                fields.push(RecordField::Spread(expr));
            } else {
                let key = self.expect_ident()?;
                self.expect(&Token::Colon)?;
                let value = self.parse_expr()?;
                fields.push(RecordField::Named(key, value));
            }
            self.skip_separator();
        }
        self.expect(&Token::RBrace)?;
        Ok(self.mk_expr(ExprKind::Record(fields), start))
    }

    fn parse_if_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume 'if'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Then)?;
        let then_body = self.parse_block_until(&[Token::Elsif, Token::Else, Token::End])?;
        let else_body = self.parse_else_chain()?;
        Ok(self.mk_expr(ExprKind::If {
            condition: Box::new(condition),
            then_body,
            else_body,
        }, start))
    }

    /// Parse the tail of an if-expression after the then-body. Consumes the
    /// single closing `end` for the whole if/elsif/else chain. Precondition:
    /// peek is Elsif, Else, or End.
    fn parse_else_chain(&mut self) -> Result<Option<ElseBranch>, String> {
        match self.peek() {
            Token::Elsif => {
                let start = self.pos;
                self.advance(); // consume 'elsif'
                let condition = self.parse_expr()?;
                self.skip_newlines();
                self.expect(&Token::Then)?;
                let then_body = self.parse_block_until(&[Token::Elsif, Token::Else, Token::End])?;
                let else_body = self.parse_else_chain()?; // consumes the final 'end'
                let inner = self.mk_expr(ExprKind::If {
                    condition: Box::new(condition),
                    then_body,
                    else_body,
                }, start);
                Ok(Some(ElseBranch::ElseIf(Box::new(inner))))
            }
            Token::Else => {
                self.advance(); // consume 'else'
                let body = self.parse_block_until(&[Token::End])?;
                self.expect(&Token::End)?;
                Ok(Some(ElseBranch::Block(body)))
            }
            _ => {
                self.expect(&Token::End)?;
                Ok(None)
            }
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume 'match'
        let subject = self.parse_expr()?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while !matches!(self.peek(), Token::End | Token::Eof) {
            let arm = self.parse_match_arm()?;
            arms.push(arm);
            self.skip_newlines();
        }
        self.expect(&Token::End)?;
        Ok(self.mk_expr(ExprKind::Match { subject: Box::new(subject), arms }, start))
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, String> {
        self.expect(&Token::When)?;
        let pattern = self.parse_pattern()?;
        let guard = if matches!(self.peek(), Token::If) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let body = if matches!(self.peek(), Token::Do) {
            let start = self.pos;
            self.advance(); // consume 'do'
            let stmts = self.parse_block_until(&[Token::End])?;
            self.expect(&Token::End)?;
            self.mk_expr(ExprKind::Block(stmts), start)
        } else {
            self.expect(&Token::Arrow)?;
            self.skip_newlines();
            self.parse_expr()?
        };
        self.skip_newlines();
        Ok(MatchArm { pattern, guard, body })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Token::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Ident(name) => {
                self.advance();
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let mut fields = Vec::new();
                    self.skip_newlines();
                    while !matches!(self.peek(), Token::RParen | Token::Eof) {
                        let field_pat = self.parse_pattern()?;
                        fields.push(field_pat);
                        self.skip_newlines();
                        if matches!(self.peek(), Token::Comma) {
                            self.advance();
                            self.skip_newlines();
                        }
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Variant { name, fields })
                } else {
                    Ok(Pattern::Variable(name))
                }
            }
            Token::Int(n) => {
                self.advance();
                Ok(Pattern::Literal(Literal::Int(n)))
            }
            Token::Float(f) => {
                self.advance();
                Ok(Pattern::Literal(Literal::Float(f)))
            }
            Token::String(s) => {
                self.advance();
                Ok(Pattern::Literal(Literal::String(s)))
            }
            Token::True => {
                self.advance();
                Ok(Pattern::Literal(Literal::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Pattern::Literal(Literal::Bool(false)))
            }
            Token::Nil => {
                self.advance();
                Ok(Pattern::Literal(Literal::Nil))
            }
            Token::LBracket => self.parse_list_pattern(),
            Token::LBrace => self.parse_record_pattern(),
            Token::Minus => {
                self.advance();
                match self.peek().clone() {
                    Token::Int(n) => {
                        self.advance();
                        Ok(Pattern::Literal(Literal::Int(-n)))
                    }
                    Token::Float(f) => {
                        self.advance();
                        Ok(Pattern::Literal(Literal::Float(-f)))
                    }
                    _ => Err(self.error_at_current("Expected number after '-' in pattern".to_string())),
                }
            }
            other => Err(self.error_at_current(format!("Expected pattern, got {:?}", other))),
        }
    }

    fn parse_list_pattern(&mut self) -> Result<Pattern, String> {
        self.advance(); // consume '['
        let mut elements = Vec::new();
        let mut rest = None;
        self.skip_newlines();

        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            if matches!(self.peek(), Token::DotDotDot) {
                self.advance();
                let name = self.expect_ident()?;
                rest = Some(name);
                self.skip_newlines();
                if matches!(self.peek(), Token::Comma) {
                    self.advance();
                    self.skip_newlines();
                }
                continue;
            }

            let elem = self.parse_pattern()?;
            elements.push(elem);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
            }
        }
        self.expect(&Token::RBracket)?;
        Ok(Pattern::List { elements, rest })
    }

    fn parse_record_pattern(&mut self) -> Result<Pattern, String> {
        self.advance(); // consume '{'
        let mut fields = Vec::new();
        self.skip_newlines();

        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let key = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let pat = self.parse_pattern()?;
            fields.push((key, pat));
            self.skip_separator();
        }
        self.expect(&Token::RBrace)?;
        Ok(Pattern::Record(fields))
    }

    fn parse_lambda(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume 'fn'
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        if matches!(self.peek(), Token::Arrow) {
            self.advance(); // consume '->'
            self.skip_newlines();
            let expr = self.parse_expr()?;
            let body = vec![self.mk_stmt(StmtKind::Expr(expr), start)];
            Ok(self.mk_expr(ExprKind::Lambda { params, body }, start))
        } else {
            self.skip_newlines();
            let body = self.parse_block_until(&[Token::End])?;
            self.expect(&Token::End)?;
            Ok(self.mk_expr(ExprKind::Lambda { params, body }, start))
        }
    }

    fn parse_string_interp(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume InterpStart
        let mut parts: Vec<String> = Vec::new();
        let mut exprs: Vec<Expr> = Vec::new();

        loop {
            match self.peek().clone() {
                Token::String(s) => {
                    self.advance();
                    parts.push(s);
                }
                Token::InterpEnd => {
                    parts.push(String::new());
                }
                other => {
                    return Err(self.error_at_current(format!(
                        "Expected string part in interpolation, got {:?}",
                        other
                    )));
                }
            }

            if matches!(self.peek(), Token::InterpEnd) {
                self.advance();
                break;
            }

            let expr = self.parse_expr()?;
            exprs.push(expr);
        }

        if exprs.is_empty() {
            Ok(self.mk_expr(
                ExprKind::Literal(Literal::String(parts.into_iter().next().unwrap_or_default())),
                start,
            ))
        } else {
            Ok(self.mk_expr(ExprKind::StringInterp { parts, exprs }, start))
        }
    }

    fn parse_jsx_element(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.advance(); // consume JsxOpenStart
        let tag = match self.advance() {
            Token::JsxTagName(name) => name,
            other => return Err(self.error_at(self.pos - 1, format!("Expected JSX tag name, got {:?}", other))),
        };

        let mut props = Vec::new();
        loop {
            match self.peek().clone() {
                Token::Gt => {
                    self.advance();
                    break;
                }
                Token::JsxSelfClose => {
                    self.advance();
                    return Ok(self.mk_expr(ExprKind::Element {
                        tag,
                        props,
                        children: Vec::new(),
                    }, start));
                }
                Token::Ident(attr_name) => {
                    self.advance();
                    self.expect(&Token::Assign)?;
                    let value = match self.peek().clone() {
                        Token::String(s) => {
                            let attr_start = self.pos;
                            self.advance();
                            self.mk_expr(ExprKind::Literal(Literal::String(s)), attr_start)
                        }
                        Token::LBrace => {
                            self.advance();
                            let expr = self.parse_expr()?;
                            self.expect(&Token::RBrace)?;
                            expr
                        }
                        other => {
                            return Err(self.error_at_current(format!(
                                "Expected string or {{expr}} for attribute value, got {:?}",
                                other
                            )))
                        }
                    };
                    props.push((attr_name, value));
                }
                other => {
                    return Err(self.error_at_current(format!(
                        "Unexpected token in JSX tag: {:?}",
                        other
                    )))
                }
            }
        }

        let mut children = Vec::new();
        loop {
            match self.peek().clone() {
                Token::JsxCloseStart => {
                    self.advance();
                    match self.advance() {
                        Token::JsxTagName(close_tag) => {
                            if close_tag != tag {
                                return Err(self.error_at(self.pos - 1, format!(
                                    "Mismatched JSX tags: <{}> and </{}>",
                                    tag, close_tag
                                )));
                            }
                        }
                        other => {
                            return Err(self.error_at(self.pos - 1, format!(
                                "Expected closing tag name, got {:?}",
                                other
                            )))
                        }
                    }
                    break;
                }
                Token::JsxText(text) => {
                    self.advance();
                    children.push(JsxChild::Text(text));
                }
                Token::LBrace => {
                    self.advance();
                    let expr = self.parse_expr()?;
                    self.expect(&Token::RBrace)?;
                    children.push(JsxChild::Expr(expr));
                }
                Token::JsxOpenStart => {
                    let nested = self.parse_jsx_element()?;
                    children.push(JsxChild::Expr(nested));
                }
                Token::Eof => {
                    return Err(self.error_at_current(format!("Unclosed JSX element <{}>", tag)));
                }
                other => {
                    return Err(self.error_at_current(format!(
                        "Unexpected token in JSX children: {:?}",
                        other
                    )))
                }
            }
        }

        Ok(self.mk_expr(ExprKind::Element {
            tag,
            props,
            children,
        }, start))
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            let arg = self.parse_expr()?;
            args.push(arg);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
            }
        }
        Ok(args)
    }
}

/// Parse a hex color string (without '#') into (field_name, value) pairs.
/// Supports #rgb, #rgba, #rrggbb, #rrggbbaa formats.
fn parse_color_hex(hex: &str) -> Vec<(&'static str, i64)> {
    let expand = |c: u8| -> i64 {
        let v = if c.is_ascii_digit() { c - b'0' } else { (c.to_ascii_lowercase() - b'a') + 10 };
        (v as i64) * 17 // e.g. 0xf -> 255, 0x8 -> 136
    };
    let parse2 = |hi: u8, lo: u8| -> i64 {
        let h = if hi.is_ascii_digit() { hi - b'0' } else { (hi.to_ascii_lowercase() - b'a') + 10 };
        let l = if lo.is_ascii_digit() { lo - b'0' } else { (lo.to_ascii_lowercase() - b'a') + 10 };
        (h as i64) * 16 + (l as i64)
    };
    let b = hex.as_bytes();
    match b.len() {
        3 => vec![("r", expand(b[0])), ("g", expand(b[1])), ("b", expand(b[2]))],
        4 => vec![("r", expand(b[0])), ("g", expand(b[1])), ("b", expand(b[2])), ("a", expand(b[3]))],
        6 => vec![("r", parse2(b[0], b[1])), ("g", parse2(b[2], b[3])), ("b", parse2(b[4], b[5]))],
        8 => vec![("r", parse2(b[0], b[1])), ("g", parse2(b[2], b[3])), ("b", parse2(b[4], b[5])), ("a", parse2(b[6], b[7]))],
        _ => unreachable!("lexer validates hex length"),
    }
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    let span = expr.span;
    match expr.kind {
        ExprKind::Ident(name) => Ok(AssignTarget::Name(name)),
        ExprKind::FieldAccess { object, field } => Ok(AssignTarget::Field(object, field)),
        ExprKind::IndexAccess { object, index } => Ok(AssignTarget::Index(object, index)),
        _ => {
            if span.start.line > 0 {
                Err(format!("Invalid assignment target [line {}, column {}]", span.start.line, span.start.column))
            } else {
                Err("Invalid assignment target".to_string())
            }
        }
    }
}
