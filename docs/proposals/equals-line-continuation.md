# Proposal: a value may start on the line after `=`

Status: proposed, 2026-11-19

## Summary

Let the right-hand side of a binding or an assignment start on the next line
when the `=` ends its line and the next line of code is **indented further
than the statement**:

```petal ignore
let header =
  <text size={20}>Settings</text>
```

The continuation holds exactly one expression, as the line after a trailing
`+` does. `=` does not become a block opener. The rule covers `let`, `var`,
`const`, `config let`, `state`, `state var`, `memo`, `set`, plain assignment
and the compound forms (`+=`, `++=`, …). A next line that is *not* indented
further is a new statement, and the error says how to write the value.

**Recommendation: adopt.** It is a parser-only change, about 30 lines in
`core/src/parse.rs`. No existing program parses differently: all 5,402 corpus
files give identical ASTs on a prototype. It removes the single most common
first-try error in the design-app B1 bench. Improve the error messages either
way, because the not-indented case needs them anyway.

## Motivation

**Newcomers and agents write it, and experienced Petal writers never do.**

- In the design-app-prototype B1 authoring bench, `let x =` followed by a line
  break caused **13 of 30** first-try failures in the docs-only arm
  (`~/design-app-prototype/bench/b1-authoring/README.md:47`, #1 at `:142`).
  Every agent fixed it on its second attempt by turning the `let` into a
  `fn header()`. That fixes the error, but it changes the meaning: the value
  becomes a function that is called, not a value.
- The Petal corpus (1,059 `.ptl` files in this repo, `petal-creative`,
  `cheesecake/games` and `worlds-fair`, not counting `.claude/worktrees`)
  holds 58,682 bindings and **0** of this form. It can't hold any, because
  the form is a parse error. So the corpus shows that people who know Petal
  have learned the rule. It says nothing about how often the form is wanted.
- The rule is written down (`docs/syntax/line-continuation.md`, "What does
  not continue"), but no reason was ever recorded. The text arrived in the
  docs reorg `7c004b97` as a description of how the parser behaved. The
  indentation plan (`docs/tasks/pythonic-syntax.md`) doesn't discuss `=`
  continuation. It did reject Scala's `=` as a *function body* opener (B8,
  `:260`), which is a different construct.
- Until commit `499a653c` (2026-11-19) the obvious workaround,
  `let x = (⏎ … ⏎)`, was itself a parse error, against the same doc's
  promise that line breaks inside `( )` are free. It works now.

What the corpus does show is how multi-line values are written today:

| Form at the end of the `=` line | Bindings that continue past the line |
|---|---|
| `if` / `match` / `for` / `while` header | 912 |
| `[` | 376 |
| `fn(` lambda | 179 |
| `{` | 113 |
| `<element` | 17 |
| `name(` call left open | 17 |
| `(` grouping | 0 |
| a trailing binary operator | 51 |

There are also 999 single-line bindings longer than 120 columns: 332 calls,
277 list or record literals, 266 one-line `if`/`match`, 41 strings and 83
other expressions. Almost all of them can be wrapped today: inside the
call's parens, inside the literal's brackets, or after an operator. So for
experienced writers the change adds little. It is a learnability fix.

## Design

**Rule.** When `=` (or a compound `op=`) is the last token on its line,
blank lines and comment lines are skipped. If the next line of code starts
at a column greater than the indentation of the statement's own line, the
value starts there. That line is not measured for layout, just like a line
continued after an operator. The value is one expression. It may itself be a
construct with an indented body (`if`, `match`, `for`, a lambda), laid out
from the line it starts on, as usual.

**Not a block.** A second statement-like line at the continuation's column
is an error, not a block whose last line is the value:

```petal ignore
let a =
  1
  2      # error: unexpected indent
```

**Not indented further: no continuation.** When the next line of code starts
at the statement's column or to its left, it is a new statement, and the `=`
has no value:

```
let a =
print(1)
error: the value after `=` is missing: write it on the same line as the `=`,
or on the next line indented further than this one [line 1, column 8]
```

This is the important safety rule for live coding. While someone is halfway
through typing `let a =` above existing code, the file must stay a parse
error (Garden keeps the last good program), not silently become
`let a = print(1)` and hot-reload. A trailing operator has no such rule
today (`a +⏎print(1)` continues at any indentation). The difference is
deliberate, because an unfinished `=` is far more common while typing.

**Also proposed:**

- `=` at the end of the input gives the same "value is missing" message.
  Today it is "Unexpected token: end of input".
- The "unexpected indent" error on the line after a one-line continuation
  adds: "the value after `=` is one expression; to compute it in steps, use
  a `fn` or a `( )` block".

**Unchanged:** parameter defaults (`x = 1` in a parameter list) already skip
line breaks (`parse.rs:2951`). Element attributes (`a=⏎{1}`) are inside a tag,
where the lexer already drops line breaks. `.` still does not continue.

## Before and after

All "before" snippets were run today. All "after" snippets were run against
a prototype in a throwaway worktree (see the implementation sketch).

**1. The B1 failure (the case this is for).**
From `bench/b1-authoring/README.md:146`, as the agents wrote it, and their fix:

```petal ignore
# before: a parse error
let a =
  <text>hi</text>

# before: what every agent changed it to (now a function, called where used)
fn a()
  <text>hi</text>
```

```petal ignore
# after (proposed)
let a =
  <text>hi</text>
```

For the writer, there's no error, no retry and no change of meaning. A
reader sees one more layout of a `let`, the same one JS, Kotlin, Elm and F#
use.

**2. A long arithmetic binding**, from
`examples/productivity/file-browser/app.ptl:133` (trimmed context):

```petal ignore
# before: one 122-column line
let COL_NAME = main.w - COL_SIZE - 16 - (if SHOW_DATE then COL_DATE else 0 end) - (if SHOW_KIND then COL_KIND else 0 end)
```

The obvious wrap today fails, because a line that starts with `-` is a new
statement:

```petal ignore
let COL_NAME = main.w - COL_SIZE - 16
  - (if SHOW_DATE then COL_DATE else 0 end)   # error: unexpected indent
```

```petal ignore
# after (proposed): ran on the prototype, prints 584 as the original does
let COL_NAME =
  main.w - COL_SIZE - 16 - (if SHOW_DATE then COL_DATE else 0 end) - (if SHOW_KIND then COL_KIND else 0 end)
```

The new form buys 15 columns. Breaking after each `-` already works and is
still the better fix here. This change doesn't help much with long
expressions.

**3. Where the old form is better: a call.** From
`examples/games/solar-system/app.ptl:282`:

```petal ignore
set pan = vec2(approach(cur: pan.x, target: target.x, rate: follow_rate), approach(cur: pan.y, target: target.y, rate: follow_rate))
```

Wrapping inside the call works today and reads better than starting `vec2(`
on its own line:

```petal ignore
set pan = vec2(
  approach(cur: pan.x, target: target.x, rate: follow_rate),
  approach(cur: pan.y, target: target.y, rate: follow_rate))
```

The guide should keep recommending "open the bracket, element or block on
the `=` line" as the normal form. The 1,600 bindings in the table above
already do this, and nothing about them changes.

**4. An element with a long open tag**, from
`petal-creative/design/studio/screens.ptl:89`:

```petal ignore
let art = <zstack width={640} height={640} fill={linear(160, [:2A1A10, :0E0806])} effects={[shadow(:00000099, blur: 40, y: 12)]} align="top_leading">
```

Moving `<zstack` to the next line saves only the `let art = ` prefix. An open
tag's attributes can already go on separate lines (`language-guide.md`,
Elements), which is the real fix. This is neutral.

