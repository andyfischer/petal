//! AST-based diff of two versions of one source file: what kind of change an
//! edit is, and where every position of the old text went.
//!
//! This is the first half of intelligent hot reload (docs/hot-reload.md). A
//! host that notices a source file changed asks [`diff_source`] what changed,
//! and the answer decides how much work the reload is:
//!
//! - [`SourceChange::None`] — the two texts parse to the same program. Either
//!   they are byte-identical, or they differ only in whitespace, comments and
//!   layout. Nothing has to be recompiled; the running program only needs its
//!   source positions moved ([`FileDiff::map_span`]).
//! - [`SourceChange::Values`] — the only differences are the *values* of
//!   literals (`0.35` became `0.4`, `"red"` became `"blue"`), each keeping its
//!   kind. A literal compiles to one constant load, and nothing else in the
//!   compiler looks at a literal's value (see "What a literal's value can
//!   reach" below), so the running program can be patched in place.
//! - [`SourceChange::Constructs`] — the structure changed: a function body, a
//!   binding added or removed, a literal that changed type. The list says
//!   which top-level constructs differ.
//! - [`SourceChange::Full`] — the new text does not parse, or differs in a way
//!   the AST does not show (`export` respelled `pub`, redundant parentheses).
//!
//! # What a literal's value can reach
//!
//! The claim behind `Values` is that swapping a literal's value changes one
//! constant and nothing else the compiler produced. That holds because the
//! front end reads literal *values* in exactly two places: the term the
//! literal compiles to (`compiler/expr.rs`), and the constant-`let` hoisting
//! rule, which accepts `a / <literal>` and `a % <literal>` only for a non-zero
//! literal (`compiler/mod.rs`, `hoistable_const_lets`). The type checker reads
//! only a literal's kind. So a literal that is the direct right operand of
//! `/` or `%` and crosses zero is reported as a construct change, and every
//! other same-kind change is a value change. Match-arm *patterns* hold their
//! literals in the program's arm metadata, not in constants; a changed pattern
//! is a construct change.
//!
//! # Signed numbers in config data
//!
//! `-3` parses as a negation of `3`, so dragging a value across zero changes
//! the tree's shape. Inside the initializer of a `config let` — the top level,
//! and through list elements, record fields and call arguments — the compiler
//! folds a negated number literal into one constant
//! ([`config_data_children`], [`signed_number`]), and the diff treats it as
//! one literal the same way, so `-0.5` to `0.5` is a value change there.

use std::collections::HashMap;

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, JsxChild, Literal, MatchArm, Param, Pattern,
    RecordField, Stmt, StmtKind, UnaryOp,
};
use crate::lexer::{Lexer, Token};
use crate::source_map::{FileId, SourcePosition, SourceSpan};

// ---------------------------------------------------------------------------
// Config data positions (shared with the compiler)
// ---------------------------------------------------------------------------

/// The value of `e` when it is a number literal or the negation of one —
/// `3`, `-3`, `0.5`, `-0.5` — with the sign applied.
///
/// In a config data position (see [`config_data_children`]) this is the unit
/// the compiler emits one constant for and the diff compares as one literal.
pub fn signed_number(e: &Expr) -> Option<Literal> {
    match &e.kind {
        ExprKind::Literal(l @ (Literal::Int(_) | Literal::Float(_))) => Some(l.clone()),
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            // `checked_neg` cannot fail for a literal (the lexer only produces
            // non-negative integers), but a hand-built AST could hold MIN.
            ExprKind::Literal(Literal::Int(n)) => n.checked_neg().map(Literal::Int),
            ExprKind::Literal(Literal::Float(f)) => Some(Literal::Float(-*f)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether the direct children of `e` that hold *data* stay in config data
/// position when `e` is in one: the elements of a list, the named fields of a
/// record, and the arguments of a call (a config file's constructors:
/// `rgb(255, 0, 80)`, `vec3(0.3, -1.0, 0.5)`). Everything else — operators,
/// accesses, blocks, lambdas — ends the data position.
///
/// The compiler and the diff both walk a `config let` initializer with this
/// rule, which is what keeps "the compiler folded this `-3`" and "the diff
/// treats this `-3` as one literal" the same set of expressions.
pub fn config_data_children(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::List(_) | ExprKind::Record(_) | ExprKind::Call { .. }
    )
}

/// Every negated number literal in a config data position of `value`, the
/// initializer of a `config let`: the expressions the compiler folds into one
/// constant each.
pub fn config_folded_numbers(value: &Expr, out: &mut impl FnMut(&Expr)) {
    if matches!(value.kind, ExprKind::UnaryOp { .. }) {
        if signed_number(value).is_some() {
            out(value);
        }
        return;
    }
    if !config_data_children(value) {
        return;
    }
    match &value.kind {
        ExprKind::List(items) => {
            for item in items {
                config_folded_numbers(item, out);
            }
        }
        ExprKind::Record(fields) => {
            for field in fields {
                if let RecordField::Named(_, v) = field {
                    config_folded_numbers(v, out);
                }
            }
        }
        ExprKind::Call { args, .. } => {
            for arg in args {
                config_folded_numbers(arg, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// The result
// ---------------------------------------------------------------------------

/// How two versions of a file differ. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceChange {
    /// The same program: identical text, or whitespace/comment/layout edits.
    None,
    /// Only literal values changed, each keeping its kind.
    Values(Vec<ValueChange>),
    /// Top-level constructs changed, appeared or disappeared.
    Constructs(Vec<ConstructChange>),
    /// Not classifiable as anything narrower; the string says why.
    Full(String),
}

impl SourceChange {
    /// Whether a running program can take this change without recompiling.
    pub fn is_incremental(&self) -> bool {
        matches!(self, SourceChange::None | SourceChange::Values(_))
    }

    /// The name used in reports: `none`, `values`, `constructs`, `full`.
    pub fn label(&self) -> &'static str {
        match self {
            SourceChange::None => "none",
            SourceChange::Values(_) => "values",
            SourceChange::Constructs(_) => "constructs",
            SourceChange::Full(_) => "full",
        }
    }
}

/// One literal whose value changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange {
    /// Where the literal sits inside a top-level `let` / `config let`, as a
    /// binding path (`GEN.diagonal.x0`, `POST.effects[2].amount`): the name,
    /// then `.field` into records and `[index]` into lists and call
    /// arguments. `None` for a literal anywhere else (a function body, a
    /// statement, an operand).
    pub path: Option<String>,
    /// The literal is inside a `config let` initializer.
    pub config: bool,
    pub old: Literal,
    pub new: Literal,
    /// The literal's span in the old text (the whole `-3` for a folded
    /// negative number in config data).
    pub old_span: SourceSpan,
    /// Its span in the new text.
    pub new_span: SourceSpan,
    /// This literal's index among the old file's literals that share
    /// `old_span`, in source order, and how many share it. More than one
    /// literal has the same span only for a color literal, whose `r`/`g`/`b`
    /// components are separate literals written by one token.
    pub ordinal: u32,
    pub span_count: u32,
}

/// What a changed top-level construct is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructKind {
    Function,
    Let,
    ConfigLet,
    Var,
    State,
    Enum,
    Class,
    Import,
    /// Any other top-level statement (a call, a loop, an assignment).
    Statement,
}

/// What happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructEdit {
    Added,
    Removed,
    Changed,
}

/// One top-level construct that differs between the two versions.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstructChange {
    pub kind: ConstructKind,
    /// The declared name, for the constructs that have one.
    pub name: Option<String>,
    pub edit: ConstructEdit,
    /// Where it is: in the new text, or in the old text for a removal.
    pub span: SourceSpan,
}

/// The diff of one file: the classification, plus (for a change that needs no
/// recompile) the map from old positions to new ones.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub change: SourceChange,
    /// The two texts are not byte-identical.
    pub text_changed: bool,
    remap: Option<PositionMap>,
}

