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
Every repro was re-run on a debug build of `4fcedb6`. Line numbers are as of
that commit. Ordered by value per effort: the first four are small and
independent, and issue 1 needs a decision first.

### Step 1: issue 4, nested `import` (about 10 lines, parser)

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

### Step 2: misleading parse errors after an import (about 30 lines, parser)

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

### Step 3: issue 5, trace frames without file names (about 5 lines)

**Cause.** `full_stack_trace` (`backend/errors.rs`, around 248) names the
entry file's frames only when some *call site* is in a module. Frames show
where each call was made, so when every call is made from `main.ptl` but
the error is in `m.ptl`, the frames stay bare.

**Fix.** Pass the failing span in, and treat `span.file != ENTRY_FILE` as
crossing modules too.

**Tests:** `core/tests/cli_args.rs`, vitest `error-format` and
`error-positions`.

### Step 4: issue 8, nested packages reachable from the outer one (about 20 lines)

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

### Step 5: issues 6 and 7, messages that name the wrong module (about 30 lines each)

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

### Step 6: issue 1, a loose file named like a package (small code change, needs a decision)

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
`petal check main.ptl` and `petal run main.ptl`.

### 1. A loose file named like a bundled package breaks the program

Severity: medium-high.

```petal ignore
// color.ptl
pub fn tint(c) "tinted" end

// main.ptl
import ./color
import ui
print(tint(1))
```

Both `check` and `run` fail:

```
Error: two different sources are both the module 'color': …/color.ptl and
<bundled:color>/color.ptl (imported by ui.ptl)
```

`ui` imports the bundled `color`, and a loose file is named by its path from
the entry file, so the two collide on the canonical name `color`. The same
happens for any loose sibling named after a package the program loads,
directly or through another package. A loose file and a package should be
different modules; module-system.md does not mention the limit.

- Workaround: give the directory a `petal.toml`, which makes the file
  `<package>/color`. Every Cheesecake game has one, so none is affected.
- Knock-on: `petal fix-imports` abandons every import it would add to such a
  file ("adding import … would not compile … left as it was").

### 4. An `import` nested in a block or function is silently ignored

Severity: medium-low.

```petal ignore
fn f()
  import ./does_not_exist
  1
end
print(f())
```

Runs clean and prints `1`: no error, nothing loaded, nothing bound, the path
never resolved. The same inside `if … end`. The docs list "imports anywhere
but the top of a file, including conditional imports" as unsupported, but the
"must appear before any other statement" error only fires at top level.

### 5. Stack trace frames lose the file name when every frame is in the entry file

Severity: low.

```petal ignore
// m.ptl
pub fn inner(x)
  x + nil
end

// main.ptl
import ./m
fn outer() m.inner(1) end
outer()
```

The headline now names the file (`[m.ptl line 2, column 3]`), but the
frames still read `in inner() [line 2, column 12]`
/ `in outer() [line 3, column 1]`, so line 2 looks like `main.ptl`'s. With a
module-to-module call in the chain every frame is named, as documented.

### 6. The arity-merge warning suggests the wrong module

Severity: low.

`base` exports `score(points)`, `level` exports `score(points, bonus)`, and
the entry file plainly imports both. `score(1, 2)` resolves to `level`'s, but
the warning on that call says to qualify it as `base.score`; following the
advice produces an arity error. The warning is also printed for
`score(1, 2, 3)`, which is already an error.

### 7. Facade ambiguity errors name things the user's file cannot write

Severity: low.

A file that says only `import ./bloomx`, where `bloomx` re-exports `shared`
from both `button` and `menu`, gets: "write `button.shared` or `menu.shared`,
or pin one with `import ./button: shared`". Neither qualifier is bound in
that file, and the relative path is right only by accident of the layout.

### 8. A nested package's files are importable through the outer package

Severity: low; code or docs, whichever is intended.

With `proj/petal.toml` (`modules = "code"`) and a second manifest at
`proj/code/inner/petal.toml`, `import proj/inner/inner` works. packages.md
says a nested package's files "are not modules of the one around it".

### Smaller things

- A compile error inside an imported module has no source snippet and uses a
  different format (`Error: badsyn.ptl: … [line 2, column 3]`), contrary to
  "Source snippets … all show the right file".
- Unsupported forms give generic or misleading parse errors:
  - `import ui.hud` and `import ./level.ptl`: `Unexpected token: '.'`.
  - `import ./thing: v as vv`: `undefined variable 'as'`.
  - `import ./my-mod` parses as `import ./my` followed by the expression
    `-mod`: `cannot find module './my'`, or `undefined variable mod` when
    `my.ptl` exists. The quoted form (`import "./my-mod" as my_mod`, since
    1064dcd) is the supported spelling, but the unquoted one still gives no
    hint to use it.
  - `set tally.hits = 5`: `` `tally` is not defined `` rather than the
    read-only-`var` message.
- `petal fmt` does not normalise the spacing in `pub   import ./m`
  (`import   ./m as q` is normalised now).
- `petal lint --fix` for `prefer-pub` also reformats the whole file.
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
