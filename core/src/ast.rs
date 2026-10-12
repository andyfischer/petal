use serde::{Deserialize, Serialize};

use crate::source_map::SourceSpan;

// The serialized AST (`show-ast --json`) omits fields holding their default
// value — `false` flags, `None` annotations, empty variant field lists — via
// `skip_serializing_if`. Absence means the default. The two AST types that are
// also *de*serialized (`Pattern`/`Literal`, embedded in the IR JSON via
// `Program.match_arms`) pair each skip with `#[serde(default)]` so both the
// compact and the explicit spellings load.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `??` — short-circuit coalescing: yields the RHS when the LHS is absent
    /// (`Nil` or `Pending`), otherwise the LHS.
    Coalesce,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// An expression with source location.
#[derive(Debug, Clone, Serialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Serialize)]
pub enum ExprKind {
    Literal(Literal),
    Ident(String),
    /// `@name` — an in-out argument marker. Only ever produced by the parser;
    /// the [`crate::desugar`] pass rewrites `f(@x)` into `x = f(x)` and strips
    /// every `AtVar` before compilation. Any `AtVar` that survives to the
    /// compiler is an `@` used somewhere the desugar pass can't lift (e.g. not
    /// inside a call at statement level) and compiles to a deferred error.
    AtVar(String),
    /// `get name` — an explicit read of a `var` cell's current contents.
    ///
    /// Required wherever the read crosses a function boundary from the
    /// declaration, because that is exactly where a bare name would be
    /// ambiguous: a captured `let`/`state` reads the value as of the
    /// function's definition point, while a cell reads the value now. Inside
    /// the declaring scope both spellings mean the same thing and the bare one
    /// is still allowed.
    CellGet(String),
    BinaryOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Call {
        function: Box<Expr>,
        args: Vec<Expr>,
        /// The written name of each argument (`f(x, limit: 10)`), parallel to
        /// `args`. Empty means every argument is positional — the common case;
        /// when non-empty it has exactly the length of `args`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arg_names: Vec<Option<String>>,
    },
    If {
        condition: Box<Expr>,
        then_body: Vec<Stmt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        else_body: Option<ElseBranch>,
    },
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// A `for` loop used in value position (`x = for … do … end`): evaluates to
    /// a list built from the last expression of each iteration (a mapping). The
    /// statement form ([`StmtKind::For`]) runs purely for side effects and
    /// collects nothing. `while` has no expression form — it is statement-only.
    For {
        var: String,
        iter: Box<Expr>,
        body: Vec<Stmt>,
    },
    List(Vec<Expr>),
    Record(Vec<RecordField>),
    FieldAccess {
        object: Box<Expr>,
        field: String,
    },
    IndexAccess {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// An access chain written with `?.` somewhere in it (`cfg?.window.width`,
    /// `rows?.[0]`). The inner expression is an ordinary `FieldAccess`/
    /// `IndexAccess` spine; the wrapper marks that the *whole* spine reads
    /// absence-tolerantly, so a record that simply does not carry a link yields
    /// `Nil` rather than aborting — the same tolerance a `??` left-hand side
    /// already gets, made available without writing a fallback.
    ///
    /// One `?.` covers the chain it appears in, matching JavaScript's
    /// short-circuit: in `a?.b.c` a missing `b` yields nil instead of erroring
    /// on the `.c`.
    OptionalAccess(Box<Expr>),
    Block(Vec<Stmt>),
    Lambda {
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    /// String interpolation: alternating string parts and expressions.
    /// parts has one more element than exprs (parts[0], exprs[0], parts[1], exprs[1], ..., parts[N]).
    StringInterp {
        parts: Vec<String>,
        exprs: Vec<Expr>,
    },
    /// JSX-like element: `<tag props...>children</tag>`
    Element {
        tag: String,
        props: Vec<(String, Expr)>,
        children: Vec<JsxChild>,
    },
}

/// A field in a record literal: either a named field or a spread expression.
#[derive(Debug, Clone, Serialize)]
pub enum RecordField {
    /// Named field: `key: value`
    Named(String, Expr),
    /// Spread: `...expr` — copies all fields from another record
    Spread(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub enum JsxChild {
    Text(String),
    Expr(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub enum ElseBranch {
    Block(Vec<Stmt>),
    ElseIf(Box<Expr>),
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// Where the pattern is written — what a diagnostic about the pattern
    /// (an unknown variant, an unreachable arm) points at.
    #[serde(skip)]
    pub pattern_span: SourceSpan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    Wildcard,
    Literal(Literal),
    Variable(String),
    Variant {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<Pattern>,
    },
    List {
        elements: Vec<Pattern>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rest: Option<String>,
    },
    Record(Vec<(String, Pattern)>),
    /// `a | b | c`: matches what any alternative matches, trying them left to
    /// right. Every alternative binds the same names (the compiler checks),
    /// so the names an or-pattern binds are those of its first alternative.
    Or(Vec<Pattern>),
}

impl Pattern {
    /// The names this pattern binds, in the order they are written. An
    /// or-pattern contributes its first alternative's names: the others bind
    /// the same set.
    pub fn bound_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_bound_names(&mut out);
        out
    }

    fn collect_bound_names(&self, out: &mut Vec<String>) {
        match self {
            Pattern::Wildcard | Pattern::Literal(_) => {}
            Pattern::Variable(name) => out.push(name.clone()),
            Pattern::Variant { fields, .. } => {
                fields.iter().for_each(|f| f.collect_bound_names(out));
            }
            Pattern::List { elements, rest } => {
                elements.iter().for_each(|e| e.collect_bound_names(out));
                out.extend(rest.iter().cloned());
            }
            Pattern::Record(fields) => {
                fields.iter().for_each(|(_, p)| p.collect_bound_names(out));
            }
            Pattern::Or(alts) => {
                if let Some(first) = alts.first() {
                    first.collect_bound_names(out);
                }
            }
        }
    }

    /// The alternatives of the pattern's top level: the pattern itself, or
    /// the members of an or-pattern, flattened.
    pub fn alternatives(&self) -> Vec<&Pattern> {
        match self {
            Pattern::Or(alts) => alts.iter().flat_map(Pattern::alternatives).collect(),
            other => vec![other],
        }
    }

    /// Whether the pattern matches every value: `_`, a binding, or an
    /// or-pattern with such an alternative.
    pub fn is_irrefutable(&self) -> bool {
        match self {
            Pattern::Wildcard | Pattern::Variable(_) => true,
            Pattern::Or(alts) => alts.iter().any(Pattern::is_irrefutable),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum AssignTarget {
    Name(String),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
}

/// One variant of an `enum` declaration: `red`, `custom(r, g, b)`,
/// `park(shape: "area")`, `fight(place, won, tone: "bad")`.
#[derive(Debug, Clone, Serialize)]
pub struct EnumVariant {
    pub name: String,
    /// Payload fields: the bare identifiers inside the parens, in order. They
    /// are the variant constructor's positional parameters.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    /// Constant data: the `key: expr` entries inside the parens, in order.
    /// Evaluated once per declaration and read back as `variant.key`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<EnumData>,
    /// The whole variant as written, for a diagnostic about this variant.
    #[serde(skip)]
    pub span: SourceSpan,
}

/// One `key: expr` constant-data entry of an [`EnumVariant`].
#[derive(Debug, Clone, Serialize)]
pub struct EnumData {
    pub key: String,
    pub value: Expr,
}

/// A written type annotation: the source name plus its resolution.
/// `resolved` is `None` when `name` is not a recognized type — the checker
/// warns on that but treats it as `any`.
///
/// `span` covers the *name as written* (`nosuch` in `fn f(a: nosuch)`), which is
/// what a diagnostic about the annotation should underline. It is deliberately
/// outside `PartialEq`: two annotations are the same annotation when they spell
/// the same type, wherever they were written.
#[derive(Debug, Clone, Serialize)]
pub struct TypeAnn {
    /// The type name exactly as written (`int`, `str`, `banana`).
    pub name: String,
    /// The resolved static type, or `None` for an unrecognized name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<crate::types::Type>,
    /// Where the type name was written. [`ZERO_SPAN`] when synthesized.
    #[serde(skip)]
    pub span: SourceSpan,
}

impl PartialEq for TypeAnn {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.resolved == other.resolved
    }
}

impl TypeAnn {
    /// An annotation with no recorded position — for synthesized annotations
    /// and for tests. Real parses go through [`TypeAnn::at`].
    pub fn new(name: String) -> Self {
        Self::at(name, crate::source_map::ZERO_SPAN)
    }

    /// The annotation `name` as written at `span`.
    pub fn at(name: String, span: SourceSpan) -> Self {
        let resolved = crate::types::Type::from_name(&name);
        TypeAnn {
            name,
            resolved,
            span,
        }
    }
}

/// Is this declared return type `-> nil`? Such a function has no implicit
/// return: its body is compiled with the tail in statement position, and the
/// type checker does not hold the tail to the declared type. Any other
/// annotation, or none, leaves the tail as the function's value. See
/// docs/implicit-return-values.md.
pub fn declares_nil(ret: Option<&TypeAnn>) -> bool {
    ret.is_some_and(|ann| ann.resolved == Some(crate::types::Type::Nil))
}

/// One field of a `class` declaration: `x: int`. The annotation is optional
/// (an un-annotated field is `any`) and is kept verbatim even when the name is
/// unrecognized, exactly like a parameter's.
#[derive(Debug, Clone, Serialize)]
pub struct ClassFieldDecl {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeAnn>,
    /// The whole field declaration, `x: int` — what a diagnostic about *this
    /// field* (a duplicate name, say) underlines. Without it every field-level
    /// message fell back to the class's own span, i.e. line 1, column 1.
    #[serde(skip)]
    pub span: SourceSpan,
}

/// A function/lambda parameter with an optional declared type.
/// `ty` is `None` when the parameter is un-annotated. A written annotation is
/// preserved even when its name is unrecognized (`resolved: None`).
///
/// `default` is the expression written after `=` (`fn f(a, b = a * 2)`). It is
/// not a value: it is evaluated in the callee, on every call that leaves the
/// argument out. See docs/language-guide.md (Default Parameter Values).
#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeAnn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Expr>,
    /// Set by [`crate::desugar`] once it has moved `default` to the head of
    /// the function body (where every later pass sees it as ordinary code).
    /// `default` is then `None`; ask [`Param::has_default`] rather than either
    /// field.
    #[serde(skip)]
    pub default_in_body: bool,
    /// Written `name?` in a `native fn` declaration: a trailing parameter a
    /// call may leave off. A native has no default *expression* (the host
    /// fills the gap itself), so this is a flag rather than a `default`.
    /// Always `false` on a Petal `fn` or lambda parameter.
    #[serde(skip_serializing_if = "is_false")]
    pub optional: bool,
}

impl Param {
    /// A parameter with no annotation and no default.
    pub fn plain(name: String) -> Self {
        Param {
            name,
            ty: None,
            default: None,
            default_in_body: false,
            optional: false,
        }
    }

    /// Whether a call may leave this parameter out — true before and after
    /// the desugar pass relocates the default expression.
    pub fn has_default(&self) -> bool {
        self.default.is_some() || self.default_in_body
    }
}

/// The hidden binding that tells a function body whether the caller passed
/// the defaulted parameter `param`. `#` cannot appear in a source identifier,
/// so no program can name, shadow or capture it by accident.
pub fn arg_given_name(param: &str) -> String {
    format!("{param}#given")
}

/// How many of `params` a call must supply: the ones before the first default
/// (the parser guarantees every defaulted parameter is trailing).
pub fn required_param_count(params: &[Param]) -> usize {
    params.iter().take_while(|p| !p.has_default()).count()
}

/// A statement with source location.
#[derive(Debug, Clone, Serialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: SourceSpan,
    /// Whether this top-level declaration was written with the `pub`
    /// modifier (`pub fn`, `pub let`, `pub state`, `pub enum`).
    /// Only meaningful for a module's top-level `fn`/`let`/`state`/`enum`: it
    /// gates what importers can see (see `docs/module-system.md`). `false`
    /// everywhere else — nested statements and the entry file never export.
    #[serde(skip_serializing_if = "is_false")]
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum StmtKind {
    Let {
        name: String,
        /// Optional declared type (`let x: int = …`). `None` only when
        /// un-annotated; a written but unrecognized name is preserved as a
        /// [`TypeAnn`] with `resolved: None`.
        #[serde(skip_serializing_if = "Option::is_none")]
        ty: Option<TypeAnn>,
        value: Expr,
        /// Written `var x = …` rather than `let x = …`: a mutable cell, which
        /// is written with `set` and rejects `=`.
        /// See docs/var.md (Two write keywords).
        #[serde(skip_serializing_if = "is_false")]
        is_var: bool,
        /// Written `config let x = …`: the binding is declared as a tuning
        /// knob. Direct manipulation prefers editing config bindings and
        /// leaves the rest alone (docs/direct-manipulation.md); hosts may
        /// also render them as sliders. No effect on evaluation.
        #[serde(skip_serializing_if = "is_false")]
        is_config: bool,
    },
    Assign {
        target: AssignTarget,
        value: Expr,
    },
    /// `set x = …` — a write through a `var` cell. Distinct from [`Self::Assign`]
    /// on purpose: `=` is a dataflow rebind and `set` is a mutation, and each
    /// binding kind accepts exactly one of them.
    Set {
        target: AssignTarget,
        value: Expr,
    },
    Expr(Expr),
    FnDecl {
        /// The bound name. For a method declaration this is the *qualified*
        /// name `Class.method` — the same string the class's method table and
        /// the runtime dispatcher key on — so every existing path that binds,
        /// overloads or captures a function keeps working unchanged.
        name: String,
        /// `Some("Rect")` for `fn Rect.center_x(...)`: this declaration is a
        /// method on that class, and the first parameter is its receiver.
        /// `None` for an ordinary function.
        #[serde(skip_serializing_if = "Option::is_none")]
        class: Option<String>,
        params: Vec<Param>,
        /// Optional declared return type (`fn f(…) -> int`). `None` when
        /// un-annotated; a written but unrecognized name is preserved as a
        /// [`TypeAnn`] with `resolved: None`. Named functions only; lambdas have
        /// no return-type slot.
        #[serde(skip_serializing_if = "Option::is_none")]
        ret: Option<TypeAnn>,
        body: Vec<Stmt>,
    },
    EnumDecl {
        name: String,
        variants: Vec<EnumVariant>,
    },
    /// `class Rect ... end` — a named record type. The fields are in
    /// declaration order, which is also the order the generated constructor
    /// takes them in. See docs/language-guide.md (Classes & Methods).
    ClassDecl {
        name: String,
        fields: Vec<ClassFieldDecl>,
    },
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    State {
        name: String,
        /// Optional `state x: int = …` annotation. A reactive binding has no
        /// trustworthy inferred type (a later frame or `set` can replace it), so
        /// this is the only thing that lets the checker constrain a state name.
        #[serde(skip_serializing_if = "Option::is_none")]
        ty: Option<TypeAnn>,
        init: Expr,
        /// Optional explicit key expression for per-iteration state: `state(expr) name = init`
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<Expr>,
        /// Written `state var x = …`: a cell that persists across frames.
        #[serde(skip_serializing_if = "is_false")]
        is_var: bool,
    },
    /// `native fn name(a: int, b?) -> type` — the declaration of a function
    /// the *host* implements. No body and no `end`. See [`NativeFnDecl`] and
    /// docs/ffi.md (Native interfaces).
    NativeFn(NativeFnDecl),
    /// `import m` / `import m as u` / `import m: a, b`. Only allowed before
    /// any other statement in a file (the parser enforces this); consumed by
    /// the module loader (`crate::module`) before compilation — the compiler
    /// itself receives imports pre-resolved.
    Import(ImportDecl),
}

/// One parsed `native fn` declaration: a module-scoped name bound to an entry
/// in the host's native table.
///
/// ```petal ignore
/// native fn sqrt(x: num) -> float
/// native fn slice(collection, start: num, end?: num)
/// native fn print(...values)
/// pub native fn mouse_x() -> int
/// native fn _native_rect(x, y, w, h, r, g, b, a?) = "draw_rect"
/// ```
///
/// Several declarations of one name that bind the same host native are that
/// native's *call forms* (`random()`, `random(max)`, `random(min, max)`).
#[derive(Debug, Clone, Serialize)]
pub struct NativeFnDecl {
    /// The name the declaration binds in the module. For a method this is the
    /// qualified `Class.method`, as on [`StmtKind::FnDecl`].
    pub name: String,
    /// `Some("Rect")` for `native fn Rect.inset(r, n)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The fixed parameters. A trailing run may be [`Param::optional`]; none
    /// carries a default expression.
    pub params: Vec<Param>,
    /// `...name` — the native takes any number of further arguments. Such a
    /// native refuses named arguments, as a variadic one always has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest: Option<String>,
    /// The declared result type (`-> int`); `None` leaves it `any`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ret: Option<TypeAnn>,
    /// `= "host_name"` — the host native this binds, when it is registered
    /// under a different name. `None` binds the native of the same name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl NativeFnDecl {
    /// The name the host registered the native under.
    pub fn host_name(&self) -> &str {
        self.host.as_deref().unwrap_or(&self.name)
    }

    /// How many leading parameters every call must fill.
    pub fn required(&self) -> usize {
        self.params.iter().take_while(|p| !p.optional).count()
    }
}

/// One parsed `import` statement.
#[derive(Debug, Clone, Serialize)]
pub struct ImportDecl {
    /// The import path as written: a package path (`ui`, `bloom/menu`) or a
    /// relative one (`./level`, `../shared/rng`). The loader replaces it with
    /// the module's canonical name once it has resolved the path (see
    /// [`crate::module`]), so two spellings of one module compare equal from
    /// there on.
    pub module: String,
    /// `import ui as u` → Some("u"): the qualifier the module is reachable
    /// through. A path of several segments defaults it to the last one
    /// (`import bloom/menu` → Some("menu")), which the parser fills in; a
    /// single-segment path leaves this `None` and the qualifier is the module
    /// name itself. A selective import binds no qualifier, whatever this is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// `import ui: button, clicked` → Some(["button", "clicked"]): the only
    /// names bound. `None` is a plain import, which binds every export of
    /// the module bare, and the qualifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub names: Option<Vec<String>>,
    /// `import ui: *` — a form that no longer exists (a plain `import ui` is
    /// what it meant). Still parsed, so that the loader can reject it with
    /// an error that says so and `petal fix-imports` can rewrite it.
    #[serde(skip_serializing_if = "is_false")]
    pub star: bool,
    /// `pub import …` — what this import binds becomes part of what the
    /// importing module exports: every export of the target for a plain
    /// import, the listed names for a selective one. This is the declarative
    /// facade: one module re-presents another's surface.
    #[serde(skip_serializing_if = "is_false")]
    pub exported: bool,
}

/// The qualifier a module path binds by default: its last segment
/// (`"bloom/menu"` → `"menu"`, `"./ui/hud"` → `"hud"`, `"ui"` → `"ui"`).
/// Qualified access always goes through this name (`menu.open`), never the
/// path.
pub fn module_local_name(module: &str) -> &str {
    module.rsplit('/').next().unwrap_or(module)
}

// ---------------------------------------------------------------------------
// AST traversal
//
// A single exhaustive traversal over `ExprKind`/`StmtKind` lives here so the
// handful of hand-rolled walkers across the compiler (desugar's `@`-lifter,
// the phi pre-scan, lint's rebind and total walks, …) no longer each
// re-enumerate every variant. Adding an AST variant means updating `walk_expr`
// / `walk_stmt` (and their `_mut` twins) once, here, and the compiler then
// points every visitor at the missing arm.
//
// The default `visit_*` methods descend into *every* child. A walker that
// needs a narrower policy — stop at a call boundary, skip match arms, treat a
// nested body as its own scope — overrides only the methods whose children it
// treats differently and calls `walk_*` for the rest.
// ---------------------------------------------------------------------------

/// Read-only AST visitor. Default methods perform a total traversal; override
/// `visit_expr`/`visit_stmt` for the nodes whose children need a different
/// policy and delegate to [`walk_expr`]/[`walk_stmt`] for everything else.
pub trait ExprVisitor {
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s);
    }
}

/// Visit every direct child expression/statement of `e` with `v`.
pub fn walk_expr<V: ExprVisitor + ?Sized>(v: &mut V, e: &Expr) {
    match &e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) | ExprKind::CellGet(_) => {}
        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Call { function, args, .. } => {
            v.visit_expr(function);
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => {
            v.visit_expr(condition);
            for s in then_body {
                v.visit_stmt(s);
            }
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    for s in stmts {
                        v.visit_stmt(s);
                    }
                }
                Some(ElseBranch::ElseIf(e)) => v.visit_expr(e),
                None => {}
            }
        }
        ExprKind::Match { subject, arms } => {
            v.visit_expr(subject);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&arm.body);
            }
        }
        ExprKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body {
                v.visit_stmt(s);
            }
        }
        ExprKind::List(items) => {
            for e in items {
                v.visit_expr(e);
            }
        }
        ExprKind::Record(fields) => {
            for f in fields {
                match f {
                    RecordField::Named(_, e) | RecordField::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::OptionalAccess(inner) => v.visit_expr(inner),
        ExprKind::IndexAccess { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                v.visit_stmt(s);
            }
        }
        ExprKind::Lambda { params, body } => {
            for d in params.iter().filter_map(|p| p.default.as_ref()) {
                v.visit_expr(d);
            }
            for s in body {
                v.visit_stmt(s);
            }
        }
        ExprKind::StringInterp { exprs, .. } => {
            for e in exprs {
                v.visit_expr(e);
            }
        }
        ExprKind::Element {
            props, children, ..
        } => {
            for (_, e) in props {
                v.visit_expr(e);
            }
            for c in children {
                if let JsxChild::Expr(e) = c {
                    v.visit_expr(e);
                }
            }
        }
    }
}

