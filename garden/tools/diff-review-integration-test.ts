#!/usr/bin/env node
//
// Functional integration test for the `garden-diff` review client — the one
// diff/review tool behind `:Diff`, `:Review*`, `:PR`, `garden diff`, `garden pr`.
//
// `garden diff <base>` projects `git diff <base>` (base branch → working tree)
// into a panel with three views: an editable unified stream (the default — a real
// vim `edit_view` where deleting a `+` line drops that addition and deleting a `-`
// line reverts that deletion), an editable before/after split (the right column is
// the working tree, and a projection in its own right — `^S` folds its edits back
// into the files), and a per-file stat diagram. This test drives that whole loop
// over the debug server (see docs/debug-server.md), asserting on the panel's
// observed bindings (`panes[].panel.values` — every value the drawer's frame
// named, in its real type) and — the real proof — the underlying file on disk.
//
// It builds a throwaway git repo (a base commit on `main`, a working-tree change
// on a feature branch), opens `garden diff main` headless, then checks: the diff
// loads, the header pills switch views, deleting a `-` line in the unified view
// and saving with `^S` restores the base line, an edit typed into the after column
// and saved reaches the file, and the reloaded diff reflects it. It then covers
// the structural edits the projection makes possible: `dd` on a hunk header
// reverts that hunk in the file, and `dd` on the view's own title is refused.
//
// Usage:  node tools/diff-review-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { git, makeWorkDir, removeOnExit, sleep, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const work = await makeWorkDir("garden-diff-it");
removeOnExit(work);
const repo = join(work, "repo");
const aTxt = join(repo, "a.txt");
const logPath = join(work, "app.log");

const checks = new Checks();

