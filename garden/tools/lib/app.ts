// Building and launching Garden for the functional tests.
//
// Every integration test does the same three things before it can assert
// anything: build the crates it needs, start the app with `--debug-port 0`, and
// discover the port the app actually chose from its startup line. That is all
// this module is.

import { type ChildProcess, spawn } from "node:child_process";
import { openSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { DebugClient } from "./debug-client.ts";
import { cleanupOnExit, die, runOrDie, sleep } from "./util.ts";

/** The repo root — the parent of `tools/` (this file lives in `tools/lib/`). */
export const GARDEN_DIR = dirname(dirname(dirname(fileURLToPath(import.meta.url))));

/** The debug build of the `garden` binary, which most tests launch directly. */
export const GARDEN_BIN = join(GARDEN_DIR, "target", "debug", "garden");

export async function cargoBuild(packages: string[], opts: { quiet?: boolean } = {}): Promise<void> {
  console.log("building...");
  const args = ["build"];
  if (opts.quiet !== false) args.push("-q");
  for (const p of packages) args.push("-p", p);
  await runOrDie("cargo", args, { cwd: GARDEN_DIR, message: "build failed" });
}

export interface LaunchOptions {
  /** Arguments after the binary name (`--debug-port 0` is added for you). */
  args: string[];
  /** Working directory — the fixture repo, for the tests that need one. */
  cwd?: string;
  /** Where to tee the app's stdout+stderr; the port is read back out of it. */
  logPath: string;
  /**
   * Extra environment for the app (e.g. a redirected HOME). Replaces the
   * inherited environment rather than adding to it, so a caller that sets this
   * usually spreads `process.env` into it. `GARDEN_HEADLESS_IDLE_TIMEOUT` is
   * added underneath either way (see [`IDLE_TIMEOUT_SECONDS`]) and can be
   * overridden here.
   */
  env?: NodeJS.ProcessEnv;
  /** Binary to run; defaults to the debug `garden`. */
  bin?: string;
  /** What to call the app in the "did not start" message. */
  label?: string;
  /**
   * Feature flags (`GET /version` → `features`) this test needs, e.g.
   * `["cli.panel-wake", "state.values-filter"]`. Checked before the test runs
   * so a stale binary fails with "this build lacks X, rebuild" instead of a
   * confusing assertion twenty steps later — which is exactly how a stale
   * install used to be discovered. Names are listed in
   * `garden-app/src/version.rs`.
   */
  requireFeatures?: string[];
}

/** A launched app and a client pointed at its debug server. */
export interface LaunchedApp {
  child: ChildProcess;
  client: DebugClient;
  base: string;
  /** Is the process still running? */
  alive(): boolean;
  kill(): void;
}

/**
 * How long a test's app may sit without a debug request before it shuts itself
 * down (`GARDEN_HEADLESS_IDLE_TIMEOUT`, seconds).
 *
 * Every launch here gets it, and `kill()` below is the normal way a test's app
 * dies — this is for the runs where that never happens: a `kill -9`'d test, a
 * crashed harness, an agent session that goes away mid-run. Garden's own orphan
 * check handles most of those, but not a launcher that exits before its pid can
 * be sampled, which leaves a headless app holding a port forever. Generous
 * enough that no real test comes near it (they poll several times a second),
 * short enough that an abandoned one is gone within the hour.
 */
const IDLE_TIMEOUT_SECONDS = 600;

/**
 * Launch Garden with its debug server on a free port and wait for it to answer.
 *
 * The port is chosen by the OS (`--debug-port 0`), so it is discovered from the
 * app's own startup line in the log rather than assumed.
 */
export async function launchGarden(opts: LaunchOptions): Promise<LaunchedApp> {
  const bin = opts.bin ?? GARDEN_BIN;
  const log = openSync(opts.logPath, "w");
  const child = spawn(bin, [...opts.args, "--debug-port", "0"], {
    cwd: opts.cwd,
    env: {
      GARDEN_HEADLESS_IDLE_TIMEOUT: String(IDLE_TIMEOUT_SECONDS),
      ...(opts.env ?? process.env),
    },
    stdio: ["ignore", log, log],
  });
  child.on("error", (e) => die(`could not launch ${bin}: ${e.message}`));

  let killed = false;
  const kill = () => {
    if (killed) return;
    killed = true;
    try {
      child.kill();
    } catch {
      // Already gone.
    }
  };
  cleanupOnExit(kill);

  const base = await discoverBase(opts.logPath);
  if (!base) {
    console.error(`${opts.label ?? "app"} did not start; log:`);
    console.error(readLog(opts.logPath));
    process.exit(1);
  }
  console.log(`debug server at ${base}`);

  if (opts.requireFeatures?.length) await requireFeatures(base, bin, opts.requireFeatures);

  return {
    child,
    client: new DebugClient(base),
    base,
    alive: () => {
      try {
        // Signal 0 tests for the process without touching it.
        process.kill(child.pid!, 0);
        return true;
      } catch {
        return false;
      }
    },
    kill,
  };
}

/**
 * Fail fast when the launched binary predates a feature the test depends on.
 *
 * A 404 means the binary is older than `/version` itself, which is the same
 * diagnosis with a blunter instrument.
 */
async function requireFeatures(base: string, bin: string, wanted: string[]): Promise<void> {
  const stale = (why: string) =>
    die(
      `${bin} ${why}\n` +
        `  needs: ${wanted.join(", ")}\n` +
        "  rebuild with `cargo build -p garden-app` (or re-run garden/install-local.sh " +
        "if this is an installed binary)",
    );
  let report: { features?: string[]; build?: { commit?: string; build_date?: string } };
  try {
    const res = await fetch(`${base}/version`);
    if (res.status === 404) return stale("predates the /version endpoint");
    report = await res.json();
  } catch (e) {
    return stale(`could not be asked for its version (${(e as Error).message})`);
  }
  const have = new Set(report.features ?? []);
  const missing = wanted.filter((f) => !have.has(f));
  if (missing.length) {
    const b = report.build ?? {};
    stale(`is commit ${b.commit ?? "?"} (built ${b.build_date ?? "?"}) and lacks ${missing.join(", ")}`);
  }
}

/** Poll the app's log for the debug server's URL, then for the server itself. */
async function discoverBase(logPath: string): Promise<string | null> {
  for (let i = 0; i < 60; i++) {
    const m = readLog(logPath).match(/http:\/\/127\.0\.0\.1:(\d+)/);
    if (m) {
      const base = `http://127.0.0.1:${m[1]}`;
      if (await answers(base)) return base;
    }
    await sleep(250);
  }
  return null;
}

async function answers(base: string): Promise<boolean> {
  try {
    const res = await fetch(`${base}/state`);
    return res.ok;
  } catch {
    return false;
  }
}

function readLog(path: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
}
