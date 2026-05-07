import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

// Pins behavior for edge cases in loop carries. The break-mid-body cases
// verify that the shared carry-slot register allocation keeps the slot in
// sync even when `break` skips rebinds that appear later in source order.

describe("carry values on break-before-last-rebind", () => {
  it("break after a mid-body rebind preserves that rebind in the carry", () => {
    // The compile-time "latest binding" for `total` is `total + 100`, but
    // when `break` fires at x == 2 that term never runs. Shared carry-slot
    // allocation makes every body-level rebind share one register, so the
    // slot still holds `total + x` (103 in iter 2) when the frame pops.
    const out = runPetal(`let total = 0
for x in [1, 2, 3] {
  total = total + x
  if x == 2 { break }
  total = total + 100
}
print(total)`);
    expect(out.trim()).toBe("103");
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

  it("break from inside a nested if still sees the outer rebind in the slot", () => {
    const out = runPetal(`let n = 0
for x in [10, 20, 30] {
  n = n + x
  if x == 20 {
    if true { break }
  }
}
print(n)`);
    expect(out.trim()).toBe("30");
  });

  it("break inside an inner loop exits only that loop and the outer carry is updated", () => {
    // Inner break should not propagate to the outer loop. Expected sum:
    //   i=1: j=10 -> 10, j=20 -> 30, break
    //   i=2: j=10 -> 50, j=20 -> 90, break
    const out = runPetal(`let t = 0
for i in [1, 2] {
  for j in [10, 20] {
    t = t + i * j
    if j == 20 { break }
  }
}
print(t)`);
    expect(out.trim()).toBe("90");
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
