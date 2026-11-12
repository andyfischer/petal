//! Corpus invariance for the layout parser (docs/tasks/pythonic-syntax.md §9).
//!
//! Indentation closes blocks and `end` is optional, and the rule that makes
//! that safe is that code written before the change still means what it
//! meant: every file that parsed must parse to the same AST, or fail with an
//! indentation error. This tool checks that against a baseline written by an
//! older build:
//!
//! ```text
//! cd core
//! cargo run --release --example layout_corpus -- write BASELINE DIR...   # old build
//! cargo run --release --example layout_corpus -- check BASELINE DIR...   # new build
//! cargo run --release --example layout_corpus -- time DIR...             # parse time
//! ```
//!
//! With `LAYOUT_CORPUS_BY_END` set, files are parsed in the reading of
//! `Parser::by_end` (every block runs to its `end`, columns mean nothing),
//! which `petal fmt` and `petal migrate-old-code` fall back to for old source.
//! That reading is the parser from before the change, so against such a
//! baseline `check` must report every file the same.
//!
//! Each `.ptl` file under the directories is parsed twice: as the language
//! reads it today, and as `petal migrate-old-code` reads source from before
//! the lexical migrations (`Lexer::legacy`: `#f80` colours, `else if` as an
//! `if` nested in an `else`), so an unmigrated project is covered too. The
//! baseline holds one hash per top-level statement of the AST with every
//! source position removed, so a difference names the statement.
//!
//! `check` prints the counts of files that are the same, that now fail with
//! an indentation error, that fail some other way, and that parse to a
//! different AST. The last two must be zero; the process exits 1 otherwise.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use petal::lexer::Lexer;
use petal::parse::Parser;

