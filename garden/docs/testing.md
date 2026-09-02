# Testing strategy

Garden uses a layered strategy adapted to a Rust GPU app: push as much coverage
as possible down to fast unit tests, and reserve the slow top layer for what
only the running app can exercise.

## Layer 1: unit tests (`cargo test`)

Pure logic, no window or GPU. This is where most coverage lives and the first
place to add a test for any new behavior.

| Area | Location | Covers |
|------|----------|--------|
| Text model | `garden-core/src/tests.rs` | `Buffer`, `Point`, `Selection`, undo/redo coalescing, file I/O |
| Editable projections | `garden-core/src/projection/tests.rs` | the origin table: splice transform, fold back into sources, undo/redo of origins, structural intents |
| Pane layout | `garden-app/src/layout.rs` | the layout solver: nesting, ratios |
| Editor view | `garden-app/src/editor_view.rs` | cursor/selection geometry, tab stops, scrolling math, scene primitives |
| Vim | `garden-app/src/vim.rs` | the mode/motion/operator state machine, driven key by key; vim commands over a projected view |
| Search | `garden-app/src/search.rs` | wraparound matching, whole-word boundaries, substitution, non-ASCII columns |
| Command line | `garden-app/src/command_line.rs` | ex-command parsing (including `:s` ranges and flags), the `/` and `?` prompts |
| Window nav | `garden-app/src/window_nav.rs` | spatial pane navigation and the pure layout-tree edits |
| App core | `garden-app/src/app/tests.rs` | the `App` input/command surface in memory: key routing, ex commands, splits, clipboard, file refresh, `/state` JSON |
| File finder | `garden-app/src/file_finder.rs` | the fuzzy matcher, ranking, and the finder state machine |
| Panel tessellation | `garden-app/src/panel_tess.rs` | geometry to mesh triangles for Petal panels |
| State / event log | `garden-app/src/state.rs`, `event_log.rs`, `recents.rs` | the SQLite state dir: window ids, migrations, event buffering, `:report`, recents (temp-dir DBs) |
| Terminal grid | `garden-app/src/frontend/grid.rs` | scene to character-grid rasterization for the TUI |
| Window registry | `garden-app/src/frontend/registry.rs` | multi-window bookkeeping, close vs. quit |
| GPP contract | `gpp/src/lib.rs` | v2 envelope serde round-trips and NDJSON framing |
| GPP clients | `gpp-apps/*/src/` | directory-browser's listing core; `git-log`'s git plumbing (`cargo test -p git-viewers`); `garden-diff`'s diff parsing, projection spec, comment weaving, and write-back splicing (`cargo test -p garden-diff`), against real temporary repos |
| Renderer | `garden-render/tests/srgb_compositing.rs` | pins the sRGB blending numbers across all three pipelines |

```bash
cargo test                 # whole workspace
cargo test -p garden-app   # one crate
```

Keep editor and vim logic pure (data in, data out, over `EditorView`'s public
API) so it never needs a window. `vim::handle(view, key, ..)` is the model:
fully tested without rendering a frame.

## Layer 2: integration tests (`tools/*.ts`)

The whole stack (frontend loop, key routing, vim, command line, file I/O)
driven through the [debug server](debug-server.md) over HTTP, asserting on
`/state`, `/buffer`, and files on disk. This catches wiring bugs unit tests
structurally cannot.

```bash
node tools/integration-test.ts            # headless frontend (no window, CI-safe)
node tools/integration-test.ts --window   # the same through the real winit/wgpu frontend
```

Each script boots the app on a free port (`--headless --debug-port 0`), runs a
scripted user flow, and tears the app down. Keep them fast and deterministic:
drive with injected input, assert on observable state, avoid sleeps beyond the
startup poll.

A script declares the build features it depends on, and the harness checks
`GET /version` before the first assertion:

```ts
launchGarden({ requireFeatures: ["cli.panel-wake", "state.values-filter"] })
```

A binary too old for the test then fails at launch with its commit and build
date. Feature names live in `garden-app/src/version.rs`; see
[debug-server.md](debug-server.md#which-build-am-i-talking-to).

The scripts, and what each proves:

- **`integration-test.ts`**: vim editing, the command line, file I/O, and the
  directory-browser pane.
- **`diff-review-integration-test.ts`**: `garden-diff` on a throwaway git
  repo. Asserts on the values the panel's frame bound (`/state` →
  `panes[].panel.values`): the diff loads, the header pills switch views, and
  both write-back paths reach the file on disk. The drawer gives its click
  targets plain `let` names, so the test clicks by geometry it reads rather
  than hard-codes.
- **`git-panel-integration-test.ts`**: the `:Git` panel (`git-log`) on a
  multi-commit fixture with a dirty worktree: selection, the Tab focus ring,
  lazy per-commit fetches, hover-scoped wheel scrolling.
- **`gpp-test-app-integration-test.ts`**: `garden --subprocess gpp-test-app
  <mode>` for each fixture mode: a healthy panel that keeps ticking, a failed
  query surfaced through `error_of`, and a frame error raising the error card.
- **`python-gpp-integration-test.ts`**: runs `gpp-python/test_gpp.py`, then
  boots both Python apps headless (`sysmon`, `repo-stats`): live data reaching
  the drawer, a screenshot, and the soft error path on a non-repo directory.
- **`main-menu-integration-test.ts`**: the start screen, under a redirected
  `$HOME`. It seeds the recents database by really opening files through
  `:e` and quitting, which catches schema drift between the writer
  (garden-app) and the reader (main-menu). Then: the menu lists those recents,
  the keyboard walks the selection, and a click on a row turns the pane into
  an editor on that file. A second launch on an empty `$HOME` covers the first
  run.
- **`multi-window-integration-test.ts`**: windowed only (it opens two real OS
  windows). Spawns a second window with `:windownew` and drives the per-window
  debug addressing (`?window=<n>`, `/windows`) to assert isolation, that
  closing one window leaves the other intact, and that Cmd+Q quits.
- **`screenshot-consistency-test.ts`**: the debug server's settle-then-capture
  contract, down to the captured PNG's pixels.

### Headless apps clean up after themselves

A headless app has no window to close, so a run whose launcher died would hold
its port until reboot. The harness sets `GARDEN_HEADLESS_IDLE_TIMEOUT` on every
launch (`tools/lib/app.ts`), and the app exits after that long with no debug
request. When launching headless by hand, set it yourself:

```bash
GARDEN_HEADLESS_IDLE_TIMEOUT=1800 garden --headless --debug-port 8080 &
```

Before assuming a test hangs, check for leftovers: `ps -eo pid,ppid,command |
grep '[g]arden --headless'`. Anything with a ppid of 1 is an orphan. Details:
[debug-server.md](debug-server.md#when-a-headless-run-stops-by-itself).

## Layer 3: exploratory (manual, via the debug server)

Ad-hoc verification while developing, not saved as repeatable tests. Launch
with `--debug-port <n>` (and the idle timeout, if headless) and poke at it:
inject input, read `/state`, capture `/screenshot`. When this surfaces a bug,
fix it and add a unit or integration test so it cannot come back.

## Adding coverage

1. New pure logic: a unit test next to it (Layer 1). Refactor toward pure
   functions if it is hard to test.
2. New end-to-end behavior (a keybinding, a command, persistence): a check in
   an integration script (Layer 2).
3. Found a bug exploring: reproduce it at Layer 1 or 2 first, then fix.
