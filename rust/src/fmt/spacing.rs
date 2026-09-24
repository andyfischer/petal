//! Pass 1 — canonical spacing *within* a line.
//!
//! The lexer's spans tile the source (`crate::trivia`), so a file is a run of
//! tokens with a gap of whitespace or a `//` comment between each pair. This
//! pass rewrites only the gaps that sit wholly inside one line and hold
//! nothing but spaces and tabs; the tokens themselves, line breaks, comments
//! and the leading indentation (the re-indenter's job) are copied through
//! untouched. Every gap becomes one of:
//!
//! - `""` — no space: inside brackets, before `,` `:` `;`, around `.` `?.`
//!   `..`, after a unary `-` `!` `@` or a spread `...`, and between a callee or
//!   an indexed value and its `(` / `[`;
//! - `" "` — one space: around binary and assignment operators and `->`,
//!   after `,` and `:`, between a keyword and a `(`;
//! - otherwise the gap *collapses*: a run of blanks becomes one space, and a
//!   gap that was empty stays empty.
//!
//! Two regions are left exactly as written, because their whitespace is
//! content rather than layout: JSX tags and children (attribute spacing, text
//! runs), and lines the author fenced off with `// petal-fmt-ignore` or
//! `// petal-fmt-off` … `// petal-fmt-on`.
//!
//! Nothing here can change what the file means — [`super::format_source`]
//! re-lexes the result and refuses it unless the token stream is identical —
//! but a rule that picked the wrong gap could still produce a stream that
//! *doesn't* match (`a < b` → `a <b` lexes as a JSX tag), which is why the
//! only gaps that ever lose all their space are ones between tokens that
//! cannot fuse.

use crate::lexer::{Lexer, Token};

/// What a JSX-aware scan sees at a given gap.
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    /// Ordinary code, including a `{…}` hole inside a JSX element.
    Code,
    /// Between `<tag` and its `>` / `/>`: attribute spacing is left alone.
    JsxTag,
    /// Between a tag's `>` and its `</tag>`: children are text.
    JsxContent,
}

