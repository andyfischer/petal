# 25 Spreadsheet mini-clone (Ledger)

**Status:** complete
**Viewport:** 1240x880 window → **1200x800 pane** (row count derived from pane height)
**What works:** editable 26x8 grid seeded with a quarterly SaaS plan; a
recursive-descent formula engine (`+ - * / ^`, unary minus, parens,
comparisons, string literals, A1 refs, `B2:E4` ranges, `SUM/AVG/MIN/MAX/COUNT/
ROUND/ABS/IF`, propagating `#DIV/0!` `#VALUE!` `#NAME?` `#REF!` `#SYNTAX`
`#CYCLE`); keyboard-first editing (type-to-replace, ⏎/⇥ commit-and-advance,
esc cancel, shift+arrows range, delete clears a range, `esc z` undo) with full
mouse parity (click, drag-sweep, double-click to edit, header click, wheel);
formula-bar editing; a right rail with a cell inspector, a Q1–Q4 bar profile of
the selected row, and sum/avg/min/max of a range selection; a status bar with
filled/formula/edit counts and an error tally. No `status_error` at any point
across the interaction script I ran.
**What I could not do:** nothing I set out to do. Undo had to become a two-key
chord (see Issues) and there is no dependency graph — a reference re-evaluates
its target, bounded by a depth budget — which is fine at 208 cells but would not
be at 20 000.

## Blockers

None.

## Issues

**1. `fn` has no forward reference — mutual recursion needs a `var` trampoline.**
A `fn` is bound when its declaration *runs*, so a call to a function declared
further down the file reaches `nil`:

```petal
fn a(n)  if n <= 0 then 0 else b(n - 1) end end
fn b(n)  if n <= 0 then 1 else a(n - 1) + 1 end end
print(a(5))
// Error: Cannot call nil [line 1, column 25]
//   in a() [line 3, column 7]
```

A recursive-descent parser is mutually recursive by construction (primary →
expression), and a spreadsheet adds a second back-edge (a cell reference
re-enters the evaluator). The workaround is a `var` box filled in after the
declarations, which works because a closure captures the box:

```petal
var _expr = nil
fn parse_primary(cx, toks, i) … _expr(cx, toks, i + 1) … end
fn parse_cmp(cx, toks, i) … end
set _expr = parse_cmp
```

