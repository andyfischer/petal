#!/usr/bin/env node
//
// Functional integration test for the start screen (`garden` with no arguments).
//
// A bare `garden` opens the `main-menu` panel-mode GPP app (gpp-apps/main-menu):
// it pushes a Petal drawer the host runs in-process — recent projects, files and
// PRs as one flat list — and answers its `query("recents", "")` by reading the
// recents tables of `$HOME/.garden/state/db.sqlite` read-only. Opening a row is
// not the app's doing: the drawer calls `mutate("open_path", …)`, which
// `App::host_mutation` intercepts before any forwarding, so a click here has to
// travel drawer → host → pane to prove anything.
//
// The fixture is built by RUNNING GARDEN, not by writing DB rows: a first
// headless launch opens two fixture files through the real `:e` path (which
// records the file and its repo root), then quits with Cmd-Q so the WAL is
// checkpointed. That is what catches schema drift between garden-app, the
// writer, and main-menu, the reader — a hand-seeded database would agree with
// whichever of the two it was written against.
//
// $HOME is redirected to a throwaway directory (as the multi-window test does)
// so the recents database is hermetic and the real ~/.garden is never touched.
//
// It then checks: the menu comes up with the seeded recents, the keyboard walks
// and clamps the flat selection, clicking a Recent Files row turns the pane into
// an editor on that file, and — with a second, empty $HOME — the first-ever
// launch still renders a menu with three empty sections instead of an error.
//
// Usage:  node tools/main-menu-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { mkdir, realpath, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden, type LaunchedApp } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { git, makeWorkDir, removeOnExit, waitUntil } from "./lib/util.ts";

const windowed = process.argv.includes("--window");
const mode = windowed ? [] : ["--headless"];

const work = await makeWorkDir("garden-menu-it");
removeOnExit(work);
const seededHome = join(work, "home-seeded");
const emptyHome = join(work, "home-empty");
const repo = join(work, "repo");

const checks = new Checks();

// The drawer's row geometry — its ROW_H constant and every row's offset inside
// the scrolled content (`row_off`) — is published by the panel, so click
// targets are read from the drawer rather than hardcoded here.

