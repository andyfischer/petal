# Script regression tests

Each subdirectory is one test case:

```
test/<case>/
  main.ptl    # a runnable Petal program
  expects     # expected output + performance ceilings
```

The harness lives in [`rust/tests/script_cases.rs`](../rust/tests/script_cases.rs)
and runs as part of `cargo test`. It runs every `main.ptl` through the embedded
interpreter, then checks the result against `expects`.

## `expects` format

Plain text, one directive per line. Lines starting with `#` are comments; blank
lines are ignored.

```text
out: <line>                    # expected console output, one per print(), in order
max dup.<kind>.<metric>: <N>   # the run must NOT exceed N
```

- `out:` lines are matched **exactly and in order** against the program's
  console output (one entry per `print()` call). One optional space after the
  colon is dropped, so `out: 5` expects the line `5`.
- `max dup.<kind>.<metric>:` sets a ceiling on a value-duplication metric.
  - `<kind>` ∈ `list`, `map`, `f64array`, `fork`, `total`
  - `<metric>` ∈ `count`, `bytes`

## What the metric ceilings are for

Petal values are immutable, so every "mutation" and every speculative fork
copies the underlying heap payload (see [`rust/src/stats.rs`](../rust/src/stats.rs)).
The ceilings pin how much copying a known scenario does **today**. As escape
analysis and structural sharing teach the runtime to reuse live payloads instead
of duplicating them, these numbers should fall — and we tighten the ceilings to
lock the win in. A change that copies *more* than the ceiling fails the test.

Metric ceilings are only enforced when duplication stats are compiled in — debug
builds (which `cargo test` uses) and the `dup-stats` feature. The `out:` checks
always run.

## Adding a case

1. `mkdir test/my-case` and write `test/my-case/main.ptl`.
2. Capture the current numbers: `cd rust && cargo run -- run --dup-stats ../test/my-case/main.ptl`.
3. Write `test/my-case/expects` with the `out:` lines and `max` ceilings.
4. `cd rust && cargo test --test script_cases`.
