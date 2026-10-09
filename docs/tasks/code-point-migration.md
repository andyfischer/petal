# String offsets are code points: what to change when you re-vendor

Status: 2026-10-09. Petal `core` switched string offsets from UTF-8 bytes to
Unicode code points (todo item 4, [testbed-takeaways §1](testbed-takeaways.md)).
This file lists what each project that vendors or installs Petal has to do.
Nothing outside `~/petal` was edited.

## What changed

| | Before | Now |
|---|---|---|
| `len(s)`, `s.length` | UTF-8 bytes (`len("Óscar")` was 6) | code points (5) |
| `slice(s, a, b)` | byte offsets, snapped inward to a character boundary (`slice("Óscar", 0, 1)` was `""`) | code-point offsets (`"Ó"`) |
| `s[i]` | error `Cannot index string with int` | the one-character string at code point `i`; negative counts from the end, out of range is an error |
| `index_of(s, needle)` | code points | unchanged |
| `char_len`, `char_at`, `char_slice`, `chars` | code points | unchanged; `char_len`/`char_slice` are now synonyms of `len`/`slice` |
| `byte_len(s)`, `byte_slice(s, a, b?)` | did not exist | the old `len`/`slice` behaviour on a string, under explicit names |

Lists and `f64_array`s are untouched. A script that only ever sees ASCII text
behaves exactly as before.

`len`, `s[i]`, `slice` and `char_at` are O(1) on an ASCII string of any length.
On a string holding a non-ASCII character they walk from the nearest of the
start, the end and the previous lookup on that string, so a front-to-back scan
is linear.

## How to check a script

A script changes behaviour only where a string that can hold non-ASCII text
meets one of these:

1. **A byte budget.** `slice(s, 0, min(len(s), N))` where `N` is a limit in
   bytes (a wire field, a buffer). It now keeps `N` characters, up to `4 * N`
   bytes. Use `byte_slice(s, 0, N)` and `byte_len(s)`.
2. **An offset from outside the script** that is in bytes: a host native or a
   JSON payload reporting a byte position, passed to `slice`. None was found
   (see "Natives" below).
3. **A workaround for the old behaviour**: widening `slice(s, 0, n)` until it
   stops returning `""`, or subtracting 3 for a trailing `"…"`. The widening
   loop still gives the right answer and can be deleted; byte arithmetic on a
   known character must go.
4. **A name clash.** A script that defines its own `byte_len` or `byte_slice`
   keeps working, since a script's own `fn` wins over a builtin.

`grep -nE '\b(len|slice)\(' file.ptl` and read the hits that take a string.
`char_len`/`char_at`/`char_slice` calls need no change.

## Sites outside `~/petal`

Swept read-only: `~/cheesecake/games` (164 scripts), `~/cheesecake/engine` (2),
`~/worlds-fair/ui/ptl` (19), `~/.garden` (72), `~/biz/petal-lang.org/frontend/public`
(17), `~/biz/hotlaps` (0), `~/biz/experiment-cube-browser` (1),
`~/biz/experiment-todo-app` (1), `~/tools` (3, `temp/` and `out/` ignored).

### Needs a decision

- `~/worlds-fair/ui/ptl/screens/direct_connect.ptl:64` and
  `~/worlds-fair/ui/ptl/screens/server_browser.ptl:41`:
  `slice(text, 0, min(len(text), 255))` bounds a typed server address. It was
  255 bytes and is now 255 characters. The comment calls it a UI bound, and the
  server validates on its own, so it is safe as it stands. If the 255 is meant
  to match a byte-sized field, write `byte_slice(text, 0, 255)`.

### Stale comment only

- `~/tools/session-retro-rs/src/retro.ptl:1099`: the comment says "`len` is a
  *byte* count". The code measures with `text_width` and is right either way.

### Improves with no edit

- `~/worlds-fair/ui/ptl/screens/server_browser.ptl:88`: the lobby name is cut
  to 48 with `slice`/`len`. `sim/wf-server/src/matchmaking.rs:311` rejects
  names over 48 `chars()`, so the two now agree. Before, a non-ASCII name was
  cut to 48 bytes.
- `~/.garden/state/window-105/git.ptl:333`: a string is split at a column
  width, which now counts characters.

### Nothing to do

- `~/cheesecake/games`: every string offset already goes through `char_len` /
  `char_at` / `char_slice` / `chars` (football `game.ptl:92-149`, lantern
  `scenery.ptl:627-629`, solitude `hud.ptl`, `util.ptl`, `missions.ptl`,
  `title.ptl`, moonbear `hud.ptl:89`, neon `rng.ptl:41`). They can be
  shortened to `len`/`slice`/`s[i]` at leisure. The remaining `len`/`slice`
  calls are on lists.
- `~/cheesecake/engine`: the two preludes hold no string offsets. The C++
  natives (`engine/script/src/natives_*.cpp`) pass no string offset to or from
  a script.
- `~/biz/petal-lang.org/frontend/public`, `~/biz/experiment-*`, `~/biz/hotlaps`:
  no string offsets.
- Vendored copies of the docs (for example
  `~/worlds-fair/vendor/petal/docs/Builtins.md`) still describe byte indexing
  until the project re-vendors.

## Inside `~/petal`, left for the owner of those directories

`core-libs/petal-ui/` and `garden/` were being edited by another task when this
landed, so these were not touched. None is needed for the build or the tests.

- `core-libs/petal-ui/prelude/ui.ptl:1151-1153` (`wrap`), `:1170-1173`
  (`preview`) and `:1234-1240` (`ellipsize`): the comments say widths are
  bytes and that `slice` snaps to UTF-8 boundaries. `wrap`, `preview`,
  `truncate_head` (`:1105`) and `truncate_tail` (`:1096`) now count
  characters, which is what their `max_chars` parameter always claimed. A
  line holding non-ASCII text used to be cut by bytes, so it now keeps more of
  the text for the same `max_chars`; the result is at most `max_chars`
  characters. Panel goldens that show such a line will change.
- `core-libs/petal-ui/prelude/ui.ptl:1332-1345`, `:2135-2259` (`text_field`
  and its helpers) use `char_len`/`char_slice` and are unaffected.
- `garden/docs/petal-graphical-panels.md:260-261` says "`len` and `slice` are
  byte-indexed".
- `garden/gpp-apps/git-viewers/src/git_panel.ptl:420-421` and
  `garden/gpp-apps/sqlite-browser/src/db_view.ptl:102` slice strings by
  offsets they computed with `len`, so they stay consistent.

## Natives that exchange string offsets with scripts

Checked in `core-libs/petal-ui/src`, `core-libs/petal-query/src`,
`integrations/*/src`, `garden/garden-script/src` and
`examples/custom-apps/*/src`. None disagrees with the new unit.

| Native | Unit | Agrees |
|---|---|---|
| `text_index_at(s, style, dx)` (`petal-ui/src/text.rs`) and `text_layout.caret_index` over it | code points | yes |
| `text_wrap(s, {offsets: true}, w)` rows `{text, start, end}` (in progress in `petal-ui/src/text.rs`) | code points | yes |
| `text_width`, `text_advance`, `text_metrics`, `text_ellipsize`, `text_input` | take or return text, no offsets | n/a |
| `edit_view_edits`, `text_view_line_styles`, `text_view_scroll_to` (garden) | line numbers | n/a |
| `petal-c-bridge` source spans (`src/source.rs`) | character offsets | yes |

`petal-desktop-sdl`, `petal-web-canvas` and `petal-web-html` pass no string
offsets.
