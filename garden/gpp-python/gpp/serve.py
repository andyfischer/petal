"""The GPP protocol loop: handshake, script push, request dispatch.

This is the only module that knows about stdio and message ordering. It maps
each incoming envelope onto one of :class:`Provider`'s public dispatch calls,
so the loop and a unit test exercise exactly the same code path.
"""

from __future__ import annotations

import json
import sys
import traceback
from typing import Any, Callable, Optional

from .cache import mutate_response, query_response
from .panel import PanelUi
from .protocol import (
    CLIENT_CAPABILITIES,
    PROTOCOL_VERSION,
    AppError,
    Ctx,
    ErrorCode,
    Init,
    error_response,
    notification,
    response,
)
from .provider import Provider
from .sink import ScriptSink, watch_script


def serve(
    provider: Provider,
    ui: PanelUi,
    on_ready: Optional[Callable[[ScriptSink], None]] = None,
    watch: Any = False,
) -> None:
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


def serve_on(
    provider: Provider,
    ui: PanelUi,
    reader: Any,
    writer: Any,
    on_ready: Optional[Callable[[ScriptSink], None]] = None,
) -> None:
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
            print("gpp: unparseable line from host: %s" % e, file=sys.stderr)
            return


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
                "this app speaks gpp protocol %d, the host sent %s" % (PROTOCOL_VERSION, protocol),
            )
        )
        return
    init = Init(params)
    state = provider.build(init)

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
            ctx = Ctx(init, params.get("arg"), kind=kind, sink=sink)
            sink.send(query_response(req_id or 0, provider.answer(state, ctx)))

        elif method == "mutate":
            mname = params.get("name")
            if not isinstance(mname, str):
                sink.send(error_response(req_id or 0, ErrorCode.INVALID_PARAMS, "mutate needs a string 'name'"))
                continue
            ctx = Ctx(init, params.get("arg"), name=mname, sink=sink)
            reply = provider.mutate(state, ctx)
            # An unknown mutation is an APP error response, not a result.
            sink.send(mutate_response(req_id or 0, reply))

        elif method == "navigate":
            screen = params.get("screen")
            if not isinstance(screen, str):
                sink.send(error_response(req_id or 0, ErrorCode.INVALID_PARAMS, "navigate needs a string 'screen'"))
                continue
            ctx = Ctx(init, params.get("arg"), screen=screen, sink=sink)
            if provider.has_navigate():
                # A registered handler wins (side effects + source). It runs on
                # every visit — back/forward re-issue navigate — so it should
                # be idempotent per visit.
                try:
                    source = provider.navigate(state, ctx)
                    sink.send(response(req_id or 0, {"screen": screen, "source": source}))
                except AppError as e:
                    sink.send(error_response(req_id or 0, ErrorCode.APP, str(e)))
                except Exception as e:
                    traceback.print_exc()
                    sink.send(error_response(req_id or 0, ErrorCode.APP,
                                             "navigate raised %s: %s" % (type(e).__name__, e)))
            elif screen in ui.screens:
                sink.send(response(req_id or 0, {"screen": screen, "source": ui.screens[screen]}))
            else:
                sink.send(error_response(req_id or 0, ErrorCode.APP, "no such screen '%s'" % screen))

        elif method == "emit":
            event = params.get("event")
            if isinstance(event, str):
                provider.handle_emit(state, Ctx(init, params.get("arg"), event=event, sink=sink))
            # Unknown events are silently skipped (forward compatibility).

        elif method == "shutdown":
            return

        elif req_id is not None and method is not None:
            # An unknown *request* deserves an answer, or the host would wait
            # out its timeout; unknown notifications are silently skipped.
            sink.send(error_response(req_id, ErrorCode.METHOD_NOT_FOUND, "unknown method '%s'" % method))
