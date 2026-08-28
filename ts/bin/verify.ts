#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// verify.ts — prove a refactor was behavior-preserving over a corpus of .ptl files.
//
// Two independent A/B axes, because the two big use cases differ:
//   source A/B, same binary  — `petal lint --fix`, hand refactors of .ptl
//   binary A/B, same sources — compiler / VM / optimizer changes
//
//   ./ts/bin/verify.ts --plan test/verify-plans/lint-fix.json --before ab3304a~1 --after .
//   ./ts/bin/verify.ts --plan test/verify-plans/compiler.json \
//        --before-bin old/petal --after-bin rust/target/debug/petal
//
// A plan is an ordered list of checks, cheapest first, that short-circuits per
// file (see docs/dev/refactor-verification.md §4-6). Each file gets one table
// row and a verdict; the process exits non-zero if any file is `changed` or
// `compile-error`.
//
// Flags:
//   --plan <plan.json>       required; a file, or a name under test/verify-plans/
//   --before <git-ref|dir>   source A/B: a git ref is materialized under --out
//   --after <dir>            source A/B; defaults to the working tree
//   --before-bin/--after-bin binary A/B: two `petal` binaries
//   --before-ui-bin/--after-ui-bin   ditto for petal-ui-run (default: the built one)
//   --out <dir>              artifacts dir (default .temp/verify-runs/<plan>-<ISO>)
//   --only <glob>            restrict the corpus (matched against the relative path)
//   --jobs N                 parallelism (default cpus-1)
//   --frames N               override the plan's frame count
//   --update-golden          rewrite test/ui-golden/index.json from the after side
//   --quiet                  only the summary
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
    existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync,
} from 'node:fs';
import { cpus, homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

// ── Types ────────────────────────────────────────────────────────────────

type Kind = 'console' | 'ui' | 'module' | 'unsupported';
type Verdict =
    | 'identical-ir' | 'identical-trace' | 'nondeterministic'
    | 'changed' | 'compile-error' | 'unsupported' | 'module';

interface PlanStep {
    check: 'compiles' | 'ir-equal' | 'control-run' | 'run-diff' | 'golden';
    /** Short-circuit the pipeline for this file when the step passes / fails. */
    stop_on?: 'pass' | 'fail';
    seeds?: number[];
    frames?: number;
    /** "checked-in" (every scenarios/*.json beside the app) or "monkey:K" (seeds 1..K). */
    scenarios?: string[];
    seed?: number;
    scenario?: string;
}

interface Plan {
    name: string;
    /** Which A/B axis the plan is written for; the CLI flags must match. */
    mode: 'source' | 'binary';
    corpus: string[];
    size?: string;
    steps: PlanStep[];
}

/** One side of the comparison: where its sources live and which binaries drive them. */
interface Side {
    label: 'before' | 'after';
    root: string;
    petal: string;
    uiRun: string;
}

interface Target {
    /** Display path — relative to the repo root, or absolute for an external root. */
    rel: string;
    before: string;
    after: string;
    /** Directory of the *after* copy; where scenarios/ and layout.ptl are looked up. */
    dir: string;
}

interface Outcome {
    rel: string;
    kind: Kind;
    verdict: Verdict;
    steps: string[];
    detail: string;
    bundle?: string;
}

interface Run { code: number; stdout: string; stderr: string }

/** A concrete driver invocation: which binary and its argv. */
interface Cmd { bin: string; args: string[] }

/** Result of the compiles step: `both` = failed identically on both sides. */
interface CompileCheck { ok: boolean; detail: string; both: boolean; warnNote: string }

interface IrEqualCheck { state: 'pass' | 'fail' | 'skip'; detail: string }

// ── Small helpers ────────────────────────────────────────────────────────

function fail(msg: string): never {
    console.error(`verify: ${msg}`);
    process.exit(2);
}

function expandTilde(p: string): string {
    return p === '~' || p.startsWith('~/') ? join(homedir(), p.slice(1)) : p;
}

function exec(cmd: string, args: string[], opts: { cwd?: string; env?: NodeJS.ProcessEnv } = {}): Promise<Run> {
    return new Promise(res => {
        const child = spawn(cmd, args, {
            cwd: opts.cwd, env: opts.env ?? process.env, stdio: ['ignore', 'pipe', 'pipe'],
        });
        let stdout = '', stderr = '';
        child.stdout.on('data', d => { stdout += d; });
        child.stderr.on('data', d => { stderr += d; });
        child.on('error', e => res({ code: 127, stdout, stderr: String(e) }));
        child.on('close', code => res({ code: code ?? -1, stdout, stderr }));
    });
}

/**
 * Run a command whose stdout is a (potentially huge) trace, and return only its
 * sha256. UI traces reach tens of megabytes per app-run; hashing the stream
 * keeps a whole-corpus sweep in constant memory. When two hashes disagree the
 * caller re-runs the pair with `--out`, which is the only time the bytes are
 * kept.
 */
function execHash(cmd: string, args: string[]): Promise<{ code: number; hash: string; stderr: string }> {
    return new Promise(res => {
        const child = spawn(cmd, args, { stdio: ['ignore', 'pipe', 'pipe'] });
        const h = createHash('sha256');
        let stderr = '';
        child.stdout.on('data', d => h.update(d));
        child.stderr.on('data', d => { stderr += d; });
        child.on('error', e => res({ code: 127, hash: '', stderr: String(e) }));
        child.on('close', code => res({ code: code ?? -1, hash: h.digest('hex'), stderr }));
    });
}

/** Glob → RegExp, with `**` crossing separators and `*`/`?` not. */
function globToRegExp(glob: string): RegExp {
    let re = '';
    for (let i = 0; i < glob.length; i++) {
        const c = glob[i];
        if (c === '*') {
            if (glob[i + 1] === '*') { re += '.*'; i++; if (glob[i + 1] === '/') i++; }
            else re += '[^/]*';
        } else if (c === '?') re += '[^/]';
        else re += c.replace(/[.+^${}()|[\]\\]/g, '\\$&');
    }
    return new RegExp(`^${re}$`);
}

function walkPtl(dir: string, out: string[]) {
    let entries;
    try { entries = readdirSync(dir, { withFileTypes: true }); } catch { return; }
    for (const e of entries.sort((a, b) => a.name.localeCompare(b.name))) {
        if (e.name.startsWith('.') || e.name === 'node_modules' || e.name === 'target') continue;
        const p = join(dir, e.name);
        if (e.isDirectory()) walkPtl(p, out);
        else if (e.name.endsWith('.ptl')) out.push(p);
    }
}

// ── Argument parsing ─────────────────────────────────────────────────────

interface Opts {
    plan: string;
    before?: string;
    after: string;
    beforeBin?: string;
    afterBin?: string;
    beforeUiBin?: string;
    afterUiBin?: string;
    out?: string;
    only?: string;
    jobs: number;
    frames?: number;
    updateGolden: boolean;
    quiet: boolean;
}

function parseArgs(argv: string[]): Opts {
    const o: Opts = {
        plan: '', after: repoRoot, jobs: Math.max(1, cpus().length - 1),
        updateGolden: false, quiet: false,
    };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        const next = () => {
            const v = argv[++i];
            if (v === undefined) fail(`${a} needs a value`);
            return v;
        };
        switch (a) {
            case '--plan': o.plan = next(); break;
            case '--before': o.before = next(); break;
            case '--after': o.after = resolve(expandTilde(next())); break;
            case '--before-bin': o.beforeBin = resolve(expandTilde(next())); break;
            case '--after-bin': o.afterBin = resolve(expandTilde(next())); break;
            case '--before-ui-bin': o.beforeUiBin = resolve(expandTilde(next())); break;
            case '--after-ui-bin': o.afterUiBin = resolve(expandTilde(next())); break;
            case '--out': o.out = resolve(expandTilde(next())); break;
            case '--only': o.only = next(); break;
            case '--jobs': o.jobs = Math.max(1, parseInt(next(), 10)); break;
            case '--frames': o.frames = parseInt(next(), 10); break;
            case '--update-golden': o.updateGolden = true; break;
            case '--quiet': o.quiet = true; break;
            case '-h': case '--help':
                console.log(readFileSync(fileURLToPath(import.meta.url), 'utf-8')
                    .split('\n').slice(1).filter(l => l.startsWith('//')).map(l => l.slice(3)).join('\n'));
                process.exit(0);
                break;
            default: fail(`unknown flag \`${a}\``);
        }
    }
    if (!o.plan) fail('--plan is required');
    return o;
}