// --- fixture: a base commit on main, a working-tree change on a branch --------
await mkdir(repo, { recursive: true });
await git(repo, "init", "-q", "-b", "main");
await writeFile(aTxt, "one\ntwo\nthree\nfour\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "base");
await git(repo, "checkout", "-q", "-b", "feature");
await writeFile(aTxt, "one\nTWO\nfour\nfive\n"); // change one line, drop one, add one

// --- launch (cwd = fixture, so `garden diff` resolves that repo) -------------
await cargoBuild(["garden-app", "garden-diff"]);

console.log("launching garden diff with debug server...");
const app = await launchGarden({
  args: ["diff", "main", ...mode],
  cwd: repo,
  logPath,
});
const g = app.client;
// Body rows are laid out in cells; the drawer's own geometry values are
// panel-local, so every hit target below is (value, row) → a pane-local click.
const cellH = (await g.state()).cell.height;

// --- helpers ----------------------------------------------------------------

/**
 * One value the drawer's last frame bound, by name (see garden_diff.ptl), or ""
 * when that binding has not run yet — a name whose term never executed is absent
 * from the map, and an empty read is the sentinel every wait loop below tests
 * against. Values keep their real types, so bools read back as bools.
 */
async function dstate(name: string): Promise<unknown> {
  return (await g.panelValue(name)) ?? "";
}

/** Wait for the drawer's `ready` flag — every scope change re-enters the loader. */
async function waitReady(): Promise<void> {
  await waitUntil(() => dstate("ready"), (v) => v === true);
}

/** Did the status line's error slot report this refusal? */
async function refused(needle: string): Promise<boolean> {
  return (await g.statusError()).includes(needle);
}

/**
 * The review's scope is the `doc` query argument — "", "commit:<sha>" or
 * "since:<sha>" — and the sha is fixture-dependent, so report the kind.
 */
async function scopeKind(): Promise<string> {
  const scope = String(await dstate("scope"));
  if (scope.startsWith("commit:")) return "commit";
  if (scope.startsWith("since:")) return "since";
  if (scope === "") return "whole";
  return "?";
}

/** Click a panel-local point named by two of the drawer's own bound values. */
async function clickAt(x: number, y: number): Promise<void> {
  await g.clickPaneLocal(x, y);
}

async function num(name: string): Promise<number> {
  return Number(await dstate(name));
}

/** The y of a body row, in panel-local coordinates. */
async function bodyRowY(row: number): Promise<number> {
  return (await num("body_top")) + (row + 0.5) * cellH;
}

async function fileText(): Promise<string> {
  return await readFile(aTxt, "utf8");
}

// The panel loads its diff asynchronously (the client shells `git`), so wait for
// the drawer's own ready flag before asserting anything.
await waitReady();

// --- assertions -------------------------------------------------------------
console.log("running assertions...");

checks.check("the pane is the garden-diff panel", (await g.pane()).kind, "panel");
checks.check("the diff loaded", await dstate("ready"), true);
checks.check("no load error", await dstate("has_error"), false);
checks.check("one changed file", await dstate("files"), 1);
checks.check("opens in the unified view", await dstate("mode"), "unified");
checks.check("a local diff has no PR block", await dstate("has_pr"), false);

// --- the header pills switch views -------------------------------------------
const pillY = await num("pill_y");

await clickAt(await num("stat_x"), pillY);
await sleep(300);
checks.check("the stat pill switches view", await dstate("mode"), "stat");
await clickAt(await num("split_x"), pillY);
await sleep(300);
checks.check("the split pill switches view", await dstate("mode"), "split");
await clickAt(await num("unified_x"), pillY);
await sleep(300);
checks.check("the unified pill switches back", await dstate("mode"), "unified");

// --- the wrap toggle (unified only) ------------------------------------------
// Long diff lines soft-wrap to the column by default; the pill turns that off
// for the frames where the exact columns matter.
checks.check("the unified view wraps by default", await dstate("uni_wrap"), true);
await clickAt(await num("wrap_x"), pillY);
await sleep(300);
checks.check("the wrap pill turns wrapping off", await dstate("uni_wrap"), false);
await clickAt(await num("wrap_x"), pillY);
await sleep(300);
checks.check("the wrap pill turns it back on", await dstate("uni_wrap"), true);

// --- editing the after column and saving with ^S -----------------------------
await clickAt(await num("split_x"), pillY);
await sleep(500);
checks.check("back in the split view", await dstate("mode"), "split");
// The after column's projected lines are:
//   0 review: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 one / 4 TWO / 5 four / 6 five
// Click line 4 ("TWO") to focus the editable region with the cursor on its first
// char, then delete that char (`x`) and write the files back (`^S`).
await clickAt(await num("after_body_x"), await bodyRowY(4));
await sleep(300);
await g.key("x");
await sleep(300);
await g.key("s", ["ctrl"]);
await sleep(1500);

checks.check("the ^S save reached the file", await fileText(), "one\nWO\nfour\nfive\n");

// The save invalidates the query, so the drawer reloads the (now different) diff.
await waitReady();
checks.check("the reloaded diff still has the file", await dstate("files"), 1);
checks.check("the reload carried no error", await dstate("has_error"), false);

// The after column is a projection too, but an undecorated one: it shows only the
// new file, so it holds nothing to revert a hunk *back* to. Its `@@@` markers are
// therefore locked chrome — `dd` on one is refused rather than half-reverting the
// hunk (dropping its additions while leaving its deletions in place).
await clickAt(await num("after_body_x"), await bodyRowY(2));
await sleep(300);
await g.key("d");
await g.key("d");
await sleep(500);
checks.check("dd on the after column's marker is refused", await refused("not the change"), true);
checks.check("the refusal left the file alone", await fileText(), "one\nWO\nfour\nfive\n");

// --- editing the unified diff and saving with ^S -----------------------------
// The file is now `one/WO/four/five` against a base of `one/two/three/four`, so
// the reloaded unified stream reads:
//   0 unified: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 " one" / 4 "-two"
//   5 "-three" / 6 "+WO" / 7 " four" / 8 "+five"
// Deleting line 5 (`dd` on "-three") reverts that deletion, so `three` returns to
// the file at the point the diff showed it — the gesture the split view can't do.
await clickAt(await num("unified_x"), pillY);
await sleep(500);
checks.check("back in the unified view", await dstate("mode"), "unified");
await clickAt(await num("unified_body_x"), await bodyRowY(5));
await sleep(300);
await g.key("d");
await g.key("d");
await sleep(300);
await g.key("s", ["ctrl"]);
await sleep(1500);

checks.check(
  "deleting a '-' line reverts that deletion",
  await fileText(),
  "one\nthree\nWO\nfour\nfive\n",
);

await waitReady();
checks.check("the unified reload carried no error", await dstate("has_error"), false);

// --- structural edits: the projection's tier-2 intents ------------------------
// The file is now `one/three/WO/four/five` against a base of `one/two/three/four`,
// so the reloaded unified stream reads:
//   0 unified: … / 1 @@@ file: a.txt / 2 @@@ hunk: … / 3 " one" / 4 "-two"
//   5 "+three" / 6 "+WO" / 7 " four" / 8 "+five"
//
// `dd` on the *hunk header* is not a line deletion: the projection reads it as a
// request to revert the hunk, so the file goes back to exactly what the base
// holds. Nothing in the diff text says this — it works because the host knows
// each line's origin.
await clickAt(await num("unified_body_x"), await bodyRowY(2));
await sleep(300);
await g.key("d");
await g.key("d");
await sleep(300);
await g.key("s", ["ctrl"]);
await sleep(1500);

checks.check("dd on the hunk header reverts the hunk", await fileText(), "one\ntwo\nthree\nfour\n");

await waitReady();
// Reverting every hunk leaves the working tree identical to the base, so the
// reloaded diff is empty — proof the revert reached the file, not just the view.
checks.check("the reverted file leaves nothing to diff", await dstate("files"), 0);
checks.check("the revert reload carried no error", await dstate("has_error"), false);

// --- a locked line refuses rather than corrupting the view --------------------
// The title line belongs to the view, not to the change. Deleting it is refused
// (with a status message) instead of silently removing a line from a file it has
// nothing to do with.
await writeFile(aTxt, "one\nTWO\nthree\nfour\n");
await g.command("Diff main");
await waitReady();
await clickAt(await num("unified_body_x"), await bodyRowY(0));
await sleep(300);
await g.key("d");
await g.key("d");
await sleep(500);
checks.check("deleting the title is refused", await refused("not the change"), true);

// --- the commits view, the context menu, and scoping --------------------------
// The review is `main..feature`, so it has exactly one commit of its own once
// the working-tree change is committed. Committing it also empties the
// whole-review *working-tree* diff, which is what makes the scoping assertions
// below unambiguous: any file the diff shows after this is one the scope put
// there, not a leftover uncommitted edit.
await writeFile(aTxt, "one\nTWO\nthree\nfour\n");
await git(repo, "commit", "-qam", "shout the second line");
await writeFile(join(repo, "b.txt"), "beta\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "add b.txt");
await g.command("Diff main");
await waitReady();

await clickAt(await num("commits_x"), pillY);
await sleep(1000);
checks.check("the commits pill switches view", await dstate("mode"), "commits");
checks.check("the review's two commits are listed", await dstate("commit_rows"), 2);

// Row 0 is the newest commit ("add b.txt"). A left click scopes the diff to it.
const crow0Y = (await num("body_top")) + 20;
await clickAt(300, crow0Y);
await sleep(500);
await waitReady();
checks.check("clicking a commit scopes the diff", await scopeKind(), "commit");
checks.check("the scoped diff is read-only", await dstate("editable"), false);
checks.check("it shows only that commit's file", await dstate("files"), 1);
checks.check("the scoped load carried no error", await dstate("has_error"), false);

// Right-click opens the context menu on that row. Its rows are 24px tall from
// 6px below the menu's top edge, which is the pointer: item 0 spans +6..+30,
// item 1 +30..+54, the separator +54..+63, and item 3 ("Whole review") +63..+87.
await clickAt(await num("commits_x"), pillY);
await sleep(500);
await g.rightClickPaneLocal(300, crow0Y);
await sleep(500);
checks.check("right-click opens the context menu", await dstate("menu_open"), true);

// "Everything since this commit" still ends at the working tree, so unlike
// "only this commit" it stays editable.
await clickAt(330, crow0Y + 42);
await sleep(500);
await waitReady();
checks.check("the menu scopes to 'since this commit'", await scopeKind(), "since");
checks.check("a 'since' scope stays editable", await dstate("editable"), true);

// And back to the whole review, which the menu offers only while scoped.
await clickAt(await num("commits_x"), pillY);
await sleep(500);
await g.rightClickPaneLocal(300, crow0Y);
await sleep(500);
await clickAt(330, crow0Y + 72);
await sleep(500);
await waitReady();
checks.check("the menu returns to the whole review", await scopeKind(), "whole");
checks.check("the whole review is editable again", await dstate("editable"), true);

// --- `/` searches the unified diff -------------------------------------------
// The prompt is the host's, opened from inside the region; the pattern searches
// that region's buffer and the cursor lands on the match.
await clickAt(await num("unified_x"), pillY);
await sleep(500);
await clickAt(await num("unified_body_x"), await bodyRowY(3));
await sleep(300);
await g.key("/");
checks.check("/ opens the search prompt in a region", await g.commandLine(), "/");
await g.keys("beta");
await g.key("return");
await sleep(400);
checks.check("a search that hits reports no error", await g.statusError(), "");
await g.key("/");
await g.keys("zzz");
await g.key("return");
await sleep(400);
checks.check("a search that misses says so", await refused("pattern not found"), true);

// --- report -----------------------------------------------------------------
process.exit(checks.report());
