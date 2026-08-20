# Garden — engineering notes

GPU-accelerated IDE in Rust, with Petal as the scripting layer.

## Where to look

- `README.md` — user-facing overview + how to run
- `docs/architecture.md` — crate map, public interfaces, design influences
- `docs/notes/agent-workflow.md` — conventions this repo expects: how to verify
  a change, where new behavior goes, traps that have cost real time
- `docs/testing.md` — the layered testing strategy
- `tools/README.md` — the dev tools (integration tests, fuzzer, bundler); all
  TypeScript run directly by Node, no build step
- `docs/debug-server.md` — live inspection + input injection protocol
- `docs/gpp.md` + `docs/writing-gpp-apps.md` — Garden Pane Protocol
  (subprocess-backed panes) and how to write a client
- `docs/petal-graphical-panels.md` — Petal-drawn `panel(...)` panes (draw/input API)
- `docs/petal-ide-mode.md` — `garden petal-ide` live editor + editor↔panel binding
- `docs/keybindings.md` — key routing
- `../docs/dev/releasing-garden.md` — cutting a Garden release + the Homebrew tap