// ── Plan + sides ─────────────────────────────────────────────────────────

function loadPlan(spec: string): Plan {
    const candidates = [
        resolve(expandTilde(spec)),
        join(repoRoot, 'test', 'verify-plans', spec),
        join(repoRoot, 'test', 'verify-plans', `${spec}.json`),
    ];
    const path = candidates.find(existsSync);
    if (!path) fail(`no plan at ${spec}`);
    const plan = JSON.parse(readFileSync(path, 'utf-8')) as Plan;
    if (!plan.steps?.length) fail(`plan ${path} has no steps`);
    plan.name ??= spec.replace(/\.json$/, '');
    return plan;
}

/**
 * Turn `--before` into a directory. A path that exists is used as is; anything
 * else must name a commit, which is materialized under the artifacts dir with
 * `git archive` (no worktree lock to clean up, and the checkout is read-only by
 * construction).
 */
function materialize(ref: string, outDir: string): string {
    const asDir = resolve(expandTilde(ref));
    if (existsSync(asDir) && statSync(asDir).isDirectory()) return asDir;
    const rev = spawnSync('git', ['rev-parse', '--verify', `${ref}^{commit}`],
        { cwd: repoRoot, encoding: 'utf-8' });
    if (rev.status !== 0) fail(`--before \`${ref}\` is neither a directory nor a git ref`);
    const sha = rev.stdout.trim();
    const dest = join(outDir, `src-before-${sha.slice(0, 10)}`);
    if (existsSync(dest)) return dest;
    mkdirSync(dest, { recursive: true });
    const ar = spawnSync('sh', ['-c',
        `git -C ${JSON.stringify(repoRoot)} archive ${sha} | tar -x -C ${JSON.stringify(dest)}`],
        { encoding: 'utf-8' });
    if (ar.status !== 0) fail(`git archive ${ref} failed: ${ar.stderr}`);
    return dest;
}

