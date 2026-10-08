# Petal, judged by a fantasy console

Notes from building petal-fantasy-nes: a Rust host of about 4,900 lines, a
Petal prelude of about 2,600, and 5,000 lines of carts. Everything above the
two chips (artwork, maps, text, menus, scenes, collision, a tracker-shaped
music driver, and sample synthesis) is Petal, so this was a fairly severe test
of the language away from "draw a few shapes".

**Verdict: yes, with caveats.** The frame model is the right model for this
kind of app, and the console's whole authoring surface fell out of the language
rather than being imposed on it. The two problems serious enough to block a
cart (§1 and §8) have since been fixed in the language. What remains is
ergonomics, and one real cost: per-character string work, which is exactly
what an art-from-strings console does most of (§9).

Each item below is marked **fixed** or **still open** as of 2026-09-02; the
open ones were re-checked against the current `petal` CLI.

---

## What worked well

**Re-run-the-whole-file is the right frame model.** The console pushes its
entire state every frame and Petal makes that free of ceremony: no
`setup()`/`draw()` split, and hot reload is not a feature anyone implemented,
it is what happens when the file runs again. Edit a pixel of a tile while the
game runs and it repaints in place with the player where he was.

**Records as the universal shape.** Nothing in the prelude needed a class.
`move_box` returns `{rect, hit_x, hit_y, grounded}`; `menu` returns `{index,
chosen, cancelled}`; an instrument is a record of optional envelopes with `??`
filling the gaps. Records indexed by a computed string key stand in for a map
(the sound bank, menu cursors, the art character table).

**Multi-line strings made data look like the thing.** Tile art *is* the picture
and a tracker pattern *is* a column of notes, both as plain string literals.
This is the single biggest reason the console has no asset pipeline, and it
means art and music diff cleanly.

**Arity overloading** (`palette(i, c1, c2, c3)` / `palette(i, list)`, `btn(name)`
/ `btn(pad, name)`, an optional flags argument on `sprite`) keeps the cart-facing
API to about half the names it would otherwise need.

**Calling script functions from the host.** Realtime synthesis is a Petal
function the host calls by name every frame, and the block helpers take a
closure. That the host can call into a script function mid-frame is what made
`enable_dsp` possible.

