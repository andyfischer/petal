// Regressions for the numeric/string builtin bugs reported by the testbed apps:
//
//   • `clamp` returned a float for an all-int call, poisoning list indices and
//     `range` bounds (five apps hit it; petal-ui carried a private `_clamp`);
//   • `float("3.5")` rejected numeric strings while `int("42")` accepted them;
//   • there was no failable string->number conversion, so reading user input
//     meant either aborting or hand-rolling a digit scanner;
//   • `round` had no places form;
//   • `len`/`slice` are byte-indexed, so the obvious "first letter" loop
//     silently produced wrong data for a non-ASCII name.

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, runPetalError } from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("clamp preserves int-ness", () => {
  it("returns an int for three int arguments", () => {
    expect(runPetal(`print(clamp(3, 0, 5))`)).toBe("3");
    expect(runPetal(`print(type(clamp(3, 0, 5)))`)).toBe("int");
  });

  it("clamps at both ends and stays int", () => {
    expect(runPetal(`print(clamp(-4, 0, 5), clamp(9, 0, 5))`)).toBe("0 5");
  });

  it("still returns a float when any argument is a float", () => {
    expect(runPetal(`print(clamp(3.0, 0, 5))`)).toBe("3.0");
    expect(runPetal(`print(clamp(3, 0.0, 5))`)).toBe("3.0");
    expect(runPetal(`print(clamp(15.0, 0.0, 10.0))`)).toBe("10.0");
  });

  it("a clamped index can index a list", () => {
    expect(
      runPetal(`let xs = [10, 20, 30]\nprint(xs[clamp(9, 0, len(xs) - 1)])`)
    ).toBe("30");
  });

  it("a clamped bound can drive a numeric for-loop", () => {
    expect(
      runPetal(`for i in range(0, clamp(2, 0, 10)) do\n  print(i)\nend`)
    ).toBe("0\n1");
  });
});

describe("float() accepts numeric strings", () => {
  it("parses a decimal string", () => {
    expect(runPetal(`print(float("3.5"))`)).toBe("3.5");
  });

  it("parses an integer string as a float", () => {
    expect(runPetal(`print(float("42"))`)).toBe("42.0");
  });

  it("ignores surrounding whitespace", () => {
    expect(runPetal(`print(float("  -2.25 "))`)).toBe("-2.25");
  });

  it("still converts numbers", () => {
    expect(runPetal(`print(float(42), float(1.5))`)).toBe("42.0 1.5");
  });

  it("aborts on a non-numeric string, like int()", () => {
    expect(runPetalError(`print(float("abc"))`)).toContain(
      "Cannot convert 'abc' to float"
    );
  });
});

describe("parse_float / parse_int return nil instead of aborting", () => {
  it("parses good input", () => {
    expect(runPetal(`print(parse_float("3.5"), parse_int("42"))`)).toBe(
      "3.5 42"
    );
  });

  it("returns nil for junk instead of raising", () => {
    expect(runPetal(`print(parse_float("abc"), parse_int("abc"))`)).toBe(
      "nil nil"
    );
  });

  it("returns nil for an empty or whitespace-only string", () => {
    expect(runPetal(`print(parse_float(""), parse_int("   "))`)).toBe(
      "nil nil"
    );
  });

  it("rejects a partially numeric string", () => {
    expect(runPetal(`print(parse_float("3.5kg"), parse_int("12abc"))`)).toBe(
      "nil nil"
    );
  });

  it("parse_int refuses to silently truncate a decimal", () => {
    expect(runPetal(`print(parse_int("3.5"))`)).toBe("nil");
    expect(runPetal(`print(int(parse_float("3.5")))`)).toBe("3");
  });

  it("passes numbers through", () => {
    expect(runPetal(`print(parse_float(2), parse_int(2.9))`)).toBe("2.0 2");
  });

  it("returns nil for a non-string, non-number", () => {
    expect(runPetal(`print(parse_float([1]), parse_int(nil))`)).toBe("nil nil");
  });

  it("supports the validate-then-use shape it exists for", () => {
    expect(
      runPetal(
        `let n = parse_float("oops")\nif n == nil then\n  print("not a number")\nelse\n  print(n * 2)\nend`
      )
    ).toBe("not a number");
  });
});

