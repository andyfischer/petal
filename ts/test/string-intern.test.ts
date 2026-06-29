import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

describe("string interning", () => {
  it("identical strings compare equal", () => {
    const code = `let a = "hello"
let b = "hello"
print(a == b)`;
    expect(runPetal(code)).toBe("true");
  });

  it("works with string operations producing same result", () => {
    // str() of same int should produce interned string
    const code = `let a = str(42)
let b = str(42)
print(a == b)`;
    expect(runPetal(code)).toBe("true");
  });

  it("different strings are not equal", () => {
    const code = `let a = "hello"
let b = "world"
print(a == b)`;
    expect(runPetal(code)).toBe("false");
  });

  it("interning survives GC cycles", () => {
    // Allocate same string many times with GC pressure
    const code = `let results = []
for i in range(0, 2000) do
  let s = "constant_string"
  if i == 1999 then
    results = append(results, s)
  end
end
print(results[0])`;
    expect(runPetal(code)).toBe("constant_string");
  });

  it("interning works with map keys", () => {
    // Map field access uses strings — interning helps here
    const code = `let data = []
for i in range(0, 100) do
  let r = { name: "test", value: i }
  if i == 99 then
    data = append(data, r.name)
  end
end
print(data[0])`;
    expect(runPetal(code)).toBe("test");
  });

  it("concatenated strings are interned too", () => {
    const code = `let a = "hel" ++ "lo"
let b = "hel" ++ "lo"
print(a == b)`;
    expect(runPetal(code)).toBe("true");
  });
});
