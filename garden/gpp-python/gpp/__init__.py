"""gpp — a stdlib-only Python client library for the Garden Pane Protocol (v2).

A GPP app is a subprocess that drives the content of a Garden pane: right
after the handshake it pushes a Petal UI script (the "page") that the host
runs in-process, then acts as that script's data server, answering the
`query` / `mutate` / `navigate` requests the host issues on the running
script's behalf. Only data crosses the pipe; the interaction loop never does.

This package mirrors the Rust client library (`petal-query`): you register a
handler per `query(kind, arg)` on a :class:`Provider` — each returning a
:class:`Reply` with the value (or error) and how cacheable it is
(:class:`CachePolicy`) — and :func:`serve` runs the whole protocol loop.
Nothing here is hand-rolled per app: the stdio handshake, the JSON-RPC
framing, and the response plumbing all live behind this one import.

    from gpp import PanelUi, Provider, Reply, serve

    provider = Provider(lambda init: init.repo_arg())

    @provider.query("log")
    def log(repo, ctx):
        return Reply.json(git_log(repo)).max_age(3.0)

    serve(provider, PanelUi.from_file("git-log", "panel.ptl"))

The modules behind this facade: :mod:`gpp.protocol` (wire envelopes, Init,
Ctx), :mod:`gpp.cache` (CachePolicy, Reply), :mod:`gpp.provider` (dispatch),
:mod:`gpp.panel` (PanelUi), :mod:`gpp.sink` (ScriptSink, the drawer watcher),
:mod:`gpp.background` (off-loop handlers), :mod:`gpp.serve` (the loop) and
:mod:`gpp.testing` (a protocol-level test harness). Importing the flat names
from `gpp` keeps working exactly as it did when this was one file.

Wire reference: `garden/docs/gpp.md` (the normative v2 spec). Python how-to:
`garden/docs/writing-gpp-apps-python.md`. Standard library only (json / os /
sys / threading / time).
"""

from __future__ import annotations

from .background import BackgroundQuery, background, run_in_background
from .cache import CachePolicy, Reply, mutate_response, query_response
from .panel import PanelUi
from .protocol import (
    CLIENT_CAPABILITIES,
    HOST_MUTATIONS,
    PROTOCOL_VERSION,
    AppError,
    Ctx,
    ErrorCode,
    Init,
    error_response,
    notification,
    request,
    response,
    script_args,
)
from .provider import Provider, run_handler
from .serve import serve, serve_on
from .sink import ScriptSink, watch_script
from .testing import TestHarness

__version__ = "2.0.0"

__all__ = [
    "AppError",
    "BackgroundQuery",
    "CLIENT_CAPABILITIES",
    "CachePolicy",
    "Ctx",
    "ErrorCode",
    "HOST_MUTATIONS",
    "Init",
    "PROTOCOL_VERSION",
    "PanelUi",
    "Provider",
    "Reply",
    "ScriptSink",
    "TestHarness",
    "background",
    "error_response",
    "mutate_response",
    "notification",
    "query_response",
    "request",
    "response",
    "run_handler",
    "run_in_background",
    "script_args",
    "serve",
    "serve_on",
    "watch_script",
]
