# Lifecycle annotations as functions: `const(…)` and `tunable(…)` replace `const` and `config`

Status: Phase 1 shipped 9 November 2026 (`const(…)` reaches parity; see
"Phase 1: what shipped" below). Phase 2 shipped the same day (the fixers,
opt-in and report only; see "Phase 2: what shipped"), and so did Phase 3
(the Petal corpus, the docs and petal-lang.org; see "Phase 3: what
shipped"), Phase 4 (Cheesecake; "Phase 4: what landed") and Phase 5 (the
deprecation warnings; "Phase 5: what shipped", with the rewrite since moved
from `petal lint --fix` to `petal migrate-old-code`: "Phase 5, revised"). Phase 6
(removal, one release after Phase 5) and Phase 7 are planned. Settings without
`config` are half done:
Phases 1 and 2 of the old removal proposal shipped on 6 November (commits
47e5e905 and ab5e99a7), and Cheesecake vendored them (a18ce96a). That
proposal is folded into this file and deleted. Read it with
`git show d6fd8291:docs/proposals/remove-config-keyword.md`. The earlier
`config-let-vs-tunable.md`, which recommended keeping the keyword, is
deleted too: `git show d6fd8291:docs/proposals/config-let-vs-tunable.md`.

## Goal

Two declaration keywords change nothing about what a program computes:

- `const NAME = e` is a `let` plus a compile-time check that `e` is static
  (`language-guide.md`, "`const`"; `features/static-values.md`).
- `config let NAME = e` is a `let` that hosts list as a setting and that
  direct manipulation prefers (`syntax/config-let.md`).

Both become **identity builtins on the initializer**, like `tunable` and
`rigid` already are:

```petal
// before
const TILES = [tile("grass", 1), tile("road", 0)]
pub config let SPEED = tunable(4.0, 0, 10)
pub config let START = "title"

// after
let TILES = const([tile("grass", 1), tile("road", 0)])
pub let SPEED = tunable(4.0, 0, 10)
pub let START = tunable("title", options: ["title", "select", "play"])
```

The rule this sets for the language: **a keyword changes what a program
does** (`let`, `var`, `state`, `memo`, `fn`). **An annotation changes only
what the checker or the tools do with a value**, and it is a reserved
builtin function that returns its first argument. At the end, `const` and
`config` are no longer keywords.

## The annotation functions, shared mechanism

| | `const(e)` | `tunable(v, …)` | `rigid(v)` |
|---|---|---|---|
| Run time | identity | identity (a `CONST` native call today) | identity |
| Read by | the static classifier: an error unless `e` is static | hosts (a setting), DM (a knob), the reload diff | DM (never move) |
| Recorded as | `Term.is_const` on the value term | `Term.is_config` on the value term and its data literals | DM hint table |
| Where it means something | the whole initializer of a top-level `let` (Phase 7 widens this) | the whole initializer of a top-level `let` = a setting; anywhere else = a DM hint | anywhere |

Shared rules, most already true of `tunable` and `rigid` (old proposal §2,
built in Phase 1 of it):

1. **Reserved names.** `core/src/compiler/hint_names.rs` refuses a binding
   of `tunable` or `rigid` anywhere (fn, let, parameter, import alias, …).
   Add `const`. That costs nothing, because `const` is a lexer keyword
   today and no program can bind it. Because the name is reserved, readers
   can soundly recognise the call by name (`static_value::tunable_args` is
   the model).
2. **Typed as identity.** `typecheck/builtin_types.rs:72` returns the
   first argument's type for `tunable` and `rigid`. Add `const`.
3. **Declared in the core package**, so `petal lint`, the LSP, `Builtins.md`
   and `docs/stdlib.json` know it: `native fn const(value)` beside
   `tunable` in `core/packages/core/src/dm.ptl` (or a new `annotations.ptl`
   that both move to; see open question 2).
4. **Erased in bytecode.** `const` is a compiler intrinsic. It compiles its
   argument, sets the flag on that term, and emits no call, so the IR of
   `let X = const(e)` is the IR of `const X = e` today. That keeps the
   static cache, `ir_equiv`, the golden IR and the differential oracle
   unchanged. `tunable` stays a native call for now. Lowering it the same
   way is the old proposal's open question 4, and it is the same code once
   `const` has paved it.

## Part A: `const(…)`

### What exists today

- Lexer: `const` is a reserved keyword, `Token::Const` (`lexer.rs:16, 228,
  258`).
- Parser: `parse_let(…, LetKeyword::Const, …)` at `parse.rs:584` (statement)
  and `:660` (after `pub`). The flag is `StmtKind::Let.is_const`
  (`ast.rs:601`, set at `parse.rs:737`). Statement-start recovery (`:1410`),
  the `config const` error (`:703`), and the keyword name for messages
  (`:76`).
- Compiler: `compiler/stmt.rs:81` copies the flag onto the value term
  (`Term.is_const`, `program.rs:417`).
  `misplaced_const_decls` (`compiler/mod.rs:2911`) makes a `const` that is
  not at the top level an error. `hoistable_const_lets` (`:2838-2930`)
  hoists literal-only `let`/`const` declarations.
