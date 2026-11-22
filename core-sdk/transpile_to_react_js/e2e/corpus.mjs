// Transpile every .ptl under the given folders and check each output parses
// as JSX (esbuild transform). Prints a summary and the failures.
//
//   node corpus.mjs <petal-to-react binary> <folder>... [--verbose]
import { execFileSync, spawnSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { transformSync } from "esbuild";

const [bin, ...rest] = process.argv.slice(2);
const verbose = rest.includes("--verbose");
const dirs = rest.filter((a) => !a.startsWith("--"));
const files = [];
function walk(d) {
  for (const f of readdirSync(d)) {
    if (f === "node_modules" || f === "target" || f.startsWith(".") || f === "vendor") continue;
    const p = join(d, f);
    const st = statSync(p);
    if (st.isDirectory()) walk(p);
    else if (f.endsWith(".ptl")) files.push(p);
  }
}
dirs.forEach(walk);
const stats = { files: files.length, transpiled: 0, parseFail: 0, crash: 0, jsFail: 0, warnings: 0 };
const codes = {};
const failures = [];
for (const f of files) {
  const r = spawnSync(bin, [f], { encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.status !== 0) {
    if (/panicked/.test(r.stderr)) { stats.crash++; failures.push(["crash", f, r.stderr.split("\n").find((l) => /panicked/.test(l))]); }
    else { stats.parseFail++; if (verbose) failures.push(["petal-parse", f, r.stderr.trim().split("\n").pop()]); }
    continue;
  }
  stats.transpiled++;
  for (const m of r.stderr.matchAll(/warning\[([a-z-]+)\]/g)) { codes[m[1]] = (codes[m[1]] ?? 0) + 1; stats.warnings++; }
  // A project prints each module after a `// ── name ──` line.
  const parts = r.stdout.split(/^\/\/ ── (.*) ──$/m);
  const modules = parts.length > 1 ? parts.slice(1).filter((_, i) => i % 2 === 1) : [r.stdout];
  for (const code of modules) {
    try {
      transformSync(code, { loader: "jsx", format: "esm" });
    } catch (e) {
      stats.jsFail++;
      failures.push(["js", f, e.message.split("\n").slice(0, 3).join(" | ")]);
      break;
    }
  }
}
console.log(JSON.stringify(stats));
console.log(JSON.stringify(Object.entries(codes).sort((a, b) => b[1] - a[1])));
for (const [k, f, m] of failures) console.log(`${k}\t${f}\t${m}`);
