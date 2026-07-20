//! Document store — tracks open files and their compilation results.

use std::collections::HashMap;

use petal::ast::{Expr, ExprKind, Stmt, StmtKind};
use petal::diagnostic::Diagnostic as PetalDiagnostic;
use petal::source_map::SourceSpan;

use crate::lsp_types;

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
        }
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
        let analysis = analyze(&text);
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
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.version = version;
            doc.text = text;
            doc.analysis = analyze(&doc.text);
        }
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

pub fn analyze(source: &str) -> Analysis {
    if source.trim().is_empty() {
        return Analysis::empty();
    }

    let mut env = petal::env::Env::new();
    match env.load_program(source) {
        Ok(pid) => {
            let program = env.get_program(pid);
            let mut diagnostics: Vec<lsp_types::Diagnostic> = Vec::new();
            if let Some(p) = program {
                for w in &p.warnings {
                    diagnostics.push(petal_diagnostic_to_lsp(w, lsp_types::DiagnosticSeverity::Warning));
                }
            }

            let definitions = match petal::cst::parse_source(source, petal::source_map::ENTRY_FILE)
            {
                Ok((_, stmts)) => collect_definitions(&stmts),
                Err(_) => Vec::new(),
            };

            Analysis {
                diagnostics,
                definitions,
            }
        }
        Err(e) => {
            let diag = error_string_to_diagnostic(&e);
            Analysis {
                diagnostics: vec![diag],
                definitions: Vec::new(),
            }
        }
    }
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

fn error_string_to_diagnostic(msg: &str) -> lsp_types::Diagnostic {
    let (line, col) = parse_error_position(msg);
    let pos = lsp_types::Position {
        line,
        character: col,
    };
    lsp_types::Diagnostic {
        range: lsp_types::Range {
            start: pos,
            end: pos,
        },
        severity: Some(lsp_types::DiagnosticSeverity::Error),
        message: msg.to_string(),
        source: Some("petal".to_string()),
    }
}

fn parse_error_position(msg: &str) -> (u32, u32) {
    // Petal errors look like: "[line 3, column 5] message"
    if let Some(rest) = msg.strip_prefix("[line ") {
        if let Some(comma) = rest.find(',') {
            let line_str = &rest[..comma];
            if let Ok(line) = line_str.parse::<u32>() {
                if let Some(col_start) = rest.find("column ") {
                    let after_col = &rest[col_start + 7..];
                    if let Some(bracket) = after_col.find(']') {
                        if let Ok(col) = after_col[..bracket].parse::<u32>() {
                            return (line.saturating_sub(1), col.saturating_sub(1));
                        }
                    }
                }
                return (line.saturating_sub(1), 0);
            }
        }
    }
    (0, 0)
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

fn collect_definitions(stmts: &[Stmt]) -> Vec<Definition> {
    let mut defs = Vec::new();
    for stmt in stmts {
        collect_definitions_from_stmt(stmt, &mut defs);
    }
    defs
}

fn collect_definitions_from_stmt(stmt: &Stmt, defs: &mut Vec<Definition>) {
    match &stmt.kind {
        StmtKind::Let { name, ty, value } => {
            let detail = ty.as_ref().map(|t| format!(": {}", t.name));
            defs.push(Definition {
                name: name.clone(),
                kind: DefinitionKind::Variable,
                span: stmt.span,
                detail,
            });
            collect_definitions_from_expr(value, defs);
        }
        StmtKind::FnDecl {
            name,
            params,
            body,
            ret,
        } => {
            let param_list = params
                .iter()
                .map(|p| {
                    if let Some(ty) = &p.ty {
                        format!("{}: {}", p.name, ty.name)
                    } else {
                        p.name.clone()
                    }
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
                    detail: p.ty.as_ref().map(|t| format!(": {}", t.name)),
                });
            }
            for s in body {
                collect_definitions_from_stmt(s, defs);
            }
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
                collect_definitions_from_stmt(s, defs);
            }
        }
        StmtKind::Expr(e) => {
            collect_definitions_from_expr(e, defs);
        }
        _ => {}
    }
}

fn collect_definitions_from_expr(expr: &Expr, defs: &mut Vec<Definition>) {
    match &expr.kind {
        ExprKind::If {
            then_body,
            else_body,
            ..
        } => {
            for s in then_body {
                collect_definitions_from_stmt(s, defs);
            }
            if let Some(petal::ast::ElseBranch::Block(stmts)) = else_body {
                for s in stmts {
                    collect_definitions_from_stmt(s, defs);
                }
            }
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                collect_definitions_from_stmt(s, defs);
            }
        }
        ExprKind::Lambda { params, body, .. } => {
            for p in params {
                defs.push(Definition {
                    name: p.name.clone(),
                    kind: DefinitionKind::Parameter,
                    span: expr.span,
                    detail: p.ty.as_ref().map(|t| format!(": {}", t.name)),
                });
            }
            for s in body {
                collect_definitions_from_stmt(s, defs);
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

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------
// Petal keywords (for completion)
// ---------------------------------------------------------------------------

pub const KEYWORDS: &[&str] = &[
    "let", "fn", "if", "elsif", "else", "end", "for", "in", "do", "while",
    "match", "when", "return", "break", "continue", "state", "import", "true",
    "false", "nil", "and", "or", "not", "enum",
];
