"""Background query handlers — how a slow answer stops stalling the pane.

The serve loop is single-threaded and the host waits on it: a handler that
shells out for two seconds blocks every other query for those two seconds,
including the ~200 ms window the host gives a freshly spawned client to prime
its first frame. The pane is simply dead until the call returns.

:func:`background` wraps such a handler so the pipe is never held:

1. the first query for a key answers :meth:`Reply.loading` immediately — the
   drawer shows its own spinner and the loop moves on;
2. the handler runs on a worker thread;
3. when it lands, the result is parked and the worker pushes
   `invalidate(kind, arg)` through the (thread-safe) :class:`ScriptSink`, so
   the host re-queries that exact key;
4. the re-query finds the parked result and answers it — with whatever cache
   policy the handler chose.

Concurrent queries for the same key coalesce onto the one in-flight job, so a
host that re-asks while the work runs does not fan out threads.

    provider.query("stats", background(expensive_stats))

    @provider.background_query("stats")     # the same, as a decorator
    def stats(repo, ctx): ...

The handler runs **off** the serve loop, so anything it touches on the shared
state must be safe to touch from another thread; the usual shape (read inputs
from `ctx`, compute, return a fresh value) already is.
"""

from __future__ import annotations

import json
import threading
from typing import Any, Callable, Dict, Optional

from .cache import Reply
from .protocol import Ctx


def _key(kind: Optional[str], arg: Any) -> str:
    """A hashable identity for one (kind, arg) query key. The arg is any JSON
    tree, so it is keyed by its canonical JSON text."""
    try:
        return "%s\x00%s" % (kind, json.dumps(arg, sort_keys=True, separators=(",", ":")))
    except (TypeError, ValueError):
        return "%s\x00%r" % (kind, arg)


class BackgroundQuery:
    """The callable :func:`background` returns: a query handler that owns a
    worker thread per in-flight key. Registered like any other handler; the
    extra methods exist for tests (:meth:`wait`) and for apps that want to
    prime or drop a key themselves."""

    def __init__(self, handler: Callable, name: Optional[str] = None) -> None:
        self._handler = handler
        self._name = name or getattr(handler, "__name__", "background")
        self._lock = threading.Lock()
        self._ready: Dict[str, Reply] = {}
        self._inflight: Dict[str, threading.Thread] = {}

    def __call__(self, state: Any, ctx: Ctx) -> Reply:
        key = _key(ctx.kind, ctx.arg)
        with self._lock:
            if key in self._ready:
                # The job landed and the host came back for it. Hand the
                # answer over once; a later refresh starts a new job.
                return self._ready.pop(key)
            if key in self._inflight:
                return Reply.loading()
            thread = threading.Thread(
                target=self._work,
                args=(state, ctx, key),
                daemon=True,
                name="gpp-bg-%s" % self._name,
            )
            self._inflight[key] = thread
        thread.start()
        return Reply.loading()

    def _work(self, state: Any, ctx: Ctx, key: str) -> None:
        reply = run_handler_safe(self._handler, state, ctx)
        with self._lock:
            self._ready[key] = reply
            self._inflight.pop(key, None)
        if ctx.sink is not None:
            # The host drops its cache entry and re-queries this exact key;
            # the next __call__ hands back the parked reply.
            ctx.sink.invalidate(ctx.kind, ctx.arg)

    def wait(self, timeout: Optional[float] = 5.0) -> bool:
        """Block until every in-flight job has finished. For tests — a serve
        loop never calls this. Returns False if the timeout ran out."""
        with self._lock:
            threads = list(self._inflight.values())
        for t in threads:
            t.join(timeout)
            if t.is_alive():
                return False
        return True

    def pending(self) -> int:
        """How many jobs are running right now."""
        with self._lock:
            return len(self._inflight)


def run_handler_safe(handler: Callable, state: Any, ctx: Ctx) -> Reply:
    """:func:`gpp.provider.run_handler`, imported lazily to keep the module
    graph acyclic."""
    from .provider import run_handler

    return run_handler(handler, state, ctx)


def background(handler: Callable, name: Optional[str] = None) -> BackgroundQuery:
    """Wrap a slow query handler so it never blocks the serve loop. See the
    module docstring for the full protocol dance."""
    return BackgroundQuery(handler, name=name)


def run_in_background(sink: Any, fn: Callable, kind: str, arg: Any = "") -> threading.Thread:
    """Fire `fn()` on a daemon thread and `invalidate(kind, arg)` when it
    returns — the unstructured version of :func:`background`, for work kicked
    off from a mutation or an emit handler ("rebuild the index, then refresh
    the view") rather than from the query being answered."""

    def work():
        try:
            fn()
        except Exception:
            import traceback

            traceback.print_exc()
        if sink is not None:
            sink.invalidate(kind, arg)

    t = threading.Thread(target=work, daemon=True, name="gpp-bg-task")
    t.start()
    return t
