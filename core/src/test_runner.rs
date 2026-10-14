//! The runner behind `petal test`: find the `test "title" do … end` blocks of
//! a file, run each in isolation, and report what its expectations recorded.
//!
//! A file is loaded as the entry of its own program, with its imports and
//! its project's `petal.toml` resolved as `petal run` would resolve them.
//! Only the tests *written in that file* are run: a module's tests run when
//! the module's own file is given (or found under a directory), so a
//! directory's tests each run once however its files import each other.
//!
//! Each test gets a stack of its own: the file's top level runs once on it
//! (so its functions, `var`s and `state` exist, with their initial values),
//! then the test body. Nothing one test writes is seen by another. A file
//! that cannot be loaded or linked here — it needs a host's natives — is
//! reported as a file error; such tests are run by the host
//! ([`crate::env::Env::test_begin`]).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::env::{Env, RunOutcome, TestStep};
use crate::expect::ExpectFailure;
use crate::program::ProgramId;

/// What `petal test` was asked to do.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Package search directories (`-I`).
    pub include_dirs: Vec<PathBuf>,
    /// Run only the tests whose title contains this text.
    pub filter: Option<String>,
    /// The longest one test may run before it is stopped and failed. `None`
    /// is no limit.
    pub timeout: Option<Duration>,
    /// Find the tests and report them without running anything.
    pub list: bool,
}

/// How one test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Every expectation held and the body returned.
    Passed,
    /// At least one expectation did not hold.
    Failed,
    /// The body raised (an `assert`, a runtime error), timed out, or the
    /// file's top level failed before the body could run.
    Error,
    /// `--list`: found, not run.
    Listed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Error => "error",
            Status::Listed => "listed",
        }
    }
}

/// One test's report.
#[derive(Debug, Clone)]
pub struct TestReport {
    /// The title.
    pub name: String,
    /// 1-based line of the `test` keyword.
    pub line: u32,
    pub status: Status,
    /// The expectations that did not hold, in order.
    pub failures: Vec<ExpectFailure>,
    /// The runtime error that ended the test, if one did.
    pub error: Option<String>,
    /// How many expectations the test evaluated.
    pub checks: u64,
    /// What the test body printed.
    pub output: Vec<String>,
    pub duration: Duration,
}

/// One file's report.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// The file as it was named to the runner.
    pub file: String,
    /// Why none of the file's tests could run: it does not parse, compile,
    /// pass `petal check`, or link against this host.
    pub error: Option<String>,
    /// Expectations at the file's top level (outside any test) that did not
    /// hold when it ran.
    pub top_level: Vec<ExpectFailure>,
    /// The file's tests that matched the filter, in source order.
    pub tests: Vec<TestReport>,
}

impl FileReport {
    /// Whether anything in the file counts against the run.
    pub fn failed(&self) -> bool {
        self.error.is_some()
            || !self.top_level.is_empty()
            || self
                .tests
                .iter()
                .any(|t| matches!(t.status, Status::Failed | Status::Error))
    }
}

/// The totals of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    /// Files that could not be run at all.
    pub file_errors: usize,
    /// Files whose top level recorded a failed expectation.
    pub top_level_failures: usize,
}

impl Summary {
    pub fn of(files: &[FileReport]) -> Summary {
        let mut s = Summary::default();
        for f in files {
            s.files += 1;
            if f.error.is_some() {
                s.file_errors += 1;
            }
            if !f.top_level.is_empty() {
                s.top_level_failures += 1;
            }
            for t in &f.tests {
                s.tests += 1;
                match t.status {
                    Status::Passed => s.passed += 1,
                    Status::Failed => s.failed += 1,
                    Status::Error => s.errors += 1,
                    Status::Listed => {}
                }
            }
        }
        s
    }

    /// Whether the run as a whole passed.
    pub fn ok(&self) -> bool {
        self.failed == 0 && self.errors == 0 && self.file_errors == 0 && self.top_level_failures == 0
    }
}

