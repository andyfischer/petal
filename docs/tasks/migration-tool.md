Add a new subcommand in the CLI called migrate-old-code

This would be used as we change the language to help migrate old code into new code. So we'll add transformation rules as in there as we go. if we currently have any transformations like that then move them into this new tool. The other functions should only have logic that handles the latest syntax and so if there's any code using old syntax, it would need to use this subcommand first before that code could be used in any other way. 

This will be used for a variety of things, including the upcoming annotation syntax and possible other syntactic changes. 
## Status (9 November 2026): shipped

`petal migrate-old-code` exists (`core/src/migrate/`, handler in
`core/src/cli/source_tools.rs`; docs: [CLI.md](../CLI.md#migrate-old-code--rewrite-old-syntax),
`petal help migrate-old-code`, contributor note
[dev/migrations.md](../dev/migrations.md)).

- **Interface.** Rewrites in place; `--check` / `--dry-run` prints a unified
  diff and exits 1 if anything would change; `--only <name>[,…]`
  (repeatable); `--list`; paths, `-e` and `-` as `petal fmt` takes them (the
  walk skips dot-directories, `node_modules` and `target`); `-I`. Running it
  twice changes nothing. Each file that compiled must still compile, or
  nothing is written for it. Sites a migration cannot rewrite safely are
  listed on stderr "for a hand edit" and do not fail the run.
- **Registry.** `MIGRATIONS` in `core/src/migrate/mod.rs`: name, summary,
  date added, what it replaces, what the compiler does with the old form,
  whether it can leave sites for a hand edit, and whether it needs the
  whole batch.

### Inventory: what moved, what stayed

Old-syntax rewrites that existed before, and where they are now:

| Was | Rewrites | Now |
|---|---|---|
| lint `prefer-pub` (`lint/export_to_pub.rs`) and fmt's keyword pass (`fmt::normalize_keywords`) | `export` → `pub` | migration `export-to-pub` (`migrate/export_to_pub.rs`, `migrate/export_keyword.rs`). fmt is whitespace only; lint keeps a report-only `old-syntax` rule because the compiler says nothing about `export` |
| lint `const-decl-deprecated`, `config-let-deprecated` (`lint/lifecycle_keywords.rs`, `migrate_lifecycle_keywords`) | `const X = e`, `config let X = v` | migrations of the same names (`migrate/lifecycle_keywords.rs`); the compile warnings (also moved there) end "; run `petal migrate-old-code`" |
| lint `state-key-deprecated` (`lint/state_key.rs`) | `state(key)` → `memo(key)` where provable | migration of the same name (`migrate/state_key.rs`); the compile warning (`compiler/keyed_state.rs`) ends "; run `petal migrate-old-code`" |
| `petal fix-imports` (`fix_imports.rs`) | removed import forms (bare sibling imports, `: *`, `import creative`, missing imports of moved builtins) | migration `old-imports`, run last over the batch. `petal fix-imports` stays as that one migration alone (own `--check` list and `--diff`); the module loader's errors for the removed forms now name `petal migrate-old-code` |

Added since: `at-rebind` (2026-11-09), the first migration for a form the
parser no longer reads: the `f(@x)` rebind operator, removed to free `@` for
annotations (`migrate/at_rebind.rs`; it finds `@x` with the lexer and plans on
a parse with the `@`s blanked).

Stayed where they were, as style or correctness rules about current code:
`const-of-literal` (now `lint/const_of_literal.rs`), `state-as-constant`,
`config-not-data`, the `dm-hint-*` and `tunable-*` rules, and every other lint
rule; `apply-change` and `suggest` held no old-syntax handling.

### Decisions

- Migration names keep the ids the compiler's warnings already carry
  (`const-decl-deprecated`, …), so `// petal-lint-ignore <id>` both silences
  the warning and keeps the migration off that site. `prefer-pub` was
  renamed `export-to-pub`.
- `lint --rules-include=<old rule>` exits with a pointer to the migration
  instead of "unknown rule".
- The tool does not reformat: each migration edits its old form and re-pads
  trailing comments itself.
- Parsing of still-deprecated forms (`const X`, `config let`, `state(key)`,
  `export`) is untouched. Phase 6 of
  [the lifecycle plan](lifecycle-annotation-functions-refactor.md) now says
  its parse errors point to `petal migrate-old-code` and the rewrite lives
  only there.

### Left open

- `push`/`pop`, deprecated aliases of `append`/`drop_last`
  (`builtins/collections.rs`), are builtins, not syntax, and have no
  migration: a rename rule would need name resolution to avoid a user's own
  `push`.
- `core/src/cst_project.rs` and the parser still accept `export` (no removal
  is planned); when one is, its error should name the tool.
- Cheesecake (not edited): `tools/petal-check.ts` runs `petal fix-imports
  --check`, which still works unchanged. `third_party/petal/VENDORED.md`
  (lines 14 and 187) says `petal lint --fix` rewrites `const`/`config let`
  and `state(key)`; that becomes stale at the next re-vendor and should say
  `petal migrate-old-code`. `docs/petal-api.md:3625` uses
  `lint --fix --rules-include=state-as-constant`, which is still a lint rule.
  No Cheesecake script runs `petal lint --fix` for a migration.
