# Testing

Petal's tests are split between Rust unit tests (`cd rust && cargo test`) and
a vitest integration suite in `ts/test/` that shells out to the compiled
`petal` binary and asserts on its output. This document covers the vitest
suite and the tooling built around it.

## Integration tests (vitest)

Run from the `ts/` directory (or `npm test` at the repo root):

```bash
cd ts

npx vitest run                     # run everything once
npx vitest                         # watch mode
npx vitest test/ir-basics.test.ts  # one file
npx vitest -t "emits Add"          # tests matching a name
```

The binary is built once per test session by `ts/test/global-setup.ts`
(wired through `globalSetup` in `vitest.config.ts`).

Each file in `ts/test/*.test.ts` covers one language area or one command:
`ir-*.test.ts` check the shape of the compiled IR (constants, control flow,
functions, state, ...), and the rest exercise builtins, syntax, error
formatting, modules, the dataflow query commands, and so on. Browse the
directory for the current set.

### Helpers (`ts/test/helpers.ts`)

The helpers export `PETAL`, the path of the built binary, and
`petalCapture(args, input?)`, the argv-based spawn every other helper builds
on. The ones most tests use:

- `runPetal(code)` / `runPetalError(code)` — run a snippet, return stdout, or
  stderr for an expected failure.
- `runPetalFile(path)` — run a file; throws if it fails.
- `checkJson(code)` / `checkStrict(code)` — `petal check --json` / `--strict`.
- `showIrJson(code)`, `showAstJson(code)`, `showTokensJson(code)`,
  `showBytecodeJson(code)` — parsed `--json` dumps.
- `showProvenanceJson`, `showDependentsJson`, `showSliceJson`, `explainJson` —
  the dataflow query commands.
- `runIr(irJson)` / `runIrFile(path)` — run JSON IR through `run --ir`.
- `userTerms(ir)` — the IR's terms minus builtin phantoms.
- `termByName(ir, name)`, `termById(ir, id)`, `termsByOp(ir, op)` — term
  lookup.

### Example programs

`ts/test/test-samples.test.ts` runs every `examples/console/*.ptl` file and
asserts it exits without error. It is part of the normal vitest run:

```bash
cd ts
npx vitest test/test-samples.test.ts
```

For a stronger check, `./ts/bin/test-examples.ts` runs each example with the
optimizer on and off, requires identical output between the two, and requires
both to match the golden corpus in `test/example-golden/`. Add `--full` to see
full output instead of an 8-line preview. `./ts/bin/gen-example-golden.ts`
re-baselines the corpus; run it deliberately, since a golden update asserts
that behavior was meant to change.

## Deterministic runs

Two knobs make a run byte-reproducible, which is what before/after diffing of
a refactor needs (see [refactor-verification.md](refactor-verification.md)):

- **`petal run --seed <n>`**, or `PETAL_SEED=<n>` for any command and any
  embedder, fixes the `random` / `random_int` / `choose` stream. Without one,
  the seed comes from the wall clock and even an old-vs-old diff fails.
  Embedders call `Env::set_seed(n)` once before the first run; the PRNG then
  advances naturally across frames and forks.
- **`petal run --error-format bare`** (also on `check`) prints just the error
  message, with no `[line N, column M]`, no echoed source line, and no caret,
  so a re-indenting refactor does not show up as an output diff. Type-checker
  warnings lose their position line and snippet too. It cannot reach a line
  number quoted inside a message's prose (`written on line 775`); the verifier
  treats a warnings-only difference as a note rather than a failure.

Covered by `ts/test/seed.test.ts`, `ts/test/error-format.test.ts`, and the
`env::tests::seed_tests` / `cli::tests` Rust unit tests.

## Verifying a refactor

A large mechanical change — `petal lint --fix` over the tree, an optimizer
pass, a prelude rewrite — wants proof that it preserved behavior.
`ts/bin/verify.ts` provides it: it runs a *plan* of cheapest-first checks
(`compiles`, `ir-equal`, `control-run`, `run-diff`, `golden`) over a corpus of
`.ptl` files, on two sides that differ along exactly one axis.

```sh
# source A/B — the same binaries over two source trees
./ts/bin/verify.ts --plan lint-fix --before ab3304a~1 --after .

# binary A/B — the same sources under two `petal` builds
./ts/bin/verify.ts --plan compiler --before-bin old/petal --after-bin rust/target/debug/petal
```

`--before` takes a git ref (materialized with `git archive` under the
artifacts dir) or a directory. Each file gets one row: its `kind` (`console`,
`ui`, `module`, `unsupported`, classified by evidence rather than a hand
list) and a verdict (`identical-ir`, `identical-trace`, `nondeterministic`,
`changed`, `compile-error`, `driver-error`, `unsupported`, `module`). The
process exits non-zero if anything is `changed`, `compile-error`, or
`driver-error`.

`driver-error` means the driver binary never launched, as opposed to running
and failing. It is reported separately because both sides of a spawn failure
emit the same empty output, which compares equal; without it a missing
`petal-ui-run` would report every UI app as `identical-trace` and exit 0. A UI
corpus checks for the driver up front (`cd petal-ui && cargo build --bin
petal-ui-run`).

A failure leaves a replay bundle under
`.temp/verify-runs/<plan>-<timestamp>/<file>/`: the resolved `plan.json`, the
`scenario.json` and `seed` used, both traces, and a `repro.sh` that
reproduces the diff from any working directory. Useful flags: `--only
<substring|glob>`, `--jobs N`, `--frames N`, `--update-golden`.

`test/ui-golden/index.json` is the UI analogue of `test/example-golden/`: the
sha256 of each UI app's 60-frame `monkey:1`, seed-1 trace. Only the hashes are
checked in (the traces total ~72 MB), so a golden mismatch is a signal to
re-run the app and diff locally. Re-baseline it deliberately with
`--update-golden`.

Plans live in `test/verify-plans/`. A plan's `include` list names module search
directories (relative to each side's root) handed to the UI driver as `-I`, so
corpus apps that import a shared Petal library — `petal-libs` — still
compile. The design is in
[refactor-verification.md](refactor-verification.md).

## IR equivalence

`petal::ir_equiv::ir_equivalent(a, b)` answers "are these two compiled
programs the same program?", ignoring everything positional: spans, file ids,
the source map, comments, whitespace, and the numeric ids of terms, blocks,
and constants (constants compare by value). It is exposed as `petal ir-equal
<a> <b>` (exit 0 equal / 1 different / 2 a side failed to compile) and used
by `petal lint --fix --verify`, which refuses to write a rewrite it cannot
prove equivalent (exit 3).

Its unit tests live in `rust/src/ir_equiv.rs`; the CLI and lint contracts are
covered by `ts/test/ir-equal.test.ts`. The load-bearing test is
`lint::tests::reindent_is_ir_equal_over_repo_corpus`: every `.ptl` in the
repo that compiles standalone is mangled (three extra spaces of indentation on
every line), re-indented, and asserted IR-equal to the original. That is the
property that makes the formatting pass safe to run over the whole corpus.
