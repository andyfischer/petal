//! Pass 2 — token-driven 2-space re-indentation.

use crate::lexer::{Lexer, Token};

/// The open-construct stack: one entry per unclosed construct, holding the
/// column its *contents* get — the column the line that opened it is shown
/// at, plus one step. Keying content to the opening line (rather than raw
/// stack depth) makes several delimiters opened on one line (`column([`)
/// indent their contents once, and a line of closers (`])`) realign with the
/// line that opened its first closer's construct.
///
/// Constructs and their closers: `end` closes fn/enum/class/if/for/while/match,
/// block lambdas, and `when … do` arms; `)` `]` `}` close their delimiters;
/// `</tag>` closes a JSX element's children. Closers just pop the innermost
/// entry — lint runs on parseable source, so they always correspond.
///
/// A construct that *hangs* ([`Hang`]) — `draw(a, b,` with `c)` wrapped
/// below — is a visual continuation: the author lined the wrapped lines up
/// under it by eye. Those lines keep their position relative to the opener's
/// line, shifting only by however far that line moved, rather than being
/// forced to the block indent, which would lose the alignment. So does a line
/// that opens with a binary operator, relative to the statement it continues.
type OpenStack = Vec<Open>;

/// Columns per indent step.
const INDENT: usize = 2;

#[derive(Clone, Copy)]
struct Open {
    /// Column of the construct's contents.
    content: usize,
    /// Set when the construct *hangs* off its opening line.
    hang: Option<Hang>,
}

/// A construct whose contents the author may have aligned by eye: a delimiter
/// with contents on its own line, or an `if` that starts mid-line
/// (`let y = if c then a` with `elsif`/`else` lined up below).
#[derive(Clone, Copy)]
struct Hang {
    /// How many columns the opener's line moved.
    shift: isize,
    /// The opener line's new indent. A hanging line keeps its (shifted)
    /// column only while it stays deeper than this.
    floor: usize,
}

impl Open {
    fn block(content: usize) -> Self {
        Open {
            content,
            hang: None,
        }
    }
}

/// Can `t` open a line that continues the one above (`* (1.0 + k)`)?
fn is_continuation_op(t: &Token) -> bool {
    use Token::*;
    matches!(
        t,
        Plus | Minus
            | Star
            | Slash
            | Percent
            | PlusPlus
            | And
            | Or
            | DoubleQuestion
            | Pipe
            | Dot
            | QuestionDot
            | Eq
            | Ne
            | Lt
            | Le
            | Gt
            | Ge
    )
}

/// Re-indent `source` to 2-space indentation, trim trailing whitespace, and
/// end with exactly one newline. Only whitespace outside tokens is touched;
/// lines that start inside a multi-line token (raw string, JSX text) are
/// copied verbatim. Works from the token stream alone, so it needs the source
/// to lex but not to parse.
pub fn reindent(source: &str) -> Result<String, String> {
    reindent_protected(source, &[])
}

