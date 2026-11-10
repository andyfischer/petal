//! `petal migrate-old-code` — rewrite source written in an old syntax into
//! the current one.
//!
//! When the language changes a form, the rewrite from the old form to the new
//! one is a **migration**, and it lives here, as one entry of [`MIGRATIONS`].
//! The other commands (`petal fmt`, `petal lint --fix`, `petal apply-change`,
//! the LSP) only carry logic for the current syntax: code in an old form is
//! migrated first. Where the compiler still accepts a deprecated form, its
//! warning says to run `petal migrate-old-code` ([`RUN_HINT`]); once a form is
//! removed from the parser, its error says the same, and the rewrite stays
//! here.
//!
//! Every migration has a kebab-case name, a summary, the date it was added
//! and what it replaces. They run in the order listed, each over the text the
//! one before left, so a later one sees an earlier one's output; and every
//! one rewrites only the old form, so running the tool again changes nothing
//! (it is idempotent, and the tests hold every rule to that).
//!
//! **Text migrations** ([`migrate_source`]) need one file and a parse:
//! `at-rebind` (which reads the old `@` itself, since the parser no longer
//! does), `export-to-pub`, `const-decl-deprecated`, `config-let-deprecated`,
//! `state-key-deprecated`. **`old-imports`** rewrites imports for the current
//! module system and needs every file of the batch and a module resolver; it
//! is [`crate::fix_imports`], which the CLI runs after the text migrations
//! (and which `petal fix-imports` still runs on its own).
//!
//! **Gates.** A migration changes tokens, so if the original compiles, the
//! migrated text must compile too, or nothing is produced for the file (a
//! migration bug, reported as an error). `export-to-pub` additionally checks
//! that exactly the counted modifiers were respelled. The output is not run
//! through `petal fmt`: a migration edits what it rewrites and nothing else
//! (re-padding a trailing comment it pushed or pulled out of its column).
//!
//! **Opting out.** `// petal-lint-ignore <migration>` on the line before a
//! site, or trailing on its line (`petal-lint-ignore-file <migration>` for the
//! whole file), keeps that site in the old form. It is the same comment that
//! silences the compiler's deprecation warning for the site.
//!
//! **Adding a migration** (see docs/dev/migrations.md): write its planner in
//! a file of this module (it returns [`crate::lint::Fix`]es: an anchor, a
//! message and the splices), add a [`Migration`] row to [`MIGRATIONS`] and a
//! call in [`run`], point the compiler's warning or the parser's error for
//! the old form at [`RUN_HINT`], and add fixtures under
//! core/tests/all/migrate_old_code.rs.

use std::path::PathBuf;

mod at_rebind;
pub mod export_keyword;
mod export_to_pub;
mod lifecycle_keywords;
mod state_key;

pub use crate::lint::Finding;
pub(crate) use lifecycle_keywords::deprecated_lifecycle_decls;
pub use lifecycle_keywords::migrate_lifecycle_keywords;

use crate::lint::{LintOptions, Pipeline, RuleFilter, compiles, reparse};

/// What a deprecation warning or a removed form's error appends: the command
/// that rewrites it.
pub const RUN_HINT: &str = "run `petal migrate-old-code`";

pub const AT_REBIND: &str = "at-rebind";
pub const EXPORT_TO_PUB: &str = "export-to-pub";
pub const CONST_DECL_DEPRECATED: &str = "const-decl-deprecated";
pub const CONFIG_LET_DEPRECATED: &str = "config-let-deprecated";
pub const STATE_KEY_DEPRECATED: &str = crate::compiler::keyed_state::RULE;
pub const OLD_IMPORTS: &str = "old-imports";

/// What the compiler does with a migration's old form today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OldForm {
    /// Accepted with no diagnostic: a spelling. `petal lint` reports it (the
    /// report-only `old-syntax` rule), since nothing else would.
    Accepted,
    /// Accepted with a deprecation warning that names the migration.
    Warns,
    /// Rejected: the form was removed, and the error names the tool.
    Removed,
}

