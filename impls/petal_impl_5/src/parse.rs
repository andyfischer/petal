use crate::{Program, Term, TermOp, Value};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    String(String),
    Ident(String),
    True,
    False,
    Nil,

    // Keywords
    Let,
    Fn,
    If,
    Else,
    For,
    In,
    State,
    Return,
    While,
    Range,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Lte,
    Gte,
    And,
    Or,
    Not,
    Dot,
    Arrow,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,

    Eof,
}

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Lexer {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn current(&self) -> Option<char> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn peek(&self) -> Option<char> {
        if self.pos + 1 < self.input.len() {
            Some(self.input[self.pos + 1])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<char> {
        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.current() {
            if c.is_whitespace() {
                self.advance();
            } else if c == '#' {
                // Skip comments
                while let Some(c) = self.current() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self) -> Result<String, String> {
        let quote = self.advance(); // consume opening quote
        let mut result = String::new();

        while let Some(c) = self.current() {
            if c == '"' || c == '\'' {
                self.advance();
                return Ok(result);
            }
            if c == '\\' {
                self.advance();
                match self.current() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('"') => result.push('"'),
                    Some('\'') => result.push('\''),
                    Some(c) => result.push(c),
                    None => return Err("Unterminated string".to_string()),
                }
                self.advance();
            } else {
                result.push(c);
                self.advance();
            }
        }

        Err("Unterminated string".to_string())
    }

    fn read_number(&mut self) -> Result<f64, String> {
        let mut num_str = String::new();
        let mut has_dot = false;

        while let Some(c) = self.current() {
            if c.is_ascii_digit() {
                num_str.push(c);
                self.advance();
            } else if c == '.' && !has_dot {
                has_dot = true;
                num_str.push(c);
                self.advance();
            } else {
                break;
            }
        }

        num_str.parse().map_err(|_| "Invalid number".to_string())
    }

    fn read_ident(&mut self) -> String {
        let mut ident = String::new();

        while let Some(c) = self.current() {
            if c.is_alphanumeric() || c == '_' {
                ident.push(c);
                self.advance();
            } else {
                break;
            }
        }

        ident
    }

    pub fn next_token(&mut self) -> Result<Token, String> {
        self.skip_whitespace();

        match self.current() {
            None => Ok(Token::Eof),
            Some(c) if c.is_ascii_digit() => {
                let num = self.read_number()?;
                Ok(Token::Number(num))
            }
            Some('"') | Some('\'') => {
                let s = self.read_string()?;
                Ok(Token::String(s))
            }
            Some(c) if c.is_alphabetic() || c == '_' => {
                let ident = self.read_ident();
                Ok(match ident.as_str() {
                    "let" => Token::Let,
                    "fn" => Token::Fn,
                    "if" => Token::If,
                    "else" => Token::Else,
                    "for" => Token::For,
                    "in" => Token::In,
                    "state" => Token::State,
                    "return" => Token::Return,
                    "while" => Token::While,
                    "range" => Token::Range,
                    "true" => Token::True,
                    "false" => Token::False,
                    "nil" => Token::Nil,
                    _ => Token::Ident(ident),
                })
            }
            Some('+') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::PlusEq)
                } else {
                    Ok(Token::Plus)
                }
            }
            Some('-') => {
                self.advance();
                if self.current() == Some('>') {
                    self.advance();
                    Ok(Token::Arrow)
                } else if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::MinusEq)
                } else if self.current().map_or(false, |c| c.is_ascii_digit()) {
                    let num = self.read_number()?;
                    Ok(Token::Number(-num))
                } else {
                    Ok(Token::Minus)
                }
            }
            Some('*') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::StarEq)
                } else {
                    Ok(Token::Star)
                }
            }
            Some('/') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::SlashEq)
                } else {
                    Ok(Token::Slash)
                }
            }
            Some('%') => {
                self.advance();
                Ok(Token::Percent)
            }
            Some('=') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::EqEq)
                } else {
                    Ok(Token::Eq)
                }
            }
            Some('!') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::NotEq)
                } else {
                    Ok(Token::Not)
                }
            }
            Some('<') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::Lte)
                } else {
                    Ok(Token::Lt)
                }
            }
            Some('>') => {
                self.advance();
                if self.current() == Some('=') {
                    self.advance();
                    Ok(Token::Gte)
                } else {
                    Ok(Token::Gt)
                }
            }
            Some('&') => {
                self.advance();
                if self.current() == Some('&') {
                    self.advance();
                    Ok(Token::And)
                } else {
                    Err("Unexpected &".to_string())
                }
            }
            Some('|') => {
                self.advance();
                if self.current() == Some('|') {
                    self.advance();
                    Ok(Token::Or)
                } else {
                    Err("Unexpected |".to_string())
                }
            }
            Some('.') => {
                self.advance();
                Ok(Token::Dot)
            }
            Some('(') => {
                self.advance();
                Ok(Token::LParen)
            }
            Some(')') => {
                self.advance();
                Ok(Token::RParen)
            }
            Some('{') => {
                self.advance();
                Ok(Token::LBrace)
            }
            Some('}') => {
                self.advance();
                Ok(Token::RBrace)
            }
            Some('[') => {
                self.advance();
                Ok(Token::LBracket)
            }
            Some(']') => {
                self.advance();
                Ok(Token::RBracket)
            }
            Some(',') => {
                self.advance();
                Ok(Token::Comma)
            }
            Some(';') => {
                self.advance();
                Ok(Token::Semicolon)
            }
            Some(':') => {
                self.advance();
                Ok(Token::Colon)
            }
            Some(c) => {
                self.advance();
                Err(format!("Unexpected character: {}", c))
            }
        }
    }
}

