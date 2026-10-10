//! Native interface declarations: what a `native fn` says about a host native.
//!
//! A host registers its natives in a flat table ([`crate::native_fn`]) as bare
//! function pointers: a name, an effect row, and nothing about parameters. A
//! `native fn` declaration is the other half —
//!
//! ```petal ignore
//! native fn sqrt(x: num) -> float
//! native fn slice(collection, start, end?)
//! native fn print(...values)
//! native fn _native_rect(x: num, y: num, w: num, h: num) = "draw_rect"
//! ```
//!
//! — and it is the only place parameter names, arity, parameter types and the
//! result type are written. This module turns the parsed declarations
//! ([`crate::ast::NativeFnDecl`]) into the tables everything else asks:
//!
//! - the compiler binds each declared name to the native's term and puts a
//!   named call in positional order;
//! - the checker reads argument slots and result types
//!   ([`crate::typecheck`]);
//! - an [`Env`](crate::env::Env) declares the parameter lists onto its
//!   registered natives, so a call the compiler could not resolve statically
//!   (`xs.slice(start: 1)`) binds names by the same lists;
//! - loading a program to run checks that every declared native it reaches is
//!   registered ([`NativeLink`]).
//!
//! The builtins every program can call are declared by the bundled `core`
//! package ([`core_decls`]); a host's natives are declared by the interface
//! module the host ships (petal-ui's `ui`).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ast::{NativeFnDecl, Stmt, StmtKind};
use crate::native_fn::NativeSignature;
use crate::typecheck::builtin_types::ArgSlot;
use crate::types::Type;

/// One declared parameter of a native.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeParam {
    pub name: String,
    /// The declared type; `None` when un-annotated or unrecognized (unchecked).
    pub ty: Option<Type>,
    /// Written `name?`: a call may leave it off.
    pub optional: bool,
}

/// One `native fn` declaration: a call form of the native.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeForm {
    pub params: Vec<NativeParam>,
    /// How many leading parameters every call must fill.
    pub required: usize,
    /// Declared with a `...rest` parameter: any number of further arguments.
    pub rest: bool,
    /// The declared result type; `None` leaves it `any`.
    pub ret: Option<Type>,
}

impl NativeForm {
    /// Read a form off its declaration. Types are the built-in vocabulary
    /// ([`Type::from_name`]); any other annotation is unchecked.
    pub fn from_decl(decl: &NativeFnDecl) -> NativeForm {
        NativeForm {
            params: decl
                .params
                .iter()
                .map(|p| NativeParam {
                    name: p.name.clone(),
                    ty: p.ty.as_ref().and_then(|t| t.resolved),
                    optional: p.optional,
                })
                .collect(),
            required: decl.required(),
            rest: decl.rest.is_some(),
            ret: decl.ret.as_ref().and_then(|t| t.resolved),
        }
    }

    /// Whether a call of `count` arguments fits this form.
    pub fn accepts_count(&self, count: usize) -> bool {
        count >= self.required && (self.rest || count <= self.params.len())
    }

    /// The parameter list a named call binds against. `None` for a form with
    /// a rest parameter: its extra arguments have no names, so the native
    /// takes arguments by position only (as a variadic one always has).
    pub fn signature(&self) -> Option<NativeSignature> {
        if self.rest {
            return None;
        }
        Some(NativeSignature::new(
            self.params.iter().map(|p| p.name.clone()).collect(),
            self.required,
        ))
    }

