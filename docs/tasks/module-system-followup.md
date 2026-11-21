# Module system follow-up

Findings from a review and test pass of the module system
([module-system.md](../module-system.md), [packages.md](../packages.md)) on
2026-10-11, at 6eb8f75. Two things were exercised: the stock CLI against
about a hundred small test apps covering each rule in those two docs, and the
Cheesecake engine (`~/cheesecake`), whose thirteen games were migrated to
explicit imports and `petal.toml` packages.

The short version: the system does what the docs say almost everywhere, and
the Cheesecake migration needed no script changes. Issues 2 and 3 were fixed (2026-10-11, 2026-10-13) and removed; the other
numbers are kept. Every issue below was re-checked on 2026-11-21 and still reproduces.

Every step of the 2026-11-03 fix plan (issues 1 and 4–8) has landed. Its
record of each cause and decision, which source comments cite as "issue N",
is in `git show 5b28d9f5:docs/tasks/module-system-followup.md`.

## Open issues

Each repro is a handful of files in one empty directory, run with
`petal lint main.ptl` and `petal run main.ptl`.

### Smaller things

- A compile error inside an imported module has no source snippet and uses a
  different format (`Error: badsyn.ptl: … [line 2, column 3]`), contrary to
  "Source snippets … all show the right file".
- A misleading error:
  - `set tally.hits = 5`: `` `tally` is not defined `` rather than the
    read-only-`var` message.
- `petal fmt` does not normalise the spacing in `pub   import ./m`
  (`import   ./m as q` is normalised now).
- `fix-imports` logs `` `import level: *` -> `import level` `` for what it
  writes as `./level`.
- A failed selective import lists methods among the exports
  (`exports: Circle, Circle.area, …`).

## Documentation follow-up

Fixed in beb6783: two example READMEs that imported a sibling file by bare
name through `-I`, bloom source comments that said the host implicitly
imports `ui`, and paths that used pre-rename directory names.

Still open:

- `test/vitest/doc-snippets.test.ts` covers only `docs/`, and with
  `check --lenient`. Fences in `examples/**/README.md`, `garden/docs`,
  `core-stdlib` and `integrations` are untested, which is how the two stale
  READMEs survived.
- `editor-support/tree-sitter-petal/grammar.js` comments still say
  `rust/src/…`.
- `core-stdlib/glsl/tests/effects.ptl:4` points at Cheesecake's
  `engine/script/prelude/postfx.ptl`, now
  `engine/script/packages/cheesecake/postfx.ptl`.

The list of what was verified to work (import forms, lookup tiers, packages,
the Cheesecake migration) is in `git show b0dfccc7:docs/tasks/module-system-followup.md`.
