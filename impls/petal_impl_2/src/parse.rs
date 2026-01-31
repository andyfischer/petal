use crate::error::Error;
use crate::program::{FunctionDef, Program, ProgramKey};
use crate::term::{StateKey, Term, TermId, TermOp};
use crate::value::Value;
use crate::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    // Literals
    Int(i64),
    Float(f64),
    String(String),
    True,
    False,
    Nil,

    // Identifiers and keywords
    Ident(String),
    Let,
    State,
    Fn,
    If,
    Else,
    For,
    While,
    In,
    Return,
    Break,
    Continue,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    EqEq,
    BangEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    AndAnd,
    OrOr,
    Eq,
    Dot,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,

    // Special
    Eof,
}

struct Lexer {
    input: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else if ch == '/' && self.input.get(self.pos + 1) == Some(&'/') {
                // Single-line comment
                while let Some(ch) = self.peek() {
                    self.advance();
                    if ch == '\n' {
                        break;
                    }
                }
            } else if ch == '/' && self.input.get(self.pos + 1) == Some(&'*') {
                // Multi-line comment
                self.advance(); // /
                self.advance(); // *
                while let Some(ch) = self.peek() {
                    if ch == '*' && self.input.get(self.pos + 1) == Some(&'/') {
                        self.advance(); // *
                        self.advance(); // /
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();

        let ch = match self.peek() {
            Some(c) => c,
            None => return Ok(Token::Eof),
        };

        // Numbers
        if ch.is_ascii_digit() {
            return self.read_number();
        }

        // Identifiers and keywords
        if ch.is_alphabetic() || ch == '_' {
            return self.read_ident();
        }

        // Strings
        if ch == '"' {
            return self.read_string();
        }

        // Operators and delimiters
        self.advance();
        let token = match ch {
            '+' => Token::Plus,
            '-' => Token::Minus,
            '*' => Token::Star,
            '/' => Token::Slash,
            '%' => Token::Percent,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '.' => Token::Dot,
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::BangEq
                } else {
                    Token::Bang
                }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::EqEq
                } else {
                    Token::Eq
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::LtEq
                } else {
                    Token::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::GtEq
                } else {
                    Token::Gt
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    Token::AndAnd
                } else {
                    return Err(Error::ParseError(format!("Unexpected character: {}", ch)));
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    Token::OrOr
                } else {
                    return Err(Error::ParseError(format!("Unexpected character: {}", ch)));
                }
            }
            _ => return Err(Error::ParseError(format!("Unexpected character: {}", ch))),
        };

        Ok(token)
    }

    fn read_number(&mut self) -> Result<Token> {
        let mut num = String::new();
        let mut is_float = false;

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                num.push(ch);
                self.advance();
            } else if ch == '.' && !is_float {
                is_float = true;
                num.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if is_float {
            let value = num
                .parse::<f64>()
                .map_err(|_| Error::ParseError(format!("Invalid float: {}", num)))?;
            Ok(Token::Float(value))
        } else {
            let value = num
                .parse::<i64>()
                .map_err(|_| Error::ParseError(format!("Invalid integer: {}", num)))?;
            Ok(Token::Int(value))
        }
    }

    fn read_ident(&mut self) -> Result<Token> {
        let mut ident = String::new();

        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                ident.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        let token = match ident.as_str() {
            "let" => Token::Let,
            "state" => Token::State,
            "fn" => Token::Fn,
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "while" => Token::While,
            "in" => Token::In,
            "return" => Token::Return,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "true" => Token::True,
            "false" => Token::False,
            "nil" => Token::Nil,
            _ => Token::Ident(ident),
        };

        Ok(token)
    }

    fn read_string(&mut self) -> Result<Token> {
        self.advance(); // consume opening "
        let mut s = String::new();

        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.advance();
                return Ok(Token::String(s));
            } else if ch == '\\' {
                self.advance();
                if let Some(escaped) = self.peek() {
                    self.advance();
                    match escaped {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        _ => {
                            s.push('\\');
                            s.push(escaped);
                        }
                    }
                }
            } else {
                s.push(ch);
                self.advance();
            }
        }

        Err(Error::ParseError("Unterminated string".to_string()))
    }
}

pub struct Parser {
    lexer: Lexer,
    current: Token,
    program: Program,
    next_term_id: usize,
    next_state_key: u64,
}

impl Parser {
    pub fn new(source: &str, program_key: ProgramKey) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token().unwrap_or(Token::Eof);

