# Aspirational Examples

These `.ptl` files are **design sketches**, not runnable programs. They describe
how Petal's goal-level features — differentiation via `grad()`, program
projection, provenance tracing, live editing, self-metaprogramming — should
feel to use once implemented. Each file references APIs that the current
compiler does not expose, so attempting to run them will fail.

See [`../../goals.md`](../../dev/goals.md) for the motivation behind
each pillar. When one of these features actually lands, the corresponding
sketch should be rewritten against the real API and moved into
[`/examples/`](../../../examples/) so it runs under `test-samples.test.ts`.

## Current sketches

| File | Targets |
|------|---------|
| `differentiation.ptl` | High-level `grad()` / `gradients()` API for automatic differentiation (forward-mode dual numbers already work — see `/examples/differentiation.ptl`). |
| `gradient_descent.ptl` | Optimizer sugar on top of `grad()`. |
| `live_editing.ptl` | Hot-reload with state reconciliation across edits. |
| `metaprogramming.ptl` | Programs as first-class values: `Program.current()`, `.terms()`, `.functions()`. |
| `projection.ptl` | Program slicing: `program.slice(target)`, forward/backward slices, dynamic slices. |
| `provenance.ptl` | Data provenance tracing through the dataflow graph. |
