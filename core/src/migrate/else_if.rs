//! Migrations `else-if-nesting` and `elsif-to-else-if` — an `if` chain
//! continues with `else if`.
//!
//! ```text
//! if a then 1 elsif b then 2 else 3 end      =>   if a then 1 else if b then 2 else 3 end
//!
//! if a then                                  =>   if a then
//!   1                                               1
//! else if b then                                  else if b then
//!   2                                               2
//! end                                             end
//! end
//! ```
//!
//! `else` and `if` on one line are one token, [`Token::ElseIf`], which
//! continues the chain the way `elsif` did (docs/tasks/pythonic-syntax.md
//! §3.6). Two old forms follow from that, and each has its migration.
//!
//! **`else-if-nesting`.** Before the merge, `else if c then … end end` was an
//! `else` block holding an `if`, and each took an `end`. Read as a chain, the
//! second `end` is one too many, so the file no longer parses (the parser's
//! error for the stray `end` names this migration). Where the nested `if` is
//! the `else`'s only statement, the nested `if`'s own `end` is deleted and
//! the two become the chain they always behaved as. Otherwise the `if`
//! moves to the line after `else`, where it is not merged and stays nested:
//!
//! ```text
//! else if b then        =>   else
//!     2                        if b then
//!   end                          2
//!   log()                      end
//! end                          log()
//!                            end
//! ```
//!
//! (only the `if` moves; the lines under it are left as written).
//!
//! *Reading the old form.* The planner lexes with
//! [`Lexer::unmerged_else_if`], in which `else if` is still two tokens, and
//! parses that ([`parse_source_unmerged_else_if`]): the sites are the `else`
//! blocks whose first statement starts with an `if` on the `else`'s own
//! line. It runs before `at-rebind`, so it reads through a removed `@x` the
//! way that migration does (each `@` blanked to a space). A file that
//! parses today has no site: its `else if`s are already chains. So the old
//! reading is tried only on a file today's parser rejects, and a file that
//! mixes the two readings parses under neither and is reported with today's
//! error.
//!
//! *Gate.* The AST changes (a block holding an `if` becomes a chain), so the
//! gate is on the compiled program: the IR of the migrated text must equal
//! ([`crate::ir_equiv`]) the IR of the original with *every* site moved to
//! its own line, which is the original's nesting spelled so that today's
//! compiler reads it (the same tokens plus line breaks, checked). A file
//! that does not compile on its own (its imports resolve only inside its
//! app) cannot be held to that, so every file is also held to the parse: the
//! old reading of the original and today's reading of the migrated text are
//! the same AST once a block that holds exactly one `if` is read as a
//! chain.
//!
//! **`elsif-to-else-if`.** `elsif` is the old spelling of `else if`: both
//! parse to the same chain, so the rewrite is one keyword splice per
//! [`Token::Elsif`] (and, where a trailing comment shares its column with a
//! line that is not rewritten, two columns less padding in front of it). The
//! compiler says nothing about `elsif`, so `petal lint` reports it
//! (`old-syntax`). *Gate:* the token stream is the same but for the
//! respelled keyword, and where the file parses the AST is the same.

use std::collections::{HashMap, HashSet};

use crate::ast::{ElseBranch, Expr, ExprKind, ExprVisitor, Stmt, StmtKind, walk_expr};
use crate::cst::parse_source_unmerged_else_if;
use crate::lexer::{Lexer, Token};
use crate::lint::{Fix, LintOptions};
use crate::rewrite::{Splice, apply_splices};

const CHAINED: &str = "`else if` continues the `if` chain, which takes one `end`: the nested \
                       `if`'s own `end` is removed";
const BROKEN: &str = "`else if` continues the `if` chain, and this `if` is nested in an `else` \
                      that holds more than it: it moves to the line after `else`";

/// What the else block of a nesting site holds.
enum Shape {
    /// Only the `if`: the char range of the nested `if`'s closing `end`.
    OnlyIf { end: (usize, usize) },
    /// More than the `if`. `column` is the 0-based column the block's second
    /// statement starts its line at, when it does.
    More { column: Option<usize> },
}

