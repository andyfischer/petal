#!/usr/bin/env node
//
// Functional integration test for Garden.
//
// Boots the real app with the debug server on a free port, drives it over HTTP
// the way a user would (vim keystrokes, the command line), and asserts on the
// observable state (/state JSON and /buffer text) and on files written to disk.
//
// This is the top layer of the testing strategy (see docs/testing.md): it
// exercises the whole stack — frontend loop, key routing, vim state machine,
// command line, and file I/O — that the pure unit tests cannot reach. It runs
// the headless frontend by default (no window, no GPU needed); pass --window
// to run the same checks through the real winit/wgpu frontend instead.
//
// Usage:  node tools/integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import type { DebugClient } from "./lib/debug-client.ts";
import { makeWorkDir, removeOnExit, sleep } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const work = await makeWorkDir("garden-it");
removeOnExit(work);
const scratch = join(work, "scratch.txt");
const other = join(work, "other.txt");
const script = join(work, "init.ptl");
const logPath = join(work, "app.log");

const checks = new Checks();

// --- fixtures ---------------------------------------------------------------
await writeFile(scratch, "alpha\nbravo\ncharlie\n");
await writeFile(other, "OTHER ONE\nOTHER TWO\n");
await writeFile(script, `layout(editor("${scratch}"))\n`);

// --- launch -----------------------------------------------------------------
// The directory-browser checks below drive that GPP client as a subprocess, so
// build it too rather than trusting whatever binary is lying in target/.
await cargoBuild(["garden-app", "directory-browser"]);

console.log("launching app with debug server...");
const app = await launchGarden({
  args: [...mode, "--init", script],
  logPath,
});
const g = app.client;

// --- assertions -------------------------------------------------------------
console.log("running checks...");

// What build is this? The wiring test for `GET /version` — the endpoint that
// lets a client (or a harness, via `requireFeatures`) tell a stale binary from
// an unsupported request.
const version = await g.version();
checks.check("reports a version", typeof version.version === "string" && version.version !== "", true);
checks.check("reports a build stamp", typeof version.build?.build_date === "string", true);
checks.check(
  "every feature flag is a non-empty name",
  version.features.length > 0 && version.features.every((f) => typeof f === "string" && f !== ""),
  true,
);
checks.check("advertises the value filter it just answered with", version.features.includes("state.values-filter"), true);
checks.check("prelude exports are reported", version.prelude.exports.includes("contrast_text/1"), true);

checks.check("starts in Normal mode", (await g.pane()).mode, "NORMAL");
checks.check("opened the scratch file", (await g.pane()).file, scratch);

// Insert mode round-trip.
await g.key("i");
checks.check("i enters Insert mode", (await g.pane()).mode, "INSERT");
await g.text("X");
await g.key("escape");
checks.check("Escape returns to Normal", (await g.pane()).mode, "NORMAL");
checks.check("typed text was inserted", await g.firstLine(), "Xalpha");

// Delete a line.
await g.key("d");
await g.key("d");
checks.check("dd deletes the current line", await g.firstLine(), "bravo");

// Yank + paste duplicates a line.
await g.key("y");
await g.key("y");
await g.key("p");
checks.check("yy then p duplicates a line", await twoLines(g), ["bravo", "bravo"]);

// Command line: write the buffer, then confirm it hit disk.
await g.ex("w");
checks.check("buffer no longer dirty after :w", (await g.pane()).dirty, false);
checks.check(":w persisted to disk", await firstLineOf(scratch), "bravo");

// External file refresh: the reload-poll watches each open file's mtime/size.
// A clean buffer is reloaded silently when the file changes underneath it.
await writeFile(scratch, "DISK ONE\nDISK TWO\n");
await sleep(500); // let the 200ms reload-poll pick up the external change
checks.check("clean buffer reloads on external change", await g.firstLine(), "DISK ONE");
checks.checkContains("a reload note shows in the status bar", await g.statusNote(), "reloaded from disk");

