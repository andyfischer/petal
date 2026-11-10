# Module system follow-up

Findings from a review and test pass of the module system
([module-system.md](../module-system.md), [packages.md](../packages.md)) on
2026-10-11, at 6eb8f75. Two things were exercised: the stock CLI against
about a hundred small test apps covering each rule in those two docs, and the
Cheesecake engine (`~/cheesecake`), whose thirteen games were migrated to
explicit imports and `petal.toml` packages.

The short version: the system does what the docs say almost everywhere, and
the Cheesecake migration needed no script changes. Issues 2 and 3 were fixed (2026-10-11, 2026-10-13) and removed; the other
numbers are kept. Every issue below was re-checked against `1064dcd` on 2026-11-02 and
still reproduces unless it says otherwise.

## Fix plan

Causes found on 2026-11-03 by reading the loader, parser and compiler.
Steps 1–4 landed the same day, with tests in `core/tests/parse_hints.rs`,
`core/tests/modules.rs` and `core/tests/packages.rs`. Their issues are gone
from the list below; their sections stay here as the record of the cause
and the choice made. Step 4 keeps a relative import into a nested package
working: the file is loaded as a module of the nested package, so it still
has one name (confirmed by the owner). Step 6 landed the same day as
decided: loose files are named `./level`, with no fallback for state saved
under the old names. Step 5 landed on 2026-11-04: the arity-merge warning
names the module whose set the call reaches, by this file's qualifier, and
says nothing on a call no set accepts; a facade's ambiguity names the facade
and spells an import from the using file (`import_spelling` in
`core/src/compiler/mod.rs`). Every issue in the plan is fixed.
Every repro was re-run on a debug build of `4fcedb6`. Line numbers are as of
that commit. Ordered by value per effort: the first four are small and
independent, and issue 1 needs a decision first.

### Step 1: issue 4, nested `import`: done 2026-11-03

**Cause.** The "imports must come first" check in `Parser::parse_program`
(`core/src/parse.rs`, around line 225) looks only at top-level statements.
`parse_stmt` already knows whether it is at top level (`top_level`, around
line 557) and uses that to reject a nested `test` block, but `Token::Import`
and `pub import` never check it. The loader's `split_imports`
(`module.rs`, around line 1661) scans only the top level. So a nested
`StmtKind::Import` reaches `desugar.rs` and `compiler/stmt.rs`
(`StmtKind::Import(_) => {}`), which drop it. It is also ignored inside
`test "…" do … end`.

**Fix.** In `parse_stmt`, when `!top_level` and the statement starts with
`import` (or `pub`/`export` then `import`), return an error at the keyword:
"an `import` is only allowed at the top of a file, before any other
statement". The parser is the right place, because every frontend goes
through `parse_program`: check, run, the LSP, fmt and reload. Make the
compiler's no-op arm an internal error as a backstop. No in-repo `.ptl` has
an indented import.

**Tests:** `core/tests/parse_hints.rs`, `test/vitest/import-forms.test.ts`.

### Step 2: misleading parse errors after an import: done 2026-11-03

**Cause.** `parse_import` (`parse.rs`, around 920–1008) returns as soon as
the path, `as` or name list is complete. Statements need no separator on one
line, so whatever follows is parsed as a new statement:

- `-mod` becomes an expression (a "discarded value" warning, then
  "cannot find module './my'");
- `.hud` and `.ptl` give "Unexpected token: '.'";
- `as vv` gives "undefined variable `as`".

**Fix.** Before `finish_import`, require a newline or end of file, and
otherwise give a targeted error:

- a `-` right after a segment: "a hyphenated name must be quoted: `import
  \"./my-mod\" as my_mod`";
- `.ptl`: "drop the extension";
- any other `.`: "module paths use `/`: `import ui/hud`";
- `as` after a name list: "a selective import cannot rename a name";
- anything else: "an import ends at the end of its line".

No in-repo source puts anything after an import on its line.

### Step 3: issue 5, trace frames without file names: done 2026-11-03

**Cause.** `full_stack_trace` (`backend/errors.rs`, around 248) names the
entry file's frames only when some *call site* is in a module. Frames show
where each call was made, so when every call is made from `main.ptl` but
the error is in `m.ptl`, the frames stay bare.

**Fix.** Pass the failing span in, and treat `span.file != ENTRY_FILE` as
crossing modules too.

**Tests:** `core/tests/cli_args.rs`, vitest `error-format` and
`error-positions`.

### Step 4: issue 8, nested packages reachable from the outer one: done 2026-11-03

**Decision: change the code, not the doc.** `scan_modules` (`package.rs`,
around 1105) already skips a directory with its own `petal.toml`. But the
disk branch of `Package::load` (around 490) reads any file under the module
directory. It is worse than the report says: `import proj/inner/inner` and
`import ./code/inner/inner` load the same file as two different modules,
which breaks the identity rule.

