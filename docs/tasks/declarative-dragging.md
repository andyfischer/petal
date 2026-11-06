# Declarative dragging (D2): status

Status, 2026-11-06: **P0 through P4 have shipped** on `main`. The design is
[the proposal](../proposals/declarative-dragging.md); its "Implementation
notes" section records every place the build departs from the design and is
authoritative over the Design section where they differ. The user guide is
[core-libs/petal-ui/docs/drag.md](../../core-libs/petal-ui/docs/drag.md); the
`speculate` primitive is [docs/features/speculate.md](../features/speculate.md).

## What shipped

| Phase | Commit | What |
|---|---|---|
| P0 | fd149a67 | `ui/drag` (`import ui/drag`, not re-exported by `ui`): `place`, `draggable`, `drag_session`, the spec constructors (`fixed_state`, `closest`, `floating`, `snap_radius`, `when_far`, `drop_target`, `on_drop`, `switch_to`), the candidate helpers (`moves`, `moves_nested`, `tree_moves`, `choices`). Examples `examples/ui/d2-list`, `d2-hanoi`, `d2-beads`. |
| P1 | cb8c5c00, b5edbe7f | `speculate(f, ...args)` in the core VM (undo log over state and cells, heap allocation epochs for in-place edits, escaping natives not called, emits captured), `speculating()` / `speculate_place(key, value)`. `ui_measure` runs on it. |
| P0/P1 | 7e331c52 | Kanban ported to `ui/drag` (605 → 531 lines); `ui/drag` speculates with core `speculate`; `place_origin`, `drag_key`. |
| P3 | 9b7266b9 | `get_in` / `set_in`; multi-generator `for a in xs, b in ys do … end` (one desugaring shared by parse.rs and cst_project.rs, tree-sitter grammar). |
| P4 | 6dc8d7b2 | `drag_debug` / `drag_explain`, the `ui_debug(name)` input native (F8 in petal-sdl, Cmd/Ctrl+Shift+G or `:DragDebug` in Garden, `--ui-debug drag`), `drag_drops` channel, `petal-ui-run --drag-trace`, scenario `drag` sugar, MCP `DragExplain`, `petal::drop_edit` (a drop on a `let` literal becomes `should_insert` / `should_remove` / `should_set_path`), Garden drag-mode model drops. |
| P2 | b6529d3e | `between` (Delaunay blend, `drag_triangulate` / `drag_weights`) and `vary` (one Gauss–Newton step per frame through `drag_solve_step`, forward differences by default, duals opt-in) on a DM solver made generic over a `Lever` trait. Examples `d2-switch`, `d2-dial`, `d2-scheduler`. |
| Test | 3b252556, 9bb744b1, this commit | The final test pass: the fixes below, `PRELUDE_LEVEL` 11, four drag goldens re-baselined, this page. |

## How it is tested

| What | Where | Run |
|---|---|---|
| `speculate` leaves nothing behind (7 programs × 4 frames, BASELINE and FAST), optimizer oracle, escaping/contained effects, RNG, emits, nesting error, places | `core/tests/speculate.rs` | `cd core && cargo test --test speculate` |
| `set_in` / `get_in`, multi-generator `for` (values, errors, BASELINE vs FAST, state slots equal to hand-written nested loops, `break` refused) | `core/tests/candidate_state.rs`, `test/ptl/set_in.ptl`, `test/multi-generator-for/` | `cargo test --test candidate_state --test script_cases`; `petal test test/ptl/set_in.ptl` |
| Multi-generator `for` in the CST, formatter and grammar | `cst_project` / `fmt` unit tests, the repo-corpus sweeps, tree-sitter corpus | `cargo test --lib`; `make expensive_lint_tests` |
| Structural drops | `core/tests/drop_edit.rs`, `garden/garden-script/tests/model_drop.rs`, garden-app tests | `cargo test --test drop_edit`; `cd garden && cargo test -p garden-script --test model_drop` |
| DM unchanged by the `Lever` refactor | the core `dm_*` tests, garden-script `dm_drag`, `dm_study ladder` output | `cargo test --test dm_solve` etc. |
| The engine: candidate sets, specs, reorder animation, click vs drag, Escape, release over nothing, snapping, `when_far`, stickiness, ambiguity, `switch_to`, reload mid-drag, the examples against their scenarios | `core-libs/petal-ui/tests/drag.rs` (25), `drag_p2.rs` (13), `drag_tools.rs` (6) | `cd core-libs/petal-ui && cargo test --test drag --test drag_p2 --test drag_tools` |
| Every `ui/drag` export documented | `core-libs/petal-ui/tests/reference.rs` | `cargo test --test reference` |
| Frame gate, memo, static values over the panel corpus (picks up `examples/ui/*`) | petal-ui `gating`, `memo`, `static_*` tests | `cargo test` in core-libs/petal-ui |
| UI goldens (60-frame `monkey:1`, seed 1) for the drag examples and kanban | `test/ui-golden/index.json` | `./tools/verify.ts --plan compiler --only 'examples/ui/d2-*' …` |
| `examples/console/speculate.ptl` golden, opt vs `--no-opt`, static cache on/off | `test/example-golden/speculate.json` | `./tools/test-examples.ts` |
| `DragExplain` | `test/vitest/petal-mcp-drag.test.ts` | `npx vitest run test/vitest/petal-mcp-drag.test.ts` |
| Doc snippets, stdlib manifest | `test/vitest/doc-snippets.test.ts`, `stdlib-extract.test.ts` | `npx vitest run`; `npm run stdlib:json && git diff --exit-code docs/stdlib.json` |

