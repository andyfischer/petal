""":class:`ScriptSink` — the one place outgoing messages go, plus the drawer
file watcher built on it.

The serve loop writes responses through the sink, and any background thread (a
poller, a finished job, a file watcher) may push `setScript` / `invalidate` /
`emit` at the same time. Every send serializes one complete envelope under one
lock, so two threads' messages can never interleave inside a line — the
transport is one compact JSON object per line.
"""

from __future__ import annotations

import json
import os
import sys
import threading
import time
from typing import Any

from .protocol import notification


class ScriptSink:
    """A thread-safe handle for pushing to the host."""

    def __init__(self, writer: Any) -> None:
        self._writer = writer
        self._lock = threading.Lock()

    def send(self, env: Any) -> None:
        data = json.dumps(env, separators=(",", ":"), ensure_ascii=False) + "\n"
        with self._lock:
            try:
                self._writer.write(data)
            except TypeError:
                # A binary writer (e.g. sys.stdout.buffer).
                self._writer.write(data.encode("utf-8"))
            self._writer.flush()

    def set_script(self, source: str) -> None:
        """Push a new UI script, replacing the one the panel is running. The
        host recompiles in place and keeps the panel's `state` and its query
        cache; a source that fails to compile leaves the old program running."""
        self.send(notification("setScript", {"source": source}))

    def invalidate(self, kind: str, arg: Any = "") -> None:
        """Drop the host's cached value for `(kind, arg)` so the script
        re-queries it — how a watcher, a poller, or a finished background job
        publishes a new answer. `arg` must equal the queried arg."""
        self.send(notification("invalidate", {"kind": kind, "arg": arg}))

    def emit(self, event: str, arg: Any = None) -> None:
        """Raise a client → host event. The host acts on the reserved names
        `open_path` ({"path": ...}) and `status` ({"text": ...}); anything
        else is ignored (reserved for future use)."""
        self.send(notification("emit", {"event": event, "arg": arg}))

    def status(self, text: str) -> None:
        """Set the pane's status-bar text (the reserved `status` event)."""
        self.emit("status", {"text": text})

    def open_path(self, path: str) -> None:
        """Ask the host to replace this pane with a normal editor on `path`
        (the reserved `open_path` event). This **ends the session** — the host
        shuts the client down — so it is the last thing an app says. The path
        is absolutized, since the pane's cwd is the host's, not the app's."""
        self.emit("open_path", {"path": os.path.abspath(path)})


def watch_script(sink: ScriptSink, path: str, interval: float = 0.5) -> threading.Thread:
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
                    print("gpp: could not reload %s: %s" % (path, e), file=sys.stderr)

    t = threading.Thread(target=loop, daemon=True, name="gpp-script-watch")
    t.start()
    return t
