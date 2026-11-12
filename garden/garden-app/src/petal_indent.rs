//! Indentation help for Petal buffers.
//!
//! Petal's blocks are delimited by indentation (`docs/syntax/indentation.md`):
//! the line after a block opener starts the body, one step in, and the block
//! ends at the first line indented less. So the editor does three things a
//! plain copy-the-indent editor does not:
//!
//! - **Enter after an opener** (`then`, `do`, `else`, `->`, or a `fn` /
//!   `class` / `enum` / `match` header) indents the new line one step
//!   ([`opens_block`]).
//! - **Typing `else` or `when` at the body's column** pulls the line back to
//!   the `if` / the previous arm it continues ([`dedent_target`]).
//! - **A paste that starts in a line's indentation** is shifted as a block to
//!   the cursor's column, keeping the lines' offsets from each other
//!   ([`reindent_paste`], [`reindent_lines`]).
//!
//! Everything here reads lines, not a parse: the text under the cursor is
//! half typed most of the time, and the answer has to be the same whether or
//! not the rest of the file currently parses. Strings and comments are
//! skipped ([`code_of`]); a line that begins inside a multi-line string is not
//! recognised as such by the per-line functions.

/// One indentation step, in spaces: what `petal fmt` writes.
pub const STEP: usize = 2;

/// How far up [`dedent_target`] looks for the line to align with.
const MAX_WALK: usize = 2000;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The width of `line`'s indentation when it is all spaces; `None` when a
/// tab is in it (the language refuses those, and a column cannot be read off
/// one).
pub fn indent_of(line: &str) -> Option<usize> {
    let mut n = 0;
    for c in line.chars() {
        match c {
            ' ' => n += 1,
            '\t' => return None,
            _ => break,
        }
    }
    Some(n)
}

/// Whether `#` at `chars[i]` is the old colour spelling (`#f80`): 3, 4, 6 or
/// 8 hex digits and then a character that cannot continue a name. The lexer
/// reports that shape as an error, not a comment, so it is not one here.
fn old_colour_at(chars: &[char], i: usize) -> bool {
    let digits = chars[i + 1..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
    matches!(digits, 3 | 4 | 6 | 8) && !chars.get(i + 1 + digits).is_some_and(|c| is_ident(*c))
}

/// The code of one line: the comment (`# …`, the deprecated `// …`, and any
/// `/* … */`) removed, the inside of each string replaced by `s`s, trailing
/// whitespace dropped. Leading whitespace is kept.
pub fn code_of(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                out.push(c);
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    out.push('s');
                    i += 1;
                }
                if i < chars.len() {
                    out.push(c);
                    i += 1;
                }
            }
            '#' if !old_colour_at(&chars, i) => break,
            '/' if chars.get(i + 1) == Some(&'/') => break,
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
                out.push(' ');
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// The byte offsets where `word` stands as a whole word in `code`.
fn word_positions<'a>(code: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    code.match_indices(word).map(|(at, _)| at).filter(move |&at| {
        let before = code[..at].chars().next_back();
        let after = code[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

fn has_word(code: &str, word: &str) -> bool {
    word_positions(code, word).next().is_some()
}

/// Whether `code` ends with the keyword `word` (not a field of that name).
fn ends_with_word(code: &str, word: &str) -> bool {
    let Some(head) = code.strip_suffix(word) else {
        return false;
    };
    !head.chars().next_back().is_some_and(|c| is_ident(c) || c == '.')
}

/// The first word of `code`, after an optional `pub` / `export`.
fn first_word(code: &str) -> &str {
    let mut words = code.split(|c: char| !is_ident(c)).filter(|w| !w.is_empty());
    match words.next() {
        Some("pub" | "export") => words.next().unwrap_or(""),
        Some(word) => word,
        None => "",
    }
}

/// Whether the last `fn` of `code` is a header with nothing after it: `fn
/// f(…)`, `fn f(…) -> type`, or a lambda's `fn(…)`. A header whose parameter
/// list is still open, a one-line body (`fn box(w) "{w}"`) and a `native fn`
/// declaration are not.
fn ends_with_fn_header(code: &str) -> bool {
    let Some(at) = word_positions(code, "fn").last() else {
        return false;
    };
    if ends_with_word(code[..at].trim_end(), "native") {
        return false;
    }
    let after = code[at + 2..].trim_start();
    let name_len = after.find(|c: char| !(is_ident(c) || c == '.')).unwrap_or(after.len());
    let named = name_len > 0;
    let Some(params) = after[name_len..].trim_start().strip_prefix('(') else {
        return false;
    };
    let mut depth = 1usize;
    let mut close = None;
    for (i, c) in params.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return false;
    };
    let rest = params[close + 1..].trim();
    if rest.is_empty() {
        return true;
    }
    // A return type is one bare name. After a lambda, `-> x` is its body.
    named
        && rest.strip_prefix("->").is_some_and(|ty| {
            let ty = ty.trim();
            !ty.is_empty() && ty.chars().all(|c| is_ident(c) || c == '?')
        })
}

