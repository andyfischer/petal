# 25 — Ledger, a spreadsheet mini-clone

A working spreadsheet drawn entirely by Petal inside a Garden panel: an editable
26 × 8 grid seeded with a quarterly SaaS plan, a real formula language with
A1 references and ranges, keyboard-first editing, and a live cell inspector.

## Running it

```bash
cd examples/testbed/25-spreadsheet
GARDEN_HEADLESS_SIZE=1240x880 \
  /Users/andy/petal/garden/target/debug/garden \
  --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

**Viewport:** designed for a **1200 × 800 pane**, which is what
`GARDEN_HEADLESS_SIZE=1240x880` produces (Garden's chrome eats 12 px of width
and 72 px of height). The row count is derived from the pane height rather than
fixed, so it degrades sensibly at other sizes — below ~700 px tall it just shows
fewer rows and scrolls.

## Controls

### Keyboard

| Key | Does |
|---|---|
| arrows | move the active cell |
| shift + arrows | extend the selection into a range |
| tab / shift+tab | move (or commit and move) one column |
| home / end | first / last column |
| pageup / pagedown | page the selection |
| any printable character | start editing, replacing the cell |
| `=` | start a formula |
| return | edit the cell; while editing, commit and move down |
| shift+return | commit and move up |
| escape | cancel the edit |
| backspace / delete | clear the cell, or the whole selected range |
| **esc then z** | undo (see below) |

Undo is a two-key chord because a Garden panel cannot observe any other kind:
Cmd- and Ctrl-chords are swallowed by the host before the script runs, the host's
`Mods` has no alt bit, and a bare letter already means "start typing here".
`esc` arms (the mode chip reads `UNDO?`); the next keystroke either takes the
undo or disarms. The `undo n` pill in the masthead is the mouse equivalent.

### Mouse

- click a cell to select it, drag to sweep out a range
- double-click a cell, or click the formula bar, to edit
- click a column letter or a row number to jump the selection there
- wheel over the grid scrolls it

## Formulas

A cell whose text starts with `=` is a formula. The engine supports:

- `+ - * / ^`, unary minus, parentheses
- comparisons `> < >= <= <> =`, yielding 1 / 0
- string literals: `=IF(E5>D5,"expanding","flat")`
- A1 references (`B7`, `AA12`) and ranges as function arguments (`B2:E4`)
- `SUM` `AVG`/`AVERAGE` `MIN` `MAX` `COUNT` `ROUND(x,d)` `ABS` `IF(c,a,b)`
- errors that propagate and are named: `#DIV/0!` `#VALUE!` `#NAME?` `#REF!`
  `#SYNTAX` `#CYCLE`

Blank cells read as 0 in arithmetic; text does not, so `=A1+1` on a label is
`#VALUE!`. There is no dependency graph — a reference simply re-evaluates its
target, with a depth budget standing in for cycle detection, so `=B2` in `B6`
plus `=B6` in `B2` paints `#CYCLE` instead of hanging the frame.

Formula cells are drawn in a lighter mint and carry a small corner flag;
literals are plain, negatives are red, errors get a tinted cell.

## What it exercises

**Language:** recursive-descent parsing in Petal (mutual recursion routed
through `var` boxes, because a `fn` is only bound when its declaration runs);
nested-list index assignment (`cells[r][c] = buf`); `for` in value position as a
map; records as a tagged-union value type; `state` across frames and hot
reloads; closures over top-level bindings; string scanning with byte-indexed
`slice`.

**Host:** every `draw_*` primitive except images; styled text runs with
`spacing` and `italic`; `text_width`-exact centering, right-alignment and
`ellipsize`; the `ui` prelude's `rect`/`hovered`/`point_in`/`preview`/
`fit_parts`/`ensure_visible`/`draw_scrollbar`; `text_input()`, `key_pressed`,
`mod_shift()`, `click_count()`, `drag_active()`, `scroll_y()`; and
`panel.values` as the assertion surface — `sel_r`, `sel_c`, `addr`, `editing`,
`buf`, `scroll`, `rev`, `edits`, `n_err`, `selr0..selc1` and `multi` are all
readable from `GET /state`.

Nothing animates, so the 10-second panel sleep is invisible here: the sheet
redraws on input and then holds its last frame.
