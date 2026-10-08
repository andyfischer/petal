//! Target paths: how `petal apply-change` names one binding in a file.
//!
//! ```text
//! step/vx               the `vx` declared in a function `step`
//! build_view/row/out[2] the second `out` declared in `row`, inside `build_view`
//! /score                the module-level `score`
//! ```
//!
//! # Grammar
//!
//! ```text
//! path    := ["/"] segment ("/" segment)*
//! segment := name ["[" n "]"]          n is 1-based
//! name    := an identifier, or `Class.method` for a method declaration
//! ```
//!
//! # What a segment names
//!
//! A segment names a *declaration*: a named `fn`, or a `let` / `var` /
//! `state` / `state var` binding. Nothing else is one — a parameter, a `for`
//! variable, a match-pattern binding and a name first bound by a bare `x = …`
//! cannot be named.
//!
//! One declaration *encloses* another when the inner one is written inside
//! its text: in the body (or a default parameter value) of a `fn`, or in the
//! initializer of a binding (`let row = fn(i) … end`, `let total = for … end`).
//! Control flow is transparent, and so is an anonymous function: a `let`
//! inside an `if`, a loop, or a callback passed straight to a call belongs to
//! the nearest declaration around it. A binding's full path is the chain of
//! declarations that enclose it, outermost first, ending in the binding.
//!
//! # Matching
//!
//! A path with a leading `/` is the full path, starting at module level. A
//! path without one matches any binding whose full path *ends* with it, so
//! `vx` finds `/step/vx` and `step/vx` finds `/physics/step/vx`.
//!
//! `name[n]` is the n-th declaration called `name` directly inside its
//! parent, counted in source order over every kind of declaration (so an
//! index does not move when one of them is converted). A segment without an
//! index matches all of them. The match has to be unique: none and several
//! are both errors, and each lists the full paths that came close.

use crate::ast::{Expr, ExprVisitor, Stmt, StmtKind, walk_expr, walk_stmt};

/// What kind of declaration a path segment landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Fn,
    Let,
    /// `config let`: a tuning knob, which cannot be a `var`.
    ConfigLet,
    Var,
    State,
    StateVar,
}

impl DeclKind {
    /// The declaration's keyword(s), for messages.
    pub fn keyword(self) -> &'static str {
        match self {
            DeclKind::Fn => "fn",
            DeclKind::Let => "let",
            DeclKind::ConfigLet => "config let",
            DeclKind::Var => "var",
            DeclKind::State => "state",
            DeclKind::StateVar => "state var",
        }
    }
}

/// One declaration in a file.
#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    /// 1-based line of the declaring statement.
    pub line: u32,
    /// Char offset of the declaring statement — its identity in the AST.
    pub offset: u32,
    /// Written with `pub` (or the deprecated `export`).
    pub exported: bool,
    /// The enclosing declaration, `None` at module level.
    pub parent: Option<usize>,
    /// 1-based position among the same-named declarations of `parent`.
    pub ordinal: usize,
    /// How many declarations of `parent` share this name.
    pub repeats: usize,
}

/// Every declaration in a file, in source order.
pub struct DeclTree {
    pub decls: Vec<Decl>,
}

impl DeclTree {
    pub fn build(stmts: &[Stmt]) -> DeclTree {
        let mut c = Collector {
            decls: Vec::new(),
            parents: Vec::new(),
        };
        for s in stmts {
            c.visit_stmt(s);
        }
        let mut decls = c.decls;
        // Ordinals: the collector visits in source order, so a running count
        // per (parent, name) is the position among same-named siblings.
        let mut seen: std::collections::HashMap<(Option<usize>, String), usize> =
            std::collections::HashMap::new();
        for d in decls.iter_mut() {
            let n = seen.entry((d.parent, d.name.clone())).or_insert(0);
            *n += 1;
            d.ordinal = *n;
        }
        for d in decls.iter_mut() {
            d.repeats = seen[&(d.parent, d.name.clone())];
        }
        DeclTree { decls }
    }

    /// The full path of declaration `i`, with a leading `/` and an index on
    /// exactly the segments that need one.
    pub fn path(&self, i: usize) -> String {
        let mut segments = Vec::new();
        let mut at = Some(i);
        while let Some(j) = at {
            let d = &self.decls[j];
            segments.push(if d.repeats > 1 {
                format!("{}[{}]", d.name, d.ordinal)
            } else {
                d.name.clone()
            });
            at = d.parent;
        }
        segments.reverse();
        format!("/{}", segments.join("/"))
    }

    /// Every declaration `path` matches, in source order.
    pub fn matches(&self, path: &TargetPath) -> Vec<usize> {
        (0..self.decls.len())
            .filter(|&i| self.matches_one(i, path))
            .collect()
    }

    fn matches_one(&self, i: usize, path: &TargetPath) -> bool {
        let mut at = Some(i);
        for segment in path.segments.iter().rev() {
            let Some(j) = at else {
                return false;
            };
            let d = &self.decls[j];
            if d.name != segment.name || segment.index.is_some_and(|n| n != d.ordinal) {
                return false;
            }
            at = d.parent;
        }
        // An anchored path must have consumed the whole chain.
        !path.anchored || at.is_none()
    }

