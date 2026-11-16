//! Migration `comment-to-doc`: the comment block above a declaration becomes
//! its `@doc`, and a file's opening comment block its `@module_doc`
//! (docs/proposals/doc-annotation.md, Migration step 1).
//!
//! ```text
//! # Sum of a list of numbers; 0 for the empty list.   =>   @doc("Sum of a list of numbers; 0 for the empty list.")
//! pub fn sum(xs)                                            pub fn sum(xs)
//! ```
//!
//! A comment is never documentation (decision 3 of the proposal), so this is
//! no rewrite of an old *form*: comments stay the language. It moves what was
//! written as documentation by convention into the syntax that is, and it is
//! **opt-in**: it runs only when `--only comment-to-doc` names it, over the
//! files and directories given. The rules:
//!
//! - **Declarations.** For each top-level `pub` declaration (`fn`, `fn T.m`,
//!   `native fn`, `class`, `enum`, `let`, `var`, `state`, `memo`; every one
//!   with `--private`) that has no `@doc`, the run of whole-line comments
//!   directly above it (no blank line between, at the declaration's
//!   indentation) becomes `@doc(…)`, in the same place.
//! - **The module.** A file's opening comment block, when a blank line
//!   separates it from the first statement, becomes `@module_doc(…)`. A
//!   shebang line and a licence block (one naming a copyright or licence) are
//!   passed over, so the block after them is the opening one. A block
//!   directly above the first statement is that statement's.
//! - **Left alone.** Section rulers (`# ── Switch ──`, `# =====`): a block's
//!   doc is what follows its last ruler, and a ruler at the edge of a file
//!   header stays above or below the `@module_doc`. Blocks that hold a
//!   `TODO`/`NOTE`/`FIXME`/`XXX`/`HACK` line or a licence line: a block of
//!   only those is passed over, and one that also holds other text is
//!   reported for a hand edit (split the note from the doc). Comments inside
//!   bodies, and trailing comments. `# petal-lint-ignore` directives directly
//!   above the declaration stay there, below the new `@doc`; one higher up in
//!   the block leaves it for a hand edit.
//! - **Settings** (`--settings`): the trailing comment of a top-level
//!   `tunable` `let` with no doc (`let g = tunable(980.0)   # px/s^2`)
//!   becomes its `@doc`, written before it on its line, `pub` or not: a
//!   settings panel labels the knob with it.
//!
//! **The text.** Each line loses its indentation and the `#`s every line of
//! the block starts with, then the leading spaces every non-empty line
//! shares; blank comment lines at the ends go. One line with no `{`, `}`,
//! `"` or `\` is written as a plain string; anything else as a raw string
//! (`"""` on lines of its own around a multi-line text), so a `{x, y}` in the
//! prose is never an interpolation. A text that holds `"""` is written as an
//! escaped plain string.
//!
//! **Gate.** The output parses to the program the input parsed to, docs
//! aside, and when the input compiles, the output compiles to IR that
//! `petal ir-equal` reports as equivalent. **Idempotent:** a declaration
//! with a `@doc`, and a file with a `@module_doc`, are passed over.

use crate::ast::{ExprKind, Stmt, StmtKind};
use crate::lint::{Fix, LintOptions};
use crate::rewrite::Splice;

/// What `--private` and `--settings` turn on.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DocRules {
    pub(super) private: bool,
    pub(super) settings: bool,
}

/// What one comment line is, for the rules above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// Text: part of a doc.
    Prose,
    /// `#` alone.
    Blank,
    /// `# ── Section ──`, `# =====`, `######`.
    Ruler,
    /// `# petal-lint-ignore …`: an instruction to the tools.
    Directive,
    /// `# TODO …`, `# NOTE …`, `# FIXME …`: a note to maintainers.
    Note,
    /// `# Copyright …`, `# SPDX-License-Identifier: …`.
    Licence,
}