impl FileDiff {
    /// Where a span of the old text is in the new text. Identity when the
    /// texts are identical. `None` when the change is not incremental, or the
    /// span does not start and end on something both versions have (a caller
    /// then falls back to recompiling).
    pub fn map_span(&self, span: SourceSpan) -> Option<SourceSpan> {
        if !self.text_changed {
            return Some(span);
        }
        self.remap.as_ref()?.map_span(span)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Compare two versions of one file. `file` is the id stamped on the spans of
/// both parses (the file's index in the program's file table).
pub fn diff_source(old: &str, new: &str, file: FileId) -> FileDiff {
    if old == new {
        return FileDiff {
            change: SourceChange::None,
            text_changed: false,
            remap: None,
        };
    }
    let full = |why: String| FileDiff {
        change: SourceChange::Full(why),
        text_changed: true,
        remap: None,
    };
    let old_parse = match Parsed::new(old, file) {
        Ok(p) => p,
        Err(e) => return full(format!("the running source does not parse: {e}")),
    };
    let new_parse = match Parsed::new(new, file) {
        Ok(p) => p,
        Err(e) => return full(format!("the new source does not parse: {e}")),
    };

    let change = classify(&old_parse.stmts, &new_parse.stmts);
    let values: &[ValueChange] = match &change {
        SourceChange::None => &[],
        SourceChange::Values(v) => v,
        _ => {
            return FileDiff {
                change,
                text_changed: true,
                remap: None,
            };
        }
    };
    // The trees agree; the token streams must too. They do not when the edit
    // is one the AST drops: `export` respelled `pub` (which changes a
    // warning), parentheses added or removed (which moves spans in ways no
    // token pairing describes).
    match PositionMap::build(&old_parse, &new_parse, values, old, new) {
        Ok(remap) => FileDiff {
            change,
            text_changed: true,
            remap: Some(remap),
        },
        Err(why) => full(why),
    }
}

struct Parsed {
    stmts: Vec<Stmt>,
    tokens: Vec<Token>,
    spans: Vec<SourceSpan>,
}

impl Parsed {
    /// Lex and parse with the parser's direct AST — the same statements the
    /// CST projection yields (`cst::parse_source_phased` asserts it), without
    /// building the tree.
    fn new(source: &str, file: FileId) -> Result<Parsed, String> {
        let mut lexer = Lexer::new_in_file(source, file);
        lexer.tokenize()?;
        let mut parser =
            crate::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
        let stmts = parser.parse_program()?;
        Ok(Parsed {
            stmts,
            tokens: lexer.tokens,
            spans: lexer.token_spans,
        })
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

fn classify(old: &[Stmt], new: &[Stmt]) -> SourceChange {
    if old.len() == new.len() {
        let mut cmp = Cmp::default();
        let mut changed: Vec<ConstructChange> = Vec::new();
        for (a, b) in old.iter().zip(new) {
            let mark = cmp.changes.len();
            if !cmp.top_stmt(a, b) {
                cmp.changes.truncate(mark);
                changed.push(construct(b, ConstructEdit::Changed));
            }
        }
        if !changed.is_empty() {
            return SourceChange::Constructs(changed);
        }
        if cmp.changes.is_empty() {
            return SourceChange::None;
        }
        for c in &mut cmp.changes {
            c.span_count = cmp.span_counts[&span_key(c.old_span)];
        }
        return SourceChange::Values(cmp.changes);
    }

    // A different number of statements: trim what the two ends share, then
    // pair what is left by declared name.
    let same = |a: &Stmt, b: &Stmt| {
        let mut cmp = Cmp::default();
        cmp.top_stmt(a, b) && cmp.changes.is_empty()
    };
    let mut head = 0;
    while head < old.len() && head < new.len() && same(&old[head], &new[head]) {
        head += 1;
    }
    let mut tail = 0;
    while tail < old.len() - head
        && tail < new.len() - head
        && same(&old[old.len() - 1 - tail], &new[new.len() - 1 - tail])
    {
        tail += 1;
    }
    let old_mid = &old[head..old.len() - tail];
    let new_mid = &new[head..new.len() - tail];
    let mut out = Vec::new();
    let mut used = vec![false; new_mid.len()];
    for a in old_mid {
        let key = construct_key(a);
        let partner = key.as_ref().and_then(|k| {
            new_mid
                .iter()
                .enumerate()
                .find(|(j, b)| !used[*j] && construct_key(b).as_ref() == Some(k))
        });
        match partner {
            Some((j, b)) => {
                used[j] = true;
                if !same(a, b) {
                    out.push(construct(b, ConstructEdit::Changed));
                }
            }
            None => out.push(construct(a, ConstructEdit::Removed)),
        }
    }
    for (j, b) in new_mid.iter().enumerate() {
        if !used[j] {
            out.push(construct(b, ConstructEdit::Added));
        }
    }
    if out.is_empty() {
        // Statements were only reordered.
        return SourceChange::Full("top-level statements were reordered".to_string());
    }
    SourceChange::Constructs(out)
}

fn construct_kind(s: &Stmt) -> (ConstructKind, Option<&str>) {
    match &s.kind {
        StmtKind::FnDecl { name, .. } => (ConstructKind::Function, Some(name)),
        StmtKind::Let {
            name,
            is_var,
            is_config,
            ..
        } => (
            if *is_config {
                ConstructKind::ConfigLet
            } else if *is_var {
                ConstructKind::Var
            } else {
                ConstructKind::Let
            },
            Some(name),
        ),
        StmtKind::State { name, .. } => (ConstructKind::State, Some(name)),
        StmtKind::EnumDecl { name, .. } => (ConstructKind::Enum, Some(name)),
        StmtKind::ClassDecl { name, .. } => (ConstructKind::Class, Some(name)),
        StmtKind::Import(decl) => (ConstructKind::Import, Some(&decl.module)),
        _ => (ConstructKind::Statement, None),
    }
}

/// What pairs a statement with its counterpart when statements were added or
/// removed around it: its kind and declared name.
fn construct_key(s: &Stmt) -> Option<(ConstructKind, String)> {
    let (kind, name) = construct_kind(s);
    name.map(|n| (kind, n.to_string()))
}

fn construct(s: &Stmt, edit: ConstructEdit) -> ConstructChange {
    let (kind, name) = construct_kind(s);
    ConstructChange {
        kind,
        name: name.map(str::to_string),
        edit,
        span: s.span,
    }
}

fn span_key(s: SourceSpan) -> (u32, u32) {
    (s.start.offset, s.end.offset)
}

fn same_kind(a: &Literal, b: &Literal) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Value equality as the constant table sees it: floats by bit pattern.
fn same_value(a: &Literal, b: &Literal) -> bool {
    match (a, b) {
        (Literal::Nil, Literal::Nil) => true,
        (Literal::Bool(x), Literal::Bool(y)) => x == y,
        (Literal::Int(x), Literal::Int(y)) => x == y,
        (Literal::Float(x), Literal::Float(y)) => x.to_bits() == y.to_bits(),
        (Literal::String(x), Literal::String(y)) => x == y,
        _ => false,
    }
}

fn is_zero(l: &Literal) -> bool {
    match l {
        Literal::Int(n) => *n == 0,
        Literal::Float(f) => *f == 0.0,
        _ => false,
    }
}

/// The structural comparison. Every method returns whether the two sides have
/// the same shape; literal value differences are collected, not failed on.
#[derive(Default)]
struct Cmp {
    changes: Vec<ValueChange>,
    /// How many literals of the old tree have each span, so far.
    span_counts: HashMap<(u32, u32), u32>,
    /// The binding path of the expression being compared, when it is data
    /// inside a top-level `let`.
    path: Option<String>,
    /// Inside a `config let` initializer at all (for [`ValueChange::config`]).
    config: bool,
    /// In a config data position right now (see [`config_data_children`]).
    config_data: bool,
}

impl Cmp {
    /// A top-level statement: the only place binding paths start.
    fn top_stmt(&mut self, a: &Stmt, b: &Stmt) -> bool {
        self.path = match &a.kind {
            StmtKind::Let {
                name, is_var: false, ..
            } => Some(name.clone()),
            _ => None,
        };
        let same = self.stmt(a, b);
        self.path = None;
        same
    }

    fn stmts(&mut self, a: &[Stmt], b: &[Stmt]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| self.stmt(x, y))
    }

    fn stmt(&mut self, a: &Stmt, b: &Stmt) -> bool {
        if a.exported != b.exported {
            return false;
        }
        // Paths belong to the top-level statement's own value; nested
        // statements (a lambda body inside it, say) have none.
        let path = self.path.take();
        let same = match (&a.kind, &b.kind) {
            (
                StmtKind::Let {
                    name: n1,
                    ty: t1,
                    value: v1,
                    is_var: var1,
                    is_config: c1,
                },
                StmtKind::Let {
                    name: n2,
                    ty: t2,
                    value: v2,
                    is_var: var2,
                    is_config: c2,
                },
            ) => {
                if n1 != n2 || t1 != t2 || var1 != var2 || c1 != c2 {
                    return false;
                }
                let saved = (self.config, self.config_data);
                // The compiler folds signed numbers in exactly this case
                // (`Compiler::compile_let_value`): a `config let` that is not
                // a `var`.
                self.config = *c1 && !*var1;
                self.config_data = self.config;
                self.path = path;
                let same = self.expr(v1, v2);
                self.path = None;
                (self.config, self.config_data) = saved;
                same
            }
            (
                StmtKind::Assign {
                    target: t1,
                    value: v1,
                },
                StmtKind::Assign {
                    target: t2,
                    value: v2,
                },
            )
            | (
                StmtKind::Set {
                    target: t1,
                    value: v1,
                },
                StmtKind::Set {
                    target: t2,
                    value: v2,
                },
            ) => self.target(t1, t2) && self.expr(v1, v2),
            (StmtKind::Expr(e1), StmtKind::Expr(e2)) => self.expr(e1, e2),
            (
                StmtKind::FnDecl {
                    name: n1,
                    class: k1,
                    params: p1,
                    ret: r1,
                    body: b1,
                },
                StmtKind::FnDecl {
                    name: n2,
                    class: k2,
                    params: p2,
                    ret: r2,
                    body: b2,
                },
            ) => n1 == n2 && k1 == k2 && r1 == r2 && self.params(p1, p2) && self.stmts(b1, b2),
            (
                StmtKind::EnumDecl {
                    name: n1,
                    variants: v1,
                },
                StmtKind::EnumDecl {
                    name: n2,
                    variants: v2,
                },
            ) => {
                n1 == n2
                    && v1.len() == v2.len()
                    && v1
                        .iter()
                        .zip(v2)
                        .all(|(x, y)| x.name == y.name && x.fields == y.fields)
            }
            (
                StmtKind::ClassDecl {
                    name: n1,
                    fields: f1,
                },
                StmtKind::ClassDecl {
                    name: n2,
                    fields: f2,
                },
            ) => {
                n1 == n2
                    && f1.len() == f2.len()
                    && f1.iter().zip(f2).all(|(x, y)| x.name == y.name && x.ty == y.ty)
            }
            (
                StmtKind::For {
                    var: x1,
                    iter: i1,
                    body: b1,
                },
                StmtKind::For {
                    var: x2,
                    iter: i2,
                    body: b2,
                },
            ) => x1 == x2 && self.expr(i1, i2) && self.stmts(b1, b2),
            (
                StmtKind::While {
                    condition: c1,
                    body: b1,
                },
                StmtKind::While {
                    condition: c2,
                    body: b2,
                },
            ) => self.expr(c1, c2) && self.stmts(b1, b2),
            (StmtKind::Return(r1), StmtKind::Return(r2)) => self.opt_expr(r1.as_ref(), r2.as_ref()),
            (StmtKind::Break, StmtKind::Break) | (StmtKind::Continue, StmtKind::Continue) => true,
            (
                StmtKind::State {
                    name: n1,
                    ty: t1,
                    init: i1,
                    key: k1,
                    is_var: v1,
                },
                StmtKind::State {
                    name: n2,
                    ty: t2,
                    init: i2,
                    key: k2,
                    is_var: v2,
                },
            ) => {
                n1 == n2
                    && t1 == t2
                    && v1 == v2
                    && self.opt_expr(k1.as_ref(), k2.as_ref())
                    && self.expr(i1, i2)
            }
            (StmtKind::Import(d1), StmtKind::Import(d2)) => {
                d1.module == d2.module
                    && d1.alias == d2.alias
                    && d1.names == d2.names
                    && d1.star == d2.star
                    && d1.exported == d2.exported
            }
            _ => false,
        };
        same
    }

    fn target(&mut self, a: &AssignTarget, b: &AssignTarget) -> bool {
        match (a, b) {
            (AssignTarget::Name(x), AssignTarget::Name(y)) => x == y,
            (AssignTarget::Field(o1, f1), AssignTarget::Field(o2, f2)) => {
                f1 == f2 && self.expr(o1, o2)
            }
            (AssignTarget::Index(o1, i1), AssignTarget::Index(o2, i2)) => {
                self.expr(o1, o2) && self.expr(i1, i2)
            }
            _ => false,
        }
    }

    fn params(&mut self, a: &[Param], b: &[Param]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.name == y.name
                    && x.ty == y.ty
                    && x.default_in_body == y.default_in_body
                    && self.opt_expr(x.default.as_ref(), y.default.as_ref())
            })
    }

    fn opt_expr(&mut self, a: Option<&Expr>, b: Option<&Expr>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => self.expr(x, y),
            _ => false,
        }
    }

