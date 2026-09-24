# Formatter and linter (`petal fmt`, `petal lint`)

Status: **shipped.** `rust/src/fmt/` is the formatter; `rust/src/lint/` holds
the rules. The command reference is in [CLI.md](../CLI.md#fmt--canonical-layout).
The catalogue of further rules in §7 is not scheduled.

## 1. The split

Until September 2026 `petal lint` did both jobs: it re-indented a file and
applied semantic rewrites in one pass. It is now two commands, drawn along the
line Go and Deno draw:

| | `gofmt` / `deno fmt` → **`petal fmt`** | `go vet` / `deno lint` → **`petal lint`** |
|---|---|---|
| Question | How is this code laid out? | Is there a better way to write this code? |
| Changes | whitespace only | tokens |
| Default action | rewrite in place | report, exit 1 |
| Knobs | none (gofmt); fmt has no style options | rules selectable (`--rules-include/-exclude`) |
| Output | the file | `file:line:col: rule: message` per finding |
| Safety | provably meaning-preserving | per-rule gates (compile, structural checks, `--verify`) |
| Opt out | `// petal-fmt-ignore`, `-off`/`-on`, `-ignore-file` | `// petal-lint-ignore <rule>`, `-ignore-file` |

The test for which side a rule belongs on: **if applying it can change the
token stream, it is lint.** Everything fmt does is re-lexed and checked
against the original tokens, so fmt can be run blind, on save, over a whole
tree; lint rules each carry their own argument for why the fix is an identity.

Diagnostics that describe a *bug* rather than a spelling — a discarded pure
call (`typecheck/unused.rs`), a type mismatch — stay in `petal check`, the way
Go keeps unused variables in the compiler. Judgement calls ("hoist this into a
`let`") stay in `petal suggest` (§6).

### Where each rule landed

**fmt** (all whitespace):

- 2-space indentation per open construct (the former lint pass 1).
- One space around binary/assignment operators, `->`, `|>`; after `,` and `:`.
- No space inside `()` `[]` `{}` or an interpolation hole; before `,` `:` `;`;
  around `.` `?.` `..`; after unary `-` `!`, `@`, `...`; between a callee or
  indexed value and its `(` / `[`.
- Other blank runs collapse to one space.
- At most one blank line in a row; none at the start or end; one final newline;
  no trailing whitespace.
- Trailing comments keep their column (≥ 1 space from the code); a group of
  consecutive comments at one column moves together.

**lint** (token-changing, each with a fix): `prefer-let`, `no-redundant-cast`,
`prefer-match`, `prefer-compound-assign` (§5).

## 2. CLI

```
petal fmt [--check] [--diff] [<path>...]      # gofmt -l/-d, deno fmt --check
petal fmt -e <code> | petal fmt -
petal lint [--fix] [--json] [--rules-include=…] [--rules-exclude=…] [<path>...]
petal lint --rules
petal lint --fix --verify[=ir|strict] <path>  # prove the rewrite (see §4)
petal lint-fix [<path>...]                    # alias for lint --fix
```

Both take files or directories (walked for `.ptl`, skipping dot-directories,
`node_modules` and `target`), default to `.`, and keep going past a file that
fails. `lint --fix` formats each file it changes, since a rewritten chain
moves code between lines.

## 3. Architecture

Neither command reprints from the AST, which would lose comments inside a
rewritten expression. The lexer's spans tile the source (`crate::trivia`), so
both work as edits over the original text.

**fmt is token-driven** and runs four passes, to a fixed point:

1. `fmt/spacing.rs` — rewrites each gap between two tokens on one line that
   holds only blanks. JSX tags/children and fenced lines are left alone.
2. `fmt/reindent.rs` — computes each line's column from the open-construct
   stack and rewrites only the leading whitespace.
3. `fmt/comments.rs` — trailing-comment columns.
4. `fmt/comments.rs` — blank-line runs.

**fmt keeps alignment.** This is the one deliberate departure from gofmt and
deno fmt, both of which flatten hand-aligned tables. Petal code is full of
them (level layouts, vertex lists, record tables, `state` blocks with aligned
`=` and comments), and a first run over `~/cheesecake` with the flattening
rules rewrote 750 lines, 726 of them just re-indenting continuation lines the
author had lined up under an open bracket. So:

- A blank run of 2+ is kept when the token after it starts where a token on
  the nearest code line above or below starts, or ends where one ends (a sign
  counts as part of its number). Comment-only lines in between are skipped.
- A double space after a comma is kept when that width repeats on the line
  (argument groups: `quad(x0, y0, z1,  x1, y0, z1,  …)`).
- A construct that *hangs* — a bracket with contents on its own line, or an
  `if` that starts mid-line — lets its continuation lines keep their column,
  shifted by however far the opener's line moved, as long as they stay deeper
  than that line. So does a line opening with a binary operator, relative to
  the line it continues.

Because alignment is judged against neighbours as they currently stand, the
passes repeat until nothing changes (a pass only ever keeps or drops an
alignment run, so this terminates), which keeps fmt idempotent.

**lint rules detect over the AST and apply span splices.** Each rule returns
one fix per site (an anchor and its splices); the pipeline filters them by
rule selection and ignore comments, applies them, and re-parses for the next
rule. Findings are reported at their position in the original text by mapping
each later stage's offsets back through the earlier stages' splices.

## 4. Safeguards

- **fmt**: re-lexes its output and refuses it unless the token stream is the
  one it started from (runs of newlines compared as one). The corpus tests
  (`fmt_is_ir_equal_and_idempotent_over_repo_corpus`,
  `fmt_undoes_mangled_indentation_over_repo_corpus`) hold every repo `.ptl` to
  identical IR, idempotence, and undoing an injected indentation mangle.
- **lint**: if the original compiles, the fixed text must too, or nothing is
  written; `prefer-match` and `prefer-let` also count the nodes they meant to
  change. `lint_preserves_compilation_over_repo_corpus` checks that every repo
  file still compiles after `--fix` and that `--fix` is a fixed point (a second
  `lint` finds nothing).
- **`--verify`** compiles both sides and compares IR with `ir-equal`.
  `--verify=ir` (the default) proves formatting and the IR-invisible rule
  (`prefer-compound-assign`) and accepts the others as expected-to-differ;
  `--verify=strict` demands IR equality of the whole rewrite. A rewrite that
  cannot be proven exits 3 without writing.

## 5. The shipped rules

### `no-redundant-cast` — the identity-cast rule

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

### `prefer-match` — the if-chain-to-`match` rule

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

## 6. What belongs here, and what belongs on the suggestion channel

`petal suggest` ([suggestions-plan.md](suggestions-plan.md)) shipped after
this document, and it takes the rules that are *judgement calls* rather than
normalizations. The test is who decides: a lint rule claims there is one right
spelling and applies itself with `--fix`; a suggestion claims there might be a
better one and waits to be asked. "Hoist a repeated pure subexpression to a
`let`" is naming, not spelling, and belongs there. Identity casts and
indentation belong here.

## 7. Catalogue of further rules (not scheduled)

The formatting items once listed here (blank-line runs, operator and comma
spacing) shipped in `petal fmt`. Not planned for fmt: line wrapping, which needs
the AST reprinter this design avoids.

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
