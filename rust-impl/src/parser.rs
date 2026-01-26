use crate::ast::*;
use crate::token::{Token, TokenInfo};

pub struct Parser {
    tokens: Vec<TokenInfo>,
    pos: usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at {}:{}: {}", self.line, self.column, self.message)
    }
}

impl Parser {
    pub fn new(tokens: Vec<TokenInfo>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens.get(self.pos).map(|t| &t.token).unwrap_or(&Token::Eof)
    }

    fn peek_info(&self) -> &TokenInfo {
        self.tokens.get(self.pos).unwrap_or(&TokenInfo { token: Token::Eof, line: 0, column: 0 })
    }

    fn advance(&mut self) -> &Token {
        let token = &self.tokens.get(self.pos).map(|t| &t.token).unwrap_or(&Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, expected: Token) -> Result<(), ParseError> {
        let info = self.peek_info().clone();
        if *self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(ParseError {
                message: format!("Expected {:?}, found {:?}", expected, self.peek()),
                line: info.line,
                column: info.column,
            })
        }
    }

    fn matches(&mut self, token: &Token) -> bool {
        if self.peek() == token {
            self.advance();
            true
        } else {
            false
        }
    }

    pub fn parse(&mut self) -> Result<Program, ParseError> {
        let mut statements = Vec::new();

        while *self.peek() != Token::Eof {
            statements.push(self.parse_statement()?);
        }

        Ok(Program { statements })
    }

    fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        match self.peek() {
            Token::Fn => self.parse_function(),
            Token::Let => self.parse_let(),
            Token::State => self.parse_state(),
            Token::Return => self.parse_return(),
            Token::If => self.parse_if_stmt(),
            Token::While => self.parse_while(),
            Token::For => self.parse_for(),
            Token::Loop => self.parse_loop(),
            Token::Break => {
                self.advance();
                Ok(Stmt::Break)
            }
            Token::Continue => {
                self.advance();
                Ok(Stmt::Continue)
            }
            Token::Struct => self.parse_struct(),
            Token::Enum => self.parse_enum(),
            _ => {
                let expr = self.parse_expression()?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_function(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Fn)?;

        let name = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected function name")),
        };

        self.expect(Token::LeftParen)?;
        let params = self.parse_params()?;
        self.expect(Token::RightParen)?;

        // Check for return type annotation
        if self.matches(&Token::Arrow) {
            // Skip type annotation for now
            if *self.peek() != Token::LeftBrace {
                // Single expression function: fn square(x) -> x * x
                let expr = self.parse_expression()?;
                return Ok(Stmt::Function {
                    name,
                    params,
                    body: vec![Stmt::Return(Some(expr))],
                });
            }
        }

        self.expect(Token::LeftBrace)?;
        let body = self.parse_block()?;

        Ok(Stmt::Function { name, params, body })
    }

    fn parse_params(&mut self) -> Result<Vec<String>, ParseError> {
        let mut params = Vec::new();

        if *self.peek() == Token::RightParen {
            return Ok(params);
        }

        loop {
            // Handle variadic params (...args)
            if self.matches(&Token::DotDot) {
                if self.matches(&Token::Dot) {
                    // ...
                }
            }

            match self.peek().clone() {
                Token::Identifier(name) => {
                    self.advance();
                    // Skip type annotation if present
                    if self.matches(&Token::Colon) {
                        // Skip type name
                        self.advance();
                    }
                    params.push(name);
                }
                _ => break,
            }

            if !self.matches(&Token::Comma) {
                break;
            }
        }

        Ok(params)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut statements = Vec::new();

        while *self.peek() != Token::RightBrace && *self.peek() != Token::Eof {
            statements.push(self.parse_statement()?);
        }

        self.expect(Token::RightBrace)?;
        Ok(statements)
    }

    fn parse_let(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Let)?;

