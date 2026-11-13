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
//! **Text migrations** ([`migrate_source`]) need one file: first the two
//! that read tokens and trivia, `colour-literals` and `hash-comments`, with
//! the lexer's legacy mode (the language's own lexer rejects a `#f80` color,
//! see [`colour_literals::legacy_lex`]); then the ones that need a parse,
//! `else-if-nesting` (which reads `else if` the old way, as an `if` nested in
//! an `else`), `elsif-to-else-if`, `at-rebind` (which reads the old `@`
//! itself, since the parser no longer does), `export-to-pub`, `const-decl-deprecated`, `config-let-deprecated`,
//! `state-key-deprecated` (which reads the removed `state(key)` itself; the
//! ones before it plan on the file with each key group blanked, see
//! [`state_key::blank_keys`]); and last `end-lines`, which is no rewrite of an
//! old form but of the layout (it formats the file and removes the `end`s
//! indentation makes redundant, see [`end_lines`]). **`old-imports`** rewrites imports for the current
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
//! `end-lines` is the exception on both counts: it formats the file, and a
//! file that fails its gate (the same AST) keeps the output of the others
//! and is reported in [`MigrateOutcome::skipped`].
//!
//! **Markdown.** [`markdown::migrate_markdown`] runs the text migrations
//! over the `petal` fences of a Markdown text, each as a fragment
//! ([`MigrateOptions::fragment`]).
//!
//! **Opting out.** `# petal-lint-ignore <migration>` on the line before a
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
mod colour_literals;
mod else_if;
mod end_lines;
pub mod export_keyword;
mod export_to_pub;
mod hash_comments;
mod lifecycle_keywords;
pub mod markdown;
mod state_key;

pub use crate::lint::Finding;
pub use colour_literals::respell_colour_literals;
pub(crate) use lifecycle_keywords::deprecated_lifecycle_decls;
pub use lifecycle_keywords::migrate_lifecycle_keywords;

use crate::lint::{LintOptions, Pipeline, RuleFilter, compiles};

/// Parse the text a migration plans against. Old source may be indented in
/// a way that disagrees with its `end`s, which the language now rejects; a
/// migration still reads it, by its `end`s
/// ([`crate::rewrite::parse_ast_or_by_end`]). It may also still hold the
/// removed `state(key)`, which is read as a plain `state` with every offset
/// unchanged ([`state_key::blank_keys`]): the statements are of that
/// reading, the chars are the source's own.
fn reparse(source: &str) -> Result<(Vec<char>, Vec<crate::ast::Stmt>), String> {
    let (_tree, stmts, _by_end) = crate::rewrite::parse_ast_or_by_end(&state_key::blank_keys(source))?;
    Ok((source.chars().collect(), stmts))
}

/// `text` as the compiler can read it. Old source may be indented in a way
/// that disagrees with its `end`s, which the language now rejects; when
/// `text` parses only by its `end`s it is re-indented by them (`petal fmt`,
/// which checks that the result is the same parse). Anything else is
/// returned as it is. What the compile gates compile: a migration leaves
/// indentation alone, so a file indented that way before it is afterwards
/// too.
pub(crate) fn indented_by_end(text: &str) -> String {
    // A `state(key)` left in the text keeps it from parsing at all: it is
    // returned as it is, and the caller's compile reports it.
    match crate::rewrite::parse_ast_or_by_end(text) {
        Ok((_, _, true)) => crate::fmt::format_source(text).unwrap_or_else(|_| text.to_string()),
        _ => text.to_string(),
    }
}

/// `source` with each removed `state(key) name` read as a plain `state
/// name`, every offset unchanged, so that a tool which must parse or compile
/// a file with such a site left for a hand edit still can (`old-imports`, in
/// the CLI). [`restore_state_keys`] puts the keys back.
pub(crate) fn blank_state_keys(source: &str) -> String {
    state_key::blank_keys(source)
}

/// `edited` is [`blank_state_keys`]`(original)` after an edit that changed
/// import lines only: the same text with `original`'s `state(key)` groups
/// back. `None` unless that is certain, which it is when every line of the
/// result that is not an import line is `original`'s, in order.
pub(crate) fn restore_state_keys(original: &str, edited: &str) -> Option<String> {
    let blanked = state_key::blank_keys(original);
    if blanked == original {
        return Some(edited.to_string());
    }
    // The lines blanking changed, in order, with what they were.
    let changed: Vec<(&str, &str)> = blanked
        .split('\n')
        .zip(original.split('\n'))
        .filter(|(b, o)| b != o)
        .collect();
    let mut next = 0;
    let mut lines: Vec<&str> = Vec::new();
    for line in edited.split('\n') {
        if next < changed.len() && line == changed[next].0 {
            lines.push(changed[next].1);
            next += 1;
        } else {
            lines.push(line);
        }
    }
    if next != changed.len() {
        return None;
    }
    let restored = lines.join("\n");
    let is_import = |l: &&str| {
        let l = l.trim_start();
        ["import ", "pub import ", "export import "].iter().any(|p| l.starts_with(p))
    };
    let rest = |t: &str| t.split('\n').filter(|l| !is_import(l)).map(str::to_string).collect::<Vec<_>>();
    (rest(&restored) == rest(original)).then_some(restored)
}