// ── Corpus ───────────────────────────────────────────────────────────────

/**
 * Expand the plan's corpus entries against the *after* tree. An entry is a
 * directory, a file, or a glob; `~` expands. A root that does not exist here
 * (an external project this machine does not have) is a note, not an error —
 * the plan is shared, the checkouts are not.
 */
function collectCorpus(plan: Plan, after: string, notes: string[]): string[] {
    const files: string[] = [];
    for (const raw of plan.corpus) {
        const entry = expandTilde(raw);
        const abs = isAbsolute(entry) ? entry : join(after, entry);
        if (/[*?]/.test(abs)) {
            const idx = abs.split(sep).findIndex(s => /[*?]/.test(s));
            const base = abs.split(sep).slice(0, idx).join(sep) || sep;
            if (!existsSync(base)) { notes.push(`corpus root missing, skipped: ${raw}`); continue; }
            const all: string[] = [];
            walkPtl(base, all);
            const re = globToRegExp(abs);
            files.push(...all.filter(f => re.test(f)));
            continue;
        }
        if (!existsSync(abs)) { notes.push(`corpus root missing, skipped: ${raw}`); continue; }
        if (statSync(abs).isDirectory()) walkPtl(abs, files);
        else files.push(abs);
    }
    return [...new Set(files)].sort();
}

/**
 * Pair each corpus file with its counterpart on the before side. Files inside
 * the after tree map by relative path; an *external* root (`~/worlds-fair/...`)
 * is outside both trees, so both sides read the same absolute path — which is
 * correct for binary A/B and a no-op for source A/B.
 */
function pairSides(files: string[], before: Side, after: Side, notes: string[]): Target[] {
    const targets: Target[] = [];
    for (const f of files) {
        const inTree = f.startsWith(after.root + sep);
        const rel = inTree ? relative(after.root, f) : f;
        const beforePath = inTree ? join(before.root, rel) : f;
        if (!existsSync(beforePath)) {
            notes.push(`only on the after side, skipped: ${rel}`);
            continue;
        }
        targets.push({ rel, before: beforePath, after: f, dir: dirname(f) });
    }
    return targets;
}

// ── Classification (§4) ──────────────────────────────────────────────────

/**
 * Natives only a petal-ui host registers (petal-ui/src/{input,draw}.rs) plus the
 * distinctive helpers in petal-ui/prelude/ui.ptl. `time` and `clear` are left
 * out deliberately: they collide with names a console script legitimately uses,
 * and a false `ui` is worse than a false `console` (the console driver reports
 * the missing native by name, so the probe below recovers).
 */
const UI_NATIVES = [
    'mouse_x', 'mouse_y', 'mouse_dx', 'mouse_dy', 'mouse_down', 'mouse_pressed',
    'mouse_released', 'scroll_x', 'scroll_y', 'key_down', 'key_pressed', 'key_released',
    'mod_shift', 'mod_ctrl', 'mod_alt', 'mod_cmd', 'drag_active', 'drag_start_x',
    'drag_start_y', 'click_count', 'text_input', 'grab_mouse', 'release_mouse',
    'dt', 'frame_count', 'screen_width', 'screen_height', 'ui_version',
    'draw_image', 'draw_rect', 'draw_rect_rounded', 'draw_rect_outline', 'draw_line',
    'draw_polyline', 'draw_circle', 'draw_circle_outline', 'draw_ellipse',
    'draw_ellipse_outline', 'fill_arc', 'fill_triangle', 'fill_poly', 'fill_polygon',
    'fill_fan', 'draw_text', 'clip', 'clip_none', 'text_width', 'create_canvas',
    'draw_to', 'draw_to_screen', 'draw_canvas',
    'hovered', 'clicked', 'ui_theme', 'draw_text_center', 'draw_text_right',
];
const UI_RE = new RegExp(`\\b(${UI_NATIVES.join('|')})\\s*\\(`);

/** `import foo`, `import foo as f`, `import foo: a, b` — the imported module name. */
const IMPORT_RE = /^\s*import\s+([A-Za-z_][A-Za-z0-9_./]*)/gm;

