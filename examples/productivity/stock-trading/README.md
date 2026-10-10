# 32 — Stock trading screen (Meridian)

A paper-trading desk drawn entirely by a Petal script inside a Garden panel. A
simulated market feed ticks twelve fictional listings; the screen shows a
watchlist whose quotes flash as they change, a candlestick chart with volume,
a moving average and a crosshair, a depth ladder, time and sales, a blotter of
positions, orders and fills, and an order ticket that validates against buying
power and fills against the book. Limit orders rest until the market trades
through them, and every fill moves cash, positions and P&L.

Nothing here is a real company, ticker or price. The feed is a seeded random
walk that carries its own generator, so the same number of simulation steps
always lands on the same quotes.

## Run it

The quickest way is `tools/run-example.ts stock-trading` from the repo root
(it finds the `garden` binary and sets the viewport; extra arguments are passed
through, e.g. `tools/run-example.ts stock-trading --headless --debug-port 0`).
By hand:

```bash
cd examples/productivity/stock-trading
GARDEN_HEADLESS_SIZE=1600x960 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s "127.0.0.1:$PORT/screenshot?pane=0" -o shot.png
```

Designed for a **1600×960** viewport, which gives the panel a 1588×888 pane.
Below 1500 px of pane width the side columns narrow and the headers drop
detail (the company name, the breadth meter, the leftmost statistics) rather
than overlap; at the 1280×850 default everything still works, but the blotter's
columns are tight.

There is no persistence: a launch, or `POST /panel/reset`, starts the session
again from the same opening book (four positions, three working limit orders,
$64,820.55 in cash). The market does not depend on the host's random stream,
so the reset seed changes nothing.

The feed is driven by `dt()`. While it is live the panel asks for frames with
`request_frame()`, so in a window it runs until you pause it. A headless panel
only advances when it is ticked: `POST /tick {"n":60,"dt":0.016}` is about ten
market seconds at 1x (two simulation steps) and about 150 at 16x (thirty).
Keep tick batches to about 60 frames; a larger one can outlast the debug
server's 5 s request timeout.

## Controls

**Screen** (when no number field has the keyboard)

| Input | Effect |
|---|---|
| `up` / `down`, `j` / `k` | previous / next listing |
| `1` `2` `3` `4` | timeframe: 1m, 5m, 15m, 1h |
| `c` | candles / line |
| `left` / `right` | pan the chart back / forward eight bars |
| `home` or `0` | back to the live edge |
| `space` | pause / resume the feed |
| `-` / `=` (or `[` / `]`) | feed speed down / up: 1x, 4x, 16x |
| `b` / `s` | ticket side: buy / sell |
| `m` / `l` | ticket type: market / limit |
| `q` or `tab` | put the keyboard in the quantity field |
| `return` | send the order on the ticket |
| `p` / `o` / `f` | blotter tab: positions / orders / fills |

**Number fields** (quantity, limit price)

| Input | Effect |
|---|---|
| typing | digits only; the price takes one decimal point and two places |
| `up` / `down` | step by 10 shares or one cent; with `shift`, 100 shares or ten cents |
| `tab` | move between quantity and limit price |
| `return` | send the order |
| `escape`, or a click elsewhere | give the keyboard back to the screen |
| click, drag, double click, arrows, `home` / `end`, `⌘A` `⌘C` `⌘X` `⌘V` `⌘Z` | the prelude's text field: caret, selection, clipboard, undo |

**Mouse**

