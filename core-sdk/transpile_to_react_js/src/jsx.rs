//! Elements as JSX.

use petal::ast::{Expr, ExprKind, JsxChild, Literal};
use petal::source_map::SourceSpan;

use crate::analysis::FnKind;
use crate::dialect::{self, Dialect, Tag};
use crate::emit::{Emitter, Kind};
use crate::expr::{P_PRIMARY, attr};
use crate::names::{self, js_string, pascal};

impl<'a> Emitter<'a> {
    pub fn element(&mut self, tag: &str, props: &[(String, Expr)], children: &[JsxChild], span: SourceSpan, key: Option<String>) -> String {
        let t = dialect::tag(self.dialect, tag);
        // A frame written at the top level of a design is lifted into a
        // component of its own: a screen an app can render by name.
        if self.dialect == Dialect::Design && tag == "frame" && self.scopes.len() == 1 && self.scope().kind() == Kind::Module {
            if let Some(id) = props.iter().find(|(k, _)| k == "id").and_then(|(_, v)| match &v.kind {
                ExprKind::Literal(Literal::String(s)) => Some(s.clone()),
                _ => None,
            }) {
                return self.lift_frame(&id, props, children, span, key);
            }
        }
        match t {
            Tag::Instance => self.instance(props, children, span, key),
            Tag::Slot => self.slot(props, children, span),
            Tag::Host(h) => {
                let attrs = self.attrs(props, true, key);
                let kids = self.children(children, &h);
                format_element(&h, &attrs, &kids)
            }
            Tag::Runtime(c) => {
                let name = self.design_rt(&c);
                if tag == "icon" {
                    self.warn_once("icon", span, "approximate", "icons (`<icon name=\"sf:…\">`) are drawn as placeholders of their size: SF Symbols are not on the web");
                }
                let attrs = self.attrs(props, false, key);
                let kids = self.children(children, tag);
                format_element(&name, &attrs, &kids)
            }
            Tag::Unknown(u) => {
                let name = pascal(&u);
                let name = if self.dialect == Dialect::Design {
                    self.warn_once(&format!("tag:{u}"), span, "unknown-element", format!("`<{u}>` is not an element of the design vocabulary: it is drawn as nothing (Mixit warns about it too)"));
                    let n = self.design_rt("Unknown");
                    let mut attrs = vec![format!("tag={}", js_string(&u))];
                    attrs.extend(self.attrs(props, false, key));
                    let kids = self.children(children, &u);
                    return format_element(&n, &attrs, &kids);
                } else {
                    self.warn_once(&format!("tag:{u}"), span, "unknown-element", format!("`<{u}>` is not an HTML element: it is the component `{name}`, imported from {}", self.opts.elements_module));
                    self.used_elements.insert(name.clone());
                    name
                };
                let attrs = self.attrs(props, false, key);
                let kids = self.children(children, &u);
                format_element(&name, &attrs, &kids)
            }
        }
    }

    fn lift_frame(&mut self, id: &str, props: &[(String, Expr)], children: &[JsxChild], span: SourceSpan, key: Option<String>) -> String {
        let base = pascal(id);
        let name = if self.names.is_taken(&base) { self.names.fresh(&format!("{base}Frame")) } else { self.names.fresh(&base) };
        // The frame itself, written inside the new component (not lifted
        // again: the scope is a component now).
        self.push_scope(Kind::Component);
        let frame = self.design_rt("Frame");
        let attrs = self.attrs(props, false, None);
        let kids = self.children(children, "frame");
        let body = format_element(&frame, &attrs, &kids);
        self.pop_scope();
        let _ = span;
        self.hoisted.push(format!(
            "/** The `{id}` frame. */\nexport function {name}() {{\n  return {};\n}}\n{name}.frameId = {};",
            crate::emit::indent_rest(&crate::emit::paren_multiline(&body)),
            js_string(id)
        ));
        self.lifted_frames.push((id.to_string(), name.clone()));
        self.exports.values.insert(format!("frame:{id}"), name.clone());
        match key {
            Some(k) => format!("<{name} key={{{k}}} />"),
            None => format!("<{name} />"),
        }
    }

