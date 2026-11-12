//! Pass 2 — token-driven 2-space re-indentation.

use crate::lexer::{Lexer, Token};

use super::blocks::{Blocks, ordinals};

/// The open-construct stack: one entry per unclosed construct, holding the
/// column its *contents* get — the column the line that opened it is shown
/// at, plus one step. Keying content to the opening line (rather than raw
/// stack depth) makes several delimiters opened on one line (`column([`)
/// indent their contents once, and a line of closers (`])`) realign with the
/// line that opened its first closer's construct.
///
/// Constructs and their closers: `end` closes fn/enum/class/if/for/while/match,
/// block lambdas, and `when … do` arms; `)` `]` `}` close their delimiters;
/// a JSX open tag holds its attributes (written one per line, they indent a
/// step) until its `>` or `/>`, which realigns with the `<` as a closer does;
/// `</tag>` closes a JSX element's children. Closers just pop the innermost
/// entry — lint runs on parseable source, so they always correspond. The one
/// construct without a closer is the body of a `when … ->` arm, which the next
/// `when` or the match's own `end` ends ([`Open::arm_body`]).
///
/// A construct may also be closed by its indentation alone, with no `end`.
/// The tokens cannot show that, so the parse says where each such construct
/// ends ([`Blocks`]), and its entry is dropped once its last token has gone
/// by ([`Open::closes_after`]). The pass thereby keeps the block structure
/// the parse found and only normalises how far each level is indented.
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
    /// The body of a `when … ->` match arm written on the lines below the
    /// arrow. It has no closer of its own: the next `when`, or the match's
    /// `end`, ends it.
    arm_body: bool,
    /// For a construct no `end` closes: the ordinal of its last token
    /// ([`Blocks`]). The construct is over at the first token past it.
    closes_after: Option<usize>,
    /// The column the first line of the contents was written at, once one
    /// has been seen. Only a comment needs it: one written after a block
    /// that indentation closed belongs to whichever open level it was
    /// lined up with.
    written_at: Option<usize>,
}

/// A construct whose contents the author may have aligned by eye: a delimiter
/// with contents on its own line, or an `if` that starts mid-line
/// (`let y = if c then a` with `else if`/`else` lined up below).
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
            arm_body: false,
            closes_after: None,
            written_at: None,
        }
    }

    /// A keyword construct, which [`Blocks`] may know to have no `end`.
    fn construct(content: usize, closes_after: Option<usize>) -> Self {
        Open {
            closes_after,
            ..Open::block(content)
        }
    }
}

/// Drop the constructs that indentation closed before the token of ordinal
/// `at`. They nest, so everything opened inside the outermost of them goes
/// with it (the body of its last `when … ->` arm included).
fn close_by_layout(stack: &mut OpenStack, at: usize) {
    if let Some(p) = layout_closed_from(stack, at) {
        stack.truncate(p);
    }
}

/// The depth of the outermost open construct that indentation closed before
/// the token of ordinal `at`.
fn layout_closed_from(stack: &OpenStack, at: usize) -> Option<usize> {
    stack
        .iter()
        .position(|o| o.closes_after.is_some_and(|last| last < at))
}

/// The header of a match arm being read: `when pattern [if guard]`, up to
/// its `->` or `do`.
#[derive(Clone, Copy)]
struct WhenHeader {
    /// Open constructs at the `when`; the arm's own `if`, `->` and `do` sit
    /// back at this depth.
    depth: usize,
    /// The guard's `if` has been read.
    guarded: bool,
}

/// Whether the `->` at token `k` is a lambda's (`fn -> x`, `fn(a, b) -> x`)
/// rather than a match arm's.
fn is_lambda_arrow(tokens: &[Token], k: usize) -> bool {
    match k.checked_sub(1).map(|p| &tokens[p]) {
        Some(Token::Fn) => true,
        Some(Token::RParen) => {
            let mut depth = 0usize;
            for p in (0..k).rev() {
                match tokens[p] {
                    Token::RParen => depth += 1,
                    Token::LParen => {
                        depth -= 1;
                        if depth == 0 {
                            return p > 0 && matches!(tokens[p - 1], Token::Fn);
                        }
                    }
                    _ => {}
                }
            }
            false
        }
        _ => false,
    }
}