/// Whether the line that ends with `line` opens a block, so the next line is
/// its body: it ends with `then`, `do`, `else` or `->`, or it is a `fn` /
/// `class` / `enum` / `match` header.
pub fn opens_block(line: &str) -> bool {
    let code = code_of(line);
    let code = code.as_str();
    if code.trim().is_empty() {
        return false;
    }
    if code.ends_with("->") || ["then", "do", "else"].iter().any(|w| ends_with_word(code, w)) {
        return true;
    }
    if ends_with_word(code, "end") {
        return false;
    }
    if matches!(first_word(code), "class" | "enum") {
        return true;
    }
    // `match x` with its arms on the following lines, also as a value
    // (`let y = match x`).
    if let Some(at) = word_positions(code, "match").last() {
        let subject = &code[at + "match".len()..];
        if !subject.trim().is_empty() && !has_word(subject, "when") {
            return true;
        }
    }
    ends_with_fn_header(code)
}

/// Whether a line ends inside an open bracket. Used only to guess where a
/// pasted fragment's lines stood (see [`reindent_lines`]).
fn ends_with_open_bracket(line: &str) -> bool {
    code_of(line).ends_with(['(', '[', '{'])
}

/// The keyword whose typing can pull its line back: `else` (and so `else
/// if`) to its `if`, `when` to the arm before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dedent {
    Else,
    When,
}

impl Dedent {
    /// The keyword a line consists of, if it is one of the two: `else` or
    /// `when`, alone on its line (trailing spaces allowed).
    pub fn of_line(line: &str) -> Option<Dedent> {
        match line.trim() {
            "else" => Some(Dedent::Else),
            "when" => Some(Dedent::When),
            _ => None,
        }
    }
}

/// Where a line holding only `keyword`, at indent `current`, belongs.
/// `above` yields the lines before it, nearest first.
///
/// `else` goes to the column of the nearest enclosing line that ends with
/// `then` (an `if` or an `else if` with a block body); `when` goes to the
/// column of the nearest enclosing `when`. "Enclosing" is by indentation: a
/// line indented less than everything between it and the keyword.
///
/// `None` (leave the line alone) when the keyword was not typed at the body's
/// column: the line above is deeper, so the author has already placed it; or
/// the line above is a one-line `if … then …` at this column, which the
/// `else` continues where it stands; or nothing above takes it.
pub fn dedent_target(
    above: impl Iterator<Item = String>,
    current: usize,
    keyword: Dedent,
) -> Option<usize> {
    let mut limit = current;
    let mut first = true;
    for line in above.take(MAX_WALK) {
        let code = code_of(&line);
        if code.trim().is_empty() {
            continue;
        }
        let indent = indent_of(&line)?;
        if first {
            first = false;
            if indent > current {
                return None;
            }
            if indent == current && keyword == Dedent::Else && one_line_if(&code) {
                return None;
            }
        }
        if indent >= limit {
            continue;
        }
        limit = indent;
        match keyword {
            Dedent::Else if ends_with_word(&code, "then") => return Some(indent),
            Dedent::When if first_word(&code) == "when" && code.trim_start().starts_with("when") => {
                return Some(indent)
            }
            // The first arm of a `match` stays where the body starts.
            Dedent::When if has_word(&code, "match") && opens_block(&code) => return None,
            _ => {}
        }
        if limit == 0 {
            return None;
        }
    }
    None
}

