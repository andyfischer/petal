# Testing

Petal's tests are split between Rust unit tests (`cd core && cargo test`) and
a vitest integration suite in `test/vitest/` that shells out to the compiled
`petal` binary and asserts on its output. This document covers the vitest
suite and the tooling built around it.

## Integration tests (vitest)

Run from the repo root (`npm test` runs everything once):

```bash
npx vitest run                     # run everything once
npx vitest                         # watch mode
npx vitest test/vitest/ir-basics.test.ts  # one file
npx vitest -t "emits Add"          # tests matching a name
```

The binary is built once per test session by `test/vitest/global-setup.ts`
(wired through `globalSetup` in `vitest.config.ts`).

Each file in `test/vitest/*.test.ts` covers one language area or one command:
`ir-*.test.ts` check the shape of the compiled IR (constants, control flow,
functions, state, ...), and the rest exercise builtins, syntax, error
formatting, modules, the dataflow query commands, and so on. Browse the
directory for the current set.

### Helpers (`test/vitest/helpers.ts`)

The helpers export `PETAL`, the path of the built binary, and
`petalCapture(args, input?)`, the argv-based spawn every other helper builds
on. The ones most tests use:

- `runPetal(code)` / `runPetalError(code)` — run a snippet, return stdout, or
  stderr for an expected failure.
- `runPetalFile(path)` — run a file; throws if it fails.
- `checkJson(code)` / `checkStrict(code)` — `petal check --json` / `--strict`.
- `showIrJson(code)`, `showAstJson(code)`, `showTokensJson(code)`,
  `showBytecodeJson(code)` — parsed `--json` dumps.
- `graphJson(code, terms, direction?, slice?)`, and the shorthands over it
  `showProvenanceJson`, `showDependentsJson`, `showSliceJson`, plus
  `explainJson` — the dataflow query commands.
- `runIr(irJson)` / `runIrFile(path)` — run JSON IR through `run --ir`.
- `userTerms(ir)` — the IR's terms minus builtin phantoms.
- `termByName(ir, name)`, `termById(ir, id)`, `termsByOp(ir, op)` — term
  lookup.

### Example programs

`test/vitest/test-samples.test.ts` runs every `examples/console/*.ptl` file and
asserts it exits without error. It is part of the normal vitest run:

```bash
cd ts
npx vitest test/test-samples.test.ts
```

For a stronger check, `./tools/test-examples.ts` runs each example with the
optimizer on and off, requires identical output between the two, and requires
both to match the golden corpus in `test/example-golden/`. Add `--full` to see
full output instead of an 8-line preview. `./tools/gen-example-golden.ts`
re-baselines the corpus; run it deliberately, since a golden update asserts
that behavior was meant to change.

## Proofs and exhaustive checks

The core numeric semantics (checked arithmetic, `==` and `<` across int and
float, the `sort` order, state-key hashing, index and slice resolution) are
proven for every input with the Kani model checker, and `core/tests/small_scope.rs`
checks every small program end to end. See
[formal-verification.md](formal-verification.md).

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

Covered by `test/vitest/seed.test.ts`, `test/vitest/error-format.test.ts`, and the
`env::tests::seed_tests` / `cli::tests` Rust unit tests.

## Verifying a refactor

A large mechanical change — `petal lint --fix` over the tree, an optimizer
pass, a prelude rewrite — wants proof that it preserved behavior.
`tools/verify.ts` provides it: it runs a *plan* of cheapest-first checks
(`compiles`, `ir-equal`, `control-run`, `run-diff`, `golden`) over a corpus of
`.ptl` files, on two sides that differ along exactly one axis.

```sh
# source A/B — the same binaries over two source trees
./tools/verify.ts --plan lint-fix --before ab3304a~1 --after .

# binary A/B — the same sources under two `petal` builds
./tools/verify.ts --plan compiler --before-bin old/petal --after-bin core/target/debug/petal
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
corpus checks for the driver up front (`cd core-libs/petal-ui && cargo build --bin
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

A switch of the UI driver can be the axis too: give one side a wrapper
script. This runs every UI app in the plan with the static-value cache off
on one side and on on the other:

```sh
printf '#!/bin/sh\nexec %s/core-libs/petal-ui/target/debug/petal-ui-run --policy fast-static "$@"\n' "$PWD" > /tmp/ui-run-nocache.sh
chmod +x /tmp/ui-run-nocache.sh
./tools/verify.ts --plan compiler \
    --before-bin core/target/debug/petal --after-bin core/target/debug/petal \
    --before-ui-bin /tmp/ui-run-nocache.sh \
    --after-ui-bin core-libs/petal-ui/target/debug/petal-ui-run
```

The `golden` step needs no such help: `petal-ui-run` runs the default policy,
cache included, and the hashes in `test/ui-golden/` were taken before the
cache existed.

The cache has a fuzzer of its own, `core/src/backend/bytecode/static_fuzz.rs`
(`cargo test --lib static_fuzz`; `PETAL_FUZZ_ITERS=<n>` for a soak, 100 seeds
by default). Its programs are static tables (`let`, `const`, `config let`,
constructor-built, enum data) with dynamic declarations between them, every
route of writing to a value that may be kept, top-level loops, and three
reloads (literals patched in place, a statement added, back to the first
text). Each is driven with the cache on and off under `replay`,
`replay-memo`, `fast` and `baseline`, and the transcripts must be equal. A
second test holds the generator to its job: programs that compile, keep
values and are reloaded both ways. `static_cache_fuzz` in `fuzz.rs` runs
the general generator's programs three times each, cache on against off.

The values the cache keeps inside function bodies
(`core/src/static_cache/in_fn.rs`) have a third side to every differential:
the cache **checking itself**. `Env::set_static_check(true)`
(`PETAL_STATIC_CHECK=1` sets it for every `Env`, so a whole suite can be
run under it) makes the skip in front of such a value fall through, so the
code that builds it runs on every call, and makes the store compare what
it built with what the slot holds; a difference is a value that was not
the same on every call, and `StaticValues::mismatches` names its function
and line (set through the environment variable, each one is also printed
on stderr as `static check failure: …`, since no test of a suite run that
way reads the list). So each case is driven with the cache on, off, and on and
checking, the three transcripts must be equal, and the check must find
nothing. That is `same_on_and_off` and `same_through_reloads` in
`core/tests/static_in_functions.rs`,
`function_constants_change_nothing_across_the_corpus` in
`core-libs/petal-ui/tests/static_in_functions.rs` (the panel corpus under
four policies), and a fuzzer, `core/src/backend/bytecode/static_fn_fuzz.rs`
(`cargo test --lib static_fn_fuzz`, `PETAL_FUZZ_ITERS=<n>`): function bodies
that mix constants with what looks like one and is a value of the call (a
parameter, a captured value that changes every run, a loop or pattern
variable, a name a branch rebinds), every route of writing to a value
that may be kept, inside the function and by its caller, recursion, early
returns, closures made per call, and three reloads. To see that these
would notice, make `FnPlan::of` call a parameter invariant and take
parameters out of `dirty_entry` in `in_fn::stretches`: half the cases and
the fuzzer's first seed fail, most of them by the check and not by the
output. `-fnstatic` on a policy (`replay-fnstatic`) is the cache without
these values, for telling which half a difference comes from.

That fuzzer's first find was not in the cache: with the optimizer on and
the cache **off**, a name a branch rebound to another value in front of a
loop that wrote to it wrote into that other value in place. The cache was
the side that was right, because a kept value is immutable. When a cache
differential fails, suspect both sides
(`a_branch_that_rebinds_the_accumulator_to_another_value_is_not_in_place`
in `core/tests/copy_elision.rs`).

