//! Expressions and calls.

use std::collections::HashSet;

use petal::ast::{BinOp, ElseBranch, Expr, ExprKind, JsxChild, Literal, Param, RecordField, Stmt, StmtKind, UnaryOp};
use petal::source_map::SourceSpan;

use crate::analysis::FnKind;
use crate::dialect::Dialect;
use crate::emit::{Emitter, Kind, Tail, W, indent_lines, literal, paren_multiline, prop_name};
use crate::names::{self, js_key, js_string, template_text};

// JS precedence, higher binds tighter.
pub const P_PRIMARY: u8 = 20;
const P_UNARY: u8 = 16;
const P_MUL: u8 = 14;
const P_ADD: u8 = 13;
const P_REL: u8 = 11;
const P_EQ: u8 = 10;
const P_AND: u8 = 6;
const P_OR: u8 = 5;
const P_COALESCE: u8 = 4;
const P_TERNARY: u8 = 3;
const P_ARROW: u8 = 2;

/// How a call's arguments are given.
pub struct Args<'e> {
    pub positional: Vec<&'e Expr>,
    pub named: Vec<(String, &'e Expr)>,
    pub spread: Option<&'e Expr>,
}

pub fn split_args<'e>(args: &'e [Expr], names: &[Option<String>]) -> Args<'e> {
    let mut a = Args { positional: vec![], named: vec![], spread: None };
    for (i, e) in args.iter().enumerate() {
        match names.get(i).and_then(|n| n.as_deref()) {
            Some(petal::ast::SPREAD_ARG) => a.spread = Some(e),
            Some(n) => a.named.push((n.to_string(), e)),
            None => a.positional.push(e),
        }
    }
    a
}

impl<'a> Emitter<'a> {
    pub fn expr(&mut self, e: &Expr) -> String {
        self.ex(e, 0)
    }

    pub fn ex(&mut self, e: &Expr, min: u8) -> String {
        let (s, p) = self.expr_p(e);
        if p < min { format!("({s})") } else { s }
    }

    fn expr_p(&mut self, e: &Expr) -> (String, u8) {
        if let Some(r) = self.replacements.get(&(e.span.start.offset, e.span.end.offset)) {
            return (r.clone(), P_PRIMARY);
        }
        match &e.kind {
            ExprKind::Literal(l) => {
                let s = literal(l);
                let p = if s.starts_with('-') { P_UNARY } else { P_PRIMARY };
                (s, p)
            }
            ExprKind::Ident(n) => (self.ident(n, e.span), P_PRIMARY),
            ExprKind::CellGet(n) => (self.cell_get(n, e.span), P_PRIMARY),
            ExprKind::BinaryOp { op, left, right } => self.binary(*op, left, right),
            ExprKind::UnaryOp { op, operand } => {
                let o = self.ex(operand, P_UNARY);
                (match op {
                    UnaryOp::Neg => format!("-{o}"),
                    UnaryOp::Not => format!("!{o}"),
                }, P_UNARY)
            }
            ExprKind::Call { function, args, arg_names } => self.call(function, args, arg_names, e.span),
            ExprKind::If { condition, then_body, else_body } => self.if_expr(condition, then_body, else_body.as_ref()),
            ExprKind::Match { .. } => (self.iife(e), P_PRIMARY),
            ExprKind::For { var, iter, key, body, flatten } => {
                (self.for_expr(var, iter, key.as_deref(), body, *flatten), P_PRIMARY)
            }
            ExprKind::List(xs) => (self.list(xs, None), P_PRIMARY),
            ExprKind::Record(fields) => (self.record(fields, e.span), P_PRIMARY),
            ExprKind::FieldAccess { object, field } => (self.field(object, field, false), P_PRIMARY),
            ExprKind::IndexAccess { object, index } => (self.index(object, index, false), P_PRIMARY),
            ExprKind::OptionalAccess(inner) => (self.optional(inner), P_PRIMARY),
            ExprKind::Lambda { params, body } => (self.lambda(params, body, None), P_ARROW),
            ExprKind::StringInterp { parts, exprs } => {
                let mut out = String::from("`");
                for (i, p) in parts.iter().enumerate() {
                    out.push_str(&template_text(p));
                    if let Some(x) = exprs.get(i) {
                        let v = self.expr(x);
                        out.push_str(&format!("${{{v}}}"));
                    }
                }
                out.push('`');
                (out, P_PRIMARY)
            }
            ExprKind::Element { tag, props, children } => (self.element(tag, props, children, e.span, None), P_PRIMARY),
        }
    }

    /// A list literal. Elements in it get a `key`, as React wants: their
    /// `id`, else their position (prefixed by the enclosing iteration's key
    /// when the list is one iteration's part of a flattened list).
    pub fn list(&mut self, xs: &[Expr], prefix: Option<&str>) -> String {
        // A list with an element in it is a list of children: every item
        // that may be one (a name, a call, a branch) is keyed too.
        let children = xs.iter().any(|x| self.builds_element(x));
        let items: Vec<String> = xs
            .iter()
            .enumerate()
            .map(|(i, x)| {
                if !self.builds_element(x) {
                    let maybe = matches!(x.kind, ExprKind::Ident(_) | ExprKind::Call { .. } | ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::FieldAccess { .. } | ExprKind::IndexAccess { .. });
                    if children && maybe {
                        let key = match prefix {
                            None => i.to_string(),
                            Some(p) => format!("`${{{p}}}-{i}`"),
                        };
                        let v = self.expr(x);
                        let f = self.react("Fragment");
                        return format!("<{f} {}>{{{v}}}</{f}>", key_attr(&key));
                    }
                    return self.expr(x);
                }
                let id = match &x.kind {
                    ExprKind::Element { props, .. } => props.iter().find(|(k, _)| k == "id").map(|(_, v)| self.expr(v)),
                    _ => None,
                };
                let key = match (id, prefix) {
                    (Some(id), None) => id,
                    (Some(id), Some(p)) => format!("`${{{p}}}-${{{id}}}`"),
                    (None, None) => i.to_string(),
                    (None, Some(p)) => format!("`${{{p}}}-{i}`"),
                };
                self.keyed(x, Some(&key))
            })
            .collect();
        list_text(&items)
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr) -> (String, u8) {
        let (sym, p) = match op {
            BinOp::Add => ("+", P_ADD),
            BinOp::Sub => ("-", P_ADD),
            BinOp::Mul => ("*", P_MUL),
            BinOp::Div => ("/", P_MUL),
            BinOp::Mod => ("%", P_MUL),
            BinOp::Lt => ("<", P_REL),
            BinOp::Le => ("<=", P_REL),
            BinOp::Gt => (">", P_REL),
            BinOp::Ge => (">=", P_REL),
            BinOp::And => ("&&", P_AND),
            BinOp::Or => ("||", P_OR),
            BinOp::Coalesce => ("??", P_COALESCE),
            BinOp::Eq | BinOp::Ne => return self.equality(op == BinOp::Eq, l, r),
            BinOp::Concat => return self.concat(l, r),
        };
        if op == BinOp::Div {
            // Petal's `/` truncates between ints. Two int literals fold; a
            // float literal on either side is a float division.
            if let (ExprKind::Literal(Literal::Int(a)), ExprKind::Literal(Literal::Int(b))) = (&l.kind, &r.kind) {
                if *b != 0 {
                    return ((a / b).to_string(), P_PRIMARY);
                }
            }
        }
        // `??` does not mix with `&&`/`||` unparenthesized.
        let (lmin, rmin) = if op == BinOp::Coalesce { (P_AND + 1, P_AND + 1) } else { (p, p + 1) };
        let ls = self.ex(l, lmin);
        let rs = self.ex(r, rmin);
        let ls = if matches!(op, BinOp::And | BinOp::Or) && ls.ends_with("??") { format!("({ls})") } else { ls };
        (format!("{ls} {sym} {rs}"), p)
    }

