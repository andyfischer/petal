// Namespaced module paths: `import bloom/menu` (docs/module-system.md).
//
// A module path is one or more identifier segments joined by '/'. The path is
// the module's identity — bloom/menu and petal/menu are two modules — while the
// name it binds locally is the last segment, or whatever `as` says.
// fixtures/module-paths/ holds two namespace directories that both ship a
// menu.ptl, which is the arrangement flat module names made impossible.

import { describe, it, expect } from "vitest";
import { execSync } from "child_process";
import { resolve } from "path";
import { PETAL, petalCapture, runPetalFile } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/module-paths");

/** Run `petal <args>` (argv form, no shell) expecting failure; return stderr. */
function runFileError(args: string[]): string {
  const r = petalCapture(args);
  if (r.code === 0) throw new Error("Expected petal to fail but it succeeded");
  return r.stderr.trim();
}

describe("namespaced module paths", () => {
  it("binds the last segment, and two namespaces coexist", () => {
    // app.ptl: `import bloom/menu` + `import petal/menu as pmenu`.
    const out = runPetalFile(resolve(FIXTURES, "app.ptl"));
    expect(out).toBe("bloom open eased\npetal open");
  });

  it("a selective import on a nested path may wrap across lines", () => {
    const out = runPetalFile(resolve(FIXTURES, "selective.ptl"));
    expect(out).toBe("bloom open eased\nbloom close");
  });

  it("a nested module's own imports resolve beside it", () => {
    // bloom/menu.ptl says `import motion`, which is bloom/motion.ptl.
    const out = execSync(
      `${PETAL} run -e 'import bloom/menu\nprint(menu.open())' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("bloom open eased");
  });

  it("`as` overrides the default local name", () => {
    const out = execSync(
      `${PETAL} run -e 'import bloom/menu as m\nprint(m.close())' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("bloom close");
  });

  it("two paths ending in the same segment collide on the local name", () => {
    const err = runFileError([
      "run",
      "-e",
      "import bloom/menu\nimport petal/menu\nprint(1)",
      "-I",
      FIXTURES,
    ]);
    expect(err).toContain(
      "'menu' is already an alias for module 'bloom/menu' and cannot also alias 'petal/menu'"
    );
  });

  it("'..' in a path is a clean error, not a traversal", () => {
    const err = runFileError(["run", resolve(FIXTURES, "bad_path.ptl")]);
    expect(err).toContain("a module path segment must be an identifier");
    expect(err).toContain("'.' and '..' are not allowed");
  });

  it("a missing nested module names the path it looked for", () => {
    const err = runFileError(["run", "-e", "import bloom/nope", "-I", FIXTURES]);
    expect(err).toContain("cannot find module 'bloom/nope'");
    expect(err).toContain("no bloom/nope.ptl");
  });

  it("diagnostics name a nested module by its path", () => {
    const ir = execSync(`${PETAL} show-ir --json ${resolve(FIXTURES, "app.ptl")}`, {
      encoding: "utf-8",
      timeout: 10000,
    });
    const names = JSON.parse(ir).source_map.files.map((f: any) => f.name);
    // bloom/menu.ptl and petal/menu.ptl would be indistinguishable as bare
    // file names; motion.ptl was imported flat and keeps its bare name.
    expect(names).toContain("bloom/menu.ptl");
    expect(names).toContain("petal/menu.ptl");
  });

  it("a flat import is unchanged", () => {
    const out = execSync(
      `${PETAL} run -e 'import palette\nprint(palette.colors.bg)' -I ${resolve(
        __dirname,
        "fixtures/modules"
      )}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("2");
  });
});
