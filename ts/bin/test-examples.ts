#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Run every examples/*.ptl file and print a short slice of its output.
// Usage:
//   ./bin/test-examples.ts          # show first 8 lines of each example
//   ./bin/test-examples.ts --full   # show full output

import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const examplesDir = join(repoRoot, 'examples');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'debug', 'petal');
const full = process.argv.includes('--full');

const build = spawnSync(
    'cargo',
    ['build', '--quiet', '--manifest-path', cargoToml],
    { stdio: 'inherit' },
);
if (build.status !== 0) process.exit(build.status ?? 1);

const files = readdirSync(examplesDir).filter(f => f.endsWith('.ptl')).sort();
let pass = 0;
let fail = 0;

for (const name of files) {
    const filePath = join(examplesDir, name);
    console.log(`=== ${name} ===`);
    const result = spawnSync(petal, [filePath], { encoding: 'utf-8' });
    const output = (result.stdout ?? '') + (result.stderr ?? '');

    if (result.status === 0) {
        if (full) {
            process.stdout.write(output);
        } else {
            const lines = output.split('\n');
            const head = lines.slice(0, 8).join('\n');
            process.stdout.write(head);
            if (!head.endsWith('\n')) process.stdout.write('\n');
            if (lines.length > 8) console.log(`  ... (${lines.length} lines total)`);
        }
        pass++;
    } else {
        const head = output.split('\n').slice(0, 5).join('\n');
        console.log(`FAILED: ${head}`);
        fail++;
    }
    console.log();
}

console.log(`Results: ${pass} passed, ${fail} failed`);
process.exit(fail > 0 ? 1 : 0);