/// Re-space every gap of `source` that the rules cover. `protected[line]`
/// (0-based) marks a line whose gaps must be kept verbatim.
pub fn respace(source: &str, protected: &[bool]) -> Result<String, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let tokens = &lexer.tokens;
    let spans = &lexer.token_spans;
    let chars: Vec<char> = source.chars().collect();

    let text = |k: usize| -> &[char] {
        &chars[spans[k].start.offset as usize..spans[k].end.offset as usize]
    };
    // A string piece that ends in `{` opens an interpolation hole; one that
    // starts with `}` closes it. A whole string starts with a quote.
    let opens_hole =
        |k: usize| matches!(tokens[k], Token::String(_)) && text(k).last() == Some(&'{');
    let closes_hole =
        |k: usize| matches!(tokens[k], Token::String(_)) && text(k).first() == Some(&'}');

    let columns = Columns::new(source, tokens, spans);
    // Per (0-based line, width): how many gaps of that many blanks follow a
    // comma. A width that repeats on one line groups the arguments
    // (`quad(x0, y0, z1,  x1, y0, z1,  …)`) and is kept.
    let mut grouping: std::collections::HashMap<(usize, usize), usize> =
        std::collections::HashMap::new();
    for k in 0..tokens.len().saturating_sub(1) {
        if !matches!(tokens[k], Token::Comma) {
            continue;
        }
        let (a, b) = (
            spans[k].end.offset as usize,
            spans[k + 1].start.offset as usize,
        );
        let width = b - a;
        if width > 1 && chars[a..b].iter().all(|&c| c == ' ') {
            *grouping
                .entry((spans[k].end.line as usize - 1, width))
                .or_default() += 1;
        }
    }
    let mut out = String::with_capacity(source.len());
    let mut ctx: Vec<Ctx> = vec![Ctx::Code];
    // Inside an `import …` line, `/` joins path segments.
    let mut in_import = false;
    // Per open `(`: is it a `fn` parameter list? After its `)` comes the
    // body, not a call or an index: `fn rx(t, a) [t, a] end`.
    let mut parens: Vec<bool> = Vec::new();

    // Whatever precedes the first token (a leading comment) is copied as is.
    if let Some(first) = spans.first() {
        out.extend(&chars[..first.start.offset as usize]);
    }
    for i in 0..tokens.len() {
        let (ts, te) = (spans[i].start.offset as usize, spans[i].end.offset as usize);
        out.extend(&chars[ts..te]);

        // Update the context with token `i`: the gap after it is read in the
        // state it leaves behind.
        let mut closes_params = false;
        match &tokens[i] {
            Token::LParen => {
                ctx.push(Ctx::Code);
                let prev = |n: usize| i.checked_sub(n).map(|p| &tokens[p]);
                parens.push(
                    matches!(prev(1), Some(Token::Fn))
                        || (matches!(prev(1), Some(Token::Ident(_)))
                            && matches!(prev(2), Some(Token::Fn))),
                );
            }
            Token::RParen => {
                if ctx.len() > 1 && ctx.last() == Some(&Ctx::Code) {
                    ctx.pop();
                }
                closes_params = parens.pop().unwrap_or(false);
            }
            Token::LBracket | Token::LBrace => ctx.push(Ctx::Code),
            Token::RBracket | Token::RBrace => {
                if ctx.len() > 1 && ctx.last() == Some(&Ctx::Code) {
                    ctx.pop();
                }
            }
            Token::JsxOpenStart => ctx.push(Ctx::JsxTag),
            Token::JsxSelfClose => {
                if ctx.last() == Some(&Ctx::JsxTag) {
                    ctx.pop();
                }
            }
            Token::Gt if ctx.last() == Some(&Ctx::JsxTag) => {
                *ctx.last_mut().unwrap() = Ctx::JsxContent;
            }
            Token::JsxCloseStart => {
                if ctx.last() == Some(&Ctx::JsxContent) {
                    ctx.pop();
                }
            }
            Token::Import => in_import = true,
            Token::Newline => in_import = false,
            _ => {}
        }

        let Some(next) = tokens.get(i + 1) else {
            break;
        };
        let gap_start = te;
        let gap_end = spans[i + 1].start.offset as usize;
        let gap = &chars[gap_start..gap_end];

        let line = spans[i].end.line as usize - 1;
        let keep = protected.get(line).copied().unwrap_or(false)
            || !gap.iter().all(|&c| c == ' ' || c == '\t')
            || (matches!(tokens[i], Token::Newline) && !is_semicolon(&chars, ts))
            || (matches!(next, Token::Newline | Token::Eof)
                && !is_semicolon(&chars, spans[i + 1].start.offset as usize))
            || ctx.last() != Some(&Ctx::Code);
        if keep {
            // A trailing comment keeps the column it was written in, however
            // the code before it was respaced — but never touches the code.
            let slash = gap.iter().position(|&c| c == '/');
            if let Some(slash) = slash
                && !protected.get(line).copied().unwrap_or(false)
                && !matches!(tokens[i], Token::Newline)
                && ctx.last() == Some(&Ctx::Code)
                && gap[..slash].iter().all(|&c| c == ' ' || c == '\t')
            {
                let original_col = spans[i].end.column as usize - 1 + slash;
                let current_col = out.chars().rev().take_while(|&c| c != '\n').count();
                let pad = original_col.saturating_sub(current_col).max(1);
                out.extend(std::iter::repeat_n(' ', pad));
                out.extend(&gap[slash..]);
                continue;
            }
            out.extend(gap);
            continue;
        }

        let rule = gap_rule(GapCtx {
            prev: i.checked_sub(1).map(|p| &tokens[p]),
            a: &tokens[i],
            b: next,
            a_opens_hole: opens_hole(i),
            a_closes_params: closes_params,
            prev_opens_hole: i > 0 && opens_hole(i - 1),
            b_closes_hole: closes_hole(i + 1),
            b_is_semicolon: is_semicolon(&chars, spans[i + 1].start.offset as usize),
            in_import,
        });
        // A run of blanks that lines a token up with one on a neighbouring
        // line is alignment, and is kept (the `None` rules still apply).
        // A sign belongs to its number: `-8.0` right-aligns by the `0`.
        let signed = matches!(next, Token::Minus | Token::Plus)
            && spans
                .get(i + 2)
                .is_some_and(|s| s.start == spans[i + 1].end);
        let end = if signed {
            spans[i + 2].end
        } else {
            spans[i + 1].end
        };
        let aligned = gap.len() > 1
            && (columns.aligned(&spans[i + 1], end)
                || (matches!(tokens[i], Token::Comma)
                    && grouping.get(&(line, gap.len())).is_some_and(|&n| n >= 2)));
        match rule {
            Gap::None => {}
            Gap::One | Gap::Collapse if aligned => out.extend(gap),
            Gap::One => out.push(' '),
            Gap::Collapse => {
                if !gap.is_empty() {
                    out.push(' ');
                }
            }
        }
    }
    Ok(out)
}