        let name = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected variable name")),
        };

        // Skip type annotation if present
        if self.matches(&Token::Colon) {
            // Skip type name
            self.advance();
        }

        let value = if self.matches(&Token::Equal) {
            Some(self.parse_expression()?)
        } else {
            None
        };

        Ok(Stmt::Let { name, value })
    }

    fn parse_state(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::State)?;

        let name = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected variable name")),
        };

        self.expect(Token::Equal)?;
        let value = self.parse_expression()?;

        Ok(Stmt::State { name, value })
    }

    fn parse_return(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Return)?;

        // Check if there's an expression following
        let value = match self.peek() {
            Token::RightBrace | Token::Eof => None,
            _ => Some(self.parse_expression()?),
        };

        Ok(Stmt::Return(value))
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::If)?;

        let condition = self.parse_expression()?;
        self.expect(Token::LeftBrace)?;
        let then_branch = self.parse_block()?;

        let else_branch = if self.matches(&Token::Else) {
            if *self.peek() == Token::If {
                // else if
                Some(vec![self.parse_if_stmt()?])
            } else {
                self.expect(Token::LeftBrace)?;
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        Ok(Stmt::If {
            condition,
            then_branch,
            else_branch,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::While)?;

        let condition = self.parse_expression()?;
        self.expect(Token::LeftBrace)?;
        let body = self.parse_block()?;

        Ok(Stmt::While { condition, body })
    }

    fn parse_for(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::For)?;

        let var = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected variable name")),
        };

        self.expect(Token::In)?;
        let iter = self.parse_expression()?;
        self.expect(Token::LeftBrace)?;
        let body = self.parse_block()?;

        Ok(Stmt::For { var, iter, body })
    }

    fn parse_loop(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Loop)?;
        self.expect(Token::LeftBrace)?;
        let body = self.parse_block()?;

        Ok(Stmt::Loop { body })
    }

    fn parse_struct(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Struct)?;

        let name = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected struct name")),
        };

        self.expect(Token::LeftBrace)?;

        let mut fields = Vec::new();
        while *self.peek() != Token::RightBrace {
            let field_name = match self.peek().clone() {
                Token::Identifier(n) => {
                    self.advance();
                    n
                }
                _ => break,
            };

            let type_name = if self.matches(&Token::Colon) {
                match self.peek().clone() {
                    Token::Identifier(t) => {
                        self.advance();
                        Some(t)
                    }
                    _ => None,
                }
            } else {
                None
            };

            fields.push((field_name, type_name));
            self.matches(&Token::Comma);
        }

        self.expect(Token::RightBrace)?;

        Ok(Stmt::Struct { name, fields })
    }

    fn parse_enum(&mut self) -> Result<Stmt, ParseError> {
        self.expect(Token::Enum)?;

        let name = match self.peek().clone() {
            Token::Identifier(n) => {
                self.advance();
                n
            }
            _ => return Err(self.error("Expected enum name")),
        };

        self.expect(Token::LeftBrace)?;

        let mut variants = Vec::new();
        while *self.peek() != Token::RightBrace {
            let variant_name = match self.peek().clone() {
                Token::Identifier(n) => {
                    self.advance();
                    n
                }
                _ => break,
            };

            let fields = if self.matches(&Token::LeftParen) {
                let mut fields = Vec::new();
                while *self.peek() != Token::RightParen {
                    let field_name = match self.peek().clone() {
                        Token::Identifier(n) => {
                            self.advance();
                            n
                        }
                        _ => break,
                    };

                    let type_name = if self.matches(&Token::Colon) {
                        match self.peek().clone() {
                            Token::Identifier(t) => {
                                self.advance();
                                Some(t)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    fields.push((field_name, type_name));
                    self.matches(&Token::Comma);
                }
                self.expect(Token::RightParen)?;
                fields
            } else {
                Vec::new()
            };

            variants.push(EnumVariant {
                name: variant_name,
                fields,
            });

            self.matches(&Token::Comma);
        }

        self.expect(Token::RightBrace)?;

        Ok(Stmt::Enum { name, variants })
    }

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_dataflow()?;

        match self.peek() {
            Token::Equal => {
                self.advance();
                let value = self.parse_assignment()?;
                Ok(Expr::Assign {
                    target: Box::new(expr),
                    value: Box::new(value),
                })
            }
            Token::PlusEqual => {
                self.advance();
                let value = self.parse_assignment()?;
                Ok(Expr::CompoundAssign {
                    op: BinaryOp::Add,
                    target: Box::new(expr),
                    value: Box::new(value),
                })
            }
            Token::MinusEqual => {
                self.advance();
                let value = self.parse_assignment()?;
                Ok(Expr::CompoundAssign {
                    op: BinaryOp::Sub,
                    target: Box::new(expr),
                    value: Box::new(value),
                })
            }
            Token::StarEqual => {
                self.advance();
                let value = self.parse_assignment()?;
                Ok(Expr::CompoundAssign {
                    op: BinaryOp::Mul,
                    target: Box::new(expr),
                    value: Box::new(value),
                })
            }
            Token::SlashEqual => {
                self.advance();
                let value = self.parse_assignment()?;
                Ok(Expr::CompoundAssign {
                    op: BinaryOp::Div,
                    target: Box::new(expr),
                    value: Box::new(value),
                })
            }
            _ => Ok(expr),
        }
    }

    fn parse_dataflow(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_or()?;

        while self.matches(&Token::At) {
            let right = self.parse_or()?;
            expr = Expr::Dataflow {
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_and()?;

        while self.matches(&Token::Or) {
            let right = self.parse_and()?;
            expr = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_equality()?;

        while self.matches(&Token::And) {
            let right = self.parse_equality()?;
            expr = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_comparison()?;

        loop {
            let op = match self.peek() {
                Token::EqualEqual => BinaryOp::Eq,
                Token::BangEqual => BinaryOp::Ne,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            expr = Expr::BinaryOp {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_term()?;

        loop {
            let op = match self.peek() {
                Token::Less => BinaryOp::Lt,
                Token::LessEqual => BinaryOp::Le,
                Token::Greater => BinaryOp::Gt,
                Token::GreaterEqual => BinaryOp::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_term()?;
            expr = Expr::BinaryOp {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_factor()?;

        loop {
            let op = match self.peek() {
                Token::Plus => BinaryOp::Add,
                Token::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_factor()?;
            expr = Expr::BinaryOp {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_power()?;

        loop {
            let op = match self.peek() {
                Token::Star => BinaryOp::Mul,
                Token::Slash => BinaryOp::Div,
                Token::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_power()?;
            expr = Expr::BinaryOp {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }

        Ok(expr)
    }

    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let expr = self.parse_unary()?;

        if self.matches(&Token::StarStar) {
            let right = self.parse_power()?; // Right associative
            return Ok(Expr::BinaryOp {
                op: BinaryOp::Pow,
                left: Box::new(expr),
                right: Box::new(right),
            });
        }

        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::Bang => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek() {
                Token::LeftParen => {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(Token::RightParen)?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                Token::Dot => {
                    self.advance();
                    let property = match self.peek().clone() {
                        Token::Identifier(n) => {
                            self.advance();
                            n
                        }
                        _ => return Err(self.error("Expected property name")),
                    };

                    // Check if it's a method call
                    if *self.peek() == Token::LeftParen {
                        self.advance();
                        let args = self.parse_args()?;
                        self.expect(Token::RightParen)?;
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: property,
                            args,
                        };
                    } else {
                        expr = Expr::PropertyAccess {
                            object: Box::new(expr),
                            property,
                        };
                    }
                }
                Token::LeftBracket => {
                    self.advance();
                    let index = self.parse_expression()?;
                    self.expect(Token::RightBracket)?;
                    expr = Expr::IndexAccess {
                        object: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Token::ColonColon => {
                    self.advance();
                    let variant = match self.peek().clone() {
                        Token::Identifier(n) => {
                            self.advance();
                            n
                        }
                        _ => return Err(self.error("Expected variant name")),
                    };

                    let enum_name = match expr {
                        Expr::Identifier(name) => name,
                        _ => return Err(self.error("Expected enum name")),
                    };

                    let args = if *self.peek() == Token::LeftParen {
                        self.advance();
                        let args = self.parse_args()?;
                        self.expect(Token::RightParen)?;
                        Some(args)
                    } else {
                        None
                    };

                    expr = Expr::EnumVariant {
                        enum_name,
                        variant,
                        args,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();

        if *self.peek() == Token::RightParen {
            return Ok(args);
        }

        loop {
            args.push(self.parse_expression()?);

            if !self.matches(&Token::Comma) {
                // Optional commas - try to continue if next token looks like an arg
                match self.peek() {
                    Token::RightParen => break,
                    Token::Integer(_) | Token::Float(_) | Token::String(_)
                    | Token::Identifier(_) | Token::LeftParen | Token::LeftBracket
                    | Token::LeftBrace | Token::True | Token::False | Token::Null
                    | Token::Symbol(_) | Token::Fn | Token::Minus | Token::Bang => continue,
                    _ => break,
                }
            }
        }

        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.peek().clone();
        match token {
            Token::Integer(n) => {
                self.advance();
                Ok(Expr::Integer(n))
            }
            Token::Float(n) => {
                self.advance();
                Ok(Expr::Float(n))
            }
            Token::String(s) => {
                self.advance();
                Ok(Expr::String(s))
            }
            Token::True => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            Token::False => {
                self.advance();
                Ok(Expr::Bool(false))
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::Symbol(s) => {
                self.advance();
                Ok(Expr::Symbol(s))
            }
            Token::Identifier(name) => {
                self.advance();
                // Check if it's a function call to range
                if name == "range" && *self.peek() == Token::LeftParen {
                    self.advance();
                    let start = self.parse_expression()?;
                    self.matches(&Token::Comma);
                    let end = self.parse_expression()?;
                    let step = if self.matches(&Token::Comma) {
                        Some(Box::new(self.parse_expression()?))
                    } else {
                        None
                    };
                    self.expect(Token::RightParen)?;
                    return Ok(Expr::Range {
                        start: Box::new(start),
                        end: Box::new(end),
                        step,
                    });
                }
                Ok(Expr::Identifier(name))
            }
            Token::LeftParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(Token::RightParen)?;
                Ok(expr)
            }
            Token::LeftBracket => {
                self.advance();
                let mut elements = Vec::new();

                while *self.peek() != Token::RightBracket {
                    elements.push(self.parse_expression()?);
                    if !self.matches(&Token::Comma) {
                        // Support optional commas
                        if *self.peek() == Token::RightBracket {
                            break;
                        }
                    }
                }

                self.expect(Token::RightBracket)?;
                Ok(Expr::Array(elements))
            }
            Token::LeftBrace => {
                self.advance();
                let mut fields = Vec::new();

                while *self.peek() != Token::RightBrace {
                    let key = match self.peek().clone() {
                        Token::Identifier(n) => {
                            self.advance();
                            n
                        }
                        _ => break,
                    };

                    self.expect(Token::Colon)?;
                    let value = self.parse_expression()?;
                    fields.push((key, value));

                    if !self.matches(&Token::Comma) {
                        if *self.peek() == Token::RightBrace {
                            break;
                        }
                    }
                }

                self.expect(Token::RightBrace)?;
                Ok(Expr::Object(fields))
            }
            Token::Fn => {
                self.advance();
                self.expect(Token::LeftParen)?;
                let params = self.parse_params()?;
                self.expect(Token::RightParen)?;
                self.expect(Token::FatArrow)?;

                let body = if *self.peek() == Token::LeftBrace {
                    self.advance();
                    let stmts = self.parse_block()?;
                    Expr::Block(stmts)
                } else {
                    self.parse_expression()?
                };

                Ok(Expr::Lambda {
                    params,
                    body: Box::new(body),
                })
            }
            Token::If => {
                self.advance();
                let condition = self.parse_expression()?;
                self.expect(Token::LeftBrace)?;
                let then_stmts = self.parse_block()?;
                let then_branch = Expr::Block(then_stmts);

                let else_branch = if self.matches(&Token::Else) {
                    if *self.peek() == Token::If {
                        Some(Box::new(self.parse_primary()?))
                    } else {
                        self.expect(Token::LeftBrace)?;
                        let else_stmts = self.parse_block()?;
                        Some(Box::new(Expr::Block(else_stmts)))
                    }
                } else {
                    None
                };

                Ok(Expr::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch,
                })
            }
            Token::Match => {
                self.advance();
                let value = self.parse_expression()?;
                self.expect(Token::LeftBrace)?;

                let mut arms = Vec::new();
                while *self.peek() != Token::RightBrace {
                    let pattern = self.parse_pattern()?;

                    // Check for guard
                    let guard = if self.matches(&Token::If) {
                        Some(Box::new(self.parse_expression()?))
                    } else {
                        None
                    };

                    self.expect(Token::Arrow)?;

                    let body = if *self.peek() == Token::LeftBrace {
                        self.advance();
                        let stmts = self.parse_block()?;
                        Expr::Block(stmts)
                    } else {
                        self.parse_expression()?
                    };

                    arms.push(MatchArm {
                        pattern,
                        guard,
                        body: Box::new(body),
                    });

                    // Optional comma after arm
                    self.matches(&Token::Comma);
                }

                self.expect(Token::RightBrace)?;

                Ok(Expr::Match {
                    value: Box::new(value),
                    arms,
                })
            }
            Token::For => {
                self.advance();
                let var = match self.peek().clone() {
                    Token::Identifier(n) => {
                        self.advance();
                        n
                    }
                    _ => return Err(self.error("Expected variable name")),
                };

                self.expect(Token::In)?;
                let iter = self.parse_expression()?;
                self.expect(Token::LeftBrace)?;

                let mut body_stmts = Vec::new();
                while *self.peek() != Token::RightBrace {
                    body_stmts.push(self.parse_statement()?);
                }
                self.expect(Token::RightBrace)?;

                // If there's only one expression statement, use it directly
                let body = if body_stmts.len() == 1 {
                    if let Stmt::Expr(expr) = &body_stmts[0] {
                        expr.clone()
                    } else {
                        Expr::Block(body_stmts)
                    }
                } else {
                    Expr::Block(body_stmts)
                };

                Ok(Expr::ForExpr {
                    var,
                    iter: Box::new(iter),
                    body: Box::new(body),
                })
            }
            _ => Err(self.error(&format!("Unexpected token: {:?}", token))),
        }
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            Token::Identifier(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Identifier(name) => {
                self.advance();
                Ok(Pattern::Variable(name))
            }
            Token::Integer(n) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Integer(n)))
            }
            Token::Float(n) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Float(n)))
            }
            Token::String(s) => {
                self.advance();
                Ok(Pattern::Literal(Expr::String(s)))
            }
            Token::True => {
                self.advance();
                Ok(Pattern::Literal(Expr::Bool(true)))
            }
            Token::False => {
                self.advance();
                Ok(Pattern::Literal(Expr::Bool(false)))
            }
            Token::Null => {
                self.advance();
                Ok(Pattern::Literal(Expr::Null))
            }
            Token::Symbol(s) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Symbol(s)))
            }
            Token::LeftBracket => {
                self.advance();
                let mut patterns = Vec::new();

                while *self.peek() != Token::RightBracket {
                    patterns.push(self.parse_pattern()?);
                    if !self.matches(&Token::Comma) {
                        break;
                    }
                }

                self.expect(Token::RightBracket)?;
                Ok(Pattern::Array(patterns))
            }
            Token::LeftBrace => {
                self.advance();
                let mut fields = Vec::new();

                while *self.peek() != Token::RightBrace {
                    let key = match self.peek().clone() {
                        Token::Identifier(n) => {
                            self.advance();
                            n
                        }
                        _ => break,
                    };

                    let pattern = if self.matches(&Token::Colon) {
                        self.parse_pattern()?
                    } else {
                        Pattern::Variable(key.clone())
                    };

                    fields.push((key, pattern));

                    if !self.matches(&Token::Comma) {
                        break;
                    }
                }

                self.expect(Token::RightBrace)?;
                Ok(Pattern::Object(fields))
            }
            _ => Err(self.error("Expected pattern")),
        }
    }

    fn error(&self, message: &str) -> ParseError {
        let info = self.peek_info();
        ParseError {
            message: message.to_string(),
            line: info.line,
            column: info.column,
        }
    }
}
