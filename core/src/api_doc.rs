//! `petal doc`: a module's, a package's or a file's public surface, read
//! from source by the parser alone (docs/proposals/doc-annotation.md, step 2).
//!
//! Signatures come from the declarations; prose comes from `@doc(…)` and
//! `@module_doc(…)`. Nothing is compiled or run, so a library that needs a
//! host (`ui`, `bloom`) is documented as easily as one that does not. A
//! comment is never documentation: an item with no `@doc` has `doc: null`.
//!
//! The JSON shape is [`DocReport`], described for consumers by
//! `docs/schemas/petal-doc.schema.json` (and CLI.md, `petal doc`).
//! [`render_brief`] and [`render_text`] are the two human-readable forms.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ast::{Doc, DocValue, Expr, Param, Stmt, StmtKind, TypeAnn};
use crate::env::Env;
use crate::package::Package;
use crate::source_map::ENTRY_FILE;

/// The `schema` tag of every report. Bumped when a consumer would have to
/// change how it reads one.
pub const SCHEMA: &str = "petal-doc/1";

/// What to include.
#[derive(Debug, Clone, Copy, Default)]
pub struct DocOptions {
    /// `--private`: items written without `pub`, too.
    pub private: bool,
}

/// What `petal doc --json` prints.
#[derive(Debug, Clone, Serialize)]
pub struct DocReport {
    /// [`SCHEMA`].
    pub schema: &'static str,
    /// The argument as given (`ui`, `bloom/menu`, `lib/util.ptl`).
    pub target: String,
    /// The package the documented modules belong to; `null` for a loose file
    /// or a module registered in memory (the core prelude `std`).
    pub package: Option<PackageApi>,
    /// The documented modules: every module of a package, in module order,
    /// or the one module asked for.
    pub modules: Vec<ModuleApi>,
}

/// A package's own facts.
#[derive(Debug, Clone, Serialize)]
pub struct PackageApi {
    pub name: String,
    pub version: Option<String>,
    /// `[package] description` in `petal.toml`: the one-line registry
    /// summary.
    pub description: Option<String>,
    /// The package root: a directory, or `<bundled:NAME>` /
    /// `<package:NAME>` for a package held in memory.
    pub root: String,
    /// The module named like the package (`ui`, `core`), whose
    /// `@module_doc` is the package's doc; `null` when there is none.
    pub root_module: Option<String>,
    /// The root module's `@module_doc`.
    pub doc: Option<DocJson>,
    /// Every module of the package, by canonical name.
    pub modules: Vec<String>,
}

/// One module's surface.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleApi {
    /// Its canonical name (`ui/draw`, `bloom`), or the path given for a
    /// loose file.
    pub name: String,
    /// Its file: relative to the package root for a package's module, as
    /// given for a loose file, `<memory:NAME>` for one registered in memory.
    pub file: String,
    /// Its `@module_doc`.
    pub doc: Option<DocJson>,
    /// Its `pub import`s: the surface it re-presents from other modules.
    pub reexports: Vec<Reexport>,
    /// Its items, in source order (an overload set where its first form is).
    pub items: Vec<Item>,
    /// Why the module could not be read (it does not parse); its other
    /// fields are then empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One `pub import`.
#[derive(Debug, Clone, Serialize)]
pub struct Reexport {
    /// The path as written (`ui/draw`, `./motion`).
    pub module: String,
    /// The module it names, by canonical name: a relative path resolved
    /// against the re-exporting module of a package (`./motion` in
    /// `bloom/menu` is `bloom/motion`), a package path as written. `null`
    /// for a relative path in a loose file.
    pub canonical: Option<String>,
    /// The names it re-exports; `null` for all of them.
    pub names: Option<Vec<String>>,
}

