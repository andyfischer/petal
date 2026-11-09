# test/

The vitest integration suite and the test data that lives outside the Rust
source tree. The full testing picture — the vitest suite, Rust unit tests, and
the refactor verifier — is in [docs/dev/testing.md](../docs/dev/testing.md).

| Path | What it is | Used by |
|------|------------|---------|
| `test/vitest/` | The vitest integration suite (`*.test.ts`) and its `fixtures/` | `npm test` |
| `test/ptl/` | Tests written in Petal: `test "name" do … end` blocks | `petal test test/ptl`; `cd core && cargo test --test ptl_tests` |
| `test/<case>/` | Script regression cases: a `main.ptl` plus an `expects` file (below) | `cd core && cargo test --test script_cases` |
| `test/example-golden/` | Frozen stdout for every `examples/console/*.ptl` | `tools/test-examples.ts`; re-baseline with `tools/gen-example-golden.ts` |
| `test/ui-golden/` | Hashes of each UI app's 60-frame headless trace | `tools/verify.ts` (`--update-golden` to re-baseline) |
| `test/verify-plans/` | Plans for `tools/verify.ts` | see "Verifying a refactor" in testing.md |
| `test/benchmarks/` | Programs timed at both optimization levels | `tools/bench-opts.ts` |
| `test/benchmarks/harness/` | Rust measurement harnesses, declared as `[[example]]`s of the `core/` crate | `cd core && cargo run --release --example <name>`; see [scripts.md](../docs/dev/scripts.md) |
| `test/ui-corpus/` | A cargo package of tests: core features that need a UI host (frame gate, memo, static-value cache, effect audit, direct manipulation) over the panel corpus | `make test-ui-corpus`; `cd test/ui-corpus && cargo test`; see below |

## Script regression cases

Each case is one directory:

```
test/<case>/
  main.ptl    # a runnable Petal program
  expects     # expected output + performance ceilings
```

The harness, [`core/tests/script_cases.rs`](../core/tests/script_cases.rs),
runs as part of `cargo test`. It picks up every `test/*/` directory that
contains a `main.ptl`, runs the program through the embedded interpreter, and
checks the result against `expects`.

### `expects` format

Plain text, one directive per line. Lines starting with `#` are comments;
blank lines are ignored.

```text
out: <line>                      # expected console output, one per print(), in order
max dup.<kind>.<metric>: <N>     # copy ceiling: the run must not exceed N
max alloc.<kind>.count: <N>      # allocation ceiling: the run must not exceed N
```

- `out:` lines are matched exactly and in order against the program's output,
  one entry per `print()` call. One optional space after the colon is dropped,
  so `out: 5` expects the line `5`.
- `max dup.<kind>.<metric>:` caps how much value copying the run does
  (copy-on-write plus fork copies).
  `<kind>` is `list`, `map`, `f64array`, `fork` or `total`;
  `<metric>` is `count` or `bytes`.
- `max alloc.<kind>.count:` caps how many new heap objects the run creates,
  including short-lived temporaries.
  `<kind>` is `string`, `list`, `f64array`, `map`, `element` or `total`.

### Why the ceilings exist

Petal values are immutable, so every "mutation" and every speculative fork
copies the underlying heap payload (see [`core/src/stats.rs`](../core/src/stats.rs)).
The ceilings pin how much copying a known scenario does today. When the runtime
learns to reuse a payload instead of copying it, the numbers fall and the
ceilings get tightened to lock the win in. A change that copies more than the
ceiling fails the test.

Ceilings are only enforced when duplication stats are on by default: debug builds
(which `cargo test` uses) and the `dup-stats` cargo feature. The `out:` checks
always run.

### Adding a case

1. `mkdir test/my-case` and write `test/my-case/main.ptl`.
2. Capture the current numbers: `cd cli && cargo run -- run --dup-stats ../test/my-case/main.ptl`.
3. Write `test/my-case/expects` with the `out:` lines and `max` ceilings.
4. `cd core && cargo test --test script_cases`.

## test/ui-corpus

The tests of core features that can only be exercised by running a UI app:
the frame gate (`petal::run_deps`), memoized scopes (`petal::memo`), the
static-value cache (`petal::static_cache` and its classifier, audit and
reload), the native effect audit (`petal::effect_audit`) and direct
manipulation (`petal::dm`). They drive apps through `petal_ui_headless::harness::Headless`,
so they depend on `petal-ui`, but they test `core/`, not `petal-ui`; the tests
of `petal-ui` itself stay in `core-libs/petal-ui/tests/`.

It is its own cargo package (not in a workspace), depending on `core/` and
`core-libs/petal-ui/` by path, with the dependencies optimized in dev builds
as in petal-ui so the suite stays under a minute or so.

```sh
make test-ui-corpus                      # or: cd test/ui-corpus && cargo test
cd test/ui-corpus && cargo test --release --test dm_study ladder -- --nocapture
```

| File | What it holds |
|------|---------------|
| `tests/common/mod.rs` | The corpus: every `examples/{productivity,dashboards,games,ui}/*/app.ptl` plus Garden's panels and GPP apps, and the parallel driver over it |
| `tests/gating.rs` | The frame gate's semantics, and gate on vs off over the corpus |
| `tests/memo.rs` | Memoized scopes' semantics, and memo on vs off over the corpus |
| `tests/effect_audit.rs` | Every native's observed effects against its declared row, over the corpus |
| `tests/static_classify.rs`, `static_audit.rs` | The static-value classifier against what the corpus computes |
| `tests/static_cache.rs`, `static_in_functions.rs`, `static_reload.rs` | The static-value cache on vs off, inside function bodies, and across hot reloads |
| `tests/dm_spec.rs`, `dm_params.rs` | Direct manipulation v2 acceptance items that need a UI host |
| `tests/dm_study.rs` | The DM v2 examples ladder and complexity study (`--release`; see [direct-manipulation-v2-study.md](../docs/dev/direct-manipulation-v2-study.md) §7) |