fn classify(comment: &str) -> Line {
    let body = comment.trim_start_matches('#');
    let text = body.trim();
    if text.is_empty() {
        // A run of `#`s long enough to be a rule; a lone `#` is a blank line.
        return if comment.len() >= 3 { Line::Ruler } else { Line::Blank };
    }
    let first = text.chars().next().unwrap();
    let run = text.chars().take_while(|&c| c == first).count();
    if matches!(first, '─' | '━' | '═' | '┄' | '╌') && run >= 2
        || matches!(first, '-' | '=' | '*' | '~' | '_' | '+') && run >= 3
    {
        return Line::Ruler;
    }
    if text.starts_with("petal-lint-ignore") || text.starts_with("petal-") && !text.contains(' ') {
        return Line::Directive;
    }
    for word in ["TODO", "NOTE", "FIXME", "XXX", "HACK"] {
        if let Some(rest) = text.strip_prefix(word)
            && !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            return Line::Note;
        }
    }
    let lower = text.to_lowercase();
    if lower.contains("copyright")
        || lower.contains("spdx-license-identifier")
        || lower.starts_with("licensed under")
        || lower.starts_with("license:")
        || lower.starts_with("licence:")
        || text.starts_with("(c) ")
        || text.starts_with("© ")
    {
        return Line::Licence;
    }
    Line::Prose
}

/// The source as lines of chars, with where each starts.
struct Lines {
    starts: Vec<usize>,
    text: Vec<String>,
}

impl Lines {
    fn new(chars: &[char]) -> Self {
        let mut starts = vec![0];
        let mut text = vec![String::new()];
        for (i, &c) in chars.iter().enumerate() {
            if c == '\n' {
                starts.push(i + 1);
                text.push(String::new());
            } else {
                text.last_mut().unwrap().push(c);
            }
        }
        Lines { starts, text }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }

    /// Whether line `i` is a whole-line comment written at `indent`.
    fn comment_at(&self, i: usize, indent: &str) -> Option<&str> {
        let line = self.text[i].trim_end();
        let rest = line.strip_prefix(indent)?;
        (rest.starts_with('#') && !(i == 0 && rest.starts_with("#!"))).then_some(rest)
    }

    /// The splice that replaces lines `from..=to` whole (newline included).
    fn replace(&self, from: usize, to: usize, text: String) -> Splice {
        let end = self.starts.get(to + 1).copied().unwrap_or_else(|| self.starts[to] + self.text[to].chars().count());
        Splice { start: self.starts[from], end, text }
    }
}