## Final test pass, 2026-11-06

On `main` at 9bb744b1 plus this commit's changes:

| Suite | Result |
|---|---|
| core `cargo test --no-fail-fast` | 82 binaries, 2967 passed, 0 failed, 16 ignored |
| core-libs/petal-ui `cargo test --no-fail-fast` | 35 binaries, 453 passed, 0 failed, 7 ignored |
| garden-script `cargo test -p garden-script` | 217 passed, 0 failed, 1 ignored (3 bloom tests failed before fix 2 below, and already failed at 3fe1d922, before D2) |
| garden-app `cargo test -p garden-app` | 1004 passed, 0 failed |
| petal-desktop-sdl `cargo test` (`LIBRARY_PATH=/opt/homebrew/lib`) | 82 passed, 0 failed |
| `npx vitest run` | 79 files, 1793 tests, all pass |
| `npx tsc --noEmit`, `docs/stdlib.json` sync | clean, current |
| `./tools/test-examples.ts` | 34 passed, 0 failed |
| `petal lint` / `petal fmt --check` on the 15 new `.ptl` files | fmt clean; lint: no errors, style hints only (`prefer-let` in three test files, kanban's 8 `+=` hints that the old app had too) |
| core `cargo clippy --target wasm32-unknown-unknown` (std::time) | clean |
| wasm builds + `node tools/wasm-smoke.ts --require canvas,html` | every script ok |
| `make expensive_lint_tests` (lint, fmt and CST sweeps over the repo) | 4 passed |
| `./tools/verify.ts --plan compiler`, binary A/B of 3fe1d922 (before D2) against now | see below |
| `./tools/oracle-external.ts` (worlds-fair, release runner, `wf-ui` built into a scratch target dir) | all 12 fragments agree |

**The binary A/B** ran the whole `compiler` corpus (370 files: `examples`,
`test`, `garden`, `~/.garden`, `~/worlds-fair/ui/ptl`) under the pre-D2
binaries and the current ones. Every difference is accounted for:

- *compile outcome differs* (9): the seven drag apps and kanban import
  `ui/drag`, and two test files use the new `for` and `set_in`. Expected.
- *run-diff* on `examples/console/speculate.ptl` and the speculate benchmark:
  they call the new builtins. Expected.
- *run-diff* on `examples/ui/layers`: two widget-animation slots first written
  inside `ui_measure` are no longer kept, since P1 undoes `ui_measure`'s
  writes. Draw commands are identical. Expected (P1 semantics).
- *run-diff* on bloom apps (bloom-gallery, `bloom_smoke`, memory, settings,
  main-menu): the overload fix below. bloom-gallery's dropdown no longer
  errors; the others draw identically and only the dump label of a call
  through bloom's self-forwarding overloads changed (`<expr>#N` for
  `bloom/button::segmented`); the slot ids, which hot reload matches, did not.
- *golden mismatch* on 63 apps: the same 63 mismatch with the pre-D2
  binaries, so `test/ui-golden/index.json` was already stale for them before
  D2 started. Not re-baselined here (see open issues).

Ecosystem A/B of the same two binaries: `~/petal-creative` and
`~/biz/petal-lang.org` (120 files) are 38 identical traces, the rest modules
or not runnable headless; `petal lint` output over `~/petal-creative`,
`~/cheesecake/games`, `~/worlds-fair/ui/ptl`, `~/biz/petal-lang.org`,
`examples/` and `garden/` is identical except for the lint bug fixed below.

## Fixes made in the test pass

1. **`shadows-builtin` read `ui/drag` into `import ui`** (3b252556). The lint
   collected the exports of every file in a bundled package, so after P0 a
   plain `import ui` plus an app's own `fn place` was reported as shadowing
   `ui`'s `place` (video-timeline, cheesecake's neon `player.ptl`). It now
   collects per module: the main file plus the siblings it `pub import`s.