/// Per line, the columns where a token starts or ends — the grid alignment
/// is judged against.
struct Columns {
    starts: Vec<std::collections::HashSet<u32>>,
    ends: Vec<std::collections::HashSet<u32>>,
    /// 1-based: the line holds nothing but a `//` comment.
    comment_only: Vec<bool>,
}

impl Columns {
    fn new(source: &str, tokens: &[Token], spans: &[crate::source_map::SourceSpan]) -> Self {
        let lines = spans.last().map_or(1, |s| s.end.line as usize + 1);
        let mut starts = vec![std::collections::HashSet::new(); lines + 1];
        let mut ends = starts.clone();
        for (t, s) in tokens.iter().zip(spans) {
            if matches!(t, Token::Newline | Token::Eof) || s.start.line != s.end.line {
                continue;
            }
            starts[s.start.line as usize].insert(s.start.column);
            ends[s.start.line as usize].insert(s.end.column);
        }
        let mut comment_only = vec![false];
        comment_only.extend(source.split('\n').map(|l| l.trim_start().starts_with("//")));
        comment_only.resize(starts.len().max(comment_only.len()), false);
        Columns {
            starts,
            ends,
            comment_only,
        }
    }

    /// Does `span` (a one-line token) start where a token on the line above or
    /// below starts, or end where one ends? Left-aligned names and
    /// right-aligned numbers both count. Comment-only lines in between are
    /// skipped (a table annotated row by row); a blank line ends the table.
    fn aligned(
        &self,
        span: &crate::source_map::SourceSpan,
        end: crate::source_map::SourcePosition,
    ) -> bool {
        if span.start.line != end.line {
            return false;
        }
        let line = span.start.line as usize;
        let neighbour = |step: isize| {
            let mut l = line as isize + step;
            while l > 0 && self.comment_only.get(l as usize) == Some(&true) {
                l += step;
            }
            l.max(0) as usize
        };
        [neighbour(-1), neighbour(1)].iter().any(|&l| {
            self.starts
                .get(l)
                .is_some_and(|c| c.contains(&span.start.column))
                || self.ends.get(l).is_some_and(|c| c.contains(&end.column))
        })
    }
}

fn is_semicolon(chars: &[char], at: usize) -> bool {
    chars.get(at) == Some(&';')
}

enum Gap {
    None,
    One,
    Collapse,
}

struct GapCtx<'a> {
    /// The token before `a`, to tell a unary `-` from a binary one.
    prev: Option<&'a Token>,
    a: &'a Token,
    b: &'a Token,
    a_opens_hole: bool,
    /// `a` is the `)` of a `fn` parameter list.
    a_closes_params: bool,
    prev_opens_hole: bool,
    b_closes_hole: bool,
    b_is_semicolon: bool,
    in_import: bool,
}

