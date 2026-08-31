"""Cache control and replies — the two values every handler returns or shapes.

:class:`CachePolicy` is the pull-model cousin of an HTTP `Cache-Control`
header; :class:`Reply` is a handler's answer (a value, an error, or "still
loading") carrying one. They live together because every builder on `Reply`
is really a builder on its policy.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, Optional, Union

from .protocol import AppError, ErrorCode, error_response, response


class CachePolicy:
    """How cacheable one query answer is. Durations are **seconds** (floats
    fine); they cross the wire as whole milliseconds.

    - :meth:`forever` (the default) — fresh until an explicit `invalidate`;
      right for a value addressed by an immutable key (a commit hash).
    - :meth:`max_age` — fresh for that long, then (without a stale window)
      hard-expired: the next query shows a spinner while it refetches.
    - :meth:`stale_while_revalidate` — how long *past* max_age the stale
      answer is still served while a background refetch runs.
    - :meth:`no_store` — never fresh, never expired: always serve the last
      value *and* revalidate. Live data with no spinner flicker.
    """

    def __init__(
        self,
        no_store: bool = False,
        max_age_ms: Optional[int] = None,
        stale_while_revalidate_ms: Optional[int] = None,
    ) -> None:
        self._no_store = no_store
        self._max_age_ms = max_age_ms
        self._swr_ms = stale_while_revalidate_ms

    @classmethod
    def forever(cls) -> "CachePolicy":
        return cls()

    # Self-documenting alias: the resource at this (kind, arg) never changes.
    immutable = forever

    @classmethod
    def max_age(cls, seconds: float) -> "CachePolicy":
        return cls(max_age_ms=int(seconds * 1000))

    @classmethod
    def no_store(cls) -> "CachePolicy":
        return cls(no_store=True)

    def stale_while_revalidate(self, seconds: float) -> "CachePolicy":
        """Builder on top of :meth:`max_age`: set the stale window."""
        return CachePolicy(self._no_store, self._max_age_ms, int(seconds * 1000))

    def with_max_age(self, seconds: float) -> "CachePolicy":
        """This policy with a new max-age, **keeping** any stale window
        already set. The order-independent counterpart of :meth:`max_age`."""
        return CachePolicy(False, int(seconds * 1000), self._swr_ms)

    def to_wire(self) -> Optional[Dict[str, Any]]:
        """The `cache` field for a query result, or None for the default
        (a forever policy serializes to nothing — it adds nothing)."""
        out: Dict[str, Any] = {}
        if self._no_store:
            out["noStore"] = True
        if self._max_age_ms is not None:
            out["maxAgeMs"] = self._max_age_ms
        if self._swr_ms is not None:
            out["staleWhileRevalidateMs"] = self._swr_ms
        return out or None


class Reply:
    """A handler's answer to one `query` or `mutate`: a value, an error, or
    "still loading", plus (for queries) a cache policy.

    Build with :meth:`json` / :meth:`error` / :meth:`loading` /
    :meth:`from_result`, then attach a policy with :meth:`cache` /
    :meth:`max_age` / :meth:`stale_while_revalidate` / :meth:`no_store` /
    :meth:`forever` (the default is forever). A handler may also return a
    plain JSON-able value — the loop wraps it in ``Reply.json(...)`` — or
    raise :class:`AppError`.
    """

    def __init__(self, kind: str, payload: Any, policy: Optional[CachePolicy] = None) -> None:
        self._kind = kind  # "value" | "error" | "loading"
        self._payload = payload
        self._policy = policy or CachePolicy.forever()

    @classmethod
    def json(cls, value: Any) -> "Reply":
        """A successful answer carrying `value` (any JSON-serializable tree).
        Numbers keep their kind in the script: send ``3.0`` for a float."""
        return cls("value", value)

    # The Rust API spells the "already a JSON value" constructor `value`.
    value = json

    @classmethod
    def error(cls, message: Any) -> "Reply":
        """A failure: a JSON-RPC APP error response on the wire; the script
        surfaces the message via `error_of`."""
        return cls("error", str(message))

    @classmethod
    def loading(cls) -> "Reply":
        """"Still loading": an empty result. The host keeps the script's
        spinner up without re-requesting until the app pushes an
        `invalidate` for the key (see :meth:`ScriptSink.invalidate`)."""
        return cls("loading", None)

    @classmethod
    def from_result(cls, result: Any) -> "Reply":
        """Python's stand-in for Rust's ``Reply::from_result(Result<T, E>)``:
        turn "the outcome of some work" into a reply.

        - a **callable** is invoked, and anything it raises becomes the error
          (``return Reply.from_result(lambda: git_log(repo))``);
        - an **exception instance** becomes ``Reply.error(str(e))``;
        - a :class:`Reply` passes through unchanged;
        - anything else becomes ``Reply.json(value)``.
        """
        if callable(result) and not isinstance(result, BaseException):
            try:
                result = result()
            except AppError as e:
                return cls.error(str(e))
            except Exception as e:
                return cls.error("%s: %s" % (type(e).__name__, e))
        if isinstance(result, Reply):
            return result
        if isinstance(result, BaseException):
            return cls.error(str(result) or type(result).__name__)
        return cls.json(result)

    def cache(self, policy: CachePolicy) -> "Reply":
        self._policy = policy
        return self

    def max_age(self, seconds: float) -> "Reply":
        """Refresh after `seconds`. Unlike a bare ``CachePolicy.max_age``,
        this **keeps** a stale window already set on this reply, so the two
        builders compose in either order."""
        self._policy = self._policy.with_max_age(seconds)
        return self

    def stale_while_revalidate(self, seconds: float) -> "Reply":
        """Serve the stale value for `seconds` past max_age while a refetch
        runs — the no-spinner-flicker knob, fluent on the reply."""
        self._policy = self._policy.stale_while_revalidate(seconds)
        return self

    def no_store(self) -> "Reply":
        self._policy = CachePolicy.no_store()
        return self

    def forever(self) -> "Reply":
        """The default policy, stated explicitly at a call site."""
        self._policy = CachePolicy.forever()
        return self

    # Introspection, for tests and for the background wrapper.

    def is_error(self) -> bool:
        return self._kind == "error"

    def is_loading(self) -> bool:
        return self._kind == "loading"

    def into_parts(self):
        """``(value, error, policy)`` — the transport-facing split, mirroring
        the Rust ``Reply::into_parts``."""
        if self._kind == "value":
            return (self._payload, None, self._policy)
        if self._kind == "error":
            return (None, self._payload, self._policy)
        return (None, None, self._policy)


def query_response(req_id: Any, reply: Reply) -> Dict[str, Any]:
    """Map a Reply onto the wire, exactly like petal-query's query_response."""
    if reply._kind == "error":
        return error_response(req_id, ErrorCode.APP, reply._payload)
    result: Dict[str, Any] = {}
    if reply._kind == "value":
        result["value"] = reply._payload
    wire = reply._policy.to_wire()
    if wire is not None:
        result["cache"] = wire
    return response(req_id, result)


def mutate_response(req_id: Any, reply: Reply) -> Dict[str, Any]:
    """A mutation's answer: like a query's, minus the cache field (a mutation
    result is never cached)."""
    if reply._kind == "error":
        return error_response(req_id, ErrorCode.APP, reply._payload)
    result: Dict[str, Any] = {}
    if reply._kind == "value":
        result["value"] = reply._payload
    return response(req_id, result)