/// One documented declaration: a function (all of its call forms), a class,
/// an enum or a top-level binding.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// `fn`, `native_fn` (every form is a `native fn`), `class`, `enum`,
    /// `let`, `const`, `var`, `state`, `state_var` or `memo`.
    pub kind: &'static str,
    /// The name it binds; `Class.method` for a method.
    pub name: String,
    /// Written `pub` (always true unless `--private`).
    pub exported: bool,
    /// The doc says `hidden: true`: left out of `--brief` and generated
    /// reference pages.
    pub hidden: bool,
    /// The line of its (first) declaration keyword, 1-based, in the
    /// module's file.
    pub line: u32,
    /// The doc of the item: for an overload set, the doc of its first
    /// documented form.
    pub doc: Option<DocJson>,
    /// For a method (`fn Rect.area`), the class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// How it is called: each overload of a function, or a class's
    /// constructor. Absent for an enum or a binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forms: Option<Vec<Form>>,
    /// A class's fields, in constructor order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldApi>>,
    /// An enum's variants.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<VariantApi>>,
    /// A binding's declared type, when written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// A binding's initializer as written, when it fits on one line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// One call form.
#[derive(Debug, Clone, Serialize)]
pub struct Form {
    pub params: Vec<ParamApi>,
    /// The declared result type (`-> int`), when written.
    pub returns: Option<String>,
    /// A `native fn` declaration: the host implements it.
    pub native: bool,
    /// For a native bound under another name (`= "draw_rect"`), that name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The line of this form's declaration keyword.
    pub line: u32,
    /// This form's own `@doc`.
    pub doc: Option<DocJson>,
}

/// One parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamApi {
    pub name: String,
    /// The declared type, when written.
    pub r#type: Option<String>,
    /// A call may leave it out: a default value, a native's `name?`, or a
    /// rest parameter.
    pub optional: bool,
    /// The default value's source text (`fn f(a, b = 2)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// A native's `...name`: any number of further arguments.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rest: bool,
}

/// One class field.
#[derive(Debug, Clone, Serialize)]
pub struct FieldApi {
    pub name: String,
    pub r#type: Option<String>,
}

/// One enum variant.
#[derive(Debug, Clone, Serialize)]
pub struct VariantApi {
    pub name: String,
    /// Its payload fields (`Circle(r)`).
    pub fields: Vec<String>,
    /// The keys of its constant data (`Red(hex: "#f00")`).
    pub data: Vec<String>,
}

/// A doc as tools read it: every field of the annotation, the body and the
/// examples with their indentation cleaned, and the summary.
#[derive(Debug, Clone, Serialize)]
pub struct DocJson {
    /// The Markdown body, cleaned ([`crate::doc_annotation::cleandoc`]).
    pub body: Option<String>,
    /// The body's first sentence ([`crate::doc_annotation::summary`]).
    pub summary: Option<String>,
    /// `params:`, keyed by parameter name, in source order.
    pub params: serde_json::Map<String, serde_json::Value>,
    pub returns: Option<String>,
    /// `options:`, keyed by option name, in source order.
    pub options: serde_json::Map<String, serde_json::Value>,
    /// `example:`, each entry cleaned.
    pub example: Vec<String>,
    pub see: Vec<String>,
    pub deprecated: Option<String>,
    pub hidden: bool,
    /// `extra:`, passed through as JSON.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl DocJson {
    pub fn of(doc: &Doc) -> DocJson {
        let strings = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect()
        };
        DocJson {
            body: doc.cleaned_body(),
            summary: doc.summary(),
            params: strings(&doc.params),
            returns: doc.returns.clone(),
            options: strings(&doc.options),
            example: doc.cleaned_examples(),
            see: doc.see.clone(),
            deprecated: doc.deprecated.clone(),
            hidden: doc.hidden,
            extra: doc
                .extra
                .iter()
                .map(|(k, v)| (k.clone(), doc_value_json(v)))
                .collect(),
        }
    }
}

