# What the testbed apps still trip on

Status: **proposed**, 2026-09-21. Collated from every testbed debrief and
checked item by item against HEAD `773a73e`.

## Where this comes from

Eighteen testbed apps were built by agents working only from the docs, and
each one left a debrief:

- `.temp/testbed-debriefs/` — fifteen apps from batches 1–3 (August), plus
  `SUMMARY.md`, which collated and ranked them on 2026-08-09.
- `.temp/testbed-debriefs/asteroids.md` — testbed 05, 2026-09-16.
- `.temp/node-editor-takeaways.md` — testbed 38, 2026-09-16.
- `.temp/markdown-editor-takeaways.md` — testbed 39, 2026-09-16.

The debriefs raised about 75 distinct items. On 2026-09-21 each one was
re-run as a snippet, tested against a headless Garden, or traced in code
and git history. **Most are fixed.** Of SUMMARY.md's five "fix first" items,
all five have landed (see [Closed](#closed)). What is left is below, ranked.

The ranking favours two kinds of item. The first is a *silent* failure: a
tool that says "fine" when it isn't, or a check that passes and then dies at
runtime. These cost the most time, and they undermine the verify loop the
guides tell authors to rely on. The second is anything more than one app hit
independently.

## Tier 1 — silent failures in the verify loop

### 1. `values_stale` reports a healthy panel as stale

`garden-app/src/app/debug_server.rs:628`:

```rust
"values_stale": pv.observed_frame() != Some(pv.frame_count() - 1),
```

Since the frame gate landed (`d90bc35`), an idle panel skips frames, so its
last *observed* frame can trail `frame_count - 1` while nothing is wrong. A
live check read `error: null, frame 256, values_frame 203, stale: true`. A
harness told to trust `values_stale` now reports failures on idle, healthy
panels. Stale should mean "the last frame that *ran* failed", not "the last
frame number was not observed". Test with a gated panel. **S.**

### 2. `petal check` passes calls that fail at runtime

This is the most-reported item across the September apps (asteroids, node
editor, markdown editor, and the vector editor before them). Every one of
them passed `check --strict` and then crashed in Garden, the expensive
round trip that `check` exists to avoid. Three separate holes:

- **Unknown globals.** `petal check --strict -e 'totally_bogus_fn(1)'` exits
  0, with or without `-I petal-ui/prelude`. `run` reports `Unknown builtin`.
  The checker needs to know which natives a host registers. Core plus the
  petal-ui set is a sound default, with a flag for Garden's extras.
- **No argument signatures for natives.** `rust/src/typecheck/builtin_types.rs`
  holds return types only. `sqrt("x")`, `range({r:1})` and
  `fill_rect(0,0,{r:1},4,1)` all pass. A user `fn f(x: int)` does warn, so
  the machinery exists. Natives just don't feed it. The declared
  `NativeEffects` rows (`540f48c`) show that every native already registers
  a descriptor, so this is a natural extension of it.
- **Prelude overloads dispatch on arity alone.** A plausible call lands on the
  wrong overload and fails with a misleading message ("Expected int at arg 5,
  got record"; "Cannot access field 'x' on int"). Only 4-arg `draw_polyline`
  checks its argument type (`petal-ui/prelude/ui.ptl:327`). Once natives
  have signatures, `check` can flag a call where no overload of that arity
  accepts the argument types.

**M** overall. The unknown-global part is the cheapest and should go first.

### 3. Mixed "flat coordinates + colour record" draw forms are missing

`8b9fa22` added record-*geometry* overloads (point, centre or rect record
plus a colour record) but none of the forms the agents actually wrote,
which take flat coordinates and a colour record. These are confirmed failing
at runtime:

| Call | Failure |
|---|---|
| `draw_rect_rounded(x, y, w, h, radius, C)` | "expects 8/9/3/4 args, got 6" |
| `fill_triangle(x1, y1, x2, y2, x3, y3, C[, a])` | "got 7/8" |
| `draw_line(x1, y1, x2, y2, C, a, w)` | "Expected int at arg 5, got record" |
| `draw_circle_outline(cx, cy, r, C)` | taken as the centre-record form; "Cannot access field 'x' on int" |

The 7-arg `draw_line` and 4-arg circle forms collide on arity with existing
forms, so they need a type test like `draw_polyline`'s. Doing this before
item 2 gives quick relief. `garden/docs/petal-graphical-panels.md:153` and
`examples/AUTHORING.md:249` say "every draw primitive has a record form",
which reads as covering these calls. **S–M.**

### 4. A top-level `fn` that reads any top-level `let` is not hoisted, and the warning doesn't prevent the crash

The rule today (`f0c52da`, `78b3711`, `94240bf`): a top-level `fn` is hoisted
unless its body, directly or transitively, mentions a top-level
`let`/`var`/`state` or an enum variant, or shadows a name in scope. That
includes a literal `let k = 4`. A top-level call above such a fn gets a
compile warning ("…cannot be hoisted") and then still dies at runtime with
`Cannot call nil`. It bit the node editor on
`state cam = fit_all(seed_nodes(), canvas)`.

Two options, not exclusive:
- hoist fns whose captured `let`s are compile-time constants, which is the
  common case (`NODE_W = 140`);
- make the warning an error under `--strict`, since the program is
  known to crash if that line runs.

The writing guide documents the caveat at `docs/writing-petal-guide.md:100`.
**M.**

### 5. Letter-spaced runs position non-ASCII glyphs at a 0.6 em fallback advance

This is new, found while checking the markdown editor's glyph-fallback
report. That report itself is fixed: `⌘ ⇧ — ·` now draw on every face via
per-cluster system fallback. But a run with `spacing:` set, which splits
into one run per glyph, advances each non-ASCII glyph by
`DEFAULT_TEXT_ADVANCE` (0.6 em, `petal-ui/src/text.rs:28`) instead of its
real width. At size 32, `/scene` showed 19.2 px for ⌘, — and ·, and in the
screenshot ⌘ overlaps the next letter. The per-glyph path likely reads the
ASCII-indexed ratio table. That path isn't traced yet. **S–M.**

## Tier 2 — friction more than one app hit

6. **`text_width` returns a rounded integer.** `run_width(...).round() as i64`
   (`petal-ui/src/text.rs:789`), and the type checker declares it `Int`.
   `text_width("m", {size: 13})` is 8, but the true advance is 7.8, so
   monospace column math drifts a full character by column 30 (markdown
   editor; the notes app's `CW` has the same latent drift). Add a float
   `text_advance(s, style)`. Leave `text_width` alone, because layouts rely
   on its integer result. AUTHORING.md already documents the workaround of
   measuring a long run. **S.**

7. **Prelude exports leak into every panel's `values`.** `GRAD_RIGHT/DOWN/
   LEFT/UP` and `theme` appear in each panel's `panel.values`. `_` and `::`
   keys are already filtered, so extend that to prelude-module bindings.
   **S.**

8. **Parse errors that should name the fix.** Each of these took an agent a
   round trip:
   - `"{\"a\": 1}"` → `Expected string part in interpolation, got Colon`.
     Hint: "`{` opens an interpolation; write `\{` for a literal brace".
   - `fn(b) -> b * 2 end` → `Expected ',' between arguments` at `end`.
     Hint: "an arrow lambda takes no `end`".
   - `"{}"` compiles to an empty string with no warning, so a `{}`-style
     template like `format("{} items", 3)` silently prints " items". Warn on
     an empty interpolation hole.
   - `a and b` → `Undefined variable: and`. Hint: "use `&&`".

   **S** each.

9. **⌘-scroll in a real window reaches the panel without modifiers.**
   `8b9fa22` fixed the debug `scroll` op, but the windowed frontend's
   `MouseWheel` handler (`garden-app/src/frontend/window.rs:394`) calls
   `handle_scroll` with no modifier state. So ⌘-wheel zoom works headless
   and not for a user. **S.**

10. **`slice`/`len` are byte-indexed.** The char-aware builtins exist
    (`405562a`: `chars`, `char_len`, `char_at`, `char_slice`), but
    `slice("Óscar", 0, 1)` is still `""` and `len("Óscar")` is 6. That is
    the silent-wrong-initials bug from the CRM app. Changing the semantics
    is a language decision, so decide it here. A cheaper middle ground is a
    lint when `slice` indexes come from a `for` over `range(len(s))`.
    Related: [char-at-fast-path](optimizations/char-at-fast-path.md).
    **S–M.**

11. **Small API gaps**, each **S**:
    - `slider(r, v, lo, hi, {key: id})`. In a loop, `slider` state is
      positional since callsite keying, so slots don't follow a reordered
      list (node editor).
    - `draw_polyline(..., closed: true)`, or a `draw_polygon_outline`,
      instead of `append(pts, pts[0])` at every call (asteroids).
    - `range(a, b, step)` with a negative step, and `rotate(v, angle)` on
      `vec2` (asteroids).

## Tier 3 — larger or lower-value

12. **`text_field` has no selection, clipboard or undo** (`ui.ptl:2043`;
    bloom's `text_field` doesn't either). Hand-rolling a text field is still
    the default for any app that edits text. **M.**
13. **Draw coordinates are `i32` throughout the command format**
    (`coord_to_i32`, `petal-ui/src/draw.rs:1545`; every primitive's fields).
    Rotating polygons jitter by up to a pixel per vertex. This is bigger
    than the asteroids report suggests: it changes the draw-command format
    and every host. **L.**
14. **Scripted gestures have no hover frame before a press.** Press ordering
    is sound (`mouse.rs` sets the position before the press, and a frame
    runs right away), so the node editor's `hit.kind == "none"` flake is not
    explained by coalescing. But a `click`/`drag` op sends no separate
    *move* frame first, so a script that needs a hover frame before the
    press would miss it. Consider an optional `hover_first` on the op, or
    document it. **S.**

## Docs batch

These are all small and could land as one commit:

- `writing-petal-guide.md` `state(id)` paragraph (~:250): the slot outlives
  deletion of the key's owner. It's a leak, not a bug.
- `petal-ui/docs/components.md` "Measure the face you draw" (:245): how to
  measure a space, and point at `text_advance` once item 6 lands.
- `examples/productivity/kanban/README.md:76` and `app.ptl:7`: delete "the
  embedded face has no bold". Only `mono` is Regular-only.
- AUTHORING.md "Idiomatic Petal" (~:353): the `get` rule for module-level
  `var` from helpers (the guide has it at :263).
- AUTHORING.md checklist (:356): `petal check --strict`, to match :109.
- `garden/docs/debug-server.md:231`: the `# exact names` comment contradicts
  the tail-match rule stated at :236. Its `/panel/reset` row should also say
  a live store is reloaded (AUTHORING.md:63 does).
- Writing guide builtins table (~:741): the `parse_float(s) ?? 0.0` idiom.
- `docs/Builtins.md`: `safe_div` is missing; it's only in `stdlib.json`.
- Optional: a real per-face glyph-coverage list in petal-graphical-panels.md
  (:476 has examples only). With system fallback working, this may no
  longer matter. Check before writing it.

## Process: stale binaries

Two of the August reports (the glyph atlas and `/screenshot` ghosting)
describe behaviour fixed well before the run, and the verification pass
found two binaries it couldn't trust:

- `garden/target/debug/garden` had been stale since 2026-09-16.
- `~/.cargo/bin/petal` doesn't accept `check`.

An agent following AUTHORING.md can easily test against an old build. The
fix: `launch.sh` rebuilds, or refuses to start, when the binary is older
than `HEAD`. It already exposes `identity.build` with commit and dirty
flag, so a mismatch warning in `/state` would also do. **S.**
(`~/.cargo/bin/petal` was reinstalled from `rust/` on 2026-09-21. Nothing
keeps it fresh, so re-run `cargo install --path rust --locked` after
language changes.)

## Closed

These were checked at HEAD and need no work. Listed so the debriefs can be
retired.

**SUMMARY.md's top five:**

| Item | Fixed by |
|---|---|
| `for` in a fn's tail returns nil | `92fef1d` (and `e882b8f` in the checker) |
| Glyph atlas corrupts under a mixed type scale | `02f47e0` (swash 0.2.10), regression tests `b19065e`; `/state.text_atlas` counters |
| Compile failure is silent; failed reload runs old code; errored panel never recovers | `d229d05`: `status_error`, stub pane recovers on save, no reset needed |
| `clamp` returns a float | `405562a` |
| Window vs pane-local mouse coordinates | documented: AUTHORING.md:132, petal-graphical-panels.md:40, `DebugClient.gesturePaneLocal` |

**Language:**
- mutual recursion and forward calls (`f0c52da`)
- line continuation with a leading operator (`499512c`)
- `sort_by` and comparator `sort` (`6fcabef`)
- `float("3.5")`, `parse_float`, `parse_int` (`405562a`)
- escaped quotes in interpolation holes (`499512c`)
- `fixed`, `commas`, `format`, `pad_*` (`6fcabef`)
- scientific-notation literals (`499512c`)
- computed record keys
- `json_stringify`/`json_parse` (`8b9fa22`)
- `safe_div` (`6fcabef`); division by zero aborting is by design
- int `10 / 3 == 3` is by design and documented

**Prelude:**
- `context_menu` shadow alpha (`98b8ee2`, `017c328`)
- record forms for polyline, polygon, fan, triangle, circle/ellipse
  outline, arc and rounded outline (`8b9fa22`)
- drag primitive `drag_state` (`017c328`)
- ellipse and arc primitives (`b9fff57`)
- `ellipsize` takes a style record (`05e4b4e`)
- `theme_set`/`theme_from_palette` (`017c328`)
- `panel_store_*` persistence
- context menu keyboard beats a resting pointer (`017c328`)
- `menu_rect` exported (`017c328`)
- `text_width(" ")` measuring zero was not reproduced

**Garden:**
- text honours painter's order (`65ef82f`)
- alt modifier, `claim_key` chords, full `/mouse` chords (`57b2c8e`); debug
  scroll carries mods (`8b9fa22`)
- held keys and modifiers as keys (`57b2c8e`, `8f8aaa6`)
- `click_count` (`57b2c8e`)
- panel `print` reaches `script.output` (`d229d05`)
- sRGB compositing (`889e214`)
- overlapping clips not reproduced
- mesh primitives in `/scene` (`57b2c8e`)
- `POST /tick` with a virtual clock (`3503f45`)
- `ui`-face glyph fallback
- `/screenshot` ghosting (`b19065e`)
- `/state.identity`
- orphaned headless shutdown is by design (`aacc98e`) and documented with
  the `nohup` form
- `GARDEN_PANEL_STORE_DIR`

**Docs:** 19 of 24 reported gaps are already fixed, including the hoisting
caveat, the `\{` escape, `== nil`, lambda syntax, the `nohup` launch form,
the store directory, font weights and italics, monospace column math,
text-editing test advice, the headless frame contract, and seed data
surviving reload. The rest are in [Docs batch](#docs-batch).
