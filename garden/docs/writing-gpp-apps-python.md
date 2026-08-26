# Writing GPP apps in Python

A **GPP app** is a subprocess that drives the content of a Garden pane: it
pushes a **Petal UI script** the host runs in-process, then serves that
script's data over a small JSON-RPC protocol on stdio (GPP **v2** —
[`gpp.md`](gpp.md) is the wire spec). This guide is the Python counterpart of
[`writing-gpp-apps.md`](writing-gpp-apps.md): read that one for the concepts
that are language-independent — the browser/server mental model, the
JSON → Petal data shape, cacheability, drawer patterns, and headless
verification — and this one for the Python specifics.

The library is **`gpp-python/gpp.py`** — one stdlib-only file (json / sys /
threading), no pip installs — mirroring the Rust `petal-query` API: a
`Provider` with per-kind handlers, a `PanelUi` naming the pane and carrying
the drawer, and `serve()` running the whole protocol loop (handshake,
framing, response plumbing). The two in-tree apps, **`gpp-python/sysmon`**
(live process monitor, sortable table, short-max-age caching) and
**`gpp-python/repo-stats`** (git dashboard: weekly bars, top authors, recent
commits), are the reference implementations — copy one to start.

## Anatomy of an app

A directory with two files:

```
my-app/
  app.py       # the provider: answers query/mutate/navigate over the pipe
  my_app.ptl   # the drawer: the Petal UI script the host runs each frame
```

The smallest complete app:

```python
#!/usr/bin/env python3
import os, sys

# gpp.py lives one directory up (gpp-python/); no packaging needed.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from gpp import PanelUi, Provider, Reply, serve

HERE = os.path.dirname(os.path.abspath(__file__))

def fortune(state, ctx):
    return Reply.json({"text": "you will ship today"}).max_age(60.0)

provider = Provider().query("fortune", fortune)
ui = PanelUi.from_file("fortune", os.path.join(HERE, "my_app.ptl"))
serve(provider, ui, watch="--dev" in sys.argv)
```

…and a drawer that reads it (see
[`petal-graphical-panels.md`](petal-graphical-panels.md) for the draw/input
API and `../../petal-ui/docs/components.md` for the widget library):

```petal
state ls = load_state()
let T = ui_theme()
clear(T.bg.r, T.bg.g, T.bg.b)
ls = load_poll(ls, "fortune", query("fortune", ""))
if draw_load(rect(0, 0, screen_width(), screen_height()), ls) then
  draw_text(ls.value.text, {x: 16, y: 16}, T.font_md, T.text)
end
```

Never print to **stdout** — every stdout line must be a protocol message.
**stderr** is yours for logging (the host inherits it).

## The API

### `Provider(build_state=None)`

`build_state(init)` materializes your per-run state from the handshake
(`init.args`, `init.cwd`, `init.repo_arg()` — the first launch arg, else the
cwd). Handlers all take `(state, ctx)`:

| registration | contract |
| --- | --- |
| `.query(kind, handler)` | Return a `Reply` — or any plain JSON-able value, which is wrapped as `Reply.json(...)`. An unregistered kind answers `null`. |
| `.on_mutation(name, handler)` | Effectful, response-carrying, never cached. An unregistered name is an APP error. Don't register the host-owned names (`open_path`, `open_project`, `open_pr`, `open_file_dialog`) — those never reach you. |
| `.on_emit(event, handler)` | Fire-and-forget; return nothing. Unknown events are skipped. |
| `.on_navigate(handler)` | Replaces the declared-screens lookup: return the target screen's UI **source** string. Runs on every visit (back/forward re-issue `navigate`), so keep it idempotent per visit. |

`ctx` carries `.arg` (**any JSON value** — v2 carries it verbatim),
`.arg_str()` (the common string case: Petal's script-side `query(kind, arg)`
passes a string), `.init`, and whichever of `.kind` / `.name` / `.event` /
`.screen` applies.

**Errors.** Raise `AppError("not a git repo: …")` from any handler for a
clean APP error the script reads via `error_of`. Any *other* exception is
also converted to an APP error (with the type name prefixed, traceback to
stderr) — a handler bug degrades to an in-pane message, never a wedged pane.

### `Reply` and `CachePolicy`

```python
Reply.json(value)                          # fresh forever (until invalidate)
Reply.json(value).max_age(3.0)             # refresh after 3s (durations in seconds)
Reply.json(value).no_store()               # live data: always revalidate
Reply.json(value).cache(
    CachePolicy.max_age(1.0).stale_while_revalidate(10.0))  # serve stale while refetching
Reply.error("upstream failed")             # an APP error response
Reply.loading()                            # a background thread will fill it in
```

