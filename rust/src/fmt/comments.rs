//! Passes 3 and 4 — trailing-comment alignment and blank-line runs. Both work
//! on whole lines of already-indented text, and both consult the lexer so a
//! line inside a multi-line token (a raw string, JSX text) is never touched.

use crate::lexer::{Lexer, Token};

/// Per 0-based line: the char column where code ends and the column where its
/// trailing `//` comment starts, for lines that have both.
fn trailing_comments(source: &str) -> Result<Vec<Option<(usize, usize)>>, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    let line_count = source.split('\n').count();
    let mut out = vec![None; line_count];
    let (tokens, spans) = (&lexer.tokens, &lexer.token_spans);
    for k in 1..tokens.len() {
        if !matches!(tokens[k], Token::Newline) || matches!(tokens[k - 1], Token::Newline) {
            continue;
        }
        let comment = lexer.token_leading_trivia[k]
            .iter()
            .find(|t| t.kind == crate::trivia::TriviaKind::LineComment);
        let Some(comment) = comment else { continue };
        let code_end = spans[k - 1].end;
        let line = code_end.line as usize - 1;
        // The comment must share the line with the code before it.
        if spans[k].start.line != code_end.line {
            continue;
        }
        let line_start = code_end.offset as usize - (code_end.column as usize - 1);
        out[line] = Some((
            code_end.column as usize - 1,
            comment.start as usize - line_start,
        ));
    }
    Ok(out)
}

/// Trailing comments keep the column they are in (the spacing pass already
/// held each one where the author put it), but never touch their code: at
/// least one space. Consecutive lines whose comments share a column are one
/// aligned group, and when one member has to move right to clear its code,
/// the whole group moves with it.
pub fn align_trailing_comments(source: &str, protected: &[bool]) -> Result<String, String> {
    let found = trailing_comments(source)?;
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    let mut i = 0;
    while i < lines.len() {
        let eligible = |j: usize| found[j].is_some() && !protected.get(j).copied().unwrap_or(false);
        if !eligible(i) {
            i += 1;
            continue;
        }
        let group_col = found[i].unwrap().1;
        let mut j = i + 1;
        while j < lines.len() && eligible(j) && found[j].unwrap().1 == group_col {
            j += 1;
        }
        let column = (i..j)
            .map(|k| found[k].unwrap().0 + 1)
            .max()
            .unwrap()
            .max(group_col);
        if column != group_col {
            for k in i..j {
                let (code_end, comment_start) = found[k].unwrap();
                let chars: Vec<char> = lines[k].chars().collect();
                let mut line: String = chars[..code_end].iter().collect();
                line.push_str(&" ".repeat(column - code_end));
                line.extend(&chars[comment_start..]);
                out[k] = line;
            }
        }
        i = j;
    }
    Ok(out.join("\n"))
}

/// At most one blank line in a row, none at the start of the file. Blank lines
/// inside a multi-line token and in protected regions are content, and stay.
pub fn collapse_blank_lines(source: &str, protected: &[bool]) -> Result<String, String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize()?;
    // Lines that begin inside a token that started on an earlier line.
    let line_count = source.split('\n').count();
    let mut inside = vec![false; line_count];
    for (t, span) in lexer.tokens_with_spans() {
        if matches!(t, Token::Newline | Token::Eof) {
            continue;
        }
        // 1-based lines `start+1 ..= end` begin inside this token: 0-based
        // indices `start .. end`.
        for line in span.start.line..span.end.line {
            inside[line as usize] = true;
        }
    }

    let mut out: Vec<&str> = Vec::with_capacity(line_count);
    let mut prev_blank = true; // treat the start of the file as blank
    for (i, line) in source.split('\n').enumerate() {
        let is_layout = !inside[i] && !protected.get(i).copied().unwrap_or(false);
        let blank = line.is_empty();
        if blank && is_layout && prev_blank {
            continue;
        }
        prev_blank = blank && is_layout;
        out.push(line);
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_keep_their_columns() {
        let src = "state a = 1        // x\nstate bbb = 2   // y\n";
        assert_eq!(align_trailing_comments(src, &[]).unwrap(), src);
    }

    #[test]
    fn an_aligned_group_moves_together() {
        // `c` grew into the group's column; the group moves right as one.
        let src = "a = 1  // x\nbb = 2 // y\nccccc = 3 // z\n";
        let src = src.replace("ccccc = 3 // z", "cccccc = 3// z");
        let out = align_trailing_comments(&src, &[]).unwrap();
        assert_eq!(out, "a = 1  // x\nbb = 2 // y\ncccccc = 3 // z\n");
        let grow = "a = 1    // x\nbbbbbbbbb// y\n";
        assert_eq!(
            align_trailing_comments(grow, &[]).unwrap(),
            "a = 1     // x\nbbbbbbbbb // y\n"
        );
    }

    #[test]
    fn a_comment_never_touches_its_code() {
        assert_eq!(
            align_trailing_comments("a = 1// x\n", &[]).unwrap(),
            "a = 1 // x\n"
        );
    }

    #[test]
    fn collapses_blank_runs() {
        let src = "\n\nlet a = 1\n\n\n\nlet b = 2\n";
        assert_eq!(
            collapse_blank_lines(src, &[]).unwrap(),
            "let a = 1\n\nlet b = 2\n"
        );
    }

    #[test]
    fn blank_lines_in_raw_strings_stay() {
        let src = "let s = \"\"\"\na\n\n\n\nb\n\"\"\"\n";
        assert_eq!(collapse_blank_lines(src, &[]).unwrap(), src);
    }
}
