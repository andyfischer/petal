#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Launch an example panel app (examples/<category>/<slug>/) in Garden.
//
//   tools/run-example.ts <example> [garden args...]
//   tools/run-example.ts snake
//   tools/run-example.ts games/snake --headless --debug-port 0
//   tools/run-example.ts --list
//
// <example> is a slug (`snake`), a `category/slug`, or a path to the example's
// directory. Inside an example's directory it may be left out. Everything
// after it is passed to garden.
//
// The Garden binary is garden/target/debug/garden, or wherever GARDEN_BIN
// points; this does not rebuild it. The headless viewport comes from the
// `// headless-size: WxH` comment in the example's layout.ptl, unless
// GARDEN_HEADLESS_SIZE is already set.
//
// Garden replaces this process (same pid, cwd = the example's directory), so
// signals and the exit status are Garden's own.

import { execFileSync } from 'node:child_process';
import { accessSync, constants, existsSync, readdirSync, readFileSync } from 'node:fs';
import { basename, join, relative, resolve } from 'node:path';

const repoRoot = resolve(import.meta.dirname, '..');
const examplesDir = join(repoRoot, 'examples');
const self = 'run-example';
// Garden's own headless default, for a layout.ptl that names no size.
const DEFAULT_HEADLESS_SIZE = '1280x850';

function fail(message: string, code = 1): never {
    for (const line of message.split('\n')) console.error(`${self}: ${line}`);
    process.exit(code);
}

const isExample = (dir: string) => existsSync(join(dir, 'layout.ptl'));

/** Every examples/<category>/<slug>/ that has a layout.ptl, as absolute paths. */
function listExamples(): string[] {
    const found: string[] = [];
    for (const category of readdirSync(examplesDir, { withFileTypes: true })) {
        if (!category.isDirectory()) continue;
        const categoryDir = join(examplesDir, category.name);
        for (const slug of readdirSync(categoryDir, { withFileTypes: true })) {
            const dir = join(categoryDir, slug.name);
            if (slug.isDirectory() && isExample(dir)) found.push(dir);
        }
    }
    return found.sort();
}

const label = (dir: string) => relative(examplesDir, dir);

function resolveExample(name: string): string {
    for (const dir of [resolve(name), join(examplesDir, name)]) {
        if (isExample(dir)) return dir;
    }
    const matches = listExamples().filter((dir) => basename(dir) === name);
    if (matches.length === 1) return matches[0];
    if (matches.length > 1) {
        fail(`"${name}" is ambiguous: ${matches.map(label).join(', ')}`);
    }
    fail(`no example named "${name}" (--list shows them all)`);
}

function headlessSize(exampleDir: string): string {
    const layout = readFileSync(join(exampleDir, 'layout.ptl'), 'utf8');
    return /^\/\/\s*headless-size:\s*(\d+x\d+)\s*$/m.exec(layout)?.[1] ?? DEFAULT_HEADLESS_SIZE;
}

function git(args: string[]): string | null {
    try {
        return execFileSync('git', ['-C', repoRoot, ...args], {
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
        }).trim();
    } catch {
        return null;
    }
}

// Stop rather than test old code: refuse a Garden binary that is behind this
// checkout. A garden/target/debug/garden left over from an earlier commit still
// launches, and then every language or host fix since looks unfixed. Garden
// prints a warning for this at startup, but a launch that goes to a log file
// hides it, so the launcher stops instead (exit 3). GARDEN_ALLOW_STALE=1 turns
// the stop into a banner and launches anyway.
//
// Returns quietly when the binary is current, or when it cannot be told (no
// git, a binary with no build stamp).
//
// "Stale" is Garden's own rule (garden-app/src/version.rs, SOURCE_PATHSPECS —
// keep the list below in step with it): source files under garden/, petal-ui/
// or rust/ differ between the commit the binary was built from and HEAD.
// Uncommitted edits are not counted.
function requireFreshGarden(garden: string): void {
    // First line of --version: "garden 0.1.0 (87bbf1c 2026-10-07, built …)".
    let version: string;
    try {
        version = execFileSync(garden, ['--version'], {
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
        });
    } catch {
        return;
    }
    const built = /^[^(]*\(([0-9a-f]{7,40})[ ,)]/.exec(version.split('\n')[0])?.[1];
    if (!built) return;
    const head = git(['rev-parse', '--short', 'HEAD']);
    if (head === null) return;

    const changed = git([
        'diff', '--name-only', built, 'HEAD', '--',
        ':(top)garden', ':(top)petal-ui', ':(top)rust',
        ':(top,exclude,glob)**/*.md', ':(top,exclude)garden/tools',
    ]);
    if (changed === '') return;
    const reason = changed === null
        ? 'that commit is not in this checkout'
        : `${changed.split('\n').length} source file(s) changed since`;

    const allowStale = process.env.GARDEN_ALLOW_STALE === '1';
    const banner = [
        '',
        '################################################################',
        '##  STALE GARDEN BINARY',
        '##',
        `##  ${garden}`,
        `##  was built from ${built}; the checkout is at ${head}`,
        `##  (${reason}).`,
        '##',
        `##  Rebuild it:   (cd ${join(repoRoot, 'garden')} && cargo build)`,
        ...(allowStale
            ? ['##', '##  GARDEN_ALLOW_STALE=1: launching it anyway. You are testing old code.']
            : ['##  Or run it as it is:   GARDEN_ALLOW_STALE=1 tools/run-example.ts …', '##', '##  NOT LAUNCHED.']),
        '################################################################',
        '',
    ];
    console.error(banner.join('\n'));
    if (!allowStale) process.exit(3);
}

function main(): void {
    const args = process.argv.slice(2);

    if (args[0] === '--list') {
        for (const dir of listExamples()) console.log(label(dir));
        return;
    }
    if (args[0] === '-h' || args[0] === '--help') {
        console.log('usage: tools/run-example.ts <example> [garden args...]\n       tools/run-example.ts --list');
        return;
    }

    let exampleDir: string;
    if (args.length > 0 && !args[0].startsWith('-')) {
        exampleDir = resolveExample(args.shift()!);
    } else if (isExample(process.cwd())) {
        exampleDir = process.cwd();
    } else {
        fail('usage: tools/run-example.ts <example> [garden args...]\n(--list shows the examples)', 2);
    }

    const garden = process.env.GARDEN_BIN
        ? resolve(process.env.GARDEN_BIN)
        : join(repoRoot, 'garden/target/debug/garden');
    try {
        accessSync(garden, constants.X_OK);
    } catch {
        fail(`garden binary not found at ${garden}\nbuild it, or point GARDEN_BIN at one`);
    }
    requireFreshGarden(garden);

    process.env.GARDEN_HEADLESS_SIZE ||= headlessSize(exampleDir);
    process.chdir(exampleDir);
    process.execve(garden, [garden, '--init', 'layout.ptl', ...args], process.env);
}

main();
