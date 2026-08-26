"""gpp — a stdlib-only Python client library for the Garden Pane Protocol (v2).

A GPP app is a subprocess that drives the content of a Garden pane: right
after the handshake it pushes a Petal UI script (the "page") that the host
runs in-process, then acts as that script's data server, answering the
`query` / `mutate` / `navigate` requests the host issues on the running
script's behalf. Only data crosses the pipe; the interaction loop never does.

This module mirrors the Rust client library (`petal-query`): you register a
handler per `query(kind, arg)` on a :class:`Provider` — each returning a
:class:`Reply` with the value (or error) and how cacheable it is
(:class:`CachePolicy`) — and :func:`serve` runs the whole protocol loop.
Nothing here is hand-rolled per app: the stdio handshake, the JSON-RPC
framing, and the response plumbing all live in this one file.

    from gpp import PanelUi, Provider, Reply, serve

    provider = (
        Provider(lambda init: init.repo_arg())
        .query("log", lambda repo, ctx: Reply.json(git_log(repo)).max_age(3.0))
    )
    serve(provider, PanelUi.from_file("git-log", "panel.ptl"))

Wire reference: `garden/docs/gpp.md` (the normative v2 spec). Python how-to:
`garden/docs/writing-gpp-apps-python.md`. This module uses only the standard
library (json / sys / threading / os / time).
"""

from __future__ import annotations

import json
import os
import sys
import threading
import time
import traceback

# The protocol major version this library speaks. Carried by both halves of
# the `initialize` handshake; a host reporting a different major is refused
# with a PROTOCOL_MISMATCH error response.
PROTOCOL_VERSION = 2

# The capability names an app built on this library reports in its
# `initialize` response: the requests it answers and the pushes it makes.
CLIENT_CAPABILITIES = ["query", "mutate", "navigate", "emit", "setScript"]


class ErrorCode:
    """JSON-RPC error codes used in error responses (see gpp.md)."""

    # An application-level failure: the handler ran and failed ("not a git
    # repo", "no such screen"). The message is what the panel script surfaces
    # via `error_of`.
    APP = 1
    # The peer speaks an incompatible protocol major version.
    PROTOCOL_MISMATCH = 2
    # The request's method (or a mutation's name) has no handler.
    METHOD_NOT_FOUND = -32601
    # The request's params did not decode.
    INVALID_PARAMS = -32602


class AppError(Exception):
    """Raise from any handler to answer the request with a clean APP error.

    The message is what the panel script reads via `error_of`. Any *other*
    exception a handler raises is also turned into an APP error (so a bug
    can't wedge the pane), but with the exception type prefixed and the
    traceback printed to stderr.
    """


# ── Envelopes ────────────────────────────────────────────────────────────────
# Every message is a JSON-RPC 2.0 shaped object, one compact JSON object per
# line. These builders return plain dicts; absent fields are simply not set.


def request(req_id, method, params):
    return {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}


def notification(method, params):
    return {"jsonrpc": "2.0", "method": method, "params": params}


def response(req_id, result):
    return {"jsonrpc": "2.0", "id": req_id, "result": result}


def error_response(req_id, code, message):
    return {"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": str(message)}}


# ── Cache control ────────────────────────────────────────────────────────────


class CachePolicy:
    """How cacheable one query answer is — the pull-model cousin of an HTTP
    `Cache-Control` header. Durations are **seconds** (floats fine); they
    cross the wire as whole milliseconds.

    - :meth:`forever` (the default) — fresh until an explicit `invalidate`;
      right for a value addressed by an immutable key (a commit hash).
    - :meth:`max_age` — fresh for that long, then (without a stale window)
      hard-expired: the next query shows a spinner while it refetches.
    - :meth:`stale_while_revalidate` — how long *past* max_age the stale
      answer is still served while a background refetch runs.
    - :meth:`no_store` — never fresh, never expired: always serve the last
      value *and* revalidate. Live data with no spinner flicker.
    """

    def __init__(self, no_store=False, max_age_ms=None, stale_while_revalidate_ms=None):
        self._no_store = no_store
        self._max_age_ms = max_age_ms
        self._swr_ms = stale_while_revalidate_ms

    @classmethod
    def forever(cls):
        return cls()

    # Self-documenting alias: the resource at this (kind, arg) never changes.
    immutable = forever

    @classmethod
    def max_age(cls, seconds):
        return cls(max_age_ms=int(seconds * 1000))

    @classmethod
    def no_store(cls):
        return cls(no_store=True)

    def stale_while_revalidate(self, seconds):
        """Builder on top of :meth:`max_age`: set the stale window."""
        return CachePolicy(self._no_store, self._max_age_ms, int(seconds * 1000))

    def to_wire(self):
        """The `cache` field for a query result, or None for the default
        (a forever policy serializes to nothing — it adds nothing)."""
        out = {}
        if self._no_store:
            out["noStore"] = True
        if self._max_age_ms is not None:
            out["maxAgeMs"] = self._max_age_ms
        if self._swr_ms is not None:
            out["staleWhileRevalidateMs"] = self._swr_ms
        return out or None