    /// `name(a, b?, ...rest)` — this form, as a message lists it.
    pub fn describe(&self, name: &str) -> String {
        let mut parts: Vec<String> = self
            .params
            .iter()
            .map(|p| {
                if p.optional {
                    format!("{}?", p.name)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        if self.rest {
            parts.push("...".to_string());
        }
        format!("{name}({})", parts.join(", "))
    }
}

/// Everything declared about one name: the host native it binds and its call
/// forms, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeDecl {
    /// The name the host registered the native under.
    pub host: String,
    pub forms: Vec<NativeForm>,
}

impl NativeDecl {
    /// The parameter lists a named call binds against, in declaration order;
    /// empty when every form is variadic (names are then refused).
    pub fn signatures(&self) -> Vec<NativeSignature> {
        self.forms.iter().filter_map(NativeForm::signature).collect()
    }

    /// Whether any form takes a call of `count` arguments.
    pub fn accepts_count(&self, count: usize) -> bool {
        self.forms.iter().any(|f| f.accepts_count(count))
    }

    /// The declared type of parameter `slot` for a call of `count` arguments:
    /// the type every form that takes the call agrees on, `None` when they
    /// differ, when it is un-annotated, or when no form takes the call. A
    /// native that reads its arguments differently by count or by kind
    /// (`text_width(text, style)` / `text_width(text, size, font?)`) is held
    /// only to what all of its readings share.
    pub fn param_type(&self, count: usize, slot: usize) -> Option<Type> {
        let mut agreed: Option<Type> = None;
        let mut any = false;
        for form in self.forms.iter().filter(|f| f.accepts_count(count)) {
            let ty = form.params.get(slot).and_then(|p| p.ty)?;
            match agreed {
                None => agreed = Some(ty),
                Some(prev) if prev == ty => {}
                Some(_) => return None,
            }
            any = true;
        }
        if any { agreed } else { None }
    }

    /// The argument slots of a call of `count` arguments, one per fixed
    /// parameter position the call fills ([`Self::param_type`], as the
    /// checker's [`ArgSlot`]).
    pub fn param_slots(&self, count: usize) -> Vec<ArgSlot> {
        (0..count)
            .map(|slot| match self.param_type(count, slot) {
                Some(ty) => ArgSlot::of(ty),
                None => ArgSlot::Any,
            })
            .collect()
    }

    /// The declared result type of a call of `count` arguments: the one every
    /// form that takes the call agrees on, else `None` (`any`).
    pub fn return_type(&self, count: usize) -> Option<Type> {
        let mut agreed: Option<Type> = None;
        for form in self.forms.iter().filter(|f| f.accepts_count(count)) {
            let ty = form.ret?;
            match agreed {
                None => agreed = Some(ty),
                Some(prev) if prev == ty => {}
                Some(_) => return None,
            }
        }
        agreed
    }
}

/// A set of native declarations, by the name each binds.
#[derive(Debug, Default, Clone)]
pub struct NativeDecls {
    by_name: HashMap<String, NativeDecl>,
}

impl NativeDecls {
    pub fn new() -> NativeDecls {
        NativeDecls::default()
    }

    /// What is declared under `name`.
    pub fn get(&self, name: &str) -> Option<&NativeDecl> {
        self.by_name.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// `(declared name, declaration)`, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &NativeDecl)> {
        self.by_name.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Add one declaration. A second declaration of a name is another call
    /// form of the same native; it is an error for it to name a different
    /// host native, since one name cannot be two functions.
    pub fn add(&mut self, decl: &NativeFnDecl) -> Result<(), String> {
        let form = NativeForm::from_decl(decl);
        let host = decl.host_name();
        match self.by_name.get_mut(&decl.name) {
            Some(existing) if existing.host != host => Err(format!(
                "native fn '{}' is declared for two host natives, '{}' and '{}'",
                decl.name, existing.host, host
            )),
            Some(existing) => {
                existing.forms.push(form);
                Ok(())
            }
            None => {
                self.by_name.insert(
                    decl.name.clone(),
                    NativeDecl {
                        host: host.to_string(),
                        forms: vec![form],
                    },
                );
                Ok(())
            }
        }
    }

    /// The top-level `native fn` declarations of one file. A name declared
    /// for two different host natives keeps the first; the compiler reports
    /// it ([`Self::add`]).
    pub fn collect(stmts: &[Stmt]) -> NativeDecls {
        let mut decls = NativeDecls::new();
        for stmt in stmts {
            if let StmtKind::NativeFn(decl) = &stmt.kind {
                let _ = decls.add(decl);
            }
        }
        decls
    }

    /// Replace-or-insert every name of `other`: a later module's declarations
    /// of a name stand in for an earlier module's.
    pub fn absorb(&mut self, other: &NativeDecls) {
        for (name, decl) in &other.by_name {
            self.by_name.insert(name.clone(), decl.clone());
        }
    }

    /// Host native → the parameter lists these declarations give it. When
    /// several declared names bind one host native their lists are joined,
    /// each name's own forms keeping their order.
    pub fn signatures_by_host(&self) -> HashMap<String, Vec<NativeSignature>> {
        let mut names: Vec<&String> = self.by_name.keys().collect();
        names.sort_unstable();
        let mut by_host: HashMap<String, Vec<NativeSignature>> = HashMap::new();
        for name in names {
            let decl = &self.by_name[name];
            by_host
                .entry(decl.host.clone())
                .or_default()
                .extend(decl.signatures());
        }
        by_host
    }

    /// The host natives these declarations bind, sorted.
    pub fn host_names(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self.by_name.values().map(|d| d.host.clone()).collect();
        hosts.sort_unstable();
        hosts.dedup();
        hosts
    }

    /// Give each registered native the parameter lists these declarations
    /// give it, replacing what it had, and return the declared host names the
    /// table does not register.
    pub fn apply_to(&self, table: &mut crate::native_fn::NativeFnTable) -> Vec<String> {
        let mut missing = Vec::new();
        for (host, signatures) in self.signatures_by_host() {
            match table.lookup_name(&host) {
                Some(id) => table.set_signatures(id, signatures),
                None => missing.push(host),
            }
        }
        missing
    }

    /// Parse `source` and collect its `native fn` declarations — what a host
    /// hands [`Env::declare_natives`](crate::env::Env::declare_natives).
    pub fn parse(source: &str) -> Result<NativeDecls, String> {
        let mut lexer = crate::lexer::Lexer::new(source);
        lexer.tokenize()?;
        let stmts = crate::parse::Parser::new(lexer.tokens, lexer.token_spans).parse_program()?;
        let mut decls = NativeDecls::new();
        for stmt in &stmts {
            if let StmtKind::NativeFn(decl) = &stmt.kind {
                decls.add(decl)?;
            }
        }
        Ok(decls)
    }
}

/// Look `name` up through `layers` (most specific first), then in the bundled
/// `core` package, which is in scope everywhere.
pub fn lookup<'a>(name: &str, layers: &[&'a NativeDecls]) -> Option<&'a NativeDecl> {
    layers
        .iter()
        .find_map(|l| l.get(name))
        .or_else(|| core_decls().get(name))
}

