# docs/examples

This directory is **not** the main examples folder. Runnable, tested examples
live in [`/examples/`](../../examples/) at the repo root; the console ones are
checked by `ts/test/test-samples.test.ts`.

## [`aspirational/`](aspirational/)

Design sketches for language features described in
[`dev/goals.md`](../dev/goals.md) but not yet implemented. They reference APIs
the runtime does not expose (`Program.current()`, `program.slice()`,
`grad(f)`, and so on), so they fail when run.

Treat them as specs, not programs. When a feature lands, move its sketch into
`/examples/` and adjust it to the real API.
