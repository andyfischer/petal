# Linter plan (`petal lint`)

Status: **shipped** (`rust/src/lint/` — mod.rs + reindent.rs + casts.rs):
`petal lint` with report / `--fix` / `--check` / `-e` modes, `petal lint-fix
<file>` as the in-place alias, the token-driven 2-space re-indenter (plus
trailing-whitespace trim and single trailing newline), and the identity-cast
rule. A corpus property test
(`lint_preserves_compilation_over_repo_corpus`) asserts that every repo `.ptl`
that compiles still compiles after linting, that a file with no cast fixes has
byte-identical IR, and that linting is idempotent. Remaining: the rest of the
normalization catalogue below, and running the linter over `integrations/*` /
`sample-apps/*` and the garden editor scripts.

### The rebind rule was removed (2026-08-02)

The flagship rule of the first slice rewrote `x = f(x)` to `f(@x)`. It is gone.
The `@` operator remains a language feature, but the sugar has to be learned
before the code reads, and a linter that *forces* it makes every file harder
for a newcomer than the plain assignment it replaced. Its IR-equivalence gate
went with it: that gate demanded the pre- and post-lint IR be byte-identical,
which is exactly the wrong shape for a rule that *deletes* a call.

Two findings from the rebind implementation are kept here because they still
describe the desugarer:

- Statement-level `f(@x)` desugars to exactly `x = f(x)` — the desugarer drops
  the residual read of `x` at the call site when the lifted call was the whole
  statement.
- The v1 desugarer does not lift `@` out of match arms or `while` conditions.

### The identity-cast rule (2026-08-02)

`int(n)` where `n` is already an `int` is the identity
(`rust/src/builtins/math.rs`), and so are `float()` on a float and `str()` on a
string. The rule deletes them. Three parts:

1. **Detection** is the type checker's
   (`typecheck::find_redundant_casts`), so it inherits that pass's
   conservatism: anything it cannot prove infers `any` and is left alone.
2. **A builtin result-type table** (`typecheck/builtin_types.rs`) is what makes
   the rule find anything at all, since almost no real Petal source carries
   annotations. It lists only certainties: `len` is an `int`, `clamp` is always
   a `float` even for int arguments, `round`/`floor`/`abs` *preserve* int-ness
   rather than producing one, and `reverse`/`slice`/`get` — whose result type
   is a runtime question — are deliberately absent. The table also feeds
   `check_module`, which is now meaningfully more informed.
3. **The rewrite** (`lint/casts.rs`) is two span splices per cast, so comments
   and layout inside the argument survive, and it re-checks each span against
   the source text before accepting it.

Parenthesization is decided by the cast's slot (`typecheck::CastSlot`):

| Slot | Example | Result |
| --- | --- | --- |
| `Delimited` — whole RHS, `return` value, statement, lone call argument | `let m = int(a + 1)` | `let m = a + 1` |
| `Operand` — operand of a larger expression | `2 * int(a + 1)` | `2 * (a + 1)` |
| `ListElement` — element of a comma-*optional* list | `f(int(a + 1), b)` | `f(a + 1, b)` |
| `ListElement`, no separator in the source | `f(int(a + 1) b)` | *skipped* |

That last row is the one that matters, and the compile gate is what found it:
Petal's commas are optional and juxtaposition is itself a separator, so
`f((a + 1) (b + 1))` parses as a *call*, not two arguments. Parentheses cannot
rescue that slot, so the fix is skipped there rather than made unsafe.

**Safeguard.** The rule removes a call, so there is no IR to hold equal.
Correctness rests on the detection rule, with a compile gate behind it: if the
original source compiles, the rewritten source must too, or `lint` refuses to
produce output. Verified end to end by running every runnable `.ptl` in the
repo before and after `lint-fix` and diffing stdout — 104 files, byte-identical
except for two whose *error message* column moved with the re-indentation.

## Goal

A first-party `petal lint [--fix] [--check] <file>` command that normalizes
Petal source. Two kinds of normalization:

1. **Formatting** — re-indent to 2-space indents, plus other whitespace/style
   normalization (see catalogue below).
2. **Semantics-preserving simplifications** — rewrite verbose patterns into
   idiomatic ones. The shipped rule is the identity cast:

   ```
   int(w / 2)    -->   w / 2        # int / int is already an int
   float(x)      -->   x            # when x is already a float
   ```

## Prerequisite (met): source preservation

The linter needs a representation that can re-emit source without deleting the
author's comments and layout. That now exists: the lossless CST
(`rust/src/cst.rs`) is the authoritative parse artifact — every token including
whitespace/comment trivia is a leaf, `SyntaxNode::text()` reproduces the source
byte-for-byte, and the typed AST is projected from the tree
(`rust/src/cst_project.rs`). `rust/src/rewrite.rs` provides trivia-preserving
tree splices plus span-based string splicing as a fallback — the right
primitives for `--fix`.

## Recommended architecture

Split the two normalization kinds by mechanism — do **not** try to do both from
one AST reprint:

### Pass 1 — re-indentation (token/CST driven, not AST-reprint)

Compute nesting depth from block-opening / block-closing tokens and delimiters,
then rewrite only the *leading whitespace* of each line. Everything else on the
line (including trailing comments) is copied verbatim. Depth increases after
`fn` / `if…then` / `else` / `for…do` / `while…do` / `match` / unclosed `(` `[`
`{`, and decreases at `end` / `)` `]` `}` / `else` / `elsif` / match arms.