// A dirty buffer is never clobbered: the external change warns instead.
for (const k of ["g", "g", "0", "i"]) await g.key(k);
await g.text("Z");
await g.key("escape"); // dirty: "ZDISK ONE..."
await writeFile(scratch, "NEWER\n");
await sleep(500);
checks.check("dirty buffer keeps the unsaved edit", await g.firstLine(), "ZDISK ONE");
checks.checkContains("an external-change warning shows", await g.statusNote(), "changed on disk");
await g.key("u"); // undo -> clean again; the next poll is free to reload the newest content
await sleep(500);
checks.check("reload resumes once the buffer is clean", await g.firstLine(), "NEWER");

// Command line: open another file in the pane.
await g.ex(`e ${other}`);
checks.check(":e opened the other file", (await g.pane()).file, other);
checks.check(":e loaded its contents", await g.firstLine(), "OTHER ONE");

// Search: / jumps to the next match, n wraps, :noh clears the highlights.
// (Buffer is OTHER: "OTHER ONE" / "OTHER TWO" — two matches for "OTHER".)
await g.key("/");
await g.keys("OTHER");
checks.check("search prompt shows in /state", await g.commandLine(), "/OTHER");
await g.key("enter");
checks.check("/OTHER jumps to the next match", (await g.pane()).cursor?.line, 1);
checks.check("search matches are highlighted", await highlightCount(g), 2);
await g.key("n");
checks.check("n wraps back to the first match", (await g.pane()).cursor?.line, 0);
await g.ex("noh");
checks.check(":noh clears the highlights", await highlightCount(g), 0);

// :s substitution — plain-text search/replace. Buffer is OTHER:
// "OTHER ONE" / "OTHER TWO".
await g.ex("%s/OTHER/X/g");
checks.check(":%s replaces across the whole buffer", await twoLines(g), ["X ONE", "X TWO"]);
await g.key("u");
checks.check("the whole :%s undoes in one step", await twoLines(g), ["OTHER ONE", "OTHER TWO"]);
await g.key("g");
await g.key("g"); // back to the first line
await g.ex("s/OTHER/Y/");
checks.check(":s replaces only the current line", await twoLines(g), ["Y ONE", "OTHER TWO"]);
await g.ex("s/Y/OTHER/"); // restore line 0 for the multi-click checks below
checks.check(":s restores the line", await twoLines(g), ["OTHER ONE", "OTHER TWO"]);

// Smartcase search: an all-lowercase pattern matches the uppercase text.
await g.key("g");
await g.key("g"); // to line 0
await g.key("/");
await g.keys("other");
await g.key("enter");
checks.check("lowercase search matches via smartcase", (await g.pane()).cursor?.line, 1);
checks.check("smartcase highlights every match", await highlightCount(g), 2);
await g.ex("noh"); // clear highlights again

// Multi-click selection: the /mouse "clicks" field drives double-click word
// and triple-click line selection. Click coordinates are computed from /state
// geometry (pane rect, cell size, gutter = max(3, digits(line_count)) + 2
// cells, pane padding 6px). Buffer is still OTHER: "OTHER ONE" / "OTHER TWO".
checks.check("double-click selects the word under it", await clickSelection(g, 2, 1, 7), "TWO");
checks.check("triple-click selects the line + newline", await clickSelection(g, 3, 0, 2), "OTHER ONE\n");

// % bracket matching: insert a bracket pair and jump across it.
for (const k of ["g", "g", "o"]) await g.key(k); // open a new line below line 0, in Insert mode
await g.text("(abc)");
await g.key("escape");
await g.key("0");
await g.key("%"); // from the '(' jump to the matching ')'
checks.check("% jumps to the matching bracket", (await g.pane()).cursor?.col, 4);
await g.key("%"); // and back from the ')' to the '('
checks.check("% jumps back to the opening bracket", (await g.pane()).cursor?.col, 0);

