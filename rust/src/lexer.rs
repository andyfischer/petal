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
    Continue,
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
    PlusAssign,    // +=
    MinusAssign,   // -=
    StarAssign,    // *=
    SlashAssign,   // /=
    PercentAssign, // %=

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

    // JSX
    JsxOpenStart,       // `<` that starts an opening tag (immediately followed by ident)
    JsxTagName(String), // tag name identifier
    JsxSelfClose,       // `/>`
    JsxCloseStart,      // `</`
    JsxText(String),    // text content between tags

    // Special
    Newline,
    Eof,
}

/// Lexer mode for JSX disambiguation.
#[derive(Debug, Clone, PartialEq)]
enum LexerMode {
    Normal,
    JsxTag,     // Inside `<tag ...>` — lexing attributes
    JsxContent, // Between `>` and `</` — lexing children
}

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    pub tokens: Vec<Token>,
    mode_stack: Vec<LexerMode>,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            tokens: Vec::new(),
            mode_stack: Vec::new(),
        }
    }

    fn current_mode(&self) -> &LexerMode {
        self.mode_stack.last().unwrap_or(&LexerMode::Normal)
    }

    pub fn tokenize(&mut self) -> Result<&[Token], String> {
        while self.pos < self.input.len() {
            if *self.current_mode() == LexerMode::JsxContent {
                self.tokenize_jsx_content()?;
                continue;
            }

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

            // Handle newlines (top-level only, not inside interpolation)
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
                _ => self.tokenize_one()?,
            }
        }

        self.tokens.push(Token::Eof);
        Ok(&self.tokens)
    }

    /// Tokenize a single token at the current position.
    fn tokenize_one(&mut self) -> Result<(), String> {
        if self.pos >= self.input.len() {
            return Ok(());
        }
        let ch = self.input[self.pos];
        match ch {
            '"' => self.read_string()?,
            '(' => { self.tokens.push(Token::LParen); self.pos += 1; }
            ')' => { self.tokens.push(Token::RParen); self.pos += 1; }
            '{' => { self.tokens.push(Token::LBrace); self.pos += 1; }
            '}' => { self.tokens.push(Token::RBrace); self.pos += 1; }
            '[' => { self.tokens.push(Token::LBracket); self.pos += 1; }
            ']' => { self.tokens.push(Token::RBracket); self.pos += 1; }
            ',' => { self.tokens.push(Token::Comma); self.pos += 1; }
            ':' => { self.tokens.push(Token::Colon); self.pos += 1; }
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
                } else if self.peek_next() == Some('=') {
                    self.tokens.push(Token::PlusAssign);
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
                } else if self.peek_next() == Some('=') {
                    self.tokens.push(Token::MinusAssign);
                    self.pos += 2;
                } else {
                    self.tokens.push(Token::Minus);
                    self.pos += 1;
                }
            }
            '*' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::StarAssign);
                    self.pos += 2;
                } else {
                    self.tokens.push(Token::Star);
                    self.pos += 1;
                }
            }
            '/' => {
                if self.peek_next() == Some('>')
                    && *self.current_mode() == LexerMode::JsxTag
                {
                    // Self-closing JSX tag: `/>`
                    self.tokens.push(Token::JsxSelfClose);
                    self.pos += 2;
                    self.mode_stack.pop(); // pop JsxTag
                } else if self.peek_next() == Some('=') {
                    self.tokens.push(Token::SlashAssign);
                    self.pos += 2;
                } else {
                    self.tokens.push(Token::Slash);
                    self.pos += 1;
                }
            }
            '%' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::PercentAssign);
                    self.pos += 2;
                } else {
                    self.tokens.push(Token::Percent);
                    self.pos += 1;
                }
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
                } else if self.peek_next().map_or(false, |c| c.is_ascii_alphabetic()) {
                    // JSX open tag: `<div`
                    self.tokens.push(Token::JsxOpenStart);
                    self.pos += 1;
                    self.read_jsx_tag_name()?;
                    self.mode_stack.push(LexerMode::JsxTag);
                } else if self.peek_next() == Some('/') {
                    // JSX close tag: `</div>`
                    self.tokens.push(Token::JsxCloseStart);
                    self.pos += 2;
                    self.read_jsx_tag_name()?;
                    self.expect_char('>')?;
                    self.mode_stack.pop(); // pop JsxContent
                } else {
                    self.tokens.push(Token::Lt);
                    self.pos += 1;
                }
            }
            '>' => {
                if self.peek_next() == Some('=') {
                    self.tokens.push(Token::Ge);
                    self.pos += 2;
                } else if *self.current_mode() == LexerMode::JsxTag {
                    // End of JSX open tag — switch to content mode
                    self.tokens.push(Token::Gt);
                    self.pos += 1;
                    // Replace JsxTag with JsxContent
                    self.mode_stack.pop();
                    self.mode_stack.push(LexerMode::JsxContent);
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
        Ok(())
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

    fn expect_char(&mut self, expected: char) -> Result<(), String> {
        if self.pos < self.input.len() && self.input[self.pos] == expected {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!(
                "Expected '{}' at position {}",
                expected, self.pos
            ))
        }
    }

    fn read_jsx_tag_name(&mut self) -> Result<(), String> {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!("Expected tag name at position {}", self.pos));
        }
        let name: String = self.input[start..self.pos].iter().collect();
        self.tokens.push(Token::JsxTagName(name));
        Ok(())
    }

    fn tokenize_jsx_content(&mut self) -> Result<(), String> {
        let mut text = String::new();
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            match ch {
                '<' => {
                    // Flush accumulated text
                    self.flush_jsx_text(&mut text);

                    if self.peek_next() == Some('/') {
                        // Closing tag: `</div>`
                        self.tokens.push(Token::JsxCloseStart);
                        self.pos += 2;
                        self.read_jsx_tag_name()?;
                        self.expect_char('>')?;
                        self.mode_stack.pop(); // pop JsxContent
                        return Ok(());
                    } else if self.peek_next().map_or(false, |c| c.is_ascii_alphabetic()) {
                        // Nested open tag
                        self.tokens.push(Token::JsxOpenStart);
                        self.pos += 1;
                        self.read_jsx_tag_name()?;
                        // Push JsxTag on top of JsxContent (content stays)
                        self.mode_stack.push(LexerMode::JsxTag);
                        return Ok(());
                    } else {
                        text.push(ch);
                        self.pos += 1;
                    }
                }
                '{' => {
                    // Expression hole
                    self.flush_jsx_text(&mut text);
                    self.tokens.push(Token::LBrace);
                    self.pos += 1;
                    // Lex expression tokens until matching `}`
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
                                self.tokens.push(Token::RBrace);
                                self.pos += 1;
                                break;
                            }
                            self.tokens.push(Token::RBrace);
                            self.pos += 1;
                        } else if inner_ch == '{' {
                            depth += 1;
                            self.tokens.push(Token::LBrace);
                            self.pos += 1;
                        } else if inner_ch == '\n' || inner_ch == '\r' {
                            self.pos += 1;
                            if inner_ch == '\r'
                                && self.pos < self.input.len()
                                && self.input[self.pos] == '\n'
                            {
                                self.pos += 1;
                            }
                        } else {
                            self.tokenize_one()?;
                        }
                    }
                    return Ok(());
                }
                _ => {
                    text.push(ch);
                    self.pos += 1;
                }
            }
        }
        // Flush remaining text
        self.flush_jsx_text(&mut text);
        Ok(())
    }

    fn flush_jsx_text(&mut self, text: &mut String) {
        // Trim and collapse whitespace
        let trimmed = collapse_jsx_whitespace(text);
        if !trimmed.is_empty() {
            self.tokens.push(Token::JsxText(trimmed));
        }
        text.clear();
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
            "continue" => Token::Continue,
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

/// Collapse JSX whitespace following React-like rules:
/// - If text contains newlines, trim each line and join non-empty lines with a single space
/// - If text is a single line, preserve it as-is
fn collapse_jsx_whitespace(s: &str) -> String {
    if !s.contains('\n') && !s.contains('\r') {
        return s.to_string();
    }
    let mut result = String::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(trimmed);
    }
    result
}
