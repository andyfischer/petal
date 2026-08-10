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
pub const CONFIG_KEYWORD: &str = "config";

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
        Token::Color(hex) => kw(&format!("#{hex}")),
        Token::Newline => "a line break".to_string(),
        Token::Eof => "end of input".to_string(),
        Token::InterpStart | Token::InterpEnd => "a string interpolation".to_string(),
        Token::True => kw("true"),
        Token::False => kw("false"),
        Token::Nil => kw("nil"),
        Token::Let => kw("let"),
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
        Token::Enum => kw("enum"),
        Token::End => kw("end"),
        Token::Then => kw("then"),
        Token::Do => kw("do"),
        Token::Elsif => kw("elsif"),
        Token::When => kw("when"),
        Token::Import => kw("import"),
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
        Token::LParen => op("("),
        Token::RParen => op(")"),
        Token::LBrace => op("{"),
        Token::RBrace => op("}"),
        Token::LBracket => op("["),
        Token::RBracket => op("]"),
        Token::Comma => op(","),
        Token::Dot => op("."),
        Token::Colon => op(":"),
        Token::At => op("@"),
        Token::Pipe => op("|>"),
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
    next_state_id: usize,
    /// CST event stream, recorded alongside AST construction. The tree built
    /// from it is the authoritative parse artifact (see
    /// [`crate::cst::parse_source`]); read it back with [`Parser::cst_events`]
    /// after `parse_program` succeeds (on error the stream may be unbalanced).
    events: EventBuilder,
}

impl Parser {
    pub fn new(tokens: Vec<Token>, token_spans: Vec<SourceSpan>) -> Self {
        Self {
            tokens,
            token_spans,
            pos: 0,
            next_state_id: 0,
            events: EventBuilder::new(),
        }
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
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.is_at_end() {
            let stmt_pos = self.pos;
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
        }
    }

