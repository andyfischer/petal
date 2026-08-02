import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetalError } from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("error positions", () => {
  it("reports line and column for division by zero", () => {
    const err = runPetalError("let x = 10 / 0");
    expect(err).toMatch(/line 1/);
    expect(err).toMatch(/Division by zero/);
  });

  it("reports correct line for multiline code", () => {
    const err = runPetalError(`let a = 1
let b = 0
let c = a / b`);
    expect(err).toMatch(/line 3/);
    expect(err).toMatch(/Division by zero/);
  });

  it("reports position for undefined variable", () => {
    const err = runPetalError("let x = foo + 1");
    expect(err).toMatch(/line 1/);
    expect(err).toMatch(/Undefined variable/);
  });

  it("reports position for type errors", () => {
    const err = runPetalError(`let x = "hello"
let y = x - 1`);
    expect(err).toMatch(/line 2/);
  });

  it("arithmetic errors name the operator and operand types", () => {
    const err = runPetalError(`let x = 1 + "a"`);
    expect(err).toMatch(/Cannot add/);
    expect(err).toMatch(/int and string/);
  });

  it("string + string suggests ++ and interpolation", () => {
    const err = runPetalError(`let x = "a" + "b"`);
    expect(err).toMatch(/Cannot add string and string/);
    expect(err).toMatch(/\+\+/);
    expect(err).toMatch(/interpolation/);
  });

  // Integer arithmetic must never panic: a Rust panic compiles to a WASM
  // `unreachable` trap that poisons the runtime for the whole page (the web
  // playground can only recover with a reload). These must surface as clean,
  // recoverable runtime errors instead.
  it("modulo by zero is a clean error, not a panic", () => {
    const err = runPetalError("let x = 5 % 0");
    expect(err).toMatch(/Division by zero/);
    expect(err).not.toMatch(/panic/i);
  });

  it("integer overflow is a clean error, not a panic", () => {
    const err = runPetalError("let x = 9223372036854775807 + 1");
    expect(err).toMatch(/overflow/i);
    expect(err).not.toMatch(/panic/i);
  });

  it("integer multiply overflow is a clean error, not a panic", () => {
    const err = runPetalError("let x = 9223372036854775807 * 2");
    expect(err).toMatch(/overflow/i);
    expect(err).not.toMatch(/panic/i);
  });

  // An error raised inside a closure called by a higher-order intrinsic
  // (map/filter/reduce/forEach) must be annotated exactly once — at the true
  // failing term — not re-annotated again at the intrinsic's call site as the
  // error unwinds through `call_closure_sync`. Regression for the old
  // double-annotation quirk (see docs/dev/bytecode-future-ideas.md).
  it("closure error inside map() is annotated once, at the real failure", () => {
    const err = runPetalError(`fn boom(x)
  x / 0
end
let ys = map([1, 2, 3], boom)`);
    expect(err).toMatch(/Division by zero/);
    // The single annotation points at the division (line 2), not the map call.
    expect(err).toMatch(/2 \|   x \/ 0/);
    // One "Division by zero" and one stack trace — no re-annotation duplicates.
    expect(err.match(/Division by zero/g)?.length ?? 0).toBe(1);
    expect(err.match(/Stack trace:/g)?.length ?? 0).toBe(1);
    // The map() call site must not be grafted into the message or the trace.
    expect(err).not.toMatch(/map\(\[1, 2, 3\], boom\)/);
  });

  // A raw argument error from the intrinsic itself (not from a called closure)
  // is a genuine first-time failure and must still be annotated at the call site.
  it("map() argument-type error is still annotated at the call site", () => {
    const err = runPetalError(`let ys = map(42, fn(x) -> x)`);
    expect(err).toMatch(/map\(\) expects a list/);
    expect(err).toMatch(/line 1/);
  });

  it("errors include a source snippet with a caret under the failing span", () => {
    const err = runPetalError(`let a = 1
let b = 2
let c = a - "bad"`);
    // The snippet should echo the offending line with a gutter.
    expect(err).toMatch(/3 \| let c = a - "bad"/);
    // And a caret line under it.
    expect(err).toMatch(/\^/);
  });

  // Type warnings and runtime errors have always carried a caret block; parse
  // errors were the odd one out, reporting a bare `[line N, column M]`.
  // See docs/syntax/commas.md.
  describe("parse errors carry the same caret block", () => {
    it("underlines the element that needed a comma", () => {
      const err = runPetalError("let e = [\n    1\n    2\n]");
      expect(err).toContain(
        "Error: Expected ',' between list elements [line 3, column 5]"
      );
      expect(err).toContain("3 |     2");
      expect(err).toMatch(/\n\s*\|\s+\^/);
    });

    it("underlines a compile-phase diagnostic too", () => {
      const err = runPetalError("class C\n  x: int,\n  x: int,\nend\nprint(1)");
      expect(err).toContain(
        "duplicate field `x` in class `C` [line 3, column 3]"
      );
      expect(err).toContain("3 |   x: int,");
    });
  });

  // Message cleanups: the parser names tokens the way they are written, and
  // names the construct whose element is missing.
  describe("parse errors name what the reader wrote", () => {
    it("blames the index, not the closing bracket, for a missing comma", () => {
      const err = runPetalError("print([[1,2] [3,4]])");
      expect(err).toContain("Expected ']' to close the index, got ','");
      expect(err).not.toContain("RBracket");
    });

    it("names the construct a stray comma appears in", () => {
      expect(runPetalError("print([,])")).toContain(
        "Expected a list element, got ','"
      );
      expect(runPetalError("print(1,,2)")).toContain(
        "Expected an argument, got ','"
      );
      expect(runPetalError("let a = [\n ,1]\nprint(a)")).toContain(
        "Expected a list element, got ','"
      );
    });

    it("spells tokens as source text, not as Rust variant names", () => {
      const err = runPetalError("let x = ;");
      expect(err).not.toMatch(/Comma|RBracket|LParen|Assign\b/);
    });
  });
});
