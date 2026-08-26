#!/usr/bin/env node
//
// Functional integration test for the gpp-test-app fixture (GPP v2).
//
// gpp-test-app exists to put a pane into a chosen situation on demand; this
// harness proves each situation actually arises end to end — the whole GPP
// pipeline (spawn, v2 handshake, setScript, query round-trip, error paths)
// stands between the layout's `process(gpp-test-app, [<mode>])` node and the
// assertions:
//
//   ok            — a healthy panel that paints and keeps running.
//   query-error   — a failed query surfaces via error_of (soft/async error),
//                   with NO frame error.
//   runtime-error — a frame error raises the panel error card.
//
// (Launched via a layout script rather than `--subprocess` because the
// launcher appends `--debug-port` after the args, and everything after
// `--subprocess <app>` belongs to the app.)
//
// Usage:  node tools/gpp-test-app-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, GARDEN_DIR, launchGarden, type LaunchedApp } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { makeWorkDir, removeOnExit, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const work = await makeWorkDir("garden-gpp-test-app-it");
removeOnExit(work);
const checks = new Checks();

// The fixture binary is addressed absolutely in the layout, so build it (and
// the host) here — a stale binary would fail as if the protocol broke.
await cargoBuild(["garden-app", "gpp-test-app"]);
const appBin = join(GARDEN_DIR, "target", "debug", "gpp-test-app");

/** Boot garden headless with gpp-test-app in the requested mode as pane 0. */
async function launchMode(appMode: string): Promise<LaunchedApp> {
  const init = join(work, `${appMode}.ptl`);
  await writeFile(init, `layout(process("${appBin}", ["${appMode}"]))\n`);
  return launchGarden({
    args: [...mode, "--init", init],
    logPath: join(work, `${appMode}.log`),
    label: `gpp-test-app ${appMode}`,
  });
}

// --- mode ok: a healthy panel ------------------------------------------------
{
  console.log("mode ok...");
  const app = await launchMode("ok");
  const g = app.client;
  await waitUntil(() => g.panelValue("mode"), (v) => v === "ok", { tries: 100, intervalMs: 50 });

  checks.check("the pane is a panel", (await g.pane()).kind, "panel");
  checks.checkContains(
    "the client is gpp-test-app",
    String((await g.pane()).panel?.client ?? ""),
    "gpp-test-app",
  );
  checks.check("the ok drawer is live", await g.panelValue("mode"), "ok");
  checks.check("a healthy panel has no error", (await g.pane()).panel?.error ?? null, null);
  checks.check("nothing on screen reads as an error", await g.sceneErrorCount(), 0);

  // The panel is actually running: /tick advances its frame counter.
  const before = Number(await g.panelValue("frame"));
  await g.tick(3);
  checks.checkGe("frames advance under /tick", Number(await g.panelValue("frame")), before + 1);
  app.kill();
}

// --- mode query-error: the soft/async error path -----------------------------
{
  console.log("mode query-error...");
  const app = await launchMode("query-error");
  const g = app.client;
  const errored = await waitUntil(() => g.panelValue("errored"), (v) => v === true, {
    tries: 200,
    intervalMs: 50,
  });

  checks.check("the failed query surfaces via error_of", errored, true);
  checks.checkContains("the drawer shows the provider's message", String(await g.panelValue("msg")), "boom");
  // The soft path is the point: the frame itself kept running.
  checks.check("a failed query is NOT a frame error", (await g.pane()).panel?.error ?? null, null);
  app.kill();
}

// --- mode runtime-error: the error card --------------------------------------
{
  console.log("mode runtime-error...");
  const app = await launchMode("runtime-error");
  const g = app.client;
  const err = await waitUntil(async () => (await g.pane()).panel?.error ?? null, (e) => e !== null, {
    tries: 200,
    intervalMs: 50,
  });

  checks.checkContains("the frame error is reported", String(err), "Cannot get length of nil");
  checks.checkGe("the error card is drawn", await g.sceneErrorCount(), 1);
  app.kill();
}

// --- report ------------------------------------------------------------------
process.exit(checks.report());
