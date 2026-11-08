# Reorganizing petal-ui

Status: 2026-11-08. A plan, nothing done yet. It comes from a review of
`core-libs/` against [docs/skills/project-organization.md](../skills/project-organization.md).
Check the current tree before acting on any item; the numbers below are from
that date.

## Summary

`gpp`, `petal-query` and `petal-pkgman` are fine. `petal-ui` is the problem,
and the problem is less its size than that it holds three different things,
plus a lot of code that isn't UI:

1. the host contract every host links,
2. headless tooling,
3. tests of core features that live here only because they need a corpus of
   UI apps.

Its `ui` package is also one 5,635-line file, half primitives and half
widgets, so almost every UI change goes through it.

## The crates today

| Crate | Size | File changes since 2026-09-01 | Verdict |
|---|---|---|---|
| `gpp` | 1.1k Rust | 4 | Fine. It's a wire protocol that more than one program has to agree on (R5). |
| `petal-query` | 1.8k Rust | 10 | Fine. It's the client half of `gpp`, used by Garden's built-in apps and optionally by the C bridge. |
| `petal-pkgman` | 2k Rust | 4 | Correctly placed, but the definition is off. `core-libs/` is described as "crates hosts link to provide a shared host interface", and the package manager is not that. Fix the definition in the skill rather than moving the crate. |
| **`petal-ui`** | **13.4k Rust + 7.1k Petal + 14.3k tests** | **272** (73 commits since 2026-10-01; `prelude/ui.ptl` alone was touched in 20 of them) | Split along the lines below. |

## What's inside petal-ui

**1. The host contract.** This is what every host links: `input`, `draw` with
`text` metrics, `hit` / `focus` / `popover`, `clipboard`, `host_data`,
`compose`, `tess`, `draw_wire`, `params`, and `frame_core`. This part belongs
in `core-libs/`.

**2. Headless tooling.** That's `harness`, `scenario`, `drag_trace`,
`panel_stubs`, and the `petal-ui-run` and `petal-ui-propose-edit` binaries.
Inside this repo only `garden/garden-script` and
`integrations/petal-c-bridge` use them. Downstream they matter a lot:
cheesecake uses `petal_ui::harness` about 40 times, `scenario` 28 and
`panel_stubs` 22; worlds-fair uses `scenario`, `panel_stubs` and `harness`
too. `panel_stubs` is also a layering problem: it is a copy of the host
natives Garden registers, and of Garden's `FALLBACK_PALETTE`, sitting in a
crate below Garden.

**3. Tests of core features.** `tests/static_*` (5 files), `gating`, `memo`,
`effect_audit` and `dm_*`, about 4–5k lines. They test
`petal::static_cache`, the frame gate, memo scopes and direct manipulation.
Core can't name a host, so they ended up here, but now half of `petal-ui`'s
test suite tests core.

**The `ui` package.** `prelude/ui.ptl` is a single 5,635-line file.

- Lines 1–2470 are primitives: the native declarations, record overloads over
  the draw natives, clipping, layers and hit testing, measuring, flow layout,
  retained drawing, gradients, scrims, shadows, elevation, offscreen canvases,
  fonts, text helpers, colour, compositing and style.
- Lines 2471–5635 are about 3,100 lines of widgets: button, list, scroll
  region, focus, text field, text area, context menu, popover, drag and drop,
  RectCut layout, motion, spinner, checkbox / toggle / radio / slider,
  inspector fields, tab bar, tooltip, modal, badge / card / hint bar,
  splitter, table, pixel-budget wrap and load state.

### Overlaps

- **Two bundled widget sets.** The `ui` prelude has its own widgets, and
  `core-runtime/bloom` (2.4k lines) is described as "the default widget set",
  built partly on `ui`'s own `button`, `checkbox` and `slider`.
- **Text layout exists twice.** `core-runtime/text-layout` does wrapping,
  elision and caret hit-testing in Petal. `src/text.rs` does the same in Rust
  as `text_wrap`, `text_ellipsize` and `text_index_at`.

### Who imports the `ui` package

Counts of `.ptl` files, 2026-11-08:

| Project | `import ui` | `import ui/drag` | `import bloom` | `text-layout` |
|---|---|---|---|---|
| `~/cheesecake/games` | 110 | 0 | 0 | 0 |
| `examples/` | 94 | 7 | 4 | 17 |
| `garden/` | 31 | 0 | 4 | 0 |
| `~/petal-creative` | 15 | 0 | 0 | 0 |
| `~/worlds-fair/ui/ptl` | 2 | 0 | 0 | 0 |

## Refactors, in order

Each one goes in its own commit, as the skill asks for moves.

### R1. Split `ui.ptl` into modules behind an unchanged `ui` facade

