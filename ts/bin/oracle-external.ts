#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// The frame-gate / memo differential oracle, run over a Petal codebase that
// lives outside this repo — today, worlds-fair's UI (`~/worlds-fair/ui/ptl`).
//
// Garden is in-tree, so its panels and GPP apps are part of the cargo corpus
// (petal-ui/tests/common/mod.rs) and run in CI. worlds-fair is not, and it
// needs two things this repo cannot generate:
//
//   - a *bundle*. A worlds-fair fragment is delivered as one flat source
//     string: the Garden host shim, then lib/theme, basics, layout, widgets,
//     parts, then the screen. The order is load-bearing (Petal resolves a name
//     at its call site and does not hoist). This mirrors `wf-ui-core`'s
//     bundle.rs; if that file's LIBS order changes, change LIBS below.
//   - *fixtures*. The screens render `wf_model()`, which under the Garden shim
//     is `query("model", <fixture>)`. With no answer every screen draws
//     "waiting for the game…" — two draw commands — and the differential
//     passes while testing nothing. `wf-ui --print-fixtures` dumps the real
//     fixture models in the shape `petal-ui-run --query-fixtures` takes.
//
// For each fragment it runs a monkey scenario under the `baseline` run policy
// (no optimizer, memo or gate) and under each shipped policy — `fast-memo` (the
// gate alone), `replay` (the memo alone), `replay-declared` (the memo
// classifying every native by inference instead of its declared effect row)
// and `fast` (both) — and compares the frames.
// `prints` is excluded deliberately — a gated frame documents empty prints.
//
// Usage:
//   ./ts/bin/oracle-external.ts [--wf <dir>] [--frames N] [--seed N]
//
// Prerequisites:
//   cd petal-ui && cargo build --release
//   cd ~/worlds-fair/ui && cargo build --release -p wf-ui-garden
//
// Exits non-zero if any fragment's frames differ, or if any ran vacuously.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
const flag = (name: string, fallback: string) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};

const wfRoot = resolve(flag('--wf', join(process.env.HOME ?? '', 'worlds-fair')));
const frames = Number(flag('--frames', '45'));
const seed = flag('--seed', '1');
const runner = join(root, 'petal-ui/target/release/petal-ui-run');
const wfBin = join(wfRoot, 'ui/target/release/wf-ui');
const ptl = join(wfRoot, 'ui/ptl');

for (const [what, path] of [
  ['petal-ui-run', runner],
  ['wf-ui', wfBin],
  ['worlds-fair ptl', ptl],
] as const) {
  if (!existsSync(path)) {
    console.error(`missing ${what}: ${path}\n(see the prerequisites in this script's header)`);
    process.exit(2);
  }
}

// Concatenated ahead of every fragment, in this order. Mirrors
// wf-ui-core/src/bundle.rs: the host shim first (lib/parts.ptl calls
// `wf_action`, which the Garden shim defines in Petal), then the libraries in
// dependency order.
const LIBS = [
  'host/garden.ptl',
  'lib/theme.ptl',
  'lib/basics.ptl',
  'lib/layout.ptl',
  'lib/widgets.ptl',
  'lib/parts.ptl',
];
// Every fragment with a distinct bundle. The eleven parts all share
// dev/solo.ptl and differ only in which fragment the registry reports, so one
// entry covers them.
const FRAGMENTS: [string, string][] = [
  ['main_menu', 'screens/main_menu.ptl'],
  ['direct_connect', 'screens/direct_connect.ptl'],
  ['server_browser', 'screens/server_browser.ptl'],
  ['hud', 'screens/hud.ptl'],
  ['pause_menu', 'screens/pause_menu.ptl'],
  ['scenarios', 'screens/scenarios.ptl'],
  ['levels', 'screens/levels.ptl'],
  ['missions', 'screens/missions.ptl'],
  ['tuning', 'screens/tuning.ptl'],
  ['settings', 'screens/settings.ptl'],
  ['components', 'dev/components.ptl'],
  ['solo', 'dev/solo.ptl'],
];

const work = mkdtempSync(join(tmpdir(), 'petal-oracle-'));
writeFileSync(join(work, 'fixtures.json'), execFileSync(wfBin, ['--print-fixtures']));

/** One frame, reduced to what must match. `prints` is excluded on purpose. */
const normalize = (trace: string) =>
  trace
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const f = JSON.parse(line);
      return JSON.stringify({ c: f.commands, s: f.state, e: f.error });
    });

/** Run policies (see rust/src/policy.rs), each compared against `baseline`. */
const VARIANTS = ['fast-memo', 'replay', 'replay-declared', 'fast'] as const;

function drive(app: string, policy: 'baseline' | (typeof VARIANTS)[number]): string[] {
  const out = join(work, `${app}.jsonl`);
  execFileSync(runner, [
    join(work, `${app}.ptl`),
    '--frames', String(frames),
    '--seed', seed,
    '--scenario', `monkey:${seed}`,
    '--query-fixtures', join(work, 'fixtures.json'),
    '--error-format', 'bare',
    '--out', out,
    '--policy', policy,
  ]);
  return normalize(readFileSync(out, 'utf8'));
}

let failures = 0;
for (const [name, entry] of FRAGMENTS) {
  const source = [...LIBS, entry]
    .map((f) => `// ==== ${f} ====\n${readFileSync(join(ptl, f), 'utf8')}\n`)
    .join('');
  writeFileSync(join(work, `${name}.ptl`), source);

  const base = drive(name, 'baseline');
  // A fragment stuck on its "waiting for the game…" path draws two commands a
  // frame and proves nothing. That is what this corpus looked like before the
  // fixtures existed, and it read as a pass.
  const drawn = base.reduce((n, f) => n + (JSON.parse(f).c?.length ?? 0), 0) / base.length;
  if (drawn < 10) {
    console.error(`VACUOUS ${name}: ${drawn.toFixed(1)} commands/frame — fixtures not applying?`);
    failures++;
    continue;
  }

  const bad: string[] = [];
  for (const variant of VARIANTS) {
    const got = drive(name, variant);
    const i = base.findIndex((f, j) => f !== got[j]);
    if (i >= 0) bad.push(`${variant} differs at frame ${i}`);
  }
  if (bad.length) {
    console.error(`DIFF ${name}: ${bad.join('; ')}`);
    failures++;
  } else {
    console.log(`ok   ${name}  (${drawn.toFixed(0)} commands/frame)`);
  }
}

console.log(
  failures ? `\n${failures} of ${FRAGMENTS.length} fragments failed` : `\nall ${FRAGMENTS.length} fragments agree`,
);
process.exit(failures ? 1 : 0);
