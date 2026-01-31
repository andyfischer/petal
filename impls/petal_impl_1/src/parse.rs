//! Parser for Petal source code

use smallvec::{smallvec, SmallVec};

use crate::error::{Error, Result};
use crate::program::{ConstantValue, Program, ProgramKey, StateKey, TermId, TermOp, UserFunction};
use crate::source_map::{SourcePosition, SourceSpan};

/// Token types
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
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
    Fn,
    If,
    Else,
    For,
    While,
    In,
    Return,
    State,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Bang,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    Semicolon,

    // End of file
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: SourceSpan,
}

/// Lexer for tokenizing source code
pub struct Lexer<'a> {
    source: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    line: u32,
    column: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.char_indices().peekable(),
            line: 1,
            column: 1,
        }
    }

    fn current_position(&self) -> SourcePosition {
        let offset = self.chars.clone().next().map(|(i, _)| i as u32).unwrap_or(self.source.len() as u32);
        SourcePosition {
            line: self.line,
            column: self.column,
            offset,
        }
    }

    fn advance(&mut self) -> Option<(usize, char)> {
        let result = self.chars.next();
        if let Some((_, c)) = result {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        result
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|(_, c)| *c)
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else if c == '/' {
                // Check for comments
                let mut chars_clone = self.chars.clone();
                chars_clone.next();
                if let Some((_, '/')) = chars_clone.next() {
                    // Single-line comment
                    self.advance(); // skip first /
                    self.advance(); // skip second /
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                } else if let Some((_, '*')) = chars_clone.next() {
                    // Multi-line comment
                    self.advance(); // skip /
                    self.advance(); // skip *
                    loop {
                        match self.advance() {
                            Some((_, '*')) => {
                                if self.peek() == Some('/') {
                                    self.advance();
                                    break;
                                }
                            }
                            None => break,
                            _ => {}
                        }
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    fn read_number(&mut self, start: usize) -> Token {
        let start_pos = self.current_position();
        let mut end = start;
        let mut is_float = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                end = self.advance().unwrap().0;
            } else if c == '.' && !is_float {
                // Check if next char is a digit (to distinguish from method calls)
                let mut chars_clone = self.chars.clone();
                chars_clone.next();
                if let Some((_, next_c)) = chars_clone.next() {
                    if next_c.is_ascii_digit() {
                        is_float = true;
                        end = self.advance().unwrap().0;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        let end_pos = self.current_position();
        let text = &self.source[start..=end];

        let kind = if is_float {
            TokenKind::Float(text.parse().unwrap_or(0.0))
        } else {
            TokenKind::Int(text.parse().unwrap_or(0))
        };

        Token {
            kind,
            span: SourceSpan::new(start_pos, end_pos),
        }
    }

    fn read_string(&mut self) -> Token {
        let start_pos = self.current_position();
        // Note: opening quote already consumed by next_token

        let mut value = String::new();
        loop {
            match self.advance() {
                Some((_, '"')) => break,
                Some((_, '\\')) => {
                    if let Some((_, c)) = self.advance() {
                        match c {
                            'n' => value.push('\n'),
                            't' => value.push('\t'),
                            'r' => value.push('\r'),
                            '\\' => value.push('\\'),
                            '"' => value.push('"'),
                            _ => value.push(c),
                        }
                    }
                }
                Some((_, c)) => value.push(c),
                None => break,
            }
        }

        let end_pos = self.current_position();
        Token {
            kind: TokenKind::String(value),
            span: SourceSpan::new(start_pos, end_pos),
        }
    }

    fn read_ident(&mut self, start: usize) -> Token {
        let start_pos = self.current_position();
        let mut end = start;

        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                end = self.advance().unwrap().0;
            } else {
                break;
            }
        }

        let end_pos = self.current_position();
        let text = &self.source[start..=end];

        let kind = match text {
            "let" => TokenKind::Let,
            "fn" => TokenKind::Fn,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "for" => TokenKind::For,
            "while" => TokenKind::While,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "state" => TokenKind::State,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "nil" => TokenKind::Nil,
            _ => TokenKind::Ident(text.to_string()),
        };

        Token {
            kind,
            span: SourceSpan::new(start_pos, end_pos),
        }
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_whitespace();

        let start_pos = self.current_position();

        let Some((start, c)) = self.advance() else {
            return Token {
                kind: TokenKind::Eof,
                span: SourceSpan::new(start_pos, start_pos),
            };
        };

        let kind = match c {
            '0'..='9' => return self.read_number(start),
            '"' => {
                // Put the quote back and read string
                return self.read_string();
            }
            'a'..='z' | 'A'..='Z' | '_' => return self.read_ident(start),

            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,

            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    TokenKind::And
                } else {
                    TokenKind::And // Single & also works
                }
            }
            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    TokenKind::Or
                } else {
                    TokenKind::Or // Single | also works
                }
            }

            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            '.' => TokenKind::Dot,
            ';' => TokenKind::Semicolon,

            _ => {
                return Token {
                    kind: TokenKind::Eof,
                    span: SourceSpan::new(start_pos, self.current_position()),
                };
            }
        };

        Token {
            kind,
            span: SourceSpan::new(start_pos, self.current_position()),
        }
    }
}

