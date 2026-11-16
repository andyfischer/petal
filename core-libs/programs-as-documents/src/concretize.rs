//! Step 5 of the ladder: write an output out as source.
//!
//! When nothing can be edited in place, the construction that made an
//! output (a helper call such as `card(2)` in a list literal) can be replaced
//! by the literal of the value it evaluated to (`<card x={2} w={40} />`),
//! and the intent applied to that. This trades the abstraction for
//! editability, so the resolver does it only when the caller says so
//! ([`ResolveOptions::concretize`](crate::ResolveOptions::concretize)).
//!
//! The value is read with [`StaticValue::from_value`], so field order and
//! int versus float are kept; a record that is a color at run time
//! (`{r, g, b}` with byte values) is written back as a color literal. A
//! value with no literal form (a function in a prop) cannot be written out.
//!
//! With [`ConcretizeOptions::tidy`] ([`concretize_with`]) the literal is
//! first written as a person would write it ([`tidied`]): no `nil` props or
//! `nil` children, a whole float as an int (`594.0` → `594`) and a text
//! child with line breaks as lines with `<br />` between them. That form is
//! kept only when every output of the written-out document is the same as
//! before up to those differences (compared through [`tidied`] on both
//! sides); otherwise the exact literal is written, as without the option.

use petal::program::TermOp;
use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;

use crate::intent::same;
use crate::output_map::OutputNode;
use crate::run::{DocumentRun, Runner};
use crate::splice::byte_range;

/// An output written out as source.
#[derive(Debug, Clone, PartialEq)]
pub struct Concretized {
    /// The whole new source; its result is the same as before.
    pub source: String,
    /// The construction that was replaced, in the old source.
    pub span: SourceSpan,
    /// "write out `card(2)` (line 5) as `<card>`".
    pub description: String,
    /// Whether the tidied literal ([`tidied`]) was written; `false` for the
    /// exact one.
    pub tidy: bool,
}

/// How [`concretize_with`] writes the literal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConcretizeOptions {
    /// Write the value as a person would ([`tidied`]) when the document's
    /// outputs stay the same up to the tidying; the exact literal otherwise.
    /// Off by default: a host that tells a `nil` prop from a missing one, or
    /// `"a\nb"` from `a<br />b`, wants the exact literal.
    pub tidy: bool,
}

/// `v` as an element written by hand would hold it: no `nil` props or
/// children (on elements; a record keeps its `nil` fields), a whole float
/// that converts to an int and back unchanged as that int (`594.0` → `594`),
/// and a text child with line breaks as a run of text per line with `<br />`
/// between them (an empty line has no run).
pub fn tidied(v: StaticValue) -> StaticValue {
    match v {
        StaticValue::Element { tag, props, children } => {
            let mut kids = Vec::with_capacity(children.len());
            for c in children {
                match c {
                    StaticValue::Nil => {}
                    StaticValue::Str(s) if s.contains('\n') => kids.extend(crate::text::text_children(&s)),
                    c => kids.push(tidied(c)),
                }
            }
            StaticValue::Element {
                tag,
                props: props.into_iter().filter(|(_, v)| *v != StaticValue::Nil).map(|(k, v)| (k, tidied(v))).collect(),
                children: kids,
            }
        }
        StaticValue::Float(f) => match whole(f) {
            Some(n) => StaticValue::Int(n),
            None => StaticValue::Float(f),
        },
        StaticValue::List(xs) => StaticValue::List(xs.into_iter().map(tidied).collect()),
        StaticValue::Record(fs) => StaticValue::Record(fs.into_iter().map(|(k, v)| (k, tidied(v))).collect()),
        other => other,
    }
}

/// `f` as an int when it converts to one and back unchanged (not `-0.0`,
/// nothing past 2^53, where floats skip integers).
fn whole(f: f64) -> Option<i64> {
    const LIMIT: f64 = 9_007_199_254_740_992.0;
    if !f.is_finite() || f.fract() != 0.0 || f.abs() > LIMIT || (f == 0.0 && f.is_sign_negative()) {
        return None;
    }
    let n = f as i64;
    (n as f64 == f).then_some(n)
}

