#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Freeze the graph engine's output for every examples/*.ptl into a checked-in
// golden corpus (test/example-golden/<name>.json). This is the oracle
// replacement for the graph-vs-bytecode differential sweep: once the graph
// evaluator is removed, test-examples.ts diffs bytecode against these frozen
// captures instead of running graph live.
//
// Run this ONLY while the graph engine still exists. Regenerate deliberately
// (never as a side effect of a bytecode change) — a golden update is a claim
// that the reference behavior itself changed.
//
// Usage:  ./ts/bin/gen-example-golden.ts
import { spawnSync } from 'node:child_process';
import { readdirSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const examplesDir = join(repoRoot, 'examples');
const goldenDir = join(repoRoot, 'test', 'example-golden');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'debug', 'petal');

const build = spawnSync(
    'cargo',
    ['build', '--quiet', '--manifest-path', cargoToml],
    { stdio: 'inherit' },
);
if (build.status !== 0) process.exit(build.status ?? 1);

mkdirSync(goldenDir, { recursive: true });

const files = readdirSync(examplesDir).filter(f => f.endsWith('.ptl')).sort();
let count = 0;
for (const file of files) {
    const filePath = join(examplesDir, file);
    const result = spawnSync(petal, [filePath, '--backend=graph'], {
        encoding: 'utf-8',
    });
    const golden = {
        example: file,
        status: result.status,
        stdout: result.stdout ?? '',
        stderr: result.stderr ?? '',
    };
    const out = join(goldenDir, file.replace(/\.ptl$/, '.json'));
    writeFileSync(out, JSON.stringify(golden, null, 2) + '\n');
    count++;
}
console.log(`Wrote ${count} golden captures to ${goldenDir} (backend: graph)`);
