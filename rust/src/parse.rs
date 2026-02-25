use crate::ast::*;
use crate::lexer::Token;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    next_state_id: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
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

    fn peek_ahead(&self, offset: usize) -> &Token {
        let idx = self.pos + offset;
        if idx < self.tokens.len() {
            &self.tokens[idx]
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
            Err(format!(
                "Expected {:?}, got {:?}",
                expected,
                self.peek()
            ))
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

    // ---- Statement Parsing ----

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        match self.peek().clone() {
            Token::Let => self.parse_let(),
            Token::Fn => self.parse_fn_decl(),
            Token::For => self.parse_for(),
            Token::While => self.parse_while(),
            Token::Return => self.parse_return(),
            Token::Break => {
                self.advance();
                Ok(Stmt::Break)
            }
            Token::Continue => {
                self.advance();
                Ok(Stmt::Continue)
            }
            Token::State => self.parse_state(),
            Token::Enum => self.parse_enum_decl(),
            _ => self.parse_expr_or_assign(),
        }
    }

    fn parse_let(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'let'
        let name = self.expect_ident()?;
        self.expect(&Token::Assign)?;
        let value = self.parse_expr()?;
        Ok(Stmt::Let { name, value })
    }

    fn parse_state(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'state'
        let name = self.expect_ident()?;
        self.expect(&Token::Assign)?;
        let init = self.parse_expr()?;
        let id = self.next_state_id;
        self.next_state_id += 1;
        Ok(Stmt::State { name, init, id })
    }

    fn parse_fn_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'fn'
        let name = self.expect_ident()?;
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::FnDecl { name, params, body })
    }

    fn parse_enum_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'enum'
        let name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;
        self.skip_newlines();

        let mut variants = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let variant_name = self.expect_ident()?;
            let fields = if matches!(self.peek(), Token::LParen) {
                self.advance(); // consume '('
                let params = self.parse_param_list()?;
                self.expect(&Token::RParen)?;
                params
            } else {
                Vec::new()
            };
            variants.push(EnumVariant {
                name: variant_name,
                fields,
            });
            self.skip_separator();
        }
        self.expect(&Token::RBrace)?;
        Ok(Stmt::EnumDecl { name, variants })
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'for'
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::For { var, iter, body })
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'while'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::While { condition, body })
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        self.advance(); // consume 'return'
        // Check if there's an expression following (not a newline or closing brace)
        if matches!(self.peek(), Token::Newline | Token::RBrace | Token::Eof) {
            Ok(Stmt::Return(None))
        } else {
            let expr = self.parse_expr()?;
            Ok(Stmt::Return(Some(expr)))
        }
    }

    fn parse_expr_or_assign(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expr()?;

        if matches!(self.peek(), Token::Assign) {
            self.advance(); // consume '='
            let value = self.parse_expr()?;
            let target = expr_to_assign_target(expr)?;
            Ok(Stmt::Assign { target, value })
        } else if let Some(op) = self.peek_compound_assign_op() {
            self.advance(); // consume the compound assignment token
            let rhs = self.parse_expr()?;
            // Desugar: target op= rhs  →  target = target op rhs
            let target = expr_to_assign_target(expr.clone())?;
            let value = Expr::BinaryOp {
                op,
                left: Box::new(expr),
                right: Box::new(rhs),
            };
            Ok(Stmt::Assign { target, value })
        } else {
            Ok(Stmt::Expr(expr))
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

    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
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

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.advance() {
            Token::Ident(name) => Ok(name),
            other => Err(format!("Expected identifier, got {:?}", other)),
        }
    }

    // ---- Expression Parsing (Pratt parser) ----

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_and()?;
            left = Expr::BinaryOp {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op: BinOp::Concat,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
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
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                })
            }
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                })
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
                    expr = Expr::FieldAccess {
                        object: Box::new(expr),
                        field,
                    };
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    expr = Expr::IndexAccess {
                        object: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Token::LParen => {
                    // Only treat as function call if the paren is on the same "logical line"
                    // We check if the expression is callable (ident, field access, etc.)
                    if self.is_callable(&expr) {
                        self.advance();
                        let args = self.parse_arg_list()?;
                        self.expect(&Token::RParen)?;
                        expr = Expr::Call {
                            function: Box::new(expr),
                            args,
                        };
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn is_callable(&self, _expr: &Expr) -> bool {
        // All expressions followed by ( are treated as calls
        true
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Token::Int(n) => {
                self.advance();
                Ok(Expr::Literal(Literal::Int(n)))
            }
            Token::Float(f) => {
                self.advance();
                Ok(Expr::Literal(Literal::Float(f)))
            }
            Token::InterpStart => {
                return self.parse_string_interp();
            }
            Token::String(s) => {
                self.advance();
                Ok(Expr::Literal(Literal::String(s)))
            }
            Token::True => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(false)))
            }
            Token::Nil => {
                self.advance();
                Ok(Expr::Literal(Literal::Nil))
            }
            Token::Ident(_) => {
                let name = self.expect_ident()?;
                Ok(Expr::Ident(name))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => self.parse_list_literal(),
            Token::LBrace => self.parse_record_or_block(),
            Token::If => self.parse_if_expr(),
            Token::Match => self.parse_match_expr(),
            Token::Fn => self.parse_lambda(),
            Token::JsxOpenStart => self.parse_jsx_element(),
            other => Err(format!("Unexpected token: {:?}", other)),
        }
    }

    fn parse_list_literal(&mut self) -> Result<Expr, String> {
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
        Ok(Expr::List(elements))
    }

    fn parse_record_or_block(&mut self) -> Result<Expr, String> {
        // Peek ahead to determine if this is a record { key: val } or block { stmts }
        // A record starts with { ident : ... }
        let is_record = self.is_record_start();
        if is_record {
            self.parse_record_literal()
        } else {
            self.parse_block_expr()
        }
    }

    fn is_record_start(&self) -> bool {
        // Look ahead past newlines for ident : pattern
        let mut offset = 1; // skip the '{'
        // Skip newlines
        while matches!(self.peek_ahead(offset), Token::Newline) {
            offset += 1;
        }
        // Check for ident followed by colon
        if let Token::Ident(_) = self.peek_ahead(offset) {
            offset += 1;
            // Skip newlines
            while matches!(self.peek_ahead(offset), Token::Newline) {
                offset += 1;
            }
            matches!(self.peek_ahead(offset), Token::Colon)
        } else {
            false
        }
    }

    fn parse_record_literal(&mut self) -> Result<Expr, String> {
        self.advance(); // consume '{'
        let mut fields = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let key = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let value = self.parse_expr()?;
            fields.push((key, value));
            self.skip_separator();
        }
        self.expect(&Token::RBrace)?;
        Ok(Expr::Record(fields))
    }

    fn parse_block_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume '{'
        let body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Expr::Block(body))
    }

    fn parse_if_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume 'if'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let then_body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        self.skip_newlines();

        let else_body = if matches!(self.peek(), Token::Else) {
            self.advance(); // consume 'else'
            self.skip_newlines();
            if matches!(self.peek(), Token::If) {
                let else_if = self.parse_if_expr()?;
                Some(ElseBranch::ElseIf(Box::new(else_if)))
            } else {
                self.expect(&Token::LBrace)?;
                let else_stmts = self.parse_block_body()?;
                self.expect(&Token::RBrace)?;
                Some(ElseBranch::Block(else_stmts))
            }
        } else {
            None
        };

        Ok(Expr::If {
            condition: Box::new(condition),
            then_body,
            else_body,
        })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // consume 'match'
        let subject = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        self.skip_newlines();

        let mut arms = Vec::new();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            let arm = self.parse_match_arm()?;
            arms.push(arm);
            self.skip_newlines();
        }
        self.expect(&Token::RBrace)?;

        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, String> {
        let pattern = self.parse_pattern()?;
        let guard = if matches!(self.peek(), Token::If) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Token::Arrow)?;
        self.skip_newlines();
        let body = self.parse_expr()?;
        self.skip_newlines();
        Ok(MatchArm {
            pattern,
            guard,
            body,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Token::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Ident(name) => {
                self.advance();
                // Check for variant with fields: Name(p1, p2)
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
                    // Could be a simple enum variant or a variable binding
                    // We distinguish at runtime based on whether the name is a known enum variant
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
                // Negative number literal
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
                    _ => Err("Expected number after '-' in pattern".to_string()),
                }
            }
            other => Err(format!("Expected pattern, got {:?}", other)),
        }
    }

    fn parse_list_pattern(&mut self) -> Result<Pattern, String> {
        self.advance(); // consume '['
        let mut elements = Vec::new();
        let mut rest = None;
        self.skip_newlines();

        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            // Check for ...rest
            if matches!(self.peek(), Token::DotDotDot) {
                self.advance(); // consume '...'
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
        self.advance(); // consume 'fn'
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        self.skip_newlines();
        self.expect(&Token::LBrace)?;
        let body = self.parse_block_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Expr::Lambda { params, body })
    }

    fn parse_string_interp(&mut self) -> Result<Expr, String> {
        self.advance(); // consume InterpStart
        let mut parts: Vec<String> = Vec::new();
        let mut exprs: Vec<Expr> = Vec::new();

        // Token stream: String, expr_tokens, String, expr_tokens, ..., String, InterpEnd
        loop {
            // Expect a string part
            match self.peek().clone() {
                Token::String(s) => {
                    self.advance();
                    parts.push(s);
                }
                Token::InterpEnd => {
                    // Edge case: shouldn't happen without a string first, but be safe
                    parts.push(String::new());
                }
                other => {
                    return Err(format!(
                        "Expected string part in interpolation, got {:?}",
                        other
                    ));
                }
            }

            // Check if we're done
            if matches!(self.peek(), Token::InterpEnd) {
                self.advance();
                break;
            }

            // Parse expression
            let expr = self.parse_expr()?;
            exprs.push(expr);
        }

        // If no expressions were found, it's just a plain string
        if exprs.is_empty() {
            Ok(Expr::Literal(Literal::String(parts.into_iter().next().unwrap_or_default())))
        } else {
            Ok(Expr::StringInterp { parts, exprs })
        }
    }

    fn parse_jsx_element(&mut self) -> Result<Expr, String> {
        // Consume JsxOpenStart (already matched by caller but not advanced)
        self.advance();
        // Expect tag name
        let tag = match self.advance() {
            Token::JsxTagName(name) => name,
            other => return Err(format!("Expected JSX tag name, got {:?}", other)),
        };

        // Parse attributes until `>` or `/>`
        let mut props = Vec::new();
        loop {
            match self.peek().clone() {
                Token::Gt => {
                    self.advance();
                    break;
                }
                Token::JsxSelfClose => {
                    self.advance();
                    return Ok(Expr::Element {
                        tag,
                        props,
                        children: Vec::new(),
                    });
                }
                Token::Ident(attr_name) => {
                    self.advance();
                    self.expect(&Token::Assign)?;
                    // Attribute value: string literal or {expr}
                    let value = match self.peek().clone() {
                        Token::String(s) => {
                            self.advance();
                            Expr::Literal(Literal::String(s))
                        }
                        Token::LBrace => {
                            self.advance();
                            let expr = self.parse_expr()?;
                            self.expect(&Token::RBrace)?;
                            expr
                        }
                        other => {
                            return Err(format!(
                                "Expected string or {{expr}} for attribute value, got {:?}",
                                other
                            ))
                        }
                    };
                    props.push((attr_name, value));
                }
                other => {
                    return Err(format!(
                        "Unexpected token in JSX tag: {:?}",
                        other
                    ))
                }
            }
        }

        // Parse children until `</tag>`
        let mut children = Vec::new();
        loop {
            match self.peek().clone() {
                Token::JsxCloseStart => {
                    self.advance();
                    // Expect matching tag name
                    match self.advance() {
                        Token::JsxTagName(close_tag) => {
                            if close_tag != tag {
                                return Err(format!(
                                    "Mismatched JSX tags: <{}> and </{}>",
                                    tag, close_tag
                                ));
                            }
                        }
                        other => {
                            return Err(format!(
                                "Expected closing tag name, got {:?}",
                                other
                            ))
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
                    return Err(format!("Unclosed JSX element <{}>", tag));
                }
                other => {
                    return Err(format!(
                        "Unexpected token in JSX children: {:?}",
                        other
                    ))
                }
            }
        }

        Ok(Expr::Element {
            tag,
            props,
            children,
        })
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

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    match expr {
        Expr::Ident(name) => Ok(AssignTarget::Name(name)),
        Expr::FieldAccess { object, field } => Ok(AssignTarget::Field(object, field)),
        Expr::IndexAccess { object, index } => Ok(AssignTarget::Index(object, index)),
        _ => Err("Invalid assignment target".to_string()),
    }
}
