# Open work, ranked

Status: 2026-11-02, checked against `1064dcd`. Each item was reproduced with
a debug build of that commit or found in the source, unless it says it could
not be checked. This file indexes what is still open across `docs/tasks/`,
the design records in `docs/dev/` and `docs/proposals/`, and the example
debriefs. The detail lives in the linked files.

The ranking puts silent wrong results first, then hard failures and
misleading diagnostics, then performance in the live-coding loop, then
language growth.

History: the 2026-10-08 list is at `git show b0dfccc7:docs/tasks/todo.md`.
Tests cite its numbering (items 5 and 9). The 2026-10-23 list ranked the
release first. `v0.1.0` has since been published (GitHub release,
2026-10-24), so that item is now a short check under "Smaller". Removed on
2026-11-02 as resolved: `dev/wasm-use-case-feedback-10-15.md`. Its blocker,
the CI wasm smoke test, the panic hook, the `std::time` lint, `BUILD_INFO`
and the size profile have all shipped, and its leftovers are below.

## 1. `memo` initializers that are not pure give wrong answers silently

[memo-purity-check.md](memo-purity-check.md). Nothing checks that a `memo(k)`
initializer reads only its key. `memo(k) v = k * scale` hands the first
caller's `scale` to every caller: `scaled(2, 10), scaled(2, 100)` prints
`20 20`, and `petal lint` is clean. The `state-key-deprecated` auto-fix no
longer makes such memos (fixed 2026-11-03: it rewrites only an initializer
built from its key alone). The check itself is planned in that file, as a
seeded mode of the static classifier.

## 2. *(Fixed 2026-11-03)* Hosts got records with their keys sorted

Core now turns on serde_json's `preserve_order`, so `value_to_json`,
element `props`, `json_stringify` and `json_parse` keep the written order
(`core/tests/json_key_order.rs`). Still open from
[images-followup.md](images-followup.md) item 2: a `value_to_static` that
skips JSON.

## 3. *(Done 2026-11-04)* Module system

Every issue in [module-system-followup.md](module-system-followup.md)'s fix
plan is fixed: loose files are named `./level` and no longer collide with
packages, nested imports and nested-package paths are errors, slips after an
import name their fix, trace frames name their file, and the arity-merge and
facade-ambiguity messages name the right module. What is left there is the
"Smaller things" list (formatter and `fix-imports` details, one misleading
`set` message) and the documentation follow-up.

## 4. `check` and `show-static` miss things they claim to see

- **`list + list` through a function or parameter passes `check`.**
  `[1] + [2]` is caught, with "use ++". But `segs + branch(1)`, where `branch`
  returns a list, and `xs + [1]` on a parameter both pass and then fail at
  run. `prefer-compound-assign` was seen turning the first into
  `segs += …`, equally wrong. That rewrite was not re-tested.
- **`show-static` reports `let`s read in an `if` expression as "not read".**
  `let c = if r > 0.5 then LIME else NAVY end` marks both `LIME` and `NAVY`
  unread, inside a `for` or not. Check whether the classifier itself, and
  not only the report, misses these reads before trusting `const` folding
  near an `if`.

## 5. Goal-based edits cannot reach element children

[images-followup.md](images-followup.md) items 1 and 3. `literal_edit`
takes `.attr` on an element but no `children[j]` step
(`core/src/literal_edit.rs`), so structural edits inside a `<group>` rewrite
the whole list. That loses the comments and hand formatting between tags.
Related:

- `StaticValue::to_source` pads records as `{ x: 1 }`, unlike the files.
- A `//` line inside element children is a comment now (item 5, done
  2026-11-09); a rewriter that edits siblings must keep such comments.

Together these make a program edited by its own GUI drift from how it was
written, which is the project's core promise.

## 6. Static-value cache: UI panels and recompile cost

[static-value-optimization.md](static-value-optimization.md).

- **Item 1, per-input-class slots** (reactive-rendering P2): values that
  depend only on the viewport or theme. This is where panels would gain:
  today they get about 1%.
- **Item 2:** classification adds 30–100 ms to every structural edit.
  Incremental classification was considered and set aside (see that file).
- **Item 4:** function-body constants are rebuilt after every reload. Moses
  rebuilds 4,381.
- **Item 5:** literal `Enum.variant.key` reads are not folded. `show-bytecode`
  shows `enum "Kind.park"` and then a field read.

## 7. Draw coordinates are `i32` in the command format