/// One `else if` that the old reading nests: an `else` block whose first
/// statement starts with an `if` on the `else`'s line.
struct Site {
    /// Char offset just past the `else`.
    else_end: usize,
    /// Char offset of the `if`.
    if_start: usize,
    shape: Shape,
}

/// (offset just past `else`, offset of `if`) of every `else if` the lexer
/// merges today, read from the unmerged token stream.
fn merge_points(source: &str) -> Result<Vec<(usize, usize)>, String> {
    if !source.contains("else") {
        return Ok(Vec::new());
    }
    let mut lexer = Lexer::new(source).without_trivia().unmerged_else_if();
    lexer.tokenize()?;
    let merged = merged_else_ifs(source)?;
    let (tokens, spans) = (&lexer.tokens, &lexer.token_spans);
    let mut out = Vec::new();
    for i in 0..tokens.len().saturating_sub(1) {
        if matches!(tokens[i], Token::Else)
            && matches!(tokens[i + 1], Token::If)
            && merged.contains(&(spans[i].start.offset as usize))
        {
            out.push((spans[i].end.offset as usize, spans[i + 1].start.offset as usize));
        }
    }
    Ok(out)
}

/// The char offset of every [`Token::ElseIf`] of `source` as the language
/// lexes it today.
fn merged_else_ifs(source: &str) -> Result<HashSet<usize>, String> {
    let mut lexer = Lexer::new(source).without_trivia();
    lexer.tokenize()?;
    Ok(lexer
        .tokens
        .iter()
        .zip(&lexer.token_spans)
        .filter(|(t, _)| matches!(t, Token::ElseIf))
        .map(|(_, s)| s.start.offset as usize)
        .collect())
}

/// `source` with each removed `@x` rebind read as a plain `x` (the same
/// offsets), so a file `at-rebind` has not run on yet still parses.
fn readable(source: &str) -> String {
    if source.contains('@')
        && let Ok(blanked) = super::at_rebind::blank_markers(source)
    {
        return blanked;
    }
    source.to_string()
}

fn nesting_sites(source: &str) -> Result<Vec<Site>, String> {
    let points = merge_points(source)?;
    if points.is_empty() {
        return Ok(Vec::new());
    }
    // A file that parses today is written for today's reading: its
    // `else if`s are chains. Only one that does not is tried the old way,
    // and if that fails too, today's error is the one to report.
    let text = readable(source);
    let Err(today) = crate::lint::reparse(&text) else {
        return Ok(Vec::new());
    };
    let stmts = parse_source_unmerged_else_if(&text).map_err(|_| today)?;
    let chars: Vec<char> = source.chars().collect();

    struct Finder<'a> {
        chars: &'a [char],
        /// `if` offset -> offset just past its `else`.
        points: HashMap<usize, usize>,
        out: Vec<Site>,
    }
    impl Finder<'_> {
        fn starts_line(&self, at: usize) -> Option<usize> {
            let line_start = self.chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
            self.chars[line_start..at]
                .iter()
                .all(|c| matches!(c, ' ' | '\t'))
                .then_some(at - line_start)
        }
    }
    impl ExprVisitor for Finder<'_> {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::If {
                else_body: Some(ElseBranch::Block(body)),
                ..
            } = &e.kind
                && let Some(first) = body.first()
                && let Some(&else_end) = self.points.get(&(first.span.start.offset as usize))
            {
                let if_start = first.span.start.offset as usize;
                let only_if = match &first.kind {
                    StmtKind::Expr(inner) if body.len() == 1 => {
                        let end = inner.span.end.offset as usize;
                        (matches!(inner.kind, ExprKind::If { .. })
                            && inner.span.start.offset as usize == if_start
                            && end >= 3
                            && self.chars[end - 3..end] == ['e', 'n', 'd'])
                        .then_some((end - 3, end))
                    }
                    _ => None,
                };
                let shape = match only_if {
                    Some(end) => Shape::OnlyIf { end },
                    None => Shape::More {
                        column: body
                            .get(1)
                            .and_then(|s| self.starts_line(s.span.start.offset as usize)),
                    },
                };
                self.out.push(Site {
                    else_end,
                    if_start,
                    shape,
                });
            }
            walk_expr(self, e);
        }
    }
    let mut finder = Finder {
        chars: &chars,
        points: points.iter().map(|&(else_end, if_start)| (if_start, else_end)).collect(),
        out: Vec::new(),
    };
    for s in &stmts {
        finder.visit_stmt(s);
    }
    finder.out.sort_by_key(|s| s.if_start);
    Ok(finder.out)
}

