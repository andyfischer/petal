// Multi-file module system tests (docs/module-system.md).
//
// The first multi-.ptl cases in the harness: fixtures/modules/ holds a shared
// palette module imported by two sibling entry scripts. Every case runs the
// compiled CLI on real files so importer-relative resolution is exercised.

import { describe, it, expect, beforeAll } from "vitest";
import { execSync } from "child_process";
import { resolve } from "path";
import { ensureBuild } from "./helpers";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");
const FIXTURES = resolve(__dirname, "fixtures/modules");

function runFile(path: string): string {
  return execSync(`${PETAL} run ${path}`, {
    encoding: "utf-8",
    timeout: 10000,
  }).trim();
}

function runFileError(args: string): string {
  try {
    execSync(`${PETAL} ${args}`, {
      encoding: "utf-8",
      timeout: 10000,
      stdio: ["pipe", "pipe", "pipe"],
    });
    throw new Error("Expected petal to fail but it succeeded");
  } catch (e: any) {
    return (e.stderr || "").trim();
  }
}

beforeAll(() => {
  ensureBuild();
});

describe("module imports across files", () => {
  it("panel.ptl: qualified + selective import", () => {
    const out = runFile(resolve(FIXTURES, "panel.ptl"));
    expect(out).toBe("15\n255\n1");
  });

  it("detail.ptl: aliased import used inside a fn", () => {
    const out = runFile(resolve(FIXTURES, "detail.ptl"));
    expect(out).toBe("<11>\n9");
  });

  it("private members are not importable", () => {
    const err = runFileError(
      `run -e 'import palette: _clamp' -I ${FIXTURES}`
    );
    expect(err).toContain("module-private");
  });

  it("-I adds a module search directory", () => {
    const out = execSync(
      `${PETAL} run -e 'import palette\nprint(palette.colors.bg)' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("2");
  });

  it("missing module reports a compile error", () => {
    const err = runFileError(`run -e 'import missing_module'`);
    expect(err).toContain("cannot find module 'missing_module'");
  });

  it("multi-file IR carries the file table and roundtrips", () => {
    const ir = execSync(
      `${PETAL} show-ir --json ${resolve(FIXTURES, "panel.ptl")}`,
      { encoding: "utf-8", timeout: 10000 }
    );
    const parsed = JSON.parse(ir);
    const names = parsed.source_map.files.map((f: any) => f.name);
    expect(names).toEqual(["panel.ptl", "palette.ptl"]);

    const out = execSync(`${PETAL} run --ir -`, {
      encoding: "utf-8",
      timeout: 10000,
      input: ir,
    }).trim();
    expect(out).toBe("15\n255\n1");
  });

  it("runtime errors in a module name the module file", () => {
    const err = runFileError(
      `run -e 'import palette\nprint(palette.colors + 1)' -I ${FIXTURES}`
    );
    // The failing add is in the entry file, so entry format; the provenance
    // of `colors` points into palette.ptl.
    expect(err).toContain("palette.ptl");
  });
});
