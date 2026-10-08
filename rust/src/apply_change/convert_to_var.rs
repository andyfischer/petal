//! `convert-to-var` — turn a `let` into a `var`, or a `state` into a
//! `state var`.
//!
//! ```text
//! state score = 0                        state var score = 0
//! fn award(n)                            fn award(n)
//!   score = score + n            =>        set score = get score + n
//! end                                    end
//! print(score)                           print(score)
//! ```
//!
//! A `let` is a dataflow binding and a `var` is a box (docs/var.md), so this
//! is not a respelling: a function that mentions the binding stops reading
//! the value captured where it was written and starts reading, and writing,
//! the one live cell. That is the point of running it — it is the fix for
//! "`score` is bound outside this function" — and it is why it lives under
//! `apply-change` rather than `lint`.
//!
//! # The rewrite
//!
//! Only mentions that resolve to the target binding are touched. Resolution
//! is lexical and follows the compiler: the binding is visible from the
//! statement after its declaration to the end of the block that declares it,
//! less every stretch where the name is bound again — by another
//! `let`/`var`/`state`, a parameter, a `for` variable, a match-pattern
//! binding, or a `fn` of that name. Within that:
//!
//! - the declaration's keyword changes: `let` → `var`, `state` → `state var`
//!   (`pub`, a `state(key)` group and a type annotation stay where they are);
//! - every `=` write rooted at the name gains `set`: `x = …`, `x += …`,
//!   `x.f = …`, `x[i] = …`, in the declaring function and in nested ones;
//! - every read inside a nested `fn` or lambda — a default parameter value
//!   included — gains `get`. A read in the declaring function stays bare, as
//!   does the root of a `set` target and the read a compound `set x += 1`
//!   implies, neither of which has a place to write one.
//!
//! # What it refuses
//!
//! - **`@x`** on the binding. `@` desugars to `x = f(x)` and is `let`-only;
//!   which of the two spellings the author wants afterwards is their call.
//! - **An importer that writes the binding** (`x = …`, `@x`, or `m.x = …`
//!   through the module's name), when the binding is exported. An importer
//!   may read an exported `var` and never write it (docs/module-system.md),
//!   so there is no rewrite to offer.
//! - **`config let`**, which cannot be a `var`; a binding that is already
//!   one; a path that names a function.

use std::path::Path;

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, Param, Pattern, Stmt, StmtKind,
    walk_expr, walk_stmt,
};
use crate::lexer::{Lexer, Token};
use crate::rewrite::Splice;

use super::importers::{self, Importer};
use super::target::{DeclKind, DeclTree, TargetPath};
use super::{ChangeOptions, Edited, ImporterNote, ImporterReport, Plan, count};