# ── Replies ──────────────────────────────────────────────────────────────────


class Reply:
    """A handler's answer to one `query` or `mutate`: a value, an error, or
    "still loading", plus (for queries) a cache policy.

    Build with :meth:`json` / :meth:`error` / :meth:`loading`, then attach a
    policy with :meth:`cache` / :meth:`max_age` / :meth:`no_store` (the
    default is forever). A handler may also return a plain JSON-able value —
    the loop wraps it in ``Reply.json(...)`` — or raise :class:`AppError`.
    """

    def __init__(self, kind, payload, policy=None):
        self._kind = kind  # "value" | "error" | "loading"
        self._payload = payload
        self._policy = policy or CachePolicy.forever()

    @classmethod
    def json(cls, value):
        """A successful answer carrying `value` (any JSON-serializable tree).
        Numbers keep their kind in the script: send ``3.0`` for a float."""
        return cls("value", value)

    @classmethod
    def error(cls, message):
        """A failure: a JSON-RPC APP error response on the wire; the script
        surfaces the message via `error_of`."""
        return cls("error", str(message))

    @classmethod
    def loading(cls):
        """"Still loading": an empty result. The host keeps the script's
        spinner up without re-requesting until the app pushes an
        `invalidate` for the key (see :meth:`ScriptSink.invalidate`)."""
        return cls("loading", None)

    def cache(self, policy):
        self._policy = policy
        return self

    def max_age(self, seconds):
        self._policy = CachePolicy.max_age(seconds)
        return self

    def no_store(self):
        self._policy = CachePolicy.no_store()
        return self


def _query_response(req_id, reply):
    """Map a Reply onto the wire, exactly like petal-query's query_response."""
    if reply._kind == "error":
        return error_response(req_id, ErrorCode.APP, reply._payload)
    result = {}
    if reply._kind == "value":
        result["value"] = reply._payload
    wire = reply._policy.to_wire()
    if wire is not None:
        result["cache"] = wire
    return response(req_id, result)


def _mutate_response(req_id, reply):
    if reply._kind == "error":
        return error_response(req_id, ErrorCode.APP, reply._payload)
    result = {}
    if reply._kind == "value":
        result["value"] = reply._payload
    return response(req_id, result)


# ── Handshake params / handler context ───────────────────────────────────────


class Init:
    """The `initialize` params: how a client learns what to serve."""

    def __init__(self, params):
        self.protocol = params.get("protocol", 1)
        self.pane_id = params.get("paneId", 0)
        self.rows = params.get("rows", 0)
        self.cols = params.get("cols", 0)
        self.args = list(params.get("args") or [])
        self.cwd = params.get("cwd", "")
        self.capabilities = list(params.get("capabilities") or [])

    def repo_arg(self):
        """The first launch arg, or the pane cwd when none was given — the
        common "which directory/target do I operate on?" resolution."""
        return self.args[0] if self.args else self.cwd


def script_args(init, script=None):
    """The launch args without the Python script's own path.

    When the spawn command is ``python3 app.py <args…>`` the host's
    `initialize` args mirror the argv — so, unlike a compiled client (whose
    binary is the command, not an arg), the list's first entry is ``app.py``
    itself. This strips that entry (compared by absolute path; `script`
    defaults to ``sys.argv[0]``) so an app reads only its real arguments:

        args = [a for a in script_args(init) if not a.startswith("-")]
        repo = args[0] if args else init.cwd
    """
    script = os.path.abspath(script or sys.argv[0])
    return [a for a in init.args if os.path.abspath(a) != script]


