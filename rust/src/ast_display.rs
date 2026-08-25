//! Compact text rendering of the AST for `show-ast`.
//!
//! Clang/swiftc-style dump: one node per line, the node kind plus its key
//! facts inline, children indented two spaces, spans as `@line:col-line:col`
//! (collapsed to `@line:col` for single-character spans). Scalar facts
//! (names, operators, literal values) live on the node line; structural
//! children get their own lines. Patterns and type annotations are rendered
//! source-like. Default/absent facts (`exported: false`, `is_var: false`,
//! `ty: None`, …) are elided.
//!
//! This is a debug view, not a contract — `show-ast --json` is the stable
//! machine-readable form.

use std::fmt::Write;

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, ImportDecl, JsxChild, Literal, MatchArm, Param,
    Pattern, RecordField, Stmt, StmtKind, TypeAnn,
};
use crate::source_map::SourceSpan;

/// Render a parsed program (a list of top-level statements) as a compact tree.
pub fn display_stmts(stmts: &[Stmt]) -> String {
    let mut p = Printer::new();
    for stmt in stmts {
        p.stmt(stmt, 0);
    }
    p.out
}

struct Printer {
    out: String,
}

impl Printer {
    fn new() -> Self {
        Printer { out: String::new() }
    }

    /// Write one node line: indentation, the rendered head, and the span.
    fn line(&mut self, depth: usize, head: &str, span: Option<&SourceSpan>) {
        for _ in 0..depth {
            self.out.push_str("  ");
        }
        self.out.push_str(head);
        if let Some(span) = span {
            let _ = write!(self.out, " {}", fmt_span(span));
        }
        self.out.push('\n');
    }

