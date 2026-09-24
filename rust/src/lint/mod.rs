//! `petal lint` — rules about *which* code to write, as opposed to how to lay
//! it out (that is `petal fmt`, [`crate::fmt`]; see docs/dev/linter-plan.md).
//!
//! Every rule has a kebab-case name, reports a [`Finding`] per site, and
//! carries a fix that `--fix` applies. The rules, in the order they run:
//!
//! 1. **`prefer-let`** ([`var_to_let`]) — a `var` that no nested function
//!    mentions is a `let` in disguise: `var` → `let`, `set x` → `x`, `get x`
//!    → `x`. Keyword splices only, so the program keeps its shape. This runs
//!    first because it gives the cast rule types to prove.
//! 2. **`no-redundant-cast`** ([`casts`]) — delete `int(n)` where `n` is
//!    already an `int` (likewise `float`/`str`). Candidates come from the type
//!    checker, which is deliberately conservative — anything it cannot prove
//!    infers `any` and is left alone — and are applied as two minimal string
//!    splices per cast, so comments and layout inside the argument survive.
//! 3. **`prefer-match`** ([`to_match`]) — an `if`/`elsif` chain that tests one
//!    subject against string/bool/nil literals becomes a `match`. The splices
//!    only ever cover the glue between the arms, so every pattern and body
//!    survives verbatim.
//! 4. **`prefer-compound-assign`** ([`compound`]) — `x = x + e` → `x += e`.
//!    The parser desugars the compound form back to the long one, so this
//!    rule is IR-invisible.
//!
//! Each rule detects over a fresh parse of the previous rule's output, so a
//! later rule sees what an earlier fix enabled and `lint --fix` is a fixed
//! point. Findings are reported at their position in the *original* text:
//! every applied splice is recorded, and a later stage's offsets are mapped
//! back through them ([`map_back`]).
//!
//! **Opting out.** `// petal-lint-ignore <rule> [<rule>…]` on the line before a
//! finding, or trailing on its line, silences those rules there (no names
//! silences every rule); `// petal-lint-ignore-file [<rule>…]` does the same
//! for the whole file. Text after `--` is a reason and is ignored. A
//! suppressed finding's fix is not applied either.
//!
//! **Gates.** The fixes change tokens, so [`lint_source`] gates them: if the
//! original source compiles, the fixed source must compile too, or lint
//! refuses to produce output. That is weaker than full IR equality — removing
//! a call *does* change the IR, which is the point — so the real guarantee
//! comes from each detection rule being an identity (`int()` on an `int` is
//! the identity, `rust/src/builtins/math.rs`). [`verify_rewrite`]
//! (`petal lint --fix --verify`) compiles both sides and compares their IR
//! with [`crate::ir_equiv::ir_equivalent`]; see
//! docs/dev/refactor-verification.md §7.
//!
//! A fixed file is run through `petal fmt` before it is written: a rewritten
//! chain has to be re-indented, and fmt is proven IR-invisible.
//!
//! A note on what is *not* here: an earlier slice rewrote `x = f(x)` to the
//! rebind form `f(@x)`. That rule is gone. The `@` operator remains a language
//! feature, but it reads as sugar that has to be learned, so the linter no
//! longer pushes code into it.

use std::path::PathBuf;

mod casts;
mod compound;
mod to_match;
mod var_to_let;

use casts::plan_cast_fixes;
use compound::plan_compound_fixes;
use to_match::{Splice, apply_match_edits, plan_match_fixes};
use var_to_let::plan_var_fixes;

pub use crate::fmt::reindent;

/// Directive comments.
pub const IGNORE_NEXT: &str = "petal-lint-ignore";
pub const IGNORE_FILE: &str = "petal-lint-ignore-file";

/// One lint rule, as `petal lint --rules` lists it.
pub struct RuleInfo {
    pub name: &'static str,
    pub summary: &'static str,
}

pub const PREFER_LET: &str = "prefer-let";
pub const NO_REDUNDANT_CAST: &str = "no-redundant-cast";
pub const PREFER_MATCH: &str = "prefer-match";
pub const PREFER_COMPOUND_ASSIGN: &str = "prefer-compound-assign";

