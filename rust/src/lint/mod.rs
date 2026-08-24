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
//! 3. **`if`-chain to `match`** ([`to_match`]) — rewrite an `if`/`elsif` chain
//!    that tests one subject against string/bool/nil literals into a `match`.
//!    Like the cast rule it detects over the AST and applies span splices, and
//!    the splices only ever cover the glue between the arms, so every pattern
//!    and body survives verbatim. It runs after the cast rule on the cast
//!    rule's output, which means a re-parse: the casts moved the spans.
//!
//! Because the cast rule changes tokens (not just whitespace), [`lint_source`]
//! gates it: if the original source compiles, the rewritten source must compile
//! too, or lint refuses to produce output. That is a weaker gate than
//! full IR equality — removing a call *does* change the IR, which is the point
//! — so the real guarantee comes from the detection rule: an `int` cast is only
//! dropped when its argument's static type is `int`, and `int()` on an `int` is
//! the identity (`rust/src/builtins/math.rs`).
//!
//! [`verify_rewrite`] (`petal lint --verify`) is the gate on top of all of
//! this: it compiles both sides and compares their IR with
//! [`crate::ir_equiv::ir_equivalent`], so a rewrite that cannot be accepted is
//! never written. Only the formatting pass is required to be IR-invisible; the
//! other two change the IR by design, and the default mode says so rather than
//! calling it a failure. See docs/dev/refactor-verification.md §7.
//!
//! A note on what is *not* here: an earlier slice rewrote `x = f(x)` to the
//! rebind form `f(@x)`. That rule is gone. The `@` operator remains a language
//! feature, but it reads as sugar that has to be learned, so the linter no
//! longer pushes code into it.

use std::path::PathBuf;

use crate::env::Env;

mod casts;
mod reindent;
mod to_match;

use casts::{apply_cast_edits, plan_cast_edits};
pub use reindent::reindent;
use to_match::{apply_match_edits, plan_match_edits};

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
    /// `if`/`elsif` chains rewritten as a `match`.
    pub chains_to_match: usize,
    /// The text after the semantic passes but *before* re-indentation — the
    /// input the formatting pass was handed. `--verify` compares this against
    /// [`LintOutcome::output`] to prove the formatting pass on its own, which
    /// is the only part of the rewrite that is supposed to leave the IR alone.
    pub pre_format: String,
    /// Human-readable notes.
    pub notes: Vec<String>,
}

impl LintOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }

    /// Did a pass that is *expected* to change the IR run on this file?
    pub fn has_semantic_rewrite(&self) -> bool {
        self.casts_removed > 0 || self.chains_to_match > 0
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
        notes.push(format!("removed {casts_removed} redundant cast(s)"));
    }

    // Pass 3 — `if`-chain to `match`. The cast splices moved every offset, so
    // this re-parses rather than reusing the AST above.
    let (chars, stmts) = if casts_removed > 0 {
        let chars: Vec<char> = rewritten.chars().collect();
        let (_tree, stmts) = crate::rewrite::parse_ast(&rewritten)?;
        (chars, stmts)
    } else {
        (chars, stmts)
    };
    let (match_edits, chains_to_match) = plan_match_edits(&stmts, &chars);
    let rewritten = if match_edits.is_empty() {
        rewritten
    } else {
        apply_match_edits(&chars, &match_edits)
    };
    if chains_to_match > 0 {
        notes.push(format!(
            "rewrote {chains_to_match} if/elsif chain(s) as match"
        ));
    }

    if casts_removed > 0 || chains_to_match > 0 {
        // Only meaningful when the original compiles here at all; a file whose
        // imports don't resolve outside its app gets the detection rules alone.
        if compile_ir(source, opts).is_ok()
            && let Err(e) = compile_ir(&rewritten, opts)
        {
            return Err(format!(
                "lint bug: a rewrite broke compilation — refusing to produce output ({e})"
            ));
        }
    }
    if chains_to_match > 0 {
        // A structural check the compile gate can't make: the rewrite must
        // have turned exactly the chains we counted into matches, and left
        // every other `if` alone.
        verify_chain_counts(&stmts, &rewritten, chains_to_match)?;
    }

    let output = reindent(&rewritten)?;
    let reindented_lines = count_changed_lines(&rewritten, &output);
    Ok(LintOutcome {
        output,
        reindented_lines,
        casts_removed,
        chains_to_match,
        pre_format: rewritten,
        notes,
    })
}

