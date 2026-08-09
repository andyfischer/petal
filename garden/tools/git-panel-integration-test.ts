#!/usr/bin/env node
//
// Functional integration test for the Git history browser (`:Git`).
//
// `garden git log` (the CLI twin of `:Git`) launches the `git-log` panel-mode GPP
// app (gpp-apps/git-viewers): it pushes a Petal-drawn drawer the host runs
// in-process — a commit list and per-commit file list on the left, the selected
// file's diff on the right — and answers its `query(kind, arg)` requests over the
// pipe by shelling out to `git`. The drawer bakes in NO data; it loads the log and
// each commit's diff at runtime through the async `query` native (on Petal's
// pending values). Because loads are async and cross a subprocess pipe, this test
// waits for the pending fetches to land (poll-until helpers) before asserting on
// the panel's observed bindings — every named value the drawer's frame bound,
// reported verbatim at /state → panes[0].panel.values (bools as bools, ints as
// ints), so the assertions below name the drawer's own variables. Same layered
// strategy as tools/diff-review-integration-test.ts.
//
// It builds a throwaway git repo fixture (three commits + a dirty working tree),
// opens `garden git log` on it headless, and checks: the worktree row and commit
// rows select with keys and clicks (each selection fetching that commit's diff),
// Tab cycles the focus ring, and the wheel scrolls the hovered region without
// moving the selection.
//
// Usage:  node tools/git-panel-integration-test.ts [--window]
// Exit:   0 if every assertion passes, 1 otherwise.

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { cargoBuild, launchGarden } from "./lib/app.ts";
import { Checks } from "./lib/check.ts";
import { git, makeWorkDir, removeOnExit, waitUntil } from "./lib/util.ts";

const mode = process.argv.includes("--window") ? [] : ["--headless"];

const work = await makeWorkDir("garden-git-it");
removeOnExit(work);
const repo = join(work, "repo");
const logPath = join(work, "app.log");

const checks = new Checks();

