#!/usr/bin/env node
//
// Functional integration test for Petal IDE's drag mode (docs/petal-ide-mode.md,
// "Drag mode"): the toggle, a plain drag that edits the code, the drag result
// panel (picking an alternative, the session chips), marking numbers tunable /
// rigid by mouse in the editor's text and gutter, and the next drag honouring
// those marks. Runs on two of the shipped examples (examples/panels/dm-v2/),
// copied to a scratch directory so nothing in the repo is edited.
//
// Usage:  node tools/dm-mode-integration-test.ts [--window] [--shots <dir>]
//         --shots saves a PNG at each stage (mode on, after a drag with the
//         result panel, after marking hints, the re-drag).
// Exit:   0 if every assertion passes, 1 otherwise.

import { copyFile, mkdir } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, GARDEN_DIR, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import type { AppState, DebugClient } from "./lib/debug-client.ts";
import { makeWorkDir, removeOnExit, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];
const shotsAt = process.argv.indexOf("--shots");
const shots = shotsAt > 0 ? process.argv[shotsAt + 1] : null;
if (shots) await mkdir(shots, { recursive: true });

const work = await makeWorkDir("garden-dm-mode-it");
removeOnExit(work);
const checks = new Checks();
const EXAMPLES = join(GARDEN_DIR, "examples", "panels", "dm-v2");
const EDITOR = 0;
const CANVAS = 1;

await cargoBuild(["garden-app"]);

type Dm = NonNullable<AppState["dm"]>;

async function open(name: string, port: string) {
  const sketch = join(work, name);
  await copyFile(join(EXAMPLES, name), sketch);
  const app = await launchGarden({
    args: ["petal-ide", sketch, ...mode],
    requireFeatures: ["debug.batch", "debug.mouse-pane", "state.dm-drag", "state.dm-mode"],
    logPath: join(work, `${port}.log`),
  });
  await waitUntil(
    async () => (await app.client.pane(CANVAS)).panel?.frame ?? 0,
    (f) => f >= 1,
  );
  return app;
}

async function dm(g: DebugClient): Promise<Dm> {
  return (await g.state()).dm as Dm;
}

/** A plain drag (no modifier) on the canvas from (x, y) by (dx, dy). */
async function drag(g: DebugClient, x: number, y: number, dx: number, dy: number): Promise<void> {
  await g.gesturePaneLocal(
    [
      { op: "down", x, y },
      { op: "move", x: x + dx / 2, y: y + dy / 2 },
      { op: "move", x: x + dx, y: y + dy },
      { op: "up", x: x + dx, y: y + dy },
    ],
    CANVAS,
  );
}

/** Click a cell of the drag result panel by its action (`pick:2`, `never:3`). */
async function clickAction(g: DebugClient, action: string): Promise<boolean> {
  const cell = (await dm(g)).overlay?.cells.find((c) => c.action === action);
  if (!cell) return false;
  await g.click(cell.rect.x + cell.rect.w / 2, cell.rect.y + cell.rect.h / 2);
  return true;
}

/** The 0-based line of the first line starting with `prefix`. */
async function lineOf(g: DebugClient, prefix: string): Promise<number> {
  return (await g.bufferLines(EDITOR)).findIndex((l) => l.startsWith(prefix));
}

/** Click the editor at (line, col), or its drag-mode gutter square (col null). */
async function clickEditor(g: DebugClient, line: number, col: number | null, mods: string[] = []): Promise<void> {
  await g.ex(String(line + 1)); // scroll it into view
  const e = (await dm(g)).editor!;
  const y = e.text_y + (line - e.top_line) * e.line_h + e.line_h / 2;
  const x = col === null ? e.gutter_x! : e.text_x + (col + 0.5) * e.col_w;
  await g.post("/mouse", { op: "click", x, y, mods });
}

async function shot(g: DebugClient, name: string): Promise<void> {
  if (shots) await g.screenshot(join(shots, name));
}