/// What a deprecation warning or a removed form's error appends: the command
/// that rewrites it.
pub const RUN_HINT: &str = "run `petal migrate-old-code`";

pub const AT_REBIND: &str = "at-rebind";
pub const COLOUR_LITERALS: &str = "colour-literals";
pub const HASH_COMMENTS: &str = "hash-comments";
pub const ELSE_IF_NESTING: &str = "else-if-nesting";
pub const ELSIF_TO_ELSE_IF: &str = "elsif-to-else-if";
pub const EXPORT_TO_PUB: &str = "export-to-pub";
pub const CONST_DECL_DEPRECATED: &str = "const-decl-deprecated";
pub const CONFIG_LET_DEPRECATED: &str = "config-let-deprecated";
pub const STATE_KEY_DEPRECATED: &str = "state-key-deprecated";
pub const END_LINES: &str = "end-lines";
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
    /// Still the language, with no diagnostic and no lint report: the
    /// migration converts a file from one spelling the language keeps to
    /// the one written since (`end-lines`: an `end` is optional).
    Optional,
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
    // First, the two that read tokens and trivia and need no parse. The
    // language's lexer rejects `#f80`, so until `colour-literals` has run
    // nothing after it can read the file.
    Migration {
        name: COLOUR_LITERALS,
        summary: "a color literal is written `:f80`: each `#f80` color token becomes `:f80` \
                  (strings, raw strings and comments are untouched); a `#` color directly \
                  after a value or a name is reported for a hand edit",
        added: "2026-11-12",
        replaces: "`#f80` color literals, removed 2026-11-12 to free `#` for comments \
                   (docs/tasks/pythonic-syntax.md)",
        old_form: OldForm::Removed,
        partial: true,
        batch: false,
    },
    Migration {
        name: HASH_COMMENTS,
        summary: "a line comment starts with `#`: `// note` becomes `# note` (a space is added \
                  when there was none, and `// petal-lint-ignore` directives follow), and a \
                  `//` comment line in element children becomes `{/* … */}`; one between two \
                  lines of text is reported for a hand edit",
        added: "2026-11-12",
        replaces: "`//` line comments, deprecated 2026-11-12 (docs/tasks/pythonic-syntax.md)",
        old_form: OldForm::Accepted,
        partial: true,
        batch: false,
    },
    // `else if`: first the old nesting form, which no longer parses (it
    // reads the old form itself), then the old spelling of the chain.
    Migration {
        name: ELSE_IF_NESTING,
        summary: "`else if` continues the `if` chain: in `else if … end end` (an `if` nested \
                  in an `else`, each with its `end`) the nested `if`'s own `end` is removed; \
                  where the `else` holds more than the `if`, the `if` moves to the line after \
                  `else` and stays nested",
        added: "2026-11-12",
        replaces: "`else if` read as an `if` nested in an `else`, removed 2026-11-12 when \
                   `else if` became one token (docs/tasks/pythonic-syntax.md)",
        old_form: OldForm::Removed,
        partial: false,
        batch: false,
    },
    Migration {
        name: ELSIF_TO_ELSE_IF,
        summary: "`elsif` is spelled `else if`",
        added: "2026-11-12",
        replaces: "`elsif`, deprecated 2026-11-12 (docs/tasks/pythonic-syntax.md)",
        old_form: OldForm::Accepted,
        partial: false,
        batch: false,
    },
    // Then the one whose old form the parser rejects: every later migration
    // needs a parse.
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
        replaces: "keyed `state(key)`, deprecated 2026-10-22 and removed 2026-11-13",
        old_form: OldForm::Removed,
        partial: true,
        batch: false,
    },
    // Last of the text migrations: it formats the file, and every one
    // before it leaves the layout as written.
    Migration {
        name: END_LINES,
        summary: "indentation closes a block: the file is formatted (`petal fmt`), then each \
                  `end` alone on its line is removed, and `end)` / `end]` / `end}` keep the \
                  bracket. An `end` on the line of its block, `end,` and any `end` the parse \
                  needs stay",
        added: "2026-11-12",
        replaces: "the `end` that closed every multi-line block, optional since 2026-11-12 \
                   (docs/syntax/indentation.md, docs/tasks/pythonic-syntax.md)",
        old_form: OldForm::Optional,
        partial: false,
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
    /// `--except`: these migrations do not run.
    pub except: Vec<String>,
    /// The source is a fragment that may not parse (a Markdown fence that
    /// elides code): when it does not, the migrations that need only tokens
    /// (`colour-literals`, `hash-comments`, `elsif-to-else-if`) still run,
    /// and the rest are skipped for it.
    pub fragment: bool,
}

