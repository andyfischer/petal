#!/usr/bin/env node
//
// Functional integration test for the Python GPP apps (gpp-python/).
//
// Proves the whole pipeline end to end with a non-Rust client: garden spawns
// `python3 <app>.py` from a layout `process(...)` node, the v2 handshake and
// setScript land, the drawer runs in the host's panel runtime, and query
// answers flow back over the pipe into painted frames:
//
//   library   — gpp.py's own unit tests (in-memory serve_on streams).
//   sysmon    — the process monitor: live ps-aux data reaches the table,
//               frames advance, a screenshot renders.
//   repo-stats — the git dashboard, launched on this repo: commit/author
//               counts arrive; a non-repo directory surfaces a clean error.
//
// Usage:  node tools/python-gpp-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, GARDEN_DIR, launchGarden, type LaunchedApp } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { makeWorkDir, removeOnExit, runOrDie, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const PY_DIR = join(GARDEN_DIR, "gpp-python");
const SYSMON = join(PY_DIR, "sysmon", "app.py");
const REPO_STATS = join(PY_DIR, "repo-stats", "app.py");

const work = await makeWorkDir("garden-python-gpp-it");
removeOnExit(work);
const checks = new Checks();

// The Python library's own tests are cheap and catch protocol drift before
// any host is involved.
console.log("gpp.py unit tests...");
await runOrDie("python3", [join(PY_DIR, "test_gpp.py")], {
  cwd: PY_DIR,
  message: "gpp.py unit tests failed",
});

// Only the host needs building — the clients are Python.
await cargoBuild(["garden-app"]);

/** Boot garden headless with `python3 app args...` as pane 0. */
async function launchPython(label: string, app: string, args: string[] = []): Promise<LaunchedApp> {
  const init = join(work, `${label}.ptl`);
  const argList = [app, ...args].map((a) => `"${a}"`).join(", ");
  await writeFile(init, `layout(process("python3", [${argList}]))\n`);
  return launchGarden({
    args: [...mode, "--init", init],
    logPath: join(work, `${label}.log`),
    label,
  });
}

// --- sysmon: live data in a rendered panel -----------------------------------
{
  console.log("sysmon...");
  const app = await launchPython("sysmon", SYSMON);
  const g = app.client;
  const ready = await waitUntil(() => g.panelValue("ready"), (v) => v === true, {
    tries: 200,
    intervalMs: 50,
  });

  checks.check("the pane is a panel", (await g.pane()).kind, "panel");
  checks.checkContains(
    "the client is the python subprocess",
    String((await g.pane()).panel?.client ?? ""),
    "python3",
  );
  checks.check("the procs query resolved", ready, true);
  checks.checkGt("processes reached the drawer", Number(await g.panelValue("proc_count")), 10);
  checks.checkGt("the table has rows", Number(await g.panelValue("row_count")), 10);
  checks.check("the default sort is cpu-descending", await g.panelValue("sort_spec"), "cpu:desc");
  checks.check("a healthy panel has no error", (await g.pane()).panel?.error ?? null, null);
  checks.check("nothing on screen reads as an error", await g.sceneErrorCount(), 0);

  // The panel is actually running: /tick advances its frame counter.
  const before = Number((await g.pane()).panel?.frame ?? 0);
  await g.tick(3);
  checks.checkGe(
    "frames advance under /tick",
    Number((await g.pane()).panel?.frame ?? 0),
    before + 1,
  );

  // The rendered pane actually rasterizes: a real PNG of non-trivial size.
  const shot = join(work, "sysmon.png");
  await g.screenshot(shot);
  const png = await readFile(shot);
  checks.check(
    "the screenshot is a PNG",
    png.subarray(0, 4).toString("latin1"),
    "\x89PNG",
  );
  checks.checkGt("the screenshot is not blank-tiny", png.length, 4000);
  app.kill();
}

// --- repo-stats: the git dashboard on this repo ------------------------------
{
  console.log("repo-stats...");
  const app = await launchPython("repo-stats", REPO_STATS, [GARDEN_DIR]);
  const g = app.client;
  const ready = await waitUntil(() => g.panelValue("ready"), (v) => v === true, {
    tries: 200,
    intervalMs: 50,
  });

  checks.check("the stats query resolved", ready, true);
  checks.checkGt("commits were counted", Number(await g.panelValue("commit_count")), 0);
  checks.checkGt("authors were tallied", Number(await g.panelValue("author_count")), 0);
  checks.check("26 weekly buckets arrived", await g.panelValue("week_count"), 26);
  checks.check("nothing on screen reads as an error", await g.sceneErrorCount(), 0);
  app.kill();
}

// --- repo-stats on a non-repo: the soft error path ---------------------------
{
  console.log("repo-stats (not a repo)...");
  const app = await launchPython("repo-stats-bad", REPO_STATS, [work]);
  const g = app.client;
  // The AppError from the Python handler surfaces through load_poll as a
  // failed load — the drawer keeps running (no frame error) and shows it.
  const ready = await waitUntil(
    async () => (await g.pane()).panel?.values?.ready,
    (v) => v === false,
    { tries: 200, intervalMs: 50 },
  );
  checks.check("the query failed softly", ready, false);
  checks.check("a failed query is NOT a frame error", (await g.pane()).panel?.error ?? null, null);
  const texts = (await g.sceneVisibleTexts()).map((t) => t.text ?? "").join("\n");
  checks.checkContains("the drawer shows the provider's message", texts, "not a git repo");
  app.kill();
}

// --- report ------------------------------------------------------------------
process.exit(checks.report());
