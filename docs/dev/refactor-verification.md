# Refactor verification

Status: **P1 and P2 built** (2026-08). The determinism knobs, the headless UI
driver, and the verifier all exist:
[`petal run --seed`/`--error-format bare`](testing.md#deterministic-runs),
[`petal-ui-run`](headless-ui-run.md), and
[`ts/bin/verify.ts`](../../ts/bin/verify.ts) with plans in
`test/verify-plans/`. See the Phasing section at the bottom for what is done
and what is left. Sections 1-6 below describe the design as built; section 7
and P3-P4 are still proposals.

## The problem

We keep doing large mechanical changes and wanting proof they were
behavior-preserving:

- `petal lint --fix` over every `.ptl` in the repo (70 files, ~5k lines churned)
- compiler / VM changes (optimizer passes, the bytecode migration)
- prelude and builtin refactors that touch every UI app

Today the proof is uneven:

| Corpus | What we have | Gap |
|---|---|---|
| `examples/console/*.ptl` | `ts/bin/test-examples.ts`: opts vs `--no-opt` differential + frozen golden | Fine for console; only ~28 files |
| `test/<case>/main.ptl` | `cargo test` script cases with `expects` | Same |
| UI apps (`examples/games`, `productivity`, `dashboards`, `custom-integrations`) | `petal-ui::harness::Headless` exists but no CLI driver; `bench_panel` is the only runner | `petal run` dies at `screen_width()`; nothing exercises them end to end |
| Garden panels (`layout.ptl` + `app.ptl`) | garden `--headless --debug-port` with `/frame`, `/mouse`, `/key`, `/panel/reset` | Nothing scripts them |
| fantasy-nes carts | `tests/carts.rs`: 120 frames in `--screenshot` mode, "ran clean + not blank" | Smoke only, no before/after diff |

The last session's ad-hoc harness found the two things that make naive
before/after diffing useless for UI apps, and they are the core of this
proposal:

1. **`random()` is seeded from the wall clock** (`builtins::initial_seed()`
   → `SystemTime::now()`), so *old-vs-old* already differs. 18 of 20 apps use
   it.
2. **Error messages carry column + echoed source line**, which any
   re-indenting refactor shifts.

Everything below is "make the harness permanent, make the runs replayable,
and make the comparison meaningful."

## Design

### 1. Deterministic runs (prerequisite)

Every source of nondeterminism gets a knob, and the verifier always sets it.

| Source | Where it lives | Proposed control |
|---|---|---|
| `random` / `random_int` / `choose` | `ExecutionContext::rng_state`, seeded by `initial_seed()` | `PETAL_SEED=<u64>` env var and `petal run --seed N`; `Env::set_seed(u64)` so embedders (Headless, garden) set it before the first frame. Forks already copy `rng_state`, so speculative execution stays consistent. |
| Perlin `noise_seed` | `ExecutionContext::noise_seed` | Same seed unless the script calls `noise_seed()` itself — already deterministic |
| `time()` / `elapsed()` / `dt` | Bound per frame by the host (`input::bind_time`, `bind_frame_info`) | Headless already uses fixed `FRAME_DT` and an explicit `time`. Garden headless should expose the same fixed clock under a flag (`--fixed-dt`). |
| Input | Host | Scripted scenario (below) |
| `host_data(kind, arg)` | Thread-local provider | Fixture provider: a JSON file of `{kind, arg} → value` answers, `nil` for misses |
| Error position text | `petal run` error formatting | `--error-format bare` (message only, no `file:line:col`, no echoed source line) or a `--json` error object the diff tool normalizes. Prefer the flag: normalizers drift. |
| Handle ids in printed output | `handle(class:slot#serial)` | Deterministic given the same allocation sequence — fine for same-binary A/B; a compiler change that alters allocation order will show up here as a spurious diff, so `--json` output should mask `slot#serial` |
| Map iteration order | `IndexMap` (checked) | Already insertion-ordered; no work |

Rule of thumb: the CLI and the harness must be able to run the same program
twice and get byte-identical output. The verifier **checks this first**
(old-vs-old control run) and reports a file as *nondeterministic* rather than
*changed* if the control fails — the exact triage the previous session did by
hand.

Seed choice: default to a fixed constant (say `0x5EED`), and let a sweep run
`--seeds 1..N` for extra coverage. A failure report always prints the seed so
the one-liner to reproduce is complete.

### 2. A headless UI driver as a first-class CLI

Promote the throwaway harness into `petal-ui` proper:

```
petal-ui run <app.ptl> [--size WxH] [--frames N] [--seed N]
                       [--scenario s.json] [--host-data fixtures.json]
                       [--out trace.jsonl]
```

(Concretely: a `bin/` target in `petal-ui`, or a `ui-run` subcommand in
`petal` behind the `ui` feature — the crate split decides which.)

Per frame it writes one JSONL record:

```json
{"frame": 12, "commands": [ {"op":"rect","x":..}, ... ],
 "state": {"score": 3, "dir": "left"},
 "prints": ["..."], "result": null, "error": null}
```

`DrawCommand` already derives `Serialize`, `Headless::state()` already
returns JSON, `ExecutionContext::output` already collects prints. This is
mostly plumbing.

### 3. Scenario files

A scenario is a list of input events keyed by frame. Small enough to hand-write,
simple enough to generate:

```json
{ "size": [1280, 850], "frames": 120,
  "events": [
    {"at": 5,  "mouse_move": [640, 400]},
    {"at": 6,  "mouse_down": 0}, {"at": 7, "mouse_up": 0},
    {"at": 20, "key": "ArrowLeft"},
    {"at": 40, "text": "hello"}
  ] }
```

Two ways scenarios come into being:

- **Hand-written**, checked in next to the app (`examples/games/snake/scenarios/*.json`).
  These are the ones that reach deep state (start a game, lose a life, open a
  dialog).
- **Generated "monkey" scenarios** from a seed: random clicks inside the window,
  random keys from the canonical key list, random text. Deterministic by seed,
  so a failing monkey run is replayable by `(app, seed, scenario-seed)`. Good
  breadth for zero authoring cost; this is what the 40-frame harness was
  approximating.

A scenario that a *human* can also reproduce is worth more than one that only
the machine understands, so keep it declarative — no scripting language in the
scenario file.

### 4. Corpus classification

The verifier needs to know how to drive each file. Classify by evidence, not a
hand list:

| Kind | Signal | Driver |
|---|---|---|
| console | none of the below | `petal run` |
| ui | references a `petal-ui` native (`screen_width`, `draw_*`, `clicked`, …) or lives beside a `layout.ptl` | `petal-ui run` |
| garden | its `layout.ptl` uses garden-only natives, or the app calls `host_data` | garden `--headless` debug server |
| nes cart | under `carts/` | fantasy-nes `--screenshot` / frame trace |
| module | imported by something else, not an entry | skip as entry; covered via its importers |

Corpus roots: this repo (`examples/`, `test/`, `garden/` —
Garden lives in-tree now), plus the external projects in `CLAUDE.local.md`
(`~/.garden`, `~/worlds-fair/ui/ptl`). Config file lists roots; the tool globs.

### 5. Multi-step verification plan

A generic runner, `ts/bin/verify.ts`, executes a **plan** — an ordered list of
checks, cheapest first, that short-circuits per file:

```
verify --plan lint-fix.json --before <git-ref|dir> --after <dir>
verify --plan compiler.json --before-bin old/petal --after-bin new/petal
```

Two independent A/B axes, because the two big use cases differ:

- **source A/B, same binary** — `lint --fix`, hand refactors of `.ptl`
- **binary A/B, same sources** — compiler/VM/optimizer changes

A plan is a list of steps, each a named check with a `stop_on: pass|fail`:

```json
{ "name": "lint-fix",
  "corpus": ["examples", "test", "garden", "~/worlds-fair/ui/ptl"],
  "steps": [
    {"check": "compiles"},
    {"check": "ir-equal",      "stop_on": "pass"},
    {"check": "control-run",   "stop_on": "fail"},
    {"check": "run-diff",      "seeds": [1, 2, 3], "frames": 120,
                               "scenarios": ["checked-in", "monkey:4"]},
    {"check": "golden"} ] }
```

The checks:

1. **compiles** — `petal check` both sides. Catches the easy 80%.
2. **ir-equal** — `petal show-ir --json` both sides, strip file ids and spans,
   compare. For a formatting-only refactor this is *proof* with no execution
   and no determinism worries; a pass ends the pipeline for that file. For a
   semantic rewrite (`if`→`match`) it fails and we fall through. (A weaker
   variant, `bytecode-equal` post-optimization, might catch more semantic
   rewrites; worth an experiment.)

   **Call structure is part of the IR being compared** (since call-path keyed
   `state` landed, 2026-08-25). Each call term carries a `call_site` id and
   `ir_equiv` compares it, so *extract a helper*, *inline one*, *move a call
   into another function*, or *add an earlier call to the same callee* are all
   reported as differences. That is correct, not noise: a `state` slot is
   keyed by the call path that reaches it, so those rewrites genuinely change
   which state is shared with what. The consequence for this plan is that
   **`ir-equal` is not the check for a call-moving refactor** — it will
   correctly refuse to certify one. Fall through to `run-diff`, which compares
   observable behavior and is the only thing that can say a call move was
   safe. The reverse is unchanged: reformatting, re-indenting, renaming a
   non-callee local, and edits elsewhere in the file all still pass, because
   the id is derived from names and structure rather than positions. See
   [../CLI.md](../CLI.md) (`ir-equal`) for the exact sensitivity list.
3. **control-run** — run the *before* side twice with the same seed/scenario.
   If they differ, the file is reported as nondeterministic and skipped, with
   the first divergence shown so the missing knob can be added.
4. **run-diff** — run before and after under the chosen driver, with every
   determinism knob set, for each seed × scenario. Compare the JSONL trace
   frame by frame. First divergence is reported as `frame N, field, before,
   after` — the same triage shape `test-examples.ts` prints.
5. **golden** — optionally freeze the *after* traces as a new golden corpus
   (`test/ui-golden/<app>/<scenario>.jsonl`), the UI analogue of
   `test/example-golden/`. Regenerate deliberately.

   **"Deliberately" means naming the field that moved before running
   `--update-golden`,** because the hashes in `test/ui-golden/index.json` are
   opaque: a re-baseline that hides a behavior change looks exactly like one
   that does not. The worked example is `gallery.ptl`, `git_panel.ptl` and
   `main_menu.ptl`, re-baselined for `886b6d5`. That commit inserted a 147-line
   `layer` block into `petal-ui/prelude/ui.ptl` ahead of the widget
   definitions, which shifted the ordinal in the *display label* a host state
   dump puts on an unnamed callsite — `call_site_labels` in
   `env/state_json.rs` numbers `<expr>` occurrences program-wide in term order,
   so `ui::button#1/<expr>#63/_anim/ui::v` became
   `ui::button#1/<expr>#78/_anim/ui::v`. The argument that this was cosmetic
   has three parts, and a re-baseline should be able to make all three:

   - **Name the field.** Across all 180 frames of the three traces, `state` is
     the only field that differs — the `commands` arrays are identical, so
     nothing was drawn differently.
   - **Show nothing else moved.** Normalizing `<expr>#\d+` → `<expr>#N` makes
     the traces byte-equal: same slot set, same values, a pure key rename.
   - **Say why the field is not behavior.** `<expr>#N` is display-only.
     Nothing resolves a label back to a slot; the real key comes from
     `compiler/state_ids.rs`, derived from names and structure (a declaration
     id plus the call path), which is why the values never moved.

   Absent that argument, the diff is a `changed` verdict, not a re-baseline.

Output: one table row per file (`kind`, `steps run`, `verdict`), non-zero exit
if any `changed`. Verdicts: `identical-ir`, `identical-trace`,
`nondeterministic`, `changed`, `compile-error`, `driver-error`.

### 6. Replayability

Every run writes a bundle under a scratch/artifacts dir:

```
.temp/verify-runs/<timestamp>/<file>/
  plan.json              # the plan as resolved for this file
  scenario.json          # exact events, including generated monkey ones
  seed                   # PETAL_SEED used
  before.jsonl  after.jsonl
  repro.sh               # the two commands that reproduce the diff
```

`repro.sh` is the deliverable of a failure. It should work with no other
context, e.g.

```sh
PETAL_SEED=2 petal-ui run examples/games/snake/app.ptl \
  --scenario .temp/verify-runs/.../scenario.json --frames 120 --out /tmp/a.jsonl
```

### 7. Integration points

- `petal lint --fix --verify`: after rewriting, run `ir-equal` (and `run-diff`
  for semantic passes like `to_match`) on the file it just changed; refuse to
  write on failure. Makes the linter self-checking. The formatting pass never
  touches call structure, so `--verify` still proves it outright; the
  identity-cast pass *deletes call terms*, which renumbers later calls to the
  same callee in that function — one more reason it is in the
  expected-to-differ set that `--verify=ir` waves through and `--verify=strict`
  sends to a run-diff.
- `make test`: run the UI golden check (step 5) over the checked-in scenarios,
  so the apps get the same regression protection console examples have.
- CI: `verify --plan compiler.json` on PRs touching `rust/src/backend` or
  `rust/src/compiler`, before-bin = `main` build.

## Phasing

**P1 — determinism + driver — done**
- ✅ `PETAL_SEED` / `--seed` / `Env::set_seed`
- ✅ `--error-format bare`, on errors *and* type-checker warnings
- ✅ `petal-ui-run` with `--scenario`, `--frames`, `--seed`, `--host-data`,
  JSONL output; `Env::set_echo(false)` keeps prints out of the trace channel
- ⬜ Handle id masking in `--json` output — not needed yet; no corpus file
  prints a handle, so no run has produced a spurious `slot#serial` diff

**P2 — verifier — done**
- ✅ `ts/bin/verify.ts` with `compiles`, `control-run`, `run-diff`, `golden`
- ✅ Corpus classification by evidence (§4); console + ui drivers
- ✅ Artifact bundle + `repro.sh` (§6), under `.temp/verify-runs/` (gitignored)
- ✅ Monkey scenario generator (in `petal-ui/src/scenario.rs`)
- 🔶 `ir-equal` is wired into the plan but the `petal ir-equal` subcommand is
  landing separately; verify probes `petal --help` for it and reports the step
  as `ir-equal(skip)` when it is absent. Nothing else changes when it lands.

Two gaps the first real payload exposed, both known and neither blocking:

- A few type-checker warnings quote a line number *inside* the message prose
  (`` `scroll` is a `state` binding written on line 775``). `--error-format
  bare` cannot reach that, so a re-indent shifts it. The verifier treats a
  warnings-only difference as a printed note and keeps going, since warnings
  are diagnostics about the source's shape, not behavior. The real fix is for
  the diagnostic to carry the span rather than inline the number.
- `test/ui-golden/` stores **hashes, not traces**. The 15 example apps at 60
  frames total ~72 MB of JSONL — far past what belongs in git — so
  `test/ui-golden/index.json` holds one sha256 per app trace and a mismatch
  means "re-run it locally and diff", not "here is the stored diff".

**P3 — breadth**
- Garden driver via the debug server (`/frame`, `/mouse`, `/key`), `--fixed-dt`.
  Today every `layout.ptl` and garden window script lands as `unsupported`
  (`no driver provides \`panel\``), which is honest but is 20-odd files of the
  corpus going unverified.
- Hand-written scenarios beside the apps. The `"checked-in"` scenario source is
  implemented and finds nothing, because no `scenarios/*.json` exists yet;
  everything is monkey-driven breadth.
- A fantasy-NES / petal-fps driver, for the same reason as the garden one.

**P4 — self-checking tools**
- `lint --fix --verify`
- CI plan for compiler changes (`--plan compiler`, before-bin = `main` build)

## Open questions

- Should the seed knob also be a builtin (`random_seed(n)`, mirroring
  `noise_seed()`)? Useful for games that want reproducible levels; orthogonal
  to the verifier, which sets the seed from outside.
- `ir-equal` after `to_match`: is there a canonicalization (e.g. compare the
  *optimized bytecode*, or a CFG hash) that would make the `if`→`match` rewrite
  provably equal without execution? Worth an afternoon's experiment before
  relying on run-diff for it.
- How much of the Garden panel surface is driveable headlessly today
  (`/panel/reset` etc.) vs. needs new endpoints.
