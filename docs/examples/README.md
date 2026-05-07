# docs/examples

This directory is **not** the main examples folder. Runnable, tested examples
live in [`/examples/`](../../examples/) at the repo root and are exercised by
`ts/test/test-samples.test.ts`.

## [`aspirational/`](aspirational/)

Design sketches for language features that are documented in
[`PETAL_GOALS.md`](../PETAL_GOALS.md) but not yet implemented. These files
**do not compile against the current compiler** — they exist to show what
the eventual API is intended to look like, and they reference APIs
(`Program.current()`, `program.slice()`, `grad(f)`, `.backpropagate()`, etc.)
that the runtime does not yet expose.

Treat them as specs, not as programs. When a feature lands, move its sketch
into `/examples/` with whatever adjustments reality imposes.
