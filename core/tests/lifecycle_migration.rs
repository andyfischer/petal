//! The Phase 2 fixers of docs/tasks/lifecycle-annotation-functions-refactor.md
//! (`const-decl-deprecated`, `config-let-deprecated`) over the Petal-owned
//! corpus: every `.ptl` file under `examples/`, `garden/`, `core/tests` and
//! `cli/tests` that declares a `const` or a `config let` is rewritten with
//! [`petal::lint::migrate_lifecycle_keywords`], and the rewrite must
//!
//! - be a fixed point (the fix of a fixed file is empty) and keep its lines;
//! - stay formatted when the original was (trailing comments re-padded);
//! - leave no `const` declaration and no `config let` behind;
//! - load wherever the original loads, with the same diagnostics;
//! - give hosts the same bindings: `static_bindings` (names, values, the
//!   text a host writes back, settings, ranges, options, comments, lines).
//!
//! This was the dry run of Phase 3, which has since applied the rewrite to
//! these files, so the corpus no longer declares either keyword (the
//! `the_petal_corpus_declares_neither_keyword` guard). The dry run now runs
//! over the directories in `PETAL_MIGRATE_DRY_RUN` (Phase 4: Cheesecake), and
//! `migrate_in_place` applies the rewrite.

use std::path::{Path, PathBuf};

use petal::ast::StmtKind;
use petal::env::Env;
use petal::source_map::FileId;

fn ptl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("target" | "node_modules" | ".git")
            ) {
                ptl_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "ptl") {
            out.push(p);
        }
    }
}

/// Load `src` check-only: `Ok(warnings)` or `Err(errors)`, by line and text.
fn load(src: &str, origin: &Path) -> Result<Vec<String>, Vec<String>> {
    let mut env = Env::new();
    env.set_check_only(true);
    match env.load_program_diag(src, Some(origin)) {
        Ok(pid) => Ok(env
            .get_program(pid)
            .expect("loaded")
            .warnings
            .iter()
            .map(|d| format!("{:?} line {}: {}", d.severity, d.span.start.line, d.message))
            .collect()),
        Err(e) => Err(e
            .items
            .iter()
            .map(|i| format!("line {:?}: {}", i.span.map(|s| s.start.line), i.message))
            .collect()),
    }
}

/// (`const` declarations, `config let`s) anywhere in `src`'s top level.
fn keyword_decls(src: &str) -> Option<(usize, usize)> {
    let (_, stmts) = petal::cst::parse_source(src, FileId(0)).ok()?;
    let mut n = (0, 0);
    for s in &stmts {
        if let StmtKind::Let {
            is_const, is_config, ..
        } = &s.kind
        {
            n.0 += *is_const as usize;
            n.1 += *is_config as usize;
        }
    }
    Some(n)
}

/// The Petal-owned `.ptl` corpus that Phase 3 migrated.
const PETAL_CORPUS: &[&str] = &[
    "examples",
    "garden",
    "core/tests",
    "cli/tests",
    "core/packages",
    "test",
    "integrations",
    "core-libs",
];

/// Phase 3's guard: nothing in the Petal corpus declares `const X = e` or
/// `config let`. Tests of the deprecated forms hold them in Rust strings.
#[test]
fn the_petal_corpus_declares_neither_keyword() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in PETAL_CORPUS {
        ptl_files(&root.join(dir), &mut files);
    }
    files.sort();
    let mut left = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if !src.contains("const") && !src.contains("config") {
            continue;
        }
        if let Some((c, k)) = keyword_decls(&src)
            && c + k > 0
        {
            let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
            left.push(format!("{shown}: {c} `const`, {k} `config let`"));
        }
    }
    assert!(
        left.is_empty(),
        "declarations the migration missed (rewrite them with `migrate_in_place`):\n{}",
        left.join("\n")
    );
    assert!(files.len() > 200, "the corpus went missing");
}

