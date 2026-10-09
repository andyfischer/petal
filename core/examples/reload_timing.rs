//! What a hot reload costs, by the path it takes.
//!
//! ```text
//! cargo run --release --example reload_timing -- <entry.ptl> [<file-to-edit.ptl>]
//! ```
//!
//! Loads the program, then times `Env::reload_program` for a layout edit (a
//! comment added), a value edit (one number literal changed) and a structural
//! edit (a statement added), each made to `<file-to-edit>` (default: the entry
//! file) without touching the disk. Every timing includes lowering, so the
//! figures are what a host pays before the next frame can run. Nothing is
//! executed: the program may need natives this binary does not have.

use std::path::PathBuf;
use std::time::Instant;

use petal::env::{Env, ReloadOutcome};
use petal::lexer::{Lexer, Token};

const ROUNDS: usize = 9;

/// `source` with the first number literal of a `config let` (else the first
/// number literal at all) nudged, or `None` when it has none.
fn retuned(source: &str, step: usize) -> Option<String> {
    let mut lexer = Lexer::new(source);
    lexer.tokenize().ok()?;
    let tokens: Vec<(&Token, &petal::source_map::SourceSpan)> = lexer.tokens_with_spans().collect();
    let is_number = |t: &Token| matches!(t, Token::Int(_) | Token::Float(_));
    let config = tokens.iter().position(|(t, _)| matches!(t, Token::Ident(n) if n == "config"));
    let at = config
        .and_then(|c| tokens[c..].iter().position(|(t, _)| is_number(t)).map(|i| c + i))
        .or_else(|| tokens.iter().position(|(t, _)| is_number(t)))?;
    let (token, span) = tokens[at];
    let text = match token {
        Token::Int(n) => (n + 1 + step as i64).to_string(),
        Token::Float(f) => format!("{:?}", f + 0.5 + step as f64),
        _ => unreachable!(),
    };
    let chars: Vec<char> = source.chars().collect();
    let mut out: String = chars[..span.start.offset as usize].iter().collect();
    out.push_str(&text);
    out.extend(&chars[span.end.offset as usize..]);
    Some(out)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let entry = PathBuf::from(args.next().expect("usage: reload_timing <entry.ptl> [<file.ptl>]"));
    let edited = args.next().map(PathBuf::from).unwrap_or_else(|| entry.clone());
    let entry_source = std::fs::read_to_string(&entry).expect("entry file");
    let original = std::fs::read_to_string(&edited).expect("edited file");
    let edits_entry = petal::module::canonical_path(&edited) == petal::module::canonical_path(&entry);

    let mut env = Env::new();
    env.set_echo(false);
    let t = Instant::now();
    let pid = env
        .load_program_diag(&entry_source, Some(&entry))
        .unwrap_or_else(|e| panic!("{e}"));
    let compile = t.elapsed();
    let t = Instant::now();
    env.lower_program(pid).unwrap();
    let lower = t.elapsed();
    let sid = env.create_stack(pid).unwrap();
    let program = env.get_program(pid).unwrap();
    println!(
        "{}: {} files, {} lines, {} terms",
        entry.display(),
        program.source_map.files.len().max(1),
        if program.source_map.files.is_empty() {
            program.source.lines().count()
        } else {
            program.source_map.files.iter().map(|f| f.source.lines().count()).sum()
        },
        program.terms.len()
    );
    println!(
        "first load: compile {:.2} ms + lower {:.2} ms",
        compile.as_secs_f64() * 1e3,
        lower.as_secs_f64() * 1e3
    );

    // Make `text` the edited file's contents, without writing it.
    let reload = |env: &mut Env, text: &str| -> (ReloadOutcome, f64) {
        if !edits_entry {
            env.override_file_source(&edited, text);
        }
        let source = if edits_entry { text } else { &entry_source };
        let t = Instant::now();
        let report = env
            .reload_program(sid, source, Some(&entry))
            .unwrap_or_else(|e| panic!("{e}"));
        env.lower_program(pid).unwrap();
        (report.outcome, t.elapsed().as_secs_f64() * 1e3)
    };
    let mut measure = |label: &str, make: &dyn Fn(usize) -> String| {
        let mut best = f64::MAX;
        let mut outcome = ReloadOutcome::Unchanged;
        for round in 0..ROUNDS {
            let (o, ms) = reload(&mut env, &make(round));
            outcome = o;
            best = best.min(ms);
            // Back to the file as it is, so each round makes the same edit.
            reload(&mut env, &original);
        }
        println!("{label:<34} {:<11} {best:>9.3} ms", outcome.label());
    };

    println!("\nreload of an edit to {} (min of {ROUNDS}):", edited.display());
    measure("comment added", &|i| format!("// note {i}\n{original}"));
    if retuned(&original, 0).is_some() {
        measure("one number changed", &|i| retuned(&original, i).unwrap());
    }
    measure("statement added", &|i| format!("{original}\nlet reload_timing_probe_{i} = {i}\n"));

    // The live half of a drag: one number of a top-level binding set by path,
    // no file involved.
    use petal::static_value::{StaticValue, static_bindings};
    let knob = static_bindings(&original).ok().and_then(|bindings| {
        bindings.into_iter().find_map(|b| match b.value {
            Ok(StaticValue::Float(f)) => Some((b.name, f)),
            _ => None,
        })
    });
    if let Some((name, value)) = knob {
        println!("\nset_config_value(`{name}`) (min of {ROUNDS}):");
        for (label, file) in [("file named", Some(edited.as_path())), ("file searched for", None)] {
            let mut best = f64::MAX;
            for round in 0..ROUNDS {
                let next = StaticValue::float(value + 1.0 + round as f64);
                let t = Instant::now();
                env.set_config_value(sid, file, &name, &next)
                    .unwrap_or_else(|e| panic!("{e}"));
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
            }
            println!("{label:<34} {:<11} {best:>9.3} ms", "patched");
        }
    }
}
