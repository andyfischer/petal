# Open work, ranked

Status: 2026-11-11, checked against `c8e51996`. Each item was reproduced with
a debug build of that commit or found in the source, unless it says it could
not be checked. This file indexes what is still open across `docs/tasks/`,
the design records in `docs/dev/` and `docs/proposals/`, and the example
debriefs. The detail lives in the linked files.

The ranking puts silent wrong results first, then hard failures and
misleading diagnostics, then downstream drift, then performance in the
live-coding loop, then language growth.

History: the 2026-10-08 list is at `git show b0dfccc7:docs/tasks/todo.md`.
Tests cite its numbering (items 5 and 9). The 2026-11-02 list is at
`git show c8e51996:docs/tasks/todo.md`. Removed on 2026-11-11 as resolved:
host records with sorted keys (fixed 2026-11-03), the module-system fix plan
(done 2026-11-04), the `{invert}` record message (done 2026-11-09),
petal-web-html's panic hook, the SipHash intern table (now Fx), and the
`PB_FX_CONTAINED` re-vendor (Cheesecake and worlds-fair both carry it).
Deleted as shipped: `tasks/annotation-syntax.md` and `tasks/migration-tool.md`.

## 1. The install one-liner is still a 404

`https://petal-lang.org/install.sh` returned 404 on 2026-11-11 (as on
2026-10-23). It is the README's first command, so every new user hits it.
Fix the hosting, then test the one-liner on a clean machine
([dev/release-checklist.md](../dev/release-checklist.md) step 8).

## 2. Silent wrong results

- **`memo` initializers that are not pure.**
  [memo-purity-check.md](memo-purity-check.md). `memo(k) v = k * scale` hands
  the first caller's `scale` to every caller: `scaled(2, 10), scaled(2, 100)`
  prints `20 20`, and `petal lint` is clean (re-checked 2026-11-11). The plan
  (a seeded mode of the static classifier) is written; nothing is built.
- **Bare names from `import bloom` override `ui`'s with no warning**
  ([ui-refactor.md](ui-refactor.md), "Left"). `clicked`, `theme`,
  `popover_open`, `select` and five more change meaning when a file adds
  `import bloom`. Rename bloom's, or warn when a second plain import changes
  what a bare name resolves to.
- **Colour channels wrap instead of clamping.** `color_of` and the
  positional draw natives (`core-libs/petal-ui/src/draw.rs`) convert with
  `as u8`, so 260 draws as 4 and −20 as 236
  ([website-improvement-ideas.md](website-improvement-ideas.md) item 5).
  A one-line fix plus a test.
- **`int(NaN)` is `0`.** `int(sqrt(-1.0))` prints `0` natively; the website
  reported a wasm trap (item 6 there). Either way it should be a runtime
  error with a span.

## 3. `lint` and `show-static` miss things they claim to see

- **`list + list` on a parameter passes `lint`.** `fn f(xs) xs + [1] end`
  lints clean and fails at run (re-checked 2026-11-11). `[1] + [2]` is
  caught, with "use ++". `prefer-compound-assign` was seen turning
  `segs + branch(1)` into `segs += …`; that rewrite was not re-tested.
- **`show-static` reports `let`s read in an `if` expression as "not read".**
  `let c = if r > 0.5 then LIME else NAVY end` still marks `LIME` and `NAVY`
  unread (re-checked 2026-11-11). Check whether the classifier itself, and
  not only the report, misses these reads before trusting `const` folding
  near an `if`.

## 4. Re-vendor wave (downstream drift)

