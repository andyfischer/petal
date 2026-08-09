# 13 Calculator

**Status:** complete
**Viewport:** 880x680
**What works:** Full infix expression calculator in pure Petal — tokenizer →
shunting-yard → RPN evaluator, run live every frame so the readout previews the
answer while you type. Keypad (25 keys) and keyboard both drive it; unary minus,
parens, precedence, modulo, named errors (`divide by zero`, `unclosed (`,
`unbalanced )`, `malformed number`, `incomplete`). Memory store with a clickable
chip, a scrolling tape of results you can click to reuse, hover states, a
`dt()`-driven key-press ring, number formatting with thousands grouping.
Everything is assertable through `panel.values` (`expr`, `live`, `ans`,
`src_line`, `memory`, `mem_set`, `tape`, `tape_scroll`, `presses`).
**What I could not do:** Nothing I set out to do. Three language limits forced
rewrites (below), and one host rendering bug forced a design constraint.

## Blockers

None that stopped the app. The one that came closest is the glyph-atlas bug,
which is a *host* bug and which I could only work around by narrowing the
design.

### Garden headless renderer mis-rasterizes new glyphs when a frame mixes many font sizes

Not a Petal bug, but it dominated the session — it silently destroyed the app's
typography and cost the most time, because `/scene` reports the run perfectly
while `/screenshot` renders it wrong.

Reproduction (`_probe.ptl`, run headless at 400x520):

```petal
state n = 0
if key_pressed("space") then n = n + 1 end
clear(10,12,16)
let digits = "0123456789"
let c = slice(digits, n % 10, n % 10 + 1)
let y = 24
for s in [16, 20, 24, 28, 32, 36, 40, 48] do
  draw_text(c ++ c ++ c ++ " " ++ str(s), 10, y, s, 240, 170, 90)
  y = y + s + 14
end
```

- First `/screenshot`: all eight sizes render correctly.
- `POST /key {"key":"space"}` three times, then `/screenshot`: the glyph `3`,
  new to the atlas, renders at roughly 16 px in *every* run except the 48 px
  one, while keeping each run's correct advances — so "333" at 40 px comes out
  as three tiny digits spread over 72 px. Repeat screenshots do not converge; it
  is stable, not a settling artifact.
- `GET /scene` reports the run correctly (`{"text":"333","size":40.0}`), so only
  the raster is wrong.
- With **one** size in the frame: correct. With **two** (14 and 40): correct.
  With **four** (11/15/22/40): correct. With eight: broken.
- Pre-warming the glyph set at every size — either with `a = 0` or off-pane at
  `x = -600` every frame — made it *worse*, producing a different wrong size per
  run.

Workaround: a strict four-step type scale (11 / 15 / 22 / 40). That is also
better typography, so the app is not worse for it — but an app that legitimately
wants six sizes has no way out.

## Issues

### 1. `float("3.5")` rejects strings, `int("42")` accepts them

```petal
print(int("42"))     // 42
print(float("3.5"))  // Error: Expected float at arg 1, got string
```

For a calculator this is the central primitive, and its absence means every
decimal literal is assembled by hand:

```petal
fn parse_num(s)
  let parts = split(s, ".")
  let v = if len(parts[0]) > 0 then float(int(parts[0])) else 0.0 end
  if len(parts) > 1 && len(parts[1]) > 0 then
    let fs = parts[1]
    let scale = 1.0
    for i in range(0, len(fs)) do scale = scale * 10.0 end
    v = v + float(int(fs)) / scale
  end
  v
end
```

The asymmetry is the surprising part: one cast parses, the sibling does not.

### 2. No forward references between functions — mutual recursion is impossible

```petal
fn a(n)
  if n <= 0 then 0 else b(n - 1) end
end
fn b(n)
  1 + a(n)
end
print(a(5))     // Error: Cannot call nil  [at `b(n-1)`]
```

`b` is nil *inside `a`'s body* even though by the time `a` actually runs, `b`
has been declared. The natural shape for an expression parser is
`expr → term → primary → expr` (parens), and that cannot be written. I replaced
recursive descent with an explicit shunting-yard pass over an operator stack.
That is a fine algorithm, but the language chose it, not me. The docs say
"nothing hoists", which explains a *top-level call* above a declaration; it
does not prepare you for a call from inside a function body that only executes
later.

### 3. `append` only appends, and prepending is not obvious