/// The splice that deletes the `end` at `start..end`: its whole line when it
/// stands alone on it, else the word and the blanks on one side of it.
fn delete_end(chars: &[char], (start, end): (usize, usize)) -> Splice {
    let blank = |c: &char| matches!(c, ' ' | '\t' | '\r');
    let line_start = chars[..start].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
    let line_end = chars[end..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| end + i);
    let before_blank = chars[line_start..start].iter().all(blank);
    let after_blank = chars[end..line_end].iter().all(blank);
    let (s, e) = if before_blank && after_blank {
        if line_end < chars.len() {
            (line_start, line_end + 1)
        } else {
            // The last line of a file with no final newline: take the line
            // break before it instead.
            (line_start.saturating_sub(1), line_end)
        }
    } else if after_blank {
        // `… end` then the line ends: the blanks before it go too.
        let blanks = chars[line_start..start].iter().rev().take_while(|c| blank(c)).count();
        (start - blanks, end)
    } else {
        // `end end`, `end # note`: the blanks after it go too.
        let blanks = chars[end..line_end].iter().take_while(|c| blank(c)).count();
        (start, end + blanks)
    };
    Splice {
        start: s,
        end: e,
        text: String::new(),
    }
}

/// The splice that moves the `if` of `site` to the line after its `else`.
fn break_line(chars: &[char], site: &Site, column: Option<usize>) -> Splice {
    let line_start = chars[..site.else_end].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
    let indent = chars[line_start..].iter().take_while(|c| matches!(c, ' ' | '\t')).count();
    // The block's own column where its next statement shows it; else one
    // level under the line the `else` is on.
    let column = column.filter(|&c| c > indent).unwrap_or(indent + 2);
    Splice {
        start: site.else_end,
        end: site.if_start,
        text: format!("\n{}", " ".repeat(column)),
    }
}

fn plan(source: &str, force_break: bool) -> Result<Vec<Fix>, String> {
    let sites = nesting_sites(source)?;
    let chars: Vec<char> = source.chars().collect();
    Ok(sites
        .iter()
        .map(|site| {
            let (message, splice) = match site.shape {
                Shape::OnlyIf { end } if !force_break => (CHAINED, delete_end(&chars, end)),
                Shape::OnlyIf { .. } => (BROKEN, break_line(&chars, site, None)),
                Shape::More { column } => (BROKEN, break_line(&chars, site, column)),
            };
            Fix {
                anchor: site.else_end - "else".len(),
                message: message.to_string(),
                splices: vec![splice],
            }
        })
        .collect())
}

/// Plan the `else-if-nesting` rewrite of every `else if` of `source` that
/// the old reading nests.
pub(super) fn plan_nesting_fixes(source: &str) -> Result<Vec<Fix>, String> {
    plan(source, false)
}

/// Whether `source` holds an `else if` written as nesting (`else if … end
/// end`): one `else-if-nesting` has not rewritten.
pub(super) fn has_nesting(source: &str) -> bool {
    nesting_sites(source).is_ok_and(|sites| !sites.is_empty())
}

/// `source` with the `if` of every old-style nested `else if` moved to the
/// line after its `else`: the same program as the old reading of `source`,
/// spelled so that today's lexer reads it. The tokens are checked to be the
/// old reading's, line breaks aside.
pub(super) fn nesting_as_read_today(source: &str) -> Result<String, String> {
    let fixes = plan(source, true)?;
    if fixes.is_empty() {
        return Ok(source.to_string());
    }
    let mut splices: Vec<Splice> = fixes.into_iter().flat_map(|f| f.splices).collect();
    splices.sort_by_key(|s| s.start);
    let chars: Vec<char> = source.chars().collect();
    let out = apply_splices(&chars, &splices);

    let signature = |lexer: Lexer| -> Result<Vec<Token>, String> {
        let mut lexer = lexer;
        lexer.tokenize()?;
        Ok(lexer.tokens.into_iter().filter(|t| !matches!(t, Token::Newline)).collect())
    };
    let old = signature(Lexer::new(source).without_trivia().unmerged_else_if())?;
    let new = signature(Lexer::new(&out).without_trivia())?;
    if old != new {
        return Err("migrate bug: moving a nested `if` under its `else` changed the token \
                    stream — refusing to produce output"
            .to_string());
    }
    Ok(out)
}