/// Count `if` and `match` nodes before and after. Converting `n` chains must
/// remove exactly `n` `if` nodes (a chain's `elsif`s are `If` nodes too, so
/// the arms beyond the first come off as well) and add exactly `n` `match`
/// nodes. Anything else means a splice landed somewhere unintended, and the
/// linter refuses the file rather than writing it.
fn verify_chain_counts(
    before: &[crate::ast::Stmt],
    after: &str,
    chains: usize,
) -> Result<(), String> {
    let (_tree, after_stmts) = crate::rewrite::parse_ast(after)
        .map_err(|e| format!("lint bug: match rewrite no longer parses ({e})"))?;
    let (if_before, match_before) = count_nodes(before);
    let (if_after, match_after) = count_nodes(&after_stmts);
    if match_after != match_before + chains {
        return Err(format!(
            "lint bug: rewriting {chains} if-chain(s) produced {} new match(es) — \
             refusing to produce output",
            match_after.saturating_sub(match_before)
        ));
    }
    // Each converted chain contributes one `If` node per arm.
    if if_after >= if_before || if_before - if_after < chains {
        return Err(format!(
            "lint bug: rewriting {chains} if-chain(s) removed only {} if node(s) — \
             refusing to produce output",
            if_before.saturating_sub(if_after)
        ));
    }
    Ok(())
}

fn count_nodes(stmts: &[crate::ast::Stmt]) -> (usize, usize) {
    use crate::ast::{Expr, ExprKind, ExprVisitor, walk_expr};
    #[derive(Default)]
    struct Counter {
        ifs: usize,
        matches: usize,
    }
    impl ExprVisitor for Counter {
        fn visit_expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::If { .. } => self.ifs += 1,
                ExprKind::Match { .. } => self.matches += 1,
                _ => {}
            }
            walk_expr(self, e);
        }
    }
    let mut c = Counter::default();
    for s in stmts {
        c.visit_stmt(s);
    }
    (c.ifs, c.matches)
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
// `--verify`
// ---------------------------------------------------------------------------

/// How hard `--verify` insists on IR equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// The default. The formatting pass must leave the IR untouched; the two
    /// semantic passes (identity casts, `if`-chain to `match`) are *allowed*
    /// to change it, and the report says so, because "the IR differs" is the
    /// intended outcome of deleting a call or replacing a branch chain.
    Ir,
    /// The whole rewrite must be IR-equal to the original. A file with a
    /// semantic rewrite pending fails this, on purpose: it wants the run-diff
    /// verification of docs/dev/refactor-verification.md §5, not a write.
    Strict,
}

/// The verdict of a successful verification.
pub enum VerifyVerdict {
    /// Nothing to prove: the lint made no change to this file.
    Unchanged,
    /// The whole rewrite is IR-equal to the original. This is proof.
    Equal,
    /// The formatting pass was proven IR-equal, and the semantic passes
    /// changed the IR as designed. Not proof of behavior preservation — the
    /// caller should say so and, if it needs proof, run a run-diff.
    SemanticChange {
        /// The first difference between the original and the final text.
        diff: crate::ir_equiv::IrDiff,
        casts_removed: usize,
        chains_to_match: usize,
    },
}

/// A verification that could not be completed, or that failed.
pub struct VerifyFailure {
    pub message: String,
    pub diff: Option<crate::ir_equiv::IrDiff>,
}

impl VerifyFailure {
    fn msg(message: impl Into<String>) -> Self {
        VerifyFailure {
            message: message.into(),
            diff: None,
        }
    }
}

