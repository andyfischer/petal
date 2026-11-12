//! Edit paths — the text that names one value inside a top-level binding, for
//! the path edits of [`crate::literal_edit`] and
//! [`crate::rewrite::find_binding_path`].
//!
//! ```text
//! POST.effects[2].amount           a record field, a list element, a field
//! layers[2].x                      an attribute of an element
//! layers[2]/group[0]/image[1].x    into an element's children, xpath-style
//! layers[2]/*[0]/@x                any tag; `/@x` is the same as `.x`
//! ```
//!
//! A path is a top-level name followed by steps:
//!
//! | Step | Enters |
//! |---|---|
//! | `.name` | a record's field, or an element's attribute |
//! | `[n]` | a list's element, or a call's argument (`vec3(…)`) |
//! | `/tag[n]` | an element's `n`-th child element with that tag |
//! | `/*[n]` | an element's `n`-th child element of any tag but `<br />` |
//! | `/tag` | an element's only child element with that tag |
//! | `/@name` | an element's attribute (XPath's spelling of `.name`) |
//!
//! Indices count from 0, like Petal's lists (XPath counts from 1). Only child
//! *elements* are counted: text, `//` and `{/* */}` comments, whitespace and
//! `{expr}` holes are not children a path can name. `/*[n]` also skips
//! `<br />`, which is a line break in text rather than a child; `/br[n]`
//! names one explicitly. The full design is in `docs/features/edit-paths.md`.

use std::fmt;

use crate::ast::{Expr, ExprKind, JsxChild};

/// One step from a value into one of its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    /// `.name`: a record field, or an element attribute (also `/@name`).
    Field(String),
    /// `[n]`: an element of a list, or an argument of a call, which a config
    /// file uses as a constructor: `vec3(0.3, -1.0, 0.5)`.
    Index(usize),
    /// `/tag[n]`, `/*[n]` or `/tag`: a child element of an element.
    Child(ChildStep),
    /// The child element at this position among *all* of an element's child
    /// elements, `<br />` included. No path text spells it: it is what a
    /// [`PathSeg::Child`] resolves to, and what planned edits use.
    ChildAt(usize),
}

/// A child-element step: `/tag[n]` (`tag: Some`, `index: Some`), `/*[n]`
/// (`tag: None`), or `/tag` (`index: None`, the only such child).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildStep {
    pub tag: Option<String>,
    pub index: Option<usize>,
}

impl fmt::Display for ChildStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag.as_deref().unwrap_or("*"))?;
        if let Some(i) = self.index {
            write!(f, "[{i}]")?;
        }
        Ok(())
    }
}

impl fmt::Display for PathSeg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSeg::Field(name) => write!(f, ".{name}"),
            PathSeg::Index(i) => write!(f, "[{i}]"),
            PathSeg::Child(step) => write!(f, "/{step}"),
            PathSeg::ChildAt(i) => write!(f, "/node()[{i}]"),
        }
    }
}

/// `name` and `segs` written back as path text.
pub fn path_text(name: &str, segs: &[PathSeg]) -> String {
    let mut out = name.to_string();
    for seg in segs {
        out.push_str(&seg.to_string());
    }
    out
}

fn ident_len(s: &str) -> usize {
    s.char_indices()
        .find(|&(i, c)| !(c == '_' || c.is_alphabetic() || (i > 0 && c.is_ascii_digit())))
        .map_or(s.len(), |(i, _)| i)
}

/// A tag name as the lexer reads one: a letter, then letters, digits, `_`
/// and `-`.
fn tag_len(s: &str) -> usize {
    if !s.starts_with(|c: char| c.is_alphabetic()) {
        return 0;
    }
    s.char_indices()
        .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map_or(s.len(), |(i, _)| i)
}

