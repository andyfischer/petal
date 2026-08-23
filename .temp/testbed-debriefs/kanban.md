# 24 Kanban board

**Status:** complete
**Viewport:** 1280x850 (panel pane 1268x778)
**What works:** Four WIP-limited lanes, twelve seeded cards. Full mouse
drag-and-drop — press-and-travel arms the drag, the source column closes up,
the target column outlines and opens a sized "drop here" gap under the
pointer, and release drops the card at that index (same column, other column,
or an empty column). Click to select, click bare well space to deselect,
hover highlight, per-column pixel scrolling with a thumb and soft clipped-edge
fades. Keyboard: arrows select (with scroll-into-view), shift+arrows reorder
and move across lanes, `1`–`4` send to a lane, `p` cycles priority, `esc`
deselects. Header progress bar and lane counts (red over the WIP limit)
update live. Every control listed in the README was exercised through
`/mouse` and `/key` with `status_error` and `panes[0].panel.error` checked
after each step; both stayed null.
**What I could not do:** Nothing I set out to do. No text entry (no card
creation/rename) — deliberate scope, not a limitation.

## Blockers

None.

## Issues

### 1. A panel that fails to compile keeps running the old program, silently

This cost me the most time by far, and it is a trap laid by AUTHORING.md
itself. The guide says to check `/state` for `status_error`. A panel script
whose *hot reload* fails to compile does **not** set `status_error` — it sets
`panes[0].panel.error`, and the pane goes on happily rendering the last good
program at ~60fps with no visual sign anything is wrong.

```bash
$ printf '\nlet zz = 1\n   || true\n' >> app.ptl        # deliberate parse error
$ curl -s localhost:$PORT/state | jq -c '{status_error, panel_error: .panes[0].panel.error}'
{"status_error":null,"panel_error":"Unexpected token: '||' [line 554, column 4]"}
```

I hit this for real: I added a `let arrowed = …` binding, the file did not
compile, and `/state` showed `status_error: null` plus *plausible-looking*
`panel.values` from the previous build. I spent several minutes reasoning
about an escape-key logic bug that did not exist. The tell, in hindsight, was
that `arrowed` was **absent** from `panel.values` while the code path that
reads it had clearly run — i.e. the values were from a different program.

Two fixes, either would do: report a panel compile error in `status_error`
(it is an editor-level failure), or change the AUTHORING.md checklist to say
"check `panes[].panel.error`, not just `status_error`". The current wording
actively points agents at the wrong field.

Workaround I adopted: run `petal check app.ptl` before every save (it *does*
catch this, exit 1), and poll `.panes[0].panel.error` instead of
`.status_error`.

### 2. A binary operator may not start a continuation line

```petal
let x = a
        || b
// Error: Unexpected token: '||' [line 2, column 9]
```

So the idiomatic way to format a long boolean — operators leading each
continuation line — is a parse error, and there is no line-continuation
escape. Everything has to go on one physical line:

```petal
let arrowed = key_pressed("up") || key_pressed("down") || key_pressed("left") || key_pressed("right")
```

That is a 101-column line in a codebase that otherwise reads nicely at 80.
Trailing-operator style (`a ||` then newline) presumably works, but that is
the less common convention and I did not want to guess again. Combined with
issue 1 the failure mode is nasty: in a *panel* the parse error is invisible.

### 3. `ui.ptl`'s own `context_menu` passes a float alpha, which truncates to 0

`petal-ui/prelude/ui.ptl:918`:

```petal
draw_rect({x: r.x + 2, y: r.y + 3, w: r.w, h: r.h}, #000000, 0.35)
```

The alpha argument is decoded by `opt_u8` in `petal-ui/src/draw.rs` via
`num_as_i64`, so `0.35` becomes `0` — the context menu's drop shadow, which
the prelude's own comment explains at length ("without it a dark menu over a
dark panel reads as part of the background"), is fully transparent. I only
noticed because I copied the idiom for my dragged-card shadow, got nothing,
and went to read the decoder. Either scale `0.0–1.0` floats to `0–255` at the
boundary, or fix the prelude to `89`.

### 4. `text_width` returns an int, so per-character division overestimates

`text_width("M", 14)` returns `8`; the real JetBrains Mono advance at 14px is
~8.4. Computing `chars_per_line = avail_px / text_width("M", size)` therefore
overruns the box by ~5% — my first card titles ran flush into the right
padding. The fix is easy once you see it, but it is not obvious:

```petal
let CHW10 = max(50, text_width("MMMMMMMMMM", TITLE_SIZE))
let LINE_CHARS = max(8, CARD_IN * 10 / CHW10)
```

