//! `petal lint` — source normalization (see docs/dev/linter-plan.md).
//!
//! Two passes, split by mechanism so neither ever reprints from the AST:
//!
//! 1. **Formatting** ([`reindent`]) — token-driven 2-space re-indentation.
//!    Nesting depth is computed from block-opening/-closing tokens and
//!    delimiters, and only the *leading whitespace* of each line is rewritten
//!    (plus trailing-whitespace trim and a single trailing newline).
//!    Everything else on a line — including comments — is copied verbatim, and
//!    any line that starts or ends inside a multi-line token (raw strings, JSX
//!    text) is left untouched, so the pass is comment- and content-safe by
//!    construction. Petal is newline-significant but not
//!    indentation-significant, so this cannot change semantics.
//!
//! 2. **Identity casts** ([`casts`]) — delete `int(n)` where `n` is already an
//!    `int` (likewise `float`/`str`). Candidates come from the type checker,
//!    which is deliberately conservative — anything it cannot prove infers
//!    `any` and is left alone — and are applied as two minimal string splices
//!    per cast, so comments and layout inside the argument survive.
//!
//! Because the cast rule changes tokens (not just whitespace), [`lint_source`]
//! gates it: if the original source compiles, the rewritten source must compile
//! too, or lint refuses to produce output. That is a weaker gate than
//! full IR equality — removing a call *does* change the IR, which is the point
//! — so the real guarantee comes from the detection rule: an `int` cast is only
//! dropped when its argument's static type is `int`, and `int()` on an `int` is
//! the identity (`rust/src/builtins/math.rs`).
//!
//! A note on what is *not* here: an earlier slice rewrote `x = f(x)` to the
//! rebind form `f(@x)`. That rule is gone. The `@` operator remains a language
//! feature, but it reads as sugar that has to be learned, so the linter no
//! longer pushes code into it.

use std::path::PathBuf;

use crate::env::Env;

mod casts;
mod reindent;

use casts::{apply_cast_edits, plan_cast_edits};
pub use reindent::reindent;

/// Context the compile gate needs to compile the source the same way
/// `petal run` would: module search dirs and the file's own path (imports
/// resolve relative to it).
#[derive(Default)]
pub struct LintOptions {
    pub include_dirs: Vec<PathBuf>,
    pub origin: Option<PathBuf>,
}

/// The result of linting one source text.
pub struct LintOutcome {
    /// The normalized source.
    pub output: String,
    /// Lines whose text changed in the formatting pass.
    pub reindented_lines: usize,
    /// Identity casts removed.
    pub casts_removed: usize,
    /// Human-readable notes.
    pub notes: Vec<String>,
}

impl LintOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }
}

/// Normalize `source`: drop identity casts (compile-gated), then re-indent.
/// Errors if the source doesn't parse, or if a rewrite fails the gate outright
/// (which indicates a lint bug and refuses all output).
pub fn lint_source(source: &str, opts: &LintOptions) -> Result<LintOutcome, String> {
    // Lint operates on valid programs only.
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;

    let mut notes = Vec::new();
    let chars: Vec<char> = source.chars().collect();

    // Lint sees no `class` declarations of its own: identity-cast detection
    // never consults one, and the built-in table is what resolves `Rect`.
    let classes = crate::classes::ClassTable::new();
    let signatures = crate::compiler::collect_fn_signatures(&stmts, &classes);
    let found = crate::typecheck::find_redundant_casts(&stmts, &signatures, &classes);
    let edits = plan_cast_edits(&found, &chars);
    let casts_removed = edits.len();
    let rewritten = if edits.is_empty() {
        source.to_string()
    } else {
        apply_cast_edits(&chars, &edits)
    };

    if casts_removed > 0 {
        // Only meaningful when the original compiles here at all; a file whose
        // imports don't resolve outside its app gets the detection rule alone.
        if compile_ir(source, opts).is_ok()
            && let Err(e) = compile_ir(&rewritten, opts)
        {
            return Err(format!(
                "lint bug: removing an identity cast broke compilation — refusing to \
                 produce output ({e})"
            ));
        }
        notes.push(format!("removed {casts_removed} redundant cast(s)"));
    }

    let output = reindent(&rewritten)?;
    let reindented_lines = count_changed_lines(&rewritten, &output);
    Ok(LintOutcome {
        output,
        reindented_lines,
        casts_removed,
        notes,
    })
}

fn count_changed_lines(before: &str, after: &str) -> usize {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let common = a.len().min(b.len());
    let mut n = (0..common).filter(|&i| a[i] != b[i]).count();
    n += a.len().max(b.len()) - common;
    n
}

/// Compile `source` and return its entry program's serialized IR, minus the
/// source text and source map (whitespace edits move spans). Used as a
/// does-it-still-compile gate, and by the corpus test to compare programs.
fn compile_ir(source: &str, opts: &LintOptions) -> Result<serde_json::Value, String> {
    let mut env = Env::new();
    for dir in &opts.include_dirs {
        env.add_module_path(dir.clone());
    }
    let pid = match &opts.origin {
        Some(path) => env.load_program_at(source, path)?,
        None => env.load_program(source)?,
    };
    let program = env
        .get_program(pid)
        .ok_or_else(|| "compiled program missing".to_string())?;
    let mut json = serde_json::to_value(program).map_err(|e| e.to_string())?;
    if let serde_json::Value::Object(map) = &mut json {
        map.remove("source");
        map.remove("source_map");
    }
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_ptl(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "node_modules" || n == "target")
                {
                    continue;
                }
                collect_ptl(&path, out);
            } else if path.extension().is_some_and(|e| e == "ptl") {
                out.push(path);
            }
        }
    }

    /// The linter-plan safeguard, as a property test over the whole repo
    /// corpus: every program that compiles must still compile after linting,
    /// and linting must be a fixed point. (A program with no casts to remove
    /// gets formatting only, which cannot change semantics at all.)
    #[test]
    fn lint_preserves_compilation_over_repo_corpus() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);
        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let opts = LintOptions {
                include_dirs: vec![],
                origin: Some(path.clone()),
            };
            let Ok(outcome) = lint_source(&src, &opts) else {
                continue;
            };
            if compile_ir(&src, &opts).is_err() {
                continue; // formatting-only file; nothing to compare
            }
            if let Err(e) = compile_ir(&outcome.output, &opts) {
                panic!("lint broke compilation for {}: {}", path.display(), e);
            }
            // A file the rules leave alone must be byte-identical in IR too,
            // which pins the formatting pass as semantics-free.
            if outcome.casts_removed == 0 {
                assert_eq!(
                    compile_ir(&src, &opts).ok(),
                    compile_ir(&outcome.output, &opts).ok(),
                    "formatting changed IR for {}",
                    path.display()
                );
            }
            // And linting again must be a fixed point.
            let again = lint_source(&outcome.output, &opts).expect("relint");
            assert_eq!(
                again.output,
                outcome.output,
                "lint not idempotent for {}",
                path.display()
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }
}
