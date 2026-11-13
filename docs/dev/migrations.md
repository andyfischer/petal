# Migrations: rewriting old syntax

When a language change deprecates or removes a form, the rewrite from the
old form to the new one is a **migration** of `petal migrate-old-code`
(`core/src/migrate/`, user docs in [CLI.md](../CLI.md#migrate-old-code--rewrite-old-syntax)).
Nothing else rewrites old syntax: `petal fmt` lays an old spelling out as
written, `petal lint --fix` leaves it, and `apply-change`, `suggest` and the
LSP plan against the current forms only. Code in an old form is migrated
first, then everything else works on it.

## Adding a migration

1. **Plan it.** Write a planner in a new file of `core/src/migrate/` that
   takes the parsed statements and the source chars and returns
   `crate::lint::Fix`es: an anchor (char offset of the site), a message
   (`"<old> is deprecated; write <new>"`), and the `crate::rewrite::Splice`s
   that rewrite it. A site the migration cannot rewrite safely gets no
   splices: it is listed for a hand edit. Edit the old form and nothing else;
   the output is not reformatted, so re-pad a trailing comment you push out
   of its column (`lifecycle_keywords.rs` and `export_keyword.rs` show how).
2. **Register it.** Add a `Migration` row to `MIGRATIONS` in
   `core/src/migrate/mod.rs`: a kebab-case `name` (the same id the compiler's
   warning carries, if it warns), a one-line `summary`, `added` (today, as
   `YYYY-MM-DD`), `replaces` (the form and when it was deprecated), `old_form`
   and `partial`. Then call the planner in `run()`, in order: each migration
   plans over the text the previous one left (`reparse(&p.text)`) and applies
   through `p.apply(NAME, fixes)`, which maps findings back to the original
   lines and honours `# petal-lint-ignore NAME`. A migration that needs the
   whole batch of files and a module resolver is `batch: true` and runs from
   `handle_migrate` in `core/src/cli/source_tools.rs`, as `old-imports` does.
3. **Point the old form at it.** While the parser still accepts the form, the
   compiler's warning ends `; run `petal migrate-old-code` [<name>]`
   (`migrate::RUN_HINT`; see `deprecated_lifecycle_decls`). If the compiler
   accepts it silently (a pure spelling, like `export`), set
   `old_form: OldForm::Accepted` and `petal lint`'s report-only `old-syntax`
   rule reports it for you. When the form is removed from the parser, its
   parse error names the command, and the migration keeps working on a parse
   that still recognises the old form (keep that recognition, or a dedicated
   pre-parse, inside `core/src/migrate/`). `at_rebind.rs` is the example of
   the second: it finds the removed `@x` with the lexer, blanks each `@` to a
   space so the file parses with every offset unchanged, and plans its
   rewrite on that parse. Such a migration runs first in `run()`, since the
   ones after it need the file to parse. `state_key.rs`
   (`state-key-deprecated`) is the example of a removed form most of whose
   sites are left for a hand edit: it finds each `state` + `(` with the
   lexer, and every migration's parse (`reparse`) and compile gate reads the
   file through `state_key::blank_keys`, which turns each `(key)` group to
   spaces (the same offsets, a plain `state`). So a site nobody can rewrite
   does not hold the other migrations back; only `end-lines`, which formats
   the real text, waits for the hand edit, and the CLI's `old-imports` puts
   the keys back after its own edit (`migrate::restore_state_keys`). When the form is removed from the
   *lexer*, the lexer keeps a legacy mode for the migrator alone:
   `colour_literals.rs` reads the removed `#f80` color with `Lexer::legacy`
   (`legacy_lex`), and it and `hash_comments.rs`, which need tokens and
   trivia but no parse, run before everything else, because until they have
   run the file does not lex. `else_if.rs` (`else-if-nesting`) is the
   example of a form whose *meaning* changed in the lexer: `else if` became
   one token, so it lexes with `Lexer::unmerged_else_if`, parses that old
   reading (`parse_source_unmerged_else_if`), and gates on IR equality
   against the old reading respelled for today's compiler.
4. **Test it.** Add a fixture row to `FIXTURES` in
   `core/tests/all/migrate_old_code.rs` (old source → migrated source; the tests
   check it alone and with every other migration, idempotency, that both
   sides compile, and that no deprecation warning is left), plus focused
   tests of the edge cases next to the planner. CLI behaviour (`--check`,
   the walk, `--only`) is covered once for all migrations in
   `cli/tests/migrate_old_code_cli.rs`.
5. **Document it.** The migrations table in [CLI.md](../CLI.md#migrate-old-code--rewrite-old-syntax),
   the `migrate-old-code` page in `core/src/cli/help.rs`, and a CHANGELOG
   entry that tells users to run `petal migrate-old-code`.

## The layout migration and Markdown

`end-lines` (`end_lines.rs`) is not a rewrite of an old form: `end` is still
the language (`OldForm::Optional`: no warning, no lint report). It formats
the file and removes the `end`s indentation makes redundant
(`fmt::remove_block_ends`), so it runs last of the text migrations, after
`run()`'s compile gate, and has its own gate (the same AST). A file that
fails it keeps what the other migrations wrote and comes back in
`MigrateOutcome::skipped`; any other gate failure is
`MigrateError::Gate` and nothing is produced for the file. A new migration of
an old form goes before it in `MIGRATIONS` and in `run()`.

`petal migrate-old-code --markdown` (`markdown.rs`) runs `migrate_source`
over each `petal` fence with `MigrateOptions::fragment` on: a fence that does
not parse still gets the migrations that only read tokens. A migration that
needs no parse should be added to that path (`tokens_only` in `run()`).

## Rules of thumb

- **Idempotent.** A migration rewrites only the old form, so its output has
  nothing left for it. The fixture tests enforce this.
- **Same program.** If the original compiles, the migrated text must compile
  (`run()` gates every file; a failure is reported as a migration bug and
  nothing is written). Prefer rewrites that are provably the same program;
  where that needs a judgement, list the site for a hand edit instead.
- **Old-syntax rewrites do not go in lint.** A lint rule is about which
  current code to write; a test in `core/src/migrate/mod.rs` checks that no
  migration name is also a lint rule.
