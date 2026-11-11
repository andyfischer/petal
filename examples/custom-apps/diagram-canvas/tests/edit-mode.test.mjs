#!/usr/bin/env node
// End-to-end test of diagram-canvas's edit mode (docs/features/direct-manipulation.md,
// "Hosts"): starts the Vite dev server, drives the page in headless Chromium,
// and checks that line selection becomes the allow set, that an empty
// selection runs best effort, that answers land in the editor and on the
// canvas, and that an unreachable goal writes nothing.
//
//   npm run test:e2e --workspace examples/custom-apps/diagram-canvas
//
// Needs petal-web-canvas's wasm built (npm run build:wasm --workspace
// integrations/petal-web-canvas) and Playwright's Chromium
// (npx playwright install chromium) or a system Chrome.

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";
import { chromium } from "playwright";

const appDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PORT = Number(process.env.PORT ?? 4113);
const URL = `http://localhost:${PORT}/`;

const SKETCH = [
  "import ui",
  "let x = 120",
  "let off = 0",
  "clear(20, 20, 30)",
  "draw_circle(x + off, 100, 30, 200, 100, 50)",
  "draw_rect(rigid(300), rigid(60), 80, 40, 50, 100, 200)",
  "",
].join("\n");

function startServer() {
  const vite = path.resolve(appDir, "../../../node_modules/.bin/vite");
  const child = spawn(vite, ["--port", String(PORT), "--strictPort"], {
    cwd: appDir,
    stdio: ["ignore", "pipe", "pipe"],
  });
  return new Promise((resolve, reject) => {
    let out = "";
    const onData = (d) => {
      out += d.toString();
      if (out.includes("Local:")) resolve(child);
    };
    child.stdout.on("data", onData);
    child.stderr.on("data", onData);
    child.on("exit", (code) => reject(new Error(`vite exited (${code}):\n${out}`)));
    setTimeout(() => reject(new Error(`vite did not start:\n${out}`)), 30000);
  });
}

let failures = 0;
async function step(name, fn) {
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (e) {
    failures++;
    console.log(`not ok - ${name}\n  ${String(e?.stack ?? e).split("\n").join("\n  ")}`);
  }
}

