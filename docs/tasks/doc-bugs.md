# What the docs pass found and did not fix

Status, 2026-10-17: **every item is fixed or settled**, including the three
pending-value behaviours (section 5) and the follow-ups of the second round
(section 6). What is left is in [Still open](#still-open): a few figures that
could not be re-measured and are marked as dated in the docs, and work outside
this repository. The list was opened on 2026-10-13, while reorganising `docs/`
and writing the new feature and internals pages. The docs were corrected then
to say what the code did. The items here were in code, in files outside
`docs/`, or could not be verified.

Each item says what was done and the commit. The integration run of
2026-10-17 passed:

| Suite | Result |
|---|---|
| core `cargo test` | 2,844 passed, 16 ignored |
| `make expensive_lint_tests` | 4 of 4 sweeps |
| petal-ui | 369 passed |
| petal-sdl | 70 passed |
| petal-query | 33 passed |
| petal-c-bridge | 43 (cargo) and 4 of 4 (ctest) |
| garden-script, garden-app, directory-browser | 1,181 passed |
| vitest | 1,695 in 75 files |
| `tools/test-examples.ts` | 33 of 33 |
| `tools/verify.ts --plan compiler` | 145 identical-trace, 0 changed, golden hashes all match |
| `petal check` | examples (151 files: no errors, 6 with warnings), garden (27), worlds-fair `ui/ptl` (20), petal-movies `ui` (1) all without errors |
| petal-movies | builds, 8 of 8 tests |
| worlds-fair `node ui/tools/check.ts` | green: lint, 44 bundles checked, 130 Rust tests, C ABI smoke test |

The six example warnings are not new: three `log` calls reported as
discarded (petal-fps and petal-fantasy-nes register an effectful `log` of
their own, which `petal check` cannot see), and three functions in
side-scroller and spreadsheet that read a `state` captured at their
definition.

## 1. Probable bugs

### Language server

All three were fixed in 239d0c9f. See [internals/lsp.md](../internals/lsp.md),
"Known gaps", for what is still left.

1. **Fixed: a diagnostic from an imported module lands in the importing file.**
   When the compile succeeds, a module's diagnostic now goes on the importing
   document's `import` line for that module. The message is prefixed with the
   module's file name and has a `relatedInformation` link to the real position.
   A module reached through another module has no import line in the document,
   so its diagnostics go on line 0. The failed-compile path places diagnostics
   the same way, and an unresolved import is reported on its own line.
2. **Fixed: positions are code points, not UTF-16.** `initialize` now
   negotiates `positionEncoding`: utf-16 by default, or utf-8 or utf-32 when
   the client asks for one. Positions coming in and every range going out are
   converted (`core/src/lsp/position.rs`).
3. **Fixed: a script that relies on a host's implicit prelude gets false
   "unknown function" diagnostics.** This is solved with a new `[host]` section
   in `petal.toml`, which takes `imports` and `prelude` files (see
   [packages.md](../packages.md)). The section is not yet in worlds-fair.
   Tried on a scratch copy of worlds-fair, `host/garden.ptl` went from 31
   diagnostics to 2 (two real bad key names) and `screens/missions.ptl` went
   from 78 to 0. **Since fixed** (8587dbe8): `petal check` reads `[host]` the
   way the language server does (shared code in `core/src/project_host.rs`),
   findings land on the script's own lines, and a relative import inside a
   prelude file resolves from that file's directory. A host import that only
   re-exports a name the script uses is now loaded. worlds-fair has its
   manifest (76f666f), and every file in `ui/ptl` checks clean. A script that
   also imports one of the `[host]` imports by name failed with an internal
   error, "module was not compiled before its importer"; fixed in 79ac7e01.

### petal-sdl timeline

All four were fixed in 373d910f. See
[features/time-travel.md](../features/time-travel.md).

4. **Fixed: `replay` after `set_state` discards the written value.** The
   timeline now records each `set_state` against its frame
   (`FrameRecord.writes`) and re-applies it on every later replay, including a
   hot reload's. A write made while frozen replays the frames after the cursor
   straight away, and the reply gives the count in `replayed`.
5. **Fixed: `state` while frozen reads the live execution, not the cursor
   frame.** It now reads the cursor frame's snapshot.
6. **Fixed: windowed `--agent` mode does not rewind `time()`.** The agent loop
   now uses a simulated clock (`ClockSource::Sim`). The clock is rewound along
   with the timeline.
7. **Tested, and it was a bug: sound requests emitted during an unfrozen
   `replay`.** They piled up and all played on the next live frame. A replayed
   frame now discards everything it emits except its draw commands.

### Differentiation

Both were fixed in 8469a156. See
[features/differentiation.md](../features/differentiation.md).

8. **Fixed: builtins that have no derivative rule drop the derivative
   silently.** `exp`, `log`, `pow`, `atan2`, `hypot`, `clamp`, `fract`,
   `lerp`, `map_range` and `smoothstep` now carry the derivative, and so do
   `radians`, `degrees`, `interp` and the scalar `mag`/`distance`. Any other
   function now raises an error when given a dual, instead of dropping the
   derivative. That covers natives too, through `get_float`. `vec2` and `vec3`
   also refuse a dual component, so `dot` can never see one.
9. **Fixed: `sign(dual)` is an error while `abs(dual)` works.** `sign(dual)`
   now returns a dual with derivative 0.

### Handles

Both were confirmed as bugs and fixed in 8469a156. See
[game-engine-features/handles.md](../game-engine-features/handles.md), "Under
the frame gate".

10. **Fixed: `is_valid` under the memo.** A recorded scope did replay a stale
    `true`. `is_valid` is now a host-data read.
11. **Fixed: handle methods and the frame gate.** A run whose only host
    interaction was handle methods could be skipped. A handle method call now
    counts as a host-data read. A host that gates frames must call
    `note_host_data_changed` whenever objects behind its handles change.

### Other

12. **Fixed: `config state`, `config fn` and `config pub let` give a poor
    error.** Each form now has its own message (8469a156).
13. **Fixed: `tools/calc-to-ir.ts` does not run under plain `node`.** The
    parameter property is gone and the file is now executable (8469a156).

## 2. Stale text outside `docs/`

### `petal help`

**Fixed** in 96d50e75 (`core/src/cli/help.rs`):

- `petal help run` now lists `--runs <n>`.
- It gives the default policy as `fast-memo-static`.
- It points `--effect-audit` at
  [usage/ffi.md](../usage/ffi.md#catching-it-with-the-effect-audit).
- `petal help show-static` describes the cache that has shipped.

### Comments that cite files that no longer exist

**Fixed** in 8469a156:

- `core/src/effect_audit.rs` and `core-libs/petal-ui/tests/effect_audit.rs`
  now point at `usage/ffi.md#catching-it-with-the-effect-audit`.
- `core/src/handle.rs`, `builtins/handle.rs` and `core/tests/handles.rs` now
  point at `docs/game-engine-features/handles.md`. `docs/stdlib.json` was
  regenerated.
- The paths in `core/src/program.rs`, `test/vitest/error-positions.test.ts`
  (the reference was dropped) and the `tools/test-examples.ts` header are
  corrected.

### Comments that disagree with the code

**Fixed** in 8469a156:

- The `core/src/run_deps.rs` comments on handles are corrected.
- The `pb_error.phase` comment in `petal_bridge.h` now lists `"link"`.

### Timeline: "after the cursor" is off by one

**Fixed** in 373d910f. `agent-protocol.md`, the petal-sdl `README.md`, the
`timeline.rs` header, the `protocol.rs` comment, and the `replay` row in
[dev/debug-protocol.md](../dev/debug-protocol.md) now say that replay restores
the frame before the cursor and re-runs the cursor frame.

### Other files

**Fixed** in db198339:

- `test/verify-plans/lint-fix.json` no longer lists `docs/examples`.
  `test/vitest/verify.test.ts` now checks that every plan's in-repo roots
  exist.
- `CHANGELOG.md`: six links to old paths are fixed, including
  `hot-reload.md#static-values-across-a-reload`.
- `examples/AUTHORING.md` now links `#draw-surface`.

## 3. Left in `docs/`, not verified

- [features/hot-reload.md](../features/hot-reload.md) **(4f6813c6):** the repo
  reload sweep was re-run (409 files, 16 October), and the `reload_timing` and
  `reload_follow` runs were repeated and added as dated notes. The machine was
  loaded and Moses has grown, so the original tables are kept beside them.
  The sweep outside the repo was re-run too (79bb01e5): 362 files, 147
  relocated, 1 patched, 144 recompiled, all identical to a full recompile.
- [dev/performance.md](../dev/performance.md) **(0683d98f, 8b261d91):** the
  test name is fixed, and the standing figures were measured again on an
  Apple M4, each dated. A new `core/examples/lower_passes.rs` times each
  optimizer pass alone. The "~50 M instructions/s" estimate was wrong (it is
  100 to 400 M/s), and copy propagation removes about a third of
  instructions, not a quarter. Four figures could not be re-measured and are
  marked with their month: the profiler's cost per native call, the cost of
  observation, the speedup on neon, and the pre-change baselines.
- [usage/embedding-c.md](../usage/embedding-c.md) **(4f6813c6):** the handle
  row of the value table is corrected. The slot and serial are in
  `raw()->integer` and `raw()->count`, `str()` is the class name, and
  `integer()` returns its fallback. A new `petal-c-bridge` test pins this. The
  claim about runtime error positions was checked against the code. The
  performance figures were re-measured (4ecf5613, a82b910d) with a new
  standalone bench, `integrations/petal-c-bridge/examples/bench_frame.cpp`: a
  frame of 200 callbacks, 200 emits and 2 draws takes 0.14 ms, and a load
  that imports `ui` 29 ms.
- [game-engine-features/petal-to-glsl.md](../game-engine-features/petal-to-glsl.md):
  a passing test confirms the caching table. The instruction counts were
  measured (2a0231a6, a82b910d): a cached frame is 414 instructions and a
  translating one 4,663.
- [usage/text-and-fonts.md](../usage/text-and-fonts.md): the SDL claim of
  synthetic bold and oblique holds. It now applies to an installed family's
  upright regular face too (7887bb86). The Garden row was checked against
  `garden-render` (bfee7f0a) and corrected: synthetic bold also covers an
  installed family with one weight, italic uses the family's italic cut or a
  14° skew, there is no `serif` role, and an installed family's vertical
  metrics are the built-in defaults.
- [language-guide.md](../language-guide.md) **(66c88d49):** callback recursion
  was measured at 256 levels in a release build and 29 in debug. The bytecode
  shows that `--no-opt` turns off the `match` jump table. Stale class labels
  across a reload are covered by `class_live_edit`.
- [features/direct-manipulation.md](../features/direct-manipulation.md)
  **(0683d98f):** the batch example now explains why plain `petal run` cannot
  produce it, and `radius` is no longer called shared.
- [writing-petal-guide.md](../writing-petal-guide.md) **(66c88d49):** the
  example uses the core `last()` instead of `panel_store_get`.
- [dev/testing.md](../dev/testing.md) **(0683d98f):** the counts are updated
  to 409 files and 1,361 unit tests. The CPU-second and vitest timings are now
  marked as approximate.
- The plan documents under `dev/` are unchanged: they were checked for paths,
  identifiers and links only.

## 4. Content still in the wrong file

**Fixed** in 4f6813c6, 66c88d49 and 63f9a2af. Each piece moved to the file
that owns it, and a link was left in its place.

| Where it was | What | Where it is now |
|---|---|---|
| `usage/embedding-guide.md` | `add_module_path` is best-effort; `package_errors()` | `module-system.md#embedding` |
| `module-system.md` | The project, package and resolver query APIs, `provide_interface` | `usage/ffi.md`, "Modules and packages". `Env` takes no custom resolver; the trait is only used by `module::load_modules`. |
| `usage/embedding-c.md` | `override_file` semantics, `Env::override_file_source` | `module-system.md#embedding` |
| `usage/ffi.md` | The `ReloadReport` fields, now including `new_diagnostics` | `features/hot-reload.md`, "The reload report" |
| `features/direct-manipulation.md` | `--trace-emits --json` fields | `CLI.md` |
| `dev/debugging-visibility.md` | Frontier JSON fields, `--observe` naming | `CLI.md` |
| `dev/testing.md`, `dev/refactor-verification.md` | `--seed`, `--error-format bare` | `CLI.md` |
| `CLI.md` | IR traversal helpers | `dev/ir-as-target.md` |
| `writing-petal-guide.md` | Hoisting rules | new `syntax/declaration-order.md`. The shared-data recipe and the flat-lists advice were already in `syntax/var.md`, so the guide now summarises them and links there. |
| `features/program-modification.md` | Fork and speculation API | Nothing to move: it held nothing beyond `time-travel.md`. The `ffi.md` speculation row was extended. |

## 5. Pages that do not exist yet

**All written** in 66c88d49, 3b949a27, e5d80436 and db81105d, and linked from
`docs/README.md`:

- `syntax/declaration-order.md`
- `features/pending-values.md`
- `features/provenance.md`
- `usage/embedding-web.md`
- `game-engine-features/frame-contract.md`
- `dev/native-effects.md`. The comment in `tools/native-effect-audit.ts` now
  points here.
- `usage/host-io.md`

**Settled: three pending-value behaviours differed from
[dev/pending-values-plan.md](../dev/pending-values-plan.md).** Each was traced
to its origin, and none had been decided; they had been left out. Two were
fixed in the code and one in the plan (d1c6f9e7):

- **`&&` / `||` with a pending left side: the code was wrong, now fixed.**
  `emit_short_circuit` (`core/src/backend/bytecode/lower.rs`) tested the left
  side's truthiness, and a pending value counts as truthy, so `u && x` was
  `x` and `u || x` was `true`. That got around the `if` guard: `if
  user.admin || override then` ran its branch while `user` was loading.
  Chunk F (bab930c7) gave `if` and `while` a `JumpIfPending` but never the
  short-circuit operators, which date from M2a (21b50375). The left side now
  has the same `JumpIfPending`: the right side does not run and the result is
  the pending value. A pending right side is the result when the left does
  not decide it.
- **Calling a pending value: the code was wrong, now fixed.** `u(x)`,
  `u(...xs)` and a method no builtin answers (`u.greet()`) raised `Cannot
  call pending`, while `map(xs, u)`, `u.len()` and `u.handler` absorbed.
  Chunk C (7f8b81ec) added the pending check on the native path only. All
  three now evaluate to the pending value, as the plan's table says
  (`core/src/backend/bytecode/vm/calls.rs`).
- **`repeat` with a pending count: the plan was out of date, now fixed.** The
  plan (2026-07-10) listed a `repeat(n)` count as a hard error before any
  `repeat` builtin existed. `repeat(s, n)` (6906228a, 2026-10-09) is a pure
  strict native, so a pending count makes the result pending and corrupts
  nothing. The plan's row now says count arguments (`repeat`, `range`) absorb.

The tests are in `core/src/env/tests.rs`,
`pending_short_circuit_and_call_tests` (left, right and resolved sides, the
`if` case, every call route). `features/pending-values.md` lost its "two
places do not follow the rule" note, and `CHANGELOG.md` lists the change.

The links to the new pages from `Builtins.md` and from `CLI.md` were added in
8b6f343c, and `syntax/declaration-order.md` is linked from the hoisting
warning in CLI.md's check section (c6400d5a).

## 6. The second round's follow-ups

- **Measured and drawn text width in petal-sdl (30b1e308).** A kerned string
  like "AVAVA" drew narrower than `text_width` measured it. The SDL host now
  loads every font size with kerning off, so the drawn width matches
  petal-ui's per-character table everywhere (truncation, wrapping and flow
  use the same sum). Pairs like "AV" draw slightly looser than real
  typesetting. Test: `kerned_pairs_draw_as_wide_as_they_measure`.
- **Key names across the ecosystem.** Garden passed typed characters to panel
  scripts (`"G"`, `"?"`, `"-"`) where every other host reports the key
  (`"g"` with `mod_shift()`, `"slash"`, `"minus"`); `panel_key_name` now
  names the key (975145e6). The always-false key names were fixed in
  worlds-fair (877824f), session-retro in `~/tools` (d9e98ca, 30cbaa3, which
  also builds it against today's Petal) and the petal-lang.org paint example
  and key mapping (68a8174a in `~/biz`).
- **UI golden hashes (6832c6c9).** All 58 stale entries were traced to their
  commits (state keys renumbered by prelude edits, the `text_ellipsize` fix,
  handlers that now fire on canonical key names); none is a regression. Six
  apps gained entries.
- **worlds-fair's vendored Petal.** `tools/vendor-petal.ts` looked for the
  old `rust/` layout. It now mirrors upstream's (`core/`, `core-libs/`,
  `core-runtime/` and Garden's `garden` interface package, which the
  manifest now takes from the snapshot), and the snapshot was refreshed from
  three weeks back. Hosts have no implicit imports now, so the two host
  shims import `ui`, `color` and (Garden) `garden`, and the engine's four
  seam natives are declared in a small `wf_seam` package that
  `register_seam` registers. `wf-tuning` handles the new `StaticValue`
  variants. The commit in worlds-fair is "chore(vendor): re-vendor Petal in
  its new layout".

## Still open

- **Kerned measurement in petal-ui.** Turning kerning back on in petal-sdl
  needs a host hook in petal-ui's `FontSource`
  (`core-libs/petal-ui/src/text.rs`) that returns a whole run's laid-out
  width, used by `run_width`. Text drawn with letter-spacing can still drift
  from its measurement by whole-pixel advances.
- **Figures marked as dated**, which a quiet machine could re-measure: the
  four in `dev/performance.md` listed above. Recompiling Moses takes about
  800 ms against 488 ms on 13 October; part of that is growth, the rest was
  not looked into.
- **Outside this repo.** petal-lang.org's downloadable SDK
  (`public/downloads/petal-web-canvas/`) is the old build until `pnpm
  build:sdk` is run, and the site's examples still match the older vendored
  WASM.
- **Corpus entries the driver never compares.** `test/ui-golden/index.json`
  keeps hashes for apps the driver marks unsupported (petal-fantasy-nes, the
  petal-fps examples). About 18 files in `~/worlds-fair/ui/ptl` and
  `~/.garden/petal-ide` became unsupported to the verify driver from
  d2899126; nobody has looked at why.