pub struct Parser {
    tokens: VecDeque<Token>,
    terms: Vec<Term>,
}

impl Parser {
    pub fn new(tokens: VecDeque<Token>) -> Self {
        Parser {
            tokens,
            terms: Vec::new(),
        }
    }

    fn current(&self) -> Token {
        self.tokens.front().cloned().unwrap_or(Token::Eof)
    }

    fn advance(&mut self) -> Token {
        self.tokens.pop_front().unwrap_or(Token::Eof)
    }

    fn expect(&mut self, expected: Token) -> Result<(), String> {
        if std::mem::discriminant(&self.current()) == std::mem::discriminant(&expected) {
            self.advance();
            Ok(())
        } else {
            Err(format!("Expected {:?}, found {:?}", expected, self.current()))
        }
    }

    fn add_term(&mut self, op: TermOp, inputs: Vec<usize>) -> usize {
        let id = self.terms.len();
        self.terms.push(Term {
            id,
            op,
            inputs,
        });
        id
    }

    fn parse_program(&mut self) -> Result<usize, String> {
        self.parse_statements()
    }

    fn parse_statements(&mut self) -> Result<usize, String> {
        // Check for let/state/fn bindings which scope over the rest
        if self.current() == Token::Let {
            return self.parse_let_scoped();
        }

        if self.current() == Token::State {
            return self.parse_state_scoped();
        }

        if self.current() == Token::Fn {
            return self.parse_fn();
        }

        // Otherwise parse statements sequentially
        let mut terms = Vec::new();

        while self.current() != Token::Eof && self.current() != Token::RBrace {
            // Check for let/state/fn in the middle
            if self.current() == Token::Let {
                // Parse let with the rest as its body
                let let_term = self.parse_let_scoped()?;
                // If we have prior terms, sequence them with the let
                if terms.is_empty() {
                    return Ok(let_term);
                } else {
                    terms.push(let_term);
                    return Ok(self.add_term(TermOp::Sequence { terms }, vec![]));
                }
            }

            if self.current() == Token::State {
                let state_term = self.parse_state_scoped()?;
                // If we have prior terms, sequence them with the state
                if terms.is_empty() {
                    return Ok(state_term);
                } else {
                    terms.push(state_term);
                    return Ok(self.add_term(TermOp::Sequence { terms }, vec![]));
                }
            }

            if self.current() == Token::Fn {
                let fn_term = self.parse_fn()?;
                // If we have prior terms, sequence them with the function
                if terms.is_empty() {
                    return Ok(fn_term);
                } else {
                    terms.push(fn_term);
                    return Ok(self.add_term(TermOp::Sequence { terms }, vec![]));
                }
            }

            let term = self.parse_statement()?;
            terms.push(term);
        }

        if terms.is_empty() {
            Ok(self.add_term(TermOp::Constant(crate::Value::Nil), vec![]))
        } else if terms.len() == 1 {
            Ok(terms[0])
        } else {
            Ok(self.add_term(TermOp::Sequence { terms }, vec![]))
        }
    }