/// The gate of `else-if-nesting`: `after` (read today) is the program
/// `before` was (read the old way). See the module docs.
pub(super) fn check_nesting(before: &str, after: &str, opts: &LintOptions) -> Result<(), String> {
    let old = ast_shape(&parse_source_unmerged_else_if(&readable(before))?, true)?;
    let new = crate::lint::reparse(&readable(after))
        .map_err(|e| {
            format!("migrate bug: rewriting a nested `else if` broke the parse — refusing to produce output ({e})")
        })
        .and_then(|(_, stmts)| ast_shape(&stmts, true))?;
    if old != new {
        return Err("migrate bug: rewriting a nested `else if` changed the parse — refusing to \
                    produce output"
            .to_string());
    }
    // The compiled program, where the original compiles here at all.
    let original = nesting_as_read_today(before)?;
    let Ok((env_a, pid_a)) =
        crate::ir_equiv::compile_for_compare(&original, &opts.include_dirs, opts.origin.as_deref())
    else {
        return Ok(());
    };
    let (env_b, pid_b) =
        crate::ir_equiv::compile_for_compare(after, &opts.include_dirs, opts.origin.as_deref()).map_err(
            |e| {
                format!(
                    "migrate bug: rewriting a nested `else if` broke compilation — refusing to produce \
                     output ({e})"
                )
            },
        )?;
    let (Some(a), Some(b)) = (env_a.get_program(pid_a), env_b.get_program(pid_b)) else {
        return Err("migrate bug: compiled program missing".to_string());
    };
    crate::ir_equiv::ir_equivalent(a, b).map_err(|diff| {
        format!(
            "migrate bug: rewriting a nested `else if` changed the compiled program — refusing to \
             produce output ({diff})"
        )
    })
}

/// The AST as JSON with every span dropped. With `chains`, an `else` block
/// that holds exactly one `if` expression reads as the `else if` chain it
/// compiles to.
fn ast_shape(stmts: &[Stmt], chains: bool) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    fn strip(v: &mut Value, chains: bool) {
        match v {
            Value::Array(items) => items.iter_mut().for_each(|i| strip(i, chains)),
            Value::Object(map) => {
                map.retain(|k, _| !k.ends_with("span"));
                for (_, child) in map.iter_mut() {
                    strip(child, chains);
                }
                if chains
                    && map.len() == 1
                    && let Some(Value::Array(body)) = map.get("Block")
                    && let [only] = body.as_slice()
                    && let Some(inner) = only.get("kind").and_then(|k| k.get("Expr"))
                    && inner.get("kind").is_some_and(|k| k.get("If").is_some())
                {
                    let inner = inner.clone();
                    map.clear();
                    map.insert("ElseIf".to_string(), inner);
                }
            }
            _ => {}
        }
    }
    let mut json = serde_json::to_value(stmts).map_err(|e| e.to_string())?;
    strip(&mut json, chains);
    Ok(json)
}

// ---- elsif-to-else-if ----------------------------------------------------

const OLD: &str = "elsif";
const NEW: &str = "else if";

