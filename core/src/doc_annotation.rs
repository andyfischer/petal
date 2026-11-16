//! `@doc(…)` and `@module_doc(…)`: reading a documentation annotation's
//! literal arguments into a [`Doc`] (docs/proposals/doc-annotation.md).
//!
//! Both parse pipelines call [`doc_from_annotation`]: the parser to check the
//! arguments (its errors are the ones a user sees), the CST projection to
//! rebuild the same [`Doc`] from the tree. Every argument is a literal, so a
//! doc is data the parser alone can read, and nothing here reaches the IR.

use crate::ast::{
    Annotation, Doc, DocValue, Expr, ExprKind, Literal, MODULE_DOC_ANNOTATION, RecordField,
    UnaryOp,
};
use crate::source_map::SourceSpan;

/// The fields a `@doc` takes, in the order the guide lists them.
pub const DOC_FIELDS: [&str; 8] =
    ["params", "returns", "options", "example", "see", "deprecated", "hidden", "extra"];

/// The fields a `@module_doc` takes: it documents a file, which has no
/// parameters, result or options.
pub const MODULE_DOC_FIELDS: [&str; 4] = ["see", "deprecated", "hidden", "extra"];

/// An error about a doc annotation: where it points and what it says.
pub type DocError = (SourceSpan, String);

/// Read `ann` (a `@doc` or `@module_doc`, by its name) into a [`Doc`] written
/// at `span`, or say what is wrong with it: a computed or interpolated
/// argument, an unknown, repeated or misplaced field, or a field of the wrong
/// shape.
pub(crate) fn doc_from_annotation(ann: &Annotation, span: SourceSpan) -> Result<Doc, DocError> {
    let module = ann.name == MODULE_DOC_ANNOTATION;
    let at = format!("`@{}`", ann.name);
    let mut doc = Doc::empty(span);
    let mut seen: Vec<&str> = Vec::new();
    for (i, arg) in ann.args.iter().enumerate() {
        let name = ann.arg_names.get(i).and_then(|n| n.as_deref());
        let Some(name) = name else {
            if i > 0 {
                return Err((
                    arg.span,
                    format!(
                        "{at} takes one positional argument, the body; name the others \
                         (`returns: …`, `see: […]`)"
                    ),
                ));
            }
            doc.body = Some(string(arg, &at, "its body")?);
            continue;
        };
        if name == crate::ast::SPREAD_ARG {
            return Err((arg.span, not_literal(&at, arg)));
        }
        let fields: &[&str] = if module { &MODULE_DOC_FIELDS } else { &DOC_FIELDS };
        if !fields.contains(&name) {
            return Err((arg.span, unknown_field(&at, name, module)));
        }
        if seen.contains(&name) {
            return Err((arg.span, format!("{at} gives `{name}:` twice")));
        }
        seen.push(name);
        let what = format!("`{name}:`");
        match name {
            "params" => doc.params = string_record(arg, &at, &what)?,
            "options" => doc.options = string_record(arg, &at, &what)?,
            "returns" => doc.returns = Some(string(arg, &at, &what)?),
            "deprecated" => doc.deprecated = Some(string(arg, &at, &what)?),
            "example" => {
                doc.example = match &arg.kind {
                    ExprKind::List(items) => items
                        .iter()
                        .map(|e| string(e, &at, "each `example:`"))
                        .collect::<Result<_, _>>()?,
                    _ => vec![string_or(arg, &at, &what, "a string, or a list of strings")?],
                }
            }
            "see" => {
                let ExprKind::List(items) = &arg.kind else {
                    literal(arg, &at)?;
                    return Err((
                        arg.span,
                        format!("{at}'s `see:` is a list of names: `see: [\"lerp\", \"ui/drag\"]`"),
                    ));
                };
                doc.see = items
                    .iter()
                    .map(|e| string(e, &at, "each `see:` entry"))
                    .collect::<Result<_, _>>()?;
            }
            "hidden" => match literal(arg, &at)? {
                DocValue::Bool(b) => doc.hidden = b,
                _ => return Err((arg.span, format!("{at}'s `hidden:` is `true` or `false`"))),
            },
            "extra" => match literal(arg, &at)? {
                DocValue::Record(fields) => doc.extra = fields,
                _ => {
                    return Err((
                        arg.span,
                        format!("{at}'s `extra:` is a record of fields: `extra: {{since: \"0.4\"}}`"),
                    ));
                }
            },
            _ => unreachable!("every field is handled"),
        }
    }
    Ok(doc)
}