| Where | Input | Effect |
|---|---|---|
| Watchlist | click a row | select the listing |
| Chart | move | crosshair; the legend reads out that bar's O/H/L/C, volume and MA20 |
| Chart | wheel | zoom, 24 to 200 bars |
| Chart | drag | pan through history; "Back to live" returns |
| Chart | click | put the price under the pointer on the ticket as a limit |
| Chart header | 1m / 5m / 15m / 1h, Candles / Line | timeframe and style |
| Order book | click a level | put that price on the ticket as a limit |
| Ticket | Buy / Sell, Market / Limit | side and type |
| Ticket | `−` / `+` in a field | step the value |
| Ticket | 10 / 50 / 100 / 500 / Max | set the quantity; Max is what buying power affords, or every free share |
| Ticket | the wide button | send the order |
| Blotter | Positions / Orders / Fills | switch tab |
| Blotter | a position row / an order row | select that listing |
| Blotter | Close | stage a market sell of the position's free shares (it does not send it) |
| Blotter | Cancel | cancel a working order |
| Blotter | wheel | scroll a list longer than the card |
| Top bar | Pause / 1x / 4x / 16x | the feed's transport |

## How trading works

- **Market orders** sweep the ladder best-first: a size bigger than the top
  level pays a worse average price, and the ticket's estimate shows it.
- **Limit orders** take whatever the ladder offers at the limit or better and
  rest the remainder. A resting order fills in full, at its own price, on the
  first simulation step whose quote crosses it (ask at or below a buy, bid at
  or above a sell). Orders are matched after every step, not once per frame.
- **Buying power** is cash less what the working buy orders have reserved.
  There is no margin and no short selling: a sell is limited to the shares
  that are not already backing a working sell.
- **Commission** is $0.005 a share with a $1.00 minimum, per execution.
- **Validation** runs on every frame, and the reason shows above the send
  button: no quantity, more than 50,000 shares, no limit price, a limit more
  than 20% from the last trade, not enough buying power, nothing to sell, or
  more than the free shares. Sending a ticket that fails shakes the message
  and counts in `obs_rejects`.
- **P&L**: a buy adds to the position at cost; a sell releases its share of
  the cost basis and realizes the difference, less commission. Session P&L is
  net liquidation value against its value when the session opened.

## What it exercises

**Language**

- A pure simulation in `market.ptl`: `seed()` builds four market days of
  history and `step(m)` advances a market record by five seconds. Both carry
  their generator through a local `var` with `rnd` and `gauss` closures over
  it, the "local cells and nested helpers" shape; nothing reads the clock, so
  the module also runs under plain `petal run`.
- Account actions as top-level functions over `state var`s (`place_order`,
  `apply_fill`, `match_orders`, `cancel_order`), read with `get` and written
  with `set`; everything else is `state` and `let` at module scope.
- Collecting `for` as `map` and `filter` (`continue` to skip), `reduce`,
  single-pass bucketing in `fold` and `with_tail`, a `while` with an early
  `return` in `nice_step`, default and named parameters, `??` on `parse_int`
  and on a fill's absent P&L, records as multi-value returns.
- Integer cents for every price and balance, with formatting in `numfmt.ptl`
  that never goes through `str(float)`.