/// Plan the `elsif` → `else if` respelling of every [`Token::Elsif`] of
/// `source` that is the keyword (not a field or a record key named `elsif`).
pub(super) fn plan_elsif_fixes(source: &str) -> Result<Vec<Fix>, String> {
    if !source.contains(OLD) {
        return Ok(Vec::new());
    }
    let mut lexer = Lexer::new(source).without_trivia();
    lexer.tokenize()?;
    let (tokens, spans) = (&lexer.tokens, &lexer.token_spans);
    let sites: Vec<_> = (0..tokens.len())
        .filter(|&i| {
            matches!(tokens[i], Token::Elsif)
                && !(i > 0 && matches!(tokens[i - 1], Token::Dot | Token::QuestionDot))
                && !matches!(tokens.get(i + 1), Some(Token::Colon))
        })
        .map(|i| spans[i])
        .collect();
    if sites.is_empty() {
        return Ok(Vec::new());
    }

    // `else if` is two columns longer, so a trailing comment on its line
    // moves right. Where the comment's column is shared with a line that is
    // not rewritten, take the growth out of the padding in front of it.
    let chars: Vec<char> = source.chars().collect();
    let comments = crate::fmt::trailing_comments(source)?;
    let comment_col = |line: usize| comments.get(line).copied().flatten().map(|(_, col)| col);
    let mut per_line: HashMap<usize, usize> = HashMap::new();
    for s in &sites {
        *per_line.entry(s.start.line as usize - 1).or_default() += 1;
    }
    let grow = NEW.len() - OLD.len();
    let mut padded: HashSet<usize> = HashSet::new();

    Ok(sites
        .iter()
        .map(|site| {
            let start = site.start.offset as usize;
            let mut splices = vec![Splice {
                start,
                end: start + OLD.len(),
                text: NEW.to_string(),
            }];
            let line = site.start.line as usize - 1;
            if let Some(col) = comment_col(line)
                && padded.insert(line)
            {
                let mut first = line;
                while first > 0 && comment_col(first - 1) == Some(col) {
                    first -= 1;
                }
                let mut last = line;
                while comment_col(last + 1) == Some(col) {
                    last += 1;
                }
                if (first..=last).any(|l| !per_line.contains_key(&l)) {
                    let at = start - (site.start.column as usize - 1) + col;
                    let blanks = chars[..at].iter().rev().take_while(|&&c| c == ' ').count();
                    // Keep at least one space before the comment.
                    let take = (grow * per_line[&line]).min(blanks.saturating_sub(1));
                    if take > 0 {
                        splices.push(Splice {
                            start: at - take,
                            end: at,
                            text: String::new(),
                        });
                    }
                }
            }
            Fix {
                anchor: start,
                message: format!("`{OLD}` is deprecated; write `{NEW}`"),
                splices,
            }
        })
        .collect())
}

