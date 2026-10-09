//! Function introspection — what `fn_info(f)` and `fn_ast(f)` read (experimental).
//!
//! A compiled program keeps IR, not syntax: a [`FunctionDef`] has parameter
//! *names* and nothing about annotations, defaults or the shape of the body.
//! The program does keep its source text, and each `FunctionDef` records where
//! its function is written ([`FunctionDef::span`]). So the AST of a function is
//! recovered on demand: parse the file it lives in, find the `fn` at that span.
//! A function nobody inspects costs nothing.
//!
//! [`Cache`] (one per [`Program`]) holds the parsed files and the per-function
//! metadata worked out so far. It is dropped whenever the program's source
//! text is changed in place (`Env::apply_program_change`), because a patched
//! literal changes the AST and a moved line changes its positions.
//!
//! The AST handed to Petal code is not serde's encoding of [`crate::ast`]
//! (`{"kind": {"Call": {...}}, "span": [...]}`) but a flat one that is
//! pleasant to walk from Petal: every node is a record with a `tag`, its own
//! fields, and `line` / `column`. Every field of a node is always present
//! (`nil` when it does not apply), since reading a missing field is an error
//! in Petal. The shapes are listed in docs/function-introspection.md.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value as J, json};

use crate::ast::{
    AssignTarget, ElseBranch, Expr, ExprKind, ExprVisitor, JsxChild, Literal, Param, RecordField,
    Stmt, StmtKind, TypeAnn, walk_expr, walk_stmt,
};
use crate::program::{FunctionId, Program};
use crate::source_map::{FileId, SourceSpan};

/// One parameter, as written.
pub struct ParamMeta {
    pub name: String,
    /// The annotation's name exactly as written (`vec3`, `Fx`), if any.
    pub ty: Option<String>,
    /// The source text of the default expression, if the parameter has one.
    pub default_source: Option<String>,
    /// The default's value when it is a static expression: a literal, a
    /// negated number, a color literal, a list or record of those, or a
    /// `vec2(...)` / `vec3(...)` of numbers. In the JSON form
    /// [`crate::value::json_to_value`] reads.
    pub default: Option<J>,
}

/// Everything about one function that comes from its source text.
pub struct FnMeta {
    /// The declared name, `None` for a lambda.
    pub name: Option<String>,
    /// Display name of the file the function is written in, when it has one.
    pub file: Option<String>,
    pub line: u32,
    pub column: u32,
    pub params: Vec<ParamMeta>,
    /// The declared return type's name (`fn f() -> float`), if any.
    pub returns: Option<String>,
    /// Hash of the function's own code: parameters, annotations, defaults and
    /// body, without positions. Comments and layout do not reach it.
    pub code_hash: u64,
    /// The function's AST in the flat encoding, positions included.
    pub ast: J,
}

#[derive(Default)]
struct Inner {
    /// Parsed statements per file index; `None` once a file failed to parse.
    files: HashMap<u16, Option<Arc<Vec<Stmt>>>>,
    metas: HashMap<u32, Option<Arc<FnMeta>>>,
}

/// Per-program introspection cache. Interior mutability because the VM holds
/// the program by shared reference.
#[derive(Default)]
pub struct Cache(Mutex<Inner>);

impl Cache {
    /// Forget everything: the program's source text changed.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.0.lock() {
            *inner = Inner::default();
        }
    }

    /// The source-level metadata of function `id`, or `None` when the program
    /// has no source for it (a synthesized constructor, a program loaded from
    /// IR, a file that no longer parses).
    pub fn meta(&self, program: &Program, id: FunctionId) -> Option<Arc<FnMeta>> {
        let mut inner = self.0.lock().ok()?;
        if let Some(found) = inner.metas.get(&id.0) {
            return found.clone();
        }
        let meta = build_meta(&mut inner, program, id);
        inner.metas.insert(id.0, meta.clone());
        meta
    }
}

fn file_text(program: &Program, file: FileId) -> Option<(&str, &str)> {
    if let Some(f) = program.source_map.files.get(file.0 as usize) {
        return Some((&f.source, &f.name));
    }
    (file.0 == 0 && !program.source.is_empty()).then_some((program.source.as_str(), ""))
}

