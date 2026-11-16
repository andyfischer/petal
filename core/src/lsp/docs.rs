//! A declaration's `@doc(…)` (docs/proposals/doc-annotation.md) as hover and
//! completion show it: Markdown under the signature.
//!
//! Hover shows the whole doc: what to use instead of a deprecated item, the
//! body, then the parameters, the option keys, the result, the examples and
//! the related names. Completion shows the body's first sentence as the
//! item's detail and the same Markdown beside the list. `hidden` and `extra`
//! are for reference generators and hosts, and are not shown.

use crate::ast::Doc;
use crate::lsp::lsp_types::{CompletionItem, CompletionItemTag, MarkupContent};

/// `doc` as Markdown, without the signature.
pub fn doc_markdown(doc: &Doc) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(instead) = &doc.deprecated {
        let instead = instead.trim();
        parts.push(if instead.is_empty() {
            "**Deprecated.**".to_string()
        } else {
            format!("**Deprecated:** {instead}")
        });
    }
    if let Some(body) = doc.cleaned_body().filter(|b| !b.is_empty()) {
        parts.push(body);
    }
    let table = |title: &str, pairs: &[(String, String)]| {
        let rows: Vec<String> = pairs
            .iter()
            .map(|(name, text)| format!("- `{name}`: {}", crate::doc_annotation::cleandoc(text)))
            .collect();
        format!("**{title}**\n\n{}", rows.join("\n"))
    };
    if !doc.params.is_empty() {
        parts.push(table("Parameters", &doc.params));
    }
    if !doc.options.is_empty() {
        parts.push(table("Options", &doc.options));
    }
    if let Some(returns) = &doc.returns {
        parts.push(format!("**Returns:** {}", crate::doc_annotation::cleandoc(returns)));
    }
    for example in doc.cleaned_examples() {
        parts.push(format!("```petal\n{example}\n```"));
    }
    if !doc.see.is_empty() {
        let names: Vec<String> = doc.see.iter().map(|n| format!("`{n}`")).collect();
        parts.push(format!("**See:** {}", names.join(", ")));
    }
    parts.join("\n\n")
}

/// A hover: `code` (a signature, a module's name) as a Petal code block,
/// with `doc` below it when there is one.
pub fn hover_markdown(code: &str, doc: Option<&Doc>) -> String {
    let mut out = format!("```petal\n{code}\n```");
    if let Some(text) = doc.map(doc_markdown).filter(|t| !t.is_empty()) {
        out.push_str("\n\n");
        out.push_str(&text);
    }
    out
}

/// The doc's summary, when it has a non-empty one.
pub fn summary(doc: &Doc) -> Option<String> {
    doc.summary().filter(|s| !s.is_empty())
}

/// Give `item` its declaration's doc: the summary as its detail, the
/// signature (the detail it had) and the whole doc as its documentation, and
/// the deprecated tag. An item with no doc is left as it is.
pub fn document_item(item: &mut CompletionItem, doc: Option<&Doc>) {
    let Some(doc) = doc else {
        return;
    };
    let signature = item.detail.take();
    let text = doc_markdown(doc);
    let documentation = match &signature {
        Some(sig) if !sig.is_empty() => hover_markdown(&format!("{} {sig}", item.label), Some(doc)),
        _ => text,
    };
    item.detail = summary(doc).or(signature);
    if !documentation.is_empty() {
        item.documentation = Some(MarkupContent {
            kind: "markdown".to_string(),
            value: documentation,
        });
    }
    if doc.deprecated.is_some() {
        item.tags = Some(vec![CompletionItemTag::Deprecated]);
    }
}
