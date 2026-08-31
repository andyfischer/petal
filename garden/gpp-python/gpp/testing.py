"""A protocol-level test harness for GPP apps.

Every app author otherwise re-writes the same fixture: build the JSON lines
for a handshake plus a request, run :func:`gpp.serve_on` over two in-memory
streams, and pick the response back out by id. :class:`TestHarness` is that
fixture, so an app's tests assert on the **wire** — the same envelopes the
host will see — with no host and no subprocess:

    h = TestHarness(provider, ui, args=["/repo"])
    self.assertEqual(h.query("stats").value()["total"], 42)
    self.assertIn("not a git repo", h.query("stats").error_message())

For a single handler with no protocol involved at all, call the provider's
dispatch directly instead (:meth:`Provider.answer` and friends) — that path is
one function call and needs no harness.
"""

from __future__ import annotations

import io
import json
from typing import Any, Dict, List, Optional

from .panel import PanelUi
from .protocol import PROTOCOL_VERSION
from .provider import Provider
from .serve import serve_on


def run(provider: Provider, ui: PanelUi, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Feed `messages` (envelope dicts) through a whole serve session over
    in-memory streams; return the envelopes it wrote. No handshake is added —
    the messages are exactly what the host sends."""
    lines = "".join(json.dumps(m) + "\n" for m in messages)
    out = io.StringIO()
    serve_on(provider, ui, io.StringIO(lines), out)
    return [json.loads(line) for line in out.getvalue().splitlines()]


def init_req(
    protocol: int = PROTOCOL_VERSION,
    args: Optional[List[str]] = None,
    cwd: str = "/repo",
    req_id: Any = 1,
    rows: int = 40,
    cols: int = 120,
) -> Dict[str, Any]:
    """An `initialize` request shaped like the host's."""
    return {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "initialize",
        "params": {
            "protocol": protocol,
            "paneId": 0,
            "rows": rows,
            "cols": cols,
            "args": args if args is not None else [cwd],
            "cwd": cwd,
            "capabilities": ["query", "mutate", "navigate", "emit", "hotReload"],
        },
    }


def req(req_id: Any, method: str, params: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}


def notif(method: str, params: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "method": method, "params": params}


def by_id(msgs: List[Dict[str, Any]], req_id: Any) -> Dict[str, Any]:
    """The response envelope with `req_id` (a response has an id and no
    method); raises if the session never answered."""
    for m in msgs:
        if m.get("id") == req_id and "method" not in m:
            return m
    raise AssertionError("no response with id %r in %r" % (req_id, msgs))


class Outcome:
    """One response envelope, with accessors that read like assertions."""

    def __init__(self, envelope: Dict[str, Any], envelopes: List[Dict[str, Any]]) -> None:
        self.envelope = envelope
        # The whole session, so a test can also look at pushes (invalidate,
        # emit) the handler made while answering.
        self.envelopes = envelopes

    def ok(self) -> bool:
        return "result" in self.envelope

    def value(self) -> Any:
        """The `result.value` — raises if the response was an error, so a
        failing handler fails the test where it happened."""
        if not self.ok():
            raise AssertionError("expected a value, got error %r" % (self.envelope["error"],))
        return self.envelope["result"].get("value")

    def cache(self) -> Optional[Dict[str, Any]]:
        """The wire `cache` field, or None for the default forever policy."""
        return self.envelope.get("result", {}).get("cache")

    def is_loading(self) -> bool:
        """A "still loading" answer: an empty result object."""
        return self.envelope.get("result") == {}

    def error_message(self) -> str:
        if self.ok():
            raise AssertionError("expected an error, got result %r" % (self.envelope["result"],))
        return self.envelope["error"]["message"]

    def error_code(self) -> int:
        return self.envelope["error"]["code"]

    def source(self) -> str:
        """A navigate response's screen source."""
        return self.envelope["result"]["source"]

    def pushed(self, method: str) -> List[Dict[str, Any]]:
        """Every notification of `method` the session wrote (`invalidate`,
        `emit`, `setScript`)."""
        return [m for m in self.envelopes if m.get("method") == method]


class TestHarness:
    """Drives `provider` + `ui` through real serve sessions.

    Each call is its own session (handshake → requests → shutdown), so state
    built from the handshake is fresh unless the provider closes over
    something shared. :meth:`send` is the multi-step form when one session
    must see several requests in order.
    """

    def __init__(
        self,
        provider: Provider,
        ui: Optional[PanelUi] = None,
        args: Optional[List[str]] = None,
        cwd: str = "/repo",
        protocol: int = PROTOCOL_VERSION,
    ) -> None:
        self.provider = provider
        self.ui = ui if ui is not None else PanelUi("test", "// test drawer")
        self.args = args
        self.cwd = cwd
        self.protocol = protocol

    # ── Whole sessions ───────────────────────────────────────────────────────

    def run(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Raw: run exactly `messages` (no handshake added)."""
        return run(self.provider, self.ui, messages)

    def init(self) -> Dict[str, Any]:
        return init_req(protocol=self.protocol, args=self.args, cwd=self.cwd)

    def send(self, *messages: Dict[str, Any]) -> List[Dict[str, Any]]:
        """A session that handshakes, sends `messages`, then shuts down."""
        return self.run([self.init()] + list(messages) + [notif("shutdown", {})])

    def handshake(self) -> List[Dict[str, Any]]:
        """Just the handshake: the initialize response and the script push."""
        return self.send()

    # ── One request at a time ────────────────────────────────────────────────

    def _one(self, method: str, params: Any) -> Outcome:
        msgs = self.send(req(99, method, params))
        return Outcome(by_id(msgs, 99), msgs)

    def query(self, kind: str, arg: Any = "") -> Outcome:
        return self._one("query", {"kind": kind, "arg": arg})

    def mutate(self, name: str, arg: Any = None) -> Outcome:
        return self._one("mutate", {"name": name, "arg": arg})

    def navigate(self, screen: str, arg: Any = None) -> Outcome:
        return self._one("navigate", {"screen": screen, "arg": arg})

    def emit(self, event: str, arg: Any = None) -> List[Dict[str, Any]]:
        """Emit is a notification: nothing comes back, so this returns the
        whole session's output (usually just handshake + setScript)."""
        return self.send(notif("emit", {"event": event, "arg": arg}))
