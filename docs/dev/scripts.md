# Developer Scripts & Commands

A reference for the commands used to build, run, test, and benchmark Petal
during development.

## Make targets

The [`Makefile`](../../Makefile) wraps the most common tasks. Run `make` (or
`make help`) to list them.

| Command | Description |
|---------|-------------|
| `make build` | Build the Petal compiler (debug) — `cd rust && cargo build`. Binary lands at `rust/target/debug/petal`. |
| `make test` | Build, then run the full vitest suite (which also runs every `examples/*.ptl`). |
| `make test-examples` | Print each example program's output for manual inspection. |
| `make clean` | Remove Rust build artifacts (`cargo clean`). |

## Building & running

| Command | Description |
|---------|-------------|
| `cd rust && cargo build` | Build the debug binary. |
| `cd rust && cargo build --release` | Build the optimized binary at `rust/target/release/petal` (used by the benchmarks). |
| `cd rust && cargo test` | Run the Rust unit tests. |
| `./ts/bin/run-petal.ts run <file.ptl>` | Helper script to run Petal locally: rebuilds the binary if needed, then forwards all args to `petal`. |
| `./ts/bin/run-petal.ts run -e '<expr>'` | Run a one-liner. |
| `rust/target/debug/petal run <file.ptl>` | Run the binary directly (no auto-rebuild). |

## Testing

See [testing.md](testing.md) for the full testing guide.

| Command | Description |
|---------|-------------|
| `cd ts && npx vitest` | Run the integration test suite in watch mode. |
| `cd ts && npx vitest run` | Run the integration suite once. |
| `npm test` | Run the vitest suite from the repo root (delegates to `ts`). |
| `npm run test:watch` | Vitest in watch mode from the repo root. |
| `./ts/bin/test-examples.ts` | Run every `examples/*.ptl` on the bytecode VM at both optimization levels (opts on / `--no-opt`), require byte-identical output between them, and require both to match the frozen `test/example-golden/` corpus. |
| `./ts/bin/test-examples.ts --full` | Same, but print full output rather than an 8-line preview. |
| `./ts/bin/gen-example-golden.ts` | Re-baseline the `test/example-golden/` corpus from the current VM output. Run deliberately — a golden update asserts the intended behavior changed. |

## Benchmarking

| Command | Description |
|---------|-------------|
| `./ts/bin/bench-opts.ts` | Time every [`test/benchmarks/`](../../test/benchmarks/)`*.ptl` on the bytecode VM at both optimization levels (release build) and report per-file medians plus the no-opt/opts speedup. |
| `./ts/bin/bench-opts.ts --runs=10` | Use more repetitions per file (default 5). |

## Tooling

| Command | Description |
|---------|-------------|
| `npm run scan-secrets` | Scan the full git history for leaked credentials with gitleaks (mirrors the CI "Secret scan" job). Run before a push or public release. |
| `cd ts && npm run stdlib:json` | Extract the standard library into JSON (`tools/extract-stdlib.ts`). |
| `cd ts && npm run tsc` | Type-check the TypeScript tooling (`tsc --noEmit`). |

## MCP introspection

The MCP server exposes tools to inspect tokens, AST, IR, and bytecode for a
snippet. See [mcp-server.md](mcp-server.md) for details.
