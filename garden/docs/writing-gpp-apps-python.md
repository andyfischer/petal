# Writing GPP apps in Python

A **GPP app** is a subprocess that drives the content of a Garden pane: it
pushes a **Petal UI script** the host runs in-process, then serves that
script's data over a small JSON-RPC protocol on stdio (GPP **v2** —
[`gpp.md`](gpp.md) is the wire spec). This guide is the Python counterpart of
[`writing-gpp-apps.md`](writing-gpp-apps.md): read that one for the concepts
that are language-independent — the browser/server mental model, the
JSON → Petal data shape, cacheability, drawer patterns, and headless
verification — and this one for the Python specifics.

The library is **`gpp-python/gpp/`** — a stdlib-only package (json / os / sys
/ threading / time), zero dependencies, Python ≥ 3.9 — mirroring the Rust
`petal-query` API: a `Provider` with per-kind handlers, a `PanelUi` naming
the pane and carrying the drawer, and `serve()` running the whole protocol
loop (handshake, framing, response plumbing). Import it either way:

```python
sys.path.insert(0, ".../gpp-python")   # a source tree; what the in-tree apps do
pip install -e garden/gpp-python       # or installed (pyproject.toml, no deps)
from gpp import PanelUi, Provider, Reply, serve
```

The two in-tree apps, **`gpp-python/sysmon`**
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
| `.background_query(kind, handler)` | A slow `query` handler run off the serve loop — see [Long-running work](#long-running-work). |

Registering the host-owned mutation names (`open_path`, `open_project`,
`open_pr`, `open_file_dialog`) raises `ValueError`: Garden answers those
itself, so a handler for one could never fire. Use `sink.open_path(path)`
instead.

**Decorator form.** Every registration also works as a decorator, so a
handler is declared where it is defined (and the decorator hands the plain
function back, so it stays unit-testable on its own):

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

**Dispatch, without a session.** The four public entry points run one handler
in one call — the cheapest way to unit-test one:

```python
state = provider.build(Init({"args": ["/repo"], "cwd": "/repo"}))
reply = provider.answer(state, Ctx(init, "", kind="stats"))     # -> Reply
reply = provider.mutate(state, Ctx(init, arg, name="apply"))    # -> Reply
provider.handle_emit(state, Ctx(init, arg, event="divider"))    # -> None
source = provider.navigate(state, Ctx(init, arg, screen="d.ptl"))  # -> str | None
provider.has_mutation("apply")   # also has_query / has_emit / has_navigate
```

An exception inside a handler never escapes these: it comes back as an error
`Reply` exactly as it would on the wire.

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
Reply.json(value).max_age(1.0).stale_while_revalidate(10.0)  # the same, fluent
Reply.json(value).forever()                # the default, stated explicitly
Reply.error("upstream failed")             # an APP error response
Reply.loading()                            # a background thread will fill it in
Reply.from_result(lambda: git_log(repo))   # value, or the raised error
```

`Reply.from_result(x)` is Python's stand-in for Rust's
`Reply::from_result(Result<T, E>)`: a **callable** is invoked and whatever it
raises becomes the error, an **exception instance** becomes `Reply.error`, a
`Reply` passes through, anything else becomes `Reply.json`.

The two max-age builders compose in **either order** — `.max_age(...)` keeps a
stale window already set, unlike `CachePolicy.max_age`, which builds a fresh
policy.

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
- `sink.open_path(path)` — the reserved `open_path` event, path absolutized.
  This **ends the session** (the pane becomes a normal editor on that file),
  so it is the last thing an app says.

Handlers get the sink too, as `ctx.sink`, so a query or a mutation can push a
status line or hand work to a thread without threading the sink through your
own state.

<a name="long-running-work"></a>
### Long-running work

The serve loop is single-threaded and the host waits on it. A handler that
shells out for two seconds blocks **every** other query for those two seconds
— including the ~200 ms window the host gives a freshly spawned client to
prime its first frame, so a slow first query paints an empty pane. Never do
slow work inline.

`background(handler)` wraps a slow handler so the pipe is never held:

```python
from gpp import Provider, Reply, background

def index_repo(repo, ctx):          # takes seconds; runs off the loop
    return Reply.json(walk(repo, ctx.arg_str())).max_age(30.0)

provider = Provider(pick_repo).query("index", background(index_repo))

@provider.background_query("index")   # …or the decorator form
def index(repo, ctx): ...
```

What happens per key:

1. the first `query("index", arg)` answers `Reply.loading()` **immediately** —
   the drawer keeps its own spinner (`draw_load`) and the loop moves on;
2. the handler runs on a daemon worker thread;
3. when it lands, the worker pushes `invalidate("index", arg)` through the
   thread-safe sink, so the host re-queries that exact key;
4. the re-query gets the real answer, with the cache policy the handler chose.

Queries for the same key while the work is in flight coalesce onto the one
job, so a re-asking host does not fan out threads. Pair it with a `max_age`
(or `forever`) policy — a `no_store` answer is re-asked constantly and would
restart the job every time.

The handler runs off the serve loop, so anything it touches on the shared
state must be safe to touch from another thread; the usual shape (read inputs
from `ctx`, compute, return a fresh value) already is.

For work kicked off from a *mutation* or an *emit* — "rebuild the index, then
refresh the view" — use the unstructured form, which fires a thread and
invalidates a key when it returns:

```python
from gpp import run_in_background

@provider.mutation("reindex")
def reindex(repo, ctx):
    run_in_background(ctx.sink, lambda: rebuild(repo), "index", "")
    return Reply.json("reindexing…")     # the status line, right now
```

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
copy its shape for your own app's test.

Below the host, `gpp.TestHarness` runs your provider through **real serve
sessions** over in-memory streams, so your tests assert on the same envelopes
the host will see — no host, no subprocess, no fixture to write:

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

Each call is its own session (handshake → request → shutdown). `h.send(*reqs)`
runs several requests in **one** session when they must share the built state,
`h.handshake()` returns just the initialize response and the script push, and
every `Outcome` also exposes `.pushed("invalidate")` / `.pushed("emit")` for
what the handler said unprompted. `gpp.testing` also exports the raw pieces
(`run`, `init_req`, `req`, `notif`, `by_id`) if you want the envelopes
directly.

For a single handler with no protocol involved, skip the harness and call the
provider's dispatch (`provider.answer(state, ctx)` and friends) — one call,
no streams. `gpp-python/test_gpp.py` unit-tests the library itself and is the
worked example of both styles.

## Checklist

1. A directory with `app.py` + a colocated `.ptl` drawer; import `gpp` via
   the `sys.path.insert` shim, or `pip install -e garden/gpp-python`.
2. Register a `.query(kind, …)` handler per data kind (decorator or
   positional); pick a `CachePolicy` per kind; raise `AppError` for clean
   failures; wrap anything slow in `background(...)`.
3. `PanelUi.from_file`, then `serve(provider, ui, watch="--dev" in sys.argv)`.
4. Drawer: `ui_theme()` colors, the component library, `load_state` +
   `load_poll` around `query`, and named bindings for whatever a test should
   read.
5. Tests: `TestHarness` for the wire, `provider.answer(...)` for one handler,
   the headless debug server for the whole pane.
6. Launch via `process("python3", ["/abs/app.py", …])`; verify over the
   debug server; never print to stdout.

## Rust ↔ Python

The two client libraries are the same API with each language's ergonomics.
`petal-query` (Rust) is the reference; this is the mapping.

| `petal-query` (Rust) | `gpp` (Python) | notes |
| --- | --- | --- |
| `Provider::new(build_state)` | `Provider(build_state)` | state built from the handshake |
| `Provider::stateless()` | `Provider.stateless()` / `Provider()` | state is `None` |
| `.query("k", \|s, ctx\| …)` | `.query("k", handler)` / `@provider.query("k")` | |
| `.on_mutation("n", …)` | `.on_mutation("n", …)` / `@provider.mutation("n")` | host-owned names raise `ValueError` |
| `.on_emit("e", …)` | `.on_emit("e", …)` / `@provider.emit("e")` | |
| `.on_navigate(…)` | `.on_navigate(…)` / `@provider.navigate` | |
| `provider.build(&init)` | `provider.build(init)` | |
| `provider.answer(&mut s, &ctx)` | `provider.answer(state, ctx)` | Python also catches handler panics/exceptions here |
| `provider.mutate(…)` / `handle_emit(…)` / `navigate(…)` | same names | `navigate` returns the source or `None` |
| `provider.has_mutation(n)` | `provider.has_mutation(n)` | plus `has_query` / `has_emit` / `has_navigate` |
| `Reply::json(v)` / `Reply::value(v)` | `Reply.json(v)` / `Reply.value(v)` | |
| `Reply::error(m)` / `Reply::loading()` | `Reply.error(m)` / `Reply.loading()` | |
| `Reply::from_result(Result<T, E>)` | `Reply.from_result(callable \| exception \| value)` | Python has no `Result` |
| `.max_age(Duration)` / `.no_store()` / `.forever()` | `.max_age(seconds)` / `.no_store()` / `.forever()` | seconds, as floats |
| `CachePolicy::max_age(d).stale_while_revalidate(d)` | same, or fluent on the `Reply` | |
| `reply.into_parts()` | `reply.into_parts()` | `(value, error, policy)` |
| `Err(String)` from a handler | `raise AppError(msg)` | any other exception is also an APP error |
| `PanelUi::new(name, src).screen(n, src)` | `PanelUi(name, src).screen(n, src)` | `.from_file` / `.screen_from_file` read from disk |
| `gpp::serve(provider, ui)` | `serve(provider, ui)` | `serve_on(…, reader, writer)` for streams |
| `ScriptSink` (`set_script` / `invalidate` / `emit`) | `ScriptSink`, same methods | Python adds `status()` and `open_path()` |
| `tokio` / a spawned thread + `invalidate` | `background(handler)` / `run_in_background(…)` | see [Long-running work](#long-running-work) |
| `#[test]` over `serve_core` | `gpp.TestHarness` | ships in the library |

Python-only, because the language asks for it: `script_args(init)` (strip the
script's own path from the argv-shaped launch args) and `ctx.sink` (the
handler's handle on the outgoing channel).
