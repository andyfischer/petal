# Formal verification

Petal's core semantics are checked in three layers, from strongest guarantee
over the narrowest code to weakest guarantee over the widest:

| Layer | What it covers | Guarantee | Where |
|---|---|---|---|
| **Kani proofs** | The pure numeric kernels every operator and builtin delegates to | Holds for **every** input (bit-precise, 64-bit ints and IEEE floats) | `rust/src/numeric.rs`, `rust/src/proofs/` |
| **Exhaustive value laws** | `==`, hashing and ordering over containers (lists, records, class instances, enums) | Holds for every pair and triple in a bounded universe of values | `value::tests` in `rust/src/value.rs` |
| **Small-scope exhaustive programs** | Lexer, parser, compiler, type checker, optimizer and VM end to end | Holds for every program in a bounded space (every token sequence up to length N; every expression up to depth D) | `rust/tests/small_scope.rs` |

Each bug fixed along the way also has an end-to-end regression test, a Petal
program and its expected output, in `rust/tests/core_semantics.rs`.

This sits on top of the existing randomized nets: the differential fuzzer
(`backend/bytecode/fuzz.rs`), which checks that the optimizer never changes a
program's result, and the golden example corpus.

## Why this shape

A proof is only as useful as the code it covers. The verified kernels are
therefore not models *of* the runtime but code *in* it: `backend::ops`,
`value::values_equal`, `value::hash_value`, `sort`, `slice`, `range`,
`random_int` and the index operators all call into `numeric.rs`, so a proof
about `numeric::num_cmp` is a proof about what `a < b` does in a running
program. Kernels are kept free of heap access, allocation and string
formatting — errors are small enums that the caller turns into messages —
which is what keeps them cheap for the model checker.

Where a property depends on the heap (records, lists), a model checker is the
wrong tool. Those laws are checked by exhaustive enumeration instead, relying
on the *small-scope hypothesis*: most bugs have a small counterexample.

## What is proven

`cargo kani` proves each of these for every input. Most take seconds to a
couple of minutes each with the CaDiCaL solver on an M-series Mac; the ones
that bit-blast a 64-bit division are far slower (`int_div_is_exact` about 13
minutes).

Division is the model checker's weak spot. `int_div_is_exact` is
restated without dividing in the spec and still takes about 13 minutes on
an idle machine (an hour or more under load);
`range_len` (a ceiling division) did not finish in an hour even with bounded
inputs, so it is checked by an exhaustive unit test in `numeric.rs` instead
(every small range against a naive count, and every combination of i64
extremes against the exact characterization).

| Harness | Property |
|---|---|
| `int_add_is_exact`, `int_sub_is_exact`, `int_mul_is_exact` | Checked `+ - *` return exactly the mathematical (i128) result when it fits in i64, and `Overflow` otherwise. Never panic. |
| `int_div_is_exact` | `/` returns the truncated quotient, characterized division-free: `a = q·b + r`, `\|r\| < \|b\|`, `r` zero or of `a`'s sign. Zero divisor → `DivisionByZero`; only `i64::MIN / -1` overflows. |
| `int_mod_errors_are_exact` | `%` fails exactly on a zero divisor and never overflows (`i64::MIN % -1` is 0). Its value is `wrapping_rem` by construction; asserting it would put a 64-bit division in the formula. |
| `int_neg_is_exact`, `int_abs_is_exact` | `-x` and `abs(x)` are exact; only `i64::MIN` is refused. |
| `cmp_int_float_is_exact` | Comparing an int with a float is exact: `i < f` iff `i < ⌈f⌉`, `i > f` iff `i > ⌊f⌋`, no rounding of `i` to 53 bits. |
| `num_cmp_is_antisymmetric` | `cmp(a, b)` is `cmp(b, a)` reversed; unordered iff NaN is involved. |
| `num_eq_is_transitive`, `num_lt_is_transitive`, `num_eq_lt_compose` | `==` and `<` on numbers are transitive and compose, across int and float. |
| `num_total_cmp_is_antisymmetric`, `num_total_cmp_is_transitive` | The `sort` order is a total order (the precondition of `slice::sort_by`, which panics otherwise). |
| `num_total_cmp_agrees_with_lt` | The `sort` order agrees with `<` wherever `<` is defined. |
| `num_key_matches_eq` | The hash key of a number agrees with `==` in both directions, so `state(key)` treats `==` keys as one key. |
| `resolve_index_is_exact`, `checked_index_is_exact` | `xs[i]` and `xs[i] = v` address slot `i` (or `len + i` for negative `i`) exactly when it exists, with no truncating `i64 → usize` cast on 32-bit (wasm) targets. |
| `clamp_slice_bound_is_exact` | `slice` bounds are the resolved index clamped into `0..=len`. |
| `range_nth_is_exact` | Every element of `range(start, end, step)` is computed without wraparound. (The length, `range_len`, is checked exhaustively; see above.) |
| `scale_unit_to_range_stays_in_range` | `random_int(lo, hi)` is always in `[lo, hi)`, for every generator output, including spans wider than `i64::MAX`. |

`proofs/spec_sanity.rs` holds `#[kani::should_panic]` harnesses that run the
same properties against the implementations these kernels replaced; Kani must
find a counterexample in each. They guard against a vacuous spec, one that
would pass any implementation.

## Bugs found and fixed

Each of these was reachable from an ordinary Petal program. They were found
while writing the specs (stating a property precisely surfaced the code that
broke it), by the model checker, and by a sweep that called every builtin with
edge-case arguments (`i64::MIN`, `i64::MAX`, NaN, infinity, `-0.0`, empty
containers) and watched for process aborts. "Guarded by" names the check that
fails if the bug comes back.