**5. Interaction with `state`, `tunable` and in-place edits.**

```petal ignore
# after (proposed)
state count =
  0
let speed =
  tunable(5.0) # m/s
```

On the prototype, `state` initializes as usual, and `show-static` still
classifies both forms of `let speed` as static. `modify_source_with_goals`
setting `speed` to 7.5 rewrote only the number and kept the line break, the
indent and the comment. Setting a record field, and replacing a whole record
value, also left the layout alone. Hot reload sees the same AST as the
same-line form, so slots and call paths are unaffected.

**Migration:** none. No existing program changes meaning, and no rewrite is
offered. `petal fmt` leaves a continuation line as written (it already
keeps a continued line's offset from its statement,
`docs/tasks/pythonic-syntax.md:965`).

## Assessment

### Project goals

*Findings.* The change advances "easy to use" directly for newcomers and
agents (the B1 numbers), and nothing else. It touches none of the dataflow,
effects or `state` design. It is generic language surface, not something
Petal should be distinctive at. *Opinion.* Generic surface is exactly where
Petal should match what people guess; being different here buys nothing.

### Ease of writing

*Findings.*
- JS/TS, Kotlin, Swift, Go, Ruby, Lua, Rust, Elm, F#, Haskell, OCaml and Nim
  all accept a value on the line after `=`. Python is the outlier: it needs
  parens or a backslash. Petal's layout rules come from the Python-style
  plan, which is probably how the restriction arrived, and the parens form
  now works as Python's does.