    fn exprs(&mut self, a: &[Expr], b: &[Expr]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| self.expr(x, y))
    }

    /// One literal of the old tree (or one folded signed number), against its
    /// counterpart.
    fn literal(
        &mut self,
        old: &Literal,
        new: &Literal,
        old_span: SourceSpan,
        new_span: SourceSpan,
    ) -> bool {
        if !same_kind(old, new) {
            return false;
        }
        let count = self.span_counts.entry(span_key(old_span)).or_insert(0);
        let ordinal = *count;
        *count += 1;
        if !same_value(old, new) {
            self.changes.push(ValueChange {
                path: self.path.clone(),
                config: self.config,
                old: old.clone(),
                new: new.clone(),
                old_span,
                new_span,
                ordinal,
                span_count: 0,
            });
        }
        true
    }

    fn expr(&mut self, a: &Expr, b: &Expr) -> bool {
        // Taken here so every child starts outside both contexts unless this
        // node hands them on (the same shape as the compiler's walk).
        let in_data = std::mem::take(&mut self.config_data);
        let path = self.path.take();

        if in_data
            && let (Some(x), Some(y)) = (signed_number(a), signed_number(b))
        {
            self.path = path;
            let same = self.literal(&x, &y, a.span, b.span);
            self.path = None;
            return same;
        }

        match (&a.kind, &b.kind) {
            (ExprKind::Literal(x), ExprKind::Literal(y)) => {
                self.path = path;
                let same = self.literal(x, y, a.span, b.span);
                self.path = None;
                same
            }
            (ExprKind::Ident(x), ExprKind::Ident(y))
            | (ExprKind::AtVar(x), ExprKind::AtVar(y))
            | (ExprKind::CellGet(x), ExprKind::CellGet(y)) => x == y,
            (
                ExprKind::BinaryOp {
                    op: o1,
                    left: l1,
                    right: r1,
                },
                ExprKind::BinaryOp {
                    op: o2,
                    left: l2,
                    right: r2,
                },
            ) => {
                if o1 != o2 {
                    return false;
                }
                // The one place a literal's value steers the compiler: a
                // constant `let` is hoisted only when it divides by a non-zero
                // literal.
                if matches!(o1, crate::ast::BinOp::Div | crate::ast::BinOp::Mod)
                    && let (ExprKind::Literal(x), ExprKind::Literal(y)) = (&r1.kind, &r2.kind)
                    && is_zero(x) != is_zero(y)
                {
                    return false;
                }
                self.expr(l1, l2) && self.expr(r1, r2)
            }
            (
                ExprKind::UnaryOp {
                    op: o1,
                    operand: e1,
                },
                ExprKind::UnaryOp {
                    op: o2,
                    operand: e2,
                },
            ) => o1 == o2 && self.expr(e1, e2),
            (
                ExprKind::Call {
                    function: f1,
                    args: a1,
                    arg_names: n1,
                },
                ExprKind::Call {
                    function: f2,
                    args: a2,
                    arg_names: n2,
                },
            ) => {
                if n1 != n2 || a1.len() != a2.len() || !self.expr(f1, f2) {
                    return false;
                }
                a1.iter().zip(a2).enumerate().all(|(i, (x, y))| {
                    self.config_data = in_data;
                    self.path = path.as_ref().map(|p| format!("{p}[{i}]"));
                    self.expr(x, y)
                })
            }
            (
                ExprKind::If {
                    condition: c1,
                    then_body: t1,
                    else_body: e1,
                },
                ExprKind::If {
                    condition: c2,
                    then_body: t2,
                    else_body: e2,
                },
            ) => {
                self.expr(c1, c2)
                    && self.stmts(t1, t2)
                    && match (e1, e2) {
                        (None, None) => true,
                        (Some(ElseBranch::Block(x)), Some(ElseBranch::Block(y))) => {
                            self.stmts(x, y)
                        }
                        (Some(ElseBranch::ElseIf(x)), Some(ElseBranch::ElseIf(y))) => {
                            self.expr(x, y)
                        }
                        _ => false,
                    }
            }
            (
                ExprKind::Match {
                    subject: s1,
                    arms: a1,
                },
                ExprKind::Match {
                    subject: s2,
                    arms: a2,
                },
            ) => {
                self.expr(s1, s2)
                    && a1.len() == a2.len()
                    && a1.iter().zip(a2).all(|(x, y)| self.arm(x, y))
            }
            (
                ExprKind::For {
                    var: v1,
                    iter: i1,
                    body: b1,
                },
                ExprKind::For {
                    var: v2,
                    iter: i2,
                    body: b2,
                },
            ) => v1 == v2 && self.expr(i1, i2) && self.stmts(b1, b2),
            (ExprKind::List(x), ExprKind::List(y)) => {
                x.len() == y.len()
                    && x.iter().zip(y).enumerate().all(|(i, (p, q))| {
                        self.config_data = in_data;
                        self.path = path.as_ref().map(|base| format!("{base}[{i}]"));
                        self.expr(p, q)
                    })
            }
            (ExprKind::Record(x), ExprKind::Record(y)) => {
                x.len() == y.len()
                    && x.iter().zip(y).all(|(p, q)| match (p, q) {
                        (RecordField::Named(n1, e1), RecordField::Named(n2, e2)) => {
                            if n1 != n2 {
                                return false;
                            }
                            self.config_data = in_data;
                            self.path = path.as_ref().map(|base| format!("{base}.{n1}"));
                            self.expr(e1, e2)
                        }
                        (RecordField::Spread(e1), RecordField::Spread(e2)) => self.expr(e1, e2),
                        _ => false,
                    })
            }
            (
                ExprKind::FieldAccess {
                    object: o1,
                    field: f1,
                },
                ExprKind::FieldAccess {
                    object: o2,
                    field: f2,
                },
            ) => f1 == f2 && self.expr(o1, o2),
            (
                ExprKind::IndexAccess {
                    object: o1,
                    index: i1,
                },
                ExprKind::IndexAccess {
                    object: o2,
                    index: i2,
                },
            ) => self.expr(o1, o2) && self.expr(i1, i2),
            (ExprKind::OptionalAccess(x), ExprKind::OptionalAccess(y)) => self.expr(x, y),
            (ExprKind::Block(x), ExprKind::Block(y)) => self.stmts(x, y),
            (
                ExprKind::Lambda {
                    params: p1,
                    body: b1,
                },
                ExprKind::Lambda {
                    params: p2,
                    body: b2,
                },
            ) => self.params(p1, p2) && self.stmts(b1, b2),
            (
                ExprKind::StringInterp {
                    parts: p1,
                    exprs: e1,
                },
                ExprKind::StringInterp {
                    parts: p2,
                    exprs: e2,
                },
            ) => p1 == p2 && self.exprs(e1, e2),
            (
                ExprKind::Element {
                    tag: t1,
                    props: p1,
                    children: c1,
                },
                ExprKind::Element {
                    tag: t2,
                    props: p2,
                    children: c2,
                },
            ) => {
                t1 == t2
                    && p1.len() == p2.len()
                    && p1
                        .iter()
                        .zip(p2)
                        .all(|((k1, v1), (k2, v2))| k1 == k2 && self.expr(v1, v2))
                    && c1.len() == c2.len()
                    && c1.iter().zip(c2).all(|(x, y)| match (x, y) {
                        (JsxChild::Text(s1), JsxChild::Text(s2)) => s1 == s2,
                        (JsxChild::Expr(e1), JsxChild::Expr(e2)) => self.expr(e1, e2),
                        _ => false,
                    })
            }
            _ => false,
        }
    }

    fn arm(&mut self, a: &MatchArm, b: &MatchArm) -> bool {
        same_pattern(&a.pattern, &b.pattern)
            && self.opt_expr(a.guard.as_ref(), b.guard.as_ref())
            && self.expr(&a.body, &b.body)
    }
}

