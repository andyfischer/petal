//! Document symbols: a file's outline — its top-level declarations, with an
//! enum's variants and a class's fields beneath them.

use crate::ast::{Stmt, StmtKind};
use crate::source_map::SourceSpan;

use crate::lsp::document::span_to_range;
use crate::lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
use crate::lsp::members::EnumShape;

pub fn document_symbols(stmts: &[Stmt], source: &str) -> Vec<DocumentSymbol> {
    let src: Vec<char> = source.chars().collect();
    let symbol = |name: &str, detail: Option<String>, kind: SymbolKind, span: SourceSpan| {
        DocumentSymbol {
            name: name.to_string(),
            detail,
            kind,
            range: span_to_range(&span),
            selection_range: name_range(name, span, &src),
            children: Vec::new(),
        }
    };
    let mut out = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::FnDecl { name, params, .. } => {
                let params: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
                out.push(symbol(
                    name,
                    Some(format!("fn({})", params.join(", "))),
                    SymbolKind::Function,
                    stmt.span,
                ));
            }
            StmtKind::NativeFn(decl) => out.push(symbol(
                &decl.name,
                Some("native fn".to_string()),
                SymbolKind::Function,
                stmt.span,
            )),
            StmtKind::Let { name, is_var, .. } => out.push(symbol(
                name,
                Some(if *is_var { "var" } else { "let" }.to_string()),
                SymbolKind::Variable,
                stmt.span,
            )),
            StmtKind::State { name, .. } => out.push(symbol(
                name,
                Some("state".to_string()),
                SymbolKind::Variable,
                stmt.span,
            )),
            StmtKind::ClassDecl { name, fields } => {
                let mut class = symbol(name, None, SymbolKind::Class, stmt.span);
                class.children = fields
                    .iter()
                    .map(|f| {
                        let ty = f.ty.as_ref().map(|t| t.name.clone());
                        symbol(&f.name, ty, SymbolKind::Field, f.span)
                    })
                    .collect();
                out.push(class);
            }
            StmtKind::EnumDecl { name, variants } => {
                let mut decl = symbol(name, None, SymbolKind::Enum, stmt.span);
                let shape = EnumShape::of(variants, &src);
                decl.children = shape
                    .variants
                    .iter()
                    .map(|v| {
                        let mut parts: Vec<String> = v.fields.clone();
                        parts.extend(v.data.iter().map(|(key, text)| format!("{key}: {text}")));
                        let detail = (!parts.is_empty()).then(|| format!("({})", parts.join(", ")));
                        symbol(&v.name, detail, SymbolKind::EnumMember, v.span)
                    })
                    .collect();
                out.push(decl);
            }
            _ => {}
        }
    }
    out
}

/// Where `name` is written within the declaration at `span`: its first
/// whole-word occurrence there. The start of the declaration if it cannot be
/// found (a method's qualified name, say), which is still inside it as the
/// protocol requires.
fn name_range(name: &str, span: SourceSpan, src: &[char]) -> Range {
    let word: Vec<char> = name.chars().collect();
    let (start, end) = (span.start.offset as usize, (span.end.offset as usize).min(src.len()));
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let found = (start..end.saturating_sub(word.len()).max(start) + 1).find(|&i| {
        src.get(i..i + word.len()) == Some(&word[..])
            && (i == 0 || !is_ident(src[i - 1]))
            && src.get(i + word.len()).is_none_or(|c| !is_ident(*c))
    });
    let Some(at) = found.filter(|_| !word.is_empty()) else {
        let start = span_to_range(&span).start;
        return Range { start, end: start };
    };
    // Line and column of `at`, counted on from the declaration's start.
    let mut pos = span_to_range(&span).start;
    for &c in &src[start..at] {
        if c == '\n' {
            pos.line += 1;
            pos.character = 0;
        } else {
            pos.character += 1;
        }
    }
    Range {
        start: pos,
        end: Position {
            line: pos.line,
            character: pos.character + word.len() as u32,
        },
    }
}
