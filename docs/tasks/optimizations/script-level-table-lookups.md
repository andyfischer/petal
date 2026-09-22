# Replace linear character scans in example scripts

Status: **proposed**, 2026-09-15. Measured but not started.
Script-level, not runtime — but it is the largest single term in the number.

## The observation

`examples/productivity/spreadsheet/app.ptl` resolves a character by scanning:

```petal
fn digit_val(c: string) -> int
  let d = -1
  for i in range(0, 10) do
    if slice(DIGITS, i, i + 1) == c then
      d = i
    end
  end
  d
end
```

`letter_val` does the same over 26 letters, and neither stops early — the loop
runs to the end after it has found the answer. Every character of every
formula pays this. On the frame that commits a formula, `slice` is **71% of
all builtin calls** (314k of 441k) and the frame costs ~50 ms; a
`bench_panel --profile` run over the edit scenario shows it directly.

This is the app's own cost, and no runtime layer removes it: the frame gate
cannot skip a frame that must recompute, and memoized scopes make it *worse*
(see the recording-cost item in
[the reactive-rendering plan](../../dev/reactive-rendering-plan.md)).

## The change

Use a record as a lookup table, built once at the top level:

```petal ignore
let DIGIT_VAL = {"0": 0, "1": 1, ...}
fn digit_val(c: string) -> int
  _get(DIGIT_VAL, c, -1)
end
```

Then sweep the other examples for the same shape — any `for` over a string
that compares one character at a time. `petal lint` may be able to carry a
rule for it (a loop whose body is a single equality against an indexed
character), which would make this a language-level win rather than one app's
cleanup; see [the linter plan](../../dev/linter-plan.md).

While there: give the scans an early exit, and prefer `char_at` once
[char-at-fast-path.md](char-at-fast-path.md) lands.

## How to check it

```bash
cd petal-ui && cargo run --release --example bench_panel -- \
  ../examples/productivity/spreadsheet/app.ptl 600 \
  --scenario ../examples/productivity/spreadsheet/bench/edit.json --no-gate
```

The commit frame (`max`) is the number to watch. The app's behavior must not
change: `test/example-golden/*.json` covers it, and `petal-ui-run` over the
`bench/` scenarios should produce an identical trace before and after.
