# 26 — Restaurant POS (Juniper & Ash)

A server's point-of-sale terminal for a fictional wood-fired bistro, drawn
entirely by a Petal script inside a Garden panel and laid out like a landscape
tablet: 48 px and larger touch targets throughout. The top bar is the floor:
ten tables, each with its own open check. Category tabs switch the menu grid;
tapping a dish adds it to the check, or opens a sheet for its modifiers
(temperature, sides, add-ons with price deltas, required and optional groups),
quantity and kitchen notes. **Send** fires the new lines to the kitchen as a
ticket on the rail, grouped by station. **Pay** splits the check evenly, adds a
tip figured on the pre-tax share, and takes card or cash, with a register-style
cash pad that counts back the change. Sent lines can be voided with a reason;
tickets on the rail age from green to amber to red and can be bumped.

All money is integer cents. Tax is 8.875%, rounded half up to the cent; a split
check gives the odd cents to the earlier shares and the last share pays
exactly what is left, so the shares always sum to the check.

The evening is seeded and plausible: table 7 has drinks already sent and three
new lines waiting, table 4 a half-fired order with a "No onions" burger, table 2
an all-sent dinner, and table 9 a six-top splitting three ways with one guest
already paid by card. Charred Octopus is 86'd.

## Run it

```bash
tools/run-example.ts restaurant-pos
```

By hand, headless:

```bash
cd examples/productivity/restaurant-pos
GARDEN_HEADLESS_SIZE=1440x920 GARDEN_PANEL_STORE_DIR=/tmp/pos-store \
  ../../../garden/target/debug/garden --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
```

Designed for a **1440×920** viewport, which gives the panel a 1428×848 pane.
The menu column takes whatever the 440 px check leaves, so a narrower pane
narrows the tiles and drops kitchen tickets off the rail (the header counts
them as "older off screen").

The floor persists through `panel_store_*`: every action that changes a table,
the rail or the ticket numbers saves `floor-v1` at the end of the frame, and a
restart (or `POST /panel/reset`) picks up where it left off, clock included.
Launch tests with `GARDEN_PANEL_STORE_DIR` pointed at a scratch directory, or
every run starts from the last run's floor. **Tap the server block** (top
right) to start a fresh service, which restores the seed floor and the
7:41 PM clock.

The panel sleeps after 10 s without input like every Garden panel, so the
rail's timers and the card reader's pulse stop until the next input; they are
not hung. Headless, the clock only moves when a test ticks it.

## Controls

**Floor and menu**

| Input | Effect |
|---|---|
| tap a table chip | show that table's check (amber dot: unsent lines; green ring: part paid) |
| tap a category tab, or `1`–`7` | switch the menu grid |
| tap a dish | add it; a plain dish tapped again bumps its unsent line's quantity. A dish with choices (three dots) opens the modifier sheet. An 86'd dish refuses with a toast |
| tap **Bump** on a ticket | the kitchen has sent it; it leaves the rail |
| tap the server block | the fresh-service sheet |

**The check**

| Input | Effect |
|---|---|
| guests `-` / `+` | seat count (the first dish on a free table seats two) |
| tap a line | select it; tap again to deselect |
| `-` / `+` on an unsent line | quantity (to zero removes it) |
| **Edit** | reopen the modifier sheet on the line, with its picks, notes and quantity |
| **Remove** | drop an unsent line |
| **Void item** on a sent line | the void sheet |
| wheel over the lines | scroll |
| **Send**, or `s` | the send sheet |
| **Pay**, or `p` | the payment sheet |

**Sheets** (escape, the close button, or a tap outside backs out)

