# gpp-python — Python GPP apps

`gpp/` is a **stdlib-only** Python client library for the Garden Pane
Protocol v2 (`../docs/gpp.md`), mirroring the Rust `petal-query` API: a
`Provider` with `query` / `on_mutation` / `on_emit` / `on_navigate` handlers
(positional or decorator), a `PanelUi` (pane name + Petal drawer + declared
screens), `background()` for work that must not stall the pane, `TestHarness`
for protocol-level tests, and `serve()` running the whole stdio loop. No
runtime dependencies, ever — Python ≥ 3.9 and `git`/`ps` are all the apps
need. Import it from the source tree (`sys.path.insert`, what the in-tree
apps do) or install it (`pip install -e garden/gpp-python`).

```
gpp/
  protocol.py    envelopes, error codes, Init, Ctx, script_args
  cache.py       CachePolicy, Reply
  provider.py    Provider — registration + the public dispatch calls
  panel.py       PanelUi
  sink.py        ScriptSink (thread-safe pushes) + the drawer watcher
  background.py  background() / run_in_background() — off-loop handlers
  serve.py       the protocol loop (serve / serve_on)
  testing.py     TestHarness — real serve sessions over in-memory streams
```

The build-your-own guide is
[`../docs/writing-gpp-apps-python.md`](../docs/writing-gpp-apps-python.md);
the wire spec is [`../docs/gpp.md`](../docs/gpp.md).

## The apps

Each app is a directory holding `app.py` (the provider) and a colocated
`.ptl` drawer (the UI, built on the `ui` component library —
`../../petal-ui/docs/components.md`).

- **`sysmon/`** — live process/CPU/memory monitor from `ps aux`. The
  sortable table's click-to-sort header re-keys `query("procs",
  "<field>:<dir>")`, so sorting happens Python-side; a `maxAge 1s` +
  `staleWhileRevalidate 10s` policy keeps it live with no spinner flicker.
- **`repo-stats/`** — git dashboard for the launch directory (first arg,
  else cwd): commits-per-week bars, top authors, recent commits — one
  `git log` pass behind `query("stats", "")`.

## Running

```bash
cd garden && cargo build -p garden-app     # the host

./target/debug/garden --subprocess python3 gpp-python/sysmon/app.py
./target/debug/garden --subprocess python3 gpp-python/repo-stats/app.py ~/petal
```

Or as a `process` node in any layout script:

```petal
layout(process("python3", ["/abs/path/to/gpp-python/sysmon/app.py"]))
```

Append `--dev` to an app's args to hot-reload its drawer on save (the
library watches the `.ptl` file and re-pushes `setScript`; panel `state` and
the query cache survive the reload).

## Testing

```bash
python3 gpp-python/test_gpp.py                    # library unit tests (in-memory streams)
cd garden && node tools/python-gpp-integration-test.ts   # boots garden headless with both apps
```

## Writing a new app

Copy `sysmon/`, then: register a `query` handler per data kind (return
`Reply.json(value).max_age(...)` or raise `AppError`; wrap anything slow in
`background(...)`), point `PanelUi.from_file` at your drawer, and call
`serve(provider, ui)`. The full
walkthrough — including cache policy choice, the JSON→Petal data shape, and
headless verification over the debug server — is
`../docs/writing-gpp-apps-python.md`.
