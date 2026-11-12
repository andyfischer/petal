# Indentation-delimited blocks (Scala 3 style), `#` comments, `:hex` colours and `else if`

Status: implemented through P5 in this repo, revised 2026-11-12. What remains is listed under "Remaining" below.

Implemented (2026-11-12):
- P1: `:hex` colours and `#` comments, with the transitional `#hex` error
  (§3.3).
- P2: `else if` as one token; `elsif` accepted and reported by `petal lint`
  (§3.6).
- P3: the layout parser, fmt, lint fixes, LSP folding and source_diff (§3.7,
  §5.3).
- P4: `end-lines`, Markdown mode, and this repo's `.ptl` migrated (§7.4).
- P5 docs: the guide, the syntax pages and the writing guide describe blocks
  closed by indentation, and the fences of the user-facing docs have no
  multi-line `end` (`docs/proposals`, `docs/tasks`, `docs/plan` and `docs/dev`
  are left as written). The rules B1–B17 and the error messages are in
  [docs/syntax/indentation.md](../syntax/indentation.md).
- P6, in part: hint and suggestion text shows blocks with no `end`, and the
  bytecode fuzzers also run on programs with no `end`.

Remaining:
- P5 tooling: tree-sitter, vim, the website highlighter and Garden's
  indentation are tracked by their own work.
- Migrating the other projects (website, worlds-fair, petal-creative,
  cheesecake) and re-vendoring Petal where it is vendored (§7.3).
- Markdown outside `docs/` (README files, `examples/`), and re-running the
  website's `scripts/import-docs.ts`.
- P6: `end-inline` for one-liners (waits on open question 1); dropping the
  transitional `#hex` error (waits on the vendored projects, open
  question 2); removing `elsif`.
- Open questions 1 to 4 (§11). No `redundant-end` lint exists and fmt removes
  no `end` until question 1 is decided.

Decided 2026-11-11:
- Comments are `#`. `--` was considered and rejected.
- Colour literals move from `#f80` to `:f80`, which frees `#` completely (§3.3).
- A one-line named fn's body runs to the end of the line (`fn box(w) "{w}"`),
  with no `=` (B8).
- `else if` replaces `elsif`. The lexer merges `else if` into one token (§3.6).
- Still open: what happens to a multi-line `end` after the migration
  (open question 1).

Revision history:
- The first draft proposed Pythonic blocks: a trailing `:` opened each block,
  and `then`/`do` were removed.
- This revision follows the Scala 3 style instead. `then` and `do` stay,
  indentation closes blocks, and `end` becomes optional.
- The trailing `:` is no longer a requirement.
- The live-coding hazard of half-typed indentation is left to the IDE layer and
  is out of scope here (§6.5).