/** Read a file (memoized — classification reads each corpus file twice). */
const readCache = new Map<string, string>();
function read(path: string): string {
    const hit = readCache.get(path);
    if (hit !== undefined) return hit;
    let text = '';
    try { text = readFileSync(path, 'utf-8'); } catch { /* missing reads as empty */ }
    readCache.set(path, text);
    return text;
}

/**
 * A file imported by another corpus file is a module: it has no entry point of
 * its own, and its behavior is covered by whoever imports it. Imports resolve
 * against the importer's own directory (`Headless::from_file`'s rule).
 */
function moduleSet(targets: Target[]): Set<string> {
    const known = new Set(targets.map(t => t.after));
    const mods = new Set<string>();
    for (const t of targets) {
        const text = read(t.after);
        for (const m of text.matchAll(IMPORT_RE)) {
            const cand = resolve(t.dir, `${m[1]}.ptl`);
            if (known.has(cand) && cand !== t.after) mods.add(cand);
        }
    }
    return mods;
}

function staticKind(t: Target, mods: Set<string>): Kind {
    if (mods.has(t.after)) return 'module';
    const text = read(t.after);
    if (UI_RE.test(text)) return 'ui';
    // A panel app is driven by the layout beside it, even if the entry file
    // itself only delegates.
    const layout = join(t.dir, 'layout.ptl');
    if (t.after !== layout && existsSync(layout)) return 'ui';
    return 'console';
}

const UNKNOWN_BUILTIN_RE = /Unknown builtin: ([A-Za-z_][A-Za-z0-9_]*)/;

// ── Drivers ──────────────────────────────────────────────────────────────

interface ScenarioSpec {
    /** `null` for console; a monkey spec or a checked-in file path for ui. */
    id: string;
    args: string[];
    /** What the bundle records: enough to regenerate the exact input. */
    describe: unknown;
}

function scenarioSpecs(t: Target, kind: Kind, step: PlanStep, plan: Plan, frames: number): ScenarioSpec[] {
    if (kind !== 'ui') return [{ id: 'run', args: [], describe: null }];
    const out: ScenarioSpec[] = [];
    const size = plan.size ?? '800x600';
    for (const s of step.scenarios ?? ['monkey:1']) {
        if (s === 'checked-in') {
            const dir = join(t.dir, 'scenarios');
            if (!existsSync(dir)) continue;
            for (const f of readdirSync(dir).filter(f => f.endsWith('.json')).sort()) {
                out.push({
                    id: f.replace(/\.json$/, ''),
                    args: ['--scenario', join(dir, f)],
                    describe: { kind: 'checked-in', path: relative(repoRoot, join(dir, f)) },
                });
            }
        } else if (s.startsWith('monkey:')) {
            const k = parseInt(s.slice(7), 10) || 1;
            for (let i = 1; i <= k; i++) {
                out.push({
                    id: `monkey${i}`,
                    args: ['--scenario', `monkey:${i}`],
                    describe: { kind: 'monkey', monkeySeed: i, frames, size },
                });
            }
        }
    }
    // A UI app with no scenario at all still gets driven — idle frames.
    if (out.length === 0) out.push({ id: 'idle', args: [], describe: { kind: 'idle', frames, size } });
    return out;
}

function driverArgs(kind: Kind, side: Side, path: string, seed: number, sc: ScenarioSpec,
                    frames: number, size: string): Cmd {
    if (kind === 'ui') {
        return {
            bin: side.uiRun,
            args: [path, '--seed', String(seed), '--frames', String(frames),
                   '--size', size, '--error-format', 'bare', ...sc.args],
        };
    }
    return {
        bin: side.petal,
        args: ['run', '--seed', String(seed), '--error-format', 'bare', path],
    };
}

// ── Divergence reporting ─────────────────────────────────────────────────

/** The first JSON field path where two objects disagree, for a UI frame record. */
function firstFieldDiff(a: unknown, b: unknown, path = ''): string | null {
    if (JSON.stringify(a) === JSON.stringify(b)) return null;
    const isObj = (v: unknown) => v !== null && typeof v === 'object';
    if (isObj(a) && isObj(b) && Array.isArray(a) === Array.isArray(b)) {
        const keys = Array.isArray(a)
            ? [...Array(Math.max((a as unknown[]).length, (b as unknown[]).length)).keys()].map(String)
            : [...new Set([...Object.keys(a as object), ...Object.keys(b as object)])];
        for (const k of keys) {
            const sub = firstFieldDiff((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k],
                path ? `${path}.${k}` : k);
            if (sub) return sub;
        }
    }
    return `${path || '<root>'}\n      before: ${JSON.stringify(a) ?? '<missing>'}` +
           `\n      after:  ${JSON.stringify(b) ?? '<missing>'}`;
}