**`f64_array` earns its keep.** One frame of 44.1 kHz audio synthesized in
Petal costs 0.32 ms with an `f64_array`, 0.59 ms appending to a list. The typed
array is the difference between a toy and a channel you can ship. Measurements
are in [design.md](docs/design.md#the-dsp-budget).

**Modules and implicit imports gave a real prelude.** `nes.ptl` and
`nes_sound.ptl` are ordinary Petal registered as implicit imports, so carts call
`draw_meta` and `music_play` bare and can read the implementation when a helper
does not do what they want.

---

## What was awkward

### 1. `state` cells were keyed by name alone — **fixed**

Two functions that each declared `state var t` used to share one cell, with the
second initializer silently ignored. A cell is now keyed by its declaration and
call path, so each callsite of a helper gets its own cell, and a top-level
`state var` read with `get`/`set` is the way to share one. `state(key)` remains
the tool for a cell whose identity outlives its callsite (a menu cursor, a
button's repeat phase). The prelude's accessor-wrapper idiom was deleted as a
result. Rules: `docs/dev/state-call-paths.md`; user-facing description in the
language guide's State section.

One cost worth knowing: a `state` used as a memo inside a function called from
a loop is now rebuilt per iteration, because each iteration is a new path. Hoist
such caches to top level, or key them absolutely with `state(arg)`.

### 2. Shadowing a `state var` with a `let` is an internal error — **still open**

```petal
state var hero = 1
let hero = {a: 2}
log(str(hero.a))      // Error: internal error: cell_read on a record
```

Whether the right answer is to shadow or to reject, it should not be an
internal error naming a compiler concept.

### 3. Two mutation models — **still open**

A record held in `state` is mutable through its fields (`s.a = s.a + 1`); the
same record in a `var` demands a whole-value `set`. Defensible once known, but
both spellings appear in the same file and the failure is a compile error in the
middle of writing a helper.

### 4. Record literals cannot carry punctuation or computed keys — **still open**

`{".": 0}` is a parse error, so the art table (`.`, `-`, `o`, `#` to palette
entries) is assembled one `t["."] = 0` statement at a time, and level legends
are a list of pairs instead of the record they want to be. A string-literal
key, and a computed `{[expr]: v}`, would delete both workarounds.

### 5. No hex literals and no bitwise operators — **still open**

Palette indices are written in decimal against hardware docs that are entirely
hex; tracker effect columns had to be redefined as decimal; sprite flags are
additive constants a cart can set but can only test with `(flags / 4) % 2`. The
noise channel's LFSR and packed-2bpp tiles are likewise reachable only through
multiply and modulo.

### 6. `time()` is frozen inside a frame — **still open**

`time()` and `dt()` are bound once before the cart runs, so a cart cannot time
a section of itself; every number in these notes was taken with a stopwatch
outside the process. There is still no monotonic `now()`.

### 7. Some error messages leak the implementation — **still open**

`internal error: cell_read on a record`, `read of an unresolved cell — no
write sites`, `Expected float at arg 1, got string` with no argument name.
Everything else about error reporting (source excerpt, caret, module-qualified
line numbers, stack traces through prelude frames) is excellent, which makes
these stand out.

---

## What was missing

### 8. The core prelude was invisible inside a host prelude module — **fixed**

`has_field` lives in `rust/prelude/std.ptl`, which is merged only when
referenced. The gated declarations used to attach to the entry file alone, so
`sfx_play` compiled clean and raised `Unknown builtin: has_field` at runtime
from inside `nes_sound.ptl`. `module.rs` now binds the gated prelude into every
loaded module (lowest precedence), covered by tests in `rust/tests/modules.rs`.

The lesson that stands: a host prelude authored and tested on the bare `petal`
CLI can differ from what carts see. `tests/carts.rs` exists partly for that.

### 9. No string builder, and no cheap character access — **still open**

The console's hottest loop turns a row of art characters into palette digits,
and every way of writing it allocates: `chars()` builds a list of one-character
strings, `char_at` is slower than that, and `++` in a loop is quadratic. 4,096
characters cost about 1.3 ms of pure interpretation per frame (roughly 0.3 µs
per character; the full table is in the authoring guide's
[Performance](docs/cart-authoring.md#performance) section). A non-allocating
char view and an amortized-constant append are the highest-value additions on
this list.

### 10. Typed arrays stop at `f64` — **still open**

The video path has the same shape as the audio path (64 tiles × 8 rows × 8
small integers) and no typed buffer for it, so tile art is a list of strings.
A `u8_array`, or `f64_array` as the general flat numeric buffer, would let art
be normalized once and pushed by reference.

### 11. No destructuring — **still open**

`let {a, b} = r` is a parse error. Every multi-value return in the prelude is
followed by a line per field. (Lists destructure inside `match`; records do
not.)

### 12. No hot-reload notification for the host — **partly fixed**

`Host::on_program_loaded` now fires when a cart is loaded or switched, but not
on a hot reload of the same file. The audio engine still has to *infer* that a
sound-rendering function changed, by re-rendering a 32-sample window of one
banked sound per frame and comparing (`src/audio.rs`, about 13 µs/frame). A
reload hook or a program revision counter would delete that mechanism.

---

## Host-side (embedding) friction

Not language issues, but they cost the same time:

- **`Host::end_frame(&mut self, env)` gives no `StackKey`**, and `Env` has no
  way to enumerate stacks, so calling a script function from the audio engine
  probes `env.heap_for(StackKey(n))` over a window and takes the highest live
  key (`src/audio.rs`, `resolve_stack`). Passing the stack down, or an
  `Env::main_stack()`, would remove the probe.
- **`Host::after_frame` is only called from the interactive loop**, so
  `launch_cart` silently does nothing in `--agent`, `--headless`,
  `--screenshot` and `--record`. `end_frame` runs in every mode; `after_frame`
  does not.
- **The crate has no `[lib]` target**, so `tests/` cannot import the PPU or APU
  and the unit tests live in `#[cfg(test)]` blocks inside the modules. Fine,
  but a decision each Shape B app re-makes.

---

## Summary

| | |
|---|---|
| Would build a console in Petal again | Yes |
| Was blocking, now fixed | §1 (`state` name collisions), §8 (`std` prelude inside host modules) |
| Highest-value additions still open | a non-allocating string/char API (§9), bitwise ops and hex literals (§5), string-literal record keys (§4) |
| Best surprise | Realtime audio synthesis at 0.32 ms/frame |
| Best-loved feature | The frame model, and the hot reload that falls out of it |