/// Prove (or refuse) a lint rewrite by comparing compiled IR.
///
/// (`VerifyFailure` carries a diff report, so it is large by nature; it is
/// produced once per file and never on a hot path.)
///
/// The caller must not write the file when this returns `Err`. See
/// [`VerifyMode`] for what each mode demands and
/// docs/dev/refactor-verification.md §7 for why this exists.
#[allow(clippy::result_large_err)]
pub fn verify_rewrite(
    source: &str,
    outcome: &LintOutcome,
    mode: VerifyMode,
    opts: &LintOptions,
) -> Result<VerifyVerdict, VerifyFailure> {
    use crate::ir_equiv::sources_equivalent;

    if !outcome.changed(source) {
        return Ok(VerifyVerdict::Unchanged);
    }
    let origin = opts.origin.as_deref();
    let whole = sources_equivalent(source, &outcome.output, &opts.include_dirs, origin)
        .map_err(VerifyFailure::msg)?;
    let diff = match whole {
        Ok(()) => return Ok(VerifyVerdict::Equal),
        Err(diff) => diff,
    };
    if mode == VerifyMode::Strict {
        return Err(VerifyFailure {
            message: "the rewrite is not IR-equal to the original".to_string(),
            diff: Some(diff),
        });
    }
    if !outcome.has_semantic_rewrite() {
        // Formatting alone must never move the IR — this is a linter bug.
        return Err(VerifyFailure {
            message: "lint bug: formatting alone changed the IR".to_string(),
            diff: Some(diff),
        });
    }
    // Semantic passes ran. Prove the part that is supposed to be inert: the
    // re-indentation applied on top of them.
    let formatting = sources_equivalent(
        &outcome.pre_format,
        &outcome.output,
        &opts.include_dirs,
        origin,
    )
    .map_err(VerifyFailure::msg)?;
    if let Err(fmt_diff) = formatting {
        return Err(VerifyFailure {
            message: "lint bug: re-indenting the rewritten source changed the IR".to_string(),
            diff: Some(fmt_diff),
        });
    }
    Ok(VerifyVerdict::SemanticChange {
        diff,
        casts_removed: outcome.casts_removed,
        chains_to_match: outcome.chains_to_match,
    })
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
            // which pins the formatting pass as semantics-free. Both semantic
            // rules change the IR on purpose — one deletes a call, the other
            // replaces an `if` chain with a `match` — so a file either of them
            // touched is exempt here and gated by compilation above instead.
            if outcome.casts_removed == 0 && outcome.chains_to_match == 0 {
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

    /// Reindentation must be *provably* semantics-free, over the whole repo
    /// corpus, using the real equivalence primitive rather than a JSON
    /// comparison of two serialized programs.
    ///
    /// The stimulus matters: every `.ptl` in the repo is already lint-clean,
    /// so `reindent(src) == src` and comparing those two proves nothing. Each
    /// file is therefore *mangled* first — three extra spaces on the front of
    /// every non-empty line — and the reindented mangled source is compared
    /// against the original. Files containing a multi-line token (raw string,
    /// JSX text) are skipped, because there the extra spaces would be content
    /// rather than layout and the mangle itself would change the program.
    #[test]
    fn reindent_is_ir_equal_over_repo_corpus() {
        use crate::ir_equiv::{ir_equivalent, sources_equivalent};

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root");
        let mut files = Vec::new();
        collect_ptl(repo_root, &mut files);
        files.sort();
        let mut checked = 0;
        let mut mangled_checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let opts = LintOptions {
                include_dirs: vec![],
                origin: Some(path.clone()),
            };
            // Only files that compile standalone can be compared at all.
            if compile_ir(&src, &opts).is_err() {
                continue;
            }
            let Ok(formatted) = reindent(&src) else {
                continue;
            };
            match sources_equivalent(&src, &formatted, &[], Some(path)) {
                Ok(Ok(())) => {}
                Ok(Err(diff)) => panic!("reindent changed IR for {}:\n{}", path.display(), diff),
                Err(e) => panic!("reindent broke compilation for {}: {}", path.display(), e),
            }
            checked += 1;

            let Some(mangled) = mangle_indentation(&src) else {
                continue;
            };
            let remangled = match reindent(&mangled) {
                Ok(t) => t,
                Err(e) => panic!("reindent failed on mangled {}: {}", path.display(), e),
            };
            assert_eq!(
                remangled,
                formatted,
                "reindent did not undo the mangle for {}",
                path.display()
            );
            match sources_equivalent(&src, &remangled, &[], Some(path)) {
                Ok(Ok(())) => {}
                Ok(Err(diff)) => panic!(
                    "reindent of mangled source changed IR for {}:\n{}",
                    path.display(),
                    diff
                ),
                Err(e) => panic!("mangled {} did not compile: {}", path.display(), e),
            }
            mangled_checked += 1;
        }
        assert!(checked > 150, "expected a real corpus, checked {checked}");
        assert!(
            mangled_checked > 150,
            "expected most of the corpus to be manglable, got {mangled_checked}"
        );

        // A program really is equivalent to itself, so the walk above can't be
        // passing by accident of never comparing anything.
        let src = "let x = 1\nprint(x)\n";
        let (env, pid) = crate::ir_equiv::compile_for_compare(src, &[], None).expect("compile");
        let program = env.get_program(pid).expect("program");
        assert!(ir_equivalent(program, program).is_ok());
    }

    /// Add three spaces to the front of every non-empty line. Returns `None`
    /// when the file has a token spanning more than one line (a raw string or
    /// JSX text), where leading whitespace is content rather than layout and
    /// the mangle itself would change the program.
    fn mangle_indentation(src: &str) -> Option<String> {
        let mut lexer = crate::lexer::Lexer::new(src);
        lexer.tokenize().ok()?;
        let multiline = lexer.tokens_with_spans().any(|(token, span)| {
            !matches!(
                token,
                crate::lexer::Token::Newline | crate::lexer::Token::Eof
            ) && span.end.line > span.start.line
        });
        if multiline {
            return None;
        }
        let mut out = String::with_capacity(src.len() + src.lines().count() * 3);
        for line in src.lines() {
            if !line.trim().is_empty() {
                out.push_str("   ");
            }
            out.push_str(line);
            out.push('\n');
        }
        Some(out)
    }
}
