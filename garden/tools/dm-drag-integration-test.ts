#!/usr/bin/env node
//
// Functional integration test for drag-to-edit v2 in Petal IDE (plan
// docs/dev/direct-manipulation-v2-plan.md §5, M4 acceptance item 3).
//
// Opens `garden petal-ide` on a sketch whose circles are placed by a helper
// function — a position v1 refuses to solve, because it flows through a call —
// and drives a Cmd-drag over the debug server. The v2 engine re-runs the frame
// with the helper's literals nudged, solves for the drag, verifies, and writes
// the answer into the editor. The checks: the buffer changed in the helper, the
// verified alternative is reported in `/state`, the saved file changed on disk,
// and the canvas re-rendered with the shape where the drag left it (traced by
// hovering, so no pixels are decoded). Then `:DragNext` / `:DragPrev` cycle the
// alternatives and `:DragAllow` restricts the next drag to one variable.
//
// Usage:  node tools/dm-drag-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { makeWorkDir, removeOnExit, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const work = await makeWorkDir("garden-dm-it");
removeOnExit(work);
const sketch = join(work, "sketch.ptl");
const logPath = join(work, "app.log");
const checks = new Checks();

// Circles at x = 40 + i * 30 (40, 70, 100); `gap` is the declared knob.
const SOURCE = [
  "import ui; import garden",
  "config let gap = 30",
  "let x0 = 40",
  "fn slot(i)",
  "  x0 + i * gap",
  "end",
  "clear(0, 0, 0)",
  "for i in range(0, 3) do",
  "  draw_circle(slot(i), 60, 8, 255, 255, 255)",
  "end",
  "",
].join("\n");
await writeFile(sketch, SOURCE);

await cargoBuild(["garden-app"]);

console.log("launching petal-ide with debug server...");
const app = await launchGarden({
  args: ["petal-ide", sketch, ...mode],
  requireFeatures: ["debug.batch", "debug.mouse-pane", "state.dm-drag"],
  logPath,
});
const g = app.client;

const EDITOR = 0;
const CANVAS = 1;

await waitUntil(
  async () => (await g.pane(CANVAS)).panel?.frame ?? 0,
  (f) => f >= 1,
);

/** The traced call under a pane-local canvas point: its 0-based line, or null. */
async function tracedLine(x: number, y: number): Promise<number | null> {
  await g.mousePaneLocal("move", x, y, CANVAS);
  const s = await g.state();
  const t = s.trace as { call?: { start?: { line?: number } } } | null | undefined;
  return t?.call?.start?.line ?? null;
}

/** Cmd-drag on the canvas from (x, y) by (dx, 0). */
async function cmdDrag(x: number, y: number, dx: number): Promise<void> {
  await g.gesturePaneLocal(
    [
      { op: "down", x, y, mods: ["cmd"] },
      { op: "move", x: x + dx / 2, y },
      { op: "move", x: x + dx, y },
      { op: "up", x: x + dx, y },
    ],
    CANVAS,
  );
}

console.log("running checks...");
checks.check("the third circle traces to its draw call", await tracedLine(100, 60), 8);

// Drag the third circle (slot(2) = 100) 12px right.
await cmdDrag(100, 60, 12);
const lines = await g.bufferLines(EDITOR);
checks.check("the drag moved the config knob (gap 30 → 36)", lines[1], "config let gap = 36");
checks.check("x0 is left alone", lines[2], "let x0 = 40");
const dm = (await g.state()).dm;
checks.check("the top alternative verified", dm?.last?.alternatives[0]?.verified, true);
checks.checkGe("alternatives are listed", dm?.last?.alternatives.length ?? 0, 2);
checks.checkContains(
  "the status bar names the edit",
  await g.statusNote(),
  "set gap to 36",
);

// The canvas re-rendered from the edited buffer: the circle is now at 112.
const moved = await waitUntil(() => tracedLine(112, 60), (l) => l === 8);
checks.check("the shape is where the drag left it", moved, 8);
checks.check("and not where it was", await tracedLine(101, 60), null);

// Saving writes the edit to disk.
await g.ex("w");
const saved = await readFile(sketch, "utf8");
checks.checkContains(":w persisted the edit", saved, "config let gap = 36");

// The next alternative replaces the first (written from the press-time text).
await g.ex("DragNext");
const next = await g.bufferLines(EDITOR);
checks.check("DragNext writes another alternative", next[1] !== "config let gap = 36" || next[2] !== "let x0 = 40", true);
await g.ex("DragPrev");
checks.check("DragPrev comes back", (await g.bufferLines(EDITOR))[1], "config let gap = 36");

// Client disambiguation: only `x0` may move now. Drag the first circle
// (slot(0) = x0 = 40) 10px right.
await g.ex("DragAllow x0");
checks.check("the allow-list is in /state", (await g.state()).dm?.allow, ["x0"]);
await cmdDrag(40, 60, 10);
const allowed = await g.bufferLines(EDITOR);
checks.check("the drag moved x0", allowed[2], "let x0 = 50");
checks.check("and not the knob", allowed[1], "config let gap = 36");
await g.ex("DragAllow");
checks.check("an empty DragAllow clears it", (await g.state()).dm?.allow, []);

app.kill();
process.exit(checks.report());