/// Whether `source` has a test block. A file that does not parse is said to
/// have one when a line starts with `test "`, so that running it reports the
/// parse error instead of passing the file over.
pub fn has_tests(source: &str) -> bool {
    // Cheap refusal first: most files of a project have none.
    if !source.contains(crate::ast::TEST_KEYWORD) {
        return false;
    }
    let looks_like_one = || {
        source
            .lines()
            .any(|l| l.trim_start().strip_prefix("test").is_some_and(|r| r.trim_start().starts_with('"')))
    };
    match crate::cst::parse_source_phased(source, crate::source_map::ENTRY_FILE) {
        Ok((_tree, stmts)) => stmts.iter().any(|s| s.test.is_some()),
        Err(_) => looks_like_one(),
    }
}

/// The `.ptl` files the given paths name: a file is itself, a directory is
/// every `.ptl` file under it (dot-directories, `node_modules` and `target`
/// skipped). A path that does not exist is returned in the second list.
pub fn expand_paths(paths: &[String]) -> (Vec<PathBuf>, Vec<String>) {
    let default = [".".to_string()];
    let paths = if paths.is_empty() { &default[..] } else { paths };
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            files.extend(crate::apply_change::ptl_files_under(path).into_iter().map(|f| {
                // `./a.ptl` under `.` reads better as `a.ptl`.
                f.strip_prefix("./").map(Path::to_path_buf).unwrap_or(f)
            }));
        } else if path.exists() {
            files.push(path.to_path_buf());
        } else {
            missing.push(p.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(crate::module::canonical_path(f)));
    (files, missing)
}

/// An `Env` set up as `petal run` sets one up, for a program that runs once.
fn make_env(include_dirs: &[PathBuf]) -> Result<Env, String> {
    let mut env = Env::new();
    for dir in include_dirs {
        env.add_module_path(dir.clone());
    }
    env.set_inline_import_dir(std::env::current_dir().ok());
    env.set_install_on_demand(std::env::var_os("PETAL_OFFLINE").is_none());
    if let Some(err) = env.package_errors().first() {
        return Err(err.to_string());
    }
    // Each test's stack runs the file once: a memo record or a kept static
    // value would never be used again.
    if std::env::var_os("PETAL_POLICY").is_none() {
        env.set_policy(env.policy().with_memo(false).with_static_cache(false));
    }
    // What a test prints is collected and shown with its result.
    env.set_echo(false);
    Ok(env)
}

/// Load `source` as the entry program of `env`, checked and linked.
fn load(env: &mut Env, source: &str, path: Option<&Path>) -> Result<ProgramId, String> {
    let was_check_only = env.is_check_only();
    env.set_check_only(true);
    let loaded = env.load_program_diag(source, path);
    env.set_check_only(was_check_only);
    let pid = loaded.map_err(|e| e.to_string())?;
    let report = env.check_program(pid);
    if let Some(e) = &report.lower_error {
        return Err(e.clone());
    }
    if !report.passed() {
        return Err(env.error_diagnostics_text(pid).trim_end().to_string());
    }
    env.link(pid).map_err(|e| {
        format!(
            "{e}\n(the file needs a host this command does not provide; run its tests with \
             the host's own test runner)"
        )
    })?;
    Ok(pid)
}