Lists that begin as `Enum.all` have a fuzzer of their own,
`core/src/backend/bytecode/enum_all_fuzz.rs` (`cargo test --lib
enum_all_fuzz`, `PETAL_FUZZ_ITERS=<n>`). `Enum.all` is one list per stack
that no reader may write through, except that a read which starts a
rebinding variable copies and is then rewritten in place
(`escape::Analysis::copies_enum_all`); the general fuzzer and the cell
fuzzer declare no enum and reach neither half. Its programs start lists
from `K.all`, from a function returning it and from containers holding it,
in `let`, `var`, `state`, parameters, record fields and nested lists,
rewrite them inside loops and branches, and keep snapshots across the
writes. Each runs three frames under the baseline, under each in-place
route alone, and under `fast` and `replay` with the cache on, off,
checking and observed; every frame must end by printing the declared
variants. A second test holds the generator to reaching the copying read
(96 of 100 programs) and each route. Its first find (seed 148) needed no
enum: a reference taken inside an `if`, in front of a loop that rewrote
the list in place, showed the rewritten list
(`inplace_does_not_fire_past_a_reference_taken_inside_the_guard` in
`bytecode/tests.rs`). The general fuzzer had passed at 3,000 seeds with
that bug in place.

A reload keeps the static values the edit cannot have changed
(`core/src/env/static_follow.rs`). The oracle for that is the reload that
rebuilds every one, `Env::set_static_follow(false)`, and a slot-by-slot
comparison of the two tables after each frame (a slot the oracle has not
filled yet says nothing). It is run by `core/tests/static_reload.rs` (the
rule's own cases), by every case and both sweeps of `core/tests/hot_reload.rs`
against `transfer_state`, by `core-libs/petal-ui/tests/static_reload.rs` over
the panel corpus, and, since `fast` and `replay` follow by default, by
`static_fuzz`'s three reloads against the cache off. To see that an oracle
would notice, make `begin_static_follow` call nothing dirty: ten
`hot_reload` cases, 18 of the 23 in `static_reload` and `static_fuzz` fail.

The cache stays on while a host observes, so every one of these
differentials runs a second time with observations on, and what the host
reads after each frame is part of what is compared: the map
`Env::get_observations_json` returns, and the buffer itself term by term.
That is `same_on_and_off` in `core/tests/static_cache.rs`, `follow_modules`
in `core/tests/static_reload.rs` (with a third stack that has no cache, so
following the diff is held to both oracles at once), both fuzzers, the two
corpus tests of `core-libs/petal-ui/tests/static_cache.rs` and
`static_reload.rs`, and `every_panel_observes_the_same_with_its_static_values_kept`
in `garden/garden-script/src/panel.rs`, which runs Garden's panels and
drawers under Garden's own host. `core/tests/static_observe.rs` holds the
cases that say what the mechanism is; each part of it was removed in turn
to see a test fail (the replay, the log's GC roots, the in-place rule, the
log handed on by a reload, the kept value and the kept enum record put
back after a segment executes again).

Printing cannot tell one table from an equal copy of it, and a followed
reload must leave the same objects shared as a rebuild of everything does.
`sharing_is_the_same_as_after_a_rebuild_of_everything` in
`core/tests/static_reload.rs` is the oracle for that: tables that hold one
another, an edit to one of them, and `freeze` through arbitrary names on
later frames, with `frozen` printed through every name (40 seeds by
default, `PETAL_FUZZ_ITERS=<n>` for more). To see that it would notice,
take the fourth condition out of `finish_static_follow` (the loop that
rebuilds a value beside a rebuilt one it read): it fails within 400 seeds,
and `a_table_two_values_are_built_from_stays_shared` at once. With
`static_cache::replaces` made to accept any identical value,
`an_equal_value_holding_another_object_is_not_swapped_for_the_old_one`
fails.

`core/tests/static_review3.rs` drives one list of steps (frames, reloads,
`set_config_value`, host calls of a function) through four worlds at once,
under five policies, each with and without a host observing: the cache on,
off, on and checking itself, and on with every reload rebuilding
everything. It holds the cases a review of the four pieces after S2 was
built from: a value of the call that reaches a function-body constant
through a closure made per call (the one rule the first tests could not
reach: make every capture count as invariant in `FnPlan::of` and
`a_value_of_the_call_reached_through_a_function_is_never_kept` fails), a
parameter or local named like a kept table, writes through state, cells,
loop variables and methods, a failed reload between two good ones, two
stacks and a fork of one program, a host that starts observing after the
values were built, and 24 ways of editing a table in place under a name
nothing reads again.

The timing test in `core/tests/hot_reload.rs`
(`a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile`) takes
the minimum of 12 rounds for the two edits that cost about a millisecond:
with three it failed in release whenever the ignored whole-corpus oracle
ran beside it (`--include-ignored`), one preemption being several times
the reload.

Plans live in `test/verify-plans/`. A plan's `exclude` list drops directories
from its corpus roots (the vitest fixtures under `test/vitest`). A plan's `include` list names module search
directories (relative to each side's root) handed as `-I` to every side of the
run — the UI driver, `petal check`, and `petal run` alike — so corpus apps that
import a shared Petal library (`core-runtime`, which ships the `bloom` package)
still compile. A step that dropped the `-I` would write such an app off as
`unsupported` and silently skip its golden hash. The design is in
[refactor-verification.md](refactor-verification.md).

## IR equivalence

`petal::ir_equiv::ir_equivalent(a, b)` answers "are these two compiled
programs the same program?", ignoring everything positional: spans, file ids,
the source map, comments, whitespace, and the numeric ids of terms, blocks,
and constants (constants compare by value). It is exposed as `petal ir-equal
<a> <b>` (exit 0 equal / 1 different / 2 a side failed to compile) and used
by `petal lint --fix --verify`, which refuses to write a rewrite it cannot
prove equivalent (exit 3).

Its unit tests live in `core/src/ir_equiv.rs`; the CLI and lint contracts are
covered by `test/vitest/ir-equal.test.ts`. The load-bearing tests are
`fmt::tests::fmt_is_ir_equal_and_idempotent_over_repo_corpus` (every `.ptl` in
the repo that compiles standalone formats to identical IR, and formatting is a
fixed point) and `fmt::tests::fmt_undoes_mangled_indentation_over_repo_corpus`
(every file with three extra spaces on every line formats back to the same
text). Those are the properties that make `petal fmt` safe to run over the
whole corpus.

### What the Rust suite costs

Four unit tests sweep every `.ptl` file in the repository (about 380) in a
debug build, and cost far more than the rest of the suite put together. They
are `#[ignore]`d, so `cargo test` does not run them; `make
expensive_lint_tests` does, and CI runs it on every push. Run it yourself
after changing `petal fmt`, a lint rule, the lexer or parser, or the CST:

| Test | CPU-seconds |
|---|---|
| `lint::tests::lint_preserves_compilation_over_repo_corpus` | about 500 |
| `fmt::tests::fmt_is_ir_equal_and_idempotent_over_repo_corpus` | about 150 |
| `fmt::tests::fmt_undoes_mangled_indentation_over_repo_corpus` | about 60 |
| `cst_project::tests::projected_ast_matches_parser_over_repo_corpus` | about 25 |

Each fans out over the corpus on half the machine's cores;
`PETAL_TEST_JOBS=<n>` sets the number. The other 1,200 unit tests take a few
seconds together.

```bash
make expensive_lint_tests                       # the four sweeps
PETAL_TEST_JOBS=2 make expensive_lint_tests     # the same, on two threads
cd core
cargo test --lib                                # every unit test but the sweeps
cargo test --lib typecheck::                    # one module's unit tests
cargo test --test check_run_agree               # one integration test file
cargo test --lib fmt_is_ir_equal -- --ignored   # one sweep
```

The vitest suite is light by comparison: about 30 seconds on two workers
(`npx vitest run --maxWorkers=2 --minWorkers=1`).

## The C/C++ bridge

`integrations/petal-c-bridge` (see [embedding-c.md](../embedding-c.md)) is
built against the in-repo `core/`, `core-libs/petal-ui/` and `core-libs/petal-query/` crates, so
it is the first thing to break when an `Env` or petal-ui API it uses changes
shape. `make test-c-bridge` configures it with CMake + Ninja under
`integrations/petal-c-bridge/build/` and runs `ctest`: the C++ suite
(`tests/bridge_tests.cpp`, filterable by name:
`integrations/petal-c-bridge/build/petal_bridge_tests reload`), a plain-C
smoke test that keeps the header valid C, the `hello` example, and the
crate's Rust unit tests. CI runs the same in the `c-bridge` job.
