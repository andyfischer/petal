# 37 — Video timeline editor (Splice)

A non-linear video editor's cutting room, drawn entirely by Petal inside a
Garden panel. A 43-second teaser, *Harbour Lights*, sits on six tracks —
titles, B-roll and main picture over dialogue, music and ambience — under a
zoomable time ruler. A playhead scrubs and plays across it at 24 fps, and
the program monitor shows the picture under it, composited bottom-up with a
title or name strap over the footage and stereo meters riding the sound.

There is no video decoder in a panel, so the footage is procedural:
`scenes.ptl` draws each shot (a harbour at dusk, a lighthouse sweeping its
beam, an interview, an old town at night, a coast road, cliffs from a drone)
as a function of source time. The same function paints the monitor, the
inspector still, the bin thumbnails and the filmstrips on the clips, so
trimming a clip's head really does change the frame the monitor shows at
that point, and a split clip's right half continues where the left one left
off. Audio waveforms and the meters read one loudness function per source
the same way.

Clips move between positions and tracks with snapping, trim from either edge
within their source media, split at the playhead, delete or ripple delete,
and arrive from the media bin by drag and drop. Every edit is one undo step.

## Run it

From the repo root, `tools/run-example.ts video-timeline` (extra arguments
pass through to Garden, e.g. `--headless --debug-port 0`). By hand:

```bash
cd examples/productivity/video-timeline
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives a
1268×778 pane: a 262 px media bin and inspector either side of the monitor,
and a 1100 px track area under them. Everything is laid out from
`screen_width()` / `screen_height()`; the timeline keeps its height and the
monitor row takes what is left.

Nothing is persisted: the edit lives in `state`, and `POST /panel/reset`
restores the seed project. While it plays the panel asks for frames itself
(`request_frame()`); stopped, it sleeps 10 s after the last input like any
panel, so a still playhead on a paused transport is not a hang.

Driving playback deterministically: tick once so the panel is on the debug
server's virtual clock, then reset on a seed. This script, run twice, ends
on identical `obs_*` values:

```bash
source tools/panel-test.sh            # with PORT set, or PANEL_LOG pointing at the log
tick; panel_reset 42
key space; tick 120 0.016; key space  # obs_playhead 46: 120 × 16 ms × 24 fps
drag 300 465 760 465                  # scrub: obs_tc 00:00:24:13, monitor ["Coast Road"]
key s                                 # split under the playhead: 19 clips
drag 520 550 640 555                  # move B-roll right; its end snaps to 504 and cuts into Cliffs
drag 942 638 1010 638                 # trim the second VO's tail; snaps to 840
drag 150 340 860 548                  # drop "Coast Road" from the bin onto V2
key z cmd; key z cmd shift            # undo + redo
key home; key space; tick 200 0.016   # obs_playhead 76, monitor ["Harbour at Dusk", "Main Title"]
```

## Controls

**Transport and keys**

| Input | Effect |
|---|---|
| `space`, or the round button | play / pause from the playhead (`l` plays, `k` pauses) |
| `left` / `right`, or the step buttons | one frame back / forward (`shift`: one second) |
| `up` / `down`, or the outer buttons | previous / next edit point (any clip start or end) |
| `home` / `end` | start / end of the sequence |
| `s`, or Split | split the selected clip at the playhead; with no selected clip under it, every clip on a live track the playhead crosses |
| `delete` / `backspace`, or Delete | lift the selected clip, leaving a gap |
| `shift delete`, or Ripple | delete the selected clip and close the gap on its track |
| `m`, or Marker | add a marker at the playhead |
| `n`, or Snap | snapping on / off |
| `=` / `-`, or `+` / `−` | zoom in / out around the playhead |
| `\`, or Fit | fit the whole sequence |
| `cmd z` / `shift cmd z` | undo / redo (80 levels, one per gesture or command) |
| `escape` | mid-drag: cancel the gesture and put the clip back. Otherwise: deselect |

**Timeline**

| Input | Effect |
|---|---|
| click or drag in the ruler | scrub; the playhead snaps to clip edges and markers, with a timecode readout |
| hover a marker flag | its name |
| click a clip | select it (the inspector shows it) |
| drag a clip's body | move it in time and to another track of its kind (picture or sound); its head or tail snaps to edges, markers and the playhead; dropping overwrites what it lands on, splitting a clip it lands inside |
| drag a clip's edge | trim it; a bracket marks the grip, the monitor switches to the IN or OUT frame, and the edge stops at the source media's first or last frame and at the neighbouring clip |
| click empty track space | deselect |
| eye / speaker button | hide a picture track from the monitor, or mute a sound track (its clips dim) |
| wheel | scroll the timeline (`cmd`, `ctrl` or `alt` + wheel zooms around the pointer) |
| zoom slider, scrollbar | drag |

**Media bin**

| Input | Effect |
|---|---|
| All / Video / Titles / Audio | filter the bin |
| drag an item onto a track | add a clip there (5 s of footage, 4 s of title, 10 s of sound), overwrite-placed with snapping; it is refused on a track of the other kind |
| wheel | scroll the list |

## What it exercises

**Timeline interaction**

- One gesture state machine (`drag`): a left press starts exactly one of
  `scrub`, `move`, `trim_l`, `trim_r`, `bin`, `zoom` or `scroll`, decided by
  where it lands, and the gesture owns the pointer until release. The
  document as the press found it (`g_doc`) is both the undo entry the
  gesture pushes on release and what `escape` restores.
- Two coordinate systems on one axis: frames ↔ pixels through a view record
  `{x0, v0, ppf}` that every hit test and every draw call share, so zoom and
  scroll are two numbers. Zoom anchors on the playhead (keys, buttons,
  slider) or on the pointer (wheel).
- Snapping as one pure function: `snap_best(points, candidates, threshold)`
  returns the shift and the line to draw, for a moving clip's head and tail,
  a trimmed edge, a bin drop and the scrubbing playhead alike.
- Overwrite editing (`place`): a dropped clip trims what it overlaps, removes
  what it covers, and splits a clip it lands inside into two with a fresh id
  and a shifted source-in. Trims are clamped by source media and neighbours.
- A ruler whose tick spacing is picked from a table of frame steps so major
  ticks stay at least 74 px apart, with `MM:SS` or `M:SS:FF` labels.

**Playback**

- The playhead advances by `dt() × 24` while playing, stops at the end of
  the sequence and pages the view along when it runs off the right edge.
- The monitor composites V1, then V2, then V3 at the playhead, skipping
  hidden tracks; titles are transparent overlays with their own fades.
- Meters with peak hold read the loudness of every live sound clip under the
  playhead, so a muted track goes quiet on the meter too.

**Language**

- The whole edit is one immutable record (`doc`: clips, markers, next id),
  edited by pure functions (`place`, `split_at`, `ripple_delete`), so undo
  is a list of earlier values and "did this frame change anything" is
  `doc != pre`.
- A second module (`import ./scenes`) holding the procedural footage, with
  `pub` on its two entry points.
- Collecting `for` with `continue` as filter-map, `match` with alternatives
  for commands, record spread for every derived clip and text style, `const`
  for the media and track tables, `layer` closures for group-faded titles
  and the toast.

**Host and prelude**

- `request_frame()` while playing, while a meter falls and while a toast is
  up; `claim_key` for `cmd z`; `scroll_x` / `scroll_y` with modifiers.
- `layer` with alpha, nested `clip_push` (rounded) for clips, filmstrips
  and the inspector still, `draw_shadow`, `draw_rect_gradient`,
  `draw_circle_gradient`, `fill_polygon` for the playhead, markers,
  headlands and beams, `draw_fade`, `text_ellipsize`, `tint` for every
  derived colour, `text_layout.draw_text_line` for centred labels.

**Debug-server values** (`/state?values_prefix=obs_`)

| Value | Meaning |
|---|---|
| `obs_playhead`, `obs_tc`, `obs_playing`, `obs_played` | playhead frame and timecode; transport; seconds of playback since reset |
| `obs_monitor` | names of the clips composited on the monitor, bottom-up |
| `obs_meter` | left / right meter levels, 0–1 |
| `obs_layout` | every clip as `TRACK:id:start+length@source_in`, sorted by track then start |
| `obs_clips`, `obs_end`, `obs_markers` | clip count, sequence end frame, marker count |
| `obs_sel`, `obs_drag` | selected clip id (-1 for none); the gesture in flight |
| `obs_zoom`, `obs_view` | pixels per frame; first visible frame |
| `obs_snap`, `obs_off` | snapping; per-track hidden / muted flags |
| `obs_undo`, `obs_redo` | stack depths |
| `obs_counts` | `{moves, trims, splits, drops, deletes, snaps}` since reset |

## Known limits

- **No video or audio.** Garden panels have no decoder and no audio output,
  so footage is procedural and sound is shown as waveforms and meters. The
  scenes are deliberately cheap (a few dozen primitives each) because every
  filmstrip thumbnail on screen redraws one every frame.
- **No ripple or roll trims, no slip/slide.** Trims are plain overwrite trims
  that stop at the neighbour; only delete has a ripple form. Clips cannot be
  multi-selected, linked (picture with its sound) or copied.
- **The toast's clock is virtual under test.** Once a test ticks, `time()`
  only moves on ticks, so a toast raised by an injected key stays up until
  ticks age it out. That is the documented contract, but it means a
  screenshot taken between commands can include a stale toast.
- **No rotated text.** Clip names and ruler labels are all upright, which is
  what an editor wants anyway; nothing needed a workaround.
- Glyphs `⌘`, `⇧`, `⌫`, `←` in the shortcut caps come from a system fallback
  face, so they render, but not in Inter.