2. **An overload variant calling its own name reached an import's set**
   (9bb744b1). Pre-existing (garden-script's bloom tests already failed at
   3fe1d922) and outside D2, but in the suites this pass runs: bloom's
   `dropdown(r, label, items)` forwarded to `ui`'s 4-argument `dropdown`. Such
   a name now goes through a hoisted cell, and the cell's value still merges
   the imported set. Test: `core/tests/modules.rs`
   `an_overload_calling_its_own_name_reaches_its_own_set_over_an_import`.
3. **`PRELUDE_LEVEL` 10 → 11** for `ui/drag`, `ui_debug` and the `ui_measure`
   behaviour change (P0 and P4 both left it for later).
4. **Four drag goldens re-baselined** (d2-list, d2-hanoi, d2-beads, kanban).
   They were taken before P2 added `state var _fns` to `ui/drag`. Traces from
   7e331c52's runner and today's differ only in that slot; the draw commands
   are identical on all 60 frames.

## Open issues

Ranked; the proposal's Implementation notes have the detail.

1. **Drag-start cost grows with the square of the board**: 19 ms at 50 kanban
   cards, 53 ms at 100, mostly `speculate` recording every draw as an `emits`
   row that the engine throws away. A core option to drop emits, or spreading
   the drag start over several frames (proposal §Performance), would fix it.
2. **Memo does nothing inside `speculate`**, and a scope around a
   `speculate` call never replays, so a widget that calls `ui_measure` now
   always re-runs. A finer scheme would record reads inside the call.
3. **In-place edits of pre-existing containers copy** during a call, so a
   loop inside `speculate` that appends to an old list is quadratic.
4. **Runtime warnings** (`Env::take_runtime_warnings`, e.g. a native not
   called inside `speculate`) are printed only by `petal run`; petal-sdl,
   Garden and petal-ui-run do not read them yet.
5. **No nested-`speculate` lint**; nesting is a runtime error only.
6. **`vary`**: no goals other than the element's centre, no step-size option,
   no dual-safe `rect()`, no drop-zone map in the debugger, and `closest`
   over n `vary` options costs n×(k+1) renderings per frame.
7. **Overlapping draggables**: the first registered wins.
8. **One drag at a time**; no multi-pointer.
9. **`DragExplain` drives headless runs only**; reaching a running app needs
   a debug-protocol channel for drag state.
10. **A moved element's own trailing comment is lost** by a structural drop
    (needs a `ShouldMove` goal).
11. **Keyboard moves during a kanban drag** change the board under the
    session; the drop commits a board built from before the move.
12. **The ui-golden index is stale for 63 apps** independently of D2 (they
    mismatch under the pre-D2 binaries too). Someone who knows which change
    moved them should re-baseline deliberately.
13. **Not built**: a filter clause and destructuring generators for
    multi-generator `for`; the scheduler's interval-graph second view;
    script-visible program generation (the session detects a reload with a
    `memo(0)` token).

## Ecosystem

No external project defines a name that now collides in a way that changes
behaviour: an app's own declaration wins over `import ui/drag`, and no
external file imports `ui/drag`. Found and checked:

- `~/cheesecake/games/gta/tests/drive_test.ptl` declares `fn get_in()` (zero
  arguments). The core `get_in` takes two, so it is an overload and the
  file's own call still reaches its function; no lint warning.
- `fn place` in several cheesecake games, `fn between` in moses, `let vary`,
  `let moves`, `state moves`: none import `ui/drag`, so nothing changes.
  Cheesecake's neon `player.ptl` got a false `shadows-builtin` warning from
  the lint bug fixed above; it is gone.
- `~/petal-creative/petal-design/packages/studio/layers.ptl` declares
  `fn drop_target`; it does not import `ui/drag`.
- Multi-generator `for`: no existing file had a comma in a `for` header (the
  one match, in moonbear, is a comma between two `for` expressions in a list
  literal, which parses as before).

Needed outside this repo (not done here):

- **Re-vendor Petal** in `~/cheesecake/third_party/petal` and worlds-fair
  (`wf-ui-ffi` / petal-c-bridge) to get `speculate`, `set_in`/`get_in`,
  multi-generator `for`, `ui/drag`, `ui_debug` and the overload fix. Hosts
  register input natives through `register_input` / `register_input_module`,
  so `ui_debug` arrives with the re-vendor.
- **petal-lang.org**: rebuild the WASM SDK (`build:sdk`) to pick up the same.
  A page for `ui/drag` and `speculate` would be new content, not a fix.
