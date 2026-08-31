"""Wire-level pieces of the Garden Pane Protocol: versions, error codes,
envelope builders, and the two params objects a handler ever sees.

Everything here is transport-shaped and stateless, so it is the one module
both the serve loop and the test harness can depend on without pulling in
providers, panels or threads.
"""

from __future__ import annotations

import os
import sys
from typing import Any, Dict, List, Optional

# The protocol major version this library speaks. Carried by both halves of
# the `initialize` handshake; a host reporting a different major is refused
# with a PROTOCOL_MISMATCH error response.
PROTOCOL_VERSION = 2

# The capability names an app built on this library reports in its
# `initialize` response: the requests it answers and the pushes it makes.
CLIENT_CAPABILITIES = ["query", "mutate", "navigate", "emit", "setScript"]

# Mutation names the *host* answers itself (gpp.md, "Host-owned mutations").
# They never reach a client, so registering a handler for one is a bug we
# refuse loudly rather than a handler that can never fire.
HOST_MUTATIONS = frozenset({"open_path", "open_project", "open_pr", "open_file_dialog"})


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


def request(req_id: Any, method: str, params: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}


def notification(method: str, params: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "method": method, "params": params}


def response(req_id: Any, result: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": req_id, "result": result}


def error_response(req_id: Any, code: int, message: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": str(message)}}


# ── Handshake params / handler context ───────────────────────────────────────


class Init:
    """The `initialize` params: how a client learns what to serve."""

    def __init__(self, params: Dict[str, Any]) -> None:
        self.protocol: int = params.get("protocol", 1)
        self.pane_id: int = params.get("paneId", 0)
        self.rows: int = params.get("rows", 0)
        self.cols: int = params.get("cols", 0)
        self.args: List[str] = list(params.get("args") or [])
        self.cwd: str = params.get("cwd", "")
        self.capabilities: List[str] = list(params.get("capabilities") or [])

    def repo_arg(self) -> str:
        """The first launch arg, or the pane cwd when none was given — the
        common "which directory/target do I operate on?" resolution."""
        return self.args[0] if self.args else self.cwd


def script_args(init: Init, script: Optional[str] = None) -> List[str]:
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
    emits, `screen` for navigations).

    `sink` is the live :class:`~gpp.sink.ScriptSink` when a serve loop is
    running (None in a bare unit-test dispatch), so a handler can push a
    status line or hand work to a background thread that invalidates later.
    """

    def __init__(
        self,
        init: Init,
        arg: Any,
        kind: Optional[str] = None,
        name: Optional[str] = None,
        event: Optional[str] = None,
        screen: Optional[str] = None,
        sink: Any = None,
    ) -> None:
        self.init = init
        self.arg = arg  # any JSON value (GPP v2 carries it verbatim)
        self.kind = kind
        self.name = name
        self.event = event
        self.screen = screen
        self.sink = sink

    def arg_str(self) -> str:
        """The argument as a string — its content when it is a JSON string,
        "" otherwise. The common case: Petal's script-side `query(kind, arg)`
        passes a string arg."""
        return self.arg if isinstance(self.arg, str) else ""

    def label(self) -> str:
        """A human name for this dispatch, used in error messages."""
        if self.kind is not None:
            return "query '%s'" % self.kind
        if self.name is not None:
            return "mutation '%s'" % self.name
        if self.event is not None:
            return "emit '%s'" % self.event
        if self.screen is not None:
            return "navigate '%s'" % self.screen
        return "handler"