describe("round(x, places)", () => {
  it("rounds to a number of decimal places", () => {
    expect(runPetal(`print(round(3.14159, 2))`)).toBe("3.14");
    expect(runPetal(`print(round(2.71828, 3))`)).toBe("2.718");
  });

  it("rounds to zero places like the one-argument form", () => {
    expect(runPetal(`print(round(3.7, 0), round(3.7))`)).toBe("4.0 4.0");
  });

  it("negative places round left of the decimal point", () => {
    expect(runPetal(`print(round(1234.0, -2))`)).toBe("1200.0");
  });

  it("keeps an int argument an int", () => {
    expect(runPetal(`print(round(7, 3), type(round(7, 3)))`)).toBe("7 int");
    expect(runPetal(`print(round(1234, -2))`)).toBe("1200");
  });

  it("errors on a non-number", () => {
    expect(runPetalError(`print(round("x", 2))`)).toContain(
      "round() expects a number"
    );
  });
});

describe("character-indexed string builtins", () => {
  it("chars() splits into characters, not bytes", () => {
    expect(runPetal(`print(chars("Óscar"))`)).toBe(`["Ó", "s", "c", "a", "r"]`);
    expect(runPetal(`print(chars(""))`)).toBe("[]");
  });

  it("char_len() counts characters where len() counts bytes", () => {
    expect(runPetal(`print(char_len("Óscar"), len("Óscar"))`)).toBe("5 6");
  });

  it("char_at() returns the character at a char index", () => {
    expect(runPetal(`print(char_at("Óscar", 0))`)).toBe("Ó");
    expect(runPetal(`print(char_at("Óscar", 2))`)).toBe("c");
  });

  it("char_at() counts from the end for a negative index", () => {
    expect(runPetal(`print(char_at("Óscar", -1))`)).toBe("r");
  });

  it("char_at() yields an empty string out of range", () => {
    expect(runPetal(`print(len(char_at("abc", 99)), len(char_at("abc", -99)))`))
      .toBe("0 0");
  });

  it("char_slice() slices by character, where slice() drops the char", () => {
    // The bug an app shipped: initials read "D" for "Óscar Delgado".
    expect(runPetal(`print(char_slice("Óscar Delgado", 0, 1))`)).toBe("Ó");
    expect(runPetal(`print(slice("Óscar Delgado", 0, 1))`)).toBe("");
  });

  it("char_slice() defaults end to the end of the string", () => {
    expect(runPetal(`print(char_slice("Óscar", 1))`)).toBe("scar");
  });

  it("char_slice() supports negative indices and clamps", () => {
    expect(runPetal(`print(char_slice("Óscar", -3, -1))`)).toBe("ca");
    expect(runPetal(`print(char_slice("Óscar", 3, 99))`)).toBe("ar");
    expect(runPetal(`print(len(char_slice("Óscar", 4, 2)))`)).toBe("0");
  });

  it("builds correct initials for a non-ASCII name", () => {
    expect(
      runPetal(
        `let parts = split("Óscar Delgado", " ")\nlet out = ""\nfor p in parts do\n  out = out ++ char_at(p, 0)\nend\nprint(out)`
      )
    ).toBe("ÓD");
  });

  it("rejects non-strings", () => {
    expect(runPetalError(`print(chars(5))`)).toContain(
      "chars() expects a string"
    );
    expect(runPetalError(`print(char_len(5))`)).toContain(
      "char_len() expects a string"
    );
    expect(runPetalError(`print(char_at(5, 0))`)).toContain(
      "char_at() expects a string"
    );
    expect(runPetalError(`print(char_slice(5, 0))`)).toContain(
      "char_slice() expects a string"
    );
  });
});

describe("index_of", () => {
  it("finds a substring by character index", () => {
    expect(runPetal(`print(index_of("hello world", "world"))`)).toBe("6");
    expect(runPetal(`print(index_of("Óscar", "s"))`)).toBe("1");
  });

  it("returns -1 when the needle is absent", () => {
    expect(runPetal(`print(index_of("abc", "z"))`)).toBe("-1");
    expect(runPetal(`print(index_of([1, 2, 3], 9))`)).toBe("-1");
  });

  it("finds a list element", () => {
    expect(runPetal(`print(index_of([10, 20, 30], 20))`)).toBe("1");
    expect(runPetal(`print(index_of(["a", "b"], "b"))`)).toBe("1");
  });

  it("reports the first occurrence", () => {
    expect(runPetal(`print(index_of("abcabc", "b"), index_of([1, 2, 1], 1))`))
      .toBe("1 0");
  });

  it("composes with char_slice", () => {
    expect(
      runPetal(
        `let s = "key=value"\nlet i = index_of(s, "=")\nprint(char_slice(s, 0, i), char_slice(s, i + 1))`
      )
    ).toBe("key value");
  });

  it("rejects bad argument types", () => {
    expect(runPetalError(`print(index_of(5, 5))`)).toContain(
      "index_of() expects a list or string"
    );
    expect(runPetalError(`print(index_of("abc", 1))`)).toContain(
      "index_of() on string expects a string"
    );
  });
});
