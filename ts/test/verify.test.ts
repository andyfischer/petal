// ts/bin/verify.ts — the refactor verifier. These drive the real script over a
// synthetic corpus in a temp dir, because what is being tested is the process
// contract: the verdict per file, the exit code, and whether the artifact
// bundle it leaves behind actually reproduces the diff it reported.
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { spawnSync } from "child_process";
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join, resolve } from "path";

const repoRoot = resolve(__dirname, "../..");
const VERIFY = join(repoRoot, "ts", "bin", "verify.ts");

let scratch: string;

beforeAll(() => {
  scratch = mkdtempSync(join(tmpdir(), "petal-verify-test-"));
});
afterAll(() => {
  rmSync(scratch, { recursive: true, force: true });
});

interface Result {
  rel: string;
  kind: string;
  verdict: string;
  detail: string;
  bundle?: string;
}

/**
 * Build a before/after pair of source trees plus a plan, run verify over them,
 * and return its exit code alongside the parsed per-file results.
 */
function verify(
  name: string,
  files: { path: string; before: string; after?: string }[],
  steps: unknown[] = [{ check: "compiles" }, { check: "run-diff", seeds: [1], frames: 5 }],
  extraArgs: string[] = [],
) {
  const base = join(scratch, name);
  const before = join(base, "before");
  const after = join(base, "after");
  const out = join(base, "out");
  for (const f of files) {
    for (const [root, text] of [[before, f.before], [after, f.after ?? f.before]] as const) {
      const dest = join(root, f.path);
      mkdirSync(join(dest, ".."), { recursive: true });
      writeFileSync(dest, text);
    }
  }
  const planPath = join(base, "plan.json");
  writeFileSync(
    planPath,
    JSON.stringify({ name, mode: "source", size: "320x240", corpus: ["."], steps }),
  );
  const r = spawnSync(
    VERIFY,
    ["--plan", planPath, "--before", before, "--after", after, "--out", out, "--jobs", "2", ...extraArgs],
    { encoding: "utf-8", timeout: 120_000 },
  );
  // A preflight failure (exit 2) aborts before any file is judged, so there is
  // no results.json to read — that is a valid outcome to assert on, not a crash.
  const resultsPath = join(out, "results.json");
  const results: Result[] = existsSync(resultsPath)
    ? JSON.parse(readFileSync(resultsPath, "utf-8"))
    : [];
  const byPath = new Map(results.map(x => [x.rel, x]));
  return { status: r.status, stdout: r.stdout, stderr: r.stderr, results, byPath, out };
}

const UI_APP = `state n = 0
n = n + 1
draw_rect(10, 10, 40, 20, 255, 0, 0)
print("n={n}")
`;

describe("verify.ts classification", () => {
  it("separates console, ui, module and unsupported by evidence", () => {
    const v = verify("classify", [
      { path: "console/plain.ptl", before: 'print("hi")\n' },
      { path: "app/main.ptl", before: 'import helper\nprint(helper.two())\n' },
      { path: "app/helper.ptl", before: "export fn two()\n  2\n" },
      { path: "ui/app.ptl", before: UI_APP },
      { path: "ui/layout.ptl", before: 'layout(panel("app.ptl"))\n' },
    ]);
    expect(v.byPath.get("console/plain.ptl")?.kind).toBe("console");
    expect(v.byPath.get("ui/app.ptl")?.kind).toBe("ui");
    // helper.ptl is imported by main.ptl, so it is covered through its importer.
    expect(v.byPath.get("app/helper.ptl")?.verdict).toBe("module");
    // No driver registers `panel`; the probe discovers that by running it.
    expect(v.byPath.get("ui/layout.ptl")?.verdict).toBe("unsupported");
    expect(v.byPath.get("ui/layout.ptl")?.detail).toContain("panel");
  });

  it("classifies a file beside a layout.ptl as ui even with no draw call", () => {
    const v = verify("layout-sibling", [
      { path: "panelapp/app.ptl", before: "state n = 0\nn = n + 1\n" },
      { path: "panelapp/layout.ptl", before: 'layout(panel("app.ptl"))\n' },
    ]);
    expect(v.byPath.get("panelapp/app.ptl")?.kind).toBe("ui");
  });
});

