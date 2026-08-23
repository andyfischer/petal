# Testbed challenge — collated summary (batches 1–3)

15 of 50 apps built. The run stopped at batch 4 when the account hit its
monthly spend limit; 35 agents died on that error, not on anything technical.

**Built and committed (all self-reported `complete`, all verified against a
live headless Garden):**

| Track | App | Lives at | Debrief |
|---|---|---|---|
| Simple games | 01 Pong | `examples/games/pong/` | [pong.md](pong.md) |
| Simple games | 02 Breakout | `examples/games/breakout/` | [breakout.md](breakout.md) |
| Simple games | 03 Snake | `examples/games/snake/` | [snake.md](snake.md) |
| Everyday UI | 13 Calculator | `examples/productivity/calculator/` | [calculator.md](calculator.md) |
| Everyday UI | 14 Todo | `examples/productivity/todo/` | [todo.md](todo.md) |
| Everyday UI | 15 Notes | `examples/productivity/notes/` | [notes.md](notes.md) |
| Business | 23 CRM contact manager | `examples/productivity/crm-contact-manager/` | [crm-contact-manager.md](crm-contact-manager.md) |
| Business | 24 Kanban | `examples/productivity/kanban/` | [kanban.md](kanban.md) |
| Business | 25 Spreadsheet | `examples/productivity/spreadsheet/` | [spreadsheet.md](spreadsheet.md) |
| Media & creative | 33 Paint | `examples/productivity/paint/` | [paint.md](paint.md) |
| Media & creative | 34 Vector editor | `examples/productivity/vector-editor/` | [vector-editor.md](vector-editor.md) |
| Media & creative | 35 Photo adjust | `examples/productivity/photo-adjust/` | [photo-adjust.md](photo-adjust.md) |
| Dataviz | 41 Analytics dashboard | `examples/dashboards/analytics-dashboard/` | [analytics-dashboard.md](analytics-dashboard.md) |
| Dataviz | 42 Server monitoring | `examples/dashboards/server-monitoring/` | [server-monitoring.md](server-monitoring.md) |
| Dataviz | 43 Finance dashboard | `examples/dashboards/finance-dashboard/` | [finance-dashboard.md](finance-dashboard.md) |

The `NN` numbers are the challenge-run slot ids; the app directories and these
debrief files dropped the numeric prefix when `examples/` was reorganized. The
numbers are still how the notes below cross-reference each app.

Creative/graphical experiments (47–50) were never reached. Remaining: 04, 05,
06, 07, 08, 09, 10, 11, 12, 16–22, 26–32, 36–40, 44–50.

Zero agents reported a hard blocker. Every limitation below was worked around
in Petal, which is itself the headline result: **the language and the panel
runtime were sufficient to build all fifteen apps without touching Rust.**

---

## The five things worth fixing first

Ranked by (independent reports × time cost). The first two are verified in
this repo, not just reported.

### 1. `for` in a function's tail position evaluates to nil — 6 reports

**Verified.** The single most-reported language issue.

```petal
fn f()
  for i in range(0, 3) do
    i * 2
  end
end
print(f())                              // nil

let g = for i in range(0, 3) do i end
print(g)                                // [0, 1, 2]  — works here
```

`for` in value position collects into a list, but not as a function's implicit
return. Every agent that hit this lost time to a helper that silently returned
`nil`, and the failure surfaces far from its cause. Reported by 25, 34, 35, 41,
43, and as a docs gap by 14.

### 2. Garden's glyph atlas corrupts under a mixed type scale — 5 reports, most time lost

The worst issue in the run, named "biggest issue" by four separate agents.
Past roughly 9–10 distinct font sizes in a process's life, text runs rasterize
at a **stale or wrong size** — sometimes per glyph within a single run — while
`GET /scene` continues to report the correct size. So the screenshot lies and
the numeric dump doesn't, which defeats the whole verification loop the guide
prescribes.

From 01 Pong, `/scene` reporting a 36 px banner:

```json
{"pos":[448,341],"size":36.0,"text":"P"}   // A, U, S, E, D all size 36.0
```

Pixels: `P` ~12 px, `A` ~12 px, `U` 36 px, `S`/`E`/`D` ~12 px — correctly
*positioned* on 36 px advances, wrongly rasterized. Which glyphs survived
correlated exactly with which `(glyph, size)` pairs the process had already
cached. A fresh process renders the identical script perfectly, so it is
accumulated process state, not the script.