    // ---- Statement Parsing ----

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.skip_newlines();
        let start = self.pos;
        match self.peek().clone() {
            Token::Let => self.parse_let(start, false, false, false),
            Token::Var => self.parse_let(start, false, true, false),
            Token::Set => self.parse_set(start),
            Token::Fn => self.parse_fn_decl(start, false),
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
                if matches!(self.tokens.get(self.pos + 1), Some(Token::Var)) {
                    return Err(self.error_at_current(
                        "`config` marks a `let` binding; a mutable `var` cell cannot be config"
                            .to_string(),
                    ));
                }
                self.parse_let(start, false, false, true)
            }
            Token::Import => self.parse_import(start),
            Token::Export => self.parse_export(start),
            _ => self.parse_expr_or_assign(start),
        }
    }

    /// `export <decl>` — a modifier on a top-level `fn`/`let`/`state`/`enum`
    /// that makes the declared name visible to importers (see
    /// `docs/module-system.md`). The `export` token is consumed inside the
    /// declaration's own CST node so the tree round-trips; the resulting
    /// [`Stmt`] carries `exported = true`. Routes on the token *after* `export`.
    fn parse_export(&mut self, start: usize) -> Result<Stmt, String> {
        match self.tokens.get(self.pos + 1) {
            Some(Token::Fn) => self.parse_fn_decl(start, true),
            Some(Token::Let) => self.parse_let(start, true, false, false),
            Some(Token::Var) => self.parse_let(start, true, true, false),
            // `export config let x = …` — both modifiers, export first.
            Some(Token::Ident(w))
                if w == CONFIG_KEYWORD
                    && matches!(self.tokens.get(self.pos + 2), Some(Token::Let)) =>
            {
                self.parse_let(start, true, false, true)
            }
            Some(Token::State) => self.parse_state(start, true),
            Some(Token::Enum) => self.parse_enum_decl(start, true),
            Some(Token::Ident(w)) if w == CLASS_KEYWORD => self.parse_class_decl(start, true),
            _ => Err(self.error_at_current(
                "`export` must be followed by a fn, let, var, state, enum, or class declaration"
                    .to_string(),
            )),
        }
    }

    /// `let x = …` and its mutable twin `var x = …`, which differ only in the
    /// keyword and the `is_var` flag. The `var` token stays a direct child of
    /// the `LetStmt` node so the CST projection can recover the flag the same
    /// way it recovers `export`.
    fn parse_let(
        &mut self,
        start: usize,
        exported: bool,
        is_var: bool,
        is_config: bool,
    ) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::LetStmt);
        if exported {
            self.advance(); // consume 'export'
        }
        if is_config {
            // The `config` ident stays a direct child of the LetStmt node so
            // the CST projection can recover the flag, like `export`/`var`.
            self.advance(); // consume 'config'
        }
        self.advance(); // consume 'let' / 'var'
        let name = self.expect_ident()?;
        let ty = self.parse_type_annotation()?;
        self.expect_initializer(if is_var { "var" } else { "let" }, &name)?;
        let value = self.parse_expr()?;
        self.ev_close();
        let mut stmt = self.mk_stmt(
            StmtKind::Let {
                name,
                ty,
                value,
                is_var,
                is_config,
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
            self.advance(); // consume 'export'
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
        let id = self.next_state_id;
        self.next_state_id += 1;
        self.ev_close();
        let mut stmt = self.mk_stmt(
            StmtKind::State {
                name,
                ty,
                init,
                id,
                key,
                is_var,
            },
            start,
        );
        stmt.exported = exported;
        Ok(stmt)
    }

    /// `import m` / `import m as u` / `import m: a, b`.
    /// The name list ends at the newline; `as` is contextual (not a keyword).
    fn parse_import(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ImportStmt);
        self.advance(); // consume 'import'
        let module = self.expect_ident()?;

        let mut alias = None;
        let mut names = None;
        match self.peek().clone() {
            Token::Ident(kw) if kw == "as" => {
                self.advance(); // consume 'as'
                alias = Some(self.expect_ident()?);
            }
            Token::Colon => {
                self.advance(); // consume ':'
                let mut list = Vec::new();
                loop {
                    list.push(self.expect_ident()?);
                    if matches!(self.peek(), Token::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                names = Some(list);
            }
            _ => {}
        }

        self.ev_close();
        Ok(self.mk_stmt(
            StmtKind::Import(ImportDecl {
                module,
                alias,
                names,
            }),
            start,
        ))
    }

    fn parse_fn_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::FnDecl);
        if exported {
            self.advance(); // consume 'export'
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
        self.skip_newlines();
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
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

    /// Whether the `class` identifier at the cursor opens a declaration —
    /// i.e. the next token is a name. `class` alone (a variable called `class`,
    /// a JSX `class=` attribute, `class.foo`) is left to expression parsing.
    fn starts_class_decl(&self) -> bool {
        matches!(self.tokens.get(self.pos + 1), Some(Token::Ident(_)))
    }

    /// `config` starts a declaration only when the very next token is a
    /// binding keyword — `config` on its own line stays an expression.
    fn starts_config_decl(&self) -> bool {
        matches!(self.tokens.get(self.pos + 1), Some(Token::Let | Token::Var))
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
            self.advance(); // consume 'export'
        }
        self.advance(); // consume the contextual `class`
        let name = self.expect_ident()?;
        let closes = format!("class `{name}`");
        self.skip_newlines();
        let mut fields: Vec<ClassFieldDecl> = Vec::new();
        while !matches!(self.peek(), Token::End | Token::Eof) {
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
            self.expect_element_separator_of(&Token::End, "class fields", Some(&closes))?;
        }
        self.expect(&Token::End)?;
        self.ev_close(); // ClassDecl
        let mut stmt = self.mk_stmt(StmtKind::ClassDecl { name, fields }, start);
        stmt.exported = exported;
        Ok(stmt)
    }

    fn parse_enum_decl(&mut self, start: usize, exported: bool) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::EnumDecl);
        if exported {
            self.advance(); // consume 'export'
        }
        self.advance(); // consume 'enum'
        let name = self.expect_ident()?;
        let closes = format!("enum `{name}`");
        self.skip_newlines();
        let mut variants = Vec::new();
        while !matches!(self.peek(), Token::End | Token::Eof) {
            let variant_name = self.expect_member_name("a variant name", &closes)?;
            let fields = if matches!(self.peek(), Token::LParen) {
                self.ev_open(SyntaxKind::ParamList);
                self.advance(); // consume '('
                let params = self.parse_param_list()?;
                self.expect(&Token::RParen)?;
                self.ev_close();
                // Enum field type annotations are deferred (see the plan): keep
                // field names only. Any `: type` was still parsed into the CST.
                params.into_iter().map(|p| p.name).collect()
            } else {
                Vec::new()
            };
            variants.push(EnumVariant {
                name: variant_name,
                fields,
            });
            self.expect_element_separator_of(&Token::End, "enum variants", Some(&closes))?;
        }
        self.expect(&Token::End)?;
        self.ev_close();
        let mut stmt = self.mk_stmt(StmtKind::EnumDecl { name, variants }, start);
        stmt.exported = exported;
        Ok(stmt)
    }

    fn parse_for(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ForStmt);
        let (var, iter, body) = self.parse_for_inner()?;
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::For { var, iter, body }, start))
    }

    /// The expression form of `for`, reached only in value position (`x = for …`,
    /// `f(for …)`, `return for …`): evaluates to a list of each iteration's last
    /// expression. A `for` that begins a statement stays [`parse_for`] (side
    /// effects, no collection).
    fn parse_for_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::ForExpr);
        let (var, iter, body) = self.parse_for_inner()?;
        self.ev_close();
        Ok(self.mk_expr(
            ExprKind::For {
                var,
                iter: Box::new(iter),
                body,
            },
            start,
        ))
    }

    /// Shared body of the statement and expression `for` forms: consumes
    /// `for <var> in <iter> do <body> end` and returns its parts.
    fn parse_for_inner(&mut self) -> Result<(String, Expr, Vec<Stmt>), String> {
        self.advance(); // consume 'for'
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
        Ok((var, iter, body))
    }

    /// `while` is statement-only (no expression / collecting form).
    fn parse_while(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::WhileStmt);
        self.advance(); // consume 'while'
        let condition = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&Token::Do)?;
        let body = self.parse_block_until(&[Token::End])?;
        self.expect(&Token::End)?;
        self.ev_close();
        Ok(self.mk_stmt(StmtKind::While { condition, body }, start))
    }

    fn parse_return(&mut self, start: usize) -> Result<Stmt, String> {
        self.ev_open(SyntaxKind::ReturnStmt);
        self.advance(); // consume 'return'
        let stmt = if matches!(
            self.peek(),
            Token::Newline | Token::End | Token::Else | Token::Elsif | Token::Eof
        ) {
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
            _ => None,
        }
    }

    /// Parse statements until the next significant token is one of `stops`
    /// (or Eof). Does NOT consume the stop token.
    fn parse_block_until(&mut self, stops: &[Token]) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        self.ev_open(SyntaxKind::Block);
        while !matches!(self.peek(), Token::Eof) && !stops.contains(self.peek()) {
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            self.skip_newlines();
        }
        self.ev_close();
        Ok(stmts)
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, String> {
        let mut params = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            self.expect_element_start("a parameter name")?;
            let name = self.expect_ident()?;
            let ty = self.parse_type_annotation()?;
            params.push(Param { name, ty });
            self.expect_element_separator(&Token::RParen, "parameters")?;
        }
        Ok(params)
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

    /// Format an error at a specific token position.
    fn error_at(&self, pos: usize, msg: String) -> String {
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
    fn expect_type_name(&mut self) -> Result<String, String> {
        let pos = self.pos;
        let name = match self.advance() {
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
        // A type is a single bare name; `list<int>` and friends are not a thing
        // (type-declarations-plan.md §1). Nothing valid follows a type name with
        // `<`, so claim the token and say what is actually wrong — otherwise the
        // mistake surfaces as whatever the *next* construct complains about,
        // which differs by position (a missing initializer for `let`/`state`, a
        // missing comma in a param list, and an unclosed JSX element for a
        // return type, since `<int>` lexes as a tag).
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
        while matches!(self.peek(), Token::Pipe) {
            let start = self.pos;
            self.advance();
            self.skip_newlines();
            let rhs = self.parse_or()?;
            self.ev_wrap(cp, SyntaxKind::CallExpr);
            left = match rhs.kind {
                ExprKind::Call { function, mut args } => {
                    args.insert(0, left);
                    self.mk_expr(ExprKind::Call { function, args }, start)
                }
                _ => self.mk_expr(
                    ExprKind::Call {
                        function: Box::new(rhs),
                        args: vec![left],
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
        while matches!(self.peek(), Token::Or) {
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
        while matches!(self.peek(), Token::And) {
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
        while matches!(self.peek(), Token::Eq | Token::Ne) {
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
        while matches!(self.peek(), Token::Lt | Token::Le | Token::Gt | Token::Ge) {
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
        while matches!(self.peek(), Token::DoubleQuestion) {
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
        while matches!(self.peek(), Token::PlusPlus) {
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
        while matches!(self.peek(), Token::Plus | Token::Minus) {
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
        while matches!(self.peek(), Token::Star | Token::Slash | Token::Percent) {
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
                Token::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
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
                    self.expect(&Token::RBracket)?;
                    self.ev_wrap(cp, SyntaxKind::IndexAccessExpr);
                    expr = Expr {
                        span: SourceSpan {
                            start: expr.span.start,
                            end: self.prev_span().end,
                            file: expr.span.file,
                        },
                        kind: ExprKind::IndexAccess {
                            object: Box::new(expr),
                            index: Box::new(index),
                        },
                    };
                }
                Token::LParen => {
                    self.check_callable(&expr)?;
                    self.ev_open(SyntaxKind::ArgList);
                    self.advance();
                    let args = self.parse_arg_list()?;
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
            | ExprKind::Call { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. } => Ok(()),

            // Not callable: literals, operators, collections, etc.
            ExprKind::AtVar(_) => {
                Err(self.error_at_current("`@var` cannot be called as a function".to_string()))
            }
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
            Token::At => {
                self.ev_open(SyntaxKind::AtVarExpr);
                self.advance(); // consume '@'
                let name = self.expect_ident()?;
                self.ev_close();
                Ok(self.mk_expr(ExprKind::AtVar(name), start))
            }
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
            Token::LParen => {
                self.ev_open(SyntaxKind::ParenExpr);
                self.advance();
                let expr = self.parse_expr()?;
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
            } else {
                let key = self.expect_ident()?;
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
        let then_body = self.parse_block_until(&[Token::Elsif, Token::Else, Token::End])?;
        let else_body = self.parse_else_chain()?;
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

    /// Parse the tail of an if-expression after the then-body. Consumes the
    /// single closing `end` for the whole if/elsif/else chain. Precondition:
    /// peek is Elsif, Else, or End.
    fn parse_else_chain(&mut self) -> Result<Option<ElseBranch>, String> {
        match self.peek() {
            Token::Elsif => {
                let start = self.pos;
                self.ev_open(SyntaxKind::ElseBranch);
                self.advance(); // consume 'elsif'
                let condition = self.parse_expr()?;
                self.skip_newlines();
                self.expect(&Token::Then)?;
                let then_body = self.parse_block_until(&[Token::Elsif, Token::Else, Token::End])?;
                let else_body = self.parse_else_chain()?; // consumes the final 'end'
                self.ev_close();
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
                self.ev_open(SyntaxKind::ElseBranch);
                self.advance(); // consume 'else'
                let body = self.parse_block_until(&[Token::End])?;
                self.expect(&Token::End)?;
                self.ev_close();
                Ok(Some(ElseBranch::Block(body)))
            }
            _ => {
                self.expect(&Token::End)?;
                Ok(None)
            }
        }
    }

    fn parse_match_expr(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.ev_open(SyntaxKind::MatchExpr);
        self.advance(); // consume 'match'
        let subject = self.parse_expr()?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while !matches!(self.peek(), Token::End | Token::Eof) {
            let arm = self.parse_match_arm()?;
            arms.push(arm);
            self.skip_newlines();
        }
        self.expect(&Token::End)?;
        self.ev_close();
        Ok(self.mk_expr(
            ExprKind::Match {
                subject: Box::new(subject),
                arms,
            },
            start,
        ))
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, String> {
        self.ev_open(SyntaxKind::MatchArm);
        self.expect(&Token::When)?;
        let pattern = self.parse_pattern()?;
        let guard = if matches!(self.peek(), Token::If) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        let body = if matches!(self.peek(), Token::Do) {
            let start = self.pos;
            self.advance(); // consume 'do'
            let stmts = self.parse_block_until(&[Token::End])?;
            self.expect(&Token::End)?;
            self.mk_expr(ExprKind::Block(stmts), start)
        } else {
            self.expect(&Token::Arrow)?;
            self.skip_newlines();
            self.parse_expr()?
        };
        self.ev_close(); // MatchArm — before the trailing newlines between arms
        self.skip_newlines();
        Ok(MatchArm {
            pattern,
            guard,
            body,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
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
                if matches!(self.peek(), Token::LParen) {
                    self.advance();
                    let mut fields = Vec::new();
                    self.skip_newlines();
                    while !matches!(self.peek(), Token::RParen | Token::Eof) {
                        self.expect_element_start("a variant field pattern")?;
                        let field_pat = self.parse_pattern()?;
                        fields.push(field_pat);
                        self.expect_element_separator(&Token::RParen, "variant fields")?;
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Pattern::Variant { name, fields })
                } else {
                    Ok(Pattern::Variable(name))
                }
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
            let key = self.expect_ident()?;
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
        self.ev_open(SyntaxKind::ParamList);
        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;
        self.ev_close(); // ParamList
        if matches!(self.peek(), Token::Arrow) {
            self.advance(); // consume '->'
            self.skip_newlines();
            let expr = self.parse_expr()?;
            let body = vec![self.mk_stmt(StmtKind::Expr(expr), start)];
            self.ev_close(); // LambdaExpr
            Ok(self.mk_expr(ExprKind::Lambda { params, body }, start))
        } else {
            self.skip_newlines();
            let body = self.parse_block_until(&[Token::End])?;
            self.expect(&Token::End)?;
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
                other => {
                    return Err(self.error_at_current(format!(
                        "Expected string part in interpolation, got {:?}",
                        other
                    )));
                }
            }

            if matches!(self.peek(), Token::InterpEnd) {
                self.advance();
                break;
            }

            let expr = self.parse_expr()?;
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
                Token::Ident(attr_name) => {
                    self.ev_open(SyntaxKind::JsxAttr);
                    self.advance();
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
                    children.push(JsxChild::Text(text));
                }
                Token::LBrace => {
                    self.advance();
                    let expr = self.parse_expr()?;
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

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut args = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek(), Token::RParen | Token::Eof) {
            self.expect_element_start("an argument")?;
            let arg = self.parse_expr()?;
            args.push(arg);
            self.expect_element_separator(&Token::RParen, "arguments")?;
        }
        Ok(args)
    }
}

/// Parse a hex color string (without '#') into (field_name, value) pairs.
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
