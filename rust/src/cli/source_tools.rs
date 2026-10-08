//! `petal fmt`, `petal lint` and `petal apply-change`: the commands that
//! rewrite source files.
//!
//! `fmt` and `lint` take paths the way `gofmt` and `deno fmt` do — files, or directories
//! searched recursively for `.ptl` (skipping dot-directories, `node_modules`
//! and `target`), `.` when none is given, `-` for stdin — or inline code with
//! `-e`. A file that fails is reported and skipped; the others still run.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;

use super::{ApplyChangeArgs, LintArgs};

/// Exit code for "the rewrite could not be proven equivalent" — distinct from
/// the plain "problems found" exit 1.
const VERIFY_FAILED_EXIT: i32 = 3;

/// One input: a path on disk, or stdin / `-e` text (named for messages).
struct Input {
    name: String,
    path: Option<PathBuf>,
    source: String,
}

/// Expand the command-line paths into inputs. Unreadable paths are reported
/// and counted in `errors`.
fn collect_inputs(paths: &[String], inline: Option<&str>, errors: &mut usize) -> Vec<Input> {
    if let Some(code) = inline {
        return vec![Input {
            name: "<inline>".to_string(),
            path: None,
            source: code.to_string(),
        }];
    }
    let default = [".".to_string()];
    let paths = if paths.is_empty() {
        &default[..]
    } else {
        paths
    };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut inputs = Vec::new();
    for p in paths {
        if p == "-" {
            let mut source = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut source) {
                eprintln!("error: reading stdin: {e}");
                *errors += 1;
            }
            inputs.push(Input {
                name: "<stdin>".to_string(),
                path: None,
                source,
            });
            continue;
        }
        let path = Path::new(p);
        if path.is_dir() {
            files.extend(crate::apply_change::ptl_files_under(path));
        } else if path.exists() {
            files.push(path.to_path_buf());
        } else {
            eprintln!("error: {p}: no such file or directory");
            *errors += 1;
        }
    }
    for file in files {
        match fs::read_to_string(&file) {
            Ok(source) => inputs.push(Input {
                name: file.display().to_string(),
                path: Some(file),
                source,
            }),
            Err(e) => {
                eprintln!("error: {}: {e}", file.display());
                *errors += 1;
            }
        }
    }
    inputs
}

// ---------------------------------------------------------------------------
// fmt
// ---------------------------------------------------------------------------

