# 18 — Chat / Slack clone (Campfire)

A team chat client drawn entirely by a Petal script inside a Garden panel.
A dark sidebar of channels and direct messages, a scrolling message feed,
a growing multi-line composer, and a thread pane. The workspace is alive:
teammates post on a scripted afternoon, type before they speak, answer what
you write (in the thread if you wrote in one), react to your messages, and
leave unread and mention badges on the channels you are not looking at. All
of it runs off a simulated clock advanced by `dt()`, so a test replays the
same afternoon frame for frame.

## Run it

The quickest way is `tools/run-example.ts chat` from the repo root (it finds
the `garden` binary and sets the viewport; extra arguments are passed
through, e.g. `tools/run-example.ts chat --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/chat
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane. The sidebar is 256 px and the thread pane 384 px; the
feed takes the rest, so other sizes reflow (the composer drops its key hints
below 560 px of feed).

Nothing is persisted: every launch starts at 2:32 PM on launch week with the
same history. The afternoon's script runs for about 150 simulated seconds
(a chat minute passes every 12 s); while anything is still scheduled the
panel asks for frames and a green dot pulses at the foot of the sidebar.
After the script ends the panel sleeps 10 s after the last input like any
other, so a still screen afterwards is not a hang.

## Controls

**Composer** (the prelude's `text_area_update`, painted by the app)

| Input | Effect |
|---|---|
| typing | insert at the caret, replacing any selection; typing with nothing focused goes to the composer |
| `return` | send (blank drafts are ignored) |
| `shift` + `return` | new line; the box grows to 8 rows, then scrolls |
| click / drag / double-click / triple-click | place the caret / select / word / everything |
| arrows, `home`, `end` (+ `shift`) | move (extend the selection); up/down keep the column |
| `alt` + `left`/`right`, `alt` + `backspace` | by word |
| `⌘A`, `⌘C` / `⌘X` / `⌘V`, `⌘Z` / `⇧⌘Z` | select all, clipboard, undo / redo |
| send button | send |

Drafts are kept per channel; a channel with an unsent draft says `draft` in
the sidebar.

**Feed**

| Input | Effect |
|---|---|
| wheel | scroll; scrolling up unpins the feed from the bottom |
| `pageup` / `pagedown` | a page |
| hover a message | action bar: 👍 🎉 👀 🙌 to react, the bubble to reply in thread |
| click a reaction chip | add or remove your reaction |
| click "N replies" | open the thread |
| right-click a message | Reply in thread · Add 👍 · Copy text · Delete message (yours only) |
| "N new messages" / "Jump to latest" pill | back to the bottom, pinned |
| `esc` | close the thread; with none open, mark the channel read and jump to the bottom |

**Navigation**

| Input | Effect |
|---|---|
| click a channel or DM | switch; the composer keeps the keyboard, caret at the end of that channel's draft |
| `alt` + `up` / `down` | previous / next channel |
| thread pane `×` | close the thread |

## What happens on its own

| When (s after start) | Where | What |
|---|---|---|
| 3 → 6 | #launch-lumen | Theo types, then reports the smoke tests |
| 14.5 → 17 | DM Maya | Maya asks for a walkthrough (DM badge) |
| 26 → 29 | #design | Kenji asks about the danger zone (unread) |
| 38.5 → 42 | #launch-lumen | Priya @-mentions you (mention badge, amber row) |
| 53.5 → 56 | thread in #launch-lumen | Lars replies in the rc.4 thread |
| 69 → 150 | #general, #eng-platform, #random, #launch-lumen, DM Theo | five more |

And in reply to you: after you send, a teammate (whoever you named, by
`@handle` or first name; in a thread, otherwise whoever started it; else the
last person to speak there) starts typing about a second later and answers a
couple of seconds after that, from a pool picked by what you wrote (a
question gets an answer, thanks get "anytime", and so on). Someone else
reacts to a channel message within two seconds.

## What it exercises

- **A variable-height feed laid out once per frame.** `layout_msgs` wraps
  every message with `text_wrap_rows` and turns the channel into items
  (day separators, the red "new" line, grouped messages with reactions and
  thread summaries), each with a height; the scroll range, the hit tests and
  the paint all read those numbers. Consecutive messages from one person
  within five minutes group under one avatar.
- **Stick-to-bottom scrolling.** Each channel keeps `{top, stuck}`. While
  stuck, new content pins the view to the bottom and the drawn offset eases
  there with `approach`; a feed that only changed size (the composer grew a
  row, the thread pane opened) snaps instead. Scrolled up, arrivals do not
  move the view and are counted into the "N new messages" pill. A short
  channel sits at the foot of the feed, below its intro.
- **Rich text spans.** `` `code` `` and `@mentions` are parsed into
  character spans and each wrapped row is drawn in segments (`text_advance`
  per segment); a mention of you tints the whole row amber with a gutter bar.
- **A composer on the prelude's text area.** `text_area_update` owns caret,
  selection, clipboard and undo; the app paints the box, the selection, the
  rows and the caret itself, grows the box with the wrap, and turns plain
  Return into "send" (see Known limits).
- **An event scheduler on a virtual clock.** `queue` holds `type`, `post`
  and `react` events stamped in simulated seconds; each frame applies those
  that are due. `typing` entries expire on their own. `request_frame()` keeps
  the panel awake only while something is scheduled, a typing indicator is
  animating, a fresh message is fading, or the feed is easing.
- **Unread and mention tracking** from a per-channel read count: the feed is
  read whenever it is pinned to the bottom.
- `hash_int` / `hash_str` for deterministic reply and reactor picks;
  `group_by`, `find`, `count`, `filter`, computed record keys
  (`{...seen, [ch]: n}`), `match` over event kinds.
- `ui` prelude: `text_area_update`, `text_wrap_rows`, `text_row_of`,
  `text_range_rects`, `context_menu` / `menu_open_on_right_click` /
  `menu_blocking`, `draw_shadow`, `draw_scroll_shadow`, `approach`,
  `clipboard_set`, `theme_set`; `text_layout.draw_text_line` /
  `draw_text_baseline`; `claim_key` for the composer's Cmd chords.

### Values a test can assert on

All under `panes[0].panel.values` (`obs` in `tools/panel-test.sh`):

| Value | Meaning |
|---|---|
| `obs_channel` | the open channel's id (`launch`, `design`, `dm-maya`, …) |
| `obs_msgs`, `obs_total` | messages in the open channel / everywhere |
| `obs_unread`, `obs_mentions` | unread messages / mentions across the sidebar |
| `obs_unseen_here` | arrivals in the open channel below a scrolled-up view |
| `obs_stuck`, `obs_top`, `obs_max_top` | pinned to the bottom, scroll offset and its range |
| `obs_typing` | who is typing where, as `who@channel` |
| `obs_pending`, `obs_arrivals`, `obs_clock` | scheduled events left, teammate posts so far, simulated seconds |
| `obs_sent`, `obs_reacted` | your posts and reaction toggles |
| `obs_draft`, `obs_composer_rows`, `obs_focus` | the composer's text, its height in rows, which composer has the keyboard |
| `obs_thread`, `obs_thread_replies` | the open thread's parent id and its reply count |
| `obs_hover` | the message showing the action bar |

A deterministic run: `tick; panel_reset 42`, then `tick 200 0.016` twice
(Theo's post lands at 6 s), type and `key return`, `tick 120 0.05` (the
answer and a reaction arrive), and so on. The same script ends on the same
values every run.

## Known limits

- **Return-to-send is a workaround.** `text_area_update` always inserts a
  line break on Return and has no "submit" mode, so the app keeps the text
  from before the update on a plain Return and sends that. A multi-line
  field with a configurable submit key (Return vs Shift+Return) would remove
  this.
- **The text area's caret is per callsite, not per buffer.** Switching
  channels hands the same widget a different draft; its caret was left
  wherever it was in the old one. The app drops the focus for one frame
  after a switch so the widget re-enters at the end of the draft.
- **`continue x` compiles.** Writing `if … then continue x end` inside a
  collecting `for` (meaning "keep x") parses as a bare `continue` followed by
  a discarded `x`, with no warning from `check --strict`: it silently drops
  every element. This wiped the message list once during development.
- **Code and mentions keep the body's metrics.** Spans change colour and
  background, not face or weight, because rows are wrapped in the body style
  first; code chips add 6 px of padding that the wrap does not account for,
  so a row ending in code can run a few pixels long.
- **Timers only advance on frames.** In a window the panel sleeps 10 s after
  the last input once the script is over; replies you trigger keep it awake
  until they land. A long sleep counts as at most one second of chat time.
- Emoji come from the system emoji face (the embedded Inter has none);
  `❤️` draws as a monochrome heart there, so the seeded reactions avoid it.
- No persistence, no search, no message editing, no file uploads, no
  per-message timestamps tooltip beyond the hover time on grouped rows.
