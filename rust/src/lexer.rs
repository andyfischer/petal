#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum Token {
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
    In,
    While,
    Match,
    Return,
    Break,
    State,
    Enum,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusPlus, // ++
    Eq,       // ==
    Ne,       // !=
    Lt,       // <
    Le,       // <=
    Gt,       // >
    Ge,       // >=
    And,      // &&
    Or,       // ||
    Bang,     // !
    Assign,   // =

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Colon,
    Arrow, // ->
    DotDot,    // ..
    DotDotDot, // ...

    // String interpolation
    InterpStart, // signals start of interpolation expression
    InterpEnd,   // signals end of interpolation expression

    // Special
    Newline,
    Eof,
}

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    pub tokens: Vec<Token>,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            tokens: Vec::new(),
        }
    }

    pub fn tokenize(&mut self) -> Result<&[Token], String> {
        while self.pos < self.input.len() {
            self.skip_whitespace_no_newline();
            if self.pos >= self.input.len() {
                break;
            }

            let ch = self.input[self.pos];

            // Skip comments
            if ch == '/' && self.peek_next() == Some('/') {
                self.skip_line_comment();
                continue;
            }

            match ch {
                '\n' => {
                    self.tokens.push(Token::Newline);
                    self.pos += 1;
                }
                '\r' => {
                    self.pos += 1;
                    if self.pos < self.input.len() && self.input[self.pos] == '\n' {
                        self.pos += 1;
                    }
                    self.tokens.push(Token::Newline);
                }
                '"' => self.read_string()?,
                '(' => {
                    self.tokens.push(Token::LParen);
                    self.pos += 1;
                }
                ')' => {
                    self.tokens.push(Token::RParen);
                    self.pos += 1;
                }
                '{' => {
                    self.tokens.push(Token::LBrace);
                    self.pos += 1;
                }
                '}' => {
                    self.tokens.push(Token::RBrace);
                    self.pos += 1;
                }
                '[' => {
                    self.tokens.push(Token::LBracket);
                    self.pos += 1;
                }
                ']' => {
                    self.tokens.push(Token::RBracket);
                    self.pos += 1;
                }
                ',' => {
                    self.tokens.push(Token::Comma);
                    self.pos += 1;
                }
                ':' => {
                    self.tokens.push(Token::Colon);
                    self.pos += 1;
                }
                '.' => {
                    if self.peek_next() == Some('.') {
                        if self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '.' {
                            self.tokens.push(Token::DotDotDot);
                            self.pos += 3;
                        } else {
                            self.tokens.push(Token::DotDot);
                            self.pos += 2;
                        }
                    } else {
                        self.tokens.push(Token::Dot);
                        self.pos += 1;
                    }
                }
                '+' => {
                    if self.peek_next() == Some('+') {
                        self.tokens.push(Token::PlusPlus);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Plus);
                        self.pos += 1;
                    }
                }
                '-' => {
                    if self.peek_next() == Some('>') {
                        self.tokens.push(Token::Arrow);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Minus);
                        self.pos += 1;
                    }
                }
                '*' => {
                    self.tokens.push(Token::Star);
                    self.pos += 1;
                }
                '/' => {
                    self.tokens.push(Token::Slash);
                    self.pos += 1;
                }
                '%' => {
                    self.tokens.push(Token::Percent);
                    self.pos += 1;
                }
                '=' => {
                    if self.peek_next() == Some('=') {
                        self.tokens.push(Token::Eq);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Assign);
                        self.pos += 1;
                    }
                }
                '!' => {
                    if self.peek_next() == Some('=') {
                        self.tokens.push(Token::Ne);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Bang);
                        self.pos += 1;
                    }
                }
                '<' => {
                    if self.peek_next() == Some('=') {
                        self.tokens.push(Token::Le);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Lt);
                        self.pos += 1;
                    }
                }
                '>' => {
                    if self.peek_next() == Some('=') {
                        self.tokens.push(Token::Ge);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Gt);
                        self.pos += 1;
                    }
                }
                '&' => {
                    if self.peek_next() == Some('&') {
                        self.tokens.push(Token::And);
                        self.pos += 2;
                    } else {
                        return Err(format!("Unexpected character '&' at position {}", self.pos));
                    }
                }
                '|' => {
                    if self.peek_next() == Some('|') {
                        self.tokens.push(Token::Or);
                        self.pos += 2;
                    } else {
                        return Err(format!("Unexpected character '|' at position {}", self.pos));
                    }
                }
                c if c.is_ascii_digit() => self.read_number()?,
                c if c.is_alphabetic() || c == '_' => self.read_identifier(),
                _ => {
                    return Err(format!("Unexpected character '{}' at position {}", ch, self.pos));
                }
            }
        }

        self.tokens.push(Token::Eof);
        Ok(&self.tokens)
    }

    fn peek_next(&self) -> Option<char> {
        if self.pos + 1 < self.input.len() {
            Some(self.input[self.pos + 1])
        } else {
            None
        }
    }

    fn skip_whitespace_no_newline(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == ' ' || ch == '\t' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos] != '\n' {
            self.pos += 1;
        }
    }

    fn read_string(&mut self) -> Result<(), String> {
        self.pos += 1; // skip opening quote
        let mut s = String::new();
        let mut has_interp = false;

        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '"' {
                self.pos += 1;
                if has_interp {
                    // Emit trailing string part (even if empty, for concatenation)
                    self.tokens.push(Token::String(s));
                    self.tokens.push(Token::InterpEnd);
                } else {
                    self.tokens.push(Token::String(s));
                }
                return Ok(());
            }
            if ch == '\\' {
                self.pos += 1;
                if self.pos >= self.input.len() {
                    return Err("Unterminated string escape".to_string());
                }
                match self.input[self.pos] {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    '\\' => s.push('\\'),
                    '"' => s.push('"'),
                    '{' => s.push('{'),
                    '}' => s.push('}'),
                    other => {
                        s.push('\\');
                        s.push(other);
                    }
                }
                self.pos += 1;
                continue;
            }
            if ch == '{' {
                // Start of interpolation
                if !has_interp {
                    has_interp = true;
                    self.tokens.push(Token::InterpStart);
                }
                // Emit the string part accumulated so far
                self.tokens.push(Token::String(s));
                s = String::new();
                self.pos += 1;

                // Tokenize the expression inside braces
                let mut depth = 1;
                while self.pos < self.input.len() && depth > 0 {
                    self.skip_whitespace_no_newline();
                    if self.pos >= self.input.len() {
                        break;
                    }
                    let inner_ch = self.input[self.pos];
                    if inner_ch == '}' {
                        depth -= 1;
                        if depth == 0 {
                            self.pos += 1;
                            break;
                        }
                        self.tokens.push(Token::RBrace);
                        self.pos += 1;
                    } else if inner_ch == '{' {
                        depth += 1;
                        self.tokens.push(Token::LBrace);
                        self.pos += 1;
                    } else {
                        // Tokenize one token from the interpolation expression
                        self.tokenize_one()?;
                    }
                }
                if depth > 0 {
                    return Err("Unterminated string interpolation".to_string());
                }
                continue;
            }
            s.push(ch);
            self.pos += 1;
        }
        Err("Unterminated string".to_string())
    }

    /// Tokenize a single token at the current position (used inside string interpolation).
    fn tokenize_one(&mut self) -> Result<(), String> {
        if self.pos >= self.input.len() {
            return Ok(());
        }
        let ch = self.input[self.pos];
        match ch {
            '"' => self.read_string()?,
            '(' => { self.tokens.push(Token::LParen); self.pos += 1; }
            ')' => { self.tokens.push(Token::RParen); self.pos += 1; }
            '[' => { self.tokens.push(Token::LBracket); self.pos += 1; }
            ']' => { self.tokens.push(Token::RBracket); self.pos += 1; }
            ',' => { self.tokens.push(Token::Comma); self.pos += 1; }
            ':' => { self.tokens.push(Token::Colon); self.pos += 1; }
            '.' => {
                if self.peek_next() == Some('.') {
                    if self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '.' {
                        self.tokens.push(Token::DotDotDot); self.pos += 3;
                    } else {
                        self.tokens.push(Token::DotDot); self.pos += 2;
                    }
                } else {
                    self.tokens.push(Token::Dot); self.pos += 1;
                }
            }
            '+' => {
                if self.peek_next() == Some('+') {
                    self.tokens.push(Token::PlusPlus); self.pos += 2;
                } else {
                    self.tokens.push(Token::Plus); self.pos += 1;
                }
            }
            '-' => {
                if self.peek_next() == Some('>') {
                    self.tokens.push(Token::Arrow); self.pos += 2;
                } else {
                    self.tokens.push(Token::Minus); self.pos += 1;
                }
            }
            '*' => { self.tokens.push(Token::Star); self.pos += 1; }
            '/' => { self.tokens.push(Token::Slash); self.pos += 1; }
            '%' => { self.tokens.push(Token::Percent); self.pos += 1; }
            '=' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::Eq); self.pos += 2;
                } else {
                    self.tokens.push(Token::Assign); self.pos += 1;
                }
            }
            '!' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::Ne); self.pos += 2;
                } else {
                    self.tokens.push(Token::Bang); self.pos += 1;
                }
            }
            '<' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::Le); self.pos += 2;
                } else {
                    self.tokens.push(Token::Lt); self.pos += 1;
                }
            }
            '>' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::Ge); self.pos += 2;
                } else {
                    self.tokens.push(Token::Gt); self.pos += 1;
                }
            }
            '&' => {
                if self.peek_next() == Some('&') {
                    self.tokens.push(Token::And); self.pos += 2;
                } else {
                    return Err(format!("Unexpected character '&' at position {}", self.pos));
                }
            }
            '|' => {
                if self.peek_next() == Some('|') {
                    self.tokens.push(Token::Or); self.pos += 2;
                } else {
                    return Err(format!("Unexpected character '|' at position {}", self.pos));
                }
            }
            c if c.is_ascii_digit() => self.read_number()?,
            c if c.is_alphabetic() || c == '_' => self.read_identifier(),
            _ => {
                return Err(format!("Unexpected character '{}' in interpolation at position {}", ch, self.pos));
            }
        }
        Ok(())
    }

    fn read_number(&mut self) -> Result<(), String> {
        let start = self.pos;
        let mut is_float = false;

        while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
            self.pos += 1;
        }

        if self.pos < self.input.len() && self.input[self.pos] == '.' {
            // Check it's not `..` (range)
            if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '.' {
                // It's a range like 1..10, don't consume the dot
            } else if self.pos + 1 < self.input.len() && self.input[self.pos + 1].is_ascii_digit() {
                is_float = true;
                self.pos += 1; // skip the dot
                while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            } else {
                // Could be a method call like `5.method()` - don't consume
            }
        }

        let text: String = self.input[start..self.pos].iter().collect();
        if is_float {
            let f: f64 = text.parse().map_err(|e| format!("Invalid float: {}", e))?;
            self.tokens.push(Token::Float(f));
        } else {
            let n: i64 = text.parse().map_err(|e| format!("Invalid integer: {}", e))?;
            self.tokens.push(Token::Int(n));
        }
        Ok(())
    }

    fn read_identifier(&mut self) {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch.is_alphanumeric() || ch == '_' || ch == '?' {
                self.pos += 1;
            } else {
                break;
            }
        }

        let text: String = self.input[start..self.pos].iter().collect();
        let token = match text.as_str() {
            "let" => Token::Let,
            "fn" => Token::Fn,
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "in" => Token::In,
            "while" => Token::While,
            "match" => Token::Match,
            "return" => Token::Return,
            "break" => Token::Break,
            "state" => Token::State,
            "enum" => Token::Enum,
            "true" => Token::True,
            "false" => Token::False,
            "nil" => Token::Nil,
            _ => Token::Ident(text),
        };
        self.tokens.push(token);
    }
}