/// Patterns compare strictly, literals included: they are compiled into the
/// program's match-arm metadata rather than into constants.
fn same_pattern(a: &Pattern, b: &Pattern) -> bool {
    match (a, b) {
        (Pattern::Wildcard, Pattern::Wildcard) => true,
        (Pattern::Literal(x), Pattern::Literal(y)) => same_kind(x, y) && same_value(x, y),
        (Pattern::Variable(x), Pattern::Variable(y)) => x == y,
        (
            Pattern::Variant {
                name: n1,
                fields: f1,
            },
            Pattern::Variant {
                name: n2,
                fields: f2,
            },
        ) => {
            n1 == n2
                && f1.len() == f2.len()
                && f1.iter().zip(f2).all(|(x, y)| same_pattern(x, y))
        }
        (
            Pattern::List {
                elements: e1,
                rest: r1,
            },
            Pattern::List {
                elements: e2,
                rest: r2,
            },
        ) => {
            r1 == r2
                && e1.len() == e2.len()
                && e1.iter().zip(e2).all(|(x, y)| same_pattern(x, y))
        }
        (Pattern::Record(x), Pattern::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, p1), (k2, p2))| k1 == k2 && same_pattern(p1, p2))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

/// One stretch of the old text and where it is in the new text: a token both
/// versions have, or a literal whose value changed.
#[derive(Debug, Clone, Copy)]
struct Piece {
    old_start: u32,
    old_end: u32,
    new_start: SourcePosition,
    new_end: SourcePosition,
    /// The two stretches are the same length, so a position inside the old
    /// one is the same distance into the new one.
    rigid: bool,
}