/// The spacing rule for the gap between `a` and `b`, in priority order.
fn gap_rule(g: GapCtx) -> Gap {
    use Token::*;
    let (a, b) = (g.a, g.b);

    if g.b_is_semicolon {
        return Gap::None;
    }
    if matches!(a, Newline) {
        return Gap::One; // after a `;`
    }
    if matches!(a, LParen | LBracket | LBrace) || g.a_opens_hole {
        return Gap::None;
    }
    if matches!(b, RParen | RBracket | RBrace) || g.b_closes_hole {
        return Gap::None;
    }
    if matches!(b, Comma | Colon) {
        return Gap::None;
    }
    if matches!(a, Comma | Colon) {
        return Gap::One;
    }
    if matches!(a, Dot | QuestionDot | DotDot | DotDotDot | At)
        || matches!(b, Dot | QuestionDot | DotDot)
    {
        return Gap::None;
    }
    if g.in_import && (matches!(a, Slash) || matches!(b, Slash)) {
        return Gap::None;
    }
    // A unary operator binds to its operand. A `+`/`-` that opens a line is
    // a continuation of the line above, not a sign, and keeps its spacing.
    if matches!(a, Minus | Plus) && matches!(g.prev, Some(Newline)) {
        return Gap::Collapse;
    }
    let prev_ends_operand = ends_operand(g.prev) && !g.prev_opens_hole;
    if matches!(a, Bang) || (matches!(a, Minus | Plus) && !prev_ends_operand) {
        return Gap::None;
    }
    let callee = ends_operand(Some(a)) && !g.a_closes_params;
    match b {
        // A call or an index: `f(x)`, `xs[0]`, `fn(x) -> …`.
        LParen if callee || matches!(a, Fn) => return Gap::None,
        LBracket if callee => return Gap::None,
        _ => {}
    }
    let a_ends_operand = ends_operand(Some(a)) && !g.a_opens_hole && !g.a_closes_params;
    if is_binary_op(a, prev_ends_operand) || is_binary_op(b, a_ends_operand) {
        return Gap::One;
    }
    Gap::Collapse
}

/// Does `t` end an operand, so that a following `-` subtracts, a `(` calls
/// and a `[` indexes?
fn ends_operand(t: Option<&Token>) -> bool {
    use Token::*;
    matches!(
        t,
        Some(
            Int(_)
                | Float(_)
                | String(_)
                | True
                | False
                | Nil
                | Ident(_)
                | Color(_)
                | RParen
                | RBracket
                | RBrace
                | InterpEnd
                | End
                | JsxTagName(_)
                | JsxSelfClose
        )
    )
}