describe("verify.ts verdicts", () => {
  it("reports identical sources as identical-trace and exits 0", () => {
    const v = verify("identical", [
      { path: "a.ptl", before: 'print(1 + 1)\n' },
      { path: "ui.ptl", before: UI_APP },
    ]);
    expect(v.status).toBe(0);
    expect(v.results.map(r => r.verdict)).toEqual(["identical-trace", "identical-trace"]);
  });

  it("catches a changed constant, names the first divergence, and exits non-zero", () => {
    const v = verify("changed-console", [
      { path: "a.ptl", before: 'print("one")\nprint(41)\n', after: 'print("one")\nprint(42)\n' },
    ]);
    expect(v.status).toBe(1);
    const r = v.byPath.get("a.ptl")!;
    expect(r.verdict).toBe("changed");
    // The first line agrees; line 2 is where they part.
    expect(r.detail).toContain("line 2");
    expect(r.detail).toContain("41");
    expect(r.detail).toContain("42");
  });

  it("names the frame and JSON field path for a changed UI app", () => {
    const v = verify("changed-ui", [
      {
        path: "ui.ptl",
        before: UI_APP,
        after: UI_APP.replace("draw_rect(10, 10, 40, 20", "draw_rect(10, 10, 41, 20"),
      },
    ]);
    expect(v.status).toBe(1);
    const r = v.byPath.get("ui.ptl")!;
    expect(r.verdict).toBe("changed");
    expect(r.detail).toContain("frame 0");
    expect(r.detail).toContain("commands.0.w");
  });

  it("reports a differing compile outcome as compile-error", () => {
    const v = verify("compile", [
      { path: "a.ptl", before: 'print(1)\n', after: 'print(1\n' },
    ]);
    expect(v.status).toBe(1);
    expect(v.byPath.get("a.ptl")?.verdict).toBe("compile-error");
  });

  it("does not fail a file that is broken on both sides — nothing changed", () => {
    const v = verify("both-broken", [{ path: "a.ptl", before: 'print(1\n' }]);
    expect(v.status).toBe(0);
    expect(v.byPath.get("a.ptl")?.verdict).toBe("unsupported");
  });
});

// A driver that never launches produces the same empty output on both sides,
// which compares equal. Left unguarded that reads as `identical-trace` and
// exit 0 — the verifier reporting "nothing changed" precisely because it
// measured nothing. Both layers of the guard are pinned here.
describe("verify.ts driver failures are never a pass", () => {
  it("refuses to start when the corpus has UI apps and no UI driver is built", () => {
    const v = verify(
      "no-ui-bin",
      [{ path: "ui.ptl", before: UI_APP, after: UI_APP.replace("40, 20", "41, 20") }],
      [{ check: "compiles" }, { check: "run-diff", seeds: [1], frames: 5 }],
      ["--after-ui-bin", join(scratch, "does-not-exist")],
    );
    expect(v.status).toBe(2);
    expect(v.stderr).toContain("no petal-ui-run binary");
    expect(v.results).toEqual([]);
  });

  it("reports a UI driver that cannot be executed as driver-error, not identical", () => {
    // Exists (so it clears the preflight) but cannot be spawned: the failure
    // surfaces from the run-diff comparison itself.
    const dud = join(scratch, "dud-ui-run");
    writeFileSync(dud, "not a binary\n");
    chmodSync(dud, 0o644);
    const v = verify(
      "dud-ui-bin",
      [{ path: "ui.ptl", before: UI_APP, after: UI_APP.replace("40, 20", "41, 20") }],
      [{ check: "compiles" }, { check: "run-diff", seeds: [1], frames: 5 }],
      ["--before-ui-bin", dud],
    );
    expect(v.status).toBe(1);
    const r = v.byPath.get("ui.ptl")!;
    expect(r.verdict).toBe("driver-error");
    expect(r.detail).toContain("failed to launch");
  });
});

describe("verify.ts artifacts", () => {
  it("writes a repro.sh that runs standalone and reproduces the diff", () => {
    const v = verify("repro", [
      { path: "a.ptl", before: 'print(41)\n', after: 'print(42)\n' },
    ]);
    const bundle = v.byPath.get("a.ptl")!.bundle!;
    expect(readFileSync(join(bundle, "seed"), "utf-8")).toBe("1");

    // Run it from an unrelated cwd with no inherited context: the script has to
    // carry every path it needs.
    const r = spawnSync("sh", [join(bundle, "repro.sh")], {
      encoding: "utf-8",
      cwd: tmpdir(),
      env: { PATH: process.env.PATH ?? "" },
      timeout: 30_000,
    });
    expect(r.stdout).not.toContain("identical");
    expect(r.stdout).toContain("41");
    expect(r.stdout).toContain("42");
    expect(readFileSync(join(bundle, "before.out"), "utf-8")).toContain("41");
    expect(readFileSync(join(bundle, "after.out"), "utf-8")).toContain("42");
  });

  it("records the resolved plan and the corpus roots it skipped", () => {
    const v = verify("plan-record", [{ path: "a.ptl", before: "print(1)\n" }]);
    const plan = JSON.parse(readFileSync(join(v.out, "plan.json"), "utf-8"));
    expect(plan.resolved.mode).toBe("source");
    expect(plan.resolved.files).toBe(1);
  });
});