/// The doc text of comment lines (each starting at its `#`): the `#`s every
/// line shares, then the spaces every non-empty line shares, removed; blank
/// lines at the ends dropped.
fn doc_text(comments: &[&str]) -> String {
    let hashes = comments
        .iter()
        .map(|c| c.chars().take_while(|&ch| ch == '#').count())
        .min()
        .unwrap_or(1);
    let bodies: Vec<&str> = comments.iter().map(|c| c[hashes..].trim_end()).collect();
    let pad = bodies
        .iter()
        .filter(|b| !b.trim().is_empty())
        .map(|b| b.len() - b.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = bodies.iter().map(|b| if b.trim().is_empty() { "" } else { &b[pad..] }).collect();
    let first = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let last = lines.iter().rposition(|l| !l.is_empty()).map_or(first, |i| i + 1);
    lines[first..last].join("\n")
}

/// `@name(text)`, with the string written the way the module doc says, its
/// continuation lines at `indent`.
fn annotation(name: &str, text: &str, indent: &str) -> String {
    let plain = |s: &str| !s.contains(['{', '}', '"', '\\']);
    if !text.contains('\n') && plain(text) {
        return format!("@{name}(\"{text}\")");
    }
    if text.contains("\"\"\"") {
        let mut escaped = String::new();
        for c in text.chars() {
            match c {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '{' => escaped.push_str("\\{"),
                '\n' => escaped.push_str("\\n"),
                c => escaped.push(c),
            }
        }
        return format!("@{name}(\"{escaped}\")");
    }
    // A quote next to the delimiter would read as part of it: those take
    // the delimiters on lines of their own.
    if !text.contains('\n') && !text.starts_with('"') && !text.ends_with('"') {
        return format!("@{name}(\"\"\"{text}\"\"\")");
    }
    let body: Vec<String> = text
        .split('\n')
        .map(|l| if l.is_empty() { String::new() } else { format!("{indent}{l}") })
        .collect();
    format!("@{name}(\"\"\"\n{}\n{indent}\"\"\")", body.join("\n"))
}

/// The name a declaration binds, if it is one a `@doc` goes on.
fn decl_name(s: &Stmt) -> Option<&str> {
    match &s.kind {
        StmtKind::FnDecl { name, .. }
        | StmtKind::EnumDecl { name, .. }
        | StmtKind::ClassDecl { name, .. }
        | StmtKind::Let { name, .. }
        | StmtKind::State { name, .. }
        | StmtKind::Memo { name, .. } => Some(name),
        StmtKind::NativeFn(decl) => Some(&decl.name),
        _ => None,
    }
}

/// Whether `s` is a `let` whose value is a `tunable(…)` (written as a call,
/// or as `@tunable`).
fn is_tunable(s: &Stmt) -> bool {
    let StmtKind::Let { value, is_var: false, .. } = &s.kind else {
        return false;
    };
    matches!(&value.kind, ExprKind::Call { function, .. }
        if matches!(&function.kind, ExprKind::Ident(f) if f == "tunable"))
}

/// Plan every conversion in `source`, in source order.
pub(super) fn plan_doc_fixes(source: &str, rules: DocRules) -> Result<Vec<Fix>, String> {
    let (chars, stmts) = super::reparse(source)?;
    let lines = Lines::new(&chars);
    let mut fixes = Vec::new();
    if let Some(fix) = plan_module_doc(&lines, &stmts) {
        fixes.push(fix);
    }
    let trailing = if rules.settings { crate::fmt::trailing_comments(source)? } else { Vec::new() };
    for (i, s) in stmts.iter().enumerate() {
        let Some(name) = decl_name(s) else { continue };
        if s.test.is_some() || s.doc.is_some() {
            continue;
        }
        let start = s.span.start.offset as usize;
        let decl_line = lines.line_of(start);
        // The block may start only below the line the statement before ends
        // on (so nothing inside a string or a body is read as a comment).
        let floor = match i {
            0 => 0,
            _ => lines.line_of((stmts[i - 1].span.end.offset as usize).saturating_sub(1)) + 1,
        };
        let indent: String = chars[lines.starts[decl_line]..start].iter().collect();
        if !indent.chars().all(|c| c == ' ' || c == '\t') {
            continue;
        }
        let mut done = false;
        if (s.exported || rules.private)
            && let Some(fix) = plan_decl_doc(&lines, decl_line, floor, &indent, name, start)
        {
            done = true;
            fixes.push(fix);
        }
        if !done && rules.settings && is_tunable(s) {
            let end_line = lines.line_of((s.span.end.offset as usize).saturating_sub(1).max(start));
            if let Some(fix) = plan_setting_doc(&lines, &trailing, end_line, name, start) {
                fixes.push(fix);
            }
        }
    }
    Ok(fixes)
}

/// The `@doc` for the comment block above `decl_line`, or the report that
/// leaves it for a hand edit. `None` when there is no block, or none that is
/// a doc (only notes, a licence, rulers).
fn plan_decl_doc(lines: &Lines, decl_line: usize, floor: usize, indent: &str, name: &str, anchor: usize) -> Option<Fix> {
    let mut top = decl_line;
    while top > floor && lines.comment_at(top - 1, indent).is_some() {
        top -= 1;
    }
    if top == decl_line {
        return None;
    }
    let block: Vec<&str> = (top..decl_line).map(|i| lines.comment_at(i, indent).unwrap()).collect();
    let kinds: Vec<Line> = block.iter().map(|c| classify(c)).collect();
    // Directives directly above the declaration stay there.
    let mut end = kinds.len();
    while end > 0 && kinds[end - 1] == Line::Directive {
        end -= 1;
    }
    // The doc is what follows the last ruler.
    let begin = kinds[..end].iter().rposition(|k| *k == Line::Ruler).map_or(0, |i| i + 1);
    let run = &kinds[begin..end];
    if !run.contains(&Line::Prose) {
        return None;
    }
    let hand = |why: &str| {
        Some(Fix {
            anchor,
            message: format!("the comment block above `{name}` {why}; split it by hand into `@doc` and a comment"),
            splices: Vec::new(),
        })
    };
    if run.contains(&Line::Note) || run.contains(&Line::Licence) {
        return hand("holds a TODO/NOTE/FIXME or licence line as well as text");
    }
    if run.contains(&Line::Directive) {
        return hand("holds a `petal-lint-ignore` line among its text");
    }
    let text = doc_text(&block[begin..end]);
    let call = annotation("doc", &text, indent);
    Some(Fix {
        anchor,
        message: format!("the comment block above `{name}` becomes its `@doc`"),
        splices: vec![lines.replace(top + begin, top + end - 1, format!("{indent}{call}\n"))],
    })
}

/// The `@module_doc` for the file's opening comment block.
fn plan_module_doc(lines: &Lines, stmts: &[Stmt]) -> Option<Fix> {
    let first = lines.line_of(stmts.first()?.span.start.offset as usize);
    // Blocks of comment lines before the first statement; anything else
    // there (a `@module_doc` already) and the file is passed over.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for i in 0..first {
        let line = lines.text[i].trim();
        if i == 0 && line.starts_with("#!") || line.is_empty() {
            if let Some(s) = open.take() {
                blocks.push((s, i));
            }
        } else if line.starts_with('#') && lines.text[i].starts_with('#') {
            open.get_or_insert(i);
        } else {
            return None;
        }
    }
    // A block still open runs into the first statement: it is that
    // statement's, not the file's.
    let (from, to) = blocks
        .into_iter()
        .find(|&(s, e)| !(s..e).any(|i| classify(lines.text[i].trim_end()) == Line::Licence))?;
    let block: Vec<&str> = (from..to).map(|i| lines.text[i].trim_end()).collect();
    let kinds: Vec<Line> = block.iter().map(|c| classify(c)).collect();
    // Rulers framing the header stay where they are.
    let mut begin = 0;
    while begin < kinds.len() && kinds[begin] == Line::Ruler {
        begin += 1;
    }
    let mut end = kinds.len();
    while end > begin && kinds[end - 1] == Line::Ruler {
        end -= 1;
    }
    let run = &kinds[begin..end];
    if !run.contains(&Line::Prose) {
        return None;
    }
    let anchor = lines.starts[from];
    let hand = |why: &str| {
        Some(Fix {
            anchor,
            message: format!("the file's opening comment block {why}; split it by hand into `@module_doc` and a comment"),
            splices: Vec::new(),
        })
    };
    if run.contains(&Line::Note) {
        return hand("holds a TODO/NOTE/FIXME line as well as text");
    }
    if run.contains(&Line::Directive) {
        return hand("holds a `petal-lint-ignore` line");
    }
    if run.contains(&Line::Ruler) {
        return hand("holds a section ruler");
    }
    let text = doc_text(&block[begin..end]);
    let call = annotation("module_doc", &text, "");
    Some(Fix {
        anchor,
        message: "the file's opening comment block becomes its `@module_doc`".to_string(),
        splices: vec![lines.replace(from + begin, from + end - 1, format!("{call}\n"))],
    })
}