fn collect_ptl(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| {
                n == "node_modules"
                    || n == "target"
                    || n == "dist"
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

/// `text` (the `Debug` rendering of an AST) with every `SourceSpan { … }` and
/// `SourcePosition { … }` removed.
fn strip_positions(text: &str) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut rest = text;
    loop {
        let next = ["SourceSpan {", "SourcePosition {"]
            .iter()
            .filter_map(|needle| rest.find(needle))
            .min();
        let Some(at) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let mut depth = 0usize;
        let mut end = rest.len();
        for (i, ch) in rest[at..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = at + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &rest[end..];
    }
}

/// One hash per top-level statement, or the parse error.
fn parse(source: &str, legacy: bool) -> Result<Vec<u64>, String> {
    let mut lexer = Lexer::new(source).without_trivia();
    if legacy {
        lexer = lexer.legacy();
    }
    lexer.tokenize()?;
    // Measured as the compiler measures it: tabs refused, a line that starts
    // with a `/* … */` comment indented to the comment.
    let mut parser = Parser::new(lexer.tokens.clone(), lexer.token_spans.clone())
        .with_tab_indentation(&lexer.token_tab_indented)
        .with_layout_columns(&lexer.token_layout_column);
    if std::env::var_os("LAYOUT_CORPUS_BY_END").is_some() {
        // The reading tools fall back to for old source: by its `end`s alone.
        // Against a baseline written before the layout parser, `check` must
        // then report every file the same.
        parser = parser.by_end();
    }
    let stmts = parser.parse_program()?;
    Ok(stmts
        .iter()
        .map(|stmt| {
            let mut hasher = DefaultHasher::new();
            strip_positions(&format!("{stmt:?}")).hash(&mut hasher);
            hasher.finish()
        })
        .collect())
}

fn render(result: &Result<Vec<u64>, String>) -> String {
    match result {
        Ok(hashes) => {
            let list: Vec<String> = hashes.iter().map(|h| format!("{h:016x}")).collect();
            format!("OK {}", list.join(","))
        }
        Err(e) => format!("ERR {}", e.replace('\n', " ")),
    }
}

/// Whether `message` is one of the layout parser's own errors: the outcome a
/// file whose indentation disagrees with its `end`s is allowed to have.
fn is_layout_error(message: &str) -> bool {
    [
        "expected an indented block",
        "is indented less than its block",
        "unexpected indent",
        "does not line up with",
        "tab in indentation",
        "Unexpected token: `end`",
        "is indented less than the first line",
        "is ambiguous; close it with `end`",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let (baseline_path, dirs) = match mode {
        "write" | "check" => (args.get(1).cloned(), &args[2.min(args.len())..]),
        "time" => (None, &args[1..]),
        _ => {
            eprintln!("usage: layout_corpus write|check BASELINE DIR... | time DIR...");
            std::process::exit(2);
        }
    };
    let mut files = Vec::new();
    for dir in dirs {
        collect_ptl(Path::new(dir), &mut files);
    }
    files.sort();
    files.dedup();

    if mode == "time" {
        let sources: Vec<String> =
            files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
        let bytes: usize = sources.iter().map(String::len).sum();
        let mut best = f64::MAX;
        let mut lex_best = f64::MAX;
        for _ in 0..7 {
            let mut lexed = Vec::new();
            let t = std::time::Instant::now();
            for source in &sources {
                let mut lexer = Lexer::new(source).without_trivia();
                if lexer.tokenize().is_err() {
                    lexer = Lexer::new(source).without_trivia().legacy();
                    let _ = lexer.tokenize();
                }
                lexed.push((lexer.tokens, lexer.token_spans));
            }
            lex_best = lex_best.min(t.elapsed().as_secs_f64());
            let t = std::time::Instant::now();
            for (tokens, spans) in lexed {
                let _ = std::hint::black_box(Parser::new(tokens, spans).parse_program());
            }
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!(
            "{} files, {:.1} MB: lex {:.1} ms, parse {:.1} ms (best of 7)",
            sources.len(),
            bytes as f64 / 1e6,
            lex_best * 1e3,
            best * 1e3
        );
        return;
    }

    let baseline_path = baseline_path.expect("a baseline path");
    let mut lines = BTreeMap::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        for legacy in [false, true] {
            let key = format!("{}\t{}", file.display(), if legacy { "legacy" } else { "new" });
            lines.insert(key, render(&parse(&source, legacy)));
        }
    }
    if mode == "write" {
        let text: String = lines.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
        std::fs::write(&baseline_path, text).expect("write the baseline");
        println!("wrote {} entries for {} files", lines.len(), files.len());
        return;
    }

    let baseline = std::fs::read_to_string(&baseline_path).expect("read the baseline");
    let mut same = 0usize;
    let mut was_error = 0usize;
    let mut now_parses = Vec::new();
    let mut layout_fail = Vec::new();
    let mut other_fail = Vec::new();
    let mut different = Vec::new();
    let mut missing = 0usize;
    for line in baseline.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(file), Some(lexing), Some(old)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let key = format!("{file}\t{lexing}");
        let Some(new) = lines.get(&key) else {
            missing += 1;
            continue;
        };
        if !old.starts_with("OK") {
            was_error += 1;
            if new.starts_with("OK") {
                now_parses.push(key);
            }
            continue;
        }
        if old == new {
            same += 1;
        } else if let Some(message) = new.strip_prefix("ERR ") {
            if is_layout_error(message) {
                layout_fail.push(format!("{key}\t{message}"));
            } else {
                other_fail.push(format!("{key}\t{message}"));
            }
        } else {
            let old_hashes: Vec<&str> = old[3..].split(',').collect();
            let new_hashes: Vec<&str> = new[3..].split(',').collect();
            let at = old_hashes
                .iter()
                .zip(&new_hashes)
                .position(|(a, b)| a != b)
                .unwrap_or(old_hashes.len().min(new_hashes.len()));
            different.push(format!(
                "{key}\ttop-level statement {} of {} (now {})",
                at + 1,
                old_hashes.len(),
                new_hashes.len()
            ));
        }
    }
    for (title, list) in [
        ("fails with an indentation error", &layout_fail),
        ("FAILS ANOTHER WAY", &other_fail),
        ("DIFFERENT AST", &different),
        ("did not parse before and parses now", &now_parses),
    ] {
        if !list.is_empty() {
            println!("== {title}: {}", list.len());
            for item in list {
                println!("  {item}");
            }
        }
    }
    println!(
        "same {same}, indentation error {}, other failure {}, DIFFERENT {}, did not parse \
         before {was_error} (of which parse now: {}), not found {missing}",
        layout_fail.len(),
        other_fail.len(),
        different.len(),
        now_parses.len()
    );
    if !other_fail.is_empty() || !different.is_empty() {
        std::process::exit(1);
    }
}