/// Run the tests written in one file. `file` is how the file is named in
/// the report; `path` is where it is, for resolving its imports (`None` for
/// code with no file).
pub fn run_source(file: &str, path: Option<&Path>, source: &str, opts: &Options) -> FileReport {
    let mut report = FileReport {
        file: file.to_string(),
        error: None,
        top_level: Vec::new(),
        tests: Vec::new(),
    };
    let mut env = match make_env(&opts.include_dirs) {
        Ok(env) => env,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    let pid = match load(&mut env, source, path) {
        Ok(pid) => pid,
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    };
    let entry_name = env
        .get_program(pid)
        .map(|p| p.entry_name.clone())
        .unwrap_or_default();
    // A failure in the entry file is named as the file was named to us.
    let rename = |mut f: ExpectFailure| {
        if f.file == entry_name {
            f.file = file.to_string();
        }
        f
    };
    let tests: Vec<_> = env
        .tests(pid)
        .iter()
        .filter(|t| t.module.is_none())
        .filter(|t| opts.filter.as_deref().is_none_or(|f| t.name.contains(f)))
        .cloned()
        .collect();

    // A top level that ran out of time once would do so again for every test.
    let mut hung: Option<String> = None;
    for (i, test) in tests.iter().enumerate() {
        let mut row = TestReport {
            name: test.name.clone(),
            line: test.line,
            status: Status::Listed,
            failures: Vec::new(),
            error: None,
            checks: 0,
            output: Vec::new(),
            duration: Duration::ZERO,
        };
        if opts.list {
            report.tests.push(row);
            continue;
        }
        let started = Instant::now();
        if let Some(e) = &hung {
            row.status = Status::Error;
            row.error = Some(e.clone());
            report.tests.push(row);
            continue;
        }
        let sid = match env.create_stack(pid) {
            Ok(sid) => sid,
            Err(e) => {
                row.status = Status::Error;
                row.error = Some(e);
                report.tests.push(row);
                continue;
            }
        };
        // The file's top level, which defines what the test calls. What it
        // prints is not the test's output, and what it expects is reported
        // once for the file.
        let setup = run_top_level(&mut env, sid, opts.timeout, started);
        env.take_output_for(sid);
        let top_level = env.take_expect_failures(sid);
        if i == 0 {
            report.top_level = top_level.into_iter().map(rename).collect();
        }
        match setup {
            Err((e, timed_out)) => {
                row.status = Status::Error;
                let e = format!("the file failed before the test ran: {e}");
                if timed_out {
                    hung = Some(e.clone());
                }
                row.error = Some(e);
            }
            Ok(_) => {
                let result = drive(&mut env, sid, &test.function, opts.timeout, started);
                row.checks = result.checks;
                row.failures = result.failures.into_iter().map(rename).collect();
                row.error = result.error;
                row.status = if row.error.is_some() {
                    Status::Error
                } else if result.failed > 0 {
                    Status::Failed
                } else {
                    Status::Passed
                };
            }
        }
        row.output = env.take_output_for(sid);
        row.duration = started.elapsed();
        report.tests.push(row);
    }
    report
}

/// How many instructions a test runs between looks at the clock.
const SLICE: u64 = 2_000_000;

/// The file's top level, held to the same clock as the test that follows it:
/// a top level that never finishes is the test's timeout, not a hang.
fn run_top_level(
    env: &mut Env,
    sid: crate::stack::StackKey,
    timeout: Option<Duration>,
    started: Instant,
) -> Result<(), (String, bool)> {
    loop {
        match env.run_bounded(sid, SLICE).map_err(|e| (e, false))? {
            RunOutcome::Done(_) => return Ok(()),
            RunOutcome::Yielded { .. } => {
                if let Some(limit) = timeout
                    && started.elapsed() >= limit
                {
                    return Err((
                        format!(
                            "timed out after {} in the file's top level (raise the limit \
                             with --timeout <seconds>)",
                            seconds(limit)
                        ),
                        true,
                    ));
                }
            }
        }
    }
}

fn drive(
    env: &mut Env,
    sid: crate::stack::StackKey,
    function: &str,
    timeout: Option<Duration>,
    started: Instant,
) -> crate::env::TestResult {
    if let Err(e) = env.test_begin(sid, function) {
        return crate::env::TestResult {
            function: function.to_string(),
            passed: false,
            failures: Vec::new(),
            failed: 0,
            checks: 0,
            error: Some(e.to_string()),
        };
    }
    loop {
        match env.test_resume_bounded(sid, SLICE) {
            TestStep::Finished(result) => return result,
            TestStep::Suspended => {
                return env
                    .test_abort(
                        sid,
                        "the test asked to wait, and `petal test` has no frames to run in the \
                         meantime; run it with the host's own test runner",
                    )
                    .expect("test in progress");
            }
            TestStep::OutOfSteps => {
                if let Some(limit) = timeout
                    && started.elapsed() >= limit
                {
                    let why = format!(
                        "timed out after {} (raise the limit with --timeout <seconds>)",
                        seconds(limit)
                    );
                    return env.test_abort(sid, &why).expect("test in progress");
                }
            }
        }
    }
}

fn seconds(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s == s.trunc() {
        format!("{}s", s as u64)
    } else {
        format!("{s:.1}s")
    }
}

/// Run the tests of every file `paths` names. Files with no test block are
/// passed over (and not listed), so a directory of a project is cheap to
/// give. A path that does not exist is a file error.
pub fn run_paths(paths: &[String], opts: &Options) -> Vec<FileReport> {
    let (files, missing) = expand_paths(paths);
    let mut reports = Vec::new();
    for p in missing {
        reports.push(FileReport {
            file: p,
            error: Some("no such file or directory".to_string()),
            top_level: Vec::new(),
            tests: Vec::new(),
        });
    }
    for path in files {
        let name = path.display().to_string();
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                reports.push(FileReport {
                    file: name,
                    error: Some(e.to_string()),
                    top_level: Vec::new(),
                    tests: Vec::new(),
                });
                continue;
            }
        };
        if !has_tests(&source) {
            continue;
        }
        let report = run_source(&name, Some(&path), &source, opts);
        // A filter that leaves a file nothing to run leaves it out.
        if report.tests.is_empty() && report.error.is_none() && opts.filter.is_some() {
            continue;
        }
        reports.push(report);
    }
    reports
}

