# Petal, judged by a fantasy console

Notes from building petal-fantasy-nes: a Rust host of about 4 900 lines, a
Petal prelude of about 2 600, and 5 000 lines of carts. Everything above the two
chips — artwork, maps, text, menus, scenes, collision, a FamiTracker-shaped
music driver, and sample synthesis — is written in Petal, so this is a fairly
severe test of the language away from its home ground of "draw a few shapes".

**The verdict: yes, with two caveats.** The frame model is the right model for
this kind of app and the ergonomics of it are genuinely good — the console's
whole authoring surface fell out of the language rather than being imposed on
it. The caveats are that `state` has a name-scoping bug serious enough to
corrupt a cart silently (§1), and that per-character string work costs more
than everything else in a frame put together — which is exactly the work an
art-from-strings console does most of (§9, and "What was slow").

Every snippet below was run on the console (`fantasy-nes --screenshot … --frames
n`); every number was measured, not estimated.

---

## What worked well

### A. Re-run-the-whole-file is the right frame model

The console pushes its entire state every frame — palettes, tiles, map cells,
sprites, chip registers — and Petal's execution model makes that free of
ceremony. There is no `setup()`/`draw()` split to keep consistent, so hot
reload is not a feature anyone implemented: it is what happens when the file
runs again.

```petal
set_backdrop(light_blue)
define_art(1, ["oooooooo", "-o--o-o-", "--------", "---o----",
               "--------", "-------o", "--------", "-o------"])
map_rect(0, 26, 32, 4, 1, 0)
state var x = 120.0
set x = clamp(x + btn_dx() * 1.5, 0, 248)
sprite(px(x), 200, 2, 4)
```

Edit a pixel of that tile while the game runs and it repaints, in place, with
the player where he was. Every other language would need an asset-reload path.

### B. Records as the universal shape

Nothing in the prelude needed a class. `move_box` returns
`{rect, hit_x, hit_y, grounded}`; `menu` returns `{index, chosen, cancelled}`;
`music_pos()` returns `{name, order, row, playing, ticks}`; an instrument is
`{vol, arp, pitch, duty, rel, mode}` with every key optional and `??` filling
the gaps. Multi-value returns cost one line and read at the call site:

```petal
let mv = move_box(hero, btn_dx() * 1.5, vy)
set hero = mv.rect
if mv.hit_y then set vy = 0.0 end
```

Records indexed by a computed string key (`bank[name]`) also stand in for a
map, which is how the sound bank, the menu cursors and the art character table
are stored.

### C. Multi-line strings made data look like the thing

Tile art *is* the picture and a tracker pattern *is* a column of notes. Both
are plain string literals, which is why neither needed an external file format
or a tool:

```petal
let lead = rows("""
  C-4 0 v13
  ...
  E-4
  G-4 0 v10 a47
""")
```

This is the single biggest reason the console has no asset pipeline. It also
means art and music diff cleanly, which for a hot-reloadable file matters more
than it sounds.

### D. Arity overloading, used heavily

`palette(i, c1, c2, c3)` and `palette(i, list)`; `btn(name)` and
`btn(pad, name)`; `sprite(x, y, tile, pal)` with an optional flags argument;
`play_sound(name)` and `play_sound(name, volume)`. The prelude leans on this
constantly and it keeps the cart-facing API to about half the names it would
otherwise need.

### E. Closures and named functions across the FFI

Realtime synthesis is a Petal function the host calls by name, and the block
helpers take a closure:

```petal
fn zap(start, count, rate)
  pcm_render(start, count, rate, fn(t, i) ->
    osc_square(t, 900.0 - 700.0 * t / 0.3, 2) * env_decay(t, 0.3))
end
register_sound("zap", 0.3, "zap")
```

That the host can call *into* a script function mid-frame is what made
`enable_dsp` possible at all.

### F. `f64_array` earns its keep

Realtime audio in an interpreted language sounded implausible and isn't. One
frame of 44.1 kHz stereo synthesized in Petal, measured over 3 000 frames:

| Block written with | ms/frame |
|---|---|
| `f64_array` + indexed writes | 0.32 |
| `pcm_render` (closure per sample) | 0.33 |
| appending to a list | 0.59 |

0.32 ms of a 16.7 ms frame. The typed array is not a micro-optimization here;
it is the difference between "a toy" and "a channel you can ship".

### G. Modules and implicit imports gave a real prelude

`nes.ptl` and `nes_sound.ptl` are 2 600 lines of ordinary Petal registered as
implicit imports, so carts call `draw_meta`, `music_play` and `move_box` bare
and can *read the implementation* when a helper does not do what they want. A
prelude in the host language would have been a wall.

---

## What was awkward

### 1. `state` cells are keyed by name, and the name is not scoped to the function

This is a bug, and it is the most dangerous one found. Two functions in one
file that each declare a `state` variable of the same name share one cell:

```petal
fn hero_timer()
  state var t = 0
  set t = t + 1
  t
end
fn enemy_timer()
  state var t = 100
  set t = t + 1
  t
end
log("hero " ++ str(hero_timer()) ++ "  enemy " ++ str(enemy_timer()))
```

```
hero 1  enemy 2
hero 3  enemy 4
hero 5  enemy 6
```

Two independent timers, one cell, and the `= 100` initializer silently ignored.
Nothing warns. The prelude works around it by prefixing every state variable
`_nes_*` and routing shared state through single-writer accessor functions; the
`petal-ui` prelude has the same hazard and no such convention. A cart that
writes `state var t` in two helpers gets a bug that looks like a game-logic
bug.

The keyed form does the right thing and is the workaround to reach for:

```petal
fn timer(who)
  state(who) var t = 0
  set t = t + 1
  t
end
```

State cells *are* scoped per module (a cart's `state cur` does not collide with
the sound driver's), so the fix looks like extending that scoping down to the
declaration site.

### 2. Shadowing a `state var` with a `let` is an internal error

```petal
state var hero = 1
let hero = {a: 2}
log(str(hero.a))
```

```
Error: internal error: cell_read on a record [line 3, column 9]
Caused by:
  read of an unresolved cell — no write sites
```

Found by accident, while pasting a doc example under a test harness that
already had a `hero`. Whatever the right answer is — shadow, or reject — it
should not be an internal error naming a compiler concept.

### 3. Two mutation models to keep straight

A record held in `state` is mutable through its fields; the same record in a
`var` is not, and demands a whole-value `set`:

```petal
state s = {a: 1}
s.a = s.a + 1        // fine

state var t = {}
t["x"] = 0           // Error: `t` is a `var`; use `set t = ...` to write it
```

The rule is defensible once you know it, but both spellings appear in the same
file, and the failure mode is a compile error in the *middle* of writing a
helper rather than at the declaration you would have to change.

### 4. Record literals cannot carry punctuation or computed keys

`{".": 0}` is a parse error (`Expected an identifier, got a string literal`).
The console's art table maps `.`, `-`, `o`, `#` to palette entries — the most
natural literal in the whole project — and has to be assembled statement by
statement:

```petal
fn _build_art_table()
  let t = {}
  t["."] = 0
  t["-"] = 1
  t["o"] = 2
  t["#"] = 3
  t
end
```

Level legends have the same problem and are passed as a list of pairs
(`[["#", 2], ["=", 1, 1]]`) rather than the record they want to be. Allowing a
string-literal key — and a computed one, `{[expr]: v}` — would delete both
workarounds.

### 5. No hex literals, and no bitwise operators

```petal
1 << 2      // Error: Unexpected token: '<'
6 & 3       // Error: Unexpected character '&'
log(0x1F)   // parse error
```

For a console this bites three times. Palette indices are written in decimal
against hardware documentation that is entirely hexadecimal (`33` for `$21`).
Tracker effect columns had to be redefined as decimal, breaking the muscle
memory of every tracker user. And sprite flags are additive constants
(`flip_x + behind_bg`) that a cart can *set* but cannot *test* without
arithmetic — `(flags / 4) % 2` where every other language writes
`flags & behind_bg`. The packed-2bpp form of `define_tile` is likewise
constructible only by multiplication.

### 6. `time()` is frozen inside a frame, so a cart cannot profile itself

`time()` and `dt()` are bound once before the cart runs, so two calls in one
frame return the same value and every in-cart benchmark reads `0.0 ms`. Every
number in this document had to be taken with a stopwatch outside the process:

```bash
time ./target/release/fantasy-nes --screenshot /tmp/o.png --frames 3300 cart.ptl
```

A monotonic `now()` that is *not* the frame clock would have saved a day, and
is what a cart would want for its own budget display.

### 7. Error messages leak the implementation at exactly the wrong moments

`internal error: cell_read on a record`, `read of an unresolved cell — no write
sites`, `Expected float at arg 1, got string` with no argument name. These land
on cart authors who have never seen the compiler. Everything else about the
error reporting — the source excerpt, the caret, the module-qualified line
numbers, the stack trace through prelude frames — is excellent, which makes the
few leaky messages stand out more.

---

## What was missing

### 8. The core prelude is invisible inside a host prelude module — and it shipped a bug

`has_field` lives in `rust/prelude/std.ptl`. It is a *gated* implicit import
merged into the entry program, but the import list is attached only to the
entry, not to the host's own registered modules. So this works in a cart and
fails inside `nes_sound.ptl`:

```
Error: Unknown builtin: has_field [nes_sound line 801, column 6]
```

The result is that `sfx_play` and `drum` are broken in the shipped console:
sound effects raise on their first call. Nothing in the language flagged it,
because the prelude was authored and tested on the bare `petal` CLI, where
`std` *is* in scope. Two things would prevent a repeat: bind the gated prelude
to every loaded module, not just the entry, and give hosts a way to compile a
registered module in the same environment their carts see.

### 9. No string builder, and no cheap character access

The console's hottest Petal loop turns a row of art characters into a row of
palette digits. Every way of writing it allocates:

| Row conversion, 64 tiles × 8 rows (4 096 characters) | ms/frame |
|---|---|
| `for ch in chars(line)`, `out = out ++ …` | 1.34 |
| `for i in range(…)`, `char_at(line, i)` | 1.59 |
| `join(for ch in chars(line) do … end, "")` | 1.21 |
| the same loop counting characters and nothing else | 0.33 |

`chars()` allocates a list of 4 096 one-character strings; `char_at` is
*slower* than that, so the obvious optimization is a pessimization; `++` in a
loop is quadratic and `join` over a collected list is the fastest of the three.
None of them is fast. What is wanted is a byte/char view that does not
allocate, and a builder (or a `String.repeat`-shaped `build` helper) whose
append is amortized constant.

### 10. Typed arrays stop at `f64`

`f64_array` transformed the audio path (§F). The video path has exactly the
same shape — 64 tiles × 8 rows × 8 pixels of small integers — and has no
equivalent, so tile art is `list` of `string`. A `u8_array`, or just letting
`f64_array` be the general "flat numeric buffer", would let art normalization
be hoisted into a buffer once and pushed by reference.

### 11. No destructuring

```petal
let {a, b} = r     // Error: Expected an identifier, got '{'
```

Still missing, still noted by petal-fps two apps ago. Every multi-value return
in the prelude is followed by three lines of `let x = r.x`.

### 12. No way to ask "has the program been reloaded?"

The audio engine has to know when a cart's sound-rendering function changed, so
it can re-render the bank. There is no revision counter, no reload hook, and
`transfer_state` deliberately preserves the program id — so the host resorts to
re-rendering a 32-sample window of one banked sound per frame and comparing it
against the cached samples (~13 µs/frame) to *infer* an edit. A
`Env::program_revision()` or an `on_program_loaded` that fires with the new
program would delete that whole mechanism.

---

## What was slow

Measured on a release build, M-series laptop, as the delta between 3 300 and
300 headless frames. Frame budget: 16.7 ms.

| Per frame | Cost |
|---|---|
| Backdrop only | 0.04 ms |
| Floor + one walking sprite | 0.08 ms |
| 64 sprites instead of one | +0.03 ms |
| 960 `set_tile` calls (the whole map) | +0.19 ms |
| 64 `define_tile` with normalized rows | +0.03 ms |
| **64 `define_art` from string art** | **+2.0 ms** |
| One frame of Petal-synthesized audio | +0.32 ms |

The shape of this is the story: **native calls and the host are free; Petal
string and list work is the entire cost.** 4 096 characters of art costs 1.3 ms
of pure interpretation — about **0.3 µs per character** — which is 12 % of a
frame to re-read art that has not changed. It is affordable, and it buys hot
reload, so the console keeps paying it and the docs tell authors to hoist only
when their art has settled. But the ratio (one character of Petal ≈ ten native
calls) is the number that decides what can and cannot be written in this
language. It is spread evenly rather than concentrated: iterating those 4 096
characters and only counting them costs 0.33 ms, a quarter of the total, with
the remaining 1.0 ms in the table lookup, the `str()` and the concatenation.
There is no single slow builtin to fix — the interpreter is simply doing a few
million small operations a second, and that is its rate.

---

## Host-side (embedding) friction

Not language issues, but they cost the same time and are worth listing:

- **`Host::end_frame(&mut self, env)` gives no `StackKey`**, and `Env` exposes
  no way to enumerate stacks, so calling a script function from the audio
  engine required probing `env.heap_for(StackKey(n))` over a 64-key window and
  taking the highest live key. Either signature — `end_frame(env, stack)` or
  `Env::main_stack()` — makes the probe unnecessary.
- **`Host::after_frame` is only called from the interactive loop**, so
  `launch_cart` (the boot menu handing over to a cart) silently does nothing in
  `--agent`, `--headless`, `--screenshot` and `--record`. `end_frame` was
  extended to every mode for this project; `after_frame` was not.
- **The crate has no `[lib]` target**, so nothing in `tests/` can import the
  PPU or the APU; all 54 unit tests live in `#[cfg(test)]` blocks inside
  the modules. Fine, but it is a decision each Shape-B app re-makes.

---

## Summary

| | |
|---|---|
| Would build a console in Petal again | Yes |
| Blocking for this class of app | §1 (`state` name collisions), §8 (`std` invisible in host modules) |
| Highest-value additions | A non-allocating string/char API (§9), bitwise ops + hex literals (§5), string-literal record keys (§4) |
| Best surprise | Realtime audio synthesis at 0.32 ms/frame (§F) |
| Best-loved feature | The frame model, and the hot reload that falls out of it (§A) |
