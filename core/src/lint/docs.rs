//! The documentation rules (docs/proposals/doc-annotation.md): they read
//! `@doc(…)` and `@module_doc(…)`, which the parser keeps on the AST, and
//! report only.
//!
//! - **`doc-params`**: a `params:` key that no form of the overload set takes
//!   (`@doc` describes `lo`, which `clamp` does not take), and a `see:` entry
//!   that names nothing reachable from the file: not a declaration of the
//!   file, a name an import brings in, a builtin, or a module or package (and
//!   its export, `"ui.button"`, `"ui/drag.vary"`).
//! - **`undocumented-pub`**: a `pub` item with no `@doc` in any form of its
//!   overload set, and a package's root module with no `@module_doc`.
//!   Opt-in (`--rules-enable`, `--rules-include` or a project's `[lint]
//!   enable`).
//! - **`deprecated`**: a use of an item whose `@doc` has `deprecated:`,
//!   whether it is declared in this file, in a module the file imports (or
//!   that one re-exports with `pub import`), or is a builtin. The finding
//!   carries the doc's replacement text.
//! - **`package-description`**: a package's `petal.toml` `description` that
//!   disagrees with the first sentence of its root module's `@module_doc`.
//!
//! Imports resolve the way `petal run` resolves them (the include dirs and
//! the file's project); one that does not resolve is the compile check's to
//! report, and the rules read past it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use crate::ast::{Doc, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr};
use crate::module::{Importer, ModuleOrigin, ModuleResolver};

use super::util::bound_names;
use super::{Fix, LintOptions};

/// One declaration of a name: a form of its overload set.
#[derive(Clone, Debug)]
struct Form {
    /// The parameter names, a native's `...rest` included.
    params: Vec<String>,
    /// How many arguments a call must pass, and may pass (`None`: any number).
    required: usize,
    most: Option<usize>,
    documented: bool,
    deprecated: Option<String>,
}

impl Form {
    fn takes(&self, argc: usize) -> bool {
        argc >= self.required && self.most.is_none_or(|m| argc <= m)
    }
}

/// What a module declares, by name.
#[derive(Default, Debug)]
struct Surface {
    items: HashMap<String, Vec<Form>>,
    /// Names a `see:` may point at that are not items of their own: class
    /// fields and enum variants (`Rect.x`, `Dir.Up`).
    members: HashSet<String>,
}

impl Surface {
    fn has(&self, name: &str) -> bool {
        self.items.contains_key(name) || self.members.contains(name)
    }

    /// Add `other`'s items, or only those named in `only`.
    fn absorb(&mut self, other: &Surface, only: Option<&[String]>) {
        for (name, forms) in &other.items {
            if only.is_none_or(|names| names.contains(name)) {
                self.items.entry(name.clone()).or_default().extend(forms.iter().cloned());
            }
        }
        for member in &other.members {
            let owner = member.split('.').next().unwrap_or(member);
            if only.is_none_or(|names| names.iter().any(|n| n == owner)) {
                self.members.insert(member.clone());
            }
        }
    }
}

/// The declaration `s` binds, as a form: its name, and the form.
fn form_of(s: &Stmt) -> Option<(&str, Form)> {
    let doc = s.doc.as_deref();
    let form = |params: Vec<String>, required: usize, most: Option<usize>| Form {
        params,
        required,
        most,
        documented: doc.is_some(),
        deprecated: doc.and_then(|d| d.deprecated.clone()),
    };
    let names = |params: &[crate::ast::Param]| params.iter().map(|p| p.name.clone()).collect();
    Some(match &s.kind {
        StmtKind::FnDecl { name, params, .. } if s.test.is_none() => (
            name,
            form(names(params), crate::ast::required_param_count(params), Some(params.len())),
        ),
        StmtKind::NativeFn(decl) => {
            let shape = crate::native_decl::NativeForm::from_decl(decl);
            let mut params: Vec<String> = names(&decl.params);
            params.extend(decl.rest.iter().cloned());
            let most = (!shape.rest).then_some(decl.params.len());
            (&decl.name, form(params, shape.required, most))
        }
        StmtKind::ClassDecl { name, fields } => {
            let params: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
            let n = params.len();
            (name, form(params, 0, Some(n)))
        }
        StmtKind::Let { name, value, .. } => match &value.kind {
            ExprKind::Lambda { params, .. } => (
                name,
                form(names(params), crate::ast::required_param_count(params), Some(params.len())),
            ),
            _ => (name, form(Vec::new(), 0, None)),
        },
        StmtKind::EnumDecl { name, .. }
        | StmtKind::State { name, .. }
        | StmtKind::Memo { name, .. } => (name, form(Vec::new(), 0, None)),
        _ => return None,
    })
}

