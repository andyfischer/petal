//! Document store — tracks open files and their compilation results.

use std::collections::HashMap;

use crate::ast::{Expr, ExprKind, Stmt, StmtKind};
use crate::diagnostic::Diagnostic as PetalDiagnostic;
use crate::source_map::SourceSpan;

use crate::lsp::lsp_types;

// ---------------------------------------------------------------------------
// A single open document
// ---------------------------------------------------------------------------

pub struct Document {
    pub uri: String,
    pub version: i32,
    pub text: String,
    pub analysis: Analysis,
}

// ---------------------------------------------------------------------------
// Analysis result for a document
// ---------------------------------------------------------------------------

pub struct Analysis {
    pub diagnostics: Vec<lsp_types::Diagnostic>,
    pub definitions: Vec<Definition>,
    /// The document's imports as they resolved, lowest precedence first
    /// (implicit ones, then each `import` in source order).
    pub imports: Vec<ImportedModule>,
}

/// One module the document imports, with what it exports.
#[derive(Debug, Clone)]
pub struct ImportedModule {
    /// The module's canonical name (`bloom/menu`, `level`).
    pub module: String,
    /// The qualifier it is reachable through (`m.name`); `None` for a
    /// selective import.
    pub qualifier: Option<String>,
    /// `Some` for a selective import: the only names it binds bare.
    pub names: Option<Vec<String>>,
    /// An import the document did not write (the `std` prelude).
    pub implicit: bool,
    /// The module's own file, when it has one.
    pub uri: Option<String>,
    /// Every name the module exports, with its declaration.
    pub exports: Vec<ImportedDefinition>,
}

/// A name a module exports and where it was declared — which, through a
/// re-export, may be another file than the module's.
#[derive(Debug, Clone)]
pub struct ImportedDefinition {
    pub def: Definition,
    /// The file the declaration is in, when it is in a file.
    pub uri: Option<String>,
}

impl ImportedModule {
    /// Does this import bind `name` bare in the document?
    pub fn binds_bare(&self, name: &str) -> bool {
        match &self.names {
            Some(names) => names.iter().any(|n| n == name),
            None => true,
        }
    }

    pub fn export(&self, name: &str) -> Option<&ImportedDefinition> {
        self.exports.iter().find(|e| e.def.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub name: String,
    pub kind: DefinitionKind,
    pub span: SourceSpan,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    Variable,
    Function,
    Enum,
    Parameter,
}

impl Analysis {
    fn empty() -> Self {
        Self {
            diagnostics: Vec::new(),
            definitions: Vec::new(),
            imports: Vec::new(),
        }
    }

    /// The declaration a *bare* use of `name` reaches through the document's
    /// imports: a selective import first, then a plain one, then an implicit
    /// one — the order the compiler resolves in.
    pub fn imported(&self, name: &str) -> Option<&ImportedDefinition> {
        let tier = |m: &ImportedModule| match (m.implicit, m.names.is_some()) {
            (true, _) => 2,
            (false, false) => 1,
            (false, true) => 0,
        };
        self.imports
            .iter()
            .filter(|m| m.binds_bare(name))
            .filter_map(|m| Some((tier(m), m.export(name)?)))
            .min_by_key(|(tier, _)| *tier)
            .map(|(_, def)| def)
    }

    /// The import the qualifier `name` stands for (`name.member`).
    pub fn qualifier(&self, name: &str) -> Option<&ImportedModule> {
        self.imports
            .iter()
            .rev()
            .find(|m| m.qualifier.as_deref() == Some(name))
    }
}

// ---------------------------------------------------------------------------
// Document store
// ---------------------------------------------------------------------------

pub struct DocumentStore {
    docs: HashMap<String, Document>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
        }
    }

    pub fn open(&mut self, uri: String, version: i32, text: String) {
        let analysis = self.analyze_document(&uri, &text);
        self.docs.insert(
            uri.clone(),
            Document {
                uri,
                version,
                text,
                analysis,
            },
        );
    }