    fn equality(&mut self, eq: bool, l: &Expr, r: &Expr) -> (String, u8) {
        let nil = |e: &Expr| matches!(e.kind, ExprKind::Literal(Literal::Nil));
        let scalar = |e: &Expr| {
            matches!(e.kind, ExprKind::Literal(_) | ExprKind::StringInterp { .. } | ExprKind::UnaryOp { .. })
                || matches!(&e.kind, ExprKind::Call { function, .. } if matches!(&function.kind, ExprKind::Ident(n) if n == "len" || n == "str" || n == "type"))
        };
        if nil(l) || nil(r) {
            let other = if nil(l) { r } else { l };
            let o = self.ex(other, P_EQ + 1);
            return (format!("{o} {} null", if eq { "==" } else { "!=" }), P_EQ);
        }
        if scalar(l) || scalar(r) {
            let ls = self.ex(l, P_EQ);
            let rs = self.ex(r, P_EQ + 1);
            return (format!("{ls} {} {rs}", if eq { "===" } else { "!==" }), P_EQ);
        }
        let f = self.rt("eq");
        let ls = self.expr(l);
        let rs = self.expr(r);
        let call = format!("{f}({ls}, {rs})");
        if eq { (call, P_PRIMARY) } else { (format!("!{call}"), P_UNARY) }
    }

    /// The operands of a concatenation, each list of children keyed apart
    /// (`0-…`, `1-…`) so the joined list has no duplicate keys.
    fn concat_operands(&mut self, xs: &[&Expr], min: u8) -> Vec<String> {
        xs.iter()
            .enumerate()
            .map(|(i, x)| {
                if let ExprKind::List(items) = &x.kind {
                    return self.list(items, Some(&i.to_string()));
                }
                self.key_prefix = Some(i.to_string());
                let v = self.ex(x, min);
                self.key_prefix = None;
                v
            })
            .collect()
    }

    fn concat(&mut self, l: &Expr, r: &Expr) -> (String, u8) {
        let stringy = |e: &Expr| matches!(e.kind, ExprKind::Literal(Literal::String(_)) | ExprKind::StringInterp { .. });
        let listy = |e: &Expr| matches!(e.kind, ExprKind::List(_));
        if stringy(l) || stringy(r) {
            let ls = self.ex(l, P_ADD);
            let rs = self.ex(r, P_ADD + 1);
            return (format!("{ls} + {rs}"), P_ADD);
        }
        if listy(l) || listy(r) {
            let v = self.concat_operands(&[l, r], P_UNARY);
            return (format!("[...{}, ...{}]", v[0], v[1]), P_PRIMARY);
        }
        let f = self.rt("concat");
        let v = self.concat_operands(&[l, r], 0);
        (format!("{f}({}, {})", v[0], v[1]), P_PRIMARY)
    }

    // ── names ───────────────────────────────────────────────────────────────