| Bug | Guarded by | Before | After |
|---|---|---|---|
| Record `==` always false, even `m == m` | Exhaustive value laws (reflexivity) | `{a: 1} == {a: 1}` → `false` | Structural, key-order-insensitive, class-aware |
| `==` not transitive across int/float | Kani `num_eq_is_transitive`; exhaustive value laws | `9007199254740993 == 9007199254740992.0` → `true` | Exact comparison |
| NaN ordering | Kani `num_cmp_is_antisymmetric` (unordered iff NaN) | `nan <= 1` and `nan >= 1` both `true` | All orderings with NaN are `false` |
| `sort` of a list with NaN aborts the process | Kani `num_total_cmp_is_transitive`; `small_scope` | Rust's `sort_by` panics: "comparison function does not correctly implement a total order" | NaNs sort last |
| `state(key)` with a record or enum key gets a fresh slot every call | Exhaustive value laws (hash consistency); Kani `num_key_matches_eq` | Key hashed by heap id; state reset every frame | Hashes content, consistent with `==` |
| `-x` for the smallest int panics | Kani `int_neg_is_exact`; `small_scope` | Debug: panic. Release: silent wrap | Runtime error |
| `abs(x)` for the smallest int panics | Kani `int_abs_is_exact` | Same | Runtime error |
| `i64::MIN % -1` reports overflow | Kani `int_mod_errors_are_exact` | "Integer overflow" | `0` |
| `random_int(lo, hi)` panics for wide ranges | Kani `scale_unit_to_range_stays_in_range` | `hi - lo` overflows | Proven in `[lo, hi)` |
| `range(n)` for huge `n` aborts the process | `range_len` exhaustive test; `core_semantics` (the length is exact, so the reservation can be refused) | Capacity-overflow abort, or an unbounded loop for `step > 1` | Runtime error |
| `f64_array(n)`, `pad_start`/`pad_end`/`format` widths abort on a huge size | Fallible reservation (no proof) | Allocation-failure abort | Runtime error |
| `match x when 2` does not match `2.0`, though `2.0 == 2` | Uses the proven `num_eq` | Literal patterns compared int to int, float to float | A numeric literal matches what it is `==` to |
| `xs[i]`, `slice`, and char-indexed string builtins truncate `i` on wasm32 | Kani `resolve_index_is_exact`, `clamp_slice_bound_is_exact`, `checked_index_is_exact` | `xs[2^32 + 1]` reads `xs[1]` in the browser | Checked conversion |

Semantic choices made along the way (documented in the language guide):
`2 == 2.0`, so the two share a `state` slot; a dual number compares by its
value, ignoring the derivative, against numbers *and* other duals (comparing
derivatives only between two duals made `==` non-transitive); functions compare
by identity.

## Running

Install Kani once (it brings its own toolchain and the CBMC model checker):

```bash
cargo install --locked kani-verifier
cargo kani setup
```

Then, from `rust/`:

```bash
cargo kani --solver cadical                                  # every harness (the division proofs dominate)
cargo kani --harness proofs::numeric::int_mod_errors_are_exact  # one harness
cargo kani --harness proofs::spec_sanity                     # the should_panic checks
```

`make prove` runs the whole set. The proofs are compiled only under
`cfg(kani)`, so ordinary builds and `cargo test` never see them.

The exhaustive checks are ordinary tests and run with `cargo test`. Their
bounds are kept small by default so the suite stays fast; raise them for a
soak run:

```bash
PETAL_SMALL_SCOPE_TOKENS=4 cargo test --release --test small_scope   # 6.3M programs, ~1 min
PETAL_SMALL_SCOPE_DEPTH=3  cargo test --release --test small_scope   # much larger
```

## Adding a proof

1. Pull the rule into `numeric.rs` (or another heap-free module) as a pure
   function over scalars, returning a small error enum rather than a
   formatted `String`.
2. Call it from the runtime so the proof covers what programs run. Do not keep
   a second copy of the logic.
3. State the *full* contract in `proofs/`, against an independent reference
   (usually i128 arithmetic), not only "does not panic".
4. If the property is expensive (three symbolic floats, 128-bit division),
   split it by case or restate it without division, and add
   `#[kani::solver(cadical)]`.
5. Add a `should_panic` harness in `spec_sanity.rs` if there is an older or
   plausible wrong implementation, to show the spec can tell them apart.

## Where to go next

Candidates, roughly in order of value per effort:

- **Optimizer passes as translation validation.** `copyprop`, `lastuse` and
  `escape` are checked by differential fuzzing today. A per-program validator
  that checks the optimized bytecode refines the unoptimized one, run over the
  whole example corpus, would turn "no divergence found" into "no divergence
  in these programs".
- **`transfer_state` (hot reload).** Its reconciliation by `StateKey` is a
  pure function of two key sets and is a good fit for exhaustive checking over
  small programs edited one token at a time.
- **Heap and GC invariants.** Generational ids, the free list, and the
  in-place mutation gate (`list_set_in_place` only when escape analysis proves
  uniqueness) are candidates for Kani with small, bounded heaps.
- **Lexer and parser.** Every program of up to 4 tokens is checked today; a
  grammar-directed generator would reach deeper nesting than raw token
  sequences do.
- **A Lean model of state keys.** The call-path keying rules
  (docs/dev/state-call-paths.md) are a small formal system of their own and
  would benefit from a mechanized statement of what "same slot" means across
  edits.
