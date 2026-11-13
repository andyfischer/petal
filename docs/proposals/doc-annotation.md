# Proposal: `@doc(…)`, a documentation annotation

Status: proposed, 2026-11-13

## Summary

Add `@doc(…)`, the first **compile-time annotation**. It fills the slot that
[annotation-syntax.md](annotation-syntax.md) reserved as F1, "annotations from a
fixed vocabulary that do not call anything". It is written before a
declaration and holds its documentation as literal data: a Markdown body, plus
optional structured fields (`params`, `returns`, `options`, `example`, `see`,
`deprecated`, `hidden`).

```petal ignore
@doc("""
Constrain a value to the range `[lo, hi]`. Three `int` arguments give an
`int`; one `float` argument makes the result a `float`.
""",
  params: {value: "The number to constrain.", lo: "The lower bound.", hi: "The upper bound."},
  example: """
    clamp(15.0, 0.0, 10.0)   # 10.0
    clamp(9, 0, 5)           # 5     (int in, int out)
  """)
pub native fn clamp(value: num, lo: num, hi: num)
```

The arguments must be literals, so the parser alone can read every doc:
nothing is evaluated and nothing reaches the IR. A new `petal doc --json`
command emits each package's public surface (signatures from the
declarations, prose from `@doc`) for the website and other tools.

**Recommendation: adopt, with three choices that differ from the
first-glance design:**

1. `@doc` is **not** a function. `@const` and `@tunable` lower to a call around
   the value; `@doc` is attached to the declaration and lowers to nothing. That
   is also why `doc` stays an ordinary name: the corpus binds `doc` about 250
   times (petal-creative alone 95), and none of those break.
2. **Tools fall back to the comment block directly above a declaration when it
   has no `@doc`.** Settings panels already read that comment today
   (`static_value::leading_comment`). With the fallback, the ~350 documented
   `pub fn`s in `ui` and `bloom` show up on day one, and `@doc` is used where
   structure pays: the builtins, widget style keys and parameter text.
3. **`params` is checked against the signature** by a lint, so a renamed
   parameter can't leave a stale doc behind. This is the main thing `@doc` has
   over a comment convention.

Implementing it is moderate: about 1–1.5k lines with tests, all in the parser,
AST, lint, LSP, formatter, tree-sitter and one new CLI command. The compiler,
IR, bytecode and VM don't change. Moving the existing prose into `@doc` is
separate work and the bigger job.

## Motivation

Inventory (2026-11-13) of what generated API docs would draw on:

- `tools/extract-stdlib.ts` → `docs/stdlib.json` already extracts 254 native
  and `std` signatures, kept current by CI (`.github/workflows/ci.yml:123`).
  It has **no descriptions**. Prose lives in hand-written Markdown:
  `docs/Builtins.md` (2,416 lines), `core-libs/petal-ui/docs/reference.md`
  (689), `core-runtime/bloom/docs/components.md` (358). Nothing ties a
  paragraph to its declaration except the heading text.
- The extractor does not see Petal-source functions outside `std.ptl`.
  `ui`'s Petal layer has ~366 `pub fn` (`draw.ptl` 120, `widgets.ptl` 94,
  `interaction.ptl` 43, …) and `bloom` has 175. None are extracted.
- Doc comments exist by convention only. The share of `pub fn`s with a comment
  block directly above them (a scan of `.ptl` files for `^pub (native )?fn`
  preceded by a whole-line comment, excluding `node_modules`/`target`):

  | Corpus | `pub fn` / `pub native fn` | with a comment above | avg lines |
  |---|---|---|---|
  | `core/prelude` (std) | 20 | 100% | 1.6 |
  | `core/packages` (mostly `native fn`) | 182 | 7% | 3.1 |
  | `core-libs/petal-ui/prelude` | 475 | 50% | 3.6 |
  | `core-runtime` (bloom, text-layout, glsl) | 211 | 55% | 3.9 |
  | `examples/` | 422 | 50% | 2.1 |
  | `~/petal-creative` | 417 | 72% | 2.1 |
  | `~/cheesecake` (incl. vendored copies) | 15,495 | 29% | 2.4 |

  The 50% for `ui` understates it, since an overload set usually documents its
  first form only (`widgets.ptl:199`, `bloom/src/controls.ptl:24-27`).