fn build_meta(inner: &mut Inner, program: &Program, id: FunctionId) -> Option<Arc<FnMeta>> {
    let span = program.functions.get(id.0 as usize)?.span?;
    let (source, file_name) = file_text(program, span.file)?;
    let stmts = inner
        .files
        .entry(span.file.0)
        .or_insert_with(|| {
            crate::cst::parse_source(source, span.file)
                .ok()
                .map(|(_tree, stmts)| Arc::new(stmts))
        })
        .clone()?;
    let mut finder = Finder {
        span,
        source,
        file: file_name,
        found: None,
    };
    for s in stmts.iter() {
        finder.visit_stmt(s);
        if finder.found.is_some() {
            break;
        }
    }
    finder.found.map(Arc::new)
}

/// Walks a file for the `fn` written at exactly `span`.
struct Finder<'a> {
    span: SourceSpan,
    source: &'a str,
    file: &'a str,
    found: Option<FnMeta>,
}

fn same_place(a: SourceSpan, b: SourceSpan) -> bool {
    a.start.offset == b.start.offset && a.end.offset == b.end.offset
}

impl ExprVisitor for Finder<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        if self.found.is_some() {
            return;
        }
        if let ExprKind::Lambda { params, body } = &e.kind
            && same_place(e.span, self.span)
        {
            self.found = Some(meta_of(None, None, params, None, body, e.span, self));
            return;
        }
        walk_expr(self, e);
    }

    fn visit_stmt(&mut self, s: &Stmt) {
        if self.found.is_some() {
            return;
        }
        if let StmtKind::FnDecl {
            name,
            class,
            params,
            ret,
            body,
        } = &s.kind
            && same_place(s.span, self.span)
        {
            self.found = Some(meta_of(
                Some(name),
                class.as_deref(),
                params,
                ret.as_ref(),
                body,
                s.span,
                self,
            ));
            return;
        }
        walk_stmt(self, s);
    }
}

fn meta_of(
    name: Option<&String>,
    class: Option<&str>,
    params: &[Param],
    ret: Option<&TypeAnn>,
    body: &[Stmt],
    span: SourceSpan,
    at: &Finder,
) -> FnMeta {
    let mut ast = Map::new();
    ast.insert("tag".into(), json!(if name.is_some() { "FnDecl" } else { "Lambda" }));
    ast.insert("name".into(), json!(name));
    ast.insert("class".into(), json!(class));
    ast.insert("params".into(), params_json(params));
    ast.insert("returns".into(), ty_json(ret));
    ast.insert("body".into(), stmts_json(body));
    ast.insert("file".into(), json!((!at.file.is_empty()).then_some(at.file)));
    put_pos(&mut ast, span);
    let ast = J::Object(ast);
    let mut hash = Fnv::new();
    hash_json(&ast, &mut hash);
    FnMeta {
        name: name.cloned(),
        file: (!at.file.is_empty()).then(|| at.file.to_string()),
        line: span.start.line,
        column: span.start.column,
        params: params
            .iter()
            .map(|p| ParamMeta {
                name: p.name.clone(),
                ty: p.ty.as_ref().map(|t| t.name.clone()),
                default_source: p.default.as_ref().and_then(|d| {
                    at.source
                        .get(d.span.start.offset as usize..d.span.end.offset as usize)
                        .map(str::to_string)
                }),
                default: p.default.as_ref().and_then(static_json),
            })
            .collect(),
        returns: ret.map(|t| t.name.clone()),
        code_hash: hash.0,
        ast,
    }
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// FNV-1a, 64-bit. Chosen because it is trivially the same on every platform
/// and in every build: a `code_hash` is compared across reloads and may be
/// stored by a host.
pub struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
    }
}

/// Hash a flat AST, leaving out where things are written (`line`, `column`,
/// `file`). Object keys are visited in the map's own (sorted) order.
fn hash_json(v: &J, h: &mut Fnv) {
    match v {
        J::Null => h.write(b"n"),
        J::Bool(b) => h.write(if *b { b"t" } else { b"f" }),
        J::Number(n) => {
            h.write(b"#");
            h.write(n.to_string().as_bytes());
            h.write(b";");
        }
        J::String(s) => {
            h.write(b"\"");
            h.write(s.as_bytes());
            h.write(b"\"");
        }
        J::Array(items) => {
            h.write(b"[");
            for item in items {
                hash_json(item, h);
                h.write(b",");
            }
            h.write(b"]");
        }
        J::Object(fields) => {
            h.write(b"{");
            for (k, item) in fields {
                if k == "line" || k == "column" || k == "file" {
                    continue;
                }
                h.write(k.as_bytes());
                h.write(b":");
                hash_json(item, h);
                h.write(b",");
            }
            h.write(b"}");
        }
    }
}