impl MigrateOptions {
    pub fn enabled(&self, name: &str) -> bool {
        self.only.as_ref().is_none_or(|only| only.iter().any(|r| r == name))
            && !self.except.iter().any(|r| r == name)
    }
}

/// Why a source was not migrated.
#[derive(Debug, Clone, PartialEq)]
pub enum MigrateError {
    /// The source could not be read: it does not lex or parse, in the
    /// current syntax or in any old form a migration reads.
    Unreadable(String),
    /// A migration's rewrite failed its gate: nothing is produced for the
    /// file, and the case is a migration bug or a source oddity to look at.
    Gate { migration: &'static str, reason: String },
}

impl MigrateError {
    fn gate(migration: &'static str) -> impl Fn(String) -> MigrateError {
        move |reason| MigrateError::Gate { migration, reason }
    }
}

impl From<String> for MigrateError {
    fn from(e: String) -> Self {
        MigrateError::Unreadable(e)
    }
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrateError::Unreadable(e) => f.write_str(e),
            MigrateError::Gate { reason, .. } => f.write_str(reason),
        }
    }
}

/// A migration that was skipped for a file because its gate failed, while
/// the output of the ones before it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub migration: &'static str,
    pub reason: String,
}

/// The result of migrating one source text.
pub struct MigrateOutcome {
    /// Every site, in source order: `fixable` is true for the ones rewritten
    /// and false for the ones left for a hand edit.
    pub findings: Vec<Finding>,
    /// The migrated source. Equal to the input when nothing was rewritten.
    pub output: String,
    /// `end-lines`: how many `end`s were removed, and how many at the start
    /// of a line were kept (reported once per file in `findings`).
    pub ends_removed: usize,
    pub ends_kept: usize,
    /// A migration whose gate failed for this file: `output` is the text
    /// without it. Only `end-lines` is skipped this way (it runs last, over
    /// text the other gates have passed); any other gate failure is an
    /// error for the whole file.
    pub skipped: Vec<Skipped>,
    /// The source did not parse and was migrated as a fragment
    /// ([`MigrateOptions::fragment`]): token migrations only.
    pub fragment: bool,
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
    migrate_source_checked(source, opts).map_err(|e| e.to_string())
}

