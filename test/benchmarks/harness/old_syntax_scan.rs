//! What is left of the old syntax in a tree, read with the lexer
//! (docs/tasks/pythonic-syntax.md §9, "migration properties").
//!
//! A grep for `elsif`, `//`, `#f80` or a line holding `end` counts strings,
//! `"""` GLSL blocks and comment text; this counts tokens and trivia:
//!
//! ```text
//! cd core
//! cargo run --example old_syntax_scan -- scan DIR...     # counts, and each file that has any
//! cargo run --example old_syntax_scan -- same A.ptl B.ptl   # do the two parse to the same AST?
//! ```
//!
//! `scan` reads every `.ptl` under the directories (skipping what `petal
//! migrate-old-code` skips: `node_modules`, `target`, `temp`, `out`, `dist`
//! and dot-directories) and reports, per file and in total:
//!
//! - `hex`: `#f80` color literals, an error since 2026-11-12 (read with
//!   `Lexer::legacy`, the reading `petal migrate-old-code` uses);
//! - `slash`: `//` line comments;
//! - `elsif`: the `elsif` keyword (not a field or key of that name);
//! - `end`: an `end` alone on its line. Some are needed (`petal
//!   migrate-old-code` keeps the ones the parse depends on), so this is a
//!   count to compare with the tool's "kept", not an error;
//! - files that do not lex, and files that lex but do not parse.
//!
//! It exits 1 when any file holds a `hex`, `slash` or `elsif`, or does not
//! lex or parse. `same` exits 1 when the two files differ as ASTs (source
//! positions aside) or either does not parse; the first is read by its
//! `end`s when its indentation disagrees with them, as old source may.

use std::path::{Path, PathBuf};

use petal::lexer::{Lexer, Token};
use petal::trivia::TriviaKind;

fn collect_ptl(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| {
                ["node_modules", "target", "temp", "out", "dist"].iter().any(|skip| n == *skip)
                    || n.to_string_lossy().starts_with('.')
            }) {
                continue;
            }
            collect_ptl(&path, out);
        } else if path.extension().is_some_and(|e| e == "ptl") {
            out.push(path);
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Counts {
    hex: usize,
    slash: usize,
    elsif: usize,
    end: usize,
}

/// The old forms in `source`, or why it does not lex.
fn count(source: &str) -> Result<Counts, String> {
    let mut lexer = Lexer::new(source).legacy();
    lexer.tokenize()?;
    let mut c = Counts {
        hex: lexer.legacy_colors.len(),
        ..Counts::default()
    };
    for trivia in &lexer.token_leading_trivia {
        c.slash += trivia
            .iter()
            .filter(|t| t.kind == TriviaKind::LineComment && t.text.starts_with("//"))
            .count();
    }
    for (i, token) in lexer.tokens.iter().enumerate() {
        let after_dot = i > 0 && matches!(lexer.tokens[i - 1], Token::Dot | Token::QuestionDot);
        match token {
            Token::Elsif if !after_dot => c.elsif += 1,
            Token::End
                if !after_dot
                    && lexer.token_first_on_line.get(i).copied().unwrap_or(false)
                    && matches!(lexer.tokens.get(i + 1), None | Some(Token::Newline | Token::Eof)) =>
            {
                c.end += 1
            }
            _ => {}
        }
    }
    Ok(c)
}

fn scan(dirs: &[String]) -> i32 {
    let mut files = Vec::new();
    for dir in dirs {
        collect_ptl(Path::new(dir), &mut files);
    }
    let mut total = Counts::default();
    let (mut unlexed, mut unparsed, mut dirty) = (0usize, 0usize, 0usize);
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            println!("{}: cannot be read", file.display());
            unlexed += 1;
            continue;
        };
        let c = match count(&source) {
            Ok(c) => c,
            Err(e) => {
                println!("{}: does not lex: {}", file.display(), e.lines().next().unwrap_or(""));
                unlexed += 1;
                continue;
            }
        };
        if c.hex + c.slash + c.elsif > 0 {
            dirty += 1;
            println!(
                "{}: hex {} slash {} elsif {} end {}",
                file.display(),
                c.hex,
                c.slash,
                c.elsif,
                c.end
            );
        } else if c.end > 0 {
            println!("{}: end {}", file.display(), c.end);
        }
        total.hex += c.hex;
        total.slash += c.slash;
        total.elsif += c.elsif;
        total.end += c.end;
        // The parse, as the language reads the file today.
        if c.hex == 0
            && let Err(e) = petal::cst::parse_source(&source, petal::source_map::ENTRY_FILE)
        {
            println!("{}: does not parse: {}", file.display(), e.lines().next().unwrap_or(""));
            unparsed += 1;
        }
    }
    println!(
        "{} file(s): hex {} slash {} elsif {} end-only lines {}; {} with an old form, {} do not lex, {} do not parse",
        files.len(),
        total.hex,
        total.slash,
        total.elsif,
        total.end,
        dirty,
        unlexed,
        unparsed
    );
    i32::from(dirty + unlexed + unparsed > 0)
}

fn same(a: &str, b: &str) -> i32 {
    let shape = |path: &str, by_end: bool| -> Result<serde_json::Value, String> {
        let source = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let stmts = if by_end {
            petal::rewrite::parse_ast_or_by_end(&source).map(|(_, stmts, _)| stmts)
        } else {
            petal::cst::parse_source(&source, petal::source_map::ENTRY_FILE).map(|(_, stmts)| stmts)
        }
        .map_err(|e| format!("{path}: {}", e.lines().next().unwrap_or("")))?;
        petal::rewrite::ast_shape(&stmts)
    };
    match (shape(a, true), shape(b, false)) {
        (Ok(x), Ok(y)) if x == y => {
            println!("same");
            0
        }
        (Ok(_), Ok(_)) => {
            println!("different");
            1
        }
        (Err(e), _) | (_, Err(e)) => {
            println!("error: {e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("scan") if args.len() > 1 => scan(&args[1..]),
        Some("same") if args.len() == 3 => same(&args[1], &args[2]),
        _ => {
            eprintln!("usage: old_syntax_scan scan DIR... | same A.ptl B.ptl");
            2
        }
    };
    std::process::exit(code);
}
