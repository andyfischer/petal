# test/

Test data that lives outside the Rust and TypeScript source trees. The full
testing picture — the vitest suite in `ts/test`, Rust unit tests, and the
refactor verifier — is in [docs/dev/testing.md](../docs/dev/testing.md).

| Path | What it is | Used by |
|------|------------|---------|
| `test/<case>/` | Script regression cases: a `main.ptl` plus an `expects` file (below) | `cd rust && cargo test --test script_cases` |
| `test/example-golden/` | Frozen stdout for every `examples/console/*.ptl` | `ts/bin/test-examples.ts`; re-baseline with `ts/bin/gen-example-golden.ts` |
| `test/ui-golden/` | Hashes of each UI app's 60-frame headless trace | `ts/bin/verify.ts` (`--update-golden` to re-baseline) |
| `test/verify-plans/` | Plans for `ts/bin/verify.ts` | see "Verifying a refactor" in testing.md |
| `test/benchmarks/` | Programs timed at both optimization levels | `ts/bin/bench-opts.ts` |

## Script regression cases

Each case is one directory:

```
test/<case>/
  main.ptl    # a runnable Petal program
  expects     # expected output + performance ceilings
```

The harness, [`rust/tests/script_cases.rs`](../rust/tests/script_cases.rs),
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
copies the underlying heap payload (see [`rust/src/stats.rs`](../rust/src/stats.rs)).
The ceilings pin how much copying a known scenario does today. When the runtime
learns to reuse a payload instead of copying it, the numbers fall and the
ceilings get tightened to lock the win in. A change that copies more than the
ceiling fails the test.

Ceilings are only enforced when duplication stats are compiled in: debug builds
(which `cargo test` uses) and the `dup-stats` cargo feature. The `out:` checks
always run.

### Adding a case

1. `mkdir test/my-case` and write `test/my-case/main.ptl`.
2. Capture the current numbers: `cd rust && cargo run -- run --dup-stats ../test/my-case/main.ptl`.
3. Write `test/my-case/expects` with the `out:` lines and `max` ceilings.
4. `cd rust && cargo test --test script_cases`.
