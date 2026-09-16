# Make `char_at` the fast way to scan a string

Status: **proposed**, 2026-09-15. Measured but not started.

## The observation

Petal already has character-indexed string ops (`chars`, `char_len`,
`char_at`, `char_slice`, `index_of` — `rust/src/builtins/collections.rs`), so
a scanner does not have to go through `slice`. But `char_at` is currently the
*slower* choice:

```rust
let count = s.chars().count();          // O(n) on every call
let ch = s.chars().nth(idx as usize);   // O(n) again
state.push_string(c.to_string());        // allocates, then interns
```

`slice(s, i, i + 1)` at least reaches `Heap::intern_substring`, which looks the
substring up without materializing it. So the builtin that exists for this
pattern loses to the one that does not, and scripts that scan text — the
spreadsheet's formula tokenizer, any parser written in Petal — reach for
`slice` and stay on byte offsets.

No example currently calls `char_at` in a scanning loop
(`examples/productivity/spreadsheet/app.ptl` has zero uses).

## The change

1. **O(1) for ASCII.** Check `s.is_ascii()` (or track an ASCII flag per heap
   string) and index directly; fall back to the `chars()` walk only for
   genuinely multi-byte text.
2. **Do not allocate the result.** Use `Heap::intern_substring(id, start, end)`
   as `slice` does, so the common case is a lookup with no allocation. This
   composes with [string-intern-hashing.md](string-intern-hashing.md) — with
   both, a single-character read is an index plus a small-table hit.
3. Once it is fast, say so: `docs/stdlib.json` and the language guide should
   steer scanners at `char_at` / `char_slice` rather than byte `slice`, and
   `petal lint` could suggest the rewrite where the byte-offset semantics are
   provably the same (see [the linter plan](../../dev/linter-plan.md)).

## How to check it

A microbenchmark of both forms over the same string, then the app that
motivated it:

```bash
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/productivity/spreadsheet/app.ptl 600 \
  --scenario ../examples/productivity/spreadsheet/bench/edit.json --no-gate
```

Multi-byte behavior is the risk: `char_at` is character-indexed and `slice` is
byte-indexed, so the ASCII fast path must be exactly equivalent. Property-test
the two paths against each other over non-ASCII input.
