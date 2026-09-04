# `petal-ui-run` — the headless UI driver

`petal run` cannot run a UI app: it dies at `screen_width()`, because the
input/draw contract is the host's job. `petal-ui-run` is that host, with no
renderer attached. It drives `petal_ui::harness::Headless` for N frames,
feeds it a scripted input scenario, and writes one JSON line per frame.

It exists so a UI app's behavior is comparable across a refactor: everything
that would otherwise differ run-to-run has a knob, and the driver pins all of
them (see [refactor-verification.md](refactor-verification.md)).

```
petal-ui-run <app.ptl> [--size WxH] [--frames N] [--seed N]
             [--scenario s.json|monkey:<seed>] [--host-data fixtures.json]
             [--out trace.jsonl] [--error-format full|bare] [-I <dir>]
```

Build it with `cd petal-ui && cargo build`; the binary lands at
`petal-ui/target/debug/petal-ui-run`.

| Flag | Default | Meaning |
|---|---|---|
| `--size WxH` | `800x600` (or the scenario's `size`) | Drawable size bound as `screen_width()`/`screen_height()`. |
| `--frames N` | `60` (or the scenario's `frames`) | How many frames to run. |
| `--seed N` | unseeded (wall clock) | `Env::set_seed` before the first frame — pins `random`, `random_int`, `choose`, and Perlin noise. |
| `--scenario` | none | A scenario file (below) or `monkey:<seed>`. |
| `--host-data` | none | Fixture answers for the `host_data(kind, arg)` native. |
| `--out` | stdout | Where the JSONL trace goes. `-` also means stdout. Safe even for a printing app: `print` does not echo. |
| `--error-format` | `full` | `bare` strips positions and echoed source lines from runtime errors. |
| `-I <dir>` | none | An extra module search directory, repeatable — the CLI's `-I`. For an app that imports a shared library from outside its own directory (`-I petal-libs`). |

Imports resolve relative to the app's own directory, so an app that imports a
sibling module (`examples/games/snake/`-style layouts) runs from any working
directory. A library that lives elsewhere — one of the
[`petal-libs/`](../../petal-libs/README.md) — comes in through `-I`:

```
petal-ui-run examples/ui/bloom-gallery/app.ptl -I petal-libs --frames 120
```

Exit codes: **0** clean, **1** a runtime error in some frame (its record is
written first, with `error` set, and the run stops there), **2** a compile or
usage error (message on stderr, no trace).

## The JSONL record

One object per line, one line per frame, frames numbered from 0:

```json
{"frame": 12,
 "commands": [{"op": "rect", "x": 10, "y": 10, "w": 100, "h": 40, "r": 255, "g": 255, "b": 255}],
 "state": {"score": 3, "dir": "left"},
 "prints": ["spawned at 4,7"],
 "result": null,
 "error": null}
```

- `commands` — the frame's `DrawCommand`s, serialized as the draw protocol
  spells them (`op`-tagged, defaults omitted).
- `state` — every `state` variable, keyed by module-qualified name
  (`Headless::state()`). A slot reached through a call path is keyed by that
  path instead, name last (`counter#1/count`, `[3]/row/hovered`,
  `k1234…/leaf`); top-level names are unchanged. Those key strings are derived
  from names and structure, never from spans, so they are stable run to run and
  across platforms — but an edit that adds, removes or *moves* a call around a
  `state` changes which slots exist and what they are called, so the app's
  `test/ui-golden` hash rotates. That rotation is a real behavior change to
  eyeball, not formatting noise; see
  [refactor-verification.md](refactor-verification.md).
- `prints` — what *this frame* printed, drained per frame.
- `result` — the value the script's top level returned.
- `error` — `null`, or the runtime error message on the frame that failed.

Object keys are emitted in sorted order, which is what makes two traces
byte-comparable with `cmp`.

`print` output appears *only* here. The driver calls `Env::set_echo(false)`
before the first frame, so a script's prints do not also go to the process's
stdout — a trace written to stdout is nothing but JSONL, even for a chatty app.

## Scenario files

A scenario is a declarative list of input events keyed by frame — no scripting
language, so a human can read one, hand-edit it, and check it in beside the app
it drives. `--size`/`--frames` on the command line override the file's.

```json
{ "size": [1280, 850], "frames": 120,
  "events": [
    {"at": 5,  "mouse_move": [640, 400]},
    {"at": 6,  "mouse_down": 0}, {"at": 7, "mouse_up": 0},
    {"at": 9,  "click": [100, 200]},
    {"at": 20, "key": "left"},
    {"at": 25, "key_down": "a"}, {"at": 30, "key_up": "a"},
    {"at": 40, "text": "hello"},
    {"at": 50, "scroll": [0, -3]},
    {"at": 60, "modifiers": {"shift": true}} ] }
```

`at: N` delivers the event to frame N — it is fed to the harness before that
frame runs, so frame N sees its edge. Key names must be canonical
(`petal_ui::input::KEY_NAMES`: `"left"`, not `"ArrowLeft"`); a non-canonical
name is a usage error rather than an event that silently drives nothing. Mouse
buttons are `0`/`1`/`2` or `"left"`/`"right"`/`"middle"`.

Two spellings are shorthand and expand at parse time, matching what
`Headless::click` / `Headless::key` do:

- `click: [x, y]` → `mouse_move` + `mouse_down` at N, `mouse_up` at N+1.
- `key: "name"` → `key_down` + `key_up`, both at N.

### Monkey scenarios

`--scenario monkey:<seed>` generates one instead: pseudo-random clicks inside
the window, keys from the canonical list, and short typed text, spread over the
frame count. It is a plain xorshift over integers, so the same seed gives the
same events on every platform, and a failing run is replayable from
`(app, --seed, monkey seed)` alone.

The generator is `Scenario::monkey(seed, frames, size)` in
`petal-ui/src/scenario.rs`; `Scenario::to_json` writes a generated scenario back
out in the format above, for a repro bundle.

## Determinism

```sh
petal-ui-run examples/games/snake/app.ptl --seed 3 --scenario monkey:7 \
  --frames 120 --size 1280x850 --out /tmp/a.jsonl
```

Run twice, `cmp /tmp/a.jsonl /tmp/b.jsonl` — identical. What is pinned: the
per-frame `dt` and clock (the harness's fixed 1/60 s), input (the scenario),
the RNG (`--seed`), and `host_data` (`--host-data`, which answers nil for any
question a fixture does not cover). `--error-format bare` removes the last
position-dependent text from the trace, so a re-indenting refactor cannot
change an error message.

### The clock

The harness clock is deterministic but not frozen. It starts at `t0 = 0.0`
and advances by the fixed `dt` after every frame, so frame *N* is published to
`time()` as `N / 60` seconds and the clock is exactly what a script summing
`dt()` would hold — a pure function of the frame count, never the system
clock. Two runs of the same script therefore still compare byte-for-byte,
while animation written against the clock (the `ui` prelude's `spinner` and
`elapsed`, a blinking caret spelled `int(time() * 2) % 2`) actually *runs* in
a trace instead of holding one value for 60 frames.

A frame that failed still advances the clock, so a run's timeline depends only
on how many frames were attempted. Embedders driving `Headless` directly can
still assign `ui.time` to jump the clock (a tooltip delay, a long fade); the
assignment is what the next frame publishes, and the automatic advance resumes
from there.

Because the clock moves, a clock-driven app's trace hash is different from what
a frozen clock produced: `test/ui-golden/index.json` was re-baselined for the
nine apps that read `time()`.

## host_data fixtures

A JSON array of answers; a `(kind, arg)` with no entry answers nil.

```json
[ {"kind": "commit", "arg": "abc", "value": {"title": "first", "n": 42}},
  {"kind": "branches", "arg": "", "value": ["main", "dev"]} ]
```

JSON numbers keep their int/float distinction (`42` is an `Int`, `0.42` a
`Float`) — a truncated fraction is unrecoverable downstream.

## Garden panel natives are stubbed

A Garden panel drawer calls host natives beyond the petal-ui contract —
`palette()`, `query`/`invalidate`, `emit`/`mutate`/`claim_key`, the
`navigate` family, `panel_store_get/set`, and the `text_view`/`edit_view`
regions. The driver registers inert, deterministic stand-ins for all of them
(`petal_ui::panel_stubs`), so the drawers under `garden/examples/panels/` and
`garden/gpp-apps/*/src/` run headlessly instead of dying at
`Unknown builtin: palette` on frame 0:

| Native | Stubbed answer |
|---|---|
| `palette()` | Garden's fallback palette — the same colors a themeless Garden resolves |
| `panel_theme()` | `{}` |
| `query(kind, arg)` | a loading pending value, forever — a headless run exercises the drawer's loading/spinner path |
| `invalidate`, `emit`, `claim_key`, `navigate`/`navigate_replace`/`navigate_back`/`navigate_forward`, `panel_store_set` | arguments validated, then dropped |
| `mutate(name, arg)` | a unique handle (1, 2, …); `mutate_result(handle)` answers nil |
| `nav_arg()`, `panel_store_get(key)` | nil |
| `text_view`, `edit_view`, `edit_view_projection`, `text_view_line_styles`, `text_view_scroll_to`, `text_view_wrap` | the same `Host` draw commands Garden's natives emit, so the regions appear in the trace |
| `edit_view_text(id)`, `edit_view_edits(id)` | `""` / `[]` |

Every stub's answer is a pure function of the script's own calls, so traces
stay byte-identical run to run, and an app that never touches these natives is
unaffected. A drawer whose whole UI hides behind a ready `query` shows its
loading state here — deterministic and intended; drive real data through the
garden integration tests instead.

## What runs under it

Every UI app under `examples/dashboards`, `examples/games`, and
`examples/productivity` runs clean under this driver, as do
`examples/custom-integrations/diagram-canvas/examples/*` and, through the
panel-native stubs above, the drawers under `garden/examples/panels/` and
`garden/gpp-apps/*/src/`.

What does not, and why. Each fails as a *runtime* error on the frame that
first reaches a native no host registered (`Unknown builtin: <name>`), so
the frame record carries the name; the driver writes that record and exits 1.

| Corpus | Missing |
|---|---|
| `examples/custom-integrations/petal-fps/examples/*` | `sky_gradient` and the rest of the petal-fps renderer's natives |
| `examples/custom-integrations/petal-fantasy-nes/carts/*` | `set_backdrop` and the cart palette bindings the fantasy-NES host installs |
| `examples/games/side-scroller/editor.ptl` | `load_text_file` (a host filesystem native) |

Those need their own embedder, not this one.