Low risk, and the biggest payoff. The mechanism already exists: `ui.ptl`
does `pub import ./input`. Make `ui.ptl` a short file of `pub import`s over:

- `ui/draw`: the record overloads, clip, gradients, shadows, elevation,
  canvases, fonts, colour and compositing
- `ui/hit`: layers, `interact`, targets, measuring, flow layout, retained
  drawing
- `ui/text`: text helpers, ranges, clipboard
- `ui/layout`: RectCut, motion, pixel-budget word wrap
- `ui/widgets`: everything from button onward, possibly split further into
  fields, overlays and data views

No script changes, because `import ui` still exposes everything (`ui/drag`
already works this way). What has to change:

- `params::interface()` reads declarations from `input.ptl` and `ui.ptl`
  only. It and `package_modules()` (`src/lib.rs`) must collect them from
  every file.
- The private `_native_*` declarations need one internal module that the
  others import.
- `core/build.rs`, and the export list `garden --version` derives from
  `prelude_source()`, follow the new files.
- Bump `PRELUDE_LEVEL` only if a symbol is added; a pure split adds none.
- Check: the golden suites, `petal lint` over the examples, and the 110
  cheesecake game files that import `ui`.

### R2. Move the headless tooling into `core-libs/petal-ui-harness`

That covers `harness`, `scenario`, `drag_trace`, `panel_stubs` and both
binaries. Hosts then stop pulling in tooling they don't use. `frame_core`
stays in `petal-ui`, since Garden runs on it.

- Cheesecake and worlds-fair break at their next re-vendor. Keep `pub use`
  re-exports in `petal-ui` behind a default-on feature for one cycle, and
  change the downstream imports in the same pass.
- Decide where `panel_stubs` lives. The clean option is for Garden to export
  its own stubs. Cheesecake's heavy use of them suggests they've turned into a
  general "fake panel host", and in that case they should be documented as
  one.

### R3. Move the core-feature tests to a test crate

For example `test/ui-corpus/`, depending on `petal` and `petal-ui`. Then
`petal-ui`'s tests are about `petal-ui`, and the static-cache, gate and memo
work stops counting as churn in the UI crate. This changes no APIs; it's
mostly moving files. The test helpers in `tests/common` may need to be
shared.

### R4. Decide whether `ui` or `bloom` owns the widgets

This is a design decision, not a move, and it needs a decision from the
owner. If `ui` ends at primitives plus interaction and the widgets come from
one library, the bundled API gets much smaller. But cheesecake, petal-creative
and 94 examples use `ui`'s widgets today, so it would be a migration with a
deprecation period. After R1, `ui/widgets` becomes a separate unit, which makes
either choice cheaper. Decide this before adding many more widgets to `ui`.

## Moving native code into Petal

Most of the Rust has to stay native. The prelude is already the Petal half,
and it is large.

**Stays native:**

- Host-side code: `compose`, `tess`, `draw_wire`, `scenario`, `frame_core` and
  the `draw` decoding. These run in the host, not in a script, and Rust hosts
  call them directly.
- Natives over data only the host has: `text_width`, `text_advance` and
  `text_metrics` (they need font tables), `host_data`, images and canvases.
- Thin wrappers over host state: the input polling natives, the draw-command
  natives, `clipboard`.

**Candidates:**

1. **The text-layout algorithms in `src/text.rs`: `wrap_rows`,
   `text_ellipsize`, `text_index_at`.** These are about 1,000 lines of logic
   sitting on top of `text_advance`. `core-runtime/text-layout` already does
   the same jobs in Petal, and bloom draws every label through it, so Petal is
   evidently fast enough there. Merge the two: keep the native measuring
   primitives and make `ui/text` and `text-layout` share one Petal
   implementation.
   - Benchmark first. A `text_area` wrapping a long document every frame is
     the case to measure, with memo on and with memo off.
   - If Petal is too slow there, keep `text_wrap` as a native fast path, but
     make the Petal version the specification and test the two against each
     other.
2. **`hit::roll`, `focus` and `popover` (about 1,500 lines). Possible, but
   hold off.** These decisions run before the script each frame, and they're
   in Rust precisely so that no script can skip them. Porting them would need
   a host-run prologue in Petal (`ui._roll()`) and a check that the frame gate
   and memo still behave the same. The code is small and stable, so the gain
   is small.

## Smaller cleanups

- The `PRELUDE_LEVEL` doc comment in `src/lib.rs` is about 100 lines of
  changelog. It would read better in `docs/reference.md` or a changelog, with
  the constant pointing there.
- Once R1–R3 land, update the "Current misplacements" list in
  `docs/skills/project-organization.md` and the `core-libs/` definition in
  its layer table (see `petal-pkgman` above).