    fn stmts(&mut self, stmts: &[Stmt], depth: usize) {
        for s in stmts {
            self.stmt(s, depth);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, depth: usize) {
        let export = if stmt.exported { "export " } else { "" };
        match &stmt.kind {
            StmtKind::Let {
                name,
                ty,
                value,
                is_var,
                is_config,
            } => {
                let config = if *is_config { "config " } else { "" };
                let var = if *is_var { "var " } else { "" };
                let head = format!("Let {export}{config}{var}{name}{}", fmt_ty_suffix(ty));
                self.line(depth, &head, Some(&stmt.span));
                self.expr(value, depth + 1);
            }
            StmtKind::Assign { target, value } => {
                self.assign_like("Assign", target, value, stmt, depth);
            }
            StmtKind::Set { target, value } => {
                self.assign_like("Set", target, value, stmt, depth);
            }
            StmtKind::Expr(e) => {
                self.line(depth, "Expr", Some(&stmt.span));
                self.expr(e, depth + 1);
            }
            StmtKind::FnDecl {
                name,
                class: _, // already visible in the qualified name (`Rect.center_x`)
                params,
                ret,
                body,
            } => {
                let ret = match ret {
                    Some(t) => format!(" -> {}", t.name),
                    None => String::new(),
                };
                let head = format!("FnDecl {export}{name} {}{ret}", fmt_params(params));
                self.line(depth, &head, Some(&stmt.span));
                self.stmts(body, depth + 1);
            }
            StmtKind::EnumDecl { name, variants } => {
                self.line(depth, &format!("EnumDecl {export}{name}"), Some(&stmt.span));
                for v in variants {
                    let head = if v.fields.is_empty() {
                        format!("Variant {}", v.name)
                    } else {
                        format!("Variant {}({})", v.name, v.fields.join(", "))
                    };
                    self.line(depth + 1, &head, None);
                }
            }
            StmtKind::ClassDecl { name, fields } => {
                self.line(
                    depth,
                    &format!("ClassDecl {export}{name}"),
                    Some(&stmt.span),
                );
                for f in fields {
                    let head = format!("Field {}{}", f.name, fmt_ty_suffix(&f.ty));
                    self.line(depth + 1, &head, Some(&f.span));
                }
            }
            StmtKind::For { var, iter, body } => {
                self.line(depth, &format!("For {var}"), Some(&stmt.span));
                self.expr(iter, depth + 1);
                self.stmts(body, depth + 1);
            }
            StmtKind::While { condition, body } => {
                self.line(depth, "While", Some(&stmt.span));
                self.expr(condition, depth + 1);
                self.stmts(body, depth + 1);
            }
            StmtKind::Return(value) => {
                self.line(depth, "Return", Some(&stmt.span));
                if let Some(e) = value {
                    self.expr(e, depth + 1);
                }
            }
            StmtKind::Break => self.line(depth, "Break", Some(&stmt.span)),
            StmtKind::Continue => self.line(depth, "Continue", Some(&stmt.span)),
            StmtKind::State {
                name,
                ty,
                init,
                key,
                is_var,
            } => {
                let var = if *is_var { "var " } else { "" };
                let head = format!("State {export}{var}{name}{}", fmt_ty_suffix(ty));
                self.line(depth, &head, Some(&stmt.span));
                if let Some(k) = key {
                    self.line(depth + 1, "Key", None);
                    self.expr(k, depth + 2);
                }
                self.expr(init, depth + 1);
            }
            StmtKind::Import(decl) => {
                self.line(depth, &fmt_import(decl), Some(&stmt.span));
            }
        }
    }

    fn assign_like(
        &mut self,
        kind: &str,
        target: &AssignTarget,
        value: &Expr,
        stmt: &Stmt,
        depth: usize,
    ) {
        match target {
            AssignTarget::Name(name) => {
                self.line(depth, &format!("{kind} {name}"), Some(&stmt.span));
            }
            AssignTarget::Field(object, field) => {
                self.line(depth, &format!("{kind} .{field}"), Some(&stmt.span));
                self.expr(object, depth + 1);
            }
            AssignTarget::Index(object, index) => {
                self.line(depth, &format!("{kind} []"), Some(&stmt.span));
                self.expr(object, depth + 1);
                self.expr(index, depth + 1);
            }
        }
        self.expr(value, depth + 1);
    }

    fn expr(&mut self, expr: &Expr, depth: usize) {
        let span = Some(&expr.span);
        match &expr.kind {
            ExprKind::Literal(lit) => {
                self.line(depth, &format!("Literal {}", fmt_literal(lit)), span);
            }
            ExprKind::Ident(name) => self.line(depth, &format!("Ident {name}"), span),
            ExprKind::AtVar(name) => self.line(depth, &format!("AtVar {name}"), span),
            ExprKind::CellGet(name) => self.line(depth, &format!("CellGet {name}"), span),
            ExprKind::BinaryOp { op, left, right } => {
                self.line(depth, &format!("BinaryOp {op:?}"), span);
                self.expr(left, depth + 1);
                self.expr(right, depth + 1);
            }
            ExprKind::UnaryOp { op, operand } => {
                self.line(depth, &format!("UnaryOp {op:?}"), span);
                self.expr(operand, depth + 1);
            }
            ExprKind::Call { function, args } => {
                self.line(depth, "Call", span);
                self.expr(function, depth + 1);
                for a in args {
                    self.expr(a, depth + 1);
                }
            }
            ExprKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.line(depth, "If", span);
                self.expr(condition, depth + 1);
                self.line(depth + 1, "Then", None);
                self.stmts(then_body, depth + 2);
                match else_body {
                    Some(ElseBranch::Block(stmts)) => {
                        self.line(depth + 1, "Else", None);
                        self.stmts(stmts, depth + 2);
                    }
                    Some(ElseBranch::ElseIf(e)) => {
                        self.line(depth + 1, "Else", None);
                        self.expr(e, depth + 2);
                    }
                    None => {}
                }
            }
            ExprKind::Match { subject, arms } => {
                self.line(depth, "Match", span);
                self.expr(subject, depth + 1);
                for arm in arms {
                    self.match_arm(arm, depth + 1);
                }
            }
            ExprKind::For { var, iter, body } => {
                self.line(depth, &format!("For {var}"), span);
                self.expr(iter, depth + 1);
                self.stmts(body, depth + 1);
            }
            ExprKind::List(items) => {
                self.line(depth, "List", span);
                for e in items {
                    self.expr(e, depth + 1);
                }
            }
            ExprKind::Record(fields) => {
                self.line(depth, "Record", span);
                for f in fields {
                    match f {
                        RecordField::Named(name, e) => {
                            self.line(depth + 1, &format!("Field {name}"), None);
                            self.expr(e, depth + 2);
                        }
                        RecordField::Spread(e) => {
                            self.line(depth + 1, "Spread", None);
                            self.expr(e, depth + 2);
                        }
                    }
                }
            }
            ExprKind::FieldAccess { object, field } => {
                self.line(depth, &format!("FieldAccess .{field}"), span);
                self.expr(object, depth + 1);
            }
            ExprKind::IndexAccess { object, index } => {
                self.line(depth, "IndexAccess", span);
                self.expr(object, depth + 1);
                self.expr(index, depth + 1);
            }
            ExprKind::OptionalAccess(inner) => {
                self.line(depth, "OptionalAccess", span);
                self.expr(inner, depth + 1);
            }
            ExprKind::Block(stmts) => {
                self.line(depth, "Block", span);
                self.stmts(stmts, depth + 1);
            }
            ExprKind::Lambda { params, body } => {
                self.line(depth, &format!("Lambda {}", fmt_params(params)), span);
                self.stmts(body, depth + 1);
            }
            ExprKind::StringInterp { parts, exprs } => {
                self.line(depth, "StringInterp", span);
                // parts and exprs alternate: parts[0], exprs[0], …, parts[N].
                // Empty string parts are elided as noise.
                for (i, part) in parts.iter().enumerate() {
                    if !part.is_empty() {
                        self.line(depth + 1, &format!("Part {part:?}"), None);
                    }
                    if let Some(e) = exprs.get(i) {
                        self.expr(e, depth + 1);
                    }
                }
            }
            ExprKind::Element {
                tag,
                props,
                children,
            } => {
                self.line(depth, &format!("Element {tag}"), span);
                for (name, e) in props {
                    self.line(depth + 1, &format!("Prop {name}"), None);
                    self.expr(e, depth + 2);
                }
                for c in children {
                    match c {
                        JsxChild::Text(text) => {
                            self.line(depth + 1, &format!("Text {text:?}"), None)
                        }
                        JsxChild::Expr(e) => self.expr(e, depth + 1),
                    }
                }
            }
        }
    }

