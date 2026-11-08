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

**Done 2026-11-08.** `prelude/ui.ptl` is a facade over `draw`, `hit`, `text`,
`layout` and `widgets` (and `input`). Where it differs from the sketch below:

- A module that declares a `state var` is the only one that can `set` it, so
  the per-run cells and everything that writes them (clipping, layers, hit
  testing, measuring, flow, retained drawing) share the internal `_frame`.
  `ui/hit` is the list of its public names, and `ui/draw` re-exports the clip
  functions. The theme lives in the internal `_style` the same way. Their
  shared helpers (`_get`, `_hot`, …) are `pub` but `_`-prefixed and not
  re-exported; `tests/host_natives.rs` holds `import ui` to exactly the
  public names of every module.
- The public natives, geometry and timing sit in the internal `_base`, below
  `_style` and `_frame`, and `ui/draw` re-exports all of it. `_anim` moved
  from motion into `ui/widgets`, its only user.
- Widgets stay one module: splitting them further would need another
  internal module for the helpers they share.
- Module-qualified names follow the code (`ui/_frame::_floor`,
  `ui/widgets::button` in state maps and `Env::call_function`).
- The export set is unchanged (`import ui` names, Garden's `name/arity`
  list, `docs/stdlib.json`), and `petal lint` over `examples/` and the
  cheesecake games that import `ui` prints the same before and after.

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

*Done 2026-11-08:* all eleven candidates (`static_*`, `gating`, `memo`,
`effect_audit`, `dm_params`, `dm_spec`, `dm_study`) and `tests/common`, whose
only users they were, are in `test/ui-corpus/` (`make test-ui-corpus`); see
`test/README.md`. Each file tests a core module (`petal::static_cache` and its
classifier/audit, the gate in `petal::run_deps`, `petal::memo`,
`petal::effect_audit`, `petal::dm`) and uses `petal-ui` only as the host.

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
   - **Benchmark (2026-11-08).** Harness:
     `core-libs/petal-ui/examples/bench_text_wrap.rs` (`cargo run --release
     --example bench_text_wrap` in `core-libs/petal-ui`); Petal side
     `test/benchmarks/text_wrap/wrap_ref.ptl`.
     - **The premise is wrong.** `text-layout` has no Petal wrap, elide or
       caret code: `wrap_text`, `elide`, `clamp_lines` and `caret_index` call
       `text_wrap`, `text_ellipsize` and `text_index_at` (it has done since
       05e4b4ec). So bloom's labels say nothing about Petal's speed, and
       `ui/text` and `text-layout` already share one implementation, the
       native one. To measure anything, the benchmark adds a bench-only Petal
       `wrap_rows` (`ref_wrap_rows`). It follows `wrap_lines` + `wrap_rows`
       closely and makes one `text_advance` call per word and per run of
       blanks. Ellipsize and index-at were not measured: they have no Petal
       version to compare against.
     - **Equivalence.** The rows (`text`, `start`, `end`) are identical in
       880 cases: 20 documents (edge cases, CRLF, tabs, non-ASCII, generated
       prose with long URLs) × 4 styles (including letter spacing and a bare
       size) × 11 widths (−5 to 2000 px). Two native quirks that a Petal
       specification must keep: a paragraph's leading blanks make an over-wide
       first word overflow instead of breaking, and a tab is measured as one
       glyph advance. Measuring whole words with `text_advance` instead of
       summing per character changed no break.
     - **Per-frame ms** (p50, best of 5 × 60 frames, release, gate off,
       pointer moving, 548 px wrap, proportional advance table). The machine
       was heavily loaded (load average ~97), so treat the absolute numbers
       as upper bounds; the ratios held across runs.

       | chars (rows) | case | `text_area` (native) | wrap native | wrap Petal |
       |---|---|---|---|---|
       | 1k (15) | still, memo | 0.11 | 0.07 | 0.17 |
       | 1k (15) | no memo / typing | 0.10 | 0.07 | 0.29–0.47 (4–7×) |
       | 10k (153) | still, memo | 0.57 | 0.28 | 8.2 (30×) |
       | 10k (153) | no memo / typing | 0.21–0.24 | 0.16–0.21 | 11–14 (55–85×) |
       | 100k (1500) | still, memo | 13.3 | 2.9 | 166 (58×) |
       | 100k (1500) | no memo / typing | 1.6–1.9 | 1.4–1.6 | 95–101 (60–70×) |

       A `text_area` on the Petal wrap would cost about `area − wrap native +
       wrap Petal`: around 11 ms a frame at 10k chars and around 100 ms at
       100k, against 0.2 ms and 1.8 ms today.
     - **The memo does not rescue it.** A Petal wrap makes one probe per
       `text_advance` call. Past `MAX_SCOPE_DEPS` (8,192) the scope is given
       up as effectful: 1,380 effectful scopes at 10k, 19,500 at 100k, and no
       replays. Recording costs extra on top (166 vs 101 ms). A side finding,
       not part of this task: memo on makes the *native* path slower too. The
       `text_wrap` probe's answer (more than ~128 rows) is larger than
       `ARG_COMPARE_BUDGET` (512), so it fails validation every frame, misses,
       and wraps again. That is why `text_area` at 100k is 13.3 ms with memo
       on and 1.8 ms with it off. Worth its own fix in the memo.
     - **Recommendation: keep `text_wrap` / `wrap_rows` as the native fast
       path.** Don't merge into one Petal implementation. Petal is fine for a
       label or a short field (under 0.5 ms at 1k chars). But wrapping a
       document every frame is 30–85× slower, which uses up the frame budget
       at 10k chars. Follow the fallback bullet above: promote
       `wrap_ref.ptl` to the specification (at that point it is a port step:
       move it into `text-layout` or `ui/text` as `wrap_rows_reference`) and
       turn the equivalence pass into a test that runs the native and Petal
       versions against each other. Treat `text_ellipsize` / `text_index_at`
       the same way if they are ever specified in Petal. They do one walk over
       one line, so the stakes are smaller.
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