It bites hardest on hot reload, which is the intended authoring loop. Two
agents worked around it by collapsing to a strict four/five-step type scale
(11/15/22/40) — a real design compromise forced by a renderer bug. Pre-warming
glyphs off-pane made it worse. Related: 33 saw `/screenshot` composite the
*previous* frame's text run on top of a changed one for one frame.

### 3. A script that fails to compile fails silently — 5 reports

A `.ptl` with a syntax error produces **no log line, `status_error: null`**, and
a pane that degrades to `kind: "editor"` with `panel: null`. A syntax error is
therefore indistinguishable from a bad `layout.ptl` or a wrong path. Only
`petal check` reveals it. Two worse variants:

- On *hot reload*, a script that fails to compile keeps silently running the
  **old program** (24) — so you edit, see no change, and conclude your edit
  had no effect.
- A panel that raises once **never recovers**, even after the file is fixed
  (13); the process must be restarted.

Cheapest high-value fix in the list: surface the compile error in
`status_error`.

### 4. `clamp` returns a float, poisoning integer geometry — 5 reports

`clamp(i, 0, len(xs) - 1)` used as an index or a pixel coordinate fails at
runtime, or worse, silently staircases a layout (41 lost a whole card grid to
this). Every use in pixel/index code has to be wrapped in `int(...)`. Reported
by 14, 15, 33, 35, 41.

### 5. Injected mouse coordinates are window coordinates; scripts read pane-local — 4 reports

`POST /mouse` takes window coordinates, but `mouse_x()`/`point_in`/`hovered`
inside the panel see pane-local ones. The offset (the pane rect origin, ~`6,38`
in a default single-pane layout) is undocumented, so every agent independently
discovered it by trial. Reported by 01, 14, 23, 24. This is a documentation fix
in the first instance — my AUTHORING.md is wrong by omission here.

---

## Verified prelude bug

`petal-ui/prelude/ui.ptl:918` — `context_menu`'s drop shadow passes a **float
alpha** into the `u8 0..255` contract:

```petal
draw_rect({x: r.x + 2, y: r.y + 3, w: r.w, h: r.h}, #000000, 0.35)
```

`0.35` truncates to `0`, so the shadow never renders. Three independent
reports (24, 25, 33). One-line fix (`89`).

---

## Renderer / host issues

- **Text is composited above every quad, regardless of draw order** (23, 25,
  33, and as a consequence 24, 34). This makes overlays structurally
  impossible: a context menu cannot cover the text underneath it. Both agents
  who needed menus had to re-derive the menu box in app code and manually
  suppress the text it would overlap — and `_menu_rect` is private, so they
  estimated it. This is the second-most-damaging host issue after the atlas.
- **Cmd/Ctrl chords never reach a panel**, and there is no alt modifier (25,
  34, 35). `ctrl+s` arrives with its character attached, so "save" types an
  `s` into the buffer (15). `mods` on `POST /mouse` delivers only `shift` (34).
- **`key_down("shift")` silently returns false** — not how modifiers are read,
  and it fails quietly (35). `POST /key` cannot express a held key at all (35),
  which makes any hold-to-move control untestable.
- **`click_count()` is always 1** for a panel, so `"clicks": 2` cannot drive a
  double-click (15).
- **`print()` from a panel never reached `/state`'s `script.output`** (42).
- **`GARDEN_HEADLESS_SIZE` is the window size, not the panel size** — the pane
  rect is inset by the chrome, and nothing documents the delta (42).
- **Alpha composites in linear space**, so translucent light-on-dark is far
  stronger than the number suggests (25, 34).
- **Overlapping `clip` rects double-composite** (41).
- **`/scene` reports text runs but not mesh geometry** for a panel — every fill
  becomes a mesh, so rounded rects and circles cannot be asserted numerically
  (23, 25). Screenshot is the only check, and see issue 2.
- **Headless panels advance ~one frame per injected event**, and `dt()` is the
  wall-clock poll interval, so any physics needs its own clamp and substepping
  (01, 03). Worth documenting as the expected headless contract.

## Language issues