The policy table (when to use which) is in
[`writing-gpp-apps.md`](writing-gpp-apps.md#cacheability); the wire form
(`maxAgeMs` / `staleWhileRevalidateMs` / `noStore`) in
[`gpp.md`](gpp.md#cache-control). `Reply.loading()` pairs with
`sink.invalidate(kind, arg)` (below) — acknowledge now, compute on a thread,
push the invalidate when the answer is ready and the host re-queries.

**Numbers keep their kind** in the script: `json` writes `7` as an int and
`3.0` as a float, and Petal recovers the split. Send a ratio or a rate as a
Python float (`round(x, 2)` keeps it one). And mind integer division: it's
Python, so `a / b` is already a float — the Rust-side `pct / 100 == 0`
footgun does not exist here.

### `PanelUi`

```python
ui = PanelUi.from_file("my-app", path)          # remembers the path (enables watch=True)
ui = PanelUi("my-app", source_string)           # or inline source
ui.screen("detail.ptl", detail_source)          # declare navigable screens (the allowlist)
PanelUi.from_file("db", path, title=lambda st: f"db — {st.filename}")  # name from state
```

### `serve(provider, ui, on_ready=None, watch=False)`

Runs the protocol loop on stdio until `shutdown`/EOF — your `main`. It
performs the v2 handshake (refusing a non-v2 host with a clean
`PROTOCOL_MISMATCH`), pushes the script, then answers requests. Unknown
*requests* get `METHOD_NOT_FOUND`; unknown *notifications* are ignored —
the forward-compatibility rule, already handled for you.

`on_ready(sink)` hands you a **`ScriptSink`** once the panel is up — the
thread-safe channel for saying something unprompted (every send is one whole
line under one lock):

```python
def on_ready(sink):
    def poll():
        while True:
            time.sleep(2.0)
            if something_changed():
                sink.invalidate("log", "")        # host drops the key; script re-queries
    threading.Thread(target=poll, daemon=True).start()

serve(provider, ui, on_ready=on_ready)
```

- `sink.set_script(source)` — hot-reload the drawer (panel `state` and the
  query cache survive; a bad compile leaves the old program running).
- `sink.invalidate(kind, arg="")` — how a watcher/poller publishes fresh data.
- `sink.emit(event, arg)` / `sink.status(text)` — client → host events; the
  host acts on the reserved `open_path` and `status` names.

### Hot reload (`watch`)

`serve(provider, ui, watch=True)` watches the `PanelUi.from_file` path and
re-pushes the drawer on save — the edit-and-see loop. The in-tree apps gate
it on a `--dev` launch arg (`watch="--dev" in sys.argv`), so production
launches don't poll the filesystem.

## Launching it

The host spawns a GPP app like any client — a `process` node in a layout
script, or `--subprocess` for a whole-window run:

```bash
garden --subprocess python3 gpp-python/sysmon/app.py            # bare python3, PATH-resolved
```

```petal
layout(process("python3", ["/abs/path/to/app.py", "/repo/to/serve"]))
```

The args list becomes both the process argv and `initialize`'s `args` — so
`"--dev" in sys.argv` and your handlers both see them. **One Python-specific
gotcha**: because the *command* is `python3`, your script's own path arrives
as `args[0]` — unlike a compiled client, whose binary is the command. Use
`script_args(init)` (which strips it) instead of `init.repo_arg()` when
reading positional args, as `repo-stats/app.py` does:

```python
def pick_repo(init):
    args = [a for a in script_args(init) if not a.startswith("-")]
    return args[0] if args else init.cwd
```

Use absolute paths for the script: the pane's cwd is the host's, not your
app directory (which is why the in-tree apps resolve their drawer via
`__file__`).

## Verifying it

Exactly as in [`writing-gpp-apps.md` Step 6](writing-gpp-apps.md#step-6--verifying-it):
boot `garden --headless --debug-port 0 --init <layout.ptl>` and assert on
`/state → panes[0].panel.values` — the host observes every named binding
your drawer makes, so **naming a value is what exposes it** (`sysmon.ptl`
binds `ready` / `row_count` / `proc_count` for exactly this reason).
`tools/python-gpp-integration-test.ts` is the worked harness: it boots both
Python apps headless, waits on their `ready` values, and takes a screenshot —
copy its shape for your own app's test. `gpp-python/test_gpp.py` unit-tests
the library itself over in-memory streams (`serve_on`), which is also the
cheap way to test your handlers without a host:

```python
serve_on(provider, ui, io.StringIO(request_lines), out := io.StringIO())
```

## Checklist

1. A directory with `app.py` + a colocated `.ptl` drawer; import `gpp` via
   the `sys.path.insert` shim (or copy `gpp.py` next to your app — it is one
   file with no dependencies).
2. Register a `.query(kind, …)` handler per data kind; pick a `CachePolicy`
   per kind; raise `AppError` for clean failures.
3. `PanelUi.from_file`, then `serve(provider, ui, watch="--dev" in sys.argv)`.
4. Drawer: `ui_theme()` colors, the component library, `load_state` +
   `load_poll` around `query`, and named bindings for whatever a test should
   read.
5. Launch via `process("python3", ["/abs/app.py", …])`; verify over the
   debug server; never print to stdout.