**Fix.** In `Package::load`, walk from the module directory down to the
file's parent. If a directory has a `petal.toml`, refuse with an error that
names the nested package and suggests
`inner = { path = "code/inner" }`. Do not check membership in
`info.modules` instead: that list is a depth-limited snapshot and would
reject files the LSP or a reload adds later. Also decide whether a relative
import from the outer package into the nested one should be rejected too.
Nothing in-repo relies on the current behaviour. Add a test to
`core/tests/packages.rs`.

### Step 5: issues 6 and 7, messages that name the wrong module: done 2026-11-04

- **Issue 6.** `check_import_call` (`compiler/mod.rs`, around 1460–1500)
  always suggests the *first*-imported module, whatever arity the call
  matches. It uses the canonical name's last segment rather than the
  qualifier bound in this file, so it is also wrong under `as`. It never
  checks that any arity fits, which is why `score(1, 2, 3)` gets the
  warning as well as the error. **Fix:** record
  `(module, qualifier, term)` per merged offer, suggest the one whose
  arities contain the call's, and skip the warning when none does.
- **Issue 7.** The `AmbiguousImport` is built while compiling the facade,
  using the facade's own qualifiers, and then rendered at the user's bare
  use. **Fix:** record the facade, and when the use is in another module,
  say "'kit/bloomx' re-exports `shared` from both 'kit/button' and
  'kit/menu'", with an import spelled from the using file. This is easier
  after step 6.

Tests: the unit tests near `module.rs` 1948–2195. One of them asserts
issue 7's exact text.

### Step 6: issue 1, a loose file named like a package: done 2026-11-03

**Cause.** Module identity is one string.

- A package module is `pkg/mod`, or plain `pkg` for the module named after
  its package (`Package::canonical_name`, `package.rs` around 517).
- A loose file is named by its path from the entry directory with no
  prefix (`relative_module_name`, used at `module.rs` around 1519), so
  `color.ptl` is `color`.
- The loader keys loaded modules by that string, and the second origin
  under the same key is the "two different sources" error.

The same name confusion makes `native_decl::builtin_package_of_module` treat
a loose `noise.ptl` as the `noise` package (it is used by `fix-imports`), and
it is why `fix-imports` abandons imports into such a file.

**Fix.** Name loose files with their relative prefix: `./color`,
`./ui/hud` (`../shared/rng` already has one). A package name is always an
identifier, so the two can no longer collide. The LSP already rebases the
same way (`project_host.rs`, around 344).

- `display_name_for` strips the `./`, so error messages still say
  `color.ptl`. Bundled modules could display as `<bundled:color>/color.ptl`
  to stay distinct.
- `Env::resolve_import` must return the same name as the loader.
- The qualifier a file binds is the last segment, so it is unchanged.

**What it changes for users (the decision to make).** Names qualified by a
loose module change from `level::x` to `./level::x`:

- state keys: saved `--state-storage` files and hot-reload state for loose
  modules are dropped once, unless state load gets a fallback to the old
  key;
- static-cache slots: a one-time miss;
- `--term level::x`, a host's `call_function("level::f")`, and the text of
  `state` warnings.

Code that takes the part after `::` (provenance, direct manipulation, the
classifier) is unaffected. Hot reload and `--set` key on file paths and
display names, so they are unaffected too. Hosts outside this repo
(cheesecake, worlds-fair) may call loose-module functions by qualified name,
so check them before landing. Update `module-system.md` (the module-names
section).

**Tests:** none cover the collision. Neighbours: `module.rs` unit tests
(`relative_module_name`, the package-identity tests), `core/tests/modules.rs`,
`core/tests/manifest.rs`, `core/tests/state_json_paths.rs`,
`core/tests/lsp_tests.rs`, and vitest `module-paths`, `modules`,
`fix-imports`.

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
- ~~`petal lint --fix` for `prefer-pub` also reformats the whole file.~~
  Resolved: the rewrite is `petal migrate-old-code`'s `export-to-pub`, which
  edits the keyword and nothing else (2026-11-09).
- A method call on a value from another module (`mk().normalize()` without
  `import vec2`) passes `check` and fails at run. Documented; `check` cannot
  see it.
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
  `core-runtime` and `integrations` are untested, which is how the two stale
  READMEs survived.
- `editor-support/tree-sitter-petal/grammar.js` comments still say
  `rust/src/…`.
- `core-runtime/glsl/tests/effects.ptl:4` points at Cheesecake's
  `engine/script/prelude/postfx.ptl`, now
  `engine/script/packages/cheesecake/postfx.ptl`.

The list of what was verified to work (import forms, lookup tiers, packages,
the Cheesecake migration) is in `git show b0dfccc7:docs/tasks/module-system-followup.md`.
