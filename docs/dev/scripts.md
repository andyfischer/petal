# Developer Scripts & Commands

The commands used to build, run, test, and benchmark Petal during
development.

## Make targets

The [`Makefile`](../../Makefile) wraps the most common tasks. Run `make` (or
`make help`) to list them.

| Command | Description |
|---------|-------------|
| `make build` | Build the Petal compiler (debug). Binary lands at `core/target/debug/petal`. |
| `make test` | Build, then run the full vitest suite (which also runs every `examples/console/*.ptl`). |
| `make test-examples` | Print each example program's output for manual inspection. |
| `make test-c-bridge` | Build `integrations/petal-c-bridge` (the C/C++ embedding bridge) with CMake + Ninja and run its `ctest` suite: C++ tests, plain-C smoke test, example, and the crate's Rust unit tests. Needs CMake, Ninja and a C++20 compiler. See [embedding-c.md](../embedding-c.md). |
| `make prove` | Run the Kani proof harnesses (`cargo kani`) for the core numeric semantics. Needs Kani installed; see [formal-verification.md](formal-verification.md). |
| `make clean` | Remove Rust build artifacts (`cargo clean`). |

## Building & running

| Command | Description |
|---------|-------------|
| `cd core && cargo build` | Build the debug binary. |
| `cd core && cargo build --release` | Build the optimized binary at `core/target/release/petal` (used by the benchmarks). |
| `cd core && cargo test` | Run the Rust unit tests. |
| `./tools/run-petal.ts run <file.ptl>` | Run Petal locally: rebuilds the binary if needed, then forwards all arguments to `petal`. |
| `./tools/run-petal.ts run -e '<expr>'` | Run a one-liner. |
| `core/target/debug/petal run <file.ptl>` | Run the binary directly (no auto-rebuild). |

## Testing

See [testing.md](testing.md) for the full guide.

