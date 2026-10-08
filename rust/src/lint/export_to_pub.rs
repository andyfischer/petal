//! Rule — spell the export modifier `pub`.
//!
//! ```text
//! export fn area(r) r.w * r.h end      =>   pub fn area(r) r.w * r.h end
//! export import bloom/button: *        =>   pub import bloom/button: *
//! ```
//!
//! `export` is the deprecated spelling of `pub`: the parser accepts both and
//! builds the same declaration, so the rewrite is one keyword splice (and, at
//! most, some padding in front of a trailing comment) and the compiled
//! program does not change. It fires on every form the modifier
//! takes — `fn`, `let`, `var`, `config let`, `state`, `enum`, `class` and
//! `import`.
//!
//! The sites come from [`crate::export_keyword`], which reads them off the
//! AST; `export` as a field or record-key name (`r.export`) is not one.

use crate::ast::Stmt;
use crate::export_keyword::{DEPRECATED, REPLACEMENT, deprecated_export_spans, plan_pub_rewrite};

use super::Fix;
use super::to_match::Splice;

/// Plan every `export` → `pub` respelling in `stmts`, in source order.
/// `chars` is the text they were parsed from.
pub(super) fn plan_export_fixes(stmts: &[Stmt], chars: &[char]) -> Result<Vec<Fix>, String> {
    let sites = deprecated_export_spans(stmts, chars);
    let source: String = chars.iter().collect();
    // The keyword splice, plus padding where the shorter word would pull a
    // trailing comment out of a column it shares (see `plan_pub_rewrite`).
    let edits = plan_pub_rewrite(&source, &sites)?;
    Ok(sites
        .iter()
        .zip(edits)
        .map(|(site, edits)| Fix {
            anchor: site.start.offset as usize,
            message: format!("`{DEPRECATED}` is deprecated; write `{REPLACEMENT}`"),
            splices: edits
                .into_iter()
                .map(|e| Splice {
                    start: e.start,
                    end: e.end,
                    text: e.text.to_string(),
                })
                .collect(),
        })
        .collect())
}
