//! Migration `colour-literals` — a color literal is written `:f80`.
//!
//! ```text
//! let tint = #ff8800                =>   let tint = :ff8800
//! draw_rect(r, color: #f80a)        =>   draw_rect(r, color: :f80a)
//! ```
//!
//! `#` is the line-comment marker, so the color sigil moved to `:`
//! (docs/tasks/pythonic-syntax.md §3.3). Both spellings lex to the same
//! [`Token::Color`] and lower to the same `{r, g, b[, a]}` record, so the
//! rewrite is one character and the program does not change.
//!
//! **Reading the old form.** The language's lexer no longer reads `#f80` as a
//! color: it is an error there (a `#` followed directly by a color-shaped
//! hex word, so that a stale literal never turns into a comment). The
//! migrator reads it with [`legacy_lex`], the lexer's legacy mode, in which
//! that same shape is the color it was and every other `#` is a comment.
//! Every step of `petal migrate-old-code` that has to read a file before
//! this migration has run on it goes through [`legacy_lex`].
//!
//! The sites come from the **token stream**, never from the text: a `"#fff"`
//! in a string, a `#define` in a `"""` GLSL block and a `#hex` word in a
//! `//` or `/* */` comment are not color tokens and are left as they are.
//!
//! **Gate.** The migrated text must lex to exactly the token stream of the
//! original (same tokens, same spans: the rewrite keeps every offset), which
//! is the same parse. `:` is also a separator, and it starts a color only
//! when the token before it cannot end an expression
//! ([`crate::lexer::can_end_operand`]); a `#` color written directly after
//! such a token (`fn bg() #fff end`) would not survive the respelling, so it
//! is reported for a hand edit (`fn bg() (:fff) end`) instead of rewritten.

use crate::lexer::{Lexer, Token};
use crate::lint::Fix;
use crate::rewrite::Splice;
use crate::source_map::SourceSpan;


/// The color literal sigil.
pub const SIGIL: char = ':';

/// One `#hex` color token.
pub(crate) struct LegacyColour {
    pub span: SourceSpan,
    /// The hex digits, as written.
    pub hex: String,
    /// Whether `:hex` in its place lexes as a color too: false when the
    /// token before it can end an expression (rule B16).
    pub respellable: bool,
}

/// `source` lexed the way the migrator reads old code: `#f80` is a color
/// token ([`Lexer::legacy`]). Migrated source lexes to the same tokens here
/// as in the language's own lexer.
pub(crate) fn legacy_lex(source: &str) -> Result<Lexer, String> {
    let mut lexer = Lexer::new(source).without_trivia().legacy();
    lexer.tokenize()?;
    Ok(lexer)
}

/// Whether `source` lexes today: false while it holds a `#f80` color (one
/// this migration was not run on, was told to skip, or left for a hand
/// edit), which only [`legacy_lex`] reads.
pub(super) fn lexes_today(source: &str) -> bool {
    Lexer::new(source).without_trivia().tokenize().is_ok()
}

/// Every color literal of `source` written with the deprecated `#`, in source
/// order.
pub(crate) fn legacy_colours(source: &str) -> Result<Vec<LegacyColour>, String> {
    let lexer = legacy_lex(source)?;
    Ok(lexer
        .legacy_colors
        .iter()
        .filter_map(|c| {
            let Token::Color(hex) = &lexer.tokens[c.token] else {
                return None;
            };
            Some(LegacyColour {
                span: lexer.token_spans[c.token],
                hex: hex.clone(),
                respellable: c.respellable,
            })
        })
        .collect())
}

/// Plan the `#hex` → `:hex` respelling of every color token of `source`.
pub(super) fn plan_colour_fixes(source: &str) -> Result<Vec<Fix>, String> {
    Ok(legacy_colours(source)?
        .into_iter()
        .map(|c| {
            let at = c.span.start.offset as usize;
            let (message, splices) = if c.respellable {
                (
                    format!("`#{0}` is no longer a color literal; write `:{0}`", c.hex),
                    vec![Splice {
                        start: at,
                        end: at + 1,
                        text: SIGIL.to_string(),
                    }],
                )
            } else {
                // `:` after a token that can end an expression is a
                // separator, so the bare respelling would not be a color.
                (
                    format!(
                        "`#{0}` is no longer a color literal; write `(:{0})` (a `:` directly after a value or \
                         a name is a separator, so this one needs the parentheses)",
                        c.hex
                    ),
                    Vec::new(),
                )
            };
            Fix {
                anchor: at,
                message,
                splices,
            }
        })
        .collect())
}

/// `source` with every respellable `#hex` color token written `:hex`, and
/// nothing else changed: the migration as a function, with its token gate
/// and without `petal-lint-ignore`. For a host that is handed Petal source
/// in the old spelling and writes it into a file (`pb_source_set`). A color
/// the respelling cannot reach (directly after a value) is left as written.
pub fn respell_colour_literals(source: &str) -> Result<String, String> {
    let mut chars: Vec<char> = source.chars().collect();
    let mut changed = false;
    for c in legacy_colours(source)? {
        if c.respellable {
            chars[c.span.start.offset as usize] = SIGIL;
            changed = true;
        }
    }
    if !changed {
        return Ok(source.to_string());
    }
    let out: String = chars.into_iter().collect();
    same_tokens(source, &out)?;
    Ok(out)
}