- Comments mix docs and layout: `# ── Switch ──` section rulers sit above
  documented functions (`bloom/src/controls.ptl:20`), separated by a blank
  line.
- Structured information is written as prose and nothing checks it:
  `widgets.ptl:196-198` lists the eleven keys `style` accepts in a sentence.
- Setting labels: hosts label a setting with the comment block above it
  (`docs/syntax/settings.md:214-221`). A trailing comment is not read, so
  `examples/games/physics-playground/app.ptl:60-64` (`let gravity_px =
  tunable(980.0)   # px/s^2 at 1.0 g`) gives a settings panel no labels.

## Design

### Syntax

```text
@doc(<body>?, <field>: <literal>, …)
```

- Written before a declaration, on its own line(s) or the same line, before
  `pub`, as every annotation is. It stacks with others in any order:
  `@doc("…") @tunable(0, 10) let SPEED = 4.0`.
- It goes on: `fn`, `pub fn`, `fn T.m`, `native fn`, `class`, `enum`, `let`,
  `var`, `state`, `memo`. That makes `fn`/`native fn`/`class`/`enum` the first
  declarations that take an `@` at all. For them the reserved-slot error now
  says `@doc` is the only annotation they take.
- The **body** (first positional argument) is Markdown. Its first sentence is
  the summary that index pages and completion show.

### Fields (fixed vocabulary)

| Field | Literal | Meaning |
|---|---|---|
| *(positional)* | string | The Markdown body. |
| `params` | record of strings | One line per parameter, keyed by name. |
| `returns` | string | What the function gives back. |
| `options` | record of strings | The keys a trailing `style`/`opts` record accepts. |
| `example` | string, or list of strings | Petal source, shown as code. |
| `see` | list of strings | Related names, rendered as links. |
| `deprecated` | string | What to use instead; tools mark the name. |
| `hidden` | bool | Leave this out of generated reference pages. |