- Comparison pages: Pythonic vs Scala 3
  (https://claude.ai/artifact/KHdbtYNuF4jXQU7SR2aiCh) and Elm for Petal
  (https://claude.ai/artifact/M7mXo5Ns4MPz6DJxxfa3dY).

## Summary

Blocks keep today's openers (`then`, `do`, or a newline after a `fn` /
`class` / `enum` / `match` header) and end where the indentation ends. `end`
becomes an **optional closer**: still legal, but no longer needed.

Three smaller changes ride along:
- `#` becomes the line-comment marker, and `//` is deprecated.
- Colour literals become `:f80`.
- `else if` replaces `elsif`.

```petal ignore
# proposed
fn clamp_all(xs, lo, hi)
  for x in xs do
    if x < lo then lo
    else if x > hi then hi
    else x
```

It is the same program as today, minus the `end` lines and with `else if`
for `elsif`.

**Recommendation: adopt.** It is a much smaller and safer change than the
Pythonic draft.

- **Correctly indented code keeps parsing.** Existing code is valid under the
  new parser, so there is no per-file mode and no flag day. The migration only
  deletes `end` lines.
  - There are 28,745 `end`-only lines across the five projects (11,205 in
    petal), plus about 350 `end)` / `end,` lines.
  - One-liners (`if c then a else b end`, about 13,400 lines) need no change at
    all. Their trailing `end` can be dropped later, one site at a time, where it
    is safe.
- **Little new grammar.** No new keyword or opener, no `->`/`:` split, and no
  `import ui:` clash. One-liners with and without `end` follow one rule: an
  `end` closes the innermost open block.
- **Prior art.** This is the layout Scala 3 shipped, and Elm, OCaml and F# use
  the same `if … then … else` shape without a closer.
- **One structural cost remains.** Blocks inside brackets (multi-line lambdas in
  calls, `{if …}` holes in JSX) need an offside rule that Python doesn't have
  (§3.2).
- **The `#` clash is settled by moving colours.** About 9,100 literals change
  from `#f80` to `:f80`, mechanically, and `#` is then free for comments.
  - The new `:` prefix needs a lexer rule to tell it from `:` as a separator
    (§3.3).
  - It also takes the spelling a future `:name` symbol literal would want.
- **The tooling still changes.** The tree-sitter grammar needs an external
  scanner, and Garden needs to indent after a block opener and reindent on
  paste (§6.8).

Answers to the original questions under this design:

| Question | Answer |
|---|---|
| Language ambiguities? | A few, all resolvable. See §3. The real ones: how far an `end`-less one-line body extends (`if c then 1 else 2 + 3`), `;` after an `end`-less one-liner, `else`/`else if` on the line after a one-line `if`, `end` alignment, `:f80` vs `:` as a separator, and legacy `else if … end end` nesting. Dangling `else` resolves to the nearest `if`, as in Scala, OCaml and Elm. |
| JSX? | Tags and children text ignore indentation. `{if c then`⏎…⏎`}` holes use the in-bracket offside rule. Today's `{if c then … else nil end}` keeps working as written. |
| Multi-line strings? | Strings are opaque to layout. Holes in strings can't contain line breaks today, so no block can start in one. Petal embedded *in* strings needs a base-indent rule (§3.5). |
| `if c then e` without `end`? | **Yes.** It is the one-line form, and it ends where the expression ends. Writing `end` stays legal, which is how today's one-liners keep working. |
| `do … end` / `then … end` fallbacks? | **These come for free.** `end` stays an optional closer, so an explicitly closed block is just today's syntax, and it covers generated code, multi-statement one-liners (`if c then a; b end`) and `end \|> f` chaining. The trade-off is two spellings, so `petal fmt` picks the canonical one: no `end` on multi-line blocks (§5). |

## 1. What changes and what does not

Changes:

- **Blocks close by indentation.** A block body that starts on the line after
  its opener is closed by the first line indented less than the body.
- **`end` becomes optional.**
  - It closes the innermost open block.
  - On its own line, it must sit at the column of the line that opened the
    block (§3, B5).
  - On the same line, it closes an inline form, as today.
- **`#` comments.** `#` starts a line comment, Python-style. `//` is
  deprecated. `/* … */` stays, mainly for `{/* … */}` in JSX.
- **Colour literals.** `#f80` becomes `:f80`. The same 3, 4, 6 or 8 hex digits
  are allowed, and it desugars to the same record.
- **`else if`.** It replaces `elsif`. The lexer turns `else` + `if` on one line
  into a single token, so `else if` always continues the chain.

Unchanged:

- **Every opener:** `then`, `else if … then` (formerly `elsif`), `else`,
  `for … do`, `while … do`, `test "…" do`, `when p ->`, and the newline after
  `fn f(…)`, `fn f(…) -> type`, `fn(…)`, `class X`, `enum E` and `match x`.
- **Keywords:** `then`, `do` and `end` stay reserved. `elsif` stays reserved
  after its removal, so the error can say "write `else if`".
- **Everything else:** expressions, `state`, `var`/`set`/`get`, types,
  annotations, imports, records, JSX and strings.
- **The AST, IR and bytecode.** The parse of a correctly indented legacy file
  must be identical before and after. That invariant is the core test (§9).

Prior decision: `docs/writing-petal-guide.md:82-83` says "`end` closes every
block … No braces, no significant indentation." This plan reverses it.

## 2. Corpus facts

These counts come from regex heuristics over `.ptl` files, excluding
`node_modules`, `target` and `.temp`; treat them as ±10%. The cheesecake count
includes its vendored copy of Petal.

| Project | files | lines | `end`-only lines | `end)` / `end,` lines | one-line `then…end` / `do…end` | `//` whole-line | `//` trailing | colour literals |
|---|---|---|---|---|---|---|---|---|
| petal | 497 | 101,203 | 11,205 | 49 | 4,134 | 14,179 | 865 | 1,844 |
| cheesecake | 552 | 151,764 | 14,960 | 299 | 9,274 | 20,004 | 2,298 | 5,784 |
| petal-creative | 93 | 16,911 | 1,370 | 4 | 1,424 | 2,680 | 71 | 1,321 |
| petal-lang.org | 142 | 9,222 | 844 | 2 | 370 | 1,083 | 70 | 126 |
| worlds-fair/ui/ptl | 20 | 4,217 | 366 | 0 | 169 | 1,156 | 24 | 27 |

**Indentation is already almost uniformly structural.** `petal fmt --check`
over `examples core-libs core-runtime garden test` reports 31 of 402 files
needing formatting, and some of those differences are spacing rather than
indentation. fmt derives indentation from block structure (`fmt/reindent.rs`,
`INDENT = 2`). So about 92% of the repo's files already have exactly the
indentation the new parser expects, and running `petal fmt` once fixes the
rest.

**Today's layout of multi-line blocks in value position is already the Scala
shape.** `else`/`elsif` (`else if` after this change) align with the statement line and the body is indented
under it:

```petal ignore no-migrate
let bucket = if ci < 0 then          # examples/dashboards/finance-dashboard/app.ptl:350
  zeros(len(CATS))
else
  zeros(len(CATS[ci].merchants))
end
```

The same holds at `examples/dashboards/network-graph/app.ptl:1113` and `:1904`.

Counts for the side changes:

| Project | `elsif` | `else if` on one line (real Petal) | of those, `else if … end end` |
|---|---|---|---|
| petal | 1,419 | 2 | 2 |
| cheesecake | 2,662 | 0 | 0 |
| petal-creative | 307 | 0 | 0 |
| petal-lang.org | 46 | 6 | 6 |
| worlds-fair/ui/ptl | 53 | 0 | 0 |

- **Most `else if` text in cheesecake isn't Petal.** The 52 grep hits there are
  GLSL inside `"""` raw strings (`cheesecake/games/skyrace/shaders.ptl:52`). So
  are its `//` comments and `#define`s. Every migration here must be
  token-based, never a text substitution.
- **No `:` followed by hex digits** appears anywhere in the corpus, with or
  without a space before it. So `:f80` collides with no existing code.
- **`symbol(name)`** is a builtin (docs/Builtins.md:2299), but there is no
  symbol literal.

Other patterns that matter:

- **`end` followed by more expression:** 1,018 lines. Mostly one-line lambdas
  in arguments (`map(base, fn(v) v * k + 0.5 end)`, `core/tests/dm/06_lists.ptl:7`),
  which are untouched because their `end` stays legal. Real chaining
  (`end |> join(" ")`, `examples/games/minesweeper/minesweeper.ptl:161`) occurs
  5 times.
- **Multi-line lambdas closing a call (`end)`):** 49 in petal and 299 in
  cheesecake (`core-runtime/bloom/src/overlay.ptl:71-76`,
  `core-runtime/glsl/src/glsl.ptl:330-333`).
- **One-line `if` chains continued with a leading `elsif`/`else` (`else if`
  after this change):** 85 in
  petal, 136 in cheesecake and 16 in petal-creative
  (`core-runtime/glsl/src/glsl.ptl:492-496`).
- **`else` at the end of a line opening a nested block:**
  `let ink = if m.disabled then t.dim else`⏎`  if m.danger then … end`⏎`end`
  (`core-runtime/bloom/src/menu.ptl:256-258`).
- **JSX:** 51 petal-creative files, and 28 multi-line `{if … then` holes
  (`petal-creative/packages/design-material/inputs.ptl:84-97`).
- **Petal embedded in host-language strings:** 81 of 95 `core/tests` files,
  21 `cli/tests` files, 159 `core/src` unit-test sites, about 68 garden files,
  39 TS vitest files and 20 cheesecake C++ files. Some are indented to the host
  code (`core/tests/all/native_fn.rs:97-102`, `test/vitest/state-call-paths.test.ts:279`).
- **Markdown:** 637 fences in `docs/` and 526 on the website (copied from
  `docs/`). `test/vitest/doc-snippets.test.ts` lints and runs them, with regexes
  that hard-code `print(...) // expected`.

## 3. Ambiguities and how each is resolved

| # | Issue | Verdict | Resolution |
|---|---|---|---|
| B1 | How far does an `end`-less one-line body extend? `let x = if c then 1 else 2 + 3` | Resolved by a rule | It extends as far as the expression grammar reaches, as arrow-lambda bodies already do (`parse.rs:3146`). So this is `else (2 + 3)`. Legacy `if … end + 3` keeps its `end` and its meaning. |
| B2 | Dangling `else`: `if a then if b then 1 else 2` | Not a problem | `else` binds to the nearest open `if`, as in Scala 3, OCaml and Elm. `if v > A then A else if v < B then B else v` (`starfield.ptl:16`) reads as intended. With `end`, it closes the innermost open block. |
| B3 | `;` after an `end`-less one-liner: `if c then a; b` | **Real** | Make it an error: "`;` after a one-line `then` is ambiguous; close it with `end` or use an indented block". `if c then a; b end` (today's form) stays valid. |
| B4 | A line starting with `else if`/`else` after a one-line `if` (the glsl.ptl:492 shape) | Resolved by a rule | A line starting with `else if`/`else` continues the innermost open `if` whose statement column is ≤ its own column. This covers both today's styles: aligned with the statement line (finance-dashboard:350) and hung deeper (glsl.ptl:492). It is the same mechanism as leading-operator continuation (`parse.rs:348`, `:372`). |
| B5 | Where may an `end` sit? | Resolved by a rule | On its own line, `end` closes the innermost open block and must sit at the column of the line that opened it. Anything else is an error, e.g. "`end` at column 2 does not line up with the `for` block opened at line 14 (column 4)". An `end` with no open block is an error. This makes misindented legacy code fail loudly (§5.1). |
| B6 | Blocks inside `()`, `[]`, `{}` and JSX holes | Needs a rule Python doesn't have | The offside rule inside brackets (§3.2) |
| B7 | `end.f()` / `end \|> f` chaining | No change | It keeps working, because `end` stays legal. Without `end`, the block form can't chain: bind a name or parenthesise an inline block. |
| B8 | A one-line named fn with no `end`: `fn box(w) "{w}"` | **Decided** | The body runs to the end of the line, or to an `end` on that line. There is no new token: `fn f(x) -> int x * 2` stays unambiguous, because a return type is a single bare name (docs/syntax/overview.md "Types"). Scala's `=` was rejected. |
| B9 | A one-line lambda without `end`: `fn(v) v * 2` | Design choice | Recommended: the same rule as named fns, but fmt and the guide prefer the existing `fn(v) -> v * 2`. Today's `fn(v) v * 2 end` stays valid. |
| B10 | A match arm body on the following lines | No conflict | `when p ->`⏎ followed by an indented block is today's "statements until the next `when`" with the indentation made binding. `when p do … end` keeps working, and fmt rewrites it to `when p ->`. |
| B11 | Tabs and mixed indentation | Resolved by a rule | Tabs in indentation are an error, and fmt converts them. The corpus has none. |
| B12 | A snippet indented as a whole (string, Markdown, REPL paste) | Resolved by a rule | Base indent: the first statement's column is the left margin (§3.5). |
| B13 | Multi-line headers: `if (a &&`⏎`b) then` (38 sites) | No conflict | Lines continued by a bracket or an operator don't count for layout. The block column is measured against the line where the statement started. |
| B14 | `#` comment vs colour literal (`lexer.rs:828`) | **Resolved by moving colours** | Colours become `:f80`, and `#` is unconditionally a comment once the transition ends. During the transition, `#` + a colour-shaped hex word is an error that points at the migration (§3.3). |
| B15 | `then`/`do`/`else` at the end of a line followed by a comment | No conflict | Comments and blank lines are ignored for layout. The block starts at the next code line. |
| B16 | `:f80` colour vs `:` as a separator (type annotations, record fields, named args, `import m:`, enum data, record patterns) | Resolved by a rule | `:` starts a colour only when it is followed directly by 3, 4, 6 or 8 hex digits and a non-identifier character, **and** the previous token cannot end an expression or name. "Can end" means an identifier, a literal, `)`, `]`, `}`, a string, or `end`/`nil`/`true`/`false`. So `{x:100}`, `f(a:fff)` and `let c:add` keep `:` as a separator, while `{tint: :000}`, `f(:f80)`, `= :fff` and `when :fff ->` are colours. The rule depends on tokens, not whitespace. The corpus has no `:hex` today (§2). As implemented (P1a): the first token of a `{…}` string or JSX hole starts a value whatever precedes the hole (`"{:f80}"`); and a colour directly after a value cannot be respelled (`fn bg() #fff end` → `fn bg() :fff end` would read a separator), so `colour-literals` lists that site for a hand edit, `(:fff)`. No corpus has one. |
| B17 | `else if` merged in the lexer vs legacy `else if … end end` (an `if` nested in an `else`) | **Real**, 8 sites | Merged, the inner `if` joins the chain, so the legacy second `end` becomes a stray `end`: an error, not a silent change. The `else-if-nesting` migration removes the inner `end` when the nested `if` is the `else`'s only statement. Otherwise it breaks the line after `else` so the nesting is kept (§3.6). |

What the Pythonic draft needed and this design doesn't: the per-file style
detection (draft A17), the `import ui:`⏎ clash (A2), the `->` vs `:` split for
arms and lambdas (A10, A11), and parenthesising nested one-liners (A6).

### 3.1 Layout is driven by the parser

The parser decides where a block starts, not a separate INDENT/DEDENT pass in
the lexer. The lexer can't tell a block opener from other syntax: `else` at the
end of a line opens a block, while `{key:`⏎`value}` doesn't.

- **Tokens carry position.** They already have line and column
  (`lexer.rs:353-355`). Add a `first_on_line` flag.
- **The parser measures blocks.** When a block opener (`then`, `else`, `do`,
  `->` of an arm, or the end of a `fn`/`class`/`enum`/`match` header) is
  followed by a line break:
  - The next code line must be indented to some column `c` greater than the
    *statement indent*, meaning the indent of the line where the enclosing
    statement began.
  - Every statement of the block starts on a new line at exactly `c`.
  - The block ends at the first line that starts below `c`, at a closing
    bracket that belongs to an enclosing bracket, or at an `end` (B5).
- **An opener followed by more code on the same line** is a one-line body. An
  `end` on that line closes it; otherwise B1 applies.
- **Newlines inside brackets are still skipped** (`parse.rs:470`, `:2790`,
  `:2807`, `:3406`). Only a block body measures columns.
- **Spans.** Block CST nodes may now end without an `end` token, so spans end at
  the last body token. `span_from` (`cst_project.rs:14-17`) and
  `for_parts`'s `end_of` change accordingly.

This mostly changes `parse_block_until` (`parse.rs:1910`) and
`expect_block_end` (`parse.rs:430`). Because `end` stays legal, the existing
`end` handling becomes one of the three ways a block closes, instead of being
replaced.

### 3.2 Blocks inside brackets: the offside rule

```petal ignore
# proposed (today: the same, with `end)` on the last line)
defer_paint(fn()
  draw_shadow(r, {radius: t.radius_sm, blur: 14, color: :000000})
  draw_rect_rounded(r, t.radius_sm, tone(t, t.overlay, 0.05), 245)
)
```

The block's column is 2. It is greater than the statement indent (0) of the
line holding `fn()`. The `)` at column 0 starts a line below 2, so the block
ends and the `)` closes the call. These cases follow:

- **A JSX hole** measures against the first token of its line:

  ```petal ignore
  # proposed (petal-creative/packages/design-material/inputs.ptl:84, minus `end`)
      {if !outlined then
        <rect id="indicator" x={0} y={m.height - thick} />
      else nil}
  ```

  `else` at column 4 continues the `if` (B4), and `}` closes the hole. A JSX
  element inside a block body is one expression whose tags and text ignore
  layout. The lexer's `JsxTag`/`JsxContent` modes already skip newlines
  (`lexer.rs:546`, `:1214`).
- **Nesting.** Brackets push a "layout suspended" frame, and a nested block
  re-enables layout relative to its own statement line.
- **Two block lambdas in one call** need a leading-comma line, or `end`s. Since
  `end` stays legal, `end, fn()` keeps working.

Precedent: Elm puts `if`, `case` and `let` blocks inside list and argument
brackets, laid out the same way, and so do Scala 3, F# and GDScript 2. Python
avoids the issue by having no multi-line lambdas.

### 3.3 `#` comments and `:hex` colour literals

`#` was the colour sigil (`lexer.rs:828`, `read_color` `:1140`), and `#fade`,
`#add` and `#bad` are valid colours. Rather than share `#` between comments and
colours, colours move to `:`.

**End state:**

- `#` starts a comment that runs to the end of the line, exactly as in Python
  and shell. `#!` on line 1 is a shebang.
- `:` followed by 3, 4, 6 or 8 hex digits and a non-identifier character is a
  colour literal, under the previous-token rule B16. It desugars to the same
  `{r, g, b[, a]}` record as today.
- `"#ff8800"` inside strings (CSS-style host APIs, `to_glsl` output) is
  untouched. It was never a literal.

**Transition**, so a stale `#f80` never silently turns into a comment:

1. **P1a (done 2026-11-12).** The lexer accepts `:f80`. `#f80` still works but warns:
   "colour literals are now written `:f80`; run `petal migrate-old-code`
   [colour-literals]". The migration rewrites each colour *token*, so `"#…"`
   strings and `"""…"""` GLSL are never touched.
2. **P1b (done 2026-11-12).** `#` becomes a comment. While the old form is
   being removed, a `#` followed directly by a colour-shaped hex word is an
   **error**, not a comment: "`#f80` is no longer a colour literal; write `:f80`
   (or `# f80` for a comment)". Comments that start `# ` or `#` + a non-hex word
   are fine. `//` is deprecated, and the `hash-comments` migration rewrites it.
   As implemented:
   - "Colour-shaped" is 3, 4, 6 or 8 hex digits and then a character that
     cannot continue an identifier, so `#fade in` is the error and `#fade_in`
     is a comment.
   - `petal migrate-old-code` reads old source with the lexer's legacy mode
     (`Lexer::legacy`), in which that shape is a colour token and every other
     `#` is a comment. `colour-literals` and `hash-comments` run first, on
     that lexer, so a fully unmigrated project migrates in one run.
   - `hash-comments` turns a run of `n` slashes into `n - 1` hashes
     (`/// doc` → `## doc`) and reports once per file, not once per comment.
   - `#` is a comment wherever code is lexed, including a JSX open tag and a
     multi-line `{…}` hole.
3. **P6.** Once the vendored projects (cheesecake, worlds-fair) have migrated,
   the error is dropped and `#` is unconditionally a comment.

The error in step 2 matters. Without it, an unmigrated `let c = a ?? #fff` would
become `let c = a ??` followed by a comment. The trailing `??` continues the
expression onto the next line (docs/syntax/line-continuation.md), giving a
different program with no error.

**Costs of `:` for colours:**

- **It takes the symbol spelling.** Ruby, Elixir and Clojure readers expect
  `:name` to be a symbol or keyword. Petal has `symbol(name)`, but no literal
  for it. If Petal later wants `:shapes` as a symbol literal, `:add`, `:bad` and
  `:face` would be colours. A symbol literal would then need another spelling,
  such as `'shapes` or `` `shapes ``, or a rule that symbols are non-hex words,
  which is fragile. This is open question 3.
- **Readability in records and arguments:** `{tint: :000000}` and
  `draw(r, color: :f80)` put two colons close together. That's acceptable, and
  fmt keeps the space after the separator.
- **It is a separate, mechanical migration** of about 9,100 sites across the
  five projects (§2), plus the colour docs and every highlighter's colour rule.

Don't reuse `//` for floor division for a long time, because a stale `// note`
would parse as an operator.

### 3.4 Comments inside JSX

Today a children line starting with `//` or `/*` is a comment
(`lexer.rs:1305-1320`). Proposed:

- Keep `{/* … */}`.
- Don't make a children line that starts with `#` a comment, because text like
  "# of items" is plausible.
- Deprecate the line-start `//` in children. The migration rewrites it to
  `{/* … */}` (`literal_edit.rs:780-784` already does this for one comment).
  As implemented: a `//` line between two lines of text is what joins them
  into one text child, and a hole there would make them two, so that one site
  is listed for a hand edit. No corpus file in this repo has one.

### 3.5 Strings, embedded snippets and the base indent

- **Strings are opaque to layout.** Ordinary strings may span lines, and
  `"""…"""` is verbatim (`lexer.rs:1065-1087`). A line that *begins* inside a
  string is not a line start for layout. String holes already reject line breaks
  (`lexer.rs:991`), so no block can start in one.
- **Base indent.** The column of the first statement is the program's left
  margin. That lets these parse unchanged:
  - `Program.parse("""`⏎`    fn step(input)`…
    (docs/language-guide.md:568-573);
  - TS template literals indented to the host;
  - indented Rust raw strings.
- **Embedded snippets mostly keep working.** Most of them are already
  indented consistently with their `end`s, so they keep parsing with no
  migration, the same as `.ptl` files.

### 3.6 `else if` in the lexer

When the lexer sees the keyword `else`, it looks ahead past spaces and tabs only
(no newline, no comment). If the next word is the keyword `if`, it emits one
`ElseIf` token spanning both words. The parser handles `ElseIf` exactly where it
handles `elsif` today (`parse.rs:2875-2908`).

- **`else if` on one line always continues the chain.** That matches Scala,
  Elm, JavaScript and C, and with the layout rule there is no other sensible
  reading.
- **`else` at the end of a line, then `if` on the next line, is not merged.** It
  stays an `else` block that contains an `if`. That's the
  `core-runtime/bloom/src/menu.ptl:256-258` shape, and its meaning is unchanged.
- **Legacy `else if c then x else y end end`** (8 sites, B17) now has a stray
  `end`, and fails loudly. The `else-if-nesting` migration fixes it.
- **`elsif`:**
  - It is first accepted silently (`OldForm::Accepted`, reported by
    `petal lint` `old-syntax`).
  - Then it warns.
  - Finally it is removed, with an error that says to write `else if` and run
    the migration.
  - The `elsif-to-else-if` migration rewrites about 4,490 sites.
- **Source preservation.** The token's text is the source as written, which may
  contain more than one space. fmt normalises it to one space. DM, `edit_path`
  and literal edits never touch keywords, so they are unaffected.
- **Tree-sitter** doesn't need the merged token: `seq('else', 'if')` in the
  grammar, with the same no-newline condition handled by the external scanner
  that layout already needs.

**As implemented (P2, done 2026-11-12):**

- The merged token is `Token::ElseIf`; `Token::Elsif` remains for the old
  spelling, and the parser takes either. The merge is skipped directly after
  `.` or `?.` (a field named `else`).
- The stray-`end` error (B17) carries a hint that names the line of the `end`
  that follows the chain's own and `petal migrate-old-code` [else-if-nesting].
- `else-if-nesting` reads the old form with `Lexer::unmerged_else_if` (also
  part of `Lexer::legacy`) and a parse of those tokens. A file that parses
  today has no site, so the old reading is tried only on a file today's
  parser rejects; a file that mixes the two readings parses under neither
  and is reported. Its IR gate compares the migrated text with the original
  in which every site is moved to the line after `else` (the old nesting,
  spelled for today's compiler); an AST comparison covers files that do not
  compile on their own.
- Both migrations run before `at-rebind`, reading through a removed `@x`.
- `petal lint`'s `prefer-match` reads `else if` chains (it matched the
  `elsif` text before).

Why the lexer rather than the parser: one token keeps every parser site that
handles `elsif` unchanged. It also means `else if` can never mean nesting by
accident, so there's no lookahead special case spread through the parser.

### 3.7 As implemented (P3 parser, done 2026-11-12)

The layout parser is in `core/src/parse.rs` (`parse_body`, `closer_is_ours`,
`end_is_ours`, `close_block`). User-facing rules: `docs/syntax/indentation.md`.
Fixtures: `core/tests/all/layout.rs`, one group per B-row plus a generative
test. Where the implementation settled something the tables above leave
open:

- **The statement indent is the indent of the line the construct's keyword
  is on** (`if`, `for`, `fn`, `when`, …; for an `else` branch's body, the
  line of that `else`). A block's column must exceed it, and an `end` on its
  own line must equal it. That is the layout `petal fmt` already produces,
  inside brackets too (`  if c then` as a list element takes its `end` at the
  element's column, not the statement's).
- **Two more columns line up for an `end`** when a construct hangs off the
  middle of a line: the column of its own keyword, and the column of the line
  its last `else` / `else if` starts (`examples/productivity/settings`, which
  `petal fmt` leaves that way).
- **An `end` left of its own construct is passed outward** only when an
  enclosing construct, inside the same brackets, starts at that column.
  Otherwise the error is reported there, against the innermost block.
- **`when` arms** sit at one column: indented from the `match`, or at its
  column (a `when` starts no statement). A class field or an enum variant is a
  bare name and must be indented.
- **B3** covers every one-line body: `then`, `else`, `do`, a `fn` header, and
  an arm's `->` (for which the whole `match` must take the `end`).
- **B11** is checked where a line's column is measured (the statements of a
  block, an `end` / `else` / `when` on its own line, the line that opens a
  block). A tab in front of a line that only continues a bracket or an
  expression is not an error. The lexer records the lines
  (`Lexer::token_tab_indented`) and the parser is told
  (`Parser::with_tab_indentation`).