- The new form is a second way to lay out a binding, so the guide must say
  which to prefer: keep the opener on the `=` line, and use the next line
  for a value that has no opener (a bare element, a long expression).
- Wrong uses give two specific errors (shown above). Both were tested on the
  prototype.

*Opinion.* The cost to learn it is close to zero, since it is what most
people try first. The "two ways" cost is real, but small and easy to
explain.

### Bug-freedom

*Findings.* No existing program changes meaning: all 5,402 files gave
identical `show-ast` output, old against prototype. The risky case, an
unfinished `let a =` swallowing the next statement, can't happen, because
the next line must be indented further. The leftover risk is an unfinished
`=` directly above an *indented* line, say inside a block:

```petal ignore
if ok then
  let a =
    step()     # a was being typed; step() becomes its value
```

That only happens when the line below was already indented deeper than the
`let`, and it is the same exposure a trailing `+` has today. Type checking,
lint, the differential oracle, goldens and the Kani proofs see an ordinary
AST and are unaffected.

*Opinion.* Acceptable. The indentation rule closes the common case.

### Performance

*Findings.* At run time there is no change, since the AST is identical. At
parse time it costs one look-ahead past line breaks, only when `=` ends a
line. The full core test suite ran in the same time on the prototype
(82.6 s and 104.3 s, against 82.2 s and 104.3 s on `main`). No benchmark is
needed.

### Live coding and hot reload

*Findings.* It is syntax only: a reloaded program equals a fresh compile.
Mid-edit states stay parse errors (see Bug-freedom), so Garden keeps
running the last good program while someone types.

### Introspection, debugging and program manipulation

*Findings.* Provenance, `explain`, spans and static values work through the
prototype unchanged. Goal-based edits keep the layout (pair 5). One cosmetic
difference: `show-static` reports a declaration at its *value's* line (2)
rather than its `let` line (1). That is worth fixing to point at the `let`
before tools rely on it.

### Maintainability of the implementation

*Findings.*
- **Parser:** a ~25-line helper and 7 call sites. Nothing changes in the
  lexer, AST, compiler, IR or VM.
- **CST:** the line break becomes trivia inside the binding node. The debug
  build's CST↔AST projection check passed across the whole suite.
- **Formatter:** no change needed.
- **tree-sitter:** already parses the form, since newlines are extras there
  (checked with `tree-sitter parse`). `queries/indents.scm` should add a
  rule so editors indent the line after a trailing `=`.
- **vim:** syntax only, so no change.
- **Docs:** `line-continuation.md` moves `=` from "does not continue" into
  its own section, the guide's binding section gets one sentence, and
  `CHANGELOG.md` gets an entry.
