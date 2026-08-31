# Building a side-scroller in Petal — debrief

This document is the artifact the experiment was actually for: a candid
record of what was pleasant, what was painful, and what the language and
runtime should consider changing, written from the seat of someone who
just used Petal as the primary application language for a real game.

## What got built

- One Rust patch (3 new natives totaling ~50 lines) to give Petal scripts
  controlled file I/O for level loading and saving.
- ~700 lines of `game.ptl`: title screen, level select, side-scrolling
  camera with ease-out follow, full physics (gravity, jump-buffer, coyote
  time, variable jump height, friction, squash & stretch), solid and
  drop-through platforms, two enemy archetypes (patrolling goomba,
  bouncing slime), spikes, coins, mid-level checkpoints, end-of-level
  goal, parallax sky/clouds/mountains, particle effects, screen shake,
  HUD with hearts/coins/timer/level name, pause / death / level-clear /
  game-complete overlays, per-level best-time and coin tracking.
- ~350 lines of `editor.ptl`: mouse placement of every entity type,
  drag-to-size for platforms, grid snap, slot switching, save/load to
  disk, delete-under-cursor, hover indicator, on-screen tool palette and
  status bar.
- 3 hand-authored levels in a tiny line-based text format that round-trips
  cleanly through both files.

The whole thing fits in one feature branch with no new Rust dependencies.

## The good

**Hot reload is the killer feature.** Editing `game.ptl` while a level is
running, with player position and coin progress preserved across the
reload, made tuning gravity / jump height / camera lerp speed enormously
faster than the usual edit/rebuild/reload-game/replay-to-the-spot loop.
Every constant at the top of the file was tuned this way.

**Records are references.** The discovery that
`for c in level.coins { c.collected = true }` mutates the underlying list
element was the single biggest productivity win. Without it, every entity
update would have needed `level.coins = map(level.coins, fn(c) { ... })`,
which makes simple gameplay code (set a flag, decrement HP, flip a
direction) unreadable. With it, the gameplay loop reads almost like Lua.

**String interpolation works inside record fields and any expression
position.** `"plat {p.x} {p.y} {p.w} {p.h}"` made the level serializer a
six-line function. Doing this with `++ str(...)` would have tripled it.

**`fn` definitions are hoisted across the whole script.** Calling
`filter_index` inside `delete_at` before `filter_index` is textually
defined "just works." This was a small relief given the file is one
big script with no module structure to organize the order.

**Match with guards is genuinely nice.** Branchy gameplay (e.g.
classify-by-velocity) reads cleanly and avoids ladder-of-`else if`.

**The petal-sdl runtime is well-designed for its size.** The native FFI
(`state.get_int`, `push_string`, etc.) is small, the draw-command buffer
+ deferred renderer keeps Petal's interpreter loop simple, and adding
the three file-I/O natives took 50 lines and one rebuild. The whole game
re-runs every frame, which makes the mental model of state-as-persistent
extremely clean.

## The frustrating

**No imports.** `parse_level` and `serialize_level` are duplicated
verbatim between `game.ptl` and `editor.ptl` because there is no `import`
or `include` statement, and the runtime takes one `.ptl` file. For a
project that grew past ~500 lines this was the loudest problem — every
time I changed the level format I had to remember to update both files
in lockstep. Even a textual `#include "lib.ptl"` resolved by the Rust
host would have been enough.

**No way to declare or check a record's shape.** When I introduced
`origin_y` to enemies for the editor, I forgot to add it to records
created in some code paths and only found out from a runtime
`field 'origin_y' not found` error long after compilation. There is no
type system, no shape annotation, and `petal check` of course can't
catch a missing field. A few options exist: structural row types, named
record types (`record Enemy { x, y, kind, ... }` plus
`Enemy{...}` literal that fills missing fields with defaults), or even
just a debug-mode "list every field accessed on this record" warning.

**`petal check` is a thin wrapper around lex+parse+IR-build and does not
flag unknown name references.** A `draw_rect_typo(...)` call passes the
checker and only fails at runtime. For a game scripted by hand-typing
function names this hurts. A symbol-resolution pass (with a registry of
known native names supplied by the host) would catch about 80% of the
runtime errors I hit.

