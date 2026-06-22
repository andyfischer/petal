#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Scans the full git commit history for leaked credentials.
//
// Mirrors what the "Secret scan" GitHub Action does, for local use before a
// push or a public release. Uses gitleaks with the project config
// (.gitleaks.toml). Resolution order:
//
//   1. a `gitleaks` binary already on PATH
//   2. docker (pulls the official zricethezav/gitleaks image)
//
// Exits non-zero if any secret is found.
//
// Usage:
//   ./ts/bin/scan-secrets.ts            # scan entire history
//   ./ts/bin/scan-secrets.ts --staged   # scan only staged changes (pre-commit)

import { spawnSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const config = join(repoRoot, '.gitleaks.toml');

const staged = process.argv.slice(2).includes('--staged');
const mode = staged ? 'protect' : 'detect'; // scan full history by default
const extraArgs = staged ? ['--staged'] : [];

function hasCommand(cmd: string): boolean {
    return spawnSync('command', ['-v', cmd], { shell: true, stdio: 'ignore' }).status === 0;
}

function run(command: string, args: string[]): number {
    const result = spawnSync(command, args, { stdio: 'inherit', cwd: repoRoot });
    if (result.error) {
        console.error(`ERROR: failed to run ${command}: ${result.error.message}`);
        return 1;
    }
    return result.status ?? 1;
}

let exitCode: number;

if (hasCommand('gitleaks')) {
    console.log(`==> Running gitleaks (${mode}) via local binary`);
    exitCode = run('gitleaks', [
        mode,
        '--source', repoRoot,
        '--config', config,
        '--redact', '-v',
        ...extraArgs,
    ]);
} else if (hasCommand('docker')) {
    console.log(`==> Running gitleaks (${mode}) via docker`);
    exitCode = run('docker', [
        'run', '--rm',
        '-v', `${repoRoot}:/repo`,
        '-w', '/repo',
        'zricethezav/gitleaks:latest',
        mode,
        '--source', '/repo',
        '--config', '/repo/.gitleaks.toml',
        '--redact', '-v',
        ...extraArgs,
    ]);
} else {
    console.error("ERROR: neither 'gitleaks' nor 'docker' is available.");
    console.error('Install gitleaks (https://github.com/gitleaks/gitleaks) or Docker, then re-run.');
    process.exit(127);
}

if (exitCode === 0) {
    console.log('==> No leaks found.');
}

process.exit(exitCode);