/// Parse an edit path into its binding name and steps, or say what is wrong
/// with it: which step, and what was expected there.
pub fn parse_edit_path(path: &str) -> Result<(String, Vec<PathSeg>), String> {
    let full = path;
    let path = path.trim();
    let n = ident_len(path);
    if n == 0 {
        return Err(format!(
            "`{full}` is not an edit path: it must start with a binding name"
        ));
    }
    let (name, mut rest) = path.split_at(n);
    let mut segs = Vec::new();
    let bad = |rest: &str, what: &str| {
        let at = path.len() - rest.len();
        Err(format!(
            "`{full}` is not an edit path: {what} at `{}` (after `{}`)",
            rest,
            &path[..at]
        ))
    };
    // `[n]`, with `rest` just past the `[`: the index and what follows.
    fn index(after: &str) -> Option<(usize, &str)> {
        let close = after.find(']')?;
        Some((after[..close].trim().parse().ok()?, &after[close + 1..]))
    }
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let n = ident_len(after);
            if n == 0 {
                return bad(rest, "expected a field or attribute name after `.`");
            }
            segs.push(PathSeg::Field(after[..n].to_string()));
            rest = &after[n..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let Some((i, after)) = index(after) else {
                return bad(rest, "expected `[n]` with a whole number n");
            };
            segs.push(PathSeg::Index(i));
            rest = after;
        } else if let Some(after) = rest.strip_prefix("/@") {
            let n = ident_len(after);
            if n == 0 {
                return bad(rest, "expected an attribute name after `/@`");
            }
            segs.push(PathSeg::Field(after[..n].to_string()));
            rest = &after[n..];
        } else if let Some(after) = rest.strip_prefix('/') {
            let (tag, after) = if let Some(after) = after.strip_prefix('*') {
                (None, after)
            } else {
                let n = tag_len(after);
                if n == 0 {
                    return bad(
                        rest,
                        "expected a tag name, `*` or `@attribute` after `/`",
                    );
                }
                (Some(after[..n].to_string()), &after[n..])
            };
            if tag.as_deref() == Some("text") && after.starts_with("()") {
                return bad(
                    rest,
                    "text children cannot be named by a path (set the whole element instead)",
                );
            }
            let (index, after) = match after.strip_prefix('[') {
                Some(inner) => match index(inner) {
                    Some((i, after)) => (Some(i), after),
                    None => return bad(&rest[rest.len() - after.len()..], "expected `[n]` with a whole number n"),
                },
                None if tag.is_none() => {
                    return bad(rest, "`/*` needs an index: `/*[n]`");
                }
                None => (None, after),
            };
            segs.push(PathSeg::Child(ChildStep { tag, index }));
            rest = after;
        } else {
            return bad(rest, "expected `.name`, `[n]` or `/tag[n]`");
        }
    }
    Ok((name.to_string(), segs))
}

/// [`parse_edit_path`] without the explanation.
pub fn parse_binding_path(path: &str) -> Option<(String, Vec<PathSeg>)> {
    parse_edit_path(path).ok()
}

/// The child elements of an element literal, in order — what child steps
/// count. Text and `{expr}` holes that are not elements are left out.
pub fn element_children(expr: &Expr) -> Vec<&Expr> {
    let ExprKind::Element { children, .. } = &expr.kind else {
        return Vec::new();
    };
    children
        .iter()
        .filter_map(|child| match child {
            JsxChild::Expr(e) if matches!(e.kind, ExprKind::Element { .. }) => Some(e),
            _ => None,
        })
        .collect()
}

/// The tag of an element literal.
pub fn tag_of(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Element { tag, .. } => Some(tag),
        _ => None,
    }
}

/// The positions (into `children`, as [`element_children`] lists them) of
/// the children a step with this `tag` test counts.
fn candidates(children: &[&Expr], tag: Option<&str>) -> Vec<usize> {
    (0..children.len())
        .filter(|&i| {
            let t = tag_of(children[i]).unwrap_or("");
            match tag {
                Some(tag) => t == tag,
                None => t != "br",
            }
        })
        .collect()
}

