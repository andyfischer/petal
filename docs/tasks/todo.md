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
`20 20`, and `petal check` is clean. The `state-key-deprecated` auto-fix also
changes behaviour: `lint --fix` rewrites `state(id) t = random(0.0, 1.0)` to
`memo(id) t = random(…)`. The fix reuses the static classifier.

## 2. Hosts get records with their keys sorted

[images-followup.md](images-followup.md) item 2. `value_to_json`
(`core/src/value.rs`) collects into a `serde_json::Map`, and no crate turns
on `preserve_order`. So an element's `props` and every record a native
receives arrive sorted by key: `{x, y, w, h}` arrives as `{h, w, x, y}`. A
host that writes values back to source reorders the user's file, and effect
order cannot follow the file. The fix is one feature flag or an ordered
map, plus a test. A `value_to_static` would let hosts skip JSON.

## 3. Module system: one hard failure, two silent no-ops

[module-system-followup.md](module-system-followup.md). All three still
reproduce:

- **Issue 1:** a loose file named like a bundled package (`color.ptl` beside
  `import ui`) stops the program with "two different sources are both the
  module 'color'". Moses hit the same limit (it renamed `map.ptl`).
- **Issue 4:** an `import` inside a function or `if` is silently ignored. A
  missing path is not even resolved.
- **Issue 8:** a nested package's files are importable through the outer
  package, contrary to packages.md. Fix either the code or the doc.
- **Lower:** issue 5 (trace frames without file names), 6 (the arity-merge
  warning suggests the wrong module), 7 (facade ambiguity names unbound
  qualifiers), and the misleading parse errors for unsupported import forms
  (`import ./my-mod` should point at the new quoted form).

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

[images-followup.md](images-followup.md) items 1, 3 and 5. `literal_edit`
takes `.attr` on an element but no `children[j]` step
(`core/src/literal_edit.rs`), so structural edits inside a `<group>` rewrite
the whole list. That loses the comments and hand formatting between tags.
Related:

- `StaticValue::to_source` pads records as `{ x: 1 }`, unlike the files.
- A `//` line inside element children becomes a text child.

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
