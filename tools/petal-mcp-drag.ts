// The engine behind petal-mcp.ts's `DragExplain` tool: run a `ui/drag` app
// headlessly under `petal-ui-run --drag-trace-full`, drive a drag into it,
// and report what the drag engine holds at one frame — the spec as built,
// the target table (where the dragged element lands in each reachable state,
// each target's distance from the element, what it drops), the chosen target
// and the branch of the spec down to it.
//
// Kept apart from the server (no MCP dependency) so test/vitest can drive it
// against the built binary. See core-libs/petal-ui/docs/drag.md
// ("Debugging") and docs/dev/headless-ui-run.md (`--drag-trace`).

import { execFile } from "node:child_process";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export const projectRoot = resolve(import.meta.dirname, "..");
export const uiRunBin = join(projectRoot, "core-libs/petal-ui/target/debug/petal-ui-run");

export type DragGesture = {
  /** Place key of the dragged element (or any registered target id). */
  target: string;
  /** Where to drag to: [x, y], or another element's key / target id. */
  to: [number, number] | string;
  /** Frames of movement (default 10). */
  frames?: number;
  /** Frame of the press (default 2). */
  at?: number;
};

export type DragExplainInput = {
  /** The app's source (an app that imports siblings needs `path`). */
  code?: string;
  /** Path to the app instead of `code`. */
  path?: string;
  /** The drag to perform. */
  drag?: DragGesture;
  /** A full petal-ui-run scenario instead of (or as well as) `drag`. */
  scenario?: { size?: [number, number]; frames?: number; events?: unknown[] };
  /** The frame to report (default: the last frame a drag was in flight). */
  frame?: number;
  size?: [number, number];
  seed?: number;
};

export type DragExplainResult = {
  /** The frame reported. */
  frame: number | null;
  /** The drags in flight at that frame, explained. */
  sessions: unknown[];
  /** Drops announced at that frame ({key, model, drop}). */
  drops: unknown[];
  /** Every frame a drag was in flight: [frame, chosen target id]. */
  timeline: [number, number | null][];
  /** Every drop of the run: {frame, key, drop}. */
  all_drops: unknown[];
  /** The model state at the end of the run. */
  final_state: Record<string, unknown> | null;
  /** A frame's runtime error, if the run failed. */
  error: string | null;
};

/** The scenario the input asks for: its own, plus the `drag` gesture. */
export function dragScenario(input: DragExplainInput): Record<string, unknown> {
  const base = input.scenario ?? {};
  const events = [...(base.events ?? [])];
  let frames = base.frames;
  if (input.drag) {
    const at = input.drag.at ?? 2;
    const n = input.drag.frames ?? 10;
    const drag: Record<string, unknown> = { target: input.drag.target, to: input.drag.to, frames: n };
    events.push({ at, drag });
    // Through the release and a couple of frames after it.
    frames = Math.max(frames ?? 0, at + n + 4);
  }
  const out: Record<string, unknown> = { events };
  const size = input.size ?? base.size;
  if (size) out.size = size;
  if (frames !== undefined) out.frames = frames;
  return out;
}

export function dragExplainArgs(app: string, scenarioPath: string, input: DragExplainInput): string[] {
  const args = [app, "--scenario", scenarioPath, "--drag-trace-full", "--error-format", "bare"];
  if (input.seed !== undefined) args.push("--seed", String(input.seed));
  return args;
}

/** Read the JSONL trace into the tool's answer. */
export function parseTrace(stdout: string, frame?: number): DragExplainResult {
  const records = stdout.split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  const timeline: [number, number | null][] = [];
  const allDrops: unknown[] = [];
  let last: any = null;
  for (const r of records) {
    const d = r.drag;
    if (d?.sessions?.length) {
      timeline.push([r.frame, d.sessions[0].choice ?? null]);
      last = r;
    }
    for (const drop of d?.drops ?? []) allDrops.push({ frame: r.frame, key: drop.key, drop: drop.drop });
  }
  const pick = frame !== undefined ? records.find((r) => r.frame === frame) ?? null : last;
  const failed = records.find((r) => r.error);
  const end = records[records.length - 1];
  return {
    frame: pick ? pick.frame : null,
    sessions: pick?.drag?.sessions ?? [],
    drops: pick?.drag?.drops ?? [],
    timeline,
    all_drops: allDrops,
    final_state: end ? userState(end.state) : null,
    error: failed ? failed.error : null,
  };
}

/** The app's own state: the engine's (`ui/…`) slots left out. */
function userState(state: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(state ?? {}).filter(([k]) => !k.startsWith("ui")));
}

function run(cmd: string, args: string[], timeout: number): Promise<{ stdout: string; stderr: string; code: number }> {
  return new Promise((done) => {
    execFile(cmd, args, { timeout, maxBuffer: 256 * 1024 * 1024 }, (error, stdout, stderr) => {
      done({ stdout, stderr, code: error ? ((error as any).code ?? 1) : 0 });
    });
  });
}

/** Build petal-ui-run if needed; an error message on failure. */
export async function ensureUiRun(): Promise<string | null> {
  const b = await run("cargo", [
    "build", "--quiet", "--manifest-path", join(projectRoot, "core-libs/petal-ui/Cargo.toml"), "--bin", "petal-ui-run",
  ], 600_000);
  return b.code === 0 ? null : `Build failed:\n${b.stderr}`;
}

/** Run the drag and explain it. Throws on a usage error (no app, bad scenario). */
export async function dragExplain(input: DragExplainInput): Promise<DragExplainResult & { stderr: string }> {
  if (!input.code && !input.path) throw new Error("DragExplain needs `code` or `path`");
  if (!input.drag && !input.scenario) throw new Error("DragExplain needs a `drag` gesture or a `scenario`");
  const dir = await mkdtemp(join(tmpdir(), "petal-drag-"));
  try {
    const app = input.path ? resolve(input.path) : join(dir, "app.ptl");
    if (!input.path) await writeFile(app, input.code!);
    const scenarioPath = join(dir, "scenario.json");
    await writeFile(scenarioPath, JSON.stringify(dragScenario(input)));
    const r = await run(uiRunBin, dragExplainArgs(app, scenarioPath, input), 60_000);
    if (r.code === 2) throw new Error(r.stderr.trim() || "petal-ui-run refused the app");
    return { ...parseTrace(r.stdout, input.frame), stderr: r.stderr };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}