/// The declarations of the bundled `core` package: every builtin a program
/// can call without an import. Parsed once per process from the embedded
/// sources ([`crate::bundled`]).
///
/// These are bound in every file's global scope by construction — the
/// compiler binds each registered native's name there, and a test holds the
/// package to exactly the natives [`crate::builtins::register_builtins`]
/// registers — so `core` costs a program nothing: it is not a module in the
/// program's file table, and no term is emitted for it.
pub fn core_decls() -> &'static NativeDecls {
    static CORE: OnceLock<NativeDecls> = OnceLock::new();
    CORE.get_or_init(|| {
        let package = crate::bundled::package(crate::bundled::CORE_PACKAGE)
            .expect("the `core` package is bundled (core/build.rs)");
        let mut decls = NativeDecls::new();
        for (path, source) in package.sources() {
            let parsed = NativeDecls::parse(source)
                .unwrap_or_else(|e| panic!("bundled package core, {path}: {e}"));
            for (name, decl) in parsed.by_name {
                if decls.by_name.insert(name.clone(), decl).is_some() {
                    panic!("bundled package core, {path}: `{name}` is declared in two modules");
                }
            }
        }
        decls
    })
}

/// One host native a compiled program needs registered before it can run:
/// a native some module of the program declares with `native fn` *and* the
/// program's code refers to — a call or a read anywhere in it, a function
/// body that may never run included. A declaration nothing refers to links
/// nothing, so a host may provide part of an interface to scripts that use
/// only that part. Recorded on the [`Program`](crate::program::Program); the
/// bundled `core` package's natives are left out, being registered by every
/// `Env`.
#[derive(Debug, Clone, PartialEq)]
pub struct NativeLink {
    /// The name the host must have registered.
    pub host: String,
    /// The module that declares it, as an import spells it (`ui`,
    /// `bloom/menu`); the entry file's display name for a declaration in the
    /// entry file.
    pub module: String,
    /// The parameter lists the declaration gives, for checking a named call
    /// to a native this process never registers.
    pub signatures: Vec<NativeSignature>,
}