- **The check reads only the term flag.** `static_classify/mod.rs:3006`:
  `declared_const = g.defs.iter().any(|d| p.get_term(*d).is_const)`, then
  `ConstCheck::{Constant, NotConstant}`. The messages are in
  `env/mod.rs:1094-1238`: not constant, built again on every run, is a
  function. `DeclKind::Const` is derived from the terms (`:1326`). So
  **once the compiler sets `is_const` from `const(…)`, the check,
  `show-static`, the runtime "built again on every run" warnings and the
  static cache all work unchanged.**
- Other readers of the AST flag, each needing "flag or `const(…)`
  initializer": `cst_project.rs:327` (`Token::Const` in the CST),
  `source_diff.rs:604` (`ConstructKind::Const`), `fn_introspect.rs:426`,
  `lsp/symbols.rs:55-67` (symbol kind "const"), `lsp/document.rs:1427`
  (keyword completion), `apply_change/target.rs:259` and
  `convert_to_var.rs:89`, `lint/dm_hints.rs:87-100`,
  `lint/config_data.rs:28`, `static_value::is_setting`
  (`static_value.rs:714`), `lint/state_const.rs` (the `state-as-constant`
  lint, which suggests writing `const`), and `env/reload.rs:748` (a token
  scan for `let|var|const NAME`).
- Outside core: tree-sitter (`grammar.js`, `highlights.scm`, regenerated
  `src/`, `test/corpus/basics.txt`), vim syntax, petal-lang.org's
  highlighter and mirrored docs.

### Design

```
const(value) -> value
```

- **`let X = const(e)` and `pub let X = const(e)` at the top level** mean
  exactly what `const X = e` means today: the binding must be static, must
  not be assigned again, and must not hold a function. The error text keeps
  its shape: "`X` is declared `const(…)` but is not constant", followed by
  the chain of reasons.
- **Anywhere else, in Phases 1 to 6,** it is the same error as a misplaced
  `const` today: "`const(…)` checks a top-level `let`; …". This covers a
  sub-expression (`let T = {a: const(x)}`), a function body, `var X =
  const(e)` (a `var` is assigned by definition) and `state x = const(e)`
  (use a `let` and initialise the state from it). Phase 7 relaxes the first
  two.
- **`const` around a setting**, `let X = const(tunable(4))`: an error, with
  the message today's `config const` has: "a setting of literals is already
  static; drop `const(…)`". Without it, the `const` would hide the
  `tunable` and the binding would silently stop being a setting.
  `tunable(const(e))` is legal and pointless, and `config-not-data` already
  warns about it.
- **The hoist** (`hoistable_const_lets`) treats `const(e)` as `e`, so a
  literal table wrapped in `const` is still in place before the file's first
  statement.
- **The construct kind** for hot reload stays `ConstructKind::Const`. Adding
  or removing the wrapper is structural, the same as `let` ↔ `const` today.
- **The name `const`**: the docs call the property "static", so
  `static(…)` would match the prose. `const` is kept because every reader
  knows the word and every error message already says it. See open
  question 1.

### Before and after

```petal
// before (examples, Cheesecake: 316 declarations)
pub const ART = "../trailkeeper/art"
const COSTS = map(TILES, fn(t) -> t.cost)
const PALETTE = [
  #101418, #ff2e88,
]

// after
pub let ART = "../trailkeeper/art"          // a literal: `const` added nothing (lint, below)
let COSTS = const(map(TILES, fn(t) -> t.cost))
let PALETTE = const([
  #101418, #ff2e88,
])
```

The migration fixer always wraps, so no check is lost. A separate opt-in
lint, `const-of-literal` (report only), lists wrappers whose argument is
literal data, where `const` checks nothing a reader cannot see. Whether
`--fix` drops them is open question 3.

## Part B: settings without `config`

The design is settled and Phases 1 and 2 of the old proposal have shipped.
In short:

- **A setting** is a top-level `let` or `pub let` whose whole initializer
  is a core `tunable(…)` call. `tunable` takes any static data (numbers,
  strings, bools, colours, records, lists), and every number literal inside
  is a DM knob. `min`, `max` and `step` are numbers, and `options:` is a
  list of allowed values. `static_value::is_setting` is the one
  definition, and `setting_value_expr` unwraps the call, so path `X` names
  `v` in every reader.
- **For hosts:** `StaticBinding.is_config` now means "is a setting".
  `pb_source_bindings_json` has a `"tunable"` object, with `"config"` and
  `"range"` as deprecated mirrors. The reload report has
  `values[].tunable` and the construct kind `"tunable let"`. The bridge has
  `pb_vm_set_tunable` (with `pb_vm_set_config` as an alias) and
  `pb_source_set_setting`.
- **Lints in place:** `tunable-value-not-in-options`, `tunable-on-var`,
  `tunable-not-whole-initializer`, `dm-hint-bounds`, and `config-not-data`
  for a computed setting.
- **Selectors:** `knob` / `--knob-strict` are canonical; `config` /
  `--config-strict` are aliases.
- **Proven equal:** `core/tests/all/setting_parity.rs` rewrites every
  single-line `config let` in `examples/` and `core/tests` into the
  `tunable` form and compares bindings, set-by-name and config-marked
  constants. It passed on 132 settings.

What remains is migration, deprecation and removal, Phases 2 to 6 below.
The one design point still open is the owner questions carried over in
open question 4.

## Phases

