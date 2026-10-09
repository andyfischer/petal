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

**Landed 2026-10-09** (the number is kept because other files cite §2 and §3).
`len`, `slice`, `s[i]` and `index_of` count code points, as in Python 3;
`byte_len`/`byte_slice` keep the byte unit; ASCII strings stay O(1). Grapheme
clusters remain out of scope and belong in a caret helper for `text_field`
(#2). What each downstream project has to check is in
[code-point-migration.md](code-point-migration.md).

## 2. `text_field` has no selection, clipboard or undo

`core-libs/petal-ui/prelude/ui.ptl:2264`. Bloom's `text_field` doesn't have them
either. Every text-editing app hand-rolls its own field. The command
palette's query box also has no caret movement at all (the caret is always
at the end). **M.**

## 3. Draw coordinates are `i32` throughout the command format

`coord_to_i32` (`core-libs/petal-ui/src/draw.rs:1550`) and every primitive's fields.
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

## Follow-ups outside this repo

- worlds-fair: `tools/check_petal_ui.ts` must pass `--host garden` for
  Garden bundles and `--native wf_action,wf_goto,wf_model,wf_fragments` for
  engine bundles once it re-copies Petal, or `check --strict` fails. It
  still doesn't (checked 2026-10-08).
- Scripts for custom hosts (petal-fps, fantasy-nes, petal-web-html) need
  `--native` for their own natives. Garden config scripts (`init.ptl`,
  `layout.ptl`) are checked with `--host garden-config`.
- `~/.cargo/bin/petal` isn't kept fresh. Re-run
  `cargo install --path core --locked` after language changes.