    pub fn change(&mut self, uri: &str, version: i32, text: String) {
        if !self.docs.contains_key(uri) {
            return;
        }
        let analysis = self.analyze_document(uri, &text);
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.version = version;
            doc.text = text;
            doc.analysis = analysis;
        }
    }

    /// Analyze the document at `uri` as holding `text`. Its imports resolve
    /// from its own path, and every other open document stands in for its
    /// file on disk, so an unsaved edit to a module is what its importers
    /// see.
    fn analyze_document(&self, uri: &str, text: &str) -> Analysis {
        let others: Vec<(std::path::PathBuf, &str)> = self
            .docs
            .values()
            .filter(|d| d.uri != uri)
            .filter_map(|d| Some((uri_to_path(&d.uri)?, d.text.as_str())))
            .collect();
        analyze_at(text, uri_to_path(uri).as_deref(), &others)
    }

    pub fn close(&mut self, uri: &str) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &str) -> Option<&Document> {
        self.docs.get(uri)
    }
}

// ---------------------------------------------------------------------------
// Analysis — compile source and extract diagnostics + definitions
// ---------------------------------------------------------------------------

/// Analyze `source` as a program with no file: its relative imports can only
/// reach modules held in memory.
pub fn analyze(source: &str) -> Analysis {
    analyze_at(source, None, &[])
}