    fn parse_let_scoped(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'let'

        let var_name = match self.advance() {
            Token::Ident(n) => n,
            _ => return Err("Expected identifier after let".to_string()),
        };

        self.expect(Token::Eq)?;

        let init_term = self.parse_expr()?;

        // The body is everything that follows
        let body_term = self.parse_statements()?;

        Ok(self.add_term(
            TermOp::Let {
                var: var_name,
                init: init_term,
                body: body_term,
            },
            vec![],
        ))
    }

    fn parse_state_scoped(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'state'

        let var_name = match self.advance() {
            Token::Ident(n) => n,
            _ => return Err("Expected identifier after state".to_string()),
        };

        self.expect(Token::Eq)?;

        let init_term = self.parse_expr()?;

        // Generate a unique state ID based on position
        let state_id = (self.terms.len() as u64) * 31 + (var_name.len() as u64);

        // The body is everything that follows
        let body_term = self.parse_statements()?;

        Ok(self.add_term(
            TermOp::StateDef {
                var: var_name,
                init: init_term,
                body: body_term,
                state_id,
            },
            vec![],
        ))
    }

    fn parse_statement(&mut self) -> Result<usize, String> {
        match self.current() {
            Token::Let => self.parse_let(),
            Token::State => self.parse_state(),
            Token::Fn => self.parse_fn(),
            Token::Return => self.parse_return(),
            Token::If => self.parse_if(),
            Token::For => self.parse_for(),
            Token::While => self.parse_while(),
            Token::Ident(_) => {
                // Check for mutation operators
                let name = match self.current() {
                    Token::Ident(n) => n.clone(),
                    _ => return self.parse_expr(),
                };

                let saved_pos = self.tokens.clone();
                self.advance(); // consume identifier

                // Check if followed by mutation operator
                match self.current() {
                    Token::PlusEq => {
                        self.advance();
                        let value = self.parse_expr()?;
                        Ok(self.add_term(
                            TermOp::Mutate {
                                var: name,
                                op: "+".to_string(),
                                value,
                            },
                            vec![],
                        ))
                    }
                    Token::MinusEq => {
                        self.advance();
                        let value = self.parse_expr()?;
                        Ok(self.add_term(
                            TermOp::Mutate {
                                var: name,
                                op: "-".to_string(),
                                value,
                            },
                            vec![],
                        ))
                    }
                    Token::StarEq => {
                        self.advance();
                        let value = self.parse_expr()?;
                        Ok(self.add_term(
                            TermOp::Mutate {
                                var: name,
                                op: "*".to_string(),
                                value,
                            },
                            vec![],
                        ))
                    }
                    Token::SlashEq => {
                        self.advance();
                        let value = self.parse_expr()?;
                        Ok(self.add_term(
                            TermOp::Mutate {
                                var: name,
                                op: "/".to_string(),
                                value,
                            },
                            vec![],
                        ))
                    }
                    _ => {
                        // Not a mutation, restore and parse as expression
                        self.tokens = saved_pos;
                        self.parse_expr()
                    }
                }
            }
            _ => self.parse_expr(),
        }
    }

    fn parse_let(&mut self) -> Result<usize, String> {
        // Let in statement context - delegate to scoped version
        self.parse_let_scoped()
    }

    fn parse_state(&mut self) -> Result<usize, String> {
        // State in statement context - delegate to scoped version
        self.parse_state_scoped()
    }

    fn parse_fn(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'fn'

        let name = match self.advance() {
            Token::Ident(n) => n,
            _ => return Err("Expected function name".to_string()),
        };

        self.expect(Token::LParen)?;

        let mut params = Vec::new();
        while self.current() != Token::RParen && self.current() != Token::Eof {
            match self.advance() {
                Token::Ident(p) => params.push(p),
                _ => return Err("Expected parameter name".to_string()),
            }
            if self.current() == Token::Comma {
                self.advance();
            }
        }

        self.expect(Token::RParen)?;
        self.expect(Token::LBrace)?;

        let body = self.parse_program()?;

        self.expect(Token::RBrace)?;

        // Parse the rest of the program as the continuation after function definition
        let next = self.parse_statements()?;

        Ok(self.add_term(
            TermOp::FunctionDef {
                name,
                params,
                body,
                next,
            },
            vec![],
        ))
    }

    fn parse_return(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'return'
        self.parse_expr()
    }