Each phase ends green on `make test`, `make test-c-bridge`, the DM v2 study
golden and `check-dm-samples.mjs`. No phase breaks a vendored host before
that host has been updated.

### Phase 1: `const(…)` reaches parity (additive; the keyword untouched)

1. Add `const` to `RESERVED_HINT_NAMES` (`hint_names.rs:17`), the core
   package declaration, and the identity type in `builtin_types.rs`.
2. Parser: `const` followed by `(` in expression position parses as a call
   to the builtin. `const` followed by a name at statement start stays the
   declaration. The lexer keeps `Token::Const` until Phase 6. The parser
   turns it into an identifier when `(` follows, as `config` is handled
   today (a contextual word).
3. A helper `static_value::const_call_arg(&Expr) -> Option<&Expr>`, and
   `Stmt`-level `declares_const(stmt) = is_const || const_call_arg(value)`.
   Use it in every reader in the list under "What exists today", in place
   of the field.
4. Compiler: the intrinsic. In `compile_bound_expr` for a top-level `let`
   whose initializer is `const(e)`, compile `e` and set `is_const` on its
   term. Anywhere else, and on `var`/`state`, report the misplacement error
   (extend `misplaced_const_decls`) and compile it as `e`. Add the
   `const(tunable(…))` error. Make the hoist look through the call.
5. Messages: `env/mod.rs:1094-1238` and the classifier say "`X` is declared
   `const`". Change that to name whichever form was written, or one wording
   for both, such as "`X` must be constant".
6. **Parity test** `core/tests/all/const_parity.rs`, on the model of
   `setting_parity.rs`. It rewrites every `const` declaration in
   `examples/`, `core/tests` and the language-guide snippets, then asserts
   for both forms: identical IR (`ir_equiv`), identical `show-static --json`
   (kind, class, `const_check`), identical diagnostics (minus the wording),
   and identical hot-reload construct kinds for an edit of each initializer.
   This is the gate for Phase 4.

#### Phase 1: what shipped (9 November 2026)

- `const` is in `RESERVED_HINT_NAMES`, typed as the identity, declared
  `pub native fn const(value)` in `core/packages/core/src/dm.ptl` and
  registered as an identity native (`builtins/hints.rs`), which only a
  first-class use would reach. `Builtins.md` has an entry, and
  `docs/stdlib.json` is regenerated.
- The parser reads `const` followed by `(` as the name `const`, in an
  expression, at statement start, and as a `native fn` name; the CST
  projection does the same. The lexer still emits `Token::Const`.
- `static_value::const_call_arg`, `declares_const_let` and `declares_const`.
  They are used by the compiler, `is_setting`, `setting_value_expr` (so the
  path `X` names `e`), the static evaluator, `source_diff`'s construct kind,
  the LSP symbol kind, `apply_change`'s declaration kind (and so
  `convert-to-var`), the shadow checker's keyword, and `dm_hints`. The other
  readers in the list needed nothing: `cst_project.rs` and
  `fn_introspect.rs` report the keyword flag as written, `config_data.rs`
  goes through `is_setting`, `state_const.rs` reads the classifier's
  `DeclKind::Const` (derived from the term), and `env/reload.rs`'s token scan
  already matches `let NAME`.
- The compiler erases every `const(e)` call to `e`'s terms (no call, no extra
  source-map entry) in `compile_expr`, and a top-level `let X = const(e)`
  marks the term `is_const`. `misplaced_const_decls` reports `const(…)` on a
  part of an initializer, in a function, loop or block, on a `var` or a
  `state`, around a setting (`const(tunable(…))` or `config let X =
  const(…)`), and with other than one argument. `tunable(const(e))` is
  allowed. The hoist looks through the call.
- Messages (step 5): **one wording for both forms**, the existing
  "`X` is declared `const` …". It reads correctly for `let X = const(e)`,
  it keeps the parity test's diagnostics identical, and hosts match on it:
  Cheesecake's `script_host.cpp:684` detects a const error by the text
  "is declared `const`", as do its `script_host_tests.cpp` and the bridge
  tests. Only the misplacement errors are new text.
- `core/tests/all/const_parity.rs` rewrites every top-level `const` in
  `examples/`, the `.ptl` files and the Rust string fixtures of
  `core/tests`, and the `petal` snippets of `language-guide.md` and
  `features/static-values.md` (74 samples, 135 declarations), and asserts
  identical diagnostics (severity, line, message), identical IR
  (`ir_equivalent`), identical `show-static --json` (columns left out) and
  identical reload construct kinds for an edit of each initializer. It also
  checks each misplacement error and a run.
