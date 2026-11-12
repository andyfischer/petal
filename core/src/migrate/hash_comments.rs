//! Migration `hash-comments` — a line comment starts with `#`.
//!
//! ```text
//! // a note                         =>   # a note
//! let x = 1 //why                   =>   let x = 1 # why
//! /// doc                           =>   ## doc
//! // petal-lint-ignore prefer-let   =>   # petal-lint-ignore prefer-let
//! <col>                                  <col>
//!   // the header                   =>     {/* the header */}
//!   <row />                                <row />
//! </col>                                 </col>
//! ```
//!
//! `//` is deprecated for `#` (docs/tasks/pythonic-syntax.md §3.3). The lexer
//! still reads it, so the old form is [`super::OldForm::Accepted`]: `petal
//! lint` reports it (`old-syntax`) and this migration rewrites it.
//!
//! **What is rewritten.** The marker only, never the comment's text: the
//! leading run of `n` slashes becomes `n - 1` hashes (so `//` is `#` and a
//! `//////` rule keeps its look), and a space is added when the text follows
//! the marker directly. The space is not only style: `//fff` respelled
//! `#fff` would be the removed color literal, an error. `/* … */` comments
//! are not touched.
//!
//! **Element children.** A children line that starts with `//` was a comment;
//! one that starts with `#` is text (`# of items`), so there the comment
//! becomes the hole form `{/* … */}`, which is the same parse: an empty hole
//! is no child. The one exception is a comment line between two lines of
//! text: the two runs are one text child today and would be two with a hole
//! between them, so that site is listed for a hand edit.
//!
//! The sites come from the **trivia between tokens**, never from the text: a
//! `//` in a string or in a `"""` GLSL block is part of a token.
//!
//! **Gate.** The migrated text must lex to the token stream of the original,
//! comments aside: the same tokens on the same lines, plus one empty `{}`
//! per rewritten children comment. Both sides are read with the legacy lexer
//! ([`super::colour_literals::legacy_lex`]), so the migration also runs on a
//! file that still holds `#f80` colors.

use crate::lexer::{Lexer, Token};
use crate::lint::Fix;
use crate::rewrite::Splice;
use crate::trivia::{LINE_COMMENT, OLD_LINE_COMMENT, TriviaKind};

/// `source` lexed for its comments: the legacy lexer, with trivia.
fn lex(source: &str) -> Result<Lexer, String> {
    let mut lexer = Lexer::new(source).legacy();
    lexer.tokenize()?;
    Ok(lexer)
}

/// `comment` (a `//` line comment's whole text) respelled with `#`.
fn respell(comment: &str) -> String {
    let slashes = comment.chars().take_while(|&c| c == '/').count();
    let body = &comment[slashes..];
    let gap = if body.is_empty() || body.starts_with(char::is_whitespace) { "" } else { " " };
    format!("{}{gap}{body}", LINE_COMMENT.repeat(slashes.saturating_sub(1).max(1)))
}

/// `comment` (a `//` line of element children) as a `{/* … */}` hole.
fn as_hole(comment: &str) -> String {
    let body = comment.trim_start_matches('/').trim().replace("*/", "* /");
    if body.is_empty() { "{/* */}".to_string() } else { format!("{{/* {body} */}}") }
}

/// Plan the respelling of every `//` comment of `source`: one fix for all the
/// ordinary ones (anchored at the first), and one per element-children line.
pub(super) fn plan_comment_fixes(source: &str) -> Result<Vec<Fix>, String> {
    if !source.contains(OLD_LINE_COMMENT) {
        return Ok(Vec::new());
    }
    let lexer = lex(source)?;
    let mut ordinary: Vec<Splice> = Vec::new();
    let mut fixes = Vec::new();
    for (k, trivia) in lexer.token_leading_trivia.iter().enumerate() {
        for t in trivia {
            if t.kind != TriviaKind::LineComment || !t.text.starts_with(OLD_LINE_COMMENT) {
                continue;
            }
            let start = t.start as usize;
            let end = start + t.text.chars().count();
            if lexer.jsx_line_comments.binary_search(&t.start).is_err() {
                ordinary.push(Splice {
                    start,
                    end,
                    text: respell(&t.text),
                });
                continue;
            }
            // Between two text runs the comment is what joins them into one
            // child; a hole there would make them two.
            let joins_text = k > 0
                && matches!(lexer.tokens[k - 1], Token::JsxText(_))
                && matches!(lexer.tokens[k], Token::JsxText(_));
            fixes.push(if joins_text {
                Fix {
                    anchor: start,
                    message: "a `//` comment line in element children is deprecated; write \
                              `{/* … */}` (between two lines of text, which a hole would split \
                              into two children, so move it by hand)"
                        .to_string(),
                    splices: Vec::new(),
                }
            } else {
                Fix {
                    anchor: start,
                    message: "a `//` comment line in element children is deprecated; write \
                              `{/* … */}`"
                        .to_string(),
                    splices: vec![Splice {
                        start,
                        end,
                        text: as_hole(&t.text),
                    }],
                }
            });
        }
    }
    if let Some(first) = ordinary.first() {
        let n = ordinary.len();
        fixes.push(Fix {
            anchor: first.start,
            message: if n == 1 {
                "a `//` comment is deprecated; write `#`".to_string()
            } else {
                format!("`//` comments are deprecated; write `#` ({n} in this file)")
            },
            splices: ordinary,
        });
    }
    fixes.sort_by_key(|f| f.anchor);
    Ok(fixes)
}