/// An `extra:` literal as JSON.
fn doc_value_json(value: &DocValue) -> serde_json::Value {
    use serde_json::Value;
    match value {
        DocValue::Nil => Value::Null,
        DocValue::Bool(b) => Value::Bool(*b),
        DocValue::Int(n) => Value::from(*n),
        DocValue::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        DocValue::String(s) => Value::String(s.clone()),
        DocValue::List(items) => Value::Array(items.iter().map(doc_value_json).collect()),
        DocValue::Record(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), doc_value_json(v)))
                .collect(),
        ),
    }
}

// ── resolving the target ──────────────────────────────────────────────

/// Document `target`: a file or package directory when it names one on
/// disk, else a package (`ui`), one module of a package (`ui/input`) or a
/// module registered in memory (`std`), found the way an import in a file
/// in `cwd` would find it.
pub fn document(
    env: &Env,
    target: &str,
    cwd: &Path,
    opts: DocOptions,
) -> Result<DocReport, String> {
    let path = Path::new(target);
    // A directory that is not a package (`core/` at the repository root)
    // does not hide the package of that name.
    let package_dir = path.is_dir() && path.join(crate::package::MANIFEST_FILE).is_file();
    let (package, modules) = if package_dir {
        let package = Package::from_dir(path).map_err(|e| e.to_string())?;
        let modules = package_modules(&package, None, opts);
        (Some(package_api(&package)), modules)
    } else if path.is_file() {
        document_file(env, path, target, opts)?
    } else {
        // A name, as an import spells it.
        let probe = cwd.join("<petal doc>.ptl");
        let (head, rest) = match target.split_once('/') {
            Some((head, rest)) => (head, Some(rest)),
            None => (target, None),
        };
        if let Some(source) = env.memory_module(target) {
            let module = document_source(target, &format!("<memory:{target}>"), source, opts);
            (None, vec![module])
        } else if let Some(package) = env.find_package(head, Some(&probe)) {
            let modules = match rest {
                Some(rest) => {
                    if !package.info.modules.iter().any(|m| m == rest) {
                        return Err(format!(
                            "package '{head}' has no module '{rest}'; its modules are: {}",
                            package.info.modules.join(", ")
                        ));
                    }
                    package_modules(&package, Some(rest), opts)
                }
                None => package_modules(&package, None, opts),
            };
            (Some(package_api(&package)), modules)
        } else if path.is_dir() {
            return Err(format!(
                "'{target}' is a directory with no {} and no package is called that: give a \
                 package directory, a .ptl file, or a package or module name",
                crate::package::MANIFEST_FILE
            ));
        } else {
            return Err(format!(
                "no file, package or module named '{target}' (see `petal packages` for the \
                 packages available)"
            ));
        }
    };
    Ok(DocReport {
        schema: SCHEMA,
        target: target.to_string(),
        package,
        modules,
    })
}

/// A `.ptl` file: a module of the package it is in, or a loose file.
fn document_file(
    env: &Env,
    path: &Path,
    target: &str,
    opts: DocOptions,
) -> Result<(Option<PackageApi>, Vec<ModuleApi>), String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read '{target}': {e}"))?;
    let package = env.package_of_file(path)?;
    let in_package = package.as_ref().and_then(|p| {
        let canonical = crate::module::canonical_path(path);
        let inside = canonical
            .strip_prefix(crate::module::canonical_path(&p.info.module_dir))
            .ok()?;
        let module = inside
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/");
        p.info.modules.contains(&module).then_some((p, module))
    });
    match in_package {
        Some((package, module)) => {
            let name = package.canonical_name(&module);
            let file = module_file(package, &module);
            let mut api = document_source(&name, &file, &source, opts);
            resolve_reexports(&mut api, package, &module);
            Ok((Some(package_api(package)), vec![api]))
        }
        None => Ok((None, vec![document_source(target, target, &source, opts)])),
    }
}

