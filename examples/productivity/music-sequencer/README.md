# 36 — Music sequencer (Cadence)

A two-bar step sequencer and piano roll drawn entirely by a Petal script
inside a Garden panel. Eight lanes run over a 32-step timeline: six drum
lanes you paint with the mouse, and a bass and a lead lane whose notes are
edited in a piano roll underneath. A transport runs a playhead across the
pattern at the tempo and swing you set, inside a loop region you drag on the
ruler; a pattern change made while it plays is queued and lands when the loop
comes round.

A panel has no audio device, so playback is made visible instead of audible:
every trigger kicks its lane's meter (drawn inside the lane's fader), lights
its step, and feeds a small output scope synthesised from the lane envelopes.
The song is seeded with four patterns (Groove, Build, Drop, Break) of a
122 bpm track in A minor.

## Run it

The quickest way is `tools/run-example.ts music-sequencer` from the repo root (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`tools/run-example.ts music-sequencer --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/music-sequencer
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane: 31 px per step, a 224 px lane header column. The grid
and both editors reflow with the pane, but the transport bar is laid out for
this width and its right-hand blocks clip in a pane much narrower than
1200 px.

Nothing is persisted; the song lives in `state`, and `POST /panel/reset`
restores the seed.

While it plays, the panel asks for frames itself (`request_frame()`), so the
playhead keeps moving. Stopped, it sleeps 10 s after the last input like any
panel; a still playhead on a stopped transport is not a hang.

To drive playback deterministically, tick once before pressing play so the
panel is already on the debug server's virtual clock:

```bash
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{"seed":42}'
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":1}'
curl -sX POST 127.0.0.1:$PORT/key  -d '{"key":"space"}'
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":130,"dt":0.016}'
# obs_pos 16.917, obs_fired 42, obs_hits [5,2,2,13,4,4,6,6]
```

## Controls

**Transport and keys**

| Input | Effect |
|---|---|
| `space`, or the play button | play / pause at the playhead |
| `return`, or the stop button | stop, return to the start of the loop, and silence the meters |
| `l`, or the LOOP button | loop region on / off (off plays the whole pattern) |
| `1` – `4`, or a pattern chip | stopped: switch pattern. Playing: queue it for the next loop; the playing pattern's own key or chip cancels the queue |
| `-` / `=` | tempo down / up by 1 bpm (`shift` for 10); range 60 – 200 |
| tempo block | drag up or down, wheel, or the two arrow buttons |
| `,` / `.` | swing down / up by 2 % |
| swing slider | drag; 0 – 60 %, shown on the ruler as shifted off-beat ticks |
| `left` / `right` | move the step cursor (`shift` jumps a beat), `home` / `end` |
| `up` / `down` | select the lane above / below |
| `x` | toggle the step under the cursor; on a melodic lane, add or remove a note there |
| `v` | cycle the step's velocity: normal, accent, ghost |
| `m` / `s` | mute / solo the selected lane |
| `c` | clear the selected lane |
| `delete` / `backspace` | delete the selected note |
| `cmd z` / `shift cmd z` | undo / redo pattern edits (60 levels, one per gesture; an edit that changes nothing, such as clearing an empty lane, takes no level) |
| `escape` | with the mouse button still down: abandon the drag and put back what it changed (pattern, tempo, swing, loop, fader, playhead). Otherwise: deselect the note, cancel a queued pattern, close the menu |

**Ruler**

| Input | Effect |
|---|---|
| drag in the top band | set the loop region, snapped to beats (turns the loop on) |
| click or drag in the bar.beat band | move the playhead (works while playing) |

**Lanes**

| Input | Effect |
|---|---|
| click a drum step | toggle it, select the lane, move the cursor |
| drag along a drum lane | paint: every step you cross takes the state of the first one |
| wheel over a lit step | velocity up / down in steps of 6 |
| click a lane header or a melodic lane | select it (its editor opens below) |
| `M` / `S` | mute / solo; solo wins over the unsoloed, mute wins over solo |
| drag the fader | lane level; the meter rides inside the fader |
| right-click a lane | menu: clear, fill every beat / eighths / sixteenths, shift left / right, repeat bar 1 in bar 2 |

**Velocity editor** (a drum lane is selected)

| Input | Effect |
|---|---|
| drag across the bars | set each hit's velocity to the pointer height; a diagonal drag draws a ramp |
| wheel over a bar | fine velocity |

**Piano roll** (Bass or Lead is selected)

| Input | Effect |
|---|---|
| click empty space | add a note with the last-used length |
| drag from empty space | add a note and stretch it |
| drag a note | move it in time and pitch |
| drag a note's right edge | resize it |
| wheel over a note | velocity |
| right-click a note, or `delete` | remove it |

The column beside the editor is an inspector: hits, average, accents and
ghosts for a drum lane; pitch, start, length and velocity for a selected
note.

## What it exercises

**Timeline and grid interaction**

- One gesture state machine. A left press starts exactly one of ten drags
  (`paint`, `vel`, `seek`, `loop`, `bpm`, `swing`, `vol`, `note_new`,
  `note_move`, `note_resize`), chosen by where it lands; the gesture then
  owns the pointer until release, wherever it travels, and `escape` before
  the release restores the snapshot taken at the press (`g0`), undo stack
  included.
- Painting and velocity drawing interpolate between pointer samples, so a
  fast drag (or a debug-server drag, which is three events) leaves no holes
  and a diagonal one draws a ramp.
- Three coordinate systems over one step axis: lanes (step × lane), the
  velocity lane (step × 0–100) and the piano roll (step × pitch, with
  note-edge hit zones), all sharing `step_x` / `step_at`.
- Loop region and playhead on a two-band ruler; everything outside the loop
  is dimmed rather than hidden.

**Playback state**

- A float playhead advanced from the frame's time delta, in a loop that
  splits a frame at the loop boundary and may cross several steps: a 200 ms
  headless frame at 122 bpm is 1.6 steps, and no trigger in it is lost.
- Swing as a per-step trigger time (`trig_time`), so "which events start in
  `[a, b)`" is one pure function (`triggers`) used for every segment.
- Play / pause / stop, loop on / off, a queued pattern that switches on the
  wrap, mute and solo gating what fires, per-lane envelopes with their own
  decay, held notes sustaining their meter.

**Language**

- The whole song is one immutable value (`pats`), edited through nested
  index and field assignment (`pats[cur].notes[k][i].len = want`), which
  makes undo a list of earlier values with no bookkeeping.
- Collecting `for` as `map` and as `filter` (`lane_fill`, `lane_shift`,
  `list_without` with `continue`), `match` on characters to parse the
  drum-machine notation of the seed lanes, record spread for text styles.
- `state` for everything that persists across frames; no `var` at all.

**Host and prelude**

- `request_frame()` while playing or while a meter rings; `time()` deltas
  for the clock; `claim_key` for `cmd z`.
- `context_menu` / `menu_show` / `menu_blocking`, `draw_polyline` for the
  scope, `draw_rect_gradient_rounded` for the velocity bars, `mix` for every
  tint, `text_layout.draw_text_line` for centred labels.

**Debug-server values** (`/state?values_prefix=obs_`)

| Value | Meaning |
|---|---|
| `obs_playing`, `obs_pos`, `obs_step` | transport state; playhead as a float and as a step |
| `obs_bpm`, `obs_swing` | tempo, swing (0 – 0.6) |
| `obs_loop`, `obs_loop_on` | `[start, end)` in steps; whether it applies |
| `obs_pattern`, `obs_queued` | `"A"`–`"D"`; the queued letter or `""` |
| `obs_sel_track`, `obs_cursor` | selected lane name, cursor step |
| `obs_lane` | the selected drum lane as text, `X` accent, `x` normal, `o` ghost, `.` rest |
| `obs_notes`, `obs_sel_note` | note count of the selected melodic lane; the selected note `{s, len, p, v}` or null |
| `obs_events` | events in the current pattern |
| `obs_hits`, `obs_fired`, `obs_loops`, `obs_wrapped` | triggers per lane and in total since reset; loop wraps since stop; whether this frame wrapped |
| `obs_mute`, `obs_solo`, `obs_vol`, `obs_audible` | mixer, per lane |
| `obs_undo`, `obs_redo` | stack depths |
| `obs_drag`, `obs_menu` | the gesture in flight; whether the lane menu is open |
| `obs_cancels` | gestures abandoned with `escape` since reset |

## Known limits

- **No sound.** The panel vocabulary has no audio output, so the sequencer
  proves its timing on screen and in `obs_hits` instead.
- **`dt()` is wall-clock even on a ticked panel.** After `POST /tick` puts
  the panel on its virtual clock, `time()` stands still on settle frames but
  `dt()` does not, so a playhead driven by `dt()` drifted by a few tenths of
  a step per `/state` read. The app derives its delta from `time()` instead.
  The first `/tick` after a reset still jumps the clock once (the switch from
  wall time), which is why the recipe above ticks once before pressing play.
- **Named arguments to builtins passed `check` and failed at run time.**
  `clamp(v, lo: 0, hi: 1)` is accepted by `petal check --strict --host
  garden` at this checkout, but the Garden binary this was built against is
  older and raised `builtin 'clamp' does not accept named arguments`. Every
  builtin call here is positional.
- **A one-line collecting `for` read back as null.** `let obs_mute = for mr
  in mixer do mr.mute end` at module scope showed `null` in `panel.values`
  on this binary, while the multi-line form and `map(mixer, fn(r) ->
  r.mute)` report the list. The `obs_` lists use `map`.
- **Undo covers the patterns only.** Mixer, tempo, swing and loop changes
  are not on the stack (`escape` during the drag is the way back for those).
- **Shifting a melodic lane is lossy at the edges.** A note cannot wrap
  around the pattern, so one pushed across the end is cut to what fits;
  shifting back does not restore its length. Undo does.
- **A queued pattern survives a pause** and lands on the first wrap after
  play resumes; its chip stops pulsing while paused, because a paused panel
  no longer asks for frames.
- **Meters freeze under `/tick` settle frames.** The envelopes decay with
  the panel clock, which stands still between ticks, so a screenshot taken
  after a pause shows the meters and scope where the last tick left them.
  Stop clears them.
- **`petal check` needs `--host garden`.** The script imports `text_layout`,
  which only the Garden host registers; without the flag the check stops at
  `cannot find module 'text_layout'`.
- **A note gesture holds a list index.** If a queued pattern lands mid-drag
  the gesture is dropped, since the index would point into another lane.
- **Fixed length, no zoom.** Every pattern is 32 sixteenths and the timeline
  always fits the pane; there is no horizontal scroll, and no pattern
  chaining beyond queueing the next one.
- **Text cannot be rotated**, so the piano keys carry only their C labels.