- **Tests:** one test (`bracket_line_breaks.rs:95`, which pins today's error)
  is inverted, and new cases are added.

It adds one invariant: a continuation after `=` must be indented further,
unlike an operator continuation. It removes a special case from the docs
(`=` was the only "obviously unfinished" token that did not continue).

### Domains

- **UI apps, Garden panels, petal-design documents:** these help. Elements
  are the value people most want on their own line (B1).
- **Games, graphics, dashboards, data:** neutral. Long math and queries
  already wrap at operators or brackets.
- **Config files (`.ptl` as configuration):** neutral to slightly helpful.
  In-place edits keep the layout.
- **Web, embedding hosts:** no effect. It's syntax only, with no API change.

### Compatibility and migration

*Findings.* Zero programs in the corpus break or change. It is purely
additive, so no warning period is needed. It doesn't close off a future
`=`-as-block-opener design, although this proposal recommends against that
design.

### Prior art

Elm (cited in the README) and F# require the value on the next line to be
indented, which is the rule proposed here. Scala 3 goes further and makes
`=` open an indentation region (a block). That was rejected for Petal's
function bodies (`pythonic-syntax.md` B8), and the same reasons apply here.
Python refuses the form and recommends parentheses (PEP 8). JS needs no
indentation, because it has no layout rule.

## Alternatives considered

1. **Do nothing, improve the error message.** "Put the value on the same
   line as `=`, or wrap it in `( )`". This is now true and cheap. The bench
   shows agents already recover on attempt 2, so this alternative keeps a
   retry per author but removes the meaning change (`fn` instead of `let`).
   *This is the fallback if the proposal is rejected, and it ships either
   way for the not-indented case.*
2. **`=` as a block opener** (`let x =⏎  a⏎  b` is `b`). This is more power
   and more ambiguity: a block value with no keyword, and a silent change
   of meaning when the indentation is off. Rejected, as for `fn` bodies.
3. **Continue at any indentation, like operators.** This is simpler to
   describe, but an unfinished `let a =` would swallow the next statement
   during live editing. Rejected.
4. **A lint or tooling fix only.** That doesn't help anyone writing outside
   the tooling, which is where B1's agents were.

## Implementation sketch

1. **Parser** (`core/src/parse.rs`): add `continue_after_assign(stmt_start)`,
   which looks past line breaks. If the next line of code is first on its
   line and `column_of(next) > indent_of(stmt_start)`, it checks for tabs
   and advances to that line. Otherwise it returns the "value is missing"
   error. Call it right after the `=` in `parse_let` (`:1979`), `set` and
   `set op=` (`:2007`, `:2028`), `parse_state` (`:2061`), `parse_memo`
   (`:2106`), and plain and compound assignment (`:2870`). The prototype is
   exactly this, and it passed the full core suite (except the pinned test).
2. **Errors:** make `=` at end of input use the same message, and add the
   unexpected-indent hint.
3. **Tests:** invert `bracket_line_breaks::a_let_value_still_starts_on_the_line_of_its_equals`,
   and add cases for every binding form, compound assignment, the
   not-indented error, the two-line error, comment and blank lines, CST
   round-trip, and goal-based edits on a next-line value.
4. **Docs, editor support and the `show-static` line**, as listed under
   Maintainability.

It's about half a day, in that order.

## Open questions

For the language owner:

1. Include `set`, plain assignment and compound `op=`, or only declarations
   (`let`/`var`/`const`/`state`/`memo`)? The prototype includes all of them,
   for consistency.
2. Should `petal fmt` pull a short continuation back onto the `=` line when
   it fits (normalizing to one layout), or leave it as written? I recommend
   leaving it, as for operator continuations.
3. Should the guide name the opener-on-the-`=`-line form as the preferred
   style, and should `petal lint` say anything about the next-line form when
   the value opens a bracket? I recommend a guide sentence and no lint.
