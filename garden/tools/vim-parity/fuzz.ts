#!/usr/bin/env node
//
// Vim-parity differential fuzzer for the Garden editor.
//
// Generates random keystroke programs drawn from *only the Vim subset Garden
// implements*, runs each program against both
//
//   * real Vim (nvim -u NONE, the parity oracle), and
//   * a live headless Garden instance (driven over its debug server),
//
// starting from identical buffer + cursor state, then reports every case where
// the two disagree on the resulting buffer, cursor, or mode. Each disagreement is
// delta-debugged down to a minimal reproducer and clustered by signature.
//
// Divergences are, by construction, candidate Garden bugs: the generator never
// emits a key Garden doesn't implement, so "Garden did something different from
// Vim" is the whole point.
//
// Usage:
//     # Garden must already be running headless with a debug server:
//     #   cargo run -p garden-app -- --headless --debug-port 8091 <file>
//     node tools/vim-parity/fuzz.ts --port 8091 --count 500 --seed 1

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { DebugClient } from "../lib/debug-client.ts";
import { run } from "../lib/util.ts";

// ---------------------------------------------------------------------------
// Token model
//
// A "program" is a flat list of tokens. Each token executes identically in
// concept against both editors; the two backends just translate it differently.
//   ["key", ch]  a literal keypress (letters, digits, symbols, uppercase)
//   ["esc"]      Escape
//   ["enter"]    Enter / <CR>
//   ["bs"]       Backspace
//   ["ctrlr"]    Ctrl-R (redo)
// A terminating Escape is *always* appended at run time (not stored in the
// program) so both editors finish in a comparable Normal-mode state and any
// program prefix is a valid, self-terminating program during minimization.
// ---------------------------------------------------------------------------

export type Token = ["key", string] | ["esc"] | ["enter"] | ["bs"] | ["ctrlr"];
/** A semantic unit: a motion, an operator+motion, one insert session, … */
export type Move = Token[];

const VIM_BYTES: Record<string, string> = {
  esc: "\x1b",
  enter: "\r",
  bs: "\x08",
  ctrlr: "\x12",
};

/** Raw byte string for the program, WITHOUT the auto-terminating Escape. */
export function toVimBytes(program: Token[]): string {
  return program.map((t) => (t[0] === "key" ? t[1] : VIM_BYTES[t[0]])).join("");
}

/** Human-readable rendering of a program for reports. */
export function pretty(program: Token[]): string {
  const names: Record<string, string> = {
    esc: "<Esc>",
    enter: "<CR>",
    bs: "<BS>",
    ctrlr: "<C-R>",
  };
  return program
    .map((t) => (t[0] === "key" ? (t[1] === " " ? "<Spc>" : t[1]) : names[t[0]]))
    .join("");
}

export function flatten(moves: Move[]): Token[] {
  return moves.flat();
}

// ---------------------------------------------------------------------------
// Seeded RNG
//
// A run is reproducible from its `--seed`, which is all the fuzzer needs. This
// is a different generator from the Python version's Mersenne Twister, so a
// given seed does NOT reproduce the same cases as the old `fuzz.py`.
// ---------------------------------------------------------------------------

export class Rng {
  private s: number;

  constructor(seed: number) {
    // Spread a small integer seed across the whole word so seed 1 and seed 2
    // don't start out neighbours in the state space.
    this.s = (seed * 0x9e3779b1) >>> 0;
  }

  /** Uniform in [0, 1). */
  random(): number {
    this.s = (this.s + 0x6d2b79f5) >>> 0;
    let t = this.s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  /** Inclusive on both ends, like Python's `randint`. */
  randint(lo: number, hi: number): number {
    return lo + Math.floor(this.random() * (hi - lo + 1));
  }

  choice<T>(items: readonly T[]): T {
    return items[Math.floor(this.random() * items.length)];
  }

  /** One weighted pick, like Python's `choices(..., weights=…)[0]`. */
  weighted<T>(items: readonly T[], weights: readonly number[]): T {
    const total = weights.reduce((a, b) => a + b, 0);
    let r = this.random() * total;
    for (let i = 0; i < items.length; i++) {
      r -= weights[i];
      if (r < 0) return items[i];
    }
    return items[items.length - 1];
  }
}

// ---------------------------------------------------------------------------
// Garden driver (debug server over HTTP)
// ---------------------------------------------------------------------------

export interface EditorResult {
  lines: string[] | null;
  cursor: [number, number] | null;
  mode: string;
  ok?: boolean;
  err?: string;
}

export class Garden {
  private client: DebugClient;

