""":class:`Provider` — the query / mutate / emit / navigate dispatch core.

A provider is "a web server for data": it answers `query(kind, arg)` against a
per-run state, runs effectful `mutate(name, arg)` calls, reacts to
fire-and-forget `emit(event, arg)` signals, and serves screen sources for
`navigate`. Like the Rust `petal_query::Provider` it owns *no* presentation
and no transport — :mod:`gpp.serve` supplies those — so the four public
dispatch entry points (:meth:`Provider.answer`, :meth:`Provider.mutate`,
:meth:`Provider.handle_emit`, :meth:`Provider.navigate`) are how a test drives
one handler in a single call instead of faking a whole stdio session.

Registration comes in two shapes, both fluent-compatible:

    provider = Provider(pick_repo).query("log", log_handler)   # positional

    @provider.query("log")                                     # decorator
    def log_handler(repo, ctx): ...
"""

from __future__ import annotations

import traceback
from typing import Any, Callable, Dict, Optional

from .cache import Reply
from .protocol import HOST_MUTATIONS, AppError, Ctx, Init


def run_handler(handler: Callable, state: Any, ctx: Ctx, label: Optional[str] = None) -> Reply:
    """Run a Reply-returning handler, mapping exceptions to APP errors and
    wrapping a plain returned value in ``Reply.json``. A handler bug must
    degrade to an in-pane message, never wedge the pane — so every exception
    is caught here and its traceback goes to stderr."""
    label = label or ctx.label()
    try:
        out = handler(state, ctx)
    except AppError as e:
        return Reply.error(str(e))
    except Exception as e:  # a handler bug must not wedge the pane
        traceback.print_exc()
        return Reply.error("%s raised %s: %s" % (label, type(e).__name__, e))
    return out if isinstance(out, Reply) else Reply.json(out)


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

    def __init__(self, build_state: Optional[Callable[[Init], Any]] = None) -> None:
        self._build_state = build_state or (lambda init: None)
        self._queries: Dict[str, Callable] = {}
        self._emits: Dict[str, Callable] = {}
        self._mutations: Dict[str, Callable] = {}
        self._navigate: Optional[Callable] = None

    @classmethod
    def stateless(cls) -> "Provider":
        """A provider with no per-run state; handlers receive ``None``."""
        return cls(lambda init: None)

    # ── Registration ─────────────────────────────────────────────────────────
    # Each of these takes the handler positionally (fluent, returns self) or
    # omits it (returns a decorator that registers and gives the function
    # back, so the module-level name still refers to the plain function).

    def query(self, kind: str, handler: Optional[Callable] = None):
        """Register a handler for `query(kind, …)`, fluently or as
        ``@provider.query("kind")``."""
        if handler is None:
            return self._decorator(self._queries, kind)
        self._queries[kind] = handler
        return self

    def on_emit(self, event: str, handler: Optional[Callable] = None):
        if handler is None:
            return self._decorator(self._emits, event)
        self._emits[event] = handler
        return self

    def on_mutation(self, name: str, handler: Optional[Callable] = None):
        # A host-owned name is answered by Garden itself and never reaches the
        # client, so a handler here could never fire: refuse rather than let
        # an app ship a mutation that silently does nothing.
        if name in HOST_MUTATIONS:
            raise ValueError(
                "'%s' is a host-owned mutation (answered by Garden itself, never "
                "forwarded to a client); use sink.open_path(...) / sink.emit(...) instead"
                % name
            )
        if handler is None:
            return self._decorator(self._mutations, name)
        self._mutations[name] = handler
        return self

    def on_navigate(self, handler: Callable) -> "Provider":
        self._navigate = handler
        return self

    # Decorator-shaped aliases, so a handler is declared where it is defined.

    def mutation(self, name: str, handler: Optional[Callable] = None):
        """``@provider.mutation("apply")`` — the decorator form of
        :meth:`on_mutation`."""
        return self.on_mutation(name, handler)

    def emit(self, event: str, handler: Optional[Callable] = None):
        """``@provider.emit("divider")`` — the decorator form of
        :meth:`on_emit`."""
        return self.on_emit(event, handler)

    def navigate_to(self, handler: Callable) -> Callable:
        """``@provider.navigate`` — registers the custom navigate handler and
        returns it unchanged (see :meth:`on_navigate` for the fluent form)."""
        self._navigate = handler
        return handler

    # `@provider.navigate` reads best bare; `provider.navigate(state, ctx)` is
    # the dispatch call. They are distinguished below in `navigate`.

    def background_query(self, kind: str, handler: Optional[Callable] = None, **kwargs):
        """Register a **slow** query handler: the pane is answered
        ``Reply.loading()`` at once and the handler runs on a worker thread,
        invalidating the key when it lands (see :func:`gpp.background`)."""
        from .background import background

        if handler is None:
            def register(fn):
                self._queries[kind] = background(fn, **kwargs)
                return fn

            return register
        self._queries[kind] = background(handler, **kwargs)
        return self

    def _decorator(self, table: Dict[str, Callable], key: str):
        def register(fn):
            table[key] = fn
            return fn

        return register

    # ── Dispatch ─────────────────────────────────────────────────────────────

    def build(self, init: Init) -> Any:
        """Build the per-run state from the handshake params. Call once,
        before serving requests."""
        return self._build_state(init)

    def answer(self, state: Any, ctx: Ctx) -> Reply:
        """Answer one `query(kind, arg)`: dispatch to the registered handler,
        or ``Reply.json(None)`` for an unregistered kind (a query for a kind
        an app does not serve conventionally answers null, not an error)."""
        handler = self._queries.get(ctx.kind)
        if handler is None:
            return Reply.json(None)
        return run_handler(handler, state, ctx)

    def mutate(self, state: Any, ctx: Ctx) -> Reply:
        """Run one `mutate(name, arg)`: dispatch, or ``Reply.error`` for an
        unregistered name (a mutation is an explicit request, so an unknown
        one is an error, not a silent null — unlike :meth:`answer`)."""
        handler = self._mutations.get(ctx.name)
        if handler is None:
            return Reply.error("no mutation handler for '%s'" % ctx.name)
        return run_handler(handler, state, ctx)

    def handle_emit(self, state: Any, ctx: Ctx) -> None:
        """Deliver one `emit(event, arg)` to its handler, if registered (else
        a no-op). Fire-and-forget: an exception is logged, never raised."""
        handler = self._emits.get(ctx.event)
        if handler is None:
            return
        try:
            handler(state, ctx)
        except Exception:
            traceback.print_exc()

    def navigate(self, state: Any, ctx: Any = None) -> Any:
        """Two shapes, told apart by what is passed:

        - ``@provider.navigate`` / ``provider.navigate(handler)`` — register
          the custom navigate handler (one argument, a callable).
        - ``provider.navigate(state, ctx)`` — dispatch: return the target
          screen's UI source, or ``None`` when no custom handler is
          registered (the caller falls back to the declared screens).
          :class:`AppError` propagates, as the refusal channel.
        """
        if ctx is None and callable(state):
            return self.navigate_to(state)
        if self._navigate is None:
            return None
        source = self._navigate(state, ctx)
        if source is None:
            raise AppError("navigate handler for '%s' returned no source" % ctx.screen)
        return source

    # ── Introspection ────────────────────────────────────────────────────────

    def has_query(self, kind: str) -> bool:
        return kind in self._queries

    def has_mutation(self, name: str) -> bool:
        return name in self._mutations

    def has_emit(self, event: str) -> bool:
        return event in self._emits

    def has_navigate(self) -> bool:
        return self._navigate is not None