/// Every rule, in the order it runs.
pub const RULES: &[RuleInfo] = &[
    RuleInfo {
        name: PREFER_LET,
        summary: "a `var` no nested function shares should be a `let`",
    },
    RuleInfo {
        name: NO_REDUNDANT_CAST,
        summary: "`int(n)` / `float(x)` / `str(s)` on a value already of that type",
    },
    RuleInfo {
        name: PREFER_MATCH,
        summary: "an if/elsif chain testing one value against literals should be a `match`",
    },
    RuleInfo {
        name: PREFER_COMPOUND_ASSIGN,
        summary: "`x = x + e` should be `x += e`",
    },
];

pub fn is_rule(name: &str) -> bool {
    RULES.iter().any(|r| r.name == name)
}

/// Which rules run (`--rules-include` / `--rules-exclude`).
#[derive(Default, Clone)]
pub struct RuleFilter {
    /// When set, only these rules run.
    pub include: Option<Vec<String>>,
    pub exclude: Vec<String>,
}

impl RuleFilter {
    pub fn enabled(&self, rule: &str) -> bool {
        self.include
            .as_ref()
            .is_none_or(|inc| inc.iter().any(|r| r == rule))
            && !self.exclude.iter().any(|r| r == rule)
    }
}

/// Context the compile gate needs to compile the source the same way
/// `petal run` would: module search dirs and the file's own path (imports
/// resolve relative to it).
#[derive(Default)]
pub struct LintOptions {
    pub include_dirs: Vec<PathBuf>,
    pub origin: Option<PathBuf>,
    pub rules: RuleFilter,
}

/// One rule firing at one place.
#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: &'static str,
    /// 1-based position in the original source.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// A rule's planned fix for one site, in the coordinates of the text the rule
/// ran on: where to report it, and the splices that fix it.
struct Fix {
    anchor: usize,
    message: String,
    splices: Vec<Splice>,
}

#[cfg(test)]
fn flatten(fixes: Vec<Fix>) -> Vec<Splice> {
    let mut all: Vec<Splice> = fixes.into_iter().flat_map(|f| f.splices).collect();
    all.sort_by_key(|s| s.start);
    all
}

/// The result of linting one source text.
pub struct LintOutcome {
    /// Every finding, in source order.
    pub findings: Vec<Finding>,
    /// The source with every finding fixed, then formatted. Equal to the input
    /// when there are no findings.
    pub output: String,
    /// The fixed text *before* formatting. `--verify` compares this against
    /// [`LintOutcome::output`] to prove the formatting on its own, which is
    /// the only part of the rewrite that is supposed to leave the IR alone.
    pub pre_format: String,
}

impl LintOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }

    pub fn count(&self, rule: &str) -> usize {
        self.findings.iter().filter(|f| f.rule == rule).count()
    }

    /// Did a rule that is *expected* to change the IR fire on this file?
    pub fn has_semantic_rewrite(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.rule != PREFER_COMPOUND_ASSIGN)
    }
}

/// The source as it moves through the rules: the current text, and the
/// splices each earlier stage applied (to map a position back to the
/// original).
struct Pipeline<'a> {
    line_starts: Vec<usize>,
    ignores: Ignores,
    rules: &'a RuleFilter,
    text: String,
    stages: Vec<Vec<Splice>>,
    findings: Vec<Finding>,
}

impl<'a> Pipeline<'a> {
    fn new(source: &'a str, rules: &'a RuleFilter) -> Result<Self, String> {
        let mut line_starts = vec![0usize];
        for (i, c) in source.chars().enumerate() {
            if c == '\n' {
                line_starts.push(i + 1);
            }
        }
        Ok(Pipeline {
            line_starts,
            ignores: Ignores::scan(source)?,
            rules,
            text: source.to_string(),
            stages: Vec::new(),
            findings: Vec::new(),
        })
    }

