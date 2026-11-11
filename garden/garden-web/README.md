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

It builds `petal` with none of its bundled libraries (`bloom`, `text_layout`,
`glsl`): no gallery sample imports one, and each would add its source to the
download. A sketch that does `import bloom` gets a compile error saying the
package is not in this build. To ship one, turn its feature on with a direct
`petal = { path = "../../core", default-features = false, features = ["bloom"] }`
dependency here.

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
| `move_live(deltas)` | live apply: the prediction for a move written into the press-time source **and hot-reloaded** (state and gesture kept): `{ok, text, rewrites}` or `{ok: false, error}` |
| `preview_text(source, deltas)` | the prediction only, no reload (older flow) |
| `finish(deltas)` | release: ranked, verified alternatives from the press-time source; the first (`applied`) is already reloaded: `{alternatives, candidates, applied, text, prefer}`, `{refused, candidates, text?}` or `{stale}` |
| `pick(k)` | show alternative k of the last release (from the press-time source, reloaded) and make it the sticky choice: `{ok, text, prefer}` |
| `alternative_text(source, k)` | the k-th alternative written into `source`, no reload |
| `preference()`, `set_preference(json)`, `clear_preference()` | the sticky choice as selector labels (`gap`, `at:4:13`, `fn:place`) |
| `cancel()` | abandon a press or gesture; after `move_live` reloads and returns the press-time source |
| `hint_literals(line)`, `mark_hint(line, col, to)` | `tunable` / `rigid` hints as text (`garden_script::dm_hints`) |

### A live gesture

```js
web.pin();                                   // press
web.begin(i, "position", filter);            // past the slop (expensive, once)
// each animation frame while held (coalesce pointer moves):
const live = JSON.parse(web.move_live([dx, dy]));
if (live.ok) editor.setText(live.text);      // the sketch already runs it
draw(JSON.parse(web.frame(t, dt, n)).cmds);  // keep the frame loop running;
                                             // don't feed it the drag's pointer
// release:
const out = JSON.parse(web.finish([dx, dy]));
if (out.text != null) editor.setText(out.text);
showChips(out.alternatives, out.applied);    // `preferred` marks the sticky one
// a chip:
const chosen = JSON.parse(web.pick(k));      // same gesture, the other way; sticky
editor.setText(chosen.text);
```

`reload(text)` of the gesture's own text (its press-time source or the last
`move_live` text) keeps it open; any other text cancels it. The sticky
choice is a soft preference: a later gesture that cannot reach its goal with
those literals ranks as usual. It lives as long as the `GardenWeb` (across
`reload` and `reset`) and is never written into the source.

`wasm32-unknown-unknown` has no clock: the page passes `time` to every
frame (from `performance.now()`), and the DM engine caps a session by run
count only.