/// The report as `petal test` prints it. `show_output` prints what passing
/// tests printed too; a failing test's output is always shown.
pub fn render(files: &[FileReport], show_output: bool) -> String {
    let mut out = String::new();
    let indent = |text: &str, pad: &str| -> String {
        text.lines()
            .map(|l| format!("{pad}{l}\n"))
            .collect::<String>()
    };
    for f in files {
        out.push_str(&format!("{}\n", f.file));
        if let Some(e) = &f.error {
            out.push_str("  ERROR the file could not be run\n");
            out.push_str(&indent(e, "        "));
        }
        if !f.top_level.is_empty() {
            out.push_str("  FAIL  (top level)\n");
            for failure in &f.top_level {
                out.push_str(&format!("        {failure}\n"));
            }
        }
        for t in &f.tests {
            let tag = match t.status {
                Status::Passed => "ok   ",
                Status::Failed => "FAIL ",
                Status::Error => "ERROR",
                Status::Listed => "     ",
            };
            out.push_str(&format!("  {tag} {}\n", t.name));
            for failure in &t.failures {
                out.push_str(&format!("        {failure}\n"));
            }
            if let Some(e) = &t.error {
                out.push_str(&indent(e, "        "));
            }
            let failed = matches!(t.status, Status::Failed | Status::Error);
            if !t.output.is_empty() && (failed || show_output) {
                out.push_str("        output:\n");
                for line in &t.output {
                    out.push_str(&indent(line, "          | "));
                }
            }
        }
    }
    let s = Summary::of(files);
    if !files.is_empty() {
        out.push('\n');
    }
    if files.iter().all(|f| f.tests.iter().all(|t| t.status == Status::Listed)) && s.tests > 0 {
        out.push_str(&format!(
            "{} in {}\n",
            count(s.tests, "test"),
            count(s.files, "file")
        ));
        return out;
    }
    if s.tests == 0 && s.file_errors == 0 && s.top_level_failures == 0 {
        out.push_str("no tests found\n");
        return out;
    }
    let mut parts = vec![format!("{} passed", s.passed)];
    if s.failed > 0 {
        parts.push(format!("{} failed", s.failed));
    }
    if s.errors > 0 {
        parts.push(format!("{} with errors", s.errors));
    }
    out.push_str(&format!("{}: {}", count(s.tests, "test"), parts.join(", ")));
    if s.file_errors > 0 {
        out.push_str(&format!("; {} could not be run", count(s.file_errors, "file")));
    }
    if s.top_level_failures > 0 {
        out.push_str(&format!(
            "; {} failed at the top level",
            count(s.top_level_failures, "file")
        ));
    }
    out.push_str(&format!(" ({})\n", count(s.files, "file")));
    out
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The report as one JSON document (`petal test --json`).
pub fn to_json(files: &[FileReport]) -> serde_json::Value {
    let s = Summary::of(files);
    serde_json::json!({
        "ok": s.ok(),
        "summary": {
            "files": s.files,
            "tests": s.tests,
            "passed": s.passed,
            "failed": s.failed,
            "errors": s.errors,
            "file_errors": s.file_errors,
            "top_level_failures": s.top_level_failures,
        },
        "files": files.iter().map(|f| serde_json::json!({
            "file": f.file,
            "error": f.error,
            "top_level": f.top_level,
            "tests": f.tests.iter().map(|t| serde_json::json!({
                "name": t.name,
                "line": t.line,
                "status": t.status.label(),
                "checks": t.checks,
                "failures": t.failures,
                "error": t.error,
                "output": t.output,
                "duration_ms": t.duration.as_secs_f64() * 1000.0,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