An unknown field is an error with a did-you-mean (`param:` → `params:`). New
fields cost a line in the parser's table. An escape hatch such as
`extra: {…}` is an [open question](#open-questions).

### Literal-only arguments

Each argument must be a string (plain or raw `"""…"""`), a bool, or a list or
record of those. A string with `{…}` interpolation is an error:

```text
error: `@doc` takes text written out; `{bg, fg}` in a "…" string is an interpolation
  = write `\{` for a literal brace, or use a raw string: """…"""
```

This matters in practice: doc text names records (`{x, y, w, h}`) all the
time. Today a `{` in a double-quoted string opens an interpolation hole, and
`print("{x: 1}")` is a parse error, as verified. The recommended style is raw
strings for anything longer than a line.

Since nothing is evaluated, any tool that can parse can read docs: `petal
doc`, the LSP, tree-sitter consumers, and the website's build with no Petal
runtime.

### Semantics

`@doc` changes nothing a program computes. The parser removes it before
[`apply_annotations`](../../core/src/ast.rs) lowers the value annotations, and
stores it on the statement (`Stmt::doc: Option<Box<Doc>>`, beside
`Stmt::test`). A program with `@doc` compiles to the same IR as without it:
`petal ir-equal` reports "IR is equivalent", as it already does for a comment
and does not for a string `let` (checked).

Overloads: each declaration carries its own doc. `petal doc` merges an
overload set's docs. With one doc, it describes the set. With several, each
describes its own form. A `params` entry may name a parameter of any form in
the set.

### Diagnostics

| Misuse | Result |
|---|---|
| `@doc` twice on one declaration | error: "one `@doc` per declaration" |
| `@doc` before a non-declaration | the existing `ANNOTATION_NEEDS_DECL` error |
| a non-literal argument | error, pointing at the argument |
| `params: {lo: …}` naming no parameter | lint `doc-params` (warning): "`@doc` describes `lo`, which `clamp` does not take" |
| `see: ["foo"]` naming nothing in scope | lint `doc-params` (warning) |
| a `pub` item with no `@doc` and no comment | lint `undocumented-pub`, off by default and on for `core/`, `core-libs/`, `core-runtime/` |

### Comment fallback

If a declaration has no `@doc`, tools use the comment block directly above it
as its body. This is exactly the rule `static_value::leading_comment` already
applies to settings: whole-line comments, stopped by a blank line, so a section
ruler separated by a blank line is not taken. `petal doc --json` reports which
source each doc came from (`"source": "annotation" | "comment"`).

### Module-level docs

Out of scope for v1. A package's description comes from `petal.toml` and its
README. See [Open questions](#open-questions).

## Before and after

"Before" snippets are quoted from the corpus. The `std`, `ui` and `bloom` ones
parse and lint today (`./tools/run-petal.ts lint`), and the `sum` pair was run.
"After" is **proposed** and does not parse yet: today `@doc` before a `fn` gives
"`@` on a function is reserved for compile-time annotations; there are none
yet."

### 1. A builtin whose prose lives 2,000 lines away (the case this fixes)

Before: `core/packages/core/src/math.ptl:40`

```petal ignore
pub native fn clamp(value: num, lo: num, hi: num)
```

…and separately, `docs/Builtins.md:363-379`:

````markdown
### `clamp(value, lo, hi)`

Constrain a value to the range `[lo, hi]`. Three `int` arguments give an
`int` (so the result can still be a list index); one `float` argument makes
the result a `float`.
```petal
clamp(15.0, 0.0, 10.0)   # 10.0
…
````

After: the `@doc` in the [Summary](#summary).

**Change:** the description sits on the declaration and the parameter names
are checked. Renaming `lo` warns instead of leaving the Markdown wrong. Cost:
`math.ptl` goes from a 60-line declaration list to several hundred lines, and
skimming the native surface gets harder. That is the strongest objection, and
it is discussed under [Alternatives](#alternatives-considered).

### 2. A widget's style keys (structure wins)

Before: `core-libs/petal-ui/prelude/widgets.ptl:196-199`

```petal ignore
# `style` overrides any of {bg, fg, outline, focus, caret, selection, size,
# font, inset, text_style, placeholder}; omitted keys come from `ui_theme()`.
# `placeholder` is drawn dim while the field is empty.
pub fn draw_text_field(r, text, has, style)
```

After (proposed):

```petal ignore
@doc("Draws a text field; `has` is whether it holds focus.",
  options: {
    bg: "Fill.", fg: "Text colour.", outline: "Border when unfocused.",
    focus: "Border when focused.", caret: "Caret colour.",
    placeholder: "Drawn dim while the field is empty.",
    # … size, font, inset, text_style, selection
  })
pub fn draw_text_field(r, text, has, style)
```

**Change:** each style key becomes a row in the generated table and can get
its own tooltip in an editor. A later lint could check `options` against the
keys the body reads. The cost is more lines than the comment.

### 3. An overload set

Before: `core-runtime/bloom/src/controls.ptl:20-27`

```petal ignore
# ── Switch ────────────────────────────────────────────────────────────────

# The on/off switch. The knob is a spring, so it lands with a small overshoot
# that makes the control feel mechanical rather than interpolated.
pub fn switch(r: record, on: bool) -> bool
  switch(r, on, {})

pub fn switch(r: record, on: bool, opts: record) -> bool
```

After: **unchanged**. The comment fallback picks up the block above the first
form, the blank line leaves the ruler out, and `petal doc` gives the set one
doc. Adding `@doc` would only matter for documenting `opts`.

### 4. A one-line helper (the old form is better)

Before: `core/prelude/std.ptl:28-30`

```petal
# Sum of a list of numbers; 0 for the empty list.
pub fn sum(xs)
  reduce(xs, 0, fn(acc, x) acc + x end)
```

After, if converted:

```petal ignore
@doc("Sum of a list of numbers; 0 for the empty list.")
pub fn sum(xs)
  reduce(xs, 0, fn(acc, x) acc + x end)
```

**Change:** none for the reader, and slightly noisier. With the fallback this
should stay a comment. The guide should say to use `@doc` when you need a
field, a checked parameter, or to hide something, and a comment otherwise.

### 5. Settings, stacked with `@tunable`

Before: `examples/games/physics-playground/app.ptl:60-61`

```petal ignore
let gravity_px  = tunable(980.0)   # px/s^2 at 1.0 g
let launch_gain = tunable(5.0)     # slingshot: px of pull to px/s of launch
```

A settings panel built from `static_bindings` shows these unlabelled, because
the trailing comment isn't read.

After (proposed):

```petal ignore
@doc("Gravity in px/s² at 1.0 g") @tunable let gravity_px = 980.0
@doc("Slingshot: px of pull to px/s of launch") @tunable let launch_gain = 5.0
```

**Change:** the panel gets a label. `@doc` is taken out before `@tunable`
lowers, so the value is still `tunable(980.0)`. Edits to the doc text don't
touch the knob. The cost is two ways to label a setting (comment above, or
`@doc`). `static_bindings` should prefer `@doc` and fall back to the comment.

### 6. Hot reload

Adding, editing or removing a `@doc` while running produces a program that
`ir-equal` reports as equivalent, so a reload keeps every `state` slot, just
as editing a comment does. One invariant needs care: the statement's span
starts at its first annotation (`ast.rs`, `skip_annotations`), so a `fn` with
`@doc` has a `FunctionDef::span` starting at the `@`. `fn_introspect` and
source tools that look for the `fn` keyword at the span start must use
`skip_annotations`, as `let` tools already do.

### Migration of existing code

The language change is purely additive. `@doc` before a declaration is a
parse error today, so no existing program contains it, and `doc` is not
reserved. Moving documentation over is mechanical with review:

- comment block → `@doc` is unnecessary given the fallback;
- `Builtins.md` sections → `@doc` on `core/packages/core/src/*.ptl` can be
  scripted by matching each `` ### `name(…)` `` heading to its declaration,
  then reviewed by hand;
- `ui/docs/reference.md` lines → `@doc` summaries: scriptable the same way.

## Assessment

These perspectives were worked through directly rather than by subagents,
since the change has no run-time semantics.

### Project goals

*Findings.* It advances **introspection** directly: docs become data that
`fn_info(f)`, the LSP and `petal doc` can read. It also advances **ease of
use**, through hover docs (the LSP hover today shows only `(function) name
signature`, `core/src/lsp/server.rs:315-327`) and a complete reference. It
keeps the rule from annotation-syntax.md that whatever follows an `@` never
changes what the program computes. *Opinion:* generic surface, but cheap, and
it fits Petal's habit of making source facts machine-readable (settings,
static values).

### Ease of writing

*Findings.* It is new to learn, but there is direct precedent: Elixir
`@doc """…"""`, and Julia and Python docstrings as string literals. The
interpolation rule is the main trap, and the literal-only error catches it
with a fix. There are now two ways to document a function. *Opinion:* the
fallback keeps the easy case easy. The guide's rule ("comment unless you need
a field") has to be stated clearly, or people will split arbitrarily.

### Bug-freedom

*Findings.* It prevents stale parameter docs (`doc-params`) and docs that drift
from the code. That drift is real today: `Builtins.md` repeats every signature
by hand, and only names are tested, never prose (`tests/reference.rs` for
`ui`). It adds the possibility of a doc that's wrong but well-formed, which
nothing can prevent. It can't change behaviour: the `ir-equal` invariant
holds, and the differential oracle and golden corpus are unaffected.

### Performance

*Measured.* Parsing 78 KB of raw-string docs (all of `Builtins.md`, in 196
chunks) next to 196 functions takes under 10 ms, below `time`'s resolution and
the same as without them. The bundled `core` package is parsed once per
process (`native_decl::core_decls`, a `OnceLock`). Run time costs nothing,
since there are no terms. The binary grows by the embedded text, about 120 KB
if all of `Builtins.md` and the `ui` reference move in, which matters a little
for the WASM build. Mitigation: strip `@doc` from the embedded sources at
build time, if it matters.

### Live coding and hot reload

No effect beyond line shifts, which comments already cause (see pair 6).

### Introspection, debugging, program manipulation

*Findings.* Provenance, `explain`, slicing, time travel, DM and autodiff work
on terms, and `@doc` emits none. `propose-edit` and source preservation
need the annotation kept with its declaration. The CST already hoists
annotation nodes into the declaration's node for `let`, and the same must be
done for `fn`/`class`/`enum`. Bonus: `fn_info(f).doc` can be added for free,
because `fn_introspect` already re-parses the function's file.

### Maintainability of the implementation

| Stage | Change | Size |
|---|---|---|
| Lexer | none (`Token::At` exists) | — |
| Parser (`parse.rs`, CST + `cst_project.rs`) | accept `fn`/`native fn`/`class`/`enum` after `@doc`; split compile-time from value annotations; literal check; field table | ~250 |
| AST | `Stmt::doc` + `Doc` struct; ~24 `Stmt {…}` literals | ~80 |
| Compiler / IR / bytecode / VM | none | — |
| Lint | `doc-params`, `undocumented-pub` | ~150 |
| LSP | hover and completion show the doc | ~60 |
| `fmt` | lay out `@doc` args; keep raw strings verbatim | ~50 |
| tree-sitter | let `annotation` precede `function_declaration`, `class`, `enum` | ~15 + regen |
| CLI | `petal doc [--json] <package\|file>` | ~350 |
| `fn_introspect` | `fn_info(f).doc` (optional) | ~30 |
| Docs | language guide (Annotations), CLI.md, settings.md | — |

It adds one concept, "compile-time annotation", that F1 already anticipated.
The new invariant: **a compile-time annotation never reaches the IR**,
enforced by an `ir-equal` test over every fixture with `@doc` stripped.

### Domains

- Graphics/creative coding, UI apps, games: the main beneficiaries are the
  libraries they import (`ui`, `bloom`, Cheesecake's
  `engine/script/packages/cheesecake/*.ptl`, which already comments its
  `pub fn`s, e.g. `box3d.ptl:213`).
- Editor scripting (Garden): hover docs in the editor; settings labels.
- Config files and embedding hosts: `static_bindings` / `pb_source_bindings_json`
  gain a `doc` field. That's additive for C and Rust hosts.
- Dashboards, web: neutral.

### Compatibility and migration

Nothing breaks. Downstream projects (`CLAUDE.local.md`):

- **cheesecake** vendors copies of `bloom` and `petal-ui`. A re-vendor that
  brings `@doc` needs a Petal from the same revision, which is already how
  vendoring works.
- **petal-lang.org** has its own syntax highlighter (commit `45913494`), which
  needs `@doc` highlighted.
- **garden** uses the tree-sitter grammar, which needs the update in the table.
- **petal-creative** and **worlds-fair**: no change required; they can adopt
  it whenever they like.

A package that uses `@doc` can't be loaded by an older Petal. That is the
normal rule for new syntax, and the package's `petal` version requirement
covers it.

### Prior art

- **Elixir `@doc` / `@moduledoc`** is the closest: an attribute holding a
  heredoc, stored in the compiled module, read by `h/1` in the REPL and by
  ExDoc. It supports `@doc false` (our `hidden`) and metadata (`@doc since:`).
  Lesson: keep the module-level form distinct.
- **Python and Julia** docstrings are string literals that tools and the
  runtime read. In Python, parameter structure was left to conventions
  (Google/NumPy/Sphinx styles) that tools parse inconsistently. That is the
  argument for structured fields from day one.
- **Rust `///`, JSDoc, KDoc, C# XML comments** are comment-based, with tags
  inside the comment. Rust lowers `///` to `#[doc = "…"]`, so it is an
  annotation underneath. `rustdoc` warns on broken intra-doc links, which is
  our `see:` lint. JSDoc `@param` names go stale because nothing checks them.
- **Doctests** (Rust, Elixir, Python) run examples. That's a natural follow-up
  for `example:` (`petal test --doc`), not part of v1.

## Alternatives considered

1. **Do nothing; the extractor reads comments.** This is the cheapest option,
   and we're adopting it anyway as the fallback. But with only comments there
   are no checked params, no structured options, no hiding, and every
   structured need becomes an unchecked tag mini-language inside comments.
   Native-effect-annotations.md rejected that already (its alternative 3).
2. **A dedicated doc-comment marker** (`##` or `#|`, like Rust's `///`). Text
   would be lighter to write, with no quotes or interpolation trap. Structure
   would still need tags inside text. `##` is also ambiguous with existing
   decorative comments. This is the serious rival. If the owner values easy
   writing over structure, choose it and drop `@doc`.
3. **Keep prose in Markdown, keyed by name** (today's `Builtins.md`), with a
   test that every exported name has a section. This keeps declaration files
   short, which matters most for `core/packages/core`, whose 200 natives would
   otherwise carry about 2,400 lines of prose. It can be combined: `@doc` for
   summary and params on the declaration, with long-form guides staying in
   Markdown and linked by `see:`. **Recommended for `core`.**
4. **`doc(…)` as a value annotation like `tunable`** (a call that returns its
   argument). Rejected: it would reserve `doc` (about 250 bindings break), and
   it can't go on a `fn`.
5. **Smallest useful version.** `@doc` on `fn`/`native fn` only, with the
   body, `params` and `hidden`. That covers the reference-docs need. `options`,
   `example`, `see`, `deprecated` and `let`/`class`/`enum` targets can follow.

## Implementation sketch

Order:

1. **`petal doc --json` with comments only.** This is no language change. It
   extends the reference beyond `stdlib.json` to all `pub` items of `std`,
   `ui`, `bloom` and the bundled packages, and gets the website pipeline built
   against real data.
2. **Parser and AST for `@doc`** on `fn`/`native fn`, with the literal check
   and the field table. Tests: parse cases, every error message, an `ir-equal`
   test, and CST round-tripping through `fmt`.
3. **Lints** `doc-params` and `undocumented-pub`.
4. **LSP hover** and `fn_info(f).doc`. Tree-sitter and the website
   highlighter.
5. **Extend the targets** to `class`/`enum`/`let`, plus `static_bindings` doc
   for settings.
6. **Content:** script `Builtins.md` summaries and params into `core`'s
   declarations, and `ui/docs/reference.md` into the `ui` prelude.
   `extract-stdlib.ts` then merges `petal doc --json` with its Rust-only facts
   (registration group, aliases, source line).

Steps 1–4 are the 1–1.5k lines estimated above.

## Open questions

For the owner to decide:

1. **`@doc` or a doc-comment marker** (alternative 2)? This turns on how much
   structured, checked docs are worth compared with easier writing.
2. **How much prose goes in `core`'s declarations?** All of it, or summary and
   params only, with `Builtins.md` kept as the long-form guide (alternative 3)?
3. **Comment fallback: permanent, or a migration aid** that
   `undocumented-pub` eventually retires?
4. **Module docs:** `petal.toml` and README only, or a later `@moduledoc`-style
   form?
5. **Fixed field vocabulary or open `extra:`?** Hosts like Cheesecake might
   want their own fields (e.g. `editor_category`).
6. **Should `example:` be run** (doctests) from the start, or later?