/// One migration, as `petal migrate-old-code --list` shows it.
pub struct Migration {
    pub name: &'static str,
    pub summary: &'static str,
    /// The date the migration was added to this tool (`YYYY-MM-DD`).
    pub added: &'static str,
    /// When and where the old form was deprecated or removed.
    pub replaces: &'static str,
    pub old_form: OldForm,
    /// Some sites are left for a hand edit (reported, not rewritten): the
    /// rewrite is only made where it is provably the same program.
    pub partial: bool,
    /// Needs the whole batch of files and a module resolver (the CLI runs
    /// it); [`migrate_source`] skips it.
    pub batch: bool,
}

/// Every migration, in the order it runs.
pub const MIGRATIONS: &[Migration] = &[
    // First: the current parser rejects its old form, and every later
    // migration needs a parse.
    Migration {
        name: AT_REBIND,
        summary: "the `@` rebind operator: `f(@x)` becomes `x = f(x)` (a nested call is \
                  lifted to an assignment before its statement); an `@` it could not lift is \
                  reported for a hand edit",
        added: "2026-11-09",
        replaces: "`f(@x)`, removed 2026-11-09 to free `@` for annotations \
                   (docs/proposals/annotation-syntax.md)",
        old_form: OldForm::Removed,
        partial: true,
        batch: false,
    },
    Migration {
        name: EXPORT_TO_PUB,
        summary: "the export modifier `export` is spelled `pub`",
        added: "2026-11-09",
        replaces: "`export`, deprecated before 0.1.0 (docs/module-system.md)",
        old_form: OldForm::Accepted,
        partial: false,
        batch: false,
    },
    Migration {
        name: CONST_DECL_DEPRECATED,
        summary: "`const X = e` becomes `let X = const(e)`",
        added: "2026-11-09",
        replaces: "`const` declarations, deprecated 2026-11-09 (lifecycle Phase 5)",
        old_form: OldForm::Warns,
        partial: false,
        batch: false,
    },
    Migration {
        name: CONFIG_LET_DEPRECATED,
        summary: "`config let X = v` becomes `let X = tunable(v)` (or drops `config` before a \
                  `tunable(…)`)",
        added: "2026-11-09",
        replaces: "`config let`, deprecated 2026-11-09 (lifecycle Phase 5)",
        old_form: OldForm::Warns,
        partial: false,
        batch: false,
    },
    Migration {
        name: STATE_KEY_DEPRECATED,
        summary: "`state(key) x = e` becomes `memo(key) x = e` where nothing writes it and `e` \
                  reads only its key; any other `state(key)` is reported for a hand edit",
        added: "2026-11-09",
        replaces: "keyed `state(key)`, deprecated 2026-10-22",
        old_form: OldForm::Warns,
        partial: true,
        batch: false,
    },
    Migration {
        name: OLD_IMPORTS,
        summary: "imports for the current module system: a sibling file is `import ./x`, \
                  `import m: *` is `import m`, imports of packages merged into core are \
                  deleted, and a missing package import is added",
        added: "2026-11-09",
        replaces: "bare sibling imports and `: *`, removed 2026-10-10 (docs/module-system.md)",
        old_form: OldForm::Removed,
        partial: true,
        batch: true,
    },
];

pub fn migration(name: &str) -> Option<&'static Migration> {
    MIGRATIONS.iter().find(|m| m.name == name)
}

pub fn is_migration(name: &str) -> bool {
    migration(name).is_some()
}

/// What a migration run needs: module search dirs and the file's own path,
/// so the compile gate (and `state-key-deprecated`'s own check) compiles the
/// file the way `petal run` would; and which migrations run.
#[derive(Default, Clone)]
pub struct MigrateOptions {
    pub include_dirs: Vec<PathBuf>,
    pub origin: Option<PathBuf>,
    /// `--only`: when set, just these migrations run.
    pub only: Option<Vec<String>>,
}

impl MigrateOptions {
    pub fn enabled(&self, name: &str) -> bool {
        self.only.as_ref().is_none_or(|only| only.iter().any(|r| r == name))
    }
}

