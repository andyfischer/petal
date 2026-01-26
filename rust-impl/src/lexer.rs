use crate::token::{Token, TokenInfo};

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else if ch == '/' && self.peek_next() == Some('/') {
                // Single-line comment
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
            } else if ch == '/' && self.peek_next() == Some('*') {
                // Multi-line comment
                self.advance(); // /
                self.advance(); // *
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek_next() == Some('/') {
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

    fn read_string(&mut self) -> Token {
        self.advance(); // opening quote
        let mut s = String::new();

        // Check for multiline string
        if self.peek() == Some('"') && self.peek_next() == Some('"') {
            self.advance(); // second quote
            self.advance(); // third quote

            // Skip leading newline
            if self.peek() == Some('\n') {
                self.advance();
            }

            while let Some(ch) = self.peek() {
                if ch == '"' && self.peek_next() == Some('"') {
                    self.advance();
                    self.advance();
                    if self.peek() == Some('"') {
                        self.advance();
                        break;
                    }
                    // Not the end, add the quotes back
                    s.push('"');
                    s.push('"');
                    continue;
                }
                s.push(ch);
                self.advance();
            }
            return Token::String(s);
        }

        while let Some(ch) = self.peek() {
            if ch == '"' {
                self.advance();
                break;
            }
            if ch == '\\' {
                self.advance();
                if let Some(escaped) = self.peek() {
                    self.advance();
                    match escaped {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        '\\' => s.push('\\'),
                        '"' => s.push('"'),
                        '$' => s.push('$'),
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
        Token::String(s)
    }

    fn read_number(&mut self) -> Token {
        let mut s = String::new();
        let mut is_float = false;
        let mut is_hex = false;
        let mut is_binary = false;

        // Check for hex or binary
        if self.peek() == Some('0') {
            s.push(self.advance().unwrap());
            if let Some(ch) = self.peek() {
                if ch == 'x' || ch == 'X' {
                    is_hex = true;
                    s.push(self.advance().unwrap());
                } else if ch == 'b' || ch == 'B' {
                    is_binary = true;
                    s.push(self.advance().unwrap());
                }
            }
        }

        while let Some(ch) = self.peek() {
            if is_hex && ch.is_ascii_hexdigit() {
                s.push(self.advance().unwrap());
            } else if is_binary && (ch == '0' || ch == '1') {
                s.push(self.advance().unwrap());
            } else if !is_hex && !is_binary && ch.is_ascii_digit() {
                s.push(self.advance().unwrap());
            } else if !is_hex && !is_binary && ch == '.' && !is_float {
                if let Some(next) = self.peek_next() {
                    if next.is_ascii_digit() {
                        is_float = true;
                        s.push(self.advance().unwrap());
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else if !is_hex && !is_binary && (ch == 'e' || ch == 'E') {
                is_float = true;
                s.push(self.advance().unwrap());
                if let Some(sign) = self.peek() {
                    if sign == '+' || sign == '-' {
                        s.push(self.advance().unwrap());
                    }
                }
            } else {
                break;
            }
        }

        if is_hex {
            let hex_str = &s[2..];
            let val = i64::from_str_radix(hex_str, 16).unwrap_or(0);
            Token::Integer(val)
        } else if is_binary {
            let bin_str = &s[2..];
            let val = i64::from_str_radix(bin_str, 2).unwrap_or(0);
            Token::Integer(val)
        } else if is_float {
            let val: f64 = s.parse().unwrap_or(0.0);
            Token::Float(val)
        } else {
            let val: i64 = s.parse().unwrap_or(0);
            Token::Integer(val)
        }
    }

    fn read_identifier(&mut self) -> Token {
        let mut s = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                s.push(self.advance().unwrap());
            } else {
                break;
            }
        }

        match s.as_str() {
            "fn" => Token::Fn,
            "let" => Token::Let,
            "return" => Token::Return,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "in" => Token::In,
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            "struct" => Token::Struct,
            "enum" => Token::Enum,
            "state" => Token::State,
            "match" => Token::Match,
            "loop" => Token::Loop,
            "break" => Token::Break,
            "continue" => Token::Continue,
            _ => Token::Identifier(s),
        }
    }

    fn read_symbol(&mut self) -> Token {
        self.advance(); // skip ':'
        let mut s = String::new();

        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                s.push(self.advance().unwrap());
            } else {
                break;
            }
        }

        Token::Symbol(s)
    }

    #[allow(dead_code)]

    pub fn tokenize(&mut self) -> Vec<TokenInfo> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace();
            let line = self.line;
            let column = self.column;

            let token = match self.peek() {
                None => Token::Eof,
                Some(ch) => match ch {
                    '"' => self.read_string(),
                    ':' => {
                        self.advance();
                        if self.peek() == Some(':') {
                            self.advance();
                            Token::ColonColon
                        } else if self.peek().map(|c| c.is_alphanumeric() || c == '_').unwrap_or(false) {
                            // Symbol like :symbol_name or :FF0000
                            let mut s = String::new();
                            while let Some(c) = self.peek() {
                                if c.is_alphanumeric() || c == '_' {
                                    s.push(self.advance().unwrap());
                                } else {
                                    break;
                                }
                            }
                            Token::Symbol(s)
                        } else {
                            Token::Colon
                        }
                    }
                    '(' => { self.advance(); Token::LeftParen }
                    ')' => { self.advance(); Token::RightParen }
                    '{' => { self.advance(); Token::LeftBrace }
                    '}' => { self.advance(); Token::RightBrace }
                    '[' => { self.advance(); Token::LeftBracket }
                    ']' => { self.advance(); Token::RightBracket }
                    ',' => { self.advance(); Token::Comma }
                    ';' => { self.advance(); Token::Semicolon }
                    '@' => { self.advance(); Token::At }
                    '+' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::PlusEqual
                        } else {
                            Token::Plus
                        }
                    }
                    '-' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::MinusEqual
                        } else if self.peek() == Some('>') {
                            self.advance();
                            Token::Arrow
                        } else {
                            Token::Minus
                        }
                    }
                    '*' => {
                        self.advance();
                        if self.peek() == Some('*') {
                            self.advance();
                            if self.peek() == Some('=') {
                                self.advance();
                                Token::StarEqual  // Using StarEqual for **= (simplification)
                            } else {
                                Token::StarStar
                            }
                        } else if self.peek() == Some('=') {
                            self.advance();
                            Token::StarEqual
                        } else {
                            Token::Star
                        }
                    }
                    '/' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::SlashEqual
                        } else {
                            Token::Slash
                        }
                    }
                    '%' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::PercentEqual
                        } else {
                            Token::Percent
                        }
                    }
                    '=' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::EqualEqual
                        } else if self.peek() == Some('>') {
                            self.advance();
                            Token::FatArrow
                        } else {
                            Token::Equal
                        }
                    }
                    '!' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::BangEqual
                        } else {
                            Token::Bang
                        }
                    }
                    '<' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::LessEqual
                        } else {
                            Token::Less
                        }
                    }
                    '>' => {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Token::GreaterEqual
                        } else {
                            Token::Greater
                        }
                    }
                    '&' => {
                        self.advance();
                        if self.peek() == Some('&') {
                            self.advance();
                            Token::And
                        } else {
                            // Single & not supported, treat as error
                            continue;
                        }
                    }
                    '|' => {
                        self.advance();
                        if self.peek() == Some('|') {
                            self.advance();
                            Token::Or
                        } else {
                            continue;
                        }
                    }
                    '.' => {
                        self.advance();
                        if self.peek() == Some('.') {
                            self.advance();
                            Token::DotDot
                        } else {
                            Token::Dot
                        }
                    }
                    '?' => {
                        self.advance();
                        if self.peek() == Some('.') {
                            self.advance();
                            Token::QuestionDot
                        } else {
                            Token::Question
                        }
                    }
                    _ if ch.is_ascii_digit() => self.read_number(),
                    _ if ch.is_alphabetic() || ch == '_' => self.read_identifier(),
                    _ => {
                        self.advance();
                        continue;
                    }
                }
            };

            let is_eof = token == Token::Eof;
            tokens.push(TokenInfo::new(token, line, column));

            if is_eof {
                break;
            }
        }

        tokens
    }
}
