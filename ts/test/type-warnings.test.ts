import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  checkJson,
  checkText,
  checkStrict,
  runWithStderr,
} from "./helpers";

beforeAll(() => ensureBuild());

// Chunk E: type-checker warnings surfaced by `petal check` and `petal run`.
// Warnings are non-fatal: `check` still exits 0 and `run` still executes the
// program (annotations are runtime-inert). `--json` check emits a `warnings`
// array; text mode prints `warning:` lines to stderr.

describe("type-checker warnings via `petal check --json`", () => {
  it("reports a let type mismatch as a single warning, ok stays true", () => {
    const out = checkJson('let x: int = "hi"');
    expect(out.ok).toBe(true);
    expect(Array.isArray(out.warnings)).toBe(true);
    expect(out.warnings).toHaveLength(1);
    const w = out.warnings[0];
    expect(w.message).toMatch(/mismatch/i);
    expect(typeof w.line).toBe("number");
    expect(typeof w.column).toBe("number");
    expect(w.line).toBeGreaterThan(0);
    expect(w.column).toBeGreaterThan(0);
  });

  it("emits an empty warnings array for a clean program", () => {
    const out = checkJson("let x: int = 5");
    expect(out.ok).toBe(true);
    expect(out.warnings).toEqual([]);
  });

  it("reports a call-argument mismatch end-to-end", () => {
    const out = checkJson('fn area(r: float) -> float\n  r\nend\nprint(area("x"))');
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/argument 1/);
  });
});

describe("type-checker warnings via `petal check` (text)", () => {
  it("prints a warning to stderr, empty stdout, exit 0", () => {
    const { stdout, stderr, code } = checkText('let x: int = "hi"');
    expect(code).toBe(0);
    expect(stdout).toBe("");
    expect(stderr).toContain("warning:");
    expect(stderr).toMatch(/mismatch/i);
  });
});

describe("`petal check --strict`", () => {
  it("exits non-zero when warnings exist", () => {
    const { code, stderr } = checkStrict('let x: int = "hi"');
    expect(code).toBe(1);
    expect(stderr).toContain("warning:");
  });

  it("exits 0 for a clean program", () => {
    const { code } = checkStrict("let x: int = 5");
    expect(code).toBe(0);
  });
});

describe("type-checker warnings via `petal run`", () => {
  it("still runs the program (runtime-inert) and warns on stderr", () => {
    const { stdout, stderr } = runWithStderr('let x: int = "hi"\nprint(x)');
    expect(stdout.trim()).toBe("hi");
    expect(stderr).toContain("warning:");
  });
});