class Ctx:
    """The context handed to a handler: what was asked, with what argument,
    plus the handshake :class:`Init`. Which name field is set depends on the
    handler kind (`kind` for queries, `name` for mutations, `event` for
    emits, `screen` for navigations)."""

    def __init__(self, init, arg, kind=None, name=None, event=None, screen=None):
        self.init = init
        self.arg = arg  # any JSON value (GPP v2 carries it verbatim)
        self.kind = kind
        self.name = name
        self.event = event
        self.screen = screen

    def arg_str(self):
        """The argument as a string — its content when it is a JSON string,
        "" otherwise. The common case: Petal's script-side `query(kind, arg)`
        passes a string arg."""
        return self.arg if isinstance(self.arg, str) else ""


# ── Provider ─────────────────────────────────────────────────────────────────


class Provider:
    """A query/response provider over a per-run state: `kind` → handler
    registrations plus a `build_state` that materializes the state from the
    handshake. Handlers take ``(state, ctx)``.

    - :meth:`query` handlers return a :class:`Reply` (or a plain value,
      wrapped as ``Reply.json``); an unregistered kind answers ``null``.
    - :meth:`on_mutation` handlers are effectful and return a Reply; an
      unregistered name is an APP error (a mutation is an explicit request).
    - :meth:`on_emit` handlers are fire-and-forget (return nothing).
    - :meth:`on_navigate` replaces the declared-screens lookup: return the
      target screen's UI source, or raise :class:`AppError` to refuse.
    """

    def __init__(self, build_state=None):
        self._build_state = build_state or (lambda init: None)
        self._queries = {}
        self._emits = {}
        self._mutations = {}
        self._navigate = None

    def query(self, kind, handler):
        self._queries[kind] = handler
        return self

    def on_emit(self, event, handler):
        self._emits[event] = handler
        return self

    def on_mutation(self, name, handler):
        self._mutations[name] = handler
        return self

    def on_navigate(self, handler):
        self._navigate = handler
        return self


# ── Panel presentation ───────────────────────────────────────────────────────


class PanelUi:
    """The editor-facing half of an app: the pane's display name and the
    Petal UI script the host runs. `screens` (name → source) declares the
    navigable screens beyond the home script; the declared set doubles as the
    navigation allowlist. `title` (a callable ``state -> str``) derives the
    pane name from the built state instead of the static name."""

    def __init__(self, name, script, screens=None, title=None):
        self.name = name
        self.script = script
        self.screens = dict(screens or {})
        self.title = title
        self.script_path = None  # set by from_file; enables watch=True

    @classmethod
    def from_file(cls, name, path, **kwargs):
        """A panel whose home script is read from `path` (remembered, so
        ``serve(..., watch=True)`` can hot-reload it on change)."""
        with open(path, "r", encoding="utf-8") as f:
            ui = cls(name, f.read(), **kwargs)
        ui.script_path = path
        return ui

    def screen(self, name, source):
        """Declare a navigable screen (fluent, like the Rust PanelUi)."""
        self.screens[name] = source
        return self


# ── The sink: one place messages go out, one whole line at a time ────────────


class ScriptSink:
    """A thread-safe handle for pushing to the host: the serve loop writes
    responses through it, and a background thread (a file watcher, a poller)
    may push `setScript` / `invalidate` / `emit` at any time. Every send
    serializes one complete envelope under one lock, so two threads' messages
    can never interleave inside a line (the transport is one compact JSON
    object per line)."""

    def __init__(self, writer):
        self._writer = writer
        self._lock = threading.Lock()

    def send(self, env):
        data = json.dumps(env, separators=(",", ":"), ensure_ascii=False) + "\n"
        with self._lock:
            try:
                self._writer.write(data)
            except TypeError:
                # A binary writer (e.g. sys.stdout.buffer).
                self._writer.write(data.encode("utf-8"))
            self._writer.flush()

    def set_script(self, source):
        """Push a new UI script, replacing the one the panel is running. The
        host recompiles in place and keeps the panel's `state` and its query
        cache; a source that fails to compile leaves the old program running."""
        self.send(notification("setScript", {"source": source}))

    def invalidate(self, kind, arg=""):
        """Drop the host's cached value for `(kind, arg)` so the script
        re-queries it — how a watcher, a poller, or a finished background job
        publishes a new answer. `arg` must equal the queried arg."""
        self.send(notification("invalidate", {"kind": kind, "arg": arg}))

    def emit(self, event, arg=None):
        """Raise a client → host event. The host acts on the reserved names
        `open_path` ({"path": ...}) and `status` ({"text": ...}); anything
        else is ignored (reserved for future use)."""
        self.send(notification("emit", {"event": event, "arg": arg}))

    def status(self, text):
        """Set the pane's status-bar text (the reserved `status` event)."""
        self.emit("status", {"text": text})