function describeDiff(kind: Kind, aText: string, bText: string): string {
    const a = aText.split('\n'), b = bText.split('\n');
    const n = Math.max(a.length, b.length);
    for (let i = 0; i < n; i++) {
        if (a[i] === b[i]) continue;
        const where = kind === 'ui' ? `frame ${i}` : `line ${i + 1}`;
        if (kind === 'ui') {
            try {
                const field = firstFieldDiff(JSON.parse(a[i] ?? 'null'), JSON.parse(b[i] ?? 'null'));
                if (field) return `${where}, field ${field}`;
            } catch { /* not JSON — fall through to the line diff */ }
        }
        return `${where}:\n      before: ${(a[i] ?? '<missing>').slice(0, 200)}` +
               `\n      after:  ${(b[i] ?? '<missing>').slice(0, 200)}`;
    }
    return 'outputs differ in length only';
}

// ── The per-file pipeline ────────────────────────────────────────────────

interface Ctx {
    plan: Plan;
    opts: Opts;
    before: Side;
    after: Side;
    outDir: string;
    irEqualAvailable: boolean;
    golden: Record<string, string>;
    goldenDirty: boolean;
}

function bundleDir(ctx: Ctx, t: Target): string {
    const slug = t.rel.replace(/^[/~]+/, '').replace(/[/\\]/g, '__');
    const dir = join(ctx.outDir, slug);
    mkdirSync(dir, { recursive: true });
    return dir;
}

function shellQuote(s: string): string {
    return /^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`;
}

function writeRepro(dir: string, kind: Kind, ctx: Ctx, t: Target, seed: number,
                    sc: ScenarioSpec, frames: number, size: string, note: string) {
    const b = driverArgs(kind, ctx.before, t.before, seed, sc, frames, size);
    const a = driverArgs(kind, ctx.after, t.after, seed, sc, frames, size);
    const line = (r: Cmd, out: string) =>
        [r.bin, ...r.args].map(shellQuote).join(' ') +
        (kind === 'ui' ? ` --out "$DIR/${out}"` : ` > "$DIR/${out}" 2>&1`);
    writeFileSync(join(dir, 'repro.sh'), [
        '#!/bin/sh',
        '# Reproduce this diff with no other context. Regenerated by ts/bin/verify.ts.',
        `# ${note}`,
        'set -u',
        'DIR="$(cd "$(dirname "$0")" && pwd)"',
        '',
        line(b, 'before.repro'),
        line(a, 'after.repro'),
        '',
        'diff "$DIR/before.repro" "$DIR/after.repro" && echo "identical"',
        '',
    ].join('\n'), { mode: 0o755 });
}

/**
 * Does the file compile the same way on both sides?
 *
 * Errors and the exit code are the verdict; *warnings* are only a note. Under
 * `--error-format bare` a warning renders as its message alone, but a few
 * messages quote a line number inside the prose ("written on line 775"), which
 * a pure re-indent shifts. Failing the file for that would be a false alarm, so
 * a warnings-only difference is reported and the pipeline continues — the run
 * itself is the real evidence.
 */
async function checkCompiles(ctx: Ctx, t: Target): Promise<CompileCheck> {
    const one = (s: Side, p: string) => exec(s.petal, ['check', '--error-format', 'bare', p]);
    const [b, a] = await Promise.all([one(ctx.before, t.before), one(ctx.after, t.after)]);
    const split = (r: Run) => {
        const lines = `${r.stdout}${r.stderr}`.split('\n');
        return {
            warnings: lines.filter(l => l.startsWith('warning:')).join('\n'),
            errors: lines.filter(l => !l.startsWith('warning:')).join('\n'),
        };
    };
    const [bs, as] = [split(b), split(a)];
    if (b.code !== a.code || bs.errors !== as.errors) {
        return {
            ok: false, both: false, warnNote: '',
            detail: `compile outcome differs\n      before(${b.code}): ${bs.errors.trim().split('\n')[0]}` +
                    `\n      after(${a.code}):  ${as.errors.trim().split('\n')[0]}`,
        };
    }
    if (b.code !== 0) {
        return { ok: false, both: true, warnNote: '',
                 detail: `does not compile on either side: ${bs.errors.trim().split('\n')[0]}` };
    }
    const warnNote = bs.warnings === as.warnings ? ''
        : `compiler warnings differ (positions quoted in the message text; not a behavior change)`;
    return { ok: true, both: false, detail: '', warnNote };
}

async function checkIrEqual(ctx: Ctx, t: Target): Promise<IrEqualCheck> {
    if (!ctx.irEqualAvailable) return { state: 'skip', detail: 'ir-equal unavailable' };
    const r = await exec(ctx.after.petal, ['ir-equal', t.before, t.after]);
    if (r.code === 0) return { state: 'pass', detail: '' };
    return { state: 'fail', detail: (r.stdout + r.stderr).trim().split('\n')[0] };
}