    /// `path (line N, keyword)` for declaration `i`, as the error lists show
    /// a candidate.
    pub fn describe(&self, i: usize) -> String {
        let d = &self.decls[i];
        format!(
            "{}  (line {}, `{}`)",
            self.path(i),
            d.line,
            d.kind.keyword()
        )
    }

    /// Resolve `path` to the one declaration it names, or explain why it
    /// names none or several, listing the candidates.
    pub fn resolve(&self, path: &TargetPath) -> Result<usize, String> {
        let found = self.matches(path);
        match found[..] {
            [one] => Ok(one),
            [] => {
                let last = &path.segments[path.segments.len() - 1].name;
                let same_name: Vec<usize> = (0..self.decls.len())
                    .filter(|&i| self.decls[i].name == *last && self.decls[i].kind != DeclKind::Fn)
                    .collect();
                let mut msg = format!("no binding matches the target `{path}`");
                if same_name.is_empty() {
                    msg.push_str(&format!(
                        ": this file declares no `let` or `state` named `{last}`"
                    ));
                    let convertible: Vec<usize> = (0..self.decls.len())
                        .filter(|&i| matches!(self.decls[i].kind, DeclKind::Let | DeclKind::State))
                        .collect();
                    if !convertible.is_empty() {
                        msg.push_str("\nbindings that can be named:");
                        msg.push_str(&self.listing(&convertible));
                    }
                } else {
                    msg.push_str(&format!("\nbindings named `{last}` in this file:"));
                    msg.push_str(&self.listing(&same_name));
                }
                Err(msg)
            }
            _ => Err(format!(
                "the target `{path}` is ambiguous: it matches {} bindings\n\
                 name one of them by its full path:{}",
                found.len(),
                self.listing(&found)
            )),
        }
    }

    /// One indented line per candidate, capped so a huge file stays readable.
    fn listing(&self, which: &[usize]) -> String {
        const MAX: usize = 20;
        let mut out = String::new();
        for &i in which.iter().take(MAX) {
            out.push_str(&format!("\n  {}", self.describe(i)));
        }
        if which.len() > MAX {
            out.push_str(&format!("\n  … and {} more", which.len() - MAX));
        }
        out
    }
}

struct Collector {
    decls: Vec<Decl>,
    /// The declarations enclosing the node being visited, outermost first.
    parents: Vec<usize>,
}

impl Collector {
    fn declare(&mut self, s: &Stmt, name: &str, kind: DeclKind) -> usize {
        self.decls.push(Decl {
            name: name.to_string(),
            kind,
            line: s.span.start.line,
            offset: s.span.start.offset,
            exported: s.exported,
            parent: self.parents.last().copied(),
            ordinal: 0,
            repeats: 0,
        });
        self.decls.len() - 1
    }
}

impl ExprVisitor for Collector {
    fn visit_stmt(&mut self, s: &Stmt) {
        let kind = match &s.kind {
            StmtKind::FnDecl { name, .. } => Some((name, DeclKind::Fn)),
            StmtKind::Let {
                name,
                is_var,
                is_config,
                ..
            } => Some((
                name,
                match (*is_var, *is_config) {
                    (true, _) => DeclKind::Var,
                    (false, true) => DeclKind::ConfigLet,
                    (false, false) => DeclKind::Let,
                },
            )),
            StmtKind::State { name, is_var, .. } => Some((
                name,
                if *is_var {
                    DeclKind::StateVar
                } else {
                    DeclKind::State
                },
            )),
            _ => None,
        };
        let Some((name, kind)) = kind else {
            walk_stmt(self, s);
            return;
        };
        let id = self.declare(s, name, kind);
        self.parents.push(id);
        walk_stmt(self, s);
        self.parents.pop();
    }

    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
}

/// One `name` or `name[n]` step of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub index: Option<usize>,
}

/// A parsed `--target` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPath {
    /// Written with a leading `/`: the path starts at module level.
    pub anchored: bool,
    pub segments: Vec<Segment>,
}

impl std::fmt::Display for TargetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.anchored {
            write!(f, "/")?;
        }
        for (i, s) in self.segments.iter().enumerate() {
            if i > 0 {
                write!(f, "/")?;
            }
            write!(f, "{}", s.name)?;
            if let Some(n) = s.index {
                write!(f, "[{n}]")?;
            }
        }
        Ok(())
    }
}