// ---------------------------------------------------------------------------
// Static defaults
// ---------------------------------------------------------------------------

fn static_number(e: &Expr) -> Option<f64> {
    match &e.kind {
        ExprKind::Literal(Literal::Int(n)) => Some(*n as f64),
        ExprKind::Literal(Literal::Float(f)) => Some(*f),
        ExprKind::UnaryOp {
            op: crate::ast::UnaryOp::Neg,
            operand,
        } => static_number(operand).map(|n| -n),
        _ => None,
    }
}

/// The value of a default expression that needs no evaluation, as JSON.
fn static_json(e: &Expr) -> Option<J> {
    match &e.kind {
        ExprKind::Literal(Literal::Nil) => Some(J::Null),
        ExprKind::Literal(Literal::Bool(b)) => Some(json!(b)),
        ExprKind::Literal(Literal::Int(n)) => Some(json!(n)),
        ExprKind::Literal(Literal::Float(f)) => Some(json!(f)),
        ExprKind::Literal(Literal::String(s)) => Some(json!(s)),
        ExprKind::UnaryOp {
            op: crate::ast::UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Literal(Literal::Int(n)) => Some(json!(-n)),
            ExprKind::Literal(Literal::Float(f)) => Some(json!(-f)),
            _ => None,
        },
        ExprKind::List(items) => items.iter().map(static_json).collect::<Option<Vec<J>>>().map(J::Array),
        ExprKind::Record(fields) => {
            let mut out = Map::new();
            for f in fields {
                let RecordField::Named(name, value) = f else {
                    return None;
                };
                out.insert(name.clone(), static_json(value)?);
            }
            Some(J::Object(out))
        }
        ExprKind::Call { function, args, arg_names } if arg_names.is_empty() => {
            let ExprKind::Ident(name) = &function.kind else {
                return None;
            };
            let nums = args.iter().map(static_number).collect::<Option<Vec<f64>>>()?;
            // The tagged shapes `json_to_value` turns back into vectors.
            match (name.as_str(), nums.as_slice()) {
                ("vec2", [x, y]) => Some(json!({"type": "vec2", "x": x, "y": y})),
                ("vec3", [x, y, z]) => Some(json!({"type": "vec3", "x": x, "y": y, "z": z})),
                _ => None,
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// AST -> flat JSON
// ---------------------------------------------------------------------------

fn put_pos(obj: &mut Map<String, J>, span: SourceSpan) {
    obj.insert("line".into(), json!(span.start.line));
    obj.insert("column".into(), json!(span.start.column));
}

fn node(tag: &str, span: SourceSpan, fields: J) -> J {
    let mut obj = match fields {
        J::Object(m) => m,
        _ => Map::new(),
    };
    obj.insert("tag".into(), json!(tag));
    put_pos(&mut obj, span);
    J::Object(obj)
}

fn ty_json(ty: Option<&TypeAnn>) -> J {
    json!(ty.map(|t| t.name.as_str()))
}

fn params_json(params: &[Param]) -> J {
    J::Array(
        params
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "type": ty_json(p.ty.as_ref()),
                    "default": p.default.as_ref().map(expr_json),
                })
            })
            .collect(),
    )
}

fn stmts_json(stmts: &[Stmt]) -> J {
    J::Array(stmts.iter().map(stmt_json).collect())
}

fn exprs_json(exprs: &[Expr]) -> J {
    J::Array(exprs.iter().map(expr_json).collect())
}

fn target_json(t: &AssignTarget) -> J {
    match t {
        AssignTarget::Name(n) => json!({"kind": "Name", "name": n, "object": null, "field": null, "index": null}),
        AssignTarget::Field(obj, field) => {
            json!({"kind": "Field", "name": null, "object": expr_json(obj), "field": field, "index": null})
        }
        AssignTarget::Index(obj, index) => {
            json!({"kind": "Index", "name": null, "object": expr_json(obj), "field": null, "index": expr_json(index)})
        }
    }
}

