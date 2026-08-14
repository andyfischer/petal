# Garden

An experimental GPU-accelerated IDE written in Rust, scripted with
[Petal](https://github.com/andyfischer/petal). Panes, layout, and several UI
screens are driven by Petal scripts; the editor is a modal vim layer over a rope
buffer, with tree-sitter syntax highlighting and live-reloading layout.

## Install

```bash
./install-local.sh   # cargo-installs `garden` + its GPP clients into
                     # ~/.cargo/bin, and seeds ~/.garden/init.ptl
```

Then type `garden` anywhere to open the GUI. On macOS, `tools/build-macos-app.ts`
produces a Finder/Dock-launchable `Garden.app` with the icon (a plain `cargo
build` only makes CLI executables; a raw binary sets the Dock icon at runtime but
has no persistent Finder icon).

`garden setup initialize-config-if-missing` re-seeds a missing `~/.garden`
(idempotent; the installer runs it), and `garden setup reset-config` resets the
config files to defaults while preserving `~/.garden/state` (window ids, layout
overlays, the event-log DB).

## Run

Build the whole workspace once (`cargo build`) so the GPP client binaries land
beside `garden` — several subcommands spawn them and they are *not* dependencies
of `garden-app`.

```bash
garden                       # GPU window: the main menu (recent projects, files, PRs)
garden --no-menu             # skip the menu — ~/.garden/init.ptl owns the layout
garden --init my.ptl         # custom layout script
garden notes.txt             # a file opens directly, no script
garden src/                  # a directory opens the navigable browser (vim netrw style)
garden open git              # `open` forces a path (opens a file named `git`, not the git view)
```

Subcommands open a specific view directly (the CLI counterparts of the `:` ex
commands):

| Command | Opens |
|---------|-------|
| `garden git log` | git history browser (`:Git`) |
| `garden diff [base]` | **editable** before/after review vs `base` (default: upstream/`main`), `^S` writes back; `diff --stat` opens the summary view (`:Diff` / `:Review`) |
| `garden diff <PR#>` | the same review scoped to a GitHub PR (base merge-base → working tree), with the PR metadata (`:PR`) |
| `garden pr [n]` | the same review for a GitHub PR: merge-base diff, description, conversation + inline `gh` comments — needs the branch checked out |
| `garden pr --local [base]` | the same review over a purely local diff vs `base` — no `gh`, no network |
| `garden petal-ide [file.ptl]` | live Petal editor + rendered canvas ([docs](docs/petal-ide-mode.md)) |

Any builtin GPP client is also launchable by name —
`garden sqlite-browser`, `garden directory-browser`, `garden git-log`,
`garden garden-diff`, `garden main-menu` — the ergonomic form of the generic
`garden --subprocess <app> [args…]` (which runs any GPP client as the whole
layout; its args come last, after Garden's own flags).

With no arguments Garden opens the **main menu** (`garden main-menu`): your
recent projects, files, and pull requests, plus Open File… / Open Folder…
`~/.garden/init.ptl` still loads, so its color scheme and settings apply, but it
no longer decides the startup layout — `garden --no-menu` (or `--init <path>`)
gives the script the layout back. Edit the script while Garden runs — the layout
updates live within ~200 ms, and a broken script keeps the last good layout with
the error in the status bar.

### Run modes

The same editor core runs behind three pluggable frontends
(`garden-app/src/frontend/`):

| Mode | Flag | What it is |
|------|------|------------|
| Window | (default) | winit window, wgpu GPU renderer; native macOS menu bar |
| Terminal | `--term` | a TUI in the current terminal — set `EDITOR="garden --term"` to use Garden from git, crontab, etc. Ctrl+Q force-quits. |
| Headless | `--headless --debug-port <n>` | no UI; the [debug server](docs/debug-server.md) is the only way to drive/observe it. Used for testing. |

## Layout scripting

```petal
layout(
    row([
        column([ editor("src/main.rs"), editor("notes.md") ], [0.7, 0.3]),
        editor("README.md"),
    ], [0.6, 0.4])
)
```

`row`/`column` nest arbitrarily; the optional second argument is a normalized
ratio list (equal split if malformed). A leaf is one of:

- `editor(path?, config?)` — a text pane; `config` keys are `line_numbers`
  (bool, off by default) and `wrap` (bool, soft-wrap, on by default; `:set
  nowrap` persists here). No path opens an empty buffer.
- `panel(script)` / `panel(script, { screens: [...] })` — a pane whose pixels
  are drawn by a Petal sketch running in-process (a processing-style `draw()`
  loop). The optional `screens` list is an explicit navigation allowlist that
  narrows which sibling `.ptl` screens the panel's `navigate(...)` API may reach.
  See [petal-graphical-panels.md](docs/petal-graphical-panels.md).
- `process(command, args?)` — a pane driven by a GPP subprocess over JSON-RPC
  stdio. See [gpp.md](docs/gpp.md), and [writing-gpp-apps.md](docs/writing-gpp-apps.md)
  to build one.

### Layout as live, editable state

The layout is code, but Garden also rearranges panes at runtime and saves the
result back as code. `Ctrl-W o` expands the focused pane ("only"); `Ctrl-W s` /
`Ctrl-W v` split it stacked / side-by-side. **Drag the thin border between two
panes** to resize the split — the ratio updates live and is saved on release.
Each change is persisted by rewriting
just the `layout(...)` call — comments, `color_theme`, and helpers preserved
verbatim — into a **per-window overlay** at
`~/.garden/state/window-<id>/window.ptl`, so windows persist layouts
independently and your committed `init.ptl` is never touched. The rewrite uses
Petal's formatting-preserving `petal::rewrite` tooling.

### Theming

A layout script can recolor the editor with `color_theme({...})` (any subset of
keys — `window_bg`, `text`, `cursor`, `selection`, `titlebar_bg`, `syntax_*`, …;
unset keys keep the dark defaults) and pick a base palette with
`color_scheme("dark" | "light" | "brown" | "amiga")`. Choosing a palette from
**View ▸ Color Scheme** writes the `color_scheme(...)` call back into your
`init.ptl` (in place, comments preserved) via Garden's goal-based settings write;
see [architecture.md](docs/architecture.md).

### Git, diff & PR views

`:Git` / `garden git log` open the git history browser — a panel-mode GPP app
(`gpp-apps/git-viewers`): a commit/file list beside the selected file's line diff,
with draggable dividers, collapsible hunks, and a **⟳ Refresh**.

Every diff and review path — `:Diff`, `:Review [base]`, `:Review2`, `:PR [n]`,
`garden diff`, `garden pr` — opens the one **garden-diff** client
(`gpp-apps/garden-diff`). It has four views, switched by the header pills:

- **unified** (default) — the classic `+`/`-`/context stream in a real-vim editor,
  as an **editable projection**: the editor knows where every line came from, so
  editing the diff *is* editing the change, and `^S` folds it back into the
  working-tree files. Delete a `+` line to drop that addition, delete a `-` line to
  revert that deletion (the base line comes back where the diff showed it), retext
  either to change what lands, type a bare line in to add one — or delete a hunk
  header to revert that whole hunk, or a file header to drop that file's changes.
  All of vim works on it, undo included. In PR mode the inline review comments are
  threaded in at the lines they were left on — and are inert to the save, so
  editing or deleting one changes no file. The `+`/`-`/space markers are drawn
  in the region's **gutter**, not stored in the buffer, so the text you edit is
  the file's own text: `J` joins two lines without dragging a `+` into the seam,
  a column selection takes no marker with it, and `/` matches what you see.
- **commits** — the review's own history, newest first. Click a commit to scope
  the diff to it alone; right-click for the rest (everything *since* that
  commit, or back to the whole review). A commit-scoped diff is **read-only**:
  it describes files as they were at that commit, so its line numbers address a
  blob rather than the checkout and there is nothing to write back to. A
  "since" scope still ends at the working tree, so it stays editable.
- **split** — a read-only base pane beside the working tree in a real-vim
  editor. Edit the right side and `^S` folds the changes straight back into the
  working-tree files. It is a projection too, just a plainer one: the column is
  the new file line for line, so there is no `-` line to revive and no header to
  revert — deleting one of its `@@@` markers is refused rather than half-doing it.
- **stat** — the per-file changed-lines diagram; `--stat` opens straight into it.

`/` searches the focused diff region: it opens the host's usual search prompt,
the pattern searches that region's buffer, and `n`/`N` step through the matches.

`:PR [n]` / `garden pr [n]` / `garden diff <PR#>` scope the review to a GitHub PR
(its base merge-base → working tree, resolved via `gh`), adding the PR
number/title/author, its description + conversation, and the inline review
comments. Because the edit writes to the working tree, the PR's branch must be
checked out first (`gh pr checkout N`, else a hint). `garden pr --local [base]`
reviews a purely local diff instead — no `gh`, no network.

## Status

Working today: multi-pane window from the Petal script with live hot reload
(Petal `state` preserved), editable buffers with coalescing undo/redo, line-number
gutters, click-to-position, rich text selection, a **modal vim editing layer**
(see [keybindings.md](docs/keybindings.md)), system-clipboard copy/cut/paste, a
**fuzzy file finder** (Cmd/Ctrl+P), auto-refresh of files changed on disk, save,
and a debug server for live inspection/automation.

Syntax highlighting (tree-sitter) for ~26 bundled languages — Rust, Python,
JS/TS/TSX, Go, C/C++, Java, C#, Ruby, PHP, HTML, CSS, Bash, YAML, Lua, Scala,
Haskell, SQL, Zig, Nix, JSON, TOML, Markdown, and Petal — colored per token and
cached by buffer revision, from a data-driven registry (`syntax.rs`).

Not yet: a file tree and tabs.

## Editing & keybindings

The modal vim layer (motions, operators, Visual mode, search, `:` command line)
and the global Mac-style shortcuts are documented in
[keybindings.md](docs/keybindings.md).

## Debug server

`--debug-port <n>` starts a localhost HTTP server inside the app for live state
inspection, synthetic input, and offscreen screenshots — so agents can drive and
verify the running editor. No default port; works in every run mode, and
`--headless` requires one:

```bash
PORT=8080
garden --headless --debug-port $PORT
curl -s localhost:$PORT/state | jq .panes
curl -s localhost:$PORT/screenshot -o shot.png   # offscreen GPU render, even headless
```

Full protocol: [debug-server.md](docs/debug-server.md).

## Workspace

| Crate | Purpose |
|-------|---------|
| `garden-core` | Text model: rope buffer, edits, undo |
| `garden-render` | wgpu renderer: quads + glyphon text (windowed + headless) |
| `garden-script` | Petal embedding, layout tree, hot reload |
| `garden-app` | App core (panes, editor views, input) + window/terminal/headless frontends |
| `gpp` | Garden Pane Protocol: the JSON-RPC contract for subprocess-backed panes |
| `gpp-apps/directory-browser` | Lines-mode GPP client: a navigable directory listing |
| `gpp-apps/git-viewers` | Panel-mode GPP app behind `:Git` (`git-log`) |
| `gpp-apps/garden-diff` | Panel-mode GPP client: the one diff/review tool — editable split, unified, stat (`:Diff`/`:Review*`/`:PR`, `garden diff`, `garden pr`) |
| `gpp-apps/sqlite-browser` | Panel-mode GPP client: a read-only SQLite/Postgres browser + visualizer (`garden sqlite-browser db.sqlite` or `… postgres://host/db`) |

### Internal only

Workspace members for developing Garden itself — not tools to use, and not
installed by `install-local.sh`. They exist so the host's tests and the protocol
docs have something concrete to drive, and neither has a bare `garden <name>`
subcommand: launch them with `garden --subprocess <name> [args…]`.

| Crate | Purpose |
|-------|---------|
| `gpp-apps/gpp-test-app` | Test fixture: puts a pane into a chosen situation on demand (healthy, query error, runtime error, long error, save) so panel behavior — chiefly the error card — is reproducible in an integration test or a screenshot |
| `gpp-apps/screens-demo` | Worked example of GPP screen navigation: two drawers, `navigate`/`navigate_back`, and the host-owned history stack — it answers no queries at all |

`petal`, `petal-ui`, `petal-query`, and `tree-sitter-petal` are path
dependencies on the Petal crates one level up in this repo.

## Development

```bash
cargo build                          # whole workspace
cargo test                           # full suite across core/script/app/gpp + the gpp-apps
node tools/integration-test.ts       # end-to-end checks through the headless frontend
cargo run -p garden-render --example demo   # renderer standalone smoke test
```

## License

MIT — see [LICENSE](LICENSE), the same terms as Petal.

The bundled font is **JetBrains Mono** (`garden-render/assets/`), Copyright 2020
The JetBrains Mono Project Authors, licensed under the SIL Open Font License
1.1 — see [OFL.txt](garden-render/assets/OFL.txt).

## Docs

- [architecture.md](docs/architecture.md) — design + crate contracts
- [petal-ide-mode.md](docs/petal-ide-mode.md) — the `garden petal-ide` live editor + canvas
- [keybindings.md](docs/keybindings.md) — vim editing layer + global shortcuts
- [debug-server.md](docs/debug-server.md) — live inspection/automation protocol
- [gpp.md](docs/gpp.md) — the Garden Pane Protocol for subprocess-backed panes
- [writing-gpp-apps.md](docs/writing-gpp-apps.md) — how to write a panel-mode GPP app
- [petal-graphical-panels.md](docs/petal-graphical-panels.md) — Petal-drawn panel panes
- [testing.md](docs/testing.md) — the layered test strategy