**Mutable globals via `state` are positional / shadowable in subtle
ways.** I generally enjoyed `state x = 0`, but a `state` declaration
inside an `if` branch behaves… non-obviously. I worked around it by
keeping every `state` declaration at the top of the file, but a clearer
spec on the scope of `state` would help.

**No file/line in many runtime errors.** When I wrote
`e.origin_y_or(e.y)` in a fit of magical-thinking, the runtime error
just said the field was missing — no location, no stack. I eventually
found it by binary-searching with comments, but a stack trace pointing
at `game.ptl:312` would have saved minutes.

**`split("", " ")` returns `[""]` rather than `[]`.** Tiny papercut, but
it bit me in the level parser — the level file's trailing newline made
every `parse_level` produce a phantom "" tag. I added a `tag == ""`
guard. A `split` that drops empty trailing tokens (or a
`split_nonempty`) would feel more pit-of-success.

**Drawing API has integer coordinates only.** Sub-pixel motion looks
fine in physics but jitters at render time, especially on the parallax
layers. An `f32` overload of `draw_rect` etc. would let the engine do
the rounding consistently and remove visible stair-stepping at slow
camera speeds.

**No native float formatting beyond `str(x)`.** I wanted to display
"1:23.45" cleanly and ended up doing `int(t * 100.0) % 100` arithmetic
that drops leading zeros (so 1:23.05 displays as "1:23.5"). A `format(...)`
or even `str(f, decimals)` would be a small high-leverage addition.

**No way to query a record's keys at runtime from inside Petal.** I
wanted the editor's "delete under cursor" routine to walk every entity
type with one block of code, but had to write six near-identical loops
because the entity collections live in fixed-name fields and there's no
way to iterate over `["plats", "oneways", "coins", ...]` and look them
up dynamically. An indexer like `level["plats"]` (already implied by the
record-as-map model) would dedupe a lot of code.

## Things I'm undecided about

- **Whole-script-per-frame model.** It's elegant for short examples and
  the hot-reload story is great, but in `game.ptl` it means every helper
  function is re-created every frame. I never noticed performance
  problems, but the feeling that all of `parse_level` recompiles each
  frame nags. A "setup" / "frame" split (à la Processing's `setup()` /
  `draw()`) could be cleaner without giving up hot reload.
- **String concat with `++` vs `+`.** I appreciate that `+` stays
  numeric-only, but `++` on every line tires the eyes. Now that string
  interpolation works (`"hi {name}"`), I almost never reach for `++`,
  which suggests it could be deprecated in favor of interpolation +
  explicit `str()`.
- **Match-on-string.** I parsed level entries with
  `if tag == "plat" { ... } else if tag == "oneway" { ... }` instead of
  `match tag { "plat" -> ..., "oneway" -> ..., _ -> ... }` because I'm
  not 100% certain match-on-string is supported with arbitrary blocks.
  A worked example in the docs would clarify.

## Concrete suggestions, ranked

1. **Add an `import "path.ptl"` form** that the host resolves at compile
   time. Even just textual concatenation of files would be enough for
   level/editor sharing.
2. **Make `petal check` resolve names against the host's native
   registry.** Catch `draw_rec` typos before runtime.
3. **Print file:line in runtime errors.** Especially "field not found"
   and "undefined name."
4. **Allow record indexing by string key**: `r["x"]` equivalent to `r.x`.
   Unlocks dynamic per-field loops.
5. **Add a `format(fmt, args...)` builtin** for HUD strings.
6. **Allow floats in drawing primitives** (host-side: take f32, round
   internally).
7. **Promote `state` declarations to file-top only** by spec, or
   document the scoping rules carefully.

## Bottom line

Petal got me from zero to a polished single-screen platformer in roughly
the same amount of code I'd write in Lua, and the hot-reload + records-
as-references combo is genuinely a nice place to live. The friction came
from small ergonomic gaps — no imports, weak compile-time validation,
sparse error messages — that any of the suggestions above would chip
away at. None of these are deep design problems; they're the next-step
items for a language that just proved it can host a real game.