/// What `stmts` declare at top level; the `pub` ones only when
/// `exported_only`.
fn surface_of(stmts: &[Stmt], exported_only: bool) -> Surface {
    let mut out = Surface::default();
    for s in stmts {
        if exported_only && !s.exported {
            continue;
        }
        let Some((name, form)) = form_of(s) else {
            continue;
        };
        match &s.kind {
            StmtKind::ClassDecl { fields, .. } => {
                out.members.extend(fields.iter().map(|f| format!("{name}.{}", f.name)));
            }
            StmtKind::EnumDecl { variants, .. } => {
                out.members.extend(variants.iter().map(|v| format!("{name}.{}", v.name)));
            }
            _ => {}
        }
        out.items.entry(name.to_string()).or_default().push(form);
    }
    out
}

/// The deprecation that applies to a use of the overload set `forms` with
/// `argc` arguments (`None`: not a call, or a spread). When one form of the
/// set has a `@doc`, it describes the set. When several do, each describes
/// its own form: the use is deprecated when every form it can reach is.
fn deprecation(forms: &[Form], argc: Option<usize>) -> Option<&str> {
    let documented: Vec<&Form> = forms.iter().filter(|f| f.documented).collect();
    if documented.len() == 1 {
        return documented[0].deprecated.as_deref();
    }
    let reached: Vec<&Form> = match argc {
        Some(n) => forms.iter().filter(|f| f.takes(n)).collect(),
        None => Vec::new(),
    };
    let reached = if reached.is_empty() { forms.iter().collect() } else { reached };
    if reached.iter().all(|f| f.deprecated.is_some()) {
        reached.first()?.deprecated.as_deref()
    } else {
        None
    }
}

fn parse(source: &str) -> Option<Vec<Stmt>> {
    crate::cst::parse_source(source, crate::source_map::ENTRY_FILE)
        .ok()
        .map(|(_, stmts)| stmts)
}

/// What every file sees with no import: the `core` package's natives and
/// the standard prelude.
fn builtins() -> &'static Surface {
    static BUILTINS: OnceLock<Surface> = OnceLock::new();
    BUILTINS.get_or_init(|| {
        let mut out = Surface::default();
        if let Some(core) = crate::bundled::package(crate::bundled::CORE_PACKAGE) {
            for (_, source) in core.sources() {
                if let Some(stmts) = parse(source) {
                    out.absorb(&surface_of(&stmts, true), None);
                }
            }
        }
        if let Some(stmts) = parse(crate::env::STD_PRELUDE) {
            out.absorb(&surface_of(&stmts, true), None);
        }
        out
    })
}

/// A module, as the resolver needs it to resolve the imports written in it.
#[derive(Clone)]
struct ModuleAt {
    name: Option<String>,
    origin: Option<ModuleOrigin>,
}

/// Resolves import paths and reads the modules they name, each once. The
/// `Env` it resolves through is made on the first lookup, so a file with no
/// imports and nothing to resolve costs nothing.
struct Resolver<'a> {
    opts: &'a LintOptions,
    env: Option<crate::env::Env>,
    surfaces: HashMap<String, Option<Rc<Surface>>>,
    visiting: HashSet<String>,
}

impl<'a> Resolver<'a> {
    fn new(opts: &'a LintOptions) -> Self {
        Resolver {
            opts,
            env: None,
            surfaces: HashMap::new(),
            visiting: HashSet::new(),
        }
    }

    fn registry(&mut self) -> &crate::module::ModuleRegistry {
        let opts = self.opts;
        self.env
            .get_or_insert_with(|| {
                let mut env = crate::env::Env::new();
                for dir in &opts.include_dirs {
                    env.add_module_path(dir.clone());
                }
                env.module_registry().begin_load(opts.origin.as_deref());
                env
            })
            .module_registry()
    }

    /// The file being linted, as an importer.
    fn entry(&mut self) -> ModuleAt {
        let origin = self.opts.origin.clone();
        let name = origin
            .as_deref()
            .and_then(|p| self.registry().module_name_of_file(p));
        ModuleAt {
            name,
            origin: origin.map(ModuleOrigin::File),
        }
    }