/// The result of migrating one source text.
pub struct MigrateOutcome {
    /// Every site, in source order: `fixable` is true for the ones rewritten
    /// and false for the ones left for a hand edit.
    pub findings: Vec<Finding>,
    /// The migrated source. Equal to the input when nothing was rewritten.
    pub output: String,
}

impl MigrateOutcome {
    pub fn changed(&self, original: &str) -> bool {
        self.output != original
    }

    /// How many sites were rewritten.
    pub fn rewritten(&self) -> usize {
        self.findings.iter().filter(|f| f.fixable).count()
    }

    /// How many sites are left for a hand edit.
    pub fn left(&self) -> usize {
        self.findings.len() - self.rewritten()
    }

    pub fn count(&self, rule: &str) -> usize {
        self.findings.iter().filter(|f| f.rule == rule).count()
    }
}

/// Run every enabled text migration over `source`. Errors if the source does
/// not parse, or if a migration fails a gate (a migration bug: nothing is
/// produced for the file).
pub fn migrate_source(source: &str, opts: &MigrateOptions) -> Result<MigrateOutcome, String> {
    run(source, opts, true)
}

/// The sites of the migrations whose old form the compiler accepts without a
/// word ([`OldForm::Accepted`]), for `petal lint`'s `old-syntax` report: per
/// site its char offset in `source`, its migration and its message. Nothing
/// is compiled.
pub(crate) fn silent_old_forms(source: &str) -> Result<Vec<(usize, Finding)>, String> {
    let only: Vec<String> = MIGRATIONS
        .iter()
        .filter(|m| m.old_form == OldForm::Accepted && !m.batch)
        .map(|m| m.name.to_string())
        .collect();
    let opts = MigrateOptions {
        only: Some(only),
        ..Default::default()
    };
    let out = run(source, &opts, false)?;
    let mut line_starts = vec![0usize];
    for (i, c) in source.chars().enumerate() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    Ok(out
        .findings
        .into_iter()
        .map(|f| (line_starts[f.line - 1] + f.column - 1, f))
        .collect())
}