    /// The attributes of an element. `host` maps HTML attribute names
    /// (`class` → `className`) and `style="…"` to a style object.
    fn attrs(&mut self, props: &[(String, Expr)], host: bool, key: Option<String>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(k) = key {
            out.push(crate::expr::key_attr(&k));
        }
        for (k, v) in props {
            let name = if host { dialect::host_attr(self.dialect, k) } else { k.clone() };
            if host && k == "style" {
                if let ExprKind::Literal(Literal::String(css)) = &v.kind {
                    if let Some(obj) = dialect::css_object(css) {
                        out.push(format!("style={{{obj}}}"));
                        continue;
                    }
                }
                if matches!(v.kind, ExprKind::StringInterp { .. }) {
                    self.warn(v.span, "style", "a `style` string with holes is passed as a string; React wants an object (`style={{ width: w }}`)");
                }
            }
            let js_name = if names::is_js_ident(&name) || name.contains('-') { name } else { format!("{}_", name.replace(|c: char| !c.is_ascii_alphanumeric(), "_")) };
            let js_name = if js_name == "key" || js_name == "ref" { format!("{js_name}_") } else { js_name };
            let val = self.expr(v);
            out.push(attr(&js_name, &val, v));
        }
        out
    }

    /// Children, each as a JSX child: an element, `{expr}`, or text.
    fn children(&mut self, children: &[JsxChild], _tag: &str) -> Vec<Child> {
        let mut out = Vec::new();
        for c in children {
            match c {
                JsxChild::Text(t) => out.push(Child::Text(jsx_text(t))),
                JsxChild::Expr(e) => match &e.kind {
                    ExprKind::Element { tag, props, children } => {
                        let s = self.element(tag, props, children, e.span, None);
                        out.push(Child::Node(s));
                    }
                    ExprKind::Literal(Literal::Nil) => {}
                    ExprKind::Literal(Literal::String(s)) => out.push(Child::Text(jsx_text(s))),
                    ExprKind::Call { function, args, arg_names } if self.is_component_call(function) => {
                        let ExprKind::Ident(n) = &function.kind else { unreachable!() };
                        let info = self.fns.get(n).or(self.imported_fns.get(n)).cloned().unwrap();
                        let s = self.component_call(&info, n, args, arg_names, e.span, None);
                        out.push(Child::Node(s));
                    }
                    _ => {
                        let s = self.ex(e, 0);
                        // A call that came out as one JSX element is a child
                        // as it is, not `{<X />}`.
                        if s.starts_with('<') && s.ends_with('>') && !s.starts_with("<>") && one_element(&s) {
                            out.push(Child::Node(s));
                        } else {
                            out.push(Child::Hole(s));
                        }
                    }
                },
            }
        }
        out
    }

    fn is_component_call(&self, f: &Expr) -> bool {
        match &f.kind {
            ExprKind::Ident(n) => !self.is_local(n) && self.fns.get(n).or(self.imported_fns.get(n)).is_some_and(|i| i.kind == FnKind::Component),
            _ => false,
        }
    }

    /// `<instance of="card" title="Today">…</instance>` → `<Card title="Today">…</Card>`.
    fn instance(&mut self, props: &[(String, Expr)], children: &[JsxChild], span: SourceSpan, key: Option<String>) -> String {
        let of = props.iter().find(|(k, _)| k == "of").map(|(_, v)| v);
        let mut rest: Vec<(String, Expr)> = props.iter().filter(|(k, _)| k != "of" && k != "props").cloned().collect();
        let spread = props.iter().find(|(k, _)| k == "props").map(|(_, v)| v.clone());
        let (name, extra) = match of.map(|e| &e.kind) {
            Some(ExprKind::Literal(Literal::String(c))) => match self.design_components.get(c).cloned() {
                Some(js) => {
                    let path = self.imported.values().find(|(_, j)| *j == js).map(|(p, _)| p.clone()).or_else(|| self.design_component_paths.get(&js).cloned());
                    if let Some(path) = path.filter(|_| !self.fns.values().any(|f| f.js == js)) {
                        self.module_imports.entry(path).or_default().insert(js.clone());
                    }
                    (js, None)
                }
                None => {
                    self.warn_once(&format!("instance:{c}"), span, "unknown-component", format!("`<instance of=\"{c}\">`: no `component(\"{c}\", …)` in this file (a kit's?); it is drawn as a placeholder, looked up by name at render time"));
                    (self.design_rt("Instance"), Some(format!("of={}", js_string(c))))
                }
            },
            Some(_) => {
                let v = self.expr(of.unwrap());
                (self.design_rt("Instance"), Some(format!("of={{{v}}}")))
            }
            None => {
                self.warn(span, "instance", "an `<instance>` without `of`");
                (self.design_rt("Instance"), None)
            }
        };
        let mut attrs = Vec::new();
        if let Some(k) = key {
            attrs.push(crate::expr::key_attr(&k));
        }
        attrs.extend(extra);
        if let Some(s) = spread {
            let v = self.ex(&s, P_PRIMARY);
            attrs.push(format!("{{...{v}}}"));
        }
        rest.retain(|(k, _)| k != "key");
        attrs.extend(self.attrs(&rest, false, None));
        let kids = self.children(children, "instance");
        format_element(&name, &attrs, &kids)
    }

