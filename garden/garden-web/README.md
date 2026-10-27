# garden-web

Garden's Petal panel runtime in the browser: the GPU-free half of
`garden petal-ide`. A sketch runs every frame on `garden-script`'s
`PanelHost`, traced so every shape resolves to the call that drew it, and
checkpointed so a gesture on a shape is answered by direct manipulation v2
(the frame re-run speculatively with each number nudged, then solved). The
page supplies the editor and paints the draw commands on a `<canvas>`.

It powers the direct manipulation gallery on petal-lang.org
(`/manipulate/`).

## Build

```bash
./build-wasm.sh          # → pkg/garden_web.js + garden_web_bg.wasm + BUILD_INFO
```

`petal-lang.org` vendors `pkg/` with `pnpm build:sdk` (into
`frontend/public/garden/`) and checks its gallery samples against it with
`node scripts/check-dm-samples.mjs`.

## API

One class, `GardenWeb`, trading JSON strings (see `src/lib.rs` for the
shapes). Spans are 0-based `[line, col, end_line, end_col]`.

| Call | What it does |
|---|---|
| `new GardenWeb(source)` | compile; throws the compile error |
| `set_size`, `set_mono_advances` | canvas size; the page font's advance ratios, so `text_width` matches what is drawn |
| `mouse_move`, `mouse_button`, `scroll`, `key`, `text` | petal-ui input events |
| `frame(time, dt, n)` | run a frame: `{cmds, error, skipped}` |
| `reload(source)` / `reset(source)` | hot reload keeping `state` / restart |
| `hit(x, y)`, `trace(i)`, `trace_all()` | the shape under a point; the call that drew it |
| `pin()`, `begin(i, property, filter)` | press, then open a gesture: `property` is `position`, `size` or `color`; `filter` is `{allow, deny}` in `:DragAllow` syntax |
| `preview_text(source, deltas)` | the linear prediction for a move, written into the press-time source |
| `finish(deltas)`, `alternative_text(source, k)` | release: ranked, verified alternatives; write the k-th |
| `cancel()` | abandon a press or gesture |
| `hint_literals(line)`, `mark_hint(line, col, to)` | `tunable` / `rigid` hints as text (`garden_script::dm_hints`) |

`wasm32-unknown-unknown` has no clock: the page passes `time` to every
frame (from `performance.now()`), and the DM engine caps a session by run
count only.
