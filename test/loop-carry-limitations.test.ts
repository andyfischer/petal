import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

// These tests pin *current* behavior for two known limitations of the
// pure-dataflow loop-carry lowering. They're here so that when either
// limitation is addressed (see the archived mutability follow-ups), the
// change is forced to be intentional and updates these expectations.

describe("known limitation: carry leak on break-before-last-rebind", () => {
  it("carry becomes nil when break fires before the body's final rebind", () => {
    // The compile-time "latest binding" for `total` is `total + 100`.
    // When `break` fires at x == 2, that term never executes in iter 2,
    // so the phi_out reads an unwritten register and writes Nil.
    // A future fix (shared-register allocation for carries) should make
    // this print 103 — update this test when that lands.
    const out = runPetal(`let total = 0
for x in [1, 2, 3] {
  total = total + x
  if x == 2 { break }
  total = total + 100
}
print(total)`);
    expect(out.trim()).toBe("nil");
  });

  it("carry behaves correctly when all rebinds execute before break", () => {
    const out = runPetal(`let total = 0
for x in [1, 2, 3] {
  total = total + x
  if x == 2 { break }
}
print(total)`);
    expect(out.trim()).toBe("3");
  });
});

describe("known limitation: let shadow disables carry detection", () => {
  it("assignment to outer name is lost when body has a let shadow", () => {
    // `let x` anywhere at the top level of the body excludes `x` from
    // carry detection entirely, so `x = 5` inside the loop never escapes.
    // Fixing this requires in-order detection (compile-time tracking of
    // currently-bound outer names).
    const out = runPetal(`let x = 1
for i in [1, 2, 3] {
  x = 5
  let x = i * 10
  x = x + 1
}
print(x)`);
    expect(out.trim()).toBe("1");
  });

  it("without a let shadow, the assignment carries correctly", () => {
    const out = runPetal(`let x = 1
for i in [1, 2, 3] {
  x = i * 10
}
print(x)`);
    expect(out.trim()).toBe("30");
  });
});
