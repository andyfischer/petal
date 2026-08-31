""":class:`PanelUi` — the editor-facing half of an app: what the pane is
called and which Petal script it runs. Kept apart from :class:`Provider`
because a provider is pure data service; only GPP needs a presentation.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, Optional


class PanelUi:
    """The pane's display name and the Petal UI script the host runs.
    `screens` (name → source) declares the navigable screens beyond the home
    script; the declared set doubles as the navigation allowlist. `title` (a
    callable ``state -> str``) derives the pane name from the built state
    instead of the static name."""

    def __init__(
        self,
        name: str,
        script: str,
        screens: Optional[Dict[str, str]] = None,
        title: Optional[Callable[[Any], str]] = None,
    ) -> None:
        self.name = name
        self.script = script
        self.screens: Dict[str, str] = dict(screens or {})
        self.title = title
        self.script_path: Optional[str] = None  # set by from_file; enables watch=True

    @classmethod
    def from_file(cls, name: str, path: str, **kwargs: Any) -> "PanelUi":
        """A panel whose home script is read from `path` (remembered, so
        ``serve(..., watch=True)`` can hot-reload it on change)."""
        with open(path, "r", encoding="utf-8") as f:
            ui = cls(name, f.read(), **kwargs)
        ui.script_path = path
        return ui

    def screen(self, name: str, source: str) -> "PanelUi":
        """Declare a navigable screen (fluent, like the Rust PanelUi)."""
        self.screens[name] = source
        return self

    def screen_from_file(self, name: str, path: str) -> "PanelUi":
        """Declare a navigable screen whose source is read from `path`."""
        with open(path, "r", encoding="utf-8") as f:
            return self.screen(name, f.read())