/// Old position → new position, for two texts whose token streams agree apart
/// from the changed literals. Whitespace and comments belong to neither
/// stream, which is how a layout edit moves every span without touching the
/// program.
#[derive(Debug, Clone)]
struct PositionMap {
    /// Sorted by `old_start`; non-overlapping.
    pieces: Vec<Piece>,
    /// Length of the old text in chars: a position there is the end of file.
    old_len: u32,
    new_eof: SourcePosition,
    /// Char offset at which each line of the new text starts.
    new_line_starts: Vec<u32>,
}

impl PositionMap {
    fn build(
        old: &Parsed,
        new: &Parsed,
        values: &[ValueChange],
        old_text: &str,
        new_text: &str,
    ) -> Result<PositionMap, String> {
        // Changed literals, one entry per span (a color's components share
        // one), in source order.
        let mut units: Vec<(SourceSpan, SourceSpan)> =
            values.iter().map(|v| (v.old_span, v.new_span)).collect();
        units.sort_by_key(|(o, _)| o.start.offset);
        units.dedup_by_key(|(o, _)| (o.start.offset, o.end.offset));

        let significant = |p: &Parsed| -> Vec<(Token, SourceSpan)> {
            p.tokens
                .iter()
                .zip(&p.spans)
                // Line breaks and commas are layout: the tree records what
                // they separate, not where they are, and no span starts or
                // ends on one. (A trailing comma added when a record is
                // wrapped onto several lines must not count as a difference.)
                .filter(|(t, _)| !matches!(t, Token::Newline | Token::Eof | Token::Comma))
                .map(|(t, s)| (t.clone(), *s))
                .collect()
        };
        let a = significant(old);
        let b = significant(new);

        let mut pieces = Vec::with_capacity(a.len());
        let (mut i, mut j, mut u) = (0usize, 0usize, 0usize);
        let differ = || {
            "the two versions differ in a way their syntax trees do not show \
             (a respelled keyword, added or removed parentheses)"
                .to_string()
        };
        while i < a.len() || j < b.len() {
            if let Some((old_span, new_span)) = units.get(u)
                && i < a.len()
                && a[i].1.start.offset == old_span.start.offset
            {
                if j >= b.len() || b[j].1.start.offset != new_span.start.offset {
                    return Err(differ());
                }
                while i < a.len() && a[i].1.end.offset <= old_span.end.offset {
                    i += 1;
                }
                while j < b.len() && b[j].1.end.offset <= new_span.end.offset {
                    j += 1;
                }
                pieces.push(Piece {
                    old_start: old_span.start.offset,
                    old_end: old_span.end.offset,
                    new_start: new_span.start,
                    new_end: new_span.end,
                    rigid: false,
                });
                u += 1;
                continue;
            }
            if i >= a.len() || j >= b.len() || a[i].0 != b[j].0 {
                return Err(differ());
            }
            let (os, ns) = (a[i].1, b[j].1);
            pieces.push(Piece {
                old_start: os.start.offset,
                old_end: os.end.offset,
                new_start: ns.start,
                new_end: ns.end,
                rigid: os.end.offset - os.start.offset == ns.end.offset - ns.start.offset,
            });
            i += 1;
            j += 1;
        }
        if u != units.len() {
            return Err(differ());
        }

        let mut new_line_starts = vec![0u32];
        let mut count = 0u32;
        for c in new_text.chars() {
            count += 1;
            if c == '\n' {
                new_line_starts.push(count);
            }
        }
        let new_eof = SourcePosition {
            line: new_line_starts.len() as u32,
            column: count - new_line_starts[new_line_starts.len() - 1] + 1,
            offset: count,
        };
        Ok(PositionMap {
            pieces,
            old_len: old_text.chars().count() as u32,
            new_eof,
            new_line_starts,
        })
    }