fn stmt_json(s: &Stmt) -> J {
    let fields = match &s.kind {
        StmtKind::Let {
            name,
            ty,
            value,
            is_var,
            is_config,
        } => {
            return node(
                "Let",
                s.span,
                json!({"name": name, "type": ty_json(ty.as_ref()), "value": expr_json(value),
                       "is_var": is_var, "is_config": is_config}),
            );
        }
        StmtKind::Assign { target, value } => {
            return node("Assign", s.span, json!({"target": target_json(target), "value": expr_json(value)}));
        }
        StmtKind::Set { target, value } => {
            return node("Set", s.span, json!({"target": target_json(target), "value": expr_json(value)}));
        }
        StmtKind::Expr(e) => return node("Expr", s.span, json!({"expr": expr_json(e)})),
        StmtKind::FnDecl {
            name,
            class,
            params,
            ret,
            body,
        } => json!({"name": name, "class": class, "params": params_json(params),
                    "returns": ty_json(ret.as_ref()), "body": stmts_json(body)}),
        StmtKind::EnumDecl { name, variants } => {
            return node(
                "EnumDecl",
                s.span,
                json!({"name": name, "variants": variants.iter()
                    .map(|v| json!({"name": v.name, "fields": v.fields})).collect::<Vec<J>>()}),
            );
        }
        StmtKind::ClassDecl { name, fields } => {
            return node(
                "ClassDecl",
                s.span,
                json!({"name": name, "fields": fields.iter()
                    .map(|f| json!({"name": f.name, "type": ty_json(f.ty.as_ref())})).collect::<Vec<J>>()}),
            );
        }
        StmtKind::For { var, iter, body } => {
            return node(
                "For",
                s.span,
                json!({"var": var, "iter": expr_json(iter), "body": stmts_json(body)}),
            );
        }
        StmtKind::While { condition, body } => {
            return node(
                "While",
                s.span,
                json!({"condition": expr_json(condition), "body": stmts_json(body)}),
            );
        }
        StmtKind::Return(value) => {
            return node("Return", s.span, json!({"value": value.as_ref().map(expr_json)}));
        }
        StmtKind::Break => return node("Break", s.span, J::Null),
        StmtKind::Continue => return node("Continue", s.span, J::Null),
        StmtKind::State {
            name,
            ty,
            init,
            key,
            is_var,
        } => {
            return node(
                "State",
                s.span,
                json!({"name": name, "type": ty_json(ty.as_ref()), "init": expr_json(init),
                       "key": key.as_ref().map(expr_json), "is_var": is_var}),
            );
        }
        StmtKind::Import(decl) => {
            return node(
                "Import",
                s.span,
                json!({"module": decl.module, "alias": decl.alias, "names": decl.names, "star": decl.star}),
            );
        }
    };
    node("FnDecl", s.span, fields)
}

fn literal_json(lit: &Literal) -> J {
    match lit {
        Literal::Nil => json!({"type": "nil", "value": null}),
        Literal::Bool(b) => json!({"type": "bool", "value": b}),
        Literal::Int(n) => json!({"type": "int", "value": n}),
        Literal::Float(f) => json!({"type": "float", "value": f}),
        Literal::String(s) => json!({"type": "string", "value": s}),
    }
}

/// An `else` branch as a statement list. An `elsif` is the nested `if` it
/// means, as the single statement of the block.
fn else_json(e: &ElseBranch) -> J {
    match e {
        ElseBranch::Block(stmts) => stmts_json(stmts),
        ElseBranch::ElseIf(inner) => json!([node("Expr", inner.span, json!({"expr": expr_json(inner)}))]),
    }
}