impl TargetPath {
    pub fn parse(text: &str) -> Result<TargetPath, String> {
        let bad = |why: &str| {
            format!(
                "invalid target path `{text}`: {why}\n\
                 a path is declaration names joined by `/`, ending in the binding: \
                 `step/vx`, `build_view/row/out[2]`, `/score`"
            )
        };
        let (anchored, rest) = match text.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        if rest.is_empty() {
            return Err(bad("it names no binding"));
        }
        let mut segments = Vec::new();
        for part in rest.split('/') {
            if part.is_empty() {
                return Err(bad("it has an empty segment"));
            }
            let (name, index) = match part.split_once('[') {
                None => (part, None),
                Some((name, tail)) => {
                    let digits = tail
                        .strip_suffix(']')
                        .ok_or_else(|| bad("an index is written `name[n]`"))?;
                    let n: usize = digits
                        .parse()
                        .map_err(|_| bad("an index is a whole number, `name[2]`"))?;
                    if n == 0 {
                        return Err(bad("an index counts from 1"));
                    }
                    (name, Some(n))
                }
            };
            if !is_decl_name(name) {
                return Err(bad(&format!("`{name}` is not a declaration name")));
            }
            segments.push(Segment {
                name: name.to_string(),
                index,
            });
        }
        Ok(TargetPath { anchored, segments })
    }
}

/// An identifier, or `Class.method`.
fn is_decl_name(name: &str) -> bool {
    let ident = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_')
    };
    match name.split_once('.') {
        Some((class, method)) => ident(class) && ident(method),
        None => ident(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(src: &str) -> DeclTree {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).expect("parse");
        DeclTree::build(&stmts)
    }

    fn paths(src: &str) -> Vec<String> {
        let t = tree(src);
        (0..t.decls.len()).map(|i| t.path(i)).collect()
    }

    fn resolve(src: &str, path: &str) -> Result<String, String> {
        let t = tree(src);
        t.resolve(&TargetPath::parse(path)?).map(|i| t.path(i))
    }

    #[test]
    fn paths_follow_enclosing_declarations() {
        let src = "let top = 1\nfn step(dt)\n  let vx = 0\n  fn inner()\n    let k = 1\n  end\n  \
                   let row = fn(i)\n    let out = i\n  end\nend\n";
        assert_eq!(
            paths(src),
            [
                "/top",
                "/step",
                "/step/vx",
                "/step/inner",
                "/step/inner/k",
                "/step/row",
                "/step/row/out"
            ]
        );
    }

    #[test]
    fn control_flow_and_callbacks_are_transparent() {
        let src = "fn f(xs)\n  if true then\n    let a = 1\n  end\n  for x in xs do\n    let b = x\n  end\n  \
                   each(xs, fn(x)\n    let c = x\n  end)\nend\n";
        assert_eq!(paths(src), ["/f", "/f/a", "/f/b", "/f/c"]);
    }

    #[test]
    fn repeats_are_indexed_in_source_order() {
        let src =
            "fn f()\n  let out = 1\n  if true then\n    let out = 2\n  end\n  var out = 3\nend\n";
        assert_eq!(paths(src), ["/f", "/f/out[1]", "/f/out[2]", "/f/out[3]"]);
        assert_eq!(resolve(src, "f/out[2]").unwrap(), "/f/out[2]");
        let err = resolve(src, "f/out").unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
        assert!(err.contains("/f/out[1]  (line 2, `let`)"), "{err}");
        assert!(err.contains("/f/out[3]  (line 6, `var`)"), "{err}");
    }

    #[test]
    fn a_leading_slash_anchors_at_module_level() {
        let src = "let score = 0\nfn step()\n  let score = 1\nend\n";
        assert_eq!(resolve(src, "/score").unwrap(), "/score");
        assert_eq!(resolve(src, "step/score").unwrap(), "/step/score");
        let err = resolve(src, "score").unwrap_err();
        assert!(err.contains("/score  (line 1"), "{err}");
        assert!(err.contains("/step/score  (line 3"), "{err}");
    }

    #[test]
    fn no_match_lists_the_bindings_of_that_name() {
        let src = "fn step()\n  let vx = 1\nend\n";
        let err = resolve(src, "stop/vx").unwrap_err();
        assert!(err.contains("no binding matches"), "{err}");
        assert!(err.contains("/step/vx  (line 2, `let`)"), "{err}");
        let err = resolve(src, "step/vy").unwrap_err();
        assert!(err.contains("no `let` or `state` named `vy`"), "{err}");
        assert!(err.contains("/step/vx"), "{err}");
    }

    #[test]
    fn overloads_and_methods_are_segments() {
        let src = "fn Rect.area(r)\n  let w = 1\nend\nfn b(a)\n  let w = 1\nend\nfn b(a, c)\n  let w = 2\nend\n";
        assert_eq!(resolve(src, "Rect.area/w").unwrap(), "/Rect.area/w");
        assert_eq!(resolve(src, "b[2]/w").unwrap(), "/b[2]/w");
        assert!(resolve(src, "b/w").unwrap_err().contains("ambiguous"));
    }

    #[test]
    fn malformed_paths_are_rejected() {
        for bad in ["", "/", "a//b", "a/", "a[0]", "a[x]", "a[1", "1a", "a b"] {
            assert!(TargetPath::parse(bad).is_err(), "{bad:?} should not parse");
        }
        assert_eq!(TargetPath::parse("/a/b[2]").unwrap().to_string(), "/a/b[2]");
    }
}