/// The gate of `elsif-to-else-if`: the same tokens but for the respelled
/// keyword and, with `parse`, the same AST (where `before` parses at all).
pub(super) fn check_elsif(before: &str, after: &str, parse: bool) -> Result<(), String> {
    let signature = |source: &str| -> Result<Vec<Token>, String> {
        let mut lexer = Lexer::new(source).without_trivia();
        lexer.tokenize()?;
        Ok(lexer
            .tokens
            .into_iter()
            .map(|t| if matches!(t, Token::Elsif) { Token::ElseIf } else { t })
            .collect())
    };
    let bug = |what: &str| {
        format!("migrate bug: respelling `elsif` changed the {what} — refusing to produce output")
    };
    if signature(before)? != signature(after)? {
        return Err(bug("token stream"));
    }
    if parse && let Ok((_, old)) = crate::lint::reparse(&readable(before)) {
        let (_, new) = crate::lint::reparse(&readable(after)).map_err(|_| bug("parse"))?;
        if ast_shape(&old, false)? != ast_shape(&new, false)? {
            return Err(bug("parse"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(src: &str, fixes: Vec<Fix>) -> String {
        let mut splices: Vec<Splice> = fixes.into_iter().flat_map(|f| f.splices).collect();
        splices.sort_by_key(|s| s.start);
        apply_splices(&src.chars().collect::<Vec<_>>(), &splices)
    }

    fn nest(src: &str) -> String {
        let out = apply(src, plan_nesting_fixes(src).unwrap());
        check_nesting(src, &out, &LintOptions::default()).unwrap();
        assert!(plan_nesting_fixes(&out).unwrap().is_empty(), "not idempotent: {out}");
        out
    }

    fn respell(src: &str) -> String {
        let out = apply(src, plan_elsif_fixes(src).unwrap());
        check_elsif(src, &out, true).unwrap();
        assert!(plan_elsif_fixes(&out).unwrap().is_empty(), "not idempotent: {out}");
        out
    }

    #[test]
    fn a_nested_if_alone_in_its_else_joins_the_chain() {
        assert_eq!(
            nest("let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end end\n"),
            "let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end\n"
        );
        assert_eq!(
            nest("let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\nend\nend\n"),
            "let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\nend\n"
        );
        // The nested `end` keeps its trailing comment's line.
        assert_eq!(
            nest("let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\n  end # inner\nend\n"),
            "let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\n  # inner\nend\n"
        );
    }

    #[test]
    fn nested_sites_each_lose_one_end() {
        assert_eq!(
            nest(
                "let a = 1\nlet x = if a > 2 then 1 else if a > 1 then 2 else if a > 0 then 3 else 4 end end end\n"
            ),
            "let a = 1\nlet x = if a > 2 then 1 else if a > 1 then 2 else if a > 0 then 3 else 4 end\n"
        );
    }

    #[test]
    fn an_else_that_holds_more_than_the_if_keeps_its_nesting() {
        assert_eq!(
            nest(
                "let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n    print(2)\n  end\n  print(3)\nend\n"
            ),
            "let a = 1\nif a > 1 then\n  print(1)\nelse\n  if a > 0 then\n    print(2)\n  end\n  print(3)\nend\n"
        );
        // The `if` is the start of a larger expression: not a chain either.
        assert_eq!(
            nest("let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end + 1 end\n"),
            "let a = 1\nlet x = if a > 1 then 1 else\n  if a > 0 then 2 else 3 end + 1 end\n"
        );
    }

    #[test]
    fn current_code_has_no_nesting_site() {
        for src in [
            "let a = 1\nif a > 1 then\n  print(1)\nelse if a > 0 then\n  print(2)\nend\n",
            "let a = 1\nif a > 1 then\n  print(1)\nelse\n  if a > 0 then\n    print(2)\n  end\nend\n",
            "let a = 1\nif a > 1 then print(1) elsif a > 0 then print(2) end\n",
            "let s = \"else if x end end\" # else if y end end\n",
        ] {
            assert!(plan_nesting_fixes(src).unwrap().is_empty(), "{src}");
            assert!(!has_nesting(src));
        }
    }

    #[test]
    fn text_in_strings_and_comments_is_not_a_site() {
        let src = "let glsl = \"\"\"\nif (a) { } else if (b) { }\n\"\"\"\n# else if c then 1 end end\nlet a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end end\n";
        let out = nest(src);
        assert!(out.contains("else if (b) { }\n\"\"\"\n# else if c then 1 end end\n"), "{out}");
        assert!(out.ends_with("else if a > 0 then 2 else 3 end\n"), "{out}");
    }

    #[test]
    fn the_old_reading_spelled_for_today_breaks_every_site() {
        let src = "let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else 3 end end\n";
        assert_eq!(
            nesting_as_read_today(src).unwrap(),
            "let a = 1\nlet x = if a > 1 then 1 else\n  if a > 0 then 2 else 3 end end\n"
        );
    }

    #[test]
    fn elsif_is_respelled_token_by_token() {
        assert_eq!(
            respell("let a = 1\nlet x = if a > 1 then 1 elsif a > 0 then 2 elsif a < 0 then 4 else 3 end\n"),
            "let a = 1\nlet x = if a > 1 then 1 else if a > 0 then 2 else if a < 0 then 4 else 3 end\n"
        );
        // Strings, raw strings and comments are not tokens.
        let src = "let s = \"elsif\" # elsif\nlet g = \"\"\"\nelsif\n\"\"\"\n";
        assert_eq!(respell(src), src);
        // A field or a record key named `elsif` is a name.
        let src = "let r = {elsif: 1}\nprint(r.elsif)\n";
        assert_eq!(respell(src), src);
    }

    #[test]
    fn a_shared_comment_column_is_kept() {
        // The comment on the rewritten line shares its column with one on a
        // line that stays as it is, so it gives up the two columns.
        let old = "let x = if n > 1 then 1 elsif n > 0 then 2 else 3 end";
        let new = "let x = if n > 1 then 1 else if n > 0 then 2 else 3 end";
        let first = format!("{:<width$}# one\n", "let n = 1", width = old.len() + 3);
        assert_eq!(
            respell(&format!("{first}{old}   # two\n")),
            format!("{first}{new} # two\n")
        );
        // Alone, the comment just moves with its line.
        assert_eq!(
            respell(&format!("let n = 1\n{old}   # two\n")),
            format!("let n = 1\n{new}   # two\n")
        );
    }
}