fn run(source: &str, opts: &MigrateOptions, gate: bool) -> Result<MigrateOutcome, String> {
    // Lint's pipeline: findings mapped back to the original text, and
    // `petal-lint-ignore` honoured under each migration's name.
    let filter = RuleFilter {
        include: Some(
            MIGRATIONS
                .iter()
                .filter(|m| !m.batch && opts.enabled(m.name))
                .map(|m| m.name.to_string())
                .collect(),
        ),
        exclude: Vec::new(),
    };
    let mut p = Pipeline::new(source, &filter)?;
    let lint_opts = LintOptions {
        include_dirs: opts.include_dirs.clone(),
        origin: opts.origin.clone(),
        rules: RuleFilter::default(),
    };

    // `@` rebinds first: until they are gone the file does not parse.
    let mut rebinds = 0;
    let mut rebinds_left = 0;
    if opts.enabled(AT_REBIND) {
        rebinds = p.apply(AT_REBIND, at_rebind::plan_at_rebind_fixes(source)?);
        rebinds_left = p.unfixed(AT_REBIND);
    }
    let (chars, stmts) = match reparse(&p.text) {
        Ok(parsed) => parsed,
        // An `@` left for a hand edit keeps the file from parsing: report
        // what was done and stop.
        Err(_) if rebinds_left > 0 => {
            let (output, mut findings) = p.finish();
            findings.sort_by_key(|f| (f.line, f.column));
            return Ok(MigrateOutcome { findings, output });
        }
        Err(e) => return Err(e),
    };

    let exports = p.apply(EXPORT_TO_PUB, export_to_pub::plan_export_fixes(&stmts, &chars)?);
    let mut rewrites = rebinds + exports;

    // One keyword, then the other over its output: each pads trailing
    // comments against the text as it stands.
    if opts.enabled(CONST_DECL_DEPRECATED) {
        let (chars, stmts) = reparse(&p.text)?;
        let fixes = lifecycle_keywords::plan_decl_fixes(&stmts, &chars, true)?;
        rewrites += p.apply(CONST_DECL_DEPRECATED, fixes);
    }
    if opts.enabled(CONFIG_LET_DEPRECATED) {
        let (chars, stmts) = reparse(&p.text)?;
        let fixes = lifecycle_keywords::plan_decl_fixes(&stmts, &chars, false)?;
        rewrites += p.apply(CONFIG_LET_DEPRECATED, fixes);
    }

    // Each candidate's rewrite is checked by compiling it, so the migration
    // is skipped outright when it is not selected.
    if opts.enabled(STATE_KEY_DEPRECATED) {
        let (chars, stmts) = reparse(&p.text)?;
        let fixes = state_key::plan_state_key_fixes(&p.text, &stmts, &chars, &lint_opts);
        rewrites += p.apply(STATE_KEY_DEPRECATED, fixes);
    }

    let (output, mut findings) = p.finish();
    findings.sort_by_key(|f| (f.line, f.column));
    if rewrites == 0 {
        return Ok(MigrateOutcome {
            findings,
            output: source.to_string(),
        });
    }
    if gate {
        // Only meaningful when the original compiles here at all; a file
        // whose imports don't resolve outside its app is migrated ungated.
        //
        // A file with `@` rebinds does not parse today; it compiled before
        // their removal if it compiles with each `@` read as nothing (the
        // same names, read instead of written).
        let compiled = if rebinds > 0 {
            compiles(&at_rebind::blank_markers(source)?, &lint_opts).is_ok()
        } else {
            compiles(source, &lint_opts).is_ok()
        };
        if compiled
            && let Err(e) = compiles(&output, &lint_opts)
        {
            return Err(format!(
                "migrate bug: a rewrite broke compilation — refusing to produce output ({e})"
            ));
        }
        if exports > 0 {
            // The original read with its `@` rebinds blanked, which parses
            // and has the same declarations.
            let before = if rebinds > 0 {
                count_exports(&at_rebind::blank_markers(source)?)?
            } else {
                count_exports(source)?
            };
            let after = count_exports(&output)?;
            if before.0 != after.0 || before.1 != after.1 + exports {
                return Err(format!(
                    "migrate bug: rewriting {exports} `export`(s) to `pub` left {} of {} — \
                     refusing to produce output",
                    after.1, before.1
                ));
            }
        }
    }
    Ok(MigrateOutcome { findings, output })
}

/// (exported declarations, how many of them are spelled `export`) in
/// `source`.
fn count_exports(source: &str) -> Result<(usize, usize), String> {
    use crate::ast::{ExprVisitor, Stmt, walk_stmt};
    struct Counter(usize);
    impl ExprVisitor for Counter {
        fn visit_stmt(&mut self, s: &Stmt) {
            if s.exported {
                self.0 += 1;
            }
            walk_stmt(self, s);
        }
    }
    let (chars, stmts) = reparse(source)?;
    let mut c = Counter(0);
    for s in &stmts {
        c.visit_stmt(s);
    }
    let deprecated = export_keyword::deprecated_export_spans(&stmts, &chars).len();
    Ok((c.0, deprecated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_migration_is_named_dated_and_unique() {
        let mut names: Vec<&str> = MIGRATIONS.iter().map(|m| m.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), MIGRATIONS.len());
        for m in MIGRATIONS {
            assert!(
                m.name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{}",
                m.name
            );
            let date = m.added.as_bytes();
            assert!(
                date.len() == 10 && date[4] == b'-' && date[7] == b'-',
                "{}: added {:?} is not YYYY-MM-DD",
                m.name,
                m.added
            );
            assert!(!m.summary.is_empty() && !m.replaces.is_empty());
            // A migration is never also a lint rule: lint carries only the
            // current syntax.
            assert!(!crate::lint::is_rule(m.name), "{} is a lint rule too", m.name);
        }
    }
}
