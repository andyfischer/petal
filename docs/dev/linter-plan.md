# Linter plan (`petal lint`)

Status: **first slice shipped** (2026-07-07, `rust/src/lint.rs`): `petal lint`
with report / `--fix` / `--check` / `-e` modes, the token-driven 2-space
re-indenter (plus trailing-whitespace trim and single trailing newline), the
rebind rule, and the IR-equivalence gate — proven by a corpus property test
(`lint_preserves_ir_over_repo_corpus`: every repo `.ptl` that compiles must
lint to IR-equivalent, idempotent output). Remaining: the rest of the
normalization catalogue below, and running the linter over `apps/*` and the
garden editor scripts.

Two findings from implementation that correct assumptions below:

- `f(@x)` does **not** desugar to exactly `x = f(x)` — the desugarer hoists
  the assignment and leaves a residual read of `x` at the call site, which
  compiles to one extra unnamed `Copy` term (a pure identity). The gate
  therefore compares IR in *canonical form*: unnamed `Copy` passthroughs
  resolved away and terms renumbered in block-traversal order.
- Rebind candidates must mirror `desugar.rs`'s recursion exactly: the v1
  desugarer does not lift `@` out of match arms or `while` conditions, so the
  linter never proposes rebinds there (the rewrite would leave an `@` that
  compiles to a deferred error).

## Goal

A first-party `petal lint [--fix] [--check] <file>` command that normalizes
Petal source. Two kinds of normalization:

1. **Formatting** — re-indent to 2-space indents, plus other whitespace/style
   normalization (see catalogue below).
2. **Semantics-preserving simplifications** — rewrite verbose patterns into
   idiomatic ones. The flagship rule is the rebind operator:

   ```
   x = f(x)      -->   f(@x)
   nums = append(nums, 4)   -->   append(@nums, 4)
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

### Pass 2 — rebind simplification (`x = f(x)` → `f(@x)`)

AST-analysis to *detect* candidates, span-splice to *apply* them (reuse
`rewrite.rs::splice`). Detection rule for a statement `Assign { target:
Name(x), value }`:
- `value` is a `Call` whose argument list contains exactly one argument that is
  `Ident(x)`, and
- `x` does not appear anywhere else in `value` (no `x = g(x, x)` ambiguity), and
- the call sits in a position the desugarer accepts (statement level — which by
  construction it does, since it's the whole RHS of an assignment).

Rewrite: replace the `Ident(x)` arg with `@x` and drop the `x = ` prefix, i.e.
splice the statement span with `f(@x)` reprinted... **but** reprinting the call
would lose comments inside it. Prefer a minimal edit: delete the `x = ` prefix
span and insert `@` before the matching argument's span. Two small splices, no
reprint.

### Safeguard: prove semantics are unchanged

The `@` operator desugars to *exactly* `x = f(x)` (`desugar.rs`), so the
rewrite is semantics-preserving by construction. Verify it mechanically anyway,
as a belt-and-suspenders gate that runs inside `lint --fix`:

1. Compile the **pre-lint** source to IR.
2. Compile the **post-lint** source to IR.
3. Assert the two IR term-graphs are structurally identical modulo term ids and
   source spans (a canonical-form comparison — reuse/extend whatever the
   bytecode differential oracle uses in `backend/bytecode/tests.rs`).

If they differ, `lint` refuses to write and reports the offending rule. This
gate should cover *every* semantic rule we add, not just rebind. Add a
fuzz/property test: for a corpus of programs, `lint --fix` then diff IR — must
always match.

## Catalogue of normalization ideas (from the syntax survey)

Formatting (Pass 1 / whitespace-only, always safe):
- 2-space indentation.
- Trim trailing whitespace; ensure single trailing newline.
- Collapse 3+ blank lines to at most one (or two) blank lines.
- One space around binary operators; no space inside `(` `[` `{`.
- Space after commas; no space before.

Semantic / idiom rules (each needs the IR-equivalence gate):
- `x = f(x)` → `f(@x)` (rebind). Flagship.
- Optional-comma normalization: pick one house style for list/arg separators
  (see `docs/syntax/optional-commas.md`) — either always-comma or the
  juxtaposition style, consistently.
- `if c then true else false end` → `c`; `if c then x else x end` → `x`.
- Redundant `return` of the last expression in a fn body → implicit return.
- `#f80` vs `#ff8800` color literal casing/length — normalize to one form.
- Collapse `x = x + 1` → `x += 1` (and friends) — verify against compound-assign
  desugaring.

Start with: 2-space indent + rebind, behind `--fix`, with `--check` (exit
non-zero if not normalized, print nothing on success — CI mode).

## CLI shape

```
petal lint <file>            # report; exit 1 if changes needed
petal lint --fix <file>      # rewrite in place
petal lint --check <file>    # CI mode: exit 0/1, no output on success
petal lint -e <code>         # lint inline code, print result to stdout
```

Wire into `cli.rs`: add a `Command::Lint { fix, check }` variant, a
`parse_lint_args`, a dispatch arm, and an entry in `print_usage`. Also update
`docs/CLI.md`. Per `CLAUDE.local.md`, run the linter over `apps/*` and the
garden editor scripts once it's stable.