/// A one-line `if c then x` (or `else if c then x`) still open for an `else`.
fn one_line_if(code: &str) -> bool {
    let Some(at) = word_positions(code, "then").last() else {
        return false;
    };
    !code[at + "then".len()..].trim().is_empty() && !ends_with_word(code, "end")
}

/// For each line of `text`, whether it begins inside a string (a `"…"` that
/// spans lines, or raw `"""…"""`) or a `/* … */` comment. Such a line is
/// content, not layout, and is never re-indented.
fn lines_starting_inside(text: &str) -> Vec<bool> {
    #[derive(PartialEq)]
    enum In {
        Code,
        Str,
        Raw,
        Block,
        Line,
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![false];
    let mut state = In::Code;
    let mut i = 0;
    let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c));
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            if state == In::Line {
                state = In::Code;
            }
            out.push(state != In::Code);
            i += 1;
            continue;
        }
        match state {
            In::Code => {
                if at(i, "\"\"\"") {
                    state = In::Raw;
                    i += 3;
                    continue;
                } else if c == '"' {
                    state = In::Str;
                } else if c == '\'' {
                    // A quoted string that stays on its line. An apostrophe in
                    // element text has no partner; do not let it eat the rest.
                    let mut j = i + 1;
                    while j < chars.len() && chars[j] != '\n' && chars[j] != '\'' {
                        if chars[j] == '\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    if chars.get(j) == Some(&'\'') {
                        i = j;
                    }
                } else if at(i, "/*") {
                    state = In::Block;
                    i += 1;
                } else if at(i, "//") || (c == '#' && !old_colour_at(&chars, i)) {
                    state = In::Line;
                }
            }
            In::Str => {
                if c == '\\' {
                    i += 1;
                } else if c == '"' {
                    state = In::Code;
                }
            }
            In::Raw => {
                if at(i, "\"\"\"") {
                    state = In::Code;
                    i += 2;
                }
            }
            In::Block => {
                if at(i, "*/") {
                    state = In::Code;
                    i += 1;
                }
            }
            In::Line => {}
        }
        i += 1;
    }
    out
}

/// The lines of `text` moved as a block so that its outermost line starts at
/// column `target`; each line keeps its offset from the others. Returns the
/// new lines in full (indentation included).
///
/// The first line may have been copied from its first character rather than
/// from the start of its line, so its own indentation is missing while the
/// following lines have theirs. Then the block's column is read off the
/// following lines: the shallowest of them, less one step when the first
/// line opens a block or a bracket and its body is the shallowest thing
/// copied.
///
/// `None` when nothing should be touched: a tab in some line's indentation,
/// or no line of code at all. Blank lines and lines that begin inside a
/// string come back as they were.
pub fn reindent_lines(text: &str, target: usize) -> Option<Vec<String>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let inside = lines_starting_inside(text);
    let is_code = |i: usize| !inside[i] && !lines[i].trim().is_empty();
    let mut indents = vec![0usize; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if is_code(i) {
            indents[i] = indent_of(line)?;
        }
    }
    let first = is_code(0).then(|| indents[0]);
    let rest = (1..lines.len()).filter(|&i| is_code(i)).map(|i| indents[i]).min();
    let base = match (first, rest) {
        (None, None) => return None,
        (None, Some(r)) => r,
        (Some(a), None) => a,
        (Some(0), Some(r)) if r > 0 => {
            // When a later line comes back out (an `else`, a closing bracket,
            // the next statement), that line shows the column itself.
            let second = (1..lines.len()).find(|&i| is_code(i)).map(|i| indents[i]);
            let opener = opens_block(lines[0]) || ends_with_open_bracket(lines[0]);
            if opener && second == Some(r) {
                r.saturating_sub(STEP)
            } else {
                r
            }
        }
        (Some(a), Some(r)) => a.min(r),
    };
    Some(
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                if !is_code(i) {
                    return line.to_string();
                }
                let indent = target + indents[i].saturating_sub(base);
                format!("{}{}", " ".repeat(indent), line.trim_start_matches(' '))
            })
            .collect(),
    )
}