/// Is `t` a binary (or assignment) operator here? `after_operand` says
/// whether the token before it ends an operand, which decides whether a `-`
/// subtracts or negates.
fn is_binary_op(t: &Token, after_operand: bool) -> bool {
    use Token::*;
    match t {
        Minus | Plus => after_operand,
        Star | Slash | Percent | PlusPlus | Eq | Ne | Lt | Le | Gt | Ge | And | Or
        | DoubleQuestion | Assign | PlusAssign | MinusAssign | StarAssign | SlashAssign
        | PercentAssign | ConcatAssign | CoalesceAssign | Pipe | Arrow => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(src: &str) -> String {
        respace(src, &[]).unwrap()
    }

    #[test]
    fn binary_operators_get_one_space() {
        assert_eq!(sp("let x=a+b*c\n"), "let x = a + b * c\n");
        assert_eq!(sp("x  +=   1\n"), "x += 1\n");
        assert_eq!(sp("a&&b||c??d\n"), "a && b || c ?? d\n");
        assert_eq!(sp("s = \"a\"++\"b\"\n"), "s = \"a\" ++ \"b\"\n");
    }

    #[test]
    fn unary_minus_and_bang_bind_tight() {
        assert_eq!(sp("let x = - 1\n"), "let x = -1\n");
        assert_eq!(sp("f(- x, a -b)\n"), "f(-x, a - b)\n");
        assert_eq!(sp("return - x\n"), "return -x\n");
        assert_eq!(sp("if ! done then 1 end\n"), "if !done then 1 end\n");
        assert_eq!(sp("let r = [1, -2]\n"), "let r = [1, -2]\n");
        // A leading `+` continues the line above.
        assert_eq!(sp("let v = a\n  + b\n"), "let v = a\n  + b\n");
    }

    #[test]
    fn brackets_commas_and_colons() {
        assert_eq!(sp("f( a ,b )\n"), "f(a, b)\n");
        assert_eq!(sp("let m = { a : 1,b:2 }\n"), "let m = {a: 1, b: 2}\n");
        assert_eq!(sp("xs [ 0 ]\n"), "xs[0]\n");
        assert_eq!(sp("f (x)\n"), "f(x)\n");
    }

    #[test]
    fn keywords_before_parens_keep_their_spacing() {
        assert_eq!(sp("if (a) then 1 end\n"), "if (a) then 1 end\n");
        assert_eq!(sp("state(id) x = 0\n"), "state(id) x = 0\n");
        assert_eq!(
            sp("let ys = map(xs, fn (x) -> x)\n"),
            "let ys = map(xs, fn(x) -> x)\n"
        );
    }

    #[test]
    fn alignment_with_a_neighbouring_line_is_kept() {
        let table = "let a = [\n  {x:   6.0, name: \"a\"},\n  {x: -12.0, name: \"bb\"},\n]\n";
        assert_eq!(sp(table), table);
        let signed = "let a = [\n  {x:  -8.0},\n  {x:   8.0},\n]\n";
        assert_eq!(sp(signed), signed);
        let eqs = "let left  = 1\nlet right = 2\n";
        assert_eq!(sp(eqs), eqs);
        let groups = "quad(x0, y0, z1,  x1, y0, z1,  r, g, b)\n";
        assert_eq!(sp(groups), groups);
        assert_eq!(sp("f(a,  b, c)\n"), "f(a, b, c)\n");
        // A double space that lines up with nothing is just a double space.
        assert_eq!(sp("let a  = 1\nlet b = 2\n"), "let a = 1\nlet b = 2\n");
    }

    #[test]
    fn a_fn_body_after_its_params_is_not_an_index() {
        assert_eq!(sp("fn rx(t, a) [t, a] end\n"), "fn rx(t, a) [t, a] end\n");
        assert_eq!(sp("fn f() (1)\nend\n"), "fn f() (1)\nend\n");
        assert_eq!(sp("fn f(xs) xs [0] end\n"), "fn f(xs) xs[0] end\n");
    }

    #[test]
    fn member_access_and_ranges_are_tight() {
        assert_eq!(sp("a . b ?. c\n"), "a.b?.c\n");
        assert_eq!(sp("for i in 0 .. n do\nend\n"), "for i in 0..n do\nend\n");
        assert_eq!(sp("let ys = [... xs]\n"), "let ys = [...xs]\n");
    }

    #[test]
    fn interpolation_holes_are_tight() {
        assert_eq!(sp("print(\"a{ x+1 }b\")\n"), "print(\"a{x + 1}b\")\n");
        assert_eq!(sp("print(\"{-d} late\")\n"), "print(\"{-d} late\")\n");
    }

    #[test]
    fn jsx_attributes_and_text_are_untouched() {
        let src = "let e = <div class=\"c\"  a={x>1}>hi   there</div>\n";
        assert_eq!(
            sp(src),
            "let e = <div class=\"c\"  a={x > 1}>hi   there</div>\n"
        );
    }

    #[test]
    fn import_paths_stay_joined() {
        assert_eq!(sp("import bloom/menu: a,b\n"), "import bloom/menu: a, b\n");
        assert_eq!(sp("import m: *\n"), "import m: *\n");
    }

    #[test]
    fn trailing_comment_gets_a_space() {
        assert_eq!(sp("let x = 1// hi\n"), "let x = 1 // hi\n");
        assert_eq!(sp("let x = 1      // hi\n"), "let x = 1      // hi\n");
        // The comment stays in its column as the code before it shrinks.
        assert_eq!(sp("f( a )    // hi\n"), "f(a)      // hi\n");
    }

    #[test]
    fn semicolons() {
        assert_eq!(sp("a = 1 ;b = 2\n"), "a = 1; b = 2\n");
    }

    #[test]
    fn protected_lines_are_verbatim() {
        let src = "let a=1\nlet b=2\n";
        assert_eq!(
            respace(src, &[false, true]).unwrap(),
            "let a = 1\nlet b=2\n"
        );
    }
}