/// [`reindent`], copying each line marked in `protected` (0-based) verbatim —
/// the lines a `// petal-fmt-ignore` / `petal-fmt-off` comment fenced off.
/// Their tokens still open and close constructs, so the lines after them
/// indent as if the fence were not there.
pub fn reindent_protected(source: &str, protected: &[bool]) -> Result<String, String> {
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
    // The most recent line that did not open with an operator: what a
    // leading-operator continuation line hangs from.
    let mut stmt = Hang { shift: 0, floor: 0 };
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

    for (line_idx, &(ls, le)) in lines.iter().enumerate() {
        // Tokens starting on this line (the Newline terminator included).
        let first_ti = ti;
        while ti < tokens.len() && (spans[ti].start.offset as usize) <= le {
            ti += 1;
        }
        let line_tokens = first_ti..ti;

        // A line beginning inside a token that started earlier (multi-line raw
        // string, JSX text) is content, not layout — copy it verbatim.
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

        // Display indent, in columns. A line opening no new construct sits at
        // the innermost open construct's content column. A run of closers at
        // the start of the line realigns with the line that opened the
        // construct its first closer closes; `else`/`elsif` realign with
        // their `if`.
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
        let leads_with_else = matches!(
            sig.first().map(|&k| &tokens[k]),
            Some(Token::Else | Token::Elsif)
        );
        // The construct this line's column is judged against, and the column
        // the block rule gives it.
        let (governing, block_cols) = if dedent > 0 || leads_with_else {
            let open = stack.last();
            (open, open.map_or(0, |o| o.content.saturating_sub(INDENT)))
        } else {
            let open = stack.last();
            (open, open.map_or(0, |o| o.content))
        };
        let old_cols = ws_end - ls;
        // A hanging continuation keeps its column, moved with the line it
        // hangs from, as long as it still sits deeper than that line.
        let keep = |shift: isize, floor: usize| {
            let cols = old_cols as isize + shift;
            (cols > floor as isize).then_some(cols as usize)
        };
        let leads_with_op = sig.first().is_some_and(|&k| is_continuation_op(&tokens[k]));
        let visual_cols = match governing {
            Some(Open { hang: Some(h), .. }) => keep(h.shift, h.floor),
            _ if leads_with_op && dedent == 0 => keep(stmt.shift, stmt.floor.saturating_sub(1)),
            _ => None,
        };
        let new_cols = visual_cols.unwrap_or(block_cols);
        let verbatim = starts_inside
            || token_in_leading_ws
            || protected.get(line_idx).copied().unwrap_or(false);
        let shown_cols = if verbatim { old_cols } else { new_cols };
        let this_line = Hang {
            shift: shown_cols as isize - old_cols as isize,
            floor: shown_cols,
        };
        if !sig.is_empty() && !leads_with_op {
            stmt = this_line;
        }

        // Render the line.
        if verbatim {
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
                let mut line = " ".repeat(new_cols);
                line.extend(chars[ws_end..content_end].iter());
                out_lines.push(line);
            }
        }

        // Update the stack with this line's tokens (done for every line — a
        // verbatim line can still contain tokens that open or close blocks).
        // Constructs opened on this line indent their contents one step past
        // the column this line is shown at, however many of them open here.
        let content = shown_cols + INDENT;
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
                        stack.push(Open::block(content)); // `when … do` arm body
                    }
                }
                Token::For | Token::While => {
                    pending_do = true;
                    stack.push(Open::block(content));
                }
                Token::If => {
                    if !when_header {
                        let mid_line = sig.first() != Some(&k);
                        stack.push(Open {
                            content,
                            hang: mid_line.then_some(this_line),
                        });
                    }
                }
                Token::Match | Token::Enum => stack.push(Open::block(content)),
                // `class Name … end` — contextual, so it arrives as an
                // identifier; only the declaration form (a name follows) opens
                // a block, never a variable or a JSX `class=` attribute.
                Token::Ident(w) if w == crate::parse::CLASS_KEYWORD => {
                    if matches!(tokens.get(k + 1), Some(Token::Ident(_))) {
                        stack.push(Open::block(content));
                    }
                }
                Token::Fn => {
                    if fn_takes_end(tokens, k) {
                        stack.push(Open::block(content));
                    }
                }
                Token::LParen | Token::LBracket | Token::LBrace => {
                    let hanging =
                        !matches!(tokens.get(k + 1), Some(Token::Newline | Token::Eof) | None)
                            && spans[k + 1].start.line == spans[k].start.line;
                    stack.push(Open {
                        content,
                        hang: hanging.then_some(this_line),
                    });
                }
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
                        stack.push(Open::block(content)); // children until `</tag>`
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
/// declaration (`fn name(…) … end`) or a block-bodied lambda, parenthesized
/// (`fn(…) … end`) or argless (`fn … end`). Only an arrow lambda
/// (`fn(…) -> expr` or `fn -> expr`) doesn't consume an `end`.
fn fn_takes_end(tokens: &[Token], k: usize) -> bool {
    let mut i = k + 1;
    while i < tokens.len() && matches!(tokens[i], Token::Newline) {
        i += 1;
    }
    match tokens.get(i) {
        None => false,
        Some(Token::Arrow) => false, // argless arrow lambda `fn -> expr`
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
        // A declaration (`fn name(…)`) or an argless lambda (`fn 42 end`).
        _ => true,
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
        let src = "let xs = [\n1,\n2\n]\nlet r = {\na: 1,\nb: 2\n}\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let xs = [\n  1,\n  2\n]\nlet r = {\n  a: 1,\n  b: 2\n}\n"
        );
    }

    #[test]
    fn hanging_continuations_keep_their_alignment() {
        let src = "  synth({wave: \"sine\", freq: 1.0,\n         decay: 0.1})\nlet a = 1\n";
        assert_eq!(
            reindent(src).unwrap(),
            "synth({wave: \"sine\", freq: 1.0,\n       decay: 0.1})\nlet a = 1\n"
        );
        // Aligned under the first element of a bracket that opens a line.
        let first = "let xs = [\n  {a: 1,\n   b: 2},\n]\n";
        assert_eq!(reindent(first).unwrap(), first);
        // A mid-line `if` whose arms are lined up under it.
        let arms = "let y = if a then 1\n        elsif b then 2\n        else 3 end\n";
        assert_eq!(reindent(arms).unwrap(), arms);
        // A leading-operator continuation lined up under the right-hand side.
        let cont = "  let s = a * b\n          * c\n";
        assert_eq!(reindent(cont).unwrap(), "let s = a * b\n        * c\n");
        // Shallower than the block indent: the block indent wins.
        let shallow = "f(a,\nb)\n";
        assert_eq!(reindent(shallow).unwrap(), "f(a,\n  b)\n");
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
    fn argless_block_lambda_opens_and_end_closes() {
        let src = "let f = fn\nprint(\"x\")\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(out, "let f = fn\n  print(\"x\")\nend\n");
        // Argless arrow lambdas still open nothing.
        let arrow = "let g = fn -> 1\nlet z = 2\n";
        assert_eq!(reindent(arrow).unwrap(), arrow);
    }

    #[test]
    fn class_fields_indent() {
        let src = "class Point\nx: int,\ny: int,\nend\n";
        assert_eq!(
            reindent(src).unwrap(),
            "class Point\n  x: int,\n  y: int,\nend\n"
        );
    }

    /// `class` is contextual, so an ordinary use of the word must not open a
    /// block — that would indent the rest of the file.
    #[test]
    fn a_bare_class_identifier_does_not_open_a_block() {
        let src = "let class = 5\nprint(class)\n";
        assert_eq!(reindent(src).unwrap(), src);
        let jsx = "let e = <div class=\"card\">hi</div>\nprint(e)\n";
        assert_eq!(reindent(jsx).unwrap(), jsx);
    }

    #[test]
    fn enum_variants_indent() {
        let src = "enum Event\nNone(),\nAdd(text),\nend\n";
        assert_eq!(
            reindent(src).unwrap(),
            "enum Event\n  None(),\n  Add(text),\nend\n"
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
