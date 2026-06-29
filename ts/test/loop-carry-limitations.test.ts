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
for x in [1, 2, 3] do
  total = total + x
  if x == 2 then break end
  total = total + 100
end
print(total)`);
    expect(out.trim()).toBe("103");
  });

  it("carry behaves correctly when all rebinds execute before break", () => {
    const out = runPetal(`let total = 0
for x in [1, 2, 3] do
  total = total + x
  if x == 2 then break end
end
print(total)`);
    expect(out.trim()).toBe("3");
  });

  it("break from inside a nested if still sees the outer rebind in the slot", () => {
    const out = runPetal(`let n = 0
for x in [10, 20, 30] do
  n = n + x
  if x == 20 then
    if true then break end
  end
end
print(n)`);
    expect(out.trim()).toBe("30");
  });

  it("break inside an inner loop exits only that loop and the outer carry is updated", () => {
    // Inner break should not propagate to the outer loop. Expected sum:
    //   i=1: j=10 -> 10, j=20 -> 30, break
    //   i=2: j=10 -> 50, j=20 -> 90, break
    const out = runPetal(`let t = 0
for i in [1, 2] do
  for j in [10, 20] do
    t = t + i * j
    if j == 20 then break end
  end
end
print(t)`);
    expect(out.trim()).toBe("90");
  });
});

describe("multiple rebinds of a carried var inside a nested if-block", () => {
  it("propagates every rebind, not just the first, out of the if-block", () => {
    // Regression: the if-block's phi-out used to capture only the FIRST
    // rebind of `s` (block_rebinds was not updated on subsequent in-block
    // reassignments), so the second `append` was silently dropped each
    // iteration. Expected: each iteration appends both i and i*10.
    const out = runPetal(`let s = [0]
for i in range(0, 3) do
  if true then
    s = append(s, i)
    s = append(s, i * 10)
  end
end
print(s)`);
    expect(out.trim()).toBe("[0, 0, 0, 1, 10, 2, 20]");
  });

  it("works through a stack-style last/drop_last pop loop", () => {
    // The pattern noc_fractal_tree.ptl relies on: pop the top with
    // last+drop_last, then push two children inside a conditional.
    const out = runPetal(`let stack = [1]
let visited = []
while len(stack) > 0 do
  let cur = last(stack)
  stack = drop_last(stack)
  visited = append(visited, cur)
  if cur < 4 then
    stack = append(stack, cur + 1)
    stack = append(stack, cur + 10)
  end
end
print(visited)`);
    // DFS order: 1, 11, 2, 12, 3, 13, 4 (children pushed +1 then +10, so
    // +10 is on top and popped first).
    expect(out.trim()).toBe("[1, 11, 2, 12, 3, 13, 4]");
  });
});

describe("known limitation: let shadow disables carry detection", () => {
  it("assignment to outer name is lost when body has a let shadow", () => {
    // `let x` anywhere at the top level of the body excludes `x` from
    // carry detection entirely, so `x = 5` inside the loop never escapes.
    // Fixing this requires in-order detection (compile-time tracking of
    // currently-bound outer names).
    const out = runPetal(`let x = 1
for i in [1, 2, 3] do
  x = 5
  let x = i * 10
  x = x + 1
end
print(x)`);
    expect(out.trim()).toBe("1");
  });

  it("without a let shadow, the assignment carries correctly", () => {
    const out = runPetal(`let x = 1
for i in [1, 2, 3] do
  x = i * 10
end
print(x)`);
    expect(out.trim()).toBe("30");
  });
});
