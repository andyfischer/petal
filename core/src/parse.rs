use crate::ast::*;
use crate::cst::{Checkpoint, Event, EventBuilder, SyntaxKind};
use crate::lexer::Token;
use crate::source_map::{SourceSpan, ZERO_SPAN};

/// The word that opens a `class` declaration. Contextual rather than a
/// reserved keyword: making it a hard keyword would break `class` as an
/// ordinary identifier and, more sharply, the JSX attribute `<div class="x">`.
/// Listed in [`crate::lexer::CONTEXTUAL_KEYWORDS`] so editor tooling still
/// highlights it.
pub const CLASS_KEYWORD: &str = "class";

/// The contextual `config` modifier (`config let x = …`), only recognized
/// immediately before a binding keyword so it stays usable as an identifier.
/// Listed in [`crate::lexer::CONTEXTUAL_KEYWORDS`] so editor tooling still
/// highlights it.
pub const CONFIG_KEYWORD: &str = "config";
/// The annotation `const(e)`: the `const` keyword read as the name of the
/// builtin, which it is whenever a `(` follows it.
pub const CONST_ANNOTATION: &str = "const";

/// The contextual `key` clause of a `for` header (`for t in tasks key t.id
/// do`), only recognized directly after the iterable so it stays usable as an
/// identifier. Listed in [`crate::lexer::CONTEXTUAL_KEYWORDS`].
pub const KEY_KEYWORD: &str = "key";

/// The keyword a binding statement is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LetKeyword {
    Let,
    Var,
    Const,
}

impl LetKeyword {
    fn text(self) -> &'static str {
        match self {
            LetKeyword::Let => "let",
            LetKeyword::Var => "var",
            LetKeyword::Const => "const",
        }
    }
}

/// The contextual keyword that opens a native interface declaration
/// (`native fn name(...)`): a keyword only directly before `fn`.
pub const NATIVE_KEYWORD: &str = "native";

/// The contextual keyword that opens a test block (`test "title" do … end`):
/// a keyword only at the start of a statement, directly before a string.
pub use crate::ast::TEST_KEYWORD;

/// How a token is spelled in a diagnostic: the source text a reader would
/// recognize (`','`, `']'`, `` `end` ``), never the Rust variant name. Parse
/// errors used to leak `Debug` output — "Expected RBracket, got Comma" names
/// two things that appear nowhere in the program.
/// Exhaustive by construction: every arm is listed, so adding a [`Token`]
/// variant is a compile error here rather than a mystery message at runtime.
/// Keywords read best in backticks, punctuation in quotes.
pub(crate) fn token_desc(tok: &Token) -> String {
    // A word the reader typed.
    let kw = |s: &str| format!("`{s}`");
    // Punctuation the reader typed.
    let op = |s: &str| format!("'{s}'");
    match tok {
        Token::Int(n) => kw(&n.to_string()),
        Token::Float(f) => kw(&f.to_string()),
        Token::String(_) => "a string literal".to_string(),
        Token::Ident(name) | Token::JsxTagName(name) => kw(name),
        Token::JsxText(_) => "text".to_string(),
        Token::Color(hex) => kw(&format!(":{hex}")),
        Token::Newline => "a line break".to_string(),
        Token::Eof => "end of input".to_string(),
        Token::InterpStart | Token::InterpEnd => "a string interpolation".to_string(),
        Token::True => kw("true"),
        Token::False => kw("false"),
        Token::Nil => kw("nil"),
        Token::Let => kw("let"),
        Token::Const => kw("const"),
        Token::Var => kw("var"),
        Token::Set => kw("set"),
        Token::Get => kw("get"),
        Token::Fn => kw("fn"),
        Token::If => kw("if"),
        Token::Else => kw("else"),
        Token::For => kw("for"),
        Token::In => kw("in"),
        Token::While => kw("while"),
        Token::Match => kw("match"),
        Token::Return => kw("return"),
        Token::Break => kw("break"),
        Token::Continue => kw("continue"),
        Token::State => kw("state"),
        Token::Memo => kw("memo"),
        Token::Enum => kw("enum"),
        Token::End => kw("end"),
        Token::Then => kw("then"),
        Token::Do => kw("do"),
        Token::Elsif => kw("elsif"),
        Token::ElseIf => kw("else if"),
        Token::When => kw("when"),
        Token::Import => kw("import"),
        Token::Pub => kw("pub"),
        Token::Export => kw("export"),
        Token::Plus => op("+"),
        Token::Minus => op("-"),
        Token::Star => op("*"),
        Token::Slash => op("/"),
        Token::Percent => op("%"),
        Token::PlusPlus => op("++"),
        Token::Eq => op("=="),
        Token::Ne => op("!="),
        Token::Lt => op("<"),
        Token::Le => op("<="),
        Token::Gt => op(">"),
        Token::Ge => op(">="),
        Token::And => op("&&"),
        Token::Or => op("||"),
        Token::DoubleQuestion => op("??"),
        Token::Bang => op("!"),
        Token::Assign => op("="),
        Token::PlusAssign => op("+="),
        Token::MinusAssign => op("-="),
        Token::StarAssign => op("*="),
        Token::SlashAssign => op("/="),
        Token::PercentAssign => op("%="),
        Token::ConcatAssign => op("++="),
        Token::CoalesceAssign => op("??="),
        Token::LParen => op("("),
        Token::RParen => op(")"),
        Token::LBrace => op("{"),
        Token::RBrace => op("}"),
        Token::LBracket => op("["),
        Token::RBracket => op("]"),
        Token::Comma => op(","),
        Token::Dot => op("."),
        Token::QuestionDot => op("?."),
        Token::Colon => op(":"),
        Token::At => op("@"),
        Token::Pipe => op("|>"),
        Token::Bar => op("|"),
        Token::Arrow => op("->"),
        Token::DotDot => op(".."),
        Token::DotDotDot => op("..."),
        Token::JsxOpenStart => op("<"),
        Token::JsxSelfClose => op("/>"),
        Token::JsxCloseStart => op("</"),
    }
}

pub struct Parser {
    tokens: Vec<Token>,
    token_spans: Vec<SourceSpan>,
    pos: usize,
    /// CST event stream, recorded alongside AST construction. The tree built
    /// from it is the authoritative parse artifact (see
    /// [`crate::cst::parse_source`]); read it back with [`Parser::cst_events`]
    /// after `parse_program` succeeds (on error the stream may be unbalanced).
    events: EventBuilder,
    /// Set when a `?.` was consumed immediately before a `[` — the `a?.[i]`
    /// index spelling. The bracket arm of `parse_postfix` takes it back and
    /// wraps the access it just built.
    pending_optional: bool,
    /// Token position just past the most recent arrow lambda
    /// (`fn(x) -> expr`). An `end` found there is the arrow lambda's
    /// block-form habit, and the error says so (see [`Parser::error_hint`]).
    arrow_lambda_end: Option<usize>,
    /// Token position of the latest `end` that directly follows the closing
    /// `end` of a chain written with `else if`. Source from before `else if`
    /// was one token nested the `if` in the `else` and closed both
    /// (`else if … end end`); read as a chain, the second `end` is one too
    /// many, and the error for an unexpected `end` says so (see
    /// [`Parser::error_hint`]).
    else_if_second_end: Option<usize>,
    /// The titles of the test blocks parsed so far, in source order. A
    /// block's position here names its function ([`crate::ast::test_fn_name`]),
    /// and a title written twice is an error.
    test_titles: Vec<String>,
    /// True while [`Parser::parse_program`] is about to parse a statement of
    /// the file's top level — the only place a test block may be written.
    at_top_level: bool,
    /// Whether each token is the first on its line
    /// ([`crate::lexer::first_on_line`]). Layout is measured at these tokens
    /// only.
    first_on_line: Vec<bool>,
    /// For each token, the column of the first token of its line: the
    /// *indent* of the line. A token after a string that spans lines carries
    /// the indent of the line the string began on.
    line_indent: Vec<u32>,
    /// For each token, how many `(`, `[` and `{` are open in front of it. An
    /// `end` on a line of its own can only close a construct that began at
    /// its own depth.
    depth: Vec<u32>,
    /// The constructs whose bodies are being parsed, outermost first. An
    /// `end` on its own line that sits left of the innermost one is checked
    /// against the rest (see [`Parser::end_is_ours`]).
    open: Vec<OpenBlock>,
    /// How many string interpolation holes (`"… {here} …"`) enclose the
    /// cursor.
    interp_holes: usize,
    /// Whether indentation delimits blocks. Off only in the reading of
    /// [`Parser::by_end`].
    layout: bool,
    /// For each token, whether the indentation of its line holds a tab
    /// ([`Parser::with_tab_indentation`]). Empty when the caller did not
    /// say, which reads every line as indented with spaces.
    tab_line: Vec<bool>,
    /// The index of the first token that is not a line break.
    first_code: usize,
    /// Set for the second reading of [`Parser::parse_program`]: the column
    /// the first line is read at.
    first_line_margin: Option<u32>,
}

/// A construct with a body, while that body is being parsed.
#[derive(Debug, Clone, Copy)]
struct OpenBlock {
    /// The token the construct starts at (`if`, `for`, `fn`, `match`, …). The
    /// indent of its line is the construct's *statement indent*: the column
    /// its `end` must sit at, and the column its body must be indented past.
    head: usize,
    /// The keyword a diagnostic names it by.
    name: &'static str,
}

/// Which closing keywords a body may stop at, and whose they are.
#[derive(Debug, Clone, Copy)]
enum Closers {
    /// The body of the construct that starts at `head`: an `end` closes that
    /// construct, and with `or_else` an `else` / `else if` continues it.
    /// `clause` is the keyword the body's own clause starts at (the `if`,
    /// an `else if`, the `else`).
    Own { head: usize, clause: usize, or_else: bool },
    /// The body of a `when … ->` arm of the `match` that starts at `head`:
    /// the next `when` and the `end` belong to the match. `arm_col` is the
    /// column its arms sit at when they are on lines of their own under the
    /// header, and `None` when the first arm shares the header's line.
    Arm { head: usize, arm_col: Option<u32> },
}