/// The fixers over `PETAL_MIGRATE_DRY_RUN=<dir>[:…]`, without writing: a
/// dry run of a migration (Phase 4 points it at Cheesecake). Skipped when
/// unset.
#[test]
fn the_fixers_rewrite_the_corpus_without_changing_it() {
    let Ok(dirs) = std::env::var("PETAL_MIGRATE_DRY_RUN") else {
        eprintln!("PETAL_MIGRATE_DRY_RUN is unset: nothing to dry-run");
        return;
    };
    let root = PathBuf::from("/");
    let mut files = Vec::new();
    for dir in dirs.split(':').filter(|d| !d.is_empty()) {
        ptl_files(Path::new(dir), &mut files);
    }
    files.sort();

    let (mut n_files, mut n_const, mut n_config, mut n_loaded, mut n_formatted) = (0, 0, 0, 0, 0);
    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if !src.contains("const") && !src.contains("config") {
            continue;
        }
        let Some((consts, configs)) = keyword_decls(&src) else { continue };
        if consts + configs == 0 {
            continue;
        }
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let out = match petal::lint::migrate_lifecycle_keywords(&src) {
            Ok(out) => out,
            Err(e) => {
                failures.push(format!("{shown}: {e}"));
                continue;
            }
        };
        n_files += 1;
        n_const += consts;
        n_config += configs;

        if petal::lint::migrate_lifecycle_keywords(&out).as_deref() != Ok(out.as_str()) {
            failures.push(format!("{shown}: the fix of the fixed file is not empty"));
        }
        if out.lines().count() != src.lines().count() {
            failures.push(format!("{shown}: the rewrite changed the line count"));
        }
        // Comments re-padded as `petal fmt` would leave them: a formatted
        // file stays formatted.
        if petal::fmt::format_source(&src).as_deref() == Ok(src.as_str()) {
            n_formatted += 1;
            if petal::fmt::format_source(&out).as_deref() != Ok(out.as_str()) {
                failures.push(format!("{shown}: the rewrite of a formatted file is not formatted"));
            }
        }
        if keyword_decls(&out) != Some((0, 0)) {
            failures.push(format!("{shown}: declarations left: {:?}", keyword_decls(&out)));
        }

        // Only a file that loads on its own is compared: a module whose
        // imports resolve inside an app, or a fixture of an error, is not.
        if let Ok(before) = load(&src, path) {
            n_loaded += 1;
            match load(&out, path) {
                Ok(after) if after == before => {}
                Ok(after) => failures.push(format!(
                    "{shown}: diagnostics differ\n  before: {before:?}\n  after:  {after:?}"
                )),
                Err(e) => failures.push(format!("{shown}: the rewrite does not load: {e:?}")),
            }
        }
        match (
            petal::static_value::static_bindings(&src),
            petal::static_value::static_bindings(&out),
        ) {
            (Ok(a), Ok(b)) if a == b => {}
            (Ok(a), Ok(b)) => {
                let diff: Vec<_> = a
                    .iter()
                    .zip(&b)
                    .filter(|(x, y)| x != y)
                    .map(|(x, y)| format!("{x:?}\n    vs {y:?}"))
                    .take(3)
                    .collect();
                failures.push(format!("{shown}: bindings differ: {}", diff.join("\n  ")));
            }
            (Err(_), Err(_)) => {}
            (a, b) => failures.push(format!(
                "{shown}: bindings readable before {} and after {}",
                a.is_ok(),
                b.is_ok()
            )),
        }
    }
    eprintln!(
        "lifecycle migration: {n_files} files, {n_const} `const`, {n_config} `config let`, \
         {n_loaded} loaded, {n_formatted} formatted"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(n_files > 0, "no file under PETAL_MIGRATE_DRY_RUN declares either keyword");
}

/// Apply the migration in place: `PETAL_MIGRATE=<file or dir>[:…]`, every
/// `.ptl` file that declares either keyword is rewritten. Phase 3 ran it over
/// the Petal corpus; Phase 4 runs it over Cheesecake.
///
/// ```text
/// PETAL_MIGRATE=../examples:../garden cargo test --test lifecycle_migration \
///   migrate_in_place -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn migrate_in_place() {
    let Ok(paths) = std::env::var("PETAL_MIGRATE") else {
        panic!("set PETAL_MIGRATE to the files or directories to rewrite");
    };
    let mut files = Vec::new();
    for p in paths.split(':').filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        if p.is_dir() {
            ptl_files(&p, &mut files);
        } else {
            files.push(p);
        }
    }
    files.sort();
    let (mut n_files, mut n_const, mut n_config) = (0, 0, 0);
    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Some((consts, configs)) = keyword_decls(&src) else {
            if src.contains("config let") || src.contains("const ") {
                failures.push(format!("{}: does not parse", path.display()));
            }
            continue;
        };
        if consts + configs == 0 {
            continue;
        }
        match petal::lint::migrate_lifecycle_keywords(&src) {
            Ok(out) => {
                std::fs::write(path, out).expect("write");
                eprintln!("rewrote {} ({consts} const, {configs} config let)", path.display());
                n_files += 1;
                n_const += consts;
                n_config += configs;
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    eprintln!("migrated {n_files} files: {n_const} `const`, {n_config} `config let`");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
