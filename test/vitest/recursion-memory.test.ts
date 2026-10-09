// Recursion stays cheap in memory. Memo records used to cost a call path
// each, so naive `fib(27)` took 541 MB under the default policy and linear
// recursion was quadratic in depth (479 MB at 4,900 frames). Three things
// keep it flat now: `petal run` does not memoize unless a policy asks for it
// (the script runs once, so no record is ever replayed), memo scopes stop at
// `MAX_SCOPE_FRAME_DEPTH` frames, and the memo table is bounded in bytes
// (`MAX_TABLE_BYTES`). See core/src/memo.rs and docs/dev/memo-scopes.md.
//
// Peak memory is read from `/usr/bin/time` (`-l` on macOS, `-v` with GNU
// time), which is the only portable way to get a child's peak RSS from node.
// The ceilings are several times what the debug binary measures, and several
// times under what each case took before.

import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { existsSync } from "fs";
import { PETAL } from "./helpers";

const TIME = "/usr/bin/time";
const MB = 1024 * 1024;

/** Run a script under `time`; returns stdout and the peak RSS in bytes. */
function measure(code: string, flags: string[] = []) {
  const timeFlag = process.platform === "darwin" ? "-l" : "-v";
  const r = spawnSync(TIME, [timeFlag, PETAL, "run", ...flags, "-e", code], {
    encoding: "utf-8",
    timeout: 60000,
  });
  const mac = /(\d+)\s+maximum resident set size/.exec(r.stderr);
  const gnu = /Maximum resident set size \(kbytes\): (\d+)/.exec(r.stderr);
  const rss = mac ? Number(mac[1]) : gnu ? Number(gnu[1]) * 1024 : NaN;
  return { stdout: r.stdout, stderr: r.stderr, code: r.status, rss };
}

const FIB = "fn fib(n) if n < 2 then n else fib(n - 1) + fib(n - 2) end end\n";
const DOWN = "fn down(n) if n == 0 then 0 else 1 + down(n - 1) end end\n";

describe.skipIf(!existsSync(TIME))("recursion memory", () => {
  it("runs fib(30) in a few MB", () => {
    // 4.6 MB release, 10 MB debug; 1.3M calls.
    const r = measure(FIB + "print(fib(30))");
    expect(r.stdout).toBe("832040\n");
    expect(r.rss).toBeLessThan(48 * MB);
  }, 60000);

  it("runs a depth-10,000 linear recursion in a few MB", () => {
    // 10 MB release, 15 MB debug: about 0.55 KB a frame.
    const r = measure(DOWN + "print(down(10000))");
    expect(r.stdout).toBe("10000\n");
    expect(r.rss).toBeLessThan(48 * MB);
  });

  it("keeps deep recursion linear with memoization on", () => {
    // Was quadratic: 479 MB at depth 4,900 under `fast`.
    const r = measure(DOWN + "print(down(10000))", ["--policy", "fast"]);
    expect(r.stdout).toBe("10000\n");
    expect(r.rss).toBeLessThan(48 * MB);
  });

  it("bounds the memo table when memoization is asked for", () => {
    // fib(25) makes 243k calls, enough to fill the table: about 135 MB at
    // the byte bound, where the record-count bound alone let it reach 500.
    const r = measure(FIB + "print(fib(25))", ["--policy", "fast"]);
    expect(r.stdout).toBe("75025\n");
    expect(r.rss).toBeLessThan(256 * MB);
  }, 60000);
});