/// A `;` that separated two statements of a one-line body: where it is, and
/// the opener of that body. An error when the construct is not closed by an
/// `end` (B3 of docs/tasks/pythonic-syntax.md).
type Semi = Option<(usize, &'static str)>;

/// A parsed body.
struct Body {
    stmts: Vec<Stmt>,
    semi: Semi,
}

impl Parser {
    pub fn new(tokens: Vec<Token>, token_spans: Vec<SourceSpan>) -> Self {
        let first_on_line = crate::lexer::first_on_line(&tokens, &token_spans);
        let first_code = tokens
            .iter()
            .position(|t| !matches!(t, Token::Newline))
            .unwrap_or(tokens.len());
        let mut line_indent = Vec::with_capacity(tokens.len());
        let mut depth = Vec::with_capacity(tokens.len());
        let (mut indent, mut open) = (1u32, 0u32);
        for (i, tok) in tokens.iter().enumerate() {
            if first_on_line[i] {
                indent = token_spans[i].start.column;
            }
            line_indent.push(indent);
            if matches!(tok, Token::RParen | Token::RBracket | Token::RBrace) {
                open = open.saturating_sub(1);
            }
            depth.push(open);
            if matches!(tok, Token::LParen | Token::LBracket | Token::LBrace) {
                open += 1;
            }
        }
        Self {
            tokens,
            token_spans,
            pos: 0,
            events: EventBuilder::new(),
            pending_optional: false,
            arrow_lambda_end: None,
            else_if_second_end: None,
            test_titles: Vec::new(),
            at_top_level: false,
            first_on_line,
            line_indent,
            depth,
            open: Vec::new(),
            interp_holes: 0,
            layout: true,
            tab_line: Vec::new(),
            first_code,
            first_line_margin: None,
        }
    }

    /// Tell the parser which lines are indented with a tab:
    /// [`crate::lexer::Lexer::token_tab_indented`], parallel to the tokens.
    /// A block is measured in columns and a tab has no agreed width, so a
    /// line whose indentation holds one is an error wherever its column
    /// decides something (B11): the statements of a block, an `end` or an
    /// `else` on a line of its own, the line that opens a block. A line that
    /// only continues a bracket or an expression may hold what it likes.
    pub fn with_tab_indentation(mut self, tab_indented: &[bool]) -> Self {
        let mut on_line = false;
        self.tab_line = (0..self.tokens.len())
            .map(|i| {
                if self.first_on_line[i] {
                    on_line = tab_indented.get(i).copied().unwrap_or(false);
                }
                on_line
            })
            .collect();
        self
    }

    /// Read the source the way it was read before indentation delimited
    /// blocks: every block runs to its `end`, which is required, and columns
    /// mean nothing. For the tools that repair source written that way
    /// whose indentation disagrees with its `end`s (`petal fmt`, which
    /// derives the indentation from the `end`s, and `petal migrate-old-code`).
    /// Nothing compiles this parse: for source both readings accept, they are
    /// the same parse (docs/tasks/pythonic-syntax.md §5.1).
    pub fn by_end(mut self) -> Self {
        self.layout = false;
        self
    }

    /// The tokens and their spans, given back when parsing is done.
    pub fn into_tokens(self) -> (Vec<Token>, Vec<SourceSpan>) {
        (self.tokens, self.token_spans)
    }

    /// The CST events recorded so far.
    pub fn cst_events(&self) -> &[Event] {
        self.events.events()
    }

    // ---- CST event recording ----

    fn ev_open(&mut self, kind: SyntaxKind) {
        self.events.open(kind);
    }

    fn ev_close(&mut self) {
        self.events.close();
    }

    fn ev_checkpoint(&self) -> Checkpoint {
        self.events.checkpoint()
    }

    fn ev_wrap(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.events.wrap(cp, kind);
    }

    pub fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let first = match self.parse_program_once() {
            Ok(stmts) => return Ok(stmts),
            Err(e) => e,
        };
        // Source held in a host-language string often starts at the opening
        // quote, with every later line indented to the host code:
        //
        //     run("import vec2
        //          let v = vec3(1, 2, 3)
        //          print(v)")
        //
        // Read as written, the second line is an unexpected indent. When
        // every later line sits right of the first, the first line is read
        // as sitting at their margin instead. At most one of the two
        // readings parses: the first line either opens a block (and the
        // lines under it are that block) or does not.
        let Some(margin) = self.hosted_margin() else {
            return Err(first);
        };
        self.restart();
        self.first_line_margin = Some(margin);
        // Everything up to the next line start hangs off the first line.
        for i in self.first_code..self.tokens.len() {
            if i > self.first_code && self.first_on_line[i] {
                break;
            }
            self.line_indent[i] = margin;
        }
        self.parse_program_once().map_err(|second| {
            // Report the reading that got further.
            let line_of = |e: &str| {
                e.rfind("[line ")
                    .and_then(|at| e[at + 6..].split(',').next()?.parse::<u32>().ok())
                    .unwrap_or(0)
            };
            if line_of(&second) > line_of(&first) { second } else { first }
        })
    }

    /// The column the first line is read at when the source looks like a
    /// host-language string that starts at its quote (see
    /// [`Parser::parse_program`]): the leftmost column a later line starts
    /// at, when that is right of the first token's. `None` otherwise.
    fn hosted_margin(&self) -> Option<u32> {
        if !self.layout || self.first_line_margin.is_some() {
            return None;
        }
        let first = self.token_spans.get(self.first_code)?;
        let later = (self.first_code + 1..self.tokens.len())
            .filter(|&i| self.first_on_line[i])
            .map(|i| self.token_spans[i].start.column)
            .min()?;
        (later > first.start.column).then_some(later)
    }

    /// Back to the first token, for a second reading of the same tokens.
    fn restart(&mut self) {
        self.pos = 0;
        self.events = EventBuilder::new();
        self.pending_optional = false;
        self.arrow_lambda_end = None;
        self.else_if_second_end = None;
        self.test_titles.clear();
        self.at_top_level = false;
        self.open.clear();
        self.interp_holes = 0;
    }

    fn parse_program_once(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        // The first statement's column is the program's left margin, so a
        // snippet indented as a whole (in a host-language string, a Markdown
        // fence, a paste) reads as it would at column 1.
        let margin = self.column_of(self.pos);
        while !self.is_at_end() {
            let stmt_pos = self.pos;
            if self.layout && self.is_first_on_line(stmt_pos) {
                self.no_tab(stmt_pos)?;
                let col = self.column_of(stmt_pos);
                if col > margin {
                    return Err(self.unexpected_indent(stmt_pos, margin));
                }
                if col < margin {
                    return Err(self.error_at(
                        stmt_pos,
                        format!(
                            "this line is indented less than the first line of the program \
                             (column {col}, and the program starts at column {margin}): \
                             top-level statements share one left margin"
                        ),
                    ));
                }
            }
            self.at_top_level = true;
            let stmt = self.parse_stmt()?;
            // Imports must come before any other statement: resolution runs
            // strictly ahead of the declaration prescan, and execution order
            // (modules first, importer after) stays obvious.
            if matches!(stmt.kind, StmtKind::Import(_))
                && stmts
                    .iter()
                    .any(|s: &Stmt| !matches!(s.kind, StmtKind::Import(_)))
            {
                return Err(self.error_at(
                    stmt_pos,
                    "import statements must appear before any other statement".to_string(),
                ));
            }
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

    /// The token `n` places past the cursor (`peek_at(0)` == [`peek`](Self::peek)).
    fn peek_at(&self, n: usize) -> &Token {
        self.tokens.get(self.pos + n).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        self.pos += 1;
        self.events.token();
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), String> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            let got = self.peek().clone();
            let msg = match (expected, &got) {
                (Token::Arrow, Token::Assign) => {
                    "Expected '->' but got '=' — use '->' for match arms, not '=>'".to_string()
                }
                (Token::RBrace, Token::Eof) => "Missing closing '}'".to_string(),
                (Token::RParen, Token::Eof) => "Missing closing ')'".to_string(),
                (Token::RBracket, Token::Eof) => "Missing closing ']'".to_string(),
                _ => {
                    format!(
                        "Expected {}, got {}",
                        token_desc(expected),
                        token_desc(&got)
                    )
                }
            };
            Err(self.error_at_current(msg))
        }
    }

    /// The `=` of a declaration is missing (`let x`, `var x`, `state x` with no
    /// initializer). Every declaration form requires one, so name the mistake
    /// instead of the token the generic `expect` stopped on.
    fn expect_initializer(&mut self, keyword: &str, name: &str) -> Result<(), String> {
        if matches!(self.peek(), Token::Assign) {
            return self.expect(&Token::Assign);
        }
        Err(self.error_at_current(format!(
            "`{keyword} {name}` needs an initializer; write `{keyword} {name} = ...`"
        )))
    }

    /// The `=` of a `set` is missing (`set x`). Not a missing initializer —
    /// `set` declares nothing — but a write with nothing to write, so it gets
    /// its own wording. Field and index targets are not re-serialized; only a
    /// plain name is echoed back.
    fn expect_set_value(&mut self, target: &Expr) -> Result<(), String> {
        if matches!(self.peek(), Token::Assign) {
            return self.expect(&Token::Assign);
        }
        let msg = match &target.kind {
            ExprKind::Ident(name) => {
                format!("`set {name}` needs a value; write `set {name} = ...`")
            }
            _ => "`set` needs a value; write `set <target> = ...`".to_string(),
        };
        Err(self.error_at_current(msg))
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Token::Eof)
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    /// Is the next token one of `ops`, allowing line breaks in front of it?
    ///
    /// A line that *begins* with a binary operator continues the expression on
    /// the previous line:
    ///
    /// ```text
    /// let total = base
    ///   + bonus
    ///   * multiplier
    /// ```
    ///
    /// None of these operators can begin a statement, so a line break before
    /// one is never a statement boundary — with the single exception of `-`
    /// (unary negation), which is why the leading-operator rule skips it: a
    /// line starting with `-` keeps meaning a fresh negated expression. Break
    /// after the `-` instead (`a -` / `b`), which every operator already
    /// supports.
    ///
    /// Line breaks are consumed only when the operator actually matches, so the
    /// precedence cascade still unwinds to whichever level owns the operator.
    fn at_binary_op(&mut self, ops: &[Token]) -> bool {
        if ops.contains(self.peek()) {
            return true;
        }
        if !matches!(self.peek(), Token::Newline) {
            return false;
        }
        let mut i = self.pos;
        while matches!(self.tokens.get(i), Some(Token::Newline)) {
            i += 1;
        }
        match self.tokens.get(i) {
            Some(tok) if Self::continues_line(tok) && ops.contains(tok) => {
                while self.pos < i {
                    self.advance();
                }
                true
            }
            _ => false,
        }
    }

    /// Operators that may open a continuation line. Everything binary except
    /// `-` (unary negation) and `<` (which also opens a JSX tag).
    fn continues_line(tok: &Token) -> bool {
        matches!(
            tok,
            Token::Plus
                | Token::Star
                | Token::Slash
                | Token::Percent
                | Token::PlusPlus
                | Token::Eq
                | Token::Ne
                | Token::Le
                | Token::Gt
                | Token::Ge
                | Token::And
                | Token::Or
                | Token::DoubleQuestion
                | Token::Pipe
                | Token::Bar
        )
    }

    /// Can this token begin a statement? Tokens that cannot (a closing
    /// delimiter, a separator, a keyword that only ever continues a construct)
    /// end a block instead of being fed to `parse_stmt`: the closing bracket
    /// of a call ends a block lambda written in it, and otherwise the
    /// *enclosing* construct reports what is unfinished. See
    /// [`Parser::ends_body`].
    fn can_start_stmt(tok: &Token) -> bool {
        !matches!(
            tok,
            Token::Eof
                | Token::Comma
                | Token::Colon
                | Token::RParen
                | Token::RBracket
                | Token::RBrace
                | Token::Arrow
                | Token::Then
                | Token::Do
                | Token::In
                | Token::Pipe
                | Token::Bar
        )
    }

    // ---- Layout ----
    //
    // Blocks are closed by indentation, and `end` is an optional closer
    // (docs/tasks/pythonic-syntax.md §3.1). The lexer knows nothing of it:
    // tokens carry their position, and the parser measures a body's column
    // where a block opener is followed by a line break. Three things close a
    // block: a line that starts left of the block, a closing bracket or
    // separator of an enclosing construct, and an `end`.

    /// The column `pos` starts at (1-based); 0 past the last token. The
    /// first token of a source read as a host string's is at the margin of
    /// the lines after it (see [`Parser::parse_program`]).
    fn column_of(&self, pos: usize) -> u32 {
        match self.first_line_margin {
            Some(margin) if pos == self.first_code => margin,
            _ => self.token_spans.get(pos).map_or(0, |s| s.start.column),
        }
    }

    fn is_first_on_line(&self, pos: usize) -> bool {
        self.first_on_line.get(pos).copied().unwrap_or(false)
    }

    /// Refuse to measure the line `pos` is on when its indentation holds a
    /// tab (see [`Parser::with_tab_indentation`]).
    fn no_tab(&self, pos: usize) -> Result<(), String> {
        if !self.tab_line.get(pos).copied().unwrap_or(false) {
            return Ok(());
        }
        let mut first = pos;
        while first > 0 && !self.first_on_line[first] {
            first -= 1;
        }
        Err(self.error_at(
            first,
            "a tab in the indentation of this line: indent with spaces (indentation delimits \
             blocks, and a tab has no fixed width)"
                .to_string(),
        ))
    }

    /// The indent of the line `pos` is on: the *statement indent* of a
    /// construct that starts at `pos`.
    fn indent_of(&self, pos: usize) -> u32 {
        self.line_indent.get(pos).copied().unwrap_or(1)
    }

    /// The position of the next token that is not a line break (or `;`).
    fn next_code(&self) -> usize {
        let mut i = self.pos;
        while matches!(self.tokens.get(i), Some(Token::Newline)) {
            i += 1;
        }
        i
    }

    fn token_at(&self, pos: usize) -> &Token {
        self.tokens.get(pos).unwrap_or(&Token::Eof)
    }

    /// Is the cursor at the end of a line: a line break (not a `;`, which the
    /// lexer also spells [`Token::Newline`]) or the end of the input?
    ///
    /// Where the lexer drops line breaks (a `{…}` hole among an element's
    /// children or attributes), the next token being the first on its line
    /// says the same thing, so a block in a hole is laid out like any other.
    fn at_line_break(&self) -> bool {
        match self.peek() {
            Token::Eof => true,
            Token::Newline => {
                let next = self.next_code();
                self.is_first_on_line(next) || matches!(self.token_at(next), Token::Eof)
            }
            _ => self.is_first_on_line(self.pos),
        }
    }

    /// The error for the line that starts at token `t`, right of the column
    /// `block` its block's statements start at.
    fn unexpected_indent(&self, t: usize, block: u32) -> String {
        let col = self.column_of(t);
        let mut message = format!(
            "unexpected indent: this line starts at column {col}, and the block it is in \
             starts at column {block}"
        );
        // The one operator that does not continue the line above when it
        // leads a line (see [`Parser::at_binary_op`]).
        if matches!(self.token_at(t), Token::Minus) {
            message.push_str(
                " — a line that starts with `-` is a new statement (a negation), not a \
                 continuation of the line above. To continue the expression, end the line \
                 above with the `-`",
            );
        }
        self.error_at(t, message)
    }

    /// Does this token end the body it is found in, whatever its column? A
    /// token no statement starts with: a closing bracket or a separator of
    /// an enclosing construct, or a keyword that closes or continues a
    /// construct.
    fn ends_body(tok: &Token) -> bool {
        !Self::can_start_stmt(tok)
            || matches!(
                tok,
                Token::End | Token::Else | Token::ElseIf | Token::Elsif | Token::When
            )
    }

    /// Does the `end` at `t`, the first token of its line, close the
    /// construct that starts at `head`? It does when it sits at that
    /// construct's statement indent (B5). Left of it, it is left for an
    /// enclosing construct, provided one starts at that column; anything
    /// else is an `end` that lines up with nothing, and is reported here,
    /// against the innermost construct, rather than wherever the stray
    /// token would later surface.
    ///
    /// Two more columns line up with a construct that hangs off the middle
    /// of a line: the column of its own keyword, and the column of the line
    /// its last clause (`clause`: an `else`, an `else if`) starts, as in
    ///
    /// ```text
    /// let title = if event == nil then "none"
    ///     else event.title
    ///     end
    /// ```
    fn end_is_ours(&self, t: usize, head: usize, clause: usize) -> Result<bool, String> {
        let col = self.column_of(t);
        let indent = self.indent_of(head);
        if col == indent
            || col == self.column_of(head)
            || (self.is_first_on_line(clause) && col == self.column_of(clause))
        {
            return Ok(true);
        }
        let inner = self.open.iter().rposition(|b| b.head == head);
        if col < indent {
            let outer = &self.open[..inner.unwrap_or(self.open.len())];
            for block in outer.iter().rev() {
                // An `end` cannot close a construct outside the brackets it
                // is written in.
                if self.depth[block.head] < self.depth[t] {
                    break;
                }
                let outer_indent = self.indent_of(block.head);
                if outer_indent == col {
                    return Ok(false);
                }
                if outer_indent < col {
                    break;
                }
            }
        }
        let name = inner.map_or("block", |i| self.open[i].name);
        let line = self.token_spans.get(head).map_or(0, |s| s.start.line);
        Err(self.error_at(
            t,
            format!(
                "`end` at column {col} does not line up with the `{name}` block opened at \
                 line {line} (column {indent})"
            ),
        ))
    }

    /// Is the closing keyword at `t` (`end`, `else`, `else if`, `when`) the
    /// one the body being parsed stops at? On the line the body is on, it
    /// is: an inline `end` closes the innermost open block, and an `else`
    /// binds to the nearest `if`. On a line of its own, its column decides
    /// (B4, B5). False for any other token.
    fn closer_is_ours(&self, t: usize, closers: Closers) -> Result<bool, String> {
        let tok = self.token_at(t);
        let inline = !self.is_first_on_line(t);
        let col = self.column_of(t);
        if !inline && self.is_closing_keyword(t) {
            self.no_tab(t)?;
            let (Closers::Own { head, .. } | Closers::Arm { head, .. }) = closers;
            self.no_tab(head)?;
        }
        match (closers, tok) {
            (Closers::Own { head, clause, .. }, Token::End) => {
                if inline { Ok(true) } else { self.end_is_ours(t, head, clause) }
            }
            (Closers::Arm { head, .. }, Token::End) => {
                if inline { Ok(true) } else { self.end_is_ours(t, head, head) }
            }
            (
                Closers::Own { head, or_else: true, .. },
                Token::Else | Token::ElseIf | Token::Elsif,
            ) => Ok(inline || col >= self.indent_of(head)),
            (Closers::Arm { head, arm_col }, Token::When) => match arm_col {
                _ if inline => Ok(true),
                Some(arm) if col == arm => Ok(true),
                Some(arm) if col > arm => {
                    Err(self.unexpected_indent(t, arm))
                }
                Some(arm) => {
                    let indent = self.indent_of(head);
                    if col > indent {
                        return Err(self.error_at(t, partial_dedent_message(col, arm, indent)));
                    }
                    Ok(false)
                }
                None => Ok(col >= self.indent_of(head)),
            },
            // A `match` has no `else` arm; the match says so.
            (Closers::Arm { arm_col, .. }, Token::Else | Token::ElseIf | Token::Elsif) => {
                Ok(inline || arm_col.is_some_and(|arm| col >= arm))
            }
            _ => Ok(false),
        }
    }

    /// Parse the body that follows a block opener (`then`, `else`, `do`, the
    /// `->` of an arm, the header of a `fn`), the cursor just past it.
    ///
    /// - **Code on the opener's line** is a one-line body: statements to the
    ///   end of the line, or to a token that ends a body. It extends as far
    ///   as the expression grammar reaches (B1), so `if c then 1 else 2 + 3`
    ///   is `else (2 + 3)`.
    /// - **A line break** opens a block: the next code line must be indented
    ///   past the indent of the line `clause` is on (the keyword that
    ///   starts this clause: `if`, `else`, `for`, `when`, `fn`, …). Its
    ///   column is the block's; every statement starts at that column, and
    ///   the block ends at the first line left of it.
    ///
    /// The closer is not consumed. When the body stops at a closing keyword
    /// that is this construct's ([`Parser::closer_is_ours`]), the line breaks
    /// in front of it are consumed and the cursor is on it; otherwise the
    /// cursor stays at the end of the body's last statement, so a construct
    /// with no `end` spans exactly its own tokens.
    fn parse_body(
        &mut self,
        clause: usize,
        opener: &'static str,
        closers: Closers,
        name: &'static str,
    ) -> Result<Body, String> {
        if !self.layout {
            return self.parse_body_by_end(closers);
        }
        if let Closers::Own { head, .. } = closers {
            self.open.push(OpenBlock { head, name });
        }
        let body = if self.at_line_break() {
            self.parse_block_body(clause, opener, closers)
        } else {
            self.parse_inline_body(opener, closers)
        };
        if matches!(closers, Closers::Own { .. }) {
            self.open.pop();
        }
        body
    }

    /// [`Parser::parse_body`] in the reading of [`Parser::by_end`]:
    /// statements up to the closing keyword, wherever they sit.
    fn parse_body_by_end(&mut self, closers: Closers) -> Result<Body, String> {
        let stops = |tok: &Token| match (closers, tok) {
            (_, Token::End) => true,
            (Closers::Own { or_else, .. }, Token::Else | Token::ElseIf | Token::Elsif) => or_else,
            (Closers::Arm { .. }, Token::When | Token::Else | Token::ElseIf | Token::Elsif) => true,
            _ => false,
        };
        let mut stmts = Vec::new();
        self.skip_newlines();
        self.ev_open(SyntaxKind::Block);
        while Self::can_start_stmt(self.peek()) && !stops(self.peek()) {
            stmts.push(self.parse_stmt()?);
            self.skip_newlines();
        }
        self.ev_close();
        Ok(Body { stmts, semi: None })
    }

    fn parse_block_body(
        &mut self,
        clause: usize,
        opener: &'static str,
        closers: Closers,
    ) -> Result<Body, String> {
        let indent = self.indent_of(clause);
        let first = self.next_code();
        let mut stmts = Vec::new();
        self.no_tab(clause)?;
        if Self::ends_body(self.token_at(first)) {
            // No statements. Legal when the construct is closed right here
            // (`fn noop()` / `end`, `if c then` / `else`); an arm reports
            // its own empty body.
            let ours = self.closer_is_ours(first, closers)?;
            if !ours && !matches!(closers, Closers::Arm { .. }) {
                return Err(self.error_at(first, missing_block_message(opener, indent)));
            }
            if ours {
                self.skip_newlines();
            }
            self.ev_open(SyntaxKind::Block);
            self.ev_close();
            return Ok(Body { stmts, semi: None });
        }
        let block = self.column_of(first);
        if block <= indent {
            if matches!(closers, Closers::Arm { .. }) {
                self.ev_open(SyntaxKind::Block);
                self.ev_close();
                return Ok(Body { stmts, semi: None });
            }
            return Err(self.error_at(first, missing_block_message(opener, indent)));
        }
        self.skip_newlines();
        self.ev_open(SyntaxKind::Block);
        loop {
            let t = self.next_code();
            if Self::ends_body(self.token_at(t)) {
                if self.closer_is_ours(t, closers)? {
                    self.skip_newlines();
                } else if self.is_first_on_line(t) {
                    // A closing keyword that is not ours ends the block like
                    // any other line: by its column.
                    let col = self.column_of(t);
                    if col > indent && col < block && self.is_closing_keyword(t) {
                        return Err(self.error_at(t, partial_dedent_message(col, block, indent)));
                    }
                }
                break;
            }
            if self.is_first_on_line(t) {
                self.no_tab(t)?;
                let col = self.column_of(t);
                if col > block {
                    return Err(self.unexpected_indent(t, block));
                }
                if col < block {
                    if col > indent {
                        return Err(self.error_at(t, partial_dedent_message(col, block, indent)));
                    }
                    break;
                }
            }
            self.skip_newlines();
            stmts.push(self.parse_stmt()?);
        }
        self.ev_close();
        Ok(Body { stmts, semi: None })
    }

    fn is_closing_keyword(&self, t: usize) -> bool {
        matches!(
            self.token_at(t),
            Token::End | Token::Else | Token::ElseIf | Token::Elsif | Token::When
        )
    }

    fn parse_inline_body(&mut self, opener: &'static str, closers: Closers) -> Result<Body, String> {
        let mut stmts = Vec::new();
        let mut semi = None;
        self.ev_open(SyntaxKind::Block);
        loop {
            if self.at_line_break() {
                // The body ends with its line. A closing keyword on a later
                // line may still be this construct's (`if a then b` / `else
                // c`, a legacy `end` on its own line).
                let t = self.next_code();
                if Self::ends_body(self.token_at(t)) && self.closer_is_ours(t, closers)? {
                    self.skip_newlines();
                }
                break;
            }
            if matches!(self.peek(), Token::Newline) {
                // A `;`.
                let at = self.pos;
                self.skip_newlines();
                if !stmts.is_empty() && semi.is_none() && !Self::ends_body(self.peek()) {
                    semi = Some((at, opener));
                }
                continue;
            }
            if Self::ends_body(self.peek()) {
                break;
            }
            // A one-line body in a string's `{…}` hole ends with the hole,
            // which no token marks: what follows is the string's next piece
            // (a string token) or its closing quote.
            if self.interp_holes > 0
                && match self.peek() {
                    Token::InterpEnd => true,
                    Token::String(_) => !stmts.is_empty(),
                    _ => false,
                }
            {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        self.ev_close();
        Ok(Body { stmts, semi })
    }

    /// Finish a construct whose bodies are parsed: consume its `end` when
    /// the cursor is on one (the bodies leave it there only when it is this
    /// construct's). Without an `end` the construct is closed by its layout,
    /// which a `;` in a one-line body makes ambiguous (B3): `if c then a; b`
    /// could end at the `;` or at the end of the line.
    ///
    /// In the reading of [`Parser::by_end`] the `end` is required: `head` is
    /// where the construct started and `what` names it with its article
    /// (``an `if` expression``), for the error that says so.
    fn close_block(&mut self, head: usize, what: &str, semi: Semi) -> Result<(), String> {
        if matches!(self.peek(), Token::End) {
            self.advance();
            return Ok(());
        }
        if !self.layout {
            let start = self.token_spans.get(head).map_or(ZERO_SPAN.start, |s| s.start);
            return Err(self.error_at_current(format!(
                "{what} started at line {} column {} is unclosed; expected `end`",
                start.line, start.column
            )));
        }
        if let Some((at, opener)) = semi {
            return Err(self.error_at(
                at,
                format!(
                    "`;` after a one-line {opener} is ambiguous; close it with `end` or use an \
                     indented block"
                ),
            ));
        }
        Ok(())
    }

    /// Consume the `,` after a member of a `class` or `enum` body, on the
    /// member's line or leading the next one. False when there is none.
    fn member_comma(&mut self) -> bool {
        if !self.layout {
            return false;
        }
        if !matches!(self.token_at(self.next_code()), Token::Comma) {
            return false;
        }
        self.skip_newlines();
        self.advance();
        true
    }

    /// Position the cursor on the next member of a `class` or `enum` body,
    /// or report the end of the body (`false`). `block` is the column the
    /// members sit at when the body is a block under the header, and `None`
    /// when it shares the header's line (`class P x, y end`).
    fn next_member(
        &mut self,
        head: usize,
        block: Option<u32>,
        after_comma: bool,
        what: &str,
        closes: &str,
    ) -> Result<bool, String> {
        if !self.layout {
            // The reading of [`Parser::by_end`]: members to the `end`.
            if after_comma {
                self.skip_newlines();
            } else {
                self.expect_element_separator_of(&Token::End, what, Some(closes))?;
            }
            return Ok(!matches!(self.peek(), Token::End | Token::Eof));
        }
        let t = self.next_code();
        let tok = self.token_at(t).clone();
        if matches!(tok, Token::Eof) {
            return Ok(false);
        }
        if matches!(tok, Token::End) {
            let ours = !self.is_first_on_line(t) || self.end_is_ours(t, head, head)?;
            if ours {
                self.skip_newlines();
            }
            return Ok(false);
        }
        if self.is_first_on_line(t) {
            let Some(block) = block else {
                return Ok(false);
            };
            self.no_tab(t)?;
            let col = self.column_of(t);
            let indent = self.indent_of(head);
            if col > block {
                return Err(self.unexpected_indent(t, block));
            }
            if col < block {
                if col > indent {
                    return Err(self.error_at(t, partial_dedent_message(col, block, indent)));
                }
                return Ok(false);
            }
        }
        if !after_comma {
            return Err(self.error_at(
                t,
                format!("Expected ',' between {what}, or {} to close {closes}", token_desc(&Token::End)),
            ));
        }
        self.skip_newlines();
        Ok(true)
    }

    /// The column a `class` / `enum` / `match` body sits at, the cursor just
    /// past the header: `None` when the body shares the header's line.
    /// `member` says whether a token can start a member.
    fn member_block(
        &self,
        head: usize,
        opener: &'static str,
        member: fn(&Token) -> bool,
    ) -> Result<Option<u32>, String> {
        if !self.layout || !self.at_line_break() {
            return Ok(None);
        }
        let t = self.next_code();
        let indent = self.indent_of(head);
        let col = self.column_of(t);
        self.no_tab(head)?;
        self.no_tab(t)?;
        if matches!(self.token_at(t), Token::End) {
            // An empty body closed by its `end`; the member loop takes it.
            return Ok(Some(indent + 1));
        }
        // A `when` cannot start a statement, so the arms of a `match` may
        // also sit at the column of the `match` itself, as an `else` sits
        // under its `if`. A class field or an enum variant is a bare name,
        // and has to be indented to be told from the next statement.
        let arms = matches!(self.token_at(t), Token::When);
        if member(self.token_at(t)) && (col > indent || (arms && col == indent)) {
            return Ok(Some(col));
        }
        Err(self.error_at(t, missing_block_message(opener, indent)))
    }

    /// Consume the separator between two elements of a delimited,
    /// comma-separated construct (list, argument list, record, pattern, …).
    ///
    /// Commas are **required**: two adjacent elements with nothing but
    /// whitespace between them is a parse error. Newlines around the comma are
    /// insignificant, and a trailing comma before the closing delimiter is
    /// allowed. See docs/syntax/commas.md.
    ///
    /// `what` names the elements for the error message ("list elements").
    fn expect_element_separator(&mut self, close: &Token, what: &str) -> Result<(), String> {
        self.expect_element_separator_of(close, what, None)
    }

    /// [`Parser::expect_element_separator`] for a construct closed by a
    /// *keyword* rather than a bracket. `closes` names it ("class `Point`") so
    /// the error can say what is missing: a body that runs off the end of its
    /// declaration is a forgotten `end`, and blaming the comma alone never
    /// mentions the word the reader has to type.
    fn expect_element_separator_of(
        &mut self,
        close: &Token,
        what: &str,
        closes: Option<&str>,
    ) -> Result<(), String> {
        self.skip_newlines();
        if matches!(self.peek(), Token::Comma) {
            self.advance();
            self.skip_newlines();
            Ok(())
        } else if self.peek() == close || matches!(self.peek(), Token::Eof) {
            Ok(())
        } else {
            Err(self.error_at_current(match closes {
                Some(closes) => format!(
                    "Expected ',' between {what}, or {} to close {closes}",
                    token_desc(close)
                ),
                None => format!("Expected ',' between {what}"),
            }))
        }
    }

    /// The name of the next member of an `end`-delimited body (a class field,
    /// an enum variant). Same as [`Parser::expect_ident`] but names the two
    /// things that could legally appear here, so `class C … <no end>` says so
    /// instead of "Expected identifier, got Let".
    fn expect_member_name(&mut self, what: &str, closes: &str) -> Result<String, String> {
        if let Token::Ident(_) = self.peek() {
            return self.expect_ident();
        }
        let got = self.peek().clone();
        Err(self.error_at_current(format!(
            "Expected {what} or {} to close {closes}, got {}",
            token_desc(&Token::End),
            token_desc(&got)
        )))
    }

    /// Reject a comma where an element is expected — a leading comma (`[,1]`),
    /// a doubled one (`f(1,,2)`), or a stray one after the separator was already
    /// consumed. Naming the construct beats the bare "Unexpected token: Comma"
    /// that `parse_primary` would otherwise produce.
    fn expect_element_start(&mut self, what: &str) -> Result<(), String> {
        if matches!(self.peek(), Token::Comma) {
            return Err(self.error_at_current(format!("Expected {what}, got ','")));
        }
        Ok(())
    }

    /// Get the span of the token at position - 1 (the last consumed token).
    fn prev_span(&self) -> SourceSpan {
        if self.pos > 0 && self.pos - 1 < self.token_spans.len() {
            self.token_spans[self.pos - 1]
        } else {
            ZERO_SPAN
        }
    }

    /// Create a span from start_pos (token index) to the last consumed token.
    fn span_from(&self, start_pos: usize) -> SourceSpan {
        let (start, file) = if start_pos < self.token_spans.len() {
            let s = self.token_spans[start_pos];
            (s.start, s.file)
        } else {
            (ZERO_SPAN.start, ZERO_SPAN.file)
        };
        let end = self.prev_span().end;
        SourceSpan { start, end, file }
    }

    /// Helper to create an Expr with a span from start_pos to the last consumed token.
    fn mk_expr(&self, kind: ExprKind, start_pos: usize) -> Expr {
        Expr {
            kind,
            span: self.span_from(start_pos),
        }
    }

    /// Helper to create a Stmt with a span from start_pos to the last consumed token.
    fn mk_stmt(&self, kind: StmtKind, start_pos: usize) -> Stmt {
        Stmt {
            kind,
            span: self.span_from(start_pos),
            exported: false,
            test: None,
        }
    }

    // ---- Statement Parsing ----

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        let start = self.pos;
        // Whatever this statement is, the statements inside it are nested.
        let top_level = std::mem::take(&mut self.at_top_level);
        // An import binds names for the whole file and is resolved before
        // anything runs, so it only means something at the top of a file. The
        // loader collects top-level imports alone; one written inside a
        // function, a block or a test used to be dropped without a word.
        if !top_level && self.starts_import() {
            return Err(self.error_at_current(
                "an `import` is only allowed at the top of a file, before any other statement, \
                 not inside a function, a block or a test"
                    .to_string(),
            ));
        }
        match self.peek().clone() {
            // `test` is contextual: a keyword only when a string follows it at
            // the start of a statement, which no expression can be.
            Token::Ident(ref w) if w == TEST_KEYWORD && self.starts_test_decl() => {
                if !top_level {
                    return Err(self.error_at_current(
                        "a `test` block is only allowed at the top level of a file, not inside \
                         a function, a loop or another block"
                            .to_string(),
                    ));
                }
                self.parse_test_decl(start)
            }
            Token::Let => self.parse_let(start, false, LetKeyword::Let, false),
            Token::Var => self.parse_let(start, false, LetKeyword::Var, false),
            // `const(e)` is the annotation call (an expression statement);
            // `const NAME = …` is the declaration.
            Token::Const if !matches!(self.peek_at(1), Token::LParen) => {
                self.parse_let(start, false, LetKeyword::Const, false)
            }
            Token::Set => self.parse_set(start),
            // `fn(` is a lambda: an expression statement, and so the value
            // of a one-expression branch or arm (`when 1 -> fn() -> 2`).
            Token::Fn if !matches!(self.peek_at(1), Token::LParen) => {
                self.parse_fn_decl(start, false)
            }
            Token::For => self.parse_for(start),
            Token::While => self.parse_while(start),
            Token::Return => self.parse_return(start),
            Token::Break => {
                self.ev_open(SyntaxKind::BreakStmt);
                self.advance();
                self.ev_close();
                Ok(self.mk_stmt(StmtKind::Break, start))
            }
            Token::Continue => {
                self.ev_open(SyntaxKind::ContinueStmt);
                self.advance();
                self.ev_close();
                Ok(self.mk_stmt(StmtKind::Continue, start))
            }
            Token::State => self.parse_state(start, false),
            Token::Memo => self.parse_memo(start),
            Token::Enum => self.parse_enum_decl(start, false),
            // `class` is contextual, not a reserved word: it stays usable as an
            // identifier (and, crucially, as the JSX attribute `class="..."`).
            // A declaration is the only place `class` is followed by a name.
            Token::Ident(ref w) if w == CLASS_KEYWORD && self.starts_class_decl() => {
                self.parse_class_decl(start, false)
            }
            // `config` is contextual too: only special immediately before a
            // `let`/`var` keyword, so it stays usable as an ordinary name.
            Token::Ident(ref w) if w == CONFIG_KEYWORD && self.starts_config_decl() => {
                if let Some(message) = self.config_modifier_error(1) {
                    return Err(self.error_at_current(message));
                }
                self.parse_let(start, false, LetKeyword::Let, true)
            }
            // `native` is contextual: only a keyword directly before `fn`.
            Token::Ident(ref w) if w == NATIVE_KEYWORD && self.starts_native_decl(0) => {
                self.parse_native_fn_decl(start, false)
            }
            Token::Import => self.parse_import(start, false),
            Token::Pub | Token::Export => self.parse_export(start),
            Token::At => self.parse_annotated(start),
            _ => self.parse_expr_or_assign(start),
        }
    }

    /// Does the statement at the cursor start with `import` (or `pub import`
    /// / `export import`)?
    fn starts_import(&self) -> bool {
        match self.peek() {
            Token::Import => true,
            Token::Pub | Token::Export => {
                matches!(self.tokens.get(self.pos + 1), Some(Token::Import))
            }
            _ => false,
        }
    }

    /// `pub <decl>` — a modifier on a top-level `fn`/`let`/`state`/`enum`
    /// that makes the declared name visible to importers (see
    /// `docs/module-system.md`). The `pub` token is consumed inside the
    /// declaration's own CST node so the tree round-trips; the resulting
    /// [`Stmt`] carries `exported = true`. Routes on the token *after* `pub`.
    ///
    /// `export` is the deprecated spelling of the same modifier and takes
    /// every path here that `pub` does; only the error text below, which
    /// quotes whichever word was written, can tell them apart. Reporting the
    /// old spelling is [`crate::migrate::export_keyword`]'s job.
    fn parse_export(&mut self, start: usize) -> Result<Stmt, String> {
        match self.tokens.get(self.pos + 1) {
            Some(Token::Fn) => self.parse_fn_decl(start, true),
            Some(Token::Let) => self.parse_let(start, true, LetKeyword::Let, false),
            Some(Token::Var) => self.parse_let(start, true, LetKeyword::Var, false),
            Some(Token::Const) => self.parse_let(start, true, LetKeyword::Const, false),
            // `pub config let x = …` — both modifiers, `pub` first.
            Some(Token::Ident(w))
                if w == CONFIG_KEYWORD
                    && matches!(self.tokens.get(self.pos + 2), Some(Token::Let)) =>
            {
                self.parse_let(start, true, LetKeyword::Let, true)
            }
            Some(Token::Ident(w))
                if w == CONFIG_KEYWORD && self.config_modifier_error(2).is_some() =>
            {
                let message = self.config_modifier_error(2).unwrap_or_default();
                Err(self.error_at_current(message))
            }
            Some(Token::State) => self.parse_state(start, true),
            Some(Token::Enum) => self.parse_enum_decl(start, true),
            Some(Token::Ident(w)) if w == CLASS_KEYWORD => self.parse_class_decl(start, true),
            Some(Token::Ident(w)) if w == NATIVE_KEYWORD && self.starts_native_decl(1) => {
                self.parse_native_fn_decl(start, true)
            }
            // `pub @tunable let …`: the annotations come first.
            Some(Token::At) => Err(self.error_at(
                self.pos + 1,
                "annotations go before `pub`: write `@… pub let NAME = …`".to_string(),
            )),
            // `pub import m` — a re-export: the names this file imports
            // become its own exports (docs/module-system.md#re-exporting).
            Some(Token::Import) => self.parse_import(start, true),
            _ => {
                let word = match self.tokens.get(self.pos) {
                    Some(Token::Export) => "export",
                    _ => "pub",
                };
                Err(self.error_at_current(format!(
                    "`{word}` must be followed by a fn, native fn, let, const, var, state, enum, \
                     class, or import declaration"
                )))
            }
        }
    }

    /// `@name` / `@name(args)` declaration annotations and the declaration
    /// they annotate (docs/proposals/annotation-syntax.md, D1).
    ///
    /// `@f(a, b) let X = v` is exactly `let X = f(v, a, b)`: the declared
    /// value goes in as the first argument, and stacked annotations apply
    /// bottom-up (the one nearest the declaration is innermost). `f` is one of
    /// [`crate::ast::ANNOTATION_NAMES`]. The annotations may share the
    /// declaration's line or sit on lines of their own above it, and they go
    /// before `pub`.
    ///
    /// The AST gets the call ([`crate::ast::apply_annotations`]), so nothing
    /// after the parser can tell the spellings apart except by spans
    /// ([`crate::ast::is_annotation_call`]). The CST keeps each annotation as
    /// an [`SyntaxKind::Annotation`] node, the first children of the
    /// declaration's node.
    ///
    /// The declarations that take one are the bindings whose initializer the
    /// call form can wrap: `let` (and `pub let`), and also `var`, `state`,
    /// `memo` and the deprecated `const NAME =` / `config let`, where the
    /// lowered call meets the diagnostics the call form meets there. `@`
    /// before a `fn` is reserved for compile-time annotations (F1), and a
    /// class or enum takes none.
    fn parse_annotated(&mut self, start: usize) -> Result<Stmt, String> {
        let outer = self.ev_checkpoint();
        let mut annotations = Vec::new();
        let mut positions = Vec::new();
        while matches!(self.peek(), Token::At) {
            positions.push(self.pos);
            annotations.push(self.parse_annotation()?);
            self.skip_newlines();
        }
        let first = &annotations[0].name;
        // What the annotations are written on.
        enum Target {
            Let(bool, LetKeyword, bool),
            State(bool),
            Memo,
        }
        let config_at = |p: &Self, n: usize| {
            matches!(p.peek_at(n), Token::Ident(w) if w == CONFIG_KEYWORD)
                && matches!(p.peek_at(n + 1), Token::Let)
        };
        let target = match self.peek() {
            Token::Let => Target::Let(false, LetKeyword::Let, false),
            Token::Var => Target::Let(false, LetKeyword::Var, false),
            Token::Const if !matches!(self.peek_at(1), Token::LParen) => {
                Target::Let(false, LetKeyword::Const, false)
            }
            Token::State => Target::State(false),
            Token::Memo => Target::Memo,
            _ if config_at(self, 0) => Target::Let(false, LetKeyword::Let, true),
            Token::Pub | Token::Export => match self.peek_at(1) {
                Token::Let => Target::Let(true, LetKeyword::Let, false),
                Token::Var => Target::Let(true, LetKeyword::Var, false),
                Token::Const => Target::Let(true, LetKeyword::Const, false),
                Token::State => Target::State(true),
                _ if config_at(self, 1) => Target::Let(true, LetKeyword::Let, true),
                Token::At => {
                    return Err(self.error_at(
                        self.pos + 1,
                        "annotations go before `pub`: write `@… pub let NAME = …`".to_string(),
                    ));
                }
                Token::Fn => return Err(self.error_at(start, annotation_on_fn_message(first))),
                Token::Ident(w) if w == NATIVE_KEYWORD => {
                    return Err(self.error_at(start, annotation_on_fn_message(first)));
                }
                Token::Enum => {
                    return Err(self.error_at(start, annotation_on_type_message(first, "an enum")));
                }
                Token::Ident(w) if w == CLASS_KEYWORD => {
                    return Err(self.error_at(start, annotation_on_type_message(first, "a class")));
                }
                _ => return Err(self.error_at_current(ANNOTATION_NEEDS_DECL.to_string())),
            },
            Token::Fn if !matches!(self.peek_at(1), Token::LParen) => {
                return Err(self.error_at(start, annotation_on_fn_message(first)));
            }
            Token::Ident(w) if w == NATIVE_KEYWORD && self.starts_native_decl(0) => {
                return Err(self.error_at(start, annotation_on_fn_message(first)));
            }
            Token::Enum => {
                return Err(self.error_at(start, annotation_on_type_message(first, "an enum")));
            }
            Token::Ident(w) if w == CLASS_KEYWORD && self.starts_class_decl() => {
                return Err(self.error_at(start, annotation_on_type_message(first, "a class")));
            }
            _ => return Err(self.error_at_current(ANNOTATION_NEEDS_DECL.to_string())),
        };
        // Only the reserved annotations: `@` never calls an arbitrary
        // function. The suggestion spells the call the author can write.
        for (ann, &pos) in annotations.iter().zip(&positions) {
            if !crate::ast::ANNOTATION_NAMES.contains(&ann.name.as_str()) {
                let decl = self.annotated_decl_preview(&ann.name);
                return Err(self.error_at(
                    pos,
                    format!(
                        "`@{}` is not an annotation. `@` takes `const`, `tunable` or `rigid`. \
                         To pass the value through a function, write `{decl}`",
                        ann.name
                    ),
                ));
            }
        }
        let inner = self.ev_checkpoint();
        let mut stmt = match target {
            Target::Let(exported, keyword, is_config) => {
                self.parse_let(start, exported, keyword, is_config)?
            }
            Target::State(exported) => self.parse_state(start, exported)?,
            Target::Memo => self.parse_memo(start)?,
        };
        // The declaration's node takes the annotations in as its first
        // children.
        self.events.hoist_open(inner, outer);
        match &mut stmt.kind {
            StmtKind::Let { value: v, .. }
            | StmtKind::State { init: v, .. }
            | StmtKind::Memo { init: v, .. } => {
                let value = std::mem::replace(
                    v,
                    Expr {
                        kind: ExprKind::Literal(Literal::Nil),
                        span: ZERO_SPAN,
                    },
                );
                *v = crate::ast::apply_annotations(value, annotations);
            }
            _ => unreachable!("the target is a binding"),
        }
        Ok(stmt)
    }

    /// One `@name` or `@name(args)`, as an [`SyntaxKind::Annotation`] node.
    /// The name is checked by the caller, which knows what is annotated.
    fn parse_annotation(&mut self) -> Result<crate::ast::Annotation, String> {
        self.ev_open(SyntaxKind::Annotation);
        let at = self.pos;
        self.advance(); // consume '@'
        let name = match self.peek() {
            Token::Ident(name) => name.clone(),
            // `const` is still a keyword token.
            Token::Const => CONST_ANNOTATION.to_string(),
            _ => {
                return Err(self.error_at(
                    at,
                    "`@` must be followed by an annotation name: `@const`, `@tunable` or \
                     `@rigid`"
                        .to_string(),
                ));
            }
        };
        let name_pos = self.pos;
        let adjacent = self.token_spans.get(at).zip(self.token_spans.get(name_pos)).is_none_or(
            |(a, n)| a.end.offset == n.start.offset,
        );
        if !adjacent {
            return Err(self.error_at(
                at,
                format!("write the annotation `@{name}` with no space after the `@`"),
            ));
        }
        self.advance(); // consume the name
        let name_span = self.prev_span();
        let (args, arg_names) = if matches!(self.peek(), Token::LParen) {
            self.ev_open(SyntaxKind::ArgList);
            self.advance();
            let open = self.pos;
            let (args, arg_names) = self.parse_arg_list()?;
            self.expect(&Token::RParen)?;
            self.ev_close(); // ArgList
            if arg_names.iter().any(|n| n.as_deref() == Some("value")) {
                return Err(self.error_at(
                    open,
                    format!(
                        "`@{name}(…)` passes the declared value as its first argument; it \
                         cannot also be named `value:`"
                    ),
                ));
            }
            (args, arg_names)
        } else {
            (Vec::new(), Vec::new())
        };
        self.ev_close(); // Annotation
        Ok(crate::ast::Annotation {
            name,
            name_span,
            args,
            arg_names,
        })
    }

    /// `let X = name(v)` for the declaration at the cursor, for the
    /// unknown-annotation error: `v` is the initializer when it is one
    /// literal token, else `…`.
    fn annotated_decl_preview(&self, name: &str) -> String {
        let mut i = 0;
        let mut words = Vec::new();
        loop {
            match self.peek_at(i) {
                Token::Pub => words.push("pub".to_string()),
                Token::Export => words.push("export".to_string()),
                Token::Ident(w) if w == CONFIG_KEYWORD && i < 2 => words.push(w.clone()),
                tok => {
                    if let Some(kw) = crate::lexer::keyword_text(tok) {
                        words.push(kw.to_string());
                    }
                    break;
                }
            }
            i += 1;
        }
        let decl_name = match self.peek_at(i + 1) {
            Token::Ident(n) => n.clone(),
            _ => "X".to_string(),
        };
        // Skip a `: type` to the `=`.
        let mut j = i + 2;
        while !matches!(self.peek_at(j), Token::Assign | Token::Newline | Token::Eof) {
            j += 1;
        }
        let value = match (self.peek_at(j), self.peek_at(j + 1), self.peek_at(j + 2)) {
            (Token::Assign, tok, Token::Newline | Token::Eof) => literal_token_text(tok),
            _ => None,
        }
        .unwrap_or_else(|| "…".to_string());
        format!("{} {decl_name} = {name}({value})", words.join(" "))
    }

    /// What is wrong with `config` in front of the binding keyword `ahead`
    /// tokens on, when something is: only a `let` can be a config binding.
    fn config_modifier_error(&self, ahead: usize) -> Option<String> {
        match self.tokens.get(self.pos + ahead) {
            Some(Token::Var) => Some(
                "`config` marks a `let` binding; a mutable `var` cell cannot be config".to_string(),
            ),
            Some(Token::Const) => Some(
                "`config` marks a `let` binding; write `config let` (a config binding of \
                 literals is already constant) or a plain `const`"
                    .to_string(),
            ),
            Some(Token::State) => Some(
                "`config` marks a `let` binding; a `state` slot cannot be config. Declare the \
                 tunable starting value with `config let` and initialise the state from it"
                    .to_string(),
            ),
            Some(Token::Fn) => Some(
                "`config` marks a `let` binding of literals; a function cannot be config"
                    .to_string(),
            ),
            Some(Token::Pub | Token::Export) => Some(
                "`pub` comes before `config`: write `pub config let`".to_string(),
            ),
            _ => None,
        }
    }

    /// `let x = …`, its mutable twin `var x = …` and its must-be-static twin
    /// `const X = …`, which differ only in the keyword and the `is_var` /
    /// `is_const` flags. The `var` or `const` token stays a direct child of
    /// the `LetStmt` node so the CST projection can recover the flag the same
    /// way it recovers `pub`.
    fn parse_let(
        &mut self,
        start: usize,
        exported: bool,
        keyword: LetKeyword,
        is_config: bool,
    ) -> Result<Stmt, String> {
        let is_var = keyword == LetKeyword::Var;
        let is_const = keyword == LetKeyword::Const;
        self.ev_open(SyntaxKind::LetStmt);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        if is_config {
            // The `config` ident stays a direct child of the LetStmt node so
            // the CST projection can recover the flag, like `pub`/`var`.
            self.advance(); // consume 'config'
        }
        self.advance(); // consume 'let' / 'var' / 'const'
        let name = self.expect_ident()?;
        let ty = self.parse_type_annotation()?;
        self.expect_initializer(keyword.text(), &name)?;
        let value = self.parse_expr()?;
        self.ev_close();
        let mut stmt = self.mk_stmt(
            StmtKind::Let {
                name,
                ty,
                value,
                is_var,
                is_config,
                is_const,
            },
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    /// `set x = …` / `set r.f = …` / `set xs[i] = …` — a write through a `var`
    /// cell. The target is parsed as an ordinary expression and converted with
    /// the same [`expr_to_assign_target`] the `=` form uses, so field and index
    /// targets and their error message come free. Compound forms
    /// (`set x += 1`) desugar exactly as `x += 1` does.
    fn parse_set(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::SetStmt);
        self.advance(); // consume 'set'
        let target_expr = self.parse_expr()?;
        let (target, value) = if let Some(op) = self.peek_compound_assign_op() {
            self.advance(); // consume the compound operator
            let rhs = self.parse_expr()?;
            // `set x += 1` desugars like `x += 1`: the value spans the whole
            // statement, which is what the CST projection reproduces.
            //
            // The read half is spelled `get x`, because `set` only ever writes
            // a cell: `set x += 1` means `set x = get x + 1`. Synthesizing the
            // bare `Ident` instead would make the statement demand a `get` the
            // author never had anywhere to write, since the read here has no
            // source text of its own.
            let left = cell_get_at_root(target_expr.clone());
            let value = Expr {
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(rhs),
                },
                span: self.span_from(start),
            };
            (expr_to_assign_target(target_expr)?, value)
        } else {
            self.expect_set_value(&target_expr)?;
            let value = self.parse_expr()?;
            (expr_to_assign_target(target_expr)?, value)
        };
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::Set { target, value }, start))
    }

    fn parse_state(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::StateStmt);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume 'state'

        // Check for explicit key: state(expr) name = init
        let key = if matches!(self.peek(), Token::LParen) {
            self.advance(); // consume '('
            let key_expr = self.parse_expr()?;
            self.expect(&Token::RParen)?;
            Some(key_expr)
        } else {
            None
        };

        // `state(key) var name = init` — the `var` modifier follows the
        // optional key group, mirroring the order they are written.
        let is_var = if matches!(self.peek(), Token::Var) {
            self.advance();
            true
        } else {
            false
        };

        let name = self.expect_ident()?;
        // `state x: int = …` takes the same annotation slot `let`/`var` do.
        let ty = self.parse_type_annotation()?;
        self.expect_initializer(if is_var { "state var" } else { "state" }, &name)?;
        let init = self.parse_expr()?;
        self.ev_close();
        let mut stmt = self.mk_stmt(
            StmtKind::State {
                name,
                ty,
                init,
                key,
                is_var,
            },
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    /// `memo(key) name = init`, optionally `memo(key) name: type = init`. The
    /// key group is required and there is no `var` form: a memo is a cache of a
    /// pure result, so it has nothing to say without a key and nothing to write
    /// afterwards (docs/features/memo.md).
    fn parse_memo(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::MemoStmt);
        self.advance(); // consume 'memo'

        if !matches!(self.peek(), Token::LParen) {
            return Err(self.error_at_current(
                "`memo` needs a key: write `memo(key) name = ...`. A value built once is a \
                 `const`; a value kept per callsite is a `state`"
                    .to_string(),
            ));
        }
        self.advance(); // consume '('
        let key = self.parse_expr()?;
        self.expect(&Token::RParen)?;

        if matches!(self.peek(), Token::Var) {
            return Err(self.error_at_current(
                "a `memo` is immutable, so there is no `memo var`. Shared data the program \
                 writes belongs in a top-level `state var`"
                    .to_string(),
            ));
        }

        let name = self.expect_ident()?;
        let ty = self.parse_type_annotation()?;
        self.expect_initializer("memo(...)", &name)?;
        let init = self.parse_expr()?;
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::Memo { name, ty, key, init }, start))
    }

    /// After the `:` or a `,` of a selective import list, with a newline at the
    /// cursor: does the list continue on a later line?
    ///
    /// Only a name that is itself followed by another `,` or the end of its
    /// line continues the list, so a trailing comma in front of an ordinary
    /// statement (`import a: b,` then `print(x)`) still ends the import rather
    /// than swallowing the next line.
    fn import_list_continues(&self) -> bool {
        let mut n = 0;
        while matches!(self.peek_at(n), Token::Newline) {
            n += 1;
        }
        matches!(self.peek_at(n), Token::Ident(_))
            && matches!(
                self.peek_at(n + 1),
                Token::Comma | Token::Newline | Token::Eof
            )
    }

    /// One segment of a module path. Only an identifier is a segment: `.` and
    /// `..` are rejected here rather than at resolution time, so a path can
    /// never climb out of the directory it is resolved against.
    fn expect_path_segment(&mut self) -> Result<String, String> {
        if matches!(
            self.peek(),
            Token::Dot | Token::DotDot | Token::DotDotDot | Token::Slash
        ) {
            return Err(self.error_at_current(
                "a module path segment must be an identifier ('.' and '..' are only allowed \
                 at the start of an import path: `./name`, `../name`)"
                    .to_string(),
            ));
        }
        self.expect_ident()
    }

    /// `import m` / `import m as u` / `import m: a, b`, where `m` is a module
    /// path of one or more identifier segments joined by `/`
    /// (`import bloom/menu`). The path is the module's identity; the local
    /// name it binds is its last segment, which `as` overrides.
    ///
    /// The path may instead be a quoted string (`import "./my-lib" as lib`),
    /// for a file or directory name that is not an identifier. A quoted path
    /// whose last segment is not a name needs `as` (or a selective list),
    /// since there is no qualifier to bind it under otherwise.
    ///
    /// A selective name list may wrap: a line break is allowed after the `:`
    /// and after each `,`, and a trailing comma is allowed. Every other import
    /// form still ends at the newline; `as` is contextual (not a keyword).
    ///
    /// `pub import …` (`exported = true` here) makes what this file imports
    /// part of what it exports — the declarative facade described in
    /// docs/module-system.md#re-exporting. `import m: *` is a form that no
    /// longer exists; it is still parsed (`star`) so the loader can say so
    /// and `petal migrate-old-code` can rewrite it.
    fn parse_import(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ImportStmt);
        if exported {
            // The `pub` token stays a direct child of the ImportStmt node so
            // the CST projection can recover the flag, as it does for `let`.
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume 'import'
        // A quoted path (`import "./my-lib"`) spells a module whose path is
        // not a run of identifiers — a hyphenated file or directory name. It
        // means exactly what the same path written bare would.
        let path_pos = self.pos;
        let mut module = String::new();
        let quoted = matches!(self.peek(), Token::String(_));
        if let Token::String(path) = self.peek().clone() {
            self.advance(); // consume the path string
            if let Some(err) = crate::ast::quoted_import_path_error(&path) {
                return Err(self.error_at(path_pos, err));
            }
            module = path;
        } else if matches!(self.peek(), Token::InterpStart) {
            return Err(self.error_at_current(
                "an import path is a plain string: it may not interpolate `{…}` \
                 (write `\\{` for a literal brace)"
                    .to_string(),
            ));
        } else {
            self.parse_bare_import_path(&mut module)?;
        }

        let mut alias = None;
        let mut names = None;
        let mut star = false;
        match self.peek().clone() {
            Token::Ident(kw) if kw == "as" => {
                self.advance(); // consume 'as'
                alias = Some(self.expect_ident()?);
            }
            Token::Colon => {
                self.advance(); // consume ':'
                if matches!(self.peek(), Token::Newline) && self.import_list_continues() {
                    self.skip_newlines();
                }
                // `: *` — the removed spelling of a plain import; kept in
                // the AST for the loader's error and for `migrate-old-code`.
                if matches!(self.peek(), Token::Star) {
                    self.advance(); // consume '*'
                    star = true;
                    self.expect_import_end(&module, quoted, true)?;
                    return self.finish_import(start, module, alias, None, star, exported);
                }
                let mut list = Vec::new();
                loop {
                    list.push(self.expect_ident()?);
                    if !matches!(self.peek(), Token::Comma) {
                        break;
                    }
                    self.advance(); // consume ','
                    if matches!(self.peek(), Token::Newline) {
                        // A trailing comma: the newline ends the statement.
                        if !self.import_list_continues() {
                            break;
                        }
                        self.skip_newlines();
                    }
                }
                names = Some(list);
            }
            _ => {}
        }

        // A plain import binds the path's last segment as its qualifier, so
        // that segment has to be a name. `import "./my-lib"` cannot bind
        // `my-lib`; it needs an `as` (a selective import binds no qualifier).
        if quoted && alias.is_none() && names.is_none() {
            let local = crate::ast::module_local_name(&module);
            if !crate::ast::is_plain_identifier(local) {
                let suggestion = crate::ast::suggested_qualifier(local);
                return Err(self.error_at(
                    path_pos,
                    format!(
                        "`{local}` is not a name, so `import \"{module}\"` cannot bind it as \
                         the module's qualifier — write `import \"{module}\" as {suggestion}`, \
                         or import names from it: `import \"{module}\": a, b`"
                    ),
                ));
            }
        }

        self.expect_import_end(&module, quoted, names.is_some())?;
        self.finish_import(start, module, alias, names, star, exported)
    }

    /// An import ends at the end of its line. Statements need no separator,
    /// so without this check whatever follows was parsed as a new statement
    /// and failed far from the cause: `import ./my-mod` read as `import ./my`
    /// then the expression `-mod`. Each common slip gets the fix spelled out.
    fn expect_import_end(&self, module: &str, quoted: bool, selective: bool) -> Result<(), String> {
        let next = self.peek().clone();
        let after = self.tokens.get(self.pos + 1).cloned();
        let word_after = match &after {
            Some(Token::Ident(w)) => Some(w.clone()),
            _ => None,
        };
        let message = match next {
            Token::Newline | Token::Eof => return Ok(()),
            Token::Minus if !quoted => {
                let rest = word_after.unwrap_or_default();
                let path = format!("{module}-{rest}");
                let local = crate::ast::module_local_name(&path);
                format!(
                    "a module name with a `-` in it must be quoted: write `import \"{path}\" as \
                     {}`",
                    crate::ast::suggested_qualifier(local)
                )
            }
            Token::Dot if word_after.as_deref() == Some("ptl") => format!(
                "an import names a module, not a file: drop the extension and write `import \
                 {module}`"
            ),
            Token::Dot => match word_after {
                Some(w) => format!(
                    "module paths are separated by `/`, not `.`: write `import {module}/{w}`"
                ),
                None => "module paths are separated by `/`, not `.`".to_string(),
            },
            Token::Ident(ref w) if w == "as" && selective => {
                "a selective import cannot rename the names it brings in: import them as they \
                 are, then bind a new name with `let`, or import the module `as` a qualifier"
                    .to_string()
            }
            other => format!(
                "unexpected {} after an import: an import ends at the end of its line",
                token_desc(&other)
            ),
        };
        Err(self.error_at_current(message))
    }

    /// The bare (unquoted) spelling of an import path: a relative prefix —
    /// `./` (beside the importing file) or one or more `../` (above it) —
    /// then identifier segments joined by `/`. Anything without a prefix
    /// starts with a package name. The prefix is kept in the path as written.
    fn parse_bare_import_path(&mut self, module: &mut String) -> Result<(), String> {
        if matches!(self.peek(), Token::Dot) {
            self.advance(); // consume '.'
            if !matches!(self.peek(), Token::Slash) {
                return Err(self.error_at_current(
                    "a relative import is written `./name` or `../name`".to_string(),
                ));
            }
            self.advance(); // consume '/'
            module.push_str("./");
        } else {
            while matches!(self.peek(), Token::DotDot) {
                self.advance(); // consume '..'
                if !matches!(self.peek(), Token::Slash) {
                    return Err(self.error_at_current(
                        "a relative import is written `./name` or `../name`".to_string(),
                    ));
                }
                self.advance(); // consume '/'
                module.push_str("../");
            }
        }
        module.push_str(&self.expect_path_segment()?);
        while matches!(self.peek(), Token::Slash) {
            self.advance(); // consume '/'
            let segment = self.expect_path_segment()?;
            module.push('/');
            module.push_str(&segment);
        }
        Ok(())
    }

    /// Close the `ImportStmt` node and build the statement. Shared by the two
    /// exits from [`parse_import`] (`: *` returns early, since nothing may
    /// follow the star).
    fn finish_import(
        &mut self,
        start: usize,
        module: String,
        alias: Option<String>,
        names: Option<Vec<String>>,
        star: bool,
        exported: bool,
    ) -> Result<Stmt, String> {
        // A nested path binds its last segment by default, so `bloom/menu`
        // reads as `menu.open`. A flat name keeps `alias: None` and binds under
        // the module name itself, exactly as before.
        let alias = match alias {
            None if module.contains('/') => {
                Some(crate::ast::module_local_name(&module).to_string())
            }
            other => other,
        };

        self.ev_close();
        let mut stmt = self.mk_stmt(
            StmtKind::Import(ImportDecl {
                module,
                alias,
                names,
                star,
                exported,
            }),
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    fn parse_fn_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::FnDecl);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume 'fn'
        // `fn Rect.center_x(…)` declares a method: the name before the dot is
        // the receiver's class, and the binding is the qualified name. A dot is
        // otherwise impossible here, so no lookahead beyond one token.
        let first = self.expect_ident()?;
        let (class, name) = if matches!(self.peek(), Token::Dot) {
            self.advance(); // consume '.'
            let method = self.expect_ident()?;
            (
                Some(first.clone()),
                crate::classes::qualified_method_name(&first, &method),
            )
        } else {
            (None, first)
        };
        self.ev_open(SyntaxKind::ParamList);
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        self.ev_close(); // ParamList
        let ret = self.parse_return_type()?;
        let closers = Closers::Own { head: start, clause: start, or_else: false };
        let Body { stmts: body, semi } =
            self.parse_body(start, "the `fn` header", closers, "fn")?;
        self.close_block(start, &format!("the function `{name}`"), semi)?;
        self.ev_close(); // FnDecl
        let mut stmt = self.mk_stmt(
            StmtKind::FnDecl {
                name,
                class,
                params,
                ret,
                body,
            },
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    /// `test` opens a test block only when a string literal follows it.
    fn starts_test_decl(&self) -> bool {
        // An interpolated string is no name for a test, and no expression
        // either: it is taken for the attempt it is, and refused by name.
        matches!(
            self.tokens.get(self.pos + 1),
            Some(Token::String(_) | Token::InterpStart)
        )
    }

    /// `test "title" do … end` — a block a test runner finds and calls, and an
    /// ordinary run never does. Carried as a function of no parameters (see
    /// [`Stmt::test`]), so its body is checked and compiled like any other.
    fn parse_test_decl(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::TestDecl);
        self.advance(); // consume 'test'
        let title_pos = self.pos;
        let Token::String(title) = self.peek().clone() else {
            return Err(self.error_at_current(
                "the name of a test is a plain string, with no `{…}` holes".to_string(),
            ));
        };
        self.advance(); // consume the title
        if !matches!(self.peek(), Token::Do) {
            return Err(self.error_at_current(format!(
                "Expected `do` after the name of the test, got {}. A test is written \
                 `test \"name\" do … end`, and its name is a plain string (no `{{…}}` holes)",
                token_desc(&self.peek().clone())
            )));
        }
        if title.trim().is_empty() {
            return Err(self.error_at(title_pos, "a test needs a name".to_string()));
        }
        if self.test_titles.contains(&title) {
            return Err(self.error_at(
                title_pos,
                format!("this file already has a test named \"{title}\""),
            ));
        }
        self.advance(); // consume 'do'
        let closers = Closers::Own { head: start, clause: start, or_else: false };
        let Body { stmts: body, semi } = self.parse_body(start, "`do`", closers, "test")?;
        self.close_block(start, &format!("the test \"{title}\""), semi)?;
        self.ev_close(); // TestDecl
        let name = crate::ast::test_fn_name(self.test_titles.len());
        self.test_titles.push(title.clone());
        let mut stmt = self.mk_stmt(
            StmtKind::FnDecl {
                name,
                class: None,
                params: Vec::new(),
                ret: None,
                body,
            },
            start,
        );
        stmt.test = Some(title);
        Ok(stmt)
    }

    /// Whether the `native` identifier `offset` tokens ahead opens a
    /// declaration — i.e. `fn` follows it. `native` anywhere else is a name.
    fn starts_native_decl(&self, offset: usize) -> bool {
        matches!(self.tokens.get(self.pos + offset + 1), Some(Token::Fn))
    }

    /// `native fn name(a: int, b?, ...rest) -> type = "host_name"` — the
    /// declaration of a function the host implements. It has no body and no
    /// `end`: everything after the signature is the next statement.
    ///
    /// The parameter grammar is a `fn`'s with two differences, both because a
    /// native has no body to put an expression in: an optional parameter is
    /// written `name?` rather than `name = expr`, and the list may end in a
    /// rest parameter `...name`.
    fn parse_native_fn_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::NativeFnDecl);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume the contextual `native`
        self.advance(); // consume 'fn'
        // The core package declares the annotation `native fn const(value)`.
        let first = if matches!(self.peek(), Token::Const) {
            self.advance();
            CONST_ANNOTATION.to_string()
        } else {
            self.expect_ident()?
        };
        let (class, name) = if matches!(self.peek(), Token::Dot) {
            self.advance(); // consume '.'
            let method = self.expect_ident()?;
            (
                Some(first.clone()),
                crate::classes::qualified_method_name(&first, &method),
            )
        } else {
            (None, first)
        };
        self.ev_open(SyntaxKind::ParamList);
        self.expect(&Token::LParen)?;
        let mut params: Vec<Param> = Vec::new();
        let mut rest: Option<String> = None;
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            if let Some(rest) = &rest {
                return Err(self.error_at_current(format!(
                    "The rest parameter '...{rest}' must be the last parameter"
                )));
            }
            if matches!(self.peek(), Token::DotDotDot) {
                self.ev_open(SyntaxKind::RestParam);
                self.advance(); // consume '...'
                rest = Some(self.expect_ident()?);
                self.ev_close();
                self.expect_element_separator(&Token::RParen, "parameters")?;
                continue;
            }
            self.expect_element_start("a parameter name")?;
            let name_pos = self.pos;
            // A member name, so a keyword works (`end`): the name is only
            // ever a call's argument label, which takes keywords too.
            let written = self.expect_field_name()?;
            let (pname, optional) = match written.strip_suffix('?') {
                Some(base) if !base.is_empty() => (base.to_string(), true),
                _ => (written, false),
            };
            let ty = self.parse_type_annotation()?;
            if matches!(self.peek(), Token::Assign) {
                return Err(self.error_at_current(format!(
                    "A native fn parameter cannot have a default value — write '{pname}?' for \
                     one a call may leave off"
                )));
            }
            if !optional && let Some(prev) = params.iter().rev().find(|p| p.optional) {
                let msg = format!(
                    "Parameter '{pname}' is required but follows '{}?', which is optional — \
                     optional parameters must come after all required parameters",
                    prev.name
                );
                return Err(self.error_at(name_pos, msg));
            }
            if params.iter().any(|p| p.name == pname) {
                return Err(self.error_at(
                    name_pos,
                    format!("Parameter '{pname}' is declared twice"),
                ));
            }
            params.push(Param {
                name: pname,
                ty,
                default: None,
                default_in_body: false,
                optional,
            });
            self.expect_element_separator(&Token::RParen, "parameters")?;
        }
        self.expect(&Token::RParen)?;
        self.ev_close(); // ParamList
        let ret = self.parse_return_type()?;
        let host = if matches!(self.peek(), Token::Assign) {
            self.ev_open(SyntaxKind::NativeBinding);
            self.advance(); // consume '='
            let host = match self.peek().clone() {
                Token::String(s) => {
                    self.advance();
                    s
                }
                _ => {
                    return Err(self.error_at_current(
                        "Expected the host native's name as a string after '=' \
                         (native fn local_name(...) = \"host_name\")"
                            .to_string(),
                    ));
                }
            };
            self.ev_close();
            Some(host)
        } else {
            None
        };
        if !matches!(self.peek(), Token::Newline | Token::Eof) {
            return Err(self.error_at_current(format!(
                "A native fn declaration has no body: expected the end of the line after \
                 the signature of `{name}`"
            )));
        }
        self.ev_close(); // NativeFnDecl
        let mut stmt = self.mk_stmt(
            StmtKind::NativeFn(crate::ast::NativeFnDecl {
                name,
                class,
                params,
                rest,
                ret,
                host,
            }),
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    /// Whether the `class` identifier at the cursor opens a declaration —
    /// i.e. the next token is a name. `class` alone (a variable called `class`,
    /// a JSX `class=` attribute, `class.foo`) is left to expression parsing.
    fn starts_class_decl(&self) -> bool {
        matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
    }

    /// `config` starts a declaration only when the very next token is a
    /// binding keyword — `config` on its own line stays an expression.
    fn starts_config_decl(&self) -> bool {
        // `let` is the declaration; the rest are mistakes that get a message
        // of their own from `config_modifier_error` rather than falling
        // through to "undefined variable `config`".
        matches!(
            self.tokens.get(self.pos + 1),
            Some(
                Token::Let | Token::Var | Token::Const | Token::State | Token::Fn | Token::Pub
                    | Token::Export
            )
        )
    }

    /// `class Name` … `end`, a comma-separated list of `field: type`. Field
    /// annotations are optional and follow the same grammar as a parameter's,
    /// so an un-annotated field is `any`.
    ///
    /// Fields follow the same comma rule as every other delimited,
    /// comma-separated construct, `enum` bodies included (docs/syntax/commas.md):
    /// a newline is not a separator, and a trailing comma before `end` is fine.
    fn parse_class_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ClassDecl);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume the contextual `class`
        let name = self.expect_ident()?;
        let closes = format!("class `{name}`");
        let block = self.member_block(start, "the `class` header", |tok| {
            matches!(tok, Token::Ident(_))
        })?;
        self.open.push(OpenBlock { head: start, name: "class" });
        let mut fields: Vec<ClassFieldDecl> = Vec::new();
        let mut after_comma = true;
        while self.next_member(start, block, after_comma, "class fields", &closes)? {
            let field_start = self.pos;
            self.ev_open(SyntaxKind::ClassField);
            let field_name = self.expect_member_name("a field name", &closes)?;
            let ty = self.parse_type_annotation()?;
            self.ev_close(); // ClassField
            fields.push(ClassFieldDecl {
                name: field_name,
                ty,
                span: self.span_from(field_start),
            });
            after_comma = self.member_comma();
        }
        self.open.pop();
        self.close_block(start, "a `class` declaration", None)?;
        self.ev_close(); // ClassDecl
        let mut stmt = self.mk_stmt(StmtKind::ClassDecl { name, fields }, start);
        stmt.exported = exported;
        Ok(stmt)
    }

    fn parse_enum_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::EnumDecl);
        if exported {
            self.advance(); // consume 'pub' / 'export'
        }
        self.advance(); // consume 'enum'
        let name = self.expect_ident()?;
        let closes = format!("enum `{name}`");
        let block = self.member_block(start, "the `enum` header", |tok| {
            matches!(tok, Token::Ident(_))
        })?;
        self.open.push(OpenBlock { head: start, name: "enum" });
        let mut variants = Vec::new();
        let mut after_comma = true;
        while self.next_member(start, block, after_comma, "enum variants", &closes)? {
            let variant_start = self.pos;
            self.ev_open(SyntaxKind::EnumVariant);
            let variant_name = self.expect_member_name("a variant name", &closes)?;
            let mut fields = Vec::new();
            let mut data = Vec::new();
            if matches!(self.peek(), Token::LParen) {
                self.ev_open(SyntaxKind::ParamList);
                self.advance(); // consume '('
                self.skip_newlines();
                while !matches!(self.peek(), Token::RParen | Token::Eof) {
                    self.expect_element_start("a field name")?;
                    // A bare identifier is a payload field; `key: expr` is
                    // constant data attached to the variant.
                    let cp = self.ev_checkpoint();
                    let field = self.expect_ident()?;
                    match self.peek() {
                        Token::Colon => {
                            self.advance();
                            self.skip_newlines();
                            let value = self.parse_expr()?;
                            self.ev_wrap(cp, SyntaxKind::EnumData);
                            data.push(EnumData { key: field, value });
                        }
                        Token::Assign => {
                            return Err(self.error_at_current(
                                "An enum variant's fields cannot have default values \
                                 (write `key: value` to attach constant data to the variant)"
                                    .to_string(),
                            ));
                        }
                        _ => fields.push(field),
                    }
                    self.expect_element_separator(&Token::RParen, "variant fields")?;
                }
                self.expect(&Token::RParen)?;
                self.ev_close();
            }
            self.ev_close(); // EnumVariant
            variants.push(EnumVariant {
                name: variant_name,
                fields,
                data,
                span: self.span_from(variant_start),
            });
            after_comma = self.member_comma();
        }
        self.open.pop();
        self.close_block(start, "an `enum` declaration", None)?;
        self.ev_close();
        let mut stmt = self.mk_stmt(StmtKind::EnumDecl { name, variants }, start);
        stmt.exported = exported;
        Ok(stmt)
    }

    fn parse_for(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ForStmt);
        let (var, iter, key, body, flatten) = self.parse_for_inner()?;
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::For { var, iter, key, body, flatten }, start))
    }

    /// The expression form of `for`, reached only in value position (`x = for …`,
    /// `f(for …)`, `return for …`): evaluates to a list of each iteration's last
    /// expression. A `for` that begins a statement stays [`parse_for`] (side
    /// effects, no collection).
    fn parse_for_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::ForExpr);
        let (var, iter, key, body, flatten) = self.parse_for_inner()?;
        self.ev_close();
        Ok(self.mk_expr(
            ExprKind::For {
                var,
                iter: Box::new(iter),
                key,
                body,
                flatten,
            },
            start,
        ))
    }

    /// Shared body of the statement and expression `for` forms: consumes
    /// `for <var> in <iter> [key <expr>] {, <var> in <iter> [key <expr>]} do
    /// <body> end` and returns the outermost loop's parts, later generators
    /// desugared into nested loops ([`crate::ast::desugar_multi_for`]).
    fn parse_for_inner(&mut self) -> Result<crate::ast::DesugaredFor, String> {
        let for_pos = self.pos;
        self.advance(); // consume 'for'
        let mut gens = vec![self.parse_for_generator()?];
        while matches!(self.peek(), Token::Comma) {
            self.ev_open(SyntaxKind::ForGenerator);
            self.advance(); // consume ','
            self.skip_newlines();
            gens.push(self.parse_for_generator()?);
            self.ev_close();
        }
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let closers = Closers::Own { head: for_pos, clause: for_pos, or_else: false };
        let Body { stmts: body, semi } = self.parse_body(for_pos, "`do`", closers, "for")?;
        self.close_block(for_pos, "a `for` loop", semi)?;
        let end = self.span_from(for_pos);
        crate::ast::desugar_multi_for(gens, body, end.end, end.file)
    }

    /// One `<var> in <iter> [key <expr>]` clause of a `for` header.
    fn parse_for_generator(&mut self) -> Result<crate::ast::ForGenerator, String> {
        let start = self.span_from(self.pos).start;
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        let key = if matches!(self.peek(), Token::Ident(w) if w == KEY_KEYWORD) {
            self.ev_open(SyntaxKind::ForKey);
            self.advance(); // consume 'key'
            let k = self.parse_expr()?;
            self.ev_close();
            Some(Box::new(k))
        } else {
            None
        };
        Ok(crate::ast::ForGenerator { var, iter, key, start })
    }

    /// `while` is statement-only (no expression / collecting form).
    fn parse_while(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::WhileStmt);
        self.advance(); // consume 'while'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let closers = Closers::Own { head: start, clause: start, or_else: false };
        let Body { stmts: body, semi } = self.parse_body(start, "`do`", closers, "while")?;
        self.close_block(start, "a `while` loop", semi)?;
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::While { condition, body }, start))
    }

    fn parse_return(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ReturnStmt);
        self.advance(); // consume 'return'
        // A bare `return` ends where its body does: at the end of the line or
        // at a token that ends a body (`foo(fn() return)`).
        let stmt = if matches!(self.peek(), Token::Newline) || Self::ends_body(self.peek()) {
            self.mk_stmt(StmtKind::Return(None), start)
        } else {
            let expr = self.parse_expr()?;
            self.mk_stmt(StmtKind::Return(Some(expr)), start)
        };
        self.ev_close();
        Ok(stmt)
    }

    fn parse_expr_or_assign(&mut self, start: usize) -> Result<Stmt, String> {
        let cp = self.ev_checkpoint();
        let expr = self.parse_expr()?;

        if matches!(self.peek(), Token::Assign) {
            self.advance(); // consume '='
            let value = self.parse_expr()?;
            let target = expr_to_assign_target(expr)?;
            self.ev_wrap(cp, SyntaxKind::AssignStmt);
            Ok(self.mk_stmt(StmtKind::Assign { target, value }, start))
        } else if let Some(op) = self.peek_compound_assign_op() {
            self.advance(); // consume the compound assignment token
            let rhs = self.parse_expr()?;
            // Desugar: target op= rhs  →  target = target op rhs
            let target = expr_to_assign_target(expr.clone())?;
            let value = Expr {
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(expr),
                    right: Box::new(rhs),
                },
                span: self.span_from(start),
            };
            self.ev_wrap(cp, SyntaxKind::AssignStmt);
            Ok(self.mk_stmt(StmtKind::Assign { target, value }, start))
        } else {
            self.ev_wrap(cp, SyntaxKind::ExprStmt);
            Ok(self.mk_stmt(StmtKind::Expr(expr), start))
        }
    }

    fn peek_compound_assign_op(&self) -> Option<BinOp> {
        match self.peek() {
            Token::PlusAssign => Some(BinOp::Add),
            Token::MinusAssign => Some(BinOp::Sub),
            Token::StarAssign => Some(BinOp::Mul),
            Token::SlashAssign => Some(BinOp::Div),
            Token::PercentAssign => Some(BinOp::Mod),
            Token::ConcatAssign => Some(BinOp::Concat),
            Token::CoalesceAssign => Some(BinOp::Coalesce),
            _ => None,
        }
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, String> {
        let mut params: Vec<Param> = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            self.expect_element_start("a parameter name")?;
            let name_pos = self.pos;
            let name = self.expect_ident()?;
            let ty = self.parse_type_annotation()?;
            let default = self.parse_param_default()?;
            // A call fills parameters left to right, so one that can be left
            // out may only be followed by others that can.
            if default.is_none()
                && let Some(prev) = params.iter().rev().find(|p| p.default.is_some())
            {
                let msg = format!(
                    "Parameter '{name}' has no default value but follows '{}', which has one \
                     — parameters with defaults must come after all required parameters",
                    prev.name
                );
                return Err(self.error_at(name_pos, msg));
            }
            params.push(Param {
                name,
                ty,
                default,
                default_in_body: false,
                optional: false,
            });
            self.expect_element_separator(&Token::RParen, "parameters")?;
        }
        Ok(params)
    }

    /// Parse an optional `= expr` default on a parameter. Consumes nothing and
    /// returns `Ok(None)` when the next token isn't `=`. The `=` and the
    /// expression are wrapped in a `ParamDefault` CST node, so a parameter
    /// without one keeps its old CST shape.
    fn parse_param_default(&mut self) -> Result<Option<Expr>, String> {
        if !matches!(self.peek(), Token::Assign) {
            return Ok(None);
        }
        self.ev_open(SyntaxKind::ParamDefault);
        self.expect(&Token::Assign)?;
        self.skip_newlines();
        let expr = self.parse_expr()?;
        self.ev_close();
        Ok(Some(expr))
    }

    /// Parse an optional `: type` annotation. Consumes nothing and returns
    /// `Ok(None)` when the next token isn't a colon. The `:` and type-name
    /// identifier are wrapped in a `TypeAnnotation` CST node so the projected
    /// AST can recover the type while un-annotated code keeps its old CST shape.
    ///
    /// Type names are contextual (not reserved): `int`/`float`/`str` are still
    /// callable builtins elsewhere. The raw name is always preserved in the AST
    /// as a [`TypeAnn`]; an unrecognized name resolves to `None` so the checker
    /// can later warn on it.
    fn parse_type_annotation(&mut self) -> Result<Option<TypeAnn>, String> {
        if !matches!(self.peek(), Token::Colon) {
            return Ok(None);
        }
        self.ev_open(SyntaxKind::TypeAnnotation);
        self.expect(&Token::Colon)?;
        let name_pos = self.pos;
        let name = self.expect_type_name()?;
        self.ev_close();
        Ok(Some(TypeAnn::at(name, self.span_from(name_pos))))
    }

    /// Parse an optional `-> type` return annotation on a named `fn`. Returns
    /// `Ok(None)` when the next token isn't `->`. The `->` and type-name
    /// identifier are wrapped in a `ReturnType` CST node. Unambiguous because a
    /// named fn body is a block (`… end`), so `->` is not otherwise valid here
    /// (unlike lambdas, whose `->` introduces the body — hence lambdas have no
    /// return-type annotation). The raw name is preserved as a [`TypeAnn`], as
    /// with parameters.
    fn parse_return_type(&mut self) -> Result<Option<TypeAnn>, String> {
        if !matches!(self.peek(), Token::Arrow) {
            return Ok(None);
        }
        self.ev_open(SyntaxKind::ReturnType);
        self.expect(&Token::Arrow)?;
        let name_pos = self.pos;
        let name = self.expect_type_name()?;
        self.ev_close();
        Ok(Some(TypeAnn::at(name, self.span_from(name_pos))))
    }

    /// Get the span of the current token (the one at self.pos).
    fn current_span(&self) -> SourceSpan {
        if self.pos < self.token_spans.len() {
            self.token_spans[self.pos]
        } else {
            ZERO_SPAN
        }
    }

    /// Format an error message with the current token's source position.
    fn error_at_current(&self, msg: String) -> String {
        let msg = self.with_hint(self.pos, msg);
        let span = self.current_span();
        if span.start.line > 0 {
            format!(
                "{} [line {}, column {}]",
                msg, span.start.line, span.start.column
            )
        } else {
            msg
        }
    }

    /// A fix for the habits from other languages that surface as a parse error
    /// somewhere near, but not naming, the actual mistake: `a and b`, `not a`,
    /// and an `end` after an arrow lambda's expression body. `pos` is the
    /// token the error is reported at.
    fn error_hint(&self, pos: usize) -> Option<String> {
        let tok = |i: usize| self.tokens.get(i);
        let fixed = |s: &str| Some(s.to_string());
        match tok(pos) {
            Some(Token::Ident(w)) if w == "and" => {
                return fixed("Petal spells logical and as `&&`");
            }
            Some(Token::Ident(w)) if w == "or" => {
                return fixed("Petal spells logical or as `||`");
            }
            Some(Token::Ident(w)) if w == "not" => {
                return fixed("Petal spells logical not as `!`");
            }
            _ => {}
        }
        if let Some(prev) = pos.checked_sub(1)
            && let Some(Token::Ident(w)) = tok(prev)
        {
            match w.as_str() {
                "not" => return fixed("Petal spells logical not as `!`"),
                "and" => return fixed("Petal spells logical and as `&&`"),
                "or" => return fixed("Petal spells logical or as `||`"),
                _ => {}
            }
        }
        if matches!(tok(pos), Some(Token::End))
            && let Some(end) = self.arrow_lambda_end
            && end <= pos
            && (end..pos).all(|i| matches!(tok(i), Some(Token::Newline)))
        {
            return fixed(
                "an arrow lambda (`fn(x) -> expr`) takes no `end`; \
                 drop the `end`, or write the block form `fn(x) expr end`",
            );
        }
        // An `end` too many after a chain written with `else if`: the shape
        // source from before the `else if` token has, where the `if` was
        // nested in the `else` and each took an `end`.
        if matches!(tok(pos), Some(Token::End))
            && let Some(second) = self.else_if_second_end
            && second <= pos
        {
            let line = self.token_spans.get(second).map_or(0, |s| s.start.line);
            return Some(format!(
                "`else if` continues its `if` chain, and the whole chain takes one `end`: \
                 if the `end` at line {line} closed an `if` nested in an `else` \
                 (`else if … end end`), delete it, or {} [{}]",
                crate::migrate::RUN_HINT,
                crate::migrate::ELSE_IF_NESTING
            ));
        }
        None
    }

    fn with_hint(&self, pos: usize, msg: String) -> String {
        match self.error_hint(pos) {
            Some(hint) => format!("{msg} — {hint}"),
            None => msg,
        }
    }

    /// Format an error at a specific token position.
    fn error_at(&self, pos: usize, msg: String) -> String {
        let msg = self.with_hint(pos, msg);
        if pos < self.token_spans.len() {
            let span = self.token_spans[pos];
            if span.start.line > 0 {
                format!(
                    "{} [line {}, column {}]",
                    msg, span.start.line, span.start.column
                )
            } else {
                msg
            }
        } else {
            msg
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        let pos = self.pos;
        match self.advance() {
            Token::Ident(name) => Ok(name),
            other => Err(self.error_at(
                pos,
                format!("Expected an identifier, got {}", token_desc(&other)),
            )),
        }
    }

    /// Read a type name in type position (after `:` or `->`). Accepts a plain
    /// identifier, plus the two type-vocabulary names that lex as keywords
    /// (`nil` → [`Token::Nil`], `enum` → [`Token::Enum`]). Without this, the
    /// documented `nil`/`enum` annotations would hit a hard "Expected identifier"
    /// parse error even though the checker recognizes them (see
    /// docs/dev/type-declarations-plan.md §2 grammar).
    /// A member name: an identifier, or any keyword. Used where a *name* is
    /// the only thing that can appear — a record key (`{when: 3}`, `{end: 1}`)
    /// and a field access (`r.when`). No keyword is ambiguous in either spot,
    /// and reserving the whole keyword set as field names is a papercut that
    /// costs a program its most natural word.
    fn expect_field_name(&mut self) -> Result<String, String> {
        if let Some(kw) = crate::lexer::keyword_text(self.peek()) {
            self.advance();
            return Ok(kw.to_string());
        }
        self.expect_ident()
    }

    fn expect_type_name(&mut self) -> Result<String, String> {
        let pos = self.pos;
        let mut name = match self.advance() {
            Token::Ident(name) => name,
            Token::Nil => "nil".to_string(),
            Token::Enum => "enum".to_string(),
            other => {
                return Err(self.error_at(
                    pos,
                    format!("Expected a type name, got {}", token_desc(&other)),
                ));
            }
        };
        // `sh.Circle`: a class or enum another module exports, named through
        // the qualifier its import binds (docs/proposals/module-name-collisions.md,
        // part A). One qualifier only; the name is kept joined, as written,
        // and resolved against the file's imports by the class table.
        if !name.ends_with('?')
            && matches!(self.peek(), Token::Dot)
            && matches!(self.peek_at(1), Token::Ident(_))
        {
            self.advance(); // '.'
            if let Token::Ident(member) = self.advance() {
                name = format!("{name}.{member}");
            }
        }
        // A type is a single bare name; `list<int>` and friends are not a thing
        // (type-declarations-plan.md §1). Nothing valid follows a type name with
        // `<`, so claim the token and say what is actually wrong — otherwise the
        // mistake surfaces as whatever the *next* construct complains about,
        // which differs by position (a missing initializer for `let`/`state`, a
        // missing comma in a param list, and an unclosed JSX element for a
        // return type, since `<int>` lexes as a tag).
        // `int??` lexes as `int` then `??`: one `?` is the whole nullable
        // spelling (docs/proposals/nullable-types.md), and nothing valid
        // follows a type name with `??`, `??=` or `?.`.
        if matches!(
            self.peek(),
            Token::DoubleQuestion | Token::CoalesceAssign | Token::QuestionDot
        ) {
            return Err(self.error_at(
                self.pos,
                format!(
                    "a type takes at most one `?`: write `{}?` for \"a `{}` or nil\" \
                     (see docs/language-guide.md#type-annotations)",
                    name.trim_end_matches('?'),
                    name.trim_end_matches('?')
                ),
            ));
        }
        if matches!(self.peek(), Token::Lt | Token::JsxOpenStart) {
            return Err(self.error_at(
                self.pos,
                format!(
                    "parameterized types are not supported; write `{name}` on its own \
                     (see docs/language-guide.md#type-annotations)"
                ),
            ));
        }
        Ok(name)
    }

    // ---- Expression Parsing (Pratt parser) ----

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_pipe()
    }

    fn parse_pipe(&mut self) -> Result<Expr, String> {
        // The CST records pipe syntax as a CallExpr, matching the AST's
        // rewrite of `a |> f` into a call.
        let cp = self.ev_checkpoint();
        let mut left = self.parse_or()?;
        while self.at_binary_op(&[Token::Pipe]) {
            let start = self.pos;
            self.advance();
            self.skip_newlines();
            let rhs = self.parse_or()?;
            self.ev_wrap(cp, SyntaxKind::CallExpr);
            left = match rhs.kind {
                ExprKind::Call {
                    function,
                    mut args,
                    mut arg_names,
                } => {
                    // The piped value fills the first slot positionally, so it
                    // stays ahead of any named argument already written.
                    args.insert(0, left);
                    if !arg_names.is_empty() {
                        arg_names.insert(0, None);
                    }
                    self.mk_expr(
                        ExprKind::Call {
                            function,
                            args,
                            arg_names,
                        },
                        start,
                    )
                }
                _ => self.mk_expr(
                    ExprKind::Call {
                        function: Box::new(rhs),
                        args: vec![left],
                        arg_names: Vec::new(),
                    },
                    start,
                ),
            };
        }
        Ok(left)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_and()?;
        while self.at_binary_op(&[Token::Or]) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_and()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_equality()?;
        while self.at_binary_op(&[Token::And]) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_equality()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_comparison()?;
        while self.at_binary_op(&[Token::Eq, Token::Ne]) {
            let op = match self.advance() {
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_comparison()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_coalesce()?;
        while self.at_binary_op(&[Token::Lt, Token::Le, Token::Gt, Token::Ge]) {
            let op = match self.advance() {
                Token::Lt => BinOp::Lt,
                Token::Le => BinOp::Le,
                Token::Gt => BinOp::Gt,
                Token::Ge => BinOp::Ge,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_coalesce()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    /// `x ?? y` — binds tighter than comparison but looser than concat (`++`),
    /// so `count ?? 0 > 5` is `(count ?? 0) > 5` and `"a" ++ b ?? "x"` is
    /// `("a" ++ b) ?? "x"`. Short-circuits at lowering (RHS runs only when the
    /// LHS is absent).
    fn parse_coalesce(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_concat()?;
        while self.at_binary_op(&[Token::DoubleQuestion]) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_concat()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::Coalesce,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_additive()?;
        while self.at_binary_op(&[Token::PlusPlus]) {
            self.advance();
            self.skip_newlines();
            let right = self.parse_additive()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op: BinOp::Concat,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_multiplicative()?;
        while self.at_binary_op(&[Token::Plus, Token::Minus]) {
            let op = match self.advance() {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_multiplicative()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut left = self.parse_unary()?;
        while self.at_binary_op(&[Token::Star, Token::Slash, Token::Percent]) {
            let op = match self.advance() {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => unreachable!(),
            };
            self.skip_newlines();
            let right = self.parse_unary()?;
            self.ev_wrap(cp, SyntaxKind::BinaryExpr);
            left = Expr {
                span: SourceSpan {
                    start: left.span.start,
                    end: right.span.end,
                    file: left.span.file,
                },
                kind: ExprKind::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        match self.peek().clone() {
            // `-3` is one negative literal, not a negation of `3`, so postfix
            // operators apply to the signed number: `-3.abs()` is `(-3).abs()`.
            Token::Minus if matches!(self.peek_at(1), Token::Int(_) | Token::Float(_)) => {
                self.parse_postfix()
            }
            Token::Minus => {
                self.ev_open(SyntaxKind::UnaryExpr);
                self.advance();
                let operand = self.parse_unary()?;
                self.ev_close();
                Ok(self.mk_expr(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    start,
                ))
            }
            Token::Bang => {
                self.ev_open(SyntaxKind::UnaryExpr);
                self.advance();
                let operand = self.parse_unary()?;
                self.ev_close();
                Ok(self.mk_expr(
                    ExprKind::UnaryOp {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    start,
                ))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let cp = self.ev_checkpoint();
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek() {
                // `a?.b` and `a?.[i]` — the optional forms. A `?.` before a `[`
                // is the index spelling, matching JavaScript; the `[` arm below
                // then builds the access itself, and the `OptionalAccess`
                // wrapper is applied to the link it produces.
                Token::QuestionDot if matches!(self.peek_at(1), Token::LBracket) => {
                    self.advance();
                    self.pending_optional = true;
                    continue;
                }
                Token::QuestionDot => {
                    self.advance();
                    let field = self.expect_field_name()?;
                    self.ev_wrap(cp, SyntaxKind::FieldAccessExpr);
                    let span = SourceSpan {
                        start: expr.span.start,
                        end: self.prev_span().end,
                        file: expr.span.file,
                    };
                    expr = Expr {
                        span,
                        kind: ExprKind::OptionalAccess(Box::new(Expr {
                            span,
                            kind: ExprKind::FieldAccess {
                                object: Box::new(expr),
                                field,
                            },
                        })),
                    };
                }
                Token::Dot => {
                    self.advance();
                    let field = self.expect_field_name()?;
                    self.ev_wrap(cp, SyntaxKind::FieldAccessExpr);
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                            file: expr.span.file,
                        },
                        kind: ExprKind::FieldAccess {
                            object: Box::new(expr),
                            field,
                        },
                    };
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    // `[[1,2] [3,4]]` parses as an *index* — the `[` after the
                    // first element is postfix. Blaming the `,` inside the
                    // second bracket ("Expected ']', got ','") points at the
                    // wrong place entirely, so name the real mistake.
                    if matches!(self.peek(), Token::Comma) {
                        return Err(self.error_at_current(
                            "Expected ']' to close the index, got ',' — an index takes one \
                             expression; separate two list elements with a ','"
                                .to_string(),
                        ));
                    }
                    // A block with no `end` leaves the closer on a later line.
                    self.skip_newlines();
                    self.expect(&Token::RBracket)?;
                    self.ev_wrap(cp, SyntaxKind::IndexAccessExpr);
                    let span = SourceSpan {
                        start: expr.span.start,
                        end: self.prev_span().end,
                        file: expr.span.file,
                    };
                    expr = Expr {
                        span,
                        kind: ExprKind::IndexAccess {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                    };
                    // `a?.[i]` — the `?.` was consumed before the bracket.
                    if std::mem::take(&mut self.pending_optional) {
                        expr = Expr {
                            span,
                            kind: ExprKind::OptionalAccess(Box::new(expr)),
                        };
                    }
                }
                Token::LParen => {
                    self.check_callable(&expr)?;
                    self.ev_open(SyntaxKind::ArgList);
                    self.advance();
                    let (args, arg_names) = self.parse_arg_list()?;
                    self.expect(&Token::RParen)?;
                    self.ev_close(); // ArgList
                    self.ev_wrap(cp, SyntaxKind::CallExpr);
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                            file: expr.span.file,
                        },
                        kind: ExprKind::Call {
                            function: Box::new(expr),
                            args,
                            arg_names,
                        },
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn check_callable(&self, expr: &Expr) -> Result<(), String> {
        match &expr.kind {
            // Definitely callable: identifiers, field/index access, calls, lambdas, blocks
            ExprKind::Ident(_)
            | ExprKind::CellGet(_)
            | ExprKind::FieldAccess { .. }
            | ExprKind::IndexAccess { .. }
            | ExprKind::OptionalAccess(_)
            | ExprKind::Call { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. } => Ok(()),

            // Not callable: literals, operators, collections, etc.
            ExprKind::Literal(_) => {
                Err(self
                    .error_at_current("Literal value cannot be called as a function".to_string()))
            }
            ExprKind::BinaryOp { .. } => Err(self.error_at_current(
                "Binary operation result cannot be called as a function".to_string(),
            )),
            ExprKind::UnaryOp { .. } => Err(self.error_at_current(
                "Unary operation result cannot be called as a function".to_string(),
            )),
            ExprKind::List(_) => {
                Err(self
                    .error_at_current("List literal cannot be called as a function".to_string()))
            }
            ExprKind::Record(_) => {
                Err(self
                    .error_at_current("Record literal cannot be called as a function".to_string()))
            }
            ExprKind::StringInterp { .. } => Err(self.error_at_current(
                "String interpolation cannot be called as a function".to_string(),
            )),
            ExprKind::Element { .. } => {
                Err(self.error_at_current("Element cannot be called as a function".to_string()))
            }
            ExprKind::For { .. } => Err(self.error_at_current(
                "For-loop result (a list) cannot be called as a function".to_string(),
            )),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        match self.peek().clone() {
            Token::Int(n) => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Int(n)), start))
            }
            Token::Float(f) => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Float(f)), start))
            }
            // A signed number: the `-` belongs to the literal and its span.
            Token::Minus if matches!(self.peek_at(1), Token::Int(_) | Token::Float(_)) => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                let literal = match self.peek().clone() {
                    Token::Int(n) => Literal::Int(-n),
                    Token::Float(f) => Literal::Float(-f),
                    _ => unreachable!("guarded by the match arm"),
                };
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(literal), start))
            }
            Token::InterpStart => self.parse_string_interp(),
            Token::String(s) => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::String(s)), start))
            }
            Token::True => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Bool(true)), start))
            }
            Token::False => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Bool(false)), start))
            }
            Token::Nil => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Literal(Literal::Nil), start))
            }
            // `@` is kept free for declaration annotations
            // (docs/proposals/annotation-syntax.md). Its old meaning, the
            // `f(@x)` rebind operator, was removed; `petal migrate-old-code`
            // (`at-rebind`) rewrites it.
            Token::At => Err(self.error_at_current(at_rebind_removed_message(
                match self.peek_at(1) {
                    Token::Ident(name) => Some(name.as_str()),
                    Token::Const => Some(CONST_ANNOTATION),
                    _ => None,
                },
            ))),
            // `get x` reads a cell. Parsed in *primary* position rather than
            // as a prefix operator so the postfix loop wraps the result:
            // `get cfg.w` is `(get cfg).w`, which is what a cell holding a
            // record needs — the dereference has to happen before the field.
            Token::Get => {
                self.ev_open(SyntaxKind::GetExpr);
                self.advance(); // consume 'get'
                let name = self.expect_ident()?;
                self.ev_close();
                Ok(self.mk_expr(ExprKind::CellGet(name), start))
            }
            Token::Ident(_) => {
                self.ev_open(SyntaxKind::IdentExpr);
                let name = self.expect_ident()?;
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Ident(name), start))
            }
            // `const(e)`: the builtin annotation
            // (docs/tasks/lifecycle-annotation-functions-refactor.md). The
            // keyword is a name when a `(` follows, as `config` is a name
            // everywhere but before `let`.
            Token::Const if matches!(self.peek_at(1), Token::LParen) => {
                self.ev_open(SyntaxKind::IdentExpr);
                self.advance();
                self.ev_close();
                Ok(self.mk_expr(ExprKind::Ident(CONST_ANNOTATION.to_string()), start))
            }
            Token::LParen => {
                self.ev_open(SyntaxKind::ParenExpr);
                self.advance();
                let expr = self.parse_expr()?;
                // A block with no `end` leaves the closer on a later line.
                self.skip_newlines();
                self.expect(&Token::RParen)?;
                self.ev_close();
                Ok(expr)
            }
            Token::LBracket => self.parse_list_literal(),
            Token::LBrace => self.parse_record_literal(),
            Token::If => self.parse_if_expr(),
            Token::Match => self.parse_match_expr(),
            Token::For => self.parse_for_expr(),
            Token::Fn => self.parse_lambda(),
            Token::Color(hex) => {
                self.ev_open(SyntaxKind::LiteralExpr);
                self.advance();
                self.ev_close();
                let fields = parse_color_hex(&hex);
                let record_fields = fields
                    .into_iter()
                    .map(|(name, value)| {
                        RecordField::Named(
                            name.to_string(),
                            Expr {
                                kind: ExprKind::Literal(Literal::Int(value)),
                                span: self.span_from(start),
                            },
                        )
                    })
                    .collect();
                Ok(self.mk_expr(ExprKind::Record(record_fields), start))
            }
            Token::JsxOpenStart => self.parse_jsx_element(),
            other => {
                Err(self.error_at_current(format!("Unexpected token: {}", token_desc(&other))))
            }
        }
    }

    fn parse_list_literal(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::ListExpr);
        self.advance(); // consume '['
        let mut elements = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            self.expect_element_start("a list element")?;
            let elem = self.parse_expr()?;
            elements.push(elem);
            self.expect_element_separator(&Token::RBracket, "list elements")?;
        }
        self.expect(&Token::RBracket)?;
        self.ev_close();
        Ok(self.mk_expr(ExprKind::List(elements), start))
    }

    fn parse_record_literal(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::RecordExpr);
        self.advance(); // consume '{'
        let mut fields = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            self.expect_element_start("a record field")?;
            self.ev_open(SyntaxKind::RecordField);
            if matches!(self.peek(), Token::DotDotDot) {
                self.advance(); // consume '...'
                let expr = self.parse_expr()?;
                fields.push(RecordField::Spread(expr));
            } else if matches!(self.peek(), Token::LBracket) {
                // `[key]: value` — a key that is computed. See
                // `ast::computed_record_field` for what it is parsed as.
                self.advance(); // consume '['
                let key = self.parse_expr()?;
                self.expect(&Token::RBracket)?;
                self.expect(&Token::Colon)?;
                let value = self.parse_expr()?;
                fields.push(crate::ast::computed_record_field(key, value));
            } else {
                let key_pos = self.pos;
                let key = self.expect_field_name()?;
                if matches!(self.peek(), Token::Comma | Token::RBrace | Token::Newline) {
                    // `{type: "rect", invert}`: a bare key. An element
                    // attribute written bare is `true`, and in JS `{invert}`
                    // means `{invert: invert}`, so the author meant one of
                    // those. Petal has neither shorthand; name both spellings.
                    return Err(self.error_at(
                        key_pos,
                        format!(
                            "Record field `{key}` needs a value. Write `{key}: true` \
                             (or `{key}: {key}` for the variable); records have no shorthand"
                        ),
                    ));
                }
                self.expect(&Token::Colon)?;
                let value = self.parse_expr()?;
                fields.push(RecordField::Named(key, value));
            }
            self.ev_close(); // RecordField
            self.expect_element_separator(&Token::RBrace, "record fields")?;
        }
        self.expect(&Token::RBrace)?;
        self.ev_close(); // RecordExpr
        Ok(self.mk_expr(ExprKind::Record(fields), start))
    }

    fn parse_if_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::IfExpr);
        self.advance(); // consume 'if'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Then)?;
        let closers = Closers::Own { head: start, clause: start, or_else: true };
        let Body { stmts: then_body, semi } = self.parse_body(start, "`then`", closers, "if")?;
        let else_body = self.parse_else_chain(start, semi)?;
        self.ev_close();
        Ok(self.mk_expr(
            ExprKind::If {
                condition: Box::new(condition),
                then_body,
                else_body,
            },
            start,
        ))
    }

    /// Parse the tail of an if-expression after the then-body: the `else if`
    /// and `else` branches that continue it, and the single `end` that
    /// closes the whole chain when one is written. The body before left the
    /// cursor on the keyword when it is this chain's (see
    /// [`Parser::parse_body`]); anything else ends the chain with no `end`.
    /// `semi` is the first `;` of a one-line branch so far.
    fn parse_else_chain(
        &mut self,
        if_pos: usize,
        semi: Semi,
    ) -> Result<Option<ElseBranch>, String> {
        match self.peek() {
            Token::ElseIf | Token::Elsif => {
                let start = self.pos;
                let merged = matches!(self.peek(), Token::ElseIf);
                self.ev_open(SyntaxKind::ElseBranch);
                self.advance(); // consume 'else if' (or 'elsif')
                let condition = self.parse_expr()?;
                self.skip_newlines();
                self.expect(&Token::Then)?;
                let closers = Closers::Own { head: if_pos, clause: start, or_else: true };
                let Body { stmts: then_body, semi: branch_semi } =
                    self.parse_body(start, "`then`", closers, "if")?;
                // Consumes the final `end`, when there is one.
                let else_body = self.parse_else_chain(if_pos, semi.or(branch_semi))?;
                self.ev_close();
                if merged {
                    let next = (self.pos..self.tokens.len())
                        .find(|&i| !matches!(self.tokens[i], Token::Newline));
                    if let Some(i) = next
                        && matches!(self.tokens[i], Token::End)
                    {
                        self.else_if_second_end = Some(i);
                    }
                }
                let inner = self.mk_expr(
                    ExprKind::If {
                        condition: Box::new(condition),
                        then_body,
                        else_body,
                    },
                    start,
                );
                Ok(Some(ElseBranch::ElseIf(Box::new(inner))))
            }
            Token::Else => {
                let start = self.pos;
                self.ev_open(SyntaxKind::ElseBranch);
                self.advance(); // consume 'else'
                let closers = Closers::Own { head: if_pos, clause: start, or_else: false };
                let Body { stmts: body, semi: branch_semi } =
                    self.parse_body(start, "`else`", closers, "if")?;
                self.close_block(if_pos, "an `if` expression", semi.or(branch_semi))?;
                self.ev_close();
                Ok(Some(ElseBranch::Block(body)))
            }
            _ => {
                self.close_block(if_pos, "an `if` expression", semi)?;
                Ok(None)
            }
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::MatchExpr);
        self.advance(); // consume 'match'
        let subject = self.parse_expr()?;
        // The arms are a block under the header (each `when` on a line of
        // its own, at one column), or share the header's line.
        let arm_col = self.member_block(start, "the `match` header", |tok| {
            matches!(tok, Token::When)
        })?;
        let closers = Closers::Arm { head: start, arm_col };
        self.open.push(OpenBlock { head: start, name: "match" });
        let mut arms = Vec::new();
        let mut semi = None;
        if !self.layout {
            self.skip_newlines();
        }
        while !self.layout && !matches!(self.peek(), Token::End | Token::Eof) {
            arms.push(self.parse_match_arm(closers)?.0);
            self.skip_newlines();
        }
        while self.layout {
            let t = self.next_code();
            if !self.is_closing_keyword(t) || !self.closer_is_ours(t, closers)? {
                break;
            }
            self.skip_newlines();
            if matches!(self.peek(), Token::End) {
                break;
            }
            let (arm, arm_semi) = self.parse_match_arm(closers)?;
            arms.push(arm);
            semi = semi.or(arm_semi);
        }
        self.open.pop();
        self.close_block(start, "a `match` expression", semi)?;
        self.ev_close();
        Ok(self.mk_expr(
            ExprKind::Match {
                subject: Box::new(subject),
                arms,
            },
            start,
        ))
    }

    /// One arm of a `match`, whose `closers` say where the arm's body stops.
    /// Also returns the first `;` of a one-line `->` body (see
    /// [`Parser::close_block`]).
    fn parse_match_arm(&mut self, closers: Closers) -> Result<(MatchArm, Semi), String> {
        self.ev_open(SyntaxKind::MatchArm);
        if matches!(self.peek(), Token::Else | Token::Elsif | Token::ElseIf) {
            return Err(self.error_at_current(format!(
                "Expected `when`, got {} — a `match` has no `else` arm; the arm for \
                 everything else is `when _ -> …`",
                token_desc(self.peek())
            )));
        }
        let when_pos = self.pos;
        self.expect(&Token::When)?;
        let pattern_start = self.pos;
        let pattern = self.parse_pattern()?;
        let pattern_span = self.span_from(pattern_start);
        let guard = if matches!(self.peek(), Token::If) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let mut semi = None;
        let body = if matches!(self.peek(), Token::Do) {
            let do_pos = self.pos;
            self.advance(); // consume 'do'
            let own = Closers::Own { head: when_pos, clause: when_pos, or_else: false };
            let Body { stmts, semi: do_semi } = self.parse_body(when_pos, "`do`", own, "when")?;
            self.close_block(do_pos, "a `do` block", do_semi)?;
            stmts
        } else {
            self.expect(&Token::Arrow)?;
            // The body is a statement list, like an `if` branch: one line, or
            // an indented block under the arm. A nested construct consumes
            // its own `when`s and `end`.
            let body = self.parse_body(when_pos, "`->`", closers, "when")?;
            if body.stmts.is_empty() {
                let at = self.next_code();
                return Err(self.error_at(
                    at,
                    format!(
                        "Expected the arm's body after '->', got {} (an arm that does nothing \
                         is written `-> nil`)",
                        token_desc(self.token_at(at))
                    ),
                ));
            }
            semi = body.semi;
            body.stmts
        };
        self.ev_close(); // MatchArm
        Ok((
            MatchArm {
                pattern,
                pattern_span,
                guard,
                body,
            },
            semi,
        ))
    }

    /// A pattern: one alternative, or several separated by `|`.
    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let cp = self.ev_checkpoint();
        let first = self.parse_pattern_alternative()?;
        // Like any binary operator, `|` may end a line or begin the next.
        if !self.at_binary_op(&[Token::Bar]) {
            return Ok(first);
        }
        let mut alternatives = vec![first];
        while self.at_binary_op(&[Token::Bar]) {
            self.advance();
            self.skip_newlines();
            alternatives.push(self.parse_pattern_alternative()?);
        }
        // An or-pattern is a Pattern node whose children are its alternatives.
        self.ev_wrap(cp, SyntaxKind::Pattern);
        Ok(Pattern::Or(alternatives))
    }

    fn parse_pattern_alternative(&mut self) -> Result<Pattern, String> {
        self.ev_open(SyntaxKind::Pattern);
        let pattern = self.parse_pattern_inner()?;
        self.ev_close();
        Ok(pattern)
    }

    fn parse_pattern_inner(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Token::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            Token::Ident(name) => {
                self.advance();
                let mut path = vec![name];
                while matches!(self.peek(), Token::Dot) {
                    self.advance();
                    path.push(self.expect_ident()?);
                }
                if !matches!(self.peek(), Token::LParen) {
                    return Ok(pattern_from_path(path, None));
                }
                self.advance();
                let mut fields = Vec::new();
                self.skip_newlines();
                while !matches!(self.peek(), Token::RParen | Token::Eof) {
                    self.expect_element_start("a variant field pattern")?;
                    fields.push(self.parse_pattern()?);
                    self.expect_element_separator(&Token::RParen, "variant fields")?;
                }
                self.expect(&Token::RParen)?;
                Ok(pattern_from_path(path, Some(fields)))
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
                    _ => {
                        Err(self
                            .error_at_current("Expected number after '-' in pattern".to_string()))
                    }
                }
            }
            other => {
                Err(self
                    .error_at_current(format!("Expected a pattern, got {}", token_desc(&other))))
            }
        }
    }

    fn parse_list_pattern(&mut self) -> Result<Pattern, String> {
        self.advance(); // consume '['
        let mut elements = Vec::new();
        let mut rest = None;
        self.skip_newlines();

        while !matches!(self.peek(), Token::RBracket | Token::Eof) {
            self.expect_element_start("a list pattern element")?;
            if matches!(self.peek(), Token::DotDotDot) {
                self.advance();
                let name = self.expect_ident()?;
                rest = Some(name);
                self.expect_element_separator(&Token::RBracket, "list pattern elements")?;
                continue;
            }

            let elem = self.parse_pattern()?;
            elements.push(elem);
            self.expect_element_separator(&Token::RBracket, "list pattern elements")?;
        }
        self.expect(&Token::RBracket)?;
        Ok(Pattern::List { elements, rest })
    }

    fn parse_record_pattern(&mut self) -> Result<Pattern, String> {
        self.advance(); // consume '{'
        let mut fields = Vec::new();
        self.skip_newlines();

        while !matches!(self.peek(), Token::RBrace | Token::Eof) {
            self.expect_element_start("a record pattern field")?;
            let key = self.expect_field_name()?;
            self.expect(&Token::Colon)?;
            let pat = self.parse_pattern()?;
            fields.push((key, pat));
            self.expect_element_separator(&Token::RBrace, "record pattern fields")?;
        }
        self.expect(&Token::RBrace)?;
        Ok(Pattern::Record(fields))
    }

    fn parse_lambda(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::LambdaExpr);
        self.advance(); // consume 'fn'
        // The parameter list is optional: `fn 42 end` is an argless lambda. A
        // body starting with `(` is read as a parameter list — a documented wart.
        let params = if matches!(self.peek(), Token::LParen) {
            self.ev_open(SyntaxKind::ParamList);
            self.expect(&Token::LParen)?;
            let params = self.parse_param_list()?;
            self.expect(&Token::RParen)?;
            self.ev_close(); // ParamList
            params
        } else {
            Vec::new()
        };
        if matches!(self.peek(), Token::Arrow) {
            self.advance(); // consume '->'
            self.skip_newlines();
            let expr = self.parse_expr()?;
            self.arrow_lambda_end = Some(self.pos);
            let body = vec![self.mk_stmt(StmtKind::Expr(expr), start)];
            self.ev_close(); // LambdaExpr
            Ok(self.mk_expr(ExprKind::Lambda { params, body }, start))
        } else {
            let closers = Closers::Own { head: start, clause: start, or_else: false };
            let Body { stmts: body, semi } =
                self.parse_body(start, "the `fn` header", closers, "fn")?;
            self.close_block(start, "a `fn` expression", semi)?;
            self.ev_close(); // LambdaExpr
            Ok(self.mk_expr(ExprKind::Lambda { params, body }, start))
        }
    }

    fn parse_string_interp(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        // Kind depends on whether any holes appear, so wrap retroactively.
        let cp = self.ev_checkpoint();
        self.advance(); // consume InterpStart
        let mut parts: Vec<String> = Vec::new();
        let mut exprs: Vec<Expr> = Vec::new();

        loop {
            match self.peek().clone() {
                Token::String(s) => {
                    self.advance();
                    parts.push(s);
                }
                Token::InterpEnd => {
                    parts.push(String::new());
                }
                // `"{x:.1f}"`, `"{n:,}"`: the format spec of other
                // languages. A hole holds an expression and nothing else, and
                // the builtins that format a number are calls like any other.
                // (After a string literal the colon is JSON in a string,
                // which the message below already answers.)
                Token::Colon
                    if exprs.last().is_some_and(|e| {
                        !matches!(e.kind, ExprKind::Literal(Literal::String(_)))
                    }) =>
                {
                    return Err(self.error_at_current(
                        "Expected string part in interpolation, got Colon — a `{` in a \
                         double-quoted string opens an interpolation hole, which holds an \
                         expression and takes no format spec: for `{x:.1f}` write \
                         `{fixed(x, 1)}`, for `{n:,}` write `{commas(n)}` (`{commas(x, 2)}` \
                         for both). Write `\\{` for a literal brace"
                            .to_string(),
                    ));
                }
                other => {
                    // The hole closed after one expression and something
                    // else followed — almost always a literal brace
                    // (`"{\"a\": 1}"`, JSON in a string) read as a hole.
                    return Err(self.error_at_current(format!(
                        "Expected string part in interpolation, got {:?} — a `{{` in a \
                         double-quoted string opens an interpolation hole; write `\\{{` \
                         for a literal brace",
                        other
                    )));
                }
            }

            if matches!(self.peek(), Token::InterpEnd) {
                self.advance();
                break;
            }

            self.interp_holes += 1;
            let expr = self.parse_expr()?;
            self.interp_holes -= 1;
            exprs.push(expr);
        }

        if exprs.is_empty() {
            self.ev_wrap(cp, SyntaxKind::LiteralExpr);
            Ok(self.mk_expr(
                ExprKind::Literal(Literal::String(
                    parts.into_iter().next().unwrap_or_default(),
                )),
                start,
            ))
        } else {
            self.ev_wrap(cp, SyntaxKind::StringInterpExpr);
            Ok(self.mk_expr(ExprKind::StringInterp { parts, exprs }, start))
        }
    }

    fn parse_jsx_element(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::ElementExpr);
        self.advance(); // consume JsxOpenStart
        let tag = match self.advance() {
            Token::JsxTagName(name) => name,
            other => {
                return Err(self.error_at(
                    self.pos - 1,
                    format!("Expected JSX tag name, got {:?}", other),
                ));
            }
        };

        let mut props = Vec::new();
        loop {
            match self.peek().clone() {
                Token::Gt => {
                    self.advance();
                    break;
                }
                Token::JsxSelfClose => {
                    self.advance();
                    self.ev_close(); // ElementExpr
                    return Ok(self.mk_expr(
                        ExprKind::Element {
                            tag,
                            props,
                            children: Vec::new(),
                        },
                        start,
                    ));
                }
                // An attribute name is a member name, so a keyword is one too
                // (`<video in={2.0} />`), as it is a record key.
                ref tok
                    if matches!(tok, Token::Ident(_))
                        || crate::lexer::keyword_text(tok).is_some() =>
                {
                    let attr_name = match tok {
                        Token::Ident(name) => name.clone(),
                        other => crate::lexer::keyword_text(other)
                            .unwrap_or_default()
                            .to_string(),
                    };
                    self.ev_open(SyntaxKind::JsxAttr);
                    let name_at = self.pos;
                    self.advance();
                    // A bare `<video muted />` is `muted={true}`, as in JSX.
                    if !matches!(self.peek(), Token::Assign) {
                        self.ev_close(); // JsxAttr
                        let value = self.mk_expr(ExprKind::Literal(Literal::Bool(true)), name_at);
                        props.push((attr_name, value));
                        continue;
                    }
                    self.expect(&Token::Assign)?;
                    let value = match self.peek().clone() {
                        Token::String(s) => {
                            let attr_start = self.pos;
                            self.advance();
                            self.mk_expr(ExprKind::Literal(Literal::String(s)), attr_start)
                        }
                        Token::LBrace => {
                            self.advance();
                            let expr = self.parse_expr()?;
                            self.skip_newlines();
                            self.expect(&Token::RBrace)?;
                            expr
                        }
                        other => {
                            return Err(self.error_at_current(format!(
                                "Expected string or {{expr}} for attribute value, got {:?}",
                                other
                            )));
                        }
                    };
                    self.ev_close(); // JsxAttr
                    props.push((attr_name, value));
                }
                other => {
                    return Err(
                        self.error_at_current(format!("Unexpected token in JSX tag: {:?}", other))
                    );
                }
            }
        }

        let mut children = Vec::new();
        loop {
            match self.peek().clone() {
                Token::JsxCloseStart => {
                    self.advance();
                    match self.advance() {
                        Token::JsxTagName(close_tag) => {
                            if close_tag != tag {
                                return Err(self.error_at(
                                    self.pos - 1,
                                    format!("Mismatched JSX tags: <{}> and </{}>", tag, close_tag),
                                ));
                            }
                        }
                        other => {
                            return Err(self.error_at(
                                self.pos - 1,
                                format!("Expected closing tag name, got {:?}", other),
                            ));
                        }
                    }
                    break;
                }
                Token::JsxText(text) => {
                    self.advance();
                    // Two text runs in a row only happen when a comment line
                    // sat between them. Join them as if the line were not
                    // there: the line breaks around it collapse to one space.
                    if let Some(JsxChild::Text(prev)) = children.last_mut()
                        && matches!(self.tokens[self.pos - 2], Token::JsxText(_))
                    {
                        prev.push(' ');
                        prev.push_str(&text);
                    } else {
                        children.push(JsxChild::Text(text));
                    }
                }
                Token::LBrace if matches!(self.peek_at(1), Token::RBrace) => {
                    // `{}` or `{/* comment */}`: an empty hole is no child.
                    self.advance();
                    self.advance();
                }
                Token::LBrace => {
                    self.advance();
                    let expr = self.parse_expr()?;
                    self.skip_newlines();
                    self.expect(&Token::RBrace)?;
                    children.push(JsxChild::Expr(expr));
                }
                Token::JsxOpenStart => {
                    let nested = self.parse_jsx_element()?;
                    children.push(JsxChild::Expr(nested));
                }
                Token::Eof => {
                    return Err(self.error_at_current(format!("Unclosed JSX element <{}>", tag)));
                }
                other => {
                    return Err(self.error_at_current(format!(
                        "Unexpected token in JSX children: {:?}",
                        other
                    )));
                }
            }
        }

        self.ev_close(); // ElementExpr
        Ok(self.mk_expr(
            ExprKind::Element {
                tag,
                props,
                children,
            },
            start,
        ))
    }

    /// The arguments of a call. An argument written `name: value` is bound by
    /// name rather than by position; every positional argument must come before
    /// the first named one.
    ///
    /// Returns the values and, parallel to them, the written names. The name
    /// vector is left *empty* when nothing was named — the common case, which
    /// every later layer takes as "all positional".
    fn parse_arg_list(&mut self) -> Result<(Vec<Expr>, Vec<Option<String>>), String> {
        let mut args = Vec::new();
        let mut arg_names: Vec<Option<String>> = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            self.expect_element_start("an argument")?;
            if crate::ast::is_spread_call(&arg_names) {
                return Err(self.error_at_current(
                    "An argument after a spread argument: `...args` must be the last argument \
                     of the call"
                        .to_string(),
                ));
            }
            if matches!(self.peek(), Token::DotDotDot) {
                // `f(...args)`: the list or record that supplies the rest of
                // the arguments. Recorded as the last argument, under the
                // name no parameter can have (`ast::SPREAD_ARG`).
                if !arg_names.is_empty() {
                    return Err(self.error_at_current(
                        "A spread argument after a named argument: write the named values \
                         into the record that is spread (`f(...{...args, name: value})`)"
                            .to_string(),
                    ));
                }
                self.ev_open(SyntaxKind::SpreadArg);
                self.advance(); // consume '...'
                let value = self.parse_expr()?;
                self.ev_close(); // SpreadArg
                arg_names.resize(args.len(), None);
                arg_names.push(Some(crate::ast::SPREAD_ARG.to_string()));
                args.push(value);
            } else if self.starts_named_arg() {
                self.ev_open(SyntaxKind::NamedArg);
                let name = self.expect_field_name()?;
                self.expect(&Token::Colon)?;
                let value = self.parse_expr()?;
                self.ev_close(); // NamedArg
                arg_names.resize(args.len(), None);
                arg_names.push(Some(name));
                args.push(value);
            } else {
                if !arg_names.is_empty() {
                    return Err(self.error_at_current(
                        "A positional argument after a named argument: once an argument is \
                         named, every later argument must be named too"
                            .to_string(),
                    ));
                }
                let arg = self.parse_expr()?;
                args.push(arg);
            }
            self.expect_element_separator(&Token::RParen, "arguments")?;
        }
        Ok((args, arg_names))
    }

    /// Whether the cursor is on a `name:` argument label. A name here is what a
    /// record key is — an identifier or any keyword ([`Parser::expect_field_name`])
    /// — so `f(end: 1)` names a parameter called `end`. One token of lookahead
    /// separates it from a positional argument that merely starts with a name.
    fn starts_named_arg(&self) -> bool {
        let is_name = matches!(self.peek(), Token::Ident(_))
            || crate::lexer::keyword_text(self.peek()).is_some();
        is_name && matches!(self.peek_at(1), Token::Colon)
    }
}