Because it only touches leading whitespace, this pass is trivially
comment-safe and cannot change semantics (Petal is newline-significant but
**not** indentation-significant — confirmed empirically).

### Pass 2 — semantic rules (AST-detect, span-splice)

AST analysis to *detect* candidates, span splices to *apply* them — never a
reprint, which would lose comments inside the rewritten expression. The shipped
instance is the identity cast; see the section above for its detection rule,
parenthesization table and gate.

### Safeguard: prove semantics are unchanged

The gate has to match the rule. A rewrite that only *moves* tokens can be held
to full IR equality; one that deletes a call cannot, and demanding it would
just mean the rule can never ship. So:

- **Whitespace-only rules** — no gate needed, and the corpus test asserts
  IR is byte-identical for any file the semantic rules didn't touch.
- **Token-moving rules** — compile before and after, assert the serialized IR
  is identical modulo the embedded source text and source map.
- **Structure-changing rules** (identity casts, and most of the catalogue
  below) — correctness comes from the detection rule being an identity, backed
  by a compile gate: if the original compiles, the rewrite must too, or `lint`
  refuses to produce output.

In every case `lint` refuses to write rather than emit something it cannot
stand behind, and the corpus property test
(`lint_preserves_compilation_over_repo_corpus`) runs all of the above over
every `.ptl` in the repo, plus an idempotence check.

## Catalogue of normalization ideas (from the syntax survey)

Formatting (Pass 1 / whitespace-only, always safe):
- 2-space indentation.
- Trim trailing whitespace; ensure single trailing newline.
- Collapse 3+ blank lines to at most one (or two) blank lines.
- One space around binary operators; no space inside `(` `[` `{`.
- Space after commas; no space before.

Semantic / idiom rules (each needs a gate — see the safeguard above):
- Optional-comma normalization: pick one house style for list/arg separators
  (see `docs/syntax/optional-commas.md`) — either always-comma or the
  juxtaposition style, consistently.
- `if c then true else false end` → `c`; `if c then x else x end` → `x`.
- Redundant `return` of the last expression in a fn body → implicit return.
- `#f80` vs `#ff8800` color literal casing/length — normalize to one form.
- Collapse `x = x + 1` → `x += 1` (and friends) — verify against compound-assign
  desugaring.

Candidates surveyed against a real 2,900-line app (`~/worlds-fair/ui/ptl`,
2026-08-02), ordered by occurrences there over implementation cost:

- **`str(a) ++ "/" ++ str(b)` → `"{a}/{b}"`.** 56 `++` operators in that corpus
  and *zero* string interpolations; 9 lines wrap an operand in an explicit
  `str()` that interpolation subsumes. Needs a guard: rewrite only when an
  operand is a string literal, or `"{a}{b}"` turns a type error into a silent
  coercion.
- **Hoist a repeated pure subexpression to a `let`.** 27 sites repeat one
  expression inside a single `fn`, 12 of them twice on the *same line*
  (`rect(i * (w / len(sw)), 0, w / len(sw), 6)`). Needs a purity whitelist —
  `hovered()`, `key_pressed()`, `time()`, `frame_count()` must never be
  hoisted.
- **`if c then x else x end` → `x`**, including the `elsif` form
  (`if active then WF.ink elsif moved then WF.ink else WF.ink_mut end`).
  Already in the list above; the survey confirms it occurs in the wild.
- **`for i in range(0, len(xs))` where `i` is only ever `xs[i]` → `for x in xs`.**
  4 of 12 range-loops qualify. Preconditions: the index is used for nothing
  else, and `xs` is not reassigned in the body.
- **Unused local binding.** `let w = screen_width()` / `let h = screen_height()`
  dead in one screen. The unused-binding analysis already exists
  (`typecheck/unused.rs`); this is wiring it to a fix.
- **`if v == nil then D else v end` → `v ?? D`.** 33 `== nil`/`!= nil` tests in
  that corpus against zero uses of `??`.

Not a rule, despite appearances: a trailing bare `nil` at the end of a `fn`
body (16 occurrences there) is load-bearing — Petal's last expression is the
implicit result, so deleting it changes what the function returns.

Shipped: 2-space indent + identity casts, behind `--fix` / `lint-fix`, with
`--check` (exit non-zero if not normalized, print nothing on success — CI
mode).

## CLI shape

```
petal lint <file>            # report; exit 1 if changes needed
petal lint --fix <file>      # rewrite in place
petal lint --check <file>    # CI mode: exit 0/1, no output on success
petal lint -e <code>         # lint inline code, print result to stdout
petal lint-fix <file>        # alias for `lint --fix <file>`; path only
```

Wired into the CLI (done): `Command::Lint { fix, check }` in `rust/src/cli/mod.rs`,
`parse_lint_args` / `parse_lint_fix_args` in `cli/args.rs`, `handle_lint` in
`cli/handlers.rs`, and entries in `print_usage`. `lint-fix` is `lint --fix`
with a path argument only; like every mode it goes through `lint_source`, which
fails on a parse error before anything is written, so an unparseable file is
never modified. Still to do per `CLAUDE.local.md`: run the linter over
`integrations/*` / `sample-apps/*` and the garden editor scripts once it's stable.