// --- directory browser (GPP subprocess pane) --------------------------------
// `garden <dir>` opens the directory-browser GPP client in pane 0: a navigable
// listing whose text the subprocess pushes over the Garden Pane Protocol. The
// host forwards the subscribed navigation keys; selecting a file asks the host
// to swap the pane for a normal editor (openPath). This is a separate app
// instance because it is launched on a directory argument, not the .ptl script.
console.log("running directory-browser checks...");
app.kill();

const dbroot = join(work, "dbtree");
await mkdir(join(dbroot, "subdir"), { recursive: true });
await writeFile(join(dbroot, "file_a.txt"), "hello world\n");
await writeFile(join(dbroot, "subdir", "inner.txt"), "second\n");

const browser = await launchGarden({
  args: [...mode, dbroot],
  logPath,
  label: "directory-browser app",
});
const b = browser.client;

// The pane is process-backed; the subprocess identifies itself as the browser.
checks.check("pane 0 is a process pane", (await b.pane()).kind, "process");
checks.check("the process is directory-browser", (await b.pane()).process?.name, "directory-browser");

// The initial listing shows ".." then the dir and file, with "> " on row 0.
checks.check("listing marks the selected row", await b.firstLine(), "> ../");
checks.check("listing shows the subdir", await countLines(b, "subdir/"), 1);
checks.check("listing shows the file", await countLines(b, "file_a.txt"), 1);

// j moves the selection marker down a row.
await b.key("j");
checks.check("j moves the selection down", await selectedRow(b), 2);

// Selection is now on "subdir/"; Enter descends into it.
await b.key("enter");
checks.check("Enter descends into the subdir", await countLines(b, "inner.txt"), 1);

// Go back up (Enter on the "..") to the original directory.
await b.key("enter");
checks.check("Enter on .. returns to the parent", await countLines(b, "file_a.txt"), 1);

// Select file_a.txt (rows: ".." , "subdir/", "file_a.txt") and open it.
await b.key("j");
await b.key("j");
checks.check("selection lands on the file", (await b.bufferLines())[2], "> file_a.txt");
await b.key("enter");
await sleep(300); // the openPath swap drops the subprocess and loads the editor
checks.check("opening a file swaps in an editor", (await b.pane()).kind, "editor");
checks.check("the opened editor shows the file", await b.firstLine(), "hello world");

// --- summary ----------------------------------------------------------------
process.exit(checks.report());

// --- helpers ----------------------------------------------------------------

/** The first two lines of pane 0's buffer. */
async function twoLines(c: DebugClient): Promise<string[]> {
  return (await c.bufferLines()).slice(0, 2);
}

async function firstLineOf(path: string): Promise<string> {
  return (await readFile(path, "utf8")).split("\n")[0];
}

/** Lines of pane 0's buffer containing `needle` (the shell test's `grep -c`). */
async function countLines(c: DebugClient, needle: string): Promise<number> {
  return (await c.bufferLines()).filter((l) => l.includes(needle)).length;
}

/** 1-based row of the browser's "> " selection marker. */
async function selectedRow(c: DebugClient): Promise<number> {
  return (await c.bufferLines()).findIndex((l) => l.startsWith("> ")) + 1;
}

/** Search-match quads in the scene (theme::SEARCH_MATCH). */
async function highlightCount(c: DebugClient): Promise<number> {
  const { primitives } = await c.scene();
  return primitives.filter((p) => {
    if (p.type !== "quad" || !p.color) return false;
    return Math.abs(p.color[0] - 0xd7 / 255) < 0.001 && Math.abs(p.color[3] - 0.3) < 0.001;
  }).length;
}

/** Click `clicks` times at a buffer cell, and report the text it selected. */
async function clickSelection(
  c: DebugClient,
  clicks: number,
  line: number,
  col: number,
): Promise<string | undefined> {
  const st = await c.state();
  const p = st.panes[0];
  const { width: cw, height: ch } = st.cell;
  const gutter = (Math.max(3, String(p.line_count).length) + 2) * cw;
  const reply = await c.click(
    p.rect.x + 6 + gutter + col * cw,
    p.rect.y + 6 + (line + 0.5) * ch,
    { clicks },
  );
  return reply?.selection?.text ?? undefined;
}
