# Writing a cart

Everything a fantasy-nes game is made of — the artwork, the level, the music,
the menus, the gameplay — lives in one `.ptl` file. This guide is the whole
authoring surface, in the order you meet it. No Rust required, and none of it
assumes you have read any.

Every code block below has been run on the console, and the values in the
comments are what it printed. The one thing that does not currently work is
flagged where it appears ([sound effects](#sound-effects-and-drums)).
If anything else here does not work, the doc is wrong, not you.

- [The frame model](#the-frame-model)
- [Colors and palettes](#colors-and-palettes)
- [Tiles: drawing with strings](#tiles-drawing-with-strings)
- [The background map](#the-background-map)
- [Scrolling and the camera](#scrolling-and-the-camera)
- [Sprites](#sprites)
- [Metasprites](#metasprites)
- [Text and the HUD](#text-and-the-hud)
- [Pads](#pads)
- [Scenes](#scenes)
- [Menus](#menus)
- [Collision and movement](#collision-and-movement)
- [Animation and numbers](#animation-and-numbers)
- [Music](#music)
- [Sound effects and drums](#sound-effects-and-drums)
- [Petal-synthesized PCM](#petal-synthesized-pcm)
- [Hot reload while playing](#hot-reload-while-playing)
- [Performance](#performance)
- [API index](#api-index)

## The frame model

**Your cart runs from top to bottom, once per frame, sixty times a second.**
There is no `setup()` and no `draw()`. The file *is* the frame.

That sounds expensive and mostly isn't (see [Performance](#performance)), and
it buys the thing that makes this console pleasant: because the whole program
re-runs, editing the file while the game is running just works.

Three consequences shape every cart:

1. **Anything you want to keep between frames goes in `state`.** A plain `let`
   is born and dies inside one frame.

   ```petal
   state var score = 0        // survives frames *and* file edits
   let elapsed = time()       // recomputed every frame, and that is fine
   ```

   A `state` cell is identified by its **name**, and by nothing else, so two
   functions in one cart that both say `state var t` are writing the same
   cell. Give per-object state an explicit key instead — `state(key) var` makes
   one cell per key, created on first use:

   ```petal
   fn timer(who)
     state(who) var t = 0
     set t = t + 1
     t
   end
   timer("hero")     // 1, 2, 3, ...   independent of
   timer("enemy")    // 1, 2, 3, ...   this one
   ```

2. **Sprites are cleared every frame; the map and the pattern table are not.**
   A `sprite(...)` call means "draw this now". A `set_tile(...)` or
   `define_art(...)` call means "the console should look like this from now
   on" — so those calls are idempotent and cost nothing when nothing changed.
   Leaving them at the top of the file, running every frame, is the intended
   style: it is what makes an artwork edit appear instantly on reload.

3. **Order inside the frame does not matter to the picture.** The console
   collects everything the cart said and draws once at the end. Sprites are
   composited in the order you pushed them (earlier = in front).

Here is a complete cart. It is the same one in the [README](../README.md):

```petal
set_backdrop(light_blue)                       // the sky — every palette's color 0
palette(0, dark_green, green, white)           // background palette 0
palette(4, maroon, red, peach)                 // sprite palette 4

define_art(1, ["oooooooo", "-o--o-o-", "--------", "---o----",     // grass
               "--------", "-------o", "--------", "-o------"])
define_art(2, ["..----..", ".-####-.", "-#-##-#-", "-######-",     // the hero
               "-#-##-#-", ".-####-.", "..oooo..", ".o.oo.o."])

set_map_size(32, 30)
map_rect(0, 26, 32, 4, 1, 0)                   // a floor along the bottom four rows

state var x = 120.0
set x = clamp(x + btn_dx() * 1.5, 0, 248)      // arrows / WASD / a d-pad

text_sprites_center(24, "HELLO", 4)
sprite(px(x), 200, 2, 4)
```

Everything after this is that cart, grown.

## Colors and palettes

There is no RGB. A color is an index into a fixed 64-entry master palette, and
the console has eight four-color palettes: **0–3 for the background, 4–7 for
sprites**. Color 0 of all eight is the same shared *backdrop*, which is the
screen behind everything and the transparent color of every sprite.

That is the whole constraint, and it is the one that makes the picture look
like an NES game.

```petal
set_backdrop(light_blue)                 // color 0, everywhere
palette(0, dark_green, green, white)     // background palette 0: the other three
palette(1, brown, orange, peach)
palette(4, maroon, red, white)           // sprite palette 4
palette(5, [black, gray, white])         // the list form, for palettes kept as data
```

`palette(i, c1, c2, c3)` fills color 0 in for you from the current backdrop, so
call `set_backdrop` **first** — at the top of the cart, where it belongs.

The named colors (`light_blue`, `maroon`, `peach`, …) are the master-palette
entries worth naming: greys, then blue/indigo/purple/red/brown/olive/green/teal
families, each in four brightnesses — `navy blue light_blue pale_blue`, and so
on. `carts/palette.ptl` shows all 64 with their numbers. Raw indices work
anywhere a name does; Petal has no hex literals, so `33` is what a palette chart
calls `$21`.

`master_rgb(c)` returns the packed `0xRRGGBB` of a master-palette entry, for a
cart that wants to compute something from the actual color:

```petal
log("sky rgb: " ++ str(master_rgb(light_blue)))     // 5020396
```

Fades and color cycles are just palette writes with a different argument each
frame — the palette is re-pushed every frame anyway.

## Tiles: drawing with strings

A tile is 8×8 pixels, 2 bits per pixel, and there are 512 of them. You draw one
by writing it:

```petal
define_art(2, [
  "..----..",
  ".-####-.",
  "-#-##-#-",
  "-######-",
  "-#-##-#-",
  ".-####-.",
  "..oooo..",
  ".o.oo.o."
])
```

One character per pixel, and the character says *which entry of the palette*,
not which color — the same tile drawn with palette 0 and palette 4 comes out in
two different color schemes.

| Character | Palette entry | |
|---|---|---|
| `.` `_` `0` or a space | 0 | the backdrop; **transparent** for a sprite |
| `-` `1` | 1 | darkest ink |
| `o` `2` | 2 | mid |
| `#` `3` | 3 | brightest ink |

The symbol spellings and the digit spellings are identical in effect; symbols
are ordered by visual weight so the source looks like the picture. Anything else
reads as 0. Short rows are padded and a short list is padded to 8 rows, so
sketching only the top of a tile works.

Your own characters, via a remap record:

```petal
define_art(3, [
  "xxxxxxxx",
  "x......x"
], {x: 3})
```

A **run** of tiles at once — and the return value is the next free index, so a
cart lays out its pattern table by composition instead of by counting:

```petal
let walk_a = ["..##....", ".####...", "..##...."]
let walk_b = ["...##...", "..####..", "...##..."]
let hero_base = 8
let next_free = define_art_run(hero_base, [walk_a, walk_b])   // -> 10
```

`art(rows)` / `art(rows, remap)` does the normalization *without* defining
anything, which is what you want when building a tile programmatically or when
[hoisting art out of the frame](#performance).

The raw native underneath is `define_tile(index, rows)`, which takes eight
strings of eight digits, or eight packed 2bpp ints. `define_art` is that plus
the character table.

External art is supported but optional:

```petal
load_tiles_png("art/tiles.png", 32)      // cut 8x8 left-to-right, top-to-bottom, from tile 32
```

Alpha below 128 becomes color 0; the opaque colors take entries 1–3 in order of
first appearance across the whole image. Decoding is cached on the file's
mtime, so calling it every frame is cheap and editing the PNG reloads it.

## The background map

The map is a grid of cells, each naming a tile and a palette. Up to 64×60
cells; 32×30 is exactly one screen.

```petal
set_map_size(64, 30)              // 512 x 240 pixels of world
fill_map(0, 0)                    // every cell -> tile 0, palette 0
map_rect(0, 26, 64, 4, 2, 0)      // x_cell, y_cell, w_cells, h_cells, tile, palette
map_row(4, 25, [1, 1, 1, 2], 1)   // a row of explicit tile indices
set_tile(10, 12, 3, 1)            // one cell
```

Levels are authored as string art too, through a legend:

```petal
map_art(2, 20, [
  "...===...",
  "..#####..",
  "#########"
], [
  ["#", 2],           // char, tile        (palette 0)
  ["=", 1, 1]         // char, tile, palette
])
```

A character the legend does not mention is **left untouched**, so `.` means "not
mine" and several `map_art` calls layer cleanly — terrain, then decoration, then
a doorway — without the later ones erasing the earlier.

`get_tile(x_cell, y_cell)` reads a cell back, including cells written earlier in
this same frame; that is what the collision helpers are built on.

The legend is a list of entries rather than a record because Petal record
literals cannot carry punctuation keys — `{".": 0}` is a parse error.

## Scrolling and the camera

`set_scroll(x, y)` says which world pixel sits at the top-left of the screen,
and the map **wraps**: a camera that counts up forever scrolls a 64-cell map
past the window with no seam and no bookkeeping.

Most games want the clamped version instead:

```petal
camera_follow(center_x(hero), center_y(hero))   // centers, clamped to the map edges
camera(x, y)                                    // top-left, clamped; returns where it landed
cam_x()  cam_y()                                // read it back
```

With a camera in play, draw in **world** coordinates and let the helpers
subtract it:

```petal
sprite_world(140, 180, 1, 4)             // same as sprite(), but world-space
draw_meta_world(80, 160, hero)
let s = world_to_screen(300, 10)         // {x, y}, when you need it by hand

if on_screen(rect(ex, ey, 16, 16)) then  // the cheap cull before spending a sprite
  sprite_world(ex, ey, enemy_tile, 5)
end
```

**Status-bar split.** `set_scroll_at(scanline, x)` overrides the horizontal
scroll for one scanline. `lock_top(height_px)` is the common case — freeze the
top N scanlines at x = 0 while the rest of the screen scrolls:

```petal
camera_follow(hero.x, hero.y)
lock_top(16)                             // the top two rows are now a HUD
```

Per-scanline scroll is also how parallax works: give each band of scanlines a
different fraction of the camera.

```petal
for y in range(16, 240) do
  set_scroll_at(y, int(cam_x() * (if y < 120 then 0.25 else 1.0 end)))
end
```

(240 `set_scroll_at` calls per frame is affordable — see
[Performance](#performance) — but a handful of bands is cheaper and looks the
same.)

## Sprites

Up to 64 per frame, cleared and re-pushed every frame.

```petal
sprite(x, y, tile, palette)
sprite(x, y, tile, palette, flags)
```

Positions are screen pixels. A float is accepted and lands on a whole pixel
anyway, so pass it through `px(v)` and decide the rounding yourself: `px` floors
(rather than rounds, so a slow-moving object never jitters back and forth across
a pixel boundary, and so it stays correct left of x = 0). Palettes 4–7 are the
sprite palettes; color 0 is transparent.

Flags are bits:

| Constant | Value | |
|---|---|---|
| `flip_x` | 1 | mirror horizontally |
| `flip_y` | 2 | mirror vertically |
| `behind_bg` | 4 | draw behind non-transparent background pixels |

```petal
sprite(px(x), px(y), tile, 4, flip(facing_left, false))   // flags from two booleans
sprite(px(x), px(y), tile, 4, flip_x + behind_bg)         // or add them
```

Earlier sprites draw in front of later ones, which is also the priority order
the drop-out uses:

```petal
set_sprite_limit(true)      // emulate the 8-sprites-per-scanline drop-out (default: off)
```

Leave the limit off unless flicker is the look you want; `carts/sprites.ptl`
shows both.

## Metasprites

Anything bigger than 8×8 is several sprites moving together. A metasprite is
one value describing that group, so gameplay code carries a position and an
object rather than a list of tile offsets.

The usual path is to draw the whole object as one grid and let the console slice
it into tiles:

```petal
let hero = define_meta(16, [
  "..############..",
  ".##############.",
  "################",
  "###--######--###",
  "################",
  "..############..",
  "..##########....",
  "....######......",
  "......####......",
  "......####......",
  "......####......",
  "........##......",
  "................",
  "................",
  "................",
  "................"
], 4)                                  // tiles 16..19, palette 4

draw_meta(hero_x, hero_y, hero, flip(facing_left, false))
draw_meta_world(hero_x, hero_y, hero)  // camera-relative
```

`define_meta` returns the metasprite and defines its tiles, so it lives at the
top of the cart with the rest of the artwork. `meta_w(m)`, `meta_h(m)` and
`meta_rect(x, y, m)` give its size and the box the collision helpers want.

For tiles you already defined, or an irregular shape:

```petal
let coin = meta(1, 1, 1, 4)             // tile_base, w_tiles, h_tiles, palette

let boss = meta_of([
  meta_part(0, 0, 1, 4),                // dx, dy, tile [, palette [, flags]]
  meta_part(8, 0, 1, 4, flip_x),
  meta_part(4, 8, 2, 5)                 // a piece in a different palette
])
```

Flipping a metasprite mirrors the *arrangement* as well as each tile, so a
flipped object faces the other way instead of turning inside out.

A metasprite costs one sprite per tile out of the 64. A 16×16 hero is four.

## Text and the HUD

The console has a built-in 64-glyph font (ASCII 32–95: uppercase, digits,
punctuation — lowercase input is folded up). It lives in the pattern table like
all other art, at tile 448 by default, and installs itself the first time you
draw with it. There is no setup call.

Two ways to put words on screen, and the choice matters:

```petal
map_text(1, 1, "SCORE " ++ pad_num(1234, 6), 0)   // into the background map: free, but it scrolls
text_sprites(8, 4, "HP " ++ pad_num(3, 2), 4)     // as sprites: floats over everything, costs sprites
```

`map_text` writes glyph tiles into the map. It costs no sprites, it scrolls
with the map (pair it with `lock_top` for a status bar), and it is *permanent*:
it writes exactly as many cells as the string is long and nothing more. A space
is a real glyph and does overwrite, but a shorter string does **not** erase the
tail of a longer one that was there before — write `"SCORE " ++ pad_num(s, 6)`
or a space-padded field of fixed width, so a live counter always covers its own
widest value.

`text_sprites` spends one of the 64 sprites per non-blank character and returns
how many it used, so a HUD can stay inside the budget deliberately rather than
by luck.

Alignment and layout:

```petal
map_text_center(3, "READY", 0)
map_text_right(30, 1, "TIME 30", 0)
text_sprites_center(210, "PRESS START", 4)
text_sprites_right(248, 220, "X3", 4)

text_cells("SCORE")        // 5   — the font is fixed-width, so these are exact
text_px("PRESS START")     // 88
pad_num(7, 3)              // "007"
```

To move the font out of the way of your own artwork, or to change which palette
entry it inks:

```petal
let next = font_tiles(400, 3)     // 64 glyphs at 400.., inked with entry 3; returns 464
font_base()                       // 400
glyph("A")                        // the tile index for one character
```

## Pads

Two 8-button pads: `"up" "down" "left" "right" "a" "b" "select" "start"`.

```petal
btn("a")                    // held, pad 0
btnp("a")                   // pressed this frame  (the one you want for jumps and menus)
btnr("a")                   // released this frame
btn(1, "a")                 // pad 1
btn_dx()                    // -1 / 0 / +1 from left+right; opposites cancel
btn_dy()
btn_repeat("down", 0.35, 0.12)   // once on press, then every 0.12s after holding 0.35s
```

`btn_repeat` is the menu/scroll input: the delay and rate are in seconds, and
the timer lives in console state keyed by (pad, button), so calling it
unconditionally every frame is correct.

The raw natives are `pad_down(pad, button)`, `pad_pressed(...)`,
`pad_released(...)` — the `btn*` helpers are exactly these with pad 0 defaulted.
An unknown button name is an error (it is nearly always a typo); an out-of-range
pad number is too.

Keyboard mapping, and gamepads, are in the [README](../README.md#controls).
`petal-ui`'s own natives (`key_down`, `mouse_x`, `dt`, `time`, `frame_count`)
are all still available if you want them.

## Scenes

A cart is title → game → game over, and since the whole file re-runs every
frame, the structure that fits is a scene name plus an "entered" edge:

```petal
start_scene("title")                  // the initial scene; safe to run every frame

if scene() == "title" then
  map_text_center(10, "PETAL QUEST", 0)
  if btnp("start") then set_scene("game") end

elsif scene() == "game" then
  if scene_entered() then             // true for exactly the first frame of the scene
    fill_map(0, 0)
    map_rect(0, 24, 32, 6, 1, 0)
  end
  // ... gameplay ...
  if btnp("start") then set_scene("paused") end

elsif scene() == "paused" then
  text_sprites_center(112, "PAUSED", 4)
  if scene_time() > 0.5 && btnp("start") then set_scene("game") end
end
```

`set_scene` takes effect on the **next** frame: the rest of this frame belongs
to the scene that is already drawing, and the new scene gets a clean frame that
starts with `scene_entered()` true. Per-scene setup — repaint the map, reset the
score, reposition the player — goes behind that check. Switching to the scene
you are already in re-enters it, so "restart level" is `set_scene(scene())`.

`scene_time()` (seconds) and `scene_frames()` measure since the scene began:
splash-screen holds, respawn delays, a "GAME OVER" that waits before accepting
input.

## Menus

```petal
let m = menu("main", 80, 100, ["START", "OPTIONS", "QUIT"], 4)
if m.chosen == 0 then set_scene("game") end
if m.cancelled then set_scene("title") end
```

`menu` draws with sprites; `map_menu(id, x_cell, y_cell, items, pal)` draws into
the background map instead (no sprite cost, but it scrolls — use it on a static
title screen). Both return `{index, chosen, cancelled}`: where the cursor is,
the index picked *this frame* or `-1`, and whether B was pressed. Up/down
auto-repeat and wrap.

The `id` names the menu, so several can coexist; each keeps its cursor across
frames and reloads. `menu_index(id)` reads a cursor without moving it, and
`menu_update(id, count)` is the input half alone, for a menu you draw yourself.

## Collision and movement

Which tiles are walls is a cart decision, declared as index ranges. Nothing is
solid until you say so:

```petal
set_solid(1, 15)         // replaces the whole set — idempotent, so it belongs at the top
add_solid(40, 42)        // ...and these too
```

```petal
solid_tile(3)                    // is this tile index solid?
solid_at(x, y)                   // is the map solid at this world pixel?
solid_box(rect(x, y, w, h))      // does any solid tile touch this box?
on_ground(hero)                  // is there solid ground directly under it?
```

Outside the map counts as solid, so a level does not need a wall of tiles around
its edge to be enclosed.

The workhorse is `move_box`, which moves a rect and stops it against the
terrain:

```petal
state var hero = rect(32, 100, 8, 8)
state var vy = 0.0

set vy = min(4.0, vy + 12.0 * dt())        // gravity, terminal velocity
if on_ground(hero) then
  set vy = 0.0
  if btnp("a") then set vy = -3.0 end      // jump
end

let mv = move_box(hero, btn_dx() * 1.5, vy)
set hero = mv.rect
if mv.hit_y then set vy = 0.0 end          // hit the ceiling or landed: kill the velocity
```

It returns `{rect, hit_x, hit_y, grounded}` and resolves X then Y in steps of at
most one pixel — the classic arrangement, and what lets a box slide along a wall
instead of sticking to it. A box that starts *inside* a wall cannot move at all,
so spawn clear of the terrain.

Box geometry, for everything that is not terrain:

```petal
overlaps(hero, coin)                          // rects
overlaps(ax, ay, aw, ah, bx, by, bw, bh)      // or eight loose numbers
point_in(120, 60, hero)
inflate(hero, -2)                             // a forgiving hitbox
inflate(coin, 4)                              // a generous pickup radius
center_x(r)  center_y(r)
```

`rect(x, y, w, h)` is the built-in `Rect`, so a hand-written
`{x: .., y: .., w: .., h: ..}` works anywhere a rect is taken.

## Animation and numbers

```petal
sprite(24, 100, anim([8, 9, 10, 9], 8.0), 4)          // loop at 8 fps off the wall clock
sprite(40, 100, anim_once([1, 2, 3], 6.0, t), 4)      // play once from t = 0, hold the last
if blink(2.0) then text_sprites_center(200, "PRESS START", 4) end
```

`anim` takes any list — tile indices, metasprites, whole records — so the same
helper drives a walk cycle and a palette flash. Its clock is absolute rather
than accumulated, so two objects sharing a cycle stay in step and nothing
drifts. Pass your own clock (`scene_time()`, a per-entity timer) when an
animation must start at zero on an event.

Movement is written in floats and becomes whole pixels only where it meets the
hardware:

```petal
px(3.7)                    // 3    — floor, for sprite positions
cell_of(-1)                // -1   — pixel -> cell, correct for negatives
cell_to_px(3)              // 24
pmod(-1, 4)                // 3    — positive modulo, for wrapping cursors and scroll
approach(v, 0.0, 0.25)     // move toward a target by at most a step, landing exactly on it
```

## Music

The music module is FamiTracker-shaped, because that is the shape the hardware
suggests: **instruments are envelopes stepped once per frame**, **patterns are
lists of rows**, and an **order** says which pattern each channel plays in each
frame of the song.

The cart's side of it is three lines:

```petal
music_play(song_title())                       // idempotent — safe every frame
if btnp("a") then sfx_play("jump") end
music_tick()                                   // exactly once per frame, near the end
```

`music_tick()` is the only thing that writes the chip. Call it unconditionally,
even on a paused frame; calling it twice in one frame is a no-op.

Two complete songs ship with the console — `song_title()` and
`song_gameplay()` — and `music_play` restarts only when handed a song with a
different `name`, which is what makes it safe at the top of a file that re-runs
60 times a second.

`carts/music_demo.ptl` draws the driver's own playhead — the pattern grid, the
row it is on, and what each channel resolved to — which is the fastest way to
see whether a song of yours is doing what you meant.

### Writing your own

Notes are MIDI semitones (60 is middle C, 69 is A440), written the way a tracker
writes them:

```petal
note("C-4")        // 60      "A#3", "Db5" and "C4" all work
note_name(60)      // "C-4"
note_hz(69)        // 440.0
transpose(60, 12)  // 72
octave(4)          // 60      — C in that octave
```

Two spellings are not notes: `"..."` holds (nothing happens on this row) and
`"---"` releases.

A pattern is written as tracker text:

```petal
let lead = rows("""
  C-4 0 v13        // note, instrument 0, volume 13
  ...              // hold
  E-4              // same instrument and volume as before
  G-4 0 v10 a47    // arpeggio: 0, +4, +7 semitones, one per frame
  ---              // release
  A-4 0 v12 p8     // portamento in at 8/16 semitone per frame
  ...
  ...
""")
```

Columns after the note may appear in any order: a bare number is the instrument,
`v<n>` is volume 0–15, and `<letter><n>` is an effect. Blank lines are skipped
and `//` starts a comment — an *empty row* must be written `...`.

| Effect | |
|---|---|
| `a<xy>` | arpeggio: cycle 0, +x, +y semitones every frame (`a47` is a major chord) |
| `p<n>` | portamento: glide to this row's note at n/16 semitone per frame |
| `s<n>` / `d<n>` | slide up / down n/16 semitone per frame |
| `m<xy>` | vibrato, speed x depth y |
| `t<n>` | set ticks-per-row from here on (tempo change) |
| `b<n>` | break: jump to the next order frame after this row |

Digits are decimal, not FamiTracker's hex — Petal has no hex literals. An effect
persists on its channel until another replaces it, so `m00`, `p00` and `a00`
turn one off.

Instruments are four optional envelopes and two scalars, stepped one entry per
frame and holding on the last value (or looping, if you say where):

```petal
let lead2 = instrument({
  vol: [13, 15, 12, 10],        // 0-15, scaled by the row's volume column
  arp: chord_arp([0, 3, 7]),    // a looping minor triad
  duty: [2, 1],                 // pulse duty 0-3 (12.5% 25% 50% 75%)
  rel: [6, 3, 0]                // the volume envelope used after a "---" row
})

envelope([15, 12, 10])          // an explicit envelope; a bare list means the same
envelope([12, 10, 8, 10], 1)    // ...looping back to index 1 forever
env_at(e, 4)                    // read one, for a debug overlay
```

`pitch:` is a detune envelope in 1/16 semitones (a blip's chirp, a slow drift)
and `mode:` is the noise channel's 0 long / 1 short.

A song ties it together:

```petal
let bass = rows("""
  A-2 1
  ...
  A-2 1
  ...
""")

let beat = rows("""
  C-2 4 v15
  ...
  A-2 5 v12
  ...
""")

let tune = song({
  name: "level1",                             // identity: what makes music_play idempotent
  bpm: 140, rows_per_beat: 4,                 // or: ticks: 6  (frames per row)
  instruments: standard_instruments(),        // indexed by a row's instrument column
  patterns: [lead, bass, beat],
  order: [song_frame(0, -1, 1, 2)],           // pattern per channel: p1, p2, tri, noise
  loop_at: 0                                  // order index to repeat from; -1 plays once
})

music_play(tune)
```

`-1` in an order frame silences that channel for that stretch. A shorter pattern
simply runs out and its channel holds, which is how a 4-row drum loop rides
under a 64-row melody.

`standard_instruments()` is the stock set the shipped songs use:
`0` lead, `1` bass, `2` pluck, `3` triangle bass, `4` kick, `5` snare, `6` hat.

Transport:

```petal
music_play(tune, 2)     // force a (re)start at order index 2
music_restart()
music_stop()   music_pause()   music_resume()
music_playing()
music_pos()             // {name, order, row, playing, ticks}
```

The playhead lives in console state, so a hot reload mid-song picks up on the
same row — and because the song *data* is re-read on every `music_play`, editing
a pattern and saving is audible on the next row without losing your place.

The chip is also directly available if you would rather write the registers
yourself: `apu_pulse(ch, note, duty, volume)`, `apu_triangle(note, on)`,
`apu_noise(period, volume, mode)`, `apu_mute()`. `note` is a float, so pitch
bends are free. A cart that does that must not also call `music_tick()`, which
writes all four channels every frame.

## Sound effects and drums

> **Known bug, as this is written.** `sfx_play` and `drum` raise
> `Unknown builtin: has_field [nes_sound line 801]`. `has_field` comes from
> Petal's core `std` prelude, which is merged into the cart but *not* into a
> host prelude module, so `nes_sound.ptl` cannot see it — the same call works
> fine from a cart. Everything else in this section is real: the bank, the
> priority rule and the channel policy are all implemented. The fix is for the
> prelude to spell the question in true builtins — `contains(keys(rec), key)`,
> or `rec[key] ?? nil` where nil-valued keys do not occur. Until then, an
> effect can be fired by writing the chip directly (`apu_noise`, `apu_pulse`),
> the way `sound_lab.ptl` does.

An effect is a one-shot instrument at a fixed pitch:

```petal
sfx_standard()                        // jump, coin, hit, explode, select + a drum kit

sfx_def("laser", {ch: "p2", note: "G-5", pri: 3,
                  vol: [12, 11, 9, 7, 5, 3, 1, 0],
                  pitch: [0, -8, -16, -24, -32, -40, -48, -56],
                  duty: [1]})

if btnp("a") then sfx_play("laser") end
if btnp("b") then drum("kick") end
sfx_stop("p2")     // hand a channel back to the music immediately
sfx_stop_all()
```

`ch` is `"p1"`, `"p2"`, `"tri"` or `"noise"`. An effect **borrows** its channel
for its duration, writing over the music there and handing it back the moment it
ends — the music voice keeps running underneath, so the melody resumes
mid-phrase instead of restarting.

`pri` (default 1) settles collisions: a new effect takes the channel unless the
one playing has a *strictly higher* priority. So equal priorities retrigger
(rapid pickups sound like rapid pickups) and a footstep can never interrupt an
explosion. Nothing is queued — an effect that loses is dropped, which at 60 fps
is the right answer.

The conventions worth keeping, because the hardware forced them: **pulse 2** is
the effects channel (losing the melody is what players notice), **noise** is
percussion and explosions, and **triangle** stays on the bass except for the
short blip half of a kick drum. `drum("kick")` plays the noise half and the
triangle half together; `drum_instruments()` returns the same three sounds as
*instruments*, for a song's noise pattern.

## Petal-synthesized PCM

Beyond the four chip channels there is a sample channel, and the samples are
synthesized **in Petal**.

### Rendered ahead of time

```petal
fn zap(start, count, rate)
  pcm_render(start, count, rate, fn(t, i) ->
    osc_square(t, 900.0 - 700.0 * t / 0.3, 2) * env_decay(t, 0.3))
end
register_sound("zap", 0.3, "zap")        // name, seconds, function name

if btnp("start") then play_sound("zap") end
play_sound("zap", 0.4)                   // at 40% volume
stop_sound("zap")
```

The host calls your function as `fn_name(start_sample, count, sample_rate)` in
blocks and caches the result; it re-renders automatically when you edit the
function. Return a list of floats in −1..1, or an `f64_array`, which is faster:

```petal
fn thud(start, count, rate)
  let out = f64_array(count)
  for k in range(0, count) do
    let t = pcm_time(start + k, rate)
    out[k] = (osc_sine(t, 90.0) * 0.7 + osc_noise(start + k) * 0.3) * env_ad(t, 0.005, 0.25)
  end
  out
end
register_sound("thud", 0.25, "thud")
```

Blocks must be **stateless**: your function gets the absolute sample index, and
the same index must always produce the same sample. That is why `osc_noise(i)`
hashes the index instead of using `rand()`.

The toolkit:

| | |
|---|---|
| `pcm_time(i, rate)` | seconds at absolute sample `i` |
| `pcm_render(start, count, rate, f)` | build a block; `f(t_seconds, sample_index)` |
| `osc_sine(t, hz)` `osc_saw` `osc_tri` | basic oscillators |
| `osc_square(t, hz, duty)` | duty is the chip's 0–3, so it can match a pulse instrument |
| `osc_noise(i)` | deterministic white noise |
| `env_decay(t, secs)` | exponential decay — most short effects are an oscillator times one of these |
| `env_ad(t, a, d)`, `env_adsr(t, a, d, s, r, dur)` | the longer envelopes |
| `pcm_scale(xs, g)`, `pcm_mix(a, b)`, `pcm_clip(xs)` | block arithmetic |

### Realtime

`enable_dsp(fn_name)` opts into synthesizing the *next frame's* samples every
frame, mixed on top of the chip. Same block signature.

```petal
fn drone(start, count, rate)
  let out = f64_array(count)
  for k in range(0, count) do
    out[k] = osc_tri(pcm_time(start + k, rate), 55.0) * 0.15
  end
  out
end

enable_dsp("drone")        // enable_dsp("") turns it off
log(str(dsp_cost_ms()))    // what the last call cost, in milliseconds
```

This is real Petal running inside the audio path, which is possible only because
the console queues audio from the main thread rather than from a callback
thread. It is also the one place a cart can spend its way out of a frame: the
host measures every call and, if the cost stays over budget for several frames
running, fades the DSP bus out rather than glitching the picture. It re-measures
periodically and fades back in if you fixed it. Watch `dsp_cost_ms()` while you
develop; the budget is a fraction of a 16.7 ms frame.

The DSP bus is mono, mixed into both channels.

`carts/dsp_lab.ptl` runs both paths side by side with the cost of each on
screen; `carts/sound_lab.ptl` does the same for the four chip channels.

## Hot reload while playing

Save the file and the running console picks it up on the next frame. What
happens then:

- **`state` survives.** Score, player position, enemy list, the music playhead,
  menu cursors. State is keyed by name, so renaming a state variable resets it.
- **Artwork, palettes and the map are re-pushed**, because your cart pushes them
  every frame. Edit a tile and it repaints in place — including the tiles
  already on screen, since the map holds *indices*, not pixels.
- **Sprites and chip writes** are per-frame anyway, so there is nothing to
  reconcile.
- **A syntax error** stops the reload, not the game: the console keeps running
  the last version that compiled and prints the error.

This is the main reason to keep artwork and level layout at the top of the cart
rather than behind an "only once" guard. The thing you cache for speed is the
thing you can no longer edit live — see below.

## Performance

Numbers below are measured, not estimated: release build on an M-series
laptop, `--screenshot --frames 3300` minus `--frames 300` over 3000 frames, so
process startup falls out. They cover the cart plus the host's command drain
plus the sound mixer; rasterizing and presenting is separate and is not your
bottleneck. One frame's budget is **16.7 ms**.

| Per frame | Cost |
|---|---|
| A cart that only sets the backdrop | 0.04 ms |
| A cart that draws a floor and walks one sprite | 0.08 ms |
| 64 sprites instead of one | +0.03 ms |
| Rewriting all 960 map cells with `set_tile` | +0.19 ms |
| 64 `define_tile` calls with already-normalized rows | +0.03 ms (the host hashes and skips the decode) |
| **64 `define_art` calls from string art** | **+2.0 ms** |
| An `enable_dsp` block written into an `f64_array` | +0.32 ms |
| The same block built with `pcm_render` (closure) | +0.33 ms |
| The same block built by appending to a list | +0.59 ms |

What that says:

**The console is cheap. Petal string and list work is what costs.** Native
calls, map writes, sprite pushes and the rasterizer are all effectively free at
these scales. Converting string art into tile rows is not: it is per-character
Petal work, about 0.03 ms per tile per frame. The underlying rate is roughly
**0.3 µs per character** — 4096 characters (64 tiles) is 1.3 ms of pure
interpretation before any host call happens, and that rate is the yardstick for
any other per-cell loop you are thinking of writing.

So the one thing worth hoisting is **art normalization** — and only once your
art has stopped changing, because hoisting it is exactly what stops hot reload
from repainting it:

```petal
// While drawing: re-normalized every frame, so an edit shows up instantly.
define_art(1, ["oooooooo", "-o--o-o-", "--------", "---o----",
               "--------", "-------o", "--------", "-o------"])

// Once it is settled: normalize once, push every frame (still idempotent,
// still correct, and now free).
state var floor_art = art(["oooooooo", "-o--o-o-", "--------", "---o----",
                           "--------", "-------o", "--------", "-o------"])
define_tile(1, floor_art)
```

A `state` initializer is evaluated only on the first frame, so the list literal
and the conversion happen once and the reload keeps the cached value.

The same trick applies to anything expensive that does not change: a level laid
out with `map_art` over a large area, a lookup table, a generated palette ramp.
Everything else — the map, the sprites, the pads, the music tick — is fine to
run flat out every frame, which is what the whole prelude is written to assume.

Two budgets are hard rather than soft:

- **64 sprites per frame.** `text_sprites` returns how many it spent; a 16×16
  metasprite costs 4. Past 64 the extras are dropped.
- **The DSP slice.** `dsp_cost_ms()` against a 16.7 ms frame, with the host
  fading the bus out if you overrun persistently.

If a cart does get slow, the profile is almost always a per-pixel or per-cell
loop in Petal. Move the work to a `state` cache, do it over fewer cells, or push
it into a tile.

**Measuring it.** You cannot time a section of your own cart from inside it:
`time()` and `dt()` are the *frame's* clock, bound once before the cart runs, so
two `time()` calls in one frame return the same number. Measure from outside
instead — run the same cart with and without the suspect block and compare wall
clock over a few thousand headless frames:

```bash
time ./target/release/fantasy-nes --screenshot /tmp/out.png --frames 3300 carts/mine.ptl
```

The two audio paths report their own cost, because there the host is holding
the stopwatch: `dsp_cost_ms()` for the realtime bus, and `dsp_lab.ptl` times an
ahead-of-time `register_sound` render on demand.

## API index

Natives (always available, with or without the prelude):

```
nes_version()  log(msg)  set_scale(n)  set_crt(on)
cart_count()  cart_name(i)  cart_path(i)  launch_cart(path)
set_palette(i, c0, c1, c2, c3)  set_backdrop(c)  master_rgb(c)
define_tile(i, rows)  define_tiles(base, list)  load_tiles_png(path, base)
set_map_size(w, h)  set_tile(x, y, tile, pal)  get_tile(x, y)  fill_map(tile, pal)
set_scroll(x, y)  set_scroll_at(scanline, x)
sprite(x, y, tile, pal[, flags])  sprite_meta(x, y, base, pal, w, h, flags)
set_sprite_limit(on)
pad_down(pad, btn)  pad_pressed(pad, btn)  pad_released(pad, btn)
apu_pulse(ch, note, duty, vol)  apu_triangle(note, on)  apu_noise(period, vol, mode)
apu_mute()
register_sound(name, secs, fn)  play_sound(name[, vol])  stop_sound(name)
enable_dsp(fn)  dsp_cost_ms()
```

Prelude module `nes` — screen constants `screen_w screen_h tile_size
screen_cols screen_rows rect`; the master-palette color names; `set_backdrop
backdrop palette`; `art define_art define_art_run`; `flip_x flip_y behind_bg
flip`; `meta meta_part meta_of define_meta meta_w meta_h meta_rect draw_meta
draw_meta_world`; `font_tiles font_base glyph map_text map_text_center
map_text_right text_sprites text_sprites_center text_sprites_right text_cells
text_px pad_num`; `set_map_size map_cols map_rows map_w map_h map_rect map_row
map_art`; `set_scroll cam_x cam_y camera camera_follow world_to_screen
sprite_world on_screen lock_top`; `btn btnp btnr btn_dx btn_dy btn_repeat`;
`start_scene scene set_scene scene_entered scene_time scene_frames`;
`menu map_menu menu_update menu_index`; `point_in overlaps inflate center_x
center_y`; `set_solid add_solid solid_tile solid_at solid_box move_box
on_ground`; `anim anim_once blink`; `px cell_of cell_to_px pmod approach`.

Prelude module `nes_sound` — `note note_name note_hz transpose transpose_rows
octave noise_period note_none note_off middle_c a440`; `envelope env_at env_len
chord_arp instrument`; `row rows song_frame song`; `music_play music_restart
music_stop music_pause music_resume music_playing music_pos music_tick
sound_tick`; `sfx_def sfx_play sfx_stop sfx_stop_all sfx_standard`; `drum
drum_kick drum_snare drum_hat drum_instruments`; `pcm_time pcm_render osc_sine
osc_saw osc_tri osc_square osc_noise env_decay env_ad env_adsr pcm_scale
pcm_mix pcm_clip`; `standard_instruments song_title song_gameplay`.

Both preludes are implicit imports — call everything bare. They are ordinary
Petal, readable at
[`prelude/nes.ptl`](../prelude/nes.ptl) and
[`prelude/nes_sound.ptl`](../prelude/nes_sound.ptl); every export carries a
comment explaining why it exists.

Petal itself — the language, and the builtins every host shares — is documented
in [the language guide](../../../docs/language-guide.md) and
[Builtins](../../../docs/Builtins.md).