/// Analyze `source` as the contents of the file at `path`. `open` is the
/// text of other files that should be read instead of what is on disk.
pub fn analyze_at(
    source: &str,
    path: Option<&std::path::Path>,
    open: &[(std::path::PathBuf, &str)],
) -> Analysis {
    if source.trim().is_empty() {
        return Analysis::empty();
    }

    // Collected independently of compilation: a file that parses but fails a
    // later phase still has usable symbols, so go-to-definition, document
    // symbols and completion keep working while the error is being fixed.
    let stmts = crate::cst::parse_source(source, crate::source_map::ENTRY_FILE)
        .map(|(_, stmts)| stmts)
        .ok();
    let definitions = stmts
        .as_deref()
        .map(|stmts| collect_definitions(stmts, source))
        .unwrap_or_default();

    let mut env = crate::env::Env::new();
    // Diagnostics only: the document may declare natives of a host this
    // process is not.
    env.set_check_only(true);
    for (file, text) in open {
        env.override_file_source(file, text);
    }
    let mut imports = Vec::new();
    let diagnostics = match env.load_program_diag(source, path) {
        Ok(pid) => env
            .get_program(pid)
            .map(|p| {
                imports = imported_modules(p);
                p.warnings
                    .iter()
                    .map(|w| {
                        let severity = if w.is_error() {
                            lsp_types::DiagnosticSeverity::Error
                        } else {
                            lsp_types::DiagnosticSeverity::Warning
                        };
                        petal_diagnostic_to_lsp(w, severity)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        // One diagnostic per item: the front end reports every error it found
        // in a single pass, and the editor should show all of them.
        Err(err) => err
            .items
            .iter()
            .map(|item| {
                let range = match item.span {
                    // Only a span in the file we are analyzing can be placed in
                    // it. A diagnostic carrying another module's name, or one
                    // with no span at all (an unresolvable import, say), has no
                    // honest position here, so it goes to the top of the file
                    // with the file name left in the message.
                    Some(span)
                        if item.file.is_none() && span.file == crate::source_map::ENTRY_FILE =>
                    {
                        span_to_range(&span)
                    }
                    _ => {
                        let pos = lsp_types::Position {
                            line: 0,
                            character: 0,
                        };
                        lsp_types::Range {
                            start: pos,
                            end: pos,
                        }
                    }
                };
                let message = match &item.file {
                    Some(file) => format!("{file}: {}", item.message),
                    None => item.message.clone(),
                };
                lsp_types::Diagnostic {
                    range,
                    severity: Some(lsp_types::DiagnosticSeverity::Error),
                    message,
                    source: Some("petal".to_string()),
                }
            })
            .collect(),
    };

    // A document that does not compile still imports what it imports: its
    // import lines alone are a program, and compiling that is enough to know
    // what they bring in.
    if imports.is_empty() {
        let header = import_header(source);
        if !header.is_empty()
            && let Ok(pid) = env.load_program_diag(&header, path)
            && let Some(p) = env.get_program(pid)
        {
            imports = imported_modules(p);
        }
    }

    Analysis {
        diagnostics,
        definitions,
        imports,
    }
}

/// The import statements at the top of `source`, as text — found without a
/// parse, since the rest of the file may not have one yet. Imports come
/// first in a file, so this is the leading run of import lines (with the
/// continuation lines of a wrapped name list), comments and blanks.
fn import_header(source: &str) -> String {
    let mut out = String::new();
    let mut continues = false;
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or("").trim();
        let is_import = ["import ", "pub import ", "export import "]
            .iter()
            .any(|kw| code.starts_with(kw));
        if !(is_import || continues || code.is_empty()) {
            break;
        }
        if !code.is_empty() {
            out.push_str(code);
            out.push('\n');
        }
        continues = code.ends_with(',') || code.ends_with(':');
    }
    out
}

/// What each import of a compiled program's entry file brings in, with the
/// declaration behind every exported name.
fn imported_modules(program: &crate::program::Program) -> Vec<ImportedModule> {
    let table = &program.imports;
    // Every module's top-level declarations, parsed once.
    let mut declared: HashMap<&str, (Option<String>, Vec<Definition>)> = HashMap::new();
    for (module, &file) in &table.files {
        let Some(file) = program.source_map.files.get(file as usize) else {
            continue;
        };
        let defs = crate::cst::parse_source(&file.source, crate::source_map::ENTRY_FILE)
            .map(|(_, stmts)| top_level_definitions(&stmts, &file.source))
            .unwrap_or_default();
        declared.insert(
            module.as_str(),
            (file.origin.as_deref().map(path_to_uri), defs),
        );
    }
    // Where an exported name is declared: in the module itself, else — a
    // re-export — in whichever loaded module both exports and declares it.
    let locate = |module: &str, name: &str| -> Option<ImportedDefinition> {
        let own = declared
            .get(module)
            .and_then(|(uri, defs)| Some((uri, defs.iter().find(|d| d.name == name)?)));
        let found = own.or_else(|| {
            let mut others: Vec<&&str> = declared.keys().collect();
            others.sort();
            others.into_iter().find_map(|other| {
                let exports = table.exports.get(*other)?;
                if !exports.iter().any(|e| e == name) {
                    return None;
                }
                let (uri, defs) = declared.get(*other)?;
                Some((uri, defs.iter().find(|d| d.name == name)?))
            })
        })?;
        Some(ImportedDefinition {
            def: found.1.clone(),
            uri: found.0.clone(),
        })
    };
    table
        .entry
        .iter()
        .map(|import| ImportedModule {
            module: import.module.clone(),
            qualifier: import.qualifier.clone(),
            names: import.names.clone(),
            implicit: import.implicit,
            uri: declared
                .get(import.module.as_str())
                .and_then(|(uri, _)| uri.clone()),
            exports: table
                .exports
                .get(&import.module)
                .into_iter()
                .flatten()
                .map(|name| {
                    locate(&import.module, name).unwrap_or_else(|| ImportedDefinition {
                        // Exported, but declared somewhere this could not
                        // read: still a name the document can use.
                        def: Definition {
                            name: name.clone(),
                            kind: DefinitionKind::Variable,
                            span: SourceSpan::default(),
                            detail: None,
                        },
                        uri: None,
                    })
                })
                .collect(),
        })
        .collect()
}

/// The declaration each top-level statement makes (not what is nested in
/// it: parameters, locals).
fn top_level_definitions(stmts: &[Stmt], source: &str) -> Vec<Definition> {
    let src: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    for stmt in stmts {
        let mut defs = Vec::new();
        match &stmt.kind {
            StmtKind::State { name, .. } | StmtKind::ClassDecl { name, .. } => {
                defs.push(Definition {
                    name: name.clone(),
                    kind: DefinitionKind::Variable,
                    span: stmt.span,
                    detail: None,
                })
            }
            _ => collect_definitions_from_stmt(stmt, &src, &mut defs),
        }
        if let Some(first) = defs.into_iter().next() {
            // An enum's variants are what importers use.
            if let StmtKind::EnumDecl { variants, .. } = &stmt.kind {
                out.extend(variants.iter().map(|v| Definition {
                    name: v.name.clone(),
                    kind: DefinitionKind::Enum,
                    span: stmt.span,
                    detail: Some(format!("variant of {}", first.name)),
                }));
            }
            out.push(first);
        }
    }
    out
}

/// The file `import <path>`, written in the document at `uri`, resolves to.
pub fn import_target_uri(path: &str, uri: &str) -> Option<String> {
    let doc = uri_to_path(uri)?;
    let env = crate::env::Env::new();
    env.resolve_module_file(path, &doc)
        .map(|file| path_to_uri(&file))
}

/// What could follow `prefix` on an `import` line of the document at `uri`:
/// the next path segment of every module that could be named there. For a
/// relative prefix those are the `.ptl` files and directories beside the
/// document; otherwise the packages this process can see.
pub fn import_path_completions(prefix: &str, uri: &str) -> Vec<String> {
    let (dir_part, partial) = match prefix.rfind('/') {
        Some(i) => (&prefix[..=i], &prefix[i + 1..]),
        None => ("", prefix),
    };
    let mut out: Vec<String> = Vec::new();
    if crate::module::is_relative_import(prefix) {
        let Some(base) = uri_to_path(uri).and_then(|p| p.parent().map(|d| d.join(dir_part)))
        else {
            return out;
        };
        let Ok(entries) = std::fs::read_dir(base) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                out.push(name);
            } else if let Some(stem) = name.strip_suffix(".ptl") {
                out.push(stem.to_string());
            }
        }
    } else {
        let env = crate::env::Env::new();
        let mut paths = env.importable_paths();
        // A package with a module of its own name lists only that name, but
        // its other modules can be imported too.
        for package in env.packages() {
            paths.extend(
                package
                    .modules
                    .iter()
                    .map(|m| format!("{}/{m}", package.name)),
            );
        }
        for path in paths {
            if let Some(rest) = path.strip_prefix(dir_part) {
                out.push(rest.split('/').next().unwrap_or(rest).to_string());
            }
        }
    }
    out.retain(|name| name.starts_with(partial) && !name.is_empty());
    out.sort();
    out.dedup();
    out
}

/// The filesystem path a `file://` URI names.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///abs/path` has an empty authority; `file://host/path` is not a
    // local file.
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(std::path::PathBuf::from(String::from_utf8(out).ok()?))
}