pub(super) fn handle_fmt(paths: &[String], inline: Option<&str>, check: bool, diff: bool) {
    let mut errors = 0usize;
    let inputs = collect_inputs(paths, inline, &mut errors);
    let mut unformatted = 0usize;
    let mut written = 0usize;

    for input in &inputs {
        let formatted = match crate::fmt::format_source(&input.source) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("error: {}: {}", input.name, e);
                errors += 1;
                continue;
            }
        };
        // stdin and `-e` have nowhere to be written back: print the result.
        let Some(path) = &input.path else {
            if check || diff {
                if formatted != input.source {
                    unformatted += 1;
                    if diff {
                        print!("{}", unified_diff(&input.name, &input.source, &formatted, "formatted"));
                    } else {
                        println!("{}", input.name);
                    }
                }
            } else {
                print!("{formatted}");
            }
            continue;
        };
        if formatted == input.source {
            continue;
        }
        unformatted += 1;
        if diff {
            print!("{}", unified_diff(&input.name, &input.source, &formatted, "formatted"));
        }
        if check {
            if !diff {
                println!("{}", input.name);
            }
        } else if !diff {
            if let Err(e) = fs::write(path, &formatted) {
                eprintln!("error: writing {}: {e}", input.name);
                errors += 1;
                continue;
            }
            written += 1;
        }
    }

    let files = inputs.iter().filter(|i| i.path.is_some()).count();
    if check || diff {
        if files > 0 {
            eprintln!(
                "{unformatted} of {files} file(s) need formatting{}",
                if unformatted > 0 {
                    " (run `petal fmt`)"
                } else {
                    ""
                }
            );
        }
        if unformatted > 0 || errors > 0 {
            process::exit(1);
        }
    } else if files > 0 {
        eprintln!("formatted {written} of {files} file(s)");
    }
    if errors > 0 {
        process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

pub(super) fn handle_lint(opts: &LintArgs, include_dirs: &[PathBuf]) {
    if opts.list_rules {
        let width = crate::lint::RULES
            .iter()
            .map(|r| r.name.len())
            .max()
            .unwrap_or(0);
        for rule in crate::lint::RULES {
            let on = if opts.rules.enabled(rule.name) {
                ""
            } else {
                " (disabled)"
            };
            println!("{:width$}  {}{}", rule.name, rule.summary, on);
        }
        return;
    }

    let mut errors = 0usize;
    let inputs = collect_inputs(&opts.paths, opts.inline.as_deref(), &mut errors);
    let mut json_rows: Vec<serde_json::Value> = Vec::new();
    let mut remaining = 0usize;
    let mut fixed = 0usize;
    let mut files_with_findings = 0usize;

    for input in &inputs {
        let lint_opts = crate::lint::LintOptions {
            include_dirs: include_dirs.to_vec(),
            origin: input.path.clone(),
            rules: opts.rules.clone(),
        };
        let outcome = match crate::lint::lint_source(&input.source, &lint_opts) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("error: {}: {}", input.name, e);
                errors += 1;
                continue;
            }
        };
        if outcome.findings.is_empty() {
            continue;
        }
        files_with_findings += 1;

        // Verification runs before any write, so `--verify` with and without
        // `--fix` agree on whether the rewrite is acceptable at all.
        if let Some(mode) = opts.verify {
            verify(mode, &input.name, &input.source, &outcome, &lint_opts);
        }

        for f in &outcome.findings {
            if opts.json {
                json_rows.push(serde_json::json!({
                    "file": input.name,
                    "line": f.line,
                    "column": f.column,
                    "rule": f.rule,
                    "message": f.message,
                    "fixed": opts.fix,
                }));
            } else if !opts.fix {
                println!(
                    "{}:{}:{}: {}: {}",
                    input.name, f.line, f.column, f.rule, f.message
                );
            }
        }

        if !opts.fix {
            remaining += outcome.findings.len();
            continue;
        }
        match &input.path {
            None => print!("{}", outcome.output),
            Some(path) => {
                if let Err(e) = fs::write(path, &outcome.output) {
                    eprintln!("error: writing {}: {e}", input.name);
                    errors += 1;
                    continue;
                }
                if !opts.json {
                    println!("fixed {}: {}", input.name, summarize(&outcome));
                }
            }
        }
        fixed += outcome.findings.len();
    }

    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json_rows).unwrap_or_else(|_| "[]".into())
        );
    } else if remaining > 0 {
        eprintln!(
            "found {remaining} problem(s) in {files_with_findings} file(s); \
             run `petal lint --fix` to fix them"
        );
    } else if fixed > 0 {
        eprintln!("fixed {fixed} problem(s) in {files_with_findings} file(s)");
    }
    if remaining > 0 || errors > 0 {
        process::exit(1);
    }
}