Three landings since the last vendoring change what embedders see:
bloom owns the widgets (R4, 2026-11-10), dual numbers are gone
(`Value::Dual`, the C bridge's dual view; 2026-11-11), and the
differentiation rewrite's edit modes and `speculate` fast path.

- **petal-lang.org**: `pnpm build:sdk` + `scripts/sync-examples.mjs`. The
  site's `aim.ptl` already calls `deriv`, which the vendored wasm lacks; the
  DM gallery's `settings-card.ptl` still uses `ui.card` / `progress_bar`;
  the SDK predates `ui/drag`, `speculate` and the memo/emit-trace fix.
- **Cheesecake** (vendored `07b0ad48`, 2026-11-09): port its remaining calls
  of deprecated `ui` widgets (e.g. `games/trailkeeper-art-tool`), then
  re-vendor. Its `third_party/petal/VENDORED.md` (lines 14 and 187) says
  `petal lint --fix` rewrites `const`/`config let` and `state(key)`; after
  the re-vendor it should say `petal migrate-old-code`. Its next re-vendor
  archives `core-libs/gpp` instead of `garden/gpp` and drops its one local
  modification (`gpp/Cargo.toml`).
- **worlds-fair** (`vendor/petal`, 2026-11-10): picks up the dual removal.

## 5. Goal-based edits reach element children (done 2026-11-11)

Done: XPath-style child steps (`layers[2]/group[0]/image[1].x`,
[edit-paths.md](../features/edit-paths.md)) with set / insert / remove /
move as minimal splices, and `to_source` writes `{x: 1}`
([images-followup.md](images-followup.md) items 1 and 3). Left downstream:
petal-images can delete `Doc::edit_nested` and its whole-`layers`
rewrite, and petal-creative's `crates/movie/src/effects.rs` tests expect the
old `{ type: … }` padding.

## 6. Differentiation rewrite: small leftovers

[differentiation-rewrite.md](differentiation-rewrite.md), "Left open".
`autodiff`'s `_run` should use `speculate_value`;
`tools/dm-mode-integration-test.ts` has 12 failing steps (pre-existing);
Phase 4 (computed layers) is petal-creative's.

## 7. Cut a release

Nothing has been released since `v0.1.0` (2026-10-24). Several removals are
gated on "one release after deprecation": lifecycle Phase 6 (the `const` and
`config let` keywords; [lifecycle-annotation-functions-refactor.md](lifecycle-annotation-functions-refactor.md)),
the `drag_solve_step` alias, `vary`'s ignored `sensitivity` key, loading a
stored dual from state storage, and eventually `ui`'s deprecated widgets.

## 8. Static-value cache: UI panels and recompile cost

[static-value-optimization.md](static-value-optimization.md).

- **Item 1, per-input-class slots** (reactive-rendering P2): values that
  depend only on the viewport or theme. This is where panels would gain:
  today they get about 1%.
- **Item 2:** classification adds 30–100 ms to every structural edit.
- **Item 4:** function-body constants are rebuilt after every reload. Moses
  rebuilds 4,381.
- **Item 5:** literal `Enum.variant.key` reads are not folded.

## 9. Declarative dragging: drag-start cost

[declarative-dragging.md](declarative-dragging.md) open issue 1: 19 ms at 50
kanban cards, 53 ms at 100, mostly `speculate` recording draws as `emits`
that the engine throws away. A core option to drop emits would fix it. The
rest of that list (memo inside `speculate`, quadratic in-place edits, runtime
warnings only printed by `petal run`) follows.

## 10. Language growth that a second large game will hit

[moses-debrief.md](moses-debrief.md). These are held back by the owner, and
each needs a proposal (`propose-language-change`):

- checked record shapes;
- `Set`, and the bit operators (`&` does not lex);
- growable numeric arrays and in-place record tables;
- module state without `get`/`set`;
- tuples and destructuring.

Related, and deferred on 2026-10-09: amortized O(1) `++` and in-place update
in general, which is a matter of better alias checking.

## 11. Interpreter and GC

From [moses-debrief.md](moses-debrief.md), "VM":

- reuse of swept slots, and a nursery. GC is 12% of a Moses career.
- a two-target `Branch`, and register coalescing.
- the one-character ASCII intern cache
  ([optimizations/string-intern-hashing.md](optimizations/string-intern-hashing.md));
  the Fx hasher half is done.

## 12. Direct manipulation v2: what the shipped engine leaves open

[dev/direct-manipulation-v2-plan.md](../dev/direct-manipulation-v2-plan.md)
§8 and §9, and [dev/direct-manipulation-v2-study.md](../dev/direct-manipulation-v2-study.md)
§3:

- **Cost:** every speculative run deep-copies the heap. Per-run statics
  invalidation (§2.3.5a) was not built.
- **Missing:** multi-frame horizons, for goals that depend on a literal
  through accumulated state (§8 item 4; hopper debrief ideas 1 and 6).
- **Branches and steps:** a branch flip needs a re-measure, and flat step
  functions get nothing.
- **Code review's left-open list** (§9): an unrounded best-effort value, `1e3`
  respelled `2500.0`, the `ObsValue` depth cost, a cancelling float/int
  screening pair, and `rigid` at an unreached place.
- **Docs:** `features/time-travel.md` should say that spec mode contains
  escaping effects (§4.5). The v2 study report still describes the old
  `l10b-ui-panel` goals (re-run `full_study` with `DM_STUDY_WRITE`).

## 13. Draw coordinates and the web hosts

Sub-pixel drawing shipped as an opt-in on 2026-11-08 (petal-ui, petal-web,
petal-sdl). The `i32` draw format remains the default, so hosts that do not
opt in (Garden, worlds-fair) still truncate. petal-lang.org has not switched
to the sub-pixel drain. The site's other per-frame costs (canvas pool,
cached layers, blend modes) are in
[website-improvement-ideas.md](website-improvement-ideas.md).

