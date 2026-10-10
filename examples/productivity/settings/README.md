# 20 — Settings screen (Fern preferences)

The preferences window of an imaginary code editor, drawn entirely by a Petal
script inside a Garden panel. A sidebar of five sections with their
sub-sections nested under the open one, a scrolling page of grouped rows, and
on the right a small mock of the editor that is redrawn from the same values
the page edits. The Appearance page is live in the other direction too: color
mode, accent, contrast, density, text scale, corner radius and motion speed
restyle the settings window itself.

57 rows across 14 groups: toggles, checkboxes, segmented controls, dropdowns,
sliders, steppers, text fields with validation, a color swatch row, a radio
list and three action buttons. Rows depend on other rows (some go inert, some
fold away), search runs across every page, every change can be undone, and
what differs from the defaults is saved to the panel store.

## Run it

The quickest way is `tools/run-example.ts settings` from the repo root (it
finds the `garden` binary and sets the viewport; extra arguments are passed
through, e.g. `tools/run-example.ts settings --headless --debug-port 0`). By
hand:

```bash
cd examples/productivity/settings
GARDEN_HEADLESS_SIZE=1280x850 GARDEN_PANEL_STORE_DIR=/tmp/fern-store \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s "127.0.0.1:$PORT/screenshot?pane=0" -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane: a 224 px sidebar, a 436 px preview column and the page
between them. A pane narrower than 1180 px shrinks the preview column first.

Changed settings are written to the panel store on the frame they change,
under the key `settings`, as a JSON object of only the values that differ from
their defaults. A second launch therefore shows what you left, not the
defaults: launch tests with `GARDEN_PANEL_STORE_DIR=<scratch dir>`. **Privacy
& data ▸ Restore all defaults** (behind a confirmation) puts every value back
and leaves `{}` in the store.

The panel sleeps 10 s after the last input, like every Garden panel. Nothing
here moves on its own for longer than that: the preview's cursor and a focused
field's caret blink for a few seconds and then rest lit.

## Files

| File | What is in it |
|---|---|
| `schema.ptl` | The sections, the groups and every setting as one record each, plus the pure helpers over them (defaults, formatting, snapping, validation, search matching) |
| `widgets.ptl` | The controls: toggle, checkbox, segmented, select, slider, stepper, swatches, radio list, text input, and the keyed animator they share |
| `preview.ptl` | The editor mock: title bar, file tree, gutter, wrapped and highlighted code, minimap, status bar, the alert |
| `icons.ptl` | Section and action glyphs as strokes |
| `app.ptl` | One frame: state, theme, layout pass, keyboard, page, sidebar, preview column, overlays, persistence |

## Controls

**Mouse**

| Input | Effect |
|---|---|
| click a section | open that page (and leave a search) |
| click a sub-section | scroll the page to that group; the marker follows the page as it scrolls |
| click or drag a control | operate it: a slider jumps to the press and keeps tracking outside its track, a stepper repeats while held |
| click a checkbox row's label | toggles it, the label is part of the target |
| hover a changed row | a reset arrow appears after the label (tooltip names the default) |
| wheel, or drag the scrollbar | scroll the page |
| **Reset section** / **Reset N** | put the open page back to its defaults, as one undo step |
| a line in **Changes from defaults** | go to that setting, on whatever page it lives |
| the arrow on such a line | revert that one setting |
| the two arrows above the list | undo, redo |

**Keyboard**

| Key | Effect |
|---|---|
| `tab` / `shift+tab`, `↓` / `↑` | move the row ring; `↑` from the first row returns to the search box |
| `←` / `→` | adjust the ringed row: off/on, previous/next option, one slider or stepper step |
| `space` / `return` | flip a toggle or checkbox, advance a segmented control, radio list or swatch, open a dropdown, press an action button |
| `backspace` / `delete` | reset the ringed row to its default |
| `/` or `⌘F` | focus search |
| `return` or `↓` in search | step into the results |
| `escape` | leave a text field; then clear the search; then drop the ring |
| `⌘1` … `⌘5` | open a section |
| `⌘Z`, `⇧⌘Z` or `⌘Y` | undo, redo (inside a text field these are the field's own) |
| `pageup` / `pagedown` | scroll the page |

Text fields (search, display name, email) take everything the `ui` prelude's
field does: selection by drag or `shift`+arrows, double-click for a word,
`⌘A`, `⌘C` / `⌘X` / `⌘V`, word movement with `alt`.

## What takes effect

| Setting | Where you see it |
|---|---|
| Color mode, accent, contrast | the whole window cross-fades to the new palette; **Auto** follows Garden's theme |
| Density, interface scale, corner radius | row heights, every text size, every corner |
| Reduce motion, animation speed | every eased value in the window, bloom's menus and dialog included |
| Sidebar position | the file tree changes sides in the mock |
| Font family, size, line height, letter spacing | the mock's code |
| Line numbers, current line, cursor style, blink, minimap (+ side, width) | the mock's gutter, band, caret and minimap |
| Wrap long lines (+ continuation indent) | the mock rewraps at its own width |
| Tab size, spaces/tabs, show whitespace | indentation, the status bar, dots or arrows |
| Notifications: master switch, position, duration, sound, volume, events | the alert in the mock: present or not, which corner, its drain bar, its volume bars, its text |
| Quiet hours | the alert greys out and says until when; a moon in the status bar |
| Language | the mock's labels |
| Display name | the avatar in the mock's title bar |
| Release channel | the version in the sidebar and the mock's status bar |

## What it exercises

**Forms.** Ten kinds of control, all written in Petal as functions from the
current value to the new one, so the app keeps the only copy of the data.
Text entry is the prelude's `text_field_update` (caret, selection, clipboard,
its own undo) under the app's own paint, with validation that reports in the
row's description line.

**Nested sections.** Two levels in the sidebar (section ▸ group, with a
scroll-spy marker), and up to two levels of dependent rows on the page.
"Volume" depends on "Play a sound", which depends on "Show notifications";
"Animation speed" depends on "Reduce motion" being *off*. Rows marked
`reveal` fold open and shut with their parent instead of greying out.

**One schema, many readers.** `SETTINGS` in `schema.ptl` is the only place a
setting is named. Layout, the keyboard ring, search, the modified dots, the
per-section badges, reset, the Changes list, export and persistence are all
loops over it.

**Language.**
- `state var` cells written with `set` from functions (`commit`, `undo_step`,
  `go_section`), including `set vals[id] = v` on a record keyed at run time
  and `set undo[len(get undo) - 1] = …` on a list.
- `state(key)` for animation: `anim(key, target)` in `widgets.ptl` keeps one
  eased value per setting id, so a knob's motion follows the setting, not the
  row position a search happens to give it.
- Per-call-path `state` for everything positional: bloom's `probe` inside a
  loop over every setting gives each row its own hover and press.
- Modules: four files imported by `app.ptl`, a module-level `var` in
  `widgets.ptl` holding the frame's theme, default parameter values
  (`S(…, x: record = {})`), `match` with `do … end` arms, collecting `for`
  loops with `continue` as the filter, record spread to build the palette.
- A lambda passed as a glyph (`icon_button(r, fn(gr, c) -> glyph("undo", gr, c), true)`).

**Host and libraries.**
- `bloom`: `probe`, `drag`, `button`, `menu` (driven by the app's own select),
  `dialog`, `toast`, `tooltip`, `theme_set`, and the input capture, which is
  used twice: an open menu or dialog silences the page, and the page silences
  rows that are clipped under the header while the pointer is not over the
  viewport.
- `text_layout` for every label (`draw_text_line`, `elide`, `caret_x`).
- `clip_push` nested three deep (viewport ▸ folding row ▸ field), `draw_shadow`,
  `draw_rect_gradient`, `draw_polyline`, `fill_arc`, `fill_polygon`, per-run
  font family, size, weight and letter spacing.
- `panel_store_get` / `panel_store_set` with `json_parse` / `json_stringify`,
  `clipboard_set`, `claim_key` for twelve `⌘` chords, `request_frame` only
  while something is easing.

**Values a test can read** (`panes[0].panel.values`):

| Name | Meaning |
|---|---|
| `obs_vals` | every setting's current value, by id |
| `obs_hits` | id → the pane-local point to click to operate that control (with `w`/`n`/`lo`/`hi`/`step` where a control has parts), plus `nav_<section>`, `sub_<group>`, `chg_<id>`, `search`, `undo`, `redo` |
| `obs_section`, `obs_group` | the open page and the sub-section the scroll-spy is on |
| `obs_query`, `obs_searching`, `obs_rows` | the search text and how many rows are shown |
| `obs_focus`, `obs_edit`, `obs_nav` | the ringed row, the field with the caret, the ids the ring can visit |
| `obs_scroll`, `obs_max_scroll` | page scroll target and its limit, px |
| `obs_modified`, `obs_modified_by_section` | counts of settings off their defaults |
| `obs_undo`, `obs_redo`, `obs_saves` | stack depths, and writes to the store |
| `obs_dark`, `obs_accent` | the resolved color mode and accent name |
| `obs_dialog`, `obs_toasts`, `obs_cache_mb`, `obs_email_problem` | overlay and action state |

A test drives it without any coordinates of its own:

```bash
cd examples/productivity/settings
T=../../../tools/panel-test.sh
hit() { $T obs obs_hits | jq -r ".obs_hits.$1 | \"\(.x) \(.y)\""; }
$T panel_start settings; $T tick; $T panel_reset 42
$T click $(hit nav_editor);  $T tick 20 0.016       # open the Editor page
$T click $(hit ed_minimap);  $T tick 20 0.016       # flip the Minimap switch
$T obs obs_vals | jq '.obs_vals.ed_minimap'         # false
$T obs obs_rows                                     # 16: its two child rows folded away
```

## Known limits

- **A bloom dialog silences its own buttons.** `bloom.dialog` claims the
  pointer with `capture`, and `bloom.button` inside it then sees the claim and
  goes inert: the buttons in the gallery's dialog cannot be clicked either.
  The app calls `bloom.capture_release()` before its two dialog buttons and
  `bloom.capture("restore")` after them.
- **`bloom.theme_set({speed: 0})` divides by zero in `bloom.toasts`** (`a /
  dur(t, 0.22)`), although the docs offer 0 as "freeze the UI". Reduce motion
  passes 0.001.
- **bloom's `slider`, `segmented` and `select` take no `disabled`,** and its
  animators are per call path, so they could not be greyed out or keep their
  motion across a search. That is why the controls are the app's own, built on
  `probe`.
- **Return on a dropdown opens it a frame late.** bloom's menu reads `return`
  as "choose the highlighted item" on the frame it opens, so opening it from
  the same keypress picked item 0 at once.
- **`import preview` did not give a module.** `preview` is also a prelude
  function, and `preview.draw(…)` failed at run time with `No method 'draw' on
  type function` after `petal check` passed. The file is imported `as pv`.
- **Colors cannot be eased channel by channel:** `mix` rounds to whole
  channels, so an exponential approach stalls a few counts short. The palette
  cross-fade is a timed blend between two whole palettes instead.
- **Interface scale above about 115 %** elides a few descriptions beside the
  widest controls (the three-way Density control), and the hint strip drops
  its last entries rather than clip one. The layout's column widths are fixed;
  only type and row heights scale.
- **No per-key event**, so there is no "press a shortcut to record it" control,
  which a real settings screen would have.
- `⌘`, `⇧`, `⌫`, the arrows and `›` are not in the embedded Inter face and
  come from a system font, so they sit slightly differently from the text
  around them.
- **Export** writes the JSON with `clipboard_set`. Under `--headless` there is
  no system clipboard to read it back from, so that path is verified by its
  toast and count only.
- The mock editor is a picture of one: thirteen fixed lines, a fixed cursor.
  Font families are whatever the machine has; one it lacks falls back to
  JetBrains Mono.
