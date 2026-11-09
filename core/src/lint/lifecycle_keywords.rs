//! Rules — the declaration keywords `const` and `config` become annotation
//! functions (docs/tasks/lifecycle-annotation-functions-refactor.md, Phase 2).
//!
//! ```text
//! const COSTS = map(TILES, cost)       =>   let COSTS = const(map(TILES, cost))
//! pub const ART = "../art"             =>   pub let ART = const("../art")
//! config let SPEED = 4.0               =>   let SPEED = tunable(4.0)
//! pub config let G = tunable(9.8, 0)   =>   pub let G = tunable(9.8, 0)
//! ```
//!
//! - **`const-decl-deprecated`**: `const X = e` → `let X = const(e)`. The
//!   wrapper always goes on, literals included, so no check is lost (open
//!   question 3 of the plan).
//! - **`config-let-deprecated`**: `config let X = v` → `let X = tunable(v)`,
//!   or only the `config` dropped when `v` is already a `tunable(…)` call.
//! - **`const-of-literal`**: a `const` (either form) on literal data, where
//!   it checks nothing a reader cannot see. Report only, with no planned fix.
//!
//! Since Phase 5 of the plan the two deprecation rules run by default and
//! `petal lint --fix` applies them, and the compiler warns about the same
//! sites ([`deprecated_lifecycle_decls`]; a `petal-lint-ignore` for the rule
//! silences both). `const-of-literal` stays opt-in and report only.
//!
//! Two things `prefer-pub` ([`super::export_to_pub`]) never needed:
//!
//! - **a closing `)`** after the initializer, which may end lines below the
//!   keyword (a multi-line table);
//! - **re-padding trailing comments.** The rewrite grows a line (`const X =`
//!   → `let X = const(`…`)` is five columns longer) or shrinks it
//!   (`config let X = tunable(…)` loses seven). A trailing comment keeps its
//!   column, and when a line's code would reach it, the whole aligned group
//!   (adjacent lines whose comments share a column, as `petal fmt` groups
//!   them) moves right together to one space past the longest code. Untouched
//!   lines of the group are padded too.

use std::collections::{BTreeMap, HashMap};

use crate::ast::{Expr, ExprKind, Literal, RecordField, Stmt, StmtKind, UnaryOp};
use crate::parse::{CONFIG_KEYWORD, CONST_ANNOTATION};
use crate::static_value::{color_literal, const_call_arg, tunable_args};

use super::Fix;
use super::to_match::Splice;

/// Which rewrite a site takes.
enum Kind {
    Const,
    Config { already_tunable: bool },
}

/// One declaration to rewrite: where it is reported and its code edits
/// (keyword and wrapper, before any comment padding).
struct Site {
    kind: Kind,
    anchor: usize,
    name: String,
    edits: Vec<Splice>,
}