    fn parse_if(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'if'

        let cond = self.parse_expr()?;

        self.expect(Token::LBrace)?;
        let then_term = self.parse_program()?;
        self.expect(Token::RBrace)?;

        let else_term = if self.current() == Token::Else {
            self.advance();
            self.expect(Token::LBrace)?;
            let e = self.parse_program()?;
            self.expect(Token::RBrace)?;
            e
        } else {
            self.add_term(TermOp::Constant(Value::Nil), vec![])
        };

        let if_term = self.add_term(
            TermOp::Branch {
                then_id: then_term,
                else_id: else_term,
            },
            vec![cond],
        );

        Ok(if_term)
    }

    fn parse_for(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'for'

        let var_name = match self.advance() {
            Token::Ident(v) => v,
            _ => return Err("Expected variable name in for loop".to_string()),
        };

        self.expect(Token::In)?;

        let iter_term = self.parse_expr()?;

        self.expect(Token::LBrace)?;
        let body_term = self.parse_program()?;
        self.expect(Token::RBrace)?;

        Ok(self.add_term(
            TermOp::For {
                var: var_name,
                iter: iter_term,
                body: body_term,
            },
            vec![],
        ))
    }

    fn parse_while(&mut self) -> Result<usize, String> {
        self.advance(); // consume 'while'

        let cond_term = self.parse_expr()?;

        self.expect(Token::LBrace)?;
        let body_term = self.parse_program()?;
        self.expect(Token::RBrace)?;

        Ok(self.add_term(
            TermOp::While {
                cond: cond_term,
                body: body_term,
            },
            vec![],
        ))
    }

