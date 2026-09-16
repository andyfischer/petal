# Faster hashing for interned string lookups

Status: **proposed**, 2026-09-15. Measured but not started.

## The observation

`Heap::intern_table` is a `HashMap<String, StringId>` on the default hasher
(SipHash). Every string a builtin produces goes through it: `intern_str`,
`alloc_string` and `intern_substring` all hash the content before they can
answer. Interning is what makes string `==` an id compare, so this lookup is
on the path of nearly all text work.

A scanner walking a string one character at a time is the worst case.
`examples/productivity/spreadsheet` does exactly that (`digit_val`,
`letter_val`, the formula tokenizer): `slice` was **71% of all builtin calls**
on the frame that commits a formula, and that frame costs ~50 ms. In a
symbolized profile of a 5M-iteration `slice(DIGITS, k, k+1)` loop
(`cargo build --profile profiling`, `sample`), the intern lookup and SipHash
together are a visible slice of samples under `native_slice`.

## The change

1. Hash the intern table with a fast, non-DoS-resistant hasher. `FastHasher`
   in `rust/src/memo.rs:74` already exists for exactly this reason (with
   `FastMap`); either reuse it or lift it somewhere both modules can share.
   The table's keys are program text, not attacker-chosen network input, so
   SipHash's guarantee is not worth its cost here.
2. Consider a direct-mapped cache for single-byte ASCII substrings: a
   `[Option<StringId>; 128]` consulted before the hash map. A one-character
   `slice` then costs an index, not a hash. This is the shape of the
   scanner pattern above, and the entries are trivially invalidated on GC
   because they are just ids into the same table.

Keep interning itself: id-equality is relied on by `==`, the memo's argument
comparison (`crate::memo`) and the frame gate's binding fingerprints.

## How to check it

Before/after on the commit frame:

```bash
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/productivity/spreadsheet/app.ptl 600 \
  --scenario ../examples/productivity/spreadsheet/bench/edit.json --no-gate
```

Watch `max` (the commit frame) and `total script ms`. A microbenchmark of a
tight `slice` loop under `./bin/test-snippet.sh` isolates it further. Expect
this to move text-heavy scripts only; the corpus goldens must not change.
