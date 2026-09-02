# Garden

An experimental GPU-accelerated IDE written in Rust, scripted with
[Petal](../README.md). Panes, layout, and several UI screens are driven by
Petal scripts. The editor is a modal vim layer over a rope buffer, with
tree-sitter syntax highlighting and a live-reloading layout.

## Install

With Homebrew (macOS only; Garden is a wgpu/winit GPU app):

```bash
brew install facetlayer/tap/garden
garden setup initialize-config-if-missing   # seeds ~/.garden
```

From source:

```bash
./install-local.sh   # cargo-installs `garden` and its GPP clients into
                     # ~/.cargo/bin, and seeds ~/.garden/init.ptl
```

Then run `garden` anywhere to open the GUI. On macOS,
`node tools/build-macos-app.ts` bundles a `Garden.app` with a Dock and Finder
icon (a plain `cargo build` only makes command-line executables).

Two setup commands manage `~/.garden`: `garden setup
initialize-config-if-missing` re-seeds a missing config (the installer runs
it), and `garden setup reset-config` resets the config files to defaults while
keeping `~/.garden/state` (window ids, layout overlays, the event log).

## Run

Build the whole workspace once (`cargo build`) so the GPP client binaries land
beside `garden`. Several subcommands spawn them, and they are not dependencies
of `garden-app`.

```bash
garden                       # the main menu: recent projects, files, PRs
garden --no-menu             # skip the menu; ~/.garden/init.ptl owns the layout
garden --init my.ptl         # custom layout script
garden notes.txt             # open a file directly, no script
garden src/                  # open a directory in the netrw-style browser
garden open git              # `open` forces a path (a file named `git`, not the git view)
garden --version             # version, git stamp, and feature flags
garden --version --json      # the same, machine-readable
```