| Sheet | Controls |
|---|---|
| Modifiers | one-of groups are radios (an optional one deselects on a second tap), add-on groups are toggles; quick-note chips per station (kitchen, pastry, bar); a free-text note field (the prelude's `text_field_update`: caret, selection, clipboard, undo; `return` adds); quantity stepper. The add button names the first required group still missing |
| Send | lists the new lines by station; **Fire**, or `return` |
| Void | four reasons; **Void item** is enabled once one is picked |
| Payment | split evenly 1–6 (locked after the first share is paid), tip None/15/18/20/22% with each amount shown, Card or Cash. Cash: quick tender (exact and the next round bills), a keypad (`00`, backspace) or the digit keys and `backspace`, change due or "short by"; **Take cash**. Card: **Charge card**. `return` takes it |
| Payment done | change due (cash) or the card slip; **Next guest** for the next share, **Close table** after the last; **Print receipt** |
| Fresh service | **Reset the floor** |

## What it exercises

- **Language**: `state var` cells written by top-level action functions
  (`add_line`, `send_order`, `take_payment`, `void_line`, `close_table`) with
  `set` on nested index and field targets (`set tables[get cur] = t`,
  `set draft.picks[gi] = …`, `set counts.adds += 1`); `const` menu tables built
  by constructor helpers; `match` on station names; collecting `for` for
  line replacement; `reduce`/`filter`/`find` over lines; default and named
  arguments in the control helpers; integer money arithmetic throughout.
- **Prelude and host**: `text_field_update` with a custom paint (selection
  rects via `text_range_rects`, caret via `text_layout.caret_x`),
  `text_layout` for vertical centring, line clamping, eliding and wrapped
  paragraphs, `draw_backdrop_blur` under the sheets, `draw_shadow`, rounded
  clips, gradients for the list fades, `fill_arc`/`fill_polygon` for the card
  reader and the backspace key, a Georgia serif via a font fallback list,
  `panel_store_get`/`set` with `json_stringify`/`json_parse`, `hash_int` for a
  deterministic card slip, `claim_key` for the note field's Cmd chords.
- **Interaction design**: one press does one thing (`look.ptl` keeps a
  per-frame "taken" flag, so the tap that opens a sheet does not land on the
  sheet), the floor stands down while a sheet is up, and eased hover, press
  and selection states settle instantly on a zero-`dt` frame so captures
  after an injected tap show the end state.

Debug-server values a test can assert on (`obs_*` in `panel.values`):
`obs_table`, `obs_guests`, `obs_cat`, `obs_sheet`, `obs_sel`, `obs_lines`
(one string per line with modifiers, note, price and sent/VOID),
`obs_unsent`, `obs_subtotal`, `obs_tax`, `obs_total`, `obs_paid`, `obs_due`,
`obs_split`, `obs_draft`, `obs_mod_missing`, `obs_mod_price`,
`obs_pay_phase`, `obs_pay_method`, `obs_share`, `obs_tip`, `obs_charge`,
`obs_tendered`, `obs_change`, `obs_tickets`, `obs_next_ticket`,
`obs_open_tables`, `obs_counts` (adds, sends, payments, voids, bumps,
refused, closed), `obs_toasts`, `obs_focus`, `obs_scroll`, `obs_saves`,
`obs_clock`. Money values are cents.

A scripted run from `tick; panel_reset 42` (fresh store): Smash Burger with
Medium / Aged cheddar / bacon / Garlic mash, "No onions" and a typed note, x2
→ subtotal 15800, tax 1402, total 17202; fire → ticket #1044; pay split 2 at
20% → share 8601 + tip 1580 = 10181, cash $110.00 → change 819; second share
by card → due 0; close → $203.62 collected.

## Known limits

- **No pointer layer between the floor and a sheet.** Immediate mode has no
  z-order, so `look.ptl` gates the floor's hit tests itself (`lk.gate`) and
  keeps a one-press-per-frame flag (`lk.press`). The prelude's layered
  `interact`/`ui_layer` would do this, but every control here would have to be
  written against it; a gate flag was less code.
- **`state var` read inside a lambda needs a local copy first.** Filtering
  with `fn(x) -> contains(draft.chips, x)` at top level is a compile error
  ("write `get draft`"), which is correct but surprising in top-level code
  that otherwise reads the cell bare; the app copies to a `let` first.
- **A hoisted function cannot read a `state var` declared below it.**
  `now()` reads `clock_skew`, so it had to move below the state block
  (`undefined variable` from `check`, not a hoisting warning).
- **`json_parse` round-trips ints as ints**, which the money math depends on;
  nothing checks the saved document's shape, so a store from an incompatible
  version would fail on first use (the key is versioned, `floor-v1`).
- **Clock only advances on ticks headless**, and a sleeping panel stops it;
  ticket timers read stale until the next input.
- **Glyphs**: `×`, `·`, `—` and the backspace symbol are avoided (the
  embedded Inter subset lacks some of them); the backspace key and the
  contactless mark are drawn with polygons and arcs.
- **Toasts wait while a sheet is open** rather than drawing over it; they age
  meanwhile, so a short sheet visit can swallow one.
- **Card payment is simulated**: approval is instant, the brand, last four
  and auth code come from `hash_int` of the amount.
- At the 1280×850 default the check column keeps its 440 px and the menu
  tiles get narrow; long dish names clamp to two lines with an ellipsis.
