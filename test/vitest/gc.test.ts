import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";


describe("garbage collection", () => {
  it("preserves reachable values through GC", () => {
    // Create values, do work that triggers GC, verify originals still accessible
    const code = `let x = "hello"
let y = [1, 2, 3]
for i in range(0, 2000) do
  let tmp = "garbage_" ++ str(i)
  let tmp2 = [i, i + 1, i + 2]
end
print(x)
print(y)`;
    const out = runPetal(code);
    expect(out).toBe("hello\n[1, 2, 3]");
  });

  it("preserves state values through GC", () => {
    const code = `state counter = 0
for i in range(0, 2000) do
  counter += 1
  let tmp = "discard_" ++ str(i)
end
print(counter)`;
    const out = runPetal(code);
    expect(out).toBe("2000");
  });

  it("preserves closure captures through GC", () => {
    const code = `let name = "world"
let greet = fn(prefix) -> prefix ++ " " ++ name
for i in range(0, 2000) do
  let tmp = [i, i * 2]
end
print(greet("hello"))`;
    const out = runPetal(code);
    expect(out).toBe("hello world");
  });

  it("preserves nested data structures through GC", () => {
    const code = `let data = { items: [1, 2, 3], label: "test" }
for i in range(0, 2000) do
  let tmp = { x: i, y: "junk" }
end
print(data.label)
print(data.items)`;
    const out = runPetal(code);
    expect(out).toBe("test\n[1, 2, 3]");
  });

  it("preserves enum variants through GC", () => {
    const code = `enum Color
  Red,
  Green,
  Blue,
end
let c = Color.Green
for i in range(0, 2000) do
  let tmp = "garbage_" ++ str(i)
end
match c
  when Color.Green -> print("green")
  when _ -> print("not green")
end`;
    const out = runPetal(code);
    expect(out).toBe("green");
  });

  it("preserves elements through GC", () => {
    const code = `let el = <div class="test">hello</div>
for i in range(0, 2000) do
  let tmp = <span>{str(i)}</span>
end
print(el)`;
    const out = runPetal(code);
    expect(out).toBe('<div class="test">hello</div>');
  });

  it("handles map/filter with GC pressure", () => {
    const code = `let nums = range(1, 100)
for i in range(0, 50) do
  let tmp = range(1, 100) |> map(fn(x) -> x * 2) |> filter(fn(x) -> x > 50)
end
let result = nums |> map(fn(x) -> x * 10) |> filter(fn(x) -> x <= 30)
print(result)`;
    const out = runPetal(code);
    expect(out).toBe("[10, 20, 30]");
  });

  it("keeps frozen data through collections that no longer trace it", () => {
    const code = `fn build(n)
  for i in range(0, n) do
    { id: i, name: "row " ++ str(i), tags: [i, i + 1] }
  end
end
let table = freeze(build(500))
for i in range(0, 20000) do
  let tmp = { a: i, b: "junk_" ++ str(i) }
end
let total = 0
for row in table do
  total += row.id + row.tags[1] + len(row.name)
end
print(frozen(table), frozen(table[499]), frozen(table[499].tags), total)
print(table[499])`;
    const out = runPetal(code);
    // ids 0..499 sum to 124750, tags[1] adds 125250, the names 4 + digits.
    expect(out).toBe('true true true 253390\n{ id: 499, name: "row 499", tags: [499, 500] }');
  });

  it("copies on an update of a frozen value and refuses to freeze a function", () => {
    const out = runPetal(`let xs = freeze([1, 2, 3])
let ys = append(xs, 4)
print(xs, ys, frozen(xs), frozen(ys))`);
    expect(out).toBe("[1, 2, 3] [1, 2, 3, 4] true false");
    const err = runPetalError("let f = fn(x) -> x\nfreeze([f])");
    expect(err).toContain("freeze: the value holds a function, which cannot be frozen");
  });
});
