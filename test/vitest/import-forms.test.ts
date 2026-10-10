// The import forms and where an import path leads (docs/module-system.md).
//
// fixtures/import-forms/ holds a small game (two modules that both export
// `draw`, as game modules do), a file shared from outside it, and a package
// `kit` whose modules reach each other both relatively and by package name.

import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { petalCapture } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/import-forms");
const game = (name: string) => resolve(FIXTURES, "game", name);
const kit = (name: string) => resolve(FIXTURES, "kit/examples", name);

describe("import forms", () => {
  it("a plain import binds every export bare and the qualifier; `as` renames it", () => {
    const r = petalCapture(["run", game("main.ptl")]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim().split("\n")).toEqual([
      "hud",
      "level",
      "only hud has this",
      "hud panel 7",
      "hud panel 7",
      "7",
    ]);
    // Both modules export `draw`; nothing used it bare, so nothing is said.
    expect(r.stderr).toBe("");
  });

  it("a name two plain imports offer is an error at each bare use, not at the import", () => {
    for (const command of ["run", "check"]) {
      const r = petalCapture([command, game("ambiguous.ptl")]);
      expect(r.code).not.toBe(0);
      expect(r.stderr).toContain("`draw` is ambiguous: 'hud' and 'level' export it");
      expect(r.stderr).toContain("write `hud.draw` or `level.draw`");
      expect(r.stderr).toContain("pin one with `import ./hud: draw`");
      expect(r.stderr).toContain("[line 5, column 7]");
    }
  });

  it("a selective import pins an ambiguous name", () => {
    const r = petalCapture(["run", game("pinned.ptl")]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim()).toBe("level\nhud");
  });

  it("disjoint overload sets from two plain imports merge, with a warning per call", () => {
    const r = petalCapture(["check", game("merged.ptl")]);
    expect(r.code).toBe(0);
    const warnings = r.stderr.split("\n").filter((l) => l.startsWith("warning:"));
    expect(warnings.length).toBe(2);
    expect(warnings[0]).toContain(
      "`score` is exported by 'hud' and 'level' with different parameter counts"
    );
    const run = petalCapture(["run", game("merged.ptl")]);
    expect(run.stdout.trim()).toBe("score 1\nscore 3");
  });

  it("a plain import that hides a builtin warns once, where the name is used", () => {
    const r = petalCapture(["check", game("shadow_builtin.ptl")]);
    expect(r.code).toBe(0);
    const warnings = r.stderr.split("\n").filter((l) => l.startsWith("warning:"));
    expect(warnings.length).toBe(1);
    expect(warnings[0]).toContain("which hides the builtin `clamp`");
    expect(warnings[0]).toContain("`import ./tween: clamp`");
    expect(r.stderr).toContain("[line 3, column 7]");
  });

  it("`import m: *` is an error that names the plain form", () => {
    // Inline, so that no file in the tree is written the old way.
    const r = petalCapture(
      ["run", "-e", "import ./hud: *\nprint(draw())"],
      undefined,
      resolve(FIXTURES, "game")
    );
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("`import ./hud: *` is no longer a form");
    expect(r.stderr).toContain("petal fix-imports");
  });
});

describe("import sources and module identity", () => {
  it("a package file is one module, reached relatively or by package name", () => {
    // kit/src/menu.ptl says `import ./counter`; the demo says
    // `import kit/counter`. One module: its top level runs once and the
    // counter is shared. No -I: the nearest petal.toml names the package.
    const r = petalCapture(["run", kit("demo.ptl")]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim().split("\n")).toEqual(["counter loaded", "menu 1", "2", "3"]);
  });

  it("a multi-file program's IR names each module canonically", () => {
    const r = petalCapture(["show-ir", "--json", kit("demo.ptl")]);
    const names = JSON.parse(r.stdout).source_map.files.map((f: any) => f.name).sort();
    expect(names).toEqual(["demo.ptl", "kit.ptl", "kit/counter.ptl", "kit/menu.ptl"]);
  });

  it("a relative import may not leave its package", () => {
    const r = petalCapture(["run", kit("escape_demo.ptl")]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("cannot find module '../../shared/rng'");
    expect(r.stderr).toContain("leaves the package 'kit'");
  });

  it("the search path offers packages, and a host registration is not needed in one", () => {
    const r = petalCapture(["run", "-e", "import kit\nprint(open())", "-I", FIXTURES]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim()).toBe("counter loaded\nmenu 1");
  });

  it("a bundled package is found with nothing on the search path", () => {
    const r = petalCapture([
      "run",
      "-e",
      "import core\nfn len(x)\n  \"mine\"\nend\nprint(len([1]))\nprint(core.len([1, 2]))",
    ]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim()).toBe("mine\n2");
  });
});