```petal
tape = append([{e: expr, v: out}], tape)   // silently appends the LIST as one element
```

No error at the point of the mistake — the corrupt list only surfaced two
functions away as `Cannot access field 'e' on list`. Worse, once a panel frame
raises, **the panel latches the error and stops running even after the script is
fixed on disk** (see issue 5), so the failure looked like "clicks stopped
working". `flat([[x], xs])` is the working prepend; a `prepend`/`concat` builtin,
or an arity-2 `append` that errors on a list argument, would have saved this.

### 4. Division by zero is a hard abort, not a value

```petal
print(1/0)     // Error: Division by zero — kills the frame
```

Reasonable, but it means an *evaluator* written in Petal has to guard every
divide before performing it, which the shunting-yard evaluator now does twice
(`/` and `%`). A `nan`/`inf` result, or a `try`-style form, would let a script
propagate the failure as data instead of pre-checking.

### 5. A panel that raises once never recovers, even after the file is fixed

After the `append` bug above, `panel.error` kept reporting the *old* source line
and the panel stopped re-running, through repeated edits to `app.ptl`. Only a
full Garden restart cleared it. Since `/state`'s top-level `status_error` stays
`null` in this case, the panel looks healthy from the outside — `panel.error` is
the field to watch, and it is easy to miss. It also means the documented hot
reload cannot be relied on during the exact situation it is most wanted.

### 6. Small things

- No exponent literals: `1.0e20` is a parse error ("Expected ',' between
  arguments"), so large constants are written out in full
  (`1000000000000000.0`).
- `draw_scrollbar` from the prelude paints in `theme.outline` / `theme.dim`,
  which are light-grey and clash with any app that has its own palette. It takes
  no color argument, so I wrote my own six-line replacement. The same is true of
  `button` (style record helps) and `section_label`.
- The debug server's `/mouse` takes **window** coordinates while `panel.values`
  and `/scene` report **pane-local** ones (offset here by (6, 38)). Not
  documented in AUTHORING.md; I lost a round of clicks to it.
- `text_input()` returns the whole typed string; there is no "the character just
  typed", so a script that maps characters to actions has to take
  `slice(t, len(t)-1, len(t))` and hope one keystroke arrived per frame.

## Praise

- **`panel.values` is genuinely excellent.** `let live = eval_str(expr)` and the
  evaluator's whole `{ok, v, err}` record is readable over HTTP with no
  instrumentation at all. I asserted every interaction against names, never
  pixels, and the one time the pixels lied (the atlas bug) `/scene` told me the
  logic was right.
- The record overloads in the prelude (`draw_rect(r, c)`,
  `draw_text(s, pos, style)`) plus the `Rect` class make layout code read like
  layout, not like arithmetic. `ellipsize` / `ellipsize_tail` / `fit_parts` are
  exactly the right primitives and their doc comments explain *why* they are
  shaped that way, which is rare.
- `text_width` measuring the real font is what makes right-aligned numerics
  possible at all; the size-fallback for an over-wide readout is three lines
  because of it.
- `state` surviving hot reload, and being visible under its own name, is a very
  good pairing.
- Error messages carry a caret, the source line, *and* a provenance chain
  ("Caused by: tape … idx … tape_scroll"). That chain is how I found the
  `append` bug.

## Feature requests

1. **`float("3.5")` should parse a string**, like `int` does. (Highest value per
   line of implementation of anything here.)
2. **Fix the headless glyph atlas** for new glyphs across mixed font sizes, or
   document the ceiling. A drawing host whose screenshots are wrong is a hard
   problem for every agent working this way.
3. **Let a panel recover from a script error on reload** instead of latching it,
   and surface `panel.error` in `status_error` (or in AUTHORING.md's checklist)
   so it is not missed.
4. **Allow forward references between top-level functions**, or add an explicit
   forward-declaration form. Mutual recursion is table stakes for parsers,
   tree-walkers and state machines.
5. **`prepend(xs, x)` / `concat(a, b)`** builtins, and an arity check that makes
   `append(list, list)` at least suspicious.
6. **A non-aborting divide** (`safe_div`, or `nan` semantics) so evaluators can
   treat arithmetic failure as data.
7. Prelude widgets should take a palette/style record (like `button` does) —
   `draw_scrollbar(r, count, rows, scroll, style)`.
8. Document the window-vs-pane coordinate offset for `/mouse` in AUTHORING.md.
