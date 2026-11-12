# petal-images follow-up: things to fix in Petal

Status: 2026-10-23, re-checked 2026-11-11. Items 5 and 6 (done
2026-11-09), the finished half of item 2 (`preserve_order`, 2026-11-03), and
items 1 and 3 (done 2026-11-11: element-child edit paths,
[edit-paths.md](../features/edit-paths.md), and unpadded `{x: 1}` records from
`to_source`) have been removed; everything below is open. petal-images can now
delete `Doc::edit_nested` and its whole-`layers` rewrite: use
`layers[i]/*[j].attr` paths, `should_insert` / `should_remove` at
`layers[i]/*[j]`, and `should_move`. These came out of building
[`petal-creative/petal-images`](../../../petal-creative/petal-images), a layered
image editor whose documents are `.ptl` scripts of JSX-style elements
(`<image>`, `<text>`, `<group>`…) edited by the GUI through goal-based literal
edits. The original notes are in `petal-images/docs/petal-notes.md`. Nothing
in `~/petal` was changed. They're listed by priority.

## 2. Values reach the host through JSON

Key order is kept now (`preserve_order`, 2026-11-03,
`core/tests/all/json_key_order.rs`), so petal-images can drop its re-sort and
apply effects in written order. What is left:

- [ ] Add `value_to_static`, which converts a Petal value straight to a
      `StaticValue` with order kept, so hosts that write values back to
      source skip the JSON detour.

## 4. AST span offsets are characters, not bytes, and this isn't documented

The lexer works on a `Vec<char>`, so `SourcePosition::offset` counts `char`s.
A host that slices the `&str` source by it gets the wrong text, or a panic,
after any non-ASCII character. Marketing copy is full of them: `café`, `→`,
`·`, `—`. petal-images converts the offsets with `char_indices`.

Action:
- [ ] Say on `SourcePosition` (and in the embedding docs) that offsets count
      characters.
- [ ] Consider a `byte_offset` alongside it, or a helper
      `span_to_byte_range(src, span)`. This ties in with the code-point
      switch ([String offsets](../Builtins.md#string-offsets)).

## 7. A stale installed CLI is hard to spot

petal-images hit a keyed-loop parse error that turned out to be a stale
`~/.cargo/bin/petal`. `petal --version` now prints the commit and build
date (`petal 0.1.0 (1064dcd004 2026-11-02, built 2026-11-02)`).

Action:
- [ ] Let `petal.toml` state a minimum Petal version and have `petal lint`
      warn when the binary is older. Today `[requires] petal = ">=9.0"` is
      read as a host interface named `petal`, and `run` fails with "requires
      the host interface `petal`". Moses asked for the same
      ([moses-debrief.md](moses-debrief.md)).

## 8. JSON encoding is hard to find

petal-images reported "no `json_encode` builtin". In fact `json_stringify`
exists behind `import json` ([Builtins.md](../Builtins.md)). The note came
from guessing the name, so this is about how easy it is to find, not a
missing feature.

Action:
- [ ] When the name being looked up is `json_encode` (or `to_json`,
      `stringify`), have the "unknown function" error suggest
      `json_stringify` and `import json`.