- **No forward references / no function hoisting** (13, 25, 35). Mutual
  recursion is impossible, so recursive-descent parsing needs a `var`
  trampoline. The failure mode is a runtime `Cannot call nil`, not a compile
  error. 13 shunting-yarded its expression parser specifically to avoid this.
- **No line continuation** (02, 24, 35). A binary operator may not start a
  continuation line and an `if` condition cannot span lines, which forces long
  boolean conditions onto one very long line.
- **`state` survives hot reload** (15, 34, 35, 43). Correct by design, but it
  makes iterating on *seeded data* impossible in place — you edit the
  generator, nothing changes, and the only fix is a process restart.
- **Byte indices vs character positions** (15, 23). `len`/`slice` are
  byte-based; `slice(s, 0, 1)` returns `""` for a leading multi-byte character,
  so the obvious "first letter of each word" loop silently produced wrong
  avatar initials for "Óscar". There is no `chars()` / `char_at()`.
- **No `sort_by` / comparator sort** (23, 43). Every table app hand-writes its
  own sort.
- **`float("3.5")` rejects strings** while `int("42")` is accepted (13); 25
  reports both rejecting.
- **Escaped quotes are not allowed inside an interpolation hole** (34, 41).
- **Draw overloads dispatch on arity**, so a plausible call silently means
  something else rather than erroring (33, 43).
- **Division by zero is a hard abort, not a value** (13) — awkward for a
  calculator.
- **No string formatting builtin** (42) — every dashboard hand-rolls
  fixed-decimal and thousands separators.
- **No scientific-notation float literals** (42) — `1.0e9` lexes as `1.0`.
- **Records can't be indexed by a computed key** (42).
- **`10 / 3 == 3` for ints** surprises in layout math (23).

## Prelude / API gaps

- No drag primitive (24) — every drag-and-drop app hand-rolls it.
- No ellipse, arc, or polyline primitive (33).
- `draw_rect_outline` has no rounded variant (23, 33), so a rounded bordered
  box needs two draws and still doesn't quite line up.
- `draw_rect_rounded` has no `(x, y, w, h, radius, color_record)` overload (41).
- `draw_line`'s record overload has no alpha or width form (14).
- `ellipsize` measures with a bare size but `draw_text` takes a style record,
  so what you measure isn't what you draw (43).
- `theme` is a fixed dark palette with no override hook (15), and `button`
  hardcodes it.
- Hand-rolling a text field is the default, not the exception (23) —
  `text_field` exists but is thin.
- No persistence primitive for panels (14). "A todo app remembers your todos"
  is unimplementable.
- `panel.values` is buried under prelude and constant bindings, is unusably
  large without `jq`, reports last-write-wins so per-iteration values are
  invisible, and does not obviously surface `state var` cells (01, 02, 14, 24).
  It reports only the last *good* frame, which reads as "that value is stale"
  rather than "the frame failed" (42).

## Praise

Consistently and unprompted:

- **`panel.values` as an assertion surface.** Being able to read a script's
  logical state by binding name, with no instrumentation in the script, was
  called out as the thing that made headless verification tractable at all —
  the complaints above are about ergonomics, not the idea.
- **The settle-then-capture contract.** Input-then-screenshot with no sleep
  worked exactly as documented, and made interaction scripts deterministic.
- **`text_width` with real font advances.** Exact centering and
  right-alignment at every size; several agents built precise typographic
  layouts on it.
- **Immediate-mode drawing with `state`** was a natural fit for UI. Agents
  repeatedly noted that the frame-as-a-function model made complex interactive
  state easy to reason about.
- **Overload-arity errors** are clear and well-worded when they fire (33).
- **Hot reload within ~200 ms**, when it worked, was the core of the loop.

## Process notes (parallel-run artifacts, not product bugs)

- Two agents `curl`ed a port belonging to a *different* agent's Garden and
  briefly debugged someone else's app (14, 42). The debug server exposes no
  identity — a `name`/`script` field in `/state`'s root would make this
  self-diagnosing.
- Concurrent commits into one index caused one agent's staged files to be swept
  into another's commit (01 swept in 33's app files). Content is intact; the
  commit boundaries are just untidy.
- `.temp/` is gitignored, so debriefs needed `git add -f`. Some agents did,
  some didn't — the debriefs are all present on disk regardless.
