# Module system follow-up

Findings from a review and test pass of the module system
([module-system.md](../module-system.md), [packages.md](../packages.md)) on
2026-10-11, at 6eb8f75. Two things were exercised: the stock CLI against
about a hundred small test apps covering each rule in those two docs, and the
Cheesecake engine (`~/cheesecake`), whose thirteen games were migrated to
explicit imports and `petal.toml` packages.

The short version: the system does what the docs say almost everywhere, and
the Cheesecake migration needed no script changes. Eight issues are open,
listed below with a repro each. None is fixed yet.

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

### 2. A malformed `petal.toml` is silently ignored by `run` and `check`

Severity: medium.

Any of these manifests beside a `main.ptl` that says `print("ran")` exits 0
from both commands and prints `ran`:

```toml
[package
name = bad
```

```toml
[package]
name = "needs3"
version = "0.1.0"
[requires]
core = "banana"
```

Also: a manifest with no `name`, a dependency with both `path` and `git`, an
unknown key. The error surfaces only from `petal install`, as a warning from
`petal packages`, or when an import of an unknown package fails — and there
it is misreported as `cannot find module 'x' (…): …petal.toml: line 5:
[requires] core: 'banana' is not a version requirement`.

A typo in the manifest therefore switches off `[requires]` enforcement and
the package name (and with it the `state` keys) without a word. packages.md
says a malformed entry "is an error naming the line".

### 3. `check` misses a qualified call to a private or missing export

Severity: medium. A run/check disagreement.

```petal ignore
// hud.ptl
fn hidden() 1 end

// main.ptl
import ./hud
print("before")
print(hud.hidden())
```

`check` exits 0. `run` prints `before` and then fails with
`module 'hud' has no export 'hidden' (declarations are private unless marked
`pub`)`, with no line, column or snippet. The value form (`hud.nope` with no
call) *is* caught by `check`, with a location; only the call form slips
through. The same holds for a name the module does not declare at all, and
through a facade.

module-system.md does say a qualified access "fails when it runs", but
`check` has what it needs to flag it, and the runtime error should carry a
location. Code that qualifies every cross-module call — which is how the
Cheesecake games are written — gets no static check on those calls today.

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

The error is in `m.ptl`, but the trace reads `in inner() [line 2, column 12]`
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
- The second and later errors of a multi-error report lack the `Error:`
  prefix.
- Unsupported forms give generic or misleading parse errors:
  - `import ui.hud` and `import ./level.ptl`: `Unexpected token: '.'`.
  - `import ./thing: v as vv`: `undefined variable 'as'`.
  - `import ./my-mod` parses as `import ./my` followed by the expression
    `-mod`: `cannot find module './my'`, or `undefined variable mod` when
    `my.ptl` exists.
  - `set tally.hits = 5`: `` `tally` is not defined `` rather than the
    read-only-`var` message.
- `petal fmt` does not normalise spacing after keywords in import lines
  (`import   ./m as q`, `pub   import`).
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

- module-system.md uses the real package name `bloom` in samples with exports
  it does not have (`import bloom/menu: open, close`, `bloom.open()`; the
  real names are `menu_open`, `menu_close`). The fences are `ignore`-tagged,
  but a reader who pastes them gets "has no export 'open'". A made-up package
  name would avoid it.
- `test/vitest/doc-snippets.test.ts` covers only `docs/`, and with
  `check --lenient`. Fences in `examples/**/README.md`, `garden/docs`,
  `core-runtime` and `integrations` are untested, which is how the two stale
  READMEs survived.
- `editor-support/tree-sitter-petal/grammar.js` comments still say
  `rust/src/…`.
- `core-runtime/glsl/tests/effects.ptl:4` points at Cheesecake's
  `engine/script/prelude/postfx.ptl`, now
  `engine/script/packages/cheesecake/postfx.ptl`.
- `examples/productivity/settings/README.md:205` records that
  `import preview` "did not give a module" because `preview` is also a
  prelude function. A qualifier outranks a prelude value since 1739d5c, so
  the note is probably obsolete.
- Broken links unrelated to modules:
  `docs/tasks/declarative-effect-refactoring.md` does not exist;
  `docs/README.md:23` links `examples/README.md` where `../examples/README.md`
  is meant.

## What was verified to work

With the stock CLI, `check` and `run` agreeing:

- **Import forms and qualifiers:** all three forms, `as`, a selective import
  binding no qualifier, the bare-qualifier error, a module exporting its own
  name, a local declaration taking the name back.
- **Lookup tiers:** file over selective over plain over prelude; the
  hides-a-builtin warning once per name; `core.len`.
- **Ambiguity:** an error only at bare use, pinning, importer-shadows-imported
  in either import order, one declaration by two routes, arity merging.
- **Paths and identity:** `./`, `../`, nesting, the bare-sibling message,
  cycles including the entry file; one load and one `var` cell for a module
  reached by two relative paths or by `./x` and `pkg/x`.
- **Facades:** whole and selective `pub import`, chains, privacy not widened,
  the re-export cycle.
- **Classes, vars, overloads:** cross-module methods, the duplicate class
  error, read-only exported `var` and the `get` rule, mixed `pub` overloads.
- **Packages:** the `src/` default and `modules =`, path dependencies,
  `[requires]` (reported by `check`, refused by `run`), `-I`, `PETAL_PATH`,
  `vendor/`, the bundled packages, and `init` / `add` / `remove` / `install`
  / `vendor` / `packages` / `bundle`.
- **`fix-imports`:** every row of the migration table except git
  dependencies, `--check`, `--diff`, idempotence, the ignore comment.
- **`fmt`** rewrites `export` to `pub`, as documented.

In this repository: every non-fixture `.ptl` passes `petal check` and
`fix-imports --check` (the eight that fail are the aspirational-syntax samples
and the two glsl shader fixtures); the doc snippet test passes 350 of 350;
the module-related cargo suites (`host_interfaces`, `manifest`, `modules`,
`native_decl`, `native_fn`, `packages`) and 14 vitest suites pass.

In Cheesecake:

- All 261 scripts pass `petal check` apart from the exceptions its AGENTS.md
  lists, and `fix-imports --check` wants no changes.
- All 223 scripts under `games/` and `docs/` run headless for 90 frames with
  no script error; the full ctest suite passes, 337 of 337.
- Through the engine: a loose multi-file game with no `petal.toml`, a
  packaged game with a path dependency and `src/` modules (`pkg/hud` and
  `./src/hud` are one module), an unmet `[requires]` refused before any line
  runs, a manifest whose dependency path does not exist (harmless, since the
  engine registers `cheesecake` in memory — the browser build relies on
  this), ambiguity errors, and an old-style script migrated with
  `fix-imports -I engine/script/packages`.
- Because of issue 3, the 9,037 qualified `module.name` uses in the games
  were scanned by script against each module's `pub` names: none points at a
  private or missing name.
- The one `petal check` warning in the repository was a real bug unrelated to
  modules: a continuation line starting with `-` in `games/moses/cam.ptl`
  dropped the drag-pan term (fixed in Cheesecake af3ff22).
