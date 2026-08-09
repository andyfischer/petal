#!/usr/bin/env node
//
// Cross-check the two candidate vim oracles against each other (no Garden).
//
// The fuzzer's oracle is `feedkeys(..., 'ntx')` (see `fuzz.runVimCase`). An
// earlier version used `nvim -s` keystroke replay. This tool re-runs both and
// reports where they disagree, documenting *why* the switch was made:
//
//   - core / paste : they agree everywhere (`-s` is faithful there), so the swap
//                    is safe.
//   - undo         : they diverge, because `-s` over-joins undo blocks across an
//                    Insert-mode `<Esc>` (a single `u` wrongly undoes two inserts).
//                    `feedkeys('ntx')` matches real interactive vim.
//
// Usage:  node tools/vim-parity/oracle-xcheck.ts {core|paste|undo} [count] [seed]

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  type EditorResult,
  GENERATORS,
  type Tier,
  type Token,
  flatten,
  genContent,
  nonemptyFirstLine,
  pretty,
  Rng,
  runVimCase,
  toVimBytes,
} from "./fuzz.ts";
import { run } from "../lib/util.ts";

/** The retired `nvim -s` keystroke-replay oracle, kept here so the oracle
 *  comparison stays reproducible. */
async function runVimS(init: string[], program: Token[]): Promise<EditorResult> {
  const dir = await mkdtemp(join(tmpdir(), "vim-xcheck-"));
  try {
    const content = join(dir, "buf.txt");
    const keysf = join(dir, "keys");
    const lout = join(dir, "lines");
    const cout = join(dir, "cur");
    await writeFile(content, init.join("\n") + "\n");
    const keys =
      toVimBytes(program) +
      "\x1b" +
      ":call writefile(getline(1,'$'), $VP_L)\r" +
      ":call writefile([(line('.')-1).' '.(col('.')-1)], $VP_C)\r" +
      ":qa!\r";
    await writeFile(keysf, Buffer.from(keys, "latin1"));
    const r = await run(
      "nvim",
      [
        "--headless", "-u", "NONE", "-n", "-i", "NONE",
        "-c",
        "set noswapfile noautoindent nosmartindent noexpandtab startofline whichwrap=",
        "-s", keysf, content,
      ],
      { env: { ...process.env, VP_L: lout, VP_C: cout } },
    );
    if (r.code !== 0) {
      return { lines: null, cursor: null, mode: "n", ok: false, err: r.stderr.trim() };
    }
    let raw: string;
    let cur: string;
    try {
      raw = await readFile(lout, "utf8");
      cur = await readFile(cout, "utf8");
    } catch {
      return { lines: null, cursor: null, mode: "n", ok: false, err: "no output" };
    }
    const lines = raw.split("\n");
    if (lines.length && lines[lines.length - 1] === "") lines.pop();
    const [cl, cc] = cur.trim().split(/\s+/).map(Number);
    return { lines, cursor: [cl, cc], mode: "n", ok: true };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

const tier = process.argv[2] as Tier;
if (!tier || !(tier in GENERATORS)) {
  console.error("Usage: node tools/vim-parity/oracle-xcheck.ts {core|paste|undo} [count] [seed]");
  process.exit(2);
}
const count = process.argv[3] ? Number(process.argv[3]) : 300;
const seed = process.argv[4] ? Number(process.argv[4]) : 1;

const rng = new Rng(seed);
const gen = GENERATORS[tier];
let disagree = 0;
const examples: Array<[string, string[], EditorResult, EditorResult]> = [];

for (let i = 0; i < count; i++) {
  let init = genContent(rng);
  if (tier === "paste" || tier === "undo") init = nonemptyFirstLine(rng, init);
  const moves = flatten(gen(rng, false, false, false));
  const s = await runVimS(init, moves); // retired -s replay oracle
  const fk = await runVimCase(init, moves); // current feedkeys('ntx') oracle
  if (!(s.ok && fk.ok)) continue;
  if (
    JSON.stringify(s.lines) !== JSON.stringify(fk.lines) ||
    JSON.stringify(s.cursor) !== JSON.stringify(fk.cursor)
  ) {
    disagree += 1;
    if (examples.length < 10) examples.push([pretty(moves), init, s, fk]);
  }
  if (i % 50 === 0) console.log(`  ${i}/${count}  -s vs feedkeys disagreements:${disagree}`);
}

console.log(`\n[${tier}] -s vs feedkeys('ntx') disagreements: ${disagree}/${count}`);
for (const [pr, init, s, fk] of examples) {
  console.log(`  ${JSON.stringify(pr)} init=${JSON.stringify(init)}`);
  console.log(`     -s (retired): ${JSON.stringify(s.lines)} cur=${JSON.stringify(s.cursor)}`);
  console.log(`     feedkeys    : ${JSON.stringify(fk.lines)} cur=${JSON.stringify(fk.cursor)}`);
}