- Owner decisions taken for this phase: open question 1, keep `const`;
  open question 2, the declaration stays in `dm.ptl` beside `tunable` and
  `rigid` (moving all three to `annotations.ptl` is still open); open
  question 3, the migration wraps literals (Phase 2's fixer).

### Phase 2: fixers

- `const-decl-deprecated`: `const X = e` → `let X = const(e)`, and
  `pub const` → `pub let … const(…)`. It is report-only until Phase 5.
- `config-let-deprecated`: `config let X = v` → `let X = tunable(v)`. If the
  initializer is already `tunable(…)`, it drops `config` only.
- Both modelled on `lint/export_to_pub.rs`, sharing two pieces that rule
  lacks: the closing `)` for a multi-line initializer (63 `const` and 51
  `config let` sites), and re-padding of column-aligned trailing comments.
  `plan_pub_rewrite` handles only a keyword that shrinks the line; about
  830 `config let` lines carry a trailing comment.
- `const-of-literal` (opt-in, report only).
- Tests: a fixture for each initializer shape (scalar, record, list,
  multi-line, typed `let X: T`, trailing comment, already `tunable`), and
  "the fix of a fixed file is empty".

#### Phase 2: what shipped (9 November 2026)

- `core/src/lint/lifecycle_keywords.rs` holds three rules, registered in
  `lint/mod.rs` and listed in `docs/CLI.md`:
  - `const-decl-deprecated`: `const X = e` → `let X = const(e)` (`pub` and
    `export` kept, a type annotation kept, literals wrapped too);
  - `config-let-deprecated`: `config let X = v` → `let X = tunable(v)`, or
    only `config` dropped when `v` is already a `tunable(…)` call;
  - `const-of-literal`: a `const X = v` or `let X = const(v)` whose `v` is
    literal data (a literal, a negated number, a colour, a list or record of
    them; a call is not, since `const` is what checks it). Report only, with
    no planned fix (open question 3).
- **Opt-in and report only.** All three run only under `--rules-include`,
  and `petal lint --fix` applies nothing: the deprecation rules plan their
  splices, and `APPLY_LIFECYCLE_FIXES` in `lint/mod.rs` (false) drops them
  before the pipeline sees them. Opt-in, not merely report-only, so that a
  plain `petal lint` over the corpus and Cheesecake does not print ~1,800
  findings before the replacements are in place. Phase 5 flips the constant
  and clears `opt_in` on the two deprecation rules (`const-of-literal`
  stays opt-in).
- **For Phase 3:** `petal::lint::migrate_lifecycle_keywords(source)` applies
  both rewrites exactly as the rules plan them. There is no CLI flag for it
  yet; a sweep can call it from a test or a small tool, or Phase 3 can flip
  `APPLY_LIFECYCLE_FIXES` early and use `petal lint --fix
  --rules-include=const-decl-deprecated,config-let-deprecated`.
- **The initializer's edges** are taken from the text, not from the value's
  span: from the first character after `=` to the end of the statement,
  because the span leaves out grouping parens (`const S = (A) ++ B` would
  otherwise become `(const(A) ++ B)`). The closing `)` goes at the
  statement's end, so a multi-line table gets it after its last bracket and
  before any trailing comment.
- **Trailing comments** keep their column. When a rewritten line's code
  would reach its comment, the whole aligned group (adjacent lines whose
  comments share a column, as `petal fmt` groups them) moves right together
  to one space past the longest code, untouched lines included. A shrinking
  line (`config let X = tunable(…)` loses seven columns) is padded back to
  its group's column. This differs from `plan_pub_rewrite`, which lets a
  wholly rewritten group move left; keeping the column is the one rule for
  both growth and shrinkage. Padding for a line goes with the fix that
  edits that line, and an untouched line's padding with the group's first
  rewritten site.
- The two rules are planned one after the other, each over the text the
  other left, so their offsets never mix when Phase 5 applies them.
- Only top-level statements are rewritten. A nested `const` is already a
  compile error. A `config let` inside a function body (legal, and a DM
  preference there) is not reported; none exists in the corpus or in
  Cheesecake.
- Tests:
  - `lint::lifecycle_keywords::tests` (13 fixtures): scalar, record, list,
    multi-line (both rules), typed `let X: T`, grouping parens at either
    edge, a trailing comment alone and in aligned groups (growing, fitting,
    shrinking), already `tunable`, `const` in a string or a `const(…)` call
    left alone, a rewrite that compiles to the same static values, and
    `const-of-literal`. Every fixture asserts that the fix of the fixed file
    is empty.
  - `core/tests/all/lifecycle_migration.rs` is a dry run of Phase 3. It
    rewrites every `.ptl` file under `examples/`, `garden/`, `core/tests`
    and `cli/tests` that declares either keyword (29 files, 37 `const`, 147
    `config let`), and checks that the result is a fixed point, keeps the
    line count, leaves no keyword declaration, stays `petal fmt`-clean (28
    files were), loads with the same diagnostics where the original loads
    on its own (26 files), and gives identical `static_bindings`.

### Phase 3: the Petal-owned corpus and docs

- Run both fixers over `examples/`, `garden/`, `core/tests`, `cli/tests`,
  the Rust string fixtures (9 test files hold `const` declarations; the
  `config let` fixtures are led by `hot_reload.rs` and `source_diff`), and
  the tree-sitter corpus. Petal has 43 `const` declarations in 7 `.ptl`
  files and 155 `config let` in 25.
- Docs:
  - `language-guide.md` "`const`" becomes "`const(…)`: a value that must be
    constant", and its anchors are cited from `writing-petal-guide.md` and
    others;
  - `features/static-values.md` §`const`;
  - `syntax/config-let.md` is rewritten as `syntax/settings.md`, with
    `let`/`tunable`/`const(…)`/`state` in its comparison table. Keep a stub
    at the old path for one release;
  - a new section in `language-guide.md`, "Annotations", states the
    keyword/annotation rule from the Goal;
  - `Builtins.md` (an entry for `const`; the `tunable` entry's "is being
    removed" note), `CLI.md` (`show-static` kinds, selectors),
    `syntax/overview.md` (keyword list), `usage/config-files.md`,
    `features/hot-reload.md`, `features/direct-manipulation.md`,
    `usage/embedding-c.md`, and the other docs that use either keyword
    (`rg -l "config let|^\s*(pub )?const " docs`);
  - regenerate `docs/stdlib.json`.
- `lint/state_const.rs` and `lint/dm_hints.rs`
  (`dm-hint-state-init`) suggest the new forms.
- petal-lang.org (`~/biz/petal-lang.org`):
  - the four DM samples (`orbits`, `flower`, `quilt`, `fractal-tree`) and
    `dm-samples.json`;
  - in `DmStudio.tsx:178-224`, `findKnobs` drops its `config let` regex
    branch;
  - the home page snippet (`page.tsx:281`);
  - re-sync the docs (`scripts/import-docs.ts`), then `pnpm build:sdk`, then
    `check-dm-samples.mjs`, plus a guard that fails on `config let` or a
    `const` declaration in a sample;
  - fractal-tree's "Alt-click `spread` twice" becomes "once" (Garden's
    cycle is free → tunable → rigid).
- Re-run the DM v2 study and confirm the golden is unchanged.

#### Phase 3: what shipped (9 November 2026)

- **The corpus.** `migrate_in_place` (an ignored test in
  `core/tests/all/lifecycle_migration.rs`; `PETAL_MIGRATE=<paths> cargo test
  --test all lifecycle_migration::migrate_in_place -- --ignored`) applied
  `migrate_lifecycle_keywords` to 31 files under `examples/`,
  `garden/examples`, `test/` and `core/tests/dm`: 43 `const` and 147
  `config let` (the doc's 155 counted declarations inside strings and
  prose). Comments that named `config let` now name the setting. No
  `.ptl` file in the repo declares either keyword, and
  `the_petal_corpus_declares_neither_keyword` keeps it so.
- **The tree-sitter corpus** tests `let X = const(…)` and
  `let X = tunable(…)` (both parse as calls already), and a "Deprecated
  const and config let declarations" test keeps the old forms, which the
  grammar accepts until Phase 6.
- **Rust string fixtures.** Where a fixture uses either keyword only in
  passing, it is rewritten: `hot_reload.rs`, the `dm_*` tests, `lsp_tests`,
  `static_reload`, `static_observe`, `static_review3`,
  `static_in_functions`, `record_templates`, `stdlib_helpers`, the cli's
  `cli_propose_edit_v2` and `static_cache`, petal-ui-headless's
  `propose_edit_bin` and garden-script's `dm_drag`. Fixtures that test the
  deprecated forms themselves stay until Phase 6: `const_decl.rs`,
  `source_preservation.rs` (its fixtures are `export config let` text
  round-trips), the unit tests in `parse.rs`, `source_diff.rs`,
  `static_value.rs`, `direct_manipulation.rs` and the fuzzers, the C bridge's
  tests (Cheesecake reaches `config let` through them until Phase 4),
  Garden's app tests (`garden-app` is not rebuilt for this), the
  desktop-SDL timeline test (it needs SDL to build), and
  `a_tunable_config_is_set_by_name_without_a_recompile` in `hot_reload.rs`.
- **One difference found.** `static_observe.rs`'s
  `a_rebuild_put_off_keeps_its_log_with_its_values` and `static_reload.rs`'s
  `DRAG` fixture (the two debounce tests) stay on `config let K = 2`. With
  `let K = tunable(2)`, a zero-threshold debounce defers `K`'s
  own slot, because `tunable(2)` is a native call where `config let`'s `2`
  is a literal patched in place. With a real threshold the one-call slot is
  never slow enough to defer, so this is a cost, not a wrong value; it is
  the "Run-time cost of `tunable` as a call" risk, and lowering `tunable`
  like `const` removes it.
- **The parity tests run both ways.** `const_parity.rs` also rewrites each
  top-level `let X = const(e)` back into `const X = e` (80 samples, 145
  declarations), and `setting_parity.rs` each single-line
  `let X = tunable(…)` back into `config let` (21 files, 135 settings), so
  both still compare the two forms after the migration.
  `lifecycle_migration.rs`'s dry run now reads its directories from
  `PETAL_MIGRATE_DRY_RUN` (Phase 4 points it at Cheesecake).
- **Lint and help.** `state-as-constant` rewrites `state X = e` to
  `let X = const(e)` (its `const(` and `)` splices sit at the initializer's
  text edges, as Phase 2's fixers do; it does not re-pad trailing comments,
  which `petal fmt` does). `dm-hint-state-init` already suggested
  `let START = tunable(…)`. `petal help lint` lists the three lifecycle
  rules, and the help for `show-static`, `propose-edit` and `apply-change`
  names settings first.
- **Docs.** `syntax/config-let.md` is rewritten as `syntax/settings.md`,
  with `let`/`tunable`/`const(…)`/`state` in its comparison table and a
  section on the deprecated form; the old path is a stub. The language
  guide's "`const(…)`: a value that must be constant" keeps its anchor
  (`#const--a-value-that-must-be-constant`), so the links from
  `writing-petal-guide.md`, `Builtins.md`, `CLI.md` and `overview.md` still
  land, and a new "Annotations" section states the keyword/annotation rule.
  Also updated: `features/static-values.md`, `Builtins.md`, `CLI.md`,
  `syntax/overview.md` (the keyword list says `const` is reserved for the
  annotation), `usage/config-files.md`, `features/hot-reload.md`,
  `features/direct-manipulation.md`, `usage/embedding-c.md`,
  `features/time-travel.md`, `features/program-modification.md`,
  `module-system.md`, `writing-petal-guide.md` and `README.md`.
  `docs/proposals/` and `docs/dev/` are records and keep the old forms.
  `docs/stdlib.json` regenerates unchanged. `CHANGELOG.md` has a "Changed"
  entry.
- **The DM v2 study.** `dm_study.rs` counts a bare top-level `tunable(v)`
  as a config knob (it is the migrated `config let`), and its `hint` aid
  counts only a hint that constrains (`rigid`, or a `tunable` with a
  bound). A full study run before and after the migration is identical in
  every outcome, run count, edit and metric; only l01b's title changed,
  and the golden (`direct-manipulation-v2-study.json`) has that one string
  edited by hand. The golden is not regenerated, since a fresh run also
  moves wall times and drops the external pass.
- **petal-lang.org** (`~/biz/petal-lang.org`, two commits): the five DM
  samples that used `config let` (orbits, flower, quilt, fractal-tree,
  kite) and `dm-samples.json` (fractal-tree's `spread` now turns rigid
  with one Alt-click), `DmStudio.tsx`'s `findKnobs` without its
  `config let` branch, the `/manipulate` copy, a guard in
  `check-dm-samples.mjs` that fails on `config let` or a `const`
  declaration in a sample, and the docs re-imported with
  `PETAL_REPO=<this worktree> pnpm import:docs`. `pnpm build:sdk` was not
  run: its paths are fixed to `~/petal`, and the vendored garden-web
  (built from main at ee6a1a03) already reads `tunable` settings;
  `check-dm-samples.mjs --verbose` prints the same before and after.
  Re-vendor after this branch merges. The home page snippet
  (`page.tsx:281`) moved to `content/home.md` and has neither keyword.

### Phase 4: Cheesecake (`~/cheesecake`)

Cheesecake has 273 `const` declarations in 66 files and 1,421 `config let`
in 187. It is the only large consumer and the only real hazard. Order:

1. Re-vendor Petal at Phase 3. Both forms still parse.
2. Engine, for settings:
   - `engine/script/src/config_overrides.cpp:65-104` is a text scanner for
     `config let NAME =`. Replace it with `pb_source_set_setting`, which
     keeps bounds and options and opens `--set mod.NAME=…` on imported
     modules (the Moses debrief asked for that);
   - the messages in `script_host.cpp:438, 466`, the help in
     `app/src/main.cpp:107, 125` and `options.hpp:88`;
   - `ScriptHost::set_config` → `set_tunable` (`script_host.cpp:905, 916`),
     with the caller `editor/src/tuning_panel.cpp:160`;
   - the Tuning screen's strings (`tuning_panel.cpp:1-14, 202, 205, 347`),
     `effects_panel.cpp:347`, `ptl_source.hpp`, `editor.hpp`,
     `editor_impl.hpp`, `natives_scene.cpp:423`, and the fixtures in
     `editor/tests/ide_tests.cpp:41-43` and
     `embed/tests/bundle/games/probe/game.ptl`;
   - `ptl_source` reads `"tunable"` instead of `"config"`/`"range"`, and the
     Tuning screen offers a dropdown for `options`.
3. Generators that write `const` as text: the trailkeeper art tool
   (`games/trailkeeper-art-tool/serialize.ptl:89`, `doc.ptl:223-226`,
   `tests/tool_test.ptl:187`) and the files it has written under
   `games/trailkeeper/art`. `games/tides/ocean.ptl:136-152` writes GLSL
   `const`, which is not Petal and must not change. The fixer works on the
   parse tree, so it leaves strings alone; a grep-based check would not.
4. Run both fixers over `games/`, `engine/script/packages/cheesecake`,
   `tests/data` and the docs (`docs/petal-api.md`, game READMEs,
   `AGENTS.md`). Then `tools/build.ts test`, which includes every
   `petal_check_*`. Spot-run the `--set` uses and every `profiles.json`.
5. Commit per game, so that one bad rewrite does not hide in a 250-file
   diff.

worlds-fair and petal-creative have no `config let`. Sweep them for `const`
in Phase 6.

#### Phase 4: what landed (9 November 2026, in Cheesecake)

Done in `~/cheesecake` on its `main`, from `dd5681e` to `e413eac`:

- `dd5681e` re-vendored Petal at 7a436afe (Phases 1 to 3).
- `6c0eeff`: `config_overrides.cpp`'s text scanner is replaced by
  `petal::Source` (`--set NAME=…` sets a setting by name, changing only its
  value; `--set mod.NAME=…` reaches one module); `ScriptHost::set_config`
  is `set_tunable`; `ptl::Binding` reads the bridge's `"tunable"` object,
  and the Tuning screen offers a dropdown for `options`. Messages, `--help`
  and the fixtures in `ide_tests.cpp` and `script_host_tests.cpp` name
  settings.
- `234464e`: the trailkeeper art tool writes `let X = const(…)`, and its
  art is migrated.
- One commit per game (`refactor(<game>): settings are tunable(…),
  constants const(…)`), then `tests/` and the docs (`e413eac`).
- After it, no `.ptl` file in Cheesecake declares either keyword (the two
  grep hits are GLSL `const` inside strings, in `games/tides/weather.ptl`
  and `engine/script/packages/cheesecake/postfx.ptl`).

### Phase 5: deprecate (after Phase 4 has landed)

- `const-decl-deprecated` and `config-let-deprecated` become warnings, and
  `--fix` applies them.
- A `CHANGELOG.md` entry, then a re-vendor of Cheesecake, which by now has
  no hits.

#### Phase 5: what shipped (9 November 2026)

- **Compile-check warnings.** `lint::deprecated_lifecycle_decls` (called
  from the compiler beside the `state(key)` warning) warns about every
  top-level `const X = e` and `config let X = v`, worded as the lint rules
  word them and tagged `[const-decl-deprecated]` /
  `[config-let-deprecated]`; the caret sits under `const X =`, not under
  the initializer. Like `state-key-deprecated`, a `// petal-lint-ignore` for
  the rule (the line before, trailing, or `-file`) silences both the
  warning and the lint finding, and a bundled first-party package is not
  warned about. A nested `config let` is not warned about, as the rule
  does not report one (none exists anywhere).
- **The lint rules run by default and `--fix` applies them** (moved to
  `petal migrate-old-code` the same day; see "Phase 5, revised"). Both
  deprecation rules lost `opt_in` and are `Fixes::All`;
  `APPLY_LIFECYCLE_FIXES` is gone. `const-of-literal` stays opt-in and
  report only. `petal help lint` and `docs/CLI.md` say so.
- **Tests:** `const_decl.rs` tests the warning, its caret and the three
  ignore forms, and its other warning tests leave the deprecation warning
  out; its render test is written with `let A = const(…)`. `const_parity.rs`
  compares diagnostics less the deprecation warning (the one intended
  difference). `lifecycle_keywords`' unit tests check a plain `petal lint`
  reports and rewrites both keywords and that the compiler warns (and is
  silenced). `pub_keyword.rs`'s every-form fixture writes
  `pub let margin = tunable(4)`.
- **The Petal-owned corpus has no hits**:
  `the_petal_corpus_declares_neither_keyword` passes, and a grep over every
  `.ptl` in the repo finds no declaration. The fixtures that still test the
  deprecated forms (Phase 3's list) now see the warning, and pass.
- **Docs:** `CHANGELOG.md` has a "Deprecated" entry (the "Added" entry for
  the opt-in rules now names only `const-of-literal`); the language guide,
  `features/static-values.md`, `Builtins.md`, `syntax/settings.md` and the
  `config-let.md` stub say the compile check warns and `petal lint --fix`
  rewrites.
- **Cheesecake** re-vendored Petal at 07b0ad48 (`7284276` there; it also
  takes nullable types, enum methods and module-scoped identity, and its
  `host_call.cpp` writes `Value::tag_display()` so tools still read
  `"Kind.park"`). Its `.ptl` files declare neither keyword. `tools/build.ts
  test`: 598 of 601 passed on the first run; the three failures were Phase 4
  leftovers anchored on pre-migration text (marble's and neon's live-edit
  tests, `cheesecake_headless_config_set`'s message) plus the
  `constant_natives` fixture, whose logged line is now the deprecation
  warning. Fixed in `6c8e6e2` and `7284276`, and all four pass. C++ string
  fixtures that still write `config let` (`mesh_tests.cpp`,
  `asset_tests.cpp`, `ide_tests.cpp`, three in `script_host_tests.cpp`) pass
  with the warning; Phase 6 must rewrite them.
- Owner decision recorded: the warnings cover top-level declarations only,
  matching the fixer.

#### Phase 5, revised: the rewrite moved to `petal migrate-old-code` (9 November 2026)

Per docs/dev/migrations.md, rewriting old syntax is no longer lint's.
The two fixers are now the migrations `const-decl-deprecated` and
`config-let-deprecated` of `petal migrate-old-code`
(`core/src/migrate/lifecycle_keywords.rs`; `migrate_lifecycle_keywords` is
`petal::migrate::migrate_lifecycle_keywords`), and `petal lint --fix` no
longer applies them; `--rules-include=const-decl-deprecated` says where the
rule went. The compile warning names the command:
"`const N = e` is deprecated; write `let N = const(e)`; run
`petal migrate-old-code` [const-decl-deprecated]". The ids are unchanged, so
`// petal-lint-ignore const-decl-deprecated` still silences the warning, and
now keeps the migration off the site. `const-of-literal` stays a lint rule
(`core/src/lint/const_of_literal.rs`). `core/tests/all/lifecycle_migration.rs`
runs the dry run through `petal::migrate::migrate_source`; its
`migrate_in_place` test is gone (`petal migrate-old-code --only
const-decl-deprecated,config-let-deprecated <dir>` does that). Cheesecake
scripts/docs that ran `petal lint --fix` for these: see the Cheesecake bullet
in todo.md, "Smaller".

### Phase 6: remove (one release after Phase 5)

- Parse errors that point at the tool: "`const X = e` was removed; write
  `let X = const(e)` (`petal migrate-old-code` rewrites it)" and
  "`config let` was removed; write `let X = tunable(v)` (`petal
  migrate-old-code` rewrites it)". The rewrite lives only in
  `petal migrate-old-code` (the `const-decl-deprecated` and
  `config-let-deprecated` migrations), so an old file is one command from
  working; nothing else (lint, fmt, the LSP) learns the old forms. The
  migration then needs a parse that still recognises them: keep that
  recognition inside `core/src/migrate/` (a pre-parse that finds
  `const X =` / `config let X =` heads, or a parser mode only the migrator
  turns on), not in the main grammar. `config` stays an ordinary
  identifier. `const` becomes an identifier the resolver refuses to bind (a
  reserved hint name).
- Delete:
  - `Token::Const` and the `const` entry in the keyword list;
  - `LetKeyword::Const`;
  - `StmtKind::Let.is_const` and `is_config`; readers use
    `declares_const`/`is_setting` on the initializer only;
  - the `config` parse paths and the `config_modifier_error` family
    (`parse.rs:690-720`);
  - `ConstructKind::ConfigLet` and `DeclKind::ConfigLet`;
  - the CST two-token rules;
  - `StaticBinding.is_config` (renamed `is_setting`, or folded into
    `tunable: Option<TunableMeta>`).
- Bridge: stop emitting `"config"` and `"range"`, and remove
  `pb_vm_set_config`, `Vm::set_config` and the `config`/`config_strict`
  selector aliases. Rename `Env::set_config_value` to `set_setting_value`.
- Tree-sitter: `const` and `config` leave the keyword lists, and `const` is
  highlighted as a builtin like `tunable`. Update vim and the LSP keyword
  and symbol kinds.
- Sweep the ecosystem projects listed in `CLAUDE.local.md`.

### Phase 7 (optional): `const(…)` as an expression assertion

Once the declaration form is gone, `const(e)` can mean "this expression is
static" anywhere:

- a sub-expression of a top-level value: `let T = {base: const(build()),
  now: time()}`;
- inside a function body, where it asserts that `e` is one of the
  function-body constants the static cache already keeps
  (`static_cache/in_fn.rs`). That gives authors a way to say "build this
  table once" where today they can only hope.

The classifier already gives every term a class (`self.cls(t)`), so the check
is per term, not per declaration. The work is in the error chain (reasons
for a term, not a declaration) and in `--static-audit` coverage of
function-body slots. That coverage is item 4 in
`static-value-optimization.md`, so do this phase after that item.

## Risks

- **Readability of wrapped data.** `let X = const([` … `])` and
  `tunable({` … `})` add nesting and a trailing `)`. That is about 114
  multi-line sites across both repos. Accepted by the old proposal for
  `tunable`, and the same trade applies here.
- **A wrapper placed around part of an initializer**, such as
  `let X = const(a) + b` or `let X = tunable(4) * 2`. For `tunable`, the
  `tunable-not-whole-initializer` lint covers it. For `const`, it is a
  Phase 1 error (misplaced) until Phase 7 makes it meaningful.
- **Hot reload during migration.** Each rewritten declaration is a
  structural change, so a running game recompiles each migrated file once,
  with state kept. Migrate with games stopped.
- **Run-time cost of `tunable` as a call.** It is one native call per
  setting per evaluation of the top level. Measure with `petal bench` on
  `examples/games/boids` after Phase 3, and lower it like `const` if the
  difference shows.

## Open questions

1. **Owner: `const(…)` or `static(…)`?** `static` matches the docs' term
   (static value, `show-static`, `--static-audit`). `const` matches every
   error message and what readers expect. The plan says `const`.
   **Decided 9 November 2026: `const`** (Phase 1 shipped with it).
2. **Owner: one home for the annotations?** Move `tunable`, `rigid` and
   `const` from `core/packages/core/src/dm.ptl` into an `annotations.ptl`,
   and give `Builtins.md` an "Annotations" section. `const` has nothing to do
   with DM, so it would read oddly in `dm.ptl`. **For Phase 1, `const` is
   declared in `dm.ptl` beside the other two**; the move is still open.
3. **Owner: should the migration drop `const` on literal data** (`pub const
   ART = "../art"` → `pub let ART = "../art"`) instead of wrapping it? That
   gives a smaller diff and more readable code, but it drops a check that
   only matters if someone later edits the value into something dynamic.
   The plan says wrap, and leaves `const-of-literal` for authors to apply by
   hand. **Decided 9 November 2026: wrap.**
4. **Owner, carried from the old proposal:**
   - should nested tunables (`POST.bloom` with its own range) be listed to
     hosts? This would be an additive `fields` array;
   - should the 10 strict `// "a" | "b"` choice comments in Cheesecake
     become `options:`? The plan says opt in per game;
   - how long between Phases 5 and 6? Every consumer is yours, so one
     release is probably enough.
5. **Other declaration modifiers.** `pub`, `state`, `memo` and `native`
   change behaviour or linkage, so they stay keywords under the rule in the
   Goal. Is there another modifier that only feeds tools? `config` and
   `const` are the two this plan found. Not yet checked one by one:
   `lexer.rs:247`'s other contextual words (`as`, `class`, `key`,
   `native`, `test`) and the hard keywords.
