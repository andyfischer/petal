
DONE (2026-10-08) — merged to main at 4c110ff

ISSUE 1 — mutable state across functions
- `set xs[i] = ...` / `set r.f = ...` through a `var` / `state var` cell now
  mutates in place from any function (docs/var.md "Writing a container in
  place"). 20k helper writes: 1.64s -> 0.007s.
- physics-playground: `step` uses nested closures over local `var`s (shape (a));
  app.ptl's intents block is functions over `state var`. Output byte-identical.
- Pattern documented in language-guide.md and writing-petal-guide.md.
- `petal apply-change convert-to-var <file> --target <path>` (docs/CLI.md).
  Importers: `--from <file>` when given, else a scan under the nearest
  `petal.toml` directory.

ISSUE 2 — `petal bench <file> --fn <name>` (docs/CLI.md)
- Inclusive and self figures, opt vs no-opt delta, `--iters`, `--json`.
- Copy/allocation counters are a runtime switch, available in release builds.

ISSUE 3 — `-> nil` disables the implicit return
- docs/implicit-return-values.md; `petal suggest` kind `return-types`
  (loop tails only).

ISSUE 4 — `pub` replaces `export`
- `export` still parses, with a deprecation warning; `prefer-pub` lint fix and
  `petal fmt` rewrite it. All in-repo sources migrated.

FOLLOW-UPS

From ISSUE 1:
- A helper ending in `set xs[i] = v`, called from a `for` that is the last
  statement of its function, copies the list per write because the loop
  collects the results. Declaring the caller `-> nil` (or a trailing `nil`)
  avoids it. Name this shape in the guides; consider a `suggest`/advice rule.
- physics-playground Cradle scene is 16% slower under `--observe` after the
  refactor.
- `apply-change` compile gate is looser than "result compiles": a file that
  already failed may keep the compile errors it had (one branch in
  `compile_gate` if it should be strict).
- `apply-change` qualified-write refusal: does not notice a local shadowing the
  module alias, and does not follow a binding re-exported through a facade.
  The directory scan misses importers outside the scan root (use `--from`).
  No `--json` output.
- Compiler quirk: in a function, reading an outer `x`, then `let x = ...`, then
  assigning `x`, is rejected as "bound outside this function".
- physics-playground/app.ptl has no entry in test/ui-golden/index.json.

From ISSUE 2:
- `bench` accepts only `--host core`, so UI apps (physics-playground/app.ptl)
  cannot be benched directly; `step` was measured through a console driver.
- Unknown flags are taken as the file path (`petal bench x.ptl --iter 3` ->
  "Error reading file '3'"); shared `parse_source_args`, affects `run` too.
- Lowering the physics solver with the optimizer on takes ~48 ms vs 0.45 ms
  off, so a one-shot run is slower optimized.

From ISSUE 4:
- `petal run` prints the `export` deprecation warning too, and
  `check --strict` fails on it. Decide whether that is wanted.

Not verified after the final integration (each passed per-branch before it):
- vitest suite, golden corpus (`test-examples.ts`), downstream `petal check`
  parity over ~/.garden and ~/worlds-fair/ui/ptl. Full garden-app suite and
  the fantasy-nes tests (missing libjxl) were never run.

Pre-existing, seen along the way:
- Six stale golden hashes in bloom-using UI apps (test/ui-golden).
- `petal-ui` test `gating::a_quiet_corpus_mostly_idles` fails (18 of 38).
