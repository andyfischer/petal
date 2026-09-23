# What the testbed apps still trip on

Status: **open items only**, 2026-09-22. Collated from every testbed debrief
(`.temp/testbed-debriefs/`, `.temp/node-editor-takeaways.md`,
`.temp/markdown-editor-takeaways.md`) and the "Known limits" sections of
the apps added since (flappy, command palette, solar system). Items that
landed were removed. See `git log 6136b14 -- docs/tasks/testbed-takeaways.md`
for the full list and the commits that fixed them.

Ranked by the same rule as before: silent failures in the verify loop
first, then anything more than one app hit.

## 1. `slice`/`len` are byte-indexed

`slice("Óscar", 0, 1)` is `""` and `len("Óscar")` is 6 (checked at HEAD).
The char-aware builtins exist (`chars`, `char_len`, `char_at`,
`char_slice`), but the obvious names give silently wrong results: this is
the CRM app's wrong-initials bug.

**Direction: code points, as in Python 3.** Most scripting languages
(Python, Ruby, Perl) make `len`, indexing and slicing count code points.
JS, Java and C# count UTF-16 units, Swift counts grapheme clusters, and
Rust and Go count bytes but keep you from silently cutting a character
in half. Petal counts bytes *and* lets the cut happen silently, which is
the worst of both. Plan:

- `len`, `slice` and string `s[i]` count code points, which makes them
  agree with the `char_*` builtins. `index_of` and any other offset-returning
  builtin switch units in the same change, so offsets stay composable.
- The byte versions stay available under explicit names (`byte_len`,
  `byte_slice`) for parsers and wire formats.
- Speed: strings are stored as UTF-8, so code-point indexing needs the
  per-string ASCII flag from
  [char-at-fast-path](optimizations/char-at-fast-path.md). The common case
  stays O(1), which gets most of what PEP 393 gives CPython.
- Grapheme clusters are out of scope for `len`. They belong in a caret
  helper for `text_field` (#2).
- Before switching: sweep the `.ptl` files in the examples, `~/garden`,
  `~/.garden` and `~/worlds-fair/ui/ptl` for code that relies on byte
  offsets, and add differential tests over non-ASCII input.

**M.**

## 2. `text_field` has no selection, clipboard or undo

`petal-ui/prelude/ui.ptl:2367`. Bloom's `text_field` doesn't have them
either. Every text-editing app hand-rolls its own field. The command
palette's query box also has no caret movement at all (the caret is always
at the end). **M.**

## 3. Draw coordinates are `i32` throughout the command format

`coord_to_i32` (`petal-ui/src/draw.rs:1546`) and every primitive's fields.
Rotating polygons jitter by up to a pixel per vertex (asteroids). Fixing it
changes the draw-command format and every host (Garden, petal-sdl,
petal-web, worlds-fair). **L.**

## Small gaps

Each is **S**, and each was hit by only one app:

- **No wall-clock → calendar builtin.** The command palette's "insert
  date" inserts a fixed string.
- **Text can't be rotated.** Solar-system labels can't follow an orbit.
  Low value on its own. Worth doing if item 3 reworks the command format
  anyway.
- **Stale binary vs `check`.** The command palette was built against a
  Garden that predated `text_advance` and `range(a, b, step)`, while
  `petal check --host garden` accepted both. `d5652ca` added the startup
  warning and `/state.identity.freshness`. Either the agent didn't see
  them, or they didn't fire. Check which, and consider having `launch.sh`
  rebuild when the binary is behind.

## Follow-ups outside this repo

- worlds-fair: `tools/check_petal_ui.ts` must pass `--host garden` for
  Garden bundles and `--native wf_action,wf_goto,wf_model,wf_fragments` for
  engine bundles once it re-copies Petal, or `check --strict` fails. It
  still doesn't (checked 2026-09-22).
- Scripts for custom hosts (petal-fps, fantasy-nes, petal-web-html) need
  `--native` for their own natives. Garden config scripts (`init.ptl`,
  `layout.ptl`) are checked with `--host garden-config`.
- `~/.cargo/bin/petal` isn't kept fresh. Re-run
  `cargo install --path rust --locked` after language changes.