const server = await startServer();
// Playwright's own Chromium when installed, else the system Chrome.
const browser = await chromium
  .launch()
  .catch(() => chromium.launch({ channel: "chrome" }))
  .catch((e) => {
    server.kill();
    throw e;
  });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(URL);
  await page.waitForFunction(() => window.petalDiagram && window.petalEdit);

  // Open the editor and load the test sketch.
  await page.click("#toggle-source");
  await page.waitForSelector(".cm-content");
  await page.evaluate((s) => window.petalDiagram.load(s), SKETCH);
  await page.waitForFunction((s) => window.petalDiagram.editorText() === s, SKETCH);
  await page.waitForTimeout(200);

  const overlay = page.locator("#edit-overlay");
  const at = async (x, y) => {
    const b = await page.locator("#canvas").boundingBox();
    return [b.x + x, b.y + y];
  };
  const pixel = (x, y) =>
    page.evaluate(
      ([x, y]) => Array.from(document.getElementById("canvas").getContext("2d").getImageData(x, y, 1, 1).data),
      [x, y],
    );
  // Edit mode runs one traced frame after entering and after each reload;
  // wait for it (and the paint after it) before pointing at anything.
  const settled = async () => {
    await page.waitForFunction(() => !window.petalEdit.pending);
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => r())));
  };
  const drag = async ([x0, y0], [x1, y1]) => {
    await settled();
    const [ax, ay] = await at(x0, y0);
    const [bx, by] = await at(x1, y1);
    await page.mouse.move(ax, ay);
    await page.mouse.down();
    await page.mouse.move(bx, by, { steps: 6 });
    await page.mouse.up();
  };
  const outcome = () => page.evaluate(() => window.petalEdit.lastOutcome);
  const panelText = () => page.locator("#edit-panel").innerText();

  await step("entering edit mode shows the panel and takes the pointer", async () => {
    await page.click("#toggle-edit");
    await settled();
    await assert.doesNotReject(overlay.waitFor({ state: "visible" }));
    assert.match(await panelText(), /Edit mode/);
    assert.match(await panelText(), /Nothing selected: best effort/);
  });

  await step("clicking a shape names its call and selects its line", async () => {
    const [x, y] = await at(120, 100);
    await settled();
    await page.mouse.click(x, y);
    const picked = page.locator('[data-testid="edit-picked"]');
    if (!(await picked.count())) assert.fail(`no call was picked; the panel says: ${await panelText()}`);
    assert.match(await picked.innerText(), /draw_circle.*line 5/);
    assert.match(await page.locator('[data-testid="edit-scope"]').innerText(), /May change: line 5/);
  });

  await step("a selection with nothing that moves the shape is refused, nothing written", async () => {
    await drag([120, 100], [150, 100]);
    const out = await outcome();
    assert.equal(out.ok, false, JSON.stringify(out));
    assert.match(await page.locator('[data-testid="edit-note"]').innerText(), /^No edit: /);
    assert.equal(await page.evaluate(() => window.petalDiagram.editorText()), SKETCH);
  });

  await step("selecting line 3 moves only the literal on line 3", async () => {
    // Triple-click selects the whole line (the range ends at the start of
    // line 4, which the allow set leaves out).
    await page.locator(".cm-line").nth(2).click({ clickCount: 3 });
    assert.match(await page.locator('[data-testid="edit-scope"]').innerText(), /May change: line 3 /);
    await drag([120, 100], [150, 100]);
    const out = await outcome();
    assert.equal(out.ok, true, JSON.stringify(out));
    const text = await page.evaluate(() => window.petalDiagram.editorText());
    assert.match(text, /^import ui\nlet x = 120\nlet off = 30\n/, text);
    // The edited program runs: the circle is drawn where it was dropped.
    await page.waitForFunction(() => window.petalDiagram.source().includes("let off = 30"));
    await settled();
    const [r, g, b] = await pixel(150, 100);
    assert.deepEqual([r, g, b], [200, 100, 50], "circle at its new place");
  });

  await step("with nothing selected the drag is best effort: one answer", async () => {
    // A single click in the editor collapses the selection.
    await page.locator(".cm-line").nth(0).click();
    assert.match(await page.locator('[data-testid="edit-scope"]').innerText(), /Nothing selected: best effort/);
    await drag([150, 100], [150, 140]);
    const out = await outcome();
    assert.equal(out.ok, true, JSON.stringify(out));
    assert.equal(out.alternatives.length, 1);
    const text = await page.evaluate(() => window.petalDiagram.editorText());
    assert.match(text, /draw_circle\(x \+ off, 140, 30/, text);
    await page.waitForFunction(() => window.petalDiagram.source().includes(", 140, 30"));
    await settled();
    assert.deepEqual((await pixel(150, 140)).slice(0, 3), [200, 100, 50]);
  });

  await step("a shape held by rigid() literals is refused and nothing is written", async () => {
    const before = await page.evaluate(() => window.petalDiagram.editorText());
    await drag([340, 80], [380, 80]);
    const out = await outcome();
    assert.equal(out.ok, false, JSON.stringify(out));
    assert.equal(await page.evaluate(() => window.petalDiagram.editorText()), before);
  });

  await step("one undo in the editor takes an applied edit back", async () => {
    await page.locator(".cm-content").focus();
    await page.keyboard.press(process.platform === "darwin" ? "Meta+z" : "Control+z");
    const text = await page.evaluate(() => window.petalDiagram.editorText());
    assert.match(text, /draw_circle\(x \+ off, 100, 30/, text);
  });

  await step("leaving edit mode hides the panel and resumes the program", async () => {
    await page.click("#toggle-edit");
    await overlay.waitFor({ state: "hidden" });
    assert.equal(await page.locator("#edit-panel").isVisible(), false);
    assert.equal(await page.evaluate(() => window.petalEdit.active), false);
  });

  await step("no page errors", async () => {
    assert.deepEqual(errors, []);
  });
} finally {
  await browser.close();
  server.kill();
}

if (failures) {
  console.log(`${failures} failed`);
  process.exit(1);
}
console.log("all passed");