- **B12:** a line left of the margin is an error too ("indented less than the
  first line of the program"). A host string whose first line starts at the
  quote and whose other lines are indented to the host code is therefore an
  error; the tests that had one were fixed by hand.
- **`{…}` holes in element children and attributes** are laid out like any
  other bracket. The lexer drops line breaks there, so the parser also treats
  "the next token is the first on its line" as a line break. A token after
  element text that began on an earlier line counts as first on its line,
  and is measured where the text on its line starts (`some text {if c then`
  is a line indented to `some`).
- **A line that starts with a `/* … */` comment** is measured at the comment
  (B15), wherever its first token is: `Lexer::token_layout_column`, passed
  with `Parser::with_layout_columns`. A comment that began on an earlier
  line counts when it starts that line. A tab in front of the comment is a
  tab in the indentation (B11).
- **A string's `{…}` hole** ends a one-line body with no `end`
  (`"{if c then 1 else 2}"`).
- **A leading `-` under the line it was meant to continue** used to be a
  separate statement with a warning; indented, it is now "unexpected indent",
  with the same advice.
- **The old reading stays available as `Parser::by_end`**
  (`cst::parse_source_by_end`, `rewrite::parse_ast_or_by_end`): every block
  runs to its `end`, columns mean nothing, tabs are whitespace. Nothing
  compiles it. `petal fmt`, `petal lint` and `petal migrate-old-code` fall
  back to it for a file whose indentation disagrees with its `end`s, so fmt
  can still indent such a file by its `end`s; fmt then checks that the result
  is the same AST under the layout parser. (fmt lays out blocks with no `end`
  as of §5.3.)
- **Corpus invariance** is `test/benchmarks/harness/layout_corpus.rs`
  (`cargo run --release --example layout_corpus`), against a baseline written
  by the parser from before the change. Result on 2026-11-12, each file read
  both as today's syntax and with `Lexer::legacy`: no file parses to a
  different AST. Files that fail with an indentation error: petal 1
  (`garden/examples/panels/fonts.ptl`, fixed with `petal fmt`), worlds-fair 20
  of 20 (the rest of the file left flush under an `else`), cheesecake 6
  source files (an `end` hung between two columns, and one test fixture
  indented on purpose), petal-creative 0, petal-lang.org 0.

### 3.8 Independent verification (2026-11-12)

A second pass probed every B-row and the bullets of §3.1, §3.2, §3.5 and
§5.1 through the CLI, without reading the fixtures first. The fixtures it
left are in `core/tests/all/layout_probes.rs`.

- **Bugs found and fixed:** a line starting with a `/* … */` comment was
  measured at its first token (B15); a hole after element text on its line
  was measured at the hole; fmt put the body of a `when … ->` arm in a
  `{…}` hole at the arm's column, and the body of a block opened on the last
  line of a multi-line string a step in from that line (both refused by the
  gate, so the file could not be formatted).
- **Real files:** 36 files of `examples/` and `core-runtime/` with their
  `end` lines stripped by a throwaway script parse to the AST they had, and
  fmt is idempotent and AST-preserving on both spellings.
- **Misindentation:** 4,300 random misindentations of those files that
  their `end`s still read are each an error or the same program (§5.1).
- **Corpus invariance** re-run over the five projects, tabs now refused as
  the compiler refuses them: no file parses to a different AST; the files
  that fail with an indentation error are the ones listed in §3.7.
- **A hazard the rules allow:** a line that starts with a binary operator
  continues the statement above it wherever it is indented (B13), so under a
  block with no `end` it continues the block's last statement:
  `let s = for x in xs do` / `  str(x)` / `|> join(" ")` joins each element.
  B7 says the block form cannot chain without `end`; nothing reports the
  attempt. `end-lines` keeps such an `end`.

## 4. Grammar, by construct

### 4.1 if / else if / else

```petal ignore
# block
if x < 0 then
  return -x
else if x == 0 then
  return 0
else
  x

# one line
if x < 0 then return -x
let label = if x > 5 then "big" else "small"

# a one-line chain continued on the following lines (B4)
let ty = if t == "float" then Ty.float
    else if t == "vec2" then Ty.vec2
    else nil

# colours
let tint = :f80
draw_rect(r, color: :ff8800aa)

# value position: else aligns with the statement line, as fmt lays it out today
let bucket = if ci < 0 then
  zeros(len(CATS))
else
  zeros(len(CATS[ci].merchants))
```

### 4.2 for / while

```petal ignore
for item in items do
  print(item)
let squares = for i in range(1, 6) do i * i
while x < 10 do x += 1
```

### 4.3 fn, lambdas, class, enum, test

```petal ignore
pub fn abs(x: int) -> int
  if x < 0 then return -x
  x

fn box(w, h = w) "{w}x{h}"            # body runs to end of line (B8)

let double = fn(x) -> x * 2           # unchanged
let describe = fn(x)                  # block lambda: today's form, minus `end`
  print(x)
  x * 2

class Point
  x: int,
  y: int,

enum Shape
  circle(radius, sides: 0),
  rect(w, h, sides: 4),

test "addition" do
  expect_eq(1 + 1, 2)
```

### 4.4 match

```petal ignore
match dir
  when "left" -> slide_left(g)
  when "right" ->
    let res = slide_left(mirror(g))
    {grid: mirror(res.grid), gained: res.gained}
  when d -> {grid: g, gained: 0, bad: d}
```

`->` takes either a one-line body or an indented block. This is Elm's
`Pattern ->` and Scala's `case p =>`.

## 5. Transition: `end` becomes optional

There is no per-file mode and no legacy parser. The new parser accepts:

- today's correctly indented code with its `end`s;
- the same code with its `end`s removed;
- any mix of the two, block by block.

`fn f(x)`⏎ means the same thing in both, because the opener set doesn't change.

### 5.1 Misindented legacy code fails loudly

Today indentation is cosmetic, so some files have indentation that disagrees
with their `end`s. That was 31 of 402 fmt-unclean files, some of them spacing
only. Under the new parser, such a file is an **error, not a silently different
program**, in every case I could construct:

- **A block body that isn't indented** is an error: "expected an indented block
  after `then`".
- **A dedented line inside a block** closes the block early. Its `end` then
  lands at a column that doesn't match the innermost open block, so B5 reports
  an error.

  ```petal ignore no-migrate
  fn f()
    if c then
      a()
    b()        # legacy: inside the if; layout: closes it
    end        # column 2 ≠ the fn's column 0 → error
  end
  ```
- **Over-indentation** is "unexpected indent".

That holds while files keep their `end`s. Once `end`s are removed, nothing
cross-checks the indentation, which is why the migration runs `petal fmt` first
(§7) and the gate compares parses.

P0 confirms this claim against the corpus: no correctly-`end`ed file may parse
to a different AST.

### 5.2 Canonical style

Two spellings now exist, so `petal fmt` picks the canonical one:

- **Multi-line blocks: no `end`.** fmt removes an `end` that is alone on its
  line.
- **One-liners: `end` only where it's needed.** That means a one-liner followed
  by an operator, postfix, `;` statement or `,`-separated sibling where B1 or B3
  would change the meaning. Otherwise fmt drops it. This is phased: P4 removes
  the multi-line `end`s, and the one-liner cleanup is a separate, later step.
- **`end` before a closing bracket** (`end)`) becomes `)`, unless two block
  lambdas share a call.

Whether a stray multi-line `end` is a lint (`redundant-end`) or simply formatted
away is open question 1. Scala 3 keeps optional `end` markers for long blocks.
Petal could do the same, but fmt would then have to leave them alone.

### 5.3 As implemented (P3 fmt, lint, LSP, done 2026-11-12)

- **fmt keeps the block structure of the parse.** `fmt::Blocks`
  (`core/src/fmt/blocks.rs`) lists the constructs no `end` closes, each with
  its last token, read off the green tree. The indentation pass
  (`fmt/reindent.rs`) is still token-driven; it drops such a construct from
  its stack once that token has gone by, where it used to wait for an `end`.
  Tokens are named by their index among the tokens that are not line breaks,
  which whitespace edits do not move.
- **The gate is AST equality** (`rewrite::ast_shape`) on top of the token
  comparison, for every file fmt changes.
- **A block opened on a line that begins inside a string** (`emit("…` /
  `…", fn(x)`) is laid out a step in from the line the string began on,
  which is the line the parser measures it from. The body of a `when … ->`
  arm in a `{…}` hole is found by the line the `->` ends, since the lexer
  drops the line breaks there.
- **Three readings.** The language's own; by `end`s, for a legacy file whose
  indentation disagrees with them; and, for a file rejected only for its tabs
  (B11), with a tab as one column, accepted when the parse is the same with a
  tab as eight columns. A file with no reading is reported with the parser's
  error.
- **A comment after a block with no `end`** goes to the deepest level still
  open that it was written at least as deep as.
- **fmt removes no `end`** (open question 1). The transform is
  `fmt::remove_block_ends` (`core/src/fmt/ends.rs`), for `end-lines` to call
  after `petal fmt`:
  - it removes an `end` alone on its line, and the `end` of `end)` / `end]` /
    `end}`;
  - a comment after a removed `end` stays, on its own line at the block's
    column;
  - it keeps an inline `end`, `end,`, an `end` followed by anything else, the
    `end` of an empty block, and one whose next line starts with an operator;
  - the gate is AST equality, and an `end` whose removal changes the parse is
    put back, one at a time (`if c then a; b` / `end`, B3);
  - it reports the lines of the `end`s it kept.

  `end,` is kept: the plan's step 3 would leave a line that starts with a
  comma. Over this repo's formatted `.ptl` it removes more than 5,000 `end`s,
  and every result is a fixed point of fmt and the same AST
  (`fmt_lays_out_blocks_without_ends_over_repo_corpus`).
- **Lint fixes.** `prefer-match` writes a `match` closed the way the chain
  was. A body over several lines is not moved, so the arms are placed where
  those lines still read as the body before fmt runs: by `end`s when the chain
  has one and the result reads that way, otherwise one column in from the
  chain with each body left on its own line. A chain neither placement reads
  for is left alone. `prefer-repeat` takes a loop with no `end`. The corpus
  sweep `lint_reads_blocks_without_ends_over_repo_corpus` holds the `end`-less
  spelling of every repo file to the findings of the `end`-ful one.
- **Other emitters.** None emits a multi-line block. `fix-imports` and the
  goal edits start an inserted top-level statement at the file's left margin
  (B12). Hint text still shows one-liners with `end`, which stay valid (P6).
- **LSP.** `textDocument/foldingRange` is new and reads the blocks off the
  parse (`lsp/folding.rs`). The outline needed no change: a declaration's
  range is its statement's span. The tolerant parse behind completion gives a
  block whose only line was blanked a `nil` to stand for its body. The server
  has no selection-range or on-type-formatting support, before or after.
- **source_diff.** A removed `end` maps to the end of the token before it, so
  the `end-lines` migration reloads as `SourceChange::None` (Relocated), with
  every span where a recompile puts it. An added `end` is `Full`: the tokens
  do not say where the old end of the block went. When some of a run of
  consecutive `end`s are removed, the kept ones are found by column; if two
  share a column the answer is `Full`.

## 6. Assessment

### 6.1 Project goals

- **Ease of use.** About 28,700 lines (~10%) of ecosystem code disappear, and
  `end end end end` pyramids go with them (`examples/dashboards/network-graph/app.ptl:1330-1351`).
  The `then`/`do` keywords that make Petal read like Lua and Ruby stay, so the
  code still looks familiar to Petal users.
- **Distinctiveness.** This is generic surface. Nothing Petal-specific depends
  on it.
- **Introspection.** Neutral: the AST is identical.
- **Program manipulation.** Slightly negative, but explicit `end` remains
  available to generators (§6.6).

### 6.2 Ease of writing and reading

- **New things to learn:** indentation is binding, including inside brackets.
  The rest is today's syntax.
- **Error messages to write:**
  - expected an indented block after `then`;
  - this line is indented less than its block, but more than the enclosing one;
  - unexpected indent;
  - `end` doesn't line up with the block opened at line N;
  - `;` after a one-line `then` is ambiguous;
  - `#f80` is no longer a colour literal; write `:f80` (transitional, §3.3).
- **Mistakes this removes:** a missing or extra `end` is no longer a parse error
  at the end of the file. With `end` optional, those errors become alignment
  errors reported near the mistake.
- **Mistakes this adds:** a statement dedented by accident silently leaves its
  block. Python has the same problem, and it is the cost of the design.

### 6.3 Bug-freedom

- **The type checker and lint can't see a wrong dedent.**
- **The migration can be proven safe.** Old parse ≡ new parse, comparing ASTs
  modulo spans. `source_diff.rs:8-21` already compares ASTs ignoring layout.
- **Below the AST nothing changes,** so the differential oracle, the golden
  corpus and the Kani proofs are unaffected. Golden outputs must be
  byte-identical after the migration.

### 6.4 Performance

Runtime is unaffected because the IR is identical. Parse cost is one column
comparison per statement start. P0 measures parse time over the corpus.

### 6.5 Live coding and hot reload

- **State survives the migration.** State keys hash module + fn chain + name +
  ordinal (`compiler/state_ids.rs:95-124`), not offsets, so deleting `end`s
  keeps `state` slots across a reload. Add a source_diff test that a migrated
  file reloads as a layout-only change. The `export`→`pub` respelling came back
  as `SourceChange::Full` (`source_diff.rs:21`), so this has to be checked.
- **Half-typed indentation is out of scope.** Mid-edit indentation can be a
  valid, different program that hot-reloads. Per the 2026-11-11 decision, the
  IDE layer handles it (reload debounce, structure-change markers, paste
  reindent), not this plan.

### 6.6 Introspection, DM and program manipulation

- **Unaffected:** literal edits (DM v2), provenance, `explain`, slicing,
  speculate/autodiff and edit paths (`edit_path.rs` is value paths only).
- **Inserting statements:**
  - `goal_based_editing.rs:432` inserts at top level, which is fine.
  - An insertion into a block must use the block's column.
  - A generator that can't be sure of indentation can emit an explicit `end`,
    which is the fallback a permanently optional `end` gives for free.

### 6.7 Domains

| Domain | Effect | Example |
|---|---|---|
| Games | Better: the `end` pyramids in per-frame code go | `cheesecake/games/*`, `network-graph/app.ptl:1330-1351` |
| Graphics / creative | Neutral. Colour-heavy code (1,321 colours in petal-creative) gets a one-time `#` → `:` respelling. | `core-runtime/bloom/src/overlay.ptl:71` |
| UI / JSX | Neutral: holes look like today minus `end` | `petal-creative/packages/design-material/inputs.ptl:84` |
| Dashboards / data | Better: value-position `if`/`for` read like OCaml or Elm | `finance-dashboard/app.ptl:350` |
| Editor scripting / config | Neutral | `garden/` |
| Embedding hosts | Neutral to slightly worse: snippets keep working with their `end`s, and base indent covers indented ones | `core/tests/all/native_fn.rs:97` |

### 6.8 Maintainability

| Stage | Change | Size |
|---|---|---|
| lexer | `#` comments, `:hex` colours with the previous-token rule (B16), the transitional `#hex` error, the `ElseIf` merge, the `first_on_line` flag, `//` and `elsif` deprecation | S–M |
| parser (parse.rs) | column-measured blocks alongside `end`; rules B1, B3, B4, B5; errors | M |
| CST / cst_project | blocks may end without `end`; spans | S–M |
| fmt | Today reindent derives indentation from structure (`fmt/reindent.rs`). It must now *preserve* structure and only normalise widths, and its gate (`fmt/mod.rs:34-40`, token-stream equality) becomes AST equality, because removing `end` changes tokens. Add `end` removal. | M |
| lint fixes | `lint/to_match.rs:252-351` and `lint/repeat.rs:348-360` emit and require `end`. They can keep emitting it (still valid) and switch later. | S |
| other emitters | `suggest/return_types.rs`, `fix_imports.rs`, `compiler/mod.rs` and `typecheck/mod.rs` hint text (19 `end`-emitting sites). All remain valid, so this is optional. | S |
| `//`-aware code | `literal_edit.rs:780,1808,1830,1953`, `fix_imports.rs:300,844`, `lsp/document.rs:836`, `lsp/server.rs:341,361,393`, `lsp/members.rs:502-545`, `project_host.rs:357`, `static_value.rs:1042`, `fmt/spacing.rs:274`, `fmt/mod.rs:184,229` | S |
| fuzzers | `backend/bytecode/*fuzz*.rs` emit `end`, which stays valid. Add an `end`-less mode to cover layout. | S |
| migrate | `hash-comments` and `end-lines`, plus Markdown mode | M |
| tree-sitter | `editor-support/tree-sitter-petal/grammar.js` needs a `scanner.c` external scanner for indent/dedent, with `end` as an optional closer. Update the queries and corpus tests. Garden's highlighting depends on it. | L |
| Garden | indent after `then`/`do`/`else`/`->`/fn headers, dedent on `else`/`else if`, paste reindent; the plain-paste test (`editor_view.rs:3202`) changes | M |
| vim, website | `editor-support/vim/syntax/petal.vim:20-32`; `petal-lang.org/frontend/src/lib/petal/highlight.ts`: `#` comment rule, `:hex` colour rule, `else if` | S |
| docs | the language guide (Color Literals at `language-guide.md:235` and its section; If / Else), syntax/overview (lexical table, keywords, `elsif`), writing-petal-guide, line-continuation, implicit-return-values, CLI.md, help.rs, CHANGELOG; `doc-snippets.test.ts` regexes | M |
| hosts and libraries that print Petal | anything that formats a colour back as Petal source (`to_glsl` emits GLSL, not Petal; check DM v2's colour edits, which rewrite colour literals in place in `literal_edit.rs`) | S |

The work is noticeably smaller than in the Pythonic draft, because `end` stays
valid and every emitter, fuzzer and embedded snippet keeps working untouched.

## 7. Migration

### 7.1 Lexical migrations (P1–P2, independent of layout)

They run in this order inside `migrate::run()`. Each is token-based: strings,
raw strings (including the GLSL in cheesecake's `"""` shaders) and comment
text are never rewritten.

| Migration | Rewrites | Sites | Gate |
|---|---|---|---|
| `colour-literals` | `#f80` → `:f80`, for colour tokens only | ~9,100 | AST equality (same record desugaring) |
| `hash-comments` | `//` → `#` (adding a space if there wasn't one); `// petal-lint-ignore X` → `# petal-lint-ignore X`; line-start `//` in JSX children → `{/* … */}` | ~42,400 lines | token-stream equality ignoring comments |
| `else-if-nesting` | legacy `else if … end end` → a chain with one `end`, when the nested `if` is the `else`'s only statement; otherwise `else`⏎`if` | 8 | IR equality (`ir_equiv.rs`), since the AST changes from a nested `if` to a chain |
| `elsif-to-else-if` | `elsif` → `else if` | ~4,490 | AST equality |

`colour-literals` must run before `hash-comments` takes effect, so that no `#f80`
is left to hit the transitional error.

### 7.2 `end-lines` (P4)

1. **`petal fmt` first.** It derives indentation from the `end` structure and
   never changes tokens. After it, indentation and `end`s agree in every file.
2. **Delete each `end` that is alone on its line,** including a trailing
   comment. That comment moves to its own line at the block's column, or onto
   the last body line.
3. **`end)`, `end,`, `end]`, `end}` → the bracket,** kept at the statement
   column. Skip when two block lambdas share a call: keep the `end`.
4. **Leave every same-line `end` alone:** one-liners and `end |> f`. They are
   still valid.
5. **Gate:** the parse before equals the parse after (AST modulo spans).
   Otherwise the file is not written, and the case is reported as a migration
   bug.
6. **Idempotent:** a second run finds no `end`-only lines.

Optional later step, `end-inline` (P6): drop the trailing `end` from
one-liners where B1 and B3 can't change the meaning. Parenthesise when an
operator follows. Same gate.

### 7.3 Beyond `.ptl` files

Embedded snippets and Markdown fences **keep working without a migration**,
because `end` stays valid. Migrate them for consistency, not correctness:

| Corpus | How |
|---|---|
| Markdown fences (`docs/`, then re-run `scripts/import-docs.ts`) | `petal migrate-old-code --markdown`, which also rewrites the `// expected` convention in `doc-snippets.test.ts` to `# expected` |
| Rust, TS and C++ test strings | Leave them. They test the parser, and `end`-ful input is still supported input. Add new tests for `end`-less input instead. |
| Vendored copies (cheesecake `third_party/petal`, worlds-fair `tools/vendor-petal.ts`) | Re-vendor any time after P3: old scripts keep parsing. Migrate their `.ptl` with the vendored CLI when convenient. |

Order: petal → website → worlds-fair → petal-creative → cheesecake. One commit
per project holds the migration alone, so `git blame --ignore-rev` can skip it.

### 7.4 As implemented (P4, done 2026-11-12)

`end-lines` is `core/src/migrate/end_lines.rs`, the last text migration of
`petal migrate-old-code`; Markdown mode is `core/src/migrate/markdown.rs`.
User docs: [CLI.md](../CLI.md#migrate-old-code--rewrite-old-syntax).
Fixtures: `core/tests/all/migrate_old_code.rs`,
`cli/tests/migrate_old_code_cli.rs`. Where it settled something §7.2 and
§7.3 leave open:

- **`end,` is kept** (§7.2 step 3 would leave a line that starts with a
  comma). This repo had three such lines. So are `end |> f`, `end.f()`, the
  `end` of a block with no statement, an `end` before a line that starts
  with an operator (§3.8), and any `end` whose removal changes the parse.
- **A comment after a removed `end`** stays on a line of its own at the
  block's column (`end # if` → `    # if`); it is not moved onto the last
  body line.
- **A file with no `end` at the start of a line is not formatted.** Any
  other file is formatted whole (step 1), so the migration's diff also holds
  what `petal fmt` changes in a file that was not formatted (spacing in
  records, aligned `=`).
- **fmt was changed first:** it moved a line continued by a trailing
  operator (`a ||` / `  b`), and the second line of an `import m: a,` list,
  to the block's column. Such a line now keeps its offset from its
  statement, as a line that starts with an operator did.
- **The gate is soft for `end-lines` only.** A file that fails it (fmt
  cannot lay it out, or the AST differs) is written as the other migrations
  left it and listed under "skipped by a gate"; the run exits 1. Any other
  gate failure writes nothing for the file, as before.
- **`OldForm::Optional`:** `end` is still the language, so the registry
  marks `end-lines` as neither deprecated nor removed, and `petal lint`
  reports nothing (open question 1).
- **Markdown:** a fence that parses gets every selected migration; one that
  only lexes gets `colour-literals`, `hash-comments` and `elsif-to-else-if`
  (`MigrateOptions::fragment`); one that does not lex is left. The `ignore`
  tag decides nothing. `no-migrate` on a fence keeps it as written.
- **CLI:** `--except`, a summary with counts per migration, the list of
  gate-skipped files (exit 1), and a walk that also skips `temp`, `out` and
  `dist`. A fully legacy tree migrates in one run
  (`a_fully_legacy_tree_migrates_in_one_run`).

**This repo, 2026-11-12:** 331 of 480 `.ptl` files changed, 11,232 `end`s
removed, 9 at the start of a line kept, no gate failure. Checked on the
migrated tree: `cargo test` in `core` (both feature configs, and the
ignored `over_repo_corpus` sweeps), `cli`, `garden` and `test/ui-corpus`
(the frame-gate, memo, static-cache and DM differentials); `npx vitest run`;
`tools/test-examples.ts` (golden corpus byte-identical, optimizer and static
cache differentials); and `tools/verify.ts --plan lint-fix` against the tree
before the migration (198 files `identical-ir`, none `changed`). Two things moved with the lines: `docs/stdlib.json` (line
numbers of the prelude) and the positions asserted in
`core/tests/all/fn_introspection.rs`. `core/tests/all/layout_probes.rs`
reads copies of ten files taken with their `end`s
(`core/tests/fixtures/with-ends/`).

**The docs' Markdown** got the lexical migrations only (`colour-literals`,
`hash-comments`, `elsif-to-else-if`: 52 files, so the documentation tests
now read `print(x) # expected`). `end-lines` was **not** run over `docs/`:
its prose is tied to the `end`s in its examples ("shares the single closing
`end`", "this document writes the `end`"), some pages quote line numbers of
their own snippets (docs/internals/lsp.md), and the guide indents by four.
A dry run removes 379 `end`s from 43 files with no gate failure, so the
command is ready when P5 rewrites the prose:
`petal migrate-old-code --markdown --only end-lines docs/`.

## 8. Phases

**P0: prototype** (worktree, about 1 week)
- Column-measured blocks with optional `end` in parse.rs.
- Run the new parser over every `.ptl` in all five projects, *unchanged*. Every
  file that parses today must parse to the same AST or fail with a B5 error.
  Count both outcomes.
- Run fmt + `end-lines` over the corpus with the gate, and report the
  automatic-conversion rate and the parse-time delta.

**P1: colours and comments** (ships on its own, before layout)
- **P1a.** `:hex` colour literals with rule B16, and the `#hex` deprecation
  warning. Ship the `colour-literals` migration, update the colour docs and the
  highlighters, then migrate every corpus, vendored projects included.
- **P1b.** `#` comments, with the transitional `#hex` error. Ship
  `hash-comments`, with `//` as `OldForm::Accepted` reported by `petal lint`
  `old-syntax`, becoming a warning later.
  - Update vim, tree-sitter (`grammar.js:765`), the website highlighter and
    `doc-snippets.test.ts` (`// expected` → `# expected`).
  - Migrate the corpora.

**P2: `else if`** (independent; can land before or after P1)
- The `ElseIf` lexer merge.
- `else-if-nesting`, then `elsif-to-else-if`.
- `elsif` goes `Accepted`, then warns, then is removed.
- Update the docs and the highlighters.

**P3: the layout parser in production**
- parse.rs and CST spans.
- Errors, with fixtures for each B-row.
- fmt: preserves structure, gates on AST equality, and removes `end`.
- LSP.

**P4: `end-lines` migration**
- Fixtures in `core/tests/all/migrate_old_code.rs`.
- Markdown mode.
- CLI.md, help.rs, CHANGELOG.
- Migrate the petal repo, then the other projects.

**P5: tooling**
- tree-sitter with an external scanner.
- Garden: indent after an opener, dedent keys, paste reindent.
- The guide and docs are rewritten in the `end`-less style.

**P6: cleanup**
- `end-inline` for one-liners, if open question 1 lands that way.
- Switch the emitters and fuzzers to the canonical style.
- Drop the transitional `#hex` error.
- Remove `elsif`.

## 9. Testing

- **Corpus invariance.** Every corpus file parses to the same AST under the old
  and new parsers, or fails with B5. Golden outputs are unchanged.
- **Migration invariance.** Each file's parse is the same before and after
  `end-lines`. The differential oracle runs on the migrated corpus.
- **Generative test.** Generate a random AST, print it with `end`s and without,
  parse both, and compare. This covers nesting depth, blocks inside brackets and
  value-position blocks.
- **One fixture per B-row,** valid and error.
- **fmt** is idempotent and never changes the AST.
- **source_diff:** a migrated file reloads as a layout-only change, and `state`
  survives.
- **Doc snippets:** all fences lint and run, in both styles during the
  transition.

## 10. Alternatives considered

- **A. Do nothing.** `end` is explicit and robust to paste and generation. The
  cost is verbosity and pyramids.
- **B. Pythonic: a trailing `:`, no `then`/`do`.** This was the first draft of
  this plan.
  - Every header gets rewritten: about 26k `then` and 8.5k `do`.
  - It needs a per-file legacy mode, because `fn f(x)`⏎ means different things
    in the two syntaxes.
  - It adds the `import ui:`⏎ and `->`/`:` ambiguities, and forces parentheses
    on nested one-liners.
  - Dropped 2026-11-11: the trailing `:` is no longer a requirement.
- **C. Make `end` mandatory-but-checked:** keep `end`s and make indentation
  binding (a misaligned `end` is an error). It gets the "indentation can't lie"
  property with zero migration, but none of the brevity. It is a possible first
  step: P3 with fmt not yet removing `end`s *is* this alternative.
- **D. `--` comments instead of `#`.** No colour-literal clash, but unfamiliar
  to JavaScript and Python users, and `a--b` becomes a comment. Rejected
  2026-11-11 in favour of `#` comments with colours moved to `:hex`.
- **F. Keep `#` for colours and require `# ` (with a space) for comments.** No
  colour migration, but `#fade in` and `#add(x)` would silently lex as colours.
  Rejected 2026-11-11.
- **E. Tooling only:** Garden block guides and `end` labels. Readability gains,
  but the code isn't shorter.

## 11. Open questions

1. **What happens to `end` after the migration?** To be revisited. The options
   are:
   - fmt always removes a multi-line `end`;
   - a lint (`redundant-end`) reports it;
   - it stays allowed as an optional marker for long blocks, as in Scala 3.

   Related: should fmt eventually strip the inline `end` from one-liners where
   it is safe (P6), or leave one-liners as written? P1–P4 don't depend on this.
2. **How long does the transitional `#hex` error last?** It stays until the
   vendored projects (cheesecake, worlds-fair) have run `colour-literals`.
3. **Symbol literals.** Is Petal ever likely to want a `:name` symbol literal?
   If so, decide its spelling now, because `:hex` colours make `:add` and
   `:face` colours (§3.3).
4. **Rename this doc** to `docs/tasks/indentation-blocks.md`, now that it
   isn't Pythonic.
