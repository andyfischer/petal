//! Pass 1 — token-driven 2-space re-indentation.

use crate::lexer::{Lexer, Token};

/// The open-construct stack: one entry per unclosed construct, holding the
/// indent its *contents* get — the display indent of the line that opened it,
/// plus one. Keying content indent to the opening line (rather than raw stack
/// depth) makes several delimiters opened on one line (`column([`) indent
/// their contents once, and their closers (`])`) realign with the opening
/// line.
///
/// Constructs and their closers: `end` closes fn/enum/if/for/while/match,
/// block lambdas, and `when … do` arms; `)` `]` `}` close their delimiters;
/// `</tag>` closes a JSX element's children. Closers just pop the innermost
/// entry — lint runs on parseable source, so they always correspond.
type OpenStack = Vec<usize>;

/// Re-indent `source` to 2-space indentation, trim trailing whitespace, and
/// end with exactly one newline. Only whitespace outside tokens is touched;
/// lines that start inside a multi-line token (raw string, JSX text) are
/// copied verbatim. Works from the token stream alone, so it needs the source
/// to lex but not to parse.
pub fn reindent(source: &str) -> Result<String, String> {
    if source.is_empty() {
        return Ok(String::new());
    }
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let tokens = &lexer.tokens;
    let spans = &lexer.token_spans;
    let chars: Vec<char> = source.chars().collect();

    // Line table: (start, end) char offsets, `end` at the `\n` (or EOF).
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    lines.push((start, chars.len()));

    let mut stack: OpenStack = Vec::new();
    // A `for`/`while` header's `do` belongs to the construct already opened at
    // the keyword; only a `do` with no pending header opens a block itself
    // (a `when … do` match arm).
    let mut pending_do = false;
    // Inside `when pattern [if guard]` the `if` is a guard, not an opener.
    // Cleared at the arm's `do`/`->` (or a newline, defensively).
    let mut when_header = false;
    // Stack of unterminated JSX opening tags, each recording the delimiter
    // depth at its `<`: the `>` that ends the tag is the one seen back at that
    // depth (a `>` inside an `attr={a > b}` brace sits deeper).
    let mut open_tags: Vec<usize> = Vec::new();

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut ti = 0usize; // next token index
    let mut covered_end = 0usize; // max token end seen so far

    for &(ls, le) in &lines {
        // Tokens starting on this line (the Newline terminator included).
        let first_ti = ti;
        while ti < tokens.len() && (spans[ti].start.offset as usize) <= le {
            covered_end = covered_end.max(spans[ti].end.offset as usize);
            ti += 1;
        }
        let line_tokens = first_ti..ti;

        // A line beginning inside a token that started earlier (multi-line raw
        // string, JSX text) is content, not layout — copy it verbatim. The
        // check uses tokens *before* this line, so recompute from `ti` bounds.
        let starts_inside = spans[..first_ti]
            .iter()
            .any(|s| (s.start.offset as usize) < ls && (s.end.offset as usize) > ls);

        // Leading-whitespace run.
        let mut ws_end = ls;
        while ws_end < le && chars[ws_end] != '\n' && chars[ws_end].is_whitespace() {
            ws_end += 1;
        }
        // A token starting within the leading run means that "whitespace" is
        // token content (JSX text beginning at line start) — leave it alone.
        let token_in_leading_ws = line_tokens.clone().any(|k| {
            let s = spans[k].start.offset as usize;
            s >= ls && s < ws_end
        });

        // Trailing trim is safe only when no token spills past the line end
        // (the spill means the tail is inside a multi-line token).
        let spills_past_end = line_tokens
            .clone()
            .any(|k| (spans[k].end.offset as usize) > le + 1);

        // Display indent. A line opening no new construct sits at the
        // innermost open construct's content indent. A run of closers at the
        // start of the line realigns with the line that opened the outermost
        // construct the run closes; `else`/`elsif` realign with their `if`.
        let sig: Vec<usize> = line_tokens
            .clone()
            .filter(|&k| !matches!(tokens[k], Token::Newline | Token::Eof))
            .collect();
        let mut dedent = 0usize;
        let mut si = 0usize;
        while si < sig.len() {
            match tokens[sig[si]] {
                Token::End | Token::RParen | Token::RBracket | Token::RBrace => {
                    dedent += 1;
                    si += 1;
                }
                Token::JsxCloseStart => {
                    dedent += 1;
                    si += 1;
                    if si < sig.len() && matches!(tokens[sig[si]], Token::JsxTagName(_)) {
                        si += 1;
                    }
                }
                _ => break,
            }
        }
        let indent = if dedent > 0 {
            // Align with the opener of the outermost construct the run closes.
            stack
                .len()
                .checked_sub(dedent)
                .and_then(|i| stack.get(i))
                .map_or(0, |content| content.saturating_sub(1))
        } else if matches!(
            sig.first().map(|&k| &tokens[k]),
            Some(Token::Else | Token::Elsif)
        ) {
            stack.last().map_or(0, |content| content.saturating_sub(1))
        } else {
            stack.last().copied().unwrap_or(0)
        };

        // Render the line.
        if starts_inside || token_in_leading_ws {
            out_lines.push(chars[ls..le].iter().collect());
        } else {
            let mut content_end = le;
            if !spills_past_end {
                while content_end > ws_end && chars[content_end - 1].is_whitespace() {
                    content_end -= 1;
                }
            }
            if content_end == ws_end {
                out_lines.push(String::new()); // blank line
            } else {
                let mut line = "  ".repeat(indent);
                line.extend(chars[ws_end..content_end].iter());
                out_lines.push(line);
            }
        }

        // Update depth with this line's tokens (done for every line — a
        // verbatim line can still contain tokens that open or close blocks).
        // Constructs opened on this line indent their contents one past this
        // line's own indent, however many of them open here.
        for k in line_tokens {
            match &tokens[k] {
                Token::Newline => when_header = false,
                Token::When => when_header = true,
                Token::Arrow => when_header = false,
                Token::Do => {
                    when_header = false;
                    if pending_do {
                        pending_do = false;
                    } else {
                        stack.push(indent + 1); // `when … do` arm body
                    }
                }
                Token::For | Token::While => {
                    pending_do = true;
                    stack.push(indent + 1);
                }
                Token::If => {
                    if !when_header {
                        stack.push(indent + 1);
                    }
                }
                Token::Match | Token::Enum => stack.push(indent + 1),
                Token::Fn => {
                    if fn_takes_end(tokens, k) {
                        stack.push(indent + 1);
                    }
                }
                Token::LParen | Token::LBracket | Token::LBrace => stack.push(indent + 1),
                Token::End | Token::RParen | Token::RBracket | Token::RBrace => {
                    stack.pop();
                }
                Token::JsxOpenStart => open_tags.push(stack.len()),
                Token::JsxSelfClose => {
                    open_tags.pop();
                }
                Token::Gt => {
                    if open_tags.last() == Some(&stack.len()) {
                        open_tags.pop();
                        stack.push(indent + 1); // children until `</tag>`
                    }
                }
                Token::JsxCloseStart => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }

    // Single trailing newline: drop blank lines at EOF, end with exactly one.
    while out_lines.last().is_some_and(|l| l.is_empty()) {
        out_lines.pop();
    }
    if out_lines.is_empty() {
        return Ok(String::new());
    }
    let mut out = out_lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Whether the `fn` at token index `k` opens an `end`-terminated block: a
/// declaration (`fn name(…) … end`) or a block-bodied lambda (`fn(…) … end`).
/// Only an arrow lambda (`fn(…) -> expr`) doesn't consume an `end`.
fn fn_takes_end(tokens: &[Token], k: usize) -> bool {
    let mut i = k + 1;
    while i < tokens.len() && matches!(tokens[i], Token::Newline) {
        i += 1;
    }
    match tokens.get(i) {
        Some(Token::Ident(_)) => true, // declaration
        Some(Token::LParen) => {
            // Lambda: skip the parameter list to its matching `)`.
            let mut depth = 0usize;
            while i < tokens.len() {
                match tokens[i] {
                    Token::LParen => depth += 1,
                    Token::RParen => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i += 1;
            while i < tokens.len() && matches!(tokens[i], Token::Newline) {
                i += 1;
            }
            !matches!(tokens.get(i), Some(Token::Arrow))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindents_fn_if_for_to_two_spaces() {
        let src = "fn f(a)\nif a > 1 then\nreturn a\nend\nfor i in [1, 2] do\nprint(i)\nend\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "fn f(a)\n  if a > 1 then\n    return a\n  end\n  for i in [1, 2] do\n    print(i)\n  end\nend\n"
        );
    }

    #[test]
    fn reindents_else_elsif_at_block_level() {
        let src = "if a then\n      x\n   elsif b then\n y\nelse\n  z\n      end\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "if a then\n  x\nelsif b then\n  y\nelse\n  z\nend\n");
    }

    #[test]
    fn reindents_match_with_do_arms_house_style() {
        // `when` at match+1, do-arm bodies one deeper, arm `end` back at `when`.
        let src = "let r = match e\nwhen Add(t) do\nitems = append(items, t)\ntrue\nend\nwhen None() -> false\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let r = match e\n  when Add(t) do\n    items = append(items, t)\n    true\n  end\n  when None() -> false\nend\n"
        );
    }

    #[test]
    fn when_guard_if_is_not_an_opener() {
        let src = "match s\nwhen Red if t >= 5 do\nx = 1\nend\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "match s\n  when Red if t >= 5 do\n    x = 1\n  end\nend\n"
        );
    }

    #[test]
    fn multiline_collections_indent_one_level() {
        let src = "let xs = [\n1,\n2\n]\nlet r = {\na: 1\nb: 2\n}\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let xs = [\n  1,\n  2\n]\nlet r = {\n  a: 1\n  b: 2\n}\n"
        );
    }

    #[test]
    fn leading_closer_run_dedents_by_run_length() {
        let src = "layout(\ncolumn([\neditor()\n])\n)\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "layout(\n  column([\n    editor()\n  ])\n)\n");
    }

    #[test]
    fn arrow_lambda_does_not_open_a_block() {
        let src = "let ys = map(xs, fn(x) -> x * 2)\nlet z = 1\n";
        assert_eq!(reindent(src).unwrap(), src);
    }

    #[test]
    fn block_lambda_opens_and_end_closes() {
        let src = "let f = fn(x)\nx * 2\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let f = fn(x)\n  x * 2\nend\n");
    }

    #[test]
    fn enum_variants_indent() {
        let src = "enum Event\nNone()\nAdd(text)\nend\n";
        assert_eq!(
            reindent(src).unwrap(),
            "enum Event\n  None()\n  Add(text)\nend\n"
        );
    }

    #[test]
    fn jsx_children_indent_and_close_tag_dedents() {
        let src = "let e = <div class=\"x\">\n<p>hi</p>\n<br/>\n</div>\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let e = <div class=\"x\">\n  <p>hi</p>\n  <br/>\n</div>\n"
        );
    }

    #[test]
    fn gt_inside_jsx_attr_brace_is_not_a_tag_end() {
        let src = "let e = <div a={x > 1}>\n<p>y</p>\n</div>\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let e = <div a={x > 1}>\n  <p>y</p>\n</div>\n");
    }

    #[test]
    fn raw_string_interior_lines_are_untouched() {
        // Lines inside a multi-line raw string are content, not layout.
        let src = "if a then\nlet s = \"\"\"\n   keep   me\n\"\"\"\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "if a then\n  let s = \"\"\"\n   keep   me\n\"\"\"\nend\n"
        );
    }

    #[test]
    fn comments_reindent_with_their_block() {
        let src = "fn f()\n// leading\nlet x = 1 // trailing\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "fn f()\n  // leading\n  let x = 1 // trailing\nend\n");
    }

    #[test]
    fn trims_trailing_whitespace_and_ensures_single_final_newline() {
        let src = "let x = 1   \n\n\nlet y = 2\t\n\n\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let x = 1\n\n\nlet y = 2\n");
    }

    #[test]
    fn adds_missing_final_newline() {
        assert_eq!(reindent("let x = 1").unwrap(), "let x = 1\n");
    }

    #[test]
    fn empty_source_stays_empty() {
        assert_eq!(reindent("").unwrap(), "");
    }

    #[test]
    fn inline_if_and_string_interp_are_neutral() {
        let src = "let x = if c then 1 else 2 end\nprint(\"sum = {2 + (3)} done\")\nlet y = 1\n";
        assert_eq!(reindent(src).unwrap(), src);
    }
}