/// Whether the keyword at token `k` is spelling a name rather than acting as
/// the keyword: `slice(xs, start: 1, end: 3)`, `{end: 3}`. A keyword directly
/// followed by `:` is always a label, and one directly after `.` or `?.` is a
/// field (`row.end`), so neither opens nor closes anything.
fn is_label(tokens: &[Token], k: usize) -> bool {
    let field = k > 0 && matches!(tokens[k - 1], Token::Dot | Token::QuestionDot);
    (field || matches!(tokens.get(k + 1), Some(Token::Colon)))
        && matches!(
            tokens[k],
            Token::End
                | Token::If
                | Token::For
                | Token::While
                | Token::Do
                | Token::Match
                | Token::Enum
                | Token::Fn
                | Token::When
        )
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
            | Bar
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

/// Does a line that ends with `t` run on into the line below (`a ||` /
/// `b`)? The operators that can only be binary there: not `<` / `>` (a tag
/// ends with one), `|` (an or-pattern) or `.`.
fn is_trailing_continuation_op(t: &Token) -> bool {
    use Token::*;
    matches!(
        t,
        Plus | Minus | Star | Slash | Percent | PlusPlus | And | Or | DoubleQuestion | Pipe | Eq | Ne | Le | Ge | Assign
    )
}

/// Re-indent `source` to 2-space indentation, trim trailing whitespace, and
/// end with exactly one newline. Only whitespace outside tokens is touched;
/// lines that start inside a multi-line token (raw string, JSX text) are
/// copied verbatim.
///
/// The source need only lex. When it also parses, the parse says which blocks
/// are closed by indentation alone; when it does not, every block is taken to
/// run to its `end`.
pub fn reindent(source: &str) -> Result<String, String> {
    let blocks = match crate::rewrite::parse_ast_or_by_end(source) {
        Ok((tree, _, _)) => Blocks::of(&tree),
        Err(_) => Blocks::all_closed_by_end(),
    };
    reindent_protected(source, &[], &blocks)
}

/// [`reindent`], copying each line marked in `protected` (0-based) verbatim —
/// the lines a `// petal-fmt-ignore` / `petal-fmt-off` comment fenced off.
/// Their tokens still open and close constructs, so the lines after them
/// indent as if the fence were not there. `blocks` is the block structure of
/// the file's parse; its ordinals name the same tokens in `source` as long as
/// `source` differs from the parsed text in whitespace only.
pub fn reindent_protected(
    source: &str,
    protected: &[bool],
    blocks: &Blocks,
) -> Result<String, String> {
    if source.is_empty() {
        return Ok(String::new());
    }
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let tokens = &lexer.tokens;
    let spans = &lexer.token_spans;
    let ord = ordinals(tokens);
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
    // The line above ended with a binary operator (or, in an `import` list,
    // a comma) outside any bracket it opened: this line continues it, and
    // hangs from the same statement as a leading-operator line does.
    let mut continues_above = false;
    // The statement the line above belongs to began with `import`.
    let mut in_import = false;
    // A `for`/`while` header's `do` belongs to the construct already opened at
    // the keyword; only a `do` with no pending header opens a block itself
    // (a `when … do` match arm).
    let mut pending_do = false;
    // Inside `when pattern [if guard]`: the stack depth at the `when`, and
    // whether the guard's `if` has been seen. That `if` is a guard, not an
    // opener, and the header ends at the arm's own `do`/`->` — the one back
    // at the `when`'s depth that is not a lambda's (a guard may hold
    // `xs.any(fn(v) -> v == x)`). Cleared there, or at a newline.
    let mut when_header: Option<WhenHeader> = None;
    // Stack of unterminated JSX opening tags, each recording the stack depth
    // with the tag's own construct pushed: the `>` that ends the tag is the
    // one seen back at that depth (a `>` inside an `attr={a > b}` brace sits
    // deeper).
    let mut open_tags: Vec<usize> = Vec::new();

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len());
    // Per line: the column a block opened on it is measured from. The column
    // the line is shown at, except for a line that begins inside a string.
    let mut anchors: Vec<usize> = Vec::with_capacity(lines.len());
    let mut ti = 0usize; // next token index

    for (line_idx, &(ls, le)) in lines.iter().enumerate() {
        // Tokens starting on this line (the Newline terminator included).
        let first_ti = ti;
        while ti < tokens.len() && (spans[ti].start.offset as usize) <= le {
            ti += 1;
        }
        let line_tokens = first_ti..ti;

        // A line beginning inside a token that started earlier (a multi-line
        // raw string) is content, not layout — copy it verbatim. JSX text is
        // the exception: the whitespace at its line breaks is dropped when it
        // is read, so its lines are laid out like code.
        let inside =
            |k: usize| (spans[k].start.offset as usize) < ls && (spans[k].end.offset as usize) > ls;
        let inside_token =
            (0..first_ti).rfind(|&k| inside(k) && !matches!(tokens[k], Token::JsxText(_)));
        let starts_inside = inside_token.is_some();

        // Leading-whitespace run.
        let mut ws_end = ls;
        while ws_end < le && chars[ws_end] != '\n' && chars[ws_end].is_whitespace() {
            ws_end += 1;
        }
        // Text that runs on past this line's indentation: the line starts with
        // text, not with whatever token follows it.
        let in_jsx_text = (0..first_ti).any(|k| {
            inside(k)
                && (spans[k].end.offset as usize) > ws_end
                && matches!(tokens[k], Token::JsxText(_))
        });
        // A token starting within the leading run means that "whitespace" is
        // token content (JSX text beginning at line start) — leave it alone.
        let token_in_leading_ws = line_tokens.clone().any(|k| {
            let s = spans[k].start.offset as usize;
            s >= ls && s < ws_end
        });

        // Trailing trim is safe only when no token spills past the line end
        // (the spill means the tail is inside a multi-line token).
        let spills_past_end = line_tokens.clone().any(|k| {
            (spans[k].end.offset as usize) > le + 1 && !matches!(tokens[k], Token::JsxText(_))
        });

        // Display indent, in columns. A line opening no new construct sits at
        // the innermost open construct's content column. A run of closers at
        // the start of the line realigns with the line that opened the
        // construct its first closer closes; `else`/`else if` realign with
        // their `if`.
        let sig: Vec<usize> = line_tokens
            .clone()
            .filter(|&k| !matches!(tokens[k], Token::Newline | Token::Eof))
            .collect();
        // The constructs indentation closed before this line's first token
        // are over. A line with no token of its own (a comment, a blank
        // line) is placed among the levels still open before the next code:
        // the deepest one it was written at least as deep as.
        let next_code = (first_ti..tokens.len()).find(|&k| !matches!(tokens[k], Token::Newline));
        let next_ord = next_code.map_or(usize::MAX, |k| ord[k]);
        let old_cols = ws_end - ls;
        let mut depth = stack.len();
        if sig.is_empty() {
            let keep = layout_closed_from(&stack, next_ord).unwrap_or(depth);
            while depth > keep && stack[depth - 1].written_at.is_none_or(|c| c > old_cols) {
                depth -= 1;
            }
        } else {
            close_by_layout(&mut stack, next_ord);
            depth = stack.len();
        }
        // A line opening with `when` or `end` ends the arm body above it, so
        // it is laid out as a line of the match, not of that body.
        if let Some(&k) = sig.first()
            && matches!(tokens[k], Token::When | Token::End)
            && !is_label(tokens, k)
            && stack.last().is_some_and(|o| o.arm_body)
        {
            stack.pop();
            depth = stack.len();
        }
        let mut dedent = 0usize;
        let mut si = 0usize;
        // Text runs first on a line that starts inside JSX text, so a `</tag>`
        // later on it is not a leading closer.
        while si < sig.len() && !in_jsx_text {
            match tokens[sig[si]] {
                Token::End if is_label(tokens, sig[si]) => break,
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
                // The `>` or `/>` ending an open tag whose attributes ran
                // over the lines above.
                Token::JsxSelfClose => {
                    dedent += 1;
                    si += 1;
                }
                Token::Gt if si == 0 && open_tags.last() == Some(&stack.len()) => {
                    dedent += 1;
                    si += 1;
                }
                _ => break,
            }
        }
        let leads_with_else = matches!(
            sig.first().map(|&k| &tokens[k]),
            Some(Token::Else | Token::Elsif | Token::ElseIf)
        );
        // The construct this line's column is judged against, and the column
        // the block rule gives it.
        let (governing, block_cols) = if dedent > 0 || leads_with_else {
            let open = stack.last();
            (open, open.map_or(0, |o| o.content.saturating_sub(INDENT)))
        } else {
            let open = stack[..depth].last();
            (open, open.map_or(0, |o| o.content))
        };
        let governing = governing.copied();
        if !sig.is_empty()
            && dedent == 0
            && !leads_with_else
            && !in_jsx_text
            && let Some(open) = stack.last_mut()
            && open.written_at.is_none()
        {
            open.written_at = Some(old_cols);
        }
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
            _ if continues_above && dedent == 0 && !leads_with_else && !in_jsx_text => {
                keep(stmt.shift, stmt.floor.saturating_sub(1))
            }
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
        if !sig.is_empty() && !leads_with_op && !continues_above {
            stmt = this_line;
            in_import = matches!(tokens[sig[0]], Token::Import)
                || (matches!(tokens[sig[0]], Token::Pub | Token::Export)
                    && sig.get(1).is_some_and(|&k| matches!(tokens[k], Token::Import)));
        }
        if let Some(&last) = sig.last() {
            // (An `import m: *` of the old module system ends with a `*`.)
            continues_above = sig.len() > 1
                && if in_import {
                    matches!(tokens[last], Token::Comma)
                } else {
                    is_trailing_continuation_op(&tokens[last])
                };
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
        // A line that begins inside a string is not a line start for layout:
        // a block opened on it is measured from the line the string began
        // on (`emit("…` / `…", fn(x)` / body), so its body goes a step in
        // from that line, wherever the string's last line happens to start.
        let anchor_cols = inside_token.map_or(shown_cols, |k| {
            anchors[spans[k].start.line as usize - 1]
        });
        anchors.push(anchor_cols);
        let block_content = anchor_cols + INDENT;
        for k in line_tokens {
            if !matches!(tokens[k], Token::Newline) {
                close_by_layout(&mut stack, ord[k]);
            }
            let construct = || Open::construct(block_content, blocks.closed_by_layout(ord[k]));
            match &tokens[k] {
                _ if is_label(tokens, k) => {}
                // An or-pattern may run over several lines, each ending or
                // beginning with `|`; any other line break ends the header.
                Token::Newline => {
                    let next = tokens[k + 1..].iter().find(|t| !matches!(t, Token::Newline));
                    let in_pattern = matches!(next, Some(Token::Bar))
                        || (k > 0 && matches!(tokens[k - 1], Token::Bar));
                    // (A break inside a guard's open bracket ends nothing.)
                    let inside = when_header.is_some_and(|h| h.depth < stack.len());
                    if !in_pattern && !inside {
                        when_header = None;
                    }
                }
                Token::When => {
                    if stack.last().is_some_and(|o| o.arm_body) {
                        stack.pop(); // the arm before this one, on the same line
                    }
                    when_header = Some(WhenHeader {
                        depth: stack.len(),
                        guarded: false,
                    });
                }
                Token::Arrow => {
                    let arms = when_header.is_some_and(|h| h.depth == stack.len())
                        && !is_lambda_arrow(tokens, k);
                    if arms {
                        // `when pattern ->` with the body on the lines below.
                        // In a `{…}` hole among an element's children the
                        // lexer drops the line breaks, so the next token
                        // sitting on a later line says the same thing.
                        let breaks = match tokens.get(k + 1) {
                            Some(Token::Newline) => true,
                            Some(Token::Eof) | None => false,
                            Some(_) => spans[k + 1].start.line > spans[k].end.line,
                        };
                        if breaks {
                            stack.push(Open {
                                arm_body: true,
                                ..Open::block(block_content)
                            });
                        }
                        when_header = None;
                    }
                }
                Token::Do => {
                    if when_header.is_some_and(|h| h.depth == stack.len()) {
                        when_header = None;
                    }
                    if pending_do {
                        pending_do = false;
                    } else {
                        // The body of a `when … do` arm or of a `test`.
                        stack.push(construct());
                    }
                }
                Token::For | Token::While => {
                    pending_do = true;
                    stack.push(construct());
                }
                Token::If => {
                    let guard = when_header
                        .as_mut()
                        .filter(|h| h.depth == stack.len() && !h.guarded);
                    if let Some(h) = guard {
                        h.guarded = true;
                    } else {
                        let mid_line = sig.first() != Some(&k);
                        stack.push(Open {
                            hang: mid_line.then_some(this_line),
                            ..construct()
                        });
                    }
                }
                Token::Match | Token::Enum => stack.push(construct()),
                // `class Name … end` — contextual, so it arrives as an
                // identifier; only the declaration form (a name follows) opens
                // a block, never a variable or a JSX `class=` attribute.
                Token::Ident(w) if w == crate::parse::CLASS_KEYWORD => {
                    if matches!(tokens.get(k + 1), Some(Token::Ident(_))) {
                        stack.push(construct());
                    }
                }
                Token::Fn => {
                    // `native fn name(…)` is a bodiless declaration.
                    let native = k > 0
                        && matches!(&tokens[k - 1], Token::Ident(w) if w == crate::parse::NATIVE_KEYWORD);
                    if !native && fn_takes_end(tokens, k) {
                        stack.push(construct());
                    }
                }
                Token::LParen | Token::LBracket | Token::LBrace => {
                    let hanging =
                        !matches!(tokens.get(k + 1), Some(Token::Newline | Token::Eof) | None)
                            && spans[k + 1].start.line == spans[k].start.line;
                    stack.push(Open {
                        hang: hanging.then_some(this_line),
                        ..Open::block(content)
                    });
                }
                Token::End => {
                    if stack.last().is_some_and(|o| o.arm_body) {
                        stack.pop(); // the last arm's body ends with its match
                    }
                    stack.pop();
                }
                Token::RParen | Token::RBracket | Token::RBrace => {
                    stack.pop();
                }
                Token::JsxOpenStart => {
                    stack.push(Open::block(content)); // attributes until `>`
                    open_tags.push(stack.len());
                }
                Token::JsxSelfClose => {
                    open_tags.pop();
                    stack.pop();
                }
                Token::Gt => {
                    if open_tags.last() == Some(&stack.len()) {
                        open_tags.pop();
                        // Children until `</tag>`, a step in from the `<`'s
                        // line wherever the `>` ended up.
                        if let Some(tag) = stack.pop() {
                            stack.push(Open::block(tag.content));
                        }
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

/// Whether the `fn` at token index `k` opens a block: a declaration
/// (`fn name(…) … end`) or a block-bodied lambda, parenthesized
/// (`fn(…) … end`) or argless (`fn … end`). Only an arrow lambda
/// (`fn(…) -> expr` or `fn -> expr`) opens none. The block runs to its `end`,
/// or to where [`Blocks`] says indentation closed it.
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
    fn a_match_arm_body_sits_under_its_arm() {
        let src = "match k\nwhen 1 ->\nlet a = 1\nif a > 0 then\nprint(a)\nend\nwhen 2\n| 3 ->\nmatch a\nwhen 0 -> x\nwhen _ ->\ny\nend\nwhen _ -> nil\nend\nafter()\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "match k\n  when 1 ->\n    let a = 1\n    if a > 0 then\n      print(a)\n    end\n  when 2\n  | 3 ->\n    match a\n      when 0 -> x\n      when _ ->\n        y\n    end\n  when _ -> nil\nend\nafter()\n"
        );
        // Already laid out: nothing moves.
        assert_eq!(reindent(&out).unwrap(), out);
    }

    #[test]
    fn an_arrow_lambda_in_a_guard_does_not_end_the_arm_header() {
        let src = "match op\nwhen Op.fight(p, _) if xs.any(fn(v) -> v == p) ->\nprint(p)\nhit()\nwhen x if fn -> x ->\nmiss()\nwhen _ if (if a then b else c end) ->\nnil\nend\nafter()\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "match op\n  when Op.fight(p, _) if xs.any(fn(v) -> v == p) ->\n    print(p)\n    hit()\n  when x if fn -> x ->\n    miss()\n  when _ if (if a then b else c end) ->\n    nil\nend\nafter()\n"
        );
        assert_eq!(reindent(&out).unwrap(), out);
    }

    #[test]
    fn a_one_line_arm_and_a_do_arm_open_nothing_past_their_line() {
        let src = "match k\n  when 1 -> a\n  when 2 do\n    b\n  end\n  when _ -> fn(x) -> x\nend\nafter()\n";
        assert_eq!(reindent(src).unwrap(), src);
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
        let src = "let r = match e\nwhen Event.Add(t) do\nitems = append(items, t)\ntrue\nend\nwhen Event.None() -> false\nend\n";
        let out = reindent(src).unwrap();
        assert_eq!(
            out,
            "let r = match e\n  when Event.Add(t) do\n    items = append(items, t)\n    true\n  end\n  when Event.None() -> false\nend\n"
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
    fn a_line_continued_by_a_trailing_operator_keeps_its_column() {
        // Hung from the statement, like a line that starts with an operator;
        // an `import` list continues after a comma. Moved with the
        // statement's line, and never left of it.
        let src = "import ui: a, b,\n           c, d\nfn f()\n    let busy = a || b ||\n        c ||\n        d\n    let n =\n  1\n    busy\nend\n";
        let want = "import ui: a, b,\n           c, d\nfn f()\n  let busy = a || b ||\n      c ||\n      d\n  let n =\n  1\n  busy\nend\n";
        assert_eq!(reindent(src).unwrap(), want);
        assert_eq!(reindent(want).unwrap(), want);
        // An enum's variants and a class's fields end with a comma and are
        // not continuations; nor is the line after `import m: *`.
        let items = "enum E\n  a,\n      b,\nend\n";
        assert_eq!(reindent(items).unwrap(), "enum E\n  a,\n  b,\nend\n");
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
    fn jsx_attributes_one_per_line_indent_under_the_tag() {
        // The attributes a step in; the `>` or `/>` back at the tag's line,
        // and the children a step in from it.
        let src = "let items = [\n<text\nstart={0.5}\nbox_lines\n>\nHi<br />\n</text>,\n<video\nsrc=\"a.mov\"\n/>,\n]\n";
        let want = "let items = [\n  <text\n    start={0.5}\n    box_lines\n  >\n    Hi<br />\n  </text>,\n  <video\n    src=\"a.mov\"\n  />,\n]\n";
        assert_eq!(reindent(src).unwrap(), want);
        assert_eq!(reindent(want).unwrap(), want);
        // A `>` ending the last attribute's line: children still one step in
        // from the `<`.
        // Text over several lines is laid out like code: its whitespace at
        // line breaks is not part of its value.
        let src = "let t = <p>\n      One\n        two\n      </p>\n";
        assert_eq!(reindent(src).unwrap(), "let t = <p>\n  One\n  two\n</p>\n");
        let src = "let t = <text\n    y={1}>\n    Hi\n</text>\n";
        assert_eq!(
            reindent(src).unwrap(),
            "let t = <text\n  y={1}>\n  Hi\n</text>\n"
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