    pub fn ident(&mut self, name: &str, span: SourceSpan) -> String {
        // Innermost first: a handler's copy, a local, a state value.
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.temps.get(name) {
                return t.clone();
            }
            if let Some(slot) = s.states.get(name) {
                return slot.value.clone();
            }
            if s.locals.contains(name) {
                return self.names.local(name);
            }
        }
        if self.cells.contains_key(name) {
            return self.cell_get(name, span);
        }
        if let Some(info) = self.fns.get(name).cloned() {
            return match info.kind {
                // A component used as a value: a function from arguments to
                // its element, so `map(xs, card)` still works.
                FnKind::Component if info.design_component.is_none() => {
                    let ps: Vec<String> = info.params.iter().map(|p| names::camel(p)).collect();
                    let attrs: Vec<String> = info.params.iter().zip(&ps).map(|(p, j)| format!("{}={{{j}}}", prop_name(p))).collect();
                    format!("({}) => <{} {} />", ps.join(", "), info.js, attrs.join(" "))
                }
                _ => info.js,
            };
        }
        if self.module_names.contains(name) {
            return self.names.local(name);
        }
        if let Some((path, js)) = self.imported.get(name).cloned() {
            self.module_imports.entry(path).or_default().insert(js.clone());
            return js;
        }
        if name == "pi" || self.runtime.contains_key(name) || self.dialect_rt.contains_key(name) {
            return self.rt(name);
        }
        match name {
            "PI" => return "Math.PI".into(),
            "ENV" => {}
            _ => {}
        }
        self.stub(name, span)
    }

    /// A name with no definition the transpiler can see.
    pub fn stub(&mut self, name: &str, span: SourceSpan) -> String {
        if let Some(js) = self.stubs.get(name) {
            return js.clone();
        }
        let js = self.names.fresh(&names::camel(name));
        self.stubs.insert(name.to_string(), js.clone());
        let hint = if !self.unknown_packages.is_empty() {
            format!(" (from `{}`?)", self.unknown_packages.join("`, `"))
        } else {
            String::new()
        };
        self.warn(span, "unknown-name", format!("`{name}` is not defined in this file or in the React runtime{hint}; it is a stub that throws when called"));
        js
    }

    fn cell_get(&mut self, name: &str, span: SourceSpan) -> String {
        if let Some(cell) = self.cells.get(name).cloned() {
            // In a component (and the callbacks written in it: a `.map`, a
            // tap): the value `useCell` subscribed to, as React reads state.
            // Not in a web click handler, nor in a callback that writes the
            // cell itself.
            if !self.in_handler() {
                for s in self.scopes.iter().rev() {
                    if s.cell_writes.contains(name) {
                        break;
                    }
                    if let Some(v) = s.cell_vals.get(name) {
                        return v.clone();
                    }
                }
            }
            return format!("{cell}.get()");
        }
        self.ident(name, span)
    }

    // ── records, fields ─────────────────────────────────────────────────────

    fn record(&mut self, fields: &[RecordField], span: SourceSpan) -> String {
        // A color literal (`:FF9500`) is parsed as its {r, g, b} record,
        // every field spanning the literal.
        if let Some(h) = color_literal(fields, span) {
            let f = self.rt("hex");
            return format!("{f}({})", js_string(&h));
        }
        if fields.is_empty() {
            return "{}".into();
        }
        let mut parts = Vec::new();
        for f in fields {
            match f {
                RecordField::Named(k, v) => {
                    let vs = self.expr(v);
                    let key = js_key(k);
                    if key == vs && names::is_js_ident(k) {
                        parts.push(key);
                    } else {
                        parts.push(format!("{key}: {vs}"));
                    }
                }
                RecordField::Spread(e) => {
                    // A computed key, `[k]: v`.
                    if let ExprKind::Call { function, args, .. } = &e.kind {
                        if matches!(&function.kind, ExprKind::Ident(n) if n == petal::ast::RECORD_OF_BUILTIN) && args.len() == 2 {
                            let k = self.expr(&args[0]);
                            let v = self.expr(&args[1]);
                            parts.push(format!("[{k}]: {v}"));
                            continue;
                        }
                    }
                    let v = self.ex(e, P_UNARY);
                    parts.push(format!("...{v}"));
                }
            }
        }
        let one = format!("{{ {} }}", parts.join(", "));
        if one.len() <= 90 && !one.contains('\n') {
            one
        } else {
            format!("{{\n{}}}", parts.iter().map(|p| format!("  {},\n", p.replace('\n', "\n  "))).collect::<String>())
        }
    }

    fn field(&mut self, object: &Expr, field: &str, optional: bool) -> String {
        // `module.name`: the name from that module.
        if let ExprKind::Ident(q) = &object.kind {
            if !self.is_local(q) && !self.module_names.contains(q) && self.aliases.contains_key(q) {
                return self.qualified(q, field, object.span);
            }
        }
        let o = self.access_base(object);
        let dot = if optional { "?." } else { "." };
        if names::is_js_ident(field) {
            format!("{o}{dot}{field}")
        } else {
            format!("{o}{}[{}]", if optional { "?." } else { "" }, js_string(field))
        }
    }

    /// `module.name`: the name as that module exports it. When this module
    /// binds the same name (`let TESS = parts.TESS`), the import is aliased.
    pub fn qualified(&mut self, q: &str, field: &str, span: SourceSpan) -> String {
        let module = self.aliases.get(q).cloned().unwrap_or_default();
        let Some(iface) = self.opts.modules.get(&module).cloned() else {
            return self.ident(field, span);
        };
        let js = iface.fns.get(field).map(|f| f.js.clone()).or_else(|| iface.values.get(field).cloned());
        let Some(js) = js else { return self.ident(field, span) };
        let clash = self.module_names.contains(field)
            || self.is_local(field)
            || self.names.get(field).is_some_and(|j| *j != js)
            || self.exported_by.get(field).is_some_and(|n| *n > 1);
        if !clash {
            self.module_imports.entry(iface.path.clone()).or_default().insert(js.clone());
            return js;
        }
        let alias = format!("{}{}", names::camel(q), pascal_keep(&js));
        self.module_imports.entry(iface.path.clone()).or_default().insert(format!("{js} as {alias}"));
        self.names.reserve(&alias);
        alias
    }

    fn index(&mut self, object: &Expr, index: &Expr, optional: bool) -> String {
        let o = self.access_base(object);
        let neg = matches!(index.kind, ExprKind::UnaryOp { op: UnaryOp::Neg, .. }) || matches!(index.kind, ExprKind::Literal(Literal::Int(i)) if i < 0);
        if neg || self.uses_maps {
            let at = self.rt("at");
            let i = self.expr(index);
            let o = self.expr(object);
            return format!("{at}({o}, {i})");
        }
        let i = self.expr(index);
        format!("{o}{}[{i}]", if optional { "?." } else { "" })
    }

    fn access_base(&mut self, object: &Expr) -> String {
        let (s, p) = self.expr_p(object);
        if p < P_PRIMARY || s.starts_with('<') { format!("({s})") } else { s }
    }

    /// `a?.b.c`: Petal's one `?.` tolerates absence along the whole chain.
    fn optional(&mut self, inner: &Expr) -> String {
        match &inner.kind {
            ExprKind::FieldAccess { object, field } => {
                let o = self.optional_base(object);
                let tmp = Expr { kind: ExprKind::Ident("\u{0}".into()), span: inner.span };
                let _ = tmp;
                if names::is_js_ident(field) { format!("{o}?.{field}") } else { format!("{o}?.[{}]", js_string(field)) }
            }
            ExprKind::IndexAccess { object, index } => {
                let o = self.optional_base(object);
                let i = self.expr(index);
                format!("{o}?.[{i}]")
            }
            _ => self.expr(inner),
        }
    }

    fn optional_base(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::FieldAccess { .. } | ExprKind::IndexAccess { .. } => self.optional(e),
            _ => self.access_base(e),
        }
    }

    // ── branches, loops, lambdas ────────────────────────────────────────────

    fn if_expr(&mut self, cond: &Expr, then_body: &[Stmt], else_body: Option<&ElseBranch>) -> (String, u8) {
        let simple = |b: &[Stmt]| b.len() == 1 && matches!(b[0].kind, StmtKind::Expr(_));
        let else_simple = match else_body {
            None => true,
            Some(ElseBranch::Block(b)) => simple(b),
            Some(ElseBranch::ElseIf(_)) => true,
        };
        if simple(then_body) && else_simple {
            let c = self.ex(cond, P_TERNARY + 1);
            self.scope().nested += 1;
            let StmtKind::Expr(t) = &then_body[0].kind else { unreachable!() };
            let t = self.ex(t, P_TERNARY + 1);
            let e = match else_body {
                None => "null".to_string(),
                Some(ElseBranch::Block(b)) => {
                    let StmtKind::Expr(x) = &b[0].kind else { unreachable!() };
                    self.ex(x, P_TERNARY)
                }
                Some(ElseBranch::ElseIf(x)) => self.ex(x, P_TERNARY),
            };
            self.scope().nested -= 1;
            let t = paren_multiline(&t);
            let e = paren_multiline(&e);
            return (format!("{c} ? {t} : {e}"), P_TERNARY);
        }
        let e = Expr {
            kind: ExprKind::If { condition: Box::new(cond.clone()), then_body: then_body.to_vec(), else_body: else_body.cloned() },
            span: cond.span,
        };
        (self.iife(&e), P_PRIMARY)
    }

    /// An expression that needs statements: `(() => { … })()`.
    fn iife(&mut self, e: &Expr) -> String {
        let mut w = W::default();
        let s = Stmt { kind: StmtKind::Expr(e.clone()), span: e.span, exported: false, test: None, doc: None };
        self.push_scope(Kind::Lambda);
        self.scope().mutable = crate::emit::mutable_names(std::slice::from_ref(&s));
        self.block(&mut w, std::slice::from_ref(&s), Tail::Return(None));
        self.pop_scope();
        format!("(() => {{\n{}}})()", indent_lines(&w.buf))
    }

    fn for_expr(&mut self, var: &str, iter: &Expr, key: Option<&Expr>, body: &[Stmt], flatten: bool) -> String {
        if crate::emit::breaks_or_skips(body) {
            // `continue` skips an element and `break` ends the list: a loop.
            self.push_scope(Kind::Lambda);
            let mut w = W::default();
            let out = self.collect_loop(&mut w, var, iter, body, flatten);
            w.line(format!("return {out};"));
            self.pop_scope();
            return format!("(() => {{\n{}}})()", indent_lines(&w.buf));
        }
        let it = self.ex(iter, P_PRIMARY);
        let method = if flatten { "flatMap" } else { "map" };
        let param = Param::plain(var.to_string());
        let f = self.lambda(std::slice::from_ref(&param), body, Some(KeyFrom::Expr(key)));
        format!("{it}.{method}({f})")
    }

    /// A lambda as a JS arrow function. `key` asks for a React `key` on the
    /// element it returns (a callback that builds list items).
    pub fn lambda(&mut self, params: &[Param], body: &[Stmt], key: Option<KeyFrom>) -> String {
        self.push_scope(Kind::Lambda);
        if !self.cells.is_empty() {
            self.scope().cell_writes = crate::analysis::own_cell_writes(body);
        }
        let mut ps = self.params(params);
        let mut index = None;
        if let Some(KeyFrom::Expr(None)) | Some(KeyFrom::Index) = key {
            let tail = crate::analysis::tail_expr(body);
            let lists_elements = tail.is_some_and(|t| matches!(&t.kind, ExprKind::List(xs) if xs.iter().any(|x| self.builds_element(x))));
            if tail.is_some_and(|t| self.builds_element(t)) || lists_elements {
                let i = self.names.fresh("i");
                ps.push(i.clone());
                index = Some(i);
            }
        }
        self.scope().mutable = crate::emit::mutable_names(body);
        let head = if ps.len() == 1 && !ps[0].contains('=') { ps[0].clone() } else { format!("({})", ps.join(", ")) };
        let head = if ps.len() == 1 { format!("({head})").replace("((", "(").replace("))", ")") } else { head };
        // The key: the loop's `key`, the element's `id`, else the index.
        let prefix = self.key_prefix.take();
        let key_js = match &key {
            Some(KeyFrom::Expr(Some(k))) => Some(self.expr(k)),
            Some(KeyFrom::Param) if crate::analysis::tail_expr(body).is_some_and(|t| self.builds_element(t)) => {
                params.first().map(|p| self.names.local(&p.name))
            }
            _ => index.clone(),
        };
        let key_js = match (key_js, prefix) {
            (Some(k), Some(p)) => Some(format!("`{p}-${{{k}}}`")),
            (k, _) => k,
        };
        // Writes to captured names rebind the lambda's copy, as in Petal.
        let mut w = W::default();
        let written = crate::analysis::assigned_names(body, false);
        let declared = crate::analysis::declared_names(body);
        let mut cap: Vec<String> = written.into_iter().filter(|n| !declared.contains(n) && !params.iter().any(|p| &p.name == n)).collect();
        cap.sort();
        for n in &cap {
            if self.cells.contains_key(n) {
                continue;
            }
            let t = self.names.fresh(&format!("{}Copy", names::camel(n)));
            let cur = self.ident(n, body[0].span);
            if self.state_slot(n).is_some() {
                self.warn(body[0].span, "write-in-callback", format!("a function changes `{n}`, which is state; in Petal that rebinds the function's own copy. To change the state, write it in a click handler (`if clicked(id) then {n} = …`)"));
            }
            w.line(format!("let {t} = {cur};"));
            self.scope().temps.insert(n.clone(), t);
        }
        let out = if cap.is_empty() && body.len() == 1 && matches!(body[0].kind, StmtKind::Expr(_)) {
            let StmtKind::Expr(e) = &body[0].kind else { unreachable!() };
            let v = self.keyed(e, key_js.as_deref());
            let v = if v.starts_with('{') { format!("({v})") } else { paren_multiline(&v) };
            format!("{head} => {v}")
        } else {
            let n = body.len();
            for (i, s) in body.iter().enumerate() {
                if i + 1 == n {
                    if let (StmtKind::Expr(e), Some(k)) = (&s.kind, key_js.as_deref()) {
                        let v = self.keyed(e, Some(k));
                        w.line(crate::emit::ret(None, &v));
                        continue;
                    }
                    self.body_stmt(&mut w, s, Tail::Return(None));
                } else {
                    self.body_stmt(&mut w, s, Tail::None);
                }
            }
            format!("{head} => {{\n{}}}", indent_lines(&w.buf))
        };
        self.pop_scope();
        out
    }

    /// Whether `e` builds an element (for list keys).
    pub fn builds_element(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Element { .. } => true,
            ExprKind::Call { function, .. } => match &function.kind {
                ExprKind::Ident(n) => {
                    self.scopes.iter().any(|s| s.elem_locals.contains(n))
                        || !self.is_local(n) && self.fns.get(n).or(self.imported_fns.get(n)).is_some_and(|f| f.kind == FnKind::Component)
                }
                _ => false,
            },
            ExprKind::If { then_body, else_body, .. } => {
                crate::analysis::tail_expr(then_body).is_some_and(|t| self.builds_element(t))
                    || match else_body {
                        Some(ElseBranch::Block(b)) => crate::analysis::tail_expr(b).is_some_and(|t| self.builds_element(t)),
                        Some(ElseBranch::ElseIf(x)) => self.builds_element(x),
                        None => false,
                    }
            }
            _ => false,
        }
    }

    /// An expression, with a React `key` when it builds an element.
    pub fn keyed(&mut self, e: &Expr, key: Option<&str>) -> String {
        let Some(k) = key else { return self.expr(e) };
        match &e.kind {
            ExprKind::List(xs) => self.list(xs, Some(k)),
            ExprKind::Element { tag, props, children } => self.element(tag, props, children, e.span, Some(k.to_string())),
            ExprKind::Call { function, args, arg_names } => {
                if let ExprKind::Ident(n) = &function.kind {
                    if let Some(info) = self.fns.get(n).or(self.imported_fns.get(n)).cloned().filter(|f| f.kind == FnKind::Component && !self.is_local(n)) {
                        return self.component_call(&info, n, args, arg_names, e.span, Some(k));
                    }
                }
                self.keyed_fragment(e, k)
            }
            ExprKind::If { .. } => self.keyed_fragment(e, k),
            _ => self.expr(e),
        }
    }

    /// `<Fragment key={k}>{e}</Fragment>`, for an element that a function
    /// builds (the key cannot go on the element itself).
    fn keyed_fragment(&mut self, e: &Expr, k: &str) -> String {
        let v = self.expr(e);
        if !self.builds_element(e) {
            return v;
        }
        let f = self.react("Fragment");
        format!("<{f} {}>{{{v}}}</{f}>", key_attr(k))
    }

    // ── calls ───────────────────────────────────────────────────────────────

    fn call(&mut self, function: &Expr, args: &[Expr], arg_names: &[Option<String>], span: SourceSpan) -> (String, u8) {
        match &function.kind {
            ExprKind::Ident(name) => {
                if self.is_local(name) {
                    let f = self.ident(name, function.span);
                    return (self.call_text(&f, None, args, arg_names), P_PRIMARY);
                }
                if let Some(s) = self.dialect_call(name, args, arg_names, span) {
                    return (s, P_PRIMARY);
                }
                if self.overloads.contains_key(name) {
                    let a = split_args(args, arg_names);
                    let n = a.positional.len() + a.named.len();
                    if let (Some(info), None) = (self.overload_for(name, n), a.spread) {
                        return (self.call_text(&info.js, Some(&info.params), args, arg_names), P_PRIMARY);
                    }
                }
                if let Some(info) = self.fns.get(name).or(self.imported_fns.get(name)).cloned() {
                    if let Some((path, js)) = self.imported.get(name).cloned() {
                        self.module_imports.entry(path).or_default().insert(js);
                    }
                    return match info.kind {
                        FnKind::Component => (self.component_call(&info, name, args, arg_names, span, None), P_PRIMARY),
                        FnKind::Hook => {
                            if !self.in_render() {
                                self.warn(span, "hook-call", format!("`{name}` keeps state, so it is the hook `{}`, which React allows only at the top of a component or hook, not here", info.js));
                            } else if self.scope().nested > 0 {
                                self.warn(span, "hook-in-loop", format!("`{name}` keeps state (the hook `{}`) and is called inside a loop or branch; React needs hooks called the same way every render", info.js));
                            }
                            (self.call_text(&info.js, Some(&info.params), args, arg_names), P_PRIMARY)
                        }
                        FnKind::Plain => (self.call_text(&info.js, Some(&info.params), args, arg_names), P_PRIMARY),
                    };
                }
                if self.module_names.contains(name) || self.cells.contains_key(name) {
                    let f = self.ident(name, function.span);
                    return (self.call_text(&f, None, args, arg_names), P_PRIMARY);
                }
                if let Some(s) = self.builtin(name, args, arg_names, span) {
                    return (s, P_PRIMARY);
                }
                if self.classes.contains(name) {
                    return (self.call_text(name, None, args, arg_names), P_PRIMARY);
                }
                if let Some(sig) = self.runtime.get(name).or(self.dialect_rt.get(name)).or(self.react_rt.get(name)).cloned() {
                    let f = self.rt(name);
                    if !sig.is_fn {
                        return (self.call_text(&f, None, args, arg_names), P_PRIMARY);
                    }
                    let params: Vec<String> = sig.params().cloned().collect();
                    return (self.sig_call(&f, &sig.positional, &sig.options, sig.rest, &params, args, arg_names), P_PRIMARY);
                }
                let f = self.stub(name, function.span);
                (self.call_text(&f, None, args, arg_names), P_PRIMARY)
            }
            ExprKind::FieldAccess { object, field } => {
                // `module.f(…)`
                if let ExprKind::Ident(q) = &object.kind {
                    if !self.is_local(q) && !self.module_names.contains(q) && self.aliases.contains_key(q) {
                        // When the bare name means something else here, the
                        // call goes through the module's own export.
                        let clash = self.module_names.contains(field) || self.is_local(field);
                        if clash {
                            let fj = self.qualified(q, field, function.span);
                            let module = self.aliases.get(q).cloned().unwrap_or_default();
                            let info = self.opts.modules.get(&module).and_then(|m| m.fns.get(field)).cloned();
                            if let Some(info) = info.clone().filter(|i| i.kind == FnKind::Component) {
                                let mut i2 = info.clone();
                                i2.js = fj;
                                return (self.component_call(&i2, field, args, arg_names, span, None), P_PRIMARY);
                            }
                            return (self.call_text(&fj, info.as_ref().map(|i| i.params.as_slice()), args, arg_names), P_PRIMARY);
                        }
                        let f = Expr { kind: ExprKind::Ident(field.clone()), span: function.span };
                        return self.call(&f, args, arg_names, span);
                    }
                    if self.enums.contains_key(q) || self.classes.contains(q) {
                        let o = self.ident(q, object.span);
                        return (self.call_text(&format!("{o}.{field}"), None, args, arg_names), P_PRIMARY);
                    }
                }
                // A class method.
                if let Some(classes) = self.methods.get(field).cloned() {
                    let o = self.expr(object);
                    let rest: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                    let all = std::iter::once(o.clone()).chain(rest).collect::<Vec<_>>().join(", ");
                    let m = names::camel(field);
                    if classes.len() == 1 {
                        return (format!("{}_{m}({all})", classes[0]), P_PRIMARY);
                    }
                    let table: Vec<String> = classes.iter().map(|c| format!("{c}: {c}_{m}")).collect();
                    return (format!("({{ {} }})[{o}.__class]({all})", table.join(", ")), P_PRIMARY);
                }
                // Method syntax: `x.f(a)` is `f(x, a)` when `f` is a function
                // this file or the runtime knows; else a callable field.
                let known = !self.is_local(field)
                    && (self.fns.contains_key(field)
                        || self.imported_fns.contains_key(field)
                        || self.runtime.contains_key(field)
                        || self.dialect_rt.contains_key(field));
                if known {
                    let mut all = vec![(**object).clone()];
                    all.extend(args.iter().cloned());
                    let mut names_all = vec![None];
                    if arg_names.is_empty() {
                        names_all.extend(std::iter::repeat_n(None, args.len()));
                    } else {
                        names_all.extend(arg_names.iter().cloned());
                    }
                    let f = Expr { kind: ExprKind::Ident(field.clone()), span: function.span };
                    return self.call(&f, &all, &names_all, span);
                }
                let f = self.field(object, field, false);
                (self.call_text(&f, None, args, arg_names), P_PRIMARY)
            }
            _ => {
                let f = self.ex(function, P_PRIMARY);
                (self.call_text(&f, None, args, arg_names), P_PRIMARY)
            }
        }
    }

    /// `f(a, b)`, placing named arguments by the parameter names when they
    /// are known, else in a trailing options object.
    pub fn call_text(&mut self, f: &str, params: Option<&[String]>, args: &[Expr], arg_names: &[Option<String>]) -> String {
        let a = split_args(args, arg_names);
        let mut out: Vec<String> = a.positional.iter().map(|e| self.expr(e)).collect();
        if !a.named.is_empty() {
            match params {
                Some(ps) => {
                    let mut slots: Vec<Option<String>> = out.drain(..).map(Some).collect();
                    for (n, e) in &a.named {
                        let v = self.expr(e);
                        match ps.iter().position(|p| p == n) {
                            Some(i) => {
                                if slots.len() <= i {
                                    slots.resize(i + 1, None);
                                }
                                slots[i] = Some(v);
                            }
                            None => slots.push(Some(v)),
                        }
                    }
                    out = slots.into_iter().map(|s| s.unwrap_or_else(|| "undefined".into())).collect();
                }
                None => {
                    let kv: Vec<String> = a.named.iter().map(|(n, e)| (n.clone(), *e)).collect::<Vec<_>>().into_iter().map(|(n, e)| {
                        let v = self.expr(e);
                        format!("{}: {v}", js_key(&n))
                    }).collect();
                    out.push(format!("{{ {} }}", kv.join(", ")));
                }
            }
        }
        if let Some(s) = a.spread {
            let v = self.ex(s, P_UNARY);
            out.push(format!("...{v}"));
        }
        args_text(f, &out)
    }

    /// A call of a runtime function, by its JS signature.
    #[allow(clippy::too_many_arguments)]
    fn sig_call(&mut self, f: &str, positional: &[String], options: &[String], rest: bool, params: &[String], args: &[Expr], arg_names: &[Option<String>]) -> String {
        let a = split_args(args, arg_names);
        if rest || (options.is_empty() && a.named.is_empty()) {
            return self.call_text(f, Some(params), args, arg_names);
        }
        let mut slots: Vec<Option<String>> = vec![None; positional.len()];
        let mut opts: Vec<(String, String)> = Vec::new();
        for (i, e) in a.positional.iter().enumerate() {
            let v = self.expr(e);
            if i < positional.len() {
                slots[i] = Some(v);
            } else if let Some(k) = options.get(i - positional.len()) {
                opts.push((k.clone(), v));
            } else {
                slots.push(Some(v));
            }
        }
        for (n, e) in &a.named {
            let v = self.expr(e);
            if let Some(i) = positional.iter().position(|p| p == n) {
                slots[i] = Some(v);
            } else {
                opts.push((n.clone(), v));
            }
        }
        while slots.last().is_some_and(|s| s.is_none()) {
            slots.pop();
        }
        let mut out: Vec<String> = slots.into_iter().map(|s| s.unwrap_or_else(|| "undefined".into())).collect();
        if !opts.is_empty() {
            let kv: Vec<String> = opts.iter().map(|(k, v)| format!("{}: {v}", js_key(k))).collect();
            out.push(format!("{{ {} }}", kv.join(", ")));
        }
        args_text(f, &out)
    }

    /// Note that the module uses an imported name, so it is imported.
    pub fn use_imported(&mut self, name: &str) {
        if self.is_local(name) || self.fns.contains_key(name) {
            return;
        }
        if let Some((path, js)) = self.imported.get(name).cloned() {
            self.module_imports.entry(path).or_default().insert(js);
        }
    }

    /// `card(item)` → `<Card item={item} />`.
    pub fn component_call(&mut self, info: &crate::analysis::FnInfo, name: &str, args: &[Expr], arg_names: &[Option<String>], span: SourceSpan, key: Option<&str>) -> String {
        self.use_imported(name);
        if info.design_component.is_some() {
            // A design render function called directly: an instance.
            let a = split_args(args, arg_names);
            let mut attrs = Vec::new();
            if let Some(k) = key {
                attrs.push(key_attr(k));
            }
            if let Some(p) = a.positional.first() {
                let v = self.ex(p, P_UNARY);
                attrs.push(format!("{{...{v}}}"));
            }
            if a.positional.len() > 1 {
                self.warn(span, "render-call", format!("`{name}(p, cx)` is called with its own cx; the React component reads cx from the frame it is drawn in"));
            }
            return format!("<{} {} />", info.js, attrs.join(" "));
        }
        let a = split_args(args, arg_names);
        let mut attrs = Vec::new();
        if let Some(k) = key {
            attrs.push(key_attr(k));
        }
        for (i, e) in a.positional.iter().enumerate() {
            let Some(p) = info.params.get(i) else {
                self.warn(span, "extra-arg", format!("`{name}` is called with more arguments than it takes"));
                break;
            };
            let v = self.expr(e);
            attrs.push(attr(&prop_name(p), &v, e));
        }
        for (n, e) in &a.named {
            let v = self.expr(e);
            attrs.push(attr(&prop_name(n), &v, e));
        }
        if let Some(s) = a.spread {
            let v = self.ex(s, P_UNARY);
            attrs.push(format!("{{...{v}}}"));
        }
        let one = format!("<{} {}/>", info.js, attrs.iter().map(|a| format!("{a} ")).collect::<String>());
        if one.len() <= 100 && !one.contains('\n') {
            one.replace("  />", " />")
        } else {
            format!("<{}\n{}/>", info.js, attrs.iter().map(|a| format!("  {}\n", a.replace('\n', "\n  "))).collect::<String>())
        }
    }

    /// A callback passed to a list function: an arrow with one parameter.
    pub fn callback(&mut self, f: &Expr, arity: usize, key: bool) -> String {
        self.callback_keyed(f, arity, if key { Some(KeyFrom::Index) } else { None })
    }

    pub fn callback_keyed(&mut self, f: &Expr, arity: usize, keyfrom: Option<KeyFrom>) -> String {
        let key = keyfrom.is_some();
        if let ExprKind::Ident(n) = &f.kind {
            // A local function that builds an element: keyed by a fragment
            // around its result.
            if key && self.scopes.iter().any(|s| s.elem_locals.contains(n)) {
                let fj = self.ident(n, f.span);
                let ps: Vec<String> = ["x", "y", "z"].iter().take(arity).map(|s| s.to_string()).collect();
                let frag = self.react("Fragment");
                let (head, k) = match keyfrom {
                    Some(KeyFrom::Param) => (format!("({})", ps.join(", ")), ps[0].clone()),
                    _ => {
                        let i = self.names.fresh("i");
                        (format!("({}, {i})", ps.join(", ")), i)
                    }
                };
                return format!("{head} => <{frag} key={{{k}}}>{{{fj}({})}}</{frag}>", ps.join(", "));
            }
        }
        match &f.kind {
            ExprKind::Lambda { params, body } => self.lambda(params, body, keyfrom),
            ExprKind::Ident(n) if !self.is_local(n) => {
                if let Some(info) = self.fns.get(n).or(self.imported_fns.get(n)).cloned() {
                    if info.kind == FnKind::Component && info.design_component.is_none() {
                        let ps: Vec<String> = info.params.iter().take(arity).map(|p| names::camel(p)).collect();
                        let attrs: Vec<String> = info.params.iter().take(arity).zip(&ps).map(|(p, j)| format!("{}={{{j}}}", prop_name(p))).collect();
                        if key {
                            let i = self.names.fresh("i");
                            return format!("({}, {i}) => <{} key={{{i}}} {} />", ps.join(", "), info.js, attrs.join(" "));
                        }
                        return format!("({}) => <{} {} />", ps.join(", "), info.js, attrs.join(" "));
                    }
                    if info.params.len() == arity {
                        if let Some((path, js)) = self.imported.get(n).cloned() {
                            self.module_imports.entry(path).or_default().insert(js);
                        }
                        // JS passes the index too; a function of exactly the
                        // callback's arity ignores it.
                        return info.js;
                    }
                }
                let fj = self.ident(n, f.span);
                let ps: Vec<String> = ["x", "y", "z"].iter().take(arity).map(|s| s.to_string()).collect();
                format!("({}) => {fj}({})", ps.join(", "), ps.join(", "))
            }
            _ => {
                let fj = self.ex(f, P_PRIMARY);
                let ps: Vec<String> = ["x", "y", "z"].iter().take(arity).map(|s| s.to_string()).collect();
                format!("({}) => {fj}({})", ps.join(", "), ps.join(", "))
            }
        }
    }

    /// Builtins with a direct JS form. Anything else the runtime exports is
    /// imported from it.
    fn builtin(&mut self, name: &str, args: &[Expr], arg_names: &[Option<String>], _span: SourceSpan) -> Option<String> {
        if !arg_names.iter().all(|n| n.is_none()) {
            return None;
        }
        let a = |s: &mut Self, i: usize| -> String { s.ex(&args[i], P_PRIMARY) };
        Some(match (name, args.len()) {
            ("print", _) => {
                let xs: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("console.log({})", xs.join(", "))
            }
            ("map", 2) | ("filter", 2) | ("forEach", 2) | ("any", 2) | ("all", 2) | ("find", 2) | ("flat_map", 2) => {
                let xs = a(self, 0);
                let key = (name == "map" || name == "flat_map") && self.callback_builds_element(&args[1]);
                let over_range = matches!(&args[0].kind, ExprKind::Call { function, .. } if matches!(&function.kind, ExprKind::Ident(n) if n == "range"));
                let keyfrom = match (key, over_range && name == "map") {
                    (false, _) => None,
                    (true, true) => Some(KeyFrom::Param),
                    (true, false) => Some(KeyFrom::Index),
                };
                let cb = self.callback_keyed(&args[1], 1, keyfrom);
                let m = match name {
                    "any" => "some",
                    "all" => "every",
                    "flat_map" => "flatMap",
                    n => n,
                };
                let call = format!("{xs}.{m}({cb})");
                if name == "find" { format!("({call} ?? null)") } else { call }
            }
            ("reduce", 3) => {
                let xs = a(self, 0);
                let init = self.expr(&args[1]);
                let cb = self.callback(&args[2], 2, false);
                format!("{xs}.reduce({cb}, {init})")
            }
            ("len", 1) if matches!(args[0].kind, ExprKind::List(_) | ExprKind::Literal(Literal::String(_))) => format!("{}.length", a(self, 0)),
            ("str", 1) if matches!(args[0].kind, ExprKind::Literal(Literal::Int(_)) | ExprKind::Literal(Literal::Float(_))) => {
                format!("String({})", self.expr(&args[0]))
            }
            ("abs" | "sqrt" | "floor" | "ceil" | "sin" | "cos" | "tan" | "atan" | "asin" | "acos" | "exp" | "log", 1) => {
                format!("Math.{name}({})", self.expr(&args[0]))
            }
            ("atan2" | "pow" | "hypot", _) => {
                let xs: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("Math.{name}({})", xs.join(", "))
            }
            ("min" | "max", n) if n >= 2 => {
                let xs: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("Math.{name}({})", xs.join(", "))
            }
            ("upper", 1) => format!("{}.toUpperCase()", a(self, 0)),
            ("lower", 1) => format!("{}.toLowerCase()", a(self, 0)),
            ("trim", 1) => format!("{}.trim()", a(self, 0)),
            ("split", 2) => format!("{}.split({})", a(self, 0), self.expr(&args[1])),
            ("starts_with", 2) => format!("{}.startsWith({})", a(self, 0), self.expr(&args[1])),
            ("ends_with", 2) => format!("{}.endsWith({})", a(self, 0), self.expr(&args[1])),
            ("keys", 1) if !self.uses_maps => format!("Object.keys({})", self.expr(&args[0])),
            ("values", 1) if !self.uses_maps => format!("Object.values({})", self.expr(&args[0])),
            ("entries", 1) if !self.uses_maps => format!("Object.entries({})", self.expr(&args[0])),
            ("append" | "push", 2) => {
                let xs = self.ex(&args[0], P_UNARY);
                // An element appended to a list is keyed by its position,
                // as Petal keys the state of unkeyed items.
                let item = if self.builds_element(&args[1]) {
                    let k = format!("{}.length", self.ex(&args[0], P_PRIMARY));
                    self.keyed(&args[1], Some(&k))
                } else {
                    self.expr(&args[1])
                };
                format!("[...{xs}, {item}]")
            }
            ("concat", _) if args.iter().all(|x| !matches!(x.kind, ExprKind::Literal(Literal::String(_)) | ExprKind::StringInterp { .. })) && args.len() >= 2 && args.iter().any(|x| matches!(x.kind, ExprKind::List(_))) => {
                let refs: Vec<&Expr> = args.iter().collect();
                let xs: Vec<String> = self.concat_operands(&refs, P_UNARY).into_iter().map(|x| format!("...{x}")).collect();
                format!("[{}]", xs.join(", "))
            }
            ("concat", n) if n >= 2 => {
                let f = self.rt("concat");
                let refs: Vec<&Expr> = args.iter().collect();
                let xs = self.concat_operands(&refs, 0);
                format!("{f}({})", xs.join(", "))
            }
            ("tunable" | "rigid" | "const", n) if n >= 1 => self.expr(&args[0]),
            ("json_stringify", 1) => format!("JSON.stringify({})", self.expr(&args[0])),
            ("json_parse", 1) => format!("JSON.parse({})", self.expr(&args[0])),
            _ => return None,
        })
    }

    fn callback_builds_element(&self, f: &Expr) -> bool {
        match &f.kind {
            ExprKind::Lambda { body, .. } => crate::analysis::tail_expr(body).is_some_and(|t| {
                self.builds_element(t) || matches!(&t.kind, ExprKind::List(xs) if xs.iter().any(|x| self.builds_element(x)))
            }),
            ExprKind::Ident(n) => {
                self.scopes.iter().any(|s| s.elem_locals.contains(n))
                    || !self.is_local(n) && self.fns.get(n).or(self.imported_fns.get(n)).is_some_and(|f| f.kind == FnKind::Component)
            }
            _ => false,
        }
    }

    /// A call the dialect gives a meaning of its own.
    fn dialect_call(&mut self, name: &str, args: &[Expr], arg_names: &[Option<String>], span: SourceSpan) -> Option<String> {
        match (self.dialect, name) {
            (Dialect::Web, "next_id") => {
                self.warn(span, "event", "`next_id()` outside a planned click handler: it is an event that runs the handlers `clicked(id)` registers");
                let e = self.rt("petal_event");
                Some(format!("{e}()"))
            }
            (Dialect::Web, "clicked") => {
                self.warn(span, "event", "`clicked(id)` is read as a value here, not as `if clicked(id) then …`: React has no per-frame click poll, so it is `false`");
                Some("false".into())
            }
            (Dialect::Design, "design") => {
                let f = self.design_rt("design");
                Some(self.call_text(&f, None, args, arg_names))
            }
            (Dialect::Design, "component") => Some(self.component_decl(args, arg_names, span)),
            (Dialect::Design, "system" | "m3" | "m3_scheme" | "glass" | "material" | "backdrop") => {
                self.warn_once(name, span, "approximate", format!("`{name}(…)` is approximated in the React runtime (petal-design.jsx), not drawn as Mixit draws it"));
                None
            }
            (Dialect::Design, "variant_grid") => {
                self.warn_once(name, span, "approximate", "`variant_grid` is drawn as a simple grid of instances in the React runtime");
                None
            }
            _ => None,
        }
    }

    /// `component("card", card, props: {…})`: the props record moves to
    /// `Card.defaults` (so the component can merge its props over them), and
    /// the call names the component.
    fn component_decl(&mut self, args: &[Expr], arg_names: &[Option<String>], span: SourceSpan) -> String {
        let f = self.design_rt("component");
        let a = split_args(args, arg_names);
        let comp = a.positional.get(1).and_then(|e| match &e.kind {
            ExprKind::Ident(n) => self.fns.get(n).or(self.imported_fns.get(n)).cloned().filter(|i| i.design_component.is_some()).map(|i| {
                self.use_imported(n);
                i.js
            }),
            _ => None,
        });
        // A render function this module did not define: its defaults stay
        // with the call (the component merges what `component` stored).
        let local = a.positional.get(1).is_some_and(|e| matches!(&e.kind, ExprKind::Ident(n) if self.fns.contains_key(n)));
        let mut out = Vec::new();
        if let Some(n) = a.positional.first() {
            out.push(self.expr(n));
        }
        match (&comp, a.positional.get(1)) {
            (Some(c), _) => out.push(c.clone()),
            (None, Some(e)) => {
                let r = self.design_rt("component_from_render");
                let v = self.expr(e);
                out.push(format!("{r}({v})"));
            }
            _ => {}
        }
        let mut opts = Vec::new();
        for (k, e) in &a.named {
            if k == "props" && local {
                if let Some(c) = &comp {
                    let v = self.expr(e);
                    self.hoisted.push(format!("{c}.defaults = {v};"));
                    opts.push(format!("props: {c}.defaults"));
                    continue;
                }
            }
            let v = self.expr(e);
            opts.push(format!("{}: {v}", js_key(k)));
        }
        if let Some(c) = comp.as_ref().filter(|_| local) {
            if !a.named.iter().any(|(k, _)| k == "props") {
                self.hoisted.push(format!("{c}.defaults = {{}};"));
            }
        }
        let _ = span;
        if !opts.is_empty() {
            out.push(format!("{{ {} }}", opts.join(", ")));
        }
        args_text(&f, &out)
    }
}