pub(super) fn plan(file: &Path, target: &str, opts: &ChangeOptions) -> Result<Plan, String> {
    let source = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (_tree, stmts) = crate::rewrite::parse_ast(&source)
        .map_err(|e| format!("{} does not parse: {e}", file.display()))?;
    let chars: Vec<char> = source.chars().collect();

    let tree = DeclTree::build(&stmts);
    let found = tree.resolve(&TargetPath::parse(target)?)?;
    let decl = &tree.decls[found];
    let path = tree.path(found);
    let name = decl.name.as_str();
    let to = match decl.kind {
        DeclKind::Let => "var",
        DeclKind::State => "state var",
        DeclKind::Var | DeclKind::StateVar => {
            return Err(format!(
                "{path} (line {}) is already a `{}`",
                decl.line,
                decl.kind.keyword()
            ));
        }
        DeclKind::ConfigLet => {
            return Err(format!(
                "{path} (line {}) is a `config let`, which cannot be a `var`: \
                 `config` marks a tuning knob, and a cell is not one",
                decl.line
            ));
        }
        DeclKind::Fn => {
            return Err(format!(
                "{path} (line {}) is a function; convert-to-var takes a `let` or `state` binding",
                decl.line
            ));
        }
    };

    let (stmt, rest) = find_declaration(&stmts, decl.offset)
        .ok_or_else(|| format!("internal error: lost the declaration of {path}"))?;
    let mut uses = Uses::new(name, &chars, false);
    uses.block(&rest);
    uses.check(file)?;

    let mut splices = vec![keyword_splice(&source, &stmt, name)?];
    splices.extend(uses.splices());
    let mut edited = vec![Edited {
        path: file.to_path_buf(),
        before: source.clone(),
        splices,
        detail: uses.detail(),
    }];

    // Only a module-level `pub` binding is visible to another file.
    let mut importer_report = None;
    let mut notes = Vec::new();
    if decl.exported && decl.parent.is_none() {
        let host = opts.host_env();
        let found = importers::discover(&host.env, file, name, &opts.from);
        notes.extend(found.notes);
        let mut refusals = Vec::new();
        let mut report = ImporterReport {
            how: found.how,
            importers: Vec::new(),
        };
        for importer in found.importers {
            let Importer {
                path,
                source,
                stmts,
                form,
                binds_bare,
                aliases,
            } = importer;
            // `m.x = …` is a write whichever way the name was imported. It
            // has never worked (a module is not a value), and it is still
            // the importer saying it wants to write the binding.
            let qualified = qualified_writes(&stmts, &aliases, name);
            if !qualified.is_empty() {
                refusals.push(importer_write_refusal(&path, name, &qualified));
                continue;
            }
            let mut detail = "reads it qualified, no change needed".to_string();
            if binds_bare {
                let chars: Vec<char> = source.chars().collect();
                let mut uses = Uses::new(name, &chars, true);
                uses.block(&stmts);
                if let Err(e) = uses.check(&path) {
                    refusals.push(e);
                    continue;
                }
                detail = uses.detail();
                let splices = uses.splices();
                if !splices.is_empty() {
                    edited.push(Edited {
                        path: path.clone(),
                        before: source,
                        splices,
                        detail: detail.clone(),
                    });
                }
            }
            report.importers.push(ImporterNote { path, form, detail });
        }
        if !refusals.is_empty() {
            return Err(refusals.join("\n"));
        }
        importer_report = Some(report);
    }

    let (files, gate_notes) = super::finish(edited, opts)?;
    notes.extend(gate_notes);
    Ok(Plan {
        operation: super::CONVERT_TO_VAR,
        summary: format!(
            "{path}: `{}` -> `{to}` ({} line {})",
            decl.kind.keyword(),
            file.display(),
            decl.line
        ),
        files,
        importers: importer_report,
        notes,
    })
}

/// The statement declared at char offset `offset`, with the statements that
/// follow it in its block — the stretch of code the binding is visible in.
/// Both are copied out: the walk goes through [`ExprVisitor`], which lends
/// each node only for the length of a call.
fn find_declaration(stmts: &[Stmt], offset: u32) -> Option<(Stmt, Vec<Stmt>)> {
    struct Finder {
        offset: u32,
        found: Option<(Stmt, Vec<Stmt>)>,
    }
    impl Finder {
        fn block(&mut self, stmts: &[Stmt]) {
            for (i, s) in stmts.iter().enumerate() {
                if self.found.is_some() {
                    return;
                }
                if s.span.start.offset == self.offset
                    && matches!(s.kind, StmtKind::Let { .. } | StmtKind::State { .. })
                {
                    self.found = Some((s.clone(), stmts[i + 1..].to_vec()));
                    return;
                }
                self.visit_stmt(s);
            }
        }
    }
    /// Route every statement list through [`Finder::block`].
    impl ExprVisitor for Finder {
        fn visit_stmt(&mut self, s: &Stmt) {
            match &s.kind {
                StmtKind::FnDecl { params, body, .. } => {
                    for default in params.iter().filter_map(|p| p.default.as_ref()) {
                        self.visit_expr(default);
                    }
                    self.block(body);
                }
                StmtKind::For { iter, body, .. } => {
                    self.visit_expr(iter);
                    self.block(body);
                }
                StmtKind::While { condition, body } => {
                    self.visit_expr(condition);
                    self.block(body);
                }
                _ => walk_stmt(self, s),
            }
        }

        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    self.visit_expr(condition);
                    self.block(then_body);
                    match else_body {
                        Some(ElseBranch::Block(stmts)) => self.block(stmts),
                        Some(ElseBranch::ElseIf(e)) => self.visit_expr(e),
                        None => {}
                    }
                }
                ExprKind::For { iter, body, .. } => {
                    self.visit_expr(iter);
                    self.block(body);
                }
                ExprKind::Lambda { params, body } => {
                    for default in params.iter().filter_map(|p| p.default.as_ref()) {
                        self.visit_expr(default);
                    }
                    self.block(body);
                }
                ExprKind::Block(stmts) => self.block(stmts),
                _ => walk_expr(self, e),
            }
        }
    }
    let mut finder = Finder {
        offset,
        found: None,
    };
    finder.block(stmts);
    finder.found
}

