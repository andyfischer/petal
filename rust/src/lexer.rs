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
    Var,
    Set,
    Get,
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
    Export,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusPlus,       // ++
    Eq,             // ==
    Ne,             // !=
    Lt,             // <
    Le,             // <=
    Gt,             // >
    Ge,             // >=
    And,            // &&
    Or,             // ||
    DoubleQuestion, // ??
    Bang,           // !
    Assign,         // =
    PlusAssign,     // +=
    MinusAssign,    // -=
    StarAssign,     // *=
    SlashAssign,    // /=
    PercentAssign,  // %=

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
    At,        // @ — in-out argument marker (see ast::ExprKind::AtVar)
    Pipe,      // |>
    Arrow,     // ->
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

/// Every word the lexer turns into a dedicated [`Token`] variant instead of an
/// [`Token::Ident`]. This is the *authoritative* keyword set of the language;
/// the LSP completion list, the tree-sitter grammar, the vim syntax file and
/// the tree-sitter highlight queries are all checked against it by
/// `rust/tests/keyword_sync.rs`.
///
/// Keep it in sync with [`keyword_token`] — the unit tests below enforce that.
pub const KEYWORDS: &[&str] = &[
    "break", "continue", "do", "else", "elsif", "end", "enum", "export", "false", "fn", "for",
    "get", "if", "import", "in", "let", "match", "nil", "return", "set", "state", "then", "true",
    "var", "when", "while",
];

/// Words the parser treats as keywords but the lexer emits as `Ident`.
///
/// `as` is only a keyword directly after the module name in an `import`, so it
/// stays usable as an ordinary identifier everywhere else. `class` opens a
/// declaration only when a name follows it, which keeps it usable as an
/// ordinary identifier and — the sharper constraint — as the JSX attribute
/// `<div class="…">`. Downstream tooling (highlighting, completion) should
/// still treat these as keywords.
pub const CONTEXTUAL_KEYWORDS: &[&str] = &["as", "class"];

/// Map an identifier-shaped word to its keyword token, or `None` if it is an
/// ordinary identifier.
///
/// This is the hottest path in the lexer: it must stay a plain `match` (which
/// rustc compiles to a length switch plus memcmp chain), never a hash lookup or
/// a scan over [`KEYWORDS`].
pub fn keyword_token(text: &str) -> Option<Token> {
    Some(match text {
        "let" => Token::Let,
        "var" => Token::Var,
        "set" => Token::Set,
        "get" => Token::Get,
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
        "export" => Token::Export,
        "true" => Token::True,
        "false" => Token::False,
        "nil" => Token::Nil,
        _ => return None,
    })
}

/// The source text of a keyword token, or `None` for anything that is not a
/// keyword. The inverse of [`keyword_token`].
///
/// Used by the parser where a *name* is expected and no keyword could mean
/// anything else — a record key (`{when: 3}`) or a field access (`r.when`).
/// Keeping keywords usable there is what stops the keyword set from quietly
/// stealing ordinary field names.
pub fn keyword_text(tok: &Token) -> Option<&'static str> {
    Some(match tok {
        Token::Let => "let",
        Token::Var => "var",
        Token::Set => "set",
        Token::Get => "get",
        Token::Fn => "fn",
        Token::If => "if",
        Token::Else => "else",
        Token::For => "for",
        Token::In => "in",
        Token::While => "while",
        Token::Match => "match",
        Token::Return => "return",
        Token::Break => "break",
        Token::Continue => "continue",
        Token::State => "state",
        Token::Enum => "enum",
        Token::End => "end",
        Token::Then => "then",
        Token::Do => "do",
        Token::Elsif => "elsif",
        Token::When => "when",
        Token::Import => "import",
        Token::Export => "export",
        Token::True => "true",
        Token::False => "false",
        Token::Nil => "nil",
        _ => return None,
    })
}

/// Lexer mode for JSX disambiguation.
#[derive(Debug, Clone, PartialEq)]
enum LexerMode {
    Normal,
    JsxTag,     // Inside `<tag ...>` — lexing attributes
    JsxContent, // Between `>` and `</` — lexing children
}