    /// `<slot />` / `<slot name="footer">placeholder</slot>`.
    fn slot(&mut self, props: &[(String, Expr)], children: &[JsxChild], span: SourceSpan) -> String {
        let props_js = self.scopes.iter().rev().find_map(|s| s.props_js.clone());
        let Some(p) = props_js else {
            self.warn(span, "slot", "a `<slot>` outside a design component draws nothing");
            return "null".into();
        };
        let name = props.iter().find(|(k, _)| k == "name").map(|(_, v)| self.expr(v)).unwrap_or_else(|| "null".into());
        let kids = self.children(children, "slot");
        let fallback = match kids.len() {
            0 => "null".to_string(),
            1 => match &kids[0] {
                Child::Node(s) => s.clone(),
                Child::Hole(s) => s.clone(),
                Child::Text(t) => js_string(t),
            },
            _ => format_element("", &[], &kids),
        };
        let f = self.design_rt("slot");
        format!("{{{f}({p}.children, {name}, {})}}", fallback)
    }
}

/// Whether JSX text is a single element (its first tag closes at its end).
fn one_element(s: &str) -> bool {
    let name: String = s[1..].chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.').collect();
    !name.is_empty() && (s.ends_with(&format!("</{name}>")) || (s.ends_with("/>") && !s[1..].contains(&format!("<{name}"))))
        && s.matches(&format!("<{name}")).count() == s.matches(&format!("</{name}>")).count() + usize::from(s.ends_with("/>") && !s.ends_with(&format!("</{name}>")))
}

pub enum Child {
    Text(String),
    Node(String),
    Hole(String),
}

/// Text as a JSX child: as written when JSX would read it back the same.
fn jsx_text(t: &str) -> String {
    if t.contains(['{', '}', '<', '>']) || t.trim() != t && (t.starts_with(char::is_whitespace) || t.ends_with(char::is_whitespace)) && t.trim().is_empty() {
        format!("{{{}}}", js_string(t))
    } else {
        t.to_string()
    }
}

/// `<Name attrs>children</Name>`. Children with text stay on one line, so
/// their spaces mean what they meant in Petal; element children go one per
/// line.
pub fn format_element(name: &str, attrs: &[String], kids: &[Child]) -> String {
    let attrs_one: String = attrs.iter().map(|a| format!(" {a}")).collect();
    let multi_attr = attrs_one.len() > 90 || attrs.iter().any(|a| a.contains('\n'));
    let open = if multi_attr {
        format!("<{name}\n{}", attrs.iter().map(|a| format!("  {}\n", a.replace('\n', "\n  "))).collect::<String>())
    } else {
        format!("<{name}{attrs_one}")
    };
    // A slot's `{slot(…)}` is the only kind of child that is already braced.
    let child_text = |c: &Child| -> String {
        match c {
            Child::Text(t) => t.clone(),
            Child::Node(s) => s.clone(),
            Child::Hole(s) if s.starts_with('{') && s.ends_with('}') && s.contains("slot(") => s.clone(),
            Child::Hole(s) => format!("{{{s}}}"),
        }
    };
    if kids.is_empty() {
        return if multi_attr { format!("{open}/>") } else { format!("{open} />") };
    }
    let has_text = kids.iter().any(|k| matches!(k, Child::Text(_)));
    let inline: String = kids.iter().map(child_text).collect();
    let close_open = if multi_attr { format!("{open}>") } else { format!("{open}>") };
    if has_text && !inline.contains('\n') {
        return format!("{close_open}{inline}</{name}>");
    }
    if has_text {
        // Text beside multi-line children: keep the run on its lines.
        return format!("{close_open}\n  {}\n</{name}>", inline.replace('\n', "\n  "));
    }
    if !multi_attr && kids.len() == 1 && !inline.contains('\n') && open.len() + inline.len() < 80 {
        return format!("{close_open}{inline}</{name}>");
    }
    let mut out = format!("{close_open}\n");
    for k in kids {
        out.push_str(&format!("  {}\n", child_text(k).replace('\n', "\n  ")));
    }
    out.push_str(&format!("</{name}>"));
    out
}