| Command | Description |
|---------|-------------|
| `npm test` | Run the vitest suite once, from the repo root. |
| `npm run test:watch` | Vitest in watch mode. |
| `cd ts && npx vitest test/ir-basics.test.ts` | Run one test file. |
| `cd core && cargo kani --solver cadical` | Prove the contracts of `core/src/numeric.rs` for every input. `--harness <name>` runs one. See [formal-verification.md](formal-verification.md). |
| `PETAL_SMALL_SCOPE_TOKENS=4 cargo test --release --test small_scope` | Run the small-scope exhaustive check over every program of up to 4 tokens (6.3M programs, about a minute). The default `cargo test` covers 2. |
| `./tools/test-examples.ts` | Run every `examples/console/*.ptl` with the optimizer on and off (`--no-opt`), require identical output between the two, and require both to match the golden corpus in `test/example-golden/`. |
| `./tools/test-examples.ts --full` | Same, but print full output rather than an 8-line preview. |
| `./tools/gen-example-golden.ts` | Re-baseline `test/example-golden/` from the current output. Run deliberately: a golden update asserts that the intended behavior changed. |
| `cd core-libs/petal-ui && cargo run --bin petal-ui-run -- <app.ptl> [flags]` | Run a **UI** app headlessly for N frames and write a JSONL trace of draw commands, `state`, prints, and errors. Deterministic given `--seed` and `--scenario`. See [headless-ui-run.md](headless-ui-run.md). |
| `petal-ui-run <app.ptl> --scenario monkey:7 --frames 120 --out trace.jsonl` | The same, driven by a generated pseudo-random input scenario. |
| `petal-ui-run <app.ptl> --gate-stats` | Frames run vs skipped by the frame gate, with a histogram of why frames ran — the first thing to check when a panel that should idle keeps running. `--no-gate` runs every frame. See [frame-gate.md](frame-gate.md). |
| `petal-ui-run <app.ptl> --memo-stats` | The memo's counters (scopes replayed, re-run, recorded, folded, effectful) — the first thing to check when a widget that should replay keeps running. `--no-memo` runs every call. See [memo-scopes.md](memo-scopes.md). |
| `petal-ui-run <app.ptl> --effect-audit` | Watch what every native does over the run and report each one whose behavior differs from its declared effect row: under-declared (a staleness bug the memo cannot see; exit 3) or over-declared (a facet this run never took). `petal run --effect-audit` does the same for a plain script; `cargo test -p petal-ui --test effect_audit -- --nocapture` runs it over the whole corpus. See [declarative-effect-refactoring.md](../tasks/declarative-effect-refactoring.md). |
| `./tools/verify.ts --plan <plan> --before <ref\|dir> --after <dir>` | Prove a refactor was behavior-preserving by running a plan of checks over a corpus. Use `--before-bin`/`--after-bin` to compare two binaries instead. Plans live in `test/verify-plans/`. See [testing.md](testing.md#verifying-a-refactor). |
| `./tools/verify.ts --plan compiler ... --update-golden` | Re-baseline `test/ui-golden/index.json` (a sha256 per UI app trace) from the after side. Run deliberately. |

## Benchmarking

| Command | Description |
|---------|-------------|
| `petal bench <file> --fn <name> [--fn <name>]...` | What one call of a named function costs, over the calls the script itself makes: calls, instructions and ms per call (min / median / p95, inclusive and self), allocations, list/record copies and collections per call, with the optimizer on and off side by side. Re-runs the file for about a second per side (`--iters N` pins the count); `--json` for a machine-readable report. Core-host scripts only. See [CLI.md](../CLI.md#bench--what-a-call-of-a-function-costs). |
| `./tools/bench-opts.ts` | Time every [`test/benchmarks/`](../../test/benchmarks/)`*.ptl` with the optimizer on and off (release build) and report per-file medians plus the speedup. |
| `./tools/bench-opts.ts --runs=10` | Use more repetitions per file (default 5). |
| `cd core-libs/petal-ui && cargo run --release --example bench_panel -- <file.ptl> [frames] [WxH]` | Per-frame cost of a **panel** script under the headless harness, which is the shape of work a Garden pane does. Add `--observe` to mirror a real panel (Garden leaves observation on) and `--profile` for the counters below. Frames run under the frame gate, so add `--wiggle` (pointer moves every frame) for an interactive frame or `--no-gate` for the script alone. Calls are memoized unless `--no-memo`; the memo's counters are printed either way. |

See [performance.md](performance.md) for how to read these numbers.

## Profiling

| Command | Description |
|---------|-------------|
| `petal run --profile <file>` | Count what the run executed (instructions per opcode, builtin calls by name, user calls, collections) and print the histogram to stderr. Works in any build, including a shipped release binary. |
| `petal run --dup-stats <file>` | Value-duplication and heap-allocation counters. A runtime switch, so it works in any build; the `dup-stats` cargo feature only makes it the default (as it is in debug builds). |
| `PETAL_OPT_STATS=1 petal run <file>` | Report what the bytecode optimizer did: instructions before/after, moves removed, reads rewritten, jumps threaded. |
| `PETAL_POLICY=<name> petal run <file>` | Run under a named [run policy](../../core/src/policy.rs) — `fast` (default), `baseline`, `explain`, `replay`, with modifiers like `fast-memo` — for every command and embedder. Same as `--policy <name>`; `PETAL_OPT=off` and `--no-opt` still mean `baseline`. |
| `cd core && cargo build --profile profiling` | Release codegen with symbols kept, at `core/target/profiling/petal`, for a sampling profiler (`sample <pid>`, `perf`, `samply`). |

## Other tooling

| Command | Description |
|---------|-------------|
| `npm run scan-secrets` | Scan the full git history for leaked credentials with gitleaks (mirrors the CI "Secret scan" job). Run before a push or public release. |
| `./tools/oracle-external.ts` | Run the frame-gate/memo differential oracle over worlds-fair's UI, which lives outside this repo and needs a generated bundle and fixture models. Garden's half runs in CI as part of the cargo corpus. See [declarative-effect-refactoring.md](../tasks/declarative-effect-refactoring.md). |
| `./tools/native-effect-audit.ts` | Static approximation of `--effect-audit` over the source of every registration site, including natives no corpus app calls: which bodies report what they do through `PetalCxt`, and which reach outside their arguments (the filesystem, a thread-local, the clock) without reporting anything — the natives whose rows want a second look. Exits non-zero if any does. `--all` lists every native, `--json` for machine-readable output. See [declarative-effect-refactoring.md](../tasks/declarative-effect-refactoring.md). |
| `tools/run-example.ts <slug>` | Launch an example panel app (`examples/<category>/<slug>/`) in Garden; extra arguments go to Garden, `--list` shows the examples. Refuses a Garden binary that is behind the checkout. See [examples/AUTHORING.md](../../examples/AUTHORING.md). |
| `tools/panel-test.sh` | Shell helpers for the headless panel test loop: `panel_start <slug>`, `click`/`move`/`drag`/`scroll` (pane-local coordinates), `key`, `tick`, `obs`, `shot`, `panel_reset`, `panel_stop`. `source` it, or run `tools/panel-test.sh <command> …` one command at a time. Needs `curl` and `jq`. See [examples/AUTHORING.md](../../examples/AUTHORING.md#running-it). |
| `npm run stdlib:json` | Extract the standard library into JSON (`tools/extract-stdlib.ts`). |
| `npm run tsc` | Type-check the vitest suite and the `tools/` scripts (`tsc --noEmit`). `cd tools && npm run tsc` also covers the MCP servers (run `npm install` there once; they need it to run too). |

## MCP introspection

The MCP server exposes tools that run snippets and inspect their tokens, AST,
IR, and bytecode. See [mcp-server.md](mcp-server.md).