/// `3 prefer-let, 1 no-redundant-cast`.
fn summarize(outcome: &crate::lint::LintOutcome) -> String {
    crate::lint::RULES
        .iter()
        .filter_map(|r| {
            let n = outcome.count(r.name);
            (n > 0).then(|| format!("{n} {}", r.name))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Run `--verify` over a computed rewrite. Returns only when the file may be
/// written; otherwise reports the diff and exits [`VERIFY_FAILED_EXIT`].
fn verify(
    mode: crate::lint::VerifyMode,
    name: &str,
    source: &str,
    outcome: &crate::lint::LintOutcome,
    opts: &crate::lint::LintOptions,
) {
    use crate::lint::VerifyVerdict;
    match crate::lint::verify_rewrite(source, outcome, mode, opts) {
        Ok(VerifyVerdict::Unchanged) => {}
        Ok(VerifyVerdict::Equal) => {
            eprintln!("verify: {name}: IR unchanged — the rewrite is provably equivalent");
        }
        Ok(VerifyVerdict::SemanticChange { diff }) => {
            // Expected, not a failure: these rules exist to change the IR.
            // Say so plainly rather than dressing it up as a proof.
            eprintln!(
                "verify: {name}: rewrite changed IR ({}); formatting was proven IR-equal. \
                 First difference:\n{}\n\
                 verify: run-diff verification needed for the semantic rules \
                 (docs/dev/refactor-verification.md §5); use --verify=strict to refuse this file.",
                summarize(outcome),
                diff
            );
        }
        Err(failure) => {
            eprintln!("verify: {name}: {}", failure.message);
            if let Some(diff) = &failure.diff {
                eprintln!("{}", diff);
            }
            eprintln!("verify: refusing to write.");
            process::exit(VERIFY_FAILED_EXIT);
        }
    }
}

// ---------------------------------------------------------------------------
// apply-change
// ---------------------------------------------------------------------------

/// `petal apply-change <operation> <file> …` — plan the change, report it,
/// and write it unless `--dry-run`. The plan is already proven when it comes
/// back (`crate::apply_change::finish`), so there is nothing left to refuse
/// here: exit 0 means every file was written (or, under `--dry-run`, would
/// be), and exit 1 means nothing was touched.
pub(super) fn handle_apply_change(args: &ApplyChangeArgs, include_dirs: &[PathBuf]) {
    use crate::apply_change::{self, CONVERT_TO_VAR, ChangeOptions, OPERATIONS, Request};

    let Some(name) = args.operation.as_deref() else {
        println!("usage: petal apply-change <operation> <file> [<options>]\n\noperations:");
        for op in OPERATIONS {
            println!("   {:<16} {}", op.name, op.summary);
        }
        println!("\nSee 'petal help apply-change'.");
        return;
    };
    let Some(op) = apply_change::operation(name) else {
        eprintln!(
            "Unknown operation '{name}' (expected {}). See 'petal help apply-change'.",
            OPERATIONS
                .iter()
                .map(|op| format!("'{}'", op.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
        process::exit(1);
    };
    let usage = || -> ! {
        eprintln!("Usage: {}", op.usage);
        process::exit(1);
    };
    let Some(file) = args.file.as_deref() else {
        usage();
    };
    let request = match op.name {
        CONVERT_TO_VAR => match &args.target {
            Some(target) => Request::ConvertToVar {
                target: target.clone(),
            },
            None => {
                eprintln!("{name} needs --target <path>: which binding to convert");
                usage();
            }
        },
        _ => unreachable!("every operation in OPERATIONS is handled"),
    };

    let opts = ChangeOptions {
        include_dirs: include_dirs.to_vec(),
        from: args.from.clone(),
        host: args.host,
    };
    let plan = match apply_change::plan(Path::new(file), &request, &opts) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("{name}: {e}");
            process::exit(1);
        }
    };

    // Importers are found by canonical path. Name them the way the target
    // was named where that is possible: relative to the working directory.
    let cwd = std::env::current_dir()
        .ok()
        .map(|d| crate::module::canonical_path(&d));
    let shown = |path: &Path| -> String {
        cwd.as_deref()
            .and_then(|d| path.strip_prefix(d).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    };

    // The diff is the dry run's product, so it alone goes to stdout; the
    // account of what was done goes to stderr either way.
    eprintln!("{name}: {}", plan.summary);
    for f in &plan.files {
        eprintln!("  {}: {}", shown(&f.path), f.detail);
    }
    match &plan.importers {
        None => eprintln!("  importers: none considered, the binding is not exported"),
        Some(report) => {
            eprintln!(
                "  importers: {} found, {}",
                report.importers.len(),
                report.how
            );
            for i in &report.importers {
                eprintln!("    {} ({}): {}", shown(&i.path), i.form, i.detail);
            }
        }
    }
    for note in &plan.notes {
        eprintln!("  note: {note}");
    }

    if args.dry_run {
        for f in &plan.files {
            print!(
                "{}",
                unified_diff(&shown(&f.path), &f.before, &f.after, name)
            );
        }
        eprintln!(
            "dry run: {} file(s) would change; nothing was written",
            plan.files.len()
        );
        return;
    }
    if let Err(e) = plan.write() {
        eprintln!("{name}: {e}");
        process::exit(1);
    }
    eprintln!("wrote {} file(s)", plan.files.len());
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// A unified diff of two texts (`gofmt -d`), 3 lines of context. `label`
/// says what the second text is (`formatted`).
fn unified_diff(name: &str, a: &str, b: &str, label: &str) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let ops = myers(&a, &b);
    const CONTEXT: usize = 3;

    let mut out = format!("--- {name}\n+++ {name} ({label})\n");
    // Each change wants CONTEXT lines either side; overlapping windows merge.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (c, op) in ops.iter().enumerate() {
        if matches!(op, Op::Equal(..)) {
            continue;
        }
        let (s, e) = (c.saturating_sub(CONTEXT), (c + CONTEXT + 1).min(ops.len()));
        match hunks.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => hunks.push((s, e)),
        }
    }
    for (start, end) in hunks {
        let (a0, b0) = ops[start].positions();
        let a_len = ops[start..end]
            .iter()
            .filter(|o| !matches!(o, Op::Insert(..)))
            .count();
        let b_len = ops[start..end]
            .iter()
            .filter(|o| !matches!(o, Op::Delete(..)))
            .count();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a0 + 1,
            a_len,
            b0 + 1,
            b_len
        ));
        for op in &ops[start..end] {
            match *op {
                Op::Equal(ai, _) => out.push_str(&format!(" {}\n", a[ai])),
                Op::Delete(ai, _) => out.push_str(&format!("-{}\n", a[ai])),
                Op::Insert(_, bi) => out.push_str(&format!("+{}\n", b[bi])),
            }
        }
    }
    out
}

/// One line of an edit script, carrying the (a, b) line positions it sits at.
#[derive(Clone, Copy)]
enum Op {
    Equal(usize, usize),
    Delete(usize, usize),
    Insert(usize, usize),
}

impl Op {
    fn positions(self) -> (usize, usize) {
        match self {
            Op::Equal(a, b) | Op::Delete(a, b) | Op::Insert(a, b) => (a, b),
        }
    }
}

/// Myers' O(ND) shortest edit script. Formatting diffs are small, so D is too.
fn myers(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let offset = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'outer: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                break 'outer;
            }
            k += 2;
        }
    }
    // Walk the trace back from (n, m).
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let idx = (k + offset) as usize;
        let prev_k = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 {
            0
        } else {
            v[(prev_k + offset) as usize]
        };
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(Op::Equal(x as usize, y as usize));
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                ops.push(Op::Insert(x as usize, y as usize));
            } else {
                x -= 1;
                ops.push(Op::Delete(x as usize, y as usize));
            }
        }
    }
    ops.reverse();
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_shows_changed_lines_with_context() {
        let a = "a\nb\nc\nd\n";
        let b = "a\nB\nc\nd\n";
        assert_eq!(
            unified_diff("f", a, b, "formatted"),
            "--- f\n+++ f (formatted)\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n d\n"
        );
    }

    #[test]
    fn diff_of_equal_texts_is_just_the_header() {
        assert_eq!(
            unified_diff("f", "x\n", "x\n", "formatted"),
            "--- f\n+++ f (formatted)\n"
        );
    }

    #[test]
    fn diff_handles_deleted_blank_lines() {
        let a = "a\n\n\nb\n";
        let b = "a\n\nb\n";
        let d = unified_diff("f", a, b, "formatted");
        assert!(d.contains("-\n"), "{d}");
        assert_eq!(d.matches("\n-").count(), 1, "{d}");
    }
}
