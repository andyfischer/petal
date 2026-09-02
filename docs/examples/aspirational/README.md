# Aspirational Examples

These `.ptl` files are **design sketches**, not runnable programs. They show
how Petal's goal-level features (differentiation via `grad()`, program
projection, provenance tracing, live editing, self-metaprogramming) should
feel to use once implemented. Each references APIs the current runtime does
not expose, so running one fails.

See [`dev/goals.md`](../../dev/goals.md) for the motivation behind each
feature. When a feature lands, rewrite its sketch against the real API and
move it into [`/examples/`](../../../examples/) so it runs under the test
suite.

## Current sketches

| File | Targets |
|------|---------|
| `differentiation.ptl` | High-level `grad()` / `gradients()` API for automatic differentiation. (Forward-mode dual numbers already work; see `/examples/console/differentiation.ptl`.) |
| `gradient_descent.ptl` | Optimizer sugar on top of `grad()`. |
| `live_editing.ptl` | Hot-reload with state reconciliation across edits. |
| `metaprogramming.ptl` | Programs as first-class values: `Program.current()`, `.terms()`, `.functions()`. |
| `projection.ptl` | Program slicing: `program.slice(target)`, forward/backward slices, dynamic slices. (The `petal show-slice`, `show-provenance` and `show-dependents` commands cover some of this from the command line.) |
| `provenance.ptl` | Data provenance tracing through the dataflow graph. |
