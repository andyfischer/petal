import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");

const DRAWS = "print(random(0, 1))\nprint(random(0, 1))\nprint(random(0, 1))";

function run(args: string[], env: Record<string, string> = {}) {
  const r = spawnSync(PETAL, args, {
    encoding: "utf-8",
    timeout: 10000,
    env: { ...process.env, ...env },
  });
  return { stdout: r.stdout, stderr: r.stderr, status: r.status };
}

describe("--seed / PETAL_SEED", () => {
  it("makes two invocations byte-identical", () => {
    const a = run(["run", "--seed", "7", "-e", DRAWS]);
    const b = run(["run", "--seed", "7", "-e", DRAWS]);
    expect(a.status).toBe(0);
    expect(a.stdout.trim().split("\n")).toHaveLength(3);
    expect(b.stdout).toBe(a.stdout);
  });

  it("advances the stream within a run (not three identical draws)", () => {
    const lines = run(["run", "--seed", "7", "-e", DRAWS]).stdout.trim().split("\n");
    expect(new Set(lines).size).toBe(3);
  });

  it("gives different output for a different seed", () => {
    const a = run(["run", "--seed", "7", "-e", DRAWS]);
    const b = run(["run", "--seed", "8", "-e", DRAWS]);
    expect(b.stdout).not.toBe(a.stdout);
  });

  it("accepts hex seeds, naming the same stream as the decimal spelling", () => {
    const hex = run(["run", "--seed", "0x1f", "-e", DRAWS]);
    const dec = run(["run", "--seed", "31", "-e", DRAWS]);
    expect(hex.stdout).toBe(dec.stdout);
  });

  it("treats seed 0 as a stable stream (xorshift has no zero state)", () => {
    const a = run(["run", "--seed", "0", "-e", DRAWS]);
    const b = run(["run", "--seed", "0", "-e", DRAWS]);
    expect(a.stdout).toBe(b.stdout);
    expect(a.stdout.trim()).not.toBe("");
  });

  it("honors PETAL_SEED with no flag, so embedders need no code change", () => {
    const a = run(["run", "-e", DRAWS], { PETAL_SEED: "7" });
    const b = run(["run", "-e", DRAWS], { PETAL_SEED: "7" });
    expect(a.stdout).toBe(b.stdout);
    expect(a.stdout).toBe(run(["run", "--seed", "7", "-e", DRAWS]).stdout);
  });

  it("lets the flag beat the env var", () => {
    const flag = run(["run", "--seed", "8", "-e", DRAWS], { PETAL_SEED: "7" });
    expect(flag.stdout).toBe(run(["run", "--seed", "8", "-e", DRAWS]).stdout);
  });

  it("is nondeterministic with neither knob set", () => {
    // The clock seed is the default; this is the behavior --seed exists to fix.
    const a = run(["run", "-e", DRAWS]);
    const b = run(["run", "-e", DRAWS]);
    expect(a.stdout).not.toBe(b.stdout);
  });
});