/// `--settings`: the trailing comment on `line`, the last line of the
/// tunable `name`, becomes a `@doc` written before it.
fn plan_setting_doc(
    lines: &Lines,
    trailing: &[Option<(usize, usize)>],
    line: usize,
    name: &str,
    start: usize,
) -> Option<Fix> {
    let (code_end, comment_col) = trailing.get(line).copied().flatten()?;
    let comment: String = lines.text[line].chars().skip(comment_col).collect();
    let comment = comment.trim_end();
    if classify(comment) != Line::Prose {
        return None;
    }
    let text = doc_text(&[comment]);
    let line_start = lines.starts[line];
    Some(Fix {
        anchor: start,
        message: format!("the trailing comment of setting `{name}` becomes its `@doc`"),
        splices: vec![
            Splice {
                start,
                end: start,
                text: format!("{} ", annotation("doc", &text, "")),
            },
            Splice {
                start: line_start + code_end,
                end: line_start + lines.text[line].chars().count(),
                text: String::new(),
            },
        ],
    })
}

/// The gate: `after` reads as the program `before` did, docs aside, and when
/// `before` compiles, the two compile to equivalent IR.
pub(super) fn check(before: &str, after: &str, opts: &LintOptions) -> Result<(), String> {
    let refuse = |why: String| format!("migrate bug: {why} — refusing to produce output");
    // A file the language reads strictly must still be read that way: the
    // `@module_doc` is checked there (the by-`end` reading drops it).
    if crate::cst::parse_source_with_module_doc(before, crate::source_map::FileId(0)).is_ok()
        && let Err(e) = crate::cst::parse_source_with_module_doc(after, crate::source_map::FileId(0))
    {
        return Err(refuse(format!("with its docs the file no longer parses ({e})")));
    }
    let (_, a) = super::reparse(before)?;
    let (_, b) = super::reparse(after).map_err(|e| refuse(format!("with its docs the file no longer parses ({e})")))?;
    if shape_without_docs(&a)? != shape_without_docs(&b)? {
        return Err(refuse("with its docs the file parses to a different program".to_string()));
    }
    let (before, after) = (
        super::indented_by_end(&super::state_key::blank_keys(before)),
        super::indented_by_end(&super::state_key::blank_keys(after)),
    );
    let Ok((env_a, pa)) = crate::ir_equiv::compile_for_compare(&before, &opts.include_dirs, opts.origin.as_deref())
    else {
        return Ok(());
    };
    let (env_b, pb) = crate::ir_equiv::compile_for_compare(&after, &opts.include_dirs, opts.origin.as_deref())
        .map_err(|e| refuse(format!("with its docs the file no longer compiles ({e})")))?;
    let (pa, pb) = (
        env_a.get_program(pa).ok_or("compiled program missing")?,
        env_b.get_program(pb).ok_or("compiled program missing")?,
    );
    crate::ir_equiv::ir_equivalent(pa, pb)
        .map_err(|d| refuse(format!("with its docs the file compiles to different IR ({})", d.to_json())))
}