/// A line that starts between its block's column and the enclosing one's.
fn partial_dedent_message(col: u32, block: u32, indent: u32) -> String {
    format!(
        "this line is indented less than its block, but more than the enclosing one: it \
         starts at column {col}, between column {indent} and the block's column {block}"
    )
}

/// A block opener followed by a line break and no indented line.
fn missing_block_message(opener: &str, indent: u32) -> String {
    format!(
        "expected an indented block after {opener}: the next line must start right of \
         column {indent}, or the body goes on the same line"
    )
}

/// The parse error for an `@` where an expression starts. `@` is reserved
/// for declaration annotations (docs/proposals/annotation-syntax.md); its old
/// meaning, the rebind operator (`f(@x)` for `x = f(x)`), was removed on
/// 2026-11-09 and `petal migrate-old-code` (`at-rebind`) rewrites it. `name`
/// is the identifier after the `@`, when there is one.
pub(crate) fn at_rebind_removed_message(name: Option<&str>) -> String {
    match name {
        Some(x) if crate::ast::ANNOTATION_NAMES.contains(&x) => format!(
            "`@` annotates a declaration and goes before `let` (`@{x} let NAME = …`). Inside \
             an expression, write the call: `{x}(…)`"
        ),
        Some(x) => format!(
            "`@{x}` is not an expression: `@` only annotates a declaration (`@tunable let \
             NAME = …`), and the `@` rebind operator was removed. Write `{x} = f({x})` \
             instead of `f(@{x})`; {}",
            crate::migrate::RUN_HINT
        ),
        None => format!(
            "`@` is not an expression (the old `f(@x)` rebind operator was removed: write \
             `x = f(x)`; {})",
            crate::migrate::RUN_HINT
        ),
    }
}