/// The modules of `package` (only `only`, when given), each documented.
fn package_modules(package: &Package, only: Option<&str>, opts: DocOptions) -> Vec<ModuleApi> {
    package
        .info
        .modules
        .iter()
        .filter(|m| only.is_none_or(|o| o == m.as_str()))
        .map(|module| {
            let name = package.canonical_name(module);
            let file = module_file(package, module);
            match package.load(module) {
                Some(loaded) => {
                    let mut api = document_source(&name, &file, &loaded.source, opts);
                    resolve_reexports(&mut api, package, module);
                    api
                }
                None => ModuleApi::failed(name, file, "the module cannot be read".to_string()),
            }
        })
        .collect()
}

/// Name the module each relative `pub import` of `module` (a module of
/// `package`, by its path in the package) re-exports.
fn resolve_reexports(api: &mut ModuleApi, package: &Package, module: &str) {
    for r in &mut api.reexports {
        if !crate::module::is_relative_import(&r.module) {
            continue;
        }
        let mut segments: Vec<&str> = module.split('/').collect();
        segments.pop();
        let mut inside = true;
        for seg in r.module.split('/') {
            match seg {
                "." | "" => {}
                ".." => inside &= segments.pop().is_some(),
                other => segments.push(other),
            }
        }
        if inside {
            r.canonical = Some(package.canonical_name(&segments.join("/")));
        }
    }
}

/// A package's facts, with the doc of its root module.
fn package_api(package: &Package) -> PackageApi {
    let info = &package.info;
    let root_module = info.modules.iter().find(|m| **m == info.name).cloned();
    let doc = root_module
        .as_deref()
        .and_then(|m| package.load(m))
        .and_then(|loaded| crate::cst::parse_source_documented(&loaded.source, ENTRY_FILE).ok())
        .and_then(|(_, _, doc)| doc)
        .as_ref()
        .map(DocJson::of);
    PackageApi {
        name: info.name.clone(),
        version: info.version.clone(),
        description: info.description.clone(),
        root: info.root.display().to_string(),
        root_module: root_module.map(|m| package.canonical_name(&m)),
        doc,
        modules: info
            .modules
            .iter()
            .map(|m| package.canonical_name(m))
            .collect(),
    }
}