/// [`crate::rewrite::ast_shape`] with every statement's `doc` dropped.
fn shape_without_docs(stmts: &[Stmt]) -> Result<serde_json::Value, String> {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            serde_json::Value::Object(map) => {
                map.remove("doc");
                map.values_mut().for_each(strip);
            }
            _ => {}
        }
    }
    let mut shape = crate::rewrite::ast_shape(stmts)?;
    strip(&mut shape);
    Ok(shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_classified() {
        assert_eq!(classify("# ── Switch ──────"), Line::Ruler);
        assert_eq!(classify("# ======"), Line::Ruler);
        assert_eq!(classify("##########"), Line::Ruler);
        assert_eq!(classify("#"), Line::Blank);
        assert_eq!(classify("# TODO: later"), Line::Note);
        assert_eq!(classify("# NOTE(andy) x"), Line::Note);
        assert_eq!(classify("# NOTEBOOK pages"), Line::Prose);
        assert_eq!(classify("# petal-lint-ignore end-lines"), Line::Directive);
        assert_eq!(classify("# Copyright 2026 Someone"), Line::Licence);
        assert_eq!(classify("# SPDX-License-Identifier: MIT"), Line::Licence);
        assert_eq!(classify("# -- the dashes rule"), Line::Prose);
        assert_eq!(classify("# The on/off switch."), Line::Prose);
    }

    #[test]
    fn text_keeps_relative_indentation() {
        assert_eq!(doc_text(&["#", "#  Title", "#", "#    code", "#  end", "#"]), "Title\n\n  code\nend");
        assert_eq!(doc_text(&["## A", "## B"]), "A\nB");
    }

    #[test]
    fn strings_are_spelled_to_read_back() {
        assert_eq!(annotation("doc", "Plain.", ""), "@doc(\"Plain.\")");
        assert_eq!(annotation("doc", "A {x, y} rect.", ""), "@doc(\"\"\"A {x, y} rect.\"\"\")");
        assert_eq!(annotation("doc", "Ends \"q\"", ""), "@doc(\"\"\"\nEnds \"q\"\n\"\"\")");
        assert_eq!(annotation("doc", "a\n\nb", "  "), "@doc(\"\"\"\n  a\n\n  b\n  \"\"\")");
        assert_eq!(annotation("doc", "x \"\"\" {y}\\", ""), "@doc(\"x \\\"\\\"\\\" \\{y}\\\\\")");
    }
}
