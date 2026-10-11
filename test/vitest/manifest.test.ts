// The manifest: [dependencies], [requires], and discovery by walking up from
// the file (docs/module-system.md#packages, core/src/package.rs).
//
// fixtures/manifest/app is a project `marble` whose petal.toml names a path
// dependency under another name (`tw` → the package `tween`, which in turn
// depends on `base`), a git dependency, a registry version, and a required
// host interface. No test here passes -I: the manifest is found from the
// script's path.
//
// The Rust side (Env::provides, resolution, the language server) is covered
// by core/tests/manifest.rs; this file covers the CLI.

import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { spawnSync } from "child_process";
import { PETAL, petalCapture } from "./helpers";

const APP = resolve(__dirname, "fixtures/manifest/app");

describe("manifest dependencies", () => {
  it("check resolves a path dependency by its key with no flag", () => {
    const r = petalCapture(["check", resolve(APP, "main.ptl")]);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });

  it("finds the manifest from a file below the project root", () => {
    const r = petalCapture(["check", resolve(APP, "screens/menu.ptl")]);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });

  it("-e code run inside the project sees its dependencies", () => {
    const r = spawnSync(PETAL, ["check", "-e", "import tw\nprint(go())"], {
      encoding: "utf-8",
      cwd: APP,
    });
    expect(r.stderr).toBe("");
    expect(r.status).toBe(0);
  });

  it("a git dependency that is not installed is a clear error where it is imported", () => {
    // PETAL_OFFLINE: the CLI would otherwise try to install it (fetching the
    // url). The package manager itself is covered by core/tests/pkgman.rs.
    const r = spawnSync(PETAL, ["check", resolve(APP, "uses_git.ptl")], {
      encoding: "utf-8",
      env: { ...process.env, PETAL_OFFLINE: "1" },
    });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("dependency 'shapes' of package 'marble' is a git dependency");
    expect(r.stderr).toContain("is not installed");
    expect(r.stderr).toContain("run `petal install`");
  });

  it("a registry version is a clear error where it is imported", () => {
    const r = petalCapture(["check", resolve(APP, "uses_registry.ptl")]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("registries are not supported yet");
  });
});

describe("required host interfaces", () => {
  it("check reports them and passes", () => {
    const r = petalCapture(["check", resolve(APP, "main.ptl")]);
    expect(r.code).toBe(0);
    expect(r.stdout.trim()).toBe(
      "requires host interfaces: audio ^1.0 (tween), gfx >=0.2 (marble)",
    );
  });

  it("only the packages a program loads count", () => {
    const r = petalCapture(["check", resolve(APP, "plain.ptl")]);
    expect(r.stdout.trim()).toBe("requires host interfaces: gfx >=0.2 (marble)");
  });

  it("check --json carries them", () => {
    const r = petalCapture(["check", "--json", resolve(APP, "main.ptl")]);
    const parsed = JSON.parse(r.stdout);
    expect(parsed.ok).toBe(true);
    expect(parsed.requires).toEqual([
      { interface: "audio", version: "^1.0", package: "tween" },
      { interface: "gfx", version: ">=0.2", package: "marble" },
    ]);
  });

  it("run refuses to load in a host that does not provide them", () => {
    const r = petalCapture(["run", resolve(APP, "main.ptl")]);
    expect(r.code).not.toBe(0);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain(
      "package 'marble' requires the host interface `gfx` >=0.2, which this host does not provide",
    );
    expect(r.stderr).toContain("package 'tween' requires the host interface `audio` ^1.0");
  });

  it("a bundled package states its own: bloom requires ui", () => {
    const check = petalCapture(["check", "-e", "import ui\nimport bloom\nprint(1)"]);
    expect(check.code).toBe(0);
    expect(check.stdout).toContain("ui >=0.1 (bloom)");
    const run = petalCapture(["run", "-e", "import ui\nimport bloom\nprint(1)"]);
    expect(run.code).not.toBe(0);
    expect(run.stderr).toContain("package 'bloom' requires the host interface `ui`");
    expect(run.stderr).toContain("petal-ui runner");
  });
});

describe("petal packages in a project", () => {
  it("shows the resolved graph and the required interfaces", () => {
    const r = petalCapture(["packages", APP]);
    expect(r.code).toBe(0);
    expect(r.stdout).toContain("project marble 0.1.0");
    expect(r.stdout).toContain("tw = tween 1.2.0  (path ../libs/tween)");
    expect(r.stdout).toContain("base 0.5.0  (path ../base)");
    expect(r.stdout).toMatch(/shapes {2}\(git https:\/\/example\.org\/shapes tag v1\.2\.0\) {2}UNRESOLVED/);
    expect(r.stdout).toMatch(/grid {2}\(registry 0\.3\) {2}UNRESOLVED/);
    expect(r.stdout).toContain("audio ^1.0  (package tween)");
    expect(r.stdout).toContain("gfx >=0.2  (package marble)");
    expect(r.stdout).toContain("bundled in this binary:");
    expect(r.stdout).toContain("bloom 0.1.0  (requires ui >=0.1)");
    expect(r.stdout).toContain("noise 0.1.0  (builtins)");
  });

  it("finds the project from a file, and from the working directory", () => {
    const fromFile = petalCapture(["packages", resolve(APP, "screens/menu.ptl")]);
    expect(fromFile.stdout).toContain("project marble 0.1.0");
    const fromCwd = spawnSync(PETAL, ["packages"], { encoding: "utf-8", cwd: APP });
    expect(fromCwd.stdout).toContain("project marble 0.1.0");
  });

  it("--json has the graph, the requirements and the bundled packages", () => {
    const parsed = JSON.parse(petalCapture(["packages", "--json", APP]).stdout);
    expect(parsed.project.name).toBe("marble");
    const byName = Object.fromEntries(parsed.project.dependencies.map((d: any) => [d.name, d]));
    expect(byName.tw.package).toBe("tween");
    expect(byName.tw.version).toBe("1.2.0");
    expect(byName.tw.requires).toEqual([{ interface: "audio", version: "^1.0" }]);
    expect(byName.tw.dependencies[0].package).toBe("base");
    expect(byName.shapes.error).toContain("git dependency");
    expect(byName.grid.error).toContain("registries are not supported yet");
    expect(parsed.project.required_interfaces).toEqual([
      { interface: "audio", version: "^1.0", package: "tween" },
      { interface: "gfx", version: ">=0.2", package: "marble" },
    ]);
    const bundled = parsed.bundled.map((p: any) => p.name);
    expect(bundled).toContain("ui");
    expect(parsed.bundled.find((p: any) => p.name === "io").builtin).toBe(true);
    expect(parsed.bundled.find((p: any) => p.name === "bloom").builtin).toBe(false);
  });

  it("outside a project there is none", () => {
    const parsed = JSON.parse(
      spawnSync(PETAL, ["packages", "--json"], { encoding: "utf-8", cwd: "/" }).stdout,
    );
    expect(parsed.project).toBe(null);
  });
});
