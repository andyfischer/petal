# Open work, ranked

Status: 2026-10-23, checked against `b0dfccc7`. This is the index of what is
still open across `docs/tasks/`. Every item of the 2026-10-08 list except
item 1 is done, so that list was replaced by this one. Tests cite the old
numbering (items 5 and 9); read it with
`git show b0dfccc7:docs/tasks/todo.md`. Also removed as resolved:
`testbed-takeaways.md`, `doc-bugs.md`, `state-loop-keys.md`,
`optimizations/char-at-fast-path.md` and
`optimizations/script-level-table-lookups.md`.

The ranking puts what blocks a release first, then silent wrong results, then
performance in the live-coding loop, then language growth.

## 1. The install path fails and nothing says 1.0

`https://petal-lang.org/install.sh` still returns 404 (re-checked
2026-10-23). There is no GitHub release (the only tag is `garden-v0.1.0`), and
`core/Cargo.toml` is at `0.1.0`. The installer is the first command in the
README. Cut a release with `release-petal.yml`, deploy the site, and test the
one-liner on a clean machine. The steps are in
[dev/release-checklist.md](../dev/release-checklist.md).

## 2. `memo` initializers that are not pure give wrong answers silently

[memo-purity-check.md](memo-purity-check.md). Nothing checks that a `memo(k)`
initializer reads only its key, so `memo(k) v = k * scale` hands the first
caller's `scale` to every caller. The `state-key-deprecated` auto-fix can
also change behaviour by rewriting `state(id) x = random()` to `memo`. The
fix reuses the static classifier.

## 3. Module system: one hard failure, one silent no-op

[module-system-followup.md](module-system-followup.md). Re-checked
2026-10-23:

- **Issue 1:** a loose file named like a bundled package (`color.ptl` beside
  `import ui`) stops the program with "two different sources are both the
  module 'color'". Moses hit the same limit (it renamed `map.ptl`).
- **Issue 4:** an `import` inside a function or `if` is silently ignored.
- **Lower:** issue 5 (trace frames without file names) and issue 6 (the
  arity-merge warning suggests the wrong module).

## 4. Static-value cache: UI panels and recompile cost

[static-value-optimization.md](static-value-optimization.md) items 1 and 2.
Per-input-class slots are where panels would gain: today they get about 1%.
Classification adds 30–100 ms to every structural edit, which slows the
live-coding loop the cache was built for.

## 5. Language growth that a second large game will hit

[moses-debrief.md](moses-debrief.md), held back by the owner. Each needs a
proposal:

- checked record shapes;
- `Set` and bit operators;
- growable numeric arrays and in-place record tables;
- module state without `get`/`set`;
- tuples.

Related and deferred on 2026-10-09: amortized O(1) `++` and in-place update
in general, which is a matter of better alias checking.

## 6. Interpreter and GC

[moses-debrief.md](moses-debrief.md), "VM":

- reuse of swept slots and a nursery (GC is 12% of a Moses career);
- a two-target `Branch` and register coalescing;
- faster interning, [optimizations/string-intern-hashing.md](optimizations/string-intern-hashing.md):
  the intern table still uses SipHash, and a one-character slice could hit a
  128-entry ASCII cache instead.

## 7. Draw coordinates are `i32` in the command format

`coord_to_i32` (`core-libs/petal-ui/src/draw.rs`). Rotating polygons jitter
by up to a pixel per vertex. A fix changes the draw-command format and every
host (Garden, petal-sdl, petal-web, worlds-fair). Rotated text would be
cheap to add in the same change.

## Smaller

- **Measurement and text:**
  - kerned measurement in petal-ui: a `FontSource` hook for a whole run's
    laid-out width, so petal-sdl can turn kerning back on;
  - four figures in `dev/performance.md` are marked as dated and could be
    re-measured on a quiet machine;
  - recompiling Moses has slowed from 488 ms to about 800 ms, and only part
    of that is the game's growth.
- **Stdlib:** a builtin from the wall clock to a calendar date.
- **Examples:** `examples/productivity/spreadsheet`'s `digit_val` and
  `letter_val` scan with `slice`. A record lookup would cut the
  formula-commit frame, where `slice` is 71% of builtin calls.
- **Corpus:** `test/ui-golden/index.json` keeps hashes for apps that the
  driver skips as unsupported. Some `~/worlds-fair/ui/ptl` files became
  unsupported at `d2899126`, and nobody has looked at why.
- **Outside this repo:** petal-lang.org's downloadable SDK and its examples
  are still on the old WASM build (`pnpm build:sdk`).