/// The message for a field `name` that `@doc`/`@module_doc` does not take.
fn unknown_field(at: &str, name: &str, module: bool) -> String {
    if module && DOC_FIELDS.contains(&name) {
        return format!(
            "{at} takes no `{name}:`: it documents a file, not a function. Its fields are \
             `see`, `deprecated`, `hidden` and `extra`"
        );
    }
    let fields: &[&str] = if module { &MODULE_DOC_FIELDS } else { &DOC_FIELDS };
    let budget = if name.len() <= 4 { 1 } else { 2 };
    match crate::diagnostic::closest(name, fields.iter().copied(), budget) {
        Some(near) => format!("{at} has no field `{name}:`; did you mean `{near}:`?"),
        None => format!(
            "{at} has no field `{name}:`; put tool-specific fields in `extra: {{{name}: …}}`"
        ),
    }
}

/// `arg` as a string literal, or an error saying `what` is text.
fn string(arg: &Expr, at: &str, what: &str) -> Result<String, DocError> {
    string_or(arg, at, what, "a string")
}

fn string_or(arg: &Expr, at: &str, what: &str, shape: &str) -> Result<String, DocError> {
    match literal(arg, at)? {
        DocValue::String(s) => Ok(s),
        _ => Err((
            arg.span,
            format!("{at}: {what} is {shape}: write it in quotes, or as a raw string \"\"\"…\"\"\""),
        )),
    }
}