/// The error for `@` with no declaration after it (`@const` at the end of a
/// file, `@tunable 4`).
const ANNOTATION_NEEDS_DECL: &str =
    "an annotation must be followed by a declaration (`let`, `pub let`)";

/// The error for `@name` on a `fn` (F1 in docs/proposals/annotation-syntax.md).
fn annotation_on_fn_message(name: &str) -> String {
    let what = match name {
        "tunable" | "rigid" => format!(" `@{name}` annotates a `let`; a function is not a setting."),
        "const" => " `@const` annotates a `let`.".to_string(),
        _ => String::new(),
    };
    format!("`@` on a function is reserved for compile-time annotations; there are none yet.{what}")
}

/// The error for `@name` on a class or enum declaration.
fn annotation_on_type_message(name: &str, what: &str) -> String {
    format!("`@{name}` annotates a `let`; {what} declaration takes no annotations")
}

/// The source text of a literal token, for an error message.
fn literal_token_text(tok: &Token) -> Option<String> {
    Some(match tok {
        Token::Int(n) => n.to_string(),
        Token::Float(f) => format!("{f:?}"),
        Token::String(s) => format!("{s:?}"),
        Token::True => "true".to_string(),
        Token::False => "false".to_string(),
        Token::Nil => "nil".to_string(),
        _ => return None,
    })
}

