//! `petal migrate-old-code --markdown`: the migrations over the ```` ```petal ````
//! fences of a Markdown file.
//!
//! Each fence whose info string starts with `petal` is migrated as a source
//! of its own, with [`MigrateOptions::fragment`] on: a fence that parses
//! gets every selected text migration, and one that only lexes (it elides
//! code with `...`, or shows half a construct) gets the ones that need no
//! parse, `colour-literals`, `hash-comments` and `elsif-to-else-if`. A fence
//! that does not lex at all (pseudo-code, a grammar sketch) is left as
//! written and counted; that is what most `ignore` fences that are not
//! Petal are, so the tag itself decides nothing. Everything outside the
//! fences, and every other fence, is copied through.
//!
//! The `print(x) // expected` convention of the documentation tests
//! (test/vitest/doc-snippets.test.ts) is a trailing comment, so
//! `hash-comments` respells it `print(x) # expected` with the rest.
//!
//! A fence tagged `no-migrate` (```` ```petal ignore no-migrate ````) is
//! left alone: the way to keep an example *of* an old form in a document
//! about it. Inside a fence, `# petal-lint-ignore <migration>` works as it
//! does in a file.
//!
//! A fence indented with its list item is migrated without that indentation
//! and gets it back.

use super::{Finding, MigrateError, MigrateOptions, Skipped, migrate_source_checked};

/// The fence tag that keeps a fence as written.
pub const NO_MIGRATE: &str = "no-migrate";

/// The result of migrating one Markdown text.
#[derive(Default)]
pub struct MarkdownOutcome {
    /// The migrated Markdown. Equal to the input when nothing was rewritten.
    pub output: String,
    /// Every site, with its line in the Markdown file.
    pub findings: Vec<Finding>,
    /// How many `petal` fences the file has.
    pub fences: usize,
    /// How many of them were rewritten.
    pub rewritten: usize,
    /// How many were migrated by their tokens alone (they do not parse).
    pub fragments: usize,
    /// The 1-based lines of the fences left as written because they do not
    /// lex.
    pub unreadable: Vec<usize>,
    /// Gate failures, by the line of the fence: a migration skipped for the
    /// fence (`end-lines`), or the whole fence left as written.
    pub skipped: Vec<(usize, Skipped)>,
    pub ends_removed: usize,
    pub ends_kept: usize,
}

/// One fenced block of a Markdown text.
struct Fence {
    /// Index, among the text's lines, of the opening line.
    open: usize,
    /// Index of the closing line (the line count when the fence never closes).
    close: usize,
    indent: usize,
    info: String,
}

fn fences(lines: &[&str]) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end_matches(['\n', '\r']);
        let body = line.trim_start_matches(' ');
        let indent = line.len() - body.len();
        let mark = body.chars().next().filter(|c| matches!(c, '`' | '~'));
        let run = mark.map_or(0, |m| body.chars().take_while(|&c| c == m).count());
        let info = &body[run.min(body.len())..];
        // An opening fence: three or more of one mark; a backtick fence's
        // info string holds no backtick (that is inline code).
        let Some(mark) = mark.filter(|&m| run >= 3 && !(m == '`' && info.contains('`'))) else {
            i += 1;
            continue;
        };
        let mut close = i + 1;
        while close < lines.len() {
            let l = lines[close].trim();
            if l.chars().count() >= run && l.chars().all(|c| c == mark) {
                break;
            }
            close += 1;
        }
        out.push(Fence { open: i, close, indent, info: info.trim().to_string() });
        i = close + 1;
    }
    out
}

/// Run the selected text migrations over every `petal` fence of `source`.
pub fn migrate_markdown(source: &str, opts: &MigrateOptions) -> MarkdownOutcome {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut out = MarkdownOutcome::default();
    let mut text = String::with_capacity(source.len());
    let mut at = 0usize;
    let opts = MigrateOptions { fragment: true, origin: None, ..opts.clone() };
    for fence in fences(&lines) {
        let mut tags = fence.info.split_whitespace();
        if tags.next() != Some("petal") {
            continue;
        }
        out.fences += 1;
        if tags.any(|t| t == NO_MIGRATE) || fence.close >= lines.len() {
            continue;
        }
        let body = &lines[fence.open + 1..fence.close];
        // The fence's own indentation comes off when every line has it.
        let margin = if body
            .iter()
            .all(|l| l.trim().is_empty() || l.len() - l.trim_start_matches(' ').len() >= fence.indent)
        {
            fence.indent
        } else {
            0
        };
        let code: String = body
            .iter()
            .map(|l| if l.trim().is_empty() { l.trim_start_matches(' ') } else { &l[margin..] })
            .collect();
        let line = fence.open + 1; // 1-based line of the opening fence
        match migrate_source_checked(&code, &opts) {
            Ok(done) => {
                out.fragments += done.fragment as usize;
                out.ends_removed += done.ends_removed;
                out.ends_kept += done.ends_kept;
                out.skipped.extend(done.skipped.into_iter().map(|s| (line, s)));
                out.findings.extend(done.findings.into_iter().map(|mut f| {
                    f.line += line;
                    f.column += margin;
                    f
                }));
                if done.output != code {
                    out.rewritten += 1;
                    for l in &lines[at..=fence.open] {
                        text.push_str(l);
                    }
                    let pad = " ".repeat(margin);
                    for l in done.output.split_inclusive('\n') {
                        if !l.trim().is_empty() {
                            text.push_str(&pad);
                        }
                        text.push_str(l);
                    }
                    if !done.output.is_empty() && !done.output.ends_with('\n') {
                        text.push('\n');
                    }
                    at = fence.close;
                }
            }
            Err(MigrateError::Unreadable(_)) => out.unreadable.push(line),
            Err(MigrateError::Gate { migration, reason }) => {
                out.skipped.push((line, Skipped { migration, reason }));
            }
        }
    }
    for l in &lines[at..] {
        text.push_str(l);
    }
    out.output = text;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_are_found_by_their_marks_and_nest_by_length() {
        let md = "a\n```petal ignore\nx\n```\n````md\n```petal\ny\n```\n````\n  ~~~petal\n  z\n  ~~~\n`inline ```petal`\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        let found: Vec<(usize, usize, usize, String)> =
            fences(&lines).into_iter().map(|f| (f.open, f.close, f.indent, f.info)).collect();
        assert_eq!(
            found,
            [
                (1, 3, 0, "petal ignore".to_string()),
                (4, 8, 0, "md".to_string()),
                (9, 11, 2, "petal".to_string()),
            ]
        );
    }
}