/// Visit every direct child expression/statement of `s` with `v`.
pub fn walk_stmt<V: ExprVisitor + ?Sized>(v: &mut V, s: &Stmt) {
    match &s.kind {
        StmtKind::Let { value, .. } => v.visit_expr(value),
        StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
            match target {
                AssignTarget::Name(_) => {}
                AssignTarget::Field(object, _) => v.visit_expr(object),
                AssignTarget::Index(object, index) => {
                    v.visit_expr(object);
                    v.visit_expr(index);
                }
            }
            v.visit_expr(value);
        }
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::FnDecl { params, body, .. } => {
            for d in params.iter().filter_map(|p| p.default.as_ref()) {
                v.visit_expr(d);
            }
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::EnumDecl { variants, .. } => {
            for d in variants.iter().flat_map(|variant| &variant.data) {
                v.visit_expr(&d.value);
            }
        }
        StmtKind::ClassDecl { .. } => {}
        StmtKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::While { condition, body } => {
            v.visit_expr(condition);
            for s in body {
                v.visit_stmt(s);
            }
        }
        StmtKind::Return(value) => {
            if let Some(e) = value {
                v.visit_expr(e);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::State { init, key, .. } => {
            v.visit_expr(init);
            if let Some(k) = key {
                v.visit_expr(k);
            }
        }
        StmtKind::Import(_) | StmtKind::NativeFn(_) => {}
    }
}

/// Mutable mirror of [`ExprVisitor`] for passes that rewrite the AST in place.
pub trait ExprVisitorMut {
    fn visit_expr(&mut self, e: &mut Expr) {
        walk_expr_mut(self, e);
    }
    fn visit_stmt(&mut self, s: &mut Stmt) {
        walk_stmt_mut(self, s);
    }
}

/// Visit every direct child expression/statement of `e` with `v` (mutable).
pub fn walk_expr_mut<V: ExprVisitorMut + ?Sized>(v: &mut V, e: &mut Expr) {
    match &mut e.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) | ExprKind::AtVar(_) | ExprKind::CellGet(_) => {}
        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Call { function, args, .. } => {
            v.visit_expr(function);
            for a in args.iter_mut() {
                v.visit_expr(a);
            }
        }
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => {
            v.visit_expr(condition);
            for s in then_body.iter_mut() {
                v.visit_stmt(s);
            }
            match else_body {
                Some(ElseBranch::Block(stmts)) => {
                    for s in stmts.iter_mut() {
                        v.visit_stmt(s);
                    }
                }
                Some(ElseBranch::ElseIf(e)) => v.visit_expr(e),
                None => {}
            }
        }
        ExprKind::Match { subject, arms } => {
            v.visit_expr(subject);
            for arm in arms.iter_mut() {
                if let Some(g) = &mut arm.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&mut arm.body);
            }
        }
        ExprKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        ExprKind::List(items) => {
            for e in items.iter_mut() {
                v.visit_expr(e);
            }
        }
        ExprKind::Record(fields) => {
            for f in fields.iter_mut() {
                match f {
                    RecordField::Named(_, e) | RecordField::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::OptionalAccess(inner) => v.visit_expr(inner),
        ExprKind::IndexAccess { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::Block(stmts) => {
            for s in stmts.iter_mut() {
                v.visit_stmt(s);
            }
        }
        ExprKind::Lambda { params, body } => {
            for d in params.iter_mut().filter_map(|p| p.default.as_mut()) {
                v.visit_expr(d);
            }
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        ExprKind::StringInterp { exprs, .. } => {
            for e in exprs.iter_mut() {
                v.visit_expr(e);
            }
        }
        ExprKind::Element {
            props, children, ..
        } => {
            for (_, e) in props.iter_mut() {
                v.visit_expr(e);
            }
            for c in children.iter_mut() {
                if let JsxChild::Expr(e) = c {
                    v.visit_expr(e);
                }
            }
        }
    }
}

/// Visit every direct child expression/statement of `s` with `v` (mutable).
pub fn walk_stmt_mut<V: ExprVisitorMut + ?Sized>(v: &mut V, s: &mut Stmt) {
    match &mut s.kind {
        StmtKind::Let { value, .. } => v.visit_expr(value),
        StmtKind::Assign { target, value } | StmtKind::Set { target, value } => {
            match target {
                AssignTarget::Name(_) => {}
                AssignTarget::Field(object, _) => v.visit_expr(object),
                AssignTarget::Index(object, index) => {
                    v.visit_expr(object);
                    v.visit_expr(index);
                }
            }
            v.visit_expr(value);
        }
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::FnDecl { params, body, .. } => {
            for d in params.iter_mut().filter_map(|p| p.default.as_mut()) {
                v.visit_expr(d);
            }
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::EnumDecl { variants, .. } => {
            for d in variants.iter_mut().flat_map(|variant| &mut variant.data) {
                v.visit_expr(&mut d.value);
            }
        }
        StmtKind::ClassDecl { .. } => {}
        StmtKind::For { iter, body, .. } => {
            v.visit_expr(iter);
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::While { condition, body } => {
            v.visit_expr(condition);
            for s in body.iter_mut() {
                v.visit_stmt(s);
            }
        }
        StmtKind::Return(value) => {
            if let Some(e) = value {
                v.visit_expr(e);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
        StmtKind::State { init, key, .. } => {
            v.visit_expr(init);
            if let Some(k) = key {
                v.visit_expr(k);
            }
        }
        StmtKind::Import(_) | StmtKind::NativeFn(_) => {}
    }
}

/// Visit every expression in `e`'s subtree (pre-order), descending into nested
/// statements. The closure-shaped counterpart to a total [`ExprVisitor`]; used
/// where a caller just wants to inspect every expression.
pub fn for_each_expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    ForEachExpr(f).visit_expr(e);
}

/// Like [`for_each_expr`] but rooted at a statement.
pub fn for_each_expr_in_stmt(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    ForEachExpr(f).visit_stmt(s);
}

struct ForEachExpr<'a, F>(&'a mut F);

impl<F: FnMut(&Expr)> ExprVisitor for ForEachExpr<'_, F> {
    fn visit_expr(&mut self, e: &Expr) {
        (self.0)(e);
        walk_expr(self, e);
    }
}
