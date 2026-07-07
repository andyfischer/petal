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
    End,
    Then,
    Do,
    Elsif,
    When,
    Import,

    // Operators
    Plus,
    Minus,
    /// A `-` with whitespace before but none after (e.g. the `-2` in `[1 -2]`).
    /// Treated as subtraction in normal contexts, but as the start of a new
    /// negated element inside comma-less list/argument juxtaposition.
    /// See docs/syntax/optional-commas.md.
    MinusPrefix,
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
    At, // @ — in-out argument marker (see ast::ExprKind::AtVar)
    Pipe,  // |>
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

    // Color literal (#rgb, #rgba, #rrggbb, #rrggbbaa)
    Color(String),

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

use crate::source_map::{FileId, SourcePosition, SourceSpan, ENTRY_FILE};

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    pub tokens: Vec<Token>,
    pub token_spans: Vec<SourceSpan>,
    /// Leading trivia (whitespace, comments) preceding each token, parallel to
    /// `tokens`. Empty until [`Lexer::tokenize`] finishes and populates it. The
    /// parser ignores this — it exists so source-preserving tooling can round-
    /// trip comments and layout. See `crate::trivia`.
    pub token_leading_trivia: Vec<Vec<crate::trivia::Trivia>>,
    mode_stack: Vec<LexerMode>,
    // Line/column tracking (1-based)
    line: u32,
    col: u32,
    /// File id stamped onto every emitted span. Entry file by default;
    /// the module loader lexes each imported module with its own id so
    /// spans stay file-local. See source_map::FileId.
    file: FileId,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self::new_in_file(input, ENTRY_FILE)
    }

    /// A lexer whose spans are tagged with `file` (used when lexing an
    /// imported module — line/column stay local to that module's source).
    pub fn new_in_file(input: &str, file: FileId) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            tokens: Vec::new(),
            token_spans: Vec::new(),
            token_leading_trivia: Vec::new(),
            mode_stack: Vec::new(),
            line: 1,
            col: 1,
            file,
        }
    }

    fn current_mode(&self) -> &LexerMode {
        self.mode_stack.last().unwrap_or(&LexerMode::Normal)
    }

    /// Record current position as a SourcePosition.
    fn current_pos(&self) -> SourcePosition {
        SourcePosition {
            line: self.line,
            column: self.col,
            offset: self.pos as u32,
        }
    }

    /// Push a token whose span runs from `start` to the current position.
    fn push_token(&mut self, token: Token, start: SourcePosition) {
        let end = self.current_pos();
        self.push_token_span(token, start, end);
    }

    /// Push a token with an explicit `[start, end)` span. Used where a token's
    /// span must be stated exactly rather than "start .. cursor" — e.g. the
    /// pieces of an interpolated string, so every character of the source is
    /// covered by exactly one token span (see `crate::trivia`).
    fn push_token_span(&mut self, token: Token, start: SourcePosition, end: SourcePosition) {
        self.tokens.push(token);
        self.token_spans.push(SourceSpan { start, end, file: self.file });
    }

    /// The position of the single non-newline character immediately before the
    /// cursor. Valid only when the last consumed character was not a newline —
    /// used to recover the position of a closing `}` that
    /// [`Lexer::tokenize_braced_expr`] has just consumed without emitting a
    /// token.
    fn prev_char_pos(&self) -> SourcePosition {
        SourcePosition { line: self.line, column: self.col - 1, offset: self.pos as u32 - 1 }
    }

    /// Extend the most recently pushed token's span to the current cursor,
    /// absorbing a trailing delimiter that has no token of its own (e.g. the
    /// `>` closing a JSX end tag, consumed by [`Lexer::expect_char`]) so it is
    /// not left in an inter-token gap. See `crate::trivia`.
    fn extend_last_span_to_cursor(&mut self) {
        let end = self.current_pos();
        if let Some(span) = self.token_spans.last_mut() {
            span.end = end;
        }
    }

    /// Advance position by one character, updating line/column tracking.
    fn advance_char(&mut self) {
        if self.pos < self.input.len() {
            if self.input[self.pos] == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    /// Advance position by n characters.
    fn advance_n(&mut self, n: usize) {
        for _ in 0..n {
            self.advance_char();
        }
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
                    let start = self.current_pos();
                    self.advance_char();
                    self.push_token(Token::Newline, start);
                }
                '\r' => {
                    let start = self.current_pos();
                    self.advance_char();
                    if self.pos < self.input.len() && self.input[self.pos] == '\n' {
                        self.advance_char();
                    }
                    self.push_token(Token::Newline, start);
                }
                _ => self.tokenize_one()?,
            }
        }

        let start = self.current_pos();
        self.push_token(Token::Eof, start);

        // Recover the trivia (whitespace/comments) between tokens from the
        // spans, so downstream tooling can reproduce the original source. This
        // does not affect `tokens`, which the parser consumes unchanged.
        let source: String = self.input.iter().collect();
        self.token_leading_trivia = crate::trivia::leading_trivia(&source, &self.token_spans);

        Ok(&self.tokens)
    }

    /// Tokenize a single token at the current position.
    fn tokenize_one(&mut self) -> Result<(), String> {
        if self.pos >= self.input.len() {
            return Ok(());
        }
        let ch = self.input[self.pos];
        let start = self.current_pos();
        match ch {
            '"' => {
                // `"""` opens a triple-quoted raw string (verbatim, no interpolation).
                if self.peek_next() == Some('"')
                    && self.pos + 2 < self.input.len()
                    && self.input[self.pos + 2] == '"'
                {
                    self.read_raw_string()?
                } else {
                    self.read_string()?
                }
            }
            '(' => { self.advance_char(); self.push_token(Token::LParen, start); }
            ')' => { self.advance_char(); self.push_token(Token::RParen, start); }
            '{' => { self.advance_char(); self.push_token(Token::LBrace, start); }
            '}' => { self.advance_char(); self.push_token(Token::RBrace, start); }
            '[' => { self.advance_char(); self.push_token(Token::LBracket, start); }
            ']' => { self.advance_char(); self.push_token(Token::RBracket, start); }
            ',' => { self.advance_char(); self.push_token(Token::Comma, start); }
            ':' => { self.advance_char(); self.push_token(Token::Colon, start); }
            '@' => { self.advance_char(); self.push_token(Token::At, start); }
            '.' => {
                if self.peek_next() == Some('.') {
                    if self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '.' {
                        self.advance_n(3);
                        self.push_token(Token::DotDotDot, start);
                    } else {
                        self.advance_n(2);
                        self.push_token(Token::DotDot, start);
                    }
                } else {
                    self.advance_char();
                    self.push_token(Token::Dot, start);
                }
            }
            '+' => {
                if self.peek_next() == Some('+') {
                    self.advance_n(2);
                    self.push_token(Token::PlusPlus, start);
                } else if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::PlusAssign, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Plus, start);
                }
            }
            '-' => {
                if self.peek_next() == Some('>') {
                    self.advance_n(2);
                    self.push_token(Token::Arrow, start);
                } else if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::MinusAssign, start);
                } else {
                    // Spacing-aware: a `-` with whitespace before but none
                    // after (e.g. the `-2` in `[1 -2]`) becomes a MinusPrefix
                    // so it can begin a new negated element in comma-less
                    // juxtaposition. See docs/syntax/optional-commas.md.
                    let space_before = self.pos == 0
                        || matches!(self.input[self.pos - 1], ' ' | '\t' | '\n');
                    let space_after =
                        matches!(self.peek_next(), Some(' ') | Some('\t') | Some('\n') | None);
                    self.advance_char();
                    if space_before && !space_after {
                        self.push_token(Token::MinusPrefix, start);
                    } else {
                        self.push_token(Token::Minus, start);
                    }
                }
            }
            '*' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::StarAssign, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Star, start);
                }
            }
            '/' => {
                if self.peek_next() == Some('>')
                    && *self.current_mode() == LexerMode::JsxTag
                {
                    // Self-closing JSX tag: `/>`
                    self.advance_n(2);
                    self.push_token(Token::JsxSelfClose, start);
                    self.mode_stack.pop(); // pop JsxTag
                } else if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::SlashAssign, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Slash, start);
                }
            }
            '%' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::PercentAssign, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Percent, start);
                }
            }
            '=' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::Eq, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Assign, start);
                }
            }
            '!' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::Ne, start);
                } else {
                    self.advance_char();
                    self.push_token(Token::Bang, start);
                }
            }
            '<' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::Le, start);
                } else if self.peek_next().is_some_and(|c| c.is_ascii_alphabetic()) {
                    // JSX open tag: `<div`
                    self.advance_char();
                    self.push_token(Token::JsxOpenStart, start);
                    self.read_jsx_tag_name()?;
                    self.mode_stack.push(LexerMode::JsxTag);
                } else if self.peek_next() == Some('/') {
                    // JSX close tag: `</div>`
                    self.advance_n(2);
                    self.push_token(Token::JsxCloseStart, start);
                    self.read_jsx_tag_name()?;
                    self.expect_char('>')?;
                    // Fold the `>` into the tag-name span so no delimiter is
                    // left ungoverned by a token (see crate::trivia).
                    self.extend_last_span_to_cursor();
                    self.mode_stack.pop(); // pop JsxContent
                } else {
                    self.advance_char();
                    self.push_token(Token::Lt, start);
                }
            }
            '>' => {
                if self.peek_next() == Some('=') {
                    self.advance_n(2);
                    self.push_token(Token::Ge, start);
                } else if *self.current_mode() == LexerMode::JsxTag {
                    // End of JSX open tag — switch to content mode
                    self.advance_char();
                    self.push_token(Token::Gt, start);
                    // Replace JsxTag with JsxContent
                    self.mode_stack.pop();
                    self.mode_stack.push(LexerMode::JsxContent);
                } else {
                    self.advance_char();
                    self.push_token(Token::Gt, start);
                }
            }
            '&' => {
                if self.peek_next() == Some('&') {
                    self.advance_n(2);
                    self.push_token(Token::And, start);
                } else {
                    return Err(format!("Unexpected character '&' [line {}, column {}]", self.line, self.col));
                }
            }
            '|' => {
                if self.peek_next() == Some('|') {
                    self.advance_n(2);
                    self.push_token(Token::Or, start);
                } else if self.peek_next() == Some('>') {
                    self.advance_n(2);
                    self.push_token(Token::Pipe, start);
                } else {
                    return Err(format!("Unexpected character '|' [line {}, column {}]", self.line, self.col));
                }
            }
            '#' => self.read_color()?,
            c if c.is_ascii_digit() => self.read_number()?,
            c if c.is_alphabetic() || c == '_' => self.read_identifier(),
            ';' => {
                let start = self.current_pos();
                self.advance_char();
                self.push_token(Token::Newline, start);
            }
            _ => {
                return Err(format!("Unexpected character '{}' [line {}, column {}]", ch, self.line, self.col));
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
                self.advance_char();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.input.len() && self.input[self.pos] != '\n' {
            self.advance_char();
        }
    }

    fn read_string(&mut self) -> Result<(), String> {
        let open_quote = self.current_pos();
        self.advance_char(); // skip opening quote
        let mut s = String::new();
        let mut has_interp = false;
        // Source position where the current literal part's span begins. It
        // starts just after the opening quote; after each interpolation hole it
        // moves to the closing `}` so the following literal part absorbs that
        // delimiter. This keeps every token span gap-free so the source
        // reconstructs exactly — see docs/dev/source-preservation-plan.md Step 2
        // and `crate::trivia`.
        let mut part_start = self.current_pos();

        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '"' {
                let close_quote = self.current_pos();
                self.advance_char();
                if has_interp {
                    // Trailing literal part runs up to the closing quote; the
                    // quote itself is the InterpEnd token's span.
                    self.push_token_span(Token::String(s), part_start, close_quote);
                    let end = self.current_pos();
                    self.push_token_span(Token::InterpEnd, close_quote, end);
                } else {
                    self.push_token(Token::String(s), open_quote);
                }
                return Ok(());
            }
            if ch == '\\' {
                self.advance_char();
                if self.pos >= self.input.len() {
                    return Err(format!("Unterminated string escape [line {}, column {}]", self.line, self.col));
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
                self.advance_char();
                continue;
            }
            if ch == '{' {
                // Start of interpolation.
                if !has_interp {
                    has_interp = true;
                    // InterpStart's span is exactly the opening quote.
                    self.push_token_span(Token::InterpStart, open_quote, part_start);
                }
                // Emit the literal part accumulated so far, spanning from its
                // start through and including this opening `{`.
                self.advance_char(); // consume `{`
                let after_brace = self.current_pos();
                self.push_token_span(Token::String(s), part_start, after_brace);
                s = String::new();

                self.tokenize_braced_expr(false, false)?;
                // The next literal part absorbs the closing `}` just consumed,
                // so no delimiter is left in an inter-token gap.
                part_start = self.prev_char_pos();
                continue;
            }
            s.push(ch);
            self.advance_char();
        }
        Err(format!("Unterminated string [line {}, column {}]", open_quote.line, open_quote.column))
    }

    /// Read a triple-quoted raw string: `"""..."""`. Everything between the
    /// delimiters is captured verbatim — `{`/`}` are literal (no interpolation),
    /// backslashes are not treated as escapes, and raw newlines are allowed.
    /// Useful for embedding source code, e.g. `Program.parse("""...""")`.
    fn read_raw_string(&mut self) -> Result<(), String> {
        let start = self.current_pos();
        self.advance_n(3); // skip opening """
        let mut s = String::new();

        while self.pos < self.input.len() {
            if self.input[self.pos] == '"'
                && self.pos + 2 < self.input.len()
                && self.input[self.pos + 1] == '"'
                && self.input[self.pos + 2] == '"'
            {
                self.advance_n(3); // skip closing """
                self.push_token(Token::String(s), start);
                return Ok(());
            }
            s.push(self.input[self.pos]);
            self.advance_char();
        }
        Err(format!("Unterminated raw string [line {}, column {}]", start.line, start.column))
    }

    fn read_number(&mut self) -> Result<(), String> {
        let start_pos = self.current_pos();
        let start = self.pos;
        let mut is_float = false;

        while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
            self.advance_char();
        }

        if self.pos < self.input.len() && self.input[self.pos] == '.' {
            // Check it's not `..` (range)
            if self.pos + 1 < self.input.len() && self.input[self.pos + 1] == '.' {
                // It's a range like 1..10, don't consume the dot
            } else if self.pos + 1 < self.input.len() && self.input[self.pos + 1].is_ascii_digit() {
                is_float = true;
                self.advance_char(); // skip the dot
                while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                    self.advance_char();
                }
            } else {
                // Could be a method call like `5.method()` - don't consume
            }
        }

        let text: String = self.input[start..self.pos].iter().collect();
        if is_float {
            let f: f64 = text.parse().map_err(|e| format!("Invalid float: {}", e))?;
            self.push_token(Token::Float(f), start_pos);
        } else {
            let n: i64 = text.parse().map_err(|e| format!("Invalid integer: {}", e))?;
            self.push_token(Token::Int(n), start_pos);
        }
        Ok(())
    }

    fn read_color(&mut self) -> Result<(), String> {
        let start = self.current_pos();
        self.advance_char(); // skip '#'
        let hex_start = self.pos;
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_hexdigit() {
            self.advance_char();
        }
        let hex: String = self.input[hex_start..self.pos].iter().collect();
        match hex.len() {
            3 | 4 | 6 | 8 => {
                self.push_token(Token::Color(hex), start);
                Ok(())
            }
            _ => Err(format!(
                "Invalid color literal: #{} (expected 3, 4, 6, or 8 hex digits) [line {}, column {}]",
                hex, start.line, start.column
            )),
        }
    }

    /// Tokenize an expression inside braces (already past the opening `{`).
    /// Tracks brace depth and stops at the matching `}`.
    /// - `emit_close`: whether to emit `RBrace` for the final `}`
    /// - `skip_newlines`: whether to silently skip newline characters
    fn tokenize_braced_expr(
        &mut self,
        emit_close: bool,
        skip_newlines: bool,
    ) -> Result<(), String> {
        let mut depth = 1;
        while self.pos < self.input.len() && depth > 0 {
            self.skip_whitespace_no_newline();
            if self.pos >= self.input.len() {
                break;
            }
            let ch = self.input[self.pos];
            if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    if emit_close {
                        let start = self.current_pos();
                        self.advance_char();
                        self.push_token(Token::RBrace, start);
                    } else {
                        self.advance_char();
                    }
                    break;
                }
                let start = self.current_pos();
                self.advance_char();
                self.push_token(Token::RBrace, start);
            } else if ch == '{' {
                depth += 1;
                let start = self.current_pos();
                self.advance_char();
                self.push_token(Token::LBrace, start);
            } else if skip_newlines && (ch == '\n' || ch == '\r') {
                self.advance_char();
                if ch == '\r'
                    && self.pos < self.input.len()
                    && self.input[self.pos] == '\n'
                {
                    self.advance_char();
                }
            } else {
                self.tokenize_one()?;
            }
        }
        if depth > 0 {
            return Err(format!("Unterminated braced expression [line {}, column {}]", self.line, self.col));
        }
        Ok(())
    }

    fn expect_char(&mut self, expected: char) -> Result<(), String> {
        if self.pos < self.input.len() && self.input[self.pos] == expected {
            self.advance_char();
            Ok(())
        } else {
            Err(format!(
                "Expected '{}' [line {}, column {}]",
                expected, self.line, self.col
            ))
        }
    }

    fn read_jsx_tag_name(&mut self) -> Result<(), String> {
        let start = self.current_pos();
        let text_start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                self.advance_char();
            } else {
                break;
            }
        }
        if self.pos == text_start {
            return Err(format!("Expected tag name [line {}, column {}]", self.line, self.col));
        }
        let name: String = self.input[text_start..self.pos].iter().collect();
        self.push_token(Token::JsxTagName(name), start);
        Ok(())
    }

    fn tokenize_jsx_content(&mut self) -> Result<(), String> {
        let mut text = String::new();
        // Where this run of raw text began. The JsxText token's collapsed value
        // differs from the raw source, so the token carries a span covering the
        // raw text (`[run_start, cursor)`); reconstruction replays those bytes
        // verbatim while the parser sees the collapsed value. See `crate::trivia`.
        let run_start = self.current_pos();
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            match ch {
                '<' => {
                    // Flush accumulated text
                    self.flush_jsx_text(&mut text, run_start);

                    if self.peek_next() == Some('/') {
                        // Closing tag: `</div>`
                        let start = self.current_pos();
                        self.advance_n(2);
                        self.push_token(Token::JsxCloseStart, start);
                        self.read_jsx_tag_name()?;
                        self.expect_char('>')?;
                        // Fold the `>` into the tag-name span (see crate::trivia).
                        self.extend_last_span_to_cursor();
                        self.mode_stack.pop(); // pop JsxContent
                        return Ok(());
                    } else if self.peek_next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        // Nested open tag
                        let start = self.current_pos();
                        self.advance_char();
                        self.push_token(Token::JsxOpenStart, start);
                        self.read_jsx_tag_name()?;
                        // Push JsxTag on top of JsxContent (content stays)
                        self.mode_stack.push(LexerMode::JsxTag);
                        return Ok(());
                    } else {
                        text.push(ch);
                        self.advance_char();
                    }
                }
                '{' => {
                    // Expression hole
                    self.flush_jsx_text(&mut text, run_start);
                    let start = self.current_pos();
                    self.advance_char();
                    self.push_token(Token::LBrace, start);
                    self.tokenize_braced_expr(true, true)?;
                    return Ok(());
                }
                _ => {
                    text.push(ch);
                    self.advance_char();
                }
            }
        }
        // Flush remaining text
        self.flush_jsx_text(&mut text, run_start);
        Ok(())
    }

    fn flush_jsx_text(&mut self, text: &mut String, run_start: SourcePosition) {
        // Trim and collapse whitespace
        let trimmed = collapse_jsx_whitespace(text);
        if !trimmed.is_empty() {
            // Span the raw text consumed since `run_start`, not the collapsed
            // value — so reconstruction replays the original bytes exactly.
            let end = self.current_pos();
            self.push_token_span(Token::JsxText(trimmed), run_start, end);
        }
        text.clear();
    }

    fn read_identifier(&mut self) {
        let start = self.current_pos();
        let text_start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch.is_alphanumeric() || ch == '_' || ch == '?' {
                self.advance_char();
            } else {
                break;
            }
        }

        let text: String = self.input[text_start..self.pos].iter().collect();
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
            "end" => Token::End,
            "then" => Token::Then,
            "do" => Token::Do,
            "elsif" => Token::Elsif,
            "when" => Token::When,
            "import" => Token::Import,
            "true" => Token::True,
            "false" => Token::False,
            "nil" => Token::Nil,
            _ => Token::Ident(text),
        };
        self.push_token(token, start);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(src: &str) -> Vec<Token> {
        let mut lexer = Lexer::new(src);
        lexer.tokenize().expect("tokenize failed");
        lexer.tokens.into_iter().filter(|t| !matches!(t, Token::Newline | Token::Eof)).collect()
    }

    #[test]
    fn lex_integer_literal() {
        assert_eq!(tokenize("42"), vec![Token::Int(42)]);
    }

    #[test]
    fn lex_float_literal() {
        assert_eq!(tokenize("3.25"), vec![Token::Float(3.25)]);
    }

    #[test]
    fn lex_string_literal() {
        assert_eq!(tokenize(r#""hello""#), vec![Token::String("hello".into())]);
    }

    #[test]
    fn lex_keywords() {
        let tokens = tokenize("let fn if else for in while match return break continue state enum end then do elsif when");
        assert_eq!(tokens, vec![
            Token::Let, Token::Fn, Token::If, Token::Else,
            Token::For, Token::In, Token::While, Token::Match,
            Token::Return, Token::Break, Token::Continue,
            Token::State, Token::Enum,
            Token::End, Token::Then, Token::Do, Token::Elsif, Token::When,
        ]);
    }

    #[test]
    fn lex_operators() {
        let tokens = tokenize("+ - * / % ++ == != < <= > >= && || !");
        assert_eq!(tokens, vec![
            Token::Plus, Token::Minus, Token::Star, Token::Slash,
            Token::Percent, Token::PlusPlus, Token::Eq, Token::Ne,
            Token::Lt, Token::Le, Token::Gt, Token::Ge,
            Token::And, Token::Or, Token::Bang,
        ]);
    }

    #[test]
    fn lex_compound_assignment() {
        let tokens = tokenize("+= -= *= /= %=");
        assert_eq!(tokens, vec![
            Token::PlusAssign, Token::MinusAssign, Token::StarAssign,
            Token::SlashAssign, Token::PercentAssign,
        ]);
    }

    #[test]
    fn lex_delimiters() {
        let tokens = tokenize("( ) { } [ ] , . :");
        assert_eq!(tokens, vec![
            Token::LParen, Token::RParen, Token::LBrace, Token::RBrace,
            Token::LBracket, Token::RBracket, Token::Comma, Token::Dot,
            Token::Colon,
        ]);
    }

    #[test]
    fn lex_arrow_and_pipe() {
        let tokens = tokenize("-> |>");
        assert_eq!(tokens, vec![Token::Arrow, Token::Pipe]);
    }

    #[test]
    fn lex_identifier() {
        assert_eq!(tokenize("foo_bar"), vec![Token::Ident("foo_bar".into())]);
    }

    #[test]
    fn lex_skips_comments() {
        let tokens = tokenize("42 // this is a comment\n7");
        assert_eq!(tokens, vec![Token::Int(42), Token::Int(7)]);
    }

    #[test]
    fn lex_string_interp() {
        let tokens = tokenize(r#""hello {name}""#);
        assert_eq!(tokens, vec![
            Token::InterpStart,
            Token::String("hello ".into()),
            Token::Ident("name".into()),
            Token::String(String::new()),
            Token::InterpEnd,
        ]);
    }

    #[test]
    fn lex_booleans_and_nil() {
        let tokens = tokenize("true false nil");
        assert_eq!(tokens, vec![Token::True, Token::False, Token::Nil]);
    }

    #[test]
    fn collapse_jsx_whitespace_single_line() {
        assert_eq!(collapse_jsx_whitespace("hello world"), "hello world");
    }

    #[test]
    fn collapse_jsx_whitespace_multiline() {
        assert_eq!(collapse_jsx_whitespace("  hello\n  world  "), "hello world");
    }

    #[test]
    fn collapse_jsx_whitespace_blank_lines() {
        assert_eq!(collapse_jsx_whitespace("a\n\n\nb"), "a b");
    }
}