    /// The module `path` names, written in `from`, and what it exports: its
    /// own `pub` items and what its `pub import`s re-export.
    fn resolve(&mut self, path: &str, from: &ModuleAt) -> Option<(ModuleAt, Rc<Surface>)> {
        let importer = Importer {
            name: from.name.as_deref(),
            origin: from.origin.as_ref(),
        };
        let found = self.registry().resolve(path, &importer).ok()?;
        let key = match (&found.name, &found.origin) {
            (Some(name), _) => name.clone(),
            (None, ModuleOrigin::File(p)) => p.display().to_string(),
            (None, ModuleOrigin::Memory) => path.to_string(),
        };
        let at = ModuleAt {
            name: found.name.clone(),
            origin: Some(found.origin.clone()),
        };
        if let Some(known) = self.surfaces.get(&key) {
            return known.clone().map(|s| (at, s));
        }
        if !self.visiting.insert(key.clone()) {
            return None; // an import cycle: the compile check reports it
        }
        let surface = parse(&found.source).map(|stmts| {
            let mut surface = surface_of(&stmts, true);
            for s in &stmts {
                if let StmtKind::Import(import) = &s.kind
                    && import.exported
                    && let Some((_, inner)) = self.resolve(&import.module, &at)
                {
                    surface.absorb(&inner, import.names.as_deref());
                }
            }
            Rc::new(surface)
        });
        self.visiting.remove(&key);
        self.surfaces.insert(key, surface.clone());
        surface.map(|s| (at, s))
    }
}

/// One import of the linted file, resolved.
struct Bound {
    /// The name qualified access goes through (`ui` in `ui.button`); `None`
    /// for a selective import, which binds none.
    qualifier: Option<String>,
    /// The names bound bare: every export (`None`), or the ones listed.
    names: Option<Vec<String>>,
    surface: Rc<Surface>,
}

/// What the linted file can name.
struct Scope<'a> {
    local: Surface,
    imports: Vec<Bound>,
    resolver: Resolver<'a>,
    entry: Option<ModuleAt>,
}

impl<'a> Scope<'a> {
    fn new(stmts: &[Stmt], opts: &'a LintOptions) -> Self {
        let mut scope = Scope {
            local: surface_of(stmts, false),
            imports: Vec::new(),
            resolver: Resolver::new(opts),
            entry: None,
        };
        for s in stmts {
            let StmtKind::Import(import) = &s.kind else {
                continue;
            };
            let entry = scope.entry();
            let Some((_, surface)) = scope.resolver.resolve(&import.module, &entry) else {
                continue;
            };
            let qualifier = import.names.is_none().then(|| {
                import
                    .alias
                    .clone()
                    .unwrap_or_else(|| crate::ast::module_local_name(&import.module).to_string())
            });
            scope.imports.push(Bound {
                qualifier,
                names: import.names.clone(),
                surface,
            });
        }
        scope
    }

    fn entry(&mut self) -> ModuleAt {
        if self.entry.is_none() {
            self.entry = Some(self.resolver.entry());
        }
        self.entry.clone().expect("just set")
    }

    /// The overload set a bare `name` reaches: the file's own declaration,
    /// else an import's, else a builtin.
    fn bare(&self, name: &str) -> Option<&[Form]> {
        if let Some(forms) = self.local.items.get(name) {
            return Some(forms);
        }
        for bound in &self.imports {
            if bound.names.as_ref().is_none_or(|n| n.iter().any(|n| n == name))
                && let Some(forms) = bound.surface.items.get(name)
            {
                return Some(forms);
            }
        }
        builtins().items.get(name).map(Vec::as_slice)
    }

    fn qualified(&self, qualifier: &str, name: &str) -> Option<&[Form]> {
        self.imports
            .iter()
            .filter(|b| b.qualifier.as_deref() == Some(qualifier))
            .find_map(|b| b.surface.items.get(name))
            .map(Vec::as_slice)
    }

