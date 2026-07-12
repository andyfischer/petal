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
//! 2. **Rebind** ([`find_rebinds`]) — the semantics-preserving idiom rewrite
//!    `x = f(x)` → `f(@x)`. Candidates are detected on the AST and applied as
//!    two minimal string splices (delete the `x = ` prefix, insert `@` before
//!    the matching argument) — no reprinting, so comments inside the call
//!    survive.
//!
//! Because rebind changes tokens (not just whitespace), [`lint_source`] gates
//! it behind an **IR-equivalence check**: the pre- and post-lint sources must
//! compile to structurally identical IR (modulo source text and spans). If
//! the original doesn't compile (e.g. imports unresolvable here), rebinds are
//! skipped and only formatting applies; if the gate ever reports a real
//! difference, lint refuses to produce output — that's a linter bug, not a
//! user error.

use std::path::PathBuf;

use crate::env::Env;

mod rebind;
mod reindent;

use rebind::{apply_rebinds, find_rebinds};
pub use reindent::reindent;

/// Context the IR-equivalence gate needs to compile the source the same way
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
    /// Rebind rewrites applied (post-gate).
    pub rebinds: usize,
    /// Human-readable notes (e.g. rebinds skipped because the IR gate was
    /// unavailable).
    pub notes: Vec<String>,
}

impl LintOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }
}

/// Normalize `source`: apply rebind rewrites (IR-gated), then re-indent.
/// Errors if the source doesn't parse, or if a rewrite fails the equivalence
/// gate outright (which indicates a lint bug and refuses all output).
pub fn lint_source(source: &str, opts: &LintOptions) -> Result<LintOutcome, String> {
    // Lint operates on valid programs only.
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;

    let mut notes = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let candidates = find_rebinds(&stmts, &chars);
    let mut rebinds = candidates.len();
    let mut rebound = if candidates.is_empty() {
        source.to_string()
    } else {
        apply_rebinds(&chars, &candidates)
    };

    if rebinds > 0 {
        match ir_gate(source, &rebound, opts) {
            Gate::Equivalent => {}
            Gate::Different(detail) => {
                return Err(format!(
                    "lint bug: the rebind rewrite changed the compiled IR — refusing to \
                     produce output ({detail})"
                ));
            }
            Gate::Unavailable(reason) => {
                notes.push(format!(
                    "skipped {rebinds} rebind rewrite(s): can't verify IR equivalence ({reason})"
                ));
                rebinds = 0;
                rebound = source.to_string();
            }
        }
    }

    let output = reindent(&rebound)?;
    let reindented_lines = count_changed_lines(&rebound, &output);
    Ok(LintOutcome {
        output,
        reindented_lines,
        rebinds,
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

// ---------------------------------------------------------------------------
// IR-equivalence gate
// ---------------------------------------------------------------------------

enum Gate {
    Equivalent,
    Different(String),
    Unavailable(String),
}

/// Compile both sources and compare the entry programs' full serialized IR —
/// term ids, blocks, registers, constants, everything except the source text
/// and the source map (whitespace edits move spans). No structural slack:
/// statement-level `f(@x)` desugars to exactly `x = f(x)`
/// ([`crate::desugar`]), so the rebind rewrite must produce an identical
/// program.
fn ir_gate(before: &str, after: &str, opts: &LintOptions) -> Gate {
    let pre = match compile_ir(before, opts) {
        Ok(v) => v,
        Err(e) => return Gate::Unavailable(e),
    };
    let post = match compile_ir(after, opts) {
        Ok(v) => v,
        Err(e) => return Gate::Different(format!("rewritten source fails to compile: {e}")),
    };
    if pre == post {
        Gate::Equivalent
    } else {
        Gate::Different("compiled IR differs".to_string())
    }
}

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

    // ---- IR gate + corpus property test ----

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
    /// corpus: for every program that compiles, `lint` output must compile to
    /// structurally identical IR. (Programs that parse but don't compile in
    /// isolation — e.g. import-dependent files — get formatting only, which
    /// `lint_source` guarantees by skipping unverifiable rebinds.)
    #[test]
    fn lint_preserves_ir_over_repo_corpus() {
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
            match ir_gate(&src, &outcome.output, &opts) {
                Gate::Equivalent => {}
                Gate::Different(d) => {
                    panic!("lint changed IR for {}: {}", path.display(), d)
                }
                Gate::Unavailable(e) => {
                    panic!("IR gate unavailable for {}: {}", path.display(), e)
                }
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