/// The splice that changes the declaration's keyword: `let` → `var`, or
/// `var ` inserted in front of a `state`'s name (after any `state(key)`
/// group). Found in the token stream, so a comment or an odd layout between
/// the keyword and the name cannot mislead it.
fn keyword_splice(source: &str, decl: &Stmt, name: &str) -> Result<Splice, String> {
    let lost = || format!("internal error: could not find the keyword declaring `{name}`");
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let tokens: Vec<(&Token, usize)> = lexer
        .tokens_with_spans()
        .map(|(t, span)| (t, span.start.offset as usize))
        .skip_while(|(_, at)| *at < decl.span.start.offset as usize)
        .collect();
    let is_name = |t: &Token| matches!(t, Token::Ident(n) if n == name);
    match &decl.kind {
        StmtKind::Let { .. } => {
            let i = tokens
                .iter()
                .position(|(t, _)| matches!(t, Token::Let))
                .ok_or_else(lost)?;
            if !tokens.get(i + 1).is_some_and(|(t, _)| is_name(t)) {
                return Err(lost());
            }
            let start = tokens[i].1;
            Ok(Splice {
                start,
                end: start + "let".len(),
                text: "var".to_string(),
            })
        }
        StmtKind::State { .. } => {
            let mut i = tokens
                .iter()
                .position(|(t, _)| matches!(t, Token::State))
                .ok_or_else(lost)?
                + 1;
            // Step over a `(key)` group, which may hold parentheses itself.
            if tokens
                .get(i)
                .is_some_and(|(t, _)| matches!(t, Token::LParen))
            {
                let mut depth = 0usize;
                loop {
                    match tokens.get(i).ok_or_else(lost)?.0 {
                        Token::LParen => depth += 1,
                        Token::RParen => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            match tokens.get(i) {
                Some((t, at)) if is_name(t) => Ok(Splice {
                    start: *at,
                    end: *at,
                    text: "var ".to_string(),
                }),
                _ => Err(lost()),
            }
        }
        _ => Err(lost()),
    }
}

/// Every mention of one name across a block that resolves to the binding
/// being converted, sorted into what each needs.
struct Uses<'a> {
    name: &'a str,
    chars: &'a [char],
    /// The walk is over a file that *imports* the binding, where a write is
    /// a refusal rather than a `set`.
    importer: bool,
    /// How many nested `fn`/lambda bodies deep the walk currently is.
    fn_depth: usize,
    /// The name is bound to something else from here to the end of the block.
    shadowed: bool,
    /// Start offsets of the `=` statements that write the binding.
    writes: Vec<(usize, u32)>,
    /// Start offsets of the bare reads inside nested functions.
    gets: Vec<usize>,
    /// Reads in the declaring function, which stay as they are.
    bare_reads: usize,
    /// Lines of `@name` rebinds.
    rebinds: Vec<u32>,
    /// A span that does not hold the text the AST says it does.
    lost: Option<u32>,
}

impl<'a> Uses<'a> {
    fn new(name: &'a str, chars: &'a [char], importer: bool) -> Self {
        Uses {
            name,
            chars,
            importer,
            fn_depth: 0,
            shadowed: false,
            writes: Vec::new(),
            gets: Vec::new(),
            bare_reads: 0,
            rebinds: Vec::new(),
            lost: None,
        }
    }

    /// Walk a statement list as a scope: a rebinding of the name inside it
    /// hides the binding for the rest of the list and no further.
    fn block(&mut self, stmts: &[Stmt]) {
        let outer = self.shadowed;
        for s in stmts {
            if self.shadowed {
                break;
            }
            self.visit_stmt(s);
        }
        self.shadowed = outer;
    }

    /// A nested function. One that takes a parameter of the same name reads
    /// its own parameter throughout — a default value included, since a
    /// default may only mention the parameters before it — so it is skipped
    /// whole. Otherwise its defaults and its body both run inside it.
    fn nested_fn(&mut self, params: &[Param], body: &[Stmt]) {
        if params.iter().any(|p| p.name == self.name) {
            return;
        }
        self.fn_depth += 1;
        for default in params.iter().filter_map(|p| p.default.as_ref()) {
            self.visit_expr(default);
        }
        self.block(body);
        self.fn_depth -= 1;
    }

    fn binds(&self, p: &Pattern) -> bool {
        match p {
            Pattern::Wildcard | Pattern::Literal(_) => false,
            Pattern::Variable(v) => v == self.name,
            Pattern::Variant { fields, .. } => fields.iter().any(|f| self.binds(f)),
            Pattern::List { elements, rest } => {
                elements.iter().any(|e| self.binds(e)) || rest.as_deref() == Some(self.name)
            }
            Pattern::Record(fields) => fields.iter().any(|(_, f)| self.binds(f)),
        }
    }

    /// Is the name written as a whole word at char offset `at`?
    fn name_at(&self, at: usize) -> bool {
        let end = at + self.name.chars().count();
        end <= self.chars.len()
            && self.chars[at..end].iter().copied().eq(self.name.chars())
            && self
                .chars
                .get(end)
                .is_none_or(|c| !(c.is_alphanumeric() || *c == '_'))
    }

    /// A write or read target: `x`, `x.f`, `x[i].g`. The root name is the
    /// thing written (it never takes a `get`); the index expressions along
    /// the way are ordinary reads.
    fn target(&mut self, target: &AssignTarget) {
        fn object(uses: &mut Uses<'_>, e: &Expr) {
            match &e.kind {
                ExprKind::Ident(_) | ExprKind::CellGet(_) => {}
                ExprKind::FieldAccess { object: o, .. } => object(uses, o),
                ExprKind::IndexAccess { object: o, index } => {
                    object(uses, o);
                    uses.visit_expr(index);
                }
                _ => uses.visit_expr(e),
            }
        }
        match target {
            AssignTarget::Name(_) => {}
            AssignTarget::Field(o, _) => object(self, o),
            AssignTarget::Index(o, index) => {
                object(self, o);
                self.visit_expr(index);
            }
        }
    }

    /// The value of a write. A compound write (`x += e`) is parsed as
    /// `x = x + e` with the left operand a copy of the target and the whole
    /// value spanning the statement; that copy has no text of its own, so
    /// only `e` is walked.
    fn written_value(&mut self, stmt: &Stmt, value: &Expr) {
        if value.span.start.offset == stmt.span.start.offset
            && let ExprKind::BinaryOp { right, .. } = &value.kind
        {
            self.visit_expr(right);
        } else {
            self.visit_expr(value);
        }
    }

    /// Refuse what cannot be rewritten.
    fn check(&self, file: &Path) -> Result<(), String> {
        let name = self.name;
        let file_path = file;
        let file = file.display();
        if let Some(line) = self.lost {
            return Err(format!(
                "internal error: {file} line {line}: a mention of `{name}` is not where the \
                 parser reported it; nothing was changed"
            ));
        }
        if self.importer {
            let mut written: Vec<u32> = self.writes.iter().map(|w| w.1).collect();
            written.extend(&self.rebinds);
            written.sort();
            if !written.is_empty() {
                return Err(importer_write_refusal(file_path, name, &written));
            }
        } else if !self.rebinds.is_empty() {
            return Err(format!(
                "refusing: `@{name}` rebinds the binding ({file} {}). `@` is `let`-only: it \
                 desugars to `{name} = f({name})`, which a `var` rejects. Write those calls out \
                 as `{name} = f({name})` first, and they will be converted to `set`",
                lines(&self.rebinds)
            ));
        }
        Ok(())
    }

    fn splices(&self) -> Vec<Splice> {
        let insert = |at: usize, text: &str| Splice {
            start: at,
            end: at,
            text: text.to_string(),
        };
        let mut out: Vec<Splice> = self.writes.iter().map(|w| insert(w.0, "set ")).collect();
        out.extend(self.gets.iter().map(|&at| insert(at, "get ")));
        out
    }

    /// `3 writes -> set, 2 reads -> get, 4 reads left bare`.
    fn detail(&self) -> String {
        let mut parts = Vec::new();
        if !self.writes.is_empty() {
            parts.push(format!("{} -> set", count(self.writes.len(), "write")));
        }
        if !self.gets.is_empty() {
            parts.push(format!("{} -> get", count(self.gets.len(), "read")));
        }
        if self.bare_reads > 0 {
            parts.push(format!("{} left bare", count(self.bare_reads, "read")));
        }
        if parts.is_empty() {
            "no other mention".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// `line 3`, `lines 3, 7`. Sorted input; repeats are said once.
fn lines(ls: &[u32]) -> String {
    let mut ls = ls.to_vec();
    ls.dedup();
    format!(
        "line{} {}",
        if ls.len() == 1 { "" } else { "s" },
        ls.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    )
}

/// The refusal for an importer that writes the binding on `written` lines.
fn importer_write_refusal(file: &Path, name: &str, written: &[u32]) -> String {
    format!(
        "refusing: {} imports `{name}` and writes it ({}). An importer may read an exported \
         `var` but never write it: export a function that does the write from the declaring \
         module, and call that instead",
        file.display(),
        lines(written)
    )
}

/// Lines of the writes spelled through the module's name: `m.x = …`,
/// `m.x[i] = …`, `set m.x.f = …`, for any `m` in `aliases`. A local that
/// shadows the alias is not looked for; such a write refuses too, which errs
/// on the side of asking.
fn qualified_writes(stmts: &[Stmt], aliases: &[String], name: &str) -> Vec<u32> {
    struct Finder<'a> {
        aliases: &'a [String],
        name: &'a str,
        lines: Vec<u32>,
    }
    impl Finder<'_> {
        /// Is `e` a path that starts `m.x`?
        fn through_module(&self, e: &Expr) -> bool {
            match &e.kind {
                ExprKind::FieldAccess { object, field } => match &object.kind {
                    ExprKind::Ident(m) => field == self.name && self.aliases.contains(m),
                    _ => self.through_module(object),
                },
                ExprKind::IndexAccess { object, .. } => self.through_module(object),
                _ => false,
            }
        }
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::Assign { target, .. } | StmtKind::Set { target, .. } = &s.kind {
                let written = match target {
                    AssignTarget::Name(_) => false,
                    AssignTarget::Field(object, field) => match &object.kind {
                        ExprKind::Ident(m) => field == self.name && self.aliases.contains(m),
                        _ => self.through_module(object),
                    },
                    AssignTarget::Index(object, _) => self.through_module(object),
                };
                if written {
                    self.lines.push(s.span.start.line);
                }
            }
            walk_stmt(self, s);
        }
    }
    if aliases.is_empty() {
        return Vec::new();
    }
    let mut finder = Finder {
        aliases,
        name,
        lines: Vec::new(),
    };
    for s in stmts {
        finder.visit_stmt(s);
    }
    finder.lines.sort();
    finder.lines
}

/// The name a write target is rooted at.
fn target_root(target: &AssignTarget) -> Option<&str> {
    fn root(e: &Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::Ident(n) | ExprKind::CellGet(n) => Some(n),
            ExprKind::FieldAccess { object, .. } | ExprKind::IndexAccess { object, .. } => {
                root(object)
            }
            _ => None,
        }
    }
    match target {
        AssignTarget::Name(n) => Some(n),
        AssignTarget::Field(object, _) | AssignTarget::Index(object, _) => root(object),
    }
}

impl ExprVisitor for Uses<'_> {
    fn visit_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            // The initializer still sees the binding; the new one takes over
            // from the next statement.
            StmtKind::Let { name, .. } | StmtKind::State { name, .. } => {
                walk_stmt(self, s);
                self.shadowed |= name == self.name;
            }
            StmtKind::FnDecl {
                name, params, body, ..
            } => {
                // Inside its own body the name is the function itself.
                if name == self.name {
                    self.shadowed = true;
                } else {
                    self.nested_fn(params, body);
                }
            }
            StmtKind::For { var, iter, body } => {
                self.visit_expr(iter);
                if var != self.name {
                    self.block(body);
                }
            }
            StmtKind::While { condition, body } => {
                self.visit_expr(condition);
                self.block(body);
            }
            StmtKind::Assign { target, value } => {
                if target_root(target) == Some(self.name) {
                    let at = s.span.start.offset as usize;
                    if self.name_at(at) {
                        self.writes.push((at, s.span.start.line));
                    } else {
                        self.lost = Some(s.span.start.line);
                    }
                }
                self.target(target);
                self.written_value(s, value);
            }
            // Already a `set`: nothing to add in front, but its index
            // expressions and its value are read like any other.
            StmtKind::Set { target, value } => {
                self.target(target);
                self.written_value(s, value);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) if n == self.name => {
                if self.fn_depth == 0 {
                    self.bare_reads += 1;
                } else if self.name_at(e.span.start.offset as usize) {
                    self.gets.push(e.span.start.offset as usize);
                } else {
                    self.lost = Some(e.span.start.line);
                }
            }
            ExprKind::AtVar(n) if n == self.name => self.rebinds.push(e.span.start.line),
            ExprKind::Lambda { params, body } => self.nested_fn(params, body),
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expr(condition);
                self.block(then_body);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => self.block(stmts),
                    Some(ElseBranch::ElseIf(e)) => self.visit_expr(e),
                    None => {}
                }
            }
            ExprKind::For { var, iter, body } => {
                self.visit_expr(iter);
                if var != self.name {
                    self.block(body);
                }
            }
            ExprKind::Block(stmts) => self.block(stmts),
            ExprKind::Match { subject, arms } => {
                self.visit_expr(subject);
                for arm in arms {
                    // A pattern that binds the name owns it for the arm.
                    if self.binds(&arm.pattern) {
                        continue;
                    }
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                }
            }
            _ => walk_expr(self, e),
        }
    }
}
