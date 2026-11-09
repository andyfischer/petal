# petal-ui-headless

Run a [petal-ui](../petal-ui) script with no renderer attached. This is
tooling built on petal-ui's host contract, kept out of `petal-ui` so a host
links none of it.

- `harness::Headless`: a test driver over `petal_ui::frame_core::FrameCore`
  (the frame core Garden's panels run on) with a fixed 1/60 s clock. Feed it
  input (`click`, `click_target`, `key`, `text`, `mouse_move`, `scroll`), run
  `frame()`, and read the state, draw commands and target registry.
- `scenario::Scenario`: replayable input scripts (JSON keyed by frame, or
  generated from a seed). The C bridge's `pb_scenario` uses the same format.
- `drag_trace`: explain a `ui/drag` drag in flight from the state dump.
- `run_cli` / `propose_edit_cli`: the `petal-ui-run` trace driver and
  `petal-ui-propose-edit` as library entry points that take a `CliHost`.
  This crate's binaries are those over `NoHost`.

The crate names no host. A script written for a host with natives of its own
runs with that host's stand-ins passed as a setup hook
(`Headless::from_file_with_setup`, `CliHost::setup`). Garden's are
`garden_script::headless::register_panel_stubs`, and
`garden/garden-panel-run` ships `garden-panel-run` and
`garden-panel-propose-edit`: the same CLIs with them registered, which is
what the repo's tools use, since much of `examples/` does `import garden`.

```sh
cargo run --bin petal-ui-run -- ../../examples/ui/layers/app.ptl --frames 3 --targets
cargo test
```

See [docs/dev/headless-ui-run.md](../../docs/dev/headless-ui-run.md) for the
trace format, the flags and scenarios.
