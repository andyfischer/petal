// Verifies the ```petal code blocks in the user-facing docs against the real
// runtime, so documentation can't drift from the language:
//
//   • every block must pass `petal check` (lex + parse + compile), which catches
//     renamed builtins, removed syntax, and typos;
//   • blocks whose lines are all `print(...) // expected` (plus plain bindings)
//     are executed and their stdout asserted against the trailing-comment values.
//
// A block that is intentionally illustrative (pseudo-code, elided with `...`,
// or importing a host module not available to the bare CLI) opts out with an
// `ignore` tag on the fence: ```petal ignore
//
// Design docs under docs/dev/ and the aspirational examples are excluded — they
// describe future/host syntax that isn't meant to compile standalone.

import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve, join } from "node:path";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const PETAL = resolve(repoRoot, "rust/target/debug/petal");

const EXCLUDED = [/(^|\/)dev\//, /(^|\/)examples\//];
const SKIP_TAGS = new Set(["ignore", "no-check"]);
const FENCE = /^```petal([^\n]*)\n([\s\S]*?)^```/gm;
const PRINT_WITH_EXPECTED = /^\s*print\(.*\)\s*\/\/\s*(.+?)\s*$/;
const BARE_PRINT = /^\s*print\(/;
const CONTROL_FLOW = /^\s*(for|while|if|elsif|else|match|end|fn)\b/;
// A trailing print-comment is only asserted when it's *literal* output. These
// signal the comment carries prose instead (a note, a type hint, an aside), so
// the block falls back to check-only rather than a spurious mismatch:
//   non-deterministic:  // 0.73... (varies)
//   parenthetical note: // 1 (zero-indexed)
//   type-hint quotes:   // "Alice"   (print writes Alice, unquoted)
//   dash aside:         // 6  — only double ran back into b
const NON_ASSERTABLE = /\bvaries\b|\brandom\b|warning:|\.\.\.|[("]|—|--/;

interface Snippet {
  file: string;
  index: number;
  line: number;
  code: string;
  tags: string[];
}

function docFiles(): string[] {
  const entries = readdirSync(join(repoRoot, "docs"), {
    recursive: true,
    encoding: "utf8",
  });
  return entries
    .filter((f) => f.endsWith(".md"))
    .filter((f) => !EXCLUDED.some((re) => re.test(f)))
    .sort();
}

function extractSnippets(relFile: string): Snippet[] {
  const text = readFileSync(join(repoRoot, "docs", relFile), "utf8");
  const snippets: Snippet[] = [];
  let m: RegExpExecArray | null;
  let index = 0;
  while ((m = FENCE.exec(text))) {
    index++;
    const tags = m[1].trim().split(/\s+/).filter(Boolean);
    const line = text.slice(0, m.index).split("\n").length;
    snippets.push({ file: relFile, index, line, code: m[2], tags });
  }
  return snippets;
}

function petalCheck(code: string): { ok: boolean; err: string } {
  const r = spawnSync(PETAL, ["check", "-e", code], {
    encoding: "utf8",
    timeout: 10000,
  });
  return { ok: r.status === 0, err: (r.stderr || "").trim() };
}

function petalRun(code: string): { ok: boolean; stdout: string; stderr: string } {
  const r = spawnSync(PETAL, ["run", "-e", code], {
    encoding: "utf8",
    timeout: 10000,
  });
  return {
    ok: r.status === 0,
    stdout: r.stdout || "",
    stderr: (r.stderr || "").trim(),
  };
}

/**
 * The ordered list of expected stdout lines a block asserts, or null when the
 * block isn't a clean sequence of annotated prints (only `print(...)` writes to
 * stdout, so an unannotated print or any control flow makes the mapping
 * ambiguous and we fall back to check-only).
 */
function expectedOutput(code: string): string[] | null {
  const expected: string[] = [];
  for (const raw of code.split("\n")) {
    const line = raw.replace(/\r$/, "");
    if (line.trim() === "" || line.trim().startsWith("//")) continue;
    const m = PRINT_WITH_EXPECTED.exec(line);
    if (m) {
      if (NON_ASSERTABLE.test(m[1])) return null;
      expected.push(m[1]);
      continue;
    }
    if (BARE_PRINT.test(line)) return null; // unannotated print
    if (CONTROL_FLOW.test(line)) return null; // unpredictable line count
    // Other lines (let bindings, bare expressions) produce no stdout.
  }
  return expected.length > 0 ? expected : null;
}

const snippets = docFiles().flatMap(extractSnippets);

describe("doc snippets", () => {
  it("found petal code blocks to verify", () => {
    expect(snippets.length).toBeGreaterThan(50);
  });

  for (const s of snippets) {
    const label = `${s.file}:${s.line} (block #${s.index})`;
    if (s.tags.some((t) => SKIP_TAGS.has(t))) continue;

    it(`compiles — ${label}`, () => {
      const { ok, err } = petalCheck(s.code);
      expect(ok, `\`petal check\` failed for ${label}:\n${err}\n\n${s.code}`).toBe(
        true,
      );
    });

    const expected = expectedOutput(s.code);
    if (expected) {
      it(`prints the documented output — ${label}`, () => {
        const { ok, stdout, stderr } = petalRun(s.code);
        expect(ok, `\`petal run\` failed for ${label}:\n${stderr}`).toBe(true);
        const actual = stdout.replace(/\n$/, "").split("\n").map((l) => l.trim());
        expect(actual).toEqual(expected);
      });
    }
  }
});
