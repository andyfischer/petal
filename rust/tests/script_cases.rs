//! Script-based regression tests.
//!
//! Each directory under the repo-root `test/` holds a runnable program
//! (`main.ptl`) and an `expects` file describing what running it should produce:
//! the exact console output, plus *ceilings* on the value-duplication metrics
//! (see `src/stats.rs`). The output check guards behavior; the metric ceilings
//! guard performance — they catch regressions where we start copying immutable
//! values we did not have to.
//!
//! ## `expects` format (plain text, one directive per line)
//!
//! ```text
//! # lines starting with '#' are comments; blank lines are ignored
//! out: <expected stdout line>      # one per print(), in order, matched exactly
//! max dup.<kind>.<metric>: <N>     # run must NOT exceed N
//! ```
//!
//! `<kind>` is one of `list`, `map`, `f64array`, `fork`, `total`; `<metric>` is
//! `count` or `bytes`. Metric ceilings are only enforced when duplication stats
//! are compiled in (debug builds, which `cargo test` is — see
//! `petal::stats::DUP_STATS_ENABLED`).

use std::fs;
use std::path::{Path, PathBuf};

use petal::env::Env;
use petal::stats::{DupKind, DupStats, DUP_STATS_ENABLED};

/// One `max dup.<kind>.<metric>` ceiling parsed from an `expects` file.
struct MetricCeiling {
    kind: String,
    metric: String,
    max: u64,
}

/// A parsed `expects` file.
struct Expectations {
    /// Expected stdout, one entry per `out:` line, matched in order.
    output: Vec<String>,
    ceilings: Vec<MetricCeiling>,
}

fn parse_expects(text: &str, case: &str) -> Expectations {
    let mut output = Vec::new();
    let mut ceilings = Vec::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("out:") {
            // Keep the content verbatim, dropping exactly one optional space
            // after the colon so `out: x` means "x" and `out:` means "".
            output.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if let Some(rest) = trimmed.strip_prefix("max ") {
            let (key, value) = rest.split_once(':').unwrap_or_else(|| {
                panic!("{case}/expects:{}: `max` line needs `key: value`: {line:?}", lineno + 1)
            });
            let parts: Vec<&str> = key.trim().split('.').collect();
            if parts.len() != 3 || parts[0] != "dup" {
                panic!(
                    "{case}/expects:{}: metric key must be `dup.<kind>.<metric>`, got {:?}",
                    lineno + 1,
                    key.trim()
                );
            }
            let max = value.trim().parse::<u64>().unwrap_or_else(|_| {
                panic!("{case}/expects:{}: ceiling must be a number, got {:?}", lineno + 1, value.trim())
            });
            ceilings.push(MetricCeiling {
                kind: parts[1].to_string(),
                metric: parts[2].to_string(),
                max,
            });
        } else {
            panic!("{case}/expects:{}: unrecognized directive: {line:?}", lineno + 1);
        }
    }

    Expectations { output, ceilings }
}

/// Resolve `stats.<kind>.<metric>` to its actual value.
fn actual_metric(stats: &DupStats, kind: &str, metric: &str, case: &str) -> u64 {
    let counter = match kind {
        "list" => Some(stats.get(DupKind::List)),
        "map" => Some(stats.get(DupKind::Map)),
        "f64array" => Some(stats.get(DupKind::F64Array)),
        "fork" => Some(stats.get(DupKind::Fork)),
        "total" => None, // handled below
        other => panic!("{case}/expects: unknown dup kind {other:?}"),
    };
    match (counter, metric) {
        (Some(c), "count") => c.count,
        (Some(c), "bytes") => c.bytes,
        (None, "count") => stats.total_count(),
        (None, "bytes") => stats.total_bytes(),
        (_, other) => panic!("{case}/expects: unknown dup metric {other:?}"),
    }
}

/// Run one case's `main.ptl` and return `(output_lines, errors)`.
fn check_case(dir: &Path) -> Vec<String> {
    let case = dir.file_name().unwrap().to_string_lossy().to_string();
    let mut errors = Vec::new();

    let source = fs::read_to_string(dir.join("main.ptl"))
        .unwrap_or_else(|e| panic!("{case}: cannot read main.ptl: {e}"));
    let expects_text = fs::read_to_string(dir.join("expects"))
        .unwrap_or_else(|e| panic!("{case}: cannot read expects: {e}"));
    let expects = parse_expects(&expects_text, &case);

    let mut env = Env::new();
    let pid = match env.load_program(&source) {
        Ok(pid) => pid,
        Err(e) => {
            errors.push(format!("{case}: failed to load: {e}"));
            return errors;
        }
    };
    let sid = env.create_stack(pid).unwrap_or_else(|e| panic!("{case}: create_stack: {e}"));
    if let Err(e) = env.run(sid) {
        errors.push(format!("{case}: runtime error: {e}"));
        return errors;
    }

    // ── Console output ───────────────────────────────────────────
    let output = env.take_output();
    if output != expects.output {
        errors.push(format!(
            "{case}: console output mismatch\n  expected: {:?}\n  actual:   {:?}",
            expects.output, output
        ));
    }

    // ── Performance ceilings ─────────────────────────────────────
    if DUP_STATS_ENABLED {
        let stats = env.dup_stats();
        for c in &expects.ceilings {
            let actual = actual_metric(stats, &c.kind, &c.metric, &case);
            if actual > c.max {
                errors.push(format!(
                    "{case}: dup.{}.{} regressed: {} exceeds ceiling {}",
                    c.kind, c.metric, actual, c.max
                ));
            }
        }
    }

    errors
}

/// Discover `test/<case>/` directories that contain a `main.ptl`.
fn case_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../test");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("cannot read test dir {}: {e}", root.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.join("main.ptl").is_file().then_some(path)
        })
        .collect();
    dirs.sort();
    dirs
}

#[test]
fn script_cases_match_expectations() {
    let dirs = case_dirs();
    assert!(!dirs.is_empty(), "no script test cases found under test/");

    let mut failures = Vec::new();
    for dir in &dirs {
        failures.extend(check_case(dir));
    }

    assert!(
        failures.is_empty(),
        "{} script case failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
