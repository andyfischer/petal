# Testbed challenge plan

The original list of 50 target apps for the Garden panel-app testbed, drawn up
when the testbed was started (Aug 2026). Each app exercises a different slice
of the language and host; see the "What it tests" column.

20 of the 50 exist. Fifteen are panel apps under the category directories
described in [examples/README.md](../../examples/README.md) (the first three
of each category), and five more were written as headless console programs
under `examples/challenge/`, which stress the language rather than the host.
The Status column gives each one's current path. See
[examples/AUTHORING.md](../../examples/AUTHORING.md) for how to write one.

`examples/games/side-scroller` is a sixteenth panel app that was not on this
list. The remaining 30 have not been attempted.

| # | App | What it tests | Status |
|---|---|---|---|
| **Simple games** | | | |
| 1 | **Pong** | Game loop, keyboard input, collision, animation | built — `examples/games/pong/` |
| 2 | **Breakout** | Many objects, collision, spawning/destruction | built — `examples/games/breakout/` |
| 3 | **Snake** | Grid rendering, timers, keyboard input, state | built — `examples/games/snake/` |
| 4 | **Tetris** | Complex state transitions, grids, timing | console — `examples/challenge/tetris.ptl` |
| 5 | **Asteroids** | Vector movement, rotation, particles, collision |  |
| 6 | **Flappy Bird clone** | Physics, procedural obstacles, scoring |  |
| 7 | **2048** | Grid layout, gestures/keys, transitions | console — `examples/challenge/2048.ptl` |
| 8 | **Minesweeper** | Dynamic grids, recursive behavior, right-click | console — `examples/challenge/minesweeper.ptl` |
| 9 | **Memory matching game** | Card components, animation, delayed state changes |  |
| 10 | **Tower Defense mini-game** | Paths, many entities, targeting, simulation |  |
| 11 | **Particle sandbox** | Thousands of objects, mouse interaction, performance |  |
| 12 | **Physics playground** | Dragging, gravity, collisions, constraints |  |
| **Everyday UI** | | | |
| 13 | **Calculator** | Buttons, layout, expression/state handling | built — `examples/productivity/calculator/` |
| 14 | **Todo app** | CRUD, lists, persistence, filtering | built — `examples/productivity/todo/` |
| 15 | **Notes app** | Text editing, selection, persistence, search | built — `examples/productivity/notes/` |
| 16 | **Calendar** | Dense layout, dates, navigation, drag/drop |  |
| 17 | **Email client** | Master/detail UI, lists, search, selection |  |
| 18 | **Chat / Slack clone** | Scrolling feeds, composer, async messages |  |
| 19 | **File browser** | Trees, icons, selection, context menus |  |
| 20 | **Settings screen** | Forms, toggles, sliders, nested sections |  |
| 21 | **Login + signup flow** | Forms, validation, focus, errors |  |
| 22 | **Command palette** | Keyboard handling, fuzzy search, overlays |  |
| **Business / professional UI** | | | |
| 23 | **CRM contact manager** | Tables, forms, sorting, filtering | built — `examples/productivity/crm-contact-manager/` |
| 24 | **Kanban board** | Drag/drop, columns, cards | built — `examples/productivity/kanban/` |
| 25 | **Spreadsheet mini-clone** | Editable grid, formulas, keyboard navigation | built — `examples/productivity/spreadsheet/` |
| 26 | **Restaurant POS** | Touch UI, cart/order state, modal flows |  |
| 27 | **Airline booking UI** | Search forms, results, seat selection |  |
| 28 | **Hospital patient chart** | Tabs, timelines, dense structured information |  |
| 29 | **Inventory warehouse UI** | Tables, scanning-style workflows, status |  |
| 30 | **E-commerce storefront** | Product grids, cart, filters, responsive layout |  |
| 31 | **Banking app** | Account cards, transaction feeds, money formatting |  |
| 32 | **Stock trading screen** | Streaming-ish data, charts, order forms |  |
| **Media & creative tools** | | | |
| 33 | **Drawing / paint app** | Pointer input, canvas rendering, tools | built — `examples/productivity/paint/` |
| 34 | **Vector graphics editor** | Selection, handles, transforms, layering | built — `examples/productivity/vector-editor/` |
| 35 | **Photo adjustment UI** | Sliders, image effects, before/after | built — `examples/productivity/photo-adjust/` |
| 36 | **Music sequencer** | Timeline/grid interaction, playback state |  |
| 37 | **Video timeline editor** | Scrubbing, tracks, resizing, drag/drop |  |
| 38 | **Node-based editor** | Arbitrary positioning, ports, connections |  |
| 39 | **Markdown editor + preview** | Text input, parsing, split panes |  |
| 40 | **Presentation editor** | Direct manipulation, layers, editable text |  |
| **Data visualization / dashboards** | | | |
| 41 | **Analytics dashboard** | Cards, charts, responsive grids | built — `examples/dashboards/analytics-dashboard/` |
| 42 | **Live server-monitoring dashboard** | Streaming data, sparklines, alerts | built — `examples/dashboards/server-monitoring/` |
| 43 | **Personal finance dashboard** | Pie/bar/line charts, drill-down | built — `examples/dashboards/finance-dashboard/` |
| 44 | **Interactive election map** | SVG/vector maps, hover, colors, tooltips |  |
| 45 | **Network graph explorer** | Force-directed layout, zoom/pan, selection |  |
| 46 | **Timeline explorer** | Zooming time axis, events, filtering |  |
| **Creative / graphical experiments** | | | |
| 47 | **Boids / flocking simulation** | Real-time simulation, many animated entities | console — `examples/challenge/boids.ptl` |
| 48 | **Fractal explorer** | Custom rendering, zoom/pan, computation |  |
| 49 | **Procedural terrain generator** | Noise, parameters, realtime graphical updates | console — `examples/challenge/terrain.ptl` |
| 50 | **Interactive solar system** | Hierarchical transforms, animation, zoom, labels |  |
