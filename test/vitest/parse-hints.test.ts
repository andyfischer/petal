// Errors that name the fix for a habit from another language, end to end
// (the parser-level cases are unit-tested in rust/tests/parse_hints.rs).
// See docs/tasks/testbed-takeaways.md (#8).
import { describe, it, expect } from "vitest";
import { runPetalError } from "./helpers";

describe("word operators", () => {
  it("`and` at statement level, where it reaches the compiler", () => {
    const err = runPetalError("let a = true\nlet c = a and a\nprint(c)");
    expect(err).toContain("Undefined variable: and");
    expect(err).toContain("`&&`");
  });

  it("`or` and `not`", () => {
    expect(runPetalError("let a = true\nlet c = a or a\nprint(c)")).toContain("`||`");
    expect(runPetalError("let a = true\nlet c = not a\nprint(c)")).toContain("`!`");
  });
});

describe("interpolation", () => {
  it("an empty hole is an error, not a silently dropped placeholder", () => {
    const err = runPetalError('print("{} items")');
    expect(err).toContain("Empty interpolation hole");
  });
});