A float-returning measure, or a `chars_fitting(s, avail_px, size)` prelude
helper, would remove the trap. It matters because `wrap`/`preview`/`truncate_*`
are all **character**-counted while everything they feed is pixel-measured, so
every caller has to bridge the two by hand.

### 5. Injected mouse coordinates are window coordinates; the panel reads pane-local

`POST /mouse {"x":178,"y":384}` arrived in the script as `(172, 346)` — the
pane rect is offset `(6, 38)` inside the window. Both docs are individually
correct (debug-server.md: "window-relative"; petal-graphical-panels.md:
"(0,0) is the pane's top-left") but neither warns you that driving a panel by
coordinates read off a screenshot needs the pane origin subtracted. It shows
up immediately as "my click hit the card above the one I aimed at". Worth one
sentence in AUTHORING.md: get the origin from `/state`'s `panes[0].rect`.

### 6. `panel.values` last-write-wins hides per-iteration values

Documented, but it bites in exactly the place you want observability. My
whole draw loop is `for c in range(0, 4)`, so `content`, `surf`, `cnt` and
friends all report column 3's value only, and I could not read column 0's
scroll extent without hoisting it into a list. Building the values I wanted
to assert into a list comprehension first (`let tops = for c in … end`) is the
workaround, and it is a fine one — but it means "make it observable" is a
design constraint on the code, not a free property of it.

### 7. No drag primitive in the prelude

Drag/drop is the single most fiddly interaction in an immediate-mode UI and
every panel that wants it reimplements the same five things: the press offset
into the grabbed item, the movement threshold that distinguishes click from
drag, the target container hit test, the insertion index from the pointer's
center, and the ghost/placeholder. That is ~60 lines of my app. The pieces
petal-ui *does* give (`drag_active()`, `drag_start_x/y`) cover none of it.

## Praise

- **`panel.values` is the feature.** Reading the whole board back as
  `[[.panes[0].panel.values.cols[]|[.[].id]]]` and asserting "c2 moved from
  lane 0 to lane 2 index 2" made drag/drop testable without touching a pixel.
  It turned a normally-untestable interaction into a one-line shell assertion.
  `P.tcol` / `P.tidx` / `P.lifting` fell out for free because they are just
  named `let`s in a plan function.
- **`/mouse` down/move/move/up** models a real drag faithfully — the panel saw
  a coherent press → hold → release across frames with no special casing, and
  `/screenshot` settling meant I could look at the mid-drag frame directly.
- **`preview`, `fit_parts`, `ellipsize`, `ensure_visible_px`** were each
  exactly the shape I needed, and `fit_parts`' "shed the rightmost segments"
  behaviour is a genuinely good answer for a hint bar.
- **Hot reload preserving `state`** made palette and spacing iteration
  instant — I could nudge a fade and re-screenshot in one command with the
  board still in the position I had dragged it into.
- **`text_width` measuring the real face** meant `draw_text_right` /
  `draw_text_center` were pixel-exact first try; nothing needed a fudge
  factor.
- Losing bold turned out fine: size + colour + `spacing: 1` on the small-caps
  lane headers reads as a real hierarchy. The docs warn about this up front,
  which saved me from discovering it the hard way.
- Immutable lists made the board model trivial to reason about — a drop is
  `list_insert(list_remove(src, i), j, card)` and there is no aliasing bug to
  have.

## Feature requests

1. **Surface panel compile errors where agents look** — put them in
   `status_error`, or fix the AUTHORING.md checklist to name
   `panes[].panel.error`. Highest value by far: it is the difference between a
   two-second fix and a ten-minute wild goose chase, and it will hit every
   agent that writes a panel.
2. **Allow a leading binary operator to continue an expression** across a
   newline (`&&`, `||`, `+`, `++`, `|>`). Purely a parser affordance; the
   current rule forces long lines in exactly the code that most wants
   wrapping.
3. **Fix the truncating float alphas in `ui.ptl`** (line 918) and decide the
   contract: either accept `0.0–1.0` and scale, or make a non-integer alpha a
   warning. Right now the prelude models a bug for every reader.
4. **A `drag` widget in petal-ui**: `drag_state()`, `drag_update(ds, item_id,
   r)` returning `{dragging, id, dx, dy, dropped}` plus an
   `insertion_index(rects, y)` helper. Kanban, reorderable lists, timeline
   editors and tab strips all want the same thing.
5. **Pixel-accurate text fitting**: `text_width` as a float, or
   `chars_fitting(s, avail_px, size)`, so the character-counted wrap helpers
   can be driven from a pixel budget without a hand-rolled ten-glyph probe.
6. **Document the pane origin** in AUTHORING.md's "Driving it" section — one
   sentence saying screenshot coordinates need `panes[n].rect` subtracted
   before they become panel-local.
