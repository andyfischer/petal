# Building a side-scroller in Petal — debrief

A candid record of what was pleasant and what was painful about using
Petal as the main language for a real game. The experiment produced
`game.ptl`, `editor.ptl`, three levels, and three small file-I/O natives
in `petal-desktop-sdl` (`load_text_file`, `save_text_file`,
`file_exists`).

Several of the pain points below have since been fixed in the language.
Each item is marked **shipped**, **partly shipped**, or **open** as of
September 2026.

## What worked

**Hot reload.** Editing `game.ptl` while a level is running, with player
position and coin progress preserved, made tuning gravity, jump height
and camera speed far faster than an edit/rebuild/replay loop. Every
constant at the top of the file was tuned this way.

**Records are references.** `for c in level.coins do c.collected = true end`
mutates the list element in place. Without that, every entity update
would be a `map` over the list, and simple gameplay code (set a flag,
decrement HP, flip a direction) would be unreadable.

**String interpolation everywhere.** `"plat {p.x} {p.y} {p.w} {p.h}"`
made the level serializer a six-line function.

**Functions are hoisted.** A helper can be called before it is defined
in the file, which matters when the whole game is one script.

**Match with guards** keeps branchy gameplay code (classify by velocity,
and so on) readable.

**The `petal-sdl` host is small and well shaped.** The native bridge is
tiny, the draw-command buffer keeps the interpreter loop simple, and
adding three file-I/O natives took about fifty lines. Running the whole
script every frame makes "state is what persists" a clean mental model.

## Pain points

**No imports — shipped.** `parse_level` and `serialize_level` were
duplicated between `game.ptl` and `editor.ptl` because there was no
`import`. Petal now has `import` (see `docs/language-guide.md` and
`examples/console/imports.ptl`). The two scripts here still carry the
duplicate copies.

**No way to declare a record's shape — partly shipped.** Adding
`origin_y` to enemies and forgetting it on one code path only surfaced
as a runtime "field not found". Petal now has classes with typed fields
and optional type annotations checked by `petal check` (warnings-only,
`--strict` to fail). Plain record literals are still unchecked.

**`petal check` does not flag unknown names — open.** A call to
`draw_rect_typo(...)` still passes `petal check` (exit 0) and fails at
runtime with "Unknown builtin". The checker has no registry of the
host's native names, so this is the largest remaining gap for a script
that calls dozens of host functions by name.

**`state` scoping was unclear — shipped.** `state` is now keyed per call
path and the rules are documented in the language guide (`state`
section).

**No file/line in runtime errors — shipped.** Runtime errors such as
"No field 'origin_y' on record" now report line and column and show the
source line.

**No float formatting beyond `str(x)` — shipped.** `format("%.2f", t)`
and `fixed(t, 2)` exist (see `rust/src/builtins/format.rs`), so a
"1:23.05" timer no longer needs hand-rolled arithmetic.

**No way to index a record by string key — shipped.** `r["x"]` and
`keys(r)` work, so "delete under cursor" could now walk
`["plats", "oneways", "coins", ...]` in one loop instead of six.

**`split("", " ")` returns `[""]` — open.** A trailing newline in a
level file still yields a phantom empty tag, so `parse_level` keeps its
`tag == ""` guard. A `split` that drops trailing empties would be the
pit of success here.

**Drawing takes integer coordinates only — open.** `draw_rect` and
friends still take ints, so sub-pixel motion jitters at render time,
most visibly on slow parallax layers. Accepting floats and rounding on
the host side would fix the stair-stepping.

## Undecided

- **Whole-script-per-frame.** Elegant, and the reason hot reload works
  so well, but every helper is re-created each frame and it feels
  wasteful for a 700-line game. A `setup` / `frame` split, as in
  Processing, might be cleaner without losing hot reload.
- **`++` for string concatenation.** Still the operator, but with
  interpolation it is rarely needed. It may be worth deprecating in
  favour of interpolation plus explicit `str()`.
- **Match on strings** does work (`when "plat" -> ...`); the level parser
  uses an `if`/`else if` ladder only because that was not obvious at the
  time.

## Bottom line

Petal got a polished platformer built in roughly the code a Lua version
would take, and hot reload plus records-as-references is a genuinely
nice place to live. Of the seven concrete suggestions the experiment
ended with, five have shipped (imports, error locations, record
indexing, `format`, documented `state` scoping). The two still open are
making `petal check` resolve host native names and accepting floats in
the drawing primitives.