/// Parser for Petal source code
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
    program: Program,
    state_counter: u64,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, program_key: ProgramKey) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token();

        Self {
            lexer,
            current,
            program: Program::new(program_key),
            state_counter: 0,
        }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.current.kind) == std::mem::discriminant(kind)
    }

    fn expect(&mut self, kind: TokenKind) -> Result<Token> {
        if self.check(&kind) {
            let token = self.current.clone();
            self.advance();
            Ok(token)
        } else {
            Err(Error::Parse {
                message: format!("Expected {:?}, got {:?}", kind, self.current.kind),
                span: self.current.span,
            })
        }
    }

    fn next_state_key(&mut self) -> StateKey {
        self.state_counter += 1;
        StateKey(self.state_counter)
    }

    pub fn parse(&mut self) -> Result<Program> {
        let mut statements = Vec::new();

        while !self.check(&TokenKind::Eof) {
            let stmt = self.parse_statement()?;
            statements.push(stmt);
        }

        // The program result is the last statement, or nil if empty
        let entry = if statements.is_empty() {
            let const_id = self.program.constants.add(ConstantValue::Nil);
            self.program.add_term(TermOp::Constant(const_id), smallvec![])
        } else if statements.len() == 1 {
            statements[0]
        } else {
            // Create a block containing all statements
            let inputs: SmallVec<[TermId; 4]> = statements.into_iter().collect();
            self.program.add_term(TermOp::Block, inputs)
        };

        self.program.set_entry(entry);
        let program_id = self.program.id;
        Ok(std::mem::replace(&mut self.program, Program::new(program_id)))
    }

    fn parse_statement(&mut self) -> Result<TermId> {
        match &self.current.kind {
            TokenKind::Let => self.parse_let(),
            TokenKind::Fn => self.parse_fn_def(),
            TokenKind::State => self.parse_state(),
            TokenKind::Return => self.parse_return(),
            TokenKind::For => self.parse_for(),
            TokenKind::While => self.parse_while(),
            _ => self.parse_expression(),
        }
    }

    fn parse_let(&mut self) -> Result<TermId> {
        self.advance(); // consume 'let'

        let name = match &self.current.kind {
            TokenKind::Ident(n) => n.clone(),
            _ => {
                return Err(Error::Parse {
                    message: "Expected identifier after 'let'".to_string(),
                    span: self.current.span,
                });
            }
        };
        self.advance();

        self.expect(TokenKind::Eq)?;
        let value = self.parse_expression()?;

        Ok(self.program.add_term(TermOp::Let { name }, smallvec![value]))
    }

    fn parse_state(&mut self) -> Result<TermId> {
        self.advance(); // consume 'state'

        let name = match &self.current.kind {
            TokenKind::Ident(n) => n.clone(),
            _ => {
                return Err(Error::Parse {
                    message: "Expected identifier after 'state'".to_string(),
                    span: self.current.span,
                });
            }
        };
        self.advance();

        self.expect(TokenKind::Eq)?;
        let init_value = self.parse_expression()?;
        let key = self.next_state_key();

        Ok(self.program.add_term(
            TermOp::StateDecl { name, key },
            smallvec![init_value],
        ))
    }

    fn parse_fn_def(&mut self) -> Result<TermId> {
        self.advance(); // consume 'fn'

        let name = match &self.current.kind {
            TokenKind::Ident(n) => n.clone(),
            _ => {
                return Err(Error::Parse {
                    message: "Expected function name".to_string(),
                    span: self.current.span,
                });
            }
        };
        self.advance();

        self.expect(TokenKind::LParen)?;

        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            if let TokenKind::Ident(param) = &self.current.kind {
                params.push(param.clone());
                self.advance();

                if !self.check(&TokenKind::RParen) {
                    self.expect(TokenKind::Comma)?;
                }
            } else {
                break;
            }
        }

        self.expect(TokenKind::RParen)?;
        let body = self.parse_block()?;

        let func = UserFunction {
            name: name.clone(),
            params: params.clone(),
            body,
        };
        self.program.add_function(func);

        Ok(self.program.add_term(
            TermOp::FnDef { name, params, body },
            smallvec![],
        ))
    }

    fn parse_return(&mut self) -> Result<TermId> {
        self.advance(); // consume 'return'

        if self.check(&TokenKind::RBrace) || self.check(&TokenKind::Eof) {
            let const_id = self.program.constants.add(ConstantValue::Nil);
            let nil = self.program.add_term(TermOp::Constant(const_id), smallvec![]);
            Ok(self.program.add_term(TermOp::Return, smallvec![nil]))
        } else {
            let value = self.parse_expression()?;
            Ok(self.program.add_term(TermOp::Return, smallvec![value]))
        }
    }

    fn parse_for(&mut self) -> Result<TermId> {
        self.advance(); // consume 'for'

        let var_name = match &self.current.kind {
            TokenKind::Ident(n) => n.clone(),
            _ => {
                return Err(Error::Parse {
                    message: "Expected loop variable".to_string(),
                    span: self.current.span,
                });
            }
        };
        self.advance();

        self.expect(TokenKind::In)?;
        let iterator = self.parse_expression()?;
        let body = self.parse_block()?;

        Ok(self.program.add_term(
            TermOp::ForLoop { var_name, body },
            smallvec![iterator],
        ))
    }

    fn parse_while(&mut self) -> Result<TermId> {
        self.advance(); // consume 'while'

        let condition = self.parse_expression()?;
        let body = self.parse_block()?;

        Ok(self.program.add_term(
            TermOp::WhileLoop { body },
            smallvec![condition],
        ))
    }

    fn parse_block(&mut self) -> Result<TermId> {
        self.expect(TokenKind::LBrace)?;

        let mut statements = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let stmt = self.parse_statement()?;
            statements.push(stmt);
        }

        self.expect(TokenKind::RBrace)?;

        if statements.is_empty() {
            let const_id = self.program.constants.add(ConstantValue::Nil);
            Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
        } else if statements.len() == 1 {
            Ok(statements[0])
        } else {
            let inputs: SmallVec<[TermId; 4]> = statements.into_iter().collect();
            Ok(self.program.add_term(TermOp::Block, inputs))
        }
    }

    fn parse_expression(&mut self) -> Result<TermId> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<TermId> {
        let expr = self.parse_or()?;

        if self.check(&TokenKind::Eq) {
            self.advance();
            let value = self.parse_assignment()?;

            // Check if expr is a variable reference
            if let Some(term) = self.program.get_term(expr) {
                if let TermOp::Var(name) = &term.op {
                    let name = name.clone();
                    return Ok(self.program.add_term(TermOp::Assign { name }, smallvec![value]));
                }
            }

            return Err(Error::Parse {
                message: "Invalid assignment target".to_string(),
                span: self.current.span,
            });
        }

        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<TermId> {
        let mut left = self.parse_and()?;

        while self.check(&TokenKind::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = self.program.add_term(TermOp::Or, smallvec![left, right]);
        }

        Ok(left)
    }

    fn parse_and(&mut self) -> Result<TermId> {
        let mut left = self.parse_equality()?;

        while self.check(&TokenKind::And) {
            self.advance();
            let right = self.parse_equality()?;
            left = self.program.add_term(TermOp::And, smallvec![left, right]);
        }

        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<TermId> {
        let mut left = self.parse_comparison()?;

        loop {
            if self.check(&TokenKind::EqEq) {
                self.advance();
                let right = self.parse_comparison()?;
                left = self.program.add_term(TermOp::Eq, smallvec![left, right]);
            } else if self.check(&TokenKind::BangEq) {
                self.advance();
                let right = self.parse_comparison()?;
                left = self.program.add_term(TermOp::NotEq, smallvec![left, right]);
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<TermId> {
        let mut left = self.parse_term()?;

        loop {
            if self.check(&TokenKind::Lt) {
                self.advance();
                let right = self.parse_term()?;
                left = self.program.add_term(TermOp::Lt, smallvec![left, right]);
            } else if self.check(&TokenKind::LtEq) {
                self.advance();
                let right = self.parse_term()?;
                left = self.program.add_term(TermOp::LtEq, smallvec![left, right]);
            } else if self.check(&TokenKind::Gt) {
                self.advance();
                let right = self.parse_term()?;
                left = self.program.add_term(TermOp::Gt, smallvec![left, right]);
            } else if self.check(&TokenKind::GtEq) {
                self.advance();
                let right = self.parse_term()?;
                left = self.program.add_term(TermOp::GtEq, smallvec![left, right]);
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_term(&mut self) -> Result<TermId> {
        let mut left = self.parse_factor()?;

        loop {
            if self.check(&TokenKind::Plus) {
                self.advance();
                let right = self.parse_factor()?;
                left = self.program.add_term(TermOp::Add, smallvec![left, right]);
            } else if self.check(&TokenKind::Minus) {
                self.advance();
                let right = self.parse_factor()?;
                left = self.program.add_term(TermOp::Sub, smallvec![left, right]);
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<TermId> {
        let mut left = self.parse_unary()?;

        loop {
            if self.check(&TokenKind::Star) {
                self.advance();
                let right = self.parse_unary()?;
                left = self.program.add_term(TermOp::Mul, smallvec![left, right]);
            } else if self.check(&TokenKind::Slash) {
                self.advance();
                let right = self.parse_unary()?;
                left = self.program.add_term(TermOp::Div, smallvec![left, right]);
            } else if self.check(&TokenKind::Percent) {
                self.advance();
                let right = self.parse_unary()?;
                left = self.program.add_term(TermOp::Mod, smallvec![left, right]);
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<TermId> {
        if self.check(&TokenKind::Minus) {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(self.program.add_term(TermOp::Neg, smallvec![operand]));
        }

        if self.check(&TokenKind::Bang) {
            self.advance();
            let operand = self.parse_unary()?;
            return Ok(self.program.add_term(TermOp::Not, smallvec![operand]));
        }

        self.parse_call()
    }

    fn parse_call(&mut self) -> Result<TermId> {
        let mut expr = self.parse_primary()?;

        loop {
            if self.check(&TokenKind::LParen) {
                self.advance();

                let mut args = Vec::new();
                while !self.check(&TokenKind::RParen) {
                    args.push(self.parse_expression()?);
                    if !self.check(&TokenKind::RParen) {
                        self.expect(TokenKind::Comma)?;
                    }
                }
                self.expect(TokenKind::RParen)?;

                // Check if this is a function call
                if let Some(term) = self.program.get_term(expr) {
                    if let TermOp::Var(name) = &term.op {
                        let name = name.clone();
                        let arg_count = args.len();
                        let inputs: SmallVec<[TermId; 4]> = args.into_iter().collect();
                        expr = self.program.add_term(
                            TermOp::Call { function: name, arg_count },
                            inputs,
                        );
                        continue;
                    }
                }

                // Generic call
                let arg_count = args.len();
                let mut inputs: SmallVec<[TermId; 4]> = smallvec![expr];
                inputs.extend(args);
                expr = self.program.add_term(
                    TermOp::Call { function: String::new(), arg_count },
                    inputs,
                );
            } else if self.check(&TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expression()?;
                self.expect(TokenKind::RBracket)?;
                expr = self.program.add_term(TermOp::Index, smallvec![expr, index]);
            } else if self.check(&TokenKind::Dot) {
                self.advance();
                if let TokenKind::Ident(name) = &self.current.kind {
                    let name = name.clone();
                    self.advance();
                    expr = self.program.add_term(TermOp::Field { name }, smallvec![expr]);
                } else {
                    return Err(Error::Parse {
                        message: "Expected field name after '.'".to_string(),
                        span: self.current.span,
                    });
                }
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<TermId> {
        match &self.current.kind {
            TokenKind::Int(n) => {
                let n = *n;
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::Int(n));
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::Float(n) => {
                let n = *n;
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::Float(n));
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::String(s) => {
                let s = s.clone();
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::String(s));
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::True => {
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::Bool(true));
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::False => {
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::Bool(false));
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::Nil => {
                self.advance();
                let const_id = self.program.constants.add(ConstantValue::Nil);
                Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(self.program.add_term(TermOp::Var(name), smallvec![]))
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::LBrace => {
                // Could be a block or a map
                self.parse_block_or_map()
            }
            TokenKind::LBracket => {
                self.parse_list()
            }
            TokenKind::If => {
                self.parse_if()
            }
            TokenKind::For => {
                self.parse_for()
            }
            TokenKind::While => {
                self.parse_while()
            }
            _ => {
                Err(Error::Parse {
                    message: format!("Unexpected token: {:?}", self.current.kind),
                    span: self.current.span,
                })
            }
        }
    }

    fn parse_list(&mut self) -> Result<TermId> {
        self.expect(TokenKind::LBracket)?;

        let mut elements = Vec::new();
        while !self.check(&TokenKind::RBracket) && !self.check(&TokenKind::Eof) {
            elements.push(self.parse_expression()?);
            if !self.check(&TokenKind::RBracket) {
                self.expect(TokenKind::Comma)?;
            }
        }

        self.expect(TokenKind::RBracket)?;

        let inputs: SmallVec<[TermId; 4]> = elements.into_iter().collect();
        Ok(self.program.add_term(TermOp::List, inputs))
    }

    fn parse_block_or_map(&mut self) -> Result<TermId> {
        self.expect(TokenKind::LBrace)?;

        // Empty braces = empty map
        if self.check(&TokenKind::RBrace) {
            self.advance();
            return Ok(self.program.add_term(TermOp::Map, smallvec![]));
        }

        // Check if this looks like a map (identifier followed by colon)
        let is_map = matches!(&self.current.kind, TokenKind::Ident(_)) && {
            // Peek ahead - the next token after the identifier should be a colon
            let mut lexer_clone = self.lexer.clone();
            matches!(lexer_clone.next_token().kind, TokenKind::Colon)
        };

        if is_map {
            self.parse_map_contents()
        } else {
            self.parse_block_contents()
        }
    }

    fn parse_map_contents(&mut self) -> Result<TermId> {
        let mut keys = Vec::new();
        let mut values = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let key = match &self.current.kind {
                TokenKind::Ident(k) => k.clone(),
                TokenKind::String(k) => k.clone(),
                _ => {
                    return Err(Error::Parse {
                        message: "Expected map key".to_string(),
                        span: self.current.span,
                    });
                }
            };
            self.advance();

            self.expect(TokenKind::Colon)?;
            let value = self.parse_expression()?;

            // Store key as constant
            let key_const = self.program.constants.add(ConstantValue::String(key));
            let key_term = self.program.add_term(TermOp::Constant(key_const), smallvec![]);

            keys.push(key_term);
            values.push(value);

            if !self.check(&TokenKind::RBrace) {
                if self.check(&TokenKind::Comma) {
                    self.advance();
                }
            }
        }

        self.expect(TokenKind::RBrace)?;

        // Interleave keys and values
        let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
        for (k, v) in keys.into_iter().zip(values) {
            inputs.push(k);
            inputs.push(v);
        }

        Ok(self.program.add_term(TermOp::Map, inputs))
    }

    fn parse_block_contents(&mut self) -> Result<TermId> {
        let mut statements = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let stmt = self.parse_statement()?;
            statements.push(stmt);
        }

        self.expect(TokenKind::RBrace)?;

        if statements.is_empty() {
            let const_id = self.program.constants.add(ConstantValue::Nil);
            Ok(self.program.add_term(TermOp::Constant(const_id), smallvec![]))
        } else if statements.len() == 1 {
            Ok(statements[0])
        } else {
            let inputs: SmallVec<[TermId; 4]> = statements.into_iter().collect();
            Ok(self.program.add_term(TermOp::Block, inputs))
        }
    }

    fn parse_if(&mut self) -> Result<TermId> {
        self.advance(); // consume 'if'

        let condition = self.parse_expression()?;
        let then_branch = self.parse_block()?;

        let else_branch = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                self.parse_if()?
            } else {
                self.parse_block()?
            }
        } else {
            let const_id = self.program.constants.add(ConstantValue::Nil);
            self.program.add_term(TermOp::Constant(const_id), smallvec![])
        };

        Ok(self.program.add_term(
            TermOp::If,
            smallvec![condition, then_branch, else_branch],
        ))
    }
}

impl Clone for Lexer<'_> {
    fn clone(&self) -> Self {
        Self {
            source: self.source,
            chars: self.source[self.chars.clone().next().map(|(i, _)| i).unwrap_or(self.source.len())..].char_indices().peekable(),
            line: self.line,
            column: self.column,
        }
    }
}