/// [`migrate_source`], with the error saying whether the source could not
/// be read or a gate failed.
pub fn migrate_source_checked(source: &str, opts: &MigrateOptions) -> Result<MigrateOutcome, MigrateError> {
    match run(source, opts, true, false) {
        Err(MigrateError::Unreadable(_)) if opts.fragment => {
            let mut out = run(source, opts, true, true)?;
            out.fragment = true;
            Ok(out)
        }
        other => other,
    }
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
    let out = run(source, &opts, false, false).map_err(|e| e.to_string())?;
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

/// `tokens_only`: stop after the migrations that need no parse (the source
/// is a fragment that does not parse).
fn run(source: &str, opts: &MigrateOptions, gate: bool, tokens_only: bool) -> Result<MigrateOutcome, MigrateError> {
    let done = |output: String, mut findings: Vec<Finding>| {
        findings.sort_by_key(|f| (f.line, f.column));
        MigrateOutcome {
            findings,
            output,
            ends_removed: 0,
            ends_kept: 0,
            skipped: Vec::new(),
            fragment: false,
        }
    };
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
    // The file may still hold `#f80` colors, so its ignore comments are
    // read the way the first two migrations read it.
    let mut p = Pipeline::new_legacy(source, &filter)?;
    let lint_opts = LintOptions {
        include_dirs: opts.include_dirs.clone(),
        origin: opts.origin.clone(),
        rules: RuleFilter::default(),
    };

    // Color tokens first: one character each, no offset moves, and gated on
    // the token stream (the same tokens are the same parse). Until they are
    // respelled the file does not lex.
    let mut colours = 0;
    if opts.enabled(COLOUR_LITERALS) {
        let before = p.text.clone();
        colours = p.apply(COLOUR_LITERALS, colour_literals::plan_colour_fixes(&before)?);
        if colours > 0 {
            colour_literals::same_tokens(&before, &p.text).map_err(MigrateError::gate(COLOUR_LITERALS))?;
        }
    }

    // Comments next: trivia only, gated on the token stream too.
    let mut comments = 0;
    if opts.enabled(HASH_COMMENTS) {
        let before = p.text.clone();
        comments = p.apply(HASH_COMMENTS, hash_comments::plan_comment_fixes(&before)?);
        if comments > 0 {
            hash_comments::same_tokens_but_comments(&before, &p.text).map_err(MigrateError::gate(HASH_COMMENTS))?;
        }
    }

    // A `#f80` left in the file (the migration was not selected, the site
    // is ignored, or it needs a hand edit) keeps it from lexing: report what
    // was done and stop. When nothing was done at all, the lexer's own error
    // is the report.
    if p.text.contains('#')
        && !colour_literals::lexes_today(&p.text)
        && colour_literals::legacy_lex(&p.text).is_ok()
    {
        if colours + comments == 0 && p.unfixed(COLOUR_LITERALS) == 0 {
            reparse(&p.text)?;
        }
        let (output, findings) = p.finish();
        return Ok(done(output, findings));
    }

    if tokens_only {
        if opts.enabled(ELSIF_TO_ELSE_IF) {
            let before = p.text.clone();
            if p.apply(ELSIF_TO_ELSE_IF, else_if::plan_elsif_fixes(&before)?) > 0 {
                else_if::check_elsif(&before, &p.text, false).map_err(MigrateError::gate(ELSIF_TO_ELSE_IF))?;
            }
        }
        let (output, findings) = p.finish();
        return Ok(done(output, findings));
    }

    // `else if … end end`: until the nested `if`s are chains (or on lines
    // of their own) the file does not parse. Planned on the old reading of
    // `else if`, and gated on the parse and the compiled program.
    let mut nestings = 0;
    if opts.enabled(ELSE_IF_NESTING) {
        let before = p.text.clone();
        nestings = p.apply(ELSE_IF_NESTING, else_if::plan_nesting_fixes(&before)?);
        if nestings > 0 && !else_if::has_nesting(&p.text) {
            // The gate compiles both sides: a removed `state(key)` is read
            // as a plain `state` in each.
            else_if::check_nesting(&state_key::blank_keys(&before), &state_key::blank_keys(&p.text), &lint_opts)
                .map_err(MigrateError::gate(ELSE_IF_NESTING))?;
        }
    }

    // `elsif`: one keyword each, gated on the tokens and the parse.
    let mut elsifs = 0;
    if opts.enabled(ELSIF_TO_ELSE_IF) {
        let before = p.text.clone();
        elsifs = p.apply(ELSIF_TO_ELSE_IF, else_if::plan_elsif_fixes(&before)?);
        if elsifs > 0 {
            else_if::check_elsif(&before, &p.text, gate).map_err(MigrateError::gate(ELSIF_TO_ELSE_IF))?;
        }
    }

    // `@` rebinds: until they are gone the file does not parse.
    let mut rebinds = 0;
    let mut rebinds_left = 0;
    if opts.enabled(AT_REBIND) {
        let before = p.text.clone();
        rebinds = p.apply(AT_REBIND, at_rebind::plan_at_rebind_fixes(&before)?);
        rebinds_left = p.unfixed(AT_REBIND);
    }
    let (chars, stmts) = match reparse(&p.text) {
        Ok(parsed) => parsed,
        // An `@` left for a hand edit keeps the file from parsing: report
        // what was done and stop.
        Err(_) if rebinds_left > 0 => {
            let (output, findings) = p.finish();
            return Ok(done(output, findings));
        }
        Err(e) => return Err(e.into()),
    };

    let exports = p.apply(EXPORT_TO_PUB, export_to_pub::plan_export_fixes(&stmts, &chars)?);
    let mut rewrites = rebinds + colours + comments + nestings + elsifs + exports;

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
    // is skipped outright when it is not selected. The parser no longer
    // reads `state(key)`: the planner keeps its own reading.
    if opts.enabled(STATE_KEY_DEPRECATED) {
        let fixes = state_key::plan_state_key_fixes(&p.text, &lint_opts);
        rewrites += p.apply(STATE_KEY_DEPRECATED, fixes);
    }

    let mut ends_on = opts.enabled(END_LINES) && !p.ignores_file(END_LINES);
    let (output, findings) = p.finish();
    // `end-lines` formats the file, which a `state(key)` still in it (left
    // for a hand edit, ignored, or its migration not selected) keeps from
    // parsing. Its site is the report; the `end`s wait for the run after
    // the hand edit.
    if ends_on && !state_key::find_sites(&output).is_empty() {
        ends_on = false;
    }
    let mut out = done(if rewrites == 0 { source.to_string() } else { output }, findings);
    if rewrites > 0 && gate {
        // Only meaningful when the original compiles here at all; a file
        // whose imports don't resolve outside its app is migrated ungated.
        //
        // "The original" is the source as today's compiler can read it: its
        // `#f80` colors respelled (the same tokens), and, since a file with
        // `@` rebinds does not parse, each `@` read as nothing (the same
        // names, read instead of written).
        // An `else if` that was nesting is moved to its own line, where it
        // still is. A `state(key)`, on either side, is read as a plain
        // `state` (a `memo` it became is one in the output only).
        let mut original = colour_literals::respell_colour_literals(source)?;
        if nestings > 0 {
            original = else_if::nesting_as_read_today(&original)?;
        }
        if rebinds > 0 {
            original = at_rebind::blank_markers(&original)?;
        }
        let original = indented_by_end(&state_key::blank_keys(&original));
        let compiled = compiles(&original, &lint_opts).is_ok();
        if compiled
            && let Err(e) = compiles(&indented_by_end(&state_key::blank_keys(&out.output)), &lint_opts)
        {
            return Err(MigrateError::Gate {
                migration: "compile",
                reason: format!("migrate bug: a rewrite broke compilation — refusing to produce output ({e})"),
            });
        }
        if exports > 0 {
            let before = count_exports(&original)?;
            let after = count_exports(&out.output)?;
            if before.0 != after.0 || before.1 != after.1 + exports {
                return Err(MigrateError::Gate {
                    migration: EXPORT_TO_PUB,
                    reason: format!(
                        "migrate bug: rewriting {exports} `export`(s) to `pub` left {} of {} — \
                         refusing to produce output",
                        after.1, before.1
                    ),
                });
            }
        }
    }

    // `end` lines, last: over text every other gate has passed. It has its
    // own gate (the same AST), and a file that fails it keeps what the
    // migrations before it wrote.
    if ends_on {
        match end_lines::migrate(&out.output) {
            Ok(None) => {}
            Ok(Some(ends)) => {
                // Lines moved under the earlier findings' feet only below
                // the first `end`; this one is anchored where the text it
                // was given has that `end`.
                out.findings.push(Finding {
                    rule: END_LINES,
                    line: ends.first_line,
                    column: 1,
                    message: if ends.removed > 0 {
                        format!(
                            "{} `end` line(s) removed: indentation closes the block{}",
                            ends.removed,
                            if ends.kept > 0 {
                                format!(" ({} kept: the program needs them)", ends.kept)
                            } else {
                                String::new()
                            }
                        )
                    } else {
                        "indented by its `end`s, which the program needs".to_string()
                    },
                    fixable: true,
                });
                out.findings.sort_by_key(|f| (f.line, f.column));
                out.output = ends.text;
                out.ends_removed = ends.removed;
                out.ends_kept = ends.kept;
            }
            Err(reason) => out.skipped.push(Skipped { migration: END_LINES, reason }),
        }
    }
    Ok(out)
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
    fn state_keys_go_back_after_an_edit_of_the_import_lines() {
        let original = "import tune: *\nfn hp(id, d)\n  state(id) h = 100\n  state     odd = 1\n  h -= d\n  h\n";
        let blanked = blank_state_keys(original);
        assert_eq!(blanked.lines().nth(2), Some("  state     h = 100"));
        // The import line respelled and one added: the key is back where it was.
        let edited = blanked.replace("import tune: *", "import ./tune\nimport ui");
        assert_eq!(
            restore_state_keys(original, &edited).as_deref(),
            Some("import ./tune\nimport ui\nfn hp(id, d)\n  state(id) h = 100\n  state     odd = 1\n  h -= d\n  h\n")
        );
        // An edit that touched anything else is refused.
        assert_eq!(restore_state_keys(original, &blanked.replace("h -= d", "h -= 2")), None);
        assert_eq!(restore_state_keys(original, &blanked.replace("  state     h = 100\n", "")), None);
        // No site: the edited text is the answer.
        assert_eq!(restore_state_keys("state n = 0\n", "import a\nstate n = 0\n").as_deref(), Some("import a\nstate n = 0\n"));
    }

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