async function traceHash(kind: Kind, side: Side, path: string, seed: number, sc: ScenarioSpec,
                         frames: number, size: string) {
    const { bin, args } = driverArgs(kind, side, path, seed, sc, frames, size);
    const r = await execHash(bin, args);
    // A console run's stderr is part of its observable output; a UI run's is not
    // (errors land in the trace's `error` field), so only stdout is hashed there.
    if (kind === 'ui') return { hash: r.hash, code: r.code, stderr: r.stderr };
    return {
        hash: createHash('sha256').update(`${r.hash}\n${r.code}\n${r.stderr}`).digest('hex'),
        code: r.code, stderr: r.stderr,
    };
}

/** Re-run a pair with output kept, for the divergence report and the bundle. */
async function traceToFile(kind: Kind, side: Side, path: string, seed: number, sc: ScenarioSpec,
                           frames: number, size: string, dest: string): Promise<string> {
    const { bin, args } = driverArgs(kind, side, path, seed, sc, frames, size);
    if (kind === 'ui') {
        await exec(bin, [...args, '--out', dest]);
    } else {
        const r = await exec(bin, args);
        writeFileSync(dest, `${r.stdout}${r.stderr}`);
    }
    return readFileSync(dest, 'utf-8');
}

async function runFile(ctx: Ctx, t: Target, mods: Set<string>): Promise<Outcome> {
    const steps: string[] = [];
    const kind = staticKind(t, mods);
    const size = ctx.plan.size ?? '800x600';

    if (kind === 'module') {
        return { rel: t.rel, kind, verdict: 'module', steps, detail: 'imported by another entry' };
    }

    // Probe: a file that calls a native no driver registers is `unsupported`,
    // discovered by running it rather than by a hard-coded list.
    const probe = kind === 'ui'
        ? await exec(ctx.after.uiRun, [t.after, '--frames', '1', '--seed', '1',
                                       '--error-format', 'bare', '--out', '/dev/null'])
        : await exec(ctx.after.petal, ['run', '--seed', '1', '--error-format', 'bare', t.after]);
    const missing = UNKNOWN_BUILTIN_RE.exec(probe.stdout + probe.stderr);
    if (missing) {
        return { rel: t.rel, kind: 'unsupported', verdict: 'unsupported', steps,
                 detail: `no driver provides \`${missing[1]}\`` };
    }

    let verdict: Verdict = 'identical-trace';
    let detail = '';

    for (const step of ctx.plan.steps) {
        if (step.check === 'compiles') {
            steps.push('compiles');
            const r = await checkCompiles(ctx, t);
            if (!r.ok && r.both) {
                return { rel: t.rel, kind: 'unsupported', verdict: 'unsupported', steps, detail: r.detail };
            }
            if (!r.ok) {
                const dir = bundleDir(ctx, t);
                writeFileSync(join(dir, 'detail.txt'), r.detail);
                return { rel: t.rel, kind, verdict: 'compile-error', steps, detail: r.detail, bundle: dir };
            }
            if (r.warnNote) {
                steps.push('compiles(warnings differ)');
                detail = r.warnNote;
            }
        } else if (step.check === 'ir-equal') {
            if (ctx.plan.mode !== 'source') { steps.push('ir-equal(n/a)'); continue; }
            const r = await checkIrEqual(ctx, t);
            steps.push(r.state === 'skip' ? 'ir-equal(skip)' : `ir-equal(${r.state})`);
            if (r.state === 'pass' && step.stop_on === 'pass') {
                return { rel: t.rel, kind, verdict: 'identical-ir', steps, detail: '' };
            }
        } else if (step.check === 'control-run') {
            steps.push('control-run');
            const frames = ctx.opts.frames ?? step.frames ?? 60;
            const seed = step.seeds?.[0] ?? 1;
            const sc = scenarioSpecs(t, kind, step, ctx.plan, frames)[0];
            // Both runs are deliberately the *before* side: this step measures
            // the app's own determinism, not the refactor.
            const [x, y] = await Promise.all([
                traceHash(kind, ctx.before, t.before, seed, sc, frames, size),
                traceHash(kind, ctx.before, t.before, seed, sc, frames, size),
            ]);
            if (x.hash !== y.hash) {
                const dir = bundleDir(ctx, t);
                const a = await traceToFile(kind, ctx.before, t.before, seed, sc, frames, size, join(dir, 'before.a'));
                const b = await traceToFile(kind, ctx.before, t.before, seed, sc, frames, size, join(dir, 'before.b'));
                const d = describeDiff(kind, a, b);
                writeFileSync(join(dir, 'seed'), String(seed));
                writeFileSync(join(dir, 'scenario.json'), JSON.stringify(sc.describe, null, 2));
                writeRepro(dir, kind, ctx, t, seed, sc, frames, size,
                    'the BEFORE side alone differs run-to-run; run it twice and compare');
                return { rel: t.rel, kind, verdict: 'nondeterministic', steps,
                         detail: `before-vs-before differs at ${d}`, bundle: dir };
            }
            if (step.stop_on === 'fail') continue;
        } else if (step.check === 'run-diff') {
            steps.push('run-diff');
            const frames = ctx.opts.frames ?? step.frames ?? 60;
            const specs = scenarioSpecs(t, kind, step, ctx.plan, frames);
            for (const seed of step.seeds ?? [1]) {
                for (const sc of specs) {
                    const [b, a] = await Promise.all([
                        traceHash(kind, ctx.before, t.before, seed, sc, frames, size),
                        traceHash(kind, ctx.after, t.after, seed, sc, frames, size),
                    ]);
                    if (b.hash === a.hash) {
                        // Identical *and* both failed on a native nobody provides:
                        // the file is unsupported, not verified.
                        const m = UNKNOWN_BUILTIN_RE.exec(a.stderr);
                        if (m) {
                            return { rel: t.rel, kind: 'unsupported', verdict: 'unsupported', steps,
                                     detail: `no driver provides \`${m[1]}\`` };
                        }
                        continue;
                    }
                    const dir = bundleDir(ctx, t);
                    const bt = await traceToFile(kind, ctx.before, t.before, seed, sc, frames, size,
                        join(dir, kind === 'ui' ? 'before.jsonl' : 'before.out'));
                    const at = await traceToFile(kind, ctx.after, t.after, seed, sc, frames, size,
                        join(dir, kind === 'ui' ? 'after.jsonl' : 'after.out'));
                    writeFileSync(join(dir, 'seed'), String(seed));
                    writeFileSync(join(dir, 'scenario.json'), JSON.stringify(sc.describe, null, 2));
                    writeRepro(dir, kind, ctx, t, seed, sc, frames, size,
                        `seed ${seed}, scenario ${sc.id}`);
                    return {
                        rel: t.rel, kind, verdict: 'changed', steps, bundle: dir,
                        detail: `seed ${seed} scenario ${sc.id}: ${describeDiff(kind, bt, at)}`,
                    };
                }
            }
        } else if (step.check === 'golden') {
            if (kind !== 'ui') continue;
            steps.push('golden');
            const frames = step.frames ?? 60;
            const seed = step.seed ?? 1;
            const scStr = step.scenario ?? 'monkey:1';
            const sc: ScenarioSpec = {
                id: scStr.replace(/[:/\\]/g, '-'),
                args: ['--scenario', scStr],
                // Record what was actually driven — a checked-in scenario file
                // must not be described as monkey seed 1.
                describe: scStr.startsWith('monkey:')
                    ? { kind: 'monkey', monkeySeed: parseInt(scStr.slice(7), 10) || 1, frames, size }
                    : { kind: 'checked-in', path: scStr },
            };
            const key = `${t.rel}/${sc.id}-s${seed}`;
            const got = await traceHash(kind, ctx.after, t.after, seed, sc, frames, size);
            if (ctx.opts.updateGolden) {
                ctx.golden[key] = got.hash;
                ctx.goldenDirty = true;
            } else if (ctx.golden[key] && ctx.golden[key] !== got.hash) {
                verdict = 'changed';
                detail = `golden mismatch for ${key} (rerun with --update-golden to re-baseline)`;
            }
        }
    }
    return { rel: t.rel, kind, verdict, steps, detail };
}

