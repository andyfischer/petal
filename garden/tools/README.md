# Garden dev tools

Everything here is TypeScript run directly by Node (>= 22.6, for native type
stripping). There is no build step, no `node_modules`, and no dependencies
outside Node's standard library:

```bash
node tools/integration-test.ts
```

Each file has a `#!/usr/bin/env node` shebang and the executable bit, so
`./tools/integration-test.ts` works too.

Because the files are type-stripped rather than compiled: relative imports
carry an explicit `.ts` extension, compile-time-only types use `import type`,
and there are no `enum`s, namespaces, or constructor parameter properties.

## The tools

| Tool | What it does |
| --- | --- |
| `integration-test.ts` | The top-layer functional test: vim editing, the command line, file I/O, and the directory-browser GPP pane. `--window` runs it through the real winit/wgpu frontend. |
| `diff-review-integration-test.ts` | The `garden-diff` review client end to end, against a throwaway git repo. |
| `git-panel-integration-test.ts` | The `:Git` history browser (`git-log` GPP app). |
| `gpp-test-app-integration-test.ts` | The gpp-test-app fixture's pane states: healthy panel, soft query error, and the runtime-error card. |
| `python-gpp-integration-test.ts` | The Python GPP client (`gpp-python/`): the library's unit tests, then both Python apps (sysmon, repo-stats) booted headless — live data, screenshots, and the soft error path. |
| `main-menu-integration-test.ts` | The start screen a bare `garden` opens, on a recents database seeded by really opening files under a throwaway `$HOME`. |
| `multi-window-integration-test.ts` | Two real OS windows and the per-window debug addressing. Windowed-only. |
| `screenshot-consistency-test.ts` | The debug server's settle-then-capture contract, asserted down to the captured PNG's pixels. |
| `build-macos-app.ts` | Bundles `Garden.app` with its Dock/Finder icon. macOS only. |
| `reports-tool.ts` | Browses and deletes the `:report` items in the state database. |
| `vim-parity/fuzz.ts` | Differential fuzzer against real Vim — see `vim-parity/README.md`. |
| `vim-parity/oracle-xcheck.ts` | Cross-checks the two candidate Vim oracles against each other. |

See [`../docs/testing.md`](../docs/testing.md) for where these sit in the
layered testing strategy.

## `lib/`

The shared pieces the tests are written in terms of:

- `check.ts`: the pass/fail tally. Assertions record rather than throw, so one
  failure doesn't hide the checks after it.
- `debug-client.ts`: a typed client for the
  [debug server](../docs/debug-server.md): reading `/state`, `/scene`,
  `/buffer`, `/screenshot`, and injecting keys, text, and mouse gestures
  (including pane-local coordinates).
- `app.ts`: `cargoBuild` and `launchGarden`, which start the app on an
  OS-chosen port, discover the port from the startup line, and set the
  headless idle timeout.
- `util.ts`: subprocesses, `waitUntil` polling, throwaway work directories,
  and cleanup that runs however the script exits.
- `png.ts`: a minimal RGBA8 PNG reader, for asserting on captured pixels.
