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
});