/// The gate: `after` lexes to the tokens of `before` on the same lines,
/// comments aside, plus one empty `{}` hole for each children comment that
/// became one.
pub(super) fn same_tokens_but_comments(before: &str, after: &str) -> Result<(), String> {
    let (b, a) = (lex(before)?, lex(after)?);
    let bug = |line: u32| {
        format!(
            "migrate bug: respelling comments changed the token stream at line {line} — \
             refusing to produce output"
        )
    };
    let mut holes = b.jsx_line_comments.len().saturating_sub(a.jsx_line_comments.len());
    let line = |l: &Lexer, i: usize| l.token_spans[i].start.line;
    let (mut i, mut j) = (0, 0);
    while i < b.tokens.len() || j < a.tokens.len() {
        let same = i < b.tokens.len()
            && j < a.tokens.len()
            && b.tokens[i] == a.tokens[j]
            && line(&b, i) == line(&a, j);
        if same {
            i += 1;
            j += 1;
            continue;
        }
        let hole = holes > 0
            && matches!(a.tokens.get(j), Some(Token::LBrace))
            && matches!(a.tokens.get(j + 1), Some(Token::RBrace));
        if !hole {
            return Err(bug(if j < a.tokens.len() { line(&a, j) } else { 0 }));
        }
        holes -= 1;
        j += 2;
    }
    if holes > 0 {
        return Err(bug(0));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrate(src: &str) -> String {
        let fixes = plan_comment_fixes(src).unwrap();
        let mut splices: Vec<&Splice> = fixes.iter().flat_map(|f| &f.splices).collect();
        splices.sort_by_key(|s| s.start);
        let mut out: Vec<char> = src.chars().collect();
        for s in splices.iter().rev() {
            out.splice(s.start..s.end, s.text.chars());
        }
        let out: String = out.into_iter().collect();
        same_tokens_but_comments(src, &out).unwrap();
        out
    }

    #[test]
    fn respells_the_marker_and_adds_a_space() {
        assert_eq!(
            migrate("// header\nlet a = 1 // one\nlet b = 2 //two\n//\n//fff\n  // indented\n"),
            "# header\nlet a = 1 # one\nlet b = 2 # two\n#\n# fff\n  # indented\n"
        );
    }

    #[test]
    fn longer_slash_runs_keep_their_shape() {
        assert_eq!(migrate("/// doc\n//////////\n//// x\n///y\n"), "## doc\n#########\n### x\n## y\n");
    }

    #[test]
    fn directives_keep_working() {
        let out = migrate(
            "// petal-lint-ignore-file prefer-match\n// petal-fmt-ignore\nvar x = 1 // petal-lint-ignore prefer-let\n",
        );
        assert_eq!(
            out,
            "# petal-lint-ignore-file prefer-match\n# petal-fmt-ignore\nvar x = 1 # petal-lint-ignore prefer-let\n"
        );
        let ignores = crate::lint::Ignores::scan(&out).unwrap();
        assert!(ignores.suppresses("prefer-match", 7));
        assert!(ignores.suppresses("prefer-let", 2));
    }

    #[test]
    fn strings_raw_strings_and_block_comments_are_untouched() {
        let src = "let url = \"http://x // y\"\nlet glsl = \"\"\"\n// glsl comment\n#define K 1\n\"\"\"\n/* // inside */ let s = \"{a} // b\"\nlet d = 7 / 2 // half\n";
        assert_eq!(
            migrate(src),
            "let url = \"http://x // y\"\nlet glsl = \"\"\"\n// glsl comment\n#define K 1\n\"\"\"\n/* // inside */ let s = \"{a} // b\"\nlet d = 7 / 2 # half\n"
        );
    }

    #[test]
    fn a_children_comment_line_becomes_a_hole() {
        let src = "let v = <col>\n  // the header\n  <row gap={2} // not a child\n  />\n  //\n  text here\n  // closes */ early\n</col>\n";
        assert_eq!(
            migrate(src),
            "let v = <col>\n  {/* the header */}\n  <row gap={2} # not a child\n  />\n  {/* */}\n  text here\n  {/* closes * / early */}\n</col>\n"
        );
    }

    #[test]
    fn a_comment_between_two_text_lines_is_left_for_a_hand_edit() {
        let src = "let v = <p>\n  one\n  // note\n  two\n</p>\n// after\n";
        let fixes = plan_comment_fixes(src).unwrap();
        assert_eq!(fixes.len(), 2);
        assert!(fixes[0].splices.is_empty());
        assert!(fixes[0].message.contains("by hand"), "{}", fixes[0].message);
        assert_eq!(migrate(src), "let v = <p>\n  one\n  // note\n  two\n</p>\n# after\n");
    }

    #[test]
    fn reads_a_file_that_still_has_hash_colors() {
        assert_eq!(migrate("let a = #f80 // orange\n"), "let a = #f80 # orange\n");
    }

    #[test]
    fn is_idempotent() {
        let once = migrate("// a\nlet v = <c>\n  // b\n</c> //c\n");
        assert!(plan_comment_fixes(&once).unwrap().is_empty(), "{once}");
    }

    #[test]
    fn the_gate_rejects_a_changed_token_stream() {
        assert!(same_tokens_but_comments("let a = 1 // x\n", "let a = 1 # x\n").is_ok());
        assert!(same_tokens_but_comments("let a = 1 // x\n", "let a = 1\n# x\n").is_err());
        assert!(same_tokens_but_comments("let a = 7 // 2\n", "let a = 7 / 2\n").is_err());
        assert!(same_tokens_but_comments("let a = {} // x\n", "let a = # x\n").is_err());
        // A hole is only allowed where a children comment went.
        assert!(same_tokens_but_comments("let a = 1 // x\n", "let a = 1 {}\n").is_err());
    }
}