  constructor(port: number) {
    this.client = new DebugClient(`http://127.0.0.1:${port}`);
  }

  state(): Promise<unknown> {
    return this.client.state();
  }

  async sendToken(t: Token): Promise<void> {
    switch (t[0]) {
      case "key":
        await this.client.key(t[1] === " " ? "space" : t[1]);
        break;
      case "esc":
        await this.client.key("escape");
        break;
      case "enter":
        await this.client.key("enter");
        break;
      case "bs":
        await this.client.key("backspace");
        break;
      case "ctrlr":
        await this.client.key("r", ["ctrl"]);
        break;
    }
  }

  /** Load `lines` into pane 0 and park the cursor at (0,0), Normal mode. */
  async reset(lines: string[]): Promise<void> {
    // Two escapes guarantee we leave insert / visual / operator-pending from
    // whatever the previous case left behind.
    await this.client.key("escape");
    await this.client.key("escape");
    await this.client.key("a", ["cmd"]); // select all
    await this.client.text(lines.join("\n"));
    await this.client.key("escape");
    // gg lands on first non-blank; 0 forces column 0 to match the oracle.
    await this.client.key("g");
    await this.client.key("g");
    await this.client.key("0");
  }

  async run(lines: string[], program: Token[]): Promise<EditorResult> {
    await this.reset(lines);
    for (const t of program) await this.sendToken(t);
    await this.client.key("escape"); // terminating escape
    const pane = await this.client.pane();
    // Raw split, with no trailing-newline fix-up: the oracle's `writefile`
    // output is trimmed to match this exactly.
    const buf = await this.client.buffer();
    return {
      lines: buf.split("\n"),
      cursor: [pane.cursor!.line, pane.cursor!.col],
      mode: pane.mode ?? "",
    };
  }
}

// ---------------------------------------------------------------------------
// Vim oracle — one `feedkeys(..., 'ntx')` process per case.
//
// We drive the oracle with vimscript `feedkeys(keys, 'ntx')`: the keys are
// processed **as if typed interactively** ('t'), with no remapping ('n'), and
// the call blocks until the whole typeahead is consumed ('x'). This is the
// faithful model of a user at the keyboard — a beep (e.g. `b` at column 0) does
// NOT discard the following keystrokes, and pending operator/count state carries
// across keys.
//
// Why not `nvim -s scriptfile` (an earlier version of this oracle)? Script
// replay is faithful for motions/operators/paste, but it **over-joins the undo
// history**: it does not break an undo block at Insert-mode `<Esc>` the way real
// typing does, so `A x<Esc> A y<Esc> u` collapses BOTH inserts into one block
// and a single `u` wrongly removes both. `feedkeys('ntx')` reproduces real vim's
// per-insert-session undo blocks. The two oracles were cross-checked and agree
// on 300 core + 300 paste cases; they differ only on the undo tier, where `-s`
// was the one diverging from interactive vim. See `README.md` and
// `oracle-xcheck.ts`.
//
// One process per case keeps cases hermetic (no mode/state bleed) and lets the
// batch run in parallel.
// ---------------------------------------------------------------------------

/** A double-quoted vimscript string for `feedkeys` — literal chars verbatim,
 *  special keys as `\<Esc>` / `\<CR>` / `\<BS>` / `\<C-R>` notation. */
export function toFeedkeys(program: Token[]): string {
  const special: Record<string, string> = {
    esc: "\\<Esc>",
    enter: "\\<CR>",
    bs: "\\<BS>",
    ctrlr: "\\<C-R>",
  };
  return program
    .map((t) => {
      if (t[0] !== "key") return special[t[0]];
      if (t[1] === "\\") return "\\\\";
      if (t[1] === '"') return '\\"';
      return t[1];
    })
    .join("");
}

export async function runVimCase(init: string[], program: Token[]): Promise<EditorResult> {
  // A trailing Esc normalizes to Normal mode so the dumped state is comparable
  // and any program prefix is a valid self-terminating program (minimization).
  const keys = toFeedkeys(program) + "\\<Esc>";
  const dir = await mkdtemp(join(tmpdir(), "vim-parity-"));
  try {
    const content = join(dir, "buf.txt");
    const script = join(dir, "s.vim");
    const lout = join(dir, "lines");
    const cout = join(dir, "cur");
    await writeFile(content, init.join("\n") + "\n");
    // Configure the oracle to *default Vim* semantics for the options that
    // matter here. `-u NONE` alone is too bare: it flips `startofline` OFF,
    // whereas real Vim (and Garden) land gg/G/dd/C on the first non-blank.
    // `whichwrap=` matches Garden's deliberate non-wrapping h/l/Space/BS.
    await writeFile(
      script,
      "set noswapfile noautoindent nosmartindent noexpandtab startofline whichwrap=\n" +
        `call feedkeys("${keys}", "ntx")\n` +
        `call writefile(getline(1,'$'), '${lout}')\n` +
        `call writefile([(line('.')-1).' '.(col('.')-1)], '${cout}')\n` +
        "qa!\n",
    );
    const r = await run("nvim", [
      "--headless",
      "-u",
      "NONE",
      "-n",
      "-i",
      "NONE",
      "-S",
      script,
      content,
    ]);
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
    if (lines.length && lines[lines.length - 1] === "") lines.pop(); // writefile's trailing newline
    const [cl, cc] = cur.trim().split(/\s+/).map(Number);
    return { lines, cursor: [cl, cc], mode: "n", ok: true };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

export interface Case {
  id: number;
  init: string[];
  moves: Move[];
}

/** Run every case against the oracle, `workers` processes at a time. */
export async function runVimBatch(
  cases: Case[],
  workers = 8,
): Promise<Map<number, EditorResult>> {
  const out = new Map<number, EditorResult>();
  let next = 0;
  const worker = async () => {
    while (next < cases.length) {
      const c = cases[next++];
      out.set(c.id, await runVimCase(c.init, flatten(c.moves)));
    }
  };
  await Promise.all(Array.from({ length: Math.min(workers, cases.length) }, worker));
  return out;
}

// ---------------------------------------------------------------------------
// Content + program generators
// ---------------------------------------------------------------------------

const WORDS = ["foo", "bar", "baz", "qux", "hello", "world", "a", "xy", "abc",
  "lorem", "ipsum", "the", "cat", "sat", "on", "mat", "x", "ok"];
const PUNCTY = ["(foo)", "[bar]", "{baz}", "a(b)c", "x=y;", "f(x);", "a,b,c", "()", "[]"];

export function genContent(rng: Rng): string[] {
  const n = rng.randint(1, 5);
  const lines: string[] = [];
  for (let i = 0; i < n; i++) {
    if (rng.random() < 0.12) {
      lines.push(""); // empty line
      continue;
    }
    const indent = " ".repeat(rng.choice([0, 0, 0, 2, 4]));
    const wc = rng.randint(1, 4);
    const chunks: string[] = [];
    for (let j = 0; j < wc; j++) {
      chunks.push(rng.random() < 0.25 ? rng.choice(PUNCTY) : rng.choice(WORDS));
    }
    lines.push(indent + chunks.join(" "));
  }
  return lines;
}

const INSERT_CHARS = [..."abcdefgABCXYZ012 .,()"];

function genInsertText(rng: Rng, allowEnter: boolean): Token[] {
  const n = rng.randint(1, 4);
  const toks: Token[] = [];
  for (let i = 0; i < n; i++) {
    if (allowEnter && rng.random() < 0.15) toks.push(["enter"]);
    else toks.push(["key", rng.choice(INSERT_CHARS)]);
  }
  // occasional backspace
  if (rng.random() < 0.15) toks.push(["bs"]);
  return toks;
}

function maybeCount(rng: Rng, p = 0.35, hi = 6): Token[] {
  if (rng.random() < p) {
    return [...String(rng.randint(1, hi))].map((d) => ["key", d] as Token);
  }
  return [];
}

const MOTIONS: Move[] = [
  [["key", "h"]], [["key", "j"]], [["key", "k"]], [["key", "l"]],
  [["key", "w"]], [["key", "b"]], [["key", "e"]],
  [["key", "0"]], [["key", "$"]], [["key", "%"]],
  [["key", "G"]], [["key", "g"], ["key", "g"]],
];

function genMotion(rng: Rng): Move {
  const mot = rng.choice(MOTIONS);
  // `{count}%` is Vim's go-to-percentage-of-file motion, which Garden doesn't
  // implement (its `%` is bracket-match only). Skip the count there to avoid
  // flagging that known unimplemented feature.
  if (mot.length === 1 && mot[0][0] === "key" && mot[0][1] === "%") return mot;
  return [...maybeCount(rng), ...mot];
}

function genOperator(rng: Rng, allowEnter: boolean): Move {
  const op = rng.choice(["d", "c", "y"]);
  const toks: Token[] = [...maybeCount(rng), ["key", op]];
  if (rng.random() < 0.4) toks.push(["key", op]); // doubled: dd / cc / yy
  else toks.push(...genMotion(rng));
  if (op === "c") toks.push(...genInsertText(rng, allowEnter), ["esc"]);
  return toks;
}

function genEdit(rng: Rng, allowEnter: boolean): Move {
  // NB: p / P are NOT here — a bare paste reads Garden's register, which
  // persists across test resets (unlike a fresh Vim process), so it would
  // paste stale content. Paste parity is covered by the self-priming `paste`
  // tier instead.
  const kind = rng.choice(["x", "D", "C", "s", "S", "J", "r"]);
  switch (kind) {
    case "x":
      return [...maybeCount(rng), ["key", "x"]];
    case "D":
      return [["key", "D"]];
    case "C":
      return [["key", "C"], ...genInsertText(rng, allowEnter), ["esc"]];
    case "s":
      return [...maybeCount(rng), ["key", "s"], ...genInsertText(rng, allowEnter), ["esc"]];
    case "S":
      return [...maybeCount(rng), ["key", "S"], ...genInsertText(rng, allowEnter), ["esc"]];
    case "J":
      return [...maybeCount(rng), ["key", "J"]];
    case "r":
      return [...maybeCount(rng, 0.2, 3), ["key", "r"], ["key", rng.choice([..."abXY.() "])]];
    default:
      return [];
  }
}

/** Guarantee a non-empty first line so col-0-forward primes/edits always
 *  affect the buffer (and thus the register / undo stack) deterministically. */
export function nonemptyFirstLine(rng: Rng, init: string[]): string[] {
  if (init.length === 0 || init[0] === "") {
    const head = `${rng.choice(WORDS)} ${rng.choice(WORDS)}`;
    return [head, ...init.slice(1)];
  }
  return init;
}

/**
 * A self-contained paste test. The prime must ALWAYS refresh the register
 * (Garden's register persists across resets, unlike a fresh Vim process), so
 * primes are restricted to ops that, from column 0 of a non-empty first line,
 * are guaranteed to yank/delete something in *both* editors — no no-op yanks
 * (`yb` at col 0) and no operator+motion combos Garden mis-composes (`yG`).
 */
export function genPasteProgram(rng: Rng, _allowEnter = false, _allowOpen = false, _allowUndo = false): Move[] {
  const primes: Move[] = [
    [["key", "y"], ["key", "y"]],                        // linewise: yank line
    [...maybeCount(rng, 0.35, 3), ["key", "y"], ["key", "y"]],
    [["key", "d"], ["key", "d"]],                        // linewise: delete line
    [["key", "y"], ["key", "w"]],                        // charwise forward
    [["key", "y"], ["key", "e"]],
    [["key", "y"], ["key", "l"]],
    [["key", "d"], ["key", "w"]],
    [["key", "x"]],                                      // charwise single char
  ];
  const moves: Move[] = [rng.choice(primes)];
  const extra = rng.randint(0, 2);
  for (let i = 0; i < extra; i++) moves.push(genMotion(rng));
  moves.push([...maybeCount(rng, 0.3, 3), ["key", rng.choice(["p", "P"])]]);
  return moves.filter((mv) => mv.length > 0);
}

/**
 * A self-contained undo/redo test: perform k edits that EACH always create
 * exactly one undo step (insert-based, so they mutate regardless of buffer
 * state), then at most k undos (never reaching past the program's own edits
 * into the reset), then at most that many redos.
 */
export function genUndoProgram(rng: Rng, allowEnter = false, _allowOpen = false, _allowUndo = false): Move[] {
  const oneEdit = (): Move => {
    // A / I only: their forward behavior already matches Vim (no autoindent
    // difference like o/O), so any divergence AFTER undo/redo is squarely an
    // undo bug rather than a pre-existing content difference.
    const entry = rng.choice(["A", "I"]);
    // at least one non-space char so the edit is never empty
    const txt: Token[] = [["key", rng.choice([..."abcXY0"])], ...genInsertText(rng, allowEnter)];
    return [["key", entry], ...txt, ["esc"]];
  };

  const k = rng.randint(1, 3);
  const moves: Move[] = Array.from({ length: k }, oneEdit);
  const nu = rng.randint(1, k);
  for (let i = 0; i < nu; i++) moves.push([["key", "u"]]);
  const nr = rng.randint(0, nu);
  for (let i = 0; i < nr; i++) moves.push([["ctrlr"]]);
  return moves;
}

function genInsert(rng: Rng, allowEnter: boolean, allowOpen: boolean): Move {
  const entries = allowOpen ? ["i", "a", "I", "A", "o", "O"] : ["i", "a", "I", "A"];
  return [["key", rng.choice(entries)], ...genInsertText(rng, allowEnter), ["esc"]];
}

function genVisual(rng: Rng, allowEnter: boolean): Move {
  const toks: Token[] = [["key", rng.choice(["v", "V"])]];
  const n = rng.randint(1, 2);
  for (let i = 0; i < n; i++) toks.push(...genMotion(rng));
  // NB: '>' / '<' deliberately excluded — Garden indents with 4 spaces while
  // default Vim uses a tab at shiftwidth 8; that is a known design choice, not
  // a parity bug, and would just produce one guaranteed noise cluster.
  const op = rng.choice(["d", "y", "x", "~", "u", "U", "J", "c"]);
  toks.push(["key", op]);
  if (op === "c") toks.push(...genInsertText(rng, allowEnter), ["esc"]);
  return toks;
}

function genUndo(rng: Rng): Move {
  return rng.random() < 0.6 ? [["key", "u"]] : [["ctrlr"]];
}

/**
 * Return a list of *moves*; each move is a token list (a semantic unit).
 *
 * Keeping moves grouped lets the minimizer drop whole moves without ever
 * re-purposing insert-mode payload (e.g. a typed Space or ')') into a
 * normal-mode command — which would manufacture fake divergences.
 */
export function genProgram(
  rng: Rng,
  allowEnter = false,
  allowOpen = false,
  allowUndo = false,
): Move[] {
  const kinds = ["motion", "operator", "edit", "insert", "visual"];
  const weights = [3, 4, 3, 3, 3];
  if (allowUndo) {
    kinds.push("undo");
    weights.push(1);
  }
  const nmoves = rng.weighted([1, 2, 3, 4], [2, 4, 3, 2]);
  const moves: Move[] = [];
  for (let i = 0; i < nmoves; i++) {
    switch (rng.weighted(kinds, weights)) {
      case "motion":
        moves.push(genMotion(rng));
        break;
      case "operator":
        moves.push(genOperator(rng, allowEnter));
        break;
      case "edit":
        moves.push(genEdit(rng, allowEnter));
        break;
      case "insert":
        moves.push(genInsert(rng, allowEnter, allowOpen));
        break;
      case "visual":
        moves.push(genVisual(rng, allowEnter));
        break;
      case "undo":
        moves.push(genUndo(rng));
        break;
    }
  }
  return moves.filter((mv) => mv.length > 0);
}

/** The generator for each tier, by name. */
export const GENERATORS = {
  core: genProgram,
  paste: genPasteProgram,
  undo: genUndoProgram,
} as const;

export type Tier = keyof typeof GENERATORS;

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

export function normMode(m: string): string {
  const s = m.toLowerCase();
  if (s.startsWith("n")) return "normal";
  if (s.startsWith("i")) return "insert";
  if (s.startsWith("v") || s.startsWith("\x16") || s.includes("visual")) return "visual";
  return s;
}

/** Which aspects differ: 'lines', 'cursor', 'mode' (empty == match). */
export function diffFlags(vim: EditorResult, gar: EditorResult): string[] {
  const flags: string[] = [];
  if (JSON.stringify(vim.lines) !== JSON.stringify(gar.lines)) flags.push("lines");
  if (JSON.stringify(vim.cursor) !== JSON.stringify(gar.cursor)) flags.push("cursor");
  if (normMode(vim.mode) !== normMode(gar.mode)) flags.push("mode");
  return flags;
}

// ---------------------------------------------------------------------------
// Delta-debug minimization
// ---------------------------------------------------------------------------

/**
 * Greedily drop whole *moves* while the program still diverges the same way.
 *
 * Operating on moves (not raw tokens) keeps each insert/change move's typed
 * payload intact, so minimization can't turn inserted text into a command.
 */
export async function minimize(garden: Garden, init: string[], moves: Move[]): Promise<Move[]> {
  // Reduce toward *any* divergence, not the exact same flag set: this shrinks
  // a noisy multi-op program down to the single sub-behavior that actually
  // differs (the root cause), which is what we want to read and file.
  const diverges = async (mvs: Move[]): Promise<boolean> => {
    const prog = flatten(mvs);
    if (prog.length === 0) return false;
    const vim = await runVimCase(init, prog);
    if (vim.ok === false) return false;
    const gar = await garden.run(init, prog);
    return diffFlags(vim, gar).length > 0;
  };

  let mvs = [...moves];
  let changed = true;
  while (changed) {
    changed = false;
    let i = 0;
    while (i < mvs.length) {
      const cand = [...mvs.slice(0, i), ...mvs.slice(i + 1)];
      if (await diverges(cand)) {
        mvs = cand;
        changed = true;
      } else {
        i += 1;
      }
    }
  }

  // Light within-move trim: drop leading count prefixes, always a safe
  // reduction.
  for (let k = 0; k < mvs.length; k++) {
    let mv = mvs[k];
    while (mv.length && mv[0][0] === "key" && /^[0-9]$/.test(mv[0][1])) {
      const candMv = mv.slice(1);
      const cand = [...mvs.slice(0, k), candMv, ...mvs.slice(k + 1)];
      if (candMv.length && (await diverges(cand))) {
        mvs = cand;
        mv = candMv;
      } else {
        break;
      }
    }
  }
  return mvs;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

interface Args {
  port: number;
  count: number;
  seed: number;
  allowEnter: boolean;
  allowOpen: boolean;
  allowUndo: boolean;
  tier: Tier;
  out?: string;
}

const USAGE = `Usage: node tools/vim-parity/fuzz.ts --port N [options]

  --port N         debug-server port of a running headless Garden (required)
  --count N        number of cases to generate (default 300)
  --seed N         RNG seed; makes a run reproducible (default 1)
  --allow-enter    allow <CR> in insert mode (autoindent divergence)
  --allow-open     allow o/O (autoindent divergence on indented lines)
  --allow-undo     (core tier) allow u / <C-R>; over-undo reaches past the reset
  --tier T         core (motions/operators/edits/insert/visual, the default),
                   paste (self-priming yank+paste), or undo (bounded undo/redo)
  --out PATH       write the JSON report to PATH
`;

function parseArgs(argv: string[]): Args {
  const a: Args = {
    port: 0,
    count: 300,
    seed: 1,
    allowEnter: false,
    allowOpen: false,
    allowUndo: false,
    tier: "core",
  };
  for (let i = 0; i < argv.length; i++) {
    switch (argv[i]) {
      case "--port": a.port = Number(argv[++i]); break;
      case "--count": a.count = Number(argv[++i]); break;
      case "--seed": a.seed = Number(argv[++i]); break;
      case "--allow-enter": a.allowEnter = true; break;
      case "--allow-open": a.allowOpen = true; break;
      case "--allow-undo": a.allowUndo = true; break;
      case "--tier": a.tier = argv[++i] as Tier; break;
      case "--out": a.out = argv[++i]; break;
      case "-h":
      case "--help":
        console.log(USAGE);
        process.exit(0);
        break;
      default:
        console.error(`unknown argument: ${argv[i]}\n\n${USAGE}`);
        process.exit(2);
    }
  }
  if (!a.port) {
    console.error(`--port is required\n\n${USAGE}`);
    process.exit(2);
  }
  if (!(a.tier in GENERATORS)) {
    console.error(`--tier must be one of core, paste, undo\n\n${USAGE}`);
    process.exit(2);
  }
  return a;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const rng = new Rng(args.seed);
  const garden = new Garden(args.port);

  // sanity: is Garden reachable?
  try {
    await garden.state();
  } catch (e) {
    console.log(`ERROR: cannot reach Garden debug server on :${args.port}: ${e}`);
    process.exit(2);
  }

  // 1) generate
  const gen = GENERATORS[args.tier];
  const cases: Case[] = [];
  for (let i = 0; i < args.count; i++) {
    let init = genContent(rng);
    if (args.tier === "paste" || args.tier === "undo") init = nonemptyFirstLine(rng, init);
    cases.push({ id: i, init, moves: gen(rng, args.allowEnter, args.allowOpen, args.allowUndo) });
  }

  // 2) run vim oracle (one nvim process per case, parallelized)
  console.log(`running ${cases.length} cases against Vim oracle...`);
  const vimResults = await runVimBatch(cases);

  // 3) run garden, compare
  console.log("running cases against Garden + comparing...");
  const divergences: Array<{ case: Case; vim: EditorResult; garden: EditorResult; flags: string[] }> = [];
  for (const [idx, c] of cases.entries()) {
    if (idx % 50 === 0) {
      console.log(`  ${idx}/${cases.length}  (divergences so far: ${divergences.length})`);
    }
    const vim = vimResults.get(c.id);
    if (!vim || vim.ok === false) continue; // vim itself errored on this program; skip
    let gar: EditorResult;
    try {
      gar = await garden.run(c.init, flatten(c.moves));
    } catch (e) {
      console.log(`  garden error on case ${c.id}: ${e}`);
      continue;
    }
    const flags = diffFlags(vim, gar);
    if (flags.length) divergences.push({ case: c, vim, garden: gar, flags });
  }

  console.log(`\nraw divergences: ${divergences.length} / ${cases.length}`);

  // 4) cluster by coarse signature, minimize one exemplar per cluster
  const clusters = new Map<string, typeof divergences>();
  for (const d of divergences) {
    const prog = flatten(d.case.moves);
    const kinds = [...new Set(prog.map((t) => (t[0] === "key" ? t[1] : t[0])))].sort();
    const sig = JSON.stringify([d.flags, kinds]);
    if (!clusters.has(sig)) clusters.set(sig, []);
    clusters.get(sig)!.push(d);
  }

  console.log(`clusters: ${clusters.size}. Minimizing one exemplar each...`);
  const report = [];
  for (const members of [...clusters.values()].sort((a, b) => b.length - a.length)) {
    const ex = members[0];
    const { init, moves } = ex.case;
    // Only the core tier is safe to shrink by dropping moves. The paste /
    // undo tiers rely on earlier moves priming the register / undo stack;
    // removing them would re-expose cross-test state (a bare `p` pasting a
    // previous case's yank) and manufacture a fake reproducer.
    const mini = args.tier === "core" ? flatten(await minimize(garden, init, moves)) : flatten(moves);
    // recompute results on the minimized program for display
    const vim = await runVimCase(init, mini);
    const gar = await garden.run(init, mini);
    report.push({
      flags: ex.flags,
      count: members.length,
      init,
      program: pretty(flatten(moves)),
      minimal_init: init,
      minimal_program: pretty(mini),
      vim: { lines: vim.lines, cursor: vim.cursor, mode: normMode(vim.mode) },
      garden: { lines: gar.lines, cursor: gar.cursor, mode: normMode(gar.mode) },
    });
  }

  // 5) print report
  console.log("\n" + "=".repeat(72));
  console.log(`VIM-PARITY REPORT  seed=${args.seed} count=${args.count}`);
  console.log(`raw divergences ${divergences.length} in ${clusters.size} clusters`);
  console.log("=".repeat(72));
  for (const r of [...report].sort((a, b) => b.count - a.count)) {
    console.log(`\n### [${r.flags.join(",")}]  x${r.count}   minimal: ${r.minimal_program}`);
    console.log(`    init:   ${JSON.stringify(r.minimal_init)}`);
    console.log(`    keys:   ${r.minimal_program}`);
    console.log(`    vim   : lines=${JSON.stringify(r.vim.lines)} cursor=${JSON.stringify(r.vim.cursor)} mode=${r.vim.mode}`);
    console.log(`    garden: lines=${JSON.stringify(r.garden.lines)} cursor=${JSON.stringify(r.garden.cursor)} mode=${r.garden.mode}`);
  }

  if (args.out) {
    await writeFile(
      args.out,
      JSON.stringify(
        { seed: args.seed, count: args.count, raw: divergences.length, clusters: report },
        null,
        2,
      ),
    );
    console.log(`\nJSON report written to ${args.out}`);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await main();
}