def watch_script(sink, path, interval=0.5):
    """Start a daemon thread that re-pushes `path` as the panel's script
    whenever its mtime changes — the dev-mode hot-reload loop. Returns the
    thread (already started)."""

    def mtime():
        try:
            return os.stat(path).st_mtime_ns
        except OSError:
            return None

    def loop():
        last = mtime()
        while True:
            time.sleep(interval)
            now = mtime()
            if now is not None and now != last:
                last = now
                try:
                    with open(path, "r", encoding="utf-8") as f:
                        sink.set_script(f.read())
                except OSError as e:
                    print(f"gpp: could not reload {path}: {e}", file=sys.stderr)

    t = threading.Thread(target=loop, daemon=True, name="gpp-script-watch")
    t.start()
    return t


# ── The protocol loop ────────────────────────────────────────────────────────


def serve(provider, ui, on_ready=None, watch=False):
    """Run `provider` as a GPP client app on stdio until `shutdown` / EOF,
    presenting it with `ui`. Blocks the calling thread; this is an app's
    ``main``.

    `on_ready(sink)` is called once the panel's script has been pushed, with
    the :class:`ScriptSink` — move it into a background thread to push
    `setScript` / `invalidate` / `emit` unprompted. It must not block.

    ``watch=True`` hot-reloads the home script on file change (the ui must
    come from :meth:`PanelUi.from_file`); ``watch="path.ptl"`` watches an
    explicit path.
    """
    # The wire is UTF-8 JSON lines; don't let a locale decide otherwise.
    try:
        sys.stdin.reconfigure(encoding="utf-8")
        sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    except (AttributeError, OSError):
        pass

    watch_path = None
    if watch:
        watch_path = ui.script_path if watch is True else watch
        if watch_path is None:
            raise ValueError("watch=True needs a PanelUi.from_file (no script path to watch)")

    def ready(sink):
        if watch_path:
            watch_script(sink, watch_path)
        if on_ready:
            on_ready(sink)

    serve_on(provider, ui, sys.stdin, sys.stdout, on_ready=ready)


def serve_on(provider, ui, reader, writer, on_ready=None):
    """:func:`serve` over explicit streams — the seam the tests drive."""
    sink = ScriptSink(writer)
    try:
        _serve_core(provider, ui, reader, sink, on_ready)
    except BrokenPipeError:
        pass  # the host went away; nothing left to say


def _read_envelopes(reader):
    """Yield one parsed envelope per line; stop at EOF. A line that does not
    parse is a protocol error — report it and end the session (the host does
    the same rather than guessing)."""
    for line in reader:
        if not line.strip():
            continue
        try:
            yield json.loads(line)
        except ValueError as e:
            print(f"gpp: unparseable line from host: {e}", file=sys.stderr)
            return


def _handler_reply(handler, state, ctx, label):
    """Run a Reply-returning handler, mapping exceptions to APP errors and
    wrapping a plain returned value in Reply.json."""
    try:
        out = handler(state, ctx)
    except AppError as e:
        return Reply.error(str(e))
    except Exception as e:  # a handler bug must not wedge the pane
        traceback.print_exc()
        return Reply.error(f"{label} raised {type(e).__name__}: {e}")
    return out if isinstance(out, Reply) else Reply.json(out)