// --- fixture: a git repo with two files, and two homes ------------------------
await mkdir(repo, { recursive: true });
await git(repo, "init", "-q");
await writeFile(join(repo, "a.txt"), "a1\na2\n");
await writeFile(join(repo, "b.txt"), "b1\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "base");
await mkdir(seededHome, { recursive: true });
await mkdir(emptyHome, { recursive: true });
// Recents canonicalize their paths, so on macOS a $TMPDIR fixture is recorded
// under /private/... — compare against the resolved path, not the one we built.
const realRepo = await realpath(repo);
const fileA = join(realRepo, "a.txt");
const fileB = join(realRepo, "b.txt");

// `main-menu` is the GPP client this test drives, and the host resolves it as a
// sibling of `garden`; building both here keeps a stale binary from failing the
// run as if the menu were broken.
await cargoBuild(["garden-app", "main-menu"]);

// --- phase 1: seed the recents by really opening files ------------------------
// `--no-menu` is the opt-out the menu wiring added, so this launch is a plain
// editor: the point is only to drive `:e`, whose open path records each file
// and the repo root above it.
console.log("seeding recents by opening files in a first launch...");
const seeder = await launchGarden({
  args: ["--no-menu", ...mode],
  cwd: repo,
  logPath: join(work, "seed.log"),
  env: { ...process.env, HOME: seededHome },
  label: "seeding garden",
});
await seeder.client.command(`e ${fileA}`);
await seeder.client.command(`e ${fileB}`);
checks.check("the seeding launch opened the file", (await seeder.client.pane()).file, fileB);
// Quit rather than kill: Cmd-Q closes the database, checkpointing the WAL into
// db.sqlite so the menu's read-only reader sees the rows.
await quit(seeder);
checks.check("the seeding launch quit cleanly", seeder.alive(), false);

// --- phase 2: a bare `garden` opens the menu on those recents -----------------
console.log("launching the start screen with debug server...");
const app = await launchGarden({
  args: [...mode],
  cwd: repo,
  logPath: join(work, "menu.log"),
  env: { ...process.env, HOME: seededHome },
  // A stale `garden` without these is the failure this harness used to hit as
  // an unexplained assertion; now it says so up front.
  requireFeatures: ["state.values-filter", "panel.host-mutate"],
});
const g = app.client;

/** One of the drawer's observed bindings by name (real JSON types preserved). */
function pstate(name: string): Promise<unknown> {
  return g.panelValue(name);
}

async function pnum(name: string): Promise<number> {
  return Number(await pstate(name));
}

// The recents arrive asynchronously (the app reads sqlite across the pipe), so
// wait for the drawer's own ready flag before asserting on the lists.
await waitUntil(() => pstate("recents_ready"), (v) => v === true, { tries: 200, intervalMs: 50 });

console.log("running assertions...");
checks.check("a bare `garden` opens a panel pane", (await g.pane()).kind, "panel");
checks.check("the menu is the main-menu app", panelScript(await g.state()), true);
checks.check("the recents query resolved", await pstate("recents_ready"), true);
checks.check("the reader reported no error", await pstate("recents_err"), "");
checks.check("panel has no runtime error", (await g.pane()).panel?.error ?? null, null);
checks.check("no error text on screen", await g.sceneErrorCount(), 0);

// The seeding launch opened two files inside one repo, so the writer recorded
// two files and — through `record_file` — the repo root as a project.
checks.checkGe("the seeded project is listed", await pnum("project_count"), 1);
checks.checkGe("both seeded files are listed", await pnum("file_count"), 2);
checks.check("no PRs were seeded", await pnum("pr_count"), 0);
checks.check(
  "the sections are one flat row list",
  await pnum("row_count"),
  (await pnum("project_count")) + (await pnum("file_count")) + (await pnum("pr_count")),
);

// --- the keyboard walks the flat list and clamps at both ends -----------------
const rowCount = await pnum("row_count");
checks.check("the selection starts at the top", await pstate("selected"), 0);
await g.key("up");
checks.check("up at the top stays put", await pstate("selected"), 0);
await g.key("j");
checks.check("j moves down one row", await pstate("selected"), 1);
await g.key("down");
checks.check("down moves down one row", await pstate("selected"), 2);
await g.key("k");
checks.check("k moves back up", await pstate("selected"), 1);
await g.key("end");
checks.check("End jumps to the last row", await pstate("selected"), rowCount - 1);
await g.key("j");
checks.check("j at the bottom clamps", await pstate("selected"), rowCount - 1);
await g.key("home");
checks.check("Home jumps back to the first row", await pstate("selected"), 0);

// --- the Open File… button reaches the host's picker --------------------------
// Headless only: with a window this really pops the native picker and blocks
// until someone answers it. Without one the host refuses instead of hanging,
// which is exactly what makes the button assertable here — the refusal in the
// status line proves the button's `mutate("open_file_dialog")` reached the host,
// and the pane is still the menu afterwards.
if (!windowed) {
  const btn = (await pstate("open_button")) as { x: number; y: number; w: number; h: number };
  await g.clickPaneLocal(btn.x + btn.w / 2, btn.y + btn.h / 2);
  const err = await waitUntil(() => g.statusError(), (e) => e !== "", { tries: 60, intervalMs: 50 });
  checks.checkContains("Open File… asks the host for a picker", err, "no native file picker");
  checks.check("the drawer recorded the action", await pstate("last_action"), "open_file_dialog");
  checks.check("a refused picker leaves the menu up", (await g.pane()).kind, "panel");
}

// --- clicking a Recent Files row opens that file ------------------------------
// The files section follows the projects, so the first file row's index is the
// project count; its y comes from the drawer's own `row_off` table. The click
// travels drawer → `mutate("open_path")` → `App::host_mutation` → the pane, so
// the pane becoming an editor is the whole round trip's proof.
const firstFileRow = await pnum("project_count");
const rowOff = (await pstate("row_off")) as number[];
const rowY = (await pnum("content_top")) + rowOff[firstFileRow] - (await pnum("scroll_px"));
const rowH = await pnum("ROW_H");
await g.clickPaneLocal((await pnum("col_x")) + 20, rowY + rowH / 2);

const paneFile = await waitUntil(async () => (await g.pane()).file, (f) => !!f, {
  tries: 100,
  intervalMs: 50,
});
checks.check("clicking a file row makes the pane an editor", (await g.pane()).kind, "editor");
// b.txt was opened last, so it is the newest file and heads the section.
checks.check("it opened the row's own file", paneFile, fileB);
checks.check("the host reported the open", await g.statusNote(), `opened ${fileB}`);
await quit(app);

// --- phase 3: a first-ever launch, with no recents at all ---------------------
// The regression guard for the "no database yet" path: an empty $HOME has no
// db.sqlite when the app starts, and the reader must answer with three empty
// lists rather than an error, so the screen still reads as a menu.
console.log("launching the start screen on a first-ever $HOME...");
const fresh = await launchGarden({
  args: [...mode],
  cwd: repo,
  logPath: join(work, "fresh.log"),
  env: { ...process.env, HOME: emptyHome },
  label: "fresh garden",
});
const f = fresh.client;
await waitUntil(() => f.panelValue("recents_ready"), (v) => v === true, {
  tries: 200,
  intervalMs: 50,
});

checks.check("a first launch still opens the menu", (await f.pane()).kind, "panel");
checks.check("the empty read still resolves", await f.panelValue("recents_ready"), true);
checks.check("a missing database is not an error", await f.panelValue("recents_err"), "");
checks.check("no panel runtime error", (await f.pane()).panel?.error ?? null, null);
checks.check("no recent projects", await f.panelValue("project_count"), 0);
checks.check("no recent files", await f.panelValue("file_count"), 0);
checks.check("no recent PRs", await f.panelValue("pr_count"), 0);
checks.check("no rows to select", await f.panelValue("row_count"), 0);
checks.check("nothing on screen reads as an error", await f.sceneErrorCount(), 0);
fresh.kill();

// --- report -----------------------------------------------------------------
process.exit(checks.report());

/** Is pane 0 driven by the main-menu client? `identity.panels` names the script
 *  behind each panel pane — `gpp:<binary>` for a GPP client. */
function panelScript(state: unknown): boolean {
  const panels = (state as { identity?: { panels?: { pane: number; script?: string }[] } }).identity
    ?.panels;
  return !!panels?.some((p) => p.pane === 0 && (p.script ?? "").endsWith("main-menu"));
}

/** Quit the app the way a user does, and wait for the process to actually go —
 *  the database is flushed on the way out, and phase 2 reads it. */
async function quit(app: LaunchedApp): Promise<void> {
  await app.client.keyQuitting("q", ["cmd"]);
  await waitUntil(async () => app.alive(), (alive) => !alive, { tries: 40, intervalMs: 100 });
  app.kill();
}