/// Parse a hex color string (without its sigil) into (field_name, value) pairs.
/// Supports #rgb, #rgba, #rrggbb, #rrggbbaa formats.
/// Shared with `crate::cst_project` so the CST projection can't drift.
pub(crate) fn parse_color_hex(hex: &str) -> Vec<(&'static str, i64)> {
    let expand = |c: u8| -> i64 {
        let v = if c.is_ascii_digit() {
            c - b'0'
        } else {
            (c.to_ascii_lowercase() - b'a') + 10
        };
        (v as i64) * 17 // e.g. 0xf -> 255, 0x8 -> 136
    };
    let parse2 = |hi: u8, lo: u8| -> i64 {
        let h = if hi.is_ascii_digit() {
            hi - b'0'
        } else {
            (hi.to_ascii_lowercase() - b'a') + 10
        };
        let l = if lo.is_ascii_digit() {
            lo - b'0'
        } else {
            (lo.to_ascii_lowercase() - b'a') + 10
        };
        (h as i64) * 16 + (l as i64)
    };
    let b = hex.as_bytes();
    match b.len() {
        3 => vec![
            ("r", expand(b[0])),
            ("g", expand(b[1])),
            ("b", expand(b[2])),
        ],
        4 => vec![
            ("r", expand(b[0])),
            ("g", expand(b[1])),
            ("b", expand(b[2])),
            ("a", expand(b[3])),
        ],
        6 => vec![
            ("r", parse2(b[0], b[1])),
            ("g", parse2(b[2], b[3])),
            ("b", parse2(b[4], b[5])),
        ],
        8 => vec![
            ("r", parse2(b[0], b[1])),
            ("g", parse2(b[2], b[3])),
            ("b", parse2(b[4], b[5])),
            ("a", parse2(b[6], b[7])),
        ],
        _ => unreachable!("lexer validates hex length"),
    }
}