/// `text` as it should be inserted at column `target` of a line that holds
/// only indentation up to there: [`reindent_lines`], with the first line's
/// share of the indentation left out because the line already has it. When
/// `code_follows` (the rest of the cursor's line is code) and the text ends
/// with a line break, that code is put back at `target`.
pub fn reindent_paste(text: &str, target: usize, code_follows: bool) -> Option<String> {
    let mut lines = reindent_lines(text, target)?;
    if let Some(first) = lines.first_mut() {
        if !first.trim().is_empty() {
            *first = first.get(target..).unwrap_or(first.trim_start()).to_string();
        }
    }
    if lines.len() > 1 && code_follows {
        if let Some(last) = lines.last_mut() {
            if last.is_empty() {
                *last = " ".repeat(target);
            }
        }
    }
    Some(lines.join("\n"))
}

/// The line-comment marker for a language Garden highlights, by the
/// [`Language::name`](crate::syntax::Language::name) of its registry entry.
/// `None` for a language with no line comment (JSON, Markdown, HTML, CSS).
pub fn line_comment(language: &str) -> Option<&'static str> {
    match language {
        "Petal" | "Python" | "Ruby" | "Bash" | "TOML" | "YAML" | "Nix" => Some("#"),
        "Rust" | "JavaScript" | "TypeScript" | "TSX" | "Go" | "C" | "C++" | "Java" | "C#" | "PHP"
        | "Zig" => Some("//"),
        "SQL" | "Lua" => Some("--"),
        _ => None,
    }
}

