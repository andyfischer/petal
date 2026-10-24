# Drag mode examples

Sketches for Petal IDE's drag mode: drag a shape on the canvas and Petal
rewrites the numbers that placed it, through any number of function calls.
Each file's header says what to drag and what to expect. They are adapted from
the engine's examples ladder in
[`examples/direct-manipulation-v2/`](../../../../examples/direct-manipulation-v2/)
(which the study harness runs) and use plain RGB colors and `clear(...)`, so the
background is not itself a grabbable shape.

## Opening one

From the `garden/` directory:

```bash
garden petal-ide examples/panels/dm-v2/01-helper-row.ptl
```

Then turn on drag mode: the **Drag** button in the toolbar, **Cmd+Shift+D**
(Ctrl+Shift+D off macOS), or `:DragMode`. The canvas shows a `DRAG MODE`
badge, the shape under the pointer gets an outline and a label naming the call
that drew it, and the pointer turns into a grab hand. The sketch's own mouse
input is suspended until you leave the mode; Cmd-drag works without it.

Edits go into the editor's buffer, not the file. Undo (`u`, Cmd+Z) puts a drag
or a hint toggle back in one step; nothing is written to disk until you save.

## What to try in each

| File | What it exercises |
|---|---|
| `01-helper-row.ptl` | cards placed by a helper: drag the last card (`gap` vs `x0`, an ambiguity the result panel lists), the first card (`x0` only) |
| `02-clock-and-orbit.ptl` | trigonometry: a clock hand's tip (`hours` around the dial, `len` outward) and a moon orbiting by `time()` |
| `03-fractal-tree.ptl` | recursion five calls deep: drag a leaf; mark the root `rigid` by Alt-clicking it and the tree opens instead |
| `04-flex-row.ptl` | a menu bar laid out from `text_width` and padding by a recursive helper |
| `05-projectile.ptl` | 200 Euler steps with `var`; the physics constants are `rigid(...)`, so drags change the launch |
| `06-bar-chart.ptl` | a bar's top edits its number in the `data` list; the axis rescales with the data |
| `07-ui-card.ptl` | a settings card from the `ui` package's layout helpers: every rectangle is library code, every edit lands in this file |

## The pieces of the UI

- **The result panel** (bottom of the canvas, after a drag): the ranked
  alternatives (click one to write it instead; `:DragNext`, `:DragPrev` and
  `:DragPick N` do the same), whether each was verified by re-running the
  edited code, and how many other shapes it moves. Below them, the numbers
  behind the shape with their state: `free`, `tunable`, `rigid`, `config`
  (a `config let` knob) or `fixed` (not editable, with the reason). The last
  row is the session's `:DragAllow` / `:DragDeny` selectors.
- **Marking numbers by mouse.** In drag mode the drag's candidate numbers, and
  every `tunable(...)` / `rigid(...)` literal, are underlined in their state's
  color, and a square in the gutter marks their lines (a hollow square marks a
  `let` whose literal a click would mark). Click an underlined number, a gutter
  square, or a state chip in the panel to cycle **free > tunable > rigid >
  free**. Alt-click marks any number. The edit is ordinary source
  (`tunable(40)`, `rigid(40)`), so the next drag's solver reads it.
- **Session-only constraints**: the `only` and `never` chips on a candidate's
  row add it to `:DragAllow` / `:DragDeny` for the following drags, with no
  source change; `clear` empties both.

The full user guide is [docs/petal-ide-mode.md](../../../docs/petal-ide-mode.md#drag-mode).