/// The load error for natives a program declares that the `Env` it is being
/// loaded into does not register: per declaring module, a few names and the
/// count of the rest. `None` when nothing is missing.
pub fn missing_natives_message(
    links: &[NativeLink],
    registered: impl Fn(&str) -> bool,
) -> Option<String> {
    // Declaring module → its missing natives, in declaration order.
    let mut by_module: Vec<(&str, Vec<&str>)> = Vec::new();
    for link in links {
        if registered(&link.host) {
            continue;
        }
        match by_module.iter_mut().find(|(m, _)| *m == link.module) {
            Some((_, names)) => {
                if !names.contains(&link.host.as_str()) {
                    names.push(&link.host);
                }
            }
            None => by_module.push((&link.module, vec![&link.host])),
        }
    }
    if by_module.is_empty() {
        return None;
    }
    const SHOWN: usize = 4;
    let parts: Vec<String> = by_module
        .iter()
        .map(|(module, names)| {
            let shown: Vec<&str> = names.iter().take(SHOWN).copied().collect();
            let more = match names.len().saturating_sub(SHOWN) {
                0 => String::new(),
                n => format!(", +{n} more"),
            };
            format!(
                "module '{module}' needs {} this host does not register: {}{more}",
                if names.len() == 1 {
                    "a native fn"
                } else {
                    "native fns"
                },
                shown.join(", ")
            )
        })
        .collect();
    Some(format!(
        "{} — the program cannot run here; run it with a host that provides {}",
        parts.join("; "),
        if by_module.iter().map(|(_, n)| n.len()).sum::<usize>() == 1 {
            "it"
        } else {
            "them"
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decls(src: &str) -> NativeDecls {
        NativeDecls::parse(src).expect("parses")
    }

    #[test]
    fn a_declaration_gives_names_arity_types_and_result() {
        let d = decls("native fn clampish(value: num, lo: num, hi?: num) -> float");
        let decl = d.get("clampish").unwrap();
        assert_eq!(decl.host, "clampish");
        let form = &decl.forms[0];
        assert_eq!(form.required, 2);
        assert!(form.accepts_count(2) && form.accepts_count(3));
        assert!(!form.accepts_count(1) && !form.accepts_count(4));
        assert_eq!(form.ret, Some(Type::Float));
        assert_eq!(decl.param_type(3, 2), Some(Type::Num));
        let sig = &decl.signatures()[0];
        assert_eq!(sig.params(), ["value", "lo", "hi"]);
        assert_eq!(sig.required(), 2);
    }

    #[test]
    fn a_rest_form_takes_any_count_and_no_names() {
        let d = decls("native fn fmt(template: string, ...args) -> string");
        let decl = d.get("fmt").unwrap();
        assert!(decl.accepts_count(1) && decl.accepts_count(9));
        assert!(!decl.accepts_count(0));
        assert!(decl.signatures().is_empty());
        assert_eq!(decl.param_type(4, 0), Some(Type::String));
        assert_eq!(decl.param_type(4, 1), None);
    }

    #[test]
    fn several_declarations_are_call_forms_of_one_native() {
        let d = decls(
            "native fn width(text: string, style) -> int\n\
             native fn width(text: string, size: num, font?: string) -> int",
        );
        let decl = d.get("width").unwrap();
        assert_eq!(decl.forms.len(), 2);
        // Both forms take two arguments: only what they agree on is checked.
        assert_eq!(decl.param_type(2, 0), Some(Type::String));
        assert_eq!(decl.param_type(2, 1), None);
        assert_eq!(decl.param_type(3, 1), Some(Type::Num));
        assert_eq!(decl.return_type(2), Some(Type::Int));
    }

    #[test]
    fn a_host_name_rebinds_and_must_be_consistent() {
        let d = decls("native fn _rect(x, y) = \"draw_rect\"");
        assert_eq!(d.get("_rect").unwrap().host, "draw_rect");
        let err = NativeDecls::parse("native fn f(x)\nnative fn f(x, y) = \"g\"").unwrap_err();
        assert!(err.contains("two host natives"), "{err}");
    }

    #[test]
    fn the_core_package_parses_and_is_looked_up_last() {
        let core = core_decls();
        assert!(core.get("print").is_some_and(|d| d.forms[0].rest));
        assert_eq!(core.get("len").unwrap().return_type(1), Some(Type::Int));
        assert_eq!(core.get("random").unwrap().forms.len(), 3);
        let local = decls("native fn len(thing) -> string");
        assert_eq!(
            lookup("len", &[&local]).unwrap().return_type(1),
            Some(Type::String)
        );
        assert_eq!(lookup("sqrt", &[&local]).unwrap().host, "sqrt");
        assert!(lookup("nope", &[&local]).is_none());
    }

    #[test]
    fn the_missing_native_message_lists_a_few_then_counts() {
        let link = |host: &str, module: &str| NativeLink {
            host: host.to_string(),
            module: module.to_string(),
            signatures: Vec::new(),
        };
        let links: Vec<NativeLink> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| link(n, "ui"))
            .chain([link("sqrt", "ui")])
            .collect();
        let msg = missing_natives_message(&links, |n| n == "sqrt").unwrap();
        assert!(
            msg.starts_with(
                "module 'ui' needs native fns this host does not register: a, b, c, d, +2 more"
            ),
            "{msg}"
        );
        assert!(missing_natives_message(&links, |_| true).is_none());
    }
}