`garden --version` is the first thing to run when an install misbehaves: it
prints the commit the binary came from and the named features it has, so a
missing endpoint reads as "this binary is old" instead of "unsupported". A
running Garden answers the same at `GET /version`; see
[debug-server.md](docs/debug-server.md#which-build-am-i-talking-to).

Subcommands open a specific view directly. Each is the CLI form of a `:` ex
command:

| Command | Opens |
|---------|-------|
| `garden git log` | git history browser (`:Git`) |
| `garden diff [base]` | editable review of the working tree vs `base` (default: upstream or `main`); `--stat` opens the summary view (`:Diff`, `:Review`) |
| `garden diff <PR#>` | the same review scoped to a GitHub PR (`:PR`) |
| `garden pr [n]` | the review for a GitHub PR, with its description and comments; the PR branch must be checked out |
| `garden pr --local [base]` | the same review over a purely local diff; no `gh`, no network |
| `garden petal-ide [file.ptl]` | live Petal editor plus rendered canvas ([docs](docs/petal-ide-mode.md)) |

Any builtin GPP client can also be launched by name: `garden sqlite-browser`,
`garden directory-browser`, `garden git-log`, `garden garden-diff`, `garden
main-menu`. The generic form is `garden --subprocess <app> [args…]`, which runs
any GPP client as the whole layout (the app's args come last, after Garden's
own flags).

With no arguments Garden opens the main menu: recent projects, files, and pull
requests, plus Open File… and Open Folder…. `~/.garden/init.ptl` still loads,
so its color scheme applies, but it does not decide the startup layout. `garden
--no-menu` (or `--init <path>`) gives the script the layout back. Edit the
script while Garden runs and the layout updates within about 200 ms; a broken
script keeps the last good layout and shows the error in the status bar.

### Run modes

The same editor core runs behind three frontends (`garden-app/src/frontend/`):

| Mode | Flag | What it is |
|------|------|------------|
| Window | (default) | winit window, wgpu renderer, native macOS menu bar |
| Terminal | `--term` | a TUI in the current terminal. Set `EDITOR="garden --term"` to use Garden from git, crontab, etc. Ctrl+Q force-quits. |
| Headless | `--headless --debug-port <n>` | no UI; the [debug server](docs/debug-server.md) is the only way to drive or observe it. Used for testing. |

## Layout scripting

```petal
layout(
    row([
        column([ editor("src/main.rs"), editor("notes.md") ], [0.7, 0.3]),
        editor("README.md"),
    ], [0.6, 0.4])
)
```

`row` and `column` nest arbitrarily. The optional second argument is a list of
ratios (equal split if missing or malformed). A leaf is one of:

- `editor(path?, config?)`: a text pane. `config` keys are `line_numbers`
  (bool, off by default) and `wrap` (bool, soft wrap, on by default). No path
  opens an empty buffer.
- `panel(script)` or `panel(script, { screens: [...] })`: a pane drawn by a
  Petal sketch running in-process. See
  [petal-graphical-panels.md](docs/petal-graphical-panels.md).
- `process(command, args?)`: a pane driven by a GPP subprocess. See
  [gpp.md](docs/gpp.md) and [writing-gpp-apps.md](docs/writing-gpp-apps.md).

### Layout as live state

Garden also rearranges panes at runtime and saves the result back as code.
`Ctrl-W o` expands the focused pane; `Ctrl-W s` / `Ctrl-W v` split it; dragging
the border between two panes resizes the split. Each change rewrites just the
`layout(...)` call (comments and helpers preserved) into a per-window overlay
at `~/.garden/state/window-<id>/window.ptl`, so your `init.ptl` is never
touched. Details: [architecture.md](docs/architecture.md#layout-as-editable-state-the-transient-overlay).

### Theming

A layout script can recolor the editor with `color_theme({...})` (any subset
of keys such as `window_bg`, `text`, `cursor`, `selection`, `syntax_*`) and
pick a base palette with `color_scheme("dark" | "light" | "brown" | "amiga")`.
Choosing a palette from View ▸ Color Scheme writes the `color_scheme(...)`
call back into your `init.ptl` in place.

### Git, diff and PR views

`:Git` / `garden git log` open the git history browser, a GPP app
(`gpp-apps/git-viewers`): a commit and file list beside the selected file's
diff, with draggable dividers, collapsible hunks, and a Refresh button.

Every diff and review path (`:Diff`, `:Review [base]`, `:PR [n]`, `garden
diff`, `garden pr`) opens the one `garden-diff` client
(`gpp-apps/garden-diff`). It has four views, switched by the header pills:

- **unified** (default): the classic `+`/`-`/context stream in a real vim
  editor. Editing the diff edits the change, and `^S` writes it back to the
  working tree. Delete a `+` line to drop that addition; delete a `-` line to
  revert that deletion; delete a hunk or file header to revert the whole hunk
  or file. All of vim works, undo included. In PR mode the review comments are
  threaded in at their lines and are inert to the save.
- **commits**: the review's own history. Click a commit to scope the diff to
  it (read-only); right-click for "everything since" or back to the whole
  review.
- **split**: a read-only base pane beside the editable working tree. `^S`
  writes back.
- **stat**: the per-file changed-lines diagram. `--stat` opens straight into
  it.

`/` searches the focused diff region with the usual search prompt; `n`/`N`
step through matches.

`:PR [n]`, `garden pr [n]`, and `garden diff <PR#>` scope the review to a
GitHub PR via `gh`, adding its title, description, conversation, and inline
review comments. Because edits write to the working tree, the PR's branch must
be checked out first (`gh pr checkout N`). `garden pr --local [base]` reviews
a purely local diff instead.

## Status

Working today: multi-pane window from a Petal script with live reload,
editable buffers with coalescing undo/redo, line-number gutters, mouse
selection, a modal vim layer ([keybindings.md](docs/keybindings.md)),
system-clipboard copy/cut/paste, a fuzzy file finder (Cmd/Ctrl+P), auto-refresh
of files changed on disk, and a debug server for live inspection and
automation.

Syntax highlighting (tree-sitter) covers about 26 bundled languages: Rust,
Python, JS/TS/TSX, Go, C/C++, Java, C#, Ruby, PHP, HTML, CSS, Bash, YAML, Lua,
Scala, Haskell, SQL, Zig, Nix, JSON, TOML, Markdown, and Petal.

Not yet: a file tree and tabs. An LSP client exists but is early (document
sync and diagnostics for `.ptl` files; completion is not wired up).

## Editing and keybindings

The vim layer (motions, operators, Visual mode, search, the `:` command line)
and the global Mac-style shortcuts are in
[keybindings.md](docs/keybindings.md).

## Debug server

`--debug-port <n>` starts a localhost HTTP server inside the app for state
inspection, synthetic input, and offscreen screenshots, so a script or agent
can drive and verify the running editor. It works in every run mode;
`--headless` requires it.

```bash
PORT=8080
garden --headless --debug-port $PORT
curl -s 127.0.0.1:$PORT/state | jq .panes
curl -s 127.0.0.1:$PORT/screenshot -o shot.png   # offscreen GPU render, even headless
```

Full protocol: [debug-server.md](docs/debug-server.md).

## Workspace

| Crate | Purpose |
|-------|---------|
| `garden-core` | Text model: rope buffer, edits, undo, editable projections |
| `garden-render` | wgpu renderer: quads, meshes, images, text (windowed and headless) |
| `garden-script` | Petal embedding: layout tree, hot reload, the panel runtime |
| `garden-app` | App core (panes, editor views, input) plus the window/terminal/headless frontends |
| `gpp` | Garden Pane Protocol: the JSON-RPC contract for subprocess-backed panes |
| `gpp-apps/directory-browser` | the netrw-style directory listing (`garden <dir>`, `:E`, `-`) |
| `gpp-apps/git-viewers` | the `git-log` app behind `:Git` |
| `gpp-apps/garden-diff` | the diff/review tool (`:Diff`, `:Review`, `:PR`, `garden diff`, `garden pr`) |
| `gpp-apps/sqlite-browser` | a read-only SQLite and Postgres browser (`garden sqlite-browser db.sqlite` or `… postgres://host/db`) |
| `gpp-apps/main-menu` | the start screen a bare `garden` opens |
| `gpp-python/` | a Python GPP client library plus two sample apps ([README](gpp-python/README.md)) |

Two workspace members exist only for developing Garden itself. They are not
installed by `install-local.sh`; launch them with `garden --subprocess <name>`:

| Crate | Purpose |
|-------|---------|
| `gpp-apps/gpp-test-app` | test fixture that puts a pane into a chosen state (healthy, query error, runtime error, save) for screenshots and integration tests |
| `gpp-apps/screens-demo` | worked example of GPP screen navigation: two drawers and the host-owned history stack |

`petal`, `petal-ui`, `petal-query`, and `tree-sitter-petal` are path
dependencies on the Petal crates one level up in this repo.

## Development

```bash
cargo build                          # whole workspace
cargo test                           # full suite
node tools/integration-test.ts       # end-to-end checks through the headless frontend
cargo run -p garden-render --example demo   # renderer standalone smoke test
```

See [tools/README.md](tools/README.md) for the dev tools and
[docs/testing.md](docs/testing.md) for the test strategy.

## License

MIT; see [LICENSE](LICENSE), the same terms as Petal.

The bundled font is JetBrains Mono (`garden-render/assets/`), Copyright 2020
The JetBrains Mono Project Authors, licensed under the SIL Open Font License
1.1; see [OFL.txt](garden-render/assets/OFL.txt).

## Docs

- [architecture.md](docs/architecture.md): crate map and design
- [keybindings.md](docs/keybindings.md): vim layer and global shortcuts
- [debug-server.md](docs/debug-server.md): live inspection and automation protocol
- [petal-graphical-panels.md](docs/petal-graphical-panels.md): Petal-drawn panel panes
- [petal-ide-mode.md](docs/petal-ide-mode.md): the `garden petal-ide` live editor and canvas
- [gpp.md](docs/gpp.md): the Garden Pane Protocol
- [writing-gpp-apps.md](docs/writing-gpp-apps.md): how to write a GPP app in Rust
- [writing-gpp-apps-python.md](docs/writing-gpp-apps-python.md): the same in Python
- [testing.md](docs/testing.md): the layered test strategy
- [tools/README.md](tools/README.md): dev tools
- [docs/notes/agent-workflow.md](docs/notes/agent-workflow.md): conventions for working in this repo
