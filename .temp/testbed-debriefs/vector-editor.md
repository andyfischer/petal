# 34 Vector graphics editor

**Status:** complete
**Viewport:** 1400x940 (panel pane 1388x868)
**What works:** Everything in the README. Selection (click, ⇧-click, marquee,
layer rows, A = select all, Esc); 8 scale handles + a rotation knob computed in the shape's
own rotated frame, so scaling a rotated star behaves correctly; ⇧ aspect lock;
rotation with ⇧ 15° snapping; move with 8 pt grid snapping; drag-scrub on the
six numeric inspector fields; six alignment ops against rotated bounds; full
layering (Back/↓/↑/Front buttons, `[`/`]`/`⇧[`/`⇧]`, right-click menu) plus
per-object lock and visibility; five drag-to-create tools; duplicate, delete,
recolor; 60-step snapshot undo/redo covering every one of those. Verified
end to end against a headless Garden with `/mouse`, `/key`, `/state` and
`/screenshot`.

**What I could not do:** verify the ⌥ (alt) drag modifiers headlessly — see
Issues. Nothing else was cut.

## Blockers

None.

## Issues

**1. `mods` on `POST /mouse` only delivers `shift` to a panel.**
`{"op":"down","x":849,"y":413,"mods":["alt"]}` followed by a `move` leaves
`mod_alt()` false in the script, so the "scale about the center" and "ignore the
snap grid" behaviors are unverifiable from the debug server; `mods:["shift"]`
on the same op *does* reach `mod_shift()`. `mods:["cmd"]` is worse than
unavailable — cmd-click is Garden's own direct-manipulation "jump to the code"
gesture, so a panel can never use it, and `POST /key {"key":"z","mods":["cmd"]}`
does not reach `mod_cmd()` either (Garden's global Undo eats it). So a panel
effectively has only unmodified and ⇧-modified shortcuts; I moved Select All
from ⌘A to plain `A` and dropped ⌘Z/⇧⌘Z after finding they were dead. I designed around cmd (a lock flag on the
backdrop instead of ⌘-drag-to-marquee), but I had to ship the ⌥ paths unproven.

**2. `context_menu` must be called after everything else is *drawn*, and
`docs/petal-graphical-panels.md` says the opposite clearly enough to mislead.**
The doc's sketch puts `menu_blocking` "near the top" and `context_menu` "near
the bottom" — of the *frame*, which I read as "the bottom of the input section".
Calling it there means the menu's background quad is painted before the panel's
own background quads and vanishes completely; you get floating menu text with no
panel behind it. Worth saying explicitly: `context_menu` is a draw call, put it
after your last one.

**3. Even called last, the menu still needs manual text avoidance.**
Garden's renderer draws every text run above every quad, so the panel's labels
show *through* an open menu. The 33-paint app hit this too and both apps now
carry the same workaround: re-derive the prelude's private menu rect
(`_menu_rect` logic, duplicated as `menu_est`) and skip every text band it
covers. That means duplicating `_MENU_ROW_H`, `_MENU_SEP_H`, `_MENU_PAD` and
the flip-when-offscreen rule in app code, and it silently rots if the prelude
changes. Either the renderer should respect draw order for text, or the prelude
should export `menu_rect(m, items)`.

**4. A `for` loop is not captured as a value in implicit-return position.**

```petal
fn world_pts(s)
  for p in unit_pts(s.kind) do [p[0], p[1]] end   // returns nil
end
```

This returns `nil`, and the failure surfaces far away as
`Cannot get length of nil` inside the *caller*. `let out = for ... end  out`
works. The language guide lists the value positions as "assigned to a name,
`return`ed, passed as an argument, or placed as a list element" — a function's
implicit return is conspicuously absent from that list, and it is the position
most likely to be written. `return for ... end` is documented as working, which
makes the implicit form's silence feel like an oversight rather than a rule.

**5. Escaped quotes are not allowed inside a string interpolation hole.**

```petal
let s = "snap {if snap then \"8 pt\" else \"off\" end}"
// Error: Unexpected character '\' [line N, column M]
```

The error is at least precise, but the workaround (hoist the `if` to its own
`let`) is not obvious from the message, and the reported line number was ~380
lines past the end of my file, so the caret was the only usable signal.

**6. Translucent light-on-dark is far stronger than the number suggests.**
Garden linearizes before blending, so `alpha: 16` of a cream over a near-black
navy lands at sRGB ~(54,57,66) — a clearly visible grey disc, not the 6% whisper
the number implies. Correct, but surprising enough that I re-seeded the document
three times before measuring the PNG. A sentence in
`petal-graphical-panels.md` under "Supported draw surface" would save the next
person the round trip.

**7. `state` survives an edit-and-reload, which fights iterating on seed data.**
Expected and documented, but worth flagging as a workflow trap for this exercise
specifically: every change to `seed_doc()` needs a full Garden restart, and for
two rounds I was reading a stale document and adjusting the wrong numbers.

**8. Minor: a plain click silently mutated the document.**
My own bug, but it was created by a reasonable-looking structure: the move-drag
branch ran on the press frame with a zero delta, and the snap-to-grid rounded
the shape by 4 pt with no `moved` gate and therefore no undo entry. Easy to hit;
the giveaway was `panel.values` showing `y: 72` on a shape seeded at `68` before
any edit. `panel.values` is what found it in about a minute.

## Praise

- **`panel.values` is the single best thing about this stack.** Being able to
  `jq '.panes[0].panel.values.doc[4]'` after a synthetic drag and read the exact
  post-transform record turned "does rotation-aware scaling work" from a
  pixel-squinting exercise into an assertion. Issue 8 above was found purely
  from it.
- **Settle-then-capture really does mean input-then-screenshot with no sleep.**
  Not once in ~60 injected gestures did I capture a half-applied frame.
- **`text_width` measuring the same style record `draw_text` takes** is what
  makes the auto-fitting text objects a five-line function instead of a
  calibration problem.
- **Records as values** made undo trivial: `hist = append(hist, doc)` is a real
  snapshot, no cloning ceremony, no aliasing surprises. The whole document model
  is ~40 lines of pure list helpers.
- Error messages carry a source line, a caret and a stack trace with the calling
  frames — that is better than most scripting hosts.

## Feature requests

1. **Deliver all modifiers on `POST /mouse`** (or document that only `shift`
   crosses), so ⌥/⌃ drag behaviors are testable. Highest value: it is the only
   thing in this app I could not verify.
2. **Export `menu_rect(m, items)` from the prelude**, or make Garden's renderer
   honor draw order for text runs. Two testbed apps have now duplicated the same
   ~20-line workaround.
3. **Make a `for` loop in implicit-return position collect**, or make it a
   compile-time warning. Silent `nil` from a loop that visibly produces values
   is the worst of both worlds.
4. **A `rotate(angle)` transform on text draws**, or a documented statement that
   text cannot rotate. Every 2-D editor wants rotated labels; today the honest
   move is to hide the rotation handle for text, which is what this app does.
5. Allow an interpolation hole to contain a string literal (`"{if x then "a"
   else "b" end}"`), or improve the error to name the fix.
6. A `polygon`/`path` fill for concave outlines. `fill_poly` is convex-only, so
   every star in this app is hand-fanned into 10 `fill_triangle` calls; a
   `fill_fan(center, points)` builtin would cover the 95% case.