/// The hex digits of a color literal's record: fields `r`, `g`, `b` (and
/// `a`), int literals, each written at the record's own span.
fn color_literal(fields: &[RecordField], span: SourceSpan) -> Option<String> {
    let mut out = String::new();
    let names = ["r", "g", "b", "a"];
    if !(3..=4).contains(&fields.len()) {
        return None;
    }
    for (i, f) in fields.iter().enumerate() {
        let RecordField::Named(k, v) = f else { return None };
        let ExprKind::Literal(Literal::Int(n)) = v.kind else { return None };
        if k != names[i] || v.span.start.offset != span.start.offset || v.span.end.offset != span.end.offset || !(0..=255).contains(&n) {
            return None;
        }
        out.push_str(&format!("{n:02X}"));
    }
    Some(out)
}

pub enum KeyFrom<'e> {
    /// A `for` loop: its `key` expression, or the index.
    Expr(Option<&'e Expr>),
    /// A `map` callback: the index.
    Index,
    /// A `map` over `range(…)`: the number itself.
    Param,
}

/// `TESS` → `TESS`, `tok` → `Tok`: a name's first letter up, the rest kept.
fn pascal_keep(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

pub fn args_text(f: &str, args: &[String]) -> String {
    let one = format!("{f}({})", args.join(", "));
    if one.len() <= 100 && !one.contains('\n') {
        return one;
    }
    // A last argument that is a record, list or function hugs the parens.
    if let Some((last, rest)) = args.split_last() {
        let short = rest.iter().all(|a| !a.contains('\n')) && rest.iter().map(|a| a.len() + 2).sum::<usize>() < 60;
        if short && (last.starts_with('{') || last.starts_with('[') || last.contains("=> {") || last.starts_with('(')) && last.contains('\n') {
            let head: String = rest.iter().map(|a| format!("{a}, ")).collect();
            return format!("{f}({head}{last})");
        }
    }
    format!("{f}(\n{})", args.iter().map(|a| format!("  {},\n", a.replace('\n', "\n  "))).collect::<String>())
}

/// `key="today"` for a string literal key, else `key={k}`.
pub fn key_attr(k: &str) -> String {
    if k.starts_with('"') && k.ends_with('"') && !k[1..k.len() - 1].contains(['"', '\\', '{', '}']) {
        format!("key={k}")
    } else {
        format!("key={{{k}}}")
    }
}

pub fn list_text(items: &[String]) -> String {
    let one = format!("[{}]", items.join(", "));
    if one.len() <= 90 && !one.contains('\n') {
        one
    } else if items.iter().all(|i| i.len() <= 24 && !i.contains('\n')) {
        // Short items (numbers, pairs, words) fill lines, as data is read.
        let mut lines: Vec<String> = vec![];
        let mut cur = String::new();
        for it in items {
            if !cur.is_empty() && cur.len() + it.len() + 2 > 78 {
                lines.push(std::mem::take(&mut cur));
            }
            cur.push_str(it);
            cur.push_str(", ");
        }
        lines.push(cur);
        format!("[\n{}]", lines.iter().map(|l| format!("  {}\n", l.trim_end())).collect::<String>())
    } else {
        format!("[\n{}]", items.iter().map(|a| format!("  {},\n", a.replace('\n', "\n  "))).collect::<String>())
    }
}

/// A JSX attribute: `name="text"`, `name` (true) or `name={expr}`.
pub fn attr(name: &str, v: &str, e: &Expr) -> String {
    match &e.kind {
        ExprKind::Literal(Literal::Bool(true)) => name.to_string(),
        ExprKind::Literal(Literal::String(s)) if !s.contains(['"', '\\', '\n', '{', '}', '&', '<', '>']) => format!("{name}=\"{s}\""),
        _ => format!("{name}={{{}}}", v),
    }
}

#[allow(dead_code)]
fn _unused(_: &HashSet<String>, _: &JsxChild) {}
