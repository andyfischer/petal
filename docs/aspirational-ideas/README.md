# Aspirational Ideas

Directions Petal might go, written down before any of it is built. Nothing
here is a commitment, and nothing here runs. See the
[README](../../README.md#goals) for the goals behind them.

When an idea ships, document it under [`features/`](../features/) (or the
CLI and dev docs), move any runnable example into
[`examples/`](../../examples/), and delete or trim the idea here.

## Design notes

- [programs-as-documents.md](programs-as-documents.md): a script as an
  application's saved document. Covers the write path from GUI edits,
  resolving edits by outcome against programs of many shapes, a schema for
  editable documents, and saved state in a JSON file beside the script.
  Part 1 and the write path are built: the
  [`programs-as-documents`](../../core-libs/programs-as-documents/) crate,
  documented in [features/programs-as-documents.md](../features/programs-as-documents.md),
  and used by petal-images. The schema (part 2) and saved state are not.

## Sketches

- [programs-as-values.ptl](programs-as-values.ptl): a script holding a
  program as a value (`Program.current()`, `Program.parse`, `Program.build`)
  to inspect, slice, edit, transform and compose it. It calls an API the
  runtime does not have, so it does not compile.

  The analyses behind it mostly exist already, outside the script:

  | In the sketch | What exists today |
  |---|---|
  | `.terms()`, `.find()`, inputs and dependents | Function values only: `fn_info`, `fn_ast`, `code_hash` ([function-introspection.md](../features/function-introspection.md)). The whole program as IR JSON: `petal show-ir --json` ([ir-as-target.md](../dev/ir-as-target.md)). |
  | `.slice()` | `petal graph`, backward and forward, and `Program::slice` in Rust ([provenance.md](../features/provenance.md)). Not yet: dynamic slices, slice algebra, `extract()`. |
  | `.edit()` with state reconciled | Hot reload, driven by the host ([hot-reload.md](../features/hot-reload.md)); other ways to edit a program are in [program-modification.md](../features/program-modification.md). |
  | `Program.build`, `.transform`, `.pipe` | Nothing yet. |

## Shipped, sketches removed

These were sketched here and have since shipped in another form, so their
sketches were deleted (2026-11-14):

- **Differentiation and gradient descent**: the `autodiff` package (`deriv`,
  `jacobian`, `solve`), by finite differences rather than a reverse-mode
  `grad()`. See [differentiation.md](../features/differentiation.md).
- **Provenance tracing**: `petal explain` and `petal graph`. See
  [provenance.md](../features/provenance.md).
- **Live editing with state preserved**: [hot-reload.md](../features/hot-reload.md),
  plus [time-travel.md](../features/time-travel.md) and
  [speculate.md](../features/speculate.md).
