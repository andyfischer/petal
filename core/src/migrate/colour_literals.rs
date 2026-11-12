//! Migration `colour-literals` — a color literal is written `:f80`.
//!
//! ```text
//! let tint = #ff8800                =>   let tint = :ff8800
//! draw_rect(r, color: #f80a)        =>   draw_rect(r, color: :f80a)
//! ```
//!
//! `#` is becoming the line-comment marker, so the color sigil moves to `:`
//! (docs/tasks/pythonic-syntax.md §3.3). Both spellings lex to the same
//! [`Token::Color`] and lower to the same `{r, g, b[, a]}` record, so the
//! rewrite is one character and the program does not change.
//!
//! The sites come from the **token stream**, never from the text: a `"#fff"`
//! in a string, a `#define` in a `"""` GLSL block and a `#hex` word in a
//! comment are not color tokens and are left as they are.
//!
//! **Gate.** The migrated text must lex to exactly the token stream of the
//! original (same tokens, same spans: the rewrite keeps every offset), which
//! is the same parse. `:` is also a separator, and it starts a color only
//! when the token before it cannot end an expression
//! ([`crate::lexer::can_end_operand`]); a `#` color written directly after
//! such a token (`fn bg() #fff end`) would not survive the respelling, so it
//! is reported for a hand edit (`fn bg() (:fff) end`) instead of rewritten.
//!
//! This module also finds the sites for the compiler's deprecation warning
//! ([`deprecated_colour_literals`]).
//!
//! When the main lexer stops reading `#f80` as a color (the `#` comments
//! phase), [`legacy_lex`] is where the migrator keeps reading it: every
//! migration that needs the old form goes through it.

use crate::lexer::{Lexer, Token};
use crate::lint::Fix;
use crate::rewrite::Splice;
use crate::source_map::SourceSpan;

use super::COLOUR_LITERALS;

/// The sigil of the deprecated spelling and of the current one.
pub const DEPRECATED_SIGIL: char = '#';
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
/// token. Today that is the lexer itself.
pub(crate) fn legacy_lex(source: &str) -> Result<Lexer, String> {
    let mut lexer = Lexer::new(source).without_trivia();
    lexer.tokenize()?;
    Ok(lexer)
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
                    format!("`#{0}` is deprecated; write `:{0}`", c.hex),
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
                        "`#{0}` is deprecated; write `(:{0})` (a `:` directly after a value or \
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

/// The deprecation warning for each `#hex` color literal of a module, tagged
/// `[colour-literals]` and pointing at `petal migrate-old-code`. Silenced with
/// `// petal-lint-ignore colour-literals` on the line before or trailing on
/// the line (or `petal-lint-ignore-file`), which keeps the migration off it
/// too.
///
/// A source with no `#` at all (every migrated file) costs one scan.
pub(crate) fn deprecated_colour_literals(source: &str) -> Vec<crate::diagnostic::Diagnostic> {
    if !source.contains(DEPRECATED_SIGIL) {
        return Vec::new();
    }
    // A source that does not lex is reported by the parse, not here.
    let Ok(sites) = legacy_colours(source) else {
        return Vec::new();
    };
    if sites.is_empty() {
        return Vec::new();
    }
    // A file whose comments do not scan is reported in full.
    let ignores = crate::lint::Ignores::scan(source).ok();
    sites
        .into_iter()
        .filter(|c| {
            let line = (c.span.start.line as usize).saturating_sub(1);
            !ignores.as_ref().is_some_and(|i| i.suppresses(COLOUR_LITERALS, line))
        })
        .map(|c| {
            let new = if c.respellable { format!(":{}", c.hex) } else { format!("(:{})", c.hex) };
            crate::diagnostic::Diagnostic::new(
                c.span,
                format!(
                    "color literals are now written `{new}` (not `#{}`); {} [{COLOUR_LITERALS}]",
                    c.hex,
                    super::RUN_HINT
                ),
            )
        })
        .collect()
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
    fn warning_names_the_new_spelling_and_the_tool() {
        let w = deprecated_colour_literals("let a = #f80\nlet b = :fff\nlet c = \"#fff\"\n");
        assert_eq!(w.len(), 1);
        assert!(w[0].message.contains("`:f80`"), "{}", w[0].message);
        assert!(w[0].message.contains("petal migrate-old-code"));
        assert!(w[0].message.ends_with("[colour-literals]"));
        assert_eq!(w[0].span.start.line, 1);
        assert!(deprecated_colour_literals("// petal-lint-ignore colour-literals\nlet a = #f80\n").is_empty());
    }
}