It works, but it costs the dataflow story (the guide's own warning about `var`)
and it is not discoverable — the error says "Cannot call nil", which does not
suggest "your function is declared below this one". Either hoist top-level `fn`
declarations, or say so in the error: "`b` is not declared yet at this point;
top-level functions are bound in order".

**2. A bare `for` in implicit-return position is not captured.**
The guide says a `for` in value position — "assigned to a name, `return`ed,
passed as an argument" — collects a list. A function's *implicit return* is not
in that list, and silently yields `nil`:

```petal
fn evaluate(cells)
  for r in range(0, len(cells)) do …row… end     // returns nil
end
fn evaluate(cells)
  let grid = for r in range(0, len(cells)) do …row… end
  grid                                            // returns the list
end
```

The failure surfaced far away as `Cannot get length of nil`. Either make the
implicit return a capture position (it reads exactly like one) or reject it.

**3. `float("3.5")` and `int("7")` reject strings.**
`Error: Expected float at arg 1, got string`. Parsing user-typed numbers is the
single most obvious thing a spreadsheet does, and I had to hand-roll a digit
scanner (~30 lines) to tell `"41200"` from `"Q1"`. A `parse_float(s) -> float?`
/ `to_num(s)` builtin — anything that reports failure rather than aborting —
would remove that from every app that reads text.

**4. Alpha is `u8` 0..255, but the prelude passes `0.35`.**
`petal-ui`'s own `context_menu` draws its drop shadow with
`draw_rect({…}, #000000, 0.35)`. The decoder is `opt_u8`, so `0.35` truncates to
`0` — the shadow the comment describes ("without it a dark menu over a dark
panel reads as part of the background") is fully transparent. Either accept a
float 0..1 or fix the prelude.

**5. Alpha composites in linear space, so low alpha over a dark ground is much
brighter than the number suggests.** `draw_rect(cell, #ffffff, 10)` — 4% white —
rendered around `#383838` on a `#12161e` background, a very visible grey block.
I ended up using flat colours for anything subtle. Worth one line in the panels
doc; it makes "a: 10" unusable as a hover tint.

**6. Text always draws above meshes, regardless of emission order.**
Every filled shape becomes a `Mesh` primitive and every string a `Text`
primitive, and the renderer appears to put all text on top. So a rect drawn
*after* a text run does not cover it: my in-place cell editor painted its own
background over the committed cell text and the two overlapped. The fix is to
not draw the text in the first place, which is fine, but it means "draw an
opaque panel over what is already there" — a completely ordinary
immediate-mode move — silently does not work for text. This should be in
`petal-graphical-panels.md` under the draw surface table.

**7. `/scene` only reports text runs for a panel.** Because every fill is a
`Mesh`, `/scene` carries no rectangles at all for a panel pane, so
"assert layout numerically" is text-only. That is the one thing AUTHORING.md
recommends `/scene` for. Emitting mesh bounding boxes (even just as
`{"type":"mesh","rect":…}`) would make panel layout assertable.

**8. No alt modifier, and Cmd/Ctrl chords never reach a panel.**
`classify_panel_key` swallows every Cmd/Ctrl chord except `Ctrl+S`, and the
app-level `Mods` struct has no `alt` field at all — so `mod_alt()` is
permanently false in Garden and `POST /key {"mods":["alt"]}` is a no-op. For a
panel whose bare letters are *content* (a spreadsheet, a text editor, a game
console), that leaves literally no keyboard namespace for commands. I shipped
`esc`-then-`z` for undo. A forwarded `alt` bit, or an opt-in "this panel wants
Cmd chords" flag, would fix a whole class of app.

**9. Small things.**
- The pane is not the size you asked for: `GARDEN_HEADLESS_SIZE=1200x800` gives
  a 1188x728 pane (−12, −72). Fine once you know; I derive the row count from
  `screen_height()` now. Worth a sentence in AUTHORING.md.
- `panel.values` reports *every* top-level binding including all my colour
  constants, which buries the ~15 names a test cares about. A convention (a
  `debug_*` prefix, or filtering `let`s that never change) would help.
- `state` survives hot reload, which is correct and was occasionally confusing
  while iterating: my sheet kept the edits I had injected across a file save, so
  a "fresh" screenshot needed a relaunch.

## Praise

- **`panel.values` is genuinely excellent.** Every assertion in my test loop was
  against a name I had already written for other reasons — `sel_r`, `addr`,
  `editing`, `buf`, `n_err`, `multi`. No instrumentation, no test hooks. It
  turned "does shift+arrow extend the range" into one `curl | jq`.
- **`text_width` measuring the real font** makes right-alignment and
  `ellipsize` exact, which is the entire game for a numeric grid. `ellipsize` /
  `ellipsize_tail` / `preview` / `fit_parts` are exactly the right helpers and I
  used all four.
- The `Rect` class with `center_x()` etc. reads well, and the record `draw_*`
  overloads plus the style-record `draw_text` make a layout pass compact.
- Records as an ad-hoc tagged union (`{k, n, s}`) with `if v.k == …` is
  pleasant, and immutable records meant I could snapshot the whole sheet for
  undo with `append(undo, cells)` and not think about aliasing once.
- Input-then-`/screenshot` needing no sleep made the iteration loop fast.

## Feature requests

1. **Hoist top-level `fn` declarations** (or a much clearer error). This is the
   single biggest friction: any tree-walking / parsing code hits it immediately.
2. **A string→number builtin that can fail** (`parse_float`, `to_num`).
3. **Forward an `alt` modifier to panels** — panels whose letters are content
   have no command keyspace at all today.
4. **Document the two rendering surprises**: linear-space alpha compositing, and
   text always painting above meshes.
5. **Make `for` in implicit-return position collect**, or make it an error.
6. **Mesh bounds in `/scene`**, so panel layout can be asserted numerically.
7. Nice-to-have: `index_of(haystack, needle)` for strings — I wrote
   `digit_val`/`letter_val` as 10- and 26-iteration scans because `contains`
   only answers yes/no.