    /// Does a `see:` entry name something reachable from this file?
    fn reaches(&mut self, entry: &str) -> bool {
        let named_bare = self.local.has(entry)
            || builtins().has(entry)
            || self.imports.iter().any(|b| {
                b.qualifier.as_deref() == Some(entry)
                    || (b.names.as_ref().is_none_or(|n| n.iter().any(|n| n == entry))
                        && b.surface.has(entry))
            });
        if named_bare {
            return true;
        }
        // `module.item`: the module part runs to the first `.` after the
        // last `/` (`ui/drag.vary`, `ui.button`, `Rect.inset`).
        let tail_from = entry.rfind('/').map_or(0, |i| i + 1);
        let (module, item) = match entry[tail_from..].find('.') {
            Some(dot) => (&entry[..tail_from + dot], Some(&entry[tail_from + dot + 1..])),
            None => (entry, None),
        };
        if let Some(item) = item
            && !module.contains('/')
        {
            let through_import: Vec<bool> = self
                .imports
                .iter()
                .filter(|b| b.qualifier.as_deref() == Some(module))
                .map(|b| b.surface.has(item))
                .collect();
            if !through_import.is_empty() {
                return through_import.contains(&true);
            }
        }
        let from = self.entry();
        match self.resolver.resolve(module, &from) {
            Some((_, surface)) => item.is_none_or(|i| surface.has(i)),
            None => false,
        }
    }
}

/// What the doc rules found, one list per rule.
#[derive(Default)]
pub(super) struct DocReports {
    pub(super) params: Vec<Fix>,
    pub(super) undocumented: Vec<Fix>,
    pub(super) deprecated: Vec<Fix>,
    pub(super) description: Vec<Fix>,
}

/// Run the doc rules `enabled` says to over the file's statements and its
/// `@module_doc`.
pub(super) fn plan_doc_reports(
    chars: &[char],
    stmts: &[Stmt],
    module_doc: Option<&Doc>,
    opts: &LintOptions,
    enabled: impl Fn(&str) -> bool,
) -> DocReports {
    let mut out = DocReports::default();
    // Resolving the imports is the costly part, and only these two need it.
    if enabled(super::DOC_PARAMS) || enabled(super::DEPRECATED) {
        let mut scope = Scope::new(stmts, opts);
        if enabled(super::DOC_PARAMS) {
            if let Some(doc) = module_doc {
                check_see(chars, doc, &mut scope, &mut out.params);
            }
            check_block(chars, stmts, &mut scope, &mut out.params);
        }
        if enabled(super::DEPRECATED) {
            out.deprecated = plan_deprecated(stmts, &scope);
        }
    }
    let package = opts.origin.as_deref().and_then(package_root);
    if enabled(super::UNDOCUMENTED_PUB) {
        out.undocumented = plan_undocumented(stmts);
        if let Some(pkg) = &package
            && pkg.is_root
            && module_doc.is_none()
        {
            out.undocumented.insert(
                0,
                Fix {
                    anchor: 0,
                    message: format!(
                        "this is the root module of package `{}`, and it has no `@module_doc`: \
                         the module doc is the package's doc",
                        pkg.name
                    ),
                    splices: Vec::new(),
                },
            );
        }
    }
    if enabled(super::PACKAGE_DESCRIPTION)
        && let Some(pkg) = &package
        && pkg.is_root
        && let Some(description) = &pkg.description
        && let Some(doc) = module_doc
        && let Some(summary) = doc.summary()
        && comparable(description) != comparable(&summary)
    {
        out.description.push(Fix {
            anchor: doc.span.start.offset as usize,
            message: format!(
                "petal.toml's description \"{description}\" disagrees with the first sentence \
                 of the `@module_doc`, \"{summary}\": make them say the same"
            ),
            splices: Vec::new(),
        });
    }
    out
}

/// `params:` and `see:` of every doc in `stmts` and the function bodies in
/// them. A `params:` key may name a parameter of any form of the overload
/// set, which is the declarations of that name in the same block.
fn check_block(chars: &[char], stmts: &[Stmt], scope: &mut Scope, out: &mut Vec<Fix>) {
    let block = surface_of(stmts, false);
    for s in stmts {
        if let Some(doc) = s.doc.as_deref() {
            check_see(chars, doc, scope, out);
            if let Some((name, _)) = form_of(s)
                && takes_params(s)
            {
                let forms = block.items.get(name).map(Vec::as_slice).unwrap_or(&[]);
                let taken: Vec<&str> =
                    forms.iter().flat_map(|f| f.params.iter().map(String::as_str)).collect();
                for (key, _) in &doc.params {
                    if taken.contains(&key.as_str()) {
                        continue;
                    }
                    // A one- or two-letter name is a step from too many others.
                    let budget = match key.chars().count() {
                        0..=2 => 0,
                        3..=4 => 1,
                        _ => 2,
                    };
                    let hint = (budget > 0)
                        .then(|| crate::diagnostic::closest(key, taken.iter().copied(), budget))
                        .flatten()
                        .map(|near| format!("; did you mean `{near}`?"))
                        .unwrap_or_default();
                    out.push(Fix {
                        anchor: locate(chars, doc, "params", key, |c| c.is_alphanumeric() || c == '_'),
                        message: format!("`@doc` describes `{key}`, which `{name}` does not take{hint}"),
                        splices: Vec::new(),
                    });
                }
            }
        }
        if let StmtKind::FnDecl { body, .. } = &s.kind {
            check_block(chars, body, scope, out);
        }
    }
}