Wasm size: [wasm-size-improvements.md](wasm-size-improvements.md). Item 3
(`fn_introspect` behind a feature) has no run-time cost and should land;
serving brotli gets the transfer under 1 MB.

## Smaller

- **Diagnostics:**
  - `json_encode` / `to_json` should suggest `json_stringify` and
    `import json` (still "unknown function", 2026-11-11);
  - `petal.toml` cannot set a minimum Petal version: `[requires] petal` is
    read as a host interface.
- **Web hosts** (from the facetedllc.com report):
  - petal-web-canvas has no `pixelRatio` option or `dpr` native, so
    retina strokes blur;
  - neither host is an npm package with dist JS and `.d.ts`;
  - `panic = "abort"` is not set for the wasm build.
- **Repository layout**
  ([skills/project-organization.md](../skills/project-organization.md#current-misplacements-audit-of-2026-11-04-updated-2026-11-05)):
  - `glsl` is bundled though experimental; Cheesecake imports the bundled
    copy, so unbundling waits on its script host registering its own;
  - `petal-fps` and `petal-fantasy-nes` wait on a small in-repo example of
    petal-sdl's `Host` trait before they can leave;
  - `core/prelude/std.ptl` into `core/packages/std/` needs `std` registered
    as a package;
  - the `petal-sdl` crate and binary rename waits on downstream references.
- **Headless UI from the CLI:** `petal run` refuses `ui` scripts. Today the
  answer is `petal-sdl --headless --frames N`, or `petal-ui-run` from
  core-libs. A one-command way to report draw counts and frame times is still
  missing.
- **Measurement and text:**
  - kerned measurement needs a `FontSource` hook for a whole run's width.
    petal-sdl turns kerning off (`integrations/petal-desktop-sdl/src/font.rs`).
  - rich spans and flow layout are the rest of typography phase 2.
- **AST offsets are characters:** `SourcePosition::offset` is not documented
  as such, and there is no byte-range helper
  ([images-followup.md](images-followup.md) item 4).
- **Stdlib:**
  - a builtin from the wall clock to a calendar date;
  - `split("", " ")` returns `[""]` (side-scroller debrief; still so).
- **`petal test`:** catching an expected runtime error, setup and teardown,
  and the call chain for a failure inside a helper.
- **Examples:** in `examples/productivity/spreadsheet`, `digit_val` and
  `letter_val` scan with `slice`. A record lookup would cut the
  formula-commit frame, where `slice` is 71% of builtin calls.
- **Type declarations:** `run --json` has no `warnings[]` on success, and
  there is no `// @strict` pragma
  ([dev/type-declarations-plan.md](../dev/type-declarations-plan.md) §4).
- **Reactive rendering, the rest of P0 and P1:**
  - segmented output and damage rects;
  - LIS moves in petal-web-html;
  - loop-body and top-level memo scopes;
  - hot-reload invalidation by body hash
  ([dev/reactive-rendering-plan.md](../dev/reactive-rendering-plan.md)).
- **Pending values:** move `host_data` onto petal-query (step 5 of
  [dev/pending-values-plan.md](../dev/pending-values-plan.md)).
- **Migrations:** `push`/`pop` (deprecated aliases of `append`/`drop_last`)
  have no migration; a rename rule would need name resolution to avoid a
  user's own `push`.
- **Tests and docs hygiene:**
  - `test/vitest/doc-snippets.test.ts` covers only `docs/`, with
    `check --lenient`;
  - `editor-support/tree-sitter-petal/grammar.js` comments still say
    `rust/src/…` (10 places);
  - `core-runtime/glsl/tests/effects.ptl:4` cites a Cheesecake path that has
    moved;
  - `docs/features/inline-state.md` has three `TODO` examples (recursion
    and two debugger views; website item 7).
- **Testbed:** 8 of the 50 target apps are unbuilt
  ([dev/testbed-challenge-plan.md](../dev/testbed-challenge-plan.md)).
- **Could not be checked here** (needs `~/worlds-fair`, Moses or a quiet
  machine):
  - the `test/ui-golden/index.json` hashes for apps the driver skips as
    unsupported, and the 63 apps whose goldens were already stale before D2;
  - Moses' recompile time (488 ms then about 800 ms);
  - the dated figures in `dev/performance.md`;
  - SDL sub-pixel in a live or HiDPI window.
