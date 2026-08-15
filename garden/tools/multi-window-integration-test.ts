#!/usr/bin/env node
//
// Multi-window integration test for Garden.
//
// Opens the real windowed frontend, spawns a second OS window at runtime
// (:windownew), and asserts the two windows are independent: the debug server's
// per-window addressing (?window=<ordinal>, /windows) routes to the right one,
// an edit in one window never touches the other, closing the focused window
// leaves the process and the surviving window intact, and Cmd+Q quits.
//
// WINDOWED-ONLY: this test genuinely opens (and closes) real OS windows on the
// desktop for a few seconds — there is no headless path for it, because the
// whole point is the winit/wgpu window registry that headless does not have.
//
// HOME is redirected to a throwaway dir so spawned windows load a known
// init.ptl (an empty editor) and the state DB never touches the real ~/.garden.
//
// Usage:  node tools/multi-window-integration-test.ts
// Exit:   0 if every assertion passes, 1 otherwise.

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { makeWorkDir, removeOnExit, sleep, waitUntil } from "./lib/util.ts";

const work = await makeWorkDir("garden-mwi");
removeOnExit(work);
const homeDir = join(work, "home");
const scratch = join(work, "w1.txt");
const init = join(work, "init.ptl");
const logPath = join(work, "app.log");

const checks = new Checks();

// --- fixtures ---------------------------------------------------------------
// A spawned window loads $HOME/.garden/init.ptl; make that a single empty
// editor so window 2's buffer is deterministically empty.
await mkdir(join(homeDir, ".garden"), { recursive: true });
await writeFile(join(homeDir, ".garden", "init.ptl"), "layout(editor())\n");
// Window 1 is launched with --init on a file with recognizable content.
await writeFile(scratch, "W1LINE\n");
await writeFile(init, `layout(editor("${scratch}"))\n`);

// --- launch -----------------------------------------------------------------
await cargoBuild(["garden-app"]);

// Run the built binary directly (not `cargo run`): HOME is redirected for the
// app, and cargo itself needs the real $HOME for its registry/toolchain.
console.log("launching windowed app (a real window will open)...");
const app = await launchGarden({
  args: ["--init", init],
  env: { ...process.env, HOME: homeDir },
  logPath,
});
const g = app.client;

// --- helpers ----------------------------------------------------------------

async function winCount(): Promise<number> {
  return (await g.windows()).windows.length;
}

/** Is the window with this ordinal focused — or is it gone entirely? */
async function winFocused(ordinal: number): Promise<boolean | "MISSING"> {
  const w = (await g.windows()).windows.find((x) => x.window === ordinal);
  return w ? w.focused : "MISSING";
}

// --- assertions -------------------------------------------------------------
console.log("running checks...");

// One window at startup, ordinal 1, focused, with the launch file's content.
checks.check("starts with exactly one window", await winCount(), 1);
checks.check("window 1 is focused", await winFocused(1), true);
checks.check("window 1 shows its launch file", await g.firstLine(0, 1), "W1LINE");

// Spawn a second window at runtime.
await g.ex("windownew");
await sleep(1500);

checks.check("now there are two windows", await winCount(), 2);
checks.check("the new window (2) took focus", await winFocused(2), true);
checks.check("window 1 is no longer focused", await winFocused(1), false);

// Isolation: the fresh window's buffer is empty; window 1's is untouched.
checks.check("window 2 opened an empty buffer", await g.firstLine(0, 2), "");
checks.check("window 1 still holds its content", await g.firstLine(0, 1), "W1LINE");

// Edit the focused window (2); window 1 must not change at all.
await g.key("i");
await g.text("W2EDIT");
await g.key("escape");
checks.check("the edit landed in window 2", await g.firstLine(0, 2), "W2EDIT");
checks.check("editing window 2 left window 1 alone", await g.firstLine(0, 1), "W1LINE");

// Close the focused window (Cmd+W). The process and window 1 survive.
await g.key("w", ["cmd"]);
await sleep(1000);
checks.check("the process is still running", app.alive(), true);
checks.check("one window remains", await winCount(), 1);
// The ordinal is not renumbered when a window closes.
checks.check("the survivor keeps ordinal 1", await winFocused(1), true);
checks.check("no window 2 lingers", await winFocused(2), "MISSING");
// The focused default (/buffer without ?window) now serves the survivor.
checks.check("the survivor serves its own buffer", await g.firstLine(), "W1LINE");

// Cmd+Q quits the whole process.
await g.keyQuitting("q", ["cmd"]);
const alive = await waitUntil(async () => app.alive(), (a) => !a, { tries: 12, intervalMs: 250 });
checks.check("Cmd+Q exits the process", alive, false);

// --- summary ----------------------------------------------------------------
process.exit(checks.report());