/// Whether a `params:` on `s` describes parameters it has: a function, a
/// native, a class (its constructor takes the fields) or a `let` of a
/// lambda. On any other `let` the value may still be a function, so its
/// keys are not checked.
fn takes_params(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::FnDecl { .. } | StmtKind::NativeFn(_) | StmtKind::ClassDecl { .. } => true,
        StmtKind::Let { value, .. } => matches!(value.kind, ExprKind::Lambda { .. }),
        _ => false,
    }
}

fn check_see(chars: &[char], doc: &Doc, scope: &mut Scope, out: &mut Vec<Fix>) {
    for entry in &doc.see {
        if !scope.reaches(entry) {
            out.push(Fix {
                anchor: locate(chars, doc, "see", entry, |c| c != '"'),
                message: format!(
                    "`see:` names `{entry}`, which is nothing this file can reach: not one of \
                     its declarations, a name its imports bring in, a builtin, or a module"
                ),
                splices: Vec::new(),
            });
        }
    }
}

/// Where `doc` writes `word` inside its `field:` argument (a `params:` key,
/// a `see:` entry), so that each finding has a place of its own: the first
/// occurrence after the field's name not bordered by a character `part`
/// says continues it. The doc's `@` when it is not found.
fn locate(chars: &[char], doc: &Doc, field: &str, word: &str, part: impl Fn(char) -> bool) -> usize {
    let start = doc.span.start.offset as usize;
    let end = (doc.span.end.offset as usize).min(chars.len());
    if start >= end {
        return start;
    }
    let text = &chars[start..end];
    let find = |needle: &[char], from: usize| {
        (from..text.len().saturating_sub(needle.len() - 1)).find(|&i| {
            text[i..].starts_with(needle)
                && (i == 0 || !part(text[i - 1]))
                && text.get(i + needle.len()).is_none_or(|c| !part(*c))
        })
    };
    let field: Vec<char> = field.chars().collect();
    let word: Vec<char> = word.chars().collect();
    if word.is_empty() {
        return start;
    }
    let after = find(&field, 0).map_or(0, |i| i + field.len());
    find(&word, after).map_or(start, |i| start + i)
}

/// `pub` items with no `@doc` on any form of their overload set, each
/// reported once, at its first form.
fn plan_undocumented(stmts: &[Stmt]) -> Vec<Fix> {
    let mut documented: HashSet<&str> = HashSet::new();
    for s in stmts {
        if s.exported
            && s.doc.is_some()
            && let Some((name, _)) = form_of(s)
        {
            documented.insert(name);
        }
    }
    let mut reported: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for s in stmts {
        if !s.exported {
            continue;
        }
        let Some((name, _)) = form_of(s) else {
            continue;
        };
        if documented.contains(name) || !reported.insert(name) {
            continue;
        }
        out.push(Fix {
            anchor: s.span.start.offset as usize,
            message: format!("`pub {} {name}` has no `@doc`", kind_word(s)),
            splices: Vec::new(),
        });
    }
    out
}

fn kind_word(s: &Stmt) -> &'static str {
    match &s.kind {
        StmtKind::FnDecl { .. } => "fn",
        StmtKind::NativeFn(_) => "native fn",
        StmtKind::ClassDecl { .. } => "class",
        StmtKind::EnumDecl { .. } => "enum",
        StmtKind::Let { is_var: true, .. } => "var",
        StmtKind::Let { is_const: true, .. } => "const",
        StmtKind::Let { .. } => "let",
        StmtKind::State { .. } => "state",
        StmtKind::Memo { .. } => "memo",
        _ => "item",
    }
}

