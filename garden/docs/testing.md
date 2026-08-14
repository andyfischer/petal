# Testing strategy

Garden uses a layered strategy adapted to a Rust GPU app. The principle (from
the generic test-strategy guidance) holds: push as much coverage as possible
down to fast, reliable unit tests, and reserve the slow, fragile top layer for
what only the running app can exercise.

## Layer 1 — Unit tests (`cargo test`)

Pure logic, no window or GPU. This is where the bulk of the coverage lives and
the first place to add a test for any new behavior.

| Area | Location | Covers |
|------|----------|--------|
| Text model | `garden-core/src/tests.rs` | `Buffer`, `Point`, `Selection`, undo/redo coalescing, file I/O |
| Editable projections | `garden-core/src/projection/tests.rs` | the origin table: the splice transform, the fold back into sources, undo/redo of origins, and the structural intents (revert a span/group, refuse a locked line) |
| Pane layout | `garden-app/src/layout.rs` (`#[cfg(test)]`) | the layout solver: nesting, ratios |
| Editor view | `garden-app/src/editor_view.rs` (`#[cfg(test)]`) | cursor/selection geometry, tab-stop display columns, scrolling math, scene primitives |
| Vim | `garden-app/src/vim.rs` (`#[cfg(test)]`) | the mode/motion/operator state machine, driven key-by-key (incl. `n`/`N`/`*`/`#` search repeats); and ordinary vim commands over a *projected* view, which is where the mutation choke point's line-splice reporting is exercised |
| Search core | `garden-app/src/search.rs` (`#[cfg(test)]`) | wraparound forward/backward match finding, whole-word boundaries, viewport match enumeration, substitution, non-ASCII columns |
| Command line | `garden-app/src/command_line.rs` (`#[cfg(test)]`) | ex-command parsing (incl. `:s` ranges/flags), the `/` and `?` prompt kinds |
| Window nav | `garden-app/src/window_nav.rs` (`#[cfg(test)]`) | spatial pane navigation and the pure layout-tree edits (`replace_leaf`, `remove_leaf`, `rebuild_leaves`) |
| App core | `garden-app/src/app/tests.rs` | the `App` input/command surface end to end in memory: key routing, ex commands, splits/close, clipboard, file refresh, `/state` JSON |
| File finder | `garden-app/src/file_finder.rs` (`#[cfg(test)]`) | the fuzzy matcher, ranking, and the modal finder state machine |
| Panel tessellation | `garden-app/src/panel_tess.rs` (`#[cfg(test)]`) | pure geometry→mesh-triangle tessellation for Petal panels |
| State / event log | `garden-app/src/state.rs`, `event_log.rs`, `recents.rs` | the SQLite state dir: window ids, migrations (incl. upgrading an existing DB in place), event buffering, `:report` capture, recently-opened files/projects/PRs and repo-root detection (temp-dir DBs) |
| Terminal grid | `garden-app/src/frontend/grid.rs` (`#[cfg(test)]`) | Scene→character-grid rasterization for the TUI frontend |
| GPP contract | `gpp/src/lib.rs` (`#[cfg(test)]`) | envelope/param serde round-trips and the key-name encoding |
| GPP clients | `gpp-apps/*/src/` | each Lines-mode browser's pure core (listing/log navigation, activation); the `git-log` app's git plumbing (`cargo test -p git-viewers`) and `garden-diff`'s diff parsing, projection **spec** (per-line origins, hunk spans), comment weaving, and write-back splicing (`cargo test -p garden-diff`) — the *semantics* of editing a projection live in `garden-core`, not here, all against real temporary repos |

Run everything:

```bash
cargo test                 # whole workspace
cargo test -p garden-app   # one crate
```

