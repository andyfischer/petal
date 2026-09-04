// Packages: a `petal.toml` manifest makes a directory of modules a library
// with a name (docs/module-system.md#packages, rust/src/package.rs).
//
// fixtures/packages/libs holds two of them — `bloom` (in a directory of the
// same name, with a modules = "src" and a nested module) and `widgets` (in a
// directory called widget_kit, so the manifest name is doing real work).
// fixtures/packages/broken holds a manifest that does not parse.
//
// The embedder API (Env::add_package / register_package) is covered by
// rust/tests/packages.rs; this file covers what the CLI does with -I.

import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { spawnSync } from "child_process";
import { PETAL, petalCapture } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/packages");
const LIBS = resolve(FIXTURES, "libs");

/** Run `petal <args>`, expecting success; return trimmed stdout. */
function run(args: string[]): string {
  const r = petalCapture(args);
  if (r.code !== 0) throw new Error(`petal failed: ${r.stderr}`);
  return r.stdout.trim();
}

/** Run `petal <args>`, expecting failure; return trimmed stderr. */
function runError(args: string[]): string {
  const r = petalCapture(args);
  if (r.code === 0) throw new Error("Expected petal to fail but it succeeded");
  return r.stderr.trim();
}

describe("packages", () => {
  it("a package's modules are importable as name/module", () => {
    // app.ptl: import bloom/menu, bloom/widgets/button, widgets/panel.
    const out = run(["run", resolve(FIXTURES, "app.ptl"), "-I", LIBS]);
    expect(out).toBe("bloom open eased\nbutton eased\nwidgets panel");
  });

  it("the manifest name, not the directory name, is the package name", () => {
    // The library lives in libs/widget_kit and is named `widgets`.
    const out = run([
      "run",
      "-e",
      "import widgets/panel\nprint(panel.draw())",
      "-I",
      LIBS,
    ]);
    expect(out).toBe("widgets panel");
    expect(runError(["run", "-e", "import widget_kit/panel", "-I", LIBS])).toContain(
      "cannot find module 'widget_kit/panel'"
    );
  });

  it("-I may point at the library root itself, not only its parent", () => {
    const out = run([
      "run",
      "-e",
      "import bloom/menu\nprint(menu.close())",
      "-I",
      resolve(LIBS, "bloom"),
    ]);
    expect(out).toBe("bloom close");
  });

  it("a library's internal imports keep working with flat names", () => {
    // bloom/src/menu.ptl says `import motion`, its sibling in the package.
    const out = run([
      "run",
      "-e",
      "import bloom/menu\nprint(menu.open())",
      "-I",
      LIBS,
    ]);
    expect(out).toBe("bloom open eased");
  });

  it("and with the package-qualified spelling", () => {
    // bloom/src/widgets/button.ptl says `import bloom/motion`.
    const out = run([
      "run",
      "-e",
      "import bloom/widgets/button\nprint(button.label())",
      "-I",
      LIBS,
    ]);
    expect(out).toBe("button eased");
  });

  it("a selective import reaches into a package", () => {
    const out = run(["run", resolve(FIXTURES, "flat_internal.ptl"), "-I", LIBS]);
    expect(out).toBe("bloom open eased\nbloom close");
  });

  it("a module named like the package is its facade", () => {
    // bloom/src/bloom.ptl, reached by a bare `import bloom`.
    const out = run(["run", "-e", "import bloom\nprint(bloom.open())", "-I", LIBS]);
    expect(out).toBe("bloom open eased");
  });

  it("a malformed manifest is a clear error naming the file", () => {
    const err = runError(["run", "-e", "print(1)", "-I", resolve(FIXTURES, "broken")]);
    expect(err).toContain("petal.toml");
    expect(err).toContain("line 2");
    expect(err).toContain("must be a quoted string");
  });

  it("a missing module of a real package says which module", () => {
    const err = runError(["run", "-e", "import bloom/nope", "-I", LIBS]);
    expect(err).toContain("cannot find module 'bloom/nope'");
  });

  it("an import path can't climb out of a package", () => {
    const err = runError(["run", "-e", "import bloom/../secrets", "-I", LIBS]);
    expect(err).toContain("not allowed in an import path");
  });

  it("PETAL_PATH picks up packages too", () => {
    const r = spawnSync(PETAL, ["run", "-e", "import bloom/menu\nprint(menu.close())"], {
      encoding: "utf-8",
      timeout: 10000,
      env: { ...process.env, PETAL_PATH: LIBS },
    });
    expect(r.status).toBe(0);
    expect((r.stdout || "").trim()).toBe("bloom close");
  });
});

describe("petal packages", () => {
  it("lists what the search path makes available", () => {
    const out = run(["packages", "-I", LIBS]);
    expect(out).toContain("bloom 0.1.0");
    expect(out).toContain("widgets 2.3.1");
    expect(out).toContain("bloom/widgets/button");
    expect(out).toContain("widgets/panel");
  });

  it("--json reports the manifest facts and the module list", () => {
    const parsed = JSON.parse(run(["packages", "--json", "-I", LIBS]));
    const bloom = parsed.packages.find((p: any) => p.name === "bloom");
    expect(bloom.version).toBe("0.1.0");
    expect(bloom.modules).toEqual(["bloom", "menu", "motion", "widgets/button"]);
    expect(bloom.module_dir.endsWith("libs/bloom/src")).toBe(true);
    const widgets = parsed.packages.find((p: any) => p.name === "widgets");
    expect(widgets.version).toBe("2.3.1");
    expect(widgets.modules).toEqual(["panel"]);
  });

  it("says so when there is nothing to list", () => {
    expect(run(["packages"])).toContain("No packages found");
  });

  it("reports a manifest on PETAL_PATH that would not load", () => {
    // A broken manifest on PETAL_PATH is deliberately not fatal — it is the
    // machine's ambient setting, not this command's argument — but it used to
    // be swallowed whole, so the library was simply absent with no reason
    // given. `packages` is the command whose job is to explain that.
    const r = spawnSync(PETAL, ["packages"], {
      encoding: "utf-8",
      timeout: 10000,
      env: { ...process.env, PETAL_PATH: resolve(FIXTURES, "broken") },
    });
    expect(r.status).toBe(0);
    expect(r.stderr).toContain("petal.toml");
    expect(r.stderr).toContain("must be a quoted string");
    expect(r.stdout).toContain("No packages found");
  });

  it("--json carries those errors too", () => {
    const r = spawnSync(PETAL, ["packages", "--json"], {
      encoding: "utf-8",
      timeout: 10000,
      env: { ...process.env, PETAL_PATH: resolve(FIXTURES, "broken") },
    });
    const parsed = JSON.parse(r.stdout);
    expect(parsed.packages).toEqual([]);
    expect(parsed.errors.length).toBe(1);
    expect(parsed.errors[0]).toContain("must be a quoted string");
  });
});
