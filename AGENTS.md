See these important files:

README.md - project outline
docs/dev/scripts.md - developer tools and scripts
docs/dev/testing.md - how testing is handled
docs/module-system.md - imports, name resolution, module identity, host interface packages
docs/packages.md - petal.toml, dependencies, petal.lock, the package commands, and migrating with `petal fix-imports`
docs/dev/performance.md - profiling tools, what the optimizer does, where the headroom is
docs/dev/mcp-server.md - using the local MCP server to do testing and investigation
docs/dev/formal-verification.md - Kani proofs and exhaustive small-scope checks of the core semantics
docs/dev/migrations.md - when a syntax change deprecates a form, the rewrite goes in `petal migrate-old-code` (core/src/migrate/), not in lint/fmt

## Commits

Save a git commit after each chunk of work (a finished task, a fix, or a
self-contained step) without waiting to be asked. Stage only the specific files you
changed. Use conventional-commits style.

## Running the Rust tests

`core/tests/` builds as one integration-test binary, `all`: each file under
`core/tests/all/` is a module listed in `core/tests/all/main.rs`. Run one
former test file with its module path as the filter:

```bash
cd core
cargo test --test all hot_reload::                 # one file's tests
cargo test --test all hot_reload::config_scalar   # tests matching a name
cargo test --test all -- --ignored hot_reload::whole_corpus   # an ignored one
```

Add a new file under `core/tests/all/` and a `mod` line in `main.rs`. Only a
test that changes process-wide state (an environment variable, a registered
package manager) gets its own file directly in `core/tests/`
(`static_audit_env.rs`), run as `cargo test --test static_audit_env`.

`make test-core` runs both feature configs with
[cargo-nextest](https://nexte.st) when it is installed
(`cargo install cargo-nextest --locked`), plus `cargo test --doc` for the
doctests nextest skips; without it, plain `cargo test`. With nextest, one
file's tests are `cargo nextest run --test all hot_reload::` (or
`-E 'test(/^hot_reload::/)'`).

### CPU limits

Builds and tests are capped so they do not make a 10-core machine in use
unresponsive:

| Limit | Where | Override for one run |
| --- | --- | --- |
| 6 parallel rustc jobs | `[build] jobs` in `.cargo/config.toml` (every crate in the repo) | `CARGO_BUILD_JOBS=10` or `cargo build -j 10` |
| 6 test threads per `cargo test` binary | `[env] RUST_TEST_THREADS` in `.cargo/config.toml` | `RUST_TEST_THREADS=10` or `-- --test-threads 10` |
| 6 nextest test processes | `test-threads` in `core/.config/nextest.toml` | `NEXTEST_TEST_THREADS=10` or `cargo nextest run -j 10` |
| `nice -n 10` | the Rust test targets in the `Makefile` | `make NICE= test-core` |

`make NEXTEST= test-core` runs `cargo test` even when nextest is installed.