// ── 1. The helper row: mode, plain drag, the result panel, hints ──────────
{
  console.log("01-helper-row: launching petal-ide...");
  const app = await open("01-helper-row.ptl", "row");
  const g = app.client;
  // Cards at x = x0 + i * (card_w + gap) = 40 + i * 80, y 120..192.
  const last = { x: 388, y: 156 };

  checks.check("drag mode starts off", (await dm(g)).mode, false);
  const before = await g.buffer(EDITOR);
  await drag(g, last.x, last.y, 20, 0);
  checks.check("outside drag mode a plain drag is the sketch's", await g.buffer(EDITOR), before);

  await g.post("/menu", { action: "ToggleDragMode" });
  checks.check("the toolbar action turns drag mode on", (await dm(g)).mode, true);
  checks.checkGe("the canvas shows the mode badge", await g.sceneTextCount("DRAG MODE"), 1);
  await g.mousePaneLocal("move", last.x, last.y, CANVAS);
  let d = await dm(g);
  checks.checkContains("the card under the pointer is outlined", d.hover?.label ?? "", "draw_rect");
  checks.check("the pointer is a grab hand", d.cursor, "grab");
  checks.check("the editor has a drag-mode gutter", Array.isArray(d.gutter), true);
  await shot(g, "01-mode-on-hover.png");

  await drag(g, last.x, last.y, 20, 0);
  const gapLine = await lineOf(g, "config let gap");
  checks.check("a plain drag rewrote the knob", (await g.bufferLines(EDITOR))[gapLine], "config let gap = 29");
  d = await dm(g);
  checks.check("the result panel is open", d.panel_open, true);
  checks.checkGe("it lists the alternatives", d.overlay?.cells.filter((c) => c.action?.startsWith("pick:")).length ?? 0, 2);
  checks.check(
    "the written number is underlined",
    d.marks.some((m) => m.line === gapLine && m.written),
    true,
  );
  checks.check("the pointer is back to normal off the shapes", d.cursor !== "grabbing", true);
  await shot(g, "02-result-panel.png");

  checks.check("alternative 2 is clickable", await clickAction(g, "pick:2"), true);
  d = await dm(g);
  checks.check("the panel wrote alternative 2", d.last?.current, 1);
  checks.check("the knob is back at its press-time value", (await g.bufferLines(EDITOR))[gapLine], "config let gap = 24");
  await clickAction(g, "pick:1");
  checks.check("alternative 1 again", (await g.bufferLines(EDITOR))[gapLine], "config let gap = 29");

  // Mark both row knobs rigid with the mouse: gap from its gutter square,
  // x0 by clicking its underlined number.
  await clickEditor(g, gapLine, null);
  checks.check("a gutter click marks the line tunable", (await g.bufferLines(EDITOR))[gapLine], "config let gap = tunable(29)");
  await clickEditor(g, gapLine, null);
  checks.check("a second click marks it rigid", (await g.bufferLines(EDITOR))[gapLine], "config let gap = rigid(29)");
  const x0Line = await lineOf(g, "let x0");
  const x0Col = (await g.bufferLines(EDITOR))[x0Line].indexOf("40");
  await clickEditor(g, x0Line, x0Col, ["alt"]);
  await clickEditor(g, x0Line, x0Col + 9); // now inside `tunable(40)`, underlined
  checks.check("clicking a number cycles it too", (await g.bufferLines(EDITOR))[x0Line], "let x0 = rigid(40)");
  d = await dm(g);
  checks.check("the panel says the alternatives predate the hints", d.hints_changed, true);
  const states = Object.fromEntries((d.last?.candidates ?? []).map((c) => [c.label, c.state]));
  checks.check("the candidates show their new state", [states.gap, states.x0], ["rigid", "rigid"]);
  await shot(g, "03-hints-marked.png");

  // The next drag's solver honours them: neither knob moves.
  await drag(g, last.x, last.y, 20, 0);
  const lines = await g.bufferLines(EDITOR);
  checks.check("rigid gap stays", lines[gapLine], "config let gap = rigid(29)");
  checks.check("rigid x0 stays", lines[x0Line], "let x0 = rigid(40)");
  checks.check("card_w moved instead", lines[await lineOf(g, "let card_w")] !== "let card_w = 56", true);
  await shot(g, "04-redrag-respects-rigid.png");

  // Session-only: `never` on card_w (no source change), then nothing is left.
  const cw = (await dm(g)).last!.candidates.findIndex((c) => c.label === "card_w");
  checks.check("card_w has a never chip", await clickAction(g, `never:${cw}`), true);
  checks.check("the session deny list says so", (await dm(g)).deny, ["card_w"]);
  const beforeDeny = await g.buffer(EDITOR);
  // The last drag left the card 20px right of where it was grabbed.
  await drag(g, last.x + 20, last.y, 20, 0);
  d = await dm(g);
  checks.check("with every x knob ruled out the drag declines", await g.buffer(EDITOR), beforeDeny);
  checks.check("and says so in the panel", d.overlay?.cells.some((c) => c.text === "declined") ?? false, true);
  await clickAction(g, "clear");
  checks.check("clear empties the session lists", (await dm(g)).deny, []);

  await g.ex("DragMode off");
  const off = await g.buffer(EDITOR);
  await drag(g, last.x + 20, last.y, 20, 0);
  checks.check(":DragMode off gives plain drags back to the sketch", await g.buffer(EDITOR), off);
  app.kill();
}

// ── 2. The fractal tree: the root vs the knobs, steered by a rigid hint ────
{
  console.log("03-fractal-tree: launching petal-ide...");
  const app = await open("03-fractal-tree.ptl", "tree");
  const g = app.client;
  await g.key("d", ["cmd", "shift"]);
  checks.check("Cmd+Shift+D turns drag mode on", (await dm(g)).mode, true);
  // Find the rightmost leaf by hovering: the outline names `draw_circle`.
  let leaf: { x: number; y: number } | null = null;
  for (let y = 150; y < 300 && !leaf; y += 4) {
    for (let x = 330; x > 250 && !leaf; x -= 3) {
      await g.mousePaneLocal("move", x, y, CANVAS);
      if ((await dm(g)).hover?.label.startsWith("draw_circle")) leaf = { x, y };
    }
  }
  checks.check("a leaf is grabbable", leaf !== null, true);
  if (leaf) {
    const root = await lineOf(g, "branch(200");
    await drag(g, leaf.x, leaf.y, 15, 0);
    checks.checkContains("unhinted, the root moves", (await g.bufferLines(EDITOR))[root], "branch(215,");
    await g.key("escape");
    await g.key("u"); // undo the drag: one step
    const col = (await g.bufferLines(EDITOR))[root].indexOf("200");
    await clickEditor(g, root, col, ["alt"]);
    await clickEditor(g, root, col + 9, ["alt"]);
    checks.check("Alt-click twice marks the root rigid", (await g.bufferLines(EDITOR))[root], "branch(rigid(200), 380, pi() / 2, trunk, 5)");
    await drag(g, leaf.x, leaf.y, 15, 0);
    const lines = await g.bufferLines(EDITOR);
    checks.check("the root stays", lines[root], "branch(rigid(200), 380, pi() / 2, trunk, 5)");
    checks.check(
      "the tree's knobs open it instead",
      lines[await lineOf(g, "config let spread")] !== "config let spread = 0.45" ||
        lines[await lineOf(g, "let shrink")] !== "let shrink = 0.7",
      true,
    );
    await shot(g, "05-fractal-rigid-root.png");
  }
  app.kill();
}

process.exit(checks.report());
