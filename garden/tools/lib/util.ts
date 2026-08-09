// Small shared helpers: subprocesses, polling, throwaway working directories,
// and cleanup that runs however the script exits.

import { spawn, type SpawnOptions } from "node:child_process";
import { rmSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Run a command to completion, capturing its output. Never throws on a
 *  non-zero exit — callers decide what a failure means. */
export function run(
  cmd: string,
  args: string[],
  opts: { cwd?: string; env?: NodeJS.ProcessEnv; input?: string } = {},
): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, {
      cwd: opts.cwd,
      env: opts.env ?? process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (d) => (stdout += d));
    child.stderr?.on("data", (d) => (stderr += d));
    child.on("error", reject);
    child.on("close", (code) => resolve({ code: code ?? -1, stdout, stderr }));
    if (opts.input !== undefined) child.stdin?.end(opts.input);
    else child.stdin?.end();
  });
}

/** Run a command, inheriting stdio, and fail the script if it exits non-zero. */
export async function runOrDie(
  cmd: string,
  args: string[],
  opts: SpawnOptions & { message?: string } = {},
): Promise<void> {
  const { message, ...spawnOpts } = opts;
  const code = await new Promise<number>((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: "inherit", ...spawnOpts });
    child.on("error", reject);
    child.on("close", (c) => resolve(c ?? -1));
  });
  if (code !== 0) die(message ?? `${cmd} ${args.join(" ")} failed (exit ${code})`);
}

export function die(message: string): never {
  console.error(`error: ${message}`);
  process.exit(1);
}

/**
 * Poll `read` until `done` accepts its value, or the deadline passes.
 * Returns the last value read either way — a timed-out poll is not an error
 * here; the assertion that follows is what reports it.
 */
export async function waitUntil<T>(
  read: () => Promise<T>,
  done: (v: T) => boolean,
  opts: { tries?: number; intervalMs?: number } = {},
): Promise<T> {
  const tries = opts.tries ?? 40;
  const intervalMs = opts.intervalMs ?? 250;
  let last = await read();
  for (let i = 0; i < tries && !done(last); i++) {
    await sleep(intervalMs);
    last = await read();
  }
  return last;
}

/** A throwaway directory under $TMPDIR, removed by `cleanupOnExit`. */
export function makeWorkDir(prefix: string): Promise<string> {
  return mkdtemp(join(tmpdir(), `${prefix}-`));
}

const cleanups: Array<() => void> = [];
let cleanupInstalled = false;

/**
 * Register work to undo however the script ends — normal return, an uncaught
 * throw, or Ctrl-C. The callbacks must be synchronous: Node gives an `exit`
 * handler no chance to await anything.
 */
export function cleanupOnExit(fn: () => void): void {
  cleanups.push(fn);
  if (cleanupInstalled) return;
  cleanupInstalled = true;
  const runAll = () => {
    while (cleanups.length) {
      try {
        cleanups.pop()!();
      } catch {
        // A failed cleanup must not mask the exit code we're leaving with.
      }
    }
  };
  process.on("exit", runAll);
  for (const sig of ["SIGINT", "SIGTERM"] as const) {
    process.on(sig, () => {
      runAll();
      process.exit(1);
    });
  }
}

/** Remove a directory when the script exits (synchronously — see above). */
export function removeOnExit(dir: string): void {
  cleanupOnExit(() => {
    rmSync(dir, { recursive: true, force: true });
  });
}

/** Run `git` inside a repo with a fixed identity, failing loudly. */
export async function git(repo: string, ...args: string[]): Promise<string> {
  const r = await run("git", [
    "-C",
    repo,
    "-c",
    "user.email=t@t",
    "-c",
    "user.name=t",
    ...args,
  ]);
  if (r.code !== 0) die(`git ${args.join(" ")} failed: ${r.stderr.trim()}`);
  return r.stdout;
}
