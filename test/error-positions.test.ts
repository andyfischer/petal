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

  it("errors include a source snippet with a caret under the failing span", () => {
    const err = runPetalError(`let a = 1
let b = 2
let c = a - "bad"`);
    // The snippet should echo the offending line with a gutter.
    expect(err).toMatch(/3 \| let c = a - "bad"/);
    // And a caret line under it.
    expect(err).toMatch(/\^/);
  });
});
