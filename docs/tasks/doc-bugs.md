# What the docs pass found and did not fix

Status: **all items in sections 1, 2, 4 and 5 fixed**, 2026-10-16. Section 3 is
done except for the numbers that could not be re-measured, which are now marked
in the docs as dated or approximate, and the Garden row of
`usage/text-and-fonts.md`. The list was opened on 2026-10-13, while
reorganising `docs/` and writing the new feature and internals pages. The docs
were corrected then to say what the code did. The items here were in code, in
files outside `docs/`, or could not be verified.

Each item says what was done and the commit. The integration run on 2026-10-16
passed: core `cargo test` (2,828 passed), petal-ui (369), petal-sdl (69),
petal-query (33), petal-c-bridge (43), garden-script, garden-app and
directory-browser (1,180), vitest (1,694 in 75 files), and
`tools/test-examples.ts` (33 of 33).

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
   from 78 to 0. **Still open:** `petal check` does not read `[host]`, and a
   relative import inside a prelude file resolves against the document's
   directory.

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
  **Not re-run:** the 343-file sweep outside the repo, which is now marked as
  dated.
- [dev/performance.md](../dev/performance.md) **(0683d98f):** the test name is
  fixed. No figures were re-measured.
- [usage/embedding-c.md](../usage/embedding-c.md) **(4f6813c6):** the handle
  row of the value table is corrected. The slot and serial are in
  `raw()->integer` and `raw()->count`, `str()` is the class name, and
  `integer()` returns its fallback. A new `petal-c-bridge` test pins this. The
  claim about runtime error positions was checked against the code. **Not
  re-measured:** the Cheesecake performance figures, which are now marked as
  one past measurement.
- [game-engine-features/petal-to-glsl.md](../game-engine-features/petal-to-glsl.md):
  a passing test confirms the caching table. The instruction counts are still
  not measured; they were already written as approximate.
- [usage/text-and-fonts.md](../usage/text-and-fonts.md): the SDL claim of
  synthetic bold and oblique holds. It now applies to an installed family's
  upright regular face too (7887bb86). **Still open:** no one has checked the
  Garden row.
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

**Still open: three pending-value behaviours differ from
[dev/pending-values-plan.md](../dev/pending-values-plan.md).** The new page
describes what the code does. Whether the code or the plan is right has not
been decided:

- `u && x` returns `x` and `u || x` returns a boolean; neither returns the
  pending value.
- Calling a pending value raises `Cannot call pending` instead of absorbing.
- `repeat` with a pending count absorbs instead of being a hard error.

Links to the new pages from `Builtins.md` and from the explain and graph
sections of `CLI.md` were not added.