/// Plan the rewrites of one deprecation rule over `stmts`, parsed from
/// `chars`: `const-decl-deprecated` when `consts`, else
/// `config-let-deprecated`. A site whose keyword is not where the statement
/// says it is (which the parser does not produce) is reported without a fix.
///
/// The two rules are planned one after the other, each over the text the
/// one before left: a comment group holding both kinds of line is
/// re-padded by each in turn.
pub(super) fn plan_decl_fixes(stmts: &[Stmt], chars: &[char], consts: bool) -> Result<Vec<Fix>, String> {
    let mut sites = Vec::new();
    let mut out = Vec::new();
    for stmt in stmts {
        let StmtKind::Let {
            name,
            value,
            is_config,
            is_const,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        if if consts { !is_const } else { !is_config } {
            continue;
        }
        match plan_site(stmt, name, value, consts, chars) {
            Some(site) => sites.push(site),
            None => out.push(Fix {
                anchor: stmt.span.start.offset as usize,
                message: message(consts, false, name),
                splices: Vec::new(),
            }),
        }
    }

    let source: String = chars.iter().collect();
    let mut padding = plan_comment_padding(&source, chars, &sites)?;
    for (i, site) in sites.into_iter().enumerate() {
        let mut splices = site.edits;
        splices.extend(padding.remove(&i).unwrap_or_default());
        let already = matches!(site.kind, Kind::Config { already_tunable: true });
        out.push(Fix {
            anchor: site.anchor,
            message: message(consts, already, &site.name),
            splices,
        });
    }
    out.sort_by_key(|f| f.anchor);
    Ok(out)
}

fn message(is_const: bool, already_tunable: bool, name: &str) -> String {
    if is_const {
        format!("`const {name} = e` is deprecated; write `let {name} = const(e)`")
    } else if already_tunable {
        format!("`config let` is deprecated; `tunable(…)` already makes `{name}` a setting: drop `config`")
    } else {
        format!("`config let {name} = v` is deprecated; write `let {name} = tunable(v)`")
    }
}

/// The code edits for one `const` or `config let` statement.
fn plan_site(stmt: &Stmt, name: &str, value: &Expr, is_const: bool, chars: &[char]) -> Option<Site> {
    let start = stmt.span.start.offset as usize;
    let mut at = start;
    if stmt.exported {
        // `pub` or `export`, then the space before the keyword.
        at = skip_word(chars, at)?;
        at = skip_blanks(chars, at)?;
    }
    // The initializer as written: from the first char after the `=` to the
    // end of the statement. The value's own span leaves out grouping parens
    // (`(a) ++ b` starts at `a`), so it cannot give the edges.
    let eq = at + chars[at..].iter().position(|c| *c == '=')?;
    let mut value_start = eq + 1;
    while value_start < chars.len() && chars[value_start].is_whitespace() {
        value_start += 1;
    }
    let value_end = stmt.span.end.offset as usize;
    if value_end > chars.len()
        || value_start >= value_end
        || value_start > value.span.start.offset as usize
    {
        return None;
    }
    let mut edits = Vec::new();
    let kind = if is_const {
        if !word_at(chars, at, "const") {
            return None;
        }
        edits.push(Splice {
            start: at,
            end: at + "const".len(),
            text: "let".to_string(),
        });
        edits.push(insert(value_start, "const("));
        edits.push(insert(value_end, ")"));
        Kind::Const
    } else {
        if !word_at(chars, at, CONFIG_KEYWORD) {
            return None;
        }
        // `config` and the blanks after it, so `let` takes its place.
        let after = skip_blanks(chars, at + CONFIG_KEYWORD.len())?;
        if !word_at(chars, after, "let") {
            return None;
        }
        edits.push(Splice {
            start: at,
            end: after,
            text: String::new(),
        });
        let already_tunable = tunable_args(value).is_some();
        if !already_tunable {
            edits.push(insert(value_start, "tunable("));
            edits.push(insert(value_end, ")"));
        }
        Kind::Config { already_tunable }
    };
    Some(Site {
        kind,
        anchor: start,
        name: name.to_string(),
        edits,
    })
}

fn insert(at: usize, text: &str) -> Splice {
    Splice {
        start: at,
        end: at,
        text: text.to_string(),
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Is the whole word at `at` exactly `word`?
fn word_at(chars: &[char], at: usize, word: &str) -> bool {
    let end = at + word.chars().count();
    end <= chars.len()
        && chars[at..end].iter().copied().eq(word.chars())
        && chars.get(end).is_none_or(|c| !is_word_char(*c))
}

/// The offset just past the word at `at`, if there is one.
fn skip_word(chars: &[char], at: usize) -> Option<usize> {
    let mut i = at;
    while i < chars.len() && is_word_char(chars[i]) {
        i += 1;
    }
    (i > at).then_some(i)
}

/// The offset past the spaces and tabs at `at`; at least one is required.
fn skip_blanks(chars: &[char], at: usize) -> Option<usize> {
    let mut i = at;
    while i < chars.len() && matches!(chars[i], ' ' | '\t') {
        i += 1;
    }
    (i > at).then_some(i)
}

/// Comment re-padding: per site index, the splices that keep trailing
/// comments aligned once every site's edits are applied. A line's padding
/// goes with the site that edits that line; an untouched line in a moved
/// group goes with the group's first site.
fn plan_comment_padding(
    source: &str,
    chars: &[char],
    sites: &[Site],
) -> Result<HashMap<usize, Vec<Splice>>, String> {
    let comments = crate::fmt::trailing_comments(source)?;
    let mut line_starts = vec![0usize];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset) - 1;

    // Per 0-based line: the net growth from every edit on it, and the first
    // site that edits it.
    let mut growth: BTreeMap<usize, isize> = BTreeMap::new();
    let mut owner: HashMap<usize, usize> = HashMap::new();
    for (i, site) in sites.iter().enumerate() {
        for e in &site.edits {
            let line = line_of(e.start);
            *growth.entry(line).or_default() +=
                e.text.chars().count() as isize - (e.end - e.start) as isize;
            owner.entry(line).or_insert(i);
        }
    }

    let mut out: HashMap<usize, Vec<Splice>> = HashMap::new();
    let col = |line: usize| comments.get(line).copied().flatten();
    let mut done_to = 0usize;
    for (&line, _) in growth.iter() {
        if line < done_to || col(line).is_none() {
            continue;
        }
        let (_, group_col) = col(line).unwrap();
        let in_group = |l: usize| col(l).is_some_and(|(_, c)| c == group_col);
        let mut first = line;
        while first > 0 && in_group(first - 1) {
            first -= 1;
        }
        let mut last = line;
        while last + 1 < comments.len() && in_group(last + 1) {
            last += 1;
        }
        done_to = last + 1;
        let grown = |l: usize| growth.get(&l).copied().unwrap_or(0);
        // The comments keep their column unless code would reach it; then
        // the group moves to one space past the longest line.
        let target = (first..=last)
            .map(|l| (col(l).unwrap().0 as isize + grown(l) + 1) as usize)
            .max()
            .unwrap_or(group_col)
            .max(group_col);
        let group_owner = (first..=last).find_map(|l| owner.get(&l).copied());
        for l in first..=last {
            let (code_end, comment_col) = col(l).unwrap();
            let new_end = (code_end as isize + grown(l)) as usize;
            if (comment_col as isize + grown(l)) as usize == target {
                continue;
            }
            let Some(site) = owner.get(&l).copied().or(group_owner) else {
                continue;
            };
            let base = line_starts[l];
            out.entry(site).or_default().push(Splice {
                start: base + code_end,
                end: base + comment_col,
                text: " ".repeat(target - new_end),
            });
        }
    }
    // Splices of one fix in source order; padding after an insert at the
    // same offset (the `)` at the end of the code) keeps that order.
    for splices in out.values_mut() {
        splices.sort_by_key(|s| s.start);
    }
    Ok(out)
}

/// `const-of-literal`: every top-level `const X = e` or `let X = const(e)`
/// whose `e` is literal data. Report only.
pub(super) fn plan_const_of_literal_reports(stmts: &[Stmt]) -> Vec<Fix> {
    let mut out = Vec::new();
    for stmt in stmts {
        let StmtKind::Let {
            name,
            value,
            is_var: false,
            is_const,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        let (data, anchor, how) = if *is_const {
            (value, stmt.span.start.offset as usize, format!("const {name} = …"))
        } else if let Some(arg) = const_call_arg(value) {
            (arg, value.span.start.offset as usize, format!("{CONST_ANNOTATION}(…)"))
        } else {
            continue;
        };
        if !is_literal_data(data) {
            continue;
        }
        out.push(Fix {
            anchor,
            message: format!(
                "`{how}` on literal data checks nothing a reader cannot see; \
                 `let {name} = …` says the same"
            ),
            splices: Vec::new(),
        });
    }
    out
}

/// Literal data: a literal (a negated number included), a colour, or a list
/// or record of literal data. A call is not, even of a constructor: `const`
/// is what checks that the call is static.
fn is_literal_data(e: &Expr) -> bool {
    if color_literal(e).is_some() {
        return true;
    }
    match &e.kind {
        ExprKind::Literal(_) => true,
        ExprKind::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => matches!(
            operand.kind,
            ExprKind::Literal(Literal::Int(_) | Literal::Float(_))
        ),
        ExprKind::List(items) => items.iter().all(is_literal_data),
        ExprKind::Record(fields) => fields.iter().all(|f| match f {
            RecordField::Named(_, v) => is_literal_data(v),
            _ => false,
        }),
        _ => false,
    }
}

/// The compiler's deprecation warnings for the two keywords (Phase 5): one per
/// top-level `const X = e` and `config let X = v`, worded as the lint rules
/// word them and tagged `[const-decl-deprecated]` / `[config-let-deprecated]`.
/// Silenced like a lint finding, with `// petal-lint-ignore <rule>` on the
/// line before or trailing on the line (or `petal-lint-ignore-file`).
///
/// Only top-level statements, as the rules: a nested `const` is already an
/// error (`misplaced_const_decls`), and a nested `config let` is not
/// reported (the fixer does not rewrite one; none exists in the corpus).
pub(crate) fn deprecated_lifecycle_decls(stmts: &[Stmt], source: &str) -> Vec<crate::diagnostic::Diagnostic> {
    use super::{CONFIG_LET_DEPRECATED, CONST_DECL_DEPRECATED};
    let mut out = Vec::new();
    let mut ignores = None;
    for stmt in stmts {
        let StmtKind::Let {
            name,
            value,
            is_config,
            is_const,
            ..
        } = &stmt.kind
        else {
            continue;
        };
        if !is_const && !is_config {
            continue;
        }
        let rule = if *is_const { CONST_DECL_DEPRECATED } else { CONFIG_LET_DEPRECATED };
        // A file whose comments do not scan is reported in full.
        let ignores = ignores.get_or_insert_with(|| super::Ignores::scan(source).ok());
        let line = (stmt.span.start.line as usize).saturating_sub(1);
        if ignores.as_ref().is_some_and(|i| i.suppresses(rule, line)) {
            continue;
        }
        let already = !is_const && tunable_args(value).is_some();
        // The caret goes under `const X =`, not under a long table.
        let mut head = stmt.span;
        if value.span.file == stmt.span.file && value.span.start.offset > stmt.span.start.offset {
            head.end = value.span.start;
        }
        out.push(crate::diagnostic::Diagnostic::new(
            head,
            format!("{} [{rule}]", message(*is_const, already, name)),
        ));
    }
    out
}

/// Rewrite every `const` declaration and `config let` in `source` into the
/// annotation form, exactly as the two deprecation rules plan it. `petal lint
/// --fix` applies the same rewrites since Phase 5; this is the library entry
/// for a sweep and for the fixture tests.
pub fn migrate_lifecycle_keywords(source: &str) -> Result<String, String> {
    let mut text = source.to_string();
    for consts in [true, false] {
        let (_tree, stmts) = crate::rewrite::parse_ast(&text)?;
        let chars: Vec<char> = text.chars().collect();
        let mut splices: Vec<Splice> = plan_decl_fixes(&stmts, &chars, consts)?
            .into_iter()
            .flat_map(|f| f.splices)
            .collect();
        // Stable: a fix's own splices at one offset keep their order.
        splices.sort_by_key(|s| s.start);
        text = super::to_match::apply_match_edits(&chars, &splices);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrate(src: &str) -> String {
        let out = migrate_lifecycle_keywords(src).expect("migrate");
        // The fix of a fixed file is empty.
        assert_eq!(
            migrate_lifecycle_keywords(&out).expect("re-migrate"),
            out,
            "not a fixed point"
        );
        let (_tree, stmts) = crate::rewrite::parse_ast(&out).expect("fixed file parses");
        let chars: Vec<char> = out.chars().collect();
        for consts in [true, false] {
            assert!(plan_decl_fixes(&stmts, &chars, consts).unwrap().is_empty(), "{out}");
        }
        out
    }

    #[test]
    fn const_scalar() {
        assert_eq!(migrate("const N = 4\n"), "let N = const(4)\n");
        assert_eq!(migrate("pub const ART = \"../art\"\n"), "pub let ART = const(\"../art\")\n");
        assert_eq!(migrate("export const N = -1\n"), "export let N = const(-1)\n");
    }

    #[test]
    fn const_computed() {
        assert_eq!(
            migrate("const COSTS = map(TILES, fn(t) -> t.cost)\n"),
            "let COSTS = const(map(TILES, fn(t) -> t.cost))\n"
        );
        assert_eq!(migrate("const S = A + B * 2\n"), "let S = const(A + B * 2)\n");
        // Grouping parens at either edge belong to the initializer.
        assert_eq!(migrate("const S = (A) ++ B\n"), "let S = const((A) ++ B)\n");
        assert_eq!(migrate("const S = (A + B) * 2\n"), "let S = const((A + B) * 2)\n");
        assert_eq!(migrate("config let S = (1)\n"), "let S = tunable((1))\n");
        assert_eq!(
            migrate("const S = A * (B + 1) // s\n"),
            "let S = const(A * (B + 1)) // s\n"
        );
    }

    #[test]
    fn const_record_and_list() {
        assert_eq!(
            migrate("const P = {x: 1, y: [2, 3]}\n"),
            "let P = const({x: 1, y: [2, 3]})\n"
        );
        assert_eq!(migrate("const L = [1, 2, 3]\n"), "let L = const([1, 2, 3])\n");
    }

    #[test]
    fn const_multi_line() {
        let src = "const PALETTE = [\n  #101418, #ff2e88,\n]\nprint(PALETTE)\n";
        assert_eq!(
            migrate(src),
            "let PALETTE = const([\n  #101418, #ff2e88,\n])\nprint(PALETTE)\n"
        );
        let src = "const T = {\n  a: 1, // one\n  b: 2,\n} // table\n";
        assert_eq!(migrate(src), "let T = const({\n  a: 1, // one\n  b: 2,\n}) // table\n");
    }

    #[test]
    fn const_typed() {
        assert_eq!(migrate("const N: int = 4\n"), "let N: int = const(4)\n");
        assert_eq!(
            migrate("pub const XS: list = [1, 2]\n"),
            "pub let XS: list = const([1, 2])\n"
        );
    }

    #[test]
    fn config_scalar_record_list() {
        assert_eq!(migrate("config let SPEED = 4.0\n"), "let SPEED = tunable(4.0)\n");
        assert_eq!(
            migrate("pub config let START = \"title\"\n"),
            "pub let START = tunable(\"title\")\n"
        );
        assert_eq!(
            migrate("config let C = {r: 1, g: 0.5}\n"),
            "let C = tunable({r: 1, g: 0.5})\n"
        );
        assert_eq!(migrate("config let L = [1, 2]\n"), "let L = tunable([1, 2])\n");
        assert_eq!(migrate("config let N: float = 2.5\n"), "let N: float = tunable(2.5)\n");
    }

    #[test]
    fn config_multi_line() {
        let src = "config let BLOOM = {\n  amount: 0.4,\n  radius: 3,\n}\n";
        assert_eq!(
            migrate(src),
            "let BLOOM = tunable({\n  amount: 0.4,\n  radius: 3,\n})\n"
        );
    }

    #[test]
    fn config_already_tunable_drops_config_only() {
        assert_eq!(
            migrate("pub config let G = tunable(9.8, 0, 20)\n"),
            "pub let G = tunable(9.8, 0, 20)\n"
        );
        assert_eq!(
            migrate("config let M = tunable(\"a\", options: [\"a\", \"b\"])\n"),
            "let M = tunable(\"a\", options: [\"a\", \"b\"])\n"
        );
    }

    #[test]
    fn a_lone_trailing_comment_keeps_its_column() {
        // Room in front of the comment: it stays put.
        assert_eq!(
            migrate("const N = 4            // four\n"),
            "let N = const(4)       // four\n"
        );
        // No room: one space past the code.
        assert_eq!(migrate("const N = 4 // four\n"), "let N = const(4) // four\n");
        // Shrinking: padded to stay where it was.
        assert_eq!(
            migrate("config let G = tunable(1) // g\n"),
            "let G = tunable(1)        // g\n"
        );
    }

    #[test]
    fn an_aligned_group_moves_together() {
        // A table: the code reaches the column, so the group moves right as
        // one, untouched lines included, to one space past the longest code.
        let src = "config let A = 1   // a\n\
                   config let BB = 22 // b\n\
                   let CCC = 3        // c, untouched\n";
        assert_eq!(
            migrate(src),
            "let A = tunable(1)   // a\n\
             let BB = tunable(22) // b\n\
             let CCC = 3          // c, untouched\n"
        );
        // Fits in the slack: nothing else moves.
        let src = "const A = 1           // a\n\
                   let BBBBBB = 2        // b\n";
        assert_eq!(
            migrate(src),
            "let A = const(1)      // a\n\
             let BBBBBB = 2        // b\n"
        );
        // A shrinking line in a group keeps the column with its neighbours.
        let src = "config let G = tunable(1) // g\n\
                   let HHHHHHHHHHHHHHHHH = 2 // h\n";
        assert_eq!(
            migrate(src),
            "let G = tunable(1)        // g\n\
             let HHHHHHHHHHHHHHHHH = 2 // h\n"
        );
    }

    #[test]
    fn strings_and_annotations_are_not_declarations() {
        // `const` inside a string, a `const(…)` call and a `config` name are
        // left alone.
        let src = "let S = \"const X = 1\"\nlet T = const([1])\nlet config = 2\n";
        assert_eq!(migrate(src), src);
    }

    #[test]
    fn a_rewritten_file_compiles_to_the_same_constants() {
        let src = "const BASE = 10\n\
                   pub const TABLE = map([1, 2], fn(x) -> x * BASE) // computed\n\
                   config let SPEED = 4.0\n\
                   print(TABLE, SPEED)\n";
        let out = migrate(src);
        let opts = super::super::LintOptions::default();
        super::super::compiles(src, &opts).expect("original compiles");
        super::super::compiles(&out, &opts).expect("rewrite compiles");
        assert_eq!(
            crate::static_value::static_values(src).unwrap(),
            crate::static_value::static_values(&out).unwrap()
        );
    }

    fn literal_reports(src: &str) -> Vec<String> {
        let (_tree, stmts) = crate::rewrite::parse_ast(src).unwrap();
        plan_const_of_literal_reports(&stmts)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn const_of_literal() {
        let src = "const A = 1\n\
                   let B = const(\"x\")\n\
                   let C = const([1, -2.5, {k: #ff0000}])\n\
                   const D = rgb(1, 2, 3)\n\
                   let E = const(A + 1)\n\
                   let F = const(\"{A}\")\n";
        let found = literal_reports(src);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].starts_with("`const A = …` on literal data"), "{found:?}");
        assert!(found[1].starts_with("`const(…)` on literal data"), "{found:?}");
    }

    #[test]
    fn plain_lint_reports_and_fixes_both_keywords() {
        // Phase 5: no `--rules-include` needed, and the output is rewritten.
        let src = "const N = 4\nconfig let SPEED = 2.5 // knob\npub config let G = tunable(9.8, 0, 20)\nprint(N + SPEED + G)\n";
        let out = super::super::lint_source(src, &super::super::LintOptions::default()).expect("lint");
        let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
        assert_eq!(
            rules,
            [
                super::super::CONST_DECL_DEPRECATED,
                super::super::CONFIG_LET_DEPRECATED,
                super::super::CONFIG_LET_DEPRECATED
            ]
        );
        assert!(out.findings.iter().all(|f| f.fixable));
        assert_eq!(out.output, migrate(src));
        assert!(out.output.contains("let SPEED = tunable(2.5) // knob"), "{}", out.output);
        // `const-of-literal` is still opt-in.
        assert!(!out.findings.iter().any(|f| f.rule == super::super::CONST_OF_LITERAL));
    }

    #[test]
    fn the_compiler_warns_about_both_keywords() {
        let src = "const N = 4\nconfig let SPEED = 2.5\n// petal-lint-ignore config-let-deprecated\nconfig let QUIET = 1\npub config let G = tunable(9.8, 0, 20)\nlet OK = tunable(1)\nlet K = const(2)\nprint(N + SPEED + QUIET + G + OK + K)\n";
        let mut env = crate::env::Env::new();
        env.set_echo(false);
        let pid = env.load_program(src).expect("load");
        let found: Vec<(u32, String)> = env
            .get_program(pid)
            .unwrap()
            .warnings
            .iter()
            .map(|d| (d.span.start.line, d.message.clone()))
            .collect();
        assert_eq!(
            found,
            [
                (1, "`const N = e` is deprecated; write `let N = const(e)` [const-decl-deprecated]".to_string()),
                (
                    2,
                    "`config let SPEED = v` is deprecated; write `let SPEED = tunable(v)` [config-let-deprecated]"
                        .to_string()
                ),
                (
                    5,
                    "`config let` is deprecated; `tunable(…)` already makes `G` a setting: drop `config` \
                     [config-let-deprecated]"
                        .to_string()
                ),
            ]
        );
    }
}