Design for this layer: keep the editor/vim logic pure (data in → data out,
operating on an `EditorView`'s public API) so it never needs a window. The vim
state machine is a good example — `vim::handle(view, key, ..)` is fully tested
without rendering a single frame.

## Layer 2 — Functional integration (`tools/integration-test.ts`)

The whole stack — frontend loop, key routing, vim, command line, file I/O —
driven through the **debug server** over HTTP, asserting on `/state`, `/buffer`,
and files on disk. This catches wiring bugs that unit tests structurally
cannot: key translation, mode plumbing, command execution, persistence.

```bash
node tools/integration-test.ts            # headless frontend (no window, CI-safe)
node tools/integration-test.ts --window   # same checks through the real winit/wgpu frontend
```

It boots the app on a free port (`--headless --debug-port 0`), runs a scripted
user flow, and tears the app down. The headless frontend means no window opens
and no GPU is needed, so it can run anywhere (including CI); the `--window`
variant additionally exercises the winit event loop and renderer wiring and is
run on demand. Keep it fast and deterministic: drive with injected input,
assert on observable state, avoid sleeps beyond the startup poll.

**The diff review** (`tools/diff-review-integration-test.ts`) — the same
approach for a Petal graphical panel, over `garden-diff` (the one diff/review
tool). It builds a throwaway git repo fixture, opens `garden diff main` on it
headless, and asserts on the values the panel's frame bound, read at `/state` →
`panes[].panel.values`: the diff loads, the header pills switch between the
unified / split / stat views, and — the real proof — both write-back paths reach
the file on disk, an edit typed into the editable after column and a `-` line
deleted in the editable unified view (which restores the base line). This is how a panel
that expresses its logical state only in pixels stays testable: the drawer
gives that state (and its click targets) plain `let` names, which the host
observes and reports by name — no publishing call in the script.

**Git history browser** (`tools/git-panel-integration-test.ts`) — the same
harness for the `:Git` panel, driving the `git-log` panel-mode GPP app via
`garden git log`: a multi-commit fixture with a dirty worktree, asserting
commit/file selection, the Tab focus ring, lazy per-commit fetches through
`query`, and hover-scoped wheel scrolling. The app's git plumbing and pure
parsing have *unit-level* coverage in `gpp-apps/git-viewers` (`cargo test -p
git-viewers`), tested against scratch repos — faster to iterate on than the full
harness.

**The start screen** (`tools/main-menu-integration-test.ts`) — the `main-menu`
panel a bare `garden` opens, under a redirected `$HOME` so its recents database
is hermetic. It seeds that database by *running* Garden (a first launch opens
two fixture files through the real `:e` path, then quits with Cmd-Q so the WAL
is checkpointed) rather than writing rows by hand — which is what makes it catch
schema drift between garden-app, the writer, and main-menu, the read-only
reader. It then asserts the menu comes up on those recents, that the keyboard
walks and clamps the one flat selection, and that a click on a Recent Files row
turns the pane into an editor on that file — the drawer's `mutate("open_path")`
travelling through `App::host_mutation` to the pane, which is the only thing
that proves the menu can actually open anything. A second launch on an empty
`$HOME` covers the first-ever run: three empty sections, no error.

**Multiple windows** (`tools/multi-window-integration-test.ts`) — the one
integration script that is **windowed-only** (it opens two real OS windows;
there is no headless multi-window path, since the whole point is the winit
window registry headless lacks). It launches with a redirected `$HOME` so
spawned windows load a known `init.ptl`, spawns a second window with
`:windownew`, and drives the per-window debug addressing (`?window=<ordinal>`,
`/windows`) to assert isolation (an edit in window 2 never touches window 1),
that closing the focused window leaves the process and the survivor intact
(ordinals are never renumbered), and that Cmd+Q quits. The pure
registry/close-vs-quit logic is unit-tested in `frontend::registry` and
`app::tests` (the lib+bin split exists so this logic is reachable without a
real event loop); the N-window lifecycle needs this real windowed harness.

## Layer 3 — Exploratory (manual, via the debug server)

Ad-hoc verification while developing — not saved as repeatable tests. Launch the
app with `--debug-port <n>` and poke at it; see `docs/debug-server.md` for the full
protocol and `CLAUDE.md` for the agent-driven recipe (drive input, read
`/state`, capture `/screenshot`). When this surfaces a bug, fix it *and* add a
unit or integration test so the bug can't come back — that closes the gap rather
than just the symptom.

## Adding coverage

1. New pure logic → a unit test next to it (Layer 1). Refactor toward pure
   functions if it's hard to test.
2. New end-to-end behavior (a keybinding, a command, persistence) → a check in
   the integration harness (Layer 2).
3. Found a bug exploring → reproduce it at Layer 1 or 2 first, then fix.