use crate::source_map::{ENTRY_FILE, FileId, SourcePosition, SourceSpan};

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
    /// How many string-interpolation holes (`"… {here} …"`) enclose the
    /// cursor. Inside a hole a `\"` is accepted as a string delimiter, because
    /// an editor (or a person) that escapes the quotes of a nested string is
    /// writing the same thing as a bare `"`. See [`Lexer::read_string_inner`].
    interp_depth: usize,
}

/// How to write a brace that is not an interpolation hole. Shared by both
/// diagnostics below so the tests can assert one phrase.
const LITERAL_BRACE_HELP: &str =
    "write `\\{` for a literal brace, or use a raw string: \"\"\"{\"\"\"";

/// A `{` in a double-quoted string that only ever made sense as a literal
/// brace — the hole after it lexed to a lone string constant, or to nothing at
/// all. Reported at the brace, which is the character the author has to change.
fn literal_brace_error(brace: SourcePosition) -> String {
    format!(
        "A `{{` in a double-quoted string opens an interpolation hole; {} [line {}, column {}]",
        LITERAL_BRACE_HELP, brace.line, brace.column
    )
}

/// A string opened inside an interpolation hole that ran off the end of its
/// line. Reported at that string's opening quote.
fn unclosed_hole_string_error(open_quote: SourcePosition) -> String {
    format!(
        "Unterminated string: a string opened inside an interpolation hole must close on the same line; {} [line {}, column {}]",
        LITERAL_BRACE_HELP, open_quote.line, open_quote.column
    )
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
            interp_depth: 0,
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
        self.token_spans.push(SourceSpan {
            start,
            end,
            file: self.file,
        });
    }

    /// The position of the single non-newline character immediately before the
    /// cursor. Valid only when the last consumed character was not a newline —
    /// used to recover the position of a closing `}` that
    /// [`Lexer::tokenize_braced_expr`] has just consumed without emitting a
    /// token.
    fn prev_char_pos(&self) -> SourcePosition {
        SourcePosition {
            line: self.line,
            column: self.col - 1,
            offset: self.pos as u32 - 1,
        }
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
            '(' => {
                self.advance_char();
                self.push_token(Token::LParen, start);
            }
            ')' => {
                self.advance_char();
                self.push_token(Token::RParen, start);
            }
            '{' => {
                self.advance_char();
                self.push_token(Token::LBrace, start);
                if *self.current_mode() == LexerMode::JsxTag {
                    // Attribute value hole `{...}` inside a JSX open tag. Lex it
                    // as a normal braced expression (in Normal mode) so operators
                    // like `>` and `/>` inside the expression aren't mistaken for
                    // the tag's closing/self-closing delimiter.
                    self.mode_stack.push(LexerMode::Normal);
                    self.tokenize_braced_expr(true, true)?;
                    self.mode_stack.pop();
                }
            }
            '}' => {
                self.advance_char();
                self.push_token(Token::RBrace, start);
            }
            '[' => {
                self.advance_char();
                self.push_token(Token::LBracket, start);
            }
            ']' => {
                self.advance_char();
                self.push_token(Token::RBracket, start);
            }
            ',' => {
                self.advance_char();
                self.push_token(Token::Comma, start);
            }
            ':' => {
                self.advance_char();
                self.push_token(Token::Colon, start);
            }
            '@' => {
                self.advance_char();
                self.push_token(Token::At, start);
            }
            '.' => {
                if self.peek_next() == Some('.') {
                    if self.pos + 2 < self.input.len() && self.input[self.pos + 2] == '.' {
                        self.advance_n(3);
                        self.push_token(Token::DotDotDot, start);
                    } else {
                        self.advance_n(2);
                        self.push_token(Token::DotDot, start);
                    }
                } else if self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
                    // A leading-dot float like `.001` (no integer part).
                    self.read_number()?;
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
                    // `-` is not spacing-aware: whether it negates or subtracts
                    // is decided by the grammar position alone.
                    // See docs/syntax/commas.md.
                    self.advance_char();
                    self.push_token(Token::Minus, start);
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
                if self.peek_next() == Some('>') && *self.current_mode() == LexerMode::JsxTag {
                    // Self-closing JSX tag: `/>`
                    self.advance_n(2);
                    self.push_token(Token::JsxSelfClose, start);
                    self.mode_stack.pop();
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
                    self.mode_stack.pop();
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
                    return Err(format!(
                        "Unexpected character '&' [line {}, column {}]",
                        self.line, self.col
                    ));
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
                    return Err(format!(
                        "Unexpected character '|' [line {}, column {}]",
                        self.line, self.col
                    ));
                }
            }
            '?' => {
                if self.peek_next() == Some('?') {
                    self.advance_n(2);
                    self.push_token(Token::DoubleQuestion, start);
                } else {
                    return Err(format!(
                        "Unexpected character '?' [line {}, column {}]",
                        self.line, self.col
                    ));
                }
            }
            // Inside an interpolation hole, `\"` opens a nested string: a
            // string written inside another string is often escaped by whoever
            // (or whatever) produced the source, and both spellings mean the
            // same thing. See `read_string_inner`.
            '\\' if self.interp_depth > 0 && self.peek_next() == Some('"') => {
                self.read_string_inner(true)?
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
                return Err(format!(
                    "Unexpected character '{}' [line {}, column {}]",
                    ch, self.line, self.col
                ));
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
        self.read_string_inner(false)
    }

    /// Read a string literal starting at the cursor (which sits on the opening
    /// quote).
    ///
    /// `escaped_quotes` is set when the literal was opened by `\"` rather than
    /// `"` — only possible inside a string-interpolation hole, where a nested
    /// string may reasonably be written either way:
    ///
    /// ```text
    /// "v {if t then "a" else "b" end}"
    /// "v {if t then \"a\" else \"b\" end}"
    /// ```
    ///
    /// In that mode a `\"` closes the literal (as does a bare `"`), so the two
    /// spellings lex identically.
    fn read_string_inner(&mut self, escaped_quotes: bool) -> Result<(), String> {
        let open_quote = self.current_pos();
        if escaped_quotes {
            self.advance_char();
        }
        self.advance_char();
        let mut s = String::new();
        let mut has_interp = false;
        // Source position where the current literal part's span begins. It
        // starts just after the opening quote; after each interpolation hole it
        // moves to the closing `}` so the following literal part absorbs that
        // delimiter. This keeps every token span gap-free so the source
        // reconstructs exactly — see `crate::trivia`.
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
                if escaped_quotes && self.peek_next() == Some('"') {
                    // `\"` closes a literal that was opened with `\"`.
                    let close_quote = self.current_pos();
                    self.advance_n(2);
                    if has_interp {
                        self.push_token_span(Token::String(s), part_start, close_quote);
                        let end = self.current_pos();
                        self.push_token_span(Token::InterpEnd, close_quote, end);
                    } else {
                        self.push_token(Token::String(s), open_quote);
                    }
                    return Ok(());
                }
                self.advance_char();
                if self.pos >= self.input.len() {
                    return Err(format!(
                        "Unterminated string escape [line {}, column {}]",
                        self.line, self.col
                    ));
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
            if ch == '\n' && self.interp_depth > 0 {
                // A string opened inside an interpolation hole must close on
                // the same line. Without this the literal runs on until the
                // next `"` anywhere in the file, inverting quote parity for
                // everything after it and blaming some innocent character
                // hundreds of lines later.
                return Err(unclosed_hole_string_error(open_quote));
            }
            if ch == '{' {
                // Start of interpolation.
                let brace = self.current_pos();
                // A hole that opens with a bare `"` is almost always a literal
                // brace the author meant to write (`"{"`), not an expression —
                // see the mis-lex check below. A triple quote really is a raw
                // string and is left alone.
                let hole_opens_with_quote = self.peek_next() == Some('"')
                    && !(self.input.get(self.pos + 2) == Some(&'"')
                        && self.input.get(self.pos + 3) == Some(&'"'));
                if !has_interp {
                    has_interp = true;
                    // InterpStart's span is exactly the opening quote.
                    self.push_token_span(Token::InterpStart, open_quote, part_start);
                }
                // Emit the literal part accumulated so far, spanning from its
                // start through and including this opening `{`.
                self.advance_char();
                let after_brace = self.current_pos();
                self.push_token_span(Token::String(s), part_start, after_brace);
                s = String::new();

                let mark = self.tokens.len();
                let braced = self.tokenize_braced_expr(false, false);
                if hole_opens_with_quote {
                    // Two shapes give the mistake away: the quote that follows
                    // the brace never closed on this line (nothing was pushed),
                    // or the whole hole lexed to a single string constant — as
                    // `"{" ++ name ++ "}"` does, silently, today.
                    let mis_lexed = match &braced {
                        Err(_) => self.tokens.len() == mark,
                        Ok(()) => {
                            self.tokens.len() == mark + 1
                                && matches!(self.tokens[mark], Token::String(_))
                        }
                    };
                    if mis_lexed {
                        return Err(literal_brace_error(brace));
                    }
                }
                braced?;
                // The next literal part absorbs the closing `}` just consumed,
                // so no delimiter is left in an inter-token gap.
                part_start = self.prev_char_pos();
                continue;
            }
            s.push(ch);
            self.advance_char();
        }
        Err(format!(
            "Unterminated string [line {}, column {}]",
            open_quote.line, open_quote.column
        ))
    }

    /// Read a triple-quoted raw string: `"""..."""`. Everything between the
    /// delimiters is captured verbatim — `{`/`}` are literal (no interpolation),
    /// backslashes are not treated as escapes, and raw newlines are allowed.
    /// Useful for embedding source code, e.g. `Program.parse("""...""")`.
    fn read_raw_string(&mut self) -> Result<(), String> {
        let start = self.current_pos();
        self.advance_n(3);
        let mut s = String::new();

        while self.pos < self.input.len() {
            if self.input[self.pos] == '"'
                && self.pos + 2 < self.input.len()
                && self.input[self.pos + 1] == '"'
                && self.input[self.pos + 2] == '"'
            {
                self.advance_n(3);
                self.push_token(Token::String(s), start);
                return Ok(());
            }
            s.push(self.input[self.pos]);
            self.advance_char();
        }
        Err(format!(
            "Unterminated raw string [line {}, column {}]",
            start.line, start.column
        ))
    }

    fn read_number(&mut self) -> Result<(), String> {
        let start_pos = self.current_pos();
        let start = self.pos;
        let mut is_float = false;

        while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
            self.advance_char();
        }

        // Leave the dot for ranges and method calls; consume it only when a digit follows.
        if self.pos + 1 < self.input.len()
            && self.input[self.pos] == '.'
            && self.input[self.pos + 1].is_ascii_digit()
        {
            is_float = true;
            self.advance_char();
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.advance_char();
            }
        }

        // Exponent: `1e9`, `1.5e-3`, `2E+4`. Only consumed when at least one
        // digit follows (optionally after a sign), so `1.method()` and an
        // identifier that merely starts with `e` are untouched.
        if self.pos < self.input.len() && matches!(self.input[self.pos], 'e' | 'E') {
            let mut ahead = self.pos + 1;
            if ahead < self.input.len() && matches!(self.input[ahead], '+' | '-') {
                ahead += 1;
            }
            if ahead < self.input.len() && self.input[ahead].is_ascii_digit() {
                is_float = true;
                self.advance_n(ahead - self.pos);
                while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                    self.advance_char();
                }
            }
        }

        let text: String = self.input[start..self.pos].iter().collect();
        if is_float {
            let f: f64 = text.parse().map_err(|e| format!("Invalid float: {}", e))?;
            self.push_token(Token::Float(f), start_pos);
        } else {
            let n: i64 = text
                .parse()
                .map_err(|e| format!("Invalid integer: {}", e))?;
            self.push_token(Token::Int(n), start_pos);
        }
        Ok(())
    }

    fn read_color(&mut self) -> Result<(), String> {
        let start = self.current_pos();
        self.advance_char();
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
        // Every braced hole — string interpolation and both JSX forms — accepts
        // `\"` as a string delimiter, so the escaped and bare spellings of a
        // nested literal lex identically. See `read_string_inner`.
        self.interp_depth += 1;
        let result = self.tokenize_braced_expr_inner(emit_close, skip_newlines);
        self.interp_depth -= 1;
        result
    }

    fn tokenize_braced_expr_inner(
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
                if ch == '\r' && self.pos < self.input.len() && self.input[self.pos] == '\n' {
                    self.advance_char();
                }
            } else {
                self.tokenize_one()?;
            }
        }
        if depth > 0 {
            return Err(format!(
                "Unterminated braced expression [line {}, column {}]",
                self.line, self.col
            ));
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
            return Err(format!(
                "Expected tag name [line {}, column {}]",
                self.line, self.col
            ));
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
            // A single trailing `?` stays part of a predicate identifier
            // (`is_valid?`), but a `??` is the coalescing operator — stop before
            // it so `foo??bar` splits into `foo`, `??`, `bar`.
            if ch == '?' && self.peek_next() == Some('?') {
                break;
            }
            if ch.is_alphanumeric() || ch == '_' || ch == '?' {
                self.advance_char();
            } else {
                break;
            }
        }

        let text: String = self.input[text_start..self.pos].iter().collect();
        let token = keyword_token(&text).unwrap_or(Token::Ident(text));
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
        lexer
            .tokens
            .into_iter()
            .filter(|t| !matches!(t, Token::Newline | Token::Eof))
            .collect()
    }

    // ---- KEYWORDS / keyword_token agreement -------------------------------
    //
    // `KEYWORDS` is a hand-written const sitting next to a hand-written
    // `match`. A const that can drift from the match it documents would be no
    // better than the five drifted lists `rust/tests/keyword_sync.rs` exists to
    // police, so these two tests pin them together in both directions.

    #[test]
    fn keywords_const_agrees_with_keyword_token() {
        for kw in KEYWORDS {
            assert!(
                keyword_token(kw).is_some(),
                "KEYWORDS lists `{kw}` but keyword_token() does not recognize it"
            );
        }
        for kw in CONTEXTUAL_KEYWORDS {
            assert!(
                keyword_token(kw).is_none(),
                "`{kw}` is documented as contextual but the lexer hard-keywords it"
            );
        }
        // Also lexes as a plain identifier end-to-end.
        for kw in CONTEXTUAL_KEYWORDS {
            assert_eq!(tokenize(kw), vec![Token::Ident((*kw).to_string())]);
        }
    }

    /// The reverse direction: scrape the arms of `keyword_token` out of this
    /// very file and assert the set matches `KEYWORDS` exactly. Deliberately
    /// line-agnostic so rustfmt may reflow the match however it likes.
    #[test]
    fn keyword_token_arms_match_keywords_const() {
        let src = include_str!("lexer.rs");
        let body_start = src
            .find("pub fn keyword_token")
            .expect("keyword_token not found in lexer.rs");
        let body = &src[body_start..];
        let body_end = body.find("\n}").expect("keyword_token body not terminated");
        let body = &body[..body_end];

        // Arms look like `"let" => Token::Let,`. Splitting on `"` makes every
        // odd-indexed chunk a string literal; keep it when the text that
        // follows starts the `=> Token::` arrow.
        let mut scraped = std::collections::BTreeSet::new();
        let parts: Vec<&str> = body.split('"').collect();
        for i in (1..parts.len()).step_by(2) {
            let after = parts.get(i + 1).copied().unwrap_or("");
            if after.trim_start().starts_with("=> Token::") {
                scraped.insert(parts[i].to_string());
            }
        }

        let expected: std::collections::BTreeSet<String> =
            KEYWORDS.iter().map(|s| s.to_string()).collect();
        assert!(
            scraped.len() > 20,
            "scraper found only {} arms — it has stopped working, not the code",
            scraped.len()
        );
        assert_eq!(scraped, expected);
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
    fn lex_float_without_leading_zero() {
        assert_eq!(tokenize(".001"), vec![Token::Float(0.001)]);
        assert_eq!(tokenize(".5"), vec![Token::Float(0.5)]);
        // Leading-dot float inside an expression
        assert_eq!(
            tokenize("x + .5"),
            vec![Token::Ident("x".into()), Token::Plus, Token::Float(0.5)]
        );
        // A bare dot (method access / range) must still lex as before
        assert_eq!(
            tokenize("a.b"),
            vec![
                Token::Ident("a".into()),
                Token::Dot,
                Token::Ident("b".into())
            ]
        );
        assert_eq!(
            tokenize("1..3"),
            vec![Token::Int(1), Token::DotDot, Token::Int(3)]
        );
    }

    #[test]
    fn lex_string_literal() {
        assert_eq!(tokenize(r#""hello""#), vec![Token::String("hello".into())]);
    }

    #[test]
    fn lex_keywords() {
        let tokens = tokenize(
            "let var set fn if else for in while match return break continue state enum end then do elsif when",
        );
        assert_eq!(
            tokens,
            vec![
                Token::Let,
                Token::Var,
                Token::Set,
                Token::Fn,
                Token::If,
                Token::Else,
                Token::For,
                Token::In,
                Token::While,
                Token::Match,
                Token::Return,
                Token::Break,
                Token::Continue,
                Token::State,
                Token::Enum,
                Token::End,
                Token::Then,
                Token::Do,
                Token::Elsif,
                Token::When,
            ]
        );
    }

    #[test]
    fn lex_operators() {
        let tokens = tokenize("+ - * / % ++ == != < <= > >= && || !");
        assert_eq!(
            tokens,
            vec![
                Token::Plus,
                Token::Minus,
                Token::Star,
                Token::Slash,
                Token::Percent,
                Token::PlusPlus,
                Token::Eq,
                Token::Ne,
                Token::Lt,
                Token::Le,
                Token::Gt,
                Token::Ge,
                Token::And,
                Token::Or,
                Token::Bang,
            ]
        );
    }

    #[test]
    fn lex_compound_assignment() {
        let tokens = tokenize("+= -= *= /= %=");
        assert_eq!(
            tokens,
            vec![
                Token::PlusAssign,
                Token::MinusAssign,
                Token::StarAssign,
                Token::SlashAssign,
                Token::PercentAssign,
            ]
        );
    }

    #[test]
    fn lex_delimiters() {
        let tokens = tokenize("( ) { } [ ] , . :");
        assert_eq!(
            tokens,
            vec![
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
                Token::LBracket,
                Token::RBracket,
                Token::Comma,
                Token::Dot,
                Token::Colon,
            ]
        );
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
        assert_eq!(
            tokens,
            vec![
                Token::InterpStart,
                Token::String("hello ".into()),
                Token::Ident("name".into()),
                Token::String(String::new()),
                Token::InterpEnd,
            ]
        );
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

    #[test]
    fn lex_jsx_attr_expr_with_gt() {
        // `>` inside a JSX attribute expression `{...}` is a comparison
        // operator, not the tag's closing delimiter.
        let tokens = tokenize("<div expr={x > 1}/>");
        assert_eq!(
            tokens,
            vec![
                Token::JsxOpenStart,
                Token::JsxTagName("div".into()),
                Token::Ident("expr".into()),
                Token::Assign,
                Token::LBrace,
                Token::Ident("x".into()),
                Token::Gt,
                Token::Int(1),
                Token::RBrace,
                Token::JsxSelfClose,
            ]
        );
    }

    #[test]
    fn lex_jsx_attr_expr_with_self_close_inside() {
        // A `/>` sequence inside a JSX attribute expression must not be
        // mistaken for the tag's self-close.
        let tokens = tokenize("<div expr={6 / 2 > 1}/>");
        assert_eq!(
            tokens,
            vec![
                Token::JsxOpenStart,
                Token::JsxTagName("div".into()),
                Token::Ident("expr".into()),
                Token::Assign,
                Token::LBrace,
                Token::Int(6),
                Token::Slash,
                Token::Int(2),
                Token::Gt,
                Token::Int(1),
                Token::RBrace,
                Token::JsxSelfClose,
            ]
        );
    }
    // ---- numeric literals -------------------------------------------------

    #[test]
    fn lexes_scientific_notation() {
        assert_eq!(tokenize("1e9"), vec![Token::Float(1e9)]);
        assert_eq!(tokenize("1.0e9"), vec![Token::Float(1e9)]);
        assert_eq!(tokenize("1.5e-3"), vec![Token::Float(1.5e-3)]);
        assert_eq!(tokenize("2E+4"), vec![Token::Float(2e4)]);
        assert_eq!(tokenize(".5e2"), vec![Token::Float(50.0)]);
    }

    #[test]
    fn exponent_needs_a_digit_to_be_one() {
        // `e` with nothing numeric after it is an identifier, not an exponent.
        assert_eq!(
            tokenize("1 e"),
            vec![Token::Int(1), Token::Ident("e".into())]
        );
        assert_eq!(
            tokenize("1.method"),
            vec![Token::Int(1), Token::Dot, Token::Ident("method".into())]
        );
    }

    // ---- keyword_text -----------------------------------------------------

    #[test]
    fn keyword_text_inverts_keyword_token() {
        for kw in KEYWORDS {
            let tok = keyword_token(kw).expect("keyword");
            assert_eq!(keyword_text(&tok), Some(*kw));
        }
        assert_eq!(keyword_text(&Token::Ident("when".into())), None);
        assert_eq!(keyword_text(&Token::Plus), None);
    }

    // ---- escaped quotes inside an interpolation hole ----------------------

    #[test]
    fn escaped_quotes_open_a_string_inside_a_hole() {
        let escaped = tokenize(r#""v {if t then \"a\" else \"b\" end}""#);
        let bare = tokenize(r#""v {if t then "a" else "b" end}""#);
        assert_eq!(escaped, bare);
    }

    #[test]
    fn a_backslash_outside_a_hole_is_still_an_error() {
        let mut lexer = Lexer::new("let x = \\");
        assert!(lexer.tokenize().is_err());
    }

    #[test]
    fn escaped_quotes_work_in_a_jsx_hole() {
        let escaped = tokenize(r#"<t>{b ?? \"q\"}</t>"#);
        let bare = tokenize(r#"<t>{b ?? "q"}</t>"#);
        assert_eq!(escaped, bare);
    }

    // ---- a literal `{` in a double-quoted string --------------------------

    fn tokenize_err(src: &str) -> String {
        let mut lexer = Lexer::new(src);
        lexer
            .tokenize()
            .expect_err("expected the lexer to reject this")
    }

    /// The reported bug: a bare-brace literal near the top of a file made every
    /// later non-ASCII character in a string blow up. The blame must land on
    /// the brace's own line, and nothing downstream may be touched.
    #[test]
    fn a_bare_brace_string_is_rejected_at_the_brace() {
        let err = tokenize_err("let open = \"{\"\nprint(\"mid · dot\")\n");
        assert!(err.contains("[line 1, column 13]"), "{err}");
        assert!(err.contains("interpolation hole"), "{err}");
        assert!(err.contains(r#""""{""""#), "{err}");
    }

    /// The one-line spelling used to lex *silently* into an interpolation of
    /// the constant string `" ++ name ++ "`.
    #[test]
    fn a_bare_brace_between_concatenations_is_rejected() {
        let err = tokenize_err(r#"let tok = "{" ++ name ++ "}""#);
        assert!(err.contains("[line 1, column 12]"), "{err}");
        assert!(err.contains("interpolation hole"), "{err}");
    }

    /// The two spellings the diagnostic recommends must both still work, and
    /// must not disturb the non-ASCII characters that used to be blamed.
    #[test]
    fn the_suggested_literal_brace_spellings_still_lex() {
        assert_eq!(
            tokenize(r#""a\{b}c""#),
            vec![Token::String("a{b}c".to_string())]
        );
        assert_eq!(tokenize(r#""""{""""#), vec![Token::String("{".to_string())]);
        assert_eq!(
            tokenize("\"a\\{b}c\"\n\"mid · dot\"\n\"dash — here\"\n\"arrow ↑ up\"")
                .into_iter()
                .filter(|t| matches!(t, Token::String(_)))
                .count(),
            4
        );
    }

    /// A hole whose *first* token is a string but which goes on to do something
    /// is a real interpolation, not a mis-lexed brace.
    #[test]
    fn a_hole_that_starts_with_a_string_but_computes_is_kept() {
        let toks = tokenize(r#""{"pre" ++ x}""#);
        assert!(toks.contains(&Token::PlusPlus), "{toks:?}");
    }

    /// A string opened inside a hole may not run past the end of its line —
    /// that runaway is what inverted quote parity for the rest of the file.
    #[test]
    fn a_string_inside_a_hole_may_not_cross_a_newline() {
        let err = tokenize_err("\"{ x ++ \"y\nz\" }\"");
        assert!(err.contains("must close on the same line"), "{err}");
        assert!(err.contains("[line 1, column 9]"), "{err}");
    }

    /// An unrelated error inside a hole that opens with a string must still be
    /// reported as itself, not swallowed by the literal-brace diagnostic.
    #[test]
    fn a_real_error_after_a_nested_string_is_not_masked() {
        let err = tokenize_err(r#""{"a" ++ &}""#);
        assert!(err.contains("Unexpected character '&'"), "{err}");
    }
}
