import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

describe("garbage collection", () => {
  it("preserves reachable values through GC", () => {
    // Create values, do work that triggers GC, verify originals still accessible
    const code = `let x = "hello"
let y = [1, 2, 3]
for i in range(0, 2000) {
  let tmp = "garbage_" ++ str(i)
  let tmp2 = [i, i + 1, i + 2]
}
print(x)
print(y)`;
    const out = runPetal(code);
    expect(out).toBe("hello\n[1, 2, 3]");
  });

  it("preserves state values through GC", () => {
    const code = `state counter = 0
for i in range(0, 2000) {
  counter += 1
  let tmp = "discard_" ++ str(i)
}
print(counter)`;
    const out = runPetal(code);
    expect(out).toBe("2000");
  });

  it("preserves closure captures through GC", () => {
    const code = `let name = "world"
let greet = fn(prefix) { prefix ++ " " ++ name }
for i in range(0, 2000) {
  let tmp = [i, i * 2]
}
print(greet("hello"))`;
    const out = runPetal(code);
    expect(out).toBe("hello world");
  });

  it("preserves nested data structures through GC", () => {
    const code = `let data = { items: [1, 2, 3], label: "test" }
for i in range(0, 2000) {
  let tmp = { x: i, y: "junk" }
}
print(data.label)
print(data.items)`;
    const out = runPetal(code);
    expect(out).toBe("test\n[1, 2, 3]");
  });

  it("preserves enum variants through GC", () => {
    const code = `enum Color { Red, Green, Blue }
let c = Green
for i in range(0, 2000) {
  let tmp = "garbage_" ++ str(i)
}
match c {
  Green -> print("green")
}`;
    const out = runPetal(code);
    expect(out).toBe("green");
  });

  it("preserves elements through GC", () => {
    const code = `let el = <div class="test">hello</div>
for i in range(0, 2000) {
  let tmp = <span>{str(i)}</span>
}
print(el)`;
    const out = runPetal(code);
    expect(out).toBe('<div class="test">hello</div>');
  });

  it("handles map/filter with GC pressure", () => {
    const code = `let nums = range(1, 100)
for i in range(0, 50) {
  let tmp = range(1, 100) |> map(fn(x) { x * 2 }) |> filter(fn(x) { x > 50 })
}
let result = nums |> map(fn(x) { x * 10 }) |> filter(fn(x) { x <= 30 })
print(result)`;
    const out = runPetal(code);
    expect(out).toBe("[10, 20, 30]");
  });
});