`coord_to_i32` and `as_i64` in `core-libs/petal-ui/src/draw.rs` truncate
every float coordinate toward zero, so they do not round. Rotating polygons
jitter by up to a pixel per vertex, and slow parallax layers stair-step
(side-scroller debrief). A fix changes the draw-command format and every
host: Garden, petal-sdl, petal-web and worlds-fair. Rotated text would be
cheap to add in the same change.

## 8. Language growth that a second large game will hit

[moses-debrief.md](moses-debrief.md). These are held back by the owner, and
each needs a proposal (`propose-language-change`):

- checked record shapes;
- `Set`, and the bit operators (`&` does not lex);
- growable numeric arrays and in-place record tables;
- module state without `get`/`set`;
- tuples and destructuring.

Related, and deferred on 2026-10-09: amortized O(1) `++` and in-place update
in general, which is a matter of better alias checking.

## 9. Interpreter and GC

From [moses-debrief.md](moses-debrief.md), "VM":

- reuse of swept slots, and a nursery. GC is 12% of a Moses career.
- a two-target `Branch`, and register coalescing.
- faster interning:
  [optimizations/string-intern-hashing.md](optimizations/string-intern-hashing.md).
  The table is still on SipHash. Reuse `core/src/fxhash.rs`, and add a
  128-entry ASCII cache for one-character slices.

## 10. Direct manipulation v2: what the shipped engine leaves open

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
  escaping effects (§4.5).
- **Outside this repo:** re-vendor `petal-c-bridge` (with `PB_FX_CONTAINED`)
  into cheesecake and worlds-fair.

## Smaller

- **Release:** `v0.1.0` is out. Nobody has re-checked whether
  `https://petal-lang.org/install.sh` now serves the installer (it was a 404
  on 2026-10-23), or tested the one-liner on a clean machine
  ([dev/release-checklist.md](../dev/release-checklist.md) step 8). The
  README's first command depends on it, so check it first.
- **Diagnostics:**
  - `json_encode` / `to_json` should suggest `json_stringify` and
    `import json`;
  - `{invert}` in a record should say "did you mean `invert: true`?";
  - `petal.toml` cannot set a minimum Petal version: `[requires] petal` is
    read as a host interface.
- **Web hosts** (from the facetedllc.com report):
  - petal-web-html has no panic hook, and still imports `./renderer.js`;
  - petal-web-canvas has no `pixelRatio` option or `dpr` native, so
    retina strokes blur;
  - neither host is an npm package with dist JS and `.d.ts`;
  - `panic = "abort"` is not set for the wasm build.
- **Repository layout** (updated 2026-11-05;
  [skills/project-organization.md](../skills/project-organization.md#current-misplacements-audit-of-2026-11-04-updated-2026-11-05)):
  the 2026-11-04 audit's moves are done (`gpp` in `core-libs/`, the
  element-tree wasm host in `integrations/petal-web-html`, the CLI/LSP behind
  a `tooling` feature that embedders turn off, two tools moved next to
  examples). Still open:
  - `glsl` is bundled though experimental; Cheesecake imports the bundled
    copy, so unbundling waits on its script host registering its own;
  - `petal-fps` and `petal-fantasy-nes` wait on a small in-repo example of
    petal-sdl's `Host` trait before they can leave;
  - `core/prelude/std.ptl` into `core/packages/std/` needs `std` registered
    as a package;
  - the `petal-sdl` crate and binary rename waits on downstream references.
  Downstream follow-ups: Cheesecake's next re-vendor archives
  `core-libs/gpp` instead of `garden/gpp` and drops its one local
  modification (`gpp/Cargo.toml`); a page that vendors petal-web-html's pkg
  now loads `petal_web_html.js`, not `petal.js`.
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
  - `split("", " ")` returns `[""]` (side-scroller debrief).
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
- **Tests and docs hygiene:**
  - `test/vitest/doc-snippets.test.ts` covers only `docs/`, with
    `check --lenient`;
  - `editor-support/tree-sitter-petal/grammar.js` comments still say
    `rust/src/…`;
  - `core-runtime/glsl/tests/effects.ptl:4` cites a Cheesecake path that has
    moved.
- **Testbed:** 8 of the 50 target apps are unbuilt
  ([dev/testbed-challenge-plan.md](../dev/testbed-challenge-plan.md)).
- **Could not be checked here** (needs `~/worlds-fair`, Moses or a quiet
  machine):
  - the `test/ui-golden/index.json` hashes for apps the driver skips as
    unsupported;
  - Moses' recompile time (488 ms then about 800 ms);
  - the dated figures in `dev/performance.md`;
  - petal-lang.org's SDK, which is still on an old WASM build.
