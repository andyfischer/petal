#!/usr/bin/env node
//
// Integration test for the debug server's screenshot/frame consistency contract
// (docs/debug-server.md "Frame consistency"): a `/screenshot` taken immediately
// after injected input must capture a complete, steady frame that reflects that
// input — no sleeps, no retries — and must expose the captured frame number in
// an `X-Garden-Frame` header. `GET /frame` reports the same counter instantly.
//
// The fixture is a panel script that counts `k` presses but displays the count
// through a two-frame `state` chain (shown <- lag <- count). Without the
// settle-then-capture contract, a screenshot right after POST /key renders the
// panel's cached commands mid-propagation and shows a stale value; with it,
// panels are ticked to a fixed point before the scene is built. The test drives
// input -> screenshot back-to-back 10x and asserts the captured scene reflects
// every press, the same layered strategy as tools/diff-review-integration-test.ts.
//
// Usage:  node tools/screenshot-consistency-test.ts
// Exit:   0 if every assertion passes, 1 otherwise.

import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { fileMagic, readPixel } from "./lib/png.ts";
import { makeWorkDir, removeOnExit } from "./lib/util.ts";

const work = await makeWorkDir("garden-shot-it");
removeOnExit(work);
const counterPtl = join(work, "counter.ptl");
const init = join(work, "init.ptl");
const logPath = join(work, "app.log");

const checks = new Checks();

// --- fixture: a counter panel whose display lags its state by two frames ------
await writeFile(
  counterPtl,
  `// Counts \`k\` presses. The displayed value propagates through a two-frame
// state chain (shown <- lag <- count), so a capture taken before panel
// frames settle shows a stale \`shown\` — exactly what the screenshot
// consistency contract must prevent. Every frame of the chain draws
// distinct content (count updates immediately), so the settle loop's
// fixed-point detection sees each propagation step.
state count = 0
state lag = 0
state shown = 0

if key_pressed("k") then
  count = count + 1
end

clear(16, 18, 26)
draw_text("count: " ++ str(count), 10, 10, 14, 230, 230, 230)
draw_text("lag: " ++ str(lag), 10, 30, 14, 200, 200, 200)
draw_text("shown: " ++ str(shown), 10, 50, 14, 200, 200, 200)
// A solid swatch whose red channel encodes \`shown\` (10 + shown*20), so the
// test can decode the captured PNG and assert on the *pixels* of the shot.
draw_rect(20, 80, 260, 160, 10 + shown * 20, 40, 90)

shown = lag
lag = count
`,
);
await writeFile(init, `layout(panel("${counterPtl}"))\n`);

// --- launch --------------------------------------------------------------------
await cargoBuild(["garden-app"]);

console.log("launching headless garden with counter panel...");
const app = await launchGarden({
  args: ["--headless", "--init", init],
  logPath,
});
const g = app.client;

// --- sanity ----------------------------------------------------------------------
console.log("running assertions...");
checks.check("pane is a panel", (await g.pane()).kind, "panel");
checks.check("panel has no runtime error", await g.sceneErrorCount(), 0);
checks.check("counter starts at 0", await g.panelValue("count"), 0);

// Physical-pixel sample point inside the swatch (panel-local 20,80 + 260x160),
// from the pane rect and window scale reported by /state.
const st = await g.state();
const scale = st.window.scale;
const swatchX = Math.trunc((st.panes[0].rect.x + 20 + 130) * scale);
const swatchY = Math.trunc((st.panes[0].rect.y + 80 + 80) * scale);
console.log(`swatch sample point: ${swatchX},${swatchY}`);

/**
 * Decode the swatch pixel of a shot and map its red channel back to `shown`
 * (drawn as 10 + shown*20; sRGB round-trip error is a couple of counts, so
 * snap to the nearest step and reject anything further than 5 off).
 */
function pngShown(path: string): number | string {
  const { r } = readPixel(path, swatchX, swatchY);
  const step = Math.round((r - 10) / 20);
  const off = r - (10 + step * 20);
  return off < -5 || off > 5 ? `bad-red-${r}` : step;
}

// --- the contract: input then immediate screenshot, 10x, no sleeps ---------------
let lastFrame = 0;
for (let i = 1; i <= 10; i++) {
  await g.key("k");
  const png = join(work, `shot-${i}.png`);
  const frame = await g.screenshot(png);

  // The capture carries its frame number, monotonically increasing.
  checks.checkGe(
    `shot ${i}: X-Garden-Frame present and increasing (frame=${frame ?? "missing"})`,
    frame,
    lastFrame + 1,
  );
  if (frame !== undefined) lastFrame = frame;

  // The body is a real PNG.
  checks.check(`shot ${i}: body is a PNG`, fileMagic(png), "89504e47");

  // The captured *pixels* reflect the press, including the swatch color that
  // needs two extra panel frames to propagate — the settle contract itself.
  checks.check(`shot ${i}: PNG pixels show settled shown: ${i}`, pngShown(png), i);

  // The captured (= settled) scene reflects the press, including the value that
  // needs two extra panel frames to propagate. /scene follows the same settle
  // contract, and no further input arrived, so it shows what the PNG shows.
  checks.check(`shot ${i}: scene shows count: ${i}`, await g.sceneTextCount(`count: ${i}`), 1);
  checks.check(`shot ${i}: scene shows settled shown: ${i}`, await g.sceneTextCount(`shown: ${i}`), 1);

  // /state agrees, and reports the global frame counter. At the settled fixed
  // point every link of the chain holds the same value, so the observed `shown`
  // binding matches the one that was drawn.
  checks.check(`shot ${i}: panel values settled`, await g.panelValue("shown"), i);
  checks.checkGe(`shot ${i}: /state frame >= capture frame`, (await g.state()).frame, frame ?? 0);

  // /frame reports the current counter instantly (the poll target for clients).
  checks.checkGe(`shot ${i}: /frame >= capture frame`, await g.frame(), frame ?? 0);
}

// --- report -----------------------------------------------------------------------
process.exit(checks.report());
