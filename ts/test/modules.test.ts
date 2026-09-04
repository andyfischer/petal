// Multi-file module system tests (docs/module-system.md).
//
// The first multi-.ptl cases in the harness: fixtures/modules/ holds a shared
// palette module imported by two sibling entry scripts. Every case runs the
// compiled CLI on real files so importer-relative resolution is exercised.

import { describe, it, expect } from "vitest";
import { execSync } from "child_process";
import { resolve } from "path";
import { PETAL, petalCapture, runPetalFile } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/modules");

/** Run `petal <args>` (argv form, no shell) expecting failure; return stderr. */
function runFileError(args: string[]): string {
  const r = petalCapture(args);
  if (r.code === 0) throw new Error("Expected petal to fail but it succeeded");
  return r.stderr.trim();
}


describe("module imports across files", () => {
  it("panel.ptl: qualified + selective import", () => {
    const out = runPetalFile(resolve(FIXTURES, "panel.ptl"));
    expect(out).toBe("15\n255\n1");
  });

  it("detail.ptl: aliased import used inside a fn", () => {
    const out = runPetalFile(resolve(FIXTURES, "detail.ptl"));
    expect(out).toBe("<11>\n9");
  });

  it("private members are not importable", () => {
    const err = runFileError(["run", "-e", "import palette: _clamp", "-I", FIXTURES]);
    expect(err).toContain("no export '_clamp'");
  });

  it("-I adds a module search directory", () => {
    const out = execSync(
      `${PETAL} run -e 'import palette\nprint(palette.colors.bg)' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("2");
  });

  it("missing module reports a compile error", () => {
    const err = runFileError(["run", "-e", "import missing_module"]);
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

  it("an exported `var` reads as its contents under an alias", () => {
    const out = execSync(
      `${PETAL} run -e 'import tally\nprint(tally.hits, type(tally.hits))\ntally.bump()\nprint(tally.hits + 1)' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("0 int\n2");
  });

  it("an exported `var` reads as its contents under a selective import", () => {
    // The bare name binds the term that holds the cell, so losing the `var`
    // kind here would forward the raw cell to every read (`<cell 0>`) and
    // break the containment invariant.
    const out = execSync(
      `${PETAL} run -e 'import tally: hits, bump\nprint(hits, type(hits))\nbump()\nprint(hits + 1)' -I ${FIXTURES}`,
      { encoding: "utf-8", timeout: 10000 }
    ).trim();
    expect(out).toBe("0 int\n2");
  });

  it("only the declaring module may `set` an exported `var`", () => {
    const err = runFileError(["run", "-e", "import tally: hits\nset hits = 5", "-I", FIXTURES]);
    expect(err).toContain("exported by module `tally`");
    expect(err).toContain("only `tally` can write it");
  });

  it("runtime errors in a module name the module file", () => {
    const err = runFileError(["run", "-e", "import palette\nprint(palette.colors + 1)", "-I", FIXTURES]);
    // The failing add is in the entry file, so entry format; the provenance
    // of `colors` points into palette.ptl.
    expect(err).toContain("palette.ptl");
  });
});

describe("a selective import list may wrap across lines", () => {
  /** Run `petal run -e <code> -I fixtures/modules`, returning trimmed stdout. */
  function runSnippet(code: string): string {
    const r = petalCapture(["run", "-e", code, "-I", FIXTURES]);
    if (r.code !== 0) throw new Error(r.stderr);
    return r.stdout.trim();
  }

  it("breaks after a comma", () => {
    const out = runSnippet(
      "import palette: colors,\n                brighten\nprint(brighten(colors.fg))"
    );
    expect(out).toBe("25");
  });

  it("breaks after the colon", () => {
    const out = runSnippet("import palette:\n  colors,\n  brighten\nprint(brighten(colors.bg))");
    expect(out).toBe("12");
  });

  it("allows a trailing comma", () => {
    const out = runSnippet("import palette: colors, brighten,\nprint(colors.accent)");
    expect(out).toBe("9");
  });

  it("a trailing comma does not swallow the next statement", () => {
    // `print` is an identifier, but what follows it is a `(` rather than a
    // comma or a line end, so the list ended at the trailing comma.
    const out = runSnippet("import palette: colors,\nprint(colors.fg)");
    expect(out).toBe("15");
  });

  it("`import m` is still not continued by the next line", () => {
    const out = runSnippet("import palette\nbrighten = 1\nprint(brighten)");
    expect(out).toBe("1");
  });

  it("imports-come-first still applies to a wrapped import", () => {
    const err = runFileError([
      "run",
      "-e",
      "import palette: colors,\n                brighten\nlet x = 1\nimport tally",
      "-I",
      FIXTURES,
    ]);
    expect(err).toContain("import statements must appear before any other statement");
  });

  it("a wrapped import survives a format round-trip", () => {
    const src = "import palette: colors,\n                brighten\nprint(brighten(colors.fg))\n";
    const r = petalCapture(["lint", "-e", src]);
    if (r.code !== 0) throw new Error(r.stderr);
    const formatted = r.stdout;
    const again = petalCapture(["run", "-e", formatted, "-I", FIXTURES]);
    if (again.code !== 0) throw new Error(again.stderr);
    expect(again.stdout.trim()).toBe("25");
  });
});
