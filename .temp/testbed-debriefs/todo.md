# 14 Todo app (Thicket)

**Status:** complete
**Viewport:** 1280x850 window → 1268x778 panel pane
**What works:** Everything in the README. Create (parsed composer line with
`!`/`!!`, `@today`/`@tomorrow`/`@week`/`@someday`, `#list`), read (date-grouped,
priority-sorted feed), update (title rename in place, priority, due-date nudge,
done toggle from three different affordances, subtask checklist), delete with an
undo stack, three-axis filtering (list + status + full-text search), pixel
scrolling over variable-height rows with a scrollbar, hover/selection states,
keyboard and mouse parity, empty state. `status_error` stayed null through the
whole interaction script; every control listed in the README was driven through
`/key`, `/text` and `/mouse` and verified in both `/state` and the pixels.
**What I could not do:** Real persistence. A panel has no file or key/value
native, so the app's "autosave" is a revision counter over `state`, which
survives hot reload but not a restart. Documented in the README.

## Blockers

None.

## Issues

**1. `draw_line`'s record overload has no alpha or width form.** Every other
primitive gained record overloads with the optional trailing args
(`draw_rect(r, c, a)`, `draw_circle(center, radius, c, a)`,
`draw_rect_outline(r, c, a, width)`), but `ui.ptl` only exports
`draw_line(a, b, c)`. My first draft was full of

```petal
draw_line({x: cx - 4.5, y: cy}, {x: cx - 1, y: cy + 3.5}, INK, 255, 2)
```

which fails at runtime, not compile time:

```
draw_line() expects 7 or 8 or 9 or 3 arguments, got 5 [line 762, column 1]
```

The error is excellent (source line, caret, and the `ui::draw_line` →
`_native_line` call chain), but "7 or 8 or 9 or 3" is a strange menu to read and
the asymmetry with the other primitives is a real trap — thick or translucent
strokes are exactly what checkmarks, strikethroughs and cursors need. I worked
around it with a local helper:

```petal
fn stroke(a, b, c, alpha, width)
  draw_line(a.x, a.y, b.x, b.y, c.r, c.g, c.b, alpha, width)
end
```

**2. `/mouse` takes window coordinates, the script sees pane-local ones.** The
AUTHORING.md examples (`/mouse -d '{"op":"click","x":80,"y":30}'`) sit right next
to "panel-local logical pixels, (0,0) at the pane's top-left", and I assumed the
injected coordinates were in the same space. They are not: in a default headless
window the pane origin is `(6, 38)`, so a click computed from a panel rect misses
by that offset. I lost real time convinced my checklist hit-test was broken when
it was fine — the giveaway was `mouse_x`/`mouse_y` in `panel.values` reading
`(994, 522)` for an injected `(1000, 560)`. One sentence in AUTHORING.md ("add the
pane rect from `/state` to panel coordinates") would fix this.

**3. A `/screenshot` on my own port came back as another agent's app.** One
capture returned a fully-rendered frame of a *different* testbed app (a Breakout
clone, another agent's concurrent headless Garden). `/state` on the same port
returned my panel immediately before and after, `lsof` confirmed only my PID was
listening on it, and the next `/screenshot` was correct. So it is not a port mixup
on my side. This smells like process-global state in the lazily-created headless
renderer / capture path being reachable across concurrently running Gardens (or at
minimum a response getting crossed). It matters: a screenshot is the one
verification an agent cannot sanity-check cheaply, and a silently wrong one is
worse than an error. Not reproducible on demand; happened once in ~15 captures
with roughly a dozen headless Gardens running.

**4. No persistence primitive for panels.** A todo app's natural spec is "it is
still there tomorrow". There is no file read/write, no key/value store, and
`emit` only exists in panel-mode GPP apps with a client on the other end. The
honest workaround is `state` (which does survive hot reload — genuinely useful
during development) plus a fake revision counter.

**5. `panel.values` is unusably large without `jq`.** My script binds a 22-record
seed list, and the dump repeats it inside `tasks`, `items`, `scoped`,
`sort_tasks.out`, `sort_tasks.res`, `selected`, `cur` … one `GET /state` is
~1500 lines. A `?values=a,b,c` filter (or excluding names bound before the first
input read) would make the observation loop much cheaper for an agent.

**6. Docs: `for` in value position also collects inside a record literal field.**
The language guide lists value position as "assigned to a name, `return`ed, passed
as an argument, or placed as a list element". A record *field* value works too,
and I relied on it:

```petal
{...t, subs: for j in range(0, len(t.subs)) do
  if j == sub_hit then {...t.subs[j], done: !t.subs[j].done} else t.subs[j] end
end}
```

I only tried it because there was no alternative; the docs made it look like a
coin flip whether the field would get a list or a `nil`.

**7. `clamp` returns a float.** Every use in pixel/index code has to be wrapped in
`int(...)`, and forgetting it is silent (a float index or a fractional rect). The
prelude has a private `_clamp` for exactly this reason and comments on it — that
comment is a sign the builtin's return type is the wrong default for UI code. An
`iclamp`, or `clamp` preserving int-ness when all three args are ints, would
remove a whole class of paper cuts.

**8. Minor:** `weight` degrading to regular is documented, but it does bite a
design like this one, where "title vs metadata" is the main hierarchy. I got
there with size + color + spacing, which is arguably better discipline, but it
means every app in this batch will look like it uses one font weight — because it
does.

## Praise

- **`panel.values` is the feature.** Being able to assert `selection`,
  `visible_count`, `revision` and `trash_size` by name, without decoding pixels
  or adding instrumentation, made the whole loop tight. Naming a few summary
  `let`s at the bottom of the script purely for observation felt natural.
- **Hot reload with preserved `state`** — edit the file, `curl /state`, see the
  new layout with the same selection and scroll position. I never restarted
  Garden except to check the seed data.
- **Runtime errors are first-rate**: source line, caret, and the call chain
  through the prelude down to the native. I fixed the one error in this project
  in under a minute from the message alone.
- **`ellipsize`, `preview`, `fit_parts`, `ensure_visible_px`** all did exactly
  what I needed with no fuss, and the byte-vs-char hazards are documented right
  where you read them. `fit_parts` in particular turned "a metadata line that
  degrades gracefully" into one call.
- **Exact `text_width`** — right-aligned columns, centered labels and wrap widths
  all landed on the first try, including for the styled-record form.
- **Record spread as the update idiom** reads beautifully for CRUD:
  `tasks = for t in tasks do if t.id == id then {...t, done: !t.done} else t end end`
  is the entire update path.

## Feature requests

1. **Record overloads for `draw_line` with alpha and width** (`draw_line(a, b, c, alpha)`,
   `draw_line(a, b, c, alpha, width)`) — the only primitive missing them, and the
   one that most needs them.
2. **Document the pane-origin offset for `/mouse` injection** in AUTHORING.md and
   `debug-server.md`, ideally with the "add `panes[n].rect`" one-liner.
3. **Investigate the crossed `/screenshot`** described in issue 3. If the headless
   capture path has any process-global state, several agents screenshotting at
   once will occasionally get each other's pixels.
4. **A value filter on `/state`** (`?values=sel_id,focus` or `?panel=0&values=…`)
   so an agent can poll a handful of names instead of a megabyte.
5. **Some persistence for panels** — even a single `panel_store_get/set(key, string)`
   scoped to the script path would let a todo/notes/settings app be honest.
6. **Int-preserving `clamp`** (or a documented `iclamp`).
7. Longer term: embedding the Bold face, so `weight` in a style record means
   something.