    fn match_arm(&mut self, arm: &MatchArm, depth: usize) {
        self.line(depth, &format!("Arm {}", fmt_pattern(&arm.pattern)), None);
        if let Some(guard) = &arm.guard {
            self.line(depth + 1, "Guard", None);
            self.expr(guard, depth + 2);
        }
        self.expr(&arm.body, depth + 1);
    }
}

/// `@line:col-line:col`, collapsed to `@line:col` when the (exclusive-end)
/// span covers at most one character on a single line.
fn fmt_span(span: &SourceSpan) -> String {
    let (s, e) = (&span.start, &span.end);
    if s.line == e.line && e.column <= s.column + 1 {
        format!("@{}:{}", s.line, s.column)
    } else {
        format!("@{}:{}-{}:{}", s.line, s.column, e.line, e.column)
    }
}

fn fmt_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool(b) => b.to_string(),
        Literal::Int(i) => i.to_string(),
        Literal::Float(f) => format!("{f:?}"),
        Literal::String(s) => format!("{s:?}"),
    }
}

/// `(x: number, y)` — a parameter list, source-like.
fn fmt_params(params: &[Param]) -> String {
    let rendered: Vec<String> = params
        .iter()
        .map(|p| format!("{}{}", p.name, fmt_ty_suffix(&p.ty)))
        .collect();
    format!("({})", rendered.join(", "))
}

/// `: name` for a present annotation, empty otherwise.
fn fmt_ty_suffix(ty: &Option<TypeAnn>) -> String {
    match ty {
        Some(t) => format!(": {}", t.name),
        None => String::new(),
    }
}

