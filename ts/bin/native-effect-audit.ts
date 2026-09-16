#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Which registered natives declare what they do, and which reach host state
// without declaring anything.
//
// Three layers — the frame gate, memoized scopes, and P2's dependency classes
// — need to know what a native does, and there is no place where a native says
// so. See docs/tasks/declarative-effect-refactoring.md. Until that row exists,
// a native is classified by what it happens to call: the instrumented
// `PetalCxt` methods below move the activity counters by construction, and
// `note_host_read()` / `note_effect()` are the manual escape hatch for a native
// that reaches host state by some other route.
//
// A native that calls none of them is SILENT. That is correct for a pure
// function of its arguments (`sqrt`, `len`) and a live bug for anything else:
// with no activity delta the memo records nothing, so the enclosing scope is
// replayed as a pure function of its arguments and the native is never called
// again. A `panel_store_set` in a replayed scope silently stops persisting.
//
// This is a *static* approximation — the real tool is step 4 of that plan, a
// runtime `--effect-audit` that reports what each native was observed doing.
// This script is what you run in the meantime, and what found the five
// undeclared natives fixed in the same commit.
//
// Usage:
//   ./ts/bin/native-effect-audit.ts             # summary + suspect list
//   ./ts/bin/native-effect-audit.ts --all       # every native, one per line
//   ./ts/bin/native-effect-audit.ts --json      # machine-readable

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

/** Where natives are registered, by the repo whose contract they are. */
const REPOS: [string, string][] = [
  ['core', 'rust/src'],
  ['petal-ui', 'petal-ui/src'],
  ['petal-query', 'petal-query/src'],
  ['garden', 'garden'],
  ['integrations', 'integrations'],
];

/**
 * `PetalCxt` methods that move the activity counters, and the class each one
 * moves. Keep in step with rust/src/native_fn.rs — a method that starts
 * instrumenting itself and is not listed here shows up as a false SILENT.
 */
const INSTRUMENTED: Record<string, string> = {
  note_host_read: 'host_read',
  note_effect: 'effect',
  print: 'effect',
  rng_next_f64: 'effect',
  set_noise_seed: 'effect',
  next_counter: 'effect',
  set_counter: 'effect',
  resources_mut: 'effect',
  resources: 'resource_read',
  push_output: 'emit',
  emit: 'emit',
  binding: 'binding_read',
  binding_named: 'binding_read', // delegates to binding()
};

/**
 * Routes out of the argument list that do not go through an instrumented
 * method. A SILENT native whose body matches one of these is the suspect list:
 * it answers from, or writes to, something the runtime cannot see.
 */
const REACHES: [RegExp, string][] = [
  [/\bthread_local!|\.with\s*\(\s*\|/, 'thread_local'],
  [/std::fs::|File::|read_to_string|OpenOptions/, 'fs'],
  [/std::time::|Instant::|SystemTime/, 'clock'],
  [/std::env::|env::var/, 'env'],
  [/Command::new|std::process/, 'process'],
  [/\brand::|thread_rng/, 'rng'],
  [/reqwest|TcpStream|UdpSocket|Client::/, 'net'],
];

// Both registration spellings: the embedder API (`env.register_native`) and
// the core table (`table.register`), which is how the 111 builtins go in.
const REGISTRATION =
  /\b(?:register_native(?:_class)?|table\.register)\(\s*"([A-Za-z0-9_]+)"\s*,\s*([A-Za-z0-9_:]+)/g;
const SIGNATURE = /^[ \t]*(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z0-9_]+)\s*\(/gm;

function rustFiles(dir: string, out: string[] = []): string[] {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of entries) {
    if (name === 'target' || name === 'node_modules' || name === '.git') continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) rustFiles(path, out);
    else if (name.endsWith('.rs')) out.push(path);
  }
  return out;
}

/** Map every `fn name` in `text` to its body, by matching braces. */
function bodies(text: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of text.matchAll(SIGNATURE)) {
    const open = text.indexOf('{', m.index! + m[0].length - 1);
    if (open < 0) continue;
    let depth = 0;
    let i = open;
    for (; i < text.length; i++) {
      if (text[i] === '{') depth++;
      else if (text[i] === '}' && --depth === 0) break;
    }
    out.set(m[1], text.slice(open, i));
  }
  return out;
}

type Row = {
  repo: string;
  native: string;
  fn: string;
  classes: string[];
  how: 'manual' | 'instrumented' | 'SILENT' | 'no-body';
  reaches: string[];
  file: string;
};

const rows: Row[] = [];
for (const [repo, dir] of REPOS) {
  const allBodies = new Map<string, string>();
  const registrations: [string, string, string][] = [];
  for (const file of rustFiles(join(root, dir))) {
    const text = readFileSync(file, 'utf8');
    for (const [name, body] of bodies(text)) allBodies.set(name, body);
    for (const m of text.matchAll(REGISTRATION)) {
      registrations.push([m[1], m[2].split('::').pop()!, file]);
    }
  }
  for (const [native, fn, file] of registrations) {
    const body = allBodies.get(fn);
    if (body === undefined) {
      rows.push({ repo, native, fn, classes: [], how: 'no-body', reaches: [], file });
      continue;
    }
    const classes = [
      ...new Set(
        Object.entries(INSTRUMENTED)
          .filter(([method]) => new RegExp(`\\.${method}\\s*\\(`).test(body))
          .map(([, cls]) => cls),
      ),
    ].sort();
    const manual = /\.note_(host_read|effect)\s*\(/.test(body);
    const how = manual ? 'manual' : classes.length ? 'instrumented' : 'SILENT';
    const reaches =
      how === 'SILENT' ? REACHES.filter(([re]) => re.test(body)).map(([, tag]) => tag) : [];
    rows.push({ repo, native, fn, classes, how, reaches, file: relative(root, file) });
  }
}

if (process.argv.includes('--json')) {
  console.log(JSON.stringify(rows, null, 1));
  process.exit(0);
}

if (process.argv.includes('--all')) {
  for (const r of rows.sort((a, b) => `${a.repo}${a.native}`.localeCompare(`${b.repo}${b.native}`))) {
    console.log(
      `${r.repo.padEnd(13)}${r.native.padEnd(26)}${r.how.padEnd(14)}${r.classes.join(',') || '-'}`,
    );
  }
  console.log();
}

const pad = (s: string | number, n: number) => String(s).padEnd(n);
console.log(
  `${pad('repo', 14)}${pad('total', 7)}${pad('silent', 8)}${pad('instr', 7)}${pad('manual', 8)}`,
);
for (const [repo] of REPOS) {
  const mine = rows.filter((r) => r.repo === repo);
  if (!mine.length) continue;
  const count = (how: string) => mine.filter((r) => r.how === how).length;
  console.log(
    `${pad(repo, 14)}${pad(mine.length, 7)}${pad(count('SILENT'), 8)}` +
      `${pad(count('instrumented'), 7)}${pad(count('manual'), 8)}`,
  );
}

const suspects = rows.filter((r) => r.reaches.length);
console.log(`\nSILENT natives that reach outside their arguments: ${suspects.length}`);
for (const r of suspects.sort((a, b) => a.native.localeCompare(b.native))) {
  console.log(`  ${pad(r.repo, 13)}${pad(r.native, 24)}${pad(r.reaches.join(','), 18)}${r.file}`);
}
process.exit(suspects.length ? 1 : 0);