- Four modules imported under aliases (`mk`, `nf`, `lk`, `pc`), with
  call-path-keyed `state` inside reusable controls (`ease_flag`, `segmented`,
  `num_field`, the chart's drag).

**Host / petal-ui**

- `text_field_update` for both number fields, with the app's own paint,
  selection rects from `text_range_rects`, and `claim_key` for the clipboard
  and undo chords.
- `text_layout` for every label (`draw_text_line`, `text_baseline`,
  `caret_x`, `elide`), two faces (`ui` for labels, `mono` for numbers),
  letter-spaced caps.
- The draw surface: `draw_rect` for candles, wicks, volume and dashed rules,
  `draw_polyline` for the line chart, the moving average and the sparklines,
  `draw_rect_gradient` for the area wash, `fill_triangle` for fill markers,
  `draw_shadow`, rounded outlines, `clip_push` with a radius for the
  allocation bar.
- `dt()`-driven simulation with a step budget per frame, `request_frame()`
  while the feed runs, `scroll_update` / `draw_scrollbar` for the blotter
  lists, `scroll_y`, `mouse_pressed` / `mouse_down` / `mouse_released` for the
  chart's click-versus-drag.

**Debug server** — the assertable values in `panes[0].panel.values`, all
prefixed `obs_`. Prices and balances are integer cents.

| Value | Meaning |
|---|---|
| `obs_symbol`, `obs_sel` | the selected listing |
| `obs_clock`, `obs_steps` | market time and simulation steps so far |
| `obs_last`, `obs_bid`, `obs_ask` | the selected listing's quote |
| `obs_quote_changes` | how many last-trade changes the watchlist has flashed |
| `obs_paused`, `obs_speed` | the feed's transport |
| `obs_tf`, `obs_chart_mode`, `obs_bars`, `obs_bars_shown`, `obs_pan` | the chart's view |
| `obs_hover_bar`, `obs_cursor_px`, `obs_chart_clicks` | the crosshair's bar and price, and clicks taken as a limit |
| `obs_side`, `obs_kind`, `obs_qty`, `obs_limit`, `obs_edit` | the ticket, and which field has the keyboard |
| `obs_ticket_error`, `obs_can_submit`, `obs_est` | validation and the estimated cost |
| `obs_submits`, `obs_rejects` | orders sent and tickets refused |
| `obs_cash`, `obs_buying_power`, `obs_equity`, `obs_session_pnl`, `obs_realized`, `obs_fees` | the account |
| `obs_positions` | `["HLXR:120", …]` |
| `obs_working` | `["#3 Buy 200 TLLY @ 64.40", …]` |
| `obs_orders`, `obs_fills`, `obs_last_fill` | counts, and the newest execution |
| `obs_tab`, `obs_toasts` | blotter tab and toasts on screen |

A session that proves the arc, from the example directory:

```bash
T=../../../tools/panel-test.sh
$T panel_start stock-trading
$T tick; $T panel_reset 42
$T key =; $T key =                 # 16x
$T tick 60 0.016                   # the seeded NMBS limit fills at its price
$T obs obs_fills,obs_last_fill,obs_cash
$T click 1415 554                  # "Buy 100 NMBS at market"
$T obs obs_positions,obs_cash,obs_buying_power
```

## Known limits

- **A screenshot straight after an input has no time in it.** On a ticked
  panel the frame an injected click runs sees `dt() == 0`, so anything eased
  by `dt()` is captured where it started. The controls here land on their
  target outright on such a frame (`glide` in `look.ptl`), and a toast starts
  a third of the way into its fade, purely so a still taken after a click
  shows the state it produced. In a window `dt()` is never zero and they ease.
- **The prelude's `draw_area_fill` bands under a jagged series**: it weights
  each column's alpha by its height. The line chart uses its own wash of
  whole-pixel gradient columns instead. Relatedly, a rect built from floats
  has its `x` and `w` truncated separately, which leaves hairline gaps between
  adjacent fractional columns.
- **A letter-spaced run cannot be found by text.** `/scene?find=text:` returns
  one entry per glyph for the tracked-out caps labels, so tests locate
  controls by their plain-text neighbours or by coordinates.
- **`/text` after a chord is dropped** until a plain key arrives, so a test
  that wants to replace a field's contents double-clicks it and types rather
  than sending `⌘A`.
- **A tick batch is slow for reasons outside the script.** `frame_stats`
  puts a frame of this script at about 4 ms, but a 60-frame `POST /tick`
  takes about 3 s of wall time in a debug build, which is why the batches
  above stay at 60.
- **`text_atlas` is `null` in a headless run**, so glyph-atlas pressure could
  not be checked here. The screen keeps to six text sizes per face.
- **The book is derived, not simulated.** Depth is a hash of the listing, the
  price level and a 20-second epoch, so a level keeps its size while the quote
  is still, but there is no order-by-order queue, and the account's own
  resting orders do not add to the displayed size (they get a marker).
- **The market never closes.** It trades around the clock with no sessions or
  gaps, and the "24h" figures are a rolling day.
- **Feed catch-up is capped**: a frame runs at most 16 simulation steps and
  reads at most 0.25 s of `dt()`, so a long stall slows the market down
  instead of making it jump.