    fn parse_expr(&mut self) -> Result<usize, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<usize, String> {
        let mut left = self.parse_and()?;

        while self.current() == Token::Or {
            self.advance();
            let right = self.parse_and()?;
            left = self.add_term(TermOp::Or, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_and(&mut self) -> Result<usize, String> {
        let mut left = self.parse_equality()?;

        while self.current() == Token::And {
            self.advance();
            let right = self.parse_equality()?;
            left = self.add_term(TermOp::And, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<usize, String> {
        let mut left = self.parse_comparison()?;

        while matches!(self.current(), Token::EqEq | Token::NotEq) {
            let op = match self.advance() {
                Token::EqEq => TermOp::Eq,
                Token::NotEq => {
                    let eq_term = self.parse_comparison()?;
                    let eq = self.add_term(TermOp::Eq, vec![left, eq_term]);
                    return Ok(self.add_term(TermOp::Not, vec![eq]));
                }
                _ => unreachable!(),
            };

            let right = self.parse_comparison()?;
            left = self.add_term(op, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<usize, String> {
        let mut left = self.parse_additive()?;

        while matches!(self.current(), Token::Lt | Token::Gt | Token::Lte | Token::Gte) {
            let op = match self.advance() {
                Token::Lt => TermOp::Lt,
                Token::Gt => TermOp::Gt,
                Token::Lte => TermOp::Lte,
                Token::Gte => TermOp::Gte,
                _ => unreachable!(),
            };

            let right = self.parse_additive()?;
            left = self.add_term(op, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<usize, String> {
        let mut left = self.parse_multiplicative()?;

        while matches!(self.current(), Token::Plus | Token::Minus) {
            let op = match self.advance() {
                Token::Plus => TermOp::Add,
                Token::Minus => TermOp::Sub,
                _ => unreachable!(),
            };

            let right = self.parse_multiplicative()?;
            left = self.add_term(op, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<usize, String> {
        let mut left = self.parse_unary()?;

        while matches!(self.current(), Token::Star | Token::Slash | Token::Percent) {
            let op = match self.advance() {
                Token::Star => TermOp::Mul,
                Token::Slash => TermOp::Div,
                Token::Percent => TermOp::Mod,
                _ => unreachable!(),
            };

            let right = self.parse_unary()?;
            left = self.add_term(op, vec![left, right]);
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<usize, String> {
        match self.current() {
            Token::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(self.add_term(TermOp::Not, vec![expr]))
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                let zero = self.add_term(TermOp::Constant(Value::Int(0)), vec![]);
                Ok(self.add_term(TermOp::Sub, vec![zero, expr]))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<usize, String> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.current() {
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(Token::RBracket)?;
                    expr = self.add_term(TermOp::ListIndex, vec![expr, index]);
                }
                Token::Dot => {
                    self.advance();
                    match self.advance() {
                        Token::Ident(field) => {
                            expr = self.add_term(TermOp::GetField(field), vec![expr]);
                        }
                        _ => return Err("Expected field name after dot".to_string()),
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<usize, String> {
        match self.current() {
            Token::Number(n) => {
                let n = n;
                self.advance();
                if n.fract() == 0.0 {
                    Ok(self.add_term(TermOp::Constant(Value::Int(n as i64)), vec![]))
                } else {
                    Ok(self.add_term(TermOp::Constant(Value::Float(n)), vec![]))
                }
            }
            Token::String(s) => {
                let s = s.clone();
                self.advance();
                Ok(self.add_term(TermOp::Constant(Value::String(s)), vec![]))
            }
            Token::True => {
                self.advance();
                Ok(self.add_term(TermOp::Constant(Value::Bool(true)), vec![]))
            }
            Token::False => {
                self.advance();
                Ok(self.add_term(TermOp::Constant(Value::Bool(false)), vec![]))
            }
            Token::Nil => {
                self.advance();
                Ok(self.add_term(TermOp::Constant(Value::Nil), vec![]))
            }
            Token::Range => {
                self.advance();
                self.expect(Token::LParen)?;
                let arg1 = self.parse_expr()?;
                self.expect(Token::Comma)?;
                let arg2 = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(self.add_term(TermOp::Call("range".to_string()), vec![arg1, arg2]))
            }
            Token::Ident(name) => {
                let name = name.clone();
                self.advance();

                // Check for parenthesized function call: func(arg1, arg2)
                if self.current() == Token::LParen {
                    self.advance();
                    let mut args = Vec::new();

                    while self.current() != Token::RParen && self.current() != Token::Eof {
                        args.push(self.parse_expr()?);
                        if self.current() == Token::Comma {
                            self.advance();
                        }
                    }

                    self.expect(Token::RParen)?;
                    return Ok(self.add_term(TermOp::Call(name), args));
                }

                // Check for function call without parens for known builtins
                if matches!(
                    name.as_str(),
                    "print" | "range" | "len" | "push" | "pop" | "map" | "filter" |
                    "to_string" | "to_int" | "to_float"
                ) {
                    let mut args = Vec::new();

                    // Check if there are arguments
                    if !matches!(
                        self.current(),
                        Token::Eof
                            | Token::Semicolon
                            | Token::Comma
                            | Token::RBrace
                            | Token::RParen
                            | Token::RBracket
                    ) {
                        args.push(self.parse_unary()?);
                        while self.current() == Token::Comma {
                            self.advance();
                            args.push(self.parse_unary()?);
                        }
                    }

                    if !args.is_empty() {
                        return Ok(self.add_term(TermOp::Call(name), args));
                    }
                }

                // If no arguments and not a builtin, treat as variable reference
                Ok(self.add_term(TermOp::Var(name), vec![]))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Token::LBracket => {
                self.advance();
                let mut elements = Vec::new();

                while self.current() != Token::RBracket && self.current() != Token::Eof {
                    elements.push(self.parse_expr()?);
                    if self.current() == Token::Comma {
                        self.advance();
                    }
                }

                self.expect(Token::RBracket)?;

                // For now, flatten list into a sequence of constants
                Ok(self.add_term(TermOp::ListConcat, elements))
            }
            Token::LBrace => {
                // Map literal
                self.advance();
                let mut _pairs = Vec::new();

                while self.current() != Token::RBrace && self.current() != Token::Eof {
                    match self.advance() {
                        Token::Ident(key) => {
                            self.expect(Token::Colon)?;
                            let _value = self.parse_expr()?;
                            _pairs.push((key, _value));
                        }
                        _ => return Err("Expected key in map".to_string()),
                    }
                    if self.current() == Token::Comma {
                        self.advance();
                    }
                }

                self.expect(Token::RBrace)?;

                // For now, return empty map
                Ok(self.add_term(TermOp::Constant(Value::Map(std::rc::Rc::new(
                    std::cell::RefCell::new(std::collections::HashMap::new()),
                ))), vec![]))
            }
            _ => Err(format!("Unexpected token in expression: {:?}", self.current())),
        }
    }
}

pub fn parse(source: &str) -> Result<Program, String> {
    let mut lexer = Lexer::new(source);
    let mut tokens = VecDeque::new();

    loop {
        let token = lexer.next_token()?;
        if token == Token::Eof {
            tokens.push_back(token);
            break;
        }
        tokens.push_back(token);
    }

    let mut parser = Parser::new(tokens);
    let entry = parser.parse_program()?;

    Ok(Program {
        terms: parser.terms,
        entry_term: entry,
    })
}
