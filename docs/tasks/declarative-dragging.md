# Declarative dragging (D2): status

Status, 2026-11-06: **P0 through P4 have shipped** on `main`. The design is
[the proposal](../proposals/declarative-dragging.md); its "Implementation
notes" section records every place the build departs from the design and is
authoritative over the Design section where they differ. The user guide is
[core-stdlib/ui/docs/drag.md](../../core-stdlib/ui/docs/drag.md); the
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
| `speculate` leaves nothing behind (7 programs × 4 frames, BASELINE and FAST), optimizer oracle, escaping/contained effects, RNG, emits, nesting error, places | `core/tests/all/speculate.rs` | `cd core && cargo test --test all speculate::` |
| `set_in` / `get_in`, multi-generator `for` (values, errors, BASELINE vs FAST, state slots equal to hand-written nested loops, `break` refused) | `core/tests/all/candidate_state.rs`, `test/ptl/set_in.ptl`, `test/multi-generator-for/` | `cargo test --test all -- candidate_state:: script_cases::`; `petal test test/ptl/set_in.ptl` |
| Multi-generator `for` in the CST, formatter and grammar | `cst_project` / `fmt` unit tests, the repo-corpus sweeps, tree-sitter corpus | `cargo test --lib`; `make expensive_lint_tests` |
| Structural drops | `core/tests/all/drop_edit.rs`, `garden/garden-script/tests/model_drop.rs`, garden-app tests | `cargo test --test all drop_edit::`; `cd garden && cargo test -p garden-script --test model_drop` |
| DM unchanged by the `Lever` refactor | the core `dm_*` tests, garden-script `dm_drag`, `dm_study ladder` output | `cargo test --test all dm_solve::` etc. |
| The engine: candidate sets, specs, reorder animation, click vs drag, Escape, release over nothing, snapping, `when_far`, stickiness, ambiguity, `switch_to`, reload mid-drag, the examples against their scenarios | `core-stdlib/ui/native/tests/drag.rs` (25), `drag_p2.rs` (13), `drag_tools.rs` (6) | `cd core-stdlib/ui/native && cargo test --test drag --test drag_p2 --test drag_tools` |
| Every `ui/drag` export documented | `core-stdlib/ui/native/tests/reference.rs` | `cargo test --test reference` |
| Frame gate, memo, static values over the panel corpus (picks up `examples/ui/*`) | petal-ui `gating`, `memo`, `static_*` tests | `cargo test` in core-stdlib/ui/native |
| UI goldens (60-frame `monkey:1`, seed 1) for the drag examples and kanban | `test/ui-golden/index.json` | `./tools/verify.ts --plan compiler --only 'examples/ui/d2-*' …` |
| `examples/console/speculate.ptl` golden, opt vs `--no-opt`, static cache on/off | `test/example-golden/speculate.json` | `./tools/test-examples.ts` |
| `DragExplain` | `test/vitest/petal-mcp-drag.test.ts` | `npx vitest run test/vitest/petal-mcp-drag.test.ts` |
| Doc snippets, stdlib manifest | `test/vitest/doc-snippets.test.ts`, `stdlib-extract.test.ts` | `npx vitest run`; `npm run stdlib:json && git diff --exit-code docs/stdlib.json` |

The final test pass (2026-11-06) and its fixes, and the ecosystem check, are
in `git show 5b28d9f5:docs/tasks/declarative-dragging.md`.

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
   no drop-zone map in the debugger, and `closest`
   over n `vary` options costs n×(k+1) renderings per frame.
7. **Overlapping draggables**: the first registered wins.
8. **One drag at a time**; no multi-pointer.
9. **`DragExplain` drives headless runs only**; reaching a running app needs
   a debug-protocol channel for drag state.
10. **A moved element's own trailing comment is lost** by a structural drop
    (needs a `ShouldMove` goal).
11. **Keyboard moves during a kanban drag** change the board under the
    session; the drop commits a board built from before the move.
12. **Not built**: a filter clause and destructuring generators for
    multi-generator `for`; the scheduler's interval-graph second view;
    script-visible program generation (the session detects a reload with a
    `memo(0)` token).