    /// Keep the fixes `rule` is allowed to make, record their findings, and
    /// apply them. Returns how many were applied.
    fn apply(&mut self, rule: &'static str, fixes: Vec<Fix>) -> usize {
        let mut kept: Vec<Splice> = Vec::new();
        let mut count = 0;
        if self.rules.enabled(rule) {
            for fix in fixes {
                let mut at = fix.anchor;
                for stage in self.stages.iter().rev() {
                    at = map_back(at, stage);
                }
                let line = self.line_starts.partition_point(|&s| s <= at) - 1;
                if self.ignores.suppresses(rule, line) {
                    continue;
                }
                self.findings.push(Finding {
                    rule,
                    line: line + 1,
                    column: at - self.line_starts[line] + 1,
                    message: fix.message,
                });
                kept.extend(fix.splices);
                count += 1;
            }
        }
        kept.sort_by_key(|s| s.start);
        if !kept.is_empty() {
            let chars: Vec<char> = self.text.chars().collect();
            self.text = apply_match_edits(&chars, &kept);
        }
        self.stages.push(kept);
        count
    }
}

/// Map char offset `pos` in the text *after* `splices` were applied back to
/// the text before. A position inside inserted text, or at a deletion, maps to
/// the splice start.
fn map_back(pos: usize, splices: &[Splice]) -> usize {
    let mut delta: isize = 0;
    for s in splices {
        let new_start = (s.start as isize + delta) as usize;
        if pos < new_start {
            break;
        }
        let inserted = s.text.chars().count();
        // Inside the replacement — or exactly at a deletion, where the text
        // that follows used to start after the deleted run: either way the
        // position belongs to where the splice began.
        if pos < new_start + inserted.max(1) {
            return s.start;
        }
        delta += inserted as isize - (s.end - s.start) as isize;
    }
    (pos as isize - delta) as usize
}

/// `// petal-lint-ignore` comments: per 0-based line, the rules silenced there
/// (an empty list silences every rule).
struct Ignores {
    file: Option<Vec<String>>,
    lines: std::collections::HashMap<usize, Vec<String>>,
}

impl Ignores {
    fn scan(source: &str) -> Result<Self, String> {
        let mut file = None;
        let mut lines = std::collections::HashMap::new();
        for (line, text, whole_line) in crate::fmt::line_comments(source)? {
            let Some(word) = crate::fmt::directive_word(&text) else {
                continue;
            };
            let rest = text.trim_start_matches('/').trim_start()[word.len()..]
                .split("--")
                .next()
                .unwrap_or("");
            let rules: Vec<String> = rest
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect();
            if word == IGNORE_FILE {
                file = Some(rules);
            } else if word == IGNORE_NEXT {
                // Alone on a line it covers the next line; trailing, its own.
                let target = if whole_line { line + 1 } else { line };
                lines.insert(target, rules);
            }
        }
        Ok(Ignores { file, lines })
    }

    fn suppresses(&self, rule: &str, line: usize) -> bool {
        let hit = |rules: &Vec<String>| rules.is_empty() || rules.iter().any(|r| r == rule);
        self.file.as_ref().is_some_and(hit) || self.lines.get(&line).is_some_and(hit)
    }
}