/// A pattern that opens with an identifier, from its dotted `path` and the
/// field patterns in its parens, if it has any. `Kind.park`, `mod.Kind.park`
/// and anything with parens is a variant, named through its enum; a lone
/// identifier is a binding.
///
/// Shared by the parser and the CST projection, which must agree.
pub(crate) fn pattern_from_path(path: Vec<String>, fields: Option<Vec<Pattern>>) -> Pattern {
    match fields {
        None if path.len() == 1 => Pattern::Variable(path.concat()),
        fields => Pattern::Variant {
            name: path.join("."),
            fields: fields.unwrap_or_default(),
        },
    }
}

/// Rewrite the *root* identifier of a `set` target into an explicit cell read.
///
/// A `set` target is rooted at a `var` by definition, so the read half of a
/// compound `set` is a cell read: `set p.hp -= n` is `set p.hp = get p.hp - n`,
/// where the dereference happens at `p` and the field access applies to the
/// contents. Anything that is not rooted at a plain name is returned unchanged
/// and the write-keyword check reports it.
///
/// Shared with `crate::cst_project` so the CST projection can't drift.
pub(crate) fn cell_get_at_root(expr: Expr) -> Expr {
    let span = expr.span;
    match expr.kind {
        ExprKind::Ident(name) => Expr {
            kind: ExprKind::CellGet(name),
            span,
        },
        ExprKind::FieldAccess { object, field } => Expr {
            kind: ExprKind::FieldAccess {
                object: Box::new(cell_get_at_root(*object)),
                field,
            },
            span,
        },
        ExprKind::IndexAccess { object, index } => Expr {
            kind: ExprKind::IndexAccess {
                object: Box::new(cell_get_at_root(*object)),
                index,
            },
            span,
        },
        other => Expr { kind: other, span },
    }
}

/// Shared with `crate::cst_project` so the CST projection can't drift.
pub(crate) fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, String> {
    let span = expr.span;
    match expr.kind {
        ExprKind::Ident(name) => Ok(AssignTarget::Name(name)),
        ExprKind::FieldAccess { object, field } => Ok(AssignTarget::Field(object, field)),
        ExprKind::IndexAccess { object, index } => Ok(AssignTarget::Index(object, index)),
        _ => {
            if span.start.line > 0 {
                Err(format!(
                    "Invalid assignment target [line {}, column {}]",
                    span.start.line, span.start.column
                ))
            } else {
                Err("Invalid assignment target".to_string())
            }
        }
    }
}
