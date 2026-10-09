// `repeat`, `starts_with`, `ends_with`, `trim` — the string helpers every
// text-handling testbed app wrote by hand — and the string-building loop
// (`out = out ++ piece`) that long strings used to make quadratic in hashing.

import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";

describe("repeat", () => {
  it("writes the string n times", () => {
    expect(runPetal(`print(repeat("ab", 3))`)).toBe("ababab");
    expect(runPetal(`print("-".repeat(5))`)).toBe("-----");
    expect(runPetal(`print(repeat(string: "é", count: 2))`)).toBe("éé");
  });

  it("is empty for a zero or negative count", () => {
    expect(runPetal(`print("[" ++ repeat("x", 0) ++ repeat("x", -3) ++ "]")`)).toBe("[]");
  });

  it("rejects a non-string, a float count, and an absurd size", () => {
    expect(runPetalError(`print(repeat(5, 3))`)).toContain("repeat() expects (string, int)");
    expect(runPetalError(`print(repeat("a", 1.5))`)).toContain("repeat() expects (string, int)");
    expect(runPetalError(`print(repeat("abcdefgh", 9000000000))`)).toContain("too large");
  });
});

describe("starts_with / ends_with", () => {
  it("test a prefix and a suffix", () => {
    expect(
      runPetal(`print(starts_with("hello", "he"), starts_with("hello", "lo"), starts_with("he", "hello"))`)
    ).toBe("true false false");
    expect(
      runPetal(`print(ends_with("hello", "lo"), ends_with("hello", "he"), "a.ptl".ends_with(".ptl"))`)
    ).toBe("true false true");
  });

  it("every string starts and ends with the empty string", () => {
    expect(runPetal(`print(starts_with("", ""), ends_with("x", ""))`)).toBe("true true");
  });

  it("reject a non-string", () => {
    expect(runPetalError(`print(starts_with(12, "1"))`)).toContain(
      "starts_with() expects (string, string)"
    );
  });
});

describe("trim", () => {
  it("drops whitespace at both ends only", () => {
    expect(runPetal(`print("[" ++ trim("  \\t a b \\n") ++ "]")`)).toBe("[a b]");
    expect(runPetal(`print("[" ++ trim("   ") ++ "]", "[" ++ "x".trim() ++ "]")`)).toBe("[] [x]");
  });

  it("keeps non-ASCII text whole", () => {
    expect(runPetal(`print(trim(" Óscar "), char_len(trim(" Óscar ")))`)).toBe("Óscar 5");
  });
});

describe("a script's own definition wins over the new builtins", () => {
  it("a user fn named trim, and locals named repeat/trim", () => {
    const code = `fn trim(s)
  "mine:" ++ s
end
fn f(x)
  let repeat = x + 1
  let starts_with = "#fff"
  "{starts_with} {repeat}"
end
print(trim("a"), f(1))`;
    expect(runPetal(code)).toBe("mine:a #fff 2");
  });
});

describe("long strings", () => {
  it("built two ways compare equal and key the same record field", () => {
    // Past the intern limit two equal strings have different heap ids.
    const code = `let a = repeat("abc", 400)
let b = repeat("abc", 399) ++ "abc"
let r = {}
r[a] = 1
print(a == b, a != b, len(a), r[b], contains(a, "cab"), index_of([b], a))`;
    expect(runPetal(code)).toBe("true false 1200 1 true 0");
  });

  it("a 100k-append loop finishes quickly", () => {
    const code = `let out = ""
for i in range(0, 100000) do
  out = out ++ "x"
end
print(len(out), starts_with(out, "xxx"))`;
    const start = Date.now();
    expect(runPetal(code)).toBe("100000 true");
    // 4-6 s before long strings stopped being interned; ~0.1 s after (release).
    expect(Date.now() - start).toBeLessThan(3000);
  });
});