/// The gate: `after` lexes to exactly the tokens of `before`, spans included
/// (the respelling keeps every offset), so it is the same parse.
pub(super) fn same_tokens(before: &str, after: &str) -> Result<(), String> {
    let (before, after) = (legacy_lex(before)?, legacy_lex(after)?);
    let (bt, bs, at) = (&before.tokens, &before.token_spans, &after.tokens);
    if bt != at || *bs != after.token_spans {
        let n = bt.iter().zip(at).take_while(|(b, a)| b == a).count();
        let line = bs.get(n).map_or(0, |s| s.start.line);
        return Err(format!(
            "migrate bug: respelling color literals changed the token stream at line {line} — \
             refusing to produce output"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrate(src: &str) -> String {
        let fixes = plan_colour_fixes(src).unwrap();
        let mut out: Vec<char> = src.chars().collect();
        for f in fixes.iter().rev() {
            for s in f.splices.iter().rev() {
                out.splice(s.start..s.end, s.text.chars());
            }
        }
        let out: String = out.into_iter().collect();
        same_tokens(src, &out).unwrap();
        out
    }

    #[test]
    fn respells_color_tokens_only() {
        assert_eq!(
            migrate("let a = #f80\nlet b = {tint: #000000, glass: #10203040}\ndraw(r, #FFF, color: #abcd)\n"),
            "let a = :f80\nlet b = {tint: :000000, glass: :10203040}\ndraw(r, :FFF, color: :abcd)\n"
        );
    }

    #[test]
    fn strings_raw_strings_and_comments_are_untouched() {
        let src = "// was #fff, now #000\nlet css = \"#ff8800\"\nlet glsl = \"\"\"\n#define K 1 // #abc\nvec3 c = #fff;\n\"\"\"\nlet s = \"tint {#fff} #abc\"\n/* #123 */\n<text>#fff</text>\n";
        let out = migrate(src);
        assert_eq!(
            out,
            "// was #fff, now #000\nlet css = \"#ff8800\"\nlet glsl = \"\"\"\n#define K 1 // #abc\nvec3 c = #fff;\n\"\"\"\nlet s = \"tint {:fff} #abc\"\n/* #123 */\n<text>#fff</text>\n"
        );
    }

    #[test]
    fn patterns_keywords_and_jsx_attributes() {
        assert_eq!(
            migrate("let n = match c\n  when #fff -> 1\n  when x -> if x == #000 then #111 else #222 end\nend\nlet r = <rect fill=#f80 stroke={#000} />\n"),
            "let n = match c\n  when :fff -> 1\n  when x -> if x == :000 then :111 else :222 end\nend\nlet r = <rect fill=:f80 stroke={:000} />\n"
        );
    }

    #[test]
    fn a_color_right_after_a_value_is_left_for_a_hand_edit() {
        let src = "fn bg() #fff end\nlet a = #000\n";
        let fixes = plan_colour_fixes(src).unwrap();
        assert_eq!(fixes.len(), 2);
        assert!(fixes[0].splices.is_empty());
        assert!(fixes[0].message.contains("(:fff)"), "{}", fixes[0].message);
        assert_eq!(migrate(src), "fn bg() #fff end\nlet a = :000\n");
    }

    #[test]
    fn respell_is_the_migration_as_a_function() {
        assert_eq!(respell_colour_literals("#29d9ff").unwrap(), ":29d9ff");
        assert_eq!(respell_colour_literals("{a: #fff, b: \"#000\"}").unwrap(), "{a: :fff, b: \"#000\"}");
        assert_eq!(respell_colour_literals("vec3(1, 2, 3)").unwrap(), "vec3(1, 2, 3)");
        assert!(respell_colour_literals("\"open").is_err());
    }

    #[test]
    fn is_idempotent() {
        let once = migrate("let a = [#f80, #fff]\n");
        assert!(plan_colour_fixes(&once).unwrap().is_empty());
    }

    #[test]
    fn the_gate_rejects_a_changed_token_stream() {
        assert!(same_tokens("fn bg() #fff end\n", "fn bg() :fff end\n").is_err());
        assert!(same_tokens("let a = #fff\n", "let a = :fff\n").is_ok());
    }

    #[test]
    fn only_the_legacy_lexer_reads_the_old_form() {
        assert!(!lexes_today("let a = #f80\n"));
        assert!(lexes_today("let a = :f80 # was #f80\n"));
        let err = crate::lexer::Lexer::new("let a = b ?? #fff\n").tokenize().unwrap_err();
        assert!(err.contains("`#fff` is no longer a color literal; write `:fff`"), "{err}");
        assert!(err.contains("petal migrate-old-code"), "{err}");
        assert!(err.contains("[colour-literals]"), "{err}");
        // A `#` comment is the same in both lexers.
        let src = "# note\nlet a = :fff # fade in\n#!x\n";
        let (new, old) = (crate::lexer::Lexer::new(src).tokenize().unwrap().to_vec(), legacy_lex(src).unwrap());
        assert_eq!(new, old.tokens);
        assert!(old.legacy_colors.is_empty());
    }
}