/// Every use of a deprecated item in `stmts`. A use inside a declaration
/// that is deprecated itself is not reported: old code that calls old code
/// goes away with it.
fn plan_deprecated(stmts: &[Stmt], scope: &Scope) -> Vec<Fix> {
    struct Uses<'s, 'a> {
        scope: &'s Scope<'a>,
        shadowed: HashSet<String>,
        out: Vec<Fix>,
    }
    impl Uses<'_, '_> {
        fn report(&mut self, written: String, text: Option<&str>, e: &Expr) {
            if let Some(text) = text {
                self.out.push(Fix {
                    anchor: e.span.start.offset as usize,
                    message: format!("`{written}` is deprecated: {}", text.trim()),
                    splices: Vec::new(),
                });
            }
        }

        /// A use of `f` (a name, or `q.name` through an import), called with
        /// `argc` arguments when it is called.
        fn check(&mut self, f: &Expr, argc: Option<usize>) -> bool {
            match &f.kind {
                ExprKind::Ident(name) | ExprKind::CellGet(name) => {
                    if !self.shadowed.contains(name) {
                        let text = self.scope.bare(name).and_then(|fs| deprecation(fs, argc));
                        self.report(name.clone(), text, f);
                    }
                    true
                }
                ExprKind::FieldAccess { object, field } => {
                    let ExprKind::Ident(q) = &object.kind else {
                        return false;
                    };
                    if self.shadowed.contains(q) || self.scope.local.has(q) {
                        return false;
                    }
                    let text = self.scope.qualified(q, field).and_then(|fs| deprecation(fs, argc));
                    let reported = text.is_some();
                    self.report(format!("{q}.{field}"), text, f);
                    reported
                }
                _ => false,
            }
        }
    }
    impl ExprVisitor for Uses<'_, '_> {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Call {
                    function,
                    args,
                    arg_names,
                } => {
                    let spread = arg_names.iter().flatten().any(|n| n == crate::ast::SPREAD_ARG);
                    let argc = (!spread).then_some(args.len());
                    if !self.check(function, argc) {
                        self.visit_expr(function);
                    }
                    for a in args {
                        self.visit_expr(a);
                    }
                }
                ExprKind::Ident(_) | ExprKind::CellGet(_) => {
                    self.check(e, None);
                }
                ExprKind::FieldAccess { .. } => {
                    if !self.check(e, None) {
                        walk_expr(self, e);
                    }
                }
                _ => walk_expr(self, e),
            }
        }
    }

    let mut uses = Uses {
        scope,
        shadowed: HashSet::new(),
        out: Vec::new(),
    };
    for s in stmts {
        let own = form_of(s).map(|(name, _)| name);
        let deprecated_itself = s.doc.as_ref().is_some_and(|d| d.deprecated.is_some())
            || own
                .and_then(|name| scope.local.items.get(name))
                .is_some_and(|forms| deprecation(forms, None).is_some());
        if deprecated_itself {
            continue;
        }
        // The names the statement binds inside itself (parameters, locals)
        // hide a deprecated item of the same name there.
        let mut shadowed = bound_names(std::slice::from_ref(s));
        if let Some(name) = own {
            shadowed.remove(name);
        }
        uses.shadowed = shadowed;
        uses.visit_stmt(s);
    }
    uses.out
}

/// The package a linted file belongs to, as the doc rules need it.
struct PackageRoot {
    name: String,
    description: Option<String>,
    /// The file is the package's root module (`ui.ptl` in package `ui`),
    /// whose `@module_doc` is the package's doc.
    is_root: bool,
}

fn package_root(file: &Path) -> Option<PackageRoot> {
    let manifest = crate::package::find_manifest(file)?;
    let dir = manifest.parent()?;
    let m = crate::package::Manifest::load(dir).ok()?;
    let module_dir: PathBuf = match &m.modules {
        Some(rel) => dir.join(rel),
        None if dir.join("src").is_dir() => dir.join("src"),
        None => dir.to_path_buf(),
    };
    let root = module_dir.join(format!("{}.ptl", m.name));
    let canonical = crate::module::canonical_path;
    Some(PackageRoot {
        is_root: canonical(file) == canonical(&root),
        name: m.name,
        description: m.description,
    })
}

/// A sentence as the description check compares it: case, spacing, inline
/// code marks and the closing stop do not count.
fn comparable(text: &str) -> String {
    let words = text.replace('`', "").split_whitespace().collect::<Vec<_>>().join(" ");
    words
        .trim_end_matches(['.', '!', '?'])
        .to_lowercase()
}
