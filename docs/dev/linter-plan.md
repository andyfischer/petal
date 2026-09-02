# Linter (`petal lint`)

Status: **shipped.** `rust/src/lint/` holds the re-indenter (`reindent.rs`),
the identity-cast rule (`casts.rs`) and the if-chain-to-`match` rule
(`to_match.rs`). Every `.ptl` in the repo has been run through `lint --fix`.
What remains is the catalogue of further rules in §5, none of which is
scheduled. The command reference is in [CLI.md](../CLI.md#lint--normalize-source).

## 1. Goal

A first-party command that normalizes Petal source in two ways:

1. **Formatting** — 2-space indentation, trailing-whitespace trim, a single
   trailing newline.
2. **Semantics-preserving simplifications** — rewrite verbose patterns into
   idiomatic ones, such as dropping `int(n)` when `n` is already an `int`.

## 2. CLI

```
petal lint <file>            # report; exit 1 if changes are needed
petal lint --fix <file>      # rewrite in place
petal lint --check <file>    # CI mode: exit 0/1, no output on success
petal lint -e <code>         # lint inline code, print the result to stdout
petal lint-fix <file>        # alias for `lint --fix <file>`; path only
petal lint --fix --verify[=ir|strict] <file>
                             # prove the rewrite before writing (see §4)
```

Every mode goes through `lint_source`, which fails on a parse error before
anything is written, so an unparseable file is never modified.

## 3. Architecture

The linter needs to re-emit source without losing the author's comments and
layout. The lossless CST (`rust/src/cst.rs`) provides that: every token
including whitespace and comment trivia is a leaf, `SyntaxNode::text()`
reproduces the source byte-for-byte, and the typed AST is projected from the
tree. `rust/src/rewrite.rs` provides span-based splicing.

The two kinds of normalization use different mechanisms. Neither reprints
the AST, which would lose comments inside a rewritten expression.

**Formatting is token-driven.** The re-indenter computes nesting depth from
block-opening and block-closing tokens and delimiters, then rewrites only
the *leading whitespace* of each line. Everything else on the line is copied
verbatim. Petal is newline-significant but not indentation-significant, so
this pass cannot change semantics.

**Semantic rules detect over the AST and apply span splices.** The splices
only ever cover the glue the rule removes; every operand, arm body and
comment the author wrote is preserved. The re-indenter runs afterwards, so a
rule never worries about indentation.

## 4. Safeguards

The gate has to match the rule. A rewrite that only moves tokens can be held
to full IR equality; one that deletes a call cannot, and demanding it would
mean the rule can never ship.

- **Whitespace-only rules** need no gate. The corpus test asserts the IR is
  byte-identical for any file the semantic rules did not touch.
- **Structure-changing rules** (both shipped semantic rules) rest on the
  detection rule being an identity, backed by a compile gate: if the original
  compiles, the rewrite must too, or `lint` refuses to produce output.
- **`--verify`** compiles both sides and compares IR with `ir-equal`.
  `--verify=ir` (the default) proves the formatting pass and accepts the
  semantic passes as expected-to-differ; `--verify=strict` demands IR
  equality of the whole rewrite. A rewrite that cannot be proven exits 3
  without writing.

A corpus property test (`lint_preserves_compilation_over_repo_corpus`)
checks every `.ptl` in the repo: it still compiles after linting, an
untouched file has byte-identical IR, and linting is idempotent.

## 5. The shipped rules

### The identity-cast rule

`int(n)` where `n` is already an `int` is the identity, and so are `float()`
on a float and `str()` on a string. The rule deletes them.

- **Detection** is the type checker's (`typecheck::find_redundant_casts`),
  so it inherits that pass's conservatism: anything it cannot prove infers
  `any` and is left alone. Almost no real Petal source carries annotations,
  so the builtin result-type table (`typecheck/builtin_types.rs`) is what
  makes the rule find anything. It lists only certainties (`len` is an
  `int`; `round`/`floor`/`abs`/`clamp` preserve int-ness) and deliberately
  omits builtins whose result type is a runtime question (`reverse`, `slice`,
  `get`). Any addition to that table is a correctness change for this rule.
- **The rewrite** is two span splices per cast, so comments and layout
  inside the argument survive.
- **Parenthesization** depends on the cast's slot:

| Slot | Example | Result |
| --- | --- | --- |
| Whole right-hand side, `return` value, statement, lone argument | `let m = int(a + 1)` | `let m = a + 1` |
| Operand of a larger expression | `2 * int(a + 1)` | `2 * (a + 1)` |
| Element of a comma-separated list | `f(int(a + 1), b)` | `f(a + 1, b)` |

The list-element case needs no parentheses because commas are required
between elements ([syntax/commas.md](../syntax/commas.md)); a neighbour can
never bind across the boundary once the call's own parentheses are gone.

Verified end to end by running every runnable `.ptl` in the repo before and
after: byte-identical output, except two files whose error-message column
moved with the re-indentation.

### The if-chain-to-`match` rule

An `if`/`elsif` chain that tests one subject against literals is a `match`
written the long way:

```
if ch == "@" then "spawn"          match ch
elsif ch == "o" then "coin"    -->    when "@" -> "spawn"
elsif ch == "w" then "walker"         when "o" -> "coin"
else nil                              when "w" -> "walker"
end                                   when _ -> nil
                                    end
```

The splices only cover the glue — `if`, `==`, `then`, `elsif <subject> ==`,
`else`. The chain's trailing `end` needs no edit. A chain with no `else`
gains `when _ -> nil`, which is load-bearing: an `if` that falls off the end
yields nil, but a `match` with no arm left is a runtime error.

What it refuses, and why:

| Refused | Why |
| --- | --- |
| Numeric literals | `==` compares an int against a float numerically (`1 == 1.0`), while pattern matching requires the tags to agree, so `when 1` does not match `1.0`. String, bool and nil literals have no cross-type rule and are exactly equivalent. |
| A subject that could compute | `match` reads the subject once where the chain read it per arm. A name or a field path is safe to move; a call or an index is not. |
| A comment in the glue | It would have no home in the `match`, so the whole chain is skipped rather than dropping it. |
| Multi-statement arm bodies | A `->` arm takes an expression; where a chain is really control flow rather than a lookup, `match` is not obviously better. |
| Fewer than three arms | At two arms `if … else … end` is the plainer spelling. |

Beyond the compile gate, a structural check (`verify_chain_counts`) requires
that converting *n* chains adds exactly *n* `Match` nodes and removes at
least *n* `If` nodes, or lint refuses to write. Verified end to end over the
repo (29 chains across 12 files) plus `~/worlds-fair/ui/ptl`.

Still to revisit: if `==` on a `Pending` becomes strict, the safe-literal set
needs re-deriving. Numeric chains could be admitted where the type checker
proves the subject is an `int`.

### The rebind rule (removed)

The first slice's flagship rule rewrote `x = f(x)` to `f(@x)`. It was removed
in August 2026: the `@` sugar has to be learned before the code reads, and a
linter that *forces* it makes every file harder for a newcomer than the
plain assignment. Two findings from it still describe the desugarer:
statement-level `f(@x)` desugars to exactly `x = f(x)`, and the desugarer
does not lift `@` out of match arms or `while` conditions.

## 6. Catalogue of further rules (not scheduled)

Formatting, always safe:

- Collapse 3+ blank lines to at most one or two.
- One space around binary operators; none inside `(` `[` `{`.
- Space after commas, none before.

Semantic rules, each needing a gate. Ordered by how often they occurred in a
2,900-line survey app (`~/worlds-fair/ui/ptl`):

- **`str(a) ++ "/" ++ str(b)` → `"{a}/{b}"`.** 56 `++` operators and zero
  interpolations in the survey. Rewrite only when an operand is a string
  literal, or `"{a}{b}"` turns a type error into a silent coercion.
- **Hoist a repeated pure subexpression to a `let`.** 27 sites. Needs a
  purity whitelist — `hovered()`, `key_pressed()`, `time()` must never be
  hoisted.
- **`if c then x else x end` → `x`**, including the `elsif` form.
- **`for i in range(0, len(xs))` where `i` is only ever `xs[i]` → `for x in
  xs`.** Preconditions: the index is used for nothing else and `xs` is not
  reassigned in the body.
- **Unused local binding.** The analysis exists (`typecheck/unused.rs`);
  this is wiring it to a fix.
- **`if v == nil then D else v end` → `v ?? D`.** 33 nil tests against zero
  uses of `??` in the survey.
- `if c then true else false end` → `c`; redundant `return` of a body's last
  expression; one canonical color-literal form; `x = x + 1` → `x += 1`.

Not a rule, despite appearances: a trailing bare `nil` at the end of a `fn`
body is load-bearing — the last expression is the implicit result.