// --- fixture: three commits, then a dirty working tree ------------------------
await mkdir(repo, { recursive: true });
await git(repo, "init", "-q");
await writeFile(join(repo, "a.txt"), "a1\na2\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "first: add a.txt");
// A long, scrollable diff.
await writeFile(join(repo, "big.txt"), range(1, 120).join("\n") + "\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "second: add big.txt");
await writeFile(join(repo, "a.txt"), "a1\nCHANGED\n");
await writeFile(join(repo, "c.txt"), "c1\n");
await git(repo, "add", "-A");
await git(repo, "commit", "-qm", "third: touch a.txt and c.txt");
await writeFile(join(repo, "a.txt"), "a1\nDIRTY\n"); // tracked edit → worktree row

// --- launch (cwd = fixture, so `garden git log` resolves that repo) ------------
// `git-viewers` produces the `git-log` GPP client this test drives; building it
// here keeps a stale binary from failing the run as if the panel were broken.
await cargoBuild(["garden-app", "git-viewers"]);

console.log("launching git history browser with debug server...");
const app = await launchGarden({
  args: ["git", "log", ...mode],
  cwd: repo,
  logPath,
});
const g = app.client;

// --- helpers ----------------------------------------------------------------

/** One of the panel's observed bindings by name, or undefined when the binding
 *  never ran this frame. Values keep their real JSON types. */
function pstate(name: string): Promise<unknown> {
  return g.panelValue(name);
}

async function pnum(name: string): Promise<number> {
  return Number(await pstate(name));
}

/**
 * The `query`-backed data lands asynchronously, so a value that depends on a
 * fresh fetch (the log, a newly-selected commit's diff) may take a few frames.
 * These poll the reading until it settles (~5s cap) before the assertion, so the
 * test exercises the real pending→ready path without racing it.
 */
async function checkEventually(desc: string, name: string, expected: unknown): Promise<void> {
  const got = await waitUntil(() => pstate(name), (v) => v === expected, {
    tries: 100,
    intervalMs: 50,
  });
  checks.check(desc, got, expected);
}

async function checkGtEventually(desc: string, name: string, threshold: number): Promise<void> {
  const got = await waitUntil(() => pstate(name), (v) => Number(v) > threshold, {
    tries: 100,
    intervalMs: 50,
  });
  checks.checkGt(desc, got, threshold);
}

/** The topmost text run inside the diff region (panel-local sample point). */
async function diffFirst(): Promise<string> {
  const r = (await g.pane()).rect;
  const [dx, dy] = [Math.trunc(r.x) + diffX, Math.trunc(r.y) + 130];
  const { primitives } = await g.scene();
  const runs = primitives
    .filter((p) => p.type === "text" && p.pos && p.pos[0] >= dx - 30 && p.pos[1] >= dy - 20)
    .sort((a, b) => a.pos![1] - b.pos![1]);
  return runs.length ? (runs[0].text ?? "") : "";
}

// Wait for the asynchronous `query("log", …)` to resolve before asserting on it.
console.log("waiting for the git history to load (pending → ready)...");
await waitUntil(() => pstate("log_ready"), (v) => v === true, { tries: 200, intervalMs: 50 });

// Panel-local x inside the diff region: just right of the (draggable) left
// column, whose current width the drawer binds (and the host observes) as `left_w`.
const diffX = (await pnum("left_w")) + 40;

// --- assertions -------------------------------------------------------------
console.log("running assertions...");
checks.check("pane is a panel", (await g.pane()).kind, "panel");
checks.check("the log loaded (pending→ready)", await pstate("log_ready"), true);
checks.check("panel has no runtime error", await g.sceneErrorCount(), 0);
await checkEventually("3 commits + the worktree row", "total_rows", 4);
checks.check("starts on the worktree row", await pstate("commit_selected"), 0);
await checkEventually("worktree diff has one file", "file_count", 1);
await checkEventually("no data errors", "has_error", false);

// j walks into the history; each selection fetches that commit's file list
// through `query("commit", hash)` — a Pending until the background git lands.
await g.key("j");
checks.check("j selects the newest commit", await pstate("commit_selected"), 1);
await checkEventually("third commit changed two files", "file_count", 2);
await g.key("j");
checks.check("j again → second commit", await pstate("commit_selected"), 2);
await checkEventually("second commit changed one file", "file_count", 1);
await checkGtEventually("big.txt diff is long", "diff_lines", 100);

// The wheel over the diff region scrolls it; the selection stays put. The diff
// body is a `text_view` region whose content overflows its rect, so the region
// itself consumes the wheel (native editor scroll, independent of the script's
// diff_scroll — the script only sees scroll_y() when the region has nothing to
// scroll). Observe the region's topmost visible text run changing in /scene.
const top0 = await diffFirst();
await g.scrollPaneLocal(diffX, 200, 8);
const top1 = await waitUntil(diffFirst, (t) => t !== top0, { tries: 40, intervalMs: 50 });
if (top0 !== "" && top1 !== top0) {
  checks.ok("wheel scrolls the diff (native region scroll)");
} else {
  checks.bad("wheel scrolls the diff (native region scroll)", `top diff line stayed [${top1}]`);
}
checks.check("wheel does not move selection", await pstate("commit_selected"), 2);

// Tab cycles focus: commits → files → diff; keys follow the focused region.
checks.check("commit list focused initially", await pstate("focus"), 0);
await g.key("tab");
checks.check("Tab focuses the file list", await pstate("focus"), 1);
await g.key("tab");
checks.check("Tab focuses the diff", await pstate("focus"), 2);
await g.key("pageup");
checks.check("PageUp scrolls the focused diff", await pstate("diff_scroll"), 0);
await g.key("tab");
checks.check("Tab wraps back to commits", await pstate("focus"), 0);

// Clicking a commit row selects it (row 0 = worktree, rows are 40px from y=74)
// and resets the diff scroll for the new selection.
await g.clickPaneLocal(100, 74 + 20);
checks.check("click selects the worktree row", await pstate("commit_selected"), 0);
checks.check("selection change resets scroll", await pstate("diff_scroll"), 0);
await checkEventually("worktree file list is back", "file_count", 1);

// Clicking a hunk header (the first diff row, at panel y≈114) uncollapses the
// diff to full context, and clicking it again collapses it. On the second commit
// (big.txt) the diff is long either way, so we assert the toggle + no shrink.
await g.key("j");
await g.key("j");
checks.check("on the big.txt commit", await pstate("commit_selected"), 2);
checks.check("diff starts collapsed", await pstate("diff_expanded"), false);
await g.clickPaneLocal(diffX, 114);
checks.check("hunk click expands to full ctx", await pstate("diff_expanded"), true);
await checkGtEventually("expanded diff stays full", "diff_lines", 100);
await g.clickPaneLocal(diffX, 114);
checks.check("hunk click collapses again", await pstate("diff_expanded"), false);

// Dragging the vertical divider (at panel x = 12 + left_w) widens the left
// column; the new width holds after the button is released.
const lw0 = await pnum("left_w");
const divX = 12 + lw0;
await g.mousePaneLocal("down", divX, 300);
await g.mousePaneLocal("move", divX + 140, 300);
const lw1 = await pnum("left_w");
checks.checkGt("divider drag widens left column", lw1, lw0);
await g.mousePaneLocal("up", divX + 140, 300);
checks.check("widened column holds after drag", await pnum("left_w"), lw1);

// Dragging the horizontal divider (at panel y = header 50 + commits_area + 2)
// resizes the commit list vs the file list.
const ca0 = await pnum("commits_area");
const hy = 50 + ca0 + 2;
await g.mousePaneLocal("down", 100, hy);
await g.mousePaneLocal("move", 100, hy - 90);
const ca1 = await pnum("commits_area");
await g.mousePaneLocal("up", 100, hy - 90);
checks.checkLt("horizontal drag shrinks commit list", ca1, ca0);

// The ⟳ Refresh button (top-right) re-runs git: after a new tracked change lands
// in the repo, clicking it reloads the working-tree diff and the new file appears.
// This proves Refresh calls git again rather than serving the cached diff.
await g.clickPaneLocal(100, 74 + 20); // select the worktree row
await checkEventually("back on the worktree row", "file_count", 1);
await writeFile(join(repo, "d.txt"), "brand new\n"); // a new staged file vs HEAD
await git(repo, "add", "d.txt");
const paneW = Math.trunc((await g.pane()).rect.w);
await g.clickPaneLocal(paneW - 61, 22); // click ⟳ Refresh
await checkEventually("Refresh re-runs git: new file", "file_count", 2);

// --- report -----------------------------------------------------------------
process.exit(checks.report());

function range(lo: number, hi: number): number[] {
  return Array.from({ length: hi - lo + 1 }, (_, i) => lo + i);
}