/// Toggle a line comment on `lines`, as one block: when every line with text
/// is already commented, the marker (and one space after it) comes off each;
/// otherwise the marker and a space go on each, at the block's shallowest
/// column so the code keeps its shape. Blank lines are left alone.
pub fn toggle_comment(lines: &[&str], marker: &str) -> Vec<String> {
    let texts = || lines.iter().filter(|l| !l.trim().is_empty());
    if texts().next().is_none() {
        return lines.iter().map(|l| l.to_string()).collect();
    }
    let all_commented = texts().all(|l| l.trim_start().starts_with(marker));
    let ws = |l: &str| l.len() - l.trim_start().len();
    let column = texts().map(|l| ws(l)).min().unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return line.to_string();
            }
            if all_commented {
                let at = ws(line);
                let body = &line[at + marker.len()..];
                format!("{}{}", &line[..at], body.strip_prefix(' ').unwrap_or(body))
            } else {
                // The shallowest column is all whitespace on every line with
                // text, so it is a character boundary.
                format!("{}{marker} {}", &line[..column], &line[column..])
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_of_drops_comments_and_blanks_strings() {
        assert_eq!(code_of("if c then  # why"), "if c then");
        assert_eq!(code_of("if c then  // old"), "if c then");
        assert_eq!(code_of("let s = \"a # b then\""), "let s = \"ssssssssss\"");
        assert_eq!(code_of("x /* note */ then"), "x   then");
        assert_eq!(code_of("  # only a comment"), "");
        // The old colour spelling is an error in the lexer, not a comment.
        assert_eq!(code_of("let c = #f80 then"), "let c = #f80 then");
        assert_eq!(code_of("let c = :f80 # warm"), "let c = :f80");
    }

    #[test]
    fn openers_at_the_end_of_a_line() {
        for line in [
            "if x < 0 then",
            "else if x == 0 then",
            "else",
            "  else   # fall back",
            "for item in items do",
            "while x < 10 do",
            "test \"addition\" do",
            "  when \"right\" ->",
            "let bucket = if ci < 0 then",
            "let f = fn(x) ->",
        ] {
            assert!(opens_block(line), "{line}");
        }
    }

    #[test]
    fn headers_open_a_block() {
        for line in [
            "fn clamp_all(xs, lo, hi)",
            "pub fn abs(x: int) -> int",
            "fn Rect.inset(r, n)",
            "fn f(a = g(1), b = [2])",
            "let describe = fn(x)",
            "defer_paint(fn()",
            "class Point",
            "pub class Point",
            "enum Shape",
            "match dir",
            "let y = match dir",
        ] {
            assert!(opens_block(line), "{line}");
        }
    }

    #[test]
    fn lines_that_open_nothing() {
        for line in [
            "",
            "   ",
            "let x = 1",
            "if x < 0 then return -x",
            "let label = if x > 5 then \"big\" else \"small\"",
            "for i in range(1, 6) do i * i",
            "fn box(w, h = w) \"{w}x{h}\"",
            "let double = fn(x) -> x * 2",
            "fn f(a,",
            "native fn sqrt(x: num) -> float",
            "pub native fn emit(event: string, arg)",
            "when d -> {grid: g}",
            "match x when 1 -> 2 end",
            "if c then a else b end",
            "# then",
            "print(\"what then\")",
            "r.else",
            "do_it()",
            "let classy = 1",
            "layout(",
        ] {
            assert!(!opens_block(line), "{line}");
        }
    }

    fn target(above: &[&str], current: usize, keyword: Dedent) -> Option<usize> {
        dedent_target(above.iter().rev().map(|l| l.to_string()), current, keyword)
    }

    #[test]
    fn else_goes_to_its_if() {
        assert_eq!(target(&["if a then", "  foo()"], 2, Dedent::Else), Some(0));
        // The nearest `if` wins.
        assert_eq!(
            target(&["if a then", "  if b then", "    x"], 4, Dedent::Else),
            Some(2)
        );
        // A finished `else` branch is passed over, to the `if` outside it.
        assert_eq!(
            target(&["if a then", "  if b then", "    x", "  else", "    y"], 4, Dedent::Else),
            Some(0)
        );
        // After `else if`, to that line.
        assert_eq!(
            target(&["  if a then", "    x", "  else if b then", "    y"], 4, Dedent::Else),
            Some(2)
        );
        // Value position: the statement's line.
        assert_eq!(
            target(&["let b = if c then", "  zeros(1)"], 2, Dedent::Else),
            Some(0)
        );
        // Blank lines and comments in between do not count.
        assert_eq!(
            target(&["if a then", "  foo()", "", "  # note"], 2, Dedent::Else),
            Some(0)
        );
        // Through a loop, which takes no `else`.
        assert_eq!(
            target(&["if a then", "  for x in xs do", "    f(x)"], 4, Dedent::Else),
            Some(0)
        );
    }

    #[test]
    fn else_already_in_place_stays() {
        // Typed at the `if`'s own column.
        assert_eq!(target(&["if a then", "  foo()"], 0, Dedent::Else), None);
        // Pulled back by hand to the inner `if`.
        assert_eq!(target(&["if a then", "  if b then", "    x"], 2, Dedent::Else), None);
        // Continues the one-line `if` above it (B4).
        assert_eq!(target(&["if x then", "  if a then b"], 2, Dedent::Else), None);
        assert_eq!(
            target(&["let ty = if t then A", "    else if u then B"], 4, Dedent::Else),
            None
        );
        // Nothing to attach to.
        assert_eq!(target(&["for x in xs do", "  f(x)"], 2, Dedent::Else), None);
        assert_eq!(target(&[], 2, Dedent::Else), None);
        // A tab in the indentation: no column to go by.
        assert_eq!(target(&["if a then", "\tfoo()"], 2, Dedent::Else), None);
    }

    #[test]
    fn when_goes_to_the_arm_before_it() {
        assert_eq!(
            target(&["match dir", "  when \"left\" ->", "    slide(g)"], 4, Dedent::When),
            Some(2)
        );
        // Arms at the `match`'s own column.
        assert_eq!(target(&["match dir", "when 1 ->", "  a()"], 2, Dedent::When), Some(0));
        // The first arm stays where the body starts.
        assert_eq!(target(&["match dir"], 2, Dedent::When), None);
        assert_eq!(target(&["let y = match dir"], 2, Dedent::When), None);
        // After a one-line arm it is already in place.
        assert_eq!(target(&["match dir", "  when 1 -> a"], 2, Dedent::When), None);
        // An inner match's first arm, inside an outer arm's body.
        assert_eq!(
            target(&["match a", "  when 1 ->", "    match b"], 6, Dedent::When),
            None
        );
    }

    #[test]
    fn a_block_moves_to_the_target_column() {
        let moved = |text: &str, target| reindent_lines(text, target).map(|l| l.join("\n"));
        // Whole lines, deeper than the target.
        assert_eq!(
            moved("    if a then\n      b\n    else\n      c", 2).unwrap(),
            "  if a then\n    b\n  else\n    c"
        );
        // Shallower than the target.
        assert_eq!(moved("for x in xs do\n  f(x)", 4).unwrap(), "    for x in xs do\n      f(x)");
        // Copied from the first character: the body shows the column.
        assert_eq!(
            moved("for x in xs do\n      f(x)\n      g(x)", 0).unwrap(),
            "for x in xs do\n  f(x)\n  g(x)"
        );
        assert_eq!(
            moved("if a then\n      b\n    else\n      c", 0).unwrap(),
            "if a then\n  b\nelse\n  c"
        );
        assert_eq!(moved("draw(\n      a,\n    )", 0).unwrap(), "draw(\n  a,\n)");
        // Siblings copied from the first character.
        assert_eq!(moved("a()\n    b()", 2).unwrap(), "  a()\n  b()");
        // A first line deeper than what follows keeps its offset.
        assert_eq!(moved("    x\n  else\n    y", 0).unwrap(), "  x\nelse\n  y");
        // Blank lines stay blank.
        assert_eq!(moved("  a\n\n  b\n", 0).unwrap(), "a\n\nb\n");
    }

    #[test]
    fn string_content_is_not_moved() {
        let text = "    let s = \"\"\"\n  void main() {\n    }\n  \"\"\"\n    f(s)";
        assert_eq!(
            reindent_lines(text, 0).unwrap().join("\n"),
            "let s = \"\"\"\n  void main() {\n    }\n  \"\"\"\nf(s)"
        );
        let text = "  let t = \"one\n      two\"\n  g(t)";
        assert_eq!(
            reindent_lines(text, 4).unwrap().join("\n"),
            "    let t = \"one\n      two\"\n    g(t)"
        );
        // An apostrophe in element text is not a string; a `#` comment with a
        // quote in it opens none.
        let text = "  <p>don't</p>\n  x # it's \"fine\n  y";
        assert_eq!(reindent_lines(text, 0).unwrap().join("\n"), "<p>don't</p>\nx # it's \"fine\ny");
    }

    #[test]
    fn tabs_and_empty_text_are_left_alone() {
        assert_eq!(reindent_lines("\tif a then\n\t\tb", 2), None);
        assert_eq!(reindent_lines("\n\n", 2), None);
        assert_eq!(reindent_lines("", 2), None);
    }

    #[test]
    fn a_paste_leaves_out_the_indent_the_line_already_has() {
        assert_eq!(
            reindent_paste("    if a then\n      b", 2, false).unwrap(),
            "if a then\n    b"
        );
        // The first line deeper than the block: the difference is kept.
        assert_eq!(reindent_paste("    x\n  else\n    y", 2, false).unwrap(), "  x\n  else\n    y");
        // One line: it lands at the cursor.
        assert_eq!(reindent_paste("      foo()", 2, false).unwrap(), "foo()");
        // Whole lines pasted in front of code: the code keeps its column.
        assert_eq!(reindent_paste("a()\nb()\n", 2, true).unwrap(), "a()\n  b()\n  ");
        assert_eq!(reindent_paste("a()\nb()\n", 2, false).unwrap(), "a()\n  b()\n");
    }

    #[test]
    fn comments_toggle_as_a_block() {
        let on = toggle_comment(&["  if a then", "    b", "", "  else"], "#");
        assert_eq!(on, ["  # if a then", "  #   b", "", "  # else"]);
        let lines: Vec<&str> = on.iter().map(String::as_str).collect();
        assert_eq!(toggle_comment(&lines, "#"), ["  if a then", "    b", "", "  else"]);
        // A block that is only partly commented gets commented again.
        assert_eq!(toggle_comment(&["# a", "b"], "#"), ["# # a", "# b"]);
        // No space after the marker is fine to remove.
        assert_eq!(toggle_comment(&["#a"], "#"), ["a"]);
        assert_eq!(toggle_comment(&["  // x"], "//"), ["  x"]);
        assert_eq!(toggle_comment(&["", "  "], "#"), ["", "  "]);
    }
}