    fn at_offset(&self, offset: u32) -> SourcePosition {
        let line = self.new_line_starts.partition_point(|&s| s <= offset) - 1;
        SourcePosition {
            line: line as u32 + 1,
            column: offset - self.new_line_starts[line] + 1,
            offset,
        }
    }

    /// Where an old position is in the new text. `end` says the position is
    /// the end of a span: one token's end is very often the next token's
    /// start (`{x`), and the two move apart when layout changes.
    fn map_pos(&self, p: SourcePosition, end: bool) -> Option<SourcePosition> {
        // The last piece that starts at or before `p`.
        let idx = self
            .pieces
            .partition_point(|piece| piece.old_start <= p.offset);
        let at = idx.checked_sub(1).map(|i| &self.pieces[i]);
        let before = idx.checked_sub(2).map(|i| &self.pieces[i]);
        if end {
            // The end of the piece before, when `p` is also where `at` starts.
            if let Some(prev) = before
                && prev.old_end == p.offset
            {
                return Some(prev.new_end);
            }
            if let Some(piece) = at
                && piece.old_end == p.offset
            {
                return Some(piece.new_end);
            }
        }
        if let Some(piece) = at {
            if piece.old_start == p.offset {
                return Some(piece.new_start);
            }
            if piece.old_end == p.offset {
                return Some(piece.new_end);
            }
            if p.offset < piece.old_end {
                return piece.rigid.then(|| {
                    self.at_offset(piece.new_start.offset + (p.offset - piece.old_start))
                });
            }
        }
        if p.offset >= self.old_len {
            return Some(self.new_eof);
        }
        None
    }