/// The `file://` URI of a path.
pub fn path_to_uri(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn petal_diagnostic_to_lsp(
    d: &PetalDiagnostic,
    severity: lsp_types::DiagnosticSeverity,
) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: span_to_range(&d.span),
        severity: Some(severity),
        message: d.message.clone(),
        source: Some("petal".to_string()),
    }
}

pub fn span_to_range(span: &SourceSpan) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: span.start.line.saturating_sub(1),
            character: span.start.column.saturating_sub(1),
        },
        end: lsp_types::Position {
            line: span.end.line.saturating_sub(1),
            character: span.end.column.saturating_sub(1),
        },
    }
}

// ---------------------------------------------------------------------------
// Definition collection — walk the AST to find named bindings
// ---------------------------------------------------------------------------

fn collect_definitions(stmts: &[Stmt], source: &str) -> Vec<Definition> {
    let src: Vec<char> = source.chars().collect();
    let src = &src[..];
    let mut defs = Vec::new();
    for stmt in stmts {
        collect_definitions_from_stmt(stmt, src, &mut defs);
    }
    defs
}

fn collect_definitions_from_stmt(stmt: &Stmt, src: &[char], defs: &mut Vec<Definition>) {
    match &stmt.kind {
        StmtKind::Let {
            name, ty, value, ..
        } => {
            let detail = ty.as_ref().map(|t| format!(": {}", t.name));
            defs.push(Definition {
                name: name.clone(),
                kind: DefinitionKind::Variable,
                span: stmt.span,
                detail,
            });
            collect_definitions_from_expr(value, src, defs);
        }
        StmtKind::FnDecl {
            name,
            params,
            body,
            ret,
            ..
        } => {
            let param_list = params
                .iter()
                .map(|p| {
                    let head = if let Some(ty) = &p.ty {
                        format!("{}: {}", p.name, ty.name)
                    } else {
                        p.name.clone()
                    };
                    format!("{head}{}", param_default_suffix(p, src))
                })
                .collect::<Vec<_>>()
                .join(", ");
            let ret_str = ret
                .as_ref()
                .map(|r| format!(" -> {}", r.name))
                .unwrap_or_default();
            let detail = Some(format!("fn({param_list}){ret_str}"));
            defs.push(Definition {
                name: name.clone(),
                kind: DefinitionKind::Function,
                span: stmt.span,
                detail,
            });
            for p in params {
                defs.push(Definition {
                    name: p.name.clone(),
                    kind: DefinitionKind::Parameter,
                    span: stmt.span,
                    detail: param_detail(p, src),
                });
            }
            for s in body {
                collect_definitions_from_stmt(s, src, defs);
            }
        }
        // A native declaration is a function definition with no body: hover
        // and go-to-definition show its signature as written.
        StmtKind::NativeFn(decl) => {
            let mut parts: Vec<String> = decl
                .params
                .iter()
                .map(|p| {
                    let opt = if p.optional { "?" } else { "" };
                    match &p.ty {
                        Some(ty) => format!("{}{opt}: {}", p.name, ty.name),
                        None => format!("{}{opt}", p.name),
                    }
                })
                .collect();
            if let Some(rest) = &decl.rest {
                parts.push(format!("...{rest}"));
            }
            let ret_str = decl
                .ret
                .as_ref()
                .map(|r| format!(" -> {}", r.name))
                .unwrap_or_default();
            defs.push(Definition {
                name: decl.name.clone(),
                kind: DefinitionKind::Function,
                span: stmt.span,
                detail: Some(format!("native fn({}){ret_str}", parts.join(", "))),
            });
        }
        StmtKind::EnumDecl { name, variants } => {
            let detail = Some(
                variants
                    .iter()
                    .map(|v| v.name.clone())
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            defs.push(Definition {
                name: name.clone(),
                kind: DefinitionKind::Enum,
                span: stmt.span,
                detail,
            });
        }
        StmtKind::For { body, .. } | StmtKind::While { body, .. } => {
            for s in body {
                collect_definitions_from_stmt(s, src, defs);
            }
        }
        StmtKind::Expr(e) => {
            collect_definitions_from_expr(e, src, defs);
        }
        _ => {}
    }
}

/// ` = <default>` for a parameter that has a default value, as the source
/// spells it (`…` if the text cannot be recovered); empty otherwise.
fn param_default_suffix(p: &crate::ast::Param, src: &[char]) -> String {
    let Some(default) = &p.default else {
        return String::new();
    };
    let (start, end) = (
        default.span.start.offset as usize,
        default.span.end.offset as usize,
    );
    let text: String = match src.get(start..end) {
        Some(chars) if !chars.is_empty() && !chars.contains(&'\n') => chars.iter().collect(),
        _ => "…".to_string(),
    };
    format!(" = {text}")
}

/// A parameter's hover detail: its annotation and default value, if any.
fn param_detail(p: &crate::ast::Param, src: &[char]) -> Option<String> {
    let ty =
        p.ty.as_ref()
            .map(|t| format!(": {}", t.name))
            .unwrap_or_default();
    let detail = format!("{ty}{}", param_default_suffix(p, src));
    (!detail.is_empty()).then_some(detail)
}

fn collect_definitions_from_expr(expr: &Expr, src: &[char], defs: &mut Vec<Definition>) {
    match &expr.kind {
        ExprKind::If {
            then_body,
            else_body,
            ..
        } => {
            for s in then_body {
                collect_definitions_from_stmt(s, src, defs);
            }
            if let Some(crate::ast::ElseBranch::Block(stmts)) = else_body {
                for s in stmts {
                    collect_definitions_from_stmt(s, src, defs);
                }
            }
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                collect_definitions_from_stmt(s, src, defs);
            }
        }
        ExprKind::Lambda { params, body, .. } => {
            for p in params {
                defs.push(Definition {
                    name: p.name.clone(),
                    kind: DefinitionKind::Parameter,
                    span: expr.span,
                    detail: param_detail(p, src),
                });
            }
            for s in body {
                collect_definitions_from_stmt(s, src, defs);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Identifier at position — find what word the cursor is on
// ---------------------------------------------------------------------------

pub fn identifier_at_position(source: &str, line: u32, character: u32) -> Option<String> {
    let target_line = line as usize;
    let target_col = character as usize;

    let src_line = source.lines().nth(target_line)?;
    if target_col > src_line.len() {
        return None;
    }

    let chars: Vec<char> = src_line.chars().collect();
    if target_col >= chars.len() {
        return None;
    }
    if !is_ident_char(chars[target_col]) {
        return None;
    }

    let mut start = target_col;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }

    if chars[start].is_ascii_digit() {
        return None;
    }
    let mut end = target_col;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }

    Some(chars[start..end].iter().collect())
}

/// The identifier written before the `.` that precedes the identifier at the
/// position: `hud` for a cursor on `draw` in `hud.draw`. Also answers when
/// the cursor sits right after the dot (`hud.|`), which is where completion
/// asks.
pub fn qualifier_at_position(source: &str, line: u32, character: u32) -> Option<String> {
    let chars: Vec<char> = source.lines().nth(line as usize)?.chars().collect();
    let mut at = (character as usize).min(chars.len());
    while at > 0 && is_ident_char(chars[at - 1]) {
        at -= 1;
    }
    if at == 0 || chars[at - 1] != '.' {
        return None;
    }
    let end = at - 1;
    let mut start = end;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end || chars[start].is_ascii_digit() {
        return None;
    }
    // `a.b.c`: `b` is a field of a value, not a module qualifier.
    if start > 0 && chars[start - 1] == '.' {
        return None;
    }
    Some(chars[start..end].iter().collect())
}

/// When the position is inside the path of an `import` line, the path
/// written so far, up to the cursor (`./ui/h` for `import ./ui/h|`).
pub fn import_path_prefix(source: &str, line: u32, character: u32) -> Option<String> {
    let text: String = source
        .lines()
        .nth(line as usize)?
        .chars()
        .take(character as usize)
        .collect();
    let rest = text.trim_start();
    let rest = rest
        .strip_prefix("pub ")
        .or_else(|| rest.strip_prefix("export "))
        .unwrap_or(rest)
        .trim_start();
    let path = rest.strip_prefix("import ")?.trim_start();
    path.chars()
        .all(|c| is_ident_char(c) || c == '/' || c == '.')
        .then(|| path.to_string())
}

/// The whole path of the `import` statement on `line`, when the position is
/// on it.
pub fn import_path_at_position(source: &str, line: u32, character: u32) -> Option<String> {
    let before = import_path_prefix(source, line, character)?;
    let after: String = source
        .lines()
        .nth(line as usize)?
        .chars()
        .skip(character as usize)
        .take_while(|c| is_ident_char(*c) || *c == '/' || *c == '.')
        .collect();
    let path = before + &after;
    (!path.is_empty()).then_some(path)
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------
// Petal keywords (for completion)
// ---------------------------------------------------------------------------

/// The lexer's hard keywords plus the contextual ones (`as`, `class`,
/// `config`, `native`), which completion should still offer. Checked against
/// `petal::lexer::KEYWORDS` by
/// `core/tests/keyword_sync.rs` — do not edit this list alone.
pub const KEYWORDS: &[&str] = &[
    "let", "var", "set", "get", "fn", "if", "then", "elsif", "else", "end", "for", "in", "do",
    "while", "match", "when", "return", "break", "continue", "state", "import", "pub", "export",
    "as", "true", "false", "nil", "enum", "class", "config", "native",
];