        Self {
            lexer,
            current,
            program: Program::new(program_key, source.to_string()),
            next_term_id: 0,
            next_state_key: 1,
        }
    }

    fn next_token(&mut self) -> Result<()> {
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    fn peek(&self) -> &Token {
        &self.current
    }

    fn expect(&mut self, expected: Token) -> Result<()> {
        if self.current == expected {
            self.next_token()?;
            Ok(())
        } else {
            Err(Error::ParseError(format!(
                "Expected {:?}, got {:?}",
                expected, self.current
            )))
        }
    }

    fn alloc_term_id(&mut self) -> TermId {
        let id = TermId(self.next_term_id);
        self.next_term_id += 1;
        id
    }

    fn alloc_state_key(&mut self) -> StateKey {
        let key = StateKey(self.next_state_key);
        self.next_state_key += 1;
        key
    }

    pub fn parse(&mut self) -> Result<Program> {
        let mut statements = Vec::new();

        while *self.peek() != Token::Eof {
            statements.push(self.parse_statement()?);
        }

        // Link statements together with control_flow_next
        for i in 0..statements.len().saturating_sub(1) {
            let current_id = statements[i];
            let next_id = statements[i + 1];

            if let Some(term) = self.program.get_term_mut(current_id) {
                term.control_flow_next = Some(next_id);
            }
        }

        // Entry point is the first statement (or nop if empty)
        self.program.entry = statements.first().copied().unwrap_or_else(|| {
            let id = self.alloc_term_id();
            let term = Term::new(id, TermOp::Constant(Value::Nil));
            self.program.add_term(term);
            id
        });

        Ok(self.program.clone())
    }

    fn parse_statement(&mut self) -> Result<TermId> {
        match self.peek() {
            Token::Let => self.parse_let(),
            Token::State => self.parse_state(),
            Token::Fn => self.parse_function_def(),
            Token::Return => self.parse_return(),
            Token::Break => self.parse_break(),
            Token::Continue => self.parse_continue(),
            _ => self.parse_expr_statement(),
        }
    }

    fn parse_let(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'let'

        let name = match self.peek() {
            Token::Ident(s) => s.clone(),
            _ => return Err(Error::ParseError("Expected identifier after 'let'".to_string())),
        };
        self.next_token()?;

        self.expect(Token::Eq)?;

        let value = self.parse_expr()?;

        let id = self.alloc_term_id();
        let term = Term::new(id, TermOp::StoreVar(name)).with_inputs(vec![value]);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_state(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'state'

        let name = match self.peek() {
            Token::Ident(s) => s.clone(),
            _ => return Err(Error::ParseError("Expected identifier after 'state'".to_string())),
        };
        self.next_token()?;

        self.expect(Token::Eq)?;

        let init_value = self.parse_expr()?;
        let state_key = self.alloc_state_key();

        // Use StateDeclare which combines init, read, and store
        let id = self.alloc_term_id();
        let term = Term::new(
            id,
            TermOp::StateDeclare {
                state_key,
                var_name: name,
            },
        )
        .with_inputs(vec![init_value])
        .with_state_key(state_key);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_function_def(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'fn'

        let name = match self.peek() {
            Token::Ident(s) => s.clone(),
            _ => return Err(Error::ParseError("Expected function name".to_string())),
        };
        self.next_token()?;

        self.expect(Token::LParen)?;

        let mut params = Vec::new();
        while *self.peek() != Token::RParen {
            if let Token::Ident(param) = self.peek() {
                params.push(param.clone());
                self.next_token()?;

                if *self.peek() == Token::Comma {
                    self.next_token()?;
                }
            } else {
                return Err(Error::ParseError("Expected parameter name".to_string()));
            }
        }
        self.expect(Token::RParen)?;

        let body_terms = self.parse_block()?;

        let entry = body_terms.first().copied().unwrap_or_else(|| {
            let id = self.alloc_term_id();
            let term = Term::new(id, TermOp::Constant(Value::Nil));
            self.program.add_term(term);
            id
        });

        let func_def = FunctionDef {
            name: name.clone(),
            params,
            body: body_terms,
            entry,
        };

        let func_idx = self.program.add_function(func_def);

        // Store function in variable - allocate IDs in the correct order!
        let const_id = self.alloc_term_id();
        let const_term = Term::new(const_id, TermOp::Constant(Value::Function(func_idx)));
        self.program.add_term(const_term);

        let store_id = self.alloc_term_id();
        let store_term = Term::new(store_id, TermOp::StoreVar(name)).with_inputs(vec![const_id]);
        self.program.add_term(store_term);

        Ok(store_id)
    }

    fn parse_return(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'return'

        let value = if matches!(
            self.peek(),
            Token::Semicolon | Token::RBrace | Token::Eof
        ) {
            let id = self.alloc_term_id();
            let term = Term::new(id, TermOp::Constant(Value::Nil));
            self.program.add_term(term);
            id
        } else {
            self.parse_expr()?
        };

        let id = self.alloc_term_id();
        let term = Term::new(id, TermOp::Return).with_inputs(vec![value]);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_break(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'break'

        let id = self.alloc_term_id();
        let term = Term::new(id, TermOp::Break);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_continue(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'continue'

        let id = self.alloc_term_id();
        let term = Term::new(id, TermOp::Continue);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_expr_statement(&mut self) -> Result<TermId> {
        let expr = self.parse_expr()?;

        // Optional semicolon
        if *self.peek() == Token::Semicolon {
            self.next_token()?;
        }

        Ok(expr)
    }

    fn parse_expr(&mut self) -> Result<TermId> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<TermId> {
        let expr = self.parse_logical_or()?;

        if *self.peek() == Token::Eq {
            // This is an assignment
            self.next_token()?;

            // Get the variable name from the expr
            let name_opt = self.program.get_term(expr).and_then(|term| {
                if let TermOp::LoadVar(name) = &term.op {
                    Some(name.clone())
                } else {
                    None
                }
            });

            if let Some(name) = name_opt {
                let value = self.parse_assignment()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::StoreVar(name)).with_inputs(vec![value]);
                self.program.add_term(term);
                return Ok(id);
            }

            return Err(Error::ParseError("Invalid assignment target".to_string()));
        }

        Ok(expr)
    }

    fn parse_logical_or(&mut self) -> Result<TermId> {
        let mut left = self.parse_logical_and()?;

        while *self.peek() == Token::OrOr {
            self.next_token()?;
            let right = self.parse_logical_and()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, TermOp::Or).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Result<TermId> {
        let mut left = self.parse_equality()?;

        while *self.peek() == Token::AndAnd {
            self.next_token()?;
            let right = self.parse_equality()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, TermOp::And).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<TermId> {
        let mut left = self.parse_comparison()?;

        loop {
            let op = match self.peek() {
                Token::EqEq => TermOp::Eq,
                Token::BangEq => TermOp::NotEq,
                _ => break,
            };
            self.next_token()?;

            let right = self.parse_comparison()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, op).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<TermId> {
        let mut left = self.parse_term()?;

        loop {
            let op = match self.peek() {
                Token::Lt => TermOp::Lt,
                Token::Gt => TermOp::Gt,
                Token::LtEq => TermOp::LtEq,
                Token::GtEq => TermOp::GtEq,
                _ => break,
            };
            self.next_token()?;

            let right = self.parse_term()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, op).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_term(&mut self) -> Result<TermId> {
        let mut left = self.parse_factor()?;

        loop {
            let op = match self.peek() {
                Token::Plus => TermOp::Add,
                Token::Minus => TermOp::Sub,
                _ => break,
            };
            self.next_token()?;

            let right = self.parse_factor()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, op).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<TermId> {
        let mut left = self.parse_unary()?;

        loop {
            let op = match self.peek() {
                Token::Star => TermOp::Mul,
                Token::Slash => TermOp::Div,
                Token::Percent => TermOp::Mod,
                _ => break,
            };
            self.next_token()?;

            let right = self.parse_unary()?;

            let id = self.alloc_term_id();
            let term = Term::new(id, op).with_inputs(vec![left, right]);
            self.program.add_term(term);
            left = id;
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<TermId> {
        match self.peek() {
            Token::Bang => {
                self.next_token()?;
                let operand = self.parse_unary()?;

                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Not).with_inputs(vec![operand]);
                self.program.add_term(term);

                Ok(id)
            }
            Token::Minus => {
                self.next_token()?;
                let operand = self.parse_unary()?;

                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Neg).with_inputs(vec![operand]);
                self.program.add_term(term);

                Ok(id)
            }
            _ => self.parse_call(),
        }
    }

    fn parse_call(&mut self) -> Result<TermId> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek() {
                Token::LParen => {
                    self.next_token()?;

                    let mut args = Vec::new();
                    while *self.peek() != Token::RParen {
                        args.push(self.parse_expr()?);

                        if *self.peek() == Token::Comma {
                            self.next_token()?;
                        }
                    }
                    self.expect(Token::RParen)?;

                    let id = self.alloc_term_id();
                    let term = Term::new(
                        id,
                        TermOp::Call {
                            function: expr,
                            args: args.clone(),
                        },
                    )
                    .with_inputs(vec![expr]);
                    self.program.add_term(term);

                    expr = id;
                }
                Token::LBracket => {
                    self.next_token()?;
                    let index = self.parse_expr()?;
                    self.expect(Token::RBracket)?;

                    let id = self.alloc_term_id();
                    let term = Term::new(
                        id,
                        TermOp::Index {
                            target: expr,
                            index,
                        },
                    )
                    .with_inputs(vec![expr, index]);
                    self.program.add_term(term);

                    expr = id;
                }
                Token::Dot => {
                    self.next_token()?;

                    let field = match self.peek() {
                        Token::Ident(s) => s.clone(),
                        _ => {
                            return Err(Error::ParseError(
                                "Expected field name after '.'".to_string(),
                            ))
                        }
                    };
                    self.next_token()?;

                    let id = self.alloc_term_id();
                    let term = Term::new(
                        id,
                        TermOp::FieldAccess {
                            target: expr,
                            field,
                        },
                    )
                    .with_inputs(vec![expr]);
                    self.program.add_term(term);

                    expr = id;
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<TermId> {
        match self.peek().clone() {
            Token::Int(n) => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::Int(n)));
                self.program.add_term(term);
                Ok(id)
            }
            Token::Float(f) => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::Float(f)));
                self.program.add_term(term);
                Ok(id)
            }
            Token::String(s) => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::String(s)));
                self.program.add_term(term);
                Ok(id)
            }
            Token::True => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::Bool(true)));
                self.program.add_term(term);
                Ok(id)
            }
            Token::False => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::Bool(false)));
                self.program.add_term(term);
                Ok(id)
            }
            Token::Nil => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::Constant(Value::Nil));
                self.program.add_term(term);
                Ok(id)
            }
            Token::Ident(name) => {
                self.next_token()?;
                let id = self.alloc_term_id();
                let term = Term::new(id, TermOp::LoadVar(name));
                self.program.add_term(term);
                Ok(id)
            }
            Token::LParen => {
                self.next_token()?;
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Token::If => self.parse_if(),
            Token::For => self.parse_for(),
            Token::While => self.parse_while(),
            Token::LBrace => {
                let terms = self.parse_block()?;
                Ok(terms.last().copied().unwrap_or_else(|| {
                    let id = self.alloc_term_id();
                    let term = Term::new(id, TermOp::Constant(Value::Nil));
                    self.program.add_term(term);
                    id
                }))
            }
            Token::LBracket => self.parse_list(),
            _ => Err(Error::ParseError(format!(
                "Unexpected token: {:?}",
                self.peek()
            ))),
        }
    }

    fn parse_if(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'if'

        let condition = self.parse_expr()?;
        let then_block = self.parse_block()?;

        let else_block = if *self.peek() == Token::Else {
            self.next_token()?;

            if *self.peek() == Token::If {
                vec![self.parse_if()?]
            } else {
                self.parse_block()?
            }
        } else {
            Vec::new()
        };

        let id = self.alloc_term_id();
        let term = Term::new(
            id,
            TermOp::Branch {
                condition,
                then_block,
                else_block,
            },
        )
        .with_inputs(vec![condition]);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_for(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'for'

        let var_name = match self.peek() {
            Token::Ident(s) => s.clone(),
            _ => return Err(Error::ParseError("Expected loop variable".to_string())),
        };
        self.next_token()?;

        self.expect(Token::In)?;

        let iterable = self.parse_expr()?;
        let body = self.parse_block()?;

        let id = self.alloc_term_id();
        let term = Term::new(
            id,
            TermOp::ForLoop {
                var_name,
                iterable,
                body,
            },
        )
        .with_inputs(vec![iterable]);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_while(&mut self) -> Result<TermId> {
        self.next_token()?; // consume 'while'

        let condition = self.parse_expr()?;
        let body = self.parse_block()?;

        let id = self.alloc_term_id();
        let term = Term::new(
            id,
            TermOp::WhileLoop {
                condition,
                body,
            },
        )
        .with_inputs(vec![condition]);
        self.program.add_term(term);

        Ok(id)
    }

    fn parse_block(&mut self) -> Result<Vec<TermId>> {
        self.expect(Token::LBrace)?;

        let mut statements = Vec::new();

        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            statements.push(self.parse_statement()?);
        }

        self.expect(Token::RBrace)?;

        Ok(statements)
    }

    fn parse_list(&mut self) -> Result<TermId> {
        self.next_token()?; // consume '['

        let mut elements = Vec::new();

        while *self.peek() != Token::RBracket {
            elements.push(self.parse_expr()?);

            if *self.peek() == Token::Comma {
                self.next_token()?;
            }
        }

        self.expect(Token::RBracket)?;

        let id = self.alloc_term_id();
        let term = Term::new(id, TermOp::MakeList(elements.clone())).with_inputs(elements);
        self.program.add_term(term);

        Ok(id)
    }
}