fn describe_children(tag: Option<&str>, count: usize) -> String {
    let (one, many) = match tag {
        Some(tag) => (format!("`<{tag}>` child"), format!("`<{tag}>` children")),
        None => ("child element".to_string(), "child elements".to_string()),
    };
    match count {
        0 => format!("no {many}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// Which of `children` (an element's, as [`element_children`] lists them)
/// `step` names, as a position into `children`. `Err` says what is there
/// instead.
pub fn select_child(children: &[&Expr], step: &ChildStep) -> Result<usize, String> {
    let found = candidates(children, step.tag.as_deref());
    let tag = step.tag.as_deref();
    match step.index {
        Some(i) => found
            .get(i)
            .copied()
            .ok_or_else(|| format!("has {}", describe_children(tag, found.len()))),
        None => match found.as_slice() {
            [only] => Ok(*only),
            [] => Err(format!("has {}", describe_children(tag, 0))),
            many => Err(format!(
                "has {}, so `{step}` is ambiguous: write `{step}[0]` … `{step}[{}]`",
                describe_children(tag, many.len()),
                many.len() - 1
            )),
        },
    }
}

/// Where a child inserted at `step` goes, as a position into `children`: before
/// the child `step` names now, or — for an index one past the last such child
/// — just after the last one (at the end when there is none). `Err` finishes
/// the sentence "the element …".
pub fn insert_position(children: &[&Expr], step: &ChildStep) -> Result<usize, String> {
    let Some(i) = step.index else {
        return Err(format!(
            "needs an index for an insert: `{step}[n]` says where the new child goes"
        ));
    };
    let found = candidates(children, step.tag.as_deref());
    match (found.get(i), found.last()) {
        (Some(&at), _) => Ok(at),
        (None, Some(&last)) if i == found.len() => Ok(last + 1),
        (None, None) if i == 0 => Ok(children.len()),
        _ => Err(format!(
            "has {}, so index {i} is past the end",
            describe_children(step.tag.as_deref(), found.len())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(tag: Option<&str>, index: Option<usize>) -> PathSeg {
        PathSeg::Child(ChildStep {
            tag: tag.map(str::to_string),
            index,
        })
    }

    #[test]
    fn parses_child_steps() {
        assert_eq!(
            parse_edit_path("layers[2]/group[0]/image[1].x"),
            Ok((
                "layers".into(),
                vec![
                    PathSeg::Index(2),
                    child(Some("group"), Some(0)),
                    child(Some("image"), Some(1)),
                    PathSeg::Field("x".into()),
                ]
            ))
        );
        assert_eq!(
            parse_edit_path("doc/*[3]/@fill"),
            Ok((
                "doc".into(),
                vec![child(None, Some(3)), PathSeg::Field("fill".into())]
            ))
        );
        assert_eq!(
            parse_edit_path("card/my-tag"),
            Ok(("card".into(), vec![child(Some("my-tag"), None)]))
        );
    }

    #[test]
    fn displays_steps_as_written() {
        for path in ["a.b[2]/group[0]/image.x", "a/*[1]", "POST.effects[2].amount"] {
            let (name, segs) = parse_edit_path(path).unwrap();
            assert_eq!(path_text(&name, &segs), path);
        }
    }

    #[test]
    fn explains_a_bad_path() {
        let err = |p: &str| parse_edit_path(p).unwrap_err();
        assert!(err("").contains("must start with a binding name"));
        assert!(err("a..b").contains("after `.`"), "{}", err("a..b"));
        assert!(err("a[x]").contains("whole number"));
        assert!(err("a b").contains("expected `.name`"));
        assert!(err("a/*").contains("`/*` needs an index"));
        assert!(err("a/").contains("expected a tag name"));
        assert!(err("a/image[x]").contains("whole number"));
        assert!(err("a/text()[0]").contains("text children"));
        // A `text` tag is a tag like any other.
        assert!(parse_edit_path("a/text[0]").is_ok());
    }
}