/// Run every enabled rule over `source`, collecting findings and building the
/// fixed text. Errors if the source doesn't parse, or if a fix fails a gate
/// (which indicates a lint bug and refuses all output).
pub fn lint_source(source: &str, opts: &LintOptions) -> Result<LintOutcome, String> {
    // Lint operates on valid programs only.
    let (chars, stmts) = reparse(source)?;
    let mut p = Pipeline::new(source, &opts.rules)?;

    // `prefer-let` runs first: a `var` reads as `any` to the type checker,
    // so a cast on one only becomes provably redundant once it is a `let`.
    // Running it after the cast rule would leave those casts for a second
    // `lint` to find, and lint has to be a fixed point.
    let vars_to_let = p.apply(PREFER_LET, plan_var_fixes(&stmts, &chars));

    // Lint sees no `class` declarations of its own: identity-cast detection
    // never consults one, and the built-in table is what resolves `Rect`.
    let (chars, stmts) = reparse(&p.text)?;
    let classes = crate::classes::ClassTable::new();
    let signatures = crate::compiler::collect_fn_signatures(&stmts, &classes);
    let found = crate::typecheck::find_redundant_casts(&stmts, &signatures, &classes);
    let casts_removed = p.apply(NO_REDUNDANT_CAST, plan_cast_fixes(&found, &chars));

    // The cast splices moved every offset, so each later rule re-parses.
    let (chars, match_stmts) = reparse(&p.text)?;
    let chains_to_match = p.apply(PREFER_MATCH, plan_match_fixes(&match_stmts, &chars));

    let (chars, stmts) = reparse(&p.text)?;
    let compound_assigns = p.apply(PREFER_COMPOUND_ASSIGN, plan_compound_fixes(&stmts, &chars));

    let Pipeline {
        text: rewritten,
        mut findings,
        ..
    } = p;
    findings.sort_by_key(|f| (f.line, f.column));
    if findings.is_empty() {
        return Ok(LintOutcome {
            findings,
            output: source.to_string(),
            pre_format: source.to_string(),
        });
    }

    if casts_removed > 0 || chains_to_match > 0 || vars_to_let > 0 || compound_assigns > 0 {
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
        verify_chain_counts(&match_stmts, &rewritten, chains_to_match)?;
    }
    if vars_to_let > 0 {
        // Same idea for the `var` rule: exactly the counted `var`s are gone.
        let before = count_vars(&crate::rewrite::parse_ast(source)?.1);
        let after = count_vars(&reparse(&rewritten)?.1);
        if before != after + vars_to_let {
            return Err(format!(
                "lint bug: converting {vars_to_let} var(s) to let left {after} of {before} — \
                 refusing to produce output"
            ));
        }
    }

    let output = crate::fmt::format_source(&rewritten)?;
    Ok(LintOutcome {
        findings,
        output,
        pre_format: rewritten,
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

fn reparse(source: &str) -> Result<(Vec<char>, Vec<crate::ast::Stmt>), String> {
    let (_tree, stmts) = crate::rewrite::parse_ast(source)?;
    Ok((source.chars().collect(), stmts))
}

/// The number of `var` declarations (not `state var`) anywhere in `stmts`.
fn count_vars(stmts: &[crate::ast::Stmt]) -> usize {
    use crate::ast::{ExprVisitor, Stmt, StmtKind, walk_stmt};
    struct Counter(usize);
    impl ExprVisitor for Counter {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::Let { is_var: true, .. } = s.kind {
                self.0 += 1;
            }
            walk_stmt(self, s);
        }
    }
    let mut c = Counter(0);
    for s in stmts {
        c.visit_stmt(s);
    }
    c.0
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

// ---------------------------------------------------------------------------
// `--verify`
// ---------------------------------------------------------------------------

/// How hard `--verify` insists on IR equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// The default. The formatting pass must leave the IR untouched; the
    /// semantic passes (identity casts, `if`-chain to `match`, `var` to
    /// `let`) are *allowed*
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
        // Only the IR-invisible rules fired, so the IR must not move.
        return Err(VerifyFailure {
            message: "lint bug: an IR-invisible rewrite changed the IR".to_string(),
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
            message: "lint bug: formatting the rewritten source changed the IR".to_string(),
            diff: Some(fmt_diff),
        });
    }
    Ok(VerifyVerdict::SemanticChange { diff })
}

/// Compile `source` and return its entry program's serialized IR, minus the
/// source text and source map (whitespace edits move spans). Used as a
/// does-it-still-compile gate, and by the corpus test to compare programs.
fn compile_ir(source: &str, opts: &LintOptions) -> Result<serde_json::Value, String> {
    let (env, pid) =
        crate::ir_equiv::compile_for_compare(source, &opts.include_dirs, opts.origin.as_deref())?;
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

    /// The linter-plan safeguard, as a property test over the whole repo
    /// corpus: every program that compiles must still compile after linting,
    /// and linting must be a fixed point. (A program with no casts to remove
    /// gets formatting only, which cannot change semantics at all.)
    #[test]
    fn lint_preserves_compilation_over_repo_corpus() {
        let files = crate::test_corpus::repo_ptl_files();
        let mut checked = 0;
        for path in &files {
            let Ok(src) = std::fs::read_to_string(path) else {
                continue;
            };
            let opts = LintOptions {
                include_dirs: vec![],
                origin: Some(path.clone()),
                ..Default::default()
            };
            let Ok(outcome) = lint_source(&src, &opts) else {
                continue;
            };
            let Ok(src_ir) = compile_ir(&src, &opts) else {
                continue; // doesn't compile standalone; nothing to compare
            };
            let out_ir = match compile_ir(&outcome.output, &opts) {
                Ok(ir) => ir,
                Err(e) => panic!("lint broke compilation for {}: {}", path.display(), e),
            };
            // A file only the IR-invisible rules touched must be
            // byte-identical in IR. The semantic rules change the IR on
            // purpose — deleting a call, replacing an `if` chain with a
            // `match`, a cell with a rebind — so a file one of them touched
            // is gated by compilation above instead.
            if !outcome.has_semantic_rewrite() {
                assert_eq!(
                    src_ir,
                    out_ir,
                    "IR-invisible lint changed IR for {}",
                    path.display()
                );
            }
            // Linting the fixed text must find nothing: `--fix` is a fixed
            // point.
            let again = lint_source(&outcome.output, &opts).expect("relint");
            assert!(
                again.findings.is_empty(),
                "lint --fix not a fixed point for {}: {:?}",
                path.display(),
                again.findings
            );
            checked += 1;
        }
        assert!(checked > 50, "expected a real corpus, checked {checked}");
    }

    #[test]
    fn map_back_through_splices() {
        // "var x" -> "let x"; "set x = 1" -> "x = 1" (5 chars dropped at 6).
        let splices = vec![
            Splice {
                start: 0,
                end: 3,
                text: "let".into(),
            },
            Splice {
                start: 6,
                end: 10,
                text: String::new(),
            },
        ];
        assert_eq!(map_back(0, &splices), 0);
        assert_eq!(map_back(4, &splices), 4);
        assert_eq!(map_back(6, &splices), 6);
        assert_eq!(map_back(7, &splices), 11);
        assert_eq!(map_back(8, &splices), 12);
    }

    #[test]
    fn findings_carry_rule_and_original_position() {
        let src = "fn f()\n  var n = 0\n  set n = get n + 1\n  n\nend\n";
        let out = lint_source(src, &LintOptions::default()).unwrap();
        let rules: Vec<_> = out
            .findings
            .iter()
            .map(|f| (f.rule, f.line, f.column))
            .collect();
        assert_eq!(
            rules,
            vec![(PREFER_LET, 2, 3), (PREFER_COMPOUND_ASSIGN, 3, 3)]
        );
        assert_eq!(out.output, "fn f()\n  let n = 0\n  n += 1\n  n\nend\n");
    }

    #[test]
    fn ignore_comments_suppress_findings_and_their_fixes() {
        let src = "fn f()\n  // petal-lint-ignore prefer-let -- shared later\n  var n = 0\n  set n = get n + 1\n  get n\nend\n";
        let out = lint_source(src, &LintOptions::default()).unwrap();
        assert!(out.findings.iter().all(|f| f.rule != PREFER_LET));
        assert!(out.output.contains("var n = 0"));

        let trailing = "let x = 1\nx = x + 1 // petal-lint-ignore\n";
        assert!(
            lint_source(trailing, &LintOptions::default())
                .unwrap()
                .findings
                .is_empty()
        );

        let file = "// petal-lint-ignore-file prefer-compound-assign\nlet x = 1\nx = x + 1\n";
        assert!(
            lint_source(file, &LintOptions::default())
                .unwrap()
                .findings
                .is_empty()
        );
    }

    #[test]
    fn rule_filter_selects_rules() {
        let src = "fn f()\n  var n = 0\n  set n = get n + 1\n  n\nend\n";
        let opts = LintOptions {
            rules: RuleFilter {
                include: None,
                exclude: vec![PREFER_COMPOUND_ASSIGN.to_string()],
            },
            ..Default::default()
        };
        let out = lint_source(src, &opts).unwrap();
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.findings[0].rule, PREFER_LET);
    }
}