fn expr_json(e: &Expr) -> J {
    let (tag, fields) = match &e.kind {
        ExprKind::Literal(lit) => ("Literal", literal_json(lit)),
        ExprKind::Ident(name) => ("Ident", json!({"name": name})),
        ExprKind::AtVar(name) => ("AtVar", json!({"name": name})),
        ExprKind::CellGet(name) => ("CellGet", json!({"name": name})),
        ExprKind::BinaryOp { op, left, right } => (
            "BinaryOp",
            json!({"op": format!("{op:?}"), "left": expr_json(left), "right": expr_json(right)}),
        ),
        ExprKind::UnaryOp { op, operand } => (
            "UnaryOp",
            json!({"op": format!("{op:?}"), "operand": expr_json(operand)}),
        ),
        ExprKind::Call {
            function,
            args,
            arg_names,
        } => (
            "Call",
            json!({"function": expr_json(function), "args": exprs_json(args), "arg_names": arg_names}),
        ),
        ExprKind::If {
            condition,
            then_body,
            else_body,
        } => (
            "If",
            json!({"condition": expr_json(condition), "then_body": stmts_json(then_body),
                   "else_body": else_body.as_ref().map(else_json)}),
        ),
        ExprKind::Match { subject, arms } => (
            "Match",
            json!({"subject": expr_json(subject), "arms": arms.iter().map(|arm| json!({
                "pattern": serde_json::to_value(&arm.pattern).unwrap_or(J::Null),
                "guard": arm.guard.as_ref().map(expr_json),
                "body": expr_json(&arm.body),
            })).collect::<Vec<J>>()}),
        ),
        ExprKind::For { var, iter, body } => (
            "For",
            json!({"var": var, "iter": expr_json(iter), "body": stmts_json(body)}),
        ),
        ExprKind::List(items) => ("List", json!({"items": exprs_json(items)})),
        ExprKind::Record(fields) => (
            "Record",
            json!({"fields": fields.iter().map(|f| match f {
                RecordField::Named(name, value) => json!({"name": name, "value": expr_json(value), "spread": false}),
                RecordField::Spread(value) => json!({"name": null, "value": expr_json(value), "spread": true}),
            }).collect::<Vec<J>>()}),
        ),
        ExprKind::FieldAccess { object, field } => {
            ("FieldAccess", json!({"object": expr_json(object), "field": field}))
        }
        ExprKind::IndexAccess { object, index } => (
            "IndexAccess",
            json!({"object": expr_json(object), "index": expr_json(index)}),
        ),
        ExprKind::OptionalAccess(inner) => ("OptionalAccess", json!({"value": expr_json(inner)})),
        ExprKind::Block(stmts) => ("Block", json!({"body": stmts_json(stmts)})),
        ExprKind::Lambda { params, body } => (
            "Lambda",
            json!({"params": params_json(params), "body": stmts_json(body)}),
        ),
        ExprKind::StringInterp { parts, exprs } => {
            ("StringInterp", json!({"parts": parts, "exprs": exprs_json(exprs)}))
        }
        ExprKind::Element {
            tag,
            props,
            children,
        } => (
            "Element",
            json!({"element": tag,
                   "props": props.iter().map(|(name, value)| json!({"name": name, "value": expr_json(value)})).collect::<Vec<J>>(),
                   "children": children.iter().map(|c| match c {
                       JsxChild::Text(text) => json!({"text": text, "expr": null}),
                       JsxChild::Expr(inner) => json!({"text": null, "expr": expr_json(inner)}),
                   }).collect::<Vec<J>>()}),
        ),
    };
    node(tag, e.span, fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_of(source: &str) -> u64 {
        let (_tree, stmts) = crate::cst::parse_source(source, FileId(0)).unwrap();
        let mut h = Fnv::new();
        hash_json(&stmts_json(&stmts), &mut h);
        h.0
    }

    #[test]
    fn the_hash_ignores_layout_and_comments_and_sees_code() {
        let a = hash_of("let f = fn(x: float, k = 2.0)\n  x * k\nend");
        let b = hash_of("// a comment\n\nlet f = fn(x: float,   k = 2.0)\n\n    x * k // why\nend");
        let c = hash_of("let f = fn(x: float, k = 2.0)\n  x * k + 1.0\nend");
        let d = hash_of("let f = fn(x: float, k = 2.5)\n  x * k\nend");
        let e = hash_of("let f = fn(x: int, k = 2.0)\n  x * k\nend");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(a, e);
    }

    #[test]
    fn static_defaults_cover_literals_colors_and_vectors() {
        let (_tree, stmts) =
            crate::cst::parse_source("let f = fn(a = -1.5, c = #102030, v = vec2(1, 2.5), d = a * 2) -> a", FileId(0))
                .unwrap();
        let StmtKind::Let { value, .. } = &stmts[0].kind else { panic!() };
        let ExprKind::Lambda { params, .. } = &value.kind else { panic!() };
        let got: Vec<Option<J>> = params.iter().map(|p| p.default.as_ref().and_then(static_json)).collect();
        assert_eq!(got[0], Some(json!(-1.5)));
        assert_eq!(got[1], Some(json!({"r": 16, "g": 32, "b": 48})));
        assert_eq!(got[2], Some(json!({"type": "vec2", "x": 1.0, "y": 2.5})));
        assert_eq!(got[3], None);
    }
}