/// Records that are colors at run time, back as color literals.
pub fn colors_from_records(v: StaticValue) -> StaticValue {
    match v {
        StaticValue::Record(fs) => {
            let byte = |k: &str| match fs.iter().find(|(n, _)| n == k).map(|(_, v)| v) {
                Some(StaticValue::Int(n)) if (0..=255).contains(n) => Some(*n as u8),
                _ => None,
            };
            let keys: Vec<&str> = fs.iter().map(|(k, _)| k.as_str()).collect();
            let rgb = keys == ["r", "g", "b"] || keys == ["r", "g", "b", "a"];
            match (rgb, byte("r"), byte("g"), byte("b")) {
                (true, Some(r), Some(g), Some(b)) if keys.len() == 3 || byte("a").is_some() => {
                    StaticValue::Color { r, g, b, a: byte("a") }
                }
                _ => StaticValue::Record(fs.into_iter().map(|(k, v)| (k, colors_from_records(v))).collect()),
            }
        }
        StaticValue::List(xs) => StaticValue::List(xs.into_iter().map(colors_from_records).collect()),
        StaticValue::Element { tag, props, children } => StaticValue::Element {
            tag,
            props: props.into_iter().map(|(k, v)| (k, colors_from_records(v))).collect(),
            children: children.into_iter().map(colors_from_records).collect(),
        },
        other => other,
    }
}

/// `text`'s lines after the first, indented to sit under a construction that
/// starts on a line indented by `indent`.
fn indent_after_first(text: &str, indent: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

/// Replace the construction that built `node` with the literal of its
/// value. The innermost call on its path written in the document's file is
/// tried first, then the ones around it; the first whose replacement leaves
/// the whole result unchanged is taken.
pub fn concretize(runner: &Runner, run: &DocumentRun, node: &OutputNode) -> Result<Concretized, String> {
    concretize_with(runner, run, node, &ConcretizeOptions::default())
}

/// [`concretize`], with the literal written as `opts` says (with
/// [`ConcretizeOptions::tidy`], each construction is tried tidied first,
/// then exact).
pub fn concretize_with(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    opts: &ConcretizeOptions,
) -> Result<Concretized, String> {
    let tag = &node.tag;
    let site = node
        .site
        .as_ref()
        .ok_or_else(|| format!("<{tag}> was not built by an element literal, so it cannot be written out"))?;
    let value = run
        .node_value(node)
        .ok_or_else(|| format!("<{tag}> holds a value with no literal form (a function?), so it cannot be written out"))?;
    let exact = colors_from_records(value);
    // (literal, tidy): the tidied one first, when asked for and different.
    let mut literals = Vec::new();
    if opts.tidy {
        let t = tidied(exact.clone());
        if t != exact {
            literals.push((t.to_source(), true));
        }
    }
    literals.push((exact.to_source(), false));
    let before: Vec<Option<StaticValue>> = run.static_outputs();
    let tidy_before: Vec<Option<StaticValue>> = before.iter().map(|v| v.clone().map(tidied)).collect();
    let program = run.program();
    let mut tried = Vec::new();
    for (call, span) in site.chain.iter().zip(&site.chain_spans) {
        let Some(span) = span else { continue };
        if span.file != FileId(0) {
            continue;
        }
        if !program
            .terms
            .get(call.0 as usize)
            .is_some_and(|t| matches!(t.op, TermOp::Call | TermOp::MethodCall { .. }))
        {
            continue;
        }
        let Some(range) = byte_range(&run.source, span) else { continue };
        let line_start = run.source[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let indent: String = run.source[line_start..range.start].chars().take_while(|c| c.is_whitespace()).collect();
        let text = &run.source[range.clone()];
        for (literal, tidy) in &literals {
            let mut source = String::with_capacity(run.source.len() + literal.len());
            source.push_str(&run.source[..range.start]);
            source.push_str(&indent_after_first(literal, &indent));
            source.push_str(&run.source[range.end..]);
            let after = match runner.run(&source) {
                Ok(a) => a,
                Err(e) => {
                    if !tidy {
                        tried.push(format!("`{}` (line {}): {e}", one_line(text), span.start.line));
                    }
                    continue;
                }
            };
            let got = after.static_outputs();
            let unchanged = if *tidy {
                got.len() == tidy_before.len()
                    && got.into_iter().zip(&tidy_before).all(|(a, b)| match (a, b) {
                        (Some(a), Some(b)) => same(&tidied(a), b),
                        _ => false,
                    })
            } else {
                got.len() == before.len()
                    && got.iter().zip(&before).all(|(a, b)| matches!((a, b), (Some(a), Some(b)) if same(a, b)))
            };
            if !unchanged {
                if !tidy {
                    tried.push(format!("`{}` (line {}) makes more than this <{tag}>", one_line(text), span.start.line));
                }
                continue;
            }
            return Ok(Concretized {
                source,
                span: *span,
                description: format!("write out `{}` (line {}) as <{tag}>", one_line(text), span.start.line),
                tidy: *tidy,
            });
        }
    }
    if tried.is_empty() {
        Err(format!(
            "<{tag}> is not made by a call in this document, so there is no construction to write out"
        ))
    } else {
        Err(format!("no construction of <{tag}> can be written out: {}", tried.join("; ")))
    }
}

fn one_line(s: &str) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s }
}
