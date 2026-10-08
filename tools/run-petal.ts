#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Runs the Petal binary, rebuilding it first if any Rust source is newer
// than the binary. Forwards all command-line args. Use this instead of
// calling rust/target/debug/petal directly — it keeps the binary in sync
// with source while avoiding a full `cargo build` on every call.

import { spawnSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const rustDir = join(repoRoot, 'rust');
const binary = join(rustDir, 'target', 'debug', 'petal');
const srcDir = join(rustDir, 'src');
const cargoToml = join(rustDir, 'Cargo.toml');
const cargoLock = join(rustDir, 'Cargo.lock');

function newestMtime(path: string): number {
    const stat = statSync(path);
    if (!stat.isDirectory()) return stat.mtimeMs;
    let newest = stat.mtimeMs;
    for (const entry of readdirSync(path)) {
        newest = Math.max(newest, newestMtime(join(path, entry)));
    }
    return newest;
}

function isStale(): boolean {
    let binMtime: number;
    try {
        binMtime = statSync(binary).mtimeMs;
    } catch {
        return true;
    }
    const sources = [newestMtime(srcDir), statSync(cargoToml).mtimeMs];
    try { sources.push(statSync(cargoLock).mtimeMs); } catch {}
    return Math.max(...sources) > binMtime;
}

if (isStale()) {
    process.stderr.write('[run-petal] rebuilding…\n');
    const build = spawnSync(
        'cargo',
        ['build', '--quiet', '--manifest-path', cargoToml],
        { stdio: 'inherit' },
    );
    if (build.status !== 0) {
        process.exit(build.status ?? 1);
    }
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
process.exit(result.status ?? 1);