// ── Main ─────────────────────────────────────────────────────────────────

const VERDICT_ORDER: Verdict[] = ['identical-ir', 'identical-trace', 'module', 'unsupported',
                                  'nondeterministic', 'changed', 'compile-error'];

async function main() {
    const opts = parseArgs(process.argv.slice(2));
    const plan = loadPlan(opts.plan);

    const stamp = new Date().toISOString().replace(/[:.]/g, '-');
    const outDir = opts.out ?? join(repoRoot, '.temp', 'verify-runs', `${plan.name}-${stamp}`);
    mkdirSync(outDir, { recursive: true });

    const defaultPetal = join(repoRoot, 'rust', 'target', 'debug', 'petal');
    const defaultUi = join(repoRoot, 'petal-ui', 'target', 'debug', 'petal-ui-run');

    let before: Side, after: Side;
    if (opts.beforeBin || opts.afterBin) {
        if (plan.mode === 'source') fail(`plan "${plan.name}" is a source A/B plan; use --before/--after`);
        before = { label: 'before', root: opts.after, petal: opts.beforeBin ?? defaultPetal,
                   uiRun: opts.beforeUiBin ?? defaultUi };
        after = { label: 'after', root: opts.after, petal: opts.afterBin ?? defaultPetal,
                  uiRun: opts.afterUiBin ?? defaultUi };
    } else {
        if (!opts.before) fail('source A/B needs --before <git-ref|dir> (or use --before-bin/--after-bin)');
        if (plan.mode === 'binary') fail(`plan "${plan.name}" is a binary A/B plan; use --before-bin/--after-bin`);
        const beforeRoot = materialize(opts.before, outDir);
        before = { label: 'before', root: beforeRoot, petal: opts.beforeBin ?? defaultPetal,
                   uiRun: opts.beforeUiBin ?? defaultUi };
        after = { label: 'after', root: opts.after, petal: opts.afterBin ?? defaultPetal,
                  uiRun: opts.afterUiBin ?? defaultUi };
    }
    for (const s of [before, after]) {
        if (!existsSync(s.petal)) fail(`no petal binary at ${s.petal} (cd rust && cargo build)`);
    }

    const notes: string[] = [];
    let files = collectCorpus(plan, after.root, notes);
    if (opts.only) {
        // A pattern with no wildcards is a plain substring — the common case is
        // `--only snake`, not a fully-spelled glob.
        const only = opts.only;
        const match = /[*?]/.test(only)
            ? (p: string) => globToRegExp(only).test(p)
            : (p: string) => p.includes(only);
        files = files.filter(f => match(relative(after.root, f)) || match(f));
    }
    const targets = pairSides(files, before, after, notes);
    const mods = moduleSet(targets);

    // `ir-equal` is landing separately; probe rather than assume. An unknown
    // subcommand falls through to "run the first file", which would silently
    // read as a pass, so the help text is the only honest signal.
    const help = spawnSync(after.petal, ['--help'], { encoding: 'utf-8' });
    const irEqualAvailable = /\bir-equal\b/.test(`${help.stdout}${help.stderr}`);
    if (!irEqualAvailable) notes.push('petal ir-equal is unavailable; that step is skipped');

    const goldenPath = join(repoRoot, 'test', 'ui-golden', 'index.json');
    const golden: Record<string, string> = existsSync(goldenPath)
        ? JSON.parse(readFileSync(goldenPath, 'utf-8')).traces ?? {} : {};

    const ctx: Ctx = {
        plan, opts, before, after, outDir, irEqualAvailable, golden, goldenDirty: false,
    };

    writeFileSync(join(outDir, 'plan.json'), JSON.stringify({
        plan, resolved: {
            mode: plan.mode, before: { root: before.root, petal: before.petal, uiRun: before.uiRun },
            after: { root: after.root, petal: after.petal, uiRun: after.uiRun },
            files: targets.length, jobs: opts.jobs, only: opts.only ?? null, notes,
        },
    }, null, 2));

    if (!opts.quiet) {
        console.log(`plan ${plan.name} (${plan.mode} A/B), ${targets.length} files, jobs ${opts.jobs}`);
        console.log(`  before: ${before.root}${plan.mode === 'binary' ? ` [${before.petal}]` : ''}`);
        console.log(`  after:  ${after.root}${plan.mode === 'binary' ? ` [${after.petal}]` : ''}`);
        console.log(`  out:    ${outDir}`);
        for (const n of notes) console.log(`  note: ${n}`);
        console.log('');
    }

    const results: Outcome[] = [];
    let next = 0;
    const worker = async () => {
        for (;;) {
            const i = next++;
            if (i >= targets.length) return;
            const r = await runFile(ctx, targets[i], mods);
            results.push(r);
            if (!opts.quiet) {
                console.log(`${r.verdict.padEnd(17)} ${r.kind.padEnd(12)} ${r.rel}`);
                if (r.detail) console.log(`    ${r.detail.replace(/\n/g, '\n    ')}`);
            }
        }
    };
    await Promise.all(Array.from({ length: Math.min(opts.jobs, targets.length || 1) }, worker));

    if (ctx.goldenDirty) {
        mkdirSync(dirname(goldenPath), { recursive: true });
        writeFileSync(goldenPath, `${JSON.stringify({
            note: 'sha256 of each UI app trace; see docs/dev/refactor-verification.md §5',
            config: { frames: 60, scenario: 'monkey:1', seed: 1, size: plan.size ?? '800x600' },
            traces: Object.fromEntries(Object.entries(golden).sort()),
        }, null, 2)}\n`);
        console.log(`\nwrote ${relative(repoRoot, goldenPath)} (${Object.keys(golden).length} traces)`);
    }

    const counts = new Map<Verdict, number>();
    for (const r of results) counts.set(r.verdict, (counts.get(r.verdict) ?? 0) + 1);
    console.log('\nsummary:');
    for (const v of VERDICT_ORDER) if (counts.get(v)) console.log(`  ${v.padEnd(17)} ${counts.get(v)}`);
    const bad = (counts.get('changed') ?? 0) + (counts.get('compile-error') ?? 0);
    console.log(`  ${'total'.padEnd(17)} ${results.length}`);
    console.log(`artifacts: ${outDir}`);
    writeFileSync(join(outDir, 'results.json'),
        `${JSON.stringify(results.sort((a, b) => a.rel.localeCompare(b.rel)), null, 2)}\n`);
    process.exit(bad > 0 ? 1 : 0);
}

main().catch(e => {
    console.error(e);
    process.exit(2);
});
