#!/usr/bin/env node
//
// reports-tool.ts — browse and manage the bug/feature reports filed with the
// `:report` command, stored in the garden state database
// (`~/.garden/state/db.sqlite`, see garden-app/src/state.rs + event_log.rs).
//
// Usage:
//   node scripts/reports-tool.ts list [--json]
//   node scripts/reports-tool.ts show <id> [--json]
//   node scripts/reports-tool.ts delete <id>
//
// Options:
//   --db <path>   Override the state database path
//                 (default: $GARDEN_STATE_DIR/db.sqlite or ~/.garden/state/db.sqlite)
//   --json        Emit machine-readable JSON (for agents / scripts)
//
// Runs directly under Node >= 22.6 (native TypeScript stripping + the built-in
// `node:sqlite` module); no install step or dependencies.

import { DatabaseSync } from "node:sqlite";
import { homedir } from "node:os";
import { join } from "node:path";

interface ReportRow {
  id: number;
  window_id: number;
  at_ms: number;
  message: string;
  context: string;
}

interface ReportView {
  id: number;
  windowId: number;
  at: string; // ISO-8601 UTC
  message: string;
  /** File paths touched in the captured window, most-recent last. */
  files: string[];
  /** Ex commands (`:w`, `:s/...`, `:report ...`) run in the captured window. */
  commands: string[];
  /** The full captured context block, one `timestamp [category] detail` per line. */
  context: string;
}

function parseArgs(argv: string[]): { cmd?: string; rest: string[]; json: boolean; db?: string } {
  const rest: string[] = [];
  let json = false;
  let db: string | undefined;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--json") json = true;
    else if (a === "--db") db = argv[++i];
    else rest.push(a);
  }
  return { cmd: rest.shift(), rest, json, db };
}

function dbPath(override?: string): string {
  if (override) return override;
  const dir = process.env.GARDEN_STATE_DIR ?? join(homedir(), ".garden", "state");
  return join(dir, "db.sqlite");
}

/** Pull lines of a given `[category]` out of a captured context block. */
function linesOfCategory(context: string, category: string): string[] {
  const out: string[] = [];
  // Each line is `YYYY-MM-DD HH:MM:SS  [category] detail`.
  const re = new RegExp(`^\\S+ \\S+\\s+\\[${category}\\] (.*)$`);
  for (const line of context.split("\n")) {
    const m = line.match(re);
    if (m) out.push(m[1]);
  }
  return out;
}

/** Distinct file paths from `[file] open <path>` (and other `[file]`) lines. */
function filesFromContext(context: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const detail of linesOfCategory(context, "file")) {
    const path = detail.replace(/^open\s+/, "").trim();
    if (path && !seen.has(path)) {
      seen.add(path);
      out.push(path);
    }
  }
  return out;
}

function toView(r: ReportRow): ReportView {
  return {
    id: r.id,
    windowId: r.window_id,
    at: new Date(r.at_ms).toISOString(),
    message: r.message,
    files: filesFromContext(r.context),
    commands: linesOfCategory(r.context, "command"),
    context: r.context,
  };
}

function openDb(path: string): DatabaseSync {
  try {
    return new DatabaseSync(path, { readOnly: false });
  } catch (e) {
    fail(`cannot open state database at ${path}: ${(e as Error).message}`);
  }
}

function fail(msg: string): never {
  console.error(`reports-tool: ${msg}`);
  process.exit(1);
}

function allReports(db: DatabaseSync): ReportRow[] {
  return db
    .prepare("SELECT id, window_id, at_ms, message, context FROM reports ORDER BY at_ms ASC, id ASC")
    .all() as unknown as ReportRow[];
}

function oneReport(db: DatabaseSync, id: number): ReportRow | undefined {
  return db
    .prepare("SELECT id, window_id, at_ms, message, context FROM reports WHERE id = ?")
    .get(id) as unknown as ReportRow | undefined;
}

function cmdList(db: DatabaseSync, json: boolean): void {
  const views = allReports(db).map(toView);
  if (json) {
    // For listing, omit the bulky full context; show/--json carries it.
    console.log(JSON.stringify(views.map(({ context, ...v }) => v), null, 2));
    return;
  }
  if (views.length === 0) {
    console.log("No reports.");
    return;
  }
  for (const v of views) {
    const firstLine = v.message.split("\n")[0];
    const summary = firstLine.length > 100 ? firstLine.slice(0, 97) + "..." : firstLine;
    console.log(`#${v.id}  ${v.at}  (window ${v.windowId})`);
    console.log(`    ${summary}`);
    if (v.files.length) console.log(`    files: ${v.files.join(", ")}`);
    console.log();
  }
  console.log(`${views.length} report(s). Use 'show <id>' for full context.`);
}

function cmdShow(db: DatabaseSync, idStr: string | undefined, json: boolean): void {
  const id = Number(idStr);
  if (!idStr || !Number.isInteger(id)) fail("show requires a numeric report id");
  const row = oneReport(db, id);
  if (!row) fail(`no report with id ${id}`);
  const v = toView(row);
  if (json) {
    console.log(JSON.stringify(v, null, 2));
    return;
  }
  console.log(`Report #${v.id}`);
  console.log(`Filed:  ${v.at}  (window ${v.windowId})`);
  console.log(`\nMessage:\n${v.message}`);
  if (v.files.length) console.log(`\nFiles touched:\n  ${v.files.join("\n  ")}`);
  if (v.commands.length) console.log(`\nEx commands:\n  ${v.commands.join("\n  ")}`);
  console.log(`\nContext (recent activity):\n${v.context}`);
}

function cmdDelete(db: DatabaseSync, idStr: string | undefined): void {
  const id = Number(idStr);
  if (!idStr || !Number.isInteger(id)) fail("delete requires a numeric report id");
  const row = oneReport(db, id);
  if (!row) fail(`no report with id ${id}`);
  db.prepare("DELETE FROM reports WHERE id = ?").run(id);
  console.log(`Deleted report #${id}.`);
}

function usage(): never {
  console.log(
    [
      "reports-tool — browse and manage `:report` items in the garden state db",
      "",
      "Usage:",
      "  node scripts/reports-tool.ts list [--json]",
      "  node scripts/reports-tool.ts show <id> [--json]",
      "  node scripts/reports-tool.ts delete <id>",
      "",
      "Options:",
      "  --db <path>   Override the state database path",
      "  --json        Emit machine-readable JSON",
    ].join("\n"),
  );
  process.exit(0);
}

function main(): void {
  const { cmd, rest, json, db: dbOverride } = parseArgs(process.argv.slice(2));
  if (!cmd || cmd === "help" || cmd === "--help" || cmd === "-h") usage();

  const db = openDb(dbPath(dbOverride));
  try {
    switch (cmd) {
      case "list":
        cmdList(db, json);
        break;
      case "show":
        cmdShow(db, rest[0], json);
        break;
      case "delete":
        cmdDelete(db, rest[0]);
        break;
      default:
        fail(`unknown command '${cmd}' (try: list, show, delete)`);
    }
  } finally {
    db.close();
  }
}

main();