    fn map_span(&self, span: SourceSpan) -> Option<SourceSpan> {
        // The placeholder for "no position" (`ZERO_SPAN`: line 0) has none to
        // move.
        if span.start.line == 0 && span.end.line == 0 {
            return Some(span);
        }
        Some(SourceSpan {
            start: self.map_pos(span.start, false)?,
            end: self.map_pos(span.end, true)?,
            file: span.file,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map::ENTRY_FILE;

    fn change(old: &str, new: &str) -> SourceChange {
        diff_source(old, new, ENTRY_FILE).change
    }

    fn values(old: &str, new: &str) -> Vec<ValueChange> {
        match change(old, new) {
            SourceChange::Values(v) => v,
            other => panic!("expected a value change, got {other:?}"),
        }
    }

    #[test]
    fn identical_text_is_no_change() {
        let d = diff_source("let x = 1\n", "let x = 1\n", ENTRY_FILE);
        assert_eq!(d.change, SourceChange::None);
        assert!(!d.text_changed);
    }

    #[test]
    fn whitespace_and_comments_are_no_change() {
        let old = "let x = 1\nfn f(a)\n  a + x\nend\nprint(f(2))\n";
        let new = "// header\nlet   x =  1   // one\n\n\nfn f( a )\n      a + x\nend\n\nprint( f(2) )";
        let d = diff_source(old, new, ENTRY_FILE);
        assert_eq!(d.change, SourceChange::None);
        assert!(d.text_changed);
    }

    #[test]
    fn a_scalar_edit_is_a_value_change_with_its_path() {
        let v = values(
            "config let SPEED = 4.0\nprint(SPEED)\n",
            "config let SPEED = 5.5\nprint(SPEED)\n",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path.as_deref(), Some("SPEED"));
        assert!(v[0].config);
        assert!(matches!(v[0].new, Literal::Float(f) if f == 5.5));
    }

    #[test]
    fn nested_fields_elements_and_arguments_have_paths() {
        let old = "config let POST = {effects: [{amount: 0.2}, {amount: 0.3}], tint: rgb(1, 2, 3)}\n";
        let new = "config let POST = {effects: [{amount: 0.2}, {amount: 0.9}], tint: rgb(1, 7, 3)}\n";
        let v = values(old, new);
        let paths: Vec<_> = v.iter().map(|c| c.path.clone().unwrap()).collect();
        assert_eq!(paths, ["POST.effects[1].amount", "POST.tint[1]"]);
    }

    #[test]
    fn a_literal_in_a_function_body_is_a_value_change_without_a_path() {
        let v = values(
            "fn f(a)\n  a + 10\nend\n",
            "fn f(a)\n  a + 12\nend\n",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, None);
        assert!(!v[0].config);
    }

    #[test]
    fn a_sign_flip_is_a_value_change_only_in_config_data() {
        let v = values(
            "config let P = {x: -1.5, ys: [2, -3]}\n",
            "config let P = {x: 1.5, ys: [-2, -4]}\n",
        );
        assert_eq!(v.len(), 3);
        assert!(matches!(v[0].old, Literal::Float(f) if f == -1.5));
        assert!(matches!(v[0].new, Literal::Float(f) if f == 1.5));
        assert!(matches!(v[1].new, Literal::Int(-2)));
        assert!(matches!(v[2].new, Literal::Int(-4)));
        // A plain `let` keeps the negation as an operator.
        assert!(matches!(
            change("let x = -1\n", "let x = 1\n"),
            SourceChange::Constructs(_)
        ));
        // ... but a magnitude change under it is still a value change.
        assert_eq!(values("let x = -1\n", "let x = -2\n").len(), 1);
        // An operator ends the data position.
        assert!(matches!(
            change("config let x = 2 * -1\n", "config let x = 2 * 1\n"),
            SourceChange::Constructs(_)
        ));
    }

    #[test]
    fn a_type_changing_edit_is_a_construct_change() {
        match change("config let N = 10\n", "config let N = 10.5\n") {
            SourceChange::Constructs(c) => {
                assert_eq!(c.len(), 1);
                assert_eq!(c[0].kind, ConstructKind::ConfigLet);
                assert_eq!(c[0].name.as_deref(), Some("N"));
                assert_eq!(c[0].edit, ConstructEdit::Changed);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_divisor_crossing_zero_is_a_construct_change() {
        assert!(matches!(
            change("let H = 10 / 2\n", "let H = 10 / 0\n"),
            SourceChange::Constructs(_)
        ));
        assert_eq!(values("let H = 10 / 2\n", "let H = 10 / 5\n").len(), 1);
    }

    #[test]
    fn body_edits_additions_and_removals_name_the_construct() {
        let old = "let a = 1\nfn f(x)\n  x + a\nend\nprint(f(1))\n";
        match change(old, "let a = 1\nfn f(x)\n  x * a\nend\nprint(f(1))\n") {
            SourceChange::Constructs(c) => {
                assert_eq!(c.len(), 1);
                assert_eq!((c[0].kind, c[0].name.as_deref()), (ConstructKind::Function, Some("f")));
            }
            other => panic!("{other:?}"),
        }
        match change(old, "let a = 1\nlet b = 2\nfn f(x)\n  x + a\nend\nprint(f(1))\n") {
            SourceChange::Constructs(c) => {
                assert_eq!(c.len(), 1);
                assert_eq!(c[0].edit, ConstructEdit::Added);
                assert_eq!(c[0].name.as_deref(), Some("b"));
            }
            other => panic!("{other:?}"),
        }
        match change(old, "fn f(x)\n  x + a\nend\nprint(f(1))\n") {
            SourceChange::Constructs(c) => {
                assert_eq!(c.len(), 1);
                assert_eq!(c[0].edit, ConstructEdit::Removed);
                assert_eq!(c[0].name.as_deref(), Some("a"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pattern_literals_and_interpolated_text_are_structure() {
        assert!(matches!(
            change(
                "let r = match 1\n  when 1 -> 2\n  when _ -> 3\nend\n",
                "let r = match 1\n  when 4 -> 2\n  when _ -> 3\nend\n"
            ),
            SourceChange::Constructs(_)
        ));
        assert!(matches!(
            change("let s = \"a {1} b\"\n", "let s = \"a {1} c\"\n"),
            SourceChange::Constructs(_)
        ));
    }

    #[test]
    fn an_unparsable_new_source_is_full() {
        assert!(matches!(change("let x = 1\n", "let x = (\n"), SourceChange::Full(_)));
    }

    #[test]
    fn an_edit_the_tree_does_not_show_is_full() {
        assert!(matches!(
            change("export let x = 1\n", "pub let x = 1\n"),
            SourceChange::Full(_)
        ));
        assert!(matches!(
            change("let x = 1 + 2\n", "let x = (1 + 2)\n"),
            SourceChange::Full(_)
        ));
    }

    #[test]
    fn a_color_component_is_addressed_by_ordinal() {
        let v = values("config let C = #808080\n", "config let C = #804080\n");
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].ordinal, v[0].span_count), (1, 3));
        assert_eq!(v[0].path.as_deref(), Some("C.g"));
    }

    /// Every token of the old text maps to the same token of the new one.
    #[test]
    fn spans_follow_their_tokens_across_layout_and_value_edits() {
        let old = "config let A = {x: 1, y: 22}\nprint(A.y)\n";
        let new = "// moved\n\nconfig let A = {\n  x: 1000,\n  y: 22,\n}\nprint(A.y)\n";
        let d = diff_source(old, new, ENTRY_FILE);
        assert!(matches!(d.change, SourceChange::Values(_)));
        let lex = |s: &str| {
            let mut l = Lexer::new(s);
            l.tokenize().unwrap();
            l.tokens
                .iter()
                .cloned()
                .zip(l.token_spans.iter().copied())
                .filter(|(t, _)| !matches!(t, Token::Newline | Token::Eof | Token::Comma))
                .collect::<Vec<_>>()
        };
        let (a, b) = (lex(old), lex(new));
        assert_eq!(a.len(), b.len());
        for ((_, os), (_, ns)) in a.iter().zip(&b) {
            assert_eq!(d.map_span(*os), Some(*ns));
        }
    }
}