/// `arg` as a record whose every value is a string (`params:`, `options:`).
fn string_record(arg: &Expr, at: &str, what: &str) -> Result<Vec<(String, String)>, DocError> {
    let ExprKind::Record(fields) = &arg.kind else {
        literal(arg, at)?;
        return Err((
            arg.span,
            format!("{at}: {what} is a record of strings: `{{name: \"…\"}}`"),
        ));
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for field in fields {
        let RecordField::Named(key, value) = field else {
            return Err((arg.span, spread_message(at)));
        };
        if out.iter().any(|(k, _)| k == key) {
            return Err((value.span, format!("{at}: {what} names `{key}` twice")));
        }
        out.push((key.clone(), string(value, at, &format!("`{key}`"))?));
    }
    Ok(out)
}

/// `e` as a literal value: a string, number, bool or `nil`, or a list or
/// record of those. Anything computed is an error pointing at it.
pub fn literal(e: &Expr, at: &str) -> Result<DocValue, DocError> {
    Ok(match &e.kind {
        ExprKind::Literal(Literal::Nil) => DocValue::Nil,
        ExprKind::Literal(Literal::Bool(b)) => DocValue::Bool(*b),
        ExprKind::Literal(Literal::Int(n)) => DocValue::Int(*n),
        ExprKind::Literal(Literal::Float(x)) => DocValue::Float(*x),
        ExprKind::Literal(Literal::String(s)) => DocValue::String(s.clone()),
        ExprKind::UnaryOp { op: UnaryOp::Neg, operand } => match &operand.kind {
            ExprKind::Literal(Literal::Int(n)) => DocValue::Int(-n),
            ExprKind::Literal(Literal::Float(x)) => DocValue::Float(-x),
            _ => return Err((e.span, not_literal(at, e))),
        },
        ExprKind::List(items) => {
            DocValue::List(items.iter().map(|i| literal(i, at)).collect::<Result<_, _>>()?)
        }
        ExprKind::Record(fields) => {
            let mut out: Vec<(String, DocValue)> = Vec::new();
            for field in fields {
                let RecordField::Named(key, value) = field else {
                    return Err((e.span, spread_message(at)));
                };
                if out.iter().any(|(k, _)| k == key) {
                    return Err((value.span, format!("{at}: this record names `{key}` twice")));
                }
                out.push((key.clone(), literal(value, at)?));
            }
            DocValue::Record(out)
        }
        ExprKind::StringInterp { .. } => return Err((e.span, interpolation_message(at, None))),
        _ => return Err((e.span, not_literal(at, e))),
    })
}

fn spread_message(at: &str) -> String {
    format!("{at} takes text written out: a record here lists its fields, with no `...` spread or computed `[key]`")
}

/// The error for an argument that is not a literal.
fn not_literal(at: &str, e: &Expr) -> String {
    let what = match &e.kind {
        ExprKind::Ident(name) => format!("`{name}` is a name"),
        ExprKind::Call { .. } => "this is a call".to_string(),
        _ => "this is computed".to_string(),
    };
    format!(
        "{at} takes only literals (a string, number, bool or nil, or a list or record of \
         them), since tools read it without running the program; {what}"
    )
}

/// The error for a `"…"` string with an interpolation hole in a doc
/// argument. `hole` is the hole as written (`{bg, fg}`), when known.
pub fn interpolation_message(at: &str, hole: Option<&str>) -> String {
    let hole = hole.unwrap_or("{…}");
    format!(
        "{at} takes text written out; `{hole}` in a \"…\" string is an interpolation: write `\\{{` \
         for a literal brace, or use a raw string: \"\"\"…\"\"\""
    )
}

/// A doc string as it is displayed: Python's `inspect.cleandoc`. The first
/// line loses its leading whitespace, every later line loses the indentation
/// they have in common, and leading and trailing blank lines go. Lines that
/// hold only whitespace become empty. The source keeps the text as written,
/// so a `"""` block can be indented with the code around it.
pub fn cleandoc(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let line = if line.trim().is_empty() {
            ""
        } else if i == 0 {
            line.trim_start()
        } else {
            &line[indent.min(line.len())..]
        };
        out.push(line.trim_end().to_string());
    }
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// The summary of a doc body: its first sentence, which index pages,
/// completion and hover headers show. The first paragraph of the cleaned
/// text, its lines joined by spaces, up to and including the first `.`, `!`
/// or `?` that ends the text or is followed by whitespace. A paragraph with
/// no such stop is the summary whole.
pub fn summary(body: &str) -> String {
    let cleaned = cleandoc(body);
    let paragraph = cleaned.split("\n\n").next().unwrap_or("");
    let joined = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|n| n.is_whitespace()) {
            return chars[..=i].iter().collect();
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::{cleandoc, summary};

    #[test]
    fn summary_is_the_first_sentence_of_the_first_paragraph() {
        assert_eq!(summary("Arithmetic and rounding. And more."), "Arithmetic and rounding.");
        assert_eq!(summary("\n  A UI library\n  written in Petal.\n\n  Second.\n"), "A UI library written in Petal.");
        assert_eq!(summary("Version 0.4 is out"), "Version 0.4 is out");
        assert_eq!(summary("No stop\n\nNext para."), "No stop");
    }

    /// Source tools find a declaration's keyword past its docs, whose raw
    /// strings hold quotes, backslashes, brackets and `#`.
    #[test]
    fn skip_annotations_passes_doc_raw_strings() {
        let src = "@doc(\"\"\"A \"quoted\" \\ (paren # not a comment\n\"\"\", see: [\"x\"])\n@tunable let X = 1";
        let chars: Vec<char> = src.chars().collect();
        let at = crate::ast::skip_annotations(&chars, 0);
        assert_eq!(chars[at..].iter().collect::<String>(), "let X = 1");
    }

    #[test]
    fn cleandoc_strips_common_indentation_and_blank_ends() {
        assert_eq!(cleandoc("One line."), "One line.");
        assert_eq!(cleandoc("\n    clamp(1)   # 1\n    clamp(9)\n  "), "clamp(1)   # 1\nclamp(9)");
        assert_eq!(
            cleandoc("Summary.\n\n  Detail\n    indented\n"),
            "Summary.\n\nDetail\n  indented"
        );
        assert_eq!(cleandoc("\nA {x, y} record.\n"), "A {x, y} record.");
        assert_eq!(cleandoc("   \n\n"), "");
    }
}
