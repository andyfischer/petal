# Writing GPP apps in Python

A GPP app is a subprocess that drives a Garden pane: it pushes a Petal UI
script Garden runs in-process, then serves that script's data over
[GPP](gpp.md) on stdio. This guide is the Python counterpart of
[writing-gpp-apps.md](writing-gpp-apps.md). Read that one for the concepts
that are language-independent (the browser/server model, the JSON-to-Petal
data shape, cacheability, drawer patterns, headless verification) and this
one for the Python specifics.

The library is `gpp-python/gpp/`: stdlib-only, no dependencies, Python 3.9
or later. It mirrors the Rust `petal-query` API: a `Provider` with per-kind
handlers, a `PanelUi` naming the pane and carrying the drawer, and `serve()`
running the whole protocol loop. Import it from the source tree or install
it:

```python
sys.path.insert(0, ".../gpp-python")   # what the in-tree apps do
pip install -e garden/gpp-python       # or installed
from gpp import PanelUi, Provider, Reply, serve
```

The two in-tree apps, `gpp-python/sysmon` (live process monitor) and
`gpp-python/repo-stats` (git dashboard), are the reference implementations;
copy one to start. Running and testing them is covered in
[gpp-python/README.md](../gpp-python/README.md).

## Anatomy of an app

A directory with two files:

```
my-app/
  app.py       # the provider: answers query/mutate/navigate over the pipe
  my_app.ptl   # the drawer: the Petal UI script Garden runs each frame
```

The smallest complete app:

```python
#!/usr/bin/env python3
import os, sys

# the gpp package lives one directory up (gpp-python/); no install needed.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from gpp import PanelUi, Provider, Reply, serve

HERE = os.path.dirname(os.path.abspath(__file__))

def fortune(state, ctx):
    return Reply.json({"text": "you will ship today"}).max_age(60.0)

provider = Provider().query("fortune", fortune)
ui = PanelUi.from_file("fortune", os.path.join(HERE, "my_app.ptl"))
serve(provider, ui, watch="--dev" in sys.argv)
```

And a drawer that reads it (see
[petal-graphical-panels.md](petal-graphical-panels.md) for the draw and
input API and [petal-ui/docs/components.md](../../petal-ui/docs/components.md)
for the widgets):

```petal
state ls = load_state()
let T = ui_theme()
clear(T.bg.r, T.bg.g, T.bg.b)
ls = load_poll(ls, "fortune", query("fortune", ""))
if draw_load(rect(0, 0, screen_width(), screen_height()), ls) then
  draw_text(ls.value.text, {x: 16, y: 16}, T.font_md, T.text)
end
```

Never print to stdout; every stdout line must be a protocol message. stderr
is yours for logging.

## The API

### `Provider(build_state=None)`

`build_state(init)` builds your per-run state from the handshake
(`init.args`, `init.cwd`, `init.repo_arg()` for the first launch arg or else
the cwd). Handlers take `(state, ctx)`:

| Registration | Contract |
| --- | --- |
| `.query(kind, handler)` | return a `Reply`, or any plain JSON-able value (wrapped as `Reply.json`). An unregistered kind answers `null`. |
| `.on_mutation(name, handler)` | effectful, response-carrying, never cached. An unregistered name is an APP error. |
| `.on_emit(event, handler)` | fire-and-forget; return nothing. Unknown events are skipped. |
| `.on_navigate(handler)` | replaces the declared-screens lookup; return the target screen's source string. Runs on every visit (back and forward re-issue `navigate`), so keep it idempotent. |
| `.background_query(kind, handler)` | a slow `query` handler run off the serve loop; see [Long-running work](#long-running-work). |

Registering a host-owned mutation name (`open_path`, `open_project`,
`open_pr`, `open_file_dialog`) raises `ValueError`: Garden answers those
itself. Use `sink.open_path(path)` instead.

Every registration also works as a decorator, and the decorator hands the
plain function back so it stays unit-testable:

```python
provider = Provider(pick_repo)

@provider.query("stats")
def stats(repo, ctx):
    return Reply.json(collect(repo)).max_age(5.0)

@provider.mutation("apply")
def apply_edits(repo, ctx):
    return Reply.json(write(repo, ctx.arg))

@provider.emit("divider")
def remember_divider(repo, ctx):
    save_pref("divider", ctx.arg["pos"])

@provider.navigate
def navigate(repo, ctx):
    return read_screen(ctx.screen)
```

`ctx` carries `.arg` (any JSON value), `.arg_str()` (the common string case;
Petal's script-side `query` passes a string), `.init`, `.sink`, and
whichever of `.kind` / `.name` / `.event` / `.screen` applies.

**Errors.** Raise `AppError("not a git repo: …")` from any handler for a
clean APP error the script reads via `error_of`. Any other exception is also
converted to an APP error (type name prefixed, traceback to stderr), so a
handler bug degrades to an in-pane message rather than a wedged pane.

**Dispatch without a session.** The public entry points run one handler in
one call, the cheapest way to unit-test it:

```python
state = provider.build(Init({"args": ["/repo"], "cwd": "/repo"}))
reply = provider.answer(state, Ctx(init, "", kind="stats"))     # -> Reply
reply = provider.mutate(state, Ctx(init, arg, name="apply"))    # -> Reply
provider.handle_emit(state, Ctx(init, arg, event="divider"))    # -> None
source = provider.navigate(state, Ctx(init, arg, screen="d.ptl"))  # -> str | None
provider.has_mutation("apply")   # also has_query / has_emit / has_navigate
```

### `Reply` and `CachePolicy`

```python
Reply.json(value)                          # fresh forever (until invalidate)
Reply.json(value).max_age(3.0)             # refresh after 3 s (durations in seconds)
Reply.json(value).no_store()               # live data: always revalidate
Reply.json(value).max_age(1.0).stale_while_revalidate(10.0)  # serve stale while refetching
Reply.json(value).cache(CachePolicy.max_age(1.0).stale_while_revalidate(10.0))  # the same
Reply.json(value).forever()                # the default, stated explicitly
Reply.error("upstream failed")             # an APP error response
Reply.loading()                            # a background thread will fill it in
Reply.from_result(lambda: git_log(repo))   # value, or the raised error
```

`Reply.from_result(x)` invokes a callable and turns whatever it raises into
the error; an exception instance becomes `Reply.error`, a `Reply` passes
through, anything else becomes `Reply.json`.

The policy table is in
[writing-gpp-apps.md](writing-gpp-apps.md#cacheability); the wire form in
[gpp.md](gpp.md#cache-control). `Reply.loading()` pairs with
`sink.invalidate(kind, arg)`: acknowledge now, compute on a thread, push the
invalidate when the answer is ready.

Numbers keep their kind in the script: `json` writes `7` as an int and `3.0`
as a float. Send a ratio or a rate as a Python float. Integer division is
not a concern here, since `a / b` is already a float.

### `PanelUi`

```python
ui = PanelUi.from_file("my-app", path)          # remembers the path (enables watch=True)
ui = PanelUi("my-app", source_string)           # or inline source
ui.screen("detail.ptl", detail_source)          # declare navigable screens (the allowlist)
ui.screen_from_file("detail.ptl", path)
PanelUi.from_file("db", path, title=lambda st: f"db — {st.filename}")  # name from state
```

### `serve(provider, ui, on_ready=None, watch=False)`

Runs the protocol loop on stdio until `shutdown` or EOF. It performs the v2
handshake (refusing a non-v2 host with `PROTOCOL_MISMATCH`), pushes the
script, then answers requests. Unknown requests get `METHOD_NOT_FOUND`;
unknown notifications are ignored.

`on_ready(sink)` hands you a `ScriptSink` once the panel is up: the
thread-safe channel for saying something unprompted.

```python
def on_ready(sink):
    def poll():
        while True:
            time.sleep(2.0)
            if something_changed():
                sink.invalidate("log", "")        # Garden drops the key; the script re-queries
    threading.Thread(target=poll, daemon=True).start()

serve(provider, ui, on_ready=on_ready)
```

- `sink.set_script(source)`: hot-reload the drawer (panel `state` and the
  query cache survive; a bad compile leaves the old program running).
- `sink.invalidate(kind, arg="")`: how a watcher or poller publishes fresh
  data.
- `sink.emit(event, arg)` / `sink.status(text)`: client-to-host events;
  Garden acts on the reserved `open_path` and `status` names.
- `sink.open_path(path)`: the pane becomes an editor on that file. This ends
  the session, so it is the last thing an app says.

Handlers get the sink as `ctx.sink`.

### Long-running work

The serve loop is single-threaded and Garden waits on it. A handler that
shells out for two seconds blocks every other query for those two seconds,
including the ~200 ms window Garden gives a freshly spawned client to prime
its first frame. Never do slow work inline.

`background(handler)` wraps a slow handler so the pipe is never held:

```python
from gpp import Provider, Reply, background

def index_repo(repo, ctx):          # takes seconds; runs off the loop
    return Reply.json(walk(repo, ctx.arg_str())).max_age(30.0)

provider = Provider(pick_repo).query("index", background(index_repo))

@provider.background_query("index")   # or the decorator form
def index(repo, ctx): ...
```

Per key: the first `query` answers `Reply.loading()` immediately; the
handler runs on a daemon thread; when it lands, the worker pushes
`invalidate` for that key; the re-query gets the real answer with the
handler's cache policy. Queries for the same key while the work is in flight
coalesce onto one job. Pair it with a `max_age` or `forever` policy; a
`no_store` answer would restart the job on every re-ask.

The handler runs off the serve loop, so anything it touches on shared state
must be thread-safe; the usual shape (read inputs from `ctx`, compute,
return a fresh value) already is.

For work kicked off from a mutation or an emit ("rebuild the index, then
refresh the view"), `run_in_background` fires a thread and invalidates a key
when it returns:

```python
from gpp import run_in_background

@provider.mutation("reindex")
def reindex(repo, ctx):
    run_in_background(ctx.sink, lambda: rebuild(repo), "index", "")
    return Reply.json("reindexing…")     # the status line, right now
```

### Hot reload (`watch`)

`serve(provider, ui, watch=True)` watches the `PanelUi.from_file` path and
re-pushes the drawer on save. The in-tree apps gate it on a `--dev` launch
arg so production launches do not poll the filesystem.

## Launching it

A `process` node in a layout script, or `--subprocess` for a whole-window
run:

```bash
garden --subprocess python3 gpp-python/sysmon/app.py
```

```petal
layout(process("python3", ["/abs/path/to/app.py", "/repo/to/serve"]))
```

The args list becomes both the process argv and `initialize`'s `args`. One
Python-specific gotcha: because the command is `python3`, your script's own
path arrives as `args[0]`. Use `script_args(init)`, which strips it, instead
of `init.repo_arg()` when reading positional args:

```python
def pick_repo(init):
    args = [a for a in script_args(init) if not a.startswith("-")]
    return args[0] if args else init.cwd
```

Use absolute paths for the script: the pane's cwd is Garden's, not your app
directory (which is why the in-tree apps resolve their drawer via
`__file__`).

## Verifying it

Exactly as in [writing-gpp-apps.md](writing-gpp-apps.md#step-6-verifying-it):
boot `garden --headless --debug-port 0 --init <layout.ptl>` and assert on
`/state` under `panes[0].panel.values`. Naming a value in the drawer is what
exposes it (`sysmon.ptl` binds `ready`, `row_count`, and `proc_count` for
this reason). `tools/python-gpp-integration-test.ts` is the worked harness.

Below Garden, `gpp.TestHarness` runs your provider through real serve
sessions over in-memory streams, so tests assert on the same envelopes
Garden will see:

```python
from gpp import TestHarness

h = TestHarness(provider, ui, args=["/repo"], cwd="/repo")

assert h.query("stats").value()["total"] == 42
assert h.query("stats").cache() == {"maxAgeMs": 5000}
assert "not a git repo" in h.query("stats").error_message()
assert h.mutate("apply", {"edits": []}).value() == "wrote 2 files"
assert h.navigate("detail.ptl", {"id": 7}).source() == DETAIL_SRC
assert h.query("slow").is_loading()
h.emit("divider", {"pos": 240})                  # notifications answer nothing
```

Each call is its own session (handshake, request, shutdown). `h.send(*reqs)`
runs several requests in one session when they must share state,
`h.handshake()` returns just the initialize response and the script push,
and every outcome exposes `.pushed("invalidate")` / `.pushed("emit")` for
what the handler said unprompted.

For a single handler with no protocol involved, call the provider's dispatch
(`provider.answer(state, ctx)` and friends). `gpp-python/test_gpp.py` is the
worked example of both styles.

## Checklist

1. A directory with `app.py` and a colocated `.ptl` drawer; import `gpp` via
   the `sys.path.insert` shim or `pip install -e garden/gpp-python`.
2. Register a `.query(kind, …)` handler per data kind; pick a cache policy
   per kind; raise `AppError` for clean failures; wrap anything slow in
   `background(...)`.
3. `PanelUi.from_file`, then `serve(provider, ui, watch="--dev" in sys.argv)`.
4. Drawer: `ui_theme()` colors, the component library, `load_state` plus
   `load_poll` around `query`, and named bindings for whatever a test should
   read.
5. Tests: `TestHarness` for the wire, `provider.answer(...)` for one handler,
   the headless debug server for the whole pane.
6. Launch via `process("python3", ["/abs/app.py", …])`; never print to
   stdout.

## Rust to Python

The two client libraries are the same API with each language's ergonomics.

| `petal-query` (Rust) | `gpp` (Python) | Notes |
| --- | --- | --- |
| `Provider::new(build_state)` | `Provider(build_state)` | |
| `Provider::stateless()` | `Provider.stateless()` / `Provider()` | state is `None` |
| `.query("k", \|s, ctx\| …)` | `.query("k", handler)` / `@provider.query("k")` | |
| `.on_mutation("n", …)` | `.on_mutation("n", …)` / `@provider.mutation("n")` | host-owned names raise `ValueError` |
| `.on_emit("e", …)` | `.on_emit("e", …)` / `@provider.emit("e")` | |
| `.on_navigate(…)` | `.on_navigate(…)` / `@provider.navigate` | |
| `provider.build` / `answer` / `mutate` / `handle_emit` / `navigate` | same names | Python also catches handler exceptions here |
| `Reply::json(v)` / `Reply::error(m)` / `Reply::loading()` | `Reply.json(v)` / `Reply.error(m)` / `Reply.loading()` | |
| `Reply::from_result(Result<T, E>)` | `Reply.from_result(callable \| exception \| value)` | Python has no `Result` |
| `.max_age(Duration)` / `.no_store()` / `.forever()` | `.max_age(seconds)` / `.no_store()` / `.forever()` | seconds, as floats |
| `CachePolicy::max_age(d).stale_while_revalidate(d)` | same, or fluent on the `Reply` | |
| `Err(String)` from a handler | `raise AppError(msg)` | any other exception is also an APP error |
| `PanelUi::new(name, src).screen(n, src)` | `PanelUi(name, src).screen(n, src)` | `.from_file` / `.screen_from_file` read from disk |
| `gpp::serve(provider, ui)` | `serve(provider, ui)` | `serve_on(…, reader, writer)` for streams |
| `ScriptSink` | `ScriptSink` | Python adds `status()` and `open_path()` |
| a spawned thread plus `invalidate` | `background(handler)` / `run_in_background(…)` | |
| `#[test]` over `serve_core` | `gpp.TestHarness` | ships in the library |

Python-only: `script_args(init)` and `ctx.sink`.
