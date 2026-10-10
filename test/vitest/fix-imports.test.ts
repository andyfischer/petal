// `petal fix-imports`: the migration from the old import rules
// (docs/module-system.md#migrating-with-petal-fix-imports).
//
// Each test writes a program the old way — sibling files imported by bare
// name, `: *`, a package's flat internal import — into a scratch directory,
// since the command rewrites in place. (The old-style sources live here and
// not under fixtures/, so that a `petal fix-imports` over the repository has
// nothing of ours to "fix".)

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, realpathSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { dirname, join } from "path";
import { petalCapture } from "./helpers";

const OLD: Record<string, string> = {
  "main.ptl": [
    "// An old-style program.",
    "import level                 // a sibling",
    "import level: goal",
    "import helpers/math as m",
    "import kit                   // a package: left alone",
    "import kit/menu: open",
    "",
    "print(level.name)",
    "print(goal)",
    "print(m.double(2))",
    "print(open())",
    "",
  ].join("\n"),
  "level.ptl": 'pub import parts: *\n\npub let name = "one"\npub let goal = 3\n',
  "parts.ptl": 'pub let wall = "#"\n',
  "helpers/math.ptl": "pub fn double(x)\n  x * 2\nend\n",
  "libs/kit/petal.toml": '[package]\nname = "kit"\n',
  "libs/kit/src/kit.ptl": "pub import kit/menu: *\n",
  "libs/kit/src/menu.ptl": "import label\n\npub fn open()\n  label.text()\nend\n",
  "libs/kit/src/label.ptl": 'pub fn text()\n  "open"\nend\n',
};

let dir: string;
beforeEach(() => {
  dir = realpathSync(mkdtempSync(join(tmpdir(), "petal-fix-imports-")));
  for (const [rel, text] of Object.entries(OLD)) {
    mkdirSync(dirname(join(dir, rel)), { recursive: true });
    writeFileSync(join(dir, rel), text);
  }
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const read = (rel: string) => readFileSync(join(dir, rel), "utf-8");

describe("petal fix-imports", () => {
  it("the old program does not load until it is migrated", () => {
    const r = petalCapture(["run", join(dir, "main.ptl"), "-I", join(dir, "libs")]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("there is no package named 'level'");
    expect(r.stderr).toContain("petal fix-imports");
  });

  it("rewrites sibling imports, star imports, and leaves package paths alone", () => {
    const r = petalCapture(["fix-imports", "-I", join(dir, "libs"), dir]);
    expect(r.code).toBe(0);
    expect(read("main.ptl")).toBe(
      [
        "// An old-style program.",
        "import ./level                 // a sibling",
        "import ./level: goal",
        "import ./helpers/math as m",
        "import kit                   // a package: left alone",
        "import kit/menu: open",
        "",
        "print(level.name)",
        "print(goal)",
        "print(m.double(2))",
        "print(open())",
        "",
      ].join("\n")
    );
    expect(read("level.ptl").split("\n")[0]).toBe("pub import ./parts");
    // Inside the package: the flat sibling import and the star.
    expect(read("libs/kit/src/menu.ptl").split("\n")[0]).toBe("import ./label");
    expect(read("libs/kit/src/kit.ptl").trim()).toBe("pub import kit/menu");
    // It says what it did.
    expect(r.stdout).toContain("`import level` -> `import ./level`");
    expect(r.stdout).toContain("`import parts: *` -> `import parts`");

    const run = petalCapture(["run", join(dir, "main.ptl"), "-I", join(dir, "libs")]);
    expect(run.code).toBe(0);
    expect(run.stdout.trim()).toBe("one\n3\n4\nopen");
  });

  it("is idempotent", () => {
    petalCapture(["fix-imports", "-I", join(dir, "libs"), dir]);
    const once = ["main.ptl", "level.ptl", "libs/kit/src/menu.ptl"].map(read);
    const again = petalCapture(["fix-imports", "-I", join(dir, "libs"), dir]);
    expect(again.code).toBe(0);
    expect(again.stdout).toBe("");
    expect(["main.ptl", "level.ptl", "libs/kit/src/menu.ptl"].map(read)).toEqual(once);
    expect(petalCapture(["fix-imports", "--check", "-I", join(dir, "libs"), dir]).code).toBe(0);
  });

  it("--check lists what would change and writes nothing", () => {
    const before = read("main.ptl");
    const r = petalCapture(["fix-imports", "--check", "-I", join(dir, "libs"), dir]);
    expect(r.code).toBe(1);
    const listed = r.stdout.trim().split("\n").sort();
    expect(listed).toEqual(
      ["level.ptl", "libs/kit/src/kit.ptl", "libs/kit/src/menu.ptl", "main.ptl"]
        .map((f) => join(dir, f))
        .sort()
    );
    expect(r.stderr).toContain("4 of 7 file(s) need their imports fixed");
    expect(read("main.ptl")).toBe(before);
  });

  it("--diff prints the rewrite as a unified diff", () => {
    const r = petalCapture(["fix-imports", "--diff", join(dir, "level.ptl")]);
    expect(r.code).toBe(1);
    expect(r.stdout).toContain("-pub import parts: *");
    expect(r.stdout).toContain("+pub import ./parts");
  });

  it("adds the import a bare name needs when one package exports it", () => {
    // `open` is exported by the package `kit` and nothing in the file
    // provides it. Until the package itself is migrated it cannot be read,
    // and the command says so rather than staying silent.
    const early = petalCapture(
      ["fix-imports", "-I", join(dir, "libs"), "-e", "print(open())\n"],
      undefined,
      dir
    );
    expect(early.stdout).toBe("print(open())\n");
    expect(early.stderr).toContain("`import kit` does not load");

    petalCapture(["fix-imports", join(dir, "libs")]);
    const r = petalCapture(
      ["fix-imports", "-I", join(dir, "libs"), "-e", "// header\n\nprint(open())\n"],
      undefined,
      dir
    );
    expect(r.stdout).toBe("// header\n\nimport kit\n\nprint(open())\n");
  });

  it("adds the import of a builtin that moved into a package, method syntax included", () => {
    // `lerp`, `vec2` and friends are declared by bundled packages now; a
    // script written when they were global says nothing about them.
    const src = "let v = vec2(3, 4)\nprint(lerp(0, 10, 0.5), v.mag())\nprint(clamp(2, 0, 1))\n";
    const before = petalCapture(["run", "-e", src]);
    expect(before.code).not.toBe(0);
    expect(before.stderr).toContain("add `import vec2`");

    const r = petalCapture(["fix-imports", "-e", src], undefined, dir);
    expect(r.stdout).toBe(`import creative\nimport vec2\n\n${src}`);
    expect(petalCapture(["run", "-e", r.stdout]).stdout).toBe("5.0 5.0\n1\n");
    // Idempotent.
    expect(petalCapture(["fix-imports", "-e", r.stdout], undefined, dir).stdout).toBe(r.stdout);

    // Only method syntax, which compiles either way and fails when it runs.
    const m = petalCapture(["fix-imports", "-e", "print([3, 4].mag())\n"], undefined, dir);
    expect(m.stdout).toBe("import creative\n\nprint([3, 4].mag())\n");
  });

  it("-e rewrites a star import with no file to resolve against", () => {
    const r = petalCapture(["fix-imports", "-e", "pub import kit: *\n", "-I", join(dir, "libs")]);
    expect(r.stdout).toBe("pub import kit\n");
  });
});