/// A pattern, source-like: `_`, `0`, `n`, `Circle(r)`, `[a, b, ...rest]`,
/// `{x: 0, y}`.
fn fmt_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Literal(lit) => fmt_literal(lit),
        Pattern::Variable(name) => name.clone(),
        Pattern::Variant { name, fields } => {
            if fields.is_empty() {
                name.clone()
            } else {
                let inner: Vec<String> = fields.iter().map(fmt_pattern).collect();
                format!("{}({})", name, inner.join(", "))
            }
        }
        Pattern::List { elements, rest } => {
            let mut parts: Vec<String> = elements.iter().map(fmt_pattern).collect();
            if let Some(rest) = rest {
                parts.push(format!("...{rest}"));
            }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record(fields) => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(name, p)| format!("{}: {}", name, fmt_pattern(p)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

fn fmt_import(decl: &ImportDecl) -> String {
    let mut head = format!("Import {}", decl.module);
    if let Some(alias) = &decl.alias {
        let _ = write!(head, " as {alias}");
    }
    if let Some(names) = &decl.names {
        let _ = write!(head, ": {}", names.join(", "));
    }
    head
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map::ENTRY_FILE;

    fn render(source: &str) -> String {
        let (_tree, stmts) =
            crate::cst::parse_source(source, ENTRY_FILE).expect("test source should parse");
        display_stmts(&stmts)
    }

    #[test]
    fn function_decl_with_binary_op() {
        let src = "fn square(x: number) -> number\n  x * x\nend";
        let expected = "\
FnDecl square (x: number) -> number @1:1-3:4
  Expr @2:3-2:8
    BinaryOp Mul @2:3-2:8
      Ident x @2:3
      Ident x @2:7
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn let_with_literals() {
        let src = "let x = 1 + 2.5";
        let expected = "\
Let x @1:1-1:16
  BinaryOp Add @1:9-1:16
    Literal 1 @1:9
    Literal 2.5 @1:13-1:16
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn var_and_typed_let_show_modifiers_and_annotation() {
        let src = "var count = 0\nlet x: int = 1";
        let expected = "\
Let var count @1:1-1:14
  Literal 0 @1:13
Let x: int @2:1-2:15
  Literal 1 @2:14
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn match_with_guard_and_patterns() {
        let src = "let m = match x\n  when 0 -> \"zero\"\n  when n if n > 1 -> \"big\"\n  when _ -> \"other\"\nend";
        let expected = "\
Let m @1:1-5:4
  Match @1:9-5:4
    Ident x @1:15
    Arm 0
      Literal \"zero\" @2:13-2:19
    Arm n
      Guard
        BinaryOp Gt @3:13-3:18
          Ident n @3:13
          Literal 1 @3:17
      Literal \"big\" @3:22-3:27
    Arm _
      Literal \"other\" @4:13-4:20
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn class_decl_and_method() {
        let src = "class Rect\n  w: int,\n  h,\nend\n\nfn Rect.area(self)\n  self.w * self.h\nend";
        let expected = "\
ClassDecl Rect @1:1-4:4
  Field w: int @2:3-2:9
  Field h @3:3
FnDecl Rect.area (self) @6:1-8:4
  Expr @7:3-7:18
    BinaryOp Mul @7:3-7:18
      FieldAccess .w @7:3-7:9
        Ident self @7:3-7:7
      FieldAccess .h @7:12-7:18
        Ident self @7:12-7:16
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn string_interpolation() {
        let src = "let s = \"hi {name}!\"";
        let expected = "\
Let s @1:1-1:21
  StringInterp @1:9-1:21
    Part \"hi \"
    Ident name @1:14-1:18
    Part \"!\"
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn element_with_props_and_children() {
        let src = "let el = <box pad={2}>\n  <text>hello {name}</text>\n</box>";
        let expected = "\
Let el @1:1-3:7
  Element box @1:10-3:7
    Prop pad
      Literal 2 @1:20
    Element text @2:3-2:28
      Text \"hello \"
      Ident name @2:16-2:20
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn if_else_and_record_spread() {
        let src = "let r = if ok then {x: 1, ...base} else nil end";
        let expected = "\
Let r @1:1-1:48
  If @1:9-1:48
    Ident ok @1:12-1:14
    Then
      Expr @1:20-1:35
        Record @1:20-1:35
          Field x
            Literal 1 @1:24
          Spread
            Ident base @1:30-1:34
    Else
      Expr @1:41-1:44
        Literal nil @1:41-1:44
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn state_with_explicit_key() {
        let src = "state(id) hits = 0";
        let expected = "\
State hits @1:1-1:19
  Key
    Ident id @1:7-1:9
  Literal 0 @1:18
";
        assert_eq!(render(src), expected);
    }

    #[test]
    fn control_flow_statements() {
        let src = "for x in xs do\n  if x > 3 then break end\nend\nwhile go do\n  continue\nend";
        let expected = "\
For x @1:1-3:4
  Ident xs @1:10-1:12
  Expr @2:3-2:26
    If @2:3-2:26
      BinaryOp Gt @2:6-2:11
        Ident x @2:6
        Literal 3 @2:10
      Then
        Break @2:17-2:22
While @4:1-6:4
  Ident go @4:7-4:9
  Continue @5:3-5:11
";
        assert_eq!(render(src), expected);
    }
}
