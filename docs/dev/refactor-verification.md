# Refactor verification

How to prove that a large mechanical change was behavior-preserving: a
`petal lint --fix` sweep over every `.ptl` in the repo, a compiler/VM change,
or a prelude refactor that touches every UI app.

The pieces:

- **Deterministic runs**: [`petal run --seed` and `--error-format bare`](testing.md#deterministic-runs).
- **A headless UI driver**: [`petal-ui-run`](headless-ui-run.md).
- **The verifier**: [`ts/bin/verify.ts`](../../ts/bin/verify.ts), driven by
  plans in `test/verify-plans/`.
- **IR equality**: `petal ir-equal` ([CLI.md](../CLI.md#ir-equal--are-two-files-the-same-program)).

## The problem

Before this tooling, the proof was uneven. Console examples had
`ts/bin/test-examples.ts` (opts vs `--no-opt` differential plus a frozen
golden corpus), but nothing exercised the UI apps end to end: `petal run`
dies at `screen_width()`, and Garden panels could only be driven by hand.

Two things make naive before/after diffing useless for UI apps:

1. **`random()` is seeded from the wall clock**, so *old-vs-old* already
   differs. Most apps use it.
2. **Error messages carry column and echoed source line**, which any
   re-indenting refactor shifts.

Everything below makes the runs replayable and the comparison meaningful.

## 1. Deterministic runs

Every source of nondeterminism has a knob, and the verifier always sets it.

| Source | Control |
|---|---|
| `random` / `random_int` / `choose` | `PETAL_SEED=<u64>` or `petal run --seed N`; embedders call `Env::set_seed(n)` before the first frame. Forks copy the RNG state, so speculative execution stays consistent. |
| Perlin `noise_seed` | Deterministic already, unless the script calls `noise_seed()` itself. |
| `time()` / `elapsed()` / `dt` | Bound per frame by the host; `petal-ui-run` uses a fixed frame dt and an explicit clock. |
| Input | A scenario file (§3). |
| `host_data(kind, arg)` | A fixture file of `{kind, arg} → value` answers (`--host-data`), `nil` for misses. |
| Error position text | `--error-format bare`: message only, no position, no echoed source line. |
| Map iteration order | `IndexMap`, insertion-ordered; no work. |

The rule: the CLI and the harness must be able to run the same program twice
and get byte-identical output. The verifier checks this first (an old-vs-old
control run) and reports a file as *nondeterministic* rather than *changed*
if the control fails.

One known gap: a few type-checker warnings quote a line number inside the
message prose (`` `scroll` is a `state` binding written on line 775 ``), which
`--error-format bare` cannot reach. The verifier treats a warnings-only
difference as a printed note and keeps going, since warnings describe the
source's shape, not behavior. The real fix is for the diagnostic to carry the
span rather than inline the number.

## 2. The headless UI driver

`petal-ui-run` runs a `petal-ui` app for N frames with no window and writes
one JSONL record per frame: the draw commands, the state, the prints, and any
error. See [headless-ui-run.md](headless-ui-run.md) for the record format and
flags (`--scenario`, `--frames`, `--seed`, `--host-data`, `--out`).

## 3. Scenario files

A scenario is a list of input events keyed by frame:

```json
{ "size": [1280, 850], "frames": 120,
  "events": [
    {"at": 5,  "mouse_move": [640, 400]},
    {"at": 6,  "mouse_down": 0}, {"at": 7, "mouse_up": 0},
    {"at": 20, "key": "ArrowLeft"},
    {"at": 40, "text": "hello"}
  ] }
```

Two sources:

- **Hand-written**, checked in next to the app
  (`examples/games/snake/scenarios/*.json`). These reach deep state: start a
  game, lose a life, open a dialog. The verifier's `"checked-in"` scenario
  source looks for them; none exist yet.
- **Generated "monkey" scenarios** from a seed (`petal-ui/src/scenario.rs`):
  random clicks inside the window, random keys, random text. Deterministic
  by seed, so a failing run is replayable by `(app, seed, scenario-seed)`.

Scenarios stay declarative, with no scripting language, so a human can
reproduce one too.

## 4. Corpus classification

The verifier decides how to drive each file by evidence, not a hand list:

| Kind | Signal | Driver |
|---|---|---|
| console | none of the below | `petal run` |
| ui | references a `petal-ui` native (`screen_width`, `draw_*`, `clicked`, …) or lives beside a `layout.ptl` | `petal-ui-run` |
| module | imported by something else, not an entry | skipped as an entry; covered via its importers |
| unsupported | calls a native no driver registers (Garden panels, NES carts) | reported as `unsupported` |

Corpus roots are listed in the plan file: this repo (`examples/`, `test/`,
`garden/`) plus the external projects in `CLAUDE.local.md`.

## 5. Plans and checks

`ts/bin/verify.ts` executes a **plan**: an ordered list of checks, cheapest
first, that short-circuits per file.

```
verify --plan test/verify-plans/lint-fix.json --before <git-ref|dir> --after <dir>
verify --plan test/verify-plans/compiler.json --before-bin old/petal --after-bin new/petal
```

There are two A/B axes, because the two big use cases differ:

- **source A/B, same binary** — `lint --fix`, hand refactors of `.ptl`
- **binary A/B, same sources** — compiler/VM/optimizer changes

A plan lists steps, each a named check with a `stop_on: pass|fail`:

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

1. **compiles** — `petal check` both sides.
2. **ir-equal** — `petal ir-equal` on both sides. For a formatting-only
   refactor this is proof with no execution and no determinism worries, and a
   pass ends the pipeline for that file. For a semantic rewrite (`if` →
   `match`) it fails and the plan falls through.

   Call structure is part of the comparison: each call term carries a
   `call_site` id ([state-call-paths.md](state-call-paths.md)), so extracting
   a helper, inlining one, moving a call into another function, or adding an
   earlier call to the same callee are all reported as differences. That is
   correct, because a `state` slot is keyed by the call path that reaches it.
   So **`ir-equal` is not the check for a call-moving refactor**; only
   `run-diff` can say a call move was safe. Reformatting, re-indenting,
   renaming a non-callee local, and edits elsewhere in the file still pass.
3. **control-run** — run the *before* side twice with the same seed and
   scenario. If they differ, the file is reported as nondeterministic and
   skipped, with the first divergence shown so the missing knob can be added.
4. **run-diff** — run before and after under the chosen driver, with every
   determinism knob set, for each seed × scenario. Compare the JSONL trace
   frame by frame; the first divergence is reported as `frame N, field,
   before, after`.
5. **golden** — optionally freeze the *after* traces as a golden corpus, the
   UI analogue of `test/example-golden/`. `test/ui-golden/index.json` stores
   one sha256 per app trace, not the traces themselves (the example apps at
   60 frames total ~72 MB of JSONL), so a mismatch means "re-run it locally
   and diff".

   Re-baseline deliberately: name the field that moved before running
   `--update-golden`, because a hash that hides a behavior change looks
   exactly like one that does not. A worked example: inserting a block into
   `petal-ui/prelude/ui.ptl` shifted the ordinal in the display label a
   state dump puts on an unnamed callsite (`ui::button#1/<expr>#63/…` became
   `…/<expr>#78/…`). The argument that this was cosmetic: `state` was the
   only field that differed and the `commands` arrays were identical;
   normalizing `<expr>#\d+` made the traces byte-equal; and `<expr>#N` is
   display-only, since the real key comes from `compiler/state_ids.rs` and
   the values never moved. Absent such an argument, the diff is a `changed`
   verdict, not a re-baseline.

Output: one table row per file (`kind`, `steps run`, `verdict`), non-zero
exit if any `changed`. Verdicts: `identical-ir`, `identical-trace`,
`nondeterministic`, `changed`, `compile-error`, `driver-error`,
`unsupported`, `module`.

## 6. Replayability

Every run writes a bundle under `.temp/verify-runs/<timestamp>/<file>/`
(gitignored):

```
plan.json              # the plan as resolved for this file
scenario.json          # exact events, including generated monkey ones
seed                   # PETAL_SEED used
before.jsonl  after.jsonl
repro.sh               # the two commands that reproduce the diff
```

`repro.sh` is the deliverable of a failure. It works with no other context:

```sh
PETAL_SEED=2 petal-ui-run examples/games/snake/app.ptl \
  --scenario .temp/verify-runs/.../scenario.json --frames 120 --out /tmp/a.jsonl
```

## 7. Self-checking lint

`petal lint --fix --verify` runs `ir-equal` on the file it just rewrote and
refuses to write on failure. `--verify=ir` (the default) accepts the semantic
passes that are expected to change the IR; `--verify=strict` demands IR
equality of the whole rewrite. See `petal lint --help`.

## Not built

- **Garden driver.** Every `layout.ptl` and Garden window script lands as
  `unsupported`, which is honest but leaves 20-odd files of the corpus
  unverified. The Garden headless debug server (`/frame`, `/mouse`, `/key`)
  is the natural driver; it would need a fixed-clock flag.
- **A fantasy-NES / petal-fps driver**, for the same reason.
- **Hand-written scenarios** beside the apps; everything is monkey-driven
  breadth today.
- **CI plan for compiler changes**: `--plan compiler` on PRs touching
  `rust/src/backend` or `rust/src/compiler`, before-bin = a `main` build.
- **Handle id masking** in `--json` output. No corpus file prints a handle,
  so no run has produced a spurious `slot#serial` diff.

## Further ideas: beyond whole-program diffing

The verifier compares whole-program output. Three tools would answer finer
questions; none is built.

- **`verify-equiv <old> <new>`** — the output-identity oracle between two
  source versions of a *function* over an enumerated or property-based input
  domain. For a pure branch refactor (a chain of last-wins `if`s rewritten as
  first-wins `if`/`elsif`) an enumerated domain is effectively exhaustive.
  Only as strong as the input corpus, so a check rather than a proof.
- **`blast-radius <span>`** — map a changed source span to its terms via
  `source_map`, then report the forward-reachable outputs (draw commands,
  prints, state writes) via `trace_dependents` + `slice`. "This change can
  only affect these N outputs."
- **Span-keyed trace diff** — the per-term trace (`--record-trace`) already
  captures every term's inputs and result, but term ids are compile-order and
  unstable across an edit. Keying events on source location or a structural
  term key would let two runs be aligned, so a refactor that leaves the
  observable dataflow identical produces a diff-clean trace even when the IR
  shape changed. This is the closest thing to "prove no observable change"
  without an SMT-grade normalizer.

A static equivalence proof (IR canonicalization plus subgraph equivalence) is
research-grade machinery and is not planned; structural IR comparison is
sound but rejects most real refactors, which is why `ir-equal` is only the
first step of a plan.