def _serve_core(provider, ui, reader, sink, on_ready=None):
    envelopes = _read_envelopes(reader)

    # 1. Handshake: read `initialize`, check the protocol version, build
    #    state, reply — before sending anything else (the host blocks reading
    #    exactly one line here).
    first = next(envelopes, None)
    if first is None or first.get("method") != "initialize":
        return
    init_id = first.get("id", 1)
    params = first.get("params") or {}
    protocol = params.get("protocol", 1)
    if protocol != PROTOCOL_VERSION:
        sink.send(
            error_response(
                init_id,
                ErrorCode.PROTOCOL_MISMATCH,
                f"this app speaks gpp protocol {PROTOCOL_VERSION}, the host sent {protocol}",
            )
        )
        return
    init = Init(params)
    state = provider._build_state(init)

    name = ui.title(state) if ui.title else ui.name
    sink.send(
        response(
            init_id,
            {"protocol": PROTOCOL_VERSION, "name": name, "capabilities": CLIENT_CAPABILITIES},
        )
    )

    # 2. Push the UI script; the host compiles it into a panel.
    sink.send(notification("setScript", {"source": ui.script}))
    if on_ready:
        on_ready(sink)

    # 3. Answer requests until shutdown / EOF.
    for env in envelopes:
        method = env.get("method")
        req_id = env.get("id")
        params = env.get("params") or {}

        if method == "query":
            kind = params.get("kind")
            if not isinstance(kind, str):
                sink.send(error_response(req_id or 0, ErrorCode.INVALID_PARAMS, "query needs a string 'kind'"))
                continue
            ctx = Ctx(init, params.get("arg"), kind=kind)
            handler = provider._queries.get(kind)
            if handler is None:
                # An unregistered kind conventionally answers null, not an error.
                reply = Reply.json(None)
            else:
                reply = _handler_reply(handler, state, ctx, f"query '{kind}'")
            sink.send(_query_response(req_id or 0, reply))

        elif method == "mutate":
            mname = params.get("name")
            if not isinstance(mname, str):
                sink.send(error_response(req_id or 0, ErrorCode.INVALID_PARAMS, "mutate needs a string 'name'"))
                continue
            ctx = Ctx(init, params.get("arg"), name=mname)
            handler = provider._mutations.get(mname)
            if handler is None:
                # A mutation is an explicit request: an unknown one is an error.
                sink.send(error_response(req_id or 0, ErrorCode.APP, f"no mutation handler for '{mname}'"))
                continue
            reply = _handler_reply(handler, state, ctx, f"mutation '{mname}'")
            sink.send(_mutate_response(req_id or 0, reply))

        elif method == "navigate":
            screen = params.get("screen")
            if not isinstance(screen, str):
                sink.send(error_response(req_id or 0, ErrorCode.INVALID_PARAMS, "navigate needs a string 'screen'"))
                continue
            ctx = Ctx(init, params.get("arg"), screen=screen)
            if provider._navigate is not None:
                # A registered handler wins (side effects + source). It runs on
                # every visit — back/forward re-issue navigate — so it should
                # be idempotent per visit.
                try:
                    source = provider._navigate(state, ctx)
                    sink.send(response(req_id or 0, {"screen": screen, "source": source}))
                except AppError as e:
                    sink.send(error_response(req_id or 0, ErrorCode.APP, str(e)))
                except Exception as e:
                    traceback.print_exc()
                    sink.send(error_response(req_id or 0, ErrorCode.APP, f"navigate raised {type(e).__name__}: {e}"))
            elif screen in ui.screens:
                sink.send(response(req_id or 0, {"screen": screen, "source": ui.screens[screen]}))
            else:
                sink.send(error_response(req_id or 0, ErrorCode.APP, f"no such screen '{screen}'"))

        elif method == "emit":
            event = params.get("event")
            handler = provider._emits.get(event) if isinstance(event, str) else None
            if handler is not None:
                try:
                    handler(state, Ctx(init, params.get("arg"), event=event))
                except Exception:
                    traceback.print_exc()  # fire-and-forget: log and keep serving
            # Unknown events are silently skipped (forward compatibility).

        elif method == "shutdown":
            return

        elif req_id is not None and method is not None:
            # An unknown *request* deserves an answer, or the host would wait
            # out its timeout; unknown notifications are silently skipped.
            sink.send(error_response(req_id, ErrorCode.METHOD_NOT_FOUND, f"unknown method '{method}'"))