/// The file of `module`, relative to the package root.
fn module_file(package: &Package, module: &str) -> String {
    let info = &package.info;
    let file = format!("{module}.ptl");
    if info.in_memory {
        // A bundled package keeps its files' own paths (`src/menu.ptl`).
        if let Some(bundled) = crate::bundled::package(&info.name) {
            let prefixes = ["", "src/"];
            if let Some((path, _)) = bundled.sources().find(|(path, _)| {
                prefixes
                    .iter()
                    .any(|p| path.strip_prefix(p) == Some(file.as_str()))
            }) {
                return path.to_string();
            }
        }
        return file;
    }
    let full: PathBuf = module
        .split('/')
        .fold(info.module_dir.clone(), |mut acc, seg| {
            acc.push(seg);
            acc
        });
    let full = full.with_extension("ptl");
    full.strip_prefix(&info.root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or(file)
}

// ── reading one module ────────────────────────────────────────────────

impl ModuleApi {
    fn failed(name: String, file: String, error: String) -> ModuleApi {
        ModuleApi {
            name,
            file,
            doc: None,
            reexports: Vec::new(),
            items: Vec::new(),
            error: Some(error),
        }
    }
}

/// Document one module's source. `name` is its canonical name and `file`
/// what to report as its file.
pub fn document_source(name: &str, file: &str, source: &str, opts: DocOptions) -> ModuleApi {
    let (stmts, module_doc) = match crate::cst::parse_source_documented(source, ENTRY_FILE) {
        Ok((_, stmts, doc)) => (stmts, doc),
        Err(e) => return ModuleApi::failed(name.to_string(), file.to_string(), e.to_string()),
    };
    let chars: Vec<char> = source.chars().collect();
    let mut reexports = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    for stmt in &stmts {
        if stmt.test.is_some() {
            continue;
        }
        if let StmtKind::Import(decl) = &stmt.kind {
            if decl.exported {
                reexports.push(Reexport {
                    module: decl.module.clone(),
                    canonical: (!crate::module::is_relative_import(&decl.module))
                        .then(|| decl.module.clone()),
                    names: decl.names.clone(),
                });
            }
            continue;
        }
        if !stmt.exported && !opts.private {
            continue;
        }
        let line = decl_line(&chars, stmt);
        let doc = stmt.doc.as_deref().map(DocJson::of);
        match &stmt.kind {
            StmtKind::FnDecl {
                name,
                class,
                params,
                ret,
                ..
            } => {
                let form = Form {
                    params: params.iter().map(|p| param_api(p, &chars)).collect(),
                    returns: ret.as_ref().map(type_name),
                    native: false,
                    host: None,
                    line,
                    doc,
                };
                add_form(
                    &mut items,
                    name,
                    class.as_deref(),
                    stmt.exported,
                    line,
                    form,
                );
            }
            StmtKind::NativeFn(decl) => {
                let mut params: Vec<ParamApi> =
                    decl.params.iter().map(|p| param_api(p, &chars)).collect();
                if let Some(rest) = &decl.rest {
                    params.push(ParamApi {
                        name: rest.clone(),
                        r#type: None,
                        optional: true,
                        default: None,
                        rest: true,
                    });
                }
                let form = Form {
                    params,
                    returns: decl.ret.as_ref().map(type_name),
                    native: true,
                    host: decl.host.clone(),
                    line,
                    doc,
                };
                add_form(
                    &mut items,
                    &decl.name,
                    decl.class.as_deref(),
                    stmt.exported,
                    line,
                    form,
                );
            }
            StmtKind::ClassDecl { name, fields } => {
                let fields: Vec<FieldApi> = fields
                    .iter()
                    .map(|f| FieldApi {
                        name: f.name.clone(),
                        r#type: f.ty.as_ref().map(type_name),
                    })
                    .collect();
                let constructor = Form {
                    params: fields
                        .iter()
                        .map(|f| ParamApi {
                            name: f.name.clone(),
                            r#type: f.r#type.clone(),
                            optional: false,
                            default: None,
                            rest: false,
                        })
                        .collect(),
                    returns: Some(name.clone()),
                    native: false,
                    host: None,
                    line,
                    doc: None,
                };
                let mut item = Item::new("class", name, stmt.exported, line, doc);
                item.forms = Some(vec![constructor]);
                item.fields = Some(fields);
                items.push(item);
            }
            StmtKind::EnumDecl { name, variants } => {
                let mut item = Item::new("enum", name, stmt.exported, line, doc);
                item.variants = Some(
                    variants
                        .iter()
                        .map(|v| VariantApi {
                            name: v.name.clone(),
                            fields: v.fields.clone(),
                            data: v.data.iter().map(|d| d.key.clone()).collect(),
                        })
                        .collect(),
                );
                items.push(item);
            }
            StmtKind::Let {
                name,
                ty,
                value,
                is_var,
                is_const,
                ..
            } => {
                let kind = if *is_const {
                    "const"
                } else if *is_var {
                    "var"
                } else {
                    "let"
                };
                items.push(binding(
                    kind,
                    name,
                    ty.as_ref(),
                    value,
                    stmt,
                    line,
                    doc,
                    &chars,
                ));
            }
            StmtKind::State {
                name,
                ty,
                init,
                is_var,
            } => {
                let kind = if *is_var { "state_var" } else { "state" };
                items.push(binding(
                    kind,
                    name,
                    ty.as_ref(),
                    init,
                    stmt,
                    line,
                    doc,
                    &chars,
                ));
            }
            StmtKind::Memo { name, ty, init, .. } => {
                items.push(binding(
                    "memo",
                    name,
                    ty.as_ref(),
                    init,
                    stmt,
                    line,
                    doc,
                    &chars,
                ));
            }
            _ => {}
        }
    }
    ModuleApi {
        name: name.to_string(),
        file: file.to_string(),
        doc: module_doc.as_ref().map(DocJson::of),
        reexports,
        items,
        error: None,
    }
}

impl Item {
    fn new(
        kind: &'static str,
        name: &str,
        exported: bool,
        line: u32,
        doc: Option<DocJson>,
    ) -> Item {
        Item {
            kind,
            name: name.to_string(),
            exported,
            hidden: doc.as_ref().is_some_and(|d| d.hidden),
            line,
            doc,
            receiver: None,
            forms: None,
            fields: None,
            variants: None,
            r#type: None,
            value: None,
        }
    }
}

/// Add one call form of the function `name`: to its overload set when the
/// module already declared one, else as a new item.
fn add_form(
    items: &mut Vec<Item>,
    name: &str,
    class: Option<&str>,
    exported: bool,
    line: u32,
    form: Form,
) {
    let existing = items
        .iter_mut()
        .find(|i| i.name == name && matches!(i.kind, "fn" | "native_fn"));
    match existing {
        Some(item) => {
            if !form.native {
                item.kind = "fn";
            }
            item.exported |= exported;
            // The first documented form describes the set.
            if item.doc.is_none() && form.doc.is_some() {
                item.doc = form.doc.clone();
                item.hidden = form.doc.as_ref().is_some_and(|d| d.hidden);
            }
            item.forms.get_or_insert_with(Vec::new).push(form);
        }
        None => {
            let kind = if form.native { "native_fn" } else { "fn" };
            let mut item = Item::new(kind, name, exported, line, form.doc.clone());
            item.receiver = class.map(str::to_string);
            item.forms = Some(vec![form]);
            items.push(item);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn binding(
    kind: &'static str,
    name: &str,
    ty: Option<&TypeAnn>,
    value: &Expr,
    stmt: &Stmt,
    line: u32,
    doc: Option<DocJson>,
    chars: &[char],
) -> Item {
    let mut item = Item::new(kind, name, stmt.exported, line, doc);
    item.r#type = ty.map(type_name);
    item.value = span_text(chars, value.span).filter(|t| !t.contains('\n') && t.len() <= 80);
    item
}

fn type_name(ty: &TypeAnn) -> String {
    ty.name.clone()
}

fn param_api(param: &Param, chars: &[char]) -> ParamApi {
    ParamApi {
        name: param.name.clone(),
        r#type: param.ty.as_ref().map(type_name),
        optional: param.optional || param.has_default(),
        default: param
            .default
            .as_ref()
            .and_then(|d| span_text(chars, d.span)),
        rest: false,
    }
}

/// The source text a span covers (char offsets).
fn span_text(chars: &[char], span: crate::source_map::SourceSpan) -> Option<String> {
    let (start, end) = (span.start.offset as usize, span.end.offset as usize);
    (start < end && end <= chars.len()).then(|| chars[start..end].iter().collect::<String>())
}

/// The 1-based line of a statement's keyword (or its `pub`), past the
/// annotations its span starts with.
fn decl_line(chars: &[char], stmt: &Stmt) -> u32 {
    let start = stmt.span.start.offset as usize;
    if chars.get(start) != Some(&'@') {
        return stmt.span.start.line;
    }
    let to = crate::ast::skip_annotations(chars, start).min(chars.len());
    stmt.span.start.line + chars[start..to].iter().filter(|&&c| c == '\n').count() as u32
}

// ── human-readable forms ──────────────────────────────────────────────

/// A call form as one line: `(a: num, b?: int = 2, ...rest) -> int`.
pub fn form_signature(form: &Form) -> String {
    let params: Vec<String> = form
        .params
        .iter()
        .map(|p| {
            let mut s = if p.rest {
                format!("...{}", p.name)
            } else {
                p.name.clone()
            };
            if p.optional && !p.rest && p.default.is_none() {
                s.push('?');
            }
            if let Some(ty) = &p.r#type {
                s.push_str(&format!(": {ty}"));
            }
            if let Some(d) = &p.default {
                s.push_str(&format!(" = {d}"));
            }
            s
        })
        .collect();
    let mut out = format!("({})", params.join(", "));
    if let Some(ret) = &form.returns {
        out.push_str(&format!(" -> {ret}"));
    }
    out
}

/// The kind as it is written in source (`native fn`, `state var`).
fn kind_word(kind: &str) -> &str {
    match kind {
        "native_fn" => "native fn",
        "state_var" => "state var",
        other => other,
    }
}

/// An item's declaration on one line, without its doc.
pub fn item_signature(item: &Item) -> String {
    let kind = kind_word(item.kind);
    match item.kind {
        "fn" | "native_fn" => {
            let forms: Vec<String> = item.forms.iter().flatten().map(form_signature).collect();
            format!("{kind} {}{}", item.name, forms.join(" | "))
        }
        "class" => {
            let fields: Vec<String> = item
                .fields
                .iter()
                .flatten()
                .map(|f| match &f.r#type {
                    Some(ty) => format!("{}: {ty}", f.name),
                    None => f.name.clone(),
                })
                .collect();
            format!("class {}({})", item.name, fields.join(", "))
        }
        "enum" => {
            let variants: Vec<String> = item
                .variants
                .iter()
                .flatten()
                .map(|v| {
                    let inner: Vec<&str> = v
                        .fields
                        .iter()
                        .chain(v.data.iter())
                        .map(String::as_str)
                        .collect();
                    if inner.is_empty() {
                        v.name.clone()
                    } else {
                        format!("{}({})", v.name, inner.join(", "))
                    }
                })
                .collect();
            format!("enum {} {{ {} }}", item.name, variants.join(", "))
        }
        _ => {
            let mut s = format!("{kind} {}", item.name);
            if let Some(ty) = &item.r#type {
                s.push_str(&format!(": {ty}"));
            }
            if let Some(v) = &item.value {
                s.push_str(&format!(" = {v}"));
            }
            s
        }
    }
}

fn doc_summary(doc: &Option<DocJson>) -> Option<&str> {
    doc.as_ref().and_then(|d| d.summary.as_deref())
}

/// `--brief`: a one-line-per-item index. Hidden items and hidden modules
/// are left out.
pub fn render_brief(report: &DocReport) -> String {
    let mut out = String::new();
    if let Some(p) = &report.package {
        let mut line = format!("package {}", p.name);
        if let Some(v) = &p.version {
            line.push_str(&format!(" {v}"));
        }
        if let Some(s) = doc_summary(&p.doc).or(p.description.as_deref()) {
            line.push_str(&format!(" — {s}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    for module in &report.modules {
        if module.doc.as_ref().is_some_and(|d| d.hidden) {
            continue;
        }
        let mut line = format!("{} ({})", module.name, module.file);
        if let Some(s) = doc_summary(&module.doc) {
            line.push_str(&format!(" — {s}"));
        }
        if let Some(e) = &module.error {
            line.push_str(&format!(" — error: {}", first_line(e)));
        }
        out.push_str(&line);
        out.push('\n');
        for item in module.items.iter().filter(|i| !i.hidden) {
            let mut line = format!("  {}", item_signature(item));
            if let Some(s) = doc_summary(&item.doc) {
                line.push_str(&format!(" — {s}"));
            }
            if item.doc.as_ref().is_some_and(|d| d.deprecated.is_some()) {
                line.push_str(" [deprecated]");
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// The default output: each module's doc and every item's signature and
/// doc, as Markdown. Hidden items and hidden modules are left out.
pub fn render_text(report: &DocReport) -> String {
    let mut out = String::new();
    if let Some(p) = &report.package {
        out.push_str(&format!("# package {}", p.name));
        if let Some(v) = &p.version {
            out.push_str(&format!(" {v}"));
        }
        out.push_str("\n\n");
        if let Some(d) = &p.description {
            out.push_str(&format!("{d}\n\n"));
        }
        // The package's doc is its root module's, shown with that module
        // when it is listed.
        let root_listed = p
            .root_module
            .as_ref()
            .is_some_and(|root| report.modules.iter().any(|m| m.name == *root));
        if let Some(doc) = p.doc.as_ref().filter(|_| !root_listed) {
            render_doc_body(doc, &mut out);
        }
    }
    for module in &report.modules {
        if module.doc.as_ref().is_some_and(|d| d.hidden) {
            continue;
        }
        out.push_str(&format!("## {} ({})\n\n", module.name, module.file));
        if let Some(e) = &module.error {
            out.push_str(&format!("error: {e}\n\n"));
            continue;
        }
        if let Some(doc) = &module.doc {
            render_doc_body(doc, &mut out);
        }
        if !module.reexports.is_empty() {
            let names: Vec<String> = module
                .reexports
                .iter()
                .map(|r| match &r.names {
                    Some(names) => format!("{}: {}", r.module, names.join(", ")),
                    None => r.module.clone(),
                })
                .collect();
            out.push_str(&format!("Re-exports: {}\n\n", names.join("; ")));
        }
        for item in module.items.iter().filter(|i| !i.hidden) {
            render_item(item, &module.file, &mut out);
        }
    }
    out
}

fn render_item(item: &Item, file: &str, out: &mut String) {
    let kind = kind_word(item.kind);
    let render_doc = |doc: Option<&DocJson>, out: &mut String| match doc {
        Some(doc) => {
            render_doc_body(doc, out);
            render_doc_fields(doc, out);
        }
        None => out.push_str("(undocumented)\n\n"),
    };
    match &item.forms {
        Some(forms) if matches!(item.kind, "fn" | "native_fn") => {
            let heading =
                |form: &Form| format!("### {kind} {}{}\n", item.name, form_signature(form));
            if forms.iter().filter(|f| f.doc.is_some()).count() > 1 {
                // Several documented forms: each describes its own.
                for form in forms {
                    out.push_str(&heading(form));
                    out.push('\n');
                    if let Some(doc) = &form.doc {
                        render_doc(Some(doc), out);
                    }
                }
            } else {
                for form in forms {
                    out.push_str(&heading(form));
                }
                out.push('\n');
                render_doc(item.doc.as_ref(), out);
            }
        }
        _ => {
            out.push_str(&format!("### {}\n\n", item_signature(item)));
            render_doc(item.doc.as_ref(), out);
        }
    }
    if !item.exported {
        out.push_str("(private)\n");
    }
    out.push_str(&format!("{file}:{}\n\n", item.line));
}

fn render_doc_body(doc: &DocJson, out: &mut String) {
    if let Some(dep) = &doc.deprecated {
        out.push_str(&format!("**Deprecated:** {dep}\n\n"));
    }
    if let Some(body) = &doc.body {
        out.push_str(body);
        out.push_str("\n\n");
    }
    if !doc.see.is_empty() {
        out.push_str(&format!("See also: {}\n\n", doc.see.join(", ")));
    }
}

fn render_doc_fields(doc: &DocJson, out: &mut String) {
    let list = |title: &str, map: &serde_json::Map<String, serde_json::Value>, out: &mut String| {
        if map.is_empty() {
            return;
        }
        out.push_str(&format!("{title}:\n"));
        for (k, v) in map {
            out.push_str(&format!("- `{k}`: {}\n", v.as_str().unwrap_or_default()));
        }
        out.push('\n');
    };
    list("Parameters", &doc.params, out);
    if let Some(r) = &doc.returns {
        out.push_str(&format!("Returns: {r}\n\n"));
    }
    list("Options", &doc.options, out);
    for example in &doc.example {
        out.push_str("```petal\n");
        out.push_str(example);
        out.push_str("\n```\n\n");
    }
}
